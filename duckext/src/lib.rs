//! anofox-visualization — a DuckDB C-API extension: `SELECT anofox_render()` →
//! an SVG rendered by ggplot-rs (via the anofox-visualization core). Uses the C Extension
//! API — the DuckDB
//! functions are resolved at load time through the `access` struct, so nothing
//! links against libduckdb. That's what lets the same crate become a DuckDB-Wasm
//! side-module.
#![allow(non_upper_case_globals, non_camel_case_types, non_snake_case, dead_code)]

pub(crate) mod ffi {
    include!(concat!(env!("OUT_DIR"), "/bindings.rs"));
}
use ffi::*;
use std::ffi::CString;
use std::ptr;

// Live-session HTTP server for `SELECT anofox_serve(port)` (native only).
#[cfg(not(target_arch = "wasm32"))]
mod serve;

// Rust's std links a few libc file-I/O imports (`pread`/`pwrite`) whose
// emscripten signatures don't match DuckDB-Wasm's host module, and they're never
// called in the render path. Defining them locally stops the side-module from
// importing them (resolved internally instead of from `env`).
#[cfg(target_arch = "wasm32")]
mod libc_stubs {
    use core::ffi::c_void;
    #[no_mangle]
    pub unsafe extern "C" fn pread(_fd: i32, _buf: *mut c_void, _n: usize, _off: i64) -> isize {
        -1
    }
    #[no_mangle]
    pub unsafe extern "C" fn pwrite(_fd: i32, _buf: *const c_void, _n: usize, _off: i64) -> isize {
        -1
    }
    #[no_mangle]
    pub unsafe extern "C" fn preadv(_fd: i32, _iov: *const c_void, _c: i32, _off: i64) -> isize {
        -1
    }
    #[no_mangle]
    pub unsafe extern "C" fn pwritev(_fd: i32, _iov: *const c_void, _c: i32, _off: i64) -> isize {
        -1
    }
    #[no_mangle]
    pub unsafe extern "C" fn ftruncate(_fd: i32, _len: i64) -> i32 {
        -1
    }
    // NOTE: with `lseek` defined the module links AND instantiates, but hits a
    // runtime `function signature mismatch` — an emscripten i64-legalization
    // difference at an indirect call. This is the final ABI gap; see BUILD.md.
    #[no_mangle]
    pub unsafe extern "C" fn lseek(_fd: i32, _off: i64, _whence: i32) -> i64 {
        -1
    }
}

/// The DuckDB API table — **copied by value** at load time (the pointer from
/// `get_api` is only valid during init, so we own a copy, like the C macro's
/// `duckdb_ext_api = *res`).
static mut API: Option<duckdb_ext_api_v1> = None;
pub(crate) unsafe fn api() -> &'static duckdb_ext_api_v1 {
    #[allow(static_mut_refs)]
    API.as_ref().unwrap()
}

/// `SELECT anofox_serve(port)` → start the browser builder on the live session.
#[cfg(not(target_arch = "wasm32"))]
unsafe extern "C" fn anofox_serve_fn(
    _info: duckdb_function_info,
    input: duckdb_data_chunk,
    output: duckdb_vector,
) {
    let n = (api().duckdb_data_chunk_get_size.unwrap())(input);
    let vec = (api().duckdb_data_chunk_get_vector.unwrap())(input, 0);
    let data = (api().duckdb_vector_get_data.unwrap())(vec) as *const i32;
    let port = if data.is_null() || n == 0 { 8080 } else { (*data).max(0) as u16 };
    let port = if port == 0 { 8080 } else { port };
    let msg = CString::new(serve::start(port)).unwrap_or_default();
    for i in 0..n {
        (api().duckdb_vector_assign_string_element.unwrap())(output, i, msg.as_ptr());
    }
}

fn smoke_svg() -> CString {
    use anofox_visualization::{render, Column, Kind, Role};
    use ggplot_rs::prelude::Value;
    let cols = vec![
        Column::new(
            "x",
            Role::X,
            vec![Value::Str("a".into()), Value::Str("b".into()), Value::Str("c".into())],
        ),
        Column::new(
            "n",
            Role::Value(Kind::Bar),
            vec![Value::Float(3.0), Value::Float(7.0), Value::Float(5.0)],
        ),
    ];
    CString::new(render(&cols, 320, 220).unwrap_or_default()).unwrap_or_default()
}

/// Decode a `duckdb_string_t` at index `i` (16 bytes: u32 length, then the inline
/// bytes when length ≤ 12, else a 4-byte prefix + an 8-byte data pointer).
unsafe fn read_string_t(base: *const u8, i: idx_t) -> &'static [u8] {
    let s = base.add(i as usize * 16);
    let len = *(s as *const u32) as usize;
    let data = if len <= 12 {
        s.add(4)
    } else {
        *(s.add(8) as *const *const u8)
    };
    std::slice::from_raw_parts(data, len)
}

/// `SELECT anofox_render(spec)` → an SVG per row. `spec` is the JSON panel spec
/// (rows + role annotations + size), the same shape the browser renderer takes.
unsafe extern "C" fn anofox_render_fn(
    _info: duckdb_function_info,
    input: duckdb_data_chunk,
    output: duckdb_vector,
) {
    let n = (api().duckdb_data_chunk_get_size.unwrap())(input);
    let vec = (api().duckdb_data_chunk_get_vector.unwrap())(input, 0);
    let data = (api().duckdb_vector_get_data.unwrap())(vec) as *const u8;
    let validity = (api().duckdb_vector_get_validity.unwrap())(vec);
    for i in 0..n {
        let valid = validity.is_null() || {
            let word = *validity.add((i / 64) as usize);
            (word >> (i % 64)) & 1 == 1
        };
        let svg = if valid && !data.is_null() {
            let bytes = read_string_t(data, i);
            anofox_visualization::render_spec(std::str::from_utf8(bytes).unwrap_or(""))
        } else {
            String::new()
        };
        let c = CString::new(svg).unwrap_or_default();
        (api().duckdb_vector_assign_string_element.unwrap())(output, i, c.as_ptr());
    }
}

/// Convenience SQL macros bundled with the extension, so callers don't hand-write
/// the JSON spec. Each wraps `anofox_render(json_object(...))`; the `list()` makes
/// them aggregates, so `SELECT anofox_bar(x, y) FROM t` collects the rows and
/// returns one SVG. `anofox_xy` / `anofox_xyc` take an explicit `kind` (any chart
/// role, e.g. 'LINECHART', 'SMOOTH', 'VIOLIN'); the named ones are shorthands.
const MACROS: &[&str] = &[
    r#"CREATE OR REPLACE MACRO anofox_xy(x, y, kind := 'BARCHART', width := 640, height := 400) AS
         anofox_render(json_object('rows', to_json(list({c0: x, c1: y})),
           'roles', ('[[0,"XAXIS"],[1,"' || kind || '"]]')::JSON,
           'width', width, 'height', height))"#,
    r#"CREATE OR REPLACE MACRO anofox_xyc(x, y, series, kind := 'BARCHART_STACKED', width := 640, height := 400) AS
         anofox_render(json_object('rows', to_json(list({c0: x, c1: y, c2: series})),
           'roles', ('[[0,"XAXIS"],[1,"' || kind || '"],[2,"CATEGORY"]]')::JSON,
           'width', width, 'height', height))"#,
    "CREATE OR REPLACE MACRO anofox_bar(x, y) AS anofox_xy(x, y, kind := 'BARCHART')",
    "CREATE OR REPLACE MACRO anofox_line(x, y) AS anofox_xy(x, y, kind := 'LINECHART')",
    "CREATE OR REPLACE MACRO anofox_scatter(x, y) AS anofox_xy(x, y, kind := 'SCATTER')",
    "CREATE OR REPLACE MACRO anofox_area(x, y) AS anofox_xy(x, y, kind := 'AREACHART')",
];

/// Run a SQL statement on `conn` for its side effect (macro registration).
unsafe fn run_sql(conn: duckdb_connection, sql: &str) -> bool {
    let Ok(c) = CString::new(sql) else { return false };
    let mut res: duckdb_result = std::mem::zeroed();
    let ok = (api().duckdb_query.unwrap())(conn, c.as_ptr(), &mut res) == duckdb_state::DuckDBSuccess;
    (api().duckdb_destroy_result.unwrap())(&mut res);
    ok
}

/// DuckDB calls `<extension_name>_init_c_api` on LOAD.
#[no_mangle]
pub unsafe extern "C" fn anofox_visualization_init_c_api(
    info: duckdb_extension_info,
    access: *const duckdb_extension_access,
) -> bool {
    let access = &*access;
    let Some(get_api) = access.get_api else { return false };
    let api_ptr = get_api(info, c"v1.2.0".as_ptr()) as *const duckdb_ext_api_v1;
    if api_ptr.is_null() {
        return false;
    }
    API = Some(*api_ptr); // own a copy — the pointer is only valid during init

    let Some(get_db) = access.get_database else { return false };
    let db = *get_db(info); // get_database returns *mut duckdb_database
    let mut conn: duckdb_connection = ptr::null_mut();
    if (api().duckdb_connect.unwrap())(db, &mut conn) != duckdb_state::DuckDBSuccess {
        return false;
    }

    // anofox_render(VARCHAR spec) -> VARCHAR (SVG)
    let f = (api().duckdb_create_scalar_function.unwrap())();
    (api().duckdb_scalar_function_set_name.unwrap())(f, c"anofox_render".as_ptr());
    let mut ptype = (api().duckdb_create_logical_type.unwrap())(duckdb_type::DUCKDB_TYPE_VARCHAR);
    (api().duckdb_scalar_function_add_parameter.unwrap())(f, ptype);
    let mut vtype = (api().duckdb_create_logical_type.unwrap())(duckdb_type::DUCKDB_TYPE_VARCHAR);
    (api().duckdb_scalar_function_set_return_type.unwrap())(f, vtype);
    (api().duckdb_scalar_function_set_function.unwrap())(f, Some(anofox_render_fn));
    let mut ok = (api().duckdb_register_scalar_function.unwrap())(conn, f) == duckdb_state::DuckDBSuccess;
    (api().duckdb_destroy_logical_type.unwrap())(&mut vtype);
    (api().duckdb_destroy_logical_type.unwrap())(&mut ptype);
    let mut fm = f;
    (api().duckdb_destroy_scalar_function.unwrap())(&mut fm);

    // Bundled convenience macros (anofox_bar/_line/_scatter/_xy/_xyc). Best-effort
    // — they need the json functions at call time, but defining them can't fail
    // the load.
    for m in MACROS {
        run_sql(conn, m);
    }

    // anofox_serve(INTEGER port) -> VARCHAR  (native only)
    #[cfg(not(target_arch = "wasm32"))]
    {
        let sf = (api().duckdb_create_scalar_function.unwrap())();
        (api().duckdb_scalar_function_set_name.unwrap())(sf, c"anofox_serve".as_ptr());
        let mut itype =
            (api().duckdb_create_logical_type.unwrap())(duckdb_type::DUCKDB_TYPE_INTEGER);
        (api().duckdb_scalar_function_add_parameter.unwrap())(sf, itype);
        let mut rtype =
            (api().duckdb_create_logical_type.unwrap())(duckdb_type::DUCKDB_TYPE_VARCHAR);
        (api().duckdb_scalar_function_set_return_type.unwrap())(sf, rtype);
        (api().duckdb_scalar_function_set_function.unwrap())(sf, Some(anofox_serve_fn));
        ok = ok
            && (api().duckdb_register_scalar_function.unwrap())(conn, sf)
                == duckdb_state::DuckDBSuccess;
        (api().duckdb_destroy_logical_type.unwrap())(&mut itype);
        (api().duckdb_destroy_logical_type.unwrap())(&mut rtype);
        let mut sfm = sf;
        (api().duckdb_destroy_scalar_function.unwrap())(&mut sfm);
    }

    // Native: keep this connection alive for the /query bridge (reused serially
    // by the server thread). Wasm: no server — release it.
    #[cfg(not(target_arch = "wasm32"))]
    serve::set_conn(conn);
    #[cfg(target_arch = "wasm32")]
    (api().duckdb_disconnect.unwrap())(&mut conn);
    ok
}
