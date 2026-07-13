# Building the WASM DuckDB extension — status & recipe

Goal: a DuckDB-Wasm-loadable extension (emscripten side-module) that renders
SQL-defined dashboards with ggplot-rs. Real R&D; this tracks what works.

## ✅ Proven so far (the hard feasibility chain)
1. **Core** (`src/lib.rs`) — Shaper roles → ggplot-rs → SVG. Tested; renders the
   Shaper stacked-bar example.
2. **emscripten** 3.1.57 (`~/emsdk`) + Rust `wasm32-unknown-emscripten` target.
3. **ggplot-rs compiles for emscripten** (incl. plotters).
4. **A loadable emscripten side-module `.wasm` builds** with the render path
   reachable over the C ABI (`anofox_smoke`/`anofox_free`):

   ```sh
   source ~/emsdk/emsdk_env.sh
   RUSTFLAGS="-C link-arg=-sSIDE_MODULE=1 -C link-arg=-sWASM_BIGINT" \
     cargo build --target wasm32-unknown-emscripten          # dev works
   wasm-opt -Oz --strip-debug target/.../debug/anofox-visualization.wasm -o anofox-visualization.wasm  # 35M → 23M
   ```

## ⚠️ Known snags
- **Release build** fails: cargo also builds ggplot-rs's *cdylib* (declared for
  wasm-pack) and wasm-opt's `-O3` side-module pass errors on it. Fix: stop
  ggplot-rs building a cdylib for this target (feature/profile split or a thin
  rlib-only shim), then `--release` + `opt-level="z"` + LTO → a few MB.

## ⏳ Remaining
- **DuckDB C extension entrypoint**: FFI to DuckDB-Wasm's C extension API (capi)
  matching duckdb-wasm ≈ 1.1.x — the `<name>_init_c_api` entrypoint registering a
  scalar/aggregate that calls the core and returns the SVG.
- **Signature** (`duckdb_signature` custom section) + serve from a custom repo,
  load in the browser via `allow_unsigned_extensions` + `INSTALL`/`LOAD`.
