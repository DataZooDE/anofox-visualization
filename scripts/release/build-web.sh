#!/usr/bin/env bash
# Build the web app as a standalone, platform-independent release asset
# (#417): `dashboard --site` embeds it into a published static site — the
# plan's workaround 7 ("a web build in the release"). Built once (any single
# matrix leg), the same way pages.yml builds the app for GitHub Pages:
# web/pkg (wasm-pack) + web/localext/ (the pinned DuckDB-Wasm extensions,
# scripts/fetch-wasm-extensions.sh) + web/vendor/duckdb-wasm/ (#832, DuckDB-Wasm
# itself, scripts/fetch-duckdb-wasm.sh: a published site loads it from its own
# origin; the builder and Pages still use jsdelivr) + the rest of web/.
#   output: $DIST/anofox-visualization-web-<tag>.tar.gz
set -euo pipefail
# shellcheck source=scripts/release/common.sh
. "$(dirname "$0")/common.sh"

echo "== build web/pkg (wasm) =="
ensure_web_pkg

echo "== fetch web/localext (pinned DuckDB-Wasm extensions) =="
DATAZOO_DISABLE_TELEMETRY=1 "$ROOT/scripts/fetch-wasm-extensions.sh" "$DUCKDB_VERSION"

echo "== fetch web/vendor/duckdb-wasm (pinned DuckDB-Wasm, #832) =="
"$ROOT/scripts/fetch-duckdb-wasm.sh"

mkdir -p "$DIST"
NAME="anofox-visualization-web-$TAG"
STAGE="$DIST/stage/$NAME"
rm -rf "$STAGE"; mkdir -p "$(dirname "$STAGE")"
cp -R "$ROOT/web" "$STAGE"
cd "$DIST/stage"
tar -czf "$DIST/$NAME.tar.gz" "$NAME"
cd "$ROOT"; rm -rf "$DIST/stage"
echo "== done → $DIST/$NAME.tar.gz =="
