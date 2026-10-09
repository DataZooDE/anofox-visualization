// Bindgen the DuckDB C Extension API (duckdb_ext_api_v1 + access/entry types).
// No linking against libduckdb — the extension resolves the API at load time via
// the access struct, which is what makes it a valid wasm side-module.
//
// The headers are VENDORED in `capi_hdr/` (copied from the pinned `duckdb`
// submodule, currently v1.5.6) so a clean checkout builds without the submodule
// or a separately downloaded header. `wrapper.h` pins the requested C-API
// version (v1.2.0), so newer headers still yield the v1.2.0 struct layout.
// To refresh: `cp duckdb/src/include/duckdb{,_extension}.h duckext/capi_hdr/`.
fn main() {
    let manifest = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let hdr = manifest.join("capi_hdr");
    println!("cargo:rerun-if-changed=wrapper.h");
    println!("cargo:rerun-if-changed=capi_hdr/duckdb_extension.h");
    println!("cargo:rerun-if-changed=capi_hdr/duckdb.h");
    let bindings = bindgen::Builder::default()
        .header(manifest.join("wrapper.h").to_string_lossy())
        .clang_arg(format!("-I{}", hdr.display()))
        .allowlist_type("duckdb_.*")
        .allowlist_function("duckdb_.*")
        .allowlist_var("DUCKDB_.*")
        .default_enum_style(bindgen::EnumVariation::Rust {
            non_exhaustive: false,
        })
        .generate()
        .expect("failed to bindgen duckdb_extension.h");
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());
    bindings
        .write_to_file(out.join("bindings.rs"))
        .expect("write bindings");
}
