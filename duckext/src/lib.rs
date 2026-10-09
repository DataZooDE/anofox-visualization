//! anofox-visualization — a DuckDB C-API extension: `SELECT anofox_render()` →
//! an SVG rendered by ggplot-rs (via the anofox-visualization core), plus the
//! serving functions (`anofox_serve`, `anofox_serve_dashboards`,
//! `anofox_serve_stop`; native only). Uses the C Extension API — the DuckDB
//! functions are resolved at load time through the `access` struct, so nothing
//! links against libduckdb. That's what lets the same crate become a
//! DuckDB-Wasm side-module.
//!
//! Every `extern "C"` callback runs inside `catch_unwind`; failures surface as
//! real SQL errors via `duckdb_scalar_function_set_error`.
#![allow(
    non_upper_case_globals,
    non_camel_case_types,
    non_snake_case,
    dead_code
)]

#[allow(clippy::all, unnecessary_transmutes)]
pub(crate) mod ffi {
    include!(concat!(env!("OUT_DIR"), "/bindings.rs"));
}
mod db;
use db::capi;
use ffi::*;
use std::ffi::{CStr, CString};
use std::os::raw::c_void;
use std::ptr;
use std::sync::OnceLock;

// Live-session HTTP servers (native only).
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

/// The DuckDB API table — **copied by value** at the first load (the pointer
/// from `get_api` is only valid during init). Every database that loads the
/// extension in this process gets the same table from the same library.
static API: OnceLock<duckdb_ext_api_v1> = OnceLock::new();
pub(crate) fn api() -> &'static duckdb_ext_api_v1 {
    API.get()
        .expect("anofox_visualization: C API not initialised")
}

/// Report `msg` as the SQL error of this function call.
unsafe fn set_error(info: duckdb_function_info, msg: &str) {
    let c = CString::new(msg.replace('\0', "")).unwrap_or_default();
    capi!(duckdb_scalar_function_set_error)(info, c.as_ptr());
}

/// Run a scalar-function body, turning `Err` and panics into a SQL error.
unsafe fn guarded(info: duckdb_function_info, f: impl FnOnce() -> Result<(), String>) {
    if let Err(e) = anofox_visualization::host::catch_panic(f) {
        set_error(info, &e);
    }
}

/// Decode a `duckdb_string_t` at index `i` of a flat VARCHAR vector.
unsafe fn read_string_at<'a>(data: *const duckdb_string_t, i: idx_t) -> &'a [u8] {
    db::read_string(&*data.add(i as usize))
}

unsafe fn set_null(output: duckdb_vector, i: idx_t) {
    capi!(duckdb_vector_ensure_validity_writable)(output);
    let v = capi!(duckdb_vector_get_validity)(output);
    capi!(duckdb_validity_set_row_invalid)(v, i);
}

unsafe fn assign_str(output: duckdb_vector, i: idx_t, s: &str) -> Result<(), String> {
    let c = CString::new(s).map_err(|_| "result contains a NUL byte".to_string())?;
    capi!(duckdb_vector_assign_string_element)(output, i, c.as_ptr());
    Ok(())
}

/// Column `col` of the input chunk as `(data, validity)`.
unsafe fn input_col(input: duckdb_data_chunk, col: idx_t) -> (*mut c_void, *mut u64) {
    let v = capi!(duckdb_data_chunk_get_vector)(input, col);
    (
        capi!(duckdb_vector_get_data)(v),
        capi!(duckdb_vector_get_validity)(v),
    )
}

/// `SELECT anofox_render(spec)` → an SVG per row. `spec` is the JSON panel spec
/// (rows + role annotations + size). NULL → NULL; a bad or oversize spec → SQL
/// error (see `anofox_visualization::host::render_spec_checked`).
unsafe extern "C" fn anofox_render_fn(
    info: duckdb_function_info,
    input: duckdb_data_chunk,
    output: duckdb_vector,
) {
    guarded(info, || {
        let n = capi!(duckdb_data_chunk_get_size)(input);
        let (data, validity) = input_col(input, 0);
        let data = data as *const duckdb_string_t;
        let limits = anofox_visualization::host::RenderLimits::default();
        for i in 0..n {
            if data.is_null() || !db::row_valid(validity, i) {
                set_null(output, i);
                continue;
            }
            let svg = anofox_visualization::host::render_spec_bytes_checked(
                read_string_at(data, i),
                &limits,
            )?;
            assign_str(output, i, &svg)?;
        }
        Ok(())
    })
}

#[cfg(not(target_arch = "wasm32"))]
mod serve_fns {
    use super::*;
    use std::sync::Arc;

    /// The per-database context attached to each serving function.
    pub(super) unsafe fn ctx(info: duckdb_function_info) -> Result<Arc<serve::DbCtx>, String> {
        let p = capi!(duckdb_scalar_function_get_extra_info)(info) as *const Arc<serve::DbCtx>;
        if p.is_null() {
            return Err("anofox_visualization: serving context missing".into());
        }
        Ok((*p).clone())
    }

    pub(super) unsafe extern "C" fn drop_ctx(p: *mut c_void) {
        if !p.is_null() {
            drop(Box::from_raw(p as *mut Arc<serve::DbCtx>));
        }
    }

    unsafe fn port_at(data: *mut c_void, i: idx_t) -> Result<u16, String> {
        let p = *(data as *const i32).add(i as usize);
        u16::try_from(p)
            .ok()
            .filter(|p| *p != 0)
            .ok_or_else(|| format!("port must be between 1 and 65535, got {p}"))
    }

    unsafe fn str_at(data: *mut c_void, i: idx_t) -> Result<String, String> {
        std::str::from_utf8(read_string_at(data as *const duckdb_string_t, i))
            .map(str::to_string)
            .map_err(|_| "argument is not valid UTF-8".to_string())
    }

    /// Run `f` once per row; any NULL argument → NULL result (no side effect).
    unsafe fn per_row(
        input: duckdb_data_chunk,
        output: duckdb_vector,
        ncols: idx_t,
        mut f: impl FnMut(&[*mut c_void], idx_t) -> Result<String, String>,
    ) -> Result<(), String> {
        let n = capi!(duckdb_data_chunk_get_size)(input);
        let cols: Vec<(*mut c_void, *mut u64)> = (0..ncols).map(|c| input_col(input, c)).collect();
        let data: Vec<*mut c_void> = cols.iter().map(|c| c.0).collect();
        for i in 0..n {
            if cols
                .iter()
                .any(|(d, v)| d.is_null() || !db::row_valid(*v, i))
            {
                set_null(output, i);
                continue;
            }
            let msg = f(&data, i)?;
            assign_str(output, i, &msg)?;
        }
        Ok(())
    }

    /// `anofox_serve(port)` → start the authoring builder on the live session.
    pub(super) unsafe extern "C" fn anofox_serve_fn(
        info: duckdb_function_info,
        input: duckdb_data_chunk,
        output: duckdb_vector,
    ) {
        guarded(info, || {
            let ctx = ctx(info)?;
            per_row(input, output, 1, |d, i| {
                serve::start_authoring(ctx.clone(), port_at(d[0], i)?)
            })
        })
    }

    /// `anofox_serve_dashboards(dir, port[, options])` → locked serving.
    pub(super) unsafe extern "C" fn anofox_serve_dashboards_fn(
        info: duckdb_function_info,
        input: duckdb_data_chunk,
        output: duckdb_vector,
    ) {
        guarded(info, || {
            let ctx = ctx(info)?;
            let ncols = capi!(duckdb_data_chunk_get_column_count)(input);
            per_row(input, output, ncols, |d, i| {
                let dir = str_at(d[0], i)?;
                let port = port_at(d[1], i)?;
                let opts = if ncols > 2 {
                    Some(str_at(d[2], i)?)
                } else {
                    None
                };
                let opts = serve::LockedOptions::parse(opts.as_deref())?;
                serve::start_locked(ctx.clone(), &dir, port, opts)
            })
        })
    }

    /// `anofox_serve_stop(port)`.
    pub(super) unsafe extern "C" fn anofox_serve_stop_fn(
        info: duckdb_function_info,
        input: duckdb_data_chunk,
        output: duckdb_vector,
    ) {
        guarded(info, || {
            per_row(input, output, 1, |d, i| serve::stop(port_at(d[0], i)?))
        })
    }
}

/// Convenience SQL macros bundled with the extension, so callers don't hand-write
/// the JSON spec. The table lives in the core (`anofox_visualization::macros`)
/// and is shared with the C++ build, which reads it through the FFI crate — the
/// bodies cannot drift between the builds.
fn macros() -> Vec<(&'static str, String)> {
    anofox_visualization::macros::MACROS
        .iter()
        .map(|m| (m.name, m.create_sql()))
        .collect()
}

/// Define the macros. The C extension API has no way to register *internal*
/// macros (the C++ build does that), so they are created with SQL in the
/// default database. To avoid clobbering a user's own macro of the same name,
/// an existing macro is only replaced when it is one of ours (its body calls
/// anofox_render / anofox_xy). Best-effort: a read-only database simply gets no
/// macros. See BUILD.md ("C-API build: macro caveat").
unsafe fn define_macros(conn: &db::Conn) {
    for (name, body) in macros() {
        let existing = conn
            .execute_prepared(
                "SELECT macro_definition::VARCHAR FROM duckdb_functions() \
                 WHERE function_name = ? AND function_type = 'macro' AND database_name = current_database()",
                &[name],
            )
            .and_then(|mut r| r.string_rows(16));
        let ours = |d: &Option<String>| {
            d.as_deref()
                .is_some_and(|d| d.contains("anofox_render") || d.contains("anofox_xy"))
        };
        match existing {
            Ok(rows) if rows.iter().any(|r| !ours(&r[0])) => continue, // a user macro — leave it
            Ok(_) => {}
            Err(_) => continue,
        }
        let _ = conn.exec(&format!("CREATE OR REPLACE MACRO {name}{body}"));
    }
}

/// Register a VARCHAR-returning scalar function (or overload set).
unsafe fn register_scalar(
    conn: duckdb_connection,
    name: &CStr,
    overloads: &[&[duckdb_type]],
    func: unsafe extern "C" fn(duckdb_function_info, duckdb_data_chunk, duckdb_vector),
    volatile: bool,
    extra: Option<&dyn Fn() -> *mut c_void>,
    destroy: duckdb_delete_callback_t,
) -> bool {
    let set = capi!(duckdb_create_scalar_function_set)(name.as_ptr());
    let mut ok = true;
    for params in overloads {
        let f = capi!(duckdb_create_scalar_function)();
        capi!(duckdb_scalar_function_set_name)(f, name.as_ptr());
        for t in *params {
            let mut lt = capi!(duckdb_create_logical_type)(*t);
            capi!(duckdb_scalar_function_add_parameter)(f, lt);
            capi!(duckdb_destroy_logical_type)(&mut lt);
        }
        let mut rt = capi!(duckdb_create_logical_type)(duckdb_type::DUCKDB_TYPE_VARCHAR);
        capi!(duckdb_scalar_function_set_return_type)(f, rt);
        capi!(duckdb_destroy_logical_type)(&mut rt);
        capi!(duckdb_scalar_function_set_function)(f, Some(func));
        if volatile {
            // Side-effecting: must never be constant-folded at plan time
            // (otherwise `EXPLAIN SELECT anofox_serve(…)` would start a server).
            capi!(duckdb_scalar_function_set_volatile)(f);
        }
        if let Some(make) = extra {
            capi!(duckdb_scalar_function_set_extra_info)(f, make(), destroy);
        }
        ok &= capi!(duckdb_add_scalar_function_to_set)(set, f) == duckdb_state::DuckDBSuccess;
        let mut fm = f;
        capi!(duckdb_destroy_scalar_function)(&mut fm);
    }
    ok &= capi!(duckdb_register_scalar_function_set)(conn, set) == duckdb_state::DuckDBSuccess;
    let mut sm = set;
    capi!(duckdb_destroy_scalar_function_set)(&mut sm);
    ok
}

unsafe fn init(
    info: duckdb_extension_info,
    access: &duckdb_extension_access,
) -> Result<(), String> {
    let get_api = access.get_api.ok_or("no get_api")?;
    let api_ptr = get_api(info, c"v1.2.0".as_ptr()) as *const duckdb_ext_api_v1;
    if api_ptr.is_null() {
        return Err("this DuckDB does not provide C API v1.2.0".into());
    }
    let _ = API.set(*api_ptr); // identical for every load in this process
    let get_db = access.get_database.ok_or("no get_database")?;
    let dbp = get_db(info);
    if dbp.is_null() {
        return Err("no database handle".into());
    }
    let mut raw: duckdb_connection = ptr::null_mut();
    if capi!(duckdb_connect)(*dbp, &mut raw) != duckdb_state::DuckDBSuccess {
        return Err("could not connect".into());
    }
    let conn = db::Conn::owned(raw);

    use duckdb_type::*;
    if !register_scalar(
        conn.raw,
        c"anofox_render",
        &[&[DUCKDB_TYPE_VARCHAR]],
        anofox_render_fn,
        false,
        None,
        None,
    ) {
        return Err("could not register anofox_render".into());
    }
    define_macros(&conn);

    #[cfg(not(target_arch = "wasm32"))]
    {
        use std::sync::Arc;
        // The serving functions keep this connection (the init-time handle is
        // the only way back into this database from a server thread). It is
        // attached per function as extra_info, so each database serves its own
        // data; its lifetime is the database's.
        let ctx = Arc::new(serve::DbCtx {
            conn: std::sync::Mutex::new(conn),
        });
        let make = || Box::into_raw(Box::new(ctx.clone())) as *mut c_void;
        let destroy: duckdb_delete_callback_t = Some(serve_fns::drop_ctx);
        let raw = ctx.conn.lock().map_err(|_| "poisoned")?.raw;
        let ok = register_scalar(
            raw,
            c"anofox_serve",
            &[&[DUCKDB_TYPE_INTEGER]],
            serve_fns::anofox_serve_fn,
            true,
            Some(&make),
            destroy,
        ) && register_scalar(
            raw,
            c"anofox_serve_dashboards",
            &[
                &[DUCKDB_TYPE_VARCHAR, DUCKDB_TYPE_INTEGER],
                &[
                    DUCKDB_TYPE_VARCHAR,
                    DUCKDB_TYPE_INTEGER,
                    DUCKDB_TYPE_VARCHAR,
                ],
            ],
            serve_fns::anofox_serve_dashboards_fn,
            true,
            Some(&make),
            destroy,
        ) && register_scalar(
            raw,
            c"anofox_serve_stop",
            &[&[DUCKDB_TYPE_INTEGER]],
            serve_fns::anofox_serve_stop_fn,
            true,
            None,
            None,
        );
        if !ok {
            return Err("could not register the serving functions".into());
        }
    }
    // wasm: no server — `conn` is dropped (disconnected) here.
    Ok(())
}

/// DuckDB calls `<extension_name>_init_c_api` on LOAD.
///
/// # Safety
/// Called by DuckDB with a valid `info` handle and `access` table.
#[no_mangle]
pub unsafe extern "C" fn anofox_visualization_init_c_api(
    info: duckdb_extension_info,
    access: *const duckdb_extension_access,
) -> bool {
    if access.is_null() {
        return false;
    }
    let access = &*access;
    match anofox_visualization::host::catch_panic(|| init(info, access)) {
        Ok(()) => true,
        Err(e) => {
            if let Some(set_err) = access.set_error {
                let c = CString::new(format!("anofox_visualization: {e}").replace('\0', ""))
                    .unwrap_or_default();
                set_err(info, c.as_ptr());
            }
            false
        }
    }
}
