# Building anofox-visualization

One Cargo workspace (root `Cargo.toml`):

| Crate | Path | What |
|---|---|---|
| `anofox-visualization` | `.` | the core renderer (annotated rows → SVG), the `dashboard` and `serve` binaries, the wasm build for `web/` |
| `anofox_viz_ffi` | `crates/anofox-viz-ffi` | Rust staticlib behind the **C++** extension (render-only) |
| `anofox-visualization-duckdb` | `duckext` | the **C-API** extension (render + serving) |

## Two extension builds

| | Render-only (`make`) | Full toolkit (`duckext/scripts/build-native.sh`) |
|---|---|---|
| SQL surface | `anofox_render`, `anofox_bar/_line/_scatter/_area/_xy/_xyc` | the same **plus** `anofox_serve`, `anofox_serve_dashboards`, `anofox_serve_stop` |
| Shell | C++ (`csrc/`), DuckDB C++ extension ABI, built by extension-ci-tools + CMake + corrosion | Rust only, DuckDB **C extension API** (`duckext/src`), no libduckdb linking |
| DuckDB compatibility | exactly the pinned version (`duckdb` submodule, v1.5.6) | any DuckDB providing C-API v1.2.0 (v1.2+) |
| Macros | registered as internal catalog entries (with descriptions) | created with SQL (see caveat below) |
| Ships as | the DuckDB community extension (signed) | self-hosted artifact (`.github/workflows/extension.yml`, unsigned) |
| Tests | `make test` → `test/sql/*.test` | `cargo test -p anofox-visualization-duckdb`, smoke test in `build-native.sh` |

```sh
# render-only (needs submodules, CMake, a C++ toolchain, Rust; GEN=ninja recommended)
git submodule update --init --recursive
GEN=ninja make && make test

# full toolkit (needs Rust, libclang for bindgen, wasm-pack, python3, a duckdb CLI)
duckext/scripts/build-native.sh --release        # → /tmp/anofox_visualization.duckdb_extension

# core, CLIs, web
cargo test --locked --features serve
cargo run --bin dashboard -- dashboards/sessions.sql
wasm-pack build --target web --out-dir web/pkg --no-default-features --features wasm
```

The C-API crate bindgens the **vendored** headers in `duckext/capi_hdr/` (copied
from the pinned `duckdb` submodule) — no submodule or download needed;
`duckext/wrapper.h` pins the requested C-API version (v1.2.0). To refresh after
a DuckDB bump: `cp duckdb/src/include/duckdb{,_extension}.h duckext/capi_hdr/`.

### Why two builds, not one (decision)

Unifying them was evaluated and not done:

- The serving code needs to *drive* DuckDB from server threads: open a second
  database instance for the read-only snapshot, open connections per request,
  prepare/bind/interrupt queries. The C-API extension gets all of that from the
  `duckdb_ext_api_v1` function table handed over at load time. A C++ ABI
  extension has no such table to give a Rust staticlib; doing it would mean a
  hand-written C++ callback layer re-implementing that surface, or relying on
  the DuckDB C API symbols being exported by the host process, which is not
  the case for every client (e.g. the Python module).
- The community binary is deliberately small and server-free (no embedded web
  UI, no HTTP listener): a signed, auto-installable extension should not be
  able to open sockets.
- The C++ ABI build is what extension-ci-tools/community-extensions build and
  sign today; the C-API build is portable across DuckDB versions. Each is the
  better fit for its distribution channel.

What *is* shared, so the two cannot drift: the renderer, the FFI-boundary
render entry point (`anofox_visualization::host::render_spec_checked` — spec
validation, size caps, panic → error), the macro bodies (kept identical in
`csrc/anofox_visualization_extension.cpp` and `duckext/src/lib.rs`), the
version, and `Cargo.lock`.

### C-API build: macro caveat

The C extension API cannot register *internal* macros, so the C-API build
creates its convenience macros with SQL in the default database at `LOAD`.
That means they are persisted into a file-backed database. To limit the
impact, an existing macro of the same name is only replaced if it is one of
ours (its body calls `anofox_render`/`anofox_xy`), and a read-only database
simply gets no macros. The C++ build registers them as internal, documented
catalog entries and has neither issue.

## Versions — one source of truth

- `[workspace.package] version` in the root `Cargo.toml` (CalVer
  `YYYY.M.D`) is *the* version. Both extension crates inherit it; CMake reads it
  for the C++ build's version fallback (`ANOFOX_VIZ_CARGO_VERSION`; release
  builds get the git tag via `EXT_VERSION_ANOFOX_VISUALIZATION`);
  `build-native.sh` and `extension.yml` stamp `v<version>` into the C-API
  artifact. `duckext/description.yml` must carry the same value.
- DuckDB: the C++ build is pinned to the `duckdb` submodule (v1.5.6) and
  `MainDistributionPipeline.yml`'s `duckdb_version`; bump both together. The
  C-API build requests C-API v1.2.0.

## Lockfile policy

`Cargo.lock` (workspace root) is committed. CI, `Makefile` targets,
`build-native.sh` and corrosion (`LOCKED`) all build with `--locked`; update it
deliberately with `cargo update -p <crate>`.

## WASM side-module (experimental)

Notes on loading the C-API crate into DuckDB-Wasm are in
[`duckext/BUILD.md`](duckext/BUILD.md#wasm-side-module-experimental).
