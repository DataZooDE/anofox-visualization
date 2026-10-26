#!/usr/bin/env bash
# #600 benchmark: a trainer dashboard page's 12 panels, fetched 6 at a time
# (two waves — the shape a real page load produces, review of #599 SF1),
# against a 200k-row options.state store. Not a correctness test, not wired
# into CI (it's slow by design — generating and loading 200k rows — and a
# timing assertion would just be flaky across CI runners); run by hand when
# touching seed_viewer_state/Store::snapshot_all and report the numbers.
#
# usage: duckext/scripts/bench-trainer-readback.sh <extension> [port] [rows]
#        (DUCKDB=… picks the CLI; default: duckdb on PATH)
set -euo pipefail
EXT="$(cd "$(dirname "$1")" && pwd)/$(basename "$1")"
case "$(basename "$EXT")" in
  anofox_visualization.duckdb_extension|anofox_visualization.*.duckdb_extension) ;;
  *) echo "$EXT: LOAD takes the entry point from the file name, so it must be named anofox_visualization.duckdb_extension (or anofox_visualization.<x>.duckdb_extension). Rename it, or: ln -s '$EXT' /tmp/anofox_visualization.duckdb_extension" >&2; exit 2 ;;
esac
PORT="${2:-18970}"
ROWS="${3:-200000}"
DUCKDB="${DUCKDB:-duckdb}"
HERE="$(cd "$(dirname "$0")" && pwd)"
W="$(dirname "$EXT")/bench-$$"
mkdir -p "$W/dash"
PIDS=()
cleanup() { for p in "${PIDS[@]}"; do kill "$p" 2>/dev/null || true; done; rm -rf "$W"; }
trap cleanup EXIT
fail() { echo "FAIL: $*" >&2; exit 1; }

# A trainer dashboard with 12 panels all reading viewer_state — the shape
# that triggers 12 independent seed_viewer_state calls (one per panel's own
# cold connection) on a page load.
for i in $(seq 1 12); do
  echo "SELECT \"user\" ::TABLE, dashboard, submit, name, value FROM viewer_state ORDER BY seq LIMIT 5;"
done > "$W/dash/progress.sql"

echo "generating a $ROWS-row state file..."
python3 "$HERE/gen-bench-state.py" "$W/state.jsonl" "$ROWS"
chmod 600 "$W/state.jsonl"

STATE="\"state\": {\"path\": \"$W/state.jsonl\", \"dashboards\": [], \"trainers\": {\"dashboards\": [\"progress\"], \"groups\": [\"trainers\"], \"users\": []}, \"trainer_max_rows\": $ROWS}"
SEC="benchsecret0123456789abcdef0123456789ab"
AUTH="\"auth\": [{\"mode\": \"proxy\", \"shared_secret_env\": \"SEC\", \"trusted_proxies\": [\"127.0.0.1\"], \"header_groups\": \"X-Forwarded-Groups\"}]"
OPTS="{$AUTH, \"policy\": \"none\", $STATE}"

mkfifo "$W/in"
sleep 300 > "$W/in" & PIDS+=($!)
echo "LOAD '$EXT'; SELECT anofox_serve_dashboards('$W/dash', $PORT, '$OPTS');" > "$W/serve.sql"
SEC=$SEC "$DUCKDB" -unsigned -init "$W/serve.sql" < "$W/in" > "$W/stderr.log" 2>&1 & PIDS+=($!)
for _ in $(seq 150); do curl -s -m 1 "localhost:$PORT/health" > /dev/null 2>&1 && break; sleep 0.2; done
if ! grep -q '"status":"ok"' <(curl -s "localhost:$PORT/health"); then
  cat "$W/stderr.log" >&2
  fail "server did not start"
fi
echo "server up on $PORT"

panel() {
  curl -s -o /dev/null -X POST "localhost:$PORT/api/panel" -H "Origin: http://localhost:$PORT" \
    -H "X-Anofox-Proxy-Secret: $SEC" -H "X-Forwarded-User: carol" -H "X-Forwarded-Groups: trainers" \
    -d "{\"dashboard\":\"progress\",\"panel\":$1,\"vars\":{}}"
}

T0=$(date +%s.%N)
echo "=== wave 1: panels 0-5 concurrently ==="
WAVE=()
for i in 0 1 2 3 4 5; do panel "$i" & WAVE+=($!); done
wait "${WAVE[@]}"
echo "=== wave 2: panels 6-11 concurrently ==="
WAVE=()
for i in 6 7 8 9 10 11; do panel "$i" & WAVE+=($!); done
wait "${WAVE[@]}"
T1=$(date +%s.%N)
echo "total wall-clock for 12 panels (2 waves of 6): $(echo "$T1 - $T0" | bc) s"

echo "=== server-side per-panel timings ==="
grep -E "ran progress#|400:" "$W/stderr.log" || echo "(no timing lines found)"
