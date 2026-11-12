#!/usr/bin/env bash
# #1017 P0 (D4): the GitHub Pages site follows the latest release — its own
# web build (the release's anofox-visualization-web-<tag>.tar.gz) at the root —
# and master's build at /next/ (not indexed). The DuckDB extension repository
# goes to /extensions as before (pages.yml writes it into <site>/extensions).
#
#   scripts/release/pages-site.sh <master web dir> <site dir> [<website dist> [<release website dist>]]
#
# #1020 (P1 preview): with a website dist (`website/`, Astro Starlight, built
# with SITE_BASE=/anofox-visualization/next), /next/ is that site and master's
# builder moves to /next/app/.
# #1017 (owner 2026-11-08): with a release website dist (the latest release's
# own website/, built with SITE_BASE=/anofox-visualization), / is that site and
# the release's builder moves to /app/; the landing page forwards old builder
# links (/?dashboard=…, /#sql=…) there. Without one (a release from before the
# website), / stays the release's builder.
# Needs GH_TOKEN / GH_REPO for `gh release`.
set -euo pipefail
web="$1"
site="$2"
mkdir -p "$site"
tag="$(gh release view --json tagName -q .tagName)"
asset="anofox-visualization-web-${tag}.tar.gz"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
reldist="${4:-}"
if gh release download "$tag" -p "$asset" -D "$tmp" 2>/dev/null; then
  if [ -n "$reldist" ] && [ -f "$reldist/index.html" ]; then
    mkdir -p "$site/app"
    tar xzf "$tmp/$asset" -C "$site/app" --strip-components=1
    cp -R "$reldist/." "$site/"
    echo "site: / = release $tag website, its builder at /app/"
  else
    tar xzf "$tmp/$asset" -C "$site" --strip-components=1
    echo "site: / = release $tag"
  fi
else
  # A release without its web build (it is still being built): fail rather
  # than deploy master as if it were the release (cf. #1008).
  echo "::error::release $tag has no $asset yet"
  exit 1
fi
dist="${3:-}"
app="$site/next"
if [ -n "$dist" ]; then
  mkdir -p "$site/next"
  cp -R "$dist/." "$site/next/"
  app="$site/next/app"
fi
mkdir -p "$app"
cp -R "$web/." "$app/"
rm -rf "$app/extensions"
# owner 2026-11-12: the demo builders' dashboards in the website's colours
scripts/release/site-theme.sh website/showcase/site-theme.json \
  "$site/app/index.html" "$app/index.html"
# the preview isn't the product: keep it out of search engines
find "$site/next" -name '*.html' -exec sed -i 's|<head>|<head><meta name="robots" content="noindex">|' {} +
echo "site: /next/ = master $(git rev-parse --short HEAD)${dist:+ (website, builder at /next/app/)}"
