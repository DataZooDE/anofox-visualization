#!/usr/bin/env bash
# End-to-end checks of a locked server (anofox_serve_dashboards) through the
# DuckDB CLI and curl:
#   * a `duckdb -readonly` session serves its data (#104);
#   * `search_path` (and the caller's `USE` passed through it) decides where
#     the panels' unqualified names resolve (#104);
#   * per-response size cap (413), query slots (503 + Retry-After) and the
#     limits in /healthz (#103).
# usage: duckext/scripts/test-locked-server.sh <extension> [port]
#        (DUCKDB=… picks the CLI; default: duckdb on PATH)
set -euo pipefail
EXT="$(cd "$(dirname "$1")" && pwd)/$(basename "$1")"
PORT="${2:-8983}"
DUCKDB="${DUCKDB:-duckdb}"
T="$(mktemp -d)"
PIDS=()
cleanup() {
  for p in "${PIDS[@]}"; do kill "$p" 2>/dev/null || true; done
  rm -rf "$T"
}
trap cleanup EXIT
fail() { echo "FAIL: $*" >&2; exit 1; }
q() { printf "'%s'" "${1//\'/\'\'}"; }

mkdir "$T/dash"
cat > "$T/dash/t.sql" <<'SQL'
SELECT count(*)::BIGINT AS n ::METRIC FROM t;
SELECT x::BIGINT AS x ::METRIC FROM u;
SELECT repeat('x', 1000) AS s ::TABLE FROM range(3000);
SELECT sum(i)::BIGINT AS s ::METRIC FROM range(100000000000) r(i);
SQL
"$DUCKDB" "$T/main.duckdb" -c "CREATE TABLE t AS FROM range(3);" >/dev/null
"$DUCKDB" "$T/w.duckdb" -c "CREATE TABLE t AS FROM range(7); CREATE SCHEMA s2; CREATE TABLE s2.u AS SELECT 42 AS x;" >/dev/null

# Start a session on $1 (extra CLI flags in $2) that runs $3, keep it open.
start() {
  local db="$1" flags="$2" sql="$3"
  mkfifo "$T/in"
  # shellcheck disable=SC2086
  "$DUCKDB" -unsigned $flags "$db" < "$T/in" > "$T/out" 2>&1 &
  PIDS+=($!)
  exec 3>"$T/in"
  rm "$T/in"
  printf "LOAD %s;\n%s\n" "$(q "$EXT")" "$sql" >&3
  for _ in $(seq 100); do
    curl -sf "http://127.0.0.1:$PORT/healthz" >/dev/null 2>&1 && return 0
    grep -q "Error" "$T/out" 2>/dev/null && break
    sleep 0.2
  done
  [ -n "${QUIET:-}" ] || cat "$T/out" >&2
  return 1
}
stop() {
  echo "SELECT anofox_serve_stop($PORT);" >&3
  exec 3>&-
  wait "${PIDS[-1]}" 2>/dev/null || true
  unset 'PIDS[-1]'
}
panel() {
  curl -s -o "$T/body" -D "$T/headers" -w "%{http_code}" -X POST "http://127.0.0.1:$PORT/api/panel" \
    -d "{\"dashboard\":\"t\",\"panel\":$1,\"vars\":{}}"
}
serve() { echo "SELECT anofox_serve_dashboards($(q "$T/dash"), $PORT, $(q "$1"));"; }

# 1. A read-only session serves its own data.
start "$T/main.duckdb" "-readonly" "$(serve '{}')" || fail "a -readonly session could not start a locked server"
[ "$(panel 0)" = 200 ] && grep -q '"c0":3' "$T/body" || fail "-readonly: $(cat "$T/body")"
echo "SELECT anofox_serve_refresh($PORT);" >&3
for _ in $(seq 50); do grep -q "epoch 2" "$T/out" && break; sleep 0.2; done
grep -q "epoch 2" "$T/out" || fail "-readonly refresh: $(cat "$T/out")"
stop
echo "OK: a duckdb -readonly session serves its data and refreshes it"

# 2. search_path: the attached database, by option or from the caller's USE.
start "$T/main.duckdb" "" "ATTACH $(q "$T/w.duckdb") AS warehouse (READ_ONLY); USE warehouse;
$(serve '{"search_path": "warehouse"}')" || fail "search_path warehouse"
[ "$(panel 0)" = 200 ] && grep -q '"c0":7' "$T/body" || fail "search_path warehouse: $(cat "$T/body")"
stop
start "$T/main.duckdb" "-readonly" "ATTACH $(q "$T/w.duckdb") AS warehouse (READ_ONLY); USE warehouse.s2;
SELECT anofox_serve_dashboards($(q "$T/dash"), $PORT,
  json_object('search_path', current_database() || '.' || current_schema() || ',warehouse.main'));" \
  || fail "search_path from current_database()/current_schema()"
[ "$(panel 1)" = 200 ] && grep -q '"c0":42' "$T/body" || fail "USE warehouse.s2 (schema): $(cat "$T/body")"
[ "$(panel 0)" = 200 ] && grep -q '"c0":7' "$T/body" || fail "second search_path entry: $(cat "$T/body")"
stop
# Without it, the default database's t answers (documented).
start "$T/main.duckdb" "" "ATTACH $(q "$T/w.duckdb") AS warehouse (READ_ONLY); USE warehouse; $(serve '{}')" \
  || fail "default search path"
[ "$(panel 0)" = 200 ] && grep -q '"c0":3' "$T/body" || fail "default search path: $(cat "$T/body")"
stop
# A search path naming something that isn't there fails at startup.
if QUIET=1 start "$T/main.duckdb" "" "$(serve '{"search_path": "nope"}')"; then fail "search_path nope was accepted"; fi
grep -q "search_path: 'nope' is not an attached DuckDB database" "$T/out" || fail "search_path nope: $(cat "$T/out")"
exec 3>&-
echo "OK: search_path (option, or the caller's USE passed in) decides where names resolve"

# 3. Limits.
start "$T/main.duckdb" "" "$(serve '{"max_response_mb": 1, "max_queries": 1, "timeout_ms": 1500, "threads": 4}')" \
  || fail "limits server"
[ "$(panel 2)" = 413 ] && grep -q "size limit (1 MB)" "$T/body" || fail "max_response_mb: $(cat "$T/body")"
# One query slot: three slow queries at once (distinct variables, so none
# joins another), the last one waits past its timeout.
slow=()
for i in 1 2 3; do
  ( curl -s -o "$T/slow$i" -D "$T/slowh$i" -w "%{http_code}" -X POST "http://127.0.0.1:$PORT/api/panel" \
      -d "{\"dashboard\":\"t\",\"panel\":3,\"vars\":{\"k\":$i}}" > "$T/slowc$i" ) &
  slow+=($!)
  sleep 0.1
done
wait "${slow[@]}"
codes="$(cat "$T/slowc1" "$T/slowc2" "$T/slowc3" | fold -w3 | sort | tr '\n' ' ')"
[ "$codes" = "400 400 503 " ] || fail "max_queries: expected two timeouts and one 503, got $codes"
grep -qi "^Retry-After: 1" "$T"/slowh* || fail "503 without Retry-After"
health="$(curl -s "http://127.0.0.1:$PORT/healthz")"
for k in '"max_queries":1' '"max_response_bytes":1048576' '"refused_queries":1' '"inflight_bytes":0'; do
  [[ "$health" == *"$k"* ]] || fail "/healthz lacks $k: $health"
done
stop
echo "OK: size cap (413), query slots (503 + Retry-After), limits in /healthz"
