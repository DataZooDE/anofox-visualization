#!/usr/bin/env bash
# End-to-end checks of POST /api/submit (#517 slice S2, plan §4): the one
# write path on the locked server.
#   * The CSRF gate: missing Origin, wrong Origin, missing the custom
#     header, and a non-JSON Content-Type are all 403.
#   * No credential is 401; a shared/proxy-forwarded identity without its
#     own personal sign-in would be refused (api_key/embed — not set up
#     here, see the plan's threat model for those).
#   * A valid submission: 201 with {seq, ts, gen}; the recorded value must
#     be one of the ::BUTTONS input's own options (plan §4.2 step 9).
#   * An answer outside the options is a 400, and nothing is appended.
#   * The same client id twice appends once (idempotency, step 10).
#   * The state file on disk has exactly the lines this produced.
#
# usage: duckext/scripts/test-submit-live.sh <extension> [port]
#        (DUCKDB=… picks the CLI; default: duckdb on PATH)
set -euo pipefail
EXT="$(cd "$(dirname "$1")" && pwd)/$(basename "$1")"
case "$(basename "$EXT")" in
  anofox_visualization.duckdb_extension|anofox_visualization.*.duckdb_extension) ;;
  *) echo "$EXT: LOAD takes the entry point from the file name, so it must be named anofox_visualization.duckdb_extension (or anofox_visualization.<x>.duckdb_extension). Rename it, or: ln -s '$EXT' /tmp/anofox_visualization.duckdb_extension" >&2; exit 2 ;;
esac
PORT="${2:-18420}"
DUCKDB="${DUCKDB:-duckdb}"
# Not under /tmp (or wherever $TMPDIR points): options.state.path refuses
# the system temp dir on purpose (plan §1) — a real deployment's state
# file is durable, local storage, not scratch space anything may sweep.
# Scratch lives next to the extension being tested instead.
W="$(dirname "$EXT")/submit-e2e-$$"
mkdir -p "$W"
PIDS=()
cleanup() {
  for p in "${PIDS[@]}"; do kill "$p" 2>/dev/null || true; done
  rm -rf "$W"
}
trap cleanup EXIT
fail() { echo "FAIL: $*" >&2; exit 1; }

SEC="$(python3 -c 'import secrets; print(secrets.token_hex(24))')"
mkdir -p "$W/dash"
cat > "$W/dash/d.sql" <<'EOF'
SELECT q1 ::BUTTONS FROM (VALUES ('A'), ('B'), ('C')) t(q1);
SELECT 'Check my answer' AS quiz_1::SUBMIT, 'q1' AS r::SUBMIT_VARS, 'Saved.' AS d::SUBMIT_DONE;
EOF
AUTH='"auth": {"mode": "proxy", "shared_secret_env": "SEC", "trusted_proxies": ["127.0.0.1"]}'
STATE="\"state\": {\"path\": \"$W/state.jsonl\", \"dashboards\": [\"d\"]}"
OPTS="{$AUTH, \"policy\": \"none\", $STATE, \"log\": \"$W/requests.jsonl\", \"audit\": \"$W/audit.jsonl\"}"

mkfifo "$W/in"
sleep 120 > "$W/in" & PIDS+=($!)
echo "LOAD '$EXT'; SELECT anofox_serve_dashboards('$W/dash', $PORT, '$OPTS');" > "$W/serve.sql"
SEC=$SEC "$DUCKDB" -unsigned -init "$W/serve.sql" < "$W/in" > "$W/stderr.log" 2>&1 & PIDS+=($!)
for _ in $(seq 100); do curl -s -m 1 "localhost:$PORT/health" > /dev/null 2>&1 && break; sleep 0.2; done
if ! grep -q '"status":"ok"' <(curl -s "localhost:$PORT/health"); then
  cat "$W/stderr.log" >&2
  fail "server did not start"
fi

ORIGIN="http://localhost:$PORT"
AUTH_H=(-H "X-Anofox-Proxy-Secret: $SEC" -H "X-Forwarded-User: alice")
# submit <body>: POSTs with ${AUTH_H[@]} plus whatever is in the global
# array HDRS (each caller sets HDRS first) — an array, never a
# space-joined string, so a header value with its own space survives.
submit() {
  curl -s -o "$W/out" -w '%{http_code}' -X POST "localhost:$PORT/api/submit" \
    "${AUTH_H[@]}" "${HDRS[@]}" -d "$1"
}

BODY_OK='{"dashboard":"d","panel":1,"id":"deadbeef01","vars":{"q1":"B"}}'

HDRS=(-H "X-Anofox-Submit: 1" -H "Content-Type: application/json" -H "Sec-Fetch-Site: same-origin")
CODE=$(submit "$BODY_OK")
[ "$CODE" = "403" ] || fail "missing Origin should be 403, got $CODE: $(cat "$W/out")"
echo "OK: missing Origin is 403"

HDRS=(-H "Origin: http://evil.example.com" -H "X-Anofox-Submit: 1" -H "Content-Type: application/json" -H "Sec-Fetch-Site: same-origin")
CODE=$(submit "$BODY_OK")
[ "$CODE" = "403" ] || fail "wrong Origin should be 403, got $CODE: $(cat "$W/out")"
echo "OK: wrong Origin is 403"

HDRS=(-H "Origin: $ORIGIN" -H "Content-Type: application/json" -H "Sec-Fetch-Site: same-origin")
CODE=$(submit "$BODY_OK")
[ "$CODE" = "403" ] || fail "missing X-Anofox-Submit should be 403, got $CODE: $(cat "$W/out")"
echo "OK: missing X-Anofox-Submit header is 403"

HDRS=(-H "Origin: $ORIGIN" -H "X-Anofox-Submit: 1" -H "Content-Type: text/plain" -H "Sec-Fetch-Site: same-origin")
CODE=$(submit "$BODY_OK")
[ "$CODE" = "403" ] || fail "non-JSON Content-Type should be 403, got $CODE: $(cat "$W/out")"
echo "OK: non-JSON Content-Type is 403"

HDRS=(-H "Origin: $ORIGIN" -H "X-Anofox-Submit: 1" -H "Content-Type: application/json" -H "Sec-Fetch-Site: same-origin")
CODE=$(curl -s -o "$W/out" -w '%{http_code}' -X POST "localhost:$PORT/api/submit" "${HDRS[@]}" -d "$BODY_OK")
[ "$CODE" = "401" ] || fail "no credential should be 401, got $CODE: $(cat "$W/out")"
echo "OK: no credential is 401"

HDRS=(-H "Origin: $ORIGIN" -H "X-Anofox-Submit: 1" -H "Content-Type: application/json" -H "Sec-Fetch-Site: same-origin")
CODE=$(submit '{"dashboard":"d","panel":1,"id":"baadf00d","vars":{"q1":"Z"}}')
[ "$CODE" = "400" ] || fail "an answer outside the options should be 400, got $CODE: $(cat "$W/out")"
[ ! -s "$W/state.jsonl" ] || ! grep -q '"q1":"Z"' "$W/state.jsonl" || fail "an invalid answer was appended"
echo "OK: an answer outside the options is 400 and nothing is appended"

CODE=$(submit "$BODY_OK")
[ "$CODE" = "201" ] || fail "a valid submission should be 201, got $CODE: $(cat "$W/out")"
SEQ1=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["seq"])' "$W/out")
GEN1=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["gen"])' "$W/out")
[ "$SEQ1" = "1" ] || fail "first submission's seq should be 1, got $SEQ1"
[ "$GEN1" = "1" ] || fail "first submission's gen (alice's row count) should be 1, got $GEN1"
echo "OK: a valid submission is 201 with seq=1, gen=1"

# Idempotent retry: the same id appends nothing new. HDRS is still what
# the valid submission above used.
CODE=$(submit "$BODY_OK")
[ "$CODE" = "201" ] || fail "a retried submission should still be 201, got $CODE: $(cat "$W/out")"
SEQ2=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["seq"])' "$W/out")
[ "$SEQ2" = "$SEQ1" ] || fail "a retried id should return the same seq, got $SEQ2 vs $SEQ1"
echo "OK: the same id twice appends once (idempotent retry)"

LINES=$(wc -l < "$W/state.jsonl")
[ "$LINES" = "2" ] || fail "expected a header + 1 record line, got $LINES lines: $(cat "$W/state.jsonl")"
grep -q '"user":"alice"' "$W/state.jsonl" || fail "the record should carry alice as the user"
grep -q '"q1":"B"' "$W/state.jsonl" || fail "the record should carry q1=B"
echo "OK: the state file has exactly one record line"

if [ -s "$W/audit.jsonl" ]; then
  grep -q '"audit":"submit","outcome":"ok"' "$W/audit.jsonl" || fail "expected an ok submit audit event: $(cat "$W/audit.jsonl")"
  ! grep -q '"q1"' "$W/audit.jsonl" || fail "the audit log must never carry a value"
  echo "OK: the audit log has a submit event and no values"
fi

echo "PASS: submit-live"
