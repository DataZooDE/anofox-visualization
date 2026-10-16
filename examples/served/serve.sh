#!/usr/bin/env bash
# Serve the bakery dashboards with the locked server (anofox_serve_dashboards).
#
#   serve.sh start [options]   generate data if needed, start the server (and
#                              the login proxy with --auth); Ctrl-C stops
#     --port N                 server port (default 8910; the proxy uses N+1)
#     --cache-mb N             result cache (default 64; 0 = off)
#     --refresh-s N            rebuild the snapshot every N s (default 60; 0 = only on demand)
#     --auth                   log in through auth-proxy.mjs, rows per user
#                              (needs a server with options.auth, PR #78)
#   serve.sh week              load next week's sales, refresh the snapshot
#   serve.sh refresh           reload the sales files, refresh the snapshot
#   serve.sh run [model]       write a new forecast run into the data folder
#   serve.sh status            /healthz of the running server
#   serve.sh stop              stop the server (and the proxy)
#   serve.sh check             dashboard --check --strict on every dashboard
#
# Environment: DUCKDB (a DuckDB 1.5 CLI; default `duckdb`), ANOFOX_EXT (the
# native extension; default: built into work/ on first start), WORK.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
WORK="${WORK:-$HERE/work}"
RUN="$WORK/run"
DUCKDB="${DUCKDB:-$(command -v duckdb || echo "$HOME/.local/bin/duckdb")}"
export DUCKDB WORK

# A SQL string literal; a JSON string.
q() { printf "'%s'" "${1//\'/\'\'}"; }
j() { local s="${1//\\/\\\\}"; printf '"%s"' "${s//\"/\\\"}"; }
die() { echo "serve.sh: $*" >&2; exit 1; }

port() { cat "$RUN/port" 2>/dev/null || die "no server running (start one with: serve.sh start)"; }

health() { curl -fsS --max-time 5 "http://127.0.0.1:$1/healthz"; }

# Send SQL to the serving session (a FIFO only this user can open).
ctl() {
  [ -p "$RUN/ctl" ] || die "no server running"
  printf '%s\n' "$1" >"$RUN/ctl"
}

# The session's tables: what the snapshot holds.
load_sql() {
  local in="$WORK/incoming"
  cat <<SQL
CREATE OR REPLACE TABLE stores AS FROM read_parquet($(q "$in/stores.parquet"));
CREATE OR REPLACE TABLE products AS FROM read_parquet($(q "$in/products.parquet"));
CREATE OR REPLACE TABLE sales AS FROM read_parquet($(q "$in/sales_*.parquet"));
SQL
}

# Reload the sales files and rebuild the snapshot. anofox_serve_refresh
# returns once the new snapshot serves; the session then writes a marker file
# (and a second one only if the refresh succeeded).
refresh() {
  local p n
  p="$(port)"
  n="$(date +%s%N)"
  ctl "$(load_sql)
COPY (SELECT anofox_serve_refresh($p)) TO $(q "$RUN/refresh-$n.ok") (FORMAT csv, HEADER false);
COPY (SELECT 1) TO $(q "$RUN/refresh-$n.done") (FORMAT csv, HEADER false);"
  for _ in $(seq 240); do
    if [ -e "$RUN/refresh-$n.done" ]; then
      if [ -e "$RUN/refresh-$n.ok" ]; then
        rm -f "$RUN/refresh-$n".*
        echo "snapshot refreshed"; return
      fi
      rm -f "$RUN/refresh-$n".*
      die "the refresh failed (see the server output); the old snapshot still serves"
    fi
    sleep 0.25
  done
  die "the refresh did not finish within 60 s (see the server output)"
}

extension() {
  if [ -n "${ANOFOX_EXT:-}" ]; then
    [ -f "$ANOFOX_EXT" ] || die "ANOFOX_EXT=$ANOFOX_EXT does not exist"
    echo "$ANOFOX_EXT"; return
  fi
  local out="$HERE/work/anofox_visualization.duckdb_extension"
  if [ ! -f "$out" ]; then
    echo "== building the native extension (once) → $out ==" >&2
    OUT="$out" bash "$REPO/duckext/scripts/build-native.sh" --release >&2
  fi
  echo "$out"
}

start() {
  local port=8910 cache_mb=64 refresh_s=60 auth=0
  while [ $# -gt 0 ]; do
    case "$1" in
      --port) port="$2"; shift 2 ;;
      --cache-mb) cache_mb="$2"; shift 2 ;;
      --refresh-s) refresh_s="$2"; shift 2 ;;
      --auth) auth=1; shift ;;
      *) die "unknown option $1" ;;
    esac
  done
  for n in "$port" "$cache_mb" "$refresh_s"; do
    [[ "$n" =~ ^[0-9]+$ ]] || die "not a number: $n"
  done
  [ -x "$DUCKDB" ] || command -v "$DUCKDB" >/dev/null || die "DuckDB CLI not found (set DUCKDB)"
  if [ -e "$RUN/port" ] && health "$(cat "$RUN/port")" >/dev/null 2>&1; then
    die "already running on port $(cat "$RUN/port")"
  fi
  local ext; ext="$(extension)"
  [ -f "$WORK/incoming/stores.parquet" ] || "$HERE/generate.sh" init

  rm -rf "$RUN"
  (umask 077 && mkdir -p "$RUN")
  echo "$port" >"$RUN/port"
  mkfifo -m 600 "$RUN/ctl"

  # The served folder: the dashboards, plus the per-user one with --auth.
  local dash="$HERE/dashboards"
  local opts="\"cache_mb\": $cache_mb, \"data\": $(j "$WORK/data"), \"path_vars\": [\"run\"]"
  [ "$refresh_s" -gt 0 ] && opts="$opts, \"refresh_s\": $refresh_s"
  if [ "$auth" = 1 ]; then
    command -v node >/dev/null || die "--auth needs Node.js (for auth-proxy.mjs)"
    dash="$RUN/dashboards"
    mkdir -p "$dash"
    cp "$HERE"/dashboards/*.sql "$HERE"/auth/*.sql "$dash/"
    # A fresh secret per start, only in the two processes' environments.
    ANOFOX_PROXY_SECRET="$(od -An -N32 -tx1 /dev/urandom | tr -d ' \n')"
    export ANOFOX_PROXY_SECRET
    opts="$opts, \"hosts\": [\"127.0.0.1:$((port + 1))\", \"localhost:$((port + 1))\"],
      \"auth\": {\"mode\": \"proxy\", \"shared_secret_env\": \"ANOFOX_PROXY_SECRET\",
                 \"trusted_proxies\": [\"127.0.0.1\"], \"header_groups\": \"X-Forwarded-Groups\"},
      \"log\": $(j "$RUN/requests.jsonl")"
  fi

  cat >"$RUN/session.sql" <<SQL
.bail on
.mode list
.headers off
LOAD $(q "$ext");
$(load_sql)
-- Who manages which region (used by the per-user dashboard with --auth).
CREATE OR REPLACE TABLE region_managers AS FROM (VALUES
  ('alice', 'North'), ('bob', 'Midlands'), ('bob', 'South')) t(user_name, region);
SELECT anofox_serve_dashboards($(q "$dash"), $port, $(q "{$opts}"));
.bail off
SQL

  PIDS=()
  trap cleanup INT TERM EXIT

  # The session reads further SQL (refresh, stop) from the FIFO; fd 3 keeps
  # it open so the CLI never sees end-of-file.
  exec 3<>"$RUN/ctl"
  "$DUCKDB" -unsigned -batch -noheader -list :memory: -init "$RUN/session.sql" <&3 &
  PIDS+=($!)
  for _ in $(seq 120); do
    health "$port" >/dev/null 2>&1 && break
    kill -0 "${PIDS[0]}" 2>/dev/null || die "the server did not start (see above)"
    sleep 0.5
  done
  health "$port" >/dev/null 2>&1 || die "the server did not answer on port $port"

  if [ "$auth" = 1 ]; then
    UPSTREAM_PORT="$port" PROXY_PORT="$((port + 1))" node "$HERE/auth-proxy.mjs" &
    PIDS+=($!)
    unset ANOFOX_PROXY_SECRET
    echo
    echo "  Log in:      http://127.0.0.1:$((port + 1))/   (alice · bob · carol, an admin)"
    echo "  Request log: $RUN/requests.jsonl"
  else
    echo
    echo "  Dashboards:  http://127.0.0.1:$port/"
  fi
  echo "  Try:         $0 week   ·   $0 run trend   ·   $0 status"
  echo "  Stop:        Ctrl-C (or $0 stop)"
  echo
  wait "${PIDS[0]}" || true
  cleanup
}

# Stop whatever start launched (the session, the proxy) and remove run/.
PIDS=()
cleanup() {
  trap - INT TERM EXIT
  local p
  for p in "${PIDS[@]}"; do kill "$p" 2>/dev/null || true; done
  wait 2>/dev/null || true
  rm -rf "$RUN"
}

stop() {
  local p; p="$(port)"
  ctl "SELECT anofox_serve_stop($p);
.quit"
  for _ in $(seq 40); do [ -e "$RUN" ] || { echo "stopped"; return; }; sleep 0.25; done
  die "the server did not stop"
}

check() {
  command -v cargo >/dev/null || die "check needs cargo (it runs the dashboard CLI)"
  local db="$WORK/check.duckdb" bin="$REPO/target/release/dashboard"
  [ -f "$WORK/incoming/stores.parquet" ] || "$HERE/generate.sh" init
  (cd "$REPO" && cargo build -q --release --bin dashboard)
  rm -f "$db"
  "$DUCKDB" "$db" -c "$(load_sql)
    CREATE TABLE region_managers AS FROM (VALUES ('alice', 'North')) t(user_name, region);"
  # The server sets anofox_data (and, with --auth, the viewer's claims) on
  # every request; the linter gets them as a prelude.
  local fail=0 f tmp="$WORK/check"
  mkdir -p "$tmp"
  for f in "$HERE"/dashboards/*.sql "$HERE"/auth/*.sql; do
    {
      echo "SET VARIABLE anofox_data = $(q "$WORK/data");"
      echo "SET VARIABLE __claim_user = 'alice'; SET VARIABLE __claim_groups = []::VARCHAR[];"
      cat "$f"
    } >"$tmp/$(basename "$f")"
    echo "== $(basename "$f")"
    # The dashboard CLI runs `duckdb`; give it the configured one.
    mkdir -p "$tmp/bin"
    ln -sf "$(command -v "$DUCKDB")" "$tmp/bin/duckdb"
    PATH="$tmp/bin:$PATH" "$bin" --check --strict --db "$db" "$tmp/$(basename "$f")" | tail -n 3 || fail=1
  done
  rm -rf "$tmp" "$db"
  return $fail
}

case "${1:-}" in
  start) shift; start "$@" ;;
  week) "$HERE/generate.sh" week; refresh ;;
  refresh) refresh ;;
  run) shift; "$HERE/generate.sh" run "$@"
       echo "Open it: http://127.0.0.1:$(port)/d/what-will-each-store-sell?v.run=$(ls -t "$WORK/data/runs" | head -1)" ;;
  status) health "$(port)"; echo ;;
  stop) stop ;;
  check) check ;;
  *) sed -n '2,22p' "$0" | sed 's/^# \{0,1\}//'; exit 2 ;;
esac
