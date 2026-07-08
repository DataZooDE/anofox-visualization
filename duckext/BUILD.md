
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

## WASM extension — where it got to (this is the big one)
Targeting DuckDB-Wasm 1.29.0 = **DuckDB v1.1.1, platform `wasm_eh`, C-ext-API
`v0.0.1`** (struct is `duckdb_ext_api_v0`). Extensions are major-version-pinned,
so the wasm build uses the v1.1.1 headers (native used v1.5.3/v1.2.0).

Recipe that gets it **installed + compiled + instantiated** in the browser:
```sh
source ~/emsdk/emsdk_env.sh
RUSTFLAGS="-C link-arg=-sSIDE_MODULE=1" cargo build --target wasm32-unknown-emscripten
wasm-opt -Oz --strip-debug .../duckplot_duckext.wasm -o ext.wasm
# metadata as a duckdb_signature WASM CUSTOM SECTION (NOT a raw footer — that
# breaks WebAssembly.Module). Use DuckDB's tool:
python3 append_extension_metadata.py -l ext.wasm -n ggplot -o \
  repo/v1.1.1/wasm_eh/ggplot.duckdb_extension.wasm -p wasm_eh -dv v0.0.1 -ev v0.1.0
# serve repo/, then in DuckDB-Wasm:
#   db.open({ allowUnsignedExtensions: true })
#   SET custom_extension_repository='http://host/repo';
#   INSTALL ggplot; LOAD ggplot; SELECT ggplot_smoke();
```
Progress: **INSTALL ok → module VALID (custom section correct) → dlopen reaches
instantiation** → fails on one import: `LinkError: env.pread — imported function
does not match the expected type`.

Final gap: an **emscripten-version ABI mismatch** — the DuckDB-Wasm `eh` bundle is
built with emscripten `'latest'` (unpinned at the 1.29.0 build); mine is 3.1.57,
whose libc `pread` signature differs. Fix: build with the exact emscripten the eh
bundle used (or stub/avoid the `pread` import). The COI bundle *does* pin 3.1.57
but is threaded (shared-memory) → different side-module memory model.
