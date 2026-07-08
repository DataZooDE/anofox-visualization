
## Native C-API extension — where it got to
`duckext/` is a hand-rolled DuckDB C Extension API binding (bindgen of
`duckdb_extension.h`, **no libduckdb linking** — the wasm-viable design).

Proven working:
- Compiles to a cdylib exporting `ggplot_init_c_api`.
- With the 512-byte metadata footer (magic `4`, platform, capi-version `v1.2.0`,
  ext-version, ABI `C_STRUCT`) appended → `ggplot.duckdb_extension`.
- **DuckDB LOADs it** (`duckdb -unsigned`), runs the entrypoint, gets the API +
  database, connects, and **registers the `ggplot_smoke()` scalar** — all via the
  C API struct. The scalar callback then fires when called.

Remaining bug: in the callback, `duckdb_data_chunk_get_size` reads as a null fn
pointer while `duckdb_create_scalar_function` (used at register time) is valid —
a struct field-offset mismatch between the bindgen `duckdb_ext_api_v1` and the
host's exact ABI. Needs offset debugging (dump/compare the struct, or match the
host's exact CAPI build flags / requested version string). The duckdb-rs crate
gets this right but bundles+compiles libduckdb (unusable for the wasm side-module),
so the hand-rolled path is required for wasm.

## Footer recipe (works)
```sh
# append the 512-byte metadata footer, then:
duckdb -unsigned -c "LOAD '.../ggplot.duckdb_extension'; SELECT ggplot_smoke();"
```
