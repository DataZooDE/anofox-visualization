#!/usr/bin/env bash
# End-to-end checks of the Check button on a live, authoring backend (#199):
# `anofox_serve(port)`'s new `/check` endpoint, through the DuckDB CLI and
# curl.
#   * `/check` needs the same auth as `/query` (token cookie / header; no
#     new unauthenticated surface) — a request with neither is 401.
#   * A checked script's own `CREATE TABLE`/`SET VARIABLE` never reaches the
#     live session: `/query` right after shows neither (isolation).
#   * The checked script can still read the live session's own tables (an
#     input/::SELECT_AS default needs real rows).
#   * The scratch database `/check` attaches for its isolation is gone
#     afterwards — no `anofox_check_N` pile-up in `duckdb_databases()`.
#   * A script the real lint would flag comes back with a non-empty
#     `diagnostics` array through `/check` too (not just "ok": true always).
#   * A hung/slow checked query is interrupted by its own `timeout_ms`
#     instead of holding the one check connection forever; the next,
#     ordinary check still succeeds right after (the connection recovers).
# usage: duckext/scripts/test-check-live.sh <extension> [port]
#        (DUCKDB=… picks the CLI; default: duckdb on PATH)
set -euo pipefail
EXT="$(cd "$(dirname "$1")" && pwd)/$(basename "$1")"
case "$(basename "$EXT")" in
  anofox_visualization.duckdb_extension|anofox_visualization.*.duckdb_extension) ;;
  *) echo "$EXT: LOAD takes the entry point from the file name, so it must be named anofox_visualization.duckdb_extension (or anofox_visualization.<x>.duckdb_extension). Rename it, or: ln -s '$EXT' /tmp/anofox_visualization.duckdb_extension" >&2; exit 2 ;;
esac
PORT="${2:-18234}"
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

mkfifo "$T/in"
"$DUCKDB" -unsigned ":memory:" < "$T/in" > "$T/out" 2>&1 &
PIDS+=($!)
exec 3>"$T/in"
rm "$T/in"
printf "LOAD %s;\nCREATE TABLE sales AS SELECT 1 AS amount;\nSELECT anofox_serve(%s);\n" "$(q "$EXT")" "$PORT" >&3
TOKEN=""
for _ in $(seq 100); do
  TOKEN="$(grep -oE 'token=[0-9a-f]+' "$T/out" 2>/dev/null | head -1 | cut -d= -f2 || true)"
  if [ -n "$TOKEN" ]; then
    break
  fi
  if grep -qE '^([A-Za-z][A-Za-z ]* )?Error:' "$T/out" 2>/dev/null; then
    cat "$T/out" >&2
    fail "anofox_serve did not start"
  fi
  sleep 0.2
done
if [ -z "$TOKEN" ]; then
  cat "$T/out" >&2
  fail "could not find the session token in anofox_serve's own output"
fi

# No cookie, no header: 401.
CODE="$(curl -s -o /dev/null -w '%{http_code}' -X POST "http://127.0.0.1:$PORT/check" -d '{"script":"SELECT 1;"}')"
[ "$CODE" = "401" ] || fail "/check without a token should be 401, got $CODE"
echo "OK: /check needs the token, like /query"

# Hand over the token for an HttpOnly cookie, the same bootstrap the builder's
# own page load does.
curl -s -o /dev/null -c "$T/cookie" "http://127.0.0.1:$PORT/?token=$TOKEN"

check() {
  curl -s -b "$T/cookie" -X POST "http://127.0.0.1:$PORT/check" \
    -H 'Content-Type: application/json' -d "$1"
}
query() {
  curl -s -b "$T/cookie" -X POST "http://127.0.0.1:$PORT/query" -d "$1"
}
json_get() { # $1=json $2=jq-ish python expr
  python3 -c "import json,sys; v=json.loads(sys.argv[1]); print(json.dumps(eval(sys.argv[2])))" "$1" "$2"
}

OUT="$(check '{"script": "CREATE TABLE leaked AS SELECT 1 AS x; SET VARIABLE v = 1; SELECT 1 AS ok;"}')"
[ "$(json_get "$OUT" 'v["ok"]')" = "true" ] || fail "a harmless setup script should check ok: $OUT"
echo "OK: /check runs setup + a query and reports ok"

TABLES="$(query "SELECT table_name FROM duckdb_tables() WHERE table_name = 'leaked'")"
[ "$TABLES" = "[]" ] || fail "a plain CREATE TABLE in the checked script leaked onto the live connection: $TABLES"
echo "OK: the checked script's CREATE TABLE never reached the live session"

VAR="$(query "SELECT getvariable('v') AS v")"
[ "$VAR" = "[{\"v\":null}]" ] || fail "the checked script's SET VARIABLE leaked onto the live connection: $VAR"
echo "OK: the checked script's SET VARIABLE never reached the live session"

OUT="$(check '{"script": "SELECT amount::XAXIS FROM sales;"}')"
[ "$(json_get "$OUT" 'v["ok"]')" = "true" ] || fail "a script reading the live session's own table should check ok: $OUT"
echo "OK: /check can read the live session's own tables"

DBS="$(query "SELECT database_name FROM duckdb_databases() WHERE database_name LIKE 'anofox_check_%'")"
[ "$DBS" = "[]" ] || fail "a /check scratch database was left attached: $DBS"
echo "OK: /check detaches its own scratch database"

OUT="$(check '{"script": "SELECT 1 AS x::NOT_A_ROLE;"}')"
NDIAGS="$(json_get "$OUT" 'len(v["diagnostics"])')"
[ "$NDIAGS" != "0" ] || fail "a script the real lint flags should come back with diagnostics: $OUT"
echo "OK: /check reports real diagnostics, not just \"ok\": true"

# #517, review of #530 #5: a ::SUBMIT/viewer_state dashboard's empty temp
# (seeded by lint, since no real server has populated it) must not outlive
# its own check on the one, reused check connection — otherwise it shadows
# whatever a *later*, unrelated check's own viewer_state resolves to.
OUT="$(check '{"script": "SELECT value::TEXT_SMALL FROM viewer_state;"}')"
[ "$(json_get "$OUT" 'v["ok"]')" = "true" ] || fail "a viewer_state read should check ok against the seeded empty temp: $OUT"
echo "OK: a ::SUBMIT/viewer_state dashboard checks clean against its seeded empty temp"

query "CREATE TABLE viewer_state AS SELECT 'real' AS value" >/dev/null
OUT="$(check '{"script": "SELECT value::TEXT_SMALL FROM viewer_state;"}')"
CODES="$(json_get "$OUT" '[d["code"] for d in v["diagnostics"]]' 2>/dev/null)"
case "$CODES" in
  *empty-panel*) fail "a later check's real viewer_state was shadowed by the earlier check's leftover empty temp: $OUT" ;;
esac
echo "OK: the check connection does not leak a viewer_state temp into the next check"
query "DROP TABLE viewer_state" >/dev/null

# A query that would run forever, with a short timeout_ms: /check must not
# hang (there is only the one check connection — a hung query would hold it
# for every later check, forever, without this).
OUT="$(check '{"script": "SELECT count(*) FROM range(100000000000) a, range(100000000000) b;", "timeout_ms": 1000}')"
[ "$(json_get "$OUT" 'v["ok"]')" = "false" ] || fail "a timed-out check should not read ok: true: $OUT"
CODE="$(json_get "$OUT" '[d["code"] for d in v["diagnostics"]]' 2>/dev/null)"
case "$CODE" in *check/timeout*) ;; *) fail "a timed-out check should carry a check/timeout diagnostic: $OUT" ;; esac
echo "OK: /check interrupts a query past its timeout_ms"

# The check connection recovers: an ordinary check right after still works
# (not left wedged by the interrupt above).
OUT="$(check '{"script": "SELECT 1 AS ok;"}')"
[ "$(json_get "$OUT" 'v["ok"]')" = "true" ] || fail "the check connection did not recover after a timeout: $OUT"
echo "OK: /check recovers for the next call after a timeout"

DBS="$(query "SELECT database_name FROM duckdb_databases() WHERE database_name LIKE 'anofox_check_%'")"
[ "$DBS" = "[]" ] || fail "the timed-out check's scratch database was left attached: $DBS"
echo "OK: a timed-out check still detaches its scratch database"

echo "SELECT anofox_serve_stop($PORT);" >&3
exec 3>&-
echo "PASS: check-live"
