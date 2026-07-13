#!/usr/bin/env bash
# Build + package + smoke-test the native anofox-visualization DuckDB extension.
# Produces /tmp/anofox_visualization.duckdb_extension for `duckdb -unsigned`.
set -euo pipefail
HERE="$(cd "$(dirname "$0")/.." && pwd)"
DUCKDB="${DUCKDB:-$HOME/.local/bin/duckdb}"
OUT="${OUT:-/tmp/anofox_visualization.duckdb_extension}"
PLATFORM="${PLATFORM:-$("$DUCKDB" -noheader -list -c 'PRAGMA platform;')}"

echo "== build cdylib =="
cd "$HERE"
CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-8}" cargo build --lib "$@"
SO=target/debug/libanofox_visualization_ext.so
[ -f target/release/libanofox_visualization_ext.so ] && SO=target/release/libanofox_visualization_ext.so
echo "   $SO"

echo "== package (C_STRUCT metadata footer, platform $PLATFORM) =="
python3 "$HERE/scripts/append_extension_metadata.py" -l "$SO" -n anofox_visualization \
  -p "$PLATFORM" -dv v1.2.0 -ev v0.1.0 --abi-type C_STRUCT -o "$OUT" | grep -i "output file"

echo "== smoke test =="
"$DUCKDB" -unsigned -noheader -list -c "
LOAD '$OUT';
WITH d AS (SELECT * FROM (VALUES ('app',30),('web',22),('api',12)) t(ch,n))
SELECT CASE WHEN anofox_render(json_object(
  'rows',  (SELECT to_json(list({ch: ch, n: n})) FROM d),
  'roles', json('[[0,\"XAXIS\"],[1,\"BARCHART\"]]'),
  'width', 400, 'height', 260)) LIKE '<svg%<rect%'
  THEN 'OK: rendered a bar chart' ELSE 'FAIL' END;"
echo "== done → $OUT =="
