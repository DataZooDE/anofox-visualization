# Distributing the anofox_visualization DuckDB extension

The extension works today (see `BUILD.md`). This describes how to ship it to
users. There are two routes; pick based on whether you need signed `INSTALL`.

## What's in the box
- `anofox_render(spec VARCHAR) -> VARCHAR` — a JSON panel spec → SVG.
- Convenience macros (auto-registered at LOAD): `anofox_bar/_line/_scatter/_area(x, y)`,
  `anofox_xy(x, y, kind := ...)`, `anofox_xyc(x, y, series, kind := ...)`.

## Prerequisites (both routes)
1. **Push `anofox-visualization` to a public GitHub repo.** It has no git remote yet; a
   loadable extension needs a public source repo + tags.
2. **Make the build self-contained.** The core depends on `ggplot-rs` by **path**
   (`../ggplot-rs`), and it uses APIs newer than the published crates.io
   **v0.12.0** (`legend_position`, `CoordPolar::inner_radius`, geom hover…), so a
   plain `ggplot-rs = "0.12"` does **not** compile (verified). Options:
   - **git dependency** — push ggplot-rs (it has unreleased commits) and point the
     dep at that rev: `ggplot-rs = { git = "https://github.com/sipemu/ggplot-rs", rev = "…" }`.
     Self-contained for CI, keeps every feature, no crates.io publish needed.
   - **crates.io** — publish a newer `ggplot-rs` (e.g. 0.13) that has those APIs,
     then `ggplot-rs = "0.13"`. Cleanest long-term.
   - **sibling checkout** — what `.github/workflows/extension.yml` does today
     (`ref: main`); that ref must contain the required commits.

## Route 1 — self-hosted repo (works now, unsigned)
The included `.github/workflows/extension.yml` builds + packages the extension
for linux/macOS/Windows and uploads `anofox_visualization.duckdb_extension` per
platform (via `duckext/scripts/append_extension_metadata.py`, exactly like the
local `build-native.sh`). Serve those files in the DuckDB repo layout:

```
<repo>/v1.2.0/<platform>/anofox_visualization.duckdb_extension[.gz]
```

Users then opt in:
```sql
SET custom_extension_repository = 'https://you.example.com/duckdb';
INSTALL anofox_visualization;      -- needs allow_unsigned_extensions / duckdb -unsigned
LOAD anofox_visualization;
```
Self-hosted extensions are **not signed by DuckDB**, so they load only in
unsigned mode. Fine for internal / opt-in distribution.

## Route 2 — DuckDB Community Extensions (signed `INSTALL`)
Signed, installable with a plain `INSTALL anofox_visualization FROM community`.
Submit `duckext/description.yml` to
[duckdb/community-extensions](https://github.com/duckdb/community-extensions):
copy it to `extensions/anofox_visualization/description.yml`, fill `repo.ref`,
and open a PR. Their CI builds + signs it.

**Caveat:** the community CI's Rust path expects the standard
`duckdb`/`duckdb-loadable-macros` framework. This extension is a hand-rolled
C-API binding (chosen for wasm side-module viability). To fit the community
build cleanly you'll likely need to either port the entry point to that
framework, or arrange a custom build (Makefile) in the descriptor. Until then,
Route 1 is the reliable path.

## C-API version / portability
The extension requests C-API `v1.2.0` and is packaged with `--abi-type C_STRUCT`.
C-API (`C_STRUCT`) extensions are portable across DuckDB releases that provide
that API version (v1.2.0+), so one build per platform covers many DuckDB
versions — unlike the older C++ ABI, which pins an exact version.
