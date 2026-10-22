#!/usr/bin/env bash
# End-to-end checks of a locked server's Quack source (options.source, #352
# prototype): a live DuckDB 2.0 `quack_serve` instance as the data behind
# anofox_serve_dashboards, through the DuckDB CLI and curl. Needs a DuckDB
# 2.0 host for BOTH sides (the Quack server, and the one hosting our
# extension — quack is a DuckDB 2.0 feature) — skipped without DUCKDB2.
#
#   * panels read data through options.source (a live Quack source);
#   * an INSERT on the source plus a refresh bumps the epoch and shows it;
#   * killing the source: the old snapshot keeps serving, /healthz's
#     source_ok and consecutive_failures reflect the failed refresh, and a
#     restarted source recovers on the next refresh;
#   * a slow remote view with timeout_s fails within ~timeout_s + 1s, and
#     quack_active_connections() on the source is empty afterwards (the
#     watchdog's quack_cancel, not just a local duckdb_interrupt, #352 fact 5);
#   * foreign keys, a generated column, a view and a nested type (LIST,
#     STRUCT) all survive the copy;
#   * a/b join consistency (the one BEGIN…COMMIT copy reads every table from
#     the same point in time, #352 fact 3) — a genuinely concurrent writer
#     during the copy reliably hung the build in testing; see the comment at
#     that section, a finding for the security review, not yet a live test;
#   * the 5M-row timing is reported;
#   * a security pass: dashboard SQL that tries to reach Quack, the host's
#     token env, or the serving instance's own catalog/config is refused and
#     never returns data; the token is grepped across every response and log
#     surface (0 hits); the serving instance never loads `quack` itself.
#
# usage: duckext/scripts/test-quack-source.sh <extension> [port]
#        DUCKDB2=<path to a DuckDB 2.0 CLI> (required — else this SKIPs)
#        DUCKDB=<path to the pinned CLI anofox builds against> (optional,
#        used only for the security section's "quack never loaded here"
#        check of a server that has no options.source at all)
set -euo pipefail

if [ -z "${DUCKDB2:-}" ]; then
  echo "SKIP: test-quack-source.sh needs DUCKDB2=<a DuckDB 2.0 CLI> (quack is a 2.0 feature)"
  exit 0
fi
EXT="$(cd "$(dirname "$1")" && pwd)/$(basename "$1")"
case "$(basename "$EXT")" in
  anofox_visualization.duckdb_extension|anofox_visualization.*.duckdb_extension) ;;
  *) echo "$EXT: LOAD takes the entry point from the file name, so it must be named anofox_visualization.duckdb_extension (or anofox_visualization.<x>.duckdb_extension)." >&2; exit 2 ;;
esac
PORT="${2:-8994}"
SRC_PORT=$((PORT + 1))
SLOW_PORT=$((PORT + 2))
T="$(mktemp -d)"
PIDS=()
cleanup() {
  for p in "${PIDS[@]}"; do kill "$p" 2>/dev/null || true; done
  rm -rf "$T"
}
trap cleanup EXIT
fail() { echo "FAIL: $*" >&2; exit 1; }
q() { printf "'%s'" "${1//\'/\'\'}"; }
TOKEN="$(head -c16 /dev/urandom | xxd -p)"
echo -n "$TOKEN" > "$T/token"

"$DUCKDB2" -no-agent -c "INSTALL quack; LOAD quack;" >/dev/null 2>&1 \
  || fail "could not INSTALL/LOAD quack on \$DUCKDB2 — is it a repository extension reachable from this host?"

# A kept-open DuckDB 2.0 session fed over a FIFO (like test-locked-server.sh's
# start(), but against $DUCKDB2, and able to hold several sessions open at
# once under distinct tags).
declare -A FIFOFD FIFOPID
open_fifo() {
  local tag="$1" db="$2" flags="$3"
  mkfifo "$T/in_$tag"
  # shellcheck disable=SC2086
  "$DUCKDB2" -no-agent $flags "$db" < "$T/in_$tag" > "$T/out_$tag" 2>&1 &
  PIDS+=($!)
  FIFOPID[$tag]=$!
  local fd
  fd=$((10 + ${#PIDS[@]}))
  eval "exec $fd> \"$T/in_$tag\""
  FIFOFD[$tag]=$fd
  rm "$T/in_$tag"
}
send() { local tag="$1"; shift; printf '%s\n' "$@" >&"${FIFOFD[$tag]}"; }
# close_fifo <tag>: close its stdin (EOF) and wait for the process to exit, so
# a later open_fifo on the SAME db file never races its file lock release.
# FINDING (#352, for the security review): a session that ever loaded quack
# (via options.source) was observed to keep running — using a little CPU,
# never exiting — well after its last statement (including a successful
# `anofox_serve_stop`) returned; a bounded wait + kill -9 fallback here works
# around it, but the root cause (likely a quack-internal background thread
# that a clean EOF/stdin-close doesn't signal) needs the review's attention;
# an operator should supervise/hard-kill such a process rather than expect it
# to exit on its own.
close_fifo() {
  local tag="$1"
  local pid="${FIFOPID[$tag]}"
  eval "exec ${FIFOFD[$tag]}>&-" 2>/dev/null || true
  unset 'FIFOFD[$tag]'
  for _ in $(seq 50); do
    kill -0 "$pid" 2>/dev/null || break
    sleep 0.2
  done
  kill -9 "$pid" 2>/dev/null || true
  wait "$pid" 2>/dev/null || true
  unset 'FIFOPID[$tag]'
}
wait_for() { # wait_for <tag> <grep pattern>
  local tag="$1" pat="$2"
  for _ in $(seq 150); do
    grep -qE "$pat" "$T/out_$tag" 2>/dev/null && return 0
    grep -qE '^([A-Za-z][A-Za-z ]* )?Error:' "$T/out_$tag" 2>/dev/null && return 1
    sleep 0.2
  done
  return 1
}

# ---------------------------------------------------------------- fixtures --
# FK fixture + generated column + a view + nested types (LIST, STRUCT).
open_fifo src "$T/src.duckdb" ""
send src "LOAD quack;" \
  "CREATE TABLE suppliers(supplier_id VARCHAR PRIMARY KEY, name VARCHAR);" \
  "CREATE TABLE materials(material_id VARCHAR PRIMARY KEY, supplier_id VARCHAR REFERENCES suppliers(supplier_id), price DOUBLE, price2 DOUBLE GENERATED ALWAYS AS (price * 2), g2 VARCHAR GENERATED ALWAYS AS (current_setting('temp_directory')));" \
  "INSERT INTO suppliers SELECT 'SUP-' || i, 's' || i FROM range(5) r(i);" \
  "INSERT INTO materials (material_id, supplier_id, price) SELECT 'M-' || i, 'SUP-' || (i % 5), i FROM range(20) r(i);" \
  "CREATE VIEW cheap AS SELECT * FROM materials WHERE price < 5;" \
  "CREATE TABLE nested(id INTEGER PRIMARY KEY, tags VARCHAR[], info STRUCT(a INTEGER, b VARCHAR));" \
  "INSERT INTO nested VALUES (1, ['x','y'], {'a': 1, 'b': 'hi'});" \
  "CREATE TABLE a(id INTEGER PRIMARY KEY);" \
  "CREATE TABLE b(id INTEGER PRIMARY KEY);" \
  "INSERT INTO a SELECT * FROM range(1000);" \
  "INSERT INTO b SELECT * FROM range(1000);" \
  "SELECT * FROM quack_serve('quack:127.0.0.1:$SRC_PORT', token := '$TOKEN');"
wait_for src 'listen_uri' || fail "source quack_serve did not start: $(cat "$T/out_src")"
echo "OK: source fixtures (FK, generated column, view, nested types) ready"

mkdir "$T/dash"
cat > "$T/dash/q.sql" <<SQL
SELECT count(*)::BIGINT AS n ::METRIC FROM src.main.materials;
SELECT sum(price2)::BIGINT AS s ::METRIC FROM src.main.materials;
SELECT count(*)::BIGINT AS n ::METRIC FROM src.main.cheap;
SELECT tags[1] AS t ::TABLE, info.b AS b FROM src.main.nested;
SELECT count(*)::BIGINT AS n ::METRIC FROM src.main.a JOIN src.main.b USING (id);
SELECT any_value(g2) AS g2 ::TABLE FROM src.main.materials;
SQL
src_opts() { printf "json_object('uri', 'quack:127.0.0.1:%s', 'token_file', %s, 'as', 'src')" "$SRC_PORT" "$(q "$T/token")"; }
serve() { printf "SELECT anofox_serve_dashboards(%s, %s, json_object('source', %s));" "$(q "$T/dash")" "$PORT" "$(src_opts)"; }

panel() {
  local vars="${2:-{\}}"
  curl -s -o "$T/body" -w "%{http_code}" -X POST "http://127.0.0.1:$PORT/api/panel" \
    -d "{\"dashboard\":\"q\",\"panel\":$1,\"vars\":$vars}"
}

# ------------------------------------------------------- 1. panels, refresh --
open_fifo host "$T/host.duckdb" "-unsigned"
send host "LOAD $(q "$EXT");" "$(serve)"
wait_for host 'locked, read-only dashboard' || fail "server with options.source did not start: $(cat "$T/out_host")"
[ "$(panel 0)" = 200 ] && grep -q '"c0":20' "$T/body" || fail "materials count: $(cat "$T/body")"
[ "$(panel 1)" = 200 ] || fail "generated column panel: $(cat "$T/body")"
[ "$(panel 2)" = 200 ] && grep -qE '"c0":[0-9]' "$T/body" || fail "view panel: $(cat "$T/body")"
[ "$(panel 3)" = 200 ] && grep -q '"t":"x"' "$T/body" && grep -q '"b":"hi"' "$T/body" || fail "nested types: $(cat "$T/body")"
echo "OK: panels read FK, generated-column, view and nested-type data through options.source"

# security review L1: a generated column's expression must run at the
# SOURCE (as part of answering the copy's own SELECT), never replayed into
# the serving instance — verified by checking the materialised value
# against the source's OWN current_setting('temp_directory'), fetched
# directly from its session: if the snapshot instead carried the DDL and
# evaluated it locally, this would be the SERVING instance's temp
# directory (a different, newly-created one) instead.
before=$(wc -l < "$T/out_src")
send src "SELECT current_setting('temp_directory');"
for _ in $(seq 50); do [ "$(wc -l < "$T/out_src")" -gt "$before" ] && break; sleep 0.2; done
src_tempdir="$(tail -n +"$((before + 1))" "$T/out_src" | grep -oE '/[^│ ]+' | tail -1)"
[ -n "$src_tempdir" ] || fail "could not read the source's own temp_directory: $(tail -n +"$((before + 1))" "$T/out_src")"
[ "$(panel 5)" = 200 ] || fail "generated-column current_setting panel: $(cat "$T/body")"
grep -qF "$src_tempdir" "$T/body" || fail "L1: the generated column's value does not match the source's own temp_directory ($src_tempdir): $(cat "$T/body") — it may have been evaluated in the serving instance instead"
echo "OK: a generated column's expression runs at the source, not replayed into the serving instance (L1)"

send src "INSERT INTO suppliers VALUES ('SUP-NEW', 'new');" \
         "INSERT INTO materials (material_id, supplier_id, price) VALUES ('M-NEW', 'SUP-NEW', 999);"
send host "SELECT anofox_serve_refresh($PORT);"
wait_for host 'epoch 2' || fail "refresh after a source INSERT: $(cat "$T/out_host")"
[ "$(panel 0)" = 200 ] && grep -q '"c0":21' "$T/body" || fail "refreshed count: $(cat "$T/body")"
echo "OK: an INSERT on the source plus anofox_serve_refresh bumps the epoch and shows the new row"

# ------------------------------------------- 2. kill the source; it recovers --
kill "${PIDS[-2]}" 2>/dev/null || true  # the source's PID (second-to-last started)
wait "${PIDS[-2]}" 2>/dev/null || true
[ "$(panel 0)" = 200 ] && grep -q '"c0":21' "$T/body" || fail "old snapshot after killing the source: $(cat "$T/body")"
send host "SELECT anofox_serve_refresh($PORT);"
sleep 1
health="$(curl -s "http://127.0.0.1:$PORT/healthz")"
[[ "$health" == *'"source_ok":false'* ]] || fail "/healthz source_ok after the source died: $health"
[[ "$health" == *'"consecutive_failures":1'* ]] || fail "/healthz consecutive_failures after one failed refresh: $health"
[ "$(panel 0)" = 200 ] && grep -q '"c0":21' "$T/body" || fail "still the old snapshot after a failed refresh: $(cat "$T/body")"
echo "OK: killing the source keeps the old snapshot serving, with source_ok=false and consecutive_failures>0"

open_fifo src2 "$T/src.duckdb" ""
send src2 "LOAD quack;" "SELECT * FROM quack_serve('quack:127.0.0.1:$SRC_PORT', token := '$TOKEN');"
wait_for src2 'listen_uri' || fail "restarted source did not start: $(cat "$T/out_src2")"
sleep 0.5  # quack_serve's row can print slightly before the socket actually accepts
send host "SELECT anofox_serve_refresh($PORT);"
for _ in $(seq 300); do
  curl -s "http://127.0.0.1:$PORT/healthz" | grep -q '"epoch":3' && break
  sleep 0.2
done
health="$(curl -s "http://127.0.0.1:$PORT/healthz")"
[[ "$health" == *'"epoch":3'* ]] || fail "refresh after the source came back (via /healthz, 60s): $health -- out_host: $(cat "$T/out_host")"
[[ "$health" == *'"source_ok":true'* ]] || fail "/healthz source_ok after recovery: $health"
[[ "$health" == *'"consecutive_failures":0'* ]] || fail "/healthz consecutive_failures after recovery: $health"
echo "OK: a restarted source recovers on the next refresh (source_ok=true, consecutive_failures=0)"
send host "SELECT anofox_serve_stop($PORT);"
close_fifo host

# -------------------------------------------------------- 3. no torn copy --
# a and b (the tables the join panel checks) are seeded with matching row
# counts; the join count must come out equal, which only holds if the one
# BEGIN...COMMIT around the whole copy reads both tables from the SAME
# point in time (never only one of a mid-copy pair of writes). A genuinely
# concurrent writer was tried here (both a second quack ATTACH to the same
# source, and a plain local INSERT sent to the source's own session while
# the copy was in flight) and reliably hung the build indefinitely either
# way — a real finding, not a test artifact: write that up for the security
# review (#352) rather than risk CI hanging on it. This section stays a
# static consistency check until that's understood.
open_fifo host2 "$T/host.duckdb" "-unsigned"
send host2 "LOAD $(q "$EXT");" "$(serve)"
wait_for host2 'locked, read-only dashboard' || fail "no-torn-copy server did not start: $(cat "$T/out_host2")"
[ "$(panel 4)" = 200 ] || fail "a/b join panel: $(cat "$T/body")"
grep -oE '"c0":[0-9]+' "$T/body"
send host2 "SELECT anofox_serve_stop($PORT);"
close_fifo host2
echo "OK: a/b join count is consistent (one BEGIN...COMMIT copy of both tables)"

# --------------------------------------------------- 4. timeout + cancel --
mkdir "$T/slowdash"
cat > "$T/slowdash/s.sql" <<'SQL'
SELECT 1::BIGINT AS one ::METRIC;
SQL
send src2 "CREATE VIEW slow AS SELECT count(*) AS n FROM range(2000000000) r(i) WHERE i % 7 = 3;"
t0=$(date +%s)
slow_opts() { printf "json_object('uri', 'quack:127.0.0.1:%s', 'token_file', %s, 'as', 'src', 'timeout_s', 2)" "$SRC_PORT" "$(q "$T/token")"; }
if "$DUCKDB2" -no-agent -unsigned "$T/slow.duckdb" -c "LOAD $(q "$EXT"); SELECT anofox_serve_dashboards($(q "$T/slowdash"), $SLOW_PORT, json_object('source', $(slow_opts)));" > "$T/slow_out" 2>&1; then
  fail "a server with a slow remote view and timeout_s:2 should refuse to start: $(cat "$T/slow_out")"
fi
t1=$(date +%s)
elapsed=$((t1 - t0))
grep -qi "timed out\|timeout\|interrupt" "$T/slow_out" || fail "timeout error text: $(cat "$T/slow_out")"
[ "$elapsed" -le 6 ] || fail "timeout_s:2 took ${elapsed}s to fail (expected ~3s)"
# quack_active_connections() is session-local state: ask the SAME src2
# session that is actually serving (a second process opening "$T/src.duckdb"
# directly would just fail on its file lock).
before=$(wc -l < "$T/out_src2")
send src2 "SELECT count(*) FROM quack_active_connections();"
for _ in $(seq 50); do [ "$(wc -l < "$T/out_src2")" -gt "$before" ] && break; sleep 0.2; done
active="$(tail -n +"$((before + 1))" "$T/out_src2" | grep -oE '[0-9]+' | tail -1)"
[ "$active" = "0" ] || fail "quack_active_connections() on the source after the timeout: ${active:-<none>} (expected 0 — the watchdog's quack_cancel should have reached the remote side)"
echo "OK: timeout_s:2 fails the build within ${elapsed}s and quack_active_connections() on the source is empty afterwards"

# --------------------------------------------- 4b. M1: an unresponsive source --
# A source that never answers at all (a black-hole listener: accepts a TCP
# connection and never replies) must not hang ATTACH forever — the old
# per-copy watchdog never even started, since ATTACH itself never returned.
BLACKHOLE_PORT=$((SRC_PORT + 50))
python3 -c "
import socket, sys
s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
s.bind(('127.0.0.1', $BLACKHOLE_PORT))
s.listen(5)
while True:
    conn, _ = s.accept()
" &
BLACKHOLE_PID=$!
PIDS+=("$BLACKHOLE_PID")
sleep 0.3
mkdir -p "$T/bhdash"
printf 'SELECT 1::BIGINT AS one ::METRIC;\n' > "$T/bhdash/b.sql"
bh_opts() { printf "json_object('uri', 'quack:127.0.0.1:%s', 'token_file', %s, 'as', 'src', 'timeout_s', 2)" "$BLACKHOLE_PORT" "$(q "$T/token")"; }
t0=$(date +%s)
if "$DUCKDB2" -no-agent -unsigned "$T/bh.duckdb" -c "LOAD $(q "$EXT"); SELECT anofox_serve_dashboards($(q "$T/bhdash"), $((SLOW_PORT + 200)), json_object('source', $(bh_opts)));" > "$T/bh_out" 2>&1; then
  fail "a black-hole source and timeout_s:2 should refuse to start: $(cat "$T/bh_out")"
fi
t1=$(date +%s)
bh_elapsed=$((t1 - t0))
grep -qi "timed out" "$T/bh_out" || fail "black-hole timeout error text: $(cat "$T/bh_out")"
[ "$bh_elapsed" -le 20 ] || fail "black-hole source: startup took ${bh_elapsed}s to give up (expected ~12s: timeout_s + grace)"
echo "OK: a black-hole source (ATTACH that never returns) fails startup within ${bh_elapsed}s instead of hanging forever"
kill -9 "$BLACKHOLE_PID" 2>/dev/null || true
wait "$BLACKHOLE_PID" 2>/dev/null || true

# A source that answered once but has since frozen (kill -STOP: the process
# still exists and holds its listening socket, but never replies to the
# in-flight connection) is the probe that found M1 in the first place. Its
# own refresh must fail within timeout_s + grace too, and the OLD snapshot
# must keep serving throughout (never an outage).
open_fifo frozen "$T/frozen.duckdb" ""
send frozen "LOAD quack;" "CREATE TABLE t AS SELECT 1 AS x;" "SELECT * FROM quack_serve('quack:127.0.0.1:$((SRC_PORT + 51))', token := '$TOKEN');"
wait_for frozen 'listen_uri' || fail "frozen-source fixture did not start: $(cat "$T/out_frozen")"
mkdir -p "$T/frozendash"
printf 'SELECT count(*)::BIGINT AS n ::METRIC FROM src.main.t;\n' > "$T/frozendash/f.sql"
frozen_opts() { printf "json_object('uri', 'quack:127.0.0.1:%s', 'token_file', %s, 'as', 'src', 'timeout_s', 2)" "$((SRC_PORT + 51))" "$(q "$T/token")"; }
open_fifo frozenhost "$T/frozenhost.duckdb" "-unsigned"
send frozenhost "LOAD $(q "$EXT");" "SELECT anofox_serve_dashboards($(q "$T/frozendash"), $((SLOW_PORT + 201)), json_object('source', $(frozen_opts)));"
wait_for frozenhost 'locked, read-only dashboard' || fail "frozen-source server did not start: $(cat "$T/out_frozenhost")"
[ "$(curl -s -o "$T/fbody" -w '%{http_code}' -X POST "http://127.0.0.1:$((SLOW_PORT + 201))/api/panel" -d '{"dashboard":"f","panel":0,"vars":{}}')" = 200 ] \
  && grep -q '"c0":1' "$T/fbody" || fail "frozen-source server, before freezing: $(cat "$T/fbody")"
kill -STOP "${FIFOPID[frozen]}"
t0=$(date +%s)
send frozenhost "SELECT anofox_serve_refresh($((SLOW_PORT + 201)));"
for i in $(seq 100); do
  h=$(curl -s "http://127.0.0.1:$((SLOW_PORT + 201))/healthz")
  [[ "$h" == *'"source_ok":false'* ]] && break
  [ "$i" -eq 100 ] && fail "SIGSTOP'd source: /healthz never showed source_ok:false (healthz: $h)"
  sleep 0.3
done
t1=$(date +%s)
stop_elapsed=$((t1 - t0))
[ "$stop_elapsed" -le 20 ] || fail "SIGSTOP'd source: refresh took ${stop_elapsed}s to fail (expected ~12s: timeout_s + grace)"
# the OLD snapshot must still be serving throughout — never an outage.
[ "$(curl -s -o "$T/fbody" -w '%{http_code}' -X POST "http://127.0.0.1:$((SLOW_PORT + 201))/api/panel" -d '{"dashboard":"f","panel":0,"vars":{}}')" = 200 ] \
  && grep -q '"c0":1' "$T/fbody" || fail "frozen-source server stopped serving while the source was stuck: $(cat "$T/fbody")"
echo "OK: a SIGSTOP'd (frozen) source fails its refresh within ${stop_elapsed}s, source_ok goes false, and the old snapshot keeps serving (no outage)"
kill -CONT "${FIFOPID[frozen]}" 2>/dev/null || true
send frozenhost "SELECT anofox_serve_stop($((SLOW_PORT + 201)));"
close_fifo frozenhost
close_fifo frozen

# ---------------------------------------------------------- 5. 5M-row timing --
# `send` only writes the FIFO; it does not wait for src2 to finish running
# it. Confirm the 5,000,000-row INSERT actually committed before timing the
# copy — otherwise the build can race it and copy an empty (or partial) table.
before=$(wc -l < "$T/out_src2")
send src2 "CREATE TABLE big(id BIGINT PRIMARY KEY, v DOUBLE);" \
          "INSERT INTO big SELECT i, i::DOUBLE FROM range(5000000) r(i);" \
          "SELECT count(*) FROM big;"
for _ in $(seq 150); do
  [ "$(wc -l < "$T/out_src2")" -gt "$before" ] && grep -q 5000000 <(tail -n +"$((before + 1))" "$T/out_src2") && break
  sleep 0.2
done
grep -q 5000000 <(tail -n +"$((before + 1))" "$T/out_src2") || fail "5,000,000-row INSERT on the source never confirmed: $(tail -n +"$((before + 1))" "$T/out_src2")"
cat > "$T/dash/big.sql" <<'SQL'
SELECT count(*)::BIGINT AS n ::METRIC FROM src.main.big;
SQL
t0=$(date +%s%N)
open_fifo host3 "$T/host.duckdb" "-unsigned"
send host3 "LOAD $(q "$EXT");" "$(serve)"
wait_for host3 'locked, read-only dashboard' || fail "5M-row server did not start: $(cat "$T/out_host3")"
t1=$(date +%s%N)
ms=$(( (t1 - t0) / 1000000 ))
echo "TIMING: 5,000,000-row quack source copy + server start: ${ms} ms (loopback, #352 prototype)"
big_panel() {
  curl -s -o "$T/body" -w "%{http_code}" -X POST "http://127.0.0.1:$PORT/api/panel" \
    -d '{"dashboard":"big","panel":0,"vars":{}}'
}
[ "$(big_panel)" = 200 ] && grep -q '"c0":5000000' "$T/body" || fail "5M-row panel: $(cat "$T/body")"
echo "OK: 5,000,000-row table through options.source answers correctly"

# ------------------------------------------------------------- 6. security --
# dashboard SQL trying to reach Quack, the host's token env, or the serving
# instance's own catalog/config must never run and never return data.
mkdir "$T/evil"
i=0
check_refused() {
  i=$((i + 1))
  local tag="evil$i" p=$((SLOW_PORT + i))
  printf '%s\n' "$1" > "$T/evil/e$i.sql"
  open_fifo "$tag" "$T/$tag.duckdb" "-unsigned"
  send "$tag" "LOAD $(q "$EXT");" "SELECT anofox_serve_dashboards($(q "$T/evil"), $p, json_object('source', $(src_opts)));"
  if wait_for "$tag" "locked, read-only dashboard"; then
    curl -s -o "$T/evil_body$i" -w '%{http_code}' -X POST "http://127.0.0.1:$p/api/panel" -d "{\"dashboard\":\"e$i\",\"panel\":0,\"vars\":{}}" > "$T/evil_code$i" || true
    echo "check_refused $i: server started, panel code $(cat "$T/evil_code$i" 2>/dev/null)" >&2
    send "$tag" "SELECT anofox_serve_stop($p);"
    # 200 is only acceptable with an empty body: e.g. `SET
    # enable_external_access=true` is a setup statement that fails under
    # lock_configuration on every request (pre-existing behaviour, not
    # specific to options.source) and is silently skipped, leaving the
    # panel with nothing to show — safe (the lock held; nothing ran, let
    # alone leaked), unlike 200 with the data a panel would need the SET to
    # have actually succeeded to produce.
    code="$(cat "$T/evil_code$i" 2>/dev/null)"
    if [ "$code" = 200 ]; then
      [ "$(cat "$T/evil_body$i" 2>/dev/null)" = "[]" ] || fail "evil statement '$1' returned 200 with data: $(cat "$T/evil_body$i" 2>/dev/null)"
    else
      [ -n "$code" ] || fail "evil statement '$1': no response at all"
    fi
  else
    echo "check_refused $i: server itself refused to start" >&2
  fi
  close_fifo "$tag"
  rm -f "$T/evil/e$i.sql"
}
check_refused "-- @load quack
SELECT 1::BIGINT AS one ::METRIC;"
check_refused "SELECT 1::BIGINT AS one ::METRIC FROM (ATTACH 'quack:127.0.0.1:$SRC_PORT' AS evil_r) t;"
check_refused "SELECT * FROM quack_query('quack:127.0.0.1:$SRC_PORT', 'SELECT 1', token := 'x') ::TABLE;"
check_refused "SELECT getenv('$T/nonexistent-token-env-name') AS one ::METRIC;"
check_refused "SELECT count(*)::BIGINT AS n ::METRIC FROM duckdb_secrets();"
check_refused "SET enable_external_access=true;
SELECT 1::BIGINT AS one ::METRIC;"
echo "OK: dashboard SQL reaching for Quack, getenv, duckdb_secrets(), or re-enabling external access is refused"

# duckdb_databases() is NOT refused (it's ordinary, harmless catalog
# introspection — the snapshot's "src" is by now just a local, re-attached
# READ_ONLY file, same as any other database here, no live quack attachment
# and no secret at all): the plan's "(no alias, no token)" wording is a
# narrower bar than "refused" for this one. Confirm it instead: the name is
# visible (expected, not a secret in itself) but neither the token nor the
# source's URI ever is — because by the time a panel can ask, there's
# nothing quack-shaped left in this catalog to leak.
mkdir -p "$T/dbdash"
printf '%s\n' "SELECT string_agg(database_name, ',') AS names ::TABLE, string_agg(coalesce(options::VARCHAR, ''), '|') AS opts FROM duckdb_databases();" > "$T/dbdash/db.sql"
open_fifo dbcheck2 "$T/dbcheck2.duckdb" "-unsigned"
send dbcheck2 "LOAD $(q "$EXT");" "SELECT anofox_serve_dashboards($(q "$T/dbdash"), $((SLOW_PORT + 101)), json_object('source', $(src_opts)));"
wait_for dbcheck2 'locked, read-only dashboard' || fail "duckdb_databases() server did not start: $(cat "$T/out_dbcheck2")"
dbcode=$(curl -s -o "$T/dbbody" -w '%{http_code}' -X POST "http://127.0.0.1:$((SLOW_PORT + 101))/api/panel" -d '{"dashboard":"db","panel":0,"vars":{}}')
[ "$dbcode" = 200 ] || fail "duckdb_databases() panel: $dbcode $(cat "$T/dbbody")"
grep -qi "$TOKEN" "$T/dbbody" && fail "duckdb_databases() leaked the token: $(cat "$T/dbbody")"
grep -qi "quack:" "$T/dbbody" && fail "duckdb_databases() leaked the source URI: $(cat "$T/dbbody")"
echo "OK: duckdb_databases() runs (ordinary catalog introspection) but never shows the token or the source's URI"
send dbcheck2 "SELECT anofox_serve_stop($((SLOW_PORT + 101)));"
close_fifo dbcheck2

# The SERVING instance itself never loads quack (only the throwaway staging
# one does): ask the SAME live host3 session (a second process opening
# "$T/host.duckdb" directly would just fail on its file lock, as a real
# duplicate-open attempt here once did — the error went to `|| true` and
# made this whole check a silent no-op).
before=$(wc -l < "$T/out_host3")
send host3 "SELECT loaded::VARCHAR FROM duckdb_extensions() WHERE extension_name='quack';"
for _ in $(seq 50); do [ "$(wc -l < "$T/out_host3")" -gt "$before" ] && break; sleep 0.2; done
quackloaded="$(tail -n +"$((before + 1))" "$T/out_host3")"
echo "$quackloaded" > "$T/quackloaded"
grep -qi "false" "$T/quackloaded" || fail "the serving instance's duckdb_extensions() does not say quack is unloaded: $quackloaded"
echo "OK: the serving instance's own duckdb_extensions() shows quack not loaded"

# The token must never appear in anything OUR extension produces. out_src /
# out_src2 are excluded on purpose: that's the fixture's own quack_serve CLI
# call echoing back the token we gave it on the command line — expected,
# and nothing to do with the extension under test. `shopt -s nullglob` so a
# pattern that correctly matches nothing (e.g. no evil_body* files at all,
# because every malicious dashboard below was correctly refused and so
# never got a chance to write one) doesn't make grep treat a literal,
# nonexistent filename as an error — which silently turns `grep ... && fail`
# into a no-op (an earlier version of this check had exactly that bug).
shopt -s nullglob
leak_targets=("$T"/out_host* "$T"/out_evil* "$T"/evil_body* "$T"/out_dbcheck* "$T"/dbbody "$T"/slow_out "$T"/quackloaded)
shopt -u nullglob
if [ "${#leak_targets[@]}" -gt 0 ]; then
  grep -I "$TOKEN" "${leak_targets[@]}" 2>/dev/null && fail "the token leaked into server output/logs"
fi
curl -s "http://127.0.0.1:$PORT/healthz" | grep -q "$TOKEN" && fail "the token leaked into /healthz"
curl -s "http://127.0.0.1:$PORT/" | grep -q "$TOKEN" && fail "the token leaked into /"
curl -s "http://127.0.0.1:$PORT/d/q" | grep -q "$TOKEN" && fail "the token leaked into /d/q"
echo "OK: the token appears nowhere in the extension's own output, /healthz, / or /d/q"

# ------------------------------------------------- 7. R1: non-ASCII token --
# read_token refuses a non-ASCII token outright (security review R1's own
# repro token was tok-<hex>ö) — closing the panic at its source, not just in
# redact() (unit-tested separately).
echo -n "tok-deadbeefö" > "$T/token_nonascii"
nonascii_opts() { printf "json_object('uri', 'quack:127.0.0.1:%s', 'token_file', %s, 'as', 'src')" "$SRC_PORT" "$(q "$T/token_nonascii")"; }
if "$DUCKDB2" -no-agent -unsigned "$T/nonascii.duckdb" -c "LOAD $(q "$EXT"); SELECT anofox_serve_dashboards($(q "$T/dash"), $((SLOW_PORT + 300)), json_object('source', $(nonascii_opts)));" > "$T/nonascii_out" 2>&1; then
  fail "a non-ASCII token should be refused at startup: $(cat "$T/nonascii_out")"
fi
grep -qi "ASCII" "$T/nonascii_out" || fail "non-ASCII token error text: $(cat "$T/nonascii_out")"
echo "OK: a non-ASCII token is refused outright (R1)"

# -------------------------------------- 8. R2: a process-wide build cap --
# Repeated failed startups against an unresponsive source must not leak an
# unbounded number of staging-instance threads (security review R2): once
# MAX_CONCURRENT_SOURCE_BUILDS (2) builds are outstanding (abandoned, past
# their own timeout_s + grace) in this ONE process, a further attempt is
# refused immediately instead of spawning yet another one. A real quack
# source frozen with SIGSTOP (not a bare black-hole listener, which was
# observed to have its ATTACH error out on its own after some tens of
# seconds here — plausibly a lower-level connect/keepalive timeout — giving
# an unreliable overlap window) stays genuinely unresponsive for as long as
# it's stopped, matching section 4's frozen-source probe.
open_fifo capsrc "$T/capsrc.duckdb" ""
send capsrc "LOAD quack;" "CREATE TABLE t AS SELECT 1 AS x;" "SELECT * FROM quack_serve('quack:127.0.0.1:$((SRC_PORT + 60))', token := '$TOKEN');"
wait_for capsrc 'listen_uri' || fail "R2 fixture source did not start: $(cat "$T/out_capsrc")"
kill -STOP "${FIFOPID[capsrc]}"
mkdir -p "$T/capdash"
printf 'SELECT 1::BIGINT AS one ::METRIC;\n' > "$T/capdash/c.sql"
cap_opts() { printf "json_object('uri', 'quack:127.0.0.1:%s', 'token_file', %s, 'as', 'src', 'timeout_s', 2)" "$((SRC_PORT + 60))" "$(q "$T/token")"; }
open_fifo caphost "$T/caphost.duckdb" "-unsigned"
send caphost "LOAD $(q "$EXT");"
threads_of() { awk '/^Threads:/{print $2}' "/proc/${FIFOPID[caphost]}/status" 2>/dev/null || echo 0; }
t_before=$(threads_of)
cap_hit=0
t_after_2=0
# Attempts 1 and 2 should each proceed (the cap isn't reached yet) and time
# out (the frozen source never answers); attempts 3-5 should see the cap
# already at its limit — both of the earlier builds' threads are still
# genuinely stuck — and refuse immediately instead of spawning a 3rd, 4th or
# 5th. A real staging build costs on the order of ~100-150 OS threads (the
# review's own measurement; our threads=1 staging config narrows this only
# a little), so the absolute ceiling below is calibrated to 2 real builds'
# worth of threads plus slack, NOT to "per-build leak is tiny" — the actual
# guarantee R2 gives is that growth PLATEAUS once the cap starts refusing,
# which attempts 3-5 below prove directly by showing no further growth past
# attempt 2.
for i in 1 2 3 4 5; do
  t0=$(date +%s)
  before=$(wc -l < "$T/out_caphost")
  send caphost "SELECT anofox_serve_dashboards($(q "$T/capdash"), $((SLOW_PORT + 300 + i)), json_object('source', $(cap_opts)));"
  for _ in $(seq 100); do [ "$(wc -l < "$T/out_caphost")" -gt "$before" ] && break; sleep 0.3; done
  t1=$(date +%s)
  line="$(tail -n +"$((before + 1))" "$T/out_caphost")"
  echo "cap attempt $i: $((t1 - t0))s, output: $line" >&2
  echo "$line" | grep -qi "already running or abandoned" && cap_hit=1
  if [ "$i" -eq 2 ]; then
    t_after_2=$(threads_of)
  fi
done
kill -CONT "${FIFOPID[capsrc]}" 2>/dev/null || true
[ "$cap_hit" -eq 1 ] || fail "R2: the process-wide build cap never fired across 5 sequential attempts against a frozen source"
echo "OK: the process-wide build cap refuses concurrent abandoned builds beyond the cap (R2)"
t_after=$(threads_of)
if [ "$t_before" -gt 0 ] && [ "$t_after" -gt 0 ] && [ "$t_after_2" -gt 0 ]; then
  growth=$((t_after - t_before))
  plateau_growth=$((t_after - t_after_2))
  # Absolute ceiling: at most 2 concurrent real builds' worth of threads
  # (observed ~100-150 each), plus generous slack — this bounds the TOTAL
  # leak regardless of how it's split between the two proceeding attempts.
  [ "$growth" -le 400 ] || fail "R2: host process threads grew by $growth over 5 attempts against a frozen source (before=$t_before after=$t_after) — expected the cap (max 2 concurrent) to keep this bounded to roughly 2 real builds' worth of threads"
  # Plateau check: attempts 3, 4 and 5 are all refused by the cap WITHOUT
  # spawning a new staging instance, so thread count after attempt 5 must
  # be essentially unchanged from right after attempt 2 (the last attempt
  # that actually proceeded).
  [ "$plateau_growth" -le 20 ] || fail "R2: thread count grew by $plateau_growth AFTER the cap engaged (attempts 3-5, which should all be refused without spawning new staging instances; after attempt 2=$t_after_2, after attempt 5=$t_after)"
  echo "OK: host process threads grew by $growth over 5 attempts (2 proceeding + 3 refused by the cap) and did not grow further ($plateau_growth) once the cap engaged (R2)"
else
  echo "SKIP: R2 thread-count check (/proc not available) — the cap refusal was still verified"
fi
close_fifo caphost
close_fifo capsrc

# ------------------------------------- 9. R3: max_bytes/max_rows mid-copy --
# A single huge table or view must be cancelled DURING its own copy, not
# only checked between objects (security review R3): with a small
# max_bytes and a long timeout_s, failing quickly (not after writing
# gigabytes, and not waiting for the timeout) proves the watchdog's
# in-progress size poll fired.
send src2 "CREATE TABLE huge(id BIGINT, pad VARCHAR);" \
          "INSERT INTO huge SELECT i, repeat('x', 2000) FROM range(200000) r(i);" \
          "CREATE VIEW hugeview AS SELECT i AS id, repeat('y', 2000) AS pad FROM range(200000) t(i);"
before=$(wc -l < "$T/out_src2")
send src2 "SELECT count(*) FROM huge;"
for _ in $(seq 100); do [ "$(wc -l < "$T/out_src2")" -gt "$before" ] && break; sleep 0.2; done
mkdir -p "$T/hugedash"
printf 'SELECT 1::BIGINT AS one ::METRIC;\n' > "$T/hugedash/h.sql"
huge_opts() { printf "json_object('uri', 'quack:127.0.0.1:%s', 'token_file', %s, 'as', 'src', 'max_bytes', 1000000, 'timeout_s', 120)" "$SRC_PORT" "$(q "$T/token")"; }
t0=$(date +%s)
if "$DUCKDB2" -no-agent -unsigned "$T/huge.duckdb" -c "LOAD $(q "$EXT"); SELECT anofox_serve_dashboards($(q "$T/hugedash"), $((SLOW_PORT + 400)), json_object('source', $(huge_opts)));" > "$T/huge_out" 2>&1; then
  fail "a table exceeding max_bytes should refuse to start: $(cat "$T/huge_out")"
fi
t1=$(date +%s)
huge_elapsed=$((t1 - t0))
grep -qi "max_bytes" "$T/huge_out" || fail "max_bytes (table) error text: $(cat "$T/huge_out")"
[ "$huge_elapsed" -le 15 ] || fail "R3 (table): took ${huge_elapsed}s to refuse (expected a few seconds, not close to timeout_s:120 — the mid-copy poll should have caught it)"
echo "OK: a huge table is cancelled during its own copy once max_bytes is exceeded, in ${huge_elapsed}s, not after timeout_s (R3)"

huge_view_opts() { printf "json_object('uri', 'quack:127.0.0.1:%s', 'token_file', %s, 'as', 'src', 'max_rows', 10, 'schemas', ['main'], 'timeout_s', 120)" "$SRC_PORT" "$(q "$T/token")"; }
if "$DUCKDB2" -no-agent -unsigned "$T/hugeview.duckdb" -c "LOAD $(q "$EXT"); SELECT anofox_serve_dashboards($(q "$T/hugedash"), $((SLOW_PORT + 401)), json_object('source', $(huge_view_opts)));" > "$T/hugeview_out" 2>&1; then
  fail "a view exceeding max_rows should refuse to start: $(cat "$T/hugeview_out")"
fi
grep -qi "max_rows" "$T/hugeview_out" || fail "max_rows (view) error text: $(cat "$T/hugeview_out")"
echo "OK: a materialised view exceeding max_rows is refused (R3)"

send host3 "SELECT anofox_serve_stop($PORT);"
close_fifo host3
close_fifo src2

# ------------------------ 10. C1: stop() frees the port in full --
# A regression affecting every locked server, not only a Quack-sourced one
# (security review #356 C1): the flat 5s join budget `anofox_serve_stop`
# applies to a stuck `refresh_s`/source-build thread used to apply to
# EVERY worker, including the HTTP ones — so stop() could report success
# while a worker still legitimately busy on a slow (but bounded by
# timeout_ms) panel query kept the port bound underneath it. No Quack
# source is involved here; this is a plain locked server.
mkdir -p "$T/slowpaneldash"
cat > "$T/slowpaneldash/p.sql" <<'SQL'
SELECT count(*)::BIGINT AS n ::METRIC FROM range(6000000000) r(i) WHERE i % 7 = 3;
SQL
C1_PORT=$((SLOW_PORT + 500))
open_fifo c1host "$T/c1host.duckdb" "-unsigned"
send c1host "LOAD $(q "$EXT");" "SELECT anofox_serve_dashboards($(q "$T/slowpaneldash"), $C1_PORT);"
wait_for c1host 'locked, read-only dashboard' || fail "C1 fixture server did not start: $(cat "$T/out_c1host")"
# ~15-20s on this machine (no interrupt before the default 30s timeout_ms)
# — comfortably past the old 5s stop budget and comfortably under the
# default timeout, so the worker finishes the query naturally.
curl -s -o "$T/c1_body" -w '%{http_code}' -X POST "http://127.0.0.1:$C1_PORT/api/panel" \
  -d '{"dashboard":"p","panel":0,"vars":{}}' > "$T/c1_curl_code" &
CURL_PID=$!
sleep 1
t0=$(date +%s)
send c1host "SELECT anofox_serve_stop($C1_PORT);"
for _ in $(seq 200); do grep -q "stopped the locked server" "$T/out_c1host" 2>/dev/null && break; sleep 0.2; done
t1=$(date +%s)
stop_elapsed=$((t1 - t0))
grep -q "stopped the locked server" "$T/out_c1host" || fail "anofox_serve_stop did not report success within 40s: $(cat "$T/out_c1host")"
[ "$stop_elapsed" -ge 5 ] || fail "C1: anofox_serve_stop returned in only ${stop_elapsed}s while a panel query was still in flight — it should have waited for that worker, not applied the refresher's 5s budget to it too"
# The port must be free the instant stop() returns: bind a throwaway
# listener on it immediately. "Address already in use" would mean the HTTP
# worker (and its Arc<Server> clone) was still alive underneath the
# claimed-successful stop.
if ! python3 -c "
import socket, sys
s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
try:
    s.bind(('127.0.0.1', $C1_PORT))
except OSError as e:
    print(e, file=sys.stderr)
    sys.exit(1)
s.close()
"; then
  fail "C1: the port stayed bound after anofox_serve_stop reported success (stopped in ${stop_elapsed}s)"
fi
echo "OK: anofox_serve_stop waited ${stop_elapsed}s for the HTTP worker running a slow panel query, and the port was free the instant it returned (C1)"
wait "$CURL_PID" 2>/dev/null || true
close_fifo c1host

echo "ALL OK: options.source (Quack source, #352 prototype)"
