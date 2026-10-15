#!/usr/bin/env bash
# Download the DuckDB CLI ($DUCKDB_VERSION from .duckdb-version) for $PLATFORM
# into DIR (default $ROOT/.cache/duckdb-<version>-<platform>) and print the
# binary's path on stdout.
#   usage: scripts/release/install-duckdb-cli.sh [DIR]
set -euo pipefail
# shellcheck source=scripts/release/common.sh
. "$(dirname "$0")/common.sh"

DIR="${1:-$ROOT/.cache/duckdb-$DUCKDB_VERSION-$PLATFORM}"
BIN="$DIR/duckdb$EXE"
if [ -x "$BIN" ] && "$BIN" --version 2>/dev/null | grep -q "^$DUCKDB_VERSION "; then
  echo "$BIN"; exit 0
fi
mkdir -p "$DIR"
ZIP="duckdb_cli-${PLATFORM/_/-}.zip"
URL="https://github.com/duckdb/duckdb/releases/download/$DUCKDB_VERSION/$ZIP"
echo "downloading $URL" >&2
curl -fsSL --retry 3 -o "$DIR/$ZIP" "$URL"
"$PYTHON" -m zipfile -e "$DIR/$ZIP" "$DIR" >&2
rm -f "$DIR/$ZIP"
chmod +x "$BIN"
"$BIN" --version >&2
echo "$BIN"
