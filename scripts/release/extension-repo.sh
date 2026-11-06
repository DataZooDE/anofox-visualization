#!/usr/bin/env bash
# Lay out release extension assets as a DuckDB custom extension repository:
#   <out>/<duckdb version>/<platform>/anofox_visualization.duckdb_extension.gz
# which `INSTALL anofox_visualization FROM '<base url>'` fetches (unsigned:
# `duckdb -unsigned` / SET allow_unsigned_extensions = true).
#
#   usage: scripts/release/extension-repo.sh <assets-dir> <out-dir>
#          scripts/release/extension-repo.sh --release <tag|latest> <out-dir>
# --release downloads the assets of a GitHub release with `gh` (GH_TOKEN).
# It FAILS unless the release has a complete set of extensions: SHA256SUMS
# and every extension it lists, checksums verified. pages.yml then deploys
# nothing and the previous, complete repository stays live; a warn-and-deploy
# here once published the site without /extensions mid-release (#1008).
#   EXT_REPO_WAIT=<s>  keep polling an incomplete release this long first (a
#                      release just (re)created is still uploading); default 0
#   EXT_REPO_POLL=<s>  seconds between polls; default 30
# The one exception: no published release at all (`latest` only, first
# bootstrap) is a warning and an empty <out-dir>, so the web app still deploys.
set -euo pipefail
export DATAZOO_DISABLE_TELEMETRY=1  # no usage telemetry from anofox extensions, whatever was inherited
# shellcheck source=scripts/release/common.sh
. "$(dirname "$0")/common.sh"

EXT_GLOB='anofox_visualization.*.duckdb_extension'

# The extension assets SHA256SUMS lists, one per line (sha256sum's binary
# marker `*` stripped).
listed_extensions() {
  awk '{ n = $2; sub(/^\*/, "", n); print n }' "$1" | grep -E '^anofox_visualization\..+\.duckdb_extension$' || true
}

# What release $REL still lacks, one item per line; nothing when complete.
# upload-release.sh writes SHA256SUMS over every asset, so it is the list of
# what a complete release holds.
missing_assets() {
  local names sums exts e
  names="$(gh release view "$REL" --json assets -q '.assets[].name' 2>/dev/null)" || { echo "the release itself"; return 0; }
  grep -qxF SHA256SUMS <<<"$names" || { echo SHA256SUMS; return 0; }
  sums="$SRC/SHA256SUMS"
  gh release download "$REL" --pattern SHA256SUMS --output "$sums" --clobber >/dev/null 2>&1 || { echo SHA256SUMS; return 0; }
  exts="$(listed_extensions "$sums")"
  [ -n "$exts" ] || { echo "any extension (SHA256SUMS lists none)"; return 0; }
  while IFS= read -r e; do grep -qxF "$e" <<<"$names" || echo "$e"; done <<<"$exts"
}

if [ "${1:-}" = "--release" ]; then
  REL="${2:?--release needs a tag or 'latest'}"; OUT="${3:?usage: extension-repo.sh --release <tag|latest> <out-dir>}"
  WAIT="${EXT_REPO_WAIT:-0}"; POLL="${EXT_REPO_POLL:-30}"
  SRC="$(mktemp -d)"; trap 'rm -rf "$SRC"' EXIT
  if [ "$REL" = "latest" ]; then
    if ! REL="$(gh release view --json tagName -q .tagName 2>/dev/null)"; then
      published="$(gh release list --exclude-drafts --exclude-pre-releases --limit 1 --json tagName -q '.[].tagName')" \
        || { echo "::error::cannot list the releases — extension repository not published"; exit 1; }
      if [ -z "$published" ]; then
        echo "::warning::no published release yet — extension repository not published (first bootstrap)"
        exit 0
      fi
      echo "::error::cannot resolve the latest release — extension repository not published"; exit 1
    fi
  fi
  deadline=$((SECONDS + WAIT))
  while missing="$(missing_assets)"; [ -n "$missing" ]; do
    missing="$(tr '\n' ' ' <<<"$missing")"
    if [ "$SECONDS" -ge "$deadline" ]; then
      echo "::error::release '$REL' has no complete set of extension assets (missing: ${missing% }) — failing instead of deploying a site without /extensions; the previous deployment stays live (#1008)"
      exit 1
    fi
    echo "release '$REL' is incomplete (missing: ${missing% }); polling again in ${POLL}s"
    sleep "$POLL"
  done
  gh release download "$REL" --dir "$SRC" --pattern "$EXT_GLOB" --clobber
  (cd "$SRC" && awk '{ n = $2; sub(/^\*/, "", n) } n ~ /^anofox_visualization\..+\.duckdb_extension$/' SHA256SUMS | sha256sum -c --quiet) \
    || { echo "::error::extension assets of release '$REL' do not match its SHA256SUMS"; exit 1; }
  echo "== release $REL: extensions complete, checksums verified =="
else
  SRC="${1:?usage: extension-repo.sh <assets-dir> <out-dir>}"; OUT="${2:?usage: extension-repo.sh <assets-dir> <out-dir>}"
fi

n=0
for f in "$SRC"/$EXT_GLOB; do
  [ -e "$f" ] || continue
  # anofox_visualization.<tag>.duckdb-<duckdb version>.<platform>.duckdb_extension
  base="$(basename "$f" .duckdb_extension)"
  platform="${base##*.}"
  rest="${base%.*}"
  duckdb_version="${rest##*.duckdb-}"
  case "$duckdb_version" in v*) ;; *) echo "skip $f (unexpected name)"; continue ;; esac
  dir="$OUT/$duckdb_version/$platform"
  mkdir -p "$dir"
  gzip -9 -n -c "$f" > "$dir/anofox_visualization.duckdb_extension.gz"
  echo "  $duckdb_version/$platform/anofox_visualization.duckdb_extension.gz"
  n=$((n + 1))
done
[ "$n" -gt 0 ] || { echo "::error::no extension assets found in $SRC"; exit 1; }
echo "== $n extension(s) → $OUT =="
