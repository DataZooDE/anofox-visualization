#!/usr/bin/env bash
# Build, smoke-test and package the CLI binaries for this machine:
#   dashboard (static HTML / --check linter) and serve (--features serve, with
#   the builder UI embedded) + README.md + LICENSE
#   output: $DIST/anofox-visualization-cli-<tag>-<rust target>.tar.gz  (.zip on Windows)
#   env:    DUCKDB (CLI used by `dashboard --check`; default: downloaded)
set -euo pipefail
# shellcheck source=scripts/release/common.sh
. "$(dirname "$0")/common.sh"

DUCKDB="${DUCKDB:-$("$ROOT/scripts/release/install-duckdb-cli.sh")}"
TARGET="$(rust_target)"
NAME="anofox-visualization-cli-$TAG-$TARGET"
TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
if command -v cygpath >/dev/null 2>&1; then TARGET_DIR="$(cygpath -m "$TARGET_DIR")"; fi

echo "== build dashboard + serve ($TARGET) =="
ensure_web_pkg
touch "$ROOT/src/bin/serve.rs"   # re-embed the current web/ (include_dir!)
( cd "$ROOT" && cargo build --locked --release --features serve --bin dashboard --bin serve )
BIN="$TARGET_DIR/release"

echo "== smoke =="
# dashboard/serve run queries through `duckdb` on PATH.
# On Windows (Git Bash) $DUCKDB is a C:\… path: bash's dirname can't split it,
# so convert to a POSIX path first or PATH would gain a bogus entry.
if command -v cygpath >/dev/null 2>&1; then
  DUCKDB_DIR="$(dirname "$(cygpath -u "$DUCKDB")")"
else
  DUCKDB_DIR="$(dirname "$DUCKDB")"
fi
PATH="$DUCKDB_DIR:$PATH"; export PATH
command -v duckdb >/dev/null || { echo "FAIL: duckdb not on PATH ($DUCKDB_DIR)"; exit 1; }
# Capture output before matching: `cmd | grep -q` lets grep exit on the first
# match, the CLI then dies writing the rest (EPIPE) and pipefail fails the step.
out() { "$@" 2>&1 | tr -d '\r'; }
DV="$(out "$BIN/dashboard$EXE" --version)"; SV="$(out "$BIN/serve$EXE" --version)"
DH="$(out "$BIN/dashboard$EXE" --help)";    SH="$(out "$BIN/serve$EXE" --help)"
[ "$DV" = "dashboard $VERSION" ] || { echo "FAIL: dashboard --version: $DV"; exit 1; }
[ "$SV" = "serve $VERSION" ]     || { echo "FAIL: serve --version: $SV"; exit 1; }
case "$DH" in *--check*) ;; *) echo "FAIL: dashboard --help"; exit 1 ;; esac
case "$SH" in *--dashboards*) ;; *) echo "FAIL: serve --help"; exit 1 ;; esac
echo "OK: --version / --help ($VERSION)"
"$BIN/dashboard$EXE" --check "$ROOT/dashboards/sessions.sql"
echo "OK: dashboard --check dashboards/sessions.sql"
if [ "${SKIP_WASM:-0}" != "1" ]; then
  grep -aq "pkg/anofox_visualization_bg.wasm" "$BIN/serve$EXE" || { echo "FAIL: serve lacks the builder UI"; exit 1; }
  echo "OK: serve embeds the builder UI (web/pkg)"
fi

echo "== package =="
STAGE="$DIST/stage/$NAME"
rm -rf "$STAGE"; mkdir -p "$STAGE"
cp "$BIN/dashboard$EXE" "$BIN/serve$EXE" "$ROOT/README.md" "$ROOT/LICENSE" "$STAGE/"
cd "$DIST/stage"
case "$PLATFORM" in
  windows_*) rm -f "$DIST/$NAME.zip"; "$PYTHON" -m zipfile -c "$DIST/$NAME.zip" "$NAME"; OUT="$DIST/$NAME.zip" ;;
  *) tar -czf "$DIST/$NAME.tar.gz" "$NAME"; OUT="$DIST/$NAME.tar.gz" ;;
esac
cd "$ROOT"; rm -rf "$DIST/stage"
echo "== done → $OUT =="
