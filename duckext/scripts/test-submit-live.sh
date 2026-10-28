#!/usr/bin/env bash
# End-to-end checks of POST /api/submit (#517 slice S2, plan §4): the one
# write path on the locked server. Extended per review-530's round 2 of
# #538 (should-fixes) to also cover, against a real running server:
#   * api_key and an embed link without its own user are both 403
#     (shared credentials never submit as themselves).
#   * `Sec-Fetch-Site: cross-site` is 403 (same-origin is already covered
#     by the missing-header case below; this is the explicit cross-site
#     value).
#   * a dashboard that exists but isn't in `options.state.dashboards` is
#     403, a `panel` that doesn't name a `::SUBMIT` statement is 400, and
#     a `user` field in the body is 400 (the server assigns the user).
#   * B1 (review of #538, blocker 1): a credential-locked variable (an
#     embed link's own `vars`) is recorded as its locked value, never the
#     viewer's own.
#   * B2 (blocker 2): a `::DROPDOWN` preceded by its own `::OPTION_LABEL`
#     column is validated against its own column (the code), not the
#     label text that happens to come first.
#   * B3 (blocker 3) + the per-user rate limit: one user's flood doesn't
#     429 a different user's very next submission, and a 429 carries
#     Retry-After.
#
# ...and per review-530's round 2 re-review of that same push:
#   * a NEW blocker it found (partly an ambiguous instruction of mine):
#     the per-viewer mutex must never block an HTTP worker thread — one
#     viewer's flood against a slow ::SUBMIT must not stall a different
#     viewer's unrelated /api/panel request.
#   * a nit: effective_value must pick the SAME spelling dedup_last_spelling
#     would (the last one, case-insensitively), covered by a unit test
#     (duckext/src/serve.rs) rather than here.
#
# Original coverage (plan §4, still exercised below):
#   * The CSRF gate: missing Origin, wrong Origin, missing the custom
#     header, and a non-JSON Content-Type are all 403.
#   * No credential is 401.
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
EMBED_SECRET="$(python3 -c 'import secrets; print(secrets.token_hex(24))')"
mkdir -p "$W/dash"
# `d`: state-enabled. Panel 0/1 is the original BUTTONS+SUBMIT pair (q1).
# Panel 2/3 adds a ::DROPDOWN whose own ::OPTION_LABEL column comes FIRST
# (q2) — the exact shape review-530's blocker 2 (col_idx, not the row's
# first column) needs to tell apart from the label text. Panel 4/5 adds a
# ::MULTISELECT (q3) — a locked *list* value (review-530 round 2's last
# nit: literal_to_json must invert a list literal too, not just a scalar).
cat > "$W/dash/d.sql" <<'EOF'
SELECT q1 ::BUTTONS FROM (VALUES ('A'), ('B'), ('C')) t(q1);
SELECT 'Check my answer' AS quiz_1::SUBMIT, 'q1' AS r::SUBMIT_VARS, 'Saved.' AS d::SUBMIT_DONE;
SELECT label AS lbl ::OPTION_LABEL, code AS q2 ::DROPDOWN FROM (VALUES ('Alpha', 'A'), ('Beta', 'B')) t(label, code);
SELECT 'Check q2' AS quiz_2::SUBMIT, 'q2' AS r::SUBMIT_VARS, 'Saved.' AS d::SUBMIT_DONE;
SELECT val AS q3 ::MULTISELECT FROM (VALUES ('A'), ('B'), ('C'), ('D')) t(val);
SELECT 'Check q3' AS quiz_3::SUBMIT, 'q3' AS r::SUBMIT_VARS, 'Saved.' AS d::SUBMIT_DONE;
EOF
# `d2`: exists, but deliberately left out of options.state.dashboards.
mkdir -p "$W/dash2"
cat > "$W/dash/d2.sql" <<'EOF'
SELECT q1 ::BUTTONS FROM (VALUES ('A'), ('B'), ('C')) t(q1);
SELECT 'Check my answer' AS quiz_1::SUBMIT, 'q1' AS r::SUBMIT_VARS, 'Saved.' AS d::SUBMIT_DONE;
EOF
# `slow`: its ::SUBMIT statement carries a deliberately slow extra column
# (a few seconds of real DuckDB work, not a sleep) — the concurrency
# check below floods it from one viewer to prove the per-viewer mutex
# never blocks an HTTP worker thread long enough to stall a different
# viewer's unrelated /api/panel request (review of #538 round 2, the new
# blocker).
cat > "$W/dash/slow.sql" <<'EOF'
SELECT q1 ::BUTTONS FROM (VALUES ('A'), ('B'), ('C')) t(q1);
SELECT 'Check my answer' AS quiz_1::SUBMIT, 'q1' AS r::SUBMIT_VARS, 'Saved.' AS d::SUBMIT_DONE,
       (SELECT count(*) FROM range(900000000) a, range(100) b) AS gate;
EOF

# A server-issued API key (`afx_<id>_<secret>`, file holds only its hash) —
# used only to prove the endpoint refuses it (shared/server-to-server
# credentials never submit as themselves).
APIKEY="$(python3 - "$W/keys.json" <<'PY'
import hashlib, json, secrets, sys
path = sys.argv[1]
kid = "0123456789ab"
secret = secrets.token_hex(32)
key = f"afx_{kid}_{secret}"
h = hashlib.sha256(key.encode()).hexdigest()
json.dump({"keys": [{"id": kid, "sha256": h, "user": "keyuser", "dashboards": ["d"]}]}, open(path, "w"))
print(key)
PY
)"

# Mint an embed link by hand (HS256 JWT, signed with $EMBED_SECRET): prints
# the link token for a JSON payload fragment (minus `exp`, added here).
mint_embed() {
  python3 - "$EMBED_SECRET" "$1" <<'PY'
import base64, hashlib, hmac, json, sys, time

def b64url(b):
    return base64.urlsafe_b64encode(b).rstrip(b"=").decode()

secret = sys.argv[1].encode()
payload = json.loads(sys.argv[2])
payload.setdefault("exp", int(time.time()) + 60)
h = b64url(json.dumps({"alg": "HS256", "typ": "JWT"}, separators=(",", ":")).encode())
p = b64url(json.dumps(payload, separators=(",", ":")).encode())
sig = hmac.new(secret, f"{h}.{p}".encode(), hashlib.sha256).digest()
print(f"{h}.{p}.{b64url(sig)}")
PY
}

# Exchange an embed link for a page session (GET /d/d?embed=<link>) and
# print the session token the page would carry in window.__served.session
# (and resend as X-Anofox-Session) — never in the URL after this point.
embed_session() {
  curl -s "localhost:$PORT/d/d?embed=$1" | python3 -c '
import re, sys
m = re.search(r"\"session\":\"([^\"]+)\"", sys.stdin.read())
print(m.group(1) if m else "")
'
}

AUTH="\"auth\": [{\"mode\": \"proxy\", \"shared_secret_env\": \"SEC\", \"trusted_proxies\": [\"127.0.0.1\"]}, {\"mode\": \"embed\", \"secret_env\": \"EMBED_SECRET\"}, {\"mode\": \"api_keys\", \"file\": \"$W/keys.json\"}]"
# burst raised from the default (3) to 15: the concurrency check below
# needs >= 12 of one viewer's concurrent requests to all clear the
# per-user rate check and actually contend for the per-viewer mutex (the
# thing it's testing); the 429 check further down floods past this
# larger burst instead.
STATE="\"state\": {\"path\": \"$W/state.jsonl\", \"dashboards\": [\"d\", \"slow\"], \"rate\": {\"per_minute\": 60, \"burst\": 15}}"
# The whole OPTS JSON is embedded in a SQL single-quoted string below, so a
# literal `'self'` (frame_ancestors' own CSP-keyword syntax) needs its
# quotes SQL-doubled to survive that string.
OPTS="{$AUTH, \"policy\": \"none\", $STATE, \"frame_ancestors\": [\"''self''\"], \"log\": \"$W/requests.jsonl\", \"audit\": \"$W/audit.jsonl\"}"

mkfifo "$W/in"
sleep 120 > "$W/in" & PIDS+=($!)
echo "LOAD '$EXT'; SELECT anofox_serve_dashboards('$W/dash', $PORT, '$OPTS');" > "$W/serve.sql"
SEC=$SEC EMBED_SECRET=$EMBED_SECRET "$DUCKDB" -unsigned -init "$W/serve.sql" < "$W/in" > "$W/stderr.log" 2>&1 & PIDS+=($!)
for _ in $(seq 100); do curl -s -m 1 "localhost:$PORT/health" > /dev/null 2>&1 && break; sleep 0.2; done
if ! grep -q '"status":"ok"' <(curl -s "localhost:$PORT/health"); then
  cat "$W/stderr.log" >&2
  fail "server did not start"
fi

ORIGIN="http://localhost:$PORT"
AUTH_H=(-H "X-Anofox-Proxy-Secret: $SEC" -H "X-Forwarded-User: alice")
CREDS=("${AUTH_H[@]}")
# submit <body>: POSTs with ${CREDS[@]} (the credential under test; default
# alice via the proxy) plus whatever is in the global array HDRS (each
# caller sets HDRS first) — arrays, never space-joined strings, so a header
# value with its own space survives.
submit() {
  curl -s -o "$W/out" -w '%{http_code}' -X POST "localhost:$PORT/api/submit" \
    "${CREDS[@]}" "${HDRS[@]}" -d "$1"
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

HDRS=(-H "Origin: $ORIGIN" -H "X-Anofox-Submit: 1" -H "Content-Type: application/json" -H "Sec-Fetch-Site: cross-site")
CODE=$(submit "$BODY_OK")
[ "$CODE" = "403" ] || fail "Sec-Fetch-Site: cross-site should be 403, got $CODE: $(cat "$W/out")"
echo "OK: Sec-Fetch-Site: cross-site is 403"

HDRS=(-H "Origin: $ORIGIN" -H "X-Anofox-Submit: 1" -H "Content-Type: application/json" -H "Sec-Fetch-Site: same-origin")
CODE=$(curl -s -o "$W/out" -w '%{http_code}' -X POST "localhost:$PORT/api/submit" "${HDRS[@]}" -d "$BODY_OK")
[ "$CODE" = "401" ] || fail "no credential should be 401, got $CODE: $(cat "$W/out")"
echo "OK: no credential is 401"

# A shared/server-to-server credential never submits as itself (step 3).
CREDS=(-H "Authorization: Bearer $APIKEY")
CODE=$(submit '{"dashboard":"d","panel":1,"id":"a1a1a1a1","vars":{"q1":"A"}}')
[ "$CODE" = "403" ] || fail "an api_key should be 403, got $CODE: $(cat "$W/out")"
echo "OK: an api_key identity is 403 (submissions need a personal sign-in)"
CREDS=("${AUTH_H[@]}")

LINK_NOUSER="$(mint_embed '{"dashboard":"d"}')"
SESSION_NOUSER="$(embed_session "$LINK_NOUSER")"
[ -n "$SESSION_NOUSER" ] || fail "could not mint an embed session (no user)"
CREDS=(-H "X-Anofox-Session: $SESSION_NOUSER")
CODE=$(submit '{"dashboard":"d","panel":1,"id":"b2b2b2b2","vars":{"q1":"A"}}')
[ "$CODE" = "403" ] || fail "an embed link without its own user should be 403, got $CODE: $(cat "$W/out")"
echo "OK: an embed link without its own user (embed:<dashboard>) is 403"
CREDS=("${AUTH_H[@]}")

CODE=$(submit '{"dashboard":"d2","panel":1,"id":"c3c3c3c3","vars":{"q1":"A"}}')
[ "$CODE" = "403" ] || fail "a dashboard outside options.state.dashboards should be 403, got $CODE: $(cat "$W/out")"
echo "OK: a dashboard that isn't submit-enabled is 403"

CODE=$(submit '{"dashboard":"d","panel":0,"id":"d4d4d4d4","vars":{"q1":"A"}}')
[ "$CODE" = "400" ] || fail "a panel that isn't a ::SUBMIT statement should be 400, got $CODE: $(cat "$W/out")"
echo "OK: a non-::SUBMIT panel index is 400"

CODE=$(submit '{"dashboard":"d","panel":1,"id":"e5e5e5e5","vars":{"q1":"A"},"user":"mallory"}')
[ "$CODE" = "400" ] || fail "a user field in the body should be 400, got $CODE: $(cat "$W/out")"
echo "OK: a user field in the body is 400 (the server assigns the user)"

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

# --- B2 (review of #538, blocker 2): a ::DROPDOWN's own ::OPTION_LABEL
# column comes first in the row, so the real options must be read from the
# DROPDOWN's own column index (1), never the row's first column (the label
# text, column 0). A fresh user (alice's own per-user bucket is already
# partly spent by the checks above): the point here is the column, not the
# rate limit.
CREDS=(-H "X-Anofox-Proxy-Secret: $SEC" -H "X-Forwarded-User: carol")
CODE=$(submit '{"dashboard":"d","panel":3,"id":"f6f6f6f6","vars":{"q2":"A"}}')
[ "$CODE" = "201" ] || fail "B2: a valid DROPDOWN code should be 201, got $CODE: $(cat "$W/out")"
grep -q '"q2":"A"' "$W/state.jsonl" || fail "B2: expected q2=A (the code) to be recorded"
echo "OK: B2 — a ::DROPDOWN preceded by its own ::OPTION_LABEL validates against its own column"

CODE=$(submit '{"dashboard":"d","panel":3,"id":"a7a7a7a7","vars":{"q2":"Alpha"}}')
[ "$CODE" = "400" ] || fail "B2: the label text itself should be refused, got $CODE: $(cat "$W/out")"
! grep -q '"q2":"Alpha"' "$W/state.jsonl" || fail "B2: the label text must never be recorded as the value"
echo "OK: B2 — the ::OPTION_LABEL text itself is rejected (it is not one of the real options)"
CREDS=("${AUTH_H[@]}")

# --- B1 (blocker 1): a credential-locked variable (an embed link's own
# `vars`) always wins over whatever the viewer sent — recorded as the
# locked value, never the viewer's own.
LINK_EVE="$(mint_embed '{"dashboard":"d","user":"eve","vars":{"q1":"A"}}')"
SESSION_EVE="$(embed_session "$LINK_EVE")"
[ -n "$SESSION_EVE" ] || fail "could not mint an embed session (eve, locked q1=A)"
CREDS=(-H "X-Anofox-Session: $SESSION_EVE")
CODE=$(submit '{"dashboard":"d","panel":1,"id":"b8b8b8b8","vars":{"q1":"B"}}')
[ "$CODE" = "201" ] || fail "B1: a locked-var submission should still be 201, got $CODE: $(cat "$W/out")"
EVE_LINE=$(grep '"user":"eve"' "$W/state.jsonl") || fail "B1: expected a recorded row for eve"
grep <<<"$EVE_LINE" -q '"q1":"A"' || fail "B1: expected the locked value q1=A to be recorded, got: $EVE_LINE"
if grep <<<"$EVE_LINE" -q '"q1":"B"'; then fail "B1: the viewer's own value q1=B must never override the locked value"; fi
echo "OK: B1 — a credential-locked variable records the locked value, not the viewer's own"
CREDS=("${AUTH_H[@]}")

# --- Review-530 round 2, last nit: a credential-locked *list* value
# (::MULTISELECT) must win the same way a locked scalar does (B1, above) —
# literal_to_json has to invert sv::sql_literal's list form too, not just
# its scalars.
LINK_ZOE="$(mint_embed '{"dashboard":"d","user":"zoe","vars":{"q3":["A","C"]}}')"
SESSION_ZOE="$(embed_session "$LINK_ZOE")"
[ -n "$SESSION_ZOE" ] || fail "could not mint an embed session (zoe, locked q3=[A,C])"
CREDS=(-H "X-Anofox-Session: $SESSION_ZOE")
CODE=$(submit '{"dashboard":"d","panel":5,"id":"c0ff0001","vars":{"q3":["B","D"]}}')
[ "$CODE" = "201" ] || fail "a locked ::MULTISELECT submission should be 201, got $CODE: $(cat "$W/out")"
ZOE_LINE=$(grep '"user":"zoe"' "$W/state.jsonl") || fail "expected a recorded row for zoe"
grep <<<"$ZOE_LINE" -q '"q3":\["A","C"\]' || fail "expected the locked list q3=[\"A\",\"C\"] to be recorded, got: $ZOE_LINE"
if grep <<<"$ZOE_LINE" -q '"q3":\["B","D"\]'; then fail "the viewer's own selection q3=[B,D] must never override the locked list"; fi
echo "OK: a credential-locked ::MULTISELECT (list) value records the locked list, not the viewer's own"
CREDS=("${AUTH_H[@]}")

# --- B3 (blocker 3) + the per-user rate limit: a flood from one user must
# not spend a different user's budget, and once a user's own bucket is
# spent, further submissions are 429 with Retry-After. BURST matches
# STATE's rate.burst above (raised from the plan's default of 3, for the
# concurrency check further down). A fresh global-rate window first: the
# tests above already spent a handful of the shared 20/s budget, which
# would otherwise 429 flooder before its own (per-user) burst does.
sleep 1.1
BURST=15
HDRS=(-H "Origin: $ORIGIN" -H "X-Anofox-Submit: 1" -H "Content-Type: application/json" -H "Sec-Fetch-Site: same-origin")
CREDS=(-H "X-Anofox-Proxy-Secret: $SEC" -H "X-Forwarded-User: flooder")
CODES=()
for i in $(seq 1 $((BURST + 3))); do
  id="$(printf 'f100d%02x' "$i")"
  CODES+=("$(submit "{\"dashboard\":\"d\",\"panel\":1,\"id\":\"$id\",\"vars\":{\"q1\":\"B\"}}")")
done
for i in $(seq 0 $((BURST - 1))); do
  [ "${CODES[$i]}" = "201" ] || fail "flooder's submission #$((i + 1)) (within burst=$BURST) should be 201, got ${CODES[$i]}"
done
LATER_429=0
for c in "${CODES[@]:$BURST}"; do [ "$c" = "429" ] && LATER_429=1; done
[ "$LATER_429" = "1" ] || fail "expected at least one 429 past burst=$BURST, got: ${CODES[*]}"
echo "OK: rapid submissions from one user run into 429 past its burst=$BURST (${CODES[*]})"

HEADERS=$(curl -s -D - -o /dev/null -X POST "localhost:$PORT/api/submit" "${CREDS[@]}" "${HDRS[@]}" -d '{"dashboard":"d","panel":1,"id":"f100dff","vars":{"q1":"B"}}')
grep <<<"$HEADERS" -qi '^HTTP/[0-9.]* 429' || fail "expected a 429 once the burst is spent: $HEADERS"
grep <<<"$HEADERS" -qi '^Retry-After:' || fail "a 429 must carry Retry-After: $HEADERS"
echo "OK: a 429 carries Retry-After"

# A fresh global-rate window (B3 is about the *per-user* bucket, not the
# shared global one — every request so far, including the flood, also
# spent a global token on its way through, so without this the global
# window itself could still be near-empty here, for a reason that has
# nothing to do with the per-user isolation this checks).
sleep 1.1
CREDS=(-H "X-Anofox-Proxy-Secret: $SEC" -H "X-Forwarded-User: bob")
CODE=$(submit '{"dashboard":"d","panel":1,"id":"cafed00d","vars":{"q1":"C"}}')
[ "$CODE" = "201" ] || fail "B3: bob's submission right after flooder's flood should be 201, got $CODE: $(cat "$W/out")"
echo "OK: B3 — one user's flood does not rate-limit a different user's next submission"
CREDS=("${AUTH_H[@]}")

# --- Review-530 round 2's new blocker: the per-viewer mutex must never
# block an HTTP worker thread. `slow`'s ::SUBMIT carries a column that
# takes several real seconds of DuckDB work; 12 concurrent submissions
# from one viewer ("turtle") must not keep a different viewer's
# unrelated /api/panel request waiting — with a blocking lock() instead
# of try_lock(), the review's own repro turned a 27s stall. A fresh
# global-rate window first (12 is a lot of the shared 20/s budget to risk
# on whatever's left over from the tests above).
sleep 1.1
HDRS=(-H "Origin: $ORIGIN" -H "X-Anofox-Submit: 1" -H "Content-Type: application/json" -H "Sec-Fetch-Site: same-origin")
CREDS=(-H "X-Anofox-Proxy-Secret: $SEC" -H "X-Forwarded-User: turtle")
FLOOD_PIDS=()
for id in c0ffee01 c0ffee02 c0ffee03 c0ffee04 c0ffee05 c0ffee06 c0ffee07 c0ffee08 c0ffee09 c0ffee0a c0ffee0b c0ffee0c; do
  curl -s -o /dev/null -X POST "localhost:$PORT/api/submit" "${CREDS[@]}" "${HDRS[@]}" \
    -d "{\"dashboard\":\"slow\",\"panel\":1,\"id\":\"$id\",\"vars\":{\"q1\":\"A\"}}" &
  FLOOD_PIDS+=($!)
done
sleep 0.3 # let the flood actually reach the server before probing
T0=$(date +%s%N)
# `-m 8` bounds curl itself: a blocked worker thread must fail this check
# with a clear message below, not hang the whole script or abort it
# silently under `set -e` on curl's own timeout exit code (28).
PANEL_CODE=$(curl -s -o "$W/panel_out" -w '%{http_code}' -m 8 -X POST "localhost:$PORT/api/panel" \
  -H "Origin: $ORIGIN" -H "X-Anofox-Proxy-Secret: $SEC" -H "X-Forwarded-User: dana" \
  -d '{"dashboard":"d","panel":0,"vars":{}}') || PANEL_CODE="curl-timeout"
T1=$(date +%s%N)
MS=$(( (T1 - T0) / 1000000 ))
for p in "${FLOOD_PIDS[@]}"; do wait "$p" 2>/dev/null || true; done
[ "$PANEL_CODE" = "200" ] || fail "dana's /api/panel during turtle's flood should be 200, got $PANEL_CODE after ${MS}ms (a worker thread was likely blocked on turtle's mutex): $(cat "$W/panel_out" 2>/dev/null)"
[ "$MS" -lt 2000 ] || fail "dana's /api/panel took ${MS}ms during turtle's flood of a slow ::SUBMIT — a worker thread was blocked on turtle's mutex"
echo "OK: concurrency — one viewer's flood of a slow ::SUBMIT never stalls a different viewer's /api/panel (${MS}ms)"
CREDS=("${AUTH_H[@]}")

echo "PASS: submit-live"
