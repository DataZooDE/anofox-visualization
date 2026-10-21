# Distributing the anofox_visualization DuckDB extension

There are two native binaries (see the root [`BUILD.md`](../BUILD.md)); they ship
through different routes.

| Binary | Built by | Ships via |
|---|---|---|
| **Render-only** (`anofox_render` + macros) | `make` at the repo root (C++ shell + `crates/anofox-viz-ffi`) | DuckDB **community extensions** (signed) — `.github/workflows/MainDistributionPipeline.yml` |
| **Full toolkit** (render + `anofox_serve*`) | `duckext/scripts/build-native.sh` (this crate, C-API) | **GitHub release assets + extension repository on Pages**, unsigned — `.github/workflows/extension.yml` |

## Prerequisites (both)

- The repository is public: `github.com/DataZooDE/anofox-visualization`.
- Self-contained build: `ggplot-rs` comes from crates.io (`ggplot-rs = "0.17"`
  in the root `Cargo.toml`) — no sibling checkout needed. `Cargo.lock` is
  committed and every build uses `--locked`.
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
version per build.

**DuckDB 2.0 (#352):** since 2026-10-19 the community-extensions repo builds
`repo.ref` against DuckDB v2.0.0 by default; a v1.5.6 build only happens
through the separate `variegata` field (duckdb/community-extensions#3081),
set to the same commit SHA as `ref`. Our `csrc/` + `CMakeLists.txt` build
unchanged against both v1.5.6 (`extension-ci-tools@v1.5-variegata`) and
v2.0.0 (`extension-ci-tools@v2.0-cyanoptera`) — see
`.github/workflows/MainDistributionPipeline.yml`, which runs both. When
submitting: fill in `variegata` in `description.yml` (commented out there
until a release SHA exists) and call it out at the top of the PR description,
as the issue above asks. The pin in `.duckdb-version` (this repository's own
default) stays at v1.5.6 until ~2.0.1; the community submission ships both
regardless of that pin.

## Route 2 — GitHub releases + Pages repository (full toolkit, unsigned)

Publishing a GitHub release pushes its tag `v<version>`, which runs
`.github/workflows/extension.yml` ("Release full toolkit"). Per platform
(linux_amd64, linux_arm64, osx_arm64, osx_amd64, windows_amd64) it builds
web/pkg (`wasm-pack`) and the C-API extension, packages it with
`append_extension_metadata.py` (`--abi-type C_STRUCT`, C-API `v1.2.0`), builds
the `dashboard` / `serve` CLIs, smoke-tests both against the DuckDB CLI pinned
in `.duckdb-version` (v1.5.6) on that runner, and uploads to the release:

```
anofox_visualization.<tag>.duckdb-v1.5.6.<platform>.duckdb_extension
anofox-visualization-cli-<tag>-<rust target>.tar.gz   (.zip on Windows)
SHA256SUMS
```

The logic is in `scripts/release/` (see the root `BUILD.md`, "Release
assets"); `workflow_dispatch` produces the same files as workflow artifacts
only. Before publishing a release: bump the CalVer version (`Cargo.toml`,
`duckext/description.yml`), merge, then publish the release with tag
`v<version>` — the build job fails if the tag and `Cargo.toml` disagree.

After a successful tag run, `pages.yml` downloads the latest release's
extensions and publishes them in DuckDB's repository layout
(`scripts/release/extension-repo.sh`):

```
https://datazoode.github.io/anofox-visualization/extensions/v1.5.6/<platform>/anofox_visualization.duckdb_extension.gz
```

```sql
-- duckdb -unsigned  (or SET allow_unsigned_extensions = true)
INSTALL anofox_visualization FROM 'https://datazoode.github.io/anofox-visualization/extensions';
LOAD anofox_visualization;
```

DuckDB requests `<repository>/<its own version>/<platform>/…`, so only DuckDB
v1.5.6 finds the files there. C-API (`C_STRUCT`) extensions are portable across
DuckDB releases that provide that API version (v1.2.0+), so for another DuckDB
version download the release asset and `LOAD` it directly (or mirror the same
`.gz` under another version directory) — unlike the C++ ABI build, which pins
an exact version.
