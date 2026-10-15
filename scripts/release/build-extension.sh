#!/usr/bin/env bash
# Build the full-toolkit extension for this machine's platform as a release
# asset: web/pkg (wasm-pack, embedded builder UI) + release cdylib + metadata
# footer (duckext/scripts/build-native.sh), then the release smoke test.
#   output: $DIST/anofox_visualization-<tag>-duckdb-<duckdb>-<platform>.duckdb_extension
#   env:    DUCKDB (CLI to test with; default: downloaded by install-duckdb-cli.sh)
#           SKIP_WASM=1 for a quick local build without the UI (never in a release)
set -euo pipefail
# shellcheck source=scripts/release/common.sh
. "$(dirname "$0")/common.sh"

DUCKDB="${DUCKDB:-$("$ROOT/scripts/release/install-duckdb-cli.sh")}"
mkdir -p "$DIST"
OUT="$DIST/$(ext_asset_name)"
echo "== $OUT =="
DUCKDB="$DUCKDB" PLATFORM="$PLATFORM" OUT="$OUT" PYTHON="$PYTHON" \
  "$ROOT/duckext/scripts/build-native.sh" --release
DUCKDB="$DUCKDB" "$ROOT/scripts/release/smoke-extension.sh" "$OUT"
