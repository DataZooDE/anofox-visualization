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
#   * `-- @requires` with `${var}` and alternatives over a data folder, and
#     the path rule: an expanded path stays inside the folder (#156);
#   * `-- @requires` on a ::TRACK (#162) and on a ::TAB / ::SUBTAB, whose
#     panels then never run (#163) — with NODE_PATH also in a browser
#     (requires-tabs-e2e.mjs).
#   * `-- @requires` on a ::VIEW alternative (#214): hidden exactly like a
#     ::TAB/::SUBTAB's (only its own alternative, never its siblings), every
#     alternative hidden hides the whole group, and `?view.*` never reaches
#     the server — with NODE_PATH also in a browser (views-e2e.mjs).
#   * parquet footers are cached across requests; a file renamed into place is
#     always read fresh, an in-place rewrite at the same size and mtime second
#     is not (#209).
#   * the result cache keys on the variables a panel can depend on (#210).
#   * a database with foreign keys (#283).
#   * window.__served exposes `path_vars` (#407 part 2); `nav_picker` (#407
#     part 3) refuses to start with `var` not in `path_vars` or together
#     with an access policy, and its options drop a row whose value isn't a
#     plain name — with NODE_PATH also in a browser (served-e2e.mjs §5).
# usage: duckext/scripts/test-locked-server.sh <extension> [port]
#        (DUCKDB=… picks the CLI; default: duckdb on PATH)
set -euo pipefail
EXT="$(cd "$(dirname "$1")" && pwd)/$(basename "$1")"
case "$(basename "$EXT")" in
  anofox_visualization.duckdb_extension|anofox_visualization.*.duckdb_extension) ;;
  *) echo "$EXT: LOAD takes the entry point from the file name, so it must be named anofox_visualization.duckdb_extension (or anofox_visualization.<x>.duckdb_extension). Rename it, or: ln -s '$EXT' /tmp/anofox_visualization.duckdb_extension" >&2; exit 2 ;;
esac
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
-- #210: reads k, so the three slow requests below (distinct k) stay on
-- separate cache keys instead of sharing one single-flight execution.
SELECT sum(i)::BIGINT AS s ::METRIC FROM range(100000000000) r(i) WHERE getvariable('k') IS NOT NULL;
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
    # A genuine fatal CLI error (the LOAD or the SELECT itself failed) is its
    # own top-level line, flush at column 0: "<Words> Error: <message>". A
    # *successful* FK snapshot's own banner can still contain the substring
    # "Error" — e.g. "... copied without its constraints and indexes:
    # Constraint Error: Violates foreign key constraint ..." (#283) — but
    # always embedded mid-line inside the query result's box-drawn output,
    # never at column 0. A bare `grep -q "Error"` matched that benign case
    # too and broke out of the loop before the next `curl` could see the
    # server come up (flaky: "a database with foreign keys could not be
    # served", seen on #314 and #341's CI, neither of which touch serving).
    grep -qE '^([A-Za-z][A-Za-z ]* )?Error:' "$T/out" 2>/dev/null && break
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
# panel <index> [vars JSON] [dashboard]
panel() {
  local vars="${2:-}"
  [ -n "$vars" ] || vars='{}'
  curl -s -o "$T/body" -D "$T/headers" -w "%{http_code}" -X POST "http://127.0.0.1:$PORT/api/panel" \
    -d "{\"dashboard\":\"${3:-t}\",\"panel\":$1,\"vars\":$vars}"
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
for k in '"version":"v' '"max_queries":1' '"max_response_bytes":1048576' '"refused_queries":1' '"inflight_bytes":0'; do
  [[ "$health" == *"$k"* ]] || fail "/healthz lacks $k: $health"
done
stop
echo "OK: size cap (413), query slots (503 + Retry-After), limits + version in /healthz"

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

# 4b. #210: the result cache keys on the variables a statement can depend on
# — its own getvariable()s and, through a SET VARIABLE before it, what that
# reads; a statement the analysis can't see through keys on every variable.
mkdir "$T/dash210"
cat > "$T/dash210/vs.sql" <<'SQL'
SET VARIABLE sel = coalesce(nullif(getvariable('run'), ''), 'a');
SELECT count(*)::BIGINT AS n ::METRIC, getvariable('sel') AS s ::LABEL FROM runs WHERE name = getvariable('sel');
SELECT count(*)::BIGINT AS n ::METRIC FROM runs;
SELECT count(*)::BIGINT AS n ::METRIC FROM runs WHERE name = getvariable(lower('RUN'));
SQL
start "$T/runs.duckdb" "-readonly" "SELECT anofox_serve_dashboards($(q "$T/dash210"), $PORT, $(q "{\"log\": \"$T/req210.log\"}"));" || fail "#210 server"
# p210 <panel> <vars JSON>: POST it; print how the server answered ("run", "hit", "shared")
p210() {
  local before n code
  before=$(wc -l < "$T/req210.log")
  code=$(curl -s -o "$T/body" -D "$T/headers" -w "%{http_code}" -X POST "http://127.0.0.1:$PORT/api/panel" \
    -d "{\"dashboard\":\"vs\",\"panel\":$1,\"vars\":$2}")
  [ "$code" = 200 ] || fail "#210 panel $1 $2: $code $(cat "$T/body")"
  for _ in $(seq 100); do n=$(wc -l < "$T/req210.log"); [ "$n" -gt "$before" ] && break; sleep 0.05; done
  tail -1 "$T/req210.log" | sed -n 's/.*"cache":"\([a-z]*\)".*/\1/p'
}
[ "$(p210 2 '{"run":"a"}')" = run ] || fail "#210: first request"
[ "$(p210 2 '{"run":"b"}')" = hit ] || fail "#210: a variable panel 2 doesn't read re-ran it"
[ "$(p210 2 '{}')" = hit ] || fail "#210: no variables"
[ "$(p210 1 '{"run":"a"}')" = run ] && grep -q '"c1":"a"' "$T/body" || fail "#210: panel 1 run=a: $(cat "$T/body")"
[ "$(p210 1 '{"run":"b"}')" = run ] && grep -q '"c1":"b"' "$T/body" || fail "#210: run read through SET VARIABLE sel: $(cat "$T/body")"
[ "$(p210 1 '{"run":"a","other":"z"}')" = hit ] && grep -q '"c1":"a"' "$T/body" || fail "#210: unrelated variable: $(cat "$T/body")"
[ "$(p210 1 '{"RUN":"b"}')" = hit ] && grep -q '"c1":"b"' "$T/body" || fail "#210: names are case-insensitive: $(cat "$T/body")"
[ "$(p210 3 '{"run":"a"}')" = run ] || fail "#210: panel 3 first"
[ "$(p210 3 '{"run":"a","other":"z"}')" = run ] || fail "#210: a non-literal getvariable must key on every variable"
grep -qi '^X-Anofox-Deps: [0-9a-f]\{16\}' "$T/headers" || fail "#210: no X-Anofox-Deps header"
curl -s "http://127.0.0.1:$PORT/d/vs" | grep -q '"deps":\[\[\],\["run","sel"\],\[\],null\]' || fail "#210: window.__served.deps"
echo "OK: #210 cache keys follow the variables a panel can depend on (SET VARIABLE closure, conservative fallback)"
stop

# 4c. Security review of #210/#222 (M1): a variable a panel doesn't read must
# never reach a `SET VARIABLE`, so a value chosen to make that SET fail (a
# reserved word as the name, a mixed-type list as the value) can't poison a
# single-flight shared with another request that only agrees on the
# variables the panel actually reads. Panel 0 is slow and holds the only
# query slot; panel 1 depends only on `region` (#210 narrows its key/SETs to
# that). An attacker (A) posts panel 1 with a poisoned extra variable while
# a victim (B) posts the same `region` alone; both must succeed, B must never
# see A's text, and B must have joined A's single-flight (`"cache":"shared"`
# in the request log, #237); otherwise the section never exercised M1.
mkdir "$T/dashm1"
cat > "$T/dashm1/sf.sql" <<'SQL'
SELECT sum(i)::BIGINT AS s ::METRIC FROM range(100000000000) r(i) WHERE getvariable('hold') IS NOT NULL;
SELECT count(*)::BIGINT AS n ::METRIC FROM runs WHERE name = getvariable('region');
SQL
start "$T/runs.duckdb" "-readonly" \
  "SELECT anofox_serve_dashboards($(q "$T/dashm1"), $PORT, $(q "{\"log\": \"$T/reqm1.log\", \"max_queries\": 1, \"timeout_ms\": 3000}"));" \
  || fail "#222 M1 server"
# postm1 <pidvar> <tag> <vars JSON>: POST panel 1 in the background, write
# body/code to $T/m1<tag>.*, and the job's PID into variable $pidvar. (Must
# be called directly, never through `$(...)`: a command substitution runs
# this in a subshell, and a job backgrounded there is reparented away the
# moment the subshell exits, so the caller's `wait` can no longer see it.)
postm1() {
  curl -s -o "$T/m1body$2" -w "%{http_code}" -X POST "http://127.0.0.1:$PORT/api/panel" \
    -d "{\"dashboard\":\"sf\",\"panel\":1,\"vars\":$3}" > "$T/m1code$2" &
  eval "$1=\$!"
}
curl -s -o /dev/null -X POST "http://127.0.0.1:$PORT/api/panel" -d '{"dashboard":"sf","panel":0,"vars":{"hold":1}}' &
h=$!
sleep 0.3
postm1 a A '{"region":"a","z":[1,"attacker-text"]}'
sleep 0.3
postm1 b B '{"region":"a"}'
wait "$h" "$a" "$b" 2>/dev/null
[ "$(cat "$T/m1codeA")" = 200 ] || fail "#222 M1: the attacker's own request (z dropped, never SET) should succeed too: $(cat "$T/m1codeA") $(cat "$T/m1bodyA")"
[ "$(cat "$T/m1codeB")" = 200 ] && grep -q '"c0":1' "$T/m1bodyB" || fail "#222 M1: victim B: $(cat "$T/m1codeB") $(cat "$T/m1bodyB")"
grep -qi "attacker-text" "$T/m1bodyB" "$T/m1bodyA" && fail "#222 M1: the attacker's text leaked into a response"
# #237: B (vars ["region"] only) must have joined A's flight; if it ran on its
# own, nothing above exercised M1. The log line lands just after the response.
m1shared() { grep -E '"panel":1[,}]' "$T/reqm1.log" | grep -F '"vars":["region"]' | grep -F '"cache":"shared"' >/dev/null; }
for _ in $(seq 100); do m1shared && break; sleep 0.05; done
m1shared || fail "#222 M1: victim B did not join the attacker's single-flight (no \"cache\":\"shared\"), so 4c did not exercise M1: $(cat "$T/reqm1.log")"
# `{"select": 1}` — a reserved word as the variable *name* — must be refused
# just as safely (dropped before any SET, since panel 1 doesn't read it).
postm1 c C '{"region":"a","select":1}'
wait "$c" 2>/dev/null
[ "$(cat "$T/m1codeC")" = 200 ] && grep -q '"c0":1' "$T/m1bodyC" || fail "#222 M1: reserved word 'select' as a name: $(cat "$T/m1codeC") $(cat "$T/m1bodyC")"
echo "OK: #222 M1 — a variable a panel doesn't read is dropped before any SET, so it can't fail a shared single-flight for another request (B shared A's flight)"
stop

# 5. `-- @requires` over a data folder (#156): `${var}` expands to the
# request's value, `a | b` is met by either, and an expanded path must stay
# inside the folder — a viewer's own variable, `..`, an absolute path outside
# or `~` never reach the file system.
mkdir -p "$T/runs/r1" "$T/runs/r2" "$T/runs/r3" "$T/outside" "$T/rdash"
"$DUCKDB" -c "COPY (SELECT 1 AS v) TO $(q "$T/runs/r1/forecasts.parquet");
  COPY (SELECT 2 AS v) TO $(q "$T/runs/r2/_anofox_meta_forecasts.parquet");
  COPY (SELECT 3 AS v) TO $(q "$T/outside/f.parquet");" >/dev/null
cat > "$T/rdash/r.sql" <<'SQL'
SET VARIABLE rd = getvariable('anofox_data') || '/' || getvariable('run');
-- @requires ${rd}/_anofox_meta_forecasts.parquet | ${rd}/forecasts.parquet
SELECT count(*)::BIGINT AS n ::METRIC FROM read_parquet(getvariable('rd') || '/*forecasts.parquet');
-- @requires ${rd}/extra.parquet
CREATE OR REPLACE TEMP VIEW extra AS SELECT * FROM read_parquet(getvariable('rd') || '/extra.parquet');
SELECT 1::BIGINT AS one ::METRIC;
-- @requires ${x}/forecasts.parquet
SELECT 2::BIGINT AS two ::METRIC;
-- @requires ../outside/f.parquet
SELECT 3::BIGINT AS three ::METRIC;
-- @requires /etc/passwd | ~/.profile
SELECT 4::BIGINT AS four ::METRIC;
-- @requires ${run}/forecasts.parquet
SELECT 5::BIGINT AS five ::METRIC;
SET VARIABLE qd = getvariable('anofox_data') || '/' || coalesce(getvariable('q'), 'r1');
-- @requires ${qd}
SELECT 9::BIGINT AS nine ::METRIC;
-- @requires */secret.parquet
SELECT 10::BIGINT AS ten ::METRIC;
-- @requires ${qd}/forecasts.parquet
SELECT 11::BIGINT AS eleven ::METRIC;
-- @requires */forecasts.parquet
SELECT 12::BIGINT AS twelve ::METRIC;
-- @requires cap/*/f.parquet
SELECT 13::BIGINT AS thirteen ::METRIC;
SQL
# 70 links out of the folder listed before the one real match (zz/).
mkdir -p "$T/runs/cap/zz"
for i in $(seq -w 0 69); do ln -s ../../outside "$T/runs/cap/a$i"; done
ln -s ../outside "$T/runs/lnk"
cp "$T/outside/f.parquet" "$T/outside/secret.parquet"
cp "$T/outside/f.parquet" "$T/runs/cap/zz/f.parquet"
start "$T/main.duckdb" "" "SELECT anofox_serve_dashboards($(q "$T/rdash"), $PORT,
  json_object('data', $(q "$T/runs"), 'path_vars', ['run']));" || fail "data-folder server"
na() { [ "$1" = 400 ] && grep -q "no files found" "$T/body" || fail "$2: expected Not available, got $1 $(cat "$T/body")"; }
[ "$(panel 1 '{"run":"r1"}' r)" = 200 ] && grep -q '"c0":1' "$T/body" || fail "second alternative: $(cat "$T/body")"
[ "$(panel 1 '{"run":"r2"}' r)" = 200 ] && grep -q '"c0":1' "$T/body" || fail "first alternative: $(cat "$T/body")"
na "$(panel 1 '{"run":"r3"}' r)" "no alternative"
# A skipped setup statement (its file is missing) doesn't fail the next panel.
[ "$(panel 3 '{"run":"r1"}' r)" = 200 ] || fail "after a skipped setup: $(cat "$T/body")"
grep -q "setup statement failed" "$T/out" && fail "the missing setup input was run: $(cat "$T/out")"
# `${x}` set by the viewer (not a path variable): never expanded.
na "$(panel 4 '{"run":"r1","x":"r1"}' r)" "a viewer variable in the path"
na "$(panel 5 '{"run":"r1"}' r)" "a .. path"
na "$(panel 6 '{"run":"r1"}' r)" "an absolute path outside / ~"
grep -q "@requires refused" "$T/out" || fail "refused paths are not logged: $(cat "$T/out")"
# A relative path resolves against the data folder.
[ "$(panel 7 '{"run":"r1"}' r)" = 200 ] || fail "relative path: $(cat "$T/body")"
na "$(panel 7 '{"run":"r3"}' r)" "relative path, missing"
# A dashboard variable can carry a viewer's value (`qd` from `q`): it is
# never read as a pattern, nothing outside the folder is observable through
# the `lnk -> ../outside` symlink, and a failing check leaks no error or path.
[ "$(panel 11 '{"run":"r1","q":"r1"}' r)" = 200 ] || fail "dashboard variable, plain value: $(cat "$T/body")"
for q in '*' 'r?' '[r]1' '*/secret.parquet' 'l?k/s*' 'lnk' 'lnk/secret.parquet' '**/**/nomatch' 'big/**'; do
  code="$(panel 9 "{\"run\":\"r1\",\"q\":\"$q\"}" r)"
  na "$code" "q=$q"
  grep -qiE "error|permission|$T" "$T/body" && fail "q=$q leaks: $(cat "$T/body")"
done
[ "$(panel 11 '{"run":"r1","q":"r*"}' r)" = 400 ] || fail "a pattern in a value was expanded"
grep -q "a pattern character in a variable's value" "$T/out" || fail "pattern refusal not logged"
# An author's own pattern matches inside the folder, but does not count a
# match through the symlink.
[ "$(panel 12 '{"run":"r1"}' r)" = 200 ] || fail "an author's pattern: $(cat "$T/body")"
na "$(panel 10 '{"run":"r1"}' r)" "a match outside the folder through a symlink"
# Matches through outward links don't hide a real one listed after them.
[ "$(panel 13 '{"run":"r1"}' r)" = 200 ] || fail "a real match after 70 outward links: $(cat "$T/body")"
stop
echo "OK: -- @requires: \${var}, alternatives, and paths confined to the data folder"
echo "OK: -- @requires: no patterns from values, no matches through symlinks, no leaked errors"

# 6. `-- @requires` on a ::TRACK strip (#162) and on a ::TAB / ::SUBTAB
# (#163): a hidden tab's statement answers "Not available", and so does every
# panel in its section — also one that reads no file — so the server never
# runs them, whoever asks.
mkdir -p "$T/truns/r1" "$T/truns/r2" "$T/tdash"
"$DUCKDB" -c "COPY (SELECT DATE '2026-01-01' + to_days(i::INT) AS day, i AS v FROM range(10) t(i)) TO $(q "$T/truns/r1/present.parquet");
  COPY (SELECT DATE '2026-01-01' + to_days(i::INT) AS day, i AS v FROM range(10) t(i)) TO $(q "$T/truns/r2/present.parquet");
  COPY (SELECT 7 AS v) TO $(q "$T/truns/r1/fc.parquet");" >/dev/null
cat > "$T/tdash/tabs.sql" <<'SQL'
SELECT unnest(['r1', 'r2']) AS run ::DROPDOWN;
SET VARIABLE rd = getvariable('anofox_data') || '/' || getvariable('run');
SELECT day::XAXIS, v::LINECHART, 'A chart with two strips'::TITLE FROM read_parquet(getvariable('rd') || '/present.parquet') ORDER BY day;
-- @requires ${rd}/present.parquet
SELECT day::XAXIS, 'Present'::TRACK FROM read_parquet(getvariable('rd') || '/present.parquet') WHERE v % 3 = 0;
-- @requires ${rd}/missing.parquet
SELECT day::XAXIS, 'Missing'::TRACK FROM read_parquet(getvariable('rd') || '/missing.parquet');
SELECT 'Overview'::TAB;
SELECT 1::BIGINT AS one ::METRIC;
-- @requires ${rd}/fc.parquet
SELECT 'Forecastability'::TAB;
SELECT count(*)::BIGINT AS n ::METRIC FROM read_parquet(getvariable('rd') || '/fc.parquet');
SELECT 2::BIGINT AS two ::METRIC;
-- @requires ${rd}/sub.parquet
SELECT 'Detail'::SUBTAB;
SELECT 3::BIGINT AS three ::METRIC;
SELECT 'Other'::SUBTAB;
SELECT 4::BIGINT AS four ::METRIC;
-- @requires ${later}/fc.parquet
SELECT 'Late'::TAB;
SET VARIABLE later = getvariable('rd');
SELECT v::BIGINT AS v ::METRIC FROM read_parquet(getvariable('rd') || '/fc.parquet');
SELECT 'Again'::TAB;
SELECT 2::BIGINT AS two ::METRIC;
-- @requires ${rd}/present.parquet
SELECT 'Detail'::SUBTAB;
SELECT 5::BIGINT AS five ::METRIC;
SQL
start "$T/main.duckdb" "" "SELECT anofox_serve_dashboards($(q "$T/tdash"), $PORT,
  json_object('data', $(q "$T/truns"), 'path_vars', ['run']));" || fail "tabs server"
# #162: the missing strip answers "Not available" (the page leaves it out).
[ "$(panel 3 '{"run":"r1"}' tabs)" = 200 ] || fail "present track: $(cat "$T/body")"
na "$(panel 4 '{"run":"r1"}' tabs)" "missing track"
grep -q "$T" "$T/body" && fail "the track's answer leaks the path: $(cat "$T/body")"
# #163: the tab with its input is shown, its panels run …
[ "$(panel 7 '{"run":"r1"}' tabs)" = 200 ] || fail "shown tab: $(cat "$T/body")"
[ "$(panel 8 '{"run":"r1"}' tabs)" = 200 ] && grep -q '"c0":"1"\|"c0":1' "$T/body" || fail "shown tab's panel: $(cat "$T/body")"
# … without it: the tab and each of its panels are "Not available".
for i in 7 8 9 10 11 12 13; do na "$(panel $i '{"run":"r2"}' tabs)" "hidden tab, statement $i"; done
# A sub-tab on its own: hidden with its panels; the next sub-tab is shown.
na "$(panel 10 '{"run":"r1"}' tabs)" "hidden sub-tab"
na "$(panel 11 '{"run":"r1"}' tabs)" "hidden sub-tab's panel"
[ "$(panel 12 '{"run":"r1"}' tabs)" = 200 ] || fail "the next sub-tab: $(cat "$T/body")"
[ "$(panel 13 '{"run":"r1"}' tabs)" = 200 ] || fail "the next sub-tab's panel: $(cat "$T/body")"
# Before the tabs and in another tab: unaffected.
[ "$(panel 2 '{"run":"r2"}' tabs)" = 200 ] || fail "a panel before the tabs: $(cat "$T/body")"
[ "$(panel 6 '{"run":"r2"}' tabs)" = 200 ] || fail "a panel of another tab: $(cat "$T/body")"
# A tab is decided where it stands: `${later}` is set only below the tab, so
# the tab is hidden — and so are its panels, though `later` is set by then.
na "$(panel 14 '{"run":"r1"}' tabs)" "a tab whose variable is set below it"
na "$(panel 16 '{"run":"r1"}' tabs)" "the panel of a tab whose variable is set below it"
# The same SQL in two tabs: each statement answers by its own tab.
[ "$(panel 18 '{"run":"r2"}' tabs)" = 200 ] || fail "a KPI whose twin is in a hidden tab: $(cat "$T/body")"
[ "$(panel 19 '{"run":"r1"}' tabs)" = 200 ] || fail "a sub-tab whose twin is hidden: $(cat "$T/body")"
echo "OK: -- @requires on a ::TRACK; a hidden ::TAB / ::SUBTAB's panels never run"
if [ -n "${NODE_PATH:-}" ]; then
  node "$(dirname "$0")/requires-tabs-e2e.mjs" "http://127.0.0.1:$PORT" || fail "browser checks (requires-tabs-e2e.mjs)"
  echo "OK: in a browser — a hidden tab has no button and no requests; a missing strip is left out"
else
  echo "SKIP: browser checks (set NODE_PATH to a node_modules with playwright)"
fi
stop

# 6b. `-- @requires` on a ::VIEW group (#214): like a ::TAB/::SUBTAB, an
# alternative's own `-- @requires` hides only itself — no button, no pane, a
# direct request for its own statement or its panels answers "Not available"
# — and when every alternative of a group is hidden, the whole group (and
# each of its statements) disappears too. `?view.<id>=<label>` is a
# client-side URL concern only: `/d/<id>` ignores the query string
# entirely, so a crafted value can never be reflected or change the page.
mkdir -p "$T/vruns/r1" "$T/vruns/r2" "$T/vdash"
"$DUCKDB" -c "COPY (SELECT DATE '2026-01-01' + to_days(i::INT) AS day, i AS v FROM range(10) t(i)) TO $(q "$T/vruns/r1/present.parquet");
  COPY (SELECT DATE '2026-01-01' + to_days(i::INT) AS day, i AS v FROM range(10) t(i)) TO $(q "$T/vruns/r2/present.parquet");
  COPY (SELECT 7 AS v) TO $(q "$T/vruns/r1/fc.parquet");
  COPY (SELECT 1 AS v) TO $(q "$T/vruns/r1/zx.parquet");
  COPY (SELECT 1 AS v) TO $(q "$T/vruns/r1/zy.parquet");" >/dev/null
cat > "$T/vdash/views.sql" <<'SQL'
SELECT unnest(['r1', 'r2']) AS run ::DROPDOWN;
SELECT 10 AS boost ::NUMBER;
SET VARIABLE rd = getvariable('anofox_data') || '/' || getvariable('run');
-- @requires ${rd}/fc.parquet
SELECT 'Lines' AS yoy ::VIEW;
SELECT day::XAXIS, (v * getvariable('boost'))::LINECHART, 'Revenue (lines)'::TITLE FROM read_parquet(getvariable('rd') || '/present.parquet') ORDER BY day;
SELECT 'Bars'::VIEW;
SELECT 420::HEIGHT;
SELECT day::XAXIS, (v * getvariable('boost'))::BARCHART, 'Revenue (bars)'::TITLE FROM read_parquet(getvariable('rd') || '/present.parquet') ORDER BY day;
SELECT 'Table'::VIEW;
SELECT 'all'::TOTALS;
SELECT day::TABLE, (v * getvariable('boost')) AS revenue ::MONEY FROM read_parquet(getvariable('rd') || '/present.parquet') ORDER BY day;
SELECT 1::ENDVIEW;
-- @requires ${rd}/zx.parquet
SELECT 'Alpha'::VIEW;
SELECT 1::BIGINT AS one ::METRIC;
-- @requires ${rd}/zy.parquet
SELECT 'Beta'::VIEW;
SELECT 2::BIGINT AS two ::METRIC;
SQL
start "$T/main.duckdb" "" "SELECT anofox_serve_dashboards($(q "$T/vdash"), $PORT,
  json_object('data', $(q "$T/vruns"), 'path_vars', ['run']));" || fail "views server"
# statements: 0/1 = the run dropdown / boost number; 2 = setup (rd); 3/4 =
# the "yoy" group's Lines marker/chart (needs fc.parquet, r1 only); 5-7 =
# Bars (no requires, always shown); 8-10 = Table (no requires, always
# shown); 11 = ::ENDVIEW; 12/13 = group "2"'s Alpha marker/KPI (needs
# zx.parquet, r1 only); 14/15 = Beta marker/KPI (needs zy.parquet, r1 only)
# — on r2 every alternative of group "2" is hidden, so the whole group is.
[ "$(panel 4 '{"run":"r1"}' views)" = 200 ] || fail "shown alternative (Lines, r1): $(cat "$T/body")"
na "$(panel 4 '{"run":"r2"}' views)" "Lines hidden on r2"
[ "$(panel 7 '{"run":"r2"}' views)" = 200 ] || fail "Bars has no requires, always shown: $(cat "$T/body")"
[ "$(panel 10 '{"run":"r2"}' views)" = 200 ] || fail "Table has no requires, always shown: $(cat "$T/body")"
# the marker's own statement answers "Not available" too — exactly how the
# browser decides a ::VIEW is hidden (tabShown() in web/app.js).
na "$(panel 3 '{"run":"r2"}' views)" "Lines marker, r2"
[ "$(panel 3 '{"run":"r1"}' views)" = 200 ] || fail "Lines marker, r1: $(cat "$T/body")"
# every alternative of group "2" hidden on r2 (and so is the group); on r1
# (both files present) every one of its statements runs.
for i in 12 13 14 15; do na "$(panel $i '{"run":"r2"}' views)" "group 2, statement $i, r2"; done
for i in 12 13 14 15; do [ "$(panel $i '{"run":"r1"}' views)" = 200 ] || fail "group 2, statement $i, r1: $(cat "$T/body")"; done
# `?view.<id>=<label>` never reaches the server (a purely client-side URL
# concern, #214's security review item 1): `/d/<id>` ignores the query
# string entirely, so a crafted value is never reflected and the page is
# byte-identical with or without it, apart from the per-response CSP nonce
# (#252), which is masked before comparing.
nonce_mask() { sed -E 's/nonce="[^"]*"/nonce="N"/g'; }
curl -s "http://127.0.0.1:$PORT/d/views" | nonce_mask > "$T/vpage_plain"
curl -s "http://127.0.0.1:$PORT/d/views?view.yoy=%3Cscript%3Ealert(1)%3C%2Fscript%3E" | nonce_mask > "$T/vpage_xss"
cmp -s "$T/vpage_plain" "$T/vpage_xss" || fail "?view.x=<script> changed the served page"
grep -q 'nonce="N"' "$T/vpage_plain" || fail "#252: the served page's inline scripts carry no nonce"
curl -s -D - -o /dev/null "http://127.0.0.1:$PORT/d/views" | grep -qi "^content-security-policy: default-src 'none'; script-src 'self' 'nonce-" || fail "#252: no CSP header on the dashboard page"
curl -s -D - -o /dev/null "http://127.0.0.1:$PORT/" | grep -qi "frame-ancestors 'none'" || fail "#252: the landing page may be framed"
grep -qi "<script>alert(1)" "$T/vpage_xss" && fail "?view.x=<script> was reflected in the page"
echo "OK: ::VIEW -- @requires hides only its own alternative; every alternative hidden hides the group; ?view.* never reflected"
if [ -n "${NODE_PATH:-}" ]; then
  node "$(dirname "$0")/views-e2e.mjs" "http://127.0.0.1:$PORT" || fail "browser checks (views-e2e.mjs)"
  echo "OK: in a browser — only the active alternative's panels are asked; switching views asks exactly the new one"
else
  echo "SKIP: browser checks (set NODE_PATH to a node_modules with playwright)"
fi
stop

# 7. Parquet footers are cached across requests (#209): the snapshot has
# parquet_metadata_cache on (viewers can't turn it off). A file renamed into
# the data folder is always read fresh, also through a setup view; an
# in-place rewrite at the same size that keeps the old mtime (to the second)
# can be read stale (security review L1 — this is pre-existing
# DuckDB behaviour the PR doesn't change, not something the cache fixes).
# `cache_mb: 0`: the result cache doesn't see file changes (documented), this
# is about the footers.
mkdir -p "$T/pruns" "$T/pdash"
"$DUCKDB" -c "COPY (SELECT range AS x FROM range(10)) TO $(q "$T/pruns/f.parquet");" >/dev/null
cat > "$T/pdash/p.sql" <<'SQL'
SELECT (current_setting('parquet_metadata_cache')::VARCHAR = 'true')::INT::BIGINT AS cached ::METRIC;
CREATE OR REPLACE TEMP VIEW pv AS SELECT * FROM read_parquet(getvariable('anofox_data') || '/f.parquet');
SELECT count(*)::BIGINT AS n ::METRIC FROM pv;
SELECT sum(x)::BIGINT AS s ::METRIC FROM pv;
SELECT count(*)::BIGINT AS f ::METRIC FROM pv WHERE x >= 1000;
SQL
start "$T/main.duckdb" "" "SELECT anofox_serve_dashboards($(q "$T/pdash"), $PORT,
  json_object('data', $(q "$T/pruns"), 'cache_mb', 0));" || fail "parquet cache server"
[ "$(panel 0 '{}' p)" = 200 ] && grep -qE '"c0":1(\.0)?[,}]' "$T/body" || fail "parquet_metadata_cache is not on: $(cat "$T/body")"
[ "$(panel 2 '{}' p)" = 200 ] && grep -qE '"c0":10(\.0)?[,}]' "$T/body" || fail "first read: $(cat "$T/body")"
[ "$(panel 2 '{}' p)" = 200 ] && grep -qE '"c0":10(\.0)?[,}]' "$T/body" || fail "cached read: $(cat "$T/body")"
# rewritten in place by another process. Uncompressed: it becomes the "old"
# file for the rename case below, which needs an equal-size replacement to
# rule out a size change as the reason a rename reads fresh.
"$DUCKDB" -c "COPY (SELECT range AS x FROM range(1000)) TO $(q "$T/pruns/f.parquet") (COMPRESSION uncompressed);" >/dev/null
[ "$(panel 2 '{}' p)" = 200 ] && grep -qE '"c0":1000(\.0)?[,}]' "$T/body" || fail "in-place rewrite served a stale footer: $(cat "$T/body")"
[ "$(panel 3 '{}' p)" = 200 ] && grep -qE '"c0":499500(\.0)?[,}]' "$T/body" || fail "in-place rewrite (sum): $(cat "$T/body")"
# replaced by a rename (how a writer publishes a run atomically): same row
# count, same on-disk size (uncompressed, like the file it replaces) and the
# old file's mtime copied onto the new one with touch -r — so a size or
# mtime difference can't be why this reads fresh. DuckDB's cache validity
# check is size + mtime-to-the-second, keyed by the inode (security review
# L1): a rename is read fresh regardless, an in-place rewrite at this same
# size and mtime is not. count(*) and sum() alone can't tell a stale footer
# from a fresh one here (both files have the same shape); only a predicate
# that prunes on footer min/max statistics can (panel 4: `x >= 1000`, which
# the old file's footer would report as impossible).
"$DUCKDB" -c "COPY (SELECT range AS x FROM range(1000, 2000)) TO $(q "$T/pnew.parquet") (COMPRESSION uncompressed);" >/dev/null
touch -r "$T/pruns/f.parquet" "$T/pnew.parquet"
[ "$(stat -c %s "$T/pruns/f.parquet")" = "$(stat -c %s "$T/pnew.parquet")" ] \
  || fail "setup: rename-case files are not the same size"
mv "$T/pnew.parquet" "$T/pruns/f.parquet"
[ "$(panel 2 '{}' p)" = 200 ] && grep -qE '"c0":1000(\.0)?[,}]' "$T/body" || fail "renamed-in file served a stale footer: $(cat "$T/body")"
[ "$(panel 3 '{}' p)" = 200 ] && grep -qE '"c0":1499500(\.0)?[,}]' "$T/body" || fail "renamed-in file (sum): $(cat "$T/body")"
[ "$(panel 4 '{}' p)" = 200 ] && grep -qE '"c0":1000(\.0)?[,}]' "$T/body" || fail "renamed-in file (footer-pruned filter) served a stale footer: $(cat "$T/body")"
# the rebuilt snapshot has it too
echo "SELECT anofox_serve_refresh($PORT);" >&3
for _ in $(seq 50); do grep -q "epoch 2" "$T/out" && break; sleep 0.2; done
grep -q "epoch 2" "$T/out" || fail "parquet cache refresh: $(cat "$T/out")"
[ "$(panel 0 '{}' p)" = 200 ] && grep -qE '"c0":1(\.0)?[,}]' "$T/body" || fail "parquet_metadata_cache off after a refresh: $(cat "$T/body")"
grep -q "parquet_metadata_cache:" "$T/out" && fail "parquet_metadata_cache warning: $(cat "$T/out")"
stop
echo "OK: parquet footers cached across requests; a file renamed into place reads fresh, also after a refresh"

# 8. A database with foreign keys (#283): a bare COPY FROM DATABASE fails on it
# (duckdb/duckdb#16785), so the snapshot copies schema first, then table by
# table, parents first; a generated column and a sequence default copy as
# values; a self-referencing table is copied without its constraints. From a
# -readonly session, and the source is checked unchanged afterwards.
"$DUCKDB" "$T/fk.duckdb" -c "
CREATE SEQUENCE sq;
CREATE TABLE suppliers(supplier_id VARCHAR PRIMARY KEY, name VARCHAR);
CREATE TABLE materials(material_id VARCHAR PRIMARY KEY, supplier_id VARCHAR REFERENCES suppliers(supplier_id),
  price DOUBLE, price2 DOUBLE GENERATED ALWAYS AS (price * 2));
CREATE TABLE actuals(material_id VARCHAR REFERENCES materials(material_id), qty INTEGER, n BIGINT DEFAULT nextval('sq'));
CREATE TABLE tree(id INTEGER PRIMARY KEY, parent INTEGER REFERENCES tree(id));
INSERT INTO suppliers SELECT 'SUP-' || lpad(i::VARCHAR, 3, '0'), 's' || i FROM range(50) r(i);
INSERT INTO materials (material_id, supplier_id, price) SELECT 'M-' || i, 'SUP-' || lpad((i % 50)::VARCHAR, 3, '0'), i FROM range(500) r(i);
INSERT INTO actuals (material_id, qty) SELECT 'M-' || (i % 500), i FROM range(20000) r(i);
INSERT INTO tree VALUES (1, NULL); INSERT INTO tree VALUES (2, 1); INSERT INTO tree VALUES (3, 2);
CREATE VIEW per_supplier AS SELECT supplier_id, sum(qty) AS q FROM actuals JOIN materials USING (material_id) GROUP BY ALL;" >/dev/null
if "$DUCKDB" -c "ATTACH $(q "$T/fk.duckdb") AS s (READ_ONLY); ATTACH ':memory:' AS d; COPY FROM DATABASE s TO d;" >/dev/null 2>&1; then
  echo "NOTE: a bare COPY FROM DATABASE copies the FK fixture now (duckdb/duckdb#16785 fixed?)"
fi
fkstate() { "$DUCKDB" -readonly -noheader -list "$T/fk.duckdb" -c "SELECT count(*) FROM duckdb_constraints() WHERE database_name = current_database(); SELECT count(*), sum(n) FROM actuals; SELECT count(*) FROM tree;"; }
before="$(fkstate)"
mkdir -p "$T/fkdash"
cat > "$T/fkdash/fk.sql" <<'SQL'
SELECT count(*)::BIGINT AS n ::METRIC FROM actuals;
SELECT sum(price2)::BIGINT AS s ::METRIC FROM materials;
SELECT count(*)::BIGINT AS n ::METRIC FROM tree WHERE parent IS NOT NULL;
SELECT count(*)::BIGINT AS n ::METRIC FROM per_supplier;
SELECT max(n)::BIGINT AS n ::METRIC FROM actuals;
SQL
start "$T/fk.duckdb" "-readonly" "SELECT anofox_serve_dashboards($(q "$T/fkdash"), $PORT, '{}');" || fail "a database with foreign keys could not be served"
[ "$(panel 0 '{}' fk)" = 200 ] && grep -qE '"c0":20000(\.0)?[,}]' "$T/body" || fail "FK actuals: $(cat "$T/body")"
[ "$(panel 1 '{}' fk)" = 200 ] && grep -qE '"c0":249500(\.0)?[,}]' "$T/body" || fail "FK generated column: $(cat "$T/body")"
[ "$(panel 2 '{}' fk)" = 200 ] && grep -qE '"c0":2(\.0)?[,}]' "$T/body" || fail "FK self-referencing table: $(cat "$T/body")"
[ "$(panel 3 '{}' fk)" = 200 ] && grep -qE '"c0":50(\.0)?[,}]' "$T/body" || fail "FK view: $(cat "$T/body")"
[ "$(panel 4 '{}' fk)" = 200 ] && grep -qE '"c0":20000(\.0)?[,}]' "$T/body" || fail "FK sequence default: $(cat "$T/body")"
grep -q "table main.tree of 'fk' copied without its constraints" "$T/out" || fail "FK: no warning for tree: $(cat "$T/out")"
grep -qE "main\.(materials|actuals|suppliers) of 'fk' copied without" "$T/out" && fail "FK: a table lost its constraints: $(cat "$T/out")"
echo "SELECT anofox_serve_refresh($PORT);" >&3
for _ in $(seq 50); do grep -q "epoch 2" "$T/out" && break; sleep 0.2; done
grep -q "epoch 2" "$T/out" || fail "FK refresh: $(cat "$T/out")"
[ "$(panel 0 '{}' fk)" = 200 ] && grep -qE '"c0":20000(\.0)?[,}]' "$T/body" || fail "FK after refresh: $(cat "$T/body")"
stop
[ "$before" = "$(fkstate)" ] || fail "FK: the source database changed"
echo "OK: a database with foreign keys is snapshotted table by table, parents first; a self-referencing table without its constraints; the source is untouched"

# #388: warm connections. A panel request reuses an idle connection whose
# setup ran with the same setup inputs; it must answer exactly as a fresh one.
mkdir -p "$T/wdash"
cat > "$T/wdash/w.sql" <<'SQL'
-- @vars a, b
SET VARIABLE a = coalesce(getvariable('a'), 1);
CREATE OR REPLACE TEMP TABLE w AS SELECT getvariable('a') * 10 AS v;
SELECT v::BIGINT AS v ::METRIC FROM w;
SELECT coalesce(getvariable('b'), -1)::BIGINT AS b ::METRIC FROM w;
SQL
val() { grep -oE '"c0":-?[0-9]+' "$T/body" | head -1 | cut -d: -f2; }
warms() { grep -c "ran w#.*(warm)" "$T/out" || true; }
start "$T/main.duckdb" "-readonly" "SELECT anofox_serve_dashboards($(q "$T/wdash"), $PORT, '{\"cache_mb\":0}');" || fail "warm: server"
[ "$(panel 2 '{"a":1}' w)" = 200 ] && [ "$(val)" = 10 ] || fail "warm: first a=1: $(cat "$T/body")"
[ "$(panel 2 '{"a":1}' w)" = 200 ] && [ "$(val)" = 10 ] || fail "warm: second a=1: $(cat "$T/body")"
[ "$(warms)" -ge 1 ] || fail "warm: the second a=1 request didn't reuse a connection: $(cat "$T/out")"
[ "$(panel 2 '{"a":2}' w)" = 200 ] && [ "$(val)" = 20 ] || fail "warm: a=2 got another a's setup: $(cat "$T/body")"
[ "$(panel 2 '{"a":1}' w)" = 200 ] && [ "$(val)" = 10 ] || fail "warm: a=1 after a=2: $(cat "$T/body")"
[ "$(panel 2 '{}' w)" = 200 ] && [ "$(val)" = 10 ] || fail "warm: the setup's default: $(cat "$T/body")"
# A variable the setup doesn't read: same warm connection, but the previous
# request's value must not linger.
[ "$(panel 3 '{"a":1,"b":5}' w)" = 200 ] && [ "$(val)" = 5 ] || fail "warm: b=5: $(cat "$T/body")"
[ "$(panel 3 '{"a":1}' w)" = 200 ] && [ "$(val)" = -1 ] || fail "warm: b from an earlier request lingered: $(cat "$T/body")"
[ "$(panel 3 '{"a":1,"b":7}' w)" = 200 ] && [ "$(val)" = 7 ] || fail "warm: b=7: $(cat "$T/body")"
stop
start "$T/main.duckdb" "-readonly" "SELECT anofox_serve_dashboards($(q "$T/wdash"), $PORT, '{\"cache_mb\":0,\"warm\":0}');" || fail "warm 0: server"
for _ in 1 2 3; do [ "$(panel 2 '{"a":1}' w)" = 200 ] && [ "$(val)" = 10 ] || fail "warm 0: $(cat "$T/body")"; done
[ "$(warms)" = 0 ] || fail "warm 0 still reused a connection: $(cat "$T/out")"
stop
echo "OK: warm connections answer like fresh ones (setup inputs keyed, other variables reset; warm: 0 turns them off)"

# #407 part 2: window.__served exposes `path_vars`; an unused/unrelated
# viewer variable is accepted, not rejected (the client relies on this to
# carry a path_vars value to a dashboard that may not read it). Part 3:
# `nav_picker` refuses to start with `var` not in `path_vars` or together
# with an access policy, and its options drop a row whose value isn't a
# plain name (never sent to a page).
mkdir -p "$T/navdash" "$T/navruns"
cat > "$T/navdash/m1.sql" <<'SQL'
SELECT coalesce(getvariable('model'), 'none')::VARCHAR AS m ::TEXT_SMALL;
SQL
PICKER_SQL="SELECT * FROM (VALUES ('car-price','Car price'), ('../etc','Bad -- dropped'), ('van','Van')) t(value, label)"
start "$T/main.duckdb" "" "SELECT anofox_serve_dashboards($(q "$T/navdash"), $PORT,
  json_object('data', $(q "$T/navruns"), 'path_vars', ['model'],
    'nav_picker', json_object('var', 'model', 'label', 'Model', 'sql', $(q "$PICKER_SQL"))));" || fail "nav_picker server"
page_html="$(curl -s "http://127.0.0.1:$PORT/d/m1")"
echo "$page_html" | grep -oq '"pathVars":\["model"\]' || fail "#407: window.__served.pathVars missing: $page_html"
echo "$page_html" | grep -oq '"navPickers":\[{"type":"dropdown","var":"model","label":"Model","options":\[{"value":"car-price","label":"Car price"},{"value":"van","label":"Van"}\]}\]' \
  || fail "#556: window.__served.navPickers (the bad row must be dropped): $page_html"
# An unknown/unused viewer variable on a panel request is accepted (ignored),
# not rejected — the nav relies on this to carry a path_vars value to a
# dashboard that doesn't happen to read it.
[ "$(panel 0 '{"model":"car-price","totally_unrelated":"x"}' m1)" = 200 ] && grep -q '"c0":"car-price"' "$T/body" \
  || fail "#407: an unrelated viewer variable was rejected: $(cat "$T/body")"
stop
echo "OK: #407 part 2: window.__served.pathVars; an unused viewer variable is accepted, not rejected"
echo "OK: #407 part 3: nav_picker.options drops a row whose value isn't a plain path_vars name"

# nav_picker.var must be one of path_vars.
if QUIET=1 start "$T/main.duckdb" "" "SELECT anofox_serve_dashboards($(q "$T/navdash"), $PORT,
  json_object('data', $(q "$T/navruns"), 'path_vars', ['model'],
    'nav_picker', json_object('var', 'other', 'sql', 'SELECT 1, 2')));"; then
  fail "nav_picker.var not in path_vars was accepted"
fi
grep -q "options.nav_picker(s): 'other' must be one of options.path_vars" "$T/out" || fail "nav_picker.var check: $(cat "$T/out")"
exec 3>&-

# nav_picker refuses to start together with an access policy: its SQL runs
# directly, outside the policy's row/column rewrite.
cat > "$T/navpolicy.json" <<'JSON'
{"tables": {"sales": {"rows": true}}}
JSON
if QUIET=1 start "$T/main.duckdb" "" "CREATE TABLE sales AS SELECT 1 AS x;
  SELECT anofox_serve_dashboards($(q "$T/navdash"), $PORT,
  json_object('data', $(q "$T/navruns"), 'path_vars', ['model'], 'policy', $(q "$T/navpolicy.json"),
    'nav_picker', json_object('var', 'model', 'sql', 'SELECT 1, 2')));"; then
  fail "nav_picker with an access policy was accepted"
fi
grep -q "options.nav_picker(s) cannot be combined with options.policy" "$T/out" || fail "nav_picker + policy refusal: $(cat "$T/out")"
exec 3>&-
echo "OK: #407 part 3: nav_picker.var must be one of path_vars; refuses to start with an access policy"

# #556: `nav_pickers` (an array) with a `daterange` entry — `sql`'s
# (min, max) and `presets` computed from it land in window.__served
# .navPickers, and a request is real-date-checked, from<=to-checked, and
# out-of-range values are clamped (not refused) to [min, max].
mkdir -p "$T/drdash" "$T/drruns"
cat > "$T/drdash/w1.sql" <<'SQL'
SELECT coalesce(getvariable('view_from'), 'none')::VARCHAR || '/' || coalesce(getvariable('view_to'), 'none')::VARCHAR AS m ::TEXT_SMALL;
SQL
DR_SQL="SELECT DATE '2026-01-01', DATE '2026-01-31'"
start "$T/main.duckdb" "" "SELECT anofox_serve_dashboards($(q "$T/drdash"), $PORT,
  json_object('data', $(q "$T/drruns"), 'path_vars', ['view_from','view_to'],
    'nav_pickers', [json_object('type','daterange','vars',['view_from','view_to'],'label','Time window','sql',$(q "$DR_SQL"),'presets',['last 7d','all'])]));" \
  || fail "nav_pickers daterange server"
page_html="$(curl -s "http://127.0.0.1:$PORT/d/w1")"
echo "$page_html" | grep -oq '"navPickers":\[{"type":"daterange","vars":\["view_from","view_to"\],"label":"Time window","min":"2026-01-01","max":"2026-01-31","presets":\[{"key":"last 7d","label":"Last 7 days","from":"2026-01-25","to":"2026-01-31"},{"key":"all","label":"All","from":"2026-01-01","to":"2026-01-31"}\]}\]' \
  || fail "#556: window.__served.navPickers (daterange): $page_html"
[ "$(panel 0 '{"view_from":"2026-01-05","view_to":"2026-01-10"}' w1)" = 200 ] && grep -q '"c0":"2026-01-05/2026-01-10"' "$T/body" \
  || fail "#556: in-range daterange: $(cat "$T/body")"
[ "$(panel 0 '{"view_from":"2025-01-01","view_to":"2026-06-01"}' w1)" = 200 ] && grep -q '"c0":"2026-01-01/2026-01-31"' "$T/body" \
  || fail "#556: out-of-range daterange clamp: $(cat "$T/body")"
[ "$(panel 0 '{"view_from":"2026-01-20","view_to":"2026-01-10"}' w1)" = 400 ] \
  || fail "#556: from > to was accepted: $(cat "$T/body")"
[ "$(panel 0 '{"view_from":"2026-02-30","view_to":"2026-01-10"}' w1)" = 400 ] \
  || fail "#556: an invalid calendar date was accepted: $(cat "$T/body")"
stop
echo "OK: #556: nav_pickers (array) with a daterange entry — min/max/presets from sql, request-time real-date + from<=to check, clamp to [min,max]"
