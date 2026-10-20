#!/usr/bin/env bash
# Bump the pin in scripts/wasm-extensions.lock (#235): fetch the current
# get.erpl.io DuckDB-Wasm builds, publish them as a GitHub pre-release asset
# set (never "latest" — this is not a release of anofox-visualization), and
# rewrite the lock to point at that release. After this, run
# node scripts/gen-assist-functions.mjs and commit the lock with the
# regenerated web/assist/functions.js.
#
#   usage: scripts/release/pin-wasm-extensions.sh <tag>   tag must start wasm-ext-
#
#   PIN_TARGET   commit/branch to attach the release to (default: HEAD)
#   VER          DuckDB version to pin (default: .duckdb-version)
set -euo pipefail
# shellcheck source=scripts/release/common.sh
. "$(dirname "$0")/common.sh"

TAG="${1:?usage: pin-wasm-extensions.sh <tag> (must start wasm-ext-)}"
case "$TAG" in
  wasm-ext-*) ;;
  *) echo "$TAG: tag must start with wasm-ext- (this is a pinned-build pre-release, not an anofox-visualization release)" >&2; exit 2 ;;
esac
VER="${VER:-$(cat "$ROOT/.duckdb-version")}"
EXTS=(anofox_statistics anofox_forecast)
PLATFORMS=(wasm_eh wasm_mvp)

sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | cut -d' ' -f1; else shasum -a 256 "$1" | cut -d' ' -f1; fi
}

echo "fetching the current get.erpl.io builds for $VER"
"$ROOT/scripts/fetch-wasm-extensions.sh" --latest "$VER"

STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT

for p in "${PLATFORMS[@]}"; do
  for e in "${EXTS[@]}"; do
    src="$ROOT/web/localext/$VER/$p/$e.duckdb_extension.wasm"
    [ -f "$src" ] || { echo "$src: missing after fetch --latest" >&2; exit 1; }
    cp "$src" "$STAGE/$p-$e.duckdb_extension.wasm"
  done
done

echo "creating pre-release $TAG"
gh release create "$TAG" ${PIN_TARGET:+--target "$PIN_TARGET"} --prerelease --latest=false \
  --title "Pinned DuckDB-Wasm extensions ($VER, $(date -u +%F))" \
  --notes "Pinned DuckDB-Wasm extension builds for scripts/fetch-wasm-extensions.sh — not a release of anofox-visualization; scripts/wasm-extensions.lock, #235" \
  "$STAGE"/*.wasm

REPO="$(gh repo view --json nameWithOwner -q .nameWithOwner)"
{
  echo "# The anofox DuckDB-Wasm builds CI, the Pages site and gen-assist-functions.mjs load (#235)."
  echo "# scripts/fetch-wasm-extensions.sh downloads each file and checks its sha256. Bump on purpose:"
  echo "# scripts/release/pin-wasm-extensions.sh <tag>, then node scripts/gen-assist-functions.mjs,"
  echo "# and commit this file with web/assist/functions.js."
  echo "# duckdb platform extension sha256 url"
  for p in "${PLATFORMS[@]}"; do
    for e in "${EXTS[@]}"; do
      sha="$(sha256_of "$STAGE/$p-$e.duckdb_extension.wasm")"
      echo "$VER $p $e $sha https://github.com/$REPO/releases/download/$TAG/$p-$e.duckdb_extension.wasm"
    done
  done
} > "$ROOT/scripts/wasm-extensions.lock"

echo "wrote scripts/wasm-extensions.lock pinned to $TAG"
