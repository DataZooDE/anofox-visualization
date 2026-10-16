#!/bin/sh
# Run a locked dashboard server in the foreground: the DuckDB CLI executes
# SERVE_SQL (LOAD the extension, ATTACH data, anofox_serve_dashboards(...)) and
# then waits on a stdin that never ends, so the servers keep running until the
# process gets SIGTERM/SIGINT (systemd stop, docker stop, Ctrl-C).
#
#   run-locked.sh serve.sql [database file]
#
# DUCKDB  the DuckDB CLI (default: duckdb on PATH); it must match the version
#         the extension was built for. The extension is unsigned: -unsigned.
set -eu
serve_sql=${1:?usage: run-locked.sh serve.sql [database file]}
shift
# A FIFO opened read-write never delivers data and never reaches EOF.
hold=$(mktemp -u "${TMPDIR:-/tmp}/anofox-hold.XXXXXX")
mkfifo -m 600 "$hold"
exec 0<>"$hold"
rm -f "$hold"
exec "${DUCKDB:-duckdb}" -unsigned -bail -init "$serve_sql" "$@"
