//! A distribution's DuckDB extension (#1156): its own name and entry point,
//! the modules registered, then the anofox-visualization load. With
//! `examples/hello_module.rs`'s `::HELLO`, packaged as `hello_viz`:
//!
//!     cargo build -p anofox-visualization-duckdb --example hello_ext
//!     python3 duckext/scripts/append_extension_metadata.py \
//!         -l target/debug/examples/libhello_ext.so -n hello_viz -p linux_amd64 \
//!         -dv v1.2.0 -ev v0 --abi-type C_STRUCT -o hello_viz.duckdb_extension
//!     duckdb -unsigned -c "LOAD 'hello_viz.duckdb_extension';
//!       SELECT anofox_render('{\"rows\":[{\"c0\":\"world\"}],\"roles\":[[0,\"HELLO\"]]}')"
#[path = "../../examples/hello_module.rs"]
mod hello_module;

use anofox_visualization_ext::{duckdb_extension_access, duckdb_extension_info};

/// # Safety
/// Called by DuckDB with a valid `info` handle and `access` table.
#[no_mangle]
pub unsafe extern "C" fn hello_viz_init_c_api(
    info: duckdb_extension_info,
    access: *const duckdb_extension_access,
) -> bool {
    // a second LOAD in the process finds it registered already: fine
    let _ = anofox_visualization::module::register(&hello_module::HELLO);
    anofox_visualization_ext::init_c_api(info, access)
}
