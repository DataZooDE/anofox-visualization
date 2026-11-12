---
title: Install
description: The community extension, the full toolkit from a release, or a source build.
---

## The community extension (rendering)

```sql
INSTALL anofox_visualization FROM community;
LOAD anofox_visualization;

WITH sales AS (SELECT * FROM (VALUES ('app', 30), ('web', 22), ('api', 12)) t(ch, n))
SELECT anofox_bar(ch, n) FROM sales;   -- one <svg> per group
```

It provides `anofox_render` and the `anofox_bar` / `_line` / `_scatter` / `_area` / `_xy` / `_xyc` macros. Serving and the builder are in the full toolkit.

## The full toolkit (from a release)

Every [release](https://github.com/DataZooDE/anofox-visualization/releases) carries the extension with the servers and the embedded builder, the `dashboard` and `serve` CLIs, for Linux (x86_64, arm64), macOS (Apple silicon, Intel) and Windows, with a signed `SHA256SUMS`.

```sh
TAG=v2026.11.12 PLATFORM=linux_amd64      # linux_arm64, osx_arm64, osx_amd64, windows_amd64
EXT=anofox_visualization.$TAG.duckdb-v1.5.6.$PLATFORM.duckdb_extension
curl -fLO https://github.com/DataZooDE/anofox-visualization/releases/download/$TAG/$EXT
curl -fLO https://github.com/DataZooDE/anofox-visualization/releases/download/$TAG/SHA256SUMS
sha256sum --check --ignore-missing SHA256SUMS
duckdb -unsigned -c "LOAD './$EXT'; SELECT anofox_contract_version();"
```

The release extensions are unsigned: start DuckDB with `-unsigned` (or `SET allow_unsigned_extensions = true`). The site also serves them as a DuckDB extension repository:

```sql
SET allow_unsigned_extensions = true;
INSTALL anofox_visualization FROM 'https://datazoode.github.io/anofox-visualization/extensions';
```

## From source

See [Installation → From source](https://github.com/DataZooDE/anofox-visualization/blob/master/README.md#from-source): a Rust toolchain and `duckext/scripts/build-native.sh`.
