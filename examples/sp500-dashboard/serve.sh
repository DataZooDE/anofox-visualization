#!/usr/bin/env bash
# Serve the S&P 500 example: build the data layer from the CSVs (init.sql), then
# hand the dash/ folder to anofox_serve_dashboards — locked, read-only.
#
#   ./serve.sh                 # → http://127.0.0.1:8123/
#   PORT=9000 ./serve.sh
#   EXT=/path/to/ext ./serve.sh
#
# Needs the DuckDB CLI and the anofox-visualization extension. Build the
# extension once with ../../duckext/scripts/build-native.sh (→ the default EXT
# path below), or point EXT at your own copy.
set -u
cd "$(dirname "$0")"

DUCKDB="${DUCKDB:-$(command -v duckdb || echo "$HOME/.local/bin/duckdb")}"
EXT="${EXT:-/tmp/anofox_visualization.duckdb_extension}"
PORT="${PORT:-8123}"

[ -x "$DUCKDB" ] || { echo "duckdb not found (set DUCKDB=…)"; exit 1; }
[ -f "$EXT" ]    || { echo "extension not found at $EXT — build it with ../../duckext/scripts/build-native.sh, or set EXT=…"; exit 1; }

echo "building data layer + serving dash/ on :$PORT (Ctrl-C to stop)…"
# anofox_serve_dashboards returns immediately; keep the session alive with a
# tailing FIFO so the embedded HTTP server stays up.
( cat <<SQL
LOAD '$EXT';
.read init.sql
SELECT anofox_serve_dashboards('dash', $PORT);
SQL
  tail -f /dev/null ) | "$DUCKDB" -unsigned
