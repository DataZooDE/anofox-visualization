# Building anofox-visualization

One Cargo workspace (root `Cargo.toml`):

| Crate | Path | What |
|---|---|---|
| `anofox-visualization` | `.` | the core renderer (annotated rows → SVG), the `dashboard` and `serve` binaries, the wasm build for `web/` |
| `anofox_viz_ffi` | `crates/anofox-viz-ffi` | Rust staticlib behind the **C++** extension (render-only) |
| `anofox-visualization-duckdb` | `duckext` | the **C-API** extension (render + serving) |
| `tiny_http` | `vendor/tiny_http` | tiny_http 0.12.0 with fixes, used via `[patch.crates-io]` (see its `PATCHES.md`) |

## Two extension builds

| | Render-only (`make`) | Full toolkit (`duckext/scripts/build-native.sh`) |
|---|---|---|
| SQL surface | `anofox_render`, `anofox_bar/_line/_scatter/_area/_xy/_xyc` | the same **plus** `anofox_serve`, `anofox_serve_dashboards`, `anofox_serve_refresh`, `anofox_serve_stop` |
| Shell | C++ (`csrc/`), DuckDB C++ extension ABI, built by extension-ci-tools + CMake + corrosion | Rust only, DuckDB **C extension API** (`duckext/src`), no libduckdb linking |
| DuckDB compatibility | exactly the pinned version (`duckdb` submodule, v1.5.6) | any DuckDB providing C-API v1.2.0 (v1.2+) |
| Macros | registered as internal catalog entries (with descriptions) | created with SQL (see caveat below) |
| Ships as | the DuckDB community extension (signed) | GitHub release assets + a DuckDB extension repository on Pages (`.github/workflows/extension.yml`, unsigned) |
| Tests | `make test` → `test/sql/*.test` | `cargo test -p anofox-visualization-duckdb`, smoke test in `build-native.sh` |

```sh
# render-only (needs submodules, CMake, a C++ toolchain, Rust; GEN=ninja recommended)
git submodule update --init --recursive
GEN=ninja make && make test

# full toolkit (needs Rust, libclang for bindgen, wasm-bindgen + wasm-opt — not wasm-pack, #552 — python3, a duckdb CLI)
duckext/scripts/build-native.sh --release        # → /tmp/anofox_visualization.duckdb_extension
#   OUT=<path> picks another file; LOAD needs anofox_visualization[.<x>].duckdb_extension (#201)

# core, CLIs, web
cargo test --locked --features serve
cargo run --bin dashboard -- dashboards/sessions.sql
wasm-pack build --target web --out-dir web/pkg --profile wasm --no-default-features --features wasm
```

`--profile wasm` picks the `[profile.wasm]` Cargo profile (fat LTO, one codegen
unit, opt-level tuned for size) instead of plain `--release`, which stays
Cargo's defaults for the native extension/server. CI checks the built
`web/pkg/anofox_visualization_bg.wasm` against a committed budget
(`web/wasm-size.txt` + 30 KB, `scripts/check-wasm-size.mjs --check`); after an
intentional size change, update the baseline with
`node scripts/check-wasm-size.mjs` and commit `web/wasm-size.txt` (#312).

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
validation, size caps, panic → error), the macro table (`src/macros.rs` —
the C++ build reads it through the FFI accessors `anofox_viz_macro_*`, the
C-API build runs `CREATE MACRO` from it), the version, and `Cargo.lock`.

### C-API build: macro caveat

The C extension API cannot register *internal* macros, so the C-API build
creates its convenience macros with SQL in the default database at `LOAD`.
That means they are persisted into a file-backed database. To limit the
impact, an existing macro of the same name is only replaced if it is one of
ours (its body calls `anofox_render`/`anofox_xy`), and a read-only database
simply gets no macros. The C++ build registers them as internal, documented
catalog entries and has neither issue.

Because the macros live in that database, they are not found after `USE <other database>`: DuckDB looks unqualified functions up on the search path, and `USE` replaces it (`Catalog Error: Scalar Function with name anofox_plot_summary does not exist! Did you mean "memory.anofox_plot_summary"?`). Call them qualified with the database that was the default at `LOAD` (`memory.anofox_plot_summary(s)` in the CLI's in-memory session), or keep that database on the search path after `USE f`: `SET search_path = 'f.main,memory.main'`. `CREATE TEMP MACRO` is no way out: the extension creates the macros on its own connection, and temporary objects are visible only on the connection that created them. The C++ build (`INSTALL anofox_visualization FROM community`) registers them in the system catalog and has neither issue (#179).

## CI lanes (#464)

`.github/workflows/lane.yml` (a reusable workflow both `ci.yml` and
`MainDistributionPipeline.yml` call first) decides, once per run, which lane
applies:

- **Fast lane** (target ~10 min) — every other PR: `fmt`+`clippy`+`cargo
  nextest`, the C++ ABI extension's sqllogictests (`sqllogictest` job:
  `make test` → `test/sql/*.test`, linux_amd64 only, ccache + rust-cache so
  repeat runs stay fast — #491), the web tests (`web/pkg` built once and
  shared via artifact, `tests/web/*.mjs` sharded into 4 jobs, the browser
  e2e scripts, the wasm size check). A **docs-only** PR (every changed file
  `*.md` or under `docs/`) instead runs only a markdown/link check.
- **Full lane** — everything in the fast lane, plus the DuckDB 2.0 smoke
  test, the Quack prototype, the strict `Explore starter dashboards --check
  --strict` check, the Claude Code skills-SQL validator, every
  `*-conformance` job (they install extensions from live network sources:
  get.erpl.io / the community repository), and the cross-platform extension
  builds (`MainDistributionPipeline.yml`). It runs on a push to `master`, a
  `v*` tag, `workflow_dispatch`, the nightly schedule, a `release/*` branch,
  or a PR labelled **`full-ci`**.

Add the `full-ci` label to a PR when it touches: a dependency bump
(`Cargo.lock`, `package.json`/-`lock`, `wasm-extensions.lock`), serving or
security code (`duckext/src/host/`, anything under `src/host/`, the access
policy), `csrc/` or `CMakeLists.txt`, or when it is itself a release PR.
Opening a PR with a label already attached fires `opened` and `labeled`
together; `ci.yml`'s and `MainDistributionPipeline.yml`'s `lane` job only
reacts to a bare `labeled`/`unlabeled` event when the label itself is
`full-ci`, so that still starts exactly one run per workflow (#491).

`ci-gate` (in `ci.yml`) is the one check to point branch protection / a
merge script at: it passes once every needed job has succeeded or was
skipped, so a docs-only PR resolves cleanly instead of leaving full-lane-only
checks pending forever. A full-lane run that fails on `master` or the
nightly schedule files (or comments on) a `ci-master-red` issue.

## Release assets (full toolkit)

Before tagging a release, confirm the release commit's `master` run of
`ci.yml` (full lane — a push to `master` always takes it) finished green;
a release should never go out over a red `master`.

Publishing a GitHub release pushes its `v<version>` tag, which runs
`.github/workflows/extension.yml` ("Release full toolkit"). One job per
platform (linux_amd64 / linux_arm64 on Ubuntu 22.04, osx_arm64, osx_amd64,
windows_amd64) builds and smoke-tests on its own runner, then one job writes
`SHA256SUMS` and uploads everything to the release (`gh release upload
--clobber`, so re-runs replace assets; if only the tag exists the release is
created). `workflow_dispatch` builds the same files as workflow artifacts
without touching any release. After a successful tag run, `pages.yml`
republishes the latest release's extensions as a DuckDB extension repository
under `https://datazoode.github.io/anofox-visualization/extensions`.

The steps are scripts in `scripts/release/`, runnable locally (Linux, macOS,
Git Bash on Windows; output in `dist/`, the DuckDB CLI cached in `.cache/`):

```sh
scripts/release/build-extension.sh   # web/pkg + extension + smoke test
#   → dist/anofox_visualization.v<version>.duckdb-v1.5.6.<platform>.duckdb_extension
scripts/release/package-cli.sh       # dashboard + serve, smoke test, archive
#   → dist/anofox-visualization-cli-v<version>-<rust target>.tar.gz (.zip on Windows)
DRY_RUN=1 scripts/release/upload-release.sh v<version> dist   # SHA256SUMS only
scripts/release/extension-repo.sh dist /tmp/repo   # → /tmp/repo/v1.5.6/<platform>/anofox_visualization.duckdb_extension.gz
```

The extension smoke test (`smoke-extension.sh`) loads the file in the pinned
DuckDB CLI with `-unsigned` and checks: an SVG from `anofox_render`, NULL → NULL,
a bad spec raises an error, `anofox_contract_version()`, the `anofox_serve*`
functions, the stamped extension version, and that web/pkg is embedded. The
CLI smoke test runs `--version`/`--help` and `dashboard --check
dashboards/sessions.sql`. The extension file name must start with
`anofox_visualization.` — DuckDB derives the extension name (and its
`<name>_init_c_api` entry point) from the file name up to the first dot.

## Versions — one source of truth

- `[workspace.package] version` in the root `Cargo.toml` (CalVer
  `YYYY.M.D`) is *the* version. Both extension crates inherit it; CMake reads it
  for the C++ build's version fallback (`ANOFOX_VIZ_CARGO_VERSION`; release
  builds get the git tag via `EXT_VERSION_ANOFOX_VISUALIZATION`);
  `build-native.sh` and `extension.yml` stamp `v<version>` into the C-API
  artifact. `duckext/description.yml` must carry the same value.
- The **build version** people see (#216) comes from `git describe` at build
  time (root `build.rs` → `anofox_visualization::BUILD_VERSION`): `v2026.10.18`
  for a build of a release tag, `v2026.10.18+37 · 739aba4` (commits since the
  tag · short sha, `-dirty` with uncommitted changes) for any other build, and
  `v<version>` for a build without git (a source tarball, the docker image).
  It is what the builder's sidebar footer, `dashboard --version`,
  `serve --version` and the locked server's `/healthz` (`"version"`) report.
  Extension metadata, asset names and the files the release bumps stay plain
  CalVer. CI checks out full history (`fetch-depth: 0`) where it builds what it
  ships, and the release build fails unless `--version` prints exactly the tag.
- DuckDB: the C++ build is pinned to the `duckdb` submodule (v1.5.6) and
  `MainDistributionPipeline.yml`'s `duckdb_version`; bump both together. The
  C-API build requests C-API v1.2.0 (so its metadata footer says `v1.2.0`, not
  a DuckDB release); the DuckDB release the full toolkit is tested against —
  and published for in the release asset names and the Pages extension
  repository — is `.duckdb-version` (read by `scripts/release/` and
  `pages.yml`). Bump it together with the two above.

## Lockfile policy

`Cargo.lock` (workspace root) is committed. CI, `Makefile` targets,
`build-native.sh` and corrosion (`LOCKED`) all build with `--locked`; update it
deliberately with `cargo update -p <crate>`.

## WASM side-module (experimental)

Notes on loading the C-API crate into DuckDB-Wasm are in
[`duckext/BUILD.md`](duckext/BUILD.md#wasm-side-module-experimental).
