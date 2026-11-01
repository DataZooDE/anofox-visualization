#!/usr/bin/env bash
# End-to-end checks of options.durability (#775) against a real server, with
# a local directory as the snapshot location (the same COPY path an s3://
# location takes through httpfs):
#   * a submission reaches the snapshot within snapshot_every_s, byte for byte;
#   * a new node (no local state file) starts from the snapshot, and its
#     viewer reads the restored row back; a local file always wins;
#   * a replica without durability.run answers 503 to /api/submit;
#   * the last rows before a stop reach the snapshot (the final snapshot);
#   * durability without state/writeback refuses the start.
#
# usage: duckext/scripts/test-durability.sh <extension> [port]
set -euo pipefail
EXT="$(cd "$(dirname "$1")" && pwd)/$(basename "$1")"
case "$(basename "$EXT")" in
  anofox_visualization.duckdb_extension|anofox_visualization.*.duckdb_extension) ;;
  *) echo "$EXT: must be named anofox_visualization.duckdb_extension" >&2; exit 2 ;;
esac
PORT="${2:-18431}"
DUCKDB="${DUCKDB:-duckdb}"
# Not under /tmp (options.state.path refuses the system temp dir).
W="$(dirname "$EXT")/durability-e2e-$$"
mkdir -p "$W/dash" "$W/snap"
PIDS=()
cleanup() {
  for p in "${PIDS[@]}"; do kill "$p" 2>/dev/null || true; done
  rm -rf "$W"
}
trap cleanup EXIT
fail() { echo "FAIL: $*" >&2; exit 1; }
SEC="$(python3 -c 'import secrets; print(secrets.token_hex(24))')"

cat > "$W/dash/quiz.sql" <<'EOF'
SELECT q1 ::BUTTONS FROM (VALUES ('A'), ('B'), ('C')) t(q1);
SELECT 'Check my answer' AS quiz_1::SUBMIT, 'q1' AS r::SUBMIT_VARS, 'Saved.' AS d::SUBMIT_DONE;
SELECT count(*) AS n ::METRIC FROM viewer_state;
EOF

# start <port> <durability JSON or ""> [state path]: a server on its own FIFO.
start() {
  local port="$1" dur="$2" path="${3:-$W/state.jsonl}"
  local opts="{\"auth\": {\"mode\": \"proxy\", \"shared_secret_env\": \"SEC\", \"trusted_proxies\": [\"127.0.0.1\"]}, \"policy\": \"none\", \"state\": {\"path\": \"$path\", \"dashboards\": [\"quiz\"]}${dur:+, \"durability\": $dur}}"
  rm -f "$W/in$port"; mkfifo "$W/in$port"
  SEC=$SEC "$DUCKDB" -unsigned -batch :memory: < "$W/in$port" > "$W/out$port" 2>&1 &
  PIDS+=($!)
  exec {fd}>"$W/in$port"
  eval "FD$port=$fd"
  printf "LOAD '%s';\nSELECT anofox_serve_dashboards('%s', %s, '%s');\n" "$EXT" "$W/dash" "$port" "$opts" >&"$fd"
  for _ in $(seq 100); do
    curl -sf "localhost:$port/health" >/dev/null 2>&1 && return 0
    grep -qE '^([A-Za-z][A-Za-z ]* )?Error:' "$W/out$port" 2>/dev/null && break
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
submit() { # submit <port> <user> <id> <answer> → status
  curl -s -o "$W/sub_out" -w '%{http_code}' -X POST "localhost:$1/api/submit" \
    -H "X-Anofox-Proxy-Secret: $SEC" -H "X-Forwarded-User: $2" \
    -H "X-Anofox-Submit: 1" -H "Content-Type: application/json" -H "Sec-Fetch-Site: same-origin" -H "Origin: http://localhost:$1" \
    -d "{\"dashboard\":\"quiz\",\"panel\":1,\"id\":\"$3\",\"vars\":{\"q1\":\"$4\"}}"
}
count() { # count <port> <user> → alice's viewer_state rows
  curl -s -X POST "localhost:$1/api/panel" -H "Origin: http://localhost:$1" \
    -H "X-Anofox-Proxy-Secret: $SEC" -H "X-Forwarded-User: $2" \
    -d '{"dashboard":"quiz","panel":2,"vars":{}}' | python3 -c 'import json,sys; print(int(list(json.load(sys.stdin)[0].values())[0]))'
}
DUR="{\"run\": true, \"location\": \"$W/snap\", \"snapshot_every_s\": 1}"

# 1. A submission reaches the snapshot, byte for byte.
start "$PORT" "$DUR" || { cat "$W/out$PORT" >&2; fail "the writer should start"; }
[ "$(submit "$PORT" alice a0a0a0a0 B)" = 201 ] || fail "alice's submission: $(cat "$W/sub_out")"
for _ in $(seq 50); do cmp -s "$W/state.jsonl" "$W/snap/state.jsonl" && break; sleep 0.2; done
cmp "$W/state.jsonl" "$W/snap/state.jsonl" || fail "the snapshot should equal the state file"
grep -q '"writer":true' <(curl -s "localhost:$PORT/health" -H "X-Anofox-Proxy-Secret: $SEC" -H "X-Forwarded-User: ops") || fail "/healthz: durability"
# the last row before a stop reaches the snapshot
[ "$(submit "$PORT" alice b1b1b1b1 C)" = 201 ] || fail "alice's second submission: $(cat "$W/sub_out")"
stop "$PORT"
cmp "$W/state.jsonl" "$W/snap/state.jsonl" || fail "the final snapshot at the stop should hold the last row"
echo "OK: submissions reach the snapshot byte for byte, the last one at the stop"

# 2. A new node starts from the snapshot.
mv "$W/state.jsonl" "$W/state.old"
start "$PORT" "$DUR" || { cat "$W/out$PORT" >&2; fail "the new node should start"; }
cmp "$W/state.old" "$W/state.jsonl" || fail "the restored file should equal the snapshot"
[ "$(stat -c %a "$W/state.jsonl")" = 600 ] || fail "the restored file should be 0600"
[ "$(count "$PORT" alice)" = 2 ] || fail "alice should read her 2 restored rows back"
stop "$PORT"
echo "OK: a new node starts from the snapshot (0600), its viewer reads the rows back"

# 3. A local file wins over the snapshot.
printf '' > "$W/snap/state.jsonl"
start "$PORT" "$DUR" || fail "start with a local file"
[ "$(count "$PORT" alice)" = 2 ] || fail "the local file (2 rows) should win over an empty snapshot"
stop "$PORT"
echo "OK: a local file wins over the snapshot"

# 4. A replica that isn't the writer answers 503 to a write.
start "$((PORT + 1))" "{\"location\": \"$W/snap\"}" "$W/state-reader.jsonl" || { cat "$W/out$((PORT + 1))" >&2; fail "the reader should start"; }
[ "$(submit "$((PORT + 1))" bob c2c2c2c2 A)" = 503 ] || fail "a reader should answer 503 to a submission: $(cat "$W/sub_out")"
stop "$((PORT + 1))"
echo "OK: a replica without durability.run answers 503 to writes"

# 5. Durability without state/writeback refuses the start.
rm -f "$W/inX"; mkfifo "$W/inX"
"$DUCKDB" -unsigned -batch :memory: < "$W/inX" > "$W/outX" 2>&1 & PIDS+=($!)
exec {fx}>"$W/inX"
printf "LOAD '%s';\nSELECT anofox_serve_dashboards('%s', %s, '{\"durability\": {\"run\": true, \"location\": \"%s\"}}');\n" "$EXT" "$W/dash" "$((PORT + 2))" "$W/snap" >&"$fx"
exec {fx}>&-
for _ in $(seq 50); do grep -q 'needs options.state or options.writeback' "$W/outX" && break; sleep 0.2; done
grep -q 'needs options.state or options.writeback' "$W/outX" || fail "durability alone should refuse the start: $(cat "$W/outX")"
echo "OK: durability without state/writeback refuses the start"
echo "PASS: durability"
