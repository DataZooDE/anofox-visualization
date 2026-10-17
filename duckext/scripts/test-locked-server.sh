#!/usr/bin/env bash
# End-to-end checks of a locked server (anofox_serve_dashboards) through the
# DuckDB CLI and curl:
#   * a `duckdb -readonly` session serves its data (#104);
#   * `search_path` (and the caller's `USE` passed through it) decides where
#     the panels' unqualified names resolve (#104);
#   * per-response size cap (413), query slots (503 + Retry-After) and the
#     limits in /healthz (#103);
#   * viewer variables: `-- @vars`, reserved names, values bound as literals,
#     and a dashboard's own defaulting `SET VARIABLE` (#151) — plus, with
#     NODE_PATH (Playwright), the same in a real browser (locked-vars-e2e.mjs,
#     also ::INFO in a ::PAGED table, #152).
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

# 4. Viewer variables (#151). The server sets the viewer's values first, then
# re-runs the dashboard's own SETs: a plain SET wins, a defaulting
# coalesce(getvariable(…), …) keeps the viewer's value.
mkdir "$T/dash4"
cat > "$T/dash4/runs.sql" <<'SQL'
-- @vars run
SET VARIABLE run = coalesce(nullif(getvariable('run'), ''), (SELECT max(name) FROM runs));
SELECT count(*)::BIGINT AS n ::METRIC, 'run ' || getvariable('run') ::LABEL FROM runs WHERE name = getvariable('run');
SELECT getvariable('run') AS r ::TABLE, getvariable('other') AS o FROM range(1);
SELECT name AS "Run" ::PAGED, 'Runs of the forecast pipeline' AS about ::INFO FROM runs ORDER BY name;
SQL
cat > "$T/dash4/pick.sql" <<'SQL'
-- @vars run
SELECT name AS run ::DROPDOWN FROM runs ORDER BY name;
SET VARIABLE run = coalesce(nullif(getvariable('run'), ''), (SELECT max(name) FROM runs));
SELECT count(*)::BIGINT AS n ::METRIC, 'run ' || getvariable('run') ::LABEL FROM runs WHERE name = getvariable('run');
SQL
"$DUCKDB" "$T/runs.duckdb" -c "CREATE TABLE runs AS SELECT * FROM (VALUES ('a'), ('b'), ('c')) t(name);" >/dev/null
start "$T/runs.duckdb" "-readonly" "SELECT anofox_serve_dashboards($(q "$T/dash4"), $PORT, '{}');" || fail "variables server"
vpanel() {
  curl -s -o "$T/body" -w "%{http_code}" -X POST "http://127.0.0.1:$PORT/api/panel" \
    -d "{\"dashboard\":\"runs\",\"panel\":2,\"vars\":$1}"
}
[ "$(vpanel '{}')" = 200 ] && grep -q '"r":"c"' "$T/body" || fail "default: $(cat "$T/body")"
[ "$(vpanel '{"run":"a"}')" = 200 ] && grep -q '"r":"a"' "$T/body" || fail "viewer value: $(cat "$T/body")"
# a value is a typed literal, never SQL: an expression stays its text
[ "$(vpanel '{"run":"(SELECT max(name) FROM runs)"}')" = 200 ] && grep -q '"r":"(SELECT max(name) FROM runs)"' "$T/body" \
  || fail "expression value: $(cat "$T/body")"
# not in -- @vars: dropped; reserved: refused
[ "$(vpanel '{"other":"x"}')" = 200 ] && grep -q '"o":null' "$T/body" || fail "-- @vars: $(cat "$T/body")"
[ "$(vpanel '{"__claim_user":"x"}')" = 400 ] || fail "__claim_user accepted: $(cat "$T/body")"
[ "$(vpanel '{"anofox_data":"/"}')" = 400 ] || fail "anofox_data accepted: $(cat "$T/body")"
echo "OK: viewer variables (-- @vars, reserved names, literals, a defaulting SET VARIABLE)"
# `omit` (#152): only the ::PAGED panel's own ::INFO column; anything else is a
# 400 that doesn't echo the name.
opanel() {
  curl -s -o "$T/body" -w "%{http_code}" -X POST "http://127.0.0.1:$PORT/api/panel" \
    -d "{\"dashboard\":\"runs\",\"panel\":$1,\"vars\":{},\"page\":{\"limit\":5,\"omit\":$2}}"
}
[ "$(opanel 3 '["__anofox_info"]')" = 200 ] && grep -q '"Run":"a"' "$T/body" && ! grep -q "forecast pipeline" "$T/body" \
  || fail "omit the ::INFO column: $(cat "$T/body")"
[ "$(opanel 3 '[]')" = 200 ] && grep -q "forecast pipeline" "$T/body" || fail "no omit: $(cat "$T/body")"
for bad in '["Run"]' '["secret_col"]' '["__anofox_info","Run"]'; do
  [ "$(opanel 3 "$bad")" = 400 ] && ! grep -q "secret_col\|Run" "$T/body" || fail "omit $bad: $(cat "$T/body")"
done
# a panel without an ::INFO column (the probe ::TABLE)
[ "$(opanel 2 '["__anofox_info"]')" = 400 ] || fail "omit on a panel without ::INFO: $(cat "$T/body")"
echo "OK: page omit names only the panel's own ::INFO column"
if [ -n "${NODE_PATH:-}" ]; then
  node "$(dirname "$0")/locked-vars-e2e.mjs" "http://127.0.0.1:$PORT" || fail "browser checks (locked-vars-e2e.mjs)"
  echo "OK: in a browser — a dashboard's own SET VARIABLE is no viewer value; ::INFO in ::PAGED"
else
  echo "SKIP: browser checks (set NODE_PATH to a node_modules with playwright)"
fi
stop
