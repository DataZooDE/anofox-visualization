#!/usr/bin/env bash
# Write SHA256SUMS for every asset in DIR and upload them all to the GitHub
# release TAG (creating the release if only the tag exists). Idempotent:
# re-runs replace assets (--clobber).
#   usage: GH_TOKEN=... scripts/release/upload-release.sh <tag> <dir>
#   DRY_RUN=1 only writes SHA256SUMS and prints what would be uploaded.
set -euo pipefail
TAG="${1:?usage: upload-release.sh <tag> <dir>}"
DIR="${2:?usage: upload-release.sh <tag> <dir>}"

cd "$DIR"
# SHA256SUMS lists every asset but itself and its signature
# (SHA256SUMS.sigstore.json, #766): writing it again over the same assets
# gives the same file, so a signature made in between stays valid.
mapfile -t assets < <(find . -maxdepth 1 -type f ! -name 'SHA256SUMS*' -printf '%f\n' | LC_ALL=C sort)
[ "${#assets[@]}" -gt 0 ] || { echo "no assets in $DIR"; exit 1; }
sha256sum "${assets[@]}" > SHA256SUMS.new
if [ -f SHA256SUMS ] && ! cmp -s SHA256SUMS SHA256SUMS.new; then
  [ -f SHA256SUMS.sigstore.json ] && { echo "SHA256SUMS changed after it was signed"; exit 1; }
fi
mv SHA256SUMS.new SHA256SUMS
cat SHA256SUMS
assets+=(SHA256SUMS)
[ -f SHA256SUMS.sigstore.json ] && assets+=(SHA256SUMS.sigstore.json)

if [ "${DRY_RUN:-0}" = "1" ]; then
  echo "DRY_RUN: would upload ${#assets[@]} assets to release $TAG"; exit 0
fi

if ! gh release view "$TAG" >/dev/null 2>&1; then
  echo "release $TAG not found — creating it from the existing tag"
  gh release create "$TAG" --verify-tag --title "$TAG" --generate-notes
fi
gh release upload "$TAG" "${assets[@]}" --clobber
echo "uploaded ${#assets[@]} assets to $TAG"
