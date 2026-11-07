#!/usr/bin/env bash
# #1017 P0 (D4): the GitHub Pages site follows the latest release — its own
# web build (the release's anofox-visualization-web-<tag>.tar.gz) at the root —
# and master's build at /next/ (not indexed). The DuckDB extension repository
# goes to /extensions as before (pages.yml writes it into <site>/extensions).
#
#   scripts/release/pages-site.sh <master web dir> <site dir>
# Needs GH_TOKEN / GH_REPO for `gh release`.
set -euo pipefail
web="$1"
site="$2"
mkdir -p "$site"
tag="$(gh release view --json tagName -q .tagName)"
asset="anofox-visualization-web-${tag}.tar.gz"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
if gh release download "$tag" -p "$asset" -D "$tmp" 2>/dev/null; then
  tar xzf "$tmp/$asset" -C "$site" --strip-components=1
  echo "site: / = release $tag"
else
  # A release without its web build (it is still being built): fail rather
  # than deploy master as if it were the release (cf. #1008).
  echo "::error::release $tag has no $asset yet"
  exit 1
fi
mkdir -p "$site/next"
cp -R "$web/." "$site/next/"
rm -rf "$site/next/extensions"
# the preview isn't the product: keep it out of search engines
sed -i 's|<head>|<head>\n<meta name="robots" content="noindex">|' "$site/next/index.html"
echo "site: /next/ = master $(git rev-parse --short HEAD)"
