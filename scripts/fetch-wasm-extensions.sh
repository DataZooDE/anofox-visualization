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
# Pinning (#235): by default this script downloads the builds pinned in
# scripts/wasm-extensions.lock — the ones gen-assist-functions.mjs generated
# web/assist/functions.js from — and checks each file's sha256 against the
# lock, so CI and the Pages build never silently pick up a new get.erpl.io
# build that the assistant's function lists haven't been reviewed against.
# To bump the pin on purpose: scripts/release/pin-wasm-extensions.sh <tag>,
# then node scripts/gen-assist-functions.mjs, and commit
# scripts/wasm-extensions.lock with web/assist/functions.js.
#
# usage: scripts/fetch-wasm-extensions.sh [duckdb-version]       pinned (default)
#        scripts/fetch-wasm-extensions.sh --latest [duckdb-version]
#                                                                 straight from get.erpl.io, unpinned
set -euo pipefail
export DATAZOO_DISABLE_TELEMETRY=1  # no usage telemetry from anofox extensions, whatever was inherited
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
LOCK="$ROOT/scripts/wasm-extensions.lock"
EXTS=(anofox_statistics anofox_forecast)
PLATFORMS=(wasm_eh wasm_mvp)

sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | cut -d' ' -f1; else shasum -a 256 "$1" | cut -d' ' -f1; fi
}

LATEST=0
if [ "${1:-}" = "--latest" ]; then LATEST=1; shift; fi
VER="${1:-$(cat "$ROOT/.duckdb-version")}"

if [ "$LATEST" = "1" ]; then
  BASE="${ANOFOX_EXT_BASE:-https://get.erpl.io}"
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
  exit 0
fi

matched=0
while IFS=' ' read -r ver p e sum url; do
  case "$ver" in ''|'#'*) continue ;; esac
  [ "$ver" = "$VER" ] || continue
  matched=1
  dir="$ROOT/web/localext/$ver/$p"
  mkdir -p "$dir"
  f="$dir/$e.duckdb_extension.wasm"
  if [ -f "$f" ] && [ "$(sha256_of "$f")" = "$sum" ]; then
    echo "$f: sha256 matches scripts/wasm-extensions.lock, skipping"
    continue
  fi
  echo "fetch $url"
  curl -fsSL --retry 3 -o "$f.part" "$url"
  got="$(sha256_of "$f.part")"
  if [ "$got" != "$sum" ]; then
    rm -f "$f.part"
    echo "$url: sha256 $got, but scripts/wasm-extensions.lock pins $sum" >&2
    exit 1
  fi
  # Pinned mode doesn't strip: the release assets are already stripped
  # (scripts/release/pin-wasm-extensions.sh ran --latest, which does strip).
  mv "$f.part" "$f"
done < "$LOCK"

if [ "$matched" = "0" ]; then
  echo "scripts/wasm-extensions.lock pins no builds for DuckDB $VER: run scripts/release/pin-wasm-extensions.sh" >&2
  exit 1
fi
