//! DuckDB C-API extension: `SELECT ggplot_smoke()` → an SVG rendered by
//! ggplot-rs (via the duckplot core). Uses the C Extension API — the DuckDB
//! functions are resolved at load time through the `access` struct, so nothing
//! links against libduckdb. That's what lets the same crate become a DuckDB-Wasm
//! side-module.
#![allow(non_upper_case_globals, non_camel_case_types, non_snake_case, dead_code)]

mod ffi {
    include!(concat!(env!("OUT_DIR"), "/bindings.rs"));
}
use ffi::*;
use std::ffi::CString;
use std::ptr;

/// The DuckDB API table — **copied by value** at load time (the pointer from
/// `get_api` is only valid during init, so we own a copy, like the C macro's
/// `duckdb_ext_api = *res`).
static mut API: Option<duckdb_ext_api_v1> = None;
unsafe fn api() -> &'static duckdb_ext_api_v1 {
    #[allow(static_mut_refs)]
    API.as_ref().unwrap()
}

fn smoke_svg() -> CString {
    use duckplot::{render, Column, Kind, Role};
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

/// `SELECT ggplot_smoke()` → one SVG string per input row.
unsafe extern "C" fn ggplot_smoke_fn(
    _info: duckdb_function_info,
    input: duckdb_data_chunk,
    output: duckdb_vector,
) {
    let n = (api().duckdb_data_chunk_get_size.unwrap())(input);
    let svg = smoke_svg();
    for i in 0..n {
        (api().duckdb_vector_assign_string_element.unwrap())(output, i, svg.as_ptr());
    }
}

/// DuckDB calls `<extension_name>_init_c_api` on LOAD.
#[no_mangle]
pub unsafe extern "C" fn ggplot_init_c_api(
    info: duckdb_extension_info,
    access: *const duckdb_extension_access,
) -> bool {
    let access = &*access;
    let Some(get_api) = access.get_api else { return false };
    let api_ptr = get_api(info, c"v1.0.0".as_ptr()) as *const duckdb_ext_api_v1;
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

    let f = (api().duckdb_create_scalar_function.unwrap())();
    (api().duckdb_scalar_function_set_name.unwrap())(f, c"ggplot_smoke".as_ptr());
    let mut vtype = (api().duckdb_create_logical_type.unwrap())(duckdb_type::DUCKDB_TYPE_VARCHAR);
    (api().duckdb_scalar_function_set_return_type.unwrap())(f, vtype);
    (api().duckdb_scalar_function_set_function.unwrap())(f, Some(ggplot_smoke_fn));
    let ok = (api().duckdb_register_scalar_function.unwrap())(conn, f) == duckdb_state::DuckDBSuccess;

    (api().duckdb_destroy_logical_type.unwrap())(&mut vtype);
    let mut fm = f;
    (api().duckdb_destroy_scalar_function.unwrap())(&mut fm);
    (api().duckdb_disconnect.unwrap())(&mut conn);
    ok
}
