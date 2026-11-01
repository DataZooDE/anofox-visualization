#!/usr/bin/env bash
# End-to-end checks of Ask on the locked server (#195, plan-ai-assistant.md
# §12.9) against a fake model (fake-model.py: it reads every panel, tries
# another viewer's variable value, and answers with everything it saw):
#   * canary rows: under a row policy, alice's question never sends bob's
#     rows upstream nor shows them to her (bob's own question does see his);
#     a claim-dependent ::MARKDOWN canary likewise; an `-- @ask skip` panel
#     never; the cached prefix (system + tools) is the same for both;
#   * /api/ask streams progress and the answer; citations are checked;
#   * 404 without `-- @ask on`; API keys 403; forged vars 400; an unknown
#     field 400; DELETE /api/ask with nothing running 404;
#   * the request log has the Ask line without the question;
#   * the start refuses Ask without a login, and without a key;
#   * without options.ask there is no /api/ask.
#
# usage: duckext/scripts/test-ask.sh <extension> [port]
set -euo pipefail
EXT="$(cd "$(dirname "$1")" && pwd)/$(basename "$1")"
case "$(basename "$EXT")" in
  anofox_visualization.duckdb_extension|anofox_visualization.*.duckdb_extension) ;;
  *) echo "$EXT: must be named anofox_visualization.duckdb_extension" >&2; exit 2 ;;
esac
PORT="${2:-18451}"
DUCKDB="${DUCKDB:-duckdb}"
HERE="$(cd "$(dirname "$0")" && pwd)"
W="$(dirname "$EXT")/ask-e2e-$$"
mkdir -p "$W/dash" "$W/model"
PIDS=()
cleanup() {
  for p in "${PIDS[@]}"; do kill "$p" 2>/dev/null || true; done
  rm -rf "$W"
}
trap cleanup EXIT
fail() { echo "FAIL: $*" >&2; exit 1; }
SEC="$(python3 -c 'import secrets; print(secrets.token_hex(24))')"
export FAKE_KEY="sk-ant-fake-$(python3 -c 'import secrets; print(secrets.token_hex(8))')"
export FAKE_TRY_VALUES="CANARYB"

python3 "$HERE/fake-model.py" "$W/model" & PIDS+=($!)
for _ in $(seq 50); do [ -s "$W/model/port" ] && break; sleep 0.1; done
MPORT="$(cat "$W/model/port")"

cat > "$W/dash/sales.sql" <<'SQL'
-- @title Sales
-- @ask on
-- @vars region
SELECT region ::DROPDOWN FROM (SELECT DISTINCT region FROM sales ORDER BY 1);
SELECT region AS "Region" ::XAXIS, sum(v) AS "Revenue" ::BARCHART FROM sales
WHERE region = getvariable('region') GROUP BY 1;
SELECT 'Hello ' || getvariable('__claim_user') || ', rows: ' || string_agg(region, ',') AS note ::MARKDOWN FROM sales;
-- @ask skip
SELECT secret AS "Secret" ::TABLE FROM vault;
SQL
cat > "$W/dash/plain.sql" <<'SQL'
-- @vars region
SELECT 1 AS n ::METRIC;
SQL
cat > "$W/policy.json" <<'JSON'
{"tables": {"sales": {"rows": "owner = getvariable('__claim_user')"}, "vault": {"rows": true}}}
JSON

# API keys: one for the 403 check.
KID=$(openssl rand -hex 6); KEY="afx_${KID}_$(openssl rand -hex 32)"
printf '{"keys": [{"id": "%s", "sha256": "%s", "user": "svc"}]}\n' "$KID" "$(printf %s "$KEY" | sha256sum | cut -d' ' -f1)" > "$W/keys.json"
chmod 600 "$W/keys.json"

AUTH="[{\"mode\": \"proxy\", \"shared_secret_env\": \"SEC\", \"trusted_proxies\": [\"127.0.0.1\"]}, {\"mode\": \"api_keys\", \"file\": \"$W/keys.json\"}]"
ASK="{\"enabled\": true, \"base_url\": \"http://127.0.0.1:$MPORT\"}"

# start <port> <options JSON>: a server on its own FIFO; returns 1 if it fails.
start() {
  local port="$1" opts="$2"
  rm -f "$W/in$port"; mkfifo "$W/in$port"
  ANTHROPIC_API_KEY="${KEYENV-$FAKE_KEY}" SEC=$SEC "$DUCKDB" -unsigned -batch :memory: < "$W/in$port" > "$W/out$port" 2>&1 &
  PIDS+=($!)
  exec {fd}>"$W/in$port"
  eval "FD$port=$fd"
  printf "CREATE TABLE sales AS SELECT * FROM (VALUES ('EU', 'alice', 10), ('CANARYB', 'bob', 999)) t(region, owner, v);
CREATE TABLE vault AS SELECT 'SKIPCANARY' AS secret;
LOAD '%s';
SELECT anofox_serve_dashboards('%s', %s, '%s');\n" "$EXT" "$W/dash" "$port" "$opts" >&"$fd"
  for _ in $(seq 100); do
    curl -sf "localhost:$port/health" >/dev/null 2>&1 && return 0
    grep -qE 'Error:' "$W/out$port" 2>/dev/null && break
    sleep 0.2
  done
  return 1
}
stop() {
  local port="$1" fdv="FD$1"
  echo "SELECT anofox_serve_stop($port);" >&"${!fdv}"
  eval "exec ${!fdv}>&-"
  for _ in $(seq 150); do curl -sf "localhost:$port/health" >/dev/null 2>&1 || return 0; sleep 0.2; done
}
# ask <user> <JSON body> → status; the body in $W/ask_out
ask() {
  curl -s -N -o "$W/ask_out" -w '%{http_code}' -X POST "localhost:$PORT/api/ask" \
    -H "X-Anofox-Proxy-Secret: $SEC" -H "X-Forwarded-User: $1" -H "Origin: http://localhost:$PORT" \
    -H "Content-Type: application/json" -d "$2"
}
# What the server sent upstream, without the model's own turns (it names
# the values it tries itself).
upstream() {
  [ -f "$W/model/requests.jsonl" ] || return 0
  python3 -c '
import json, sys
for line in open(sys.argv[1]):
    r = json.loads(line)
    r["messages"] = [m for m in r["messages"] if m["role"] != "assistant"]
    print(json.dumps(r, ensure_ascii=False))' "$W/model/requests.jsonl"
}

start "$PORT" "{\"auth\": $AUTH, \"policy\": \"$W/policy.json\", \"log\": \"$W/requests.log\", \"ask\": $ASK}" \
  || { cat "$W/out$PORT" >&2; fail "the Ask server should start"; }
grep -q "Ask on: model claude-sonnet-5" "$W/out$PORT" || fail "start message: $(cat "$W/out$PORT")"
grep -q "$FAKE_KEY" "$W/out$PORT" && fail "the key is in the start output"

# 1. alice: everything the fake model tries, nothing of bob's.
[ "$(ask alice '{"dashboard": "sales", "question": "Tell me everything", "tab": null, "vars": {"region": "EU"}}')" = 200 ] \
  || fail "alice's question: $(cat "$W/ask_out")"
grep -q '^event: progress' "$W/ask_out" && grep -q '^event: answer' "$W/ask_out" || fail "no stream: $(cat "$W/ask_out")"
ANS="$(grep -A1 '^event: answer' "$W/ask_out" | sed -n 's/^data: //p')"
for canary in CANARYB 999 bob SKIPCANARY; do
  upstream | grep -q "$canary" && fail "alice's question sent '$canary' upstream"
  printf %s "$ANS" | grep -q "$canary" && fail "alice's answer shows '$canary'"
done
printf %s "$ANS" | grep -q 'Hello alice, rows: EU' || fail "alice's own rows should reach her answer: $ANS"
printf %s "$ANS" | python3 -c 'import json,sys; a=json.load(sys.stdin); c=a["citations"][0]; assert c["verified"] and c["value"]==10, a' \
  || fail "the citation should verify: $ANS"
upstream | grep -q 'not one of the options' || fail "bob's region should be refused as an option"
upstream | grep -q "There is no panel 3" || fail "the @ask skip panel should be unknown to the model"
ALICE_PREFIX="$(head -1 "$W/model/requests.jsonl" | python3 -c 'import json,sys; r=json.load(sys.stdin); print(json.dumps([r["system"], r["tools"]], sort_keys=True))')"
: > "$W/model/requests.jsonl"

# 2. bob sees his own (the canary is real), with the same prefix.
[ "$(ask bob '{"dashboard": "sales", "question": "Tell me everything", "vars": {}}')" = 200 ] || fail "bob's question"
upstream | grep -q CANARYB || fail "bob's own rows should be readable to him (the fixture is wrong otherwise)"
BOB_PREFIX="$(head -1 "$W/model/requests.jsonl" | python3 -c 'import json,sys; r=json.load(sys.stdin); print(json.dumps([r["system"], r["tools"]], sort_keys=True))')"
[ "$ALICE_PREFIX" = "$BOB_PREFIX" ] || fail "the cached prefix differs between viewers"
upstream | grep -q "$FAKE_KEY" && fail "the key is in a request body"
echo "OK: Ask canaries — policy rows, claim-dependent text, @ask skip, options; same prefix for both viewers"

# 3. refusals.
[ "$(ask alice '{"dashboard": "plain", "question": "hi"}')" = 404 ] || fail "no @ask on should 404: $(cat "$W/ask_out")"
[ "$(ask alice '{"dashboard": "nope", "question": "hi"}')" = 404 ] || fail "an unknown dashboard should 404"
[ "$(ask alice '{"dashboard": "sales", "question": "hi", "vars": {"__claim_user": "bob"}}')" = 400 ] || fail "a forged claim var should 400"
[ "$(ask alice '{"dashboard": "sales", "question": "hi", "vars": {"anofox_data": "/etc"}}')" = 400 ] || fail "anofox_data should 400"
[ "$(ask alice '{"dashboard": "sales", "question": "hi", "history": []}')" = 400 ] || fail "a history field should 400"
[ "$(ask alice '{"dashboard": "sales", "question": ""}')" = 400 ] || fail "an empty question should 400"
code=$(curl -s -o "$W/ask_out" -w '%{http_code}' -X POST "localhost:$PORT/api/ask" -H "Authorization: Bearer $KEY" \
  -H "Origin: http://localhost:$PORT" -d '{"dashboard": "sales", "question": "hi"}')
[ "$code" = 403 ] || fail "an API key should 403 (got $code): $(cat "$W/ask_out")"
code=$(curl -s -o /dev/null -w '%{http_code}' -X POST "localhost:$PORT/api/ask" -H "X-Anofox-Proxy-Secret: $SEC" \
  -H "X-Forwarded-User: alice" -H "Origin: http://evil.example" -d '{"dashboard": "sales", "question": "hi"}')
[ "$code" = 403 ] || fail "a cross-origin ask should 403 (got $code)"
code=$(curl -s -o /dev/null -w '%{http_code}' -X POST "localhost:$PORT/api/ask" -d '{"dashboard": "sales", "question": "hi"}')
[ "$code" = 401 ] || fail "no login should 401 (got $code)"
code=$(curl -s -o /dev/null -w '%{http_code}' -X DELETE "localhost:$PORT/api/ask" -H "X-Anofox-Proxy-Secret: $SEC" \
  -H "X-Forwarded-User: alice" -H "Origin: http://localhost:$PORT")
[ "$code" = 404 ] || fail "DELETE with nothing running should 404 (got $code)"
echo "OK: Ask refusals — 404 without @ask on, forged vars 400, history 400, API key 403, cross-origin 403, no login 401"

# 4. the log: the Ask line, no question text.
grep -q '"path":"/api/ask"' "$W/requests.log" || fail "no Ask line in the log"
grep -q 'Tell me everything' "$W/requests.log" && fail "the question is in the log"
grep -q 'CANARYB\|Hello alice' "$W/requests.log" && fail "panel data is in the log"
echo "OK: Ask log — one line per question, no question or data"
stop "$PORT"

# 5. starts: no login → refused; no key → refused; no options.ask → no route.
if start "$PORT" "{\"policy\": \"none\", \"ask\": $ASK}"; then fail "Ask without a login should not start"; fi
grep -q "needs a login" "$W/out$PORT" || fail "no-login reason: $(cat "$W/out$PORT")"
KEYENV="" start "$PORT" "{\"auth\": $AUTH, \"policy\": \"none\", \"ask\": $ASK}" && fail "Ask without a key should not start"
grep -q "no model key" "$W/out$PORT" || fail "no-key reason: $(cat "$W/out$PORT")"
start "$PORT" "{\"auth\": $AUTH, \"policy\": \"none\"}" || fail "the plain server should start"
[ "$(ask alice '{"dashboard": "sales", "question": "hi"}')" = 404 ] || fail "without options.ask /api/ask should 404"
stop "$PORT"
echo "OK: Ask start checks — login and key required; no route without options.ask"
