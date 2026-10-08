#!/usr/bin/env bash
# Build + package + smoke-test the native anofox-visualization DuckDB extension
# (the C-API "full toolkit" build: rendering + anofox_serve*).
# Produces $OUT (default /tmp/anofox_visualization.duckdb_extension) for
# `duckdb -unsigned`. Extra args go to `cargo build` (e.g. --release).
set -euo pipefail
HERE="$(cd "$(dirname "$0")/.." && pwd)"
CORE="$(cd "$HERE/.." && pwd)"
DUCKDB="${DUCKDB:-$(command -v duckdb || echo "$HOME/.local/bin/duckdb")}"
OUT="${OUT:-/tmp/anofox_visualization.duckdb_extension}"
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
python3 "$HERE/scripts/append_extension_metadata.py" -l "$LIB" -n anofox_visualization \
  -p "$PLATFORM" -dv v1.2.0 -ev "v$VERSION" --abi-type C_STRUCT -o "$OUT" | grep -i "output file"

echo "== smoke test =="
"$DUCKDB" -unsigned -noheader -list -c "
LOAD '$OUT';
WITH d AS (SELECT * FROM (VALUES ('app',30),('web',22),('api',12)) t(ch,n))
SELECT CASE WHEN anofox_render(json_object(
  'rows',  (SELECT to_json(list({ch: ch, n: n})) FROM d),
  'roles', json('[[0,\"XAXIS\"],[1,\"BARCHART\"]]'),
  'width', 400, 'height', 260)) LIKE '<svg%<rect%'
  THEN 'OK: rendered a bar chart' ELSE 'FAIL' END;"
if "$DUCKDB" -unsigned -c "LOAD '$OUT'; SELECT anofox_render('not json');" >/dev/null 2>&1; then
  echo "FAIL: a bad spec must raise a SQL error"; exit 1
fi
echo "OK: bad spec raises a SQL error"
echo "== done → $OUT =="
