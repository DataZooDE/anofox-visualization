# Distributing the anofox_visualization DuckDB extension

There are two native binaries (see the root [`BUILD.md`](../BUILD.md)); they ship
through different routes.

| Binary | Built by | Ships via |
|---|---|---|
| **Render-only** (`anofox_render` + macros) | `make` at the repo root (C++ shell + `crates/anofox-viz-ffi`) | DuckDB **community extensions** (signed) — `.github/workflows/MainDistributionPipeline.yml` |
| **Full toolkit** (render + `anofox_serve*`) | `duckext/scripts/build-native.sh` (this crate, C-API) | **self-hosted**, unsigned — `.github/workflows/extension.yml` |

## Prerequisites (both)

- The repository is public: `github.com/DataZooDE/anofox-visualization`.
- Self-contained build: `ggplot-rs` is a **pinned git dependency** of the core
  (`git = "https://github.com/sipemu/ggplot-rs", rev = "91ebc37…"` in the root
  `Cargo.toml`), so Cargo fetches it — no sibling checkout, no crates.io
  publish. `Cargo.lock` is committed and every build uses `--locked`.
- One version: `[workspace.package] version` in the root `Cargo.toml`
  (CalVer). `duckext/description.yml`'s `version` must match it; release tags are
  `v<version>`.

## Route 1 — DuckDB Community Extensions (render-only, signed `INSTALL`)

```sql
INSTALL anofox_visualization FROM community;
LOAD anofox_visualization;
```

Submit `duckext/description.yml` to
[duckdb/community-extensions](https://github.com/duckdb/community-extensions):
copy it to `extensions/anofox_visualization/description.yml`, set `repo.ref` to
the release commit SHA, and open a PR. Their CI runs the same
extension-ci-tools pipeline as `MainDistributionPipeline.yml` (`build: cmake`,
`requires_toolchains: rust`) and signs the result. The C++ ABI pins one DuckDB
version per build (currently v1.5.6).

## Route 2 — self-hosted repository (full toolkit, unsigned)

`.github/workflows/extension.yml` (on `v*` tags or manually) builds the web UI
(`wasm-pack`), the C-API extension, packages it with
`append_extension_metadata.py` (`--abi-type C_STRUCT`, C-API `v1.2.0`) for
linux/macOS/Windows, smoke-tests it with a pinned Python `duckdb`, and uploads
`anofox_visualization.duckdb_extension` per platform. Serve the files in the
DuckDB repository layout:

```
<repo>/v1.2.0/<platform>/anofox_visualization.duckdb_extension[.gz]
```

```sql
SET custom_extension_repository = 'https://you.example.com/duckdb';
INSTALL anofox_visualization;      -- needs allow_unsigned_extensions / duckdb -unsigned
LOAD anofox_visualization;
```

C-API (`C_STRUCT`) extensions are portable across DuckDB releases that provide
that API version (v1.2.0+), so one build per platform covers many DuckDB
versions — unlike the C++ ABI build, which pins an exact version.
