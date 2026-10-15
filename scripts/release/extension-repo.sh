#!/usr/bin/env bash
# Lay out release extension assets as a DuckDB custom extension repository:
#   <out>/<duckdb version>/<platform>/anofox_visualization.duckdb_extension.gz
# which `INSTALL anofox_visualization FROM '<base url>'` fetches (unsigned:
# `duckdb -unsigned` / SET allow_unsigned_extensions = true).
#
#   usage: scripts/release/extension-repo.sh <assets-dir> <out-dir>
#          scripts/release/extension-repo.sh --release <tag|latest> <out-dir>
# --release downloads the assets of a GitHub release with `gh` (GH_TOKEN);
# a missing release or no extension assets is a warning, not an error, so the
# Pages deploy still publishes the web app before the first release exists.
set -euo pipefail
# shellcheck source=scripts/release/common.sh
. "$(dirname "$0")/common.sh"

if [ "${1:-}" = "--release" ]; then
  REL="${2:?--release needs a tag or 'latest'}"; OUT="${3:?usage: extension-repo.sh --release <tag|latest> <out-dir>}"
  SRC="$(mktemp -d)"; trap 'rm -rf "$SRC"' EXIT
  args=(); [ "$REL" = "latest" ] || args=("$REL")
  if ! gh release download "${args[@]}" --dir "$SRC" --pattern 'anofox_visualization.*.duckdb_extension'; then
    echo "::warning::no extension assets in release '$REL' — extension repository not published"
    exit 0
  fi
else
  SRC="${1:?usage: extension-repo.sh <assets-dir> <out-dir>}"; OUT="${2:?usage: extension-repo.sh <assets-dir> <out-dir>}"
fi

n=0
for f in "$SRC"/anofox_visualization.*.duckdb_extension; do
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
[ "$n" -gt 0 ] || { echo "::warning::no extension assets found in $SRC"; exit 0; }
echo "== $n extension(s) → $OUT =="
