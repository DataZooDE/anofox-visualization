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
#   * parquet footers are cached across requests and a rewritten file is read
#     fresh (#209).
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

# 7. Parquet footers are cached across requests (#209): the snapshot has
# parquet_metadata_cache on (viewers can't turn it off), and a file rewritten
# in the data folder is read fresh — in place or replaced by a rename — also
# through a setup view. `cache_mb: 0`: the result cache doesn't see file
# changes (documented), this is about the footers.
mkdir -p "$T/pruns" "$T/pdash"
"$DUCKDB" -c "COPY (SELECT range AS x FROM range(10)) TO $(q "$T/pruns/f.parquet");" >/dev/null
cat > "$T/pdash/p.sql" <<'SQL'
SELECT (current_setting('parquet_metadata_cache')::VARCHAR = 'true')::INT::BIGINT AS cached ::METRIC;
CREATE OR REPLACE TEMP VIEW pv AS SELECT * FROM read_parquet(getvariable('anofox_data') || '/f.parquet');
SELECT count(*)::BIGINT AS n ::METRIC FROM pv;
SELECT sum(x)::BIGINT AS s ::METRIC FROM pv;
SQL
start "$T/main.duckdb" "" "SELECT anofox_serve_dashboards($(q "$T/pdash"), $PORT,
  json_object('data', $(q "$T/pruns"), 'cache_mb', 0));" || fail "parquet cache server"
[ "$(panel 0 '{}' p)" = 200 ] && grep -qE '"c0":1(\.0)?[,}]' "$T/body" || fail "parquet_metadata_cache is not on: $(cat "$T/body")"
[ "$(panel 2 '{}' p)" = 200 ] && grep -qE '"c0":10(\.0)?[,}]' "$T/body" || fail "first read: $(cat "$T/body")"
[ "$(panel 2 '{}' p)" = 200 ] && grep -qE '"c0":10(\.0)?[,}]' "$T/body" || fail "cached read: $(cat "$T/body")"
# rewritten in place by another process
"$DUCKDB" -c "COPY (SELECT range AS x FROM range(1000)) TO $(q "$T/pruns/f.parquet");" >/dev/null
[ "$(panel 2 '{}' p)" = 200 ] && grep -qE '"c0":1000(\.0)?[,}]' "$T/body" || fail "in-place rewrite served a stale footer: $(cat "$T/body")"
[ "$(panel 3 '{}' p)" = 200 ] && grep -qE '"c0":499500(\.0)?[,}]' "$T/body" || fail "in-place rewrite (sum): $(cat "$T/body")"
# replaced by a rename (how a writer publishes a run atomically)
"$DUCKDB" -c "COPY (SELECT range AS x FROM range(7)) TO $(q "$T/pnew.parquet");" >/dev/null
mv "$T/pnew.parquet" "$T/pruns/f.parquet"
[ "$(panel 2 '{}' p)" = 200 ] && grep -qE '"c0":7(\.0)?[,}]' "$T/body" || fail "renamed-in file served a stale footer: $(cat "$T/body")"
[ "$(panel 3 '{}' p)" = 200 ] && grep -qE '"c0":21(\.0)?[,}]' "$T/body" || fail "renamed-in file (sum): $(cat "$T/body")"
# the rebuilt snapshot has it too
echo "SELECT anofox_serve_refresh($PORT);" >&3
for _ in $(seq 50); do grep -q "epoch 2" "$T/out" && break; sleep 0.2; done
grep -q "epoch 2" "$T/out" || fail "parquet cache refresh: $(cat "$T/out")"
[ "$(panel 0 '{}' p)" = 200 ] && grep -qE '"c0":1(\.0)?[,}]' "$T/body" || fail "parquet_metadata_cache off after a refresh: $(cat "$T/body")"
grep -q "parquet_metadata_cache:" "$T/out" && fail "parquet_metadata_cache warning: $(cat "$T/out")"
stop
echo "OK: parquet footers cached across requests; rewritten files read fresh, also after a refresh"
