#!/usr/bin/env bash
# Fetch the anofox DuckDB-Wasm extensions the browser demo loads and serve them
# same-origin from web/localext/<duckdb version>/<platform>/ (the app points
# custom_extension_repository there).
#
# Why not `INSTALL … FROM 'https://get.erpl.io'` in the browser:
#   * get.erpl.io sends no CORS headers, so a page on another origin can't fetch;
#   * its v1.5.6 wasm builds carry a duplicated duckdb_signature section
#     (DataZooDE/anofox-statistics#192, DataZooDE/anofox-forecast#272) that makes
#     LOAD fail — scripts/strip_wasm_signature.py removes it (only when present).
# TODO: switch the app to INSTALL … FROM 'https://get.erpl.io' and drop this
# script once #192/#272 and CORS are fixed upstream.
#
# wasm_eh + wasm_mvp only: wasm_threads needs COOP/COEP headers GitHub Pages
# can't set. The files are build output (gitignored); CI runs this before upload.
#
# usage: scripts/fetch-wasm-extensions.sh [duckdb-version]   (default v1.5.6)
set -euo pipefail
export DATAZOO_DISABLE_TELEMETRY="${DATAZOO_DISABLE_TELEMETRY:-1}"  # no usage telemetry from anofox extensions
VER="${1:-v1.5.6}"
BASE="${ANOFOX_EXT_BASE:-https://get.erpl.io}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
EXTS=(anofox_statistics anofox_forecast)
PLATFORMS=(wasm_eh wasm_mvp)
for p in "${PLATFORMS[@]}"; do
  dir="$ROOT/web/localext/$VER/$p"
  mkdir -p "$dir"
  for e in "${EXTS[@]}"; do
    f="$dir/$e.duckdb_extension.wasm"
    echo "fetch $BASE/$VER/$p/$e.duckdb_extension.wasm"
    curl -fsSL --compressed --retry 3 -o "$f.part" "$BASE/$VER/$p/$e.duckdb_extension.wasm"
    mv "$f.part" "$f"
    python3 "$ROOT/scripts/strip_wasm_signature.py" "$f"
  done
done
