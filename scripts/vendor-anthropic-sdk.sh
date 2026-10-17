#!/usr/bin/env bash
# Vendor the Anthropic TypeScript SDK as one ES module for the builder's
# assistant (web/assist.js loads web/vendor/anthropic-sdk-<version>.mjs).
# Why vendored, not a CDN import: the SDK runs with the user's API key and
# the page's data, a dynamic import() can't carry Subresource Integrity, and
# jsDelivr's +esm bundles are built on demand (it advises against SRI on
# them). The build is reproducible: the same version and esbuild give the
# same bytes; web/vendor/SHA256SUMS pins them and tests/web/assist_test.mjs
# checks the file against it.
#
# usage: scripts/vendor-anthropic-sdk.sh   (the version in tests/web/package.json)
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
VER="$(node -p "require('$ROOT/tests/web/package.json').dependencies['@anthropic-ai/sdk']")"
ESBUILD="esbuild@0.25.10"
npm install --prefix "$ROOT/tests/web" --no-audit --no-fund >/dev/null
OUT="web/vendor/anthropic-sdk-$VER.mjs"
(cd "$ROOT/tests/web" && npx --yes "$ESBUILD" node_modules/@anthropic-ai/sdk/index.mjs \
  --bundle --format=esm --platform=browser --minify --legal-comments=eof --outfile="$ROOT/$OUT")
(cd "$ROOT" && sha256sum "$OUT" > web/vendor/SHA256SUMS)
cat "$ROOT/web/vendor/SHA256SUMS"
