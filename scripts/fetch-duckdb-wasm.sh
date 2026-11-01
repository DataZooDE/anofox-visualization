#!/usr/bin/env bash
# Fetch DuckDB-Wasm into web/vendor/duckdb-wasm/ (#832), so a published site
# (`dashboard --site`) loads it from its own origin instead of jsdelivr: the
# host's edge cache, one TLS connection, and no third-party request behind an
# auth proxy. web/app.js uses it when it's there (published pages) and falls
# back to jsdelivr when it isn't.
#
# What: the four ES modules the page imports (jsdelivr's `+esm` builds of
# @duckdb/duckdb-wasm, apache-arrow, flatbuffers and tslib, with their
# absolute `/npm/…/+esm` imports rewritten to the local files) and the
# exception-handling bundle (worker + wasm, verbatim files of the npm
# package). A browser without wasm exceptions keeps jsdelivr's MVP bundle.
#
# Pinned: every download is checked against scripts/duckdb-wasm.lock
# (sha256, before any rewrite). The files are build output (gitignored); CI
# and scripts/release/build-web.sh run this. After bumping the DuckDB-Wasm
# version in web/app.js, re-pin: scripts/fetch-duckdb-wasm.sh --pin
#
# usage: scripts/fetch-duckdb-wasm.sh [--pin]
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
LOCK="$ROOT/scripts/duckdb-wasm.lock"
OUT="$ROOT/web/vendor/duckdb-wasm"
CDN="https://cdn.jsdelivr.net"
# The versions web/app.js and the +esm builds name.
DUCKDB_WASM="@duckdb/duckdb-wasm@1.33.1-dev65.0"
ARROW="apache-arrow@17.0.0"
FLATBUFFERS="flatbuffers@24.3.25"
TSLIB="tslib@2.6.3"
# local name <tab> URL
FILES=(
  "duckdb-wasm.mjs	$CDN/npm/$DUCKDB_WASM/+esm"
  "apache-arrow.mjs	$CDN/npm/$ARROW/+esm"
  "flatbuffers.mjs	$CDN/npm/$FLATBUFFERS/+esm"
  "tslib.mjs	$CDN/npm/$TSLIB/+esm"
  "duckdb-browser-eh.worker.js	$CDN/npm/$DUCKDB_WASM/dist/duckdb-browser-eh.worker.js"
  "duckdb-eh.wasm	$CDN/npm/$DUCKDB_WASM/dist/duckdb-eh.wasm"
)

sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | cut -d' ' -f1; else shasum -a 256 "$1" | cut -d' ' -f1; fi
}

PIN=0
[ "${1:-}" = "--pin" ] && PIN=1
mkdir -p "$OUT"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
lock_lines=()
for entry in "${FILES[@]}"; do
  name="${entry%%	*}"
  url="${entry#*	}"
  echo "fetch $url"
  curl -fsSL --retry 3 -o "$tmp/$name" "$url"
  sum="$(sha256_of "$tmp/$name")"
  if [ "$PIN" = "1" ]; then
    lock_lines+=("$sum  $name  $url")
  else
    want="$(awk -v n="$name" '$2 == n {print $1}' "$LOCK")"
    if [ -z "$want" ] || [ "$want" != "$sum" ]; then
      echo "::error::$name: sha256 $sum does not match scripts/duckdb-wasm.lock (${want:-missing}) — re-pin on purpose with --pin" >&2
      exit 1
    fi
  fi
done
if [ "$PIN" = "1" ]; then
  {
    echo "# sha256  local-name  url — scripts/fetch-duckdb-wasm.sh (#832); re-pin with --pin"
    printf '%s\n' "${lock_lines[@]}"
  } >"$LOCK"
  echo "pinned → $LOCK"
fi
# The +esm builds import each other by absolute jsdelivr path: point them at
# the local copies.
for f in duckdb-wasm.mjs apache-arrow.mjs flatbuffers.mjs tslib.mjs; do
  sed -e "s#\"/npm/$ARROW/+esm\"#\"./apache-arrow.mjs\"#g" \
      -e "s#\"/npm/$FLATBUFFERS/+esm\"#\"./flatbuffers.mjs\"#g" \
      -e "s#\"/npm/$TSLIB/+esm\"#\"./tslib.mjs\"#g" \
      "$tmp/$f" >"$OUT/$f"
  if grep -q '"/npm/' "$OUT/$f"; then
    echo "::error::$f still imports from /npm/ — a dependency this script doesn't know" >&2
    exit 1
  fi
done
cp "$tmp/duckdb-browser-eh.worker.js" "$tmp/duckdb-eh.wasm" "$OUT/"
echo "$DUCKDB_WASM" >"$OUT/VERSION"
echo "done → $OUT"
