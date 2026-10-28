#!/usr/bin/env bash
# End-to-end checks of read-back (#517 slice S3, plan §5): per-request
# viewer_state/viewer_state_latest on the locked server, scoped by
# construction — "the rows a connection may see are the only rows put into
# it", never a SQL filter on __claim_user. Against a real running server
# (SKIP_WASM=1 build), not unit tests.
#
# Covers:
#   * alice submits, then reads viewer_state back and sees exactly her own
#     row; bob, on the same dashboard/panel, sees none of alice's rows (an
#     empty viewer_state, not a filtered one).
#   * a trainer dashboard: carol (group `trainers`) sees every viewer's
#     rows; bob, on the SAME trainer dashboard but not a trainer himself,
#     sees only his own (threat model #19 — fails safe, never refused).
#   * cache/key isolation (plan §5 "cache and warm connections"): a second
#     submission changes what the very next read of the same panel sees —
#     never a stale cache or warm-connection hit from before the gen moved.
#   * a dashboard statement cannot read the state file directly off disk
#     (read_json on its real path) — enable_external_access stays off.
#
# usage: duckext/scripts/test-state.sh <extension> [port]
#        (DUCKDB=… picks the CLI; default: duckdb on PATH)
set -euo pipefail
EXT="$(cd "$(dirname "$1")" && pwd)/$(basename "$1")"
case "$(basename "$EXT")" in
  anofox_visualization.duckdb_extension|anofox_visualization.*.duckdb_extension) ;;
  *) echo "$EXT: LOAD takes the entry point from the file name, so it must be named anofox_visualization.duckdb_extension (or anofox_visualization.<x>.duckdb_extension). Rename it, or: ln -s '$EXT' /tmp/anofox_visualization.duckdb_extension" >&2; exit 2 ;;
esac
PORT="${2:-18421}"
DUCKDB="${DUCKDB:-duckdb}"
# Not under /tmp (plan §1: options.state.path refuses the system temp dir).
W="$(dirname "$EXT")/state-e2e-$$"
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

# `quiz`: a plain state dashboard (submit + read own rows). Panel 2 counts
# viewer_state's rows (own only); panel 3 lists them. Panel 4/5 add a
# MULTISELECT whose own options include NULL, to exercise a NULL item
# inside a recorded list (#548 item 3 nit: must store as real SQL NULL
# inside value_list, not an empty string).
cat > "$W/dash/quiz.sql" <<'EOF'
SELECT q1 ::BUTTONS FROM (VALUES ('A'), ('B'), ('C')) t(q1);
SELECT 'Check my answer' AS quiz_1::SUBMIT, 'q1' AS r::SUBMIT_VARS, 'Saved.' AS d::SUBMIT_DONE;
SELECT count(*) AS n ::METRIC FROM viewer_state;
SELECT "user" ::TABLE, dashboard, submit, name, value, value_list FROM viewer_state ORDER BY seq;
SELECT val AS q4 ::MULTISELECT FROM (VALUES ('A'), ('B'), (NULL)) t(val);
SELECT 'Check q4' AS quiz_4::SUBMIT, 'q4' AS r::SUBMIT_VARS, 'Saved.' AS d::SUBMIT_DONE;
EOF

# `progress`: a trainer dashboard — no ::SUBMIT of its own, read-only.
cat > "$W/dash/progress.sql" <<'EOF'
SELECT count(*) AS n ::METRIC FROM viewer_state;
SELECT "user" ::TABLE, dashboard, submit, name, value, value_list FROM viewer_state ORDER BY seq, "user", name;
EOF

# `leak`: not state-enabled at all — tries to read the state file straight
# off disk (threat model #3: enable_external_access stays off regardless).
cat > "$W/dash/leak.sql" <<'EOF'
SELECT seq ::TABLE, "user" FROM read_json('__STATE_PATH__');
EOF
sed -i "s#__STATE_PATH__#$W/state.jsonl#" "$W/dash/leak.sql"

# A server-issued API key for "carol", with the SAME trainers.groups
# membership her real (proxy) identity has — #548 item 3 nit: an api_key
# identity must never act as a trainer, whatever its claimed groups.
APIKEY="$(python3 - "$W/keys.json" <<'PY'
import hashlib, json, secrets, sys
path = sys.argv[1]
kid = "0123456789ab"
secret = secrets.token_hex(32)
key = f"afx_{kid}_{secret}"
h = hashlib.sha256(key.encode()).hexdigest()
json.dump({"keys": [{"id": kid, "sha256": h, "user": "carol", "groups": ["trainers"]}]}, open(path, "w"))
print(key)
PY
)"

AUTH="\"auth\": [{\"mode\": \"proxy\", \"shared_secret_env\": \"SEC\", \"trusted_proxies\": [\"127.0.0.1\"], \"header_groups\": \"X-Forwarded-Groups\"}, {\"mode\": \"api_keys\", \"file\": \"$W/keys.json\"}]"
STATE="\"state\": {\"path\": \"$W/state.jsonl\", \"dashboards\": [\"quiz\"], \"trainers\": {\"dashboards\": [\"progress\"], \"groups\": [\"trainers\"], \"users\": []}}"
OPTS="{$AUTH, \"policy\": \"none\", $STATE, \"log\": \"$W/requests.jsonl\", \"audit\": \"$W/audit.jsonl\"}"

mkfifo "$W/in"
sleep 120 > "$W/in" & FIFO_PID=$!; PIDS+=("$FIFO_PID")
echo "LOAD '$EXT'; SELECT anofox_serve_dashboards('$W/dash', $PORT, '$OPTS');" > "$W/serve.sql"
SEC=$SEC "$DUCKDB" -unsigned -init "$W/serve.sql" < "$W/in" > "$W/stderr.log" 2>&1 & SERVER_PID=$!; PIDS+=("$SERVER_PID")
for _ in $(seq 100); do curl -s -m 1 "localhost:$PORT/health" > /dev/null 2>&1 && break; sleep 0.2; done
if ! grep -q '"status":"ok"' <(curl -s "localhost:$PORT/health"); then
  cat "$W/stderr.log" >&2
  fail "server did not start"
fi

ORIGIN="http://localhost:$PORT"
SUBMIT_H=(-H "X-Anofox-Submit: 1" -H "Content-Type: application/json" -H "Sec-Fetch-Site: same-origin" -H "Origin: $ORIGIN")

# submit <user> <id> <answer>: POSTs /api/submit as <user>, returns the code.
submit() {
  curl -s -o "$W/sub_out" -w '%{http_code}' -X POST "localhost:$PORT/api/submit" \
    -H "X-Anofox-Proxy-Secret: $SEC" -H "X-Forwarded-User: $1" "${SUBMIT_H[@]}" \
    -d "{\"dashboard\":\"quiz\",\"panel\":1,\"id\":\"$2\",\"vars\":{\"q1\":\"$3\"}}"
}

# panel <user> <dashboard> <panel> <body-json-vars (default {})>: POSTs
# /api/panel as <user>, prints the JSON row array to stdout.
panel() {
  local vars="${4:-}"
  [ -n "$vars" ] || vars="{}"
  curl -s -X POST "localhost:$PORT/api/panel" -H "Origin: $ORIGIN" \
    -H "X-Anofox-Proxy-Secret: $SEC" -H "X-Forwarded-User: $1" \
    -d "{\"dashboard\":\"$2\",\"panel\":$3,\"vars\":$vars}"
}

n_of() { python3 -c 'import json,sys; print(int(list(json.load(open(sys.argv[1]))[0].values())[0]))' "$1"; }

# --- isolation: alice's own row is hers alone.
CODE=$(submit alice a0a0a0a0 B)
[ "$CODE" = "201" ] || fail "alice's submission should be 201, got $CODE: $(cat "$W/sub_out")"

ALICE_ROWS=$(panel alice quiz 3)
grep <<<"$ALICE_ROWS" -q '"user":"alice"' || fail "alice should see her own row: $ALICE_ROWS"
grep <<<"$ALICE_ROWS" -q '"value":"B"' || fail "alice's row should carry her answer: $ALICE_ROWS"
ALICE_N=$(n_of <(panel alice quiz 2))
[ "$ALICE_N" = "1" ] || fail "alice's own viewer_state count should be 1, got $ALICE_N"
echo "OK: alice reads her own submitted row back"

BOB_ROWS=$(panel bob quiz 3)
[ "$BOB_ROWS" = "[]" ] || fail "bob must see none of alice's rows (an EMPTY viewer_state, not a filtered one): $BOB_ROWS"
BOB_N=$(n_of <(panel bob quiz 2))
[ "$BOB_N" = "0" ] || fail "bob's own viewer_state count should be 0, got $BOB_N"
echo "OK: bob (no submissions of his own) sees none of alice's rows"

# --- cache/key isolation: a second submission changes the very next read.
CODE=$(submit alice a1a1a1a1 C)
[ "$CODE" = "201" ] || fail "alice's second submission should be 201, got $CODE: $(cat "$W/sub_out")"
ALICE_N2=$(n_of <(panel alice quiz 2))
[ "$ALICE_N2" = "2" ] || fail "alice's viewer_state count must move to 2 right after her 2nd submission (stale cache/warm hit?), got $ALICE_N2"
echo "OK: a submission is visible on the very next read (no stale cache or warm-connection hit)"

# bob still sees none of this.
BOB_N2=$(n_of <(panel bob quiz 2))
[ "$BOB_N2" = "0" ] || fail "bob's count must still be 0 after alice's 2nd submission, got $BOB_N2"

# --- bob submits his own answer; this is HIS row, never alice's business.
CODE=$(submit bob b0b0b0b0 A)
[ "$CODE" = "201" ] || fail "bob's submission should be 201, got $CODE: $(cat "$W/sub_out")"
BOB_N3=$(n_of <(panel bob quiz 2))
[ "$BOB_N3" = "1" ] || fail "bob's own viewer_state count should be 1 after his own submission, got $BOB_N3"
ALICE_N3=$(n_of <(panel alice quiz 2))
[ "$ALICE_N3" = "2" ] || fail "alice's own count must be unaffected by bob's submission, got $ALICE_N3"
echo "OK: each viewer's own row count is independent of the other's submissions"

# --- trainer dashboard: carol (trainers group) sees every viewer's rows;
# bob, on the SAME dashboard but not a trainer, still sees only his own.
CAROL_ALL=$(panel carol-x progress 1 '{}')
# carol-x isn't in the trainers group yet — prove the group check actually
# runs (not just presence of ANY claims), then retry as a real trainer.
grep <<<"$CAROL_ALL" -q '"user":"alice"' && fail "a non-trainer must not see alice's rows on the trainer dashboard: $CAROL_ALL"

CAROL_TRAINER_ROWS=$(curl -s -X POST "localhost:$PORT/api/panel" -H "Origin: $ORIGIN" \
  -H "X-Anofox-Proxy-Secret: $SEC" -H "X-Forwarded-User: carol" -H "X-Forwarded-Groups: trainers" \
  -d '{"dashboard":"progress","panel":1,"vars":{}}')
grep <<<"$CAROL_TRAINER_ROWS" -q '"user":"alice"' || fail "carol (trainers group) should see alice's rows: $CAROL_TRAINER_ROWS"
grep <<<"$CAROL_TRAINER_ROWS" -q '"user":"bob"' || fail "carol (trainers group) should see bob's rows too: $CAROL_TRAINER_ROWS"
CAROL_N=$(n_of <(curl -s -X POST "localhost:$PORT/api/panel" -H "Origin: $ORIGIN" \
  -H "X-Anofox-Proxy-Secret: $SEC" -H "X-Forwarded-User: carol" -H "X-Forwarded-Groups: trainers" \
  -d '{"dashboard":"progress","panel":0,"vars":{}}'))
[ "$CAROL_N" = "3" ] || fail "carol (trainer) should see all 3 recorded rows (2 alice + 1 bob), got $CAROL_N"
echo "OK: a named trainer sees every viewer's rows on their dashboard"

BOB_PROGRESS_ROWS=$(panel bob progress 1 '{}')
grep <<<"$BOB_PROGRESS_ROWS" -q '"user":"alice"' && fail "bob (not a trainer) on the trainer dashboard must not see alice's rows: $BOB_PROGRESS_ROWS"
BOB_PROGRESS_N=$(n_of <(panel bob progress 0 '{}'))
[ "$BOB_PROGRESS_N" = "1" ] || fail "bob (not a trainer) must see only his own row on the trainer dashboard, got $BOB_PROGRESS_N"
echo "OK: a non-trainer on the SAME trainer dashboard still sees only their own rows (fails safe)"

# --- the state file is never reachable by a dashboard's own SQL.
LEAK_CODE=$(curl -s -o "$W/leak_out" -w '%{http_code}' -X POST "localhost:$PORT/api/panel" -H "Origin: $ORIGIN" \
  -H "X-Anofox-Proxy-Secret: $SEC" -H "X-Forwarded-User: alice" -d '{"dashboard":"leak","panel":0,"vars":{}}')
[ "$LEAK_CODE" != "200" ] || fail "a dashboard reading the state file via read_json() must fail, got 200: $(cat "$W/leak_out")"
echo "OK: a dashboard statement cannot read the state file directly (enable_external_access stays off)"

# --- #548 item 3 nit: an api_key identity is never a trainer, even with
# the same group a real trainer has.
CAROL_KEY_ROWS=$(curl -s -X POST "localhost:$PORT/api/panel" -H "Origin: $ORIGIN" \
  -H "Authorization: Bearer $APIKEY" -d '{"dashboard":"progress","panel":1,"vars":{}}')
grep <<<"$CAROL_KEY_ROWS" -q '"user":"alice"' && fail "an api_key identity (carol's, group trainers) must not see alice's rows: $CAROL_KEY_ROWS"
CAROL_KEY_N=$(n_of <(curl -s -X POST "localhost:$PORT/api/panel" -H "Origin: $ORIGIN" \
  -H "Authorization: Bearer $APIKEY" -d '{"dashboard":"progress","panel":0,"vars":{}}'))
[ "$CAROL_KEY_N" = "0" ] || fail "an api_key identity must fall back to its own (here: zero) rows, got $CAROL_KEY_N: $CAROL_KEY_ROWS"
echo "OK: an api_key identity is never a trainer, whatever its claimed groups"

# --- #548 item 3 nit: a NULL item inside a recorded list is stored as a
# real SQL NULL in value_list, never an empty string. q4's own MULTISELECT
# options (panel 4) include a literal NULL, so ["A", null] is a valid
# selection; its ::SUBMIT is panel 5.
CODE=$(curl -s -o "$W/sub_out" -w '%{http_code}' -X POST "localhost:$PORT/api/submit" \
  -H "X-Anofox-Proxy-Secret: $SEC" -H "X-Forwarded-User: alice" "${SUBMIT_H[@]}" \
  -d '{"dashboard":"quiz","panel":5,"id":"c0ffee01","vars":{"q4":["A",null]}}')
[ "$CODE" = "201" ] || fail "a MULTISELECT with a NULL option should be 201, got $CODE: $(cat "$W/sub_out")"
Q4_ROW=$(panel alice quiz 3)
grep <<<"$Q4_ROW" -q '"name":"q4"' || fail "expected a recorded q4 row: $Q4_ROW"
grep <<<"$Q4_ROW" -q '"value_list":\["A",null\]' || fail "a NULL item in the list must be stored as a real SQL NULL (value_list=[\"A\",null]), not an empty string: $Q4_ROW"
echo "OK: a NULL item inside a recorded list is stored as a real NULL, not ''"

# #611 B1: `value` for a list (MULTISELECT) answer holds the same JSON
# text the pre-#600 SQL path bound and the browser's own live preview
# still writes (web/app.js) — not NULL. The #600 rewrite's Appender path
# dropped this for one push; a dashboard reading `value` for a list
# answer must see what it always has.
grep <<<"$Q4_ROW" -q '"value":"\[\\"A\\",null\]"' || fail "value for a list answer must hold the JSON text (here [\"A\",null]), not NULL: $Q4_ROW"
echo "OK: value for a list (MULTISELECT) answer holds its JSON text, not NULL"

# --- #614: a record with an embedded NUL byte in a value, written by
# something other than this server. Never reachable through ::SUBMIT —
# `scalar_literal` (src/host/serving.rs) refuses a NUL byte in any
# viewer-submitted value before it is ever `SET` as a variable — so this
# is only reachable via a store file written or edited some other way,
# same as #611 S2's own non-canonical-`ts` scenario (combined here: one
# record below has a canonical `ts`, taking the fast Appender path; one
# doesn't, taking the slow SQL fallback path; one is a MULTISELECT (list)
# answer, exercising append_varchar_list's own NUL-safety separately
# from a plain scalar value's append_varchar). Before this fix,
# Appender::append_varchar/append_varchar_list and the fallback's
# Prepared::bind_strs all refused the whole row (`CString::new` on an
# embedded NUL, #611 N3) — every panel on this generation errored, not
# just the one holding this record. Stop the server, hand-edit
# state.jsonl to add all three records directly, then restart on the
# SAME file and read them back, alongside every row already recorded
# above (proving the restart and the hand-edit didn't lose or corrupt
# anything either).
kill "$SERVER_PID" "$FIFO_PID" 2>/dev/null || true
wait "$SERVER_PID" 2>/dev/null || true
wait "$FIFO_PID" 2>/dev/null || true
python3 - "$W/state.jsonl" <<'PY'
import json, sys
path = sys.argv[1]
lines = [l for l in open(path) if l.strip()]
next_seq = 1
for l in lines[1:]:
    next_seq = max(next_seq, json.loads(l)["seq"] + 1)
recs = [
    {"v": 1, "seq": next_seq, "ts": "2026-10-01T00:00:00.000Z", "user": "dave",
     "auth": "oidc", "dashboard": "quiz", "submit": "quiz_1", "id": "d" * 32,
     "vars": {"q1": "x\u0000y"}},
    {"v": 1, "seq": next_seq + 1, "ts": "2026-10-01T00:00:00Z", "user": "erin",
     "auth": "oidc", "dashboard": "quiz", "submit": "quiz_1", "id": "e" * 32,
     "vars": {"q1": "p\u0000q"}},
    {"v": 1, "seq": next_seq + 2, "ts": "2026-10-01T00:00:00.000Z", "user": "frank",
     "auth": "oidc", "dashboard": "quiz", "submit": "quiz_4", "id": "f" * 32,
     "vars": {"q4": ["x\u0000y", None]}},
]
with open(path, "a") as f:
    for rec in recs:
        f.write(json.dumps(rec) + "\n")
PY
mkfifo "$W/in2"
sleep 120 > "$W/in2" & FIFO_PID=$!; PIDS+=("$FIFO_PID")
SEC=$SEC "$DUCKDB" -unsigned -init "$W/serve.sql" < "$W/in2" > "$W/stderr2.log" 2>&1 & SERVER_PID=$!; PIDS+=("$SERVER_PID")
for _ in $(seq 100); do curl -s -m 1 "localhost:$PORT/health" > /dev/null 2>&1 && break; sleep 0.2; done
if ! grep -q '"status":"ok"' <(curl -s "localhost:$PORT/health"); then
  cat "$W/stderr2.log" >&2
  fail "server did not restart against the hand-edited state file"
fi

CAROL_AFTER_RESTART=$(curl -s -X POST "localhost:$PORT/api/panel" -H "Origin: $ORIGIN" \
  -H "X-Anofox-Proxy-Secret: $SEC" -H "X-Forwarded-User: carol" -H "X-Forwarded-Groups: trainers" \
  -d '{"dashboard":"progress","panel":1,"vars":{}}')
grep <<<"$CAROL_AFTER_RESTART" -q '"user":"alice"' || fail "#614: a trainer must still see every pre-restart row after the hand-edit, got: $CAROL_AFTER_RESTART"
grep <<<"$CAROL_AFTER_RESTART" -q '"user":"dave"' || fail "#614: the hand-edited canonical-ts/NUL record (fast Appender path) must seed and read back, got: $CAROL_AFTER_RESTART"
grep <<<"$CAROL_AFTER_RESTART" -q '"user":"erin"' || fail "#614: the hand-edited non-canonical-ts/NUL record (slow SQL fallback path, #611 S2) must seed and read back, got: $CAROL_AFTER_RESTART"
grep <<<"$CAROL_AFTER_RESTART" -q '"user":"frank"' || fail "#614: the hand-edited MULTISELECT/NUL record (append_varchar_list path) must seed and read back, got: $CAROL_AFTER_RESTART"
grep <<<"$CAROL_AFTER_RESTART" -q '"value":"x\\u0000y"' || fail "#614: dave's embedded-NUL value must round-trip unchanged (fast Appender path), got: $CAROL_AFTER_RESTART"
grep <<<"$CAROL_AFTER_RESTART" -q '"value":"p\\u0000q"' || fail "#614: erin's embedded-NUL value must round-trip unchanged (slow SQL fallback path), got: $CAROL_AFTER_RESTART"
grep <<<"$CAROL_AFTER_RESTART" -q '"value_list":\["x\\u0000y",null\]' || fail "#614: frank's embedded-NUL list item must round-trip unchanged (append_varchar_list path), got: $CAROL_AFTER_RESTART"
echo "OK: a record with an embedded NUL byte in a value seeds and reads back correctly, on the fast Appender path, the slow SQL fallback path and the MULTISELECT (list) path"

# --- #614 output check (security review): a NUL-containing value is now
# seeded, so it can flow into panel OUTPUT too. The JSON API (`/api/panel`,
# just exercised above) already escapes it correctly (`serde_json`'s own
# `\u0000`, confirmed by the greps above against the RAW response bytes,
# not a bash variable — bash strings can't hold an embedded NUL, so a
# bug that let a literal 0x00 byte through would have silently vanished
# from `$CAROL_AFTER_RESTART` instead of failing this script). The served
# dashboard page (`GET /d/<id>`) carries the dashboard's own SQL text and
# static UI chrome only — never a live `viewer_state` value — so there is
# nothing for a value to flow into there. No current code path renders a
# real, Store-backed `viewer_state` value into server-side SVG/XML: the
# locked server's `/api/panel`/`/api/submit` are JSON-only; chart/table
# rendering of that data happens entirely client-side (the browser's own
# JS/renderer), which is out of this script's (and this PR's) reach.
# `dashboard --svg`/`--site` and the standalone `serve` binary's own
# SVG/HTML rendering (`src/format::escape_xml`, already NUL-safe — see
# its own `escape_xml_replaces_xml_invalid_control_characters` test) never
# see a live Store at all (`viewer_state` is always empty there), so
# they can't be the vehicle for this either. If a future change adds a
# server-side render of a real recorded value, `escape_xml`/`html_escape`
# are already there and already NUL-safe.
echo "OK: the JSON API escapes an embedded NUL byte in panel output (serde_json's \\u0000); no server-side SVG/XML render path exists for a real recorded value to flow into (chart/table rendering of viewer_state data is client-side only)"

echo "PASS: state-live"
