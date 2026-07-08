// Bindgen the DuckDB C Extension API (duckdb_ext_api_v1 + access/entry types).
// No linking against libduckdb — the extension resolves the API at load time via
// the access struct, which is what makes it a valid wasm side-module.
fn main() {
    println!("cargo:rerun-if-changed=wrapper.h");
    println!("cargo:rerun-if-changed=duckdb_extension.h");
    let bindings = bindgen::Builder::default()
        .header("wrapper.h")
        .clang_arg("-I.")
        .allowlist_type("duckdb_.*")
        .allowlist_function("duckdb_.*")
        .allowlist_var("DUCKDB_.*")
        .default_enum_style(bindgen::EnumVariation::Rust { non_exhaustive: false })
        .generate()
        .expect("failed to bindgen duckdb_extension.h");
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());
    bindings
        .write_to_file(out.join("bindings.rs"))
        .expect("write bindings");
}
