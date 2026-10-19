#!/usr/bin/env bash
# Build + package + smoke-test the native anofox-visualization DuckDB extension
# (the C-API "full toolkit" build: rendering + anofox_serve*).
# Produces $OUT (default /tmp/anofox_visualization.duckdb_extension) for
# `duckdb -unsigned`. Extra args go to `cargo build` (e.g. --release).
# Any OUT name builds and smoke-tests, but `LOAD '<file>'` needs the name
# anofox_visualization.duckdb_extension or anofox_visualization.<x>.duckdb_extension
# (DuckDB takes the entry point from the file name up to the first dot, #201).
set -euo pipefail
HERE="$(cd "$(dirname "$0")/.." && pwd)"
CORE="$(cd "$HERE/.." && pwd)"
DUCKDB="${DUCKDB:-$(command -v duckdb || echo "$HOME/.local/bin/duckdb")}"
OUT="${OUT:-/tmp/anofox_visualization.duckdb_extension}"
# Relative to the caller's directory (the build below runs in $CORE).
case "$OUT" in /*|[A-Za-z]:*) ;; *) OUT="$PWD/$OUT" ;; esac
PLATFORM="${PLATFORM:-$("$DUCKDB" -noheader -list -c 'PRAGMA platform;')}"
# Single version source of truth: [workspace.package] in the root Cargo.toml.
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' "$CORE/Cargo.toml" | head -1)"

# The extension embeds web/ at compile time (include_dir!) so `SELECT
# anofox_serve(port)` can serve the browser builder. Build the wasm module first
# so web/pkg is present (REQUIRED unless SKIP_WASM=1, which builds a server
# whose UI cannot render).
if [ "${SKIP_WASM:-0}" != "1" ]; then
  command -v wasm-pack >/dev/null || { echo "wasm-pack is required (or SKIP_WASM=1)"; exit 1; }
  echo "== build web/pkg (wasm) — embedded UI for anofox_serve =="
  ( cd "$CORE" && wasm-pack build --target web --out-dir web/pkg --no-default-features --features wasm )
  touch "$HERE/src/serve.rs"   # force include_dir! to re-embed the fresh web/pkg
else
  echo "== SKIP_WASM=1 — anofox_serve UI won't include the renderer =="
fi

echo "== build cdylib (v$VERSION) =="
cd "$CORE"
TARGET_DIR="${CARGO_TARGET_DIR:-$CORE/target}"
CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-8}" cargo build --locked -p anofox-visualization-duckdb --lib "$@"
PROFILE=debug
for a in "$@"; do [ "$a" = "--release" ] && PROFILE=release; done
LIB=""
for f in libanofox_visualization_ext.so libanofox_visualization_ext.dylib anofox_visualization_ext.dll; do
  [ -f "$TARGET_DIR/$PROFILE/$f" ] && LIB="$TARGET_DIR/$PROFILE/$f"
done
[ -n "$LIB" ] || { echo "built library not found in $TARGET_DIR/$PROFILE"; exit 1; }
echo "   $LIB"

echo "== package (C_STRUCT metadata footer, C-API v1.2.0, platform $PLATFORM) =="
"${PYTHON:-python3}" "$HERE/scripts/append_extension_metadata.py" -l "$LIB" -n anofox_visualization \
  -p "$PLATFORM" -dv v1.2.0 -ev "v$VERSION" --abi-type C_STRUCT -o "$OUT" | grep -i "output file"

echo "== smoke test =="
# LOAD derives <name>_init_c_api from the file name up to the first dot and the
# library exports anofox_visualization_init_c_api only: test any other OUT name
# through a symlink with the canonical name (#201).
case "$(basename "$OUT")" in
  anofox_visualization.duckdb_extension|anofox_visualization.*.duckdb_extension)
    EXT="$OUT"; RENAMED= ;;
  *)
    SMOKE_DIR="$(mktemp -d)"; trap 'rm -rf "$SMOKE_DIR"' EXIT
    EXT="$SMOKE_DIR/anofox_visualization.duckdb_extension"; ln -s "$OUT" "$EXT"; RENAMED=1 ;;
esac
SMOKE="$("$DUCKDB" -unsigned -noheader -list -c "
LOAD '$EXT';
WITH d AS (SELECT * FROM (VALUES ('app',30),('web',22),('api',12)) t(ch,n))
SELECT CASE WHEN anofox_render(json_object(
  'rows',  (SELECT to_json(list({c0: ch, c1: n})) FROM d),
  'roles', json('[[0,\"XAXIS\"],[1,\"BARCHART\"]]'),
  'width', 400, 'height', 260)) LIKE '<svg%<rect%'
  THEN 'OK: rendered a bar chart' ELSE 'FAIL' END;" | tr -d '\r')"
echo "$SMOKE"
[ "$SMOKE" = "OK: rendered a bar chart" ] || { echo "FAIL: the bar-chart smoke render returned '$SMOKE'" >&2; exit 1; }
if "$DUCKDB" -unsigned -c "LOAD '$EXT'; SELECT anofox_render('not json');" >/dev/null 2>&1; then
  echo "FAIL: a bad spec must raise a SQL error"; exit 1
fi
echo "OK: bad spec raises a SQL error"
# #179: after USE <other db> the macros are reached qualified or via search_path (BUILD.md, "macro caveat").
USE_OK="$("$DUCKDB" -unsigned -noheader -list -c "
LOAD '$EXT';
ATTACH ':memory:' AS other; USE other;
CREATE TABLE s AS SELECT 'A' AS model_id, 'mae' AS metric, 1.0 AS value;
SELECT CASE WHEN memory.anofox_plot_summary(s) LIKE '<svg%' THEN 'qualified' ELSE 'FAIL' END FROM s;
SET search_path = 'other.main,memory.main';
SELECT CASE WHEN anofox_plot_summary(s) LIKE '<svg%' THEN 'search_path' ELSE 'FAIL' END FROM s;" | tr -d '\r' | tr '\n' ' ')"
[ "$USE_OK" = "qualified search_path " ] || { echo "FAIL: plot macros after USE: '$USE_OK'" >&2; exit 1; }
echo "OK: plot macros after USE (qualified, search_path)"
echo "== done → $OUT =="
if [ -n "$RENAMED" ]; then
  echo "NOTE: LOAD '$OUT' fails (DuckDB looks for $(basename "$OUT" | cut -d. -f1)_init_c_api)."
  echo "      Rename it to anofox_visualization.duckdb_extension, or LOAD a symlink with that name."
fi
