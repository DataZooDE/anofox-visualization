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

# 3b. Prometheus metrics (#765): behind their own bearer token; without the
# option there is no such route.
if QUIET=1 start "$T/main.duckdb" "" "$(serve '{"metrics": {"token": "short"}}')"; then
  fail "a metrics token under 32 characters should refuse the start"
fi
grep -q "at least 32 printable" "$T/out" || fail "metrics: no reason given: $(cat "$T/out")"
exec 3>&-; wait "${PIDS[-1]}" 2>/dev/null || true; unset 'PIDS[-1]'
start "$T/main.duckdb" "" "$(serve '{}')" || fail "server without metrics"
[ "$(curl -s -o /dev/null -w "%{http_code}" "http://127.0.0.1:$PORT/metrics")" = 404 ] || fail "/metrics without options.metrics should 404"
stop
MTOKEN="$(printf 'm%.0s' $(seq 40))"
start "$T/main.duckdb" "" "$(serve "{\"metrics\": {\"token\": \"$MTOKEN\"}}")" || fail "metrics server"
[ "$(panel 0)" = 200 ] && [ "$(panel 0)" = 200 ] || fail "metrics: panel 0"
[ "$(panel 9)" = 404 ] || fail "metrics: a panel that does not exist"
met() { curl -s -o "$T/met" -D "$T/meth" -w "%{http_code}" "$@" "http://127.0.0.1:$PORT/metrics"; }
[ "$(met)" = 401 ] && grep -qi "^www-authenticate: bearer" "$T/meth" || fail "/metrics without a token: $(cat "$T/meth")"
[ "$(met -H "Authorization: Bearer ${MTOKEN}x")" = 401 ] || fail "/metrics with a wrong token"
[ "$(met -X POST -H "Authorization: Bearer $MTOKEN")" = 405 ] || fail "POST /metrics should be 405"
[ "$(met -H "Authorization: Bearer $MTOKEN")" = 200 ] || fail "/metrics with the token: $(cat "$T/met")"
grep -qi "^content-type: text/plain; version=0.0.4" "$T/meth" || fail "/metrics content type: $(cat "$T/meth")"
for k in 'anofox_http_requests_total{route="panel",code="2xx"} 2' 'anofox_http_requests_total{route="panel",code="4xx"} 1' \
         'anofox_http_request_duration_seconds_count{route="panel"} 3' 'anofox_panel_requests_total{cache="run"} 1' \
         'anofox_panel_requests_total{cache="hit"} 1' 'anofox_http_denied_total{reason="metrics_token"} 2' \
         'anofox_snapshot_epoch 1' 'anofox_dashboards 1' 'anofox_build_info{version="v' 'anofox_queries_running 0'; do
  grep -qF "$k" "$T/met" || fail "/metrics lacks $k: $(cat "$T/met")"
done
grep -q '/api\|"t"' "$T/met" && fail "/metrics names a path or a dashboard: $(cat "$T/met")"
stop
echo "OK: /metrics — 404 without the option, bearer token (401/405), counters, histograms, gauges; no request data in labels"

# 3c. Exports (#764): a dashboard (or one tab) as SVG, a named panel as CSV
# / JSON — through the same panel path as the page (scope, -- @vars, inputs'
# defaults, the viewer's own values).
mkdir "$T/xdash"
cat > "$T/xdash/ex.sql" <<'SQL'
-- @title Export test
-- @vars region
SELECT 'Overview'::TAB;
SELECT region ::DROPDOWN FROM (VALUES ('EU'), ('US')) t(region);
SELECT 6::COL; SELECT 200::HEIGHT;
-- @name totals
SELECT region AS "Region" ::XAXIS, sum(v) AS "Revenue" ::BARCHART FROM ex_t
WHERE region = getvariable('region') GROUP BY 1 ORDER BY 1;
-- @name evil
SELECT '=cmd()' AS "Note" ::TABLE;
-- @name styled
SELECT region AS "Region" ::XAXIS, sum(v) AS "Revenue" ::BARCHART_STACKED, region ::CATEGORY,
       'EU:bad; good'::COLORS, 1::FLIP, '€'::YFORMAT, 'Styled'::TITLE, 'why'::INFO,
       ('?v.region=' || region)::LINK FROM ex_t WHERE region = getvariable('region') GROUP BY region;
SELECT 'Details'::TAB;
SELECT 'details-marker-text' ::MARKDOWN;
SQL
cat > "$T/xdash/other.sql" <<'SQL'
SELECT 'other-dashboard-text' ::MARKDOWN;
SQL
"$DUCKDB" "$T/ex.duckdb" -c "CREATE TABLE ex_t AS SELECT * FROM (VALUES ('EU', 10), ('EU', 5), ('US', 7)) t(region, v);" >/dev/null
xserve() { echo "SELECT anofox_serve_dashboards($(q "$T/xdash"), $PORT, $(q "$1"));"; }
start "$T/ex.duckdb" "-readonly" "$(xserve '{}')" || fail "export server"
xget() { curl -s -o "$T/x" -D "$T/xh" -w "%{http_code}" "$@"; }
X="http://127.0.0.1:$PORT/api/export"
[ "$(xget "$X/ex.svg")" = 200 ] || fail "export svg: $(cat "$T/x")"
grep -qi "^content-type: image/svg+xml" "$T/xh" && grep -qi '^content-disposition: attachment; filename="ex.svg"' "$T/xh" \
  && grep -qi "^content-security-policy: default-src 'none'.*sandbox" "$T/xh" || fail "export svg headers: $(cat "$T/xh")"
grep -q "<svg" "$T/x" && grep -q ">EU<" "$T/x" && grep -q "details-marker-text" "$T/x" || fail "export svg: the default region EU and both tabs: $(head -c 600 "$T/x")"
grep -q ">US<" "$T/x" && fail "export svg: US drawn although the default is EU"
[ "$(xget "$X/ex.svg?v.region=US")" = 200 ] && grep -q ">US<" "$T/x" && ! grep -q ">EU<" "$T/x" || fail "export svg with v.region=US"
[ "$(xget "$X/ex.svg?tab=details")" = 200 ] && grep -q "details-marker-text" "$T/x" && ! grep -q ">EU<" "$T/x" || fail "export one tab"
[ "$(xget "$X/ex.svg?tab=nope")" = 404 ] || fail "export: an unknown tab should 404"
[ "$(xget "$X/ex/totals.csv")" = 200 ] || fail "export csv: $(cat "$T/x")"
grep -qi "^content-type: text/csv" "$T/xh" || fail "export csv content type"
[ "$(tr -d '\r' < "$T/x")" = "$(printf '"Region","Revenue"\n"EU","15"')" ] || fail "export csv body (the input's default applies): $(cat "$T/x")"
[ "$(xget "$X/ex/totals.json?v.region=US")" = 200 ] && [ "$(cat "$T/x")" = '[{"Region":"US","Revenue":7.0}]' ] \
  || fail "export json with v.region=US: $(cat "$T/x")"
# #817: the data only — chart options and the panel's text are left out,
# an unnamed column is named after its role.
[ "$(xget "$X/ex/styled.json")" = 200 ] && [ "$(cat "$T/x")" = '[{"Region":"EU","Revenue":15.0,"category":"EU","link":"?v.region=EU"}]' ] \
  || fail "export json: data columns only: $(cat "$T/x")"
[ "$(xget "$X/ex/styled.csv")" = 200 ] && [ "$(head -1 "$T/x" | tr -d '\r')" = '"Region","Revenue","category","link"' ] \
  || fail "export csv: data columns only: $(cat "$T/x")"
[ "$(xget "$X/ex/evil.csv")" = 200 ] && grep -qF "\"'=cmd()\"" "$T/x" || fail "export csv: a formula cell is neutralised: $(cat "$T/x")"
[ "$(xget "$X/ex/nope.csv")" = 404 ] || fail "export: an unknown panel name should 404"
[ "$(xget "$X/ex.csv")" = 404 ] || fail "export: a dashboard is not a csv"
[ "$(xget "$X/ex.png?scale=1")" = 200 ] && grep -qi "^content-type: image/png" "$T/xh" \
  && [ "$(head -c 8 "$T/x" | od -An -tx1 | tr -d ' \n')" = 89504e470d0a1a0a ] || fail "export png: $(head -c 200 "$T/x")"
# 1200 px wide at scale 1 (IHDR width)
[ "$(dd if="$T/x" bs=1 skip=16 count=4 2>/dev/null | od -An -tu1 | awk '{print $1*16777216+$2*65536+$3*256+$4}')" = 1200 ] || fail "export png width"
[ "$(xget "$X/ex.pdf?tab=details")" = 200 ] && grep -qi "^content-type: application/pdf" "$T/xh" && head -c 5 "$T/x" | grep -q "%PDF-" \
  || fail "export pdf: $(head -c 200 "$T/x")"
[ "$(xget "$X/ex.pdf?scale=2")" = 400 ] || fail "export: scale is for png only"
[ "$(xget "$X/ex.png?scale=9")" = 400 ] || fail "export: scale out of range"
# One named panel drawn alone (#815): full width (its ::COL ignored), its
# ::HEIGHT kept, no dashboard title, nothing from other panels or tabs.
[ "$(xget "$X/ex/totals.svg")" = 200 ] && grep -qi '^content-disposition: attachment; filename="ex-totals.svg"' "$T/xh" \
  && grep -qi "^content-security-policy: default-src 'none'.*sandbox" "$T/xh" || fail "export panel svg: $(head -c 300 "$T/x")"
grep -q ">EU<" "$T/x" && ! grep -q ">US<" "$T/x" && ! grep -q "Export test" "$T/x" && ! grep -q "details-marker-text" "$T/x" \
  && ! grep -q "cmd()" "$T/x" || fail "export panel svg: only the panel, default region EU: $(head -c 600 "$T/x")"
head -c 300 "$T/x" | grep -q 'width="640" height="232"' || fail "export panel svg: 640 wide, ::HEIGHT 200 + 2 x 16: $(head -c 300 "$T/x")"
[ "$(xget "$X/ex/totals.svg?v.region=US&width=320")" = 200 ] && grep -q ">US<" "$T/x" || fail "export panel svg with v.region=US, width 320"
[ "$(xget "$X/ex/totals.png?scale=1")" = 200 ] && grep -qi "^content-type: image/png" "$T/xh" \
  && [ "$(dd if="$T/x" bs=1 skip=16 count=4 2>/dev/null | od -An -tu1 | awk '{print $1*16777216+$2*65536+$3*256+$4}')" = 640 ] \
  || fail "export panel png 640 px: $(head -c 200 "$T/x")"
[ "$(xget "$X/ex/totals.pdf")" = 404 ] || fail "export: a panel is not a pdf"
[ "$(xget "$X/ex/totals.png?tab=details")" = 400 ] || fail "export: tab doesn't apply to a panel"
[ "$(xget "$X/ex/totals.png?width=200")" = 400 ] || fail "export: a panel narrower than 320 should 400"
[ "$(xget "$X/ex/nope.png")" = 404 ] || fail "export: an unknown panel image should 404"
[ "$(xget "$X/ex.svg?x=1")" = 400 ] || fail "export: an unknown parameter should 400"
[ "$(xget "$X/ex.svg?v.__claim_user=admin")" = 400 ] || fail "export: a reserved variable should 400"
[ "$(xget "$X/ex.svg?width=10")" = 400 ] || fail "export: a bad width should 400"
[ "$(xget "$X/nope.svg")" = 404 ] || fail "export: an unknown dashboard should 404"
[ "$(xget -X POST "$X/ex.svg")" = 405 ] || fail "export: POST should 405"
stop
# With a login: a credential scoped to one dashboard exports only that one.
start "$T/ex.duckdb" "-readonly" "$(xserve '{"auth": {"mode": "share", "dashboards": {"ex": {"access": "public"}}}, "policy": "none"}')" \
  || fail "export server with share links"
[ "$(xget "$X/ex.svg")" = 401 ] || fail "export without a credential should 401"
sess=$(curl -s "http://127.0.0.1:$PORT/d/ex" | grep -o '"session":"[^"]*"' | head -1 | sed 's/.*:"//; s/"$//')
[ -n "$sess" ] || fail "no page session on the shared page"
[ "$(xget -H "X-Anofox-Session: $sess" "$X/ex.svg")" = 200 ] || fail "export with the page session: $(cat "$T/x")"
[ "$(xget -H "X-Anofox-Session: $sess" "$X/other.svg")" = 403 ] || fail "export of another dashboard should 403"
[ "$(xget -H "X-Anofox-Session: $sess" "$X/ex/totals.csv")" = 200 ] || fail "export csv with the page session"
[ "$(xget -H "X-Anofox-Session: $sess" "$X/ex/totals.png")" = 200 ] || fail "export panel png with the page session"
[ "$(xget -H "X-Anofox-Session: $sess" "$X/other/x.png")" = 403 ] || fail "export panel png of another dashboard should 403"
stop
echo "OK: exports — SVG/PNG/PDF (dashboard, one tab, variables, inputs' defaults), SVG/PNG/CSV/JSON of a named panel, formula cells neutralised, scope and login"

# 3d. Tasks (#767): tasks/*.task.sql on a schedule, writing only the
# databases in tasks.write (all others must be attached READ_ONLY), then a
# refresh; the run log and /metrics.
mkdir -p "$T/tkdash/tasks"
cat > "$T/tkdash/t.sql" <<'SQL'
SELECT count(*)::BIGINT AS n ::METRIC FROM stage.loaded;
SQL
cat > "$T/tkdash/tasks/load.task.sql" <<'SQL'
SELECT INTERVAL '2 seconds' ::SCHEDULE;
CREATE OR REPLACE TABLE stage.loaded AS SELECT * FROM range(5);
SELECT 1 ::REFRESH;
SQL
# runs first: fails inside a transaction, which must not stay open on the
# shared connection (load and the refresh would fail on it)
cat > "$T/tkdash/tasks/a_txn.task.sql" <<'SQL'
SELECT 'init' ::SCHEDULE;
BEGIN TRANSACTION;
INSERT INTO stage.loaded VALUES (99);
SELECT error('boom');
COMMIT;
SQL
cat > "$T/tkdash/tasks/bad.task.sql" <<'SQL'
SELECT 'init' ::SCHEDULE;
CREATE TABLE warehouse.sneaky AS SELECT 1 AS x;
SQL
"$DUCKDB" "$T/tk.duckdb" -c "SELECT 1;" >/dev/null
"$DUCKDB" "$T/stage.duckdb" -c "CREATE TABLE loaded AS SELECT 1 AS range;" >/dev/null
"$DUCKDB" "$T/wh.duckdb" -c "CREATE TABLE t AS SELECT 1 AS x;" >/dev/null
tkattach="ATTACH $(q "$T/stage.duckdb") AS stage; ATTACH $(q "$T/wh.duckdb") AS warehouse (READ_ONLY);"
tkserve() { echo "$tkattach SELECT anofox_serve_dashboards($(q "$T/tkdash"), $PORT, $(q "$1"));"; }
# (no -readonly: the CLI flag makes every later ATTACH read-only too; the
# session's own database "tk" is writable, so it goes in tasks.write.)
if QUIET=1 start "$T/tk.duckdb" "" "$(tkserve '{"tasks": {"run": true, "write": ["tk"]}}')"; then
  fail "tasks: stage is read-write but not in tasks.write — the start should be refused"
fi
grep -q "'stage' is attached read-write but not in tasks.write" "$T/out" || fail "tasks: no reason given: $(cat "$T/out")"
exec 3>&-; wait "${PIDS[-1]}" 2>/dev/null || true; unset 'PIDS[-1]'
MTOKEN="$(printf 'm%.0s' $(seq 40))"
start "$T/tk.duckdb" "" "$(tkserve "{\"tasks\": {\"run\": true, \"write\": [\"tk\", \"stage\"], \"log\": \"$T/tasks.jsonl\"}, \"metrics\": {\"token\": \"$MTOKEN\"}}")" \
  || fail "tasks server"
for _ in $(seq 60); do [ "$(panel 0 '{}' t)" = 200 ] && grep -q '"c0":5' "$T/body" && break; sleep 0.25; done
grep -q '"c0":5' "$T/body" || fail "tasks: the panel should see the task's 5 rows after its refresh: $(cat "$T/body") $(tail -5 "$T/out")"
for _ in $(seq 20); do grep -q '"task":"bad"' "$T/tasks.jsonl" 2>/dev/null && break; sleep 0.25; done
grep '"task":"bad"' "$T/tasks.jsonl" | grep -q '"ok":false' || fail "tasks: a write to a READ_ONLY database should fail: $(cat "$T/tasks.jsonl")"
grep '"task":"a_txn"' "$T/tasks.jsonl" | grep -q '"ok":false' || fail "tasks: a_txn should fail: $(cat "$T/tasks.jsonl")"
grep '"task":"load"' "$T/tasks.jsonl" | grep -q '"ok":true' || fail "tasks: load should succeed: $(cat "$T/tasks.jsonl")"
curl -s -H "Authorization: Bearer $MTOKEN" "http://127.0.0.1:$PORT/metrics" > "$T/met"
grep -qF 'anofox_task_failures{task="bad"} 1' "$T/met" && grep -qF 'anofox_task_consecutive_failures{task="load"} 0' "$T/met" \
  || fail "tasks: /metrics: $(grep anofox_task "$T/met")"
stop
"$DUCKDB" "$T/wh.duckdb" -c "SELECT count(*) FROM duckdb_tables() WHERE table_name = 'sneaky';" | grep -q " 0 " \
  || fail "tasks: the READ_ONLY database was written"
echo "OK: tasks — ::SCHEDULE, write only to tasks.write (READ_ONLY check at start), ::REFRESH, run log, /metrics"

# 3e. Reports (#768 R1): options.notify + options.reports, rendered through
# the export path as the report's principal, sent to a local SMTP sink and a
# local webhook receiver (both fakes on 127.0.0.1; nothing leaves the host).
mkdir -p "$T/rpdash"
cat > "$T/rpdash/r.sql" <<'SQL'
-- @title EU sales
SELECT 'Data as of 2026-10-30' ::ASOF;
SELECT 42::BIGINT AS n ::METRIC;
-- @name who
SELECT getvariable('__claim_user') AS u, getvariable('__claim_region') AS region, 'a,b' AS c ::TABLE;
SQL
cat > "$T/sinks.py" <<'PY'
import hashlib, hmac, json, os, socketserver, sys, threading, http.server
out = sys.argv[1]
class Smtp(socketserver.StreamRequestHandler):
    def handle(self):
        w = lambda s: (self.wfile.write((s + "\r\n").encode()), self.wfile.flush())
        w("220 sink")
        rcpts, auth = [], None
        while True:
            line = self.rfile.readline()
            if not line:
                return
            cmd = line.decode().rstrip("\r\n")
            u = cmd.upper()
            if u.startswith("EHLO"):
                w("250-sink"); w("250 AUTH PLAIN LOGIN")
            elif u.startswith("AUTH PLAIN"):
                auth = cmd.split(" ", 2)[2]; w("235 ok")
            elif u.startswith("MAIL FROM"):
                w("250 ok")
            elif u.startswith("RCPT TO"):
                rcpts.append(cmd[8:].strip()); w("250 ok")
            elif u == "DATA":
                w("354 go")
                data = b""
                while True:
                    l = self.rfile.readline()
                    if l in (b".\r\n", b""):
                        break
                    data += l[1:] if l.startswith(b"..") else l
                with open(os.path.join(out, "mail.json"), "a") as f:
                    f.write(json.dumps({"rcpts": rcpts, "auth": auth, "data": data.decode()}) + "\n")
                w("250 queued")
            elif u == "QUIT":
                w("221 bye"); return
            else:
                w("500 what")
def save(name, obj):
    with open(os.path.join(out, name), "a") as f:
        f.write(json.dumps(obj) + "\n")
class Hook(http.server.BaseHTTPRequestHandler):
    # A fake Slack Web API (/api/…, uploads at /upload/…) and Teams (/teams…)
    # next to the signed webhook receiver.
    def answer(self, obj):
        b = json.dumps(obj).encode()
        self.send_response(200); self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(b))); self.end_headers(); self.wfile.write(b)
    def do_GET(self):
        import urllib.parse
        u = urllib.parse.urlparse(self.path)
        q = dict(urllib.parse.parse_qsl(u.query))
        if self.headers.get("Authorization") != "Bearer xoxb-test-token":
            return self.answer({"ok": False, "error": "invalid_auth"})
        if u.path == "/api/conversations.info":
            return self.answer({"ok": True, "channel": {"id": q["channel"], "is_ext_shared": q["channel"] == "CEXTERNAL1"}})
        self.answer({"ok": False, "error": "unknown_method"})
    def do_POST(self):
        import urllib.parse
        body = self.rfile.read(int(self.headers["Content-Length"]))
        if self.path.startswith("/upload/"):
            save("uploads.json", {"path": self.path, "len": len(body), "head": body[:4].hex()})
            self.send_response(200); self.end_headers(); return
        if self.path.startswith("/teams"):
            save("teams.json", {"path": self.path, "body": json.loads(body)})
            self.send_response(202); self.end_headers(); return
        if self.path.startswith("/api/"):
            if self.headers.get("Authorization") != "Bearer xoxb-test-token":
                return self.answer({"ok": False, "error": "invalid_auth"})
            m = self.path[5:]
            if self.headers.get("Content-Type", "").startswith("application/json"):
                args = json.loads(body)
            else:
                args = dict(urllib.parse.parse_qsl(body.decode()))
            save("slack.json", {"method": m, "args": args})
            if m == "files.getUploadURLExternal":
                n = sum(1 for _ in open(os.path.join(out, "slack.json")))
                return self.answer({"ok": True, "upload_url": f"http://127.0.0.1:{self.server.server_address[1]}/upload/F{n}", "file_id": f"F{n}"})
            return self.answer({"ok": True})
        ts = self.headers.get("X-Anofox-Timestamp", "")
        want = "sha256=" + hmac.new(b"0123456789abcdef-secret", ts.encode() + b"." + body, hashlib.sha256).hexdigest()
        ok = hmac.compare_digest(want, self.headers.get("X-Anofox-Signature", ""))
        with open(os.path.join(out, "hook.json"), "a") as f:
            f.write(json.dumps({"sig_ok": ok, "path": self.path, "body": json.loads(body)}) + "\n")
        self.send_response(204); self.end_headers()
    def log_message(self, *a):
        pass
socketserver.ThreadingTCPServer.allow_reuse_address = True
s = socketserver.ThreadingTCPServer(("127.0.0.1", 0), Smtp)
h = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Hook)
for srv in (s, h):
    threading.Thread(target=srv.serve_forever, daemon=True).start()
with open(os.path.join(out, "ports"), "w") as f:
    f.write(f"{s.server_address[1]} {h.server_address[1]}\n")
threading.Event().wait()
PY
mkdir -p "$T/sink"
python3 "$T/sinks.py" "$T/sink" & SINK_PID=$!
for _ in $(seq 50); do [ -s "$T/sink/ports" ] && break; sleep 0.1; done
read -r SMTP_PORT HOOK_PORT < "$T/sink/ports"
export R_SMTP_USER=mailer R_SMTP_PASS=hunter2 R_HOOK_URL="http://127.0.0.1:$HOOK_PORT/hook?token=t0ps3cret" R_HOOK_SECRET=0123456789abcdef-secret
rpopts() {
  cat <<JSON
{"notify": {"run": true, "log": "$T/reports.jsonl", "link": "https://dash.example.com",
            "smtp": {"host": "127.0.0.1", "port": $SMTP_PORT, "tls": "none", "from": "reports@example.com",
                     "user_env": "R_SMTP_USER", "password_env": "R_SMTP_PASS"},
            "webhooks": {"ops": {"url_env": "R_HOOK_URL", "secret_env": "R_HOOK_SECRET"}}},
 "metrics": {"token": "$MTOKEN"},
 "reports": [{"name": "eu", "dashboard": "r", "formats": ["png", "pdf", "csv:who"], "schedule": "init",
              "as": {"user": "report:eu", "groups": ["sales"], "claims": {"region": "EU"}},
              "to": [{"email": "a@example.com"}, {"email": "b@example.com"}, {"webhook": "ops"}]},
             {"name": "later", "dashboard": "r", "formats": ["json:who"], "schedule": "every monday 07:00 UTC",
              "as": {"user": "report:later"}, "to": [{"webhook": "ops"}]}$1]}
JSON
}
# a report naming a panel the dashboard doesn't have refuses the start
if QUIET=1 start "$T/tk.duckdb" "" "SELECT anofox_serve_dashboards($(q "$T/rpdash"), $PORT, $(q "$(rpopts ', {"name": "bad", "dashboard": "r", "formats": ["csv:nope"], "schedule": "init", "as": {"user": "report:x"}, "to": [{"webhook": "ops"}]}')"));"; then
  fail "reports: a csv format naming no panel should refuse the start"
fi
grep -q "has no panel named 'nope'" "$T/out" || fail "reports: no reason given: $(cat "$T/out")"
exec 3>&-; wait "${PIDS[-1]}" 2>/dev/null || true; unset 'PIDS[-1]'
start "$T/tk.duckdb" "" "SELECT anofox_serve_dashboards($(q "$T/rpdash"), $PORT, $(q "$(rpopts '')"));" || fail "reports server"
grep -q "report 'eu' runs as report:eu (groups: \[sales\], claims: \[region=EU\]) to 3 recipient(s)" "$T/out" \
  || fail "reports: the start message names each report's principal: $(cat "$T/out")"
for _ in $(seq 100); do grep -q '"report":"eu"' "$T/reports.jsonl" 2>/dev/null && break; sleep 0.2; done
grep '"report":"eu"' "$T/reports.jsonl" | grep -q '"ok":true.*"sent":3' \
  || fail "reports: eu should reach 3 recipients: $(cat "$T/reports.jsonl" 2>/dev/null) $(tail -5 "$T/out")"
python3 - "$T/sink" <<'PY' || fail "reports: the mail or the webhook is wrong"
import base64, email, json, sys
d = sys.argv[1]
m = [json.loads(l) for l in open(f"{d}/mail.json")]
assert len(m) == 1, m
m = m[0]
assert m["rcpts"] == ["<a@example.com>", "<b@example.com>"], m["rcpts"]
assert base64.b64decode(m["auth"]) == b"\0mailer\0hunter2"
msg = email.message_from_string(m["data"])
assert msg["To"] == "undisclosed-recipients:;" and msg["Cc"] is None and msg["Bcc"] is None, "recipients in a header"
assert msg["Subject"] == "EU sales — eu" or "=?UTF-8?" in msg["Subject"], msg["Subject"]
parts = {p.get_content_type(): p for p in msg.walk()}
for t in ("text/plain", "text/html", "image/png", "application/pdf", "text/csv"):
    assert t in parts, (t, list(parts))
assert parts["image/png"].get_payload(decode=True)[:4] == b"\x89PNG"
assert parts["application/pdf"].get_payload(decode=True)[:4] == b"%PDF"
csv = parts["text/csv"].get_payload(decode=True).decode()
assert csv.splitlines() == ['"u","region","c"', '"report:eu","EU","a,b"'], csv
text = parts["text/plain"].get_payload(decode=True).decode()
assert "Data as of 2026-10-30" in text and "https://dash.example.com/d/r" in text, text
h = [json.loads(l) for l in open(f"{d}/hook.json")]
assert len(h) == 1 and h[0]["sig_ok"] and h[0]["path"] == "/hook?token=t0ps3cret", h
b = h[0]["body"]
assert b["kind"] == "report" and b["name"] == "eu" and b["status"] is None and b["as_of"] == "Data as of 2026-10-30", b
assert [f["name"] for f in b["files"]] == ["r.png", "r.pdf", "r-who.csv"], b["files"]
PY
# the manual trigger (the session that started the server)
echo "SELECT anofox_serve_report($PORT, 'later');" >&3
for _ in $(seq 50); do grep -q '"report":"later"' "$T/reports.jsonl" 2>/dev/null && break; sleep 0.2; done
grep '"report":"later"' "$T/reports.jsonl" | grep -q '"manual":true' || fail "reports: manual run: $(cat "$T/reports.jsonl")"
[ "$(wc -l < "$T/sink/hook.json")" = 2 ] && tail -1 "$T/sink/hook.json" | grep -q '"base64"' || fail "reports: the manual webhook: $(cat "$T/sink/hook.json")"
grep -q "report 'later' sent to 1 recipient(s)" "$T/out" || fail "reports: manual result: $(tail -3 "$T/out")"
curl -s -H "Authorization: Bearer $MTOKEN" "http://127.0.0.1:$PORT/metrics" > "$T/met"
grep -qF 'anofox_report_runs{report="eu"} 1' "$T/met" && grep -qF 'anofox_report_consecutive_failures{report="later"} 0' "$T/met" \
  || fail "reports: /metrics: $(grep anofox_report "$T/met")"
# the hook's token never reaches a log
! grep -q t0ps3cret "$T/out" "$T/reports.jsonl" || fail "reports: the webhook URL's token was logged"
# a viewer can't claim a report principal (proxy header … none here: no auth); the panel API still serves viewers normally
[ "$(panel 1 '{}' r)" = 200 ] || fail "reports: a viewer's panel request"
stop
kill "$SINK_PID" 2>/dev/null; wait "$SINK_PID" 2>/dev/null || true
unset R_SMTP_USER R_SMTP_PASS R_HOOK_URL R_HOOK_SECRET
echo "OK: reports — e-mail (Bcc, inline PNG, PDF + CSV attached) and a signed webhook as the report's principal, ::ASOF + link, manual run, run log, /metrics"

# 3f. Alerts (#768 R2): a panel's rows, sent when they appear, change or go
# (D5), its state in a file across restarts; a failing task to on_failure.
mkdir -p "$T/aldash/tasks" "$T/sink2"
cat > "$T/aldash/stock.sql" <<'SQL'
-- @title Stock
-- @name below_safety
SELECT item, qty ::TABLE FROM stock WHERE qty < 5 ORDER BY item;
SQL
cat > "$T/aldash/tasks/broken.task.sql" <<'SQL'
SELECT 'init' ::SCHEDULE;
SELECT error('the inbox is empty');
SQL
"$DUCKDB" "$T/al.duckdb" -c "CREATE TABLE stock AS SELECT * FROM (VALUES ('bolt', 2), ('nut', 9)) t(item, qty);" >/dev/null
python3 "$T/sinks.py" "$T/sink2" & SINK_PID=$!
for _ in $(seq 50); do [ -s "$T/sink2/ports" ] && break; sleep 0.1; done
read -r _ HOOK_PORT < "$T/sink2/ports"
export R_HOOK_URL="http://127.0.0.1:$HOOK_PORT/hook" R_HOOK_SECRET=0123456789abcdef-secret
alopts="{\"notify\": {\"run\": true, \"log\": \"$T/alerts.jsonl\", \"state\": \"$T/alert-state.json\",
          \"webhooks\": {\"ops\": {\"url_env\": \"R_HOOK_URL\", \"secret_env\": \"R_HOOK_SECRET\"}}},
 \"tasks\": {\"run\": true, \"write\": [\"al\"], \"log\": \"$T/altasks.jsonl\", \"on_failure\": {\"to\": [{\"webhook\": \"ops\"}]}},
 \"metrics\": {\"token\": \"$MTOKEN\"},
 \"alerts\": [{\"name\": \"low\", \"dashboard\": \"stock\", \"panel\": \"below_safety\", \"check\": \"every 1 hours\",
              \"as\": {\"user\": \"report:ops\"}, \"attach\": [\"csv\"], \"to\": [{\"webhook\": \"ops\"}]}]}"
hooks() { [ -s "$T/sink2/hook.json" ] && wc -l < "$T/sink2/hook.json" || echo 0; }
check() {
  echo "SELECT anofox_serve_report($PORT, 'low');" >&3
  for _ in $(seq 50); do
    n="$(grep -c '"alert":"low"' "$T/alerts.jsonl" 2>/dev/null || true)"
    [ "${n:-0}" -ge "$1" ] && return
    sleep 0.2
  done
}
start "$T/al.duckdb" "" "SELECT anofox_serve_dashboards($(q "$T/aldash"), $PORT, $(q "$alopts"));" || fail "alerts server"
grep -q "alert 'low' runs as report:ops to 1 recipient(s)" "$T/out" || fail "alerts: start message: $(cat "$T/out")"
# the failing task's message (FAILED, the error)
for _ in $(seq 50); do [ "$(hooks)" -ge 1 ] && break; sleep 0.2; done
python3 - "$T/sink2/hook.json" <<'PY' || fail "alerts: the task's failure message"
import json, sys
h = [json.loads(l) for l in open(sys.argv[1])]
b = h[0]["body"]
assert h[0]["sig_ok"] and b["kind"] == "task" and b["name"] == "broken" and b["status"] == "FAILED", b
assert "the inbox is empty" in b["detail"], b
PY
check 1
grep -q "alert 'low': FIRING (1 row(s)), sent to 1 recipient(s)" "$T/out" || fail "alerts: first check: $(tail -3 "$T/out")"
check 2
grep -q "alert 'low': no change (1 row(s))" "$T/out" || fail "alerts: an unchanged check sends nothing: $(tail -3 "$T/out")"
[ "$(hooks)" = 2 ] || fail "alerts: two messages so far (task, FIRING): $(cat "$T/sink2/hook.json")"
python3 - "$T/sink2/hook.json" <<'PY' || fail "alerts: the FIRING message"
import base64, json, sys
b = [json.loads(l) for l in open(sys.argv[1])][1]["body"]
assert b["kind"] == "alert" and b["status"] == "FIRING" and b["detail"] == "1 row(s) in 'below_safety'", b
assert [f["name"] for f in b["files"]] == ["stock-below_safety.csv"], b["files"]
assert base64.b64decode(b["files"][0]["base64"]).decode().splitlines() == ['"item","qty"', '"bolt","2"'], b["files"]
PY
curl -s -H "Authorization: Bearer $MTOKEN" "http://127.0.0.1:$PORT/metrics" | grep -qF 'anofox_alert_firing{alert="low"} 1' \
  || fail "alerts: /metrics firing"
stop
# a restart doesn't send it again (the state file)
start "$T/al.duckdb" "" "SELECT anofox_serve_dashboards($(q "$T/aldash"), $PORT, $(q "$alopts"));" || fail "alerts server (restart)"
check 3
grep -q "alert 'low': no change (1 row(s))" "$T/out" || fail "alerts: after a restart: $(tail -3 "$T/out")"
# the rows go: RESOLVED, once
echo "UPDATE stock SET qty = 7; SELECT anofox_serve_refresh($PORT);" >&3
check 4
grep -q "alert 'low': RESOLVED (0 row(s)), sent to 1 recipient(s)" "$T/out" || fail "alerts: resolved: $(tail -3 "$T/out")"
check 5
[ "$(grep -c "alert 'low': no change (0 row(s))" "$T/out")" = 1 ] || fail "alerts: resolved only once: $(tail -3 "$T/out")"
# (the restart ran the 'init' task again: a new process tells its first failure again)
tail -1 "$T/sink2/hook.json" | grep -q '"status": "RESOLVED"' || fail "alerts: the last message is RESOLVED: $(tail -1 "$T/sink2/hook.json")"
stop
kill "$SINK_PID" 2>/dev/null; wait "$SINK_PID" 2>/dev/null || true
unset R_HOOK_URL R_HOOK_SECRET
echo "OK: alerts — FIRING once, no repeat on an unchanged check or a restart (state file), RESOLVED once; a failing task's message (on_failure)"

# 3g. Slack and Teams (#768 R3), against local fakes: a report's files
# uploaded to a Slack channel as bytes, an Adaptive Card to Teams, a Slack
# Connect channel refused, a failing task's text message to Slack.
mkdir -p "$T/sldash/tasks" "$T/sink3"
cp "$T/rpdash/r.sql" "$T/sldash/r.sql"
cat > "$T/sldash/tasks/broken.task.sql" <<'SQL'
SELECT 'init' ::SCHEDULE;
SELECT error('no <!channel> here');
SQL
python3 "$T/sinks.py" "$T/sink3" & SINK_PID=$!
for _ in $(seq 50); do [ -s "$T/sink3/ports" ] && break; sleep 0.1; done
read -r _ HOOK_PORT < "$T/sink3/ports"
export R_SLACK=xoxb-test-token R_TEAMS="http://127.0.0.1:$HOOK_PORT/teams?sig=t0ps3cret"
slopts="{\"notify\": {\"run\": true, \"log\": \"$T/slack-runs.jsonl\", \"link\": \"https://dash.example.com\",
          \"slack\": {\"token_env\": \"R_SLACK\", \"api_url\": \"http://127.0.0.1:$HOOK_PORT/api\"},
          \"teams\": {\"sales\": {\"url_env\": \"R_TEAMS\"}}},
 \"tasks\": {\"run\": true, \"write\": [\"tk\"], \"on_failure\": {\"to\": [{\"slack\": \"C0123ABCD\"}]}},
 \"reports\": [{\"name\": \"sl\", \"dashboard\": \"r\", \"formats\": [\"png\", \"csv:who\"], \"schedule\": \"every day 06:30\",
               \"as\": {\"user\": \"report:sl\", \"claims\": {\"region\": \"EU\"}},
               \"to\": [{\"slack\": \"C0123ABCD\"}, {\"teams\": \"sales\"}]},
              {\"name\": \"ext\", \"dashboard\": \"r\", \"formats\": [\"png\"], \"schedule\": \"every day 06:30\",
               \"as\": {\"user\": \"report:ext\"}, \"to\": [{\"slack\": \"CEXTERNAL1\"}]}]}"
start "$T/tk.duckdb" "" "SELECT anofox_serve_dashboards($(q "$T/sldash"), $PORT, $(q "$slopts"));" || fail "slack/teams server"
# the failing task's text message (chat.postMessage, escaped)
for _ in $(seq 50); do grep -q chat.postMessage "$T/sink3/slack.json" 2>/dev/null && break; sleep 0.2; done
echo "SELECT anofox_serve_report($PORT, 'sl');" >&3
echo "SELECT anofox_serve_report($PORT, 'ext');" >&3
for _ in $(seq 50); do
  n="$(grep -c '"report"' "$T/slack-runs.jsonl" 2>/dev/null || true)"
  [ "${n:-0}" -ge 2 ] && break
  sleep 0.2
done
grep -q "report 'sl' sent to 2 recipient(s)" "$T/out" || fail "slack/teams: sl: $(tail -5 "$T/out")"
grep '"report":"ext"' "$T/slack-runs.jsonl" | grep -q 'Slack Connect' || fail "slack: a shared channel should be refused: $(cat "$T/slack-runs.jsonl")"
python3 - "$T/sink3" <<'PY' || fail "slack/teams: the messages are wrong"
import json, sys
d = sys.argv[1]
s = [json.loads(l) for l in open(f"{d}/slack.json")]
post = [x for x in s if x["method"] == "chat.postMessage"]
assert len(post) == 1 and post[0]["args"]["channel"] == "C0123ABCD", post
t = post[0]["args"]["text"]
assert "FAILED" in t and "&lt;!channel&gt;" in t and "<!channel>" not in t, t
done = [x for x in s if x["method"] == "files.completeUploadExternal"]
assert len(done) == 1, s
a = done[0]["args"]
assert a["channel_id"] == "C0123ABCD" and [f["title"] for f in json.loads(a["files"])] == ["r.png", "r-who.csv"], a
assert "EU sales" in a["initial_comment"], a
up = [json.loads(l) for l in open(f"{d}/uploads.json")]
assert len(up) == 2 and up[0]["head"] == "89504e47", up   # bytes, a PNG first
# nothing went to the shared channel
assert not any(x["args"].get("channel_id") == "CEXTERNAL1" for x in s), s
tm = [json.loads(l) for l in open(f"{d}/teams.json")]
assert len(tm) == 1 and tm[0]["path"] == "/teams?sig=t0ps3cret", tm
card = tm[0]["body"]["attachments"][0]["content"]
assert card["type"] == "AdaptiveCard" and card["actions"][0]["url"] == "https://dash.example.com/d/r", card
assert any(b.get("type") == "Image" and b["url"].startswith("data:image/png;base64,") for b in card["body"]), card
PY
! grep -q "t0ps3cret\|xoxb-test-token" "$T/out" "$T/slack-runs.jsonl" || fail "slack/teams: a secret was logged"
stop
kill "$SINK_PID" 2>/dev/null; wait "$SINK_PID" 2>/dev/null || true
unset R_SLACK R_TEAMS
echo "OK: Slack and Teams — files uploaded as bytes, Slack Connect refused, escaped text, an Adaptive Card with the PNG, no secret in a log"

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
grep <<<"$page_html" -oq '"pathVars":\["model"\]' || fail "#407: window.__served.pathVars missing: $page_html"
grep <<<"$page_html" -oq '"navPickers":\[{"type":"dropdown","var":"model","label":"Model","options":\[{"value":"car-price","label":"Car price"},{"value":"van","label":"Van"}\]}\]' \
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
grep <<<"$page_html" -oq '"navPickers":\[{"type":"daterange","vars":\["view_from","view_to"\],"label":"Time window","min":"2026-01-01","max":"2026-01-31","presets":\[{"key":"last 7d","label":"Last 7 days","from":"2026-01-25","to":"2026-01-31"},{"key":"all","label":"All","from":"2026-01-01","to":"2026-01-31"}\]}\]' \
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

# #514 PR B: GET/HEAD /d/<id>/assets/<rest> — containment, the extension
# allow-list, the real (magic-byte) MIME, the size cap, and the response
# headers. Scope/auth are checked in scripts/auth-e2e/run.sh (this server
# has no `auth`, so nothing to check here beyond "every other /d/ route
# needs none either").
mkdir -p "$T/assetdash/assets/sub"
cat > "$T/assetdash/t.sql" <<'SQL'
SELECT 1::BIGINT AS n ::METRIC;
SQL
# A real (tiny) SVG and a real (tiny) PNG, so the magic-byte sniff accepts
# both — not just the extension.
cat > "$T/assetdash/assets/a.svg" <<'SVG'
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"><rect width="10" height="10"/></svg>
SVG
python3 -c "
import struct
sig = b'\x89PNG\r\n\x1a\n'
ihdr = b'IHDR' + struct.pack('>II', 2, 2) + bytes([8,2,0,0,0])
open('$T/assetdash/assets/sub/b.png', 'wb').write(sig + struct.pack('>I', 13) + ihdr + b'\0\0\0\0')
"
cp "$T/assetdash/assets/sub/b.png" "$T/assetdash/assets/b6m.png"
python3 -c "open('$T/assetdash/assets/b6m.png', 'ab').write(b'0' * (6 * 1024 * 1024))"
echo '{}' > "$T/assetdash/policy.json"
cp "$T/assetdash/t.sql" "$T/assetdash/assets/x.sql"
cp "$T/assetdash/t.sql" "$T/assetdash/assets/a.SVG.sql"
echo 'not actually a png' > "$T/assetdash/assets/fake.png"
echo 'secret' > "$T/assetdash/assets/.secret.png"
ln -s "../policy.json" "$T/assetdash/assets/link.png"
start "$T/main.duckdb" "" "SELECT anofox_serve_dashboards($(q "$T/assetdash"), $PORT, '{\"max_asset_bytes\":1048576}');" || fail "assets: server"
get() { curl -s -o "$T/abody" -D "$T/aheaders" -w "%{http_code}" "http://127.0.0.1:$PORT$1"; }
[ "$(get /d/t/assets/a.svg)" = 200 ] || fail "a.svg: $(cat "$T/abody")"
grep -qi "^content-type: image/svg+xml" "$T/aheaders" || fail "a.svg content-type: $(cat "$T/aheaders")"
grep -qi "^x-content-type-options: nosniff" "$T/aheaders" || fail "a.svg nosniff: $(cat "$T/aheaders")"
grep -qi "^content-security-policy: default-src 'none'; style-src 'unsafe-inline'; sandbox" "$T/aheaders" || fail "a.svg CSP: $(cat "$T/aheaders")"
grep -qi "^cache-control: private, max-age=300" "$T/aheaders" || fail "a.svg cache-control: $(cat "$T/aheaders")"
grep -qi "^cross-origin-resource-policy: same-origin" "$T/aheaders" || fail "a.svg CORP: $(cat "$T/aheaders")"
[ "$(get /d/t/assets/sub/b.png)" = 200 ] || fail "sub/b.png: $(cat "$T/abody")"
grep -qi "^content-type: image/png" "$T/aheaders" || fail "sub/b.png content-type: $(cat "$T/aheaders")"
[ "$(get /d/t/assets//etc/passwd)" = 404 ] || fail "an absolute path (leading /) under assets/ should 404"
[ "$(get /d/t/assets/../policy.json)" = 404 ] || fail "../policy.json should 404"
[ "$(get /d/t/assets/%2e%2e/policy.json)" = 404 ] || fail "%2e%2e/policy.json should 404"
[ "$(get /d/t/assets/..%2fpolicy.json)" = 404 ] || fail "..%2fpolicy.json should 404"
[ "$(get /d/t/assets/.secret.png)" = 404 ] || fail ".secret.png (hidden) should 404"
[ "$(get /d/t/assets/link.png)" = 404 ] || fail "link.png (symlink out) should 404"
[ "$(get /d/t/assets/x.sql)" = 404 ] || fail "x.sql (not an image extension) should 404"
[ "$(get /d/t/assets/a.SVG.sql)" = 404 ] || fail "a.SVG.sql (case + wrong real extension) should 404"
[ "$(get /d/t/assets/fake.png)" = 404 ] || fail "fake.png (right extension, not really an image) should 404"
[ "$(get /d/t/assets/nope.png)" = 404 ] || fail "nope.png (doesn't exist) should 404"
[ "$(get /d/t/assets)" = 404 ] || fail "assets (the folder itself, no trailing file) should 404"
[ "$(get /d/t/assets/b6m.png)" = 404 ] || fail "b6m.png (over max_asset_bytes) should 404"
[ "$(get /d/nosuchdash/assets/a.svg)" = 404 ] || fail "no such dashboard should 404"
[ "$(get /d/t/assets/a.svg)" = 200 ] || fail "a.svg (re-check before HEAD): $(cat "$T/abody")"
get_len=$(grep -i "^content-length:" "$T/aheaders" | tr -d '\r' | awk '{print $2}')
# `--head`/`-I`, not `-X HEAD`: the latter only changes the request line,
# it does not tell libcurl this request has no body — curl would then wait
# to read Content-Length bytes that a real HEAD response never sends
# (review of #554 nit 6's own `stat_asset` reports the real length via
# `fstat`, not a buffered read, exactly so HEAD and GET agree) and hang.
head_code=$(curl -s -o "$T/abody" -D "$T/aheaders" --head -w "%{http_code}" "http://127.0.0.1:$PORT/d/t/assets/a.svg")
[ "$head_code" = 200 ] || fail "HEAD should be 200: $head_code $(cat "$T/aheaders")"
grep -qi "^content-type: image/svg+xml" "$T/aheaders" || fail "HEAD content-type: $(cat "$T/aheaders")"
head_len=$(grep -i "^content-length:" "$T/aheaders" | tr -d '\r' | awk '{print $2}')
[ "$head_len" = "$get_len" ] && [ -n "$head_len" ] && [ "$head_len" != "0" ] \
  || fail "HEAD Content-Length ($head_len) should be the real size and match GET's ($get_len)"
post_code=$(curl -s -o /dev/null -w "%{http_code}" -X POST "http://127.0.0.1:$PORT/d/t/assets/a.svg")
[ "$post_code" = 405 ] || fail "POST to an asset should be 405: $post_code"
stop
echo "OK: #514 PR B: GET/HEAD /d/<id>/assets/<rest> — containment, extension allow-list, magic-byte MIME, size cap, headers; POST 405"

# #773: options.css_file — the operator's stylesheet, checked at start,
# served at /static/site.css (no login needed, like the UI's own files) and
# linked last in <head> of every page.
printf '.ax-c-kpi-hero{font-size:1.4em}\n' > "$T/site.css"
printf 'a{background:url(https://tracker.example/p.png)}\n' > "$T/remote.css"
if QUIET=1 start "$T/main.duckdb" "" "$(serve "{\"css_file\": \"$T/remote.css\"}")"; then
  fail "css_file with a remote url() should refuse the start"
fi
grep -q "only data: URLs" "$T/out" || fail "css_file: no reason given: $(cat "$T/out")"
exec 3>&-; wait "${PIDS[-1]}" 2>/dev/null || true; unset 'PIDS[-1]'
start "$T/main.duckdb" "" "$(serve "{\"css_file\": \"$T/site.css\"}")" || fail "css_file: the server should start"
[ "$(get /static/site.css)" = 200 ] || fail "/static/site.css: $(cat "$T/abody")"
grep -qi "^content-type: text/css" "$T/aheaders" || fail "site.css content-type: $(cat "$T/aheaders")"
grep -q "ax-c-kpi-hero" "$T/abody" || fail "site.css body: $(cat "$T/abody")"
for p in / /d/t; do
  [ "$(get "$p")" = 200 ] || fail "$p: $(cat "$T/abody")"
  grep -q '<link rel="stylesheet" href="/static/site.css"></head>' "$T/abody" || fail "$p: no site.css link"
done
stop
echo "OK: #773: css_file checked at start, served at /static/site.css, linked on every page"
# #772: anofox_serve_reload(port) — the dashboards folder is read again,
# checked like a start and swapped in with a new snapshot; a broken set
# leaves the running one in service; a symlinked folder swapped by a deploy
# is followed (its assets too).
mkdir -p "$T/r1/assets" "$T/r2/assets"
printf 'SELECT 1 AS one ::METRIC;\n' > "$T/r1/a.sql"
printf '<svg xmlns="http://www.w3.org/2000/svg"/>\n' > "$T/r1/assets/one.svg"
printf 'SELECT 2 AS two ::METRIC;\n' > "$T/r2/b.sql"
printf '<svg xmlns="http://www.w3.org/2000/svg"/>\n' > "$T/r2/assets/two.svg"
ln -s "$T/r1" "$T/rlink"
rget() { curl -s -o "$T/rbody" -w "%{http_code}" "http://127.0.0.1:$PORT$1"; }
reload() {
  : > "$T/out"
  echo "SELECT anofox_serve_reload($PORT);" >&3
  for _ in $(seq 100); do grep -qE 'reloaded|Error' "$T/out" && break; sleep 0.2; done
}
start "$T/main.duckdb" "" "SELECT anofox_serve_dashboards($(q "$T/rlink"), $PORT, '{}');" || fail "reload: the server should start"
[ "$(rget /d/a)" = 200 ] || fail "reload: /d/a before"
[ "$(rget /d/a/assets/one.svg)" = 200 ] || fail "reload: one.svg before"
# a new file in the same folder
printf 'SELECT 3 AS three ::METRIC;\n' > "$T/r1/c.sql"
[ "$(rget /d/c)" = 404 ] || fail "reload: /d/c should not exist before the reload"
reload
grep -q "reloaded 2 dashboard(s)" "$T/out" || fail "reload: $(cat "$T/out")"
[ "$(rget /d/c)" = 200 ] || fail "reload: /d/c after the reload"
# a broken file: the reload fails, the running set keeps serving
printf 'SELECT 1::METRIC; SELECT 2::METRIC FROM' > "$T/r1/broken.sql"
reload
grep -q "reload failed, still serving the previous dashboards" "$T/out" || fail "reload of a broken set: $(cat "$T/out")"
[ "$(rget /d/c)" = 200 ] && [ "$(rget /d/a)" = 200 ] || fail "reload: the old set should still serve"
[ "$(rget /d/broken)" = 404 ] || fail "reload: the broken dashboard must not appear"
rm "$T/r1/broken.sql"
# a deploy swaps the symlink: the new folder's dashboards and assets
ln -sfn "$T/r2" "$T/rlink"
reload
grep -q "reloaded 1 dashboard(s)" "$T/out" || fail "reload after the swap: $(cat "$T/out")"
[ "$(rget /d/b)" = 200 ] || fail "reload: /d/b after the swap"
[ "$(rget /d/a)" = 404 ] || fail "reload: /d/a should be gone after the swap"
[ "$(rget /d/b/assets/two.svg)" = 200 ] || fail "reload: the new folder's assets"
[ "$(rget /d/b/assets/one.svg)" = 404 ] || fail "reload: the old folder's assets are gone"
stop
echo "OK: #772: anofox_serve_reload — new files, a broken set keeps the old one, a swapped symlink is followed"

# #772 part 2: `-- @id` names a dashboard instead of its file; with
# `"folders": true` the subfolders' dashboards are served too, each folder a
# group; two files with one id refuse the start.
mkdir -p "$T/fd/sales" "$T/fd/tasks"
printf -- '-- @title Overview\nSELECT 1 AS n ::METRIC;\n' > "$T/fd/overview.sql"
printf -- '-- @id sales-weekly\n-- @title Weekly sales\nSELECT 2 AS n ::METRIC;\n' > "$T/fd/sales/weekly-v2.sql"
printf 'SELECT 1;\n' > "$T/fd/tasks/nightly.task.sql"
fserve() { echo "SELECT anofox_serve_dashboards($(q "$T/fd"), $PORT, $(q "$1"));"; }
start "$T/main.duckdb" "" "$(fserve '{}')" || fail "folders off: the server should start"
[ "$(rget /d/overview)" = 200 ] || fail "folders off: /d/overview"
[ "$(rget /d/sales-weekly)" = 404 ] || fail "folders off: a subfolder's dashboard must not be served"
stop
start "$T/main.duckdb" "" "$(fserve '{"folders": true}')" || fail "folders on: the server should start"
[ "$(rget /d/sales-weekly)" = 200 ] || fail "folders on: /d/sales-weekly (its -- @id)"
[ "$(rget /d/weekly-v2)" = 404 ] || fail "folders on: the file name is not the id when -- @id is set"
[ "$(rget /)" = 200 ] && grep -qE 'class="lp-group-h"[^>]*>sales ' "$T/rbody" || fail "folders on: the folder is a group on the / list: $(grep -o 'lp-group-h[^<]*' "$T/rbody" | head -3)"
[ "$(rget /d/nightly.task)" = 404 ] || fail "folders on: tasks/ is not a dashboards folder"
stop
printf 'SELECT 3 AS n ::METRIC;\n' > "$T/fd/sales/overview.sql"
if QUIET=1 start "$T/main.duckdb" "" "$(fserve '{"folders": true}')"; then
  fail "two dashboards with the id 'overview' should refuse the start"
fi
grep -q "dashboard id 'overview' is used by both" "$T/out" || fail "duplicate id: no reason given: $(cat "$T/out")"
exec 3>&-; wait "${PIDS[-1]}" 2>/dev/null || true; unset 'PIDS[-1]'
echo "OK: #772: -- @id, options.folders (a folder is a group), duplicate ids refuse the start"
# #769: the ingest API — POST /api/data/<table> from an API key with an
# ingest:<table> scope, into an attached read-write database; the first batch
# creates the table, a new key adds a column, a type that doesn't fit is a
# 400; other credentials and other tables are refused; a replica without
# ingest.run answers 503; with ingest.refresh the rows show without a manual
# refresh.
mkdir -p "$T/idash"
cat > "$T/idash/inbox.sql" <<'SQL'
SELECT count(*)::BIGINT AS n ::METRIC FROM inbox.main.events;
SQL
"$DUCKDB" "$T/inbox.duckdb" -c "CREATE TABLE events AS SELECT 'seed' AS kind, 0::BIGINT AS n LIMIT 0;" >/dev/null
mkkey() { # mkkey <scopes JSON> [dashboards JSON] → prints the key, appends its entry to $T/keys.entries
  local id key hash
  id=$(openssl rand -hex 6); key="afx_${id}_$(openssl rand -hex 32)"
  hash=$(printf %s "$key" | sha256sum | cut -d' ' -f1)
  printf '{"id": "%s", "sha256": "%s", "user": "svc-%s", "dashboards": %s, "scopes": %s}\n' "$id" "$hash" "$id" "${2:-[]}" "$1" >> "$T/keys.entries"
  printf %s "$key"
}
: > "$T/keys.entries"
K_EV=$(mkkey '["ingest:events"]')
K_OTHER=$(mkkey '["ingest:other"]')
K_NONE=$(mkkey '[]')
K_READ=$(mkkey '[]' '["inbox"]')
K_MET=$(mkkey '["ingest:metrics"]')
printf '{"keys": [%s]}\n' "$(paste -sd, "$T/keys.entries")" > "$T/keys.json"
chmod 600 "$T/keys.json"
iauth="\"auth\": {\"mode\": \"api_keys\", \"file\": \"$T/keys.json\"}, \"policy\": \"none\""
iserve() { echo "ATTACH $(q "$T/inbox.duckdb") AS inbox; SELECT anofox_serve_dashboards($(q "$T/idash"), $PORT, $(q "{$iauth, \"ingest\": $1}"));"; }
ipost() { # ipost <key> <table> <body> → status; body in $T/ibody
  curl -s -o "$T/ibody" -w "%{http_code}" -X POST "http://127.0.0.1:$PORT/api/data/$2" \
    ${1:+-H "Authorization: Bearer $1"} -H 'Content-Type: application/json' -d "$3"
}
start "$T/main.duckdb" "" "$(iserve '{"run": true, "database": "inbox", "refresh": true}')" || fail "ingest: the server should start"
[ "$(ipost "" events '{"kind": "a"}')" = 401 ] || fail "ingest without a key: $(cat "$T/ibody")"
[ "$(ipost "$K_NONE" events '{"kind": "a"}')" = 403 ] || fail "ingest with a key without the scope: $(cat "$T/ibody")"
[ "$(ipost "$K_OTHER" events '{"kind": "a"}')" = 403 ] || fail "ingest with another table's scope: $(cat "$T/ibody")"
[ "$(ipost "$K_EV" events '{"kind": "click", "n": 1}')" = 200 ] || fail "ingest one row: $(cat "$T/ibody")"
grep -q '"rows":1' "$T/ibody" && grep -q '"created":false' "$T/ibody" || fail "ingest one row: $(cat "$T/ibody")"
[ "$(ipost "$K_EV" events '[{"kind": "view", "n": 2, "city": "Bonn"}, {"kind": "view", "n": 3}]')" = 200 ] || fail "ingest a new column: $(cat "$T/ibody")"
grep -q '"added_columns":\["city"\]' "$T/ibody" || fail "ingest a new column: $(cat "$T/ibody")"
[ "$(ipost "$K_EV" events '{"kind": "x", "n": "not a number"}')" = 400 ] || fail "a VARCHAR into BIGINT should be a 400: $(cat "$T/ibody")"
grep -q "column 'n' is BIGINT" "$T/ibody" || fail "type conflict message: $(cat "$T/ibody")"
[ "$(ipost "$K_EV" events '{"_id": "x"}')" = 400 ] || fail "_id is the server's: $(cat "$T/ibody")"
[ "$(ipost "$K_EV" events 'not json')" = 400 ] || fail "a body that isn't JSON: $(cat "$T/ibody")"
[ "$(ipost "$K_EV" 'events;drop' '{"kind": "a"}')" = 400 ] || fail "a table that isn't a plain name: $(cat "$T/ibody")"
# The rows show after the (debounced) refresh: 3 rows, no manual refresh.
ipanel() { curl -s -o "$T/body" -w "%{http_code}" -X POST "http://127.0.0.1:$PORT/api/panel" \
  -H "Authorization: Bearer $K_READ" -d '{"dashboard":"inbox","panel":0,"vars":{}}'; }
# The first write's refresh runs at once, the next ones wait out the 30 s
# debounce (plus a build): allow 50 s.
for _ in $(seq 100); do [ "$(ipanel)" = 200 ] && grep -q '"c0":3' "$T/body" && break; sleep 0.5; done
grep -q '"c0":3' "$T/body" || fail "ingest refresh: the 3 rows should show: $(cat "$T/body")"
echo "SELECT kind, n, city, _id IS NOT NULL AS has_id FROM inbox.main.events ORDER BY n;" >&3
stop
grep -q "Bonn" "$T/out" || fail "ingest: the row with the new column: $(cat "$T/out")"
# a new table is created by its first batch
start "$T/main.duckdb" "" "$(iserve '{"run": true, "database": "inbox", "tables": {"metrics": {"name": "VARCHAR", "v": "DOUBLE"}}}')" || fail "ingest (2): start"
[ "$(ipost "$K_OTHER" other '{"v": 1.5, "note": null}')" = 200 ] && grep -q '"created":true' "$T/ibody" || fail "ingest creates a table: $(cat "$T/ibody")"
# a fixed schema: its own types, and no other keys
[ "$(ipost "$K_MET" metrics '{"name": "a", "v": 1}')" = 200 ] && grep -q '"created":true' "$T/ibody" || fail "ingest, fixed schema: $(cat "$T/ibody")"
[ "$(ipost "$K_MET" metrics '{"name": "b", "extra": 1}')" = 400 ] || fail "ingest, fixed schema: an extra key should be a 400: $(cat "$T/ibody")"
echo "SELECT 'metrics-type:' || typeof(v) FROM inbox.main.metrics;" >&3
stop
grep -q "metrics-type:DOUBLE" "$T/out" || fail "ingest, fixed schema: v should be DOUBLE: $(cat "$T/out")"
# a replica without run answers 503; an ingest database attached READ_ONLY refuses the start
start "$T/main.duckdb" "" "$(iserve '{"database": "inbox"}')" || fail "ingest (reader): start"
[ "$(ipost "$K_EV" events '{"kind": "a"}')" = 503 ] || fail "a replica without ingest.run should answer 503: $(cat "$T/ibody")"
stop
if QUIET=1 start "$T/main.duckdb" "" "ATTACH $(q "$T/inbox.duckdb") AS inbox (READ_ONLY); SELECT anofox_serve_dashboards($(q "$T/idash"), $PORT, $(q "{$iauth, \"ingest\": {\"run\": true, \"database\": \"inbox\"}}"));"; then
  fail "a READ_ONLY ingest database should refuse the start"
fi
grep -q "is attached READ_ONLY" "$T/out" || fail "READ_ONLY ingest database: no reason: $(cat "$T/out")"
exec 3>&-; wait "${PIDS[-1]}" 2>/dev/null || true; unset 'PIDS[-1]'
echo "OK: #769: ingest API — scopes, create/add column/400 on a conflict, refresh, 503 on a reader, READ_ONLY refused"

# #770: temporary dashboards — POST /api/dashboards from an API key with the
# preview scope; checked like a file plus every panel run once as the key (a
# broken panel is a 400); served at /p/<id> to a login, running as the key's
# claims; never in the list; ::SAVE refused; per-key limit; DELETE by its key;
# kept on disk across a restart.
mkdir -p "$T/pdash" "$T/pv"; chmod 700 "$T/pv"
printf 'SELECT 1 AS one ::METRIC;\n' > "$T/pdash/home.sql"
: > "$T/keys.entries"
K_PV=$(mkkey '["preview"]')
K_PV2=$(mkkey '["preview"]')
K_NOPV=$(mkkey '[]')
K_VIEW=$(mkkey '[]' 'null')
printf '{"keys": [%s]}\n' "$(paste -sd, "$T/keys.entries")" > "$T/keys.json"; chmod 600 "$T/keys.json"
pauth="\"auth\": {\"mode\": \"api_keys\", \"file\": \"$T/keys.json\"}, \"policy\": \"none\""
pserve() { echo "SELECT anofox_serve_dashboards($(q "$T/pdash"), $PORT, $(q "{$pauth, \"preview\": $1}"));"; }
pcreate() { # pcreate <key> <json body> → status; body in $T/pbody
  curl -s -o "$T/pbody" -w "%{http_code}" -X POST "http://127.0.0.1:$PORT/api/dashboards" \
    ${1:+-H "Authorization: Bearer $1"} -H 'Content-Type: application/json' -d "$2"
}
pget() { curl -s -o "$T/pbody" -w "%{http_code}" -H "Authorization: Bearer $K_VIEW" "http://127.0.0.1:$PORT$1"; }
start "$T/main.duckdb" "" "$(pserve "{\"dir\": \"$T/pv\", \"max_per_key\": 2}")" || fail "preview: the server should start"
[ "$(pcreate "$K_NOPV" '{"sql": "SELECT 1 AS x ::METRIC;"}')" = 403 ] || fail "preview without the scope: $(cat "$T/pbody")"
[ "$(pcreate "$K_PV" '{"sql": "SELECT getvariable('"'"'__claim_user'"'"') AS who ::TABLE;"}')" = 201 ] || fail "preview create: $(cat "$T/pbody")"
PID1=$(sed -n 's/.*"id":"\([^"]*\)".*/\1/p' "$T/pbody")
grep -q "\"url\":\"/p/$PID1\"" "$T/pbody" || fail "preview url: $(cat "$T/pbody")"
[ "$(pget "/p/$PID1")" = 200 ] || fail "preview page: $(cat "$T/pbody")"
grep -q "Referrer-Policy\|$PID1" "$T/pbody" || fail "preview page body"
# its panel runs as the key that made it, not as the viewer
code=$(curl -s -o "$T/pbody" -w "%{http_code}" -X POST "http://127.0.0.1:$PORT/api/panel" -H "Authorization: Bearer $K_VIEW" \
  -d "{\"dashboard\":\"$PID1\",\"panel\":0,\"vars\":{}}")
[ "$code" = 200 ] || fail "preview panel: $code $(cat "$T/pbody")"
grep -q "svc-$(printf %s "$K_PV" | cut -c5-16)" "$T/pbody" || fail "preview runs as its key: $(cat "$T/pbody")"
[ "$(pget "/d/$PID1")" = 404 ] || fail "a preview is not a /d/ dashboard"
[ "$(pget /)" = 200 ] && ! grep -q "$PID1" "$T/pbody" || fail "a preview is not on the / list"
# a broken panel, ::SAVE, no panels: 400
[ "$(pcreate "$K_PV" '{"sql": "SELECT x ::METRIC FROM no_such_table;"}')" = 400 ] && grep -q "no_such_table" "$T/pbody" || fail "a broken preview should be a 400 naming it: $(cat "$T/pbody")"
[ "$(pcreate "$K_PV" '{"sql": "SELECT '"'"'Save'"'"' AS s ::SAVE, '"'"'t'"'"' ::SAVE_TABLES;"}')" = 400 ] || fail "::SAVE in a preview: $(cat "$T/pbody")"
[ "$(pcreate "$K_PV" '{"sql": "CREATE TEMP VIEW v AS SELECT 1;"}')" = 400 ] || fail "a preview without panels: $(cat "$T/pbody")"
# per-key limit (2): one more is fine, the third is a 409; another key is unaffected
[ "$(pcreate "$K_PV" '{"sql": "SELECT 2 AS two ::METRIC;", "ttl_s": 60}')" = 201 ] || fail "second preview: $(cat "$T/pbody")"
PID2=$(sed -n 's/.*"id":"\([^"]*\)".*/\1/p' "$T/pbody")
[ "$(pcreate "$K_PV" '{"sql": "SELECT 3 AS three ::METRIC;"}')" = 409 ] || fail "the per-key limit: $(cat "$T/pbody")"
[ "$(pcreate "$K_PV2" '{"sql": "SELECT 3 AS three ::METRIC;"}')" = 201 ] || fail "another key's preview: $(cat "$T/pbody")"
# DELETE: only by its own key
code=$(curl -s -o /dev/null -w "%{http_code}" -X DELETE -H "Authorization: Bearer $K_PV2" "http://127.0.0.1:$PORT/api/dashboards/$PID2")
[ "$code" = 403 ] || fail "another key may not delete a preview: $code"
code=$(curl -s -o /dev/null -w "%{http_code}" -X DELETE -H "Authorization: Bearer $K_PV" "http://127.0.0.1:$PORT/api/dashboards/$PID2")
[ "$code" = 204 ] || fail "its key deletes a preview: $code"
[ "$(pget "/p/$PID2")" = 404 ] || fail "a deleted preview is gone"
[ -f "$T/pv/$PID1.json" ] && [ ! -f "$T/pv/$PID2.json" ] || fail "preview files: $(ls "$T/pv")"
stop
# kept across a restart
start "$T/main.duckdb" "" "$(pserve "{\"dir\": \"$T/pv\"}")" || fail "preview (restart): start"
[ "$(pget "/p/$PID1")" = 200 ] || fail "a kept preview after a restart: $(cat "$T/pbody")"
stop
echo "OK: #770: previews — scope, checks (a broken panel is a 400), runs as its key, /p/ only, limit, DELETE, kept on disk"
