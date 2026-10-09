# duckext — the C-API extension (full toolkit)

`duckext/` is a hand-rolled DuckDB C Extension API binding (bindgen of the
vendored `capi_hdr/duckdb_extension.h`, **no libduckdb linking** — which also
keeps the wasm side-module route open). It is the *full toolkit* build:
rendering plus the serving functions. For how it relates to the render-only
CMake build at the repository root, see [`../BUILD.md`](../BUILD.md).

## Native build ✅

```sh
duckext/scripts/build-native.sh --release   # → /tmp/anofox_visualization.duckdb_extension
duckdb -unsigned
```

The script builds `web/pkg` with wasm-pack (embedded into the extension for the
browser UI), builds the cdylib with `cargo build --locked -p
anofox-visualization-duckdb`, packages it with the official metadata footer
(abi `C_STRUCT`, C-API `v1.2.0`, version from the workspace `Cargo.toml`) via
`scripts/append_extension_metadata.py`, and smoke-tests it (renders a chart;
a bad spec must raise an error).

```sql
LOAD '/tmp/anofox_visualization.duckdb_extension';
WITH sales AS (SELECT * FROM (VALUES ('app',30),('web',22),('api',12)) t(ch,n))
SELECT anofox_bar(ch, n) FROM sales;                 -- bar chart
--   anofox_line/_scatter/_area(x, y), anofox_xy(x, y, kind := 'VIOLIN'),
--   anofox_xyc(x, y, series)  (coloured by series)
SELECT anofox_serve(8080);                           -- authoring UI (loopback + token)
SELECT anofox_serve_dashboards('dashboards', 8095);  -- locked serving
SELECT anofox_serve_stop(8095);
```

`anofox_render(spec)` is the raw form — `spec` is the same JSON the browser
renderer takes: `rows` (row objects keyed `c0`,`c1`,…), `roles`
(`[[colIdx,"ROLE","displayName"?],…]`), optional `width`/`height`/`primary`.

Behaviour at the boundary (shared with the C++ build via
`anofox_visualization::host::render_spec_checked`):

- NULL → NULL. A malformed spec, an unknown or NULL role, invalid UTF-8,
  `width`/`height` outside `[16, 8192]` or more than 200,000 rows raise a SQL
  error. Every `extern "C"` callback runs under `catch_unwind`, so a renderer
  panic is a SQL error too, never an abort.
- The serving functions are VOLATILE (never constant-folded: `EXPLAIN` does not
  start a server), return NULL for NULL arguments, and reject ports outside
  `1..65535`. Their security model is in
  [`../docs/secure-serving.md`](../docs/secure-serving.md).
- Macros are created with SQL (the C API cannot register internal macros) —
  see the caveat in [`../BUILD.md`](../BUILD.md#c-api-build-macro-caveat).

Tested against the DuckDB v1.5.x CLI (`linux_amd64`); any DuckDB that provides
C-API v1.2.0 can load it.

See `DISTRIBUTING.md` for shipping it and `../.github/workflows/extension.yml`
for the multi-platform build.

## WASM side-module (experimental)

Goal: a DuckDB-Wasm-loadable extension (emscripten side-module) that renders
with ggplot-rs in the browser. This is R&D; the browser builder in `web/` uses
the core's wasm-bindgen build instead.

Proven: emscripten + the Rust `wasm32-unknown-emscripten` target compile the
core and ggplot-rs, and a side-module `.wasm` builds with the render path
reachable over the C ABI:

```sh
source ~/emsdk/emsdk_env.sh
RUSTFLAGS="-C panic=abort -C link-arg=-sSIDE_MODULE=1" \
  cargo build -p anofox-visualization-duckdb --target wasm32-unknown-emscripten
wasm-opt -Oz --strip-debug target/wasm32-unknown-emscripten/debug/anofox_visualization_ext.wasm -o ext.wasm
# metadata as a duckdb_signature WASM CUSTOM SECTION (not a raw footer):
python3 duckext/scripts/append_extension_metadata.py -l ext.wasm -n anofox_visualization \
  -o repo/v1.1.1/wasm_eh/anofox_visualization.duckdb_extension.wasm -p wasm_eh -dv v0.0.1 -ev v0.1.0
```

(`-C panic=abort` here: the native build unwinds so panics become SQL errors;
the side-module experiments used abort to drop libc imports.) DuckDB-Wasm 1.29.0
is DuckDB v1.1.1 / platform `wasm_eh` / C-ext-API `v0.0.1` (`duckdb_ext_api_v0`),
so the wasm build uses the v1.1.1 headers in `wasm_hdr/`.

Status: `INSTALL` ok → module valid (custom section correct) → dlopen reaches
instantiation. Rust's std links libc file-I/O syscalls (`pread`, `pwrite`,
`preadv`, `pwritev`, `ftruncate`, `lseek`) whose emscripten i64 legalization
doesn't match DuckDB-Wasm's host imports; local `#[no_mangle]` stubs (in
`src/lib.rs`) stop them being imported and the module links and instantiates,
then hits a runtime `function signature mismatch` at an indirect call. Next
steps: match emscripten's i64 legalization for the stubs, remove the file-I/O
linkage, or build through extension-ci-tools' own Rust/wasm pipeline.
Other snag: `--release` fails because cargo also builds ggplot-rs's cdylib and
wasm-opt's side-module pass errors on it. `test-wasm.sh` drives the experiment.
