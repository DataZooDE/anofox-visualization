#!/bin/sh
# Run a locked dashboard server in the foreground: the DuckDB CLI executes
# SERVE_SQL (LOAD the extension, ATTACH data, anofox_serve_dashboards(...)) and
# then waits on a stdin that never ends, so the servers keep running until the
# process gets SIGTERM/SIGINT (systemd stop, docker stop, Ctrl-C).
#
#   run-locked.sh serve.sql [database file]
#
# DUCKDB          the DuckDB CLI (default: duckdb on PATH); it must match the
#                 version the extension was built for. The extension is
#                 unsigned: -unsigned.
# ANOFOX_CONTROL  optional path of a control FIFO. SQL written to it runs in the
#                 serving session, e.g.
#                   echo 'SELECT anofox_serve_refresh(8080);' > "$ANOFOX_CONTROL"
#                 Whoever can write to it can run any SQL in that session: it is
#                 created 0600, owned by the server's user.
set -eu
serve_sql=${1:?usage: run-locked.sh serve.sql [database file]}
shift
# A FIFO opened read-write never delivers data unless someone writes to it, and
# never reaches EOF.
if [ -n "${ANOFOX_CONTROL:-}" ]; then
  hold=$ANOFOX_CONTROL
  rm -f "$hold"
  mkfifo -m 600 "$hold"
  exec 0<>"$hold"
else
  hold=$(mktemp -u "${TMPDIR:-/tmp}/anofox-hold.XXXXXX")
  mkfifo -m 600 "$hold"
  exec 0<>"$hold"
  rm -f "$hold"
fi
# An error in SERVE_SQL ends the CLI with status 1 (no -bail needed, which would
# also end it on a failing control statement).
exec "${DUCKDB:-duckdb}" -unsigned -init "$serve_sql" "$@"
