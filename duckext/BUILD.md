
## anofox-visualization — native C-API extension ✅ WORKING
`duckext/` is a hand-rolled DuckDB C Extension API binding (bindgen of
`duckdb_extension.h`, **no libduckdb linking** — the wasm-viable design).

Working end-to-end on DuckDB v1.5.3 (`linux_amd64`):
- Compiles to a cdylib exporting `anofox_visualization_init_c_api`.
- Packaged with the official metadata footer (abi `C_STRUCT`, capi-version
  `v1.2.0`) via `scripts/append_extension_metadata.py` →
  `anofox_visualization.duckdb_extension`.
- **DuckDB LOADs it** (`duckdb -unsigned`), registers the
  `anofox_render(VARCHAR spec) → VARCHAR` scalar, and the callback renders each
  row's spec to an SVG via ggplot-rs. Vectorised; NULL-safe; bad JSON → an
  `<pre>error</pre>` SVG.

The old "null fn pointer for `duckdb_data_chunk_get_size`" note was stale — it
came from a mismatched header/version combo. With the v1.5.3 `duckdb_extension.h`
requesting capi `v1.2.0`, the struct offsets line up and every API call resolves.

`scripts/build-native.sh` builds + packages + smoke-tests it.

### Usage
```sh
./scripts/build-native.sh   # → /tmp/anofox_visualization.duckdb_extension
duckdb -unsigned <<'SQL'
LOAD '/tmp/anofox_visualization.duckdb_extension';
WITH d AS (SELECT * FROM (VALUES ('app',30),('web',22),('api',12)) t(ch,n))
SELECT anofox_render(json_object(
  'rows',  (SELECT to_json(list({ch: ch, n: n})) FROM d),
  'roles', json('[[0,"XAXIS"],[1,"BARCHART"]]'),
  'width', 400, 'height', 260)) AS svg;
SQL
```
The `spec` is the same JSON the browser renderer takes: `rows` (array of row
objects), `roles` (`[[colIdx, "ROLE", "displayName"?], …]`), optional `width` /
`height` / `primary`.

## WASM extension — where it got to (this is the big one)
Targeting DuckDB-Wasm 1.29.0 = **DuckDB v1.1.1, platform `wasm_eh`, C-ext-API
`v0.0.1`** (struct is `duckdb_ext_api_v0`). Extensions are major-version-pinned,
so the wasm build uses the v1.1.1 headers (native used v1.5.3/v1.2.0).

Recipe that gets it **installed + compiled + instantiated** in the browser:
```sh
source ~/emsdk/emsdk_env.sh
RUSTFLAGS="-C link-arg=-sSIDE_MODULE=1" cargo build --target wasm32-unknown-emscripten
wasm-opt -Oz --strip-debug .../anofox_visualization.wasm -o ext.wasm
# metadata as a duckdb_signature WASM CUSTOM SECTION (NOT a raw footer — that
# breaks WebAssembly.Module). Use DuckDB's tool:
python3 append_extension_metadata.py -l ext.wasm -n anofox_visualization -o \
  repo/v1.1.1/wasm_eh/anofox_visualization.duckdb_extension.wasm -p wasm_eh -dv v0.0.1 -ev v0.1.0
# serve repo/, then in DuckDB-Wasm:
#   db.open({ allowUnsignedExtensions: true })
#   SET custom_extension_repository='http://host/repo';
#   INSTALL anofox_visualization; LOAD anofox_visualization; SELECT anofox_render();
```
Progress: **INSTALL ok → module VALID (custom section correct) → dlopen reaches
instantiation** → fails on one import: `LinkError: env.pread — imported function
does not match the expected type`.

Final gap: an **emscripten-version ABI mismatch** — the DuckDB-Wasm `eh` bundle is
built with emscripten `'latest'` (unpinned at the 1.29.0 build); mine is 3.1.57,
whose libc `pread` signature differs. Fix: build with the exact emscripten the eh
bundle used (or stub/avoid the `pread` import). The COI bundle *does* pin 3.1.57
but is threaded (shared-memory) → different side-module memory model.

## WASM — final ABI findings (extension-ci-tools route)
- extension-ci-tools pins **emscripten 3.1.71** (matched it). DuckDB-Wasm 1.29's
  eh bundle was built with emscripten `'latest'` at 2024-10-07.
- The blocker is that **Rust's std links libc file-I/O syscalls** (`pread`,
  `pwrite`, `preadv`, `pwritev`, `ftruncate`, `lseek`) whose emscripten **i64
  legalization** doesn't match DuckDB-Wasm's host imports. `panic=abort` drops a
  couple; `WASM_BIGINT` doesn't help.
- **Defining them as local stubs (`#[no_mangle]`) stops them being imported** and
  advances the loader: pread→preadv→ftruncate→lseek all resolve, and the module
  **links and instantiates**. It then hits a runtime `function signature
  mismatch` — an i64-legalization difference at an indirect call site.
- Next: match emscripten's exact i64 legalization at those call sites (legalized
  i32-pair signatures for the stubs), or eliminate the file-I/O linkage entirely
  (a `no_std`-ish render path / custom target), or build the Rust extension
  through extension-ci-tools' own Rust pipeline. This is the last ~1%.
