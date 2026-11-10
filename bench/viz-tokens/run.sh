#!/bin/bash
# #1074: one command for the AI evaluation — build, run, judge, report, compare, history.
#
#   bench/viz-tokens/run.sh [--set smoke|full] [--reps N] [--model M] [--arms A,B] [--baseline DIR] [--dry]
#
# --arms: anofox-lean (default, the short reference) or anofox (what the builder's assistant sends:
# the role reference + web/assist/guide.md), or both; --model claude-haiku-4-5 for a weaker model (#1078).
#
# smoke = sets.json's ~18 questions × 1 repeat (≈ $0.65); full = the 50-question coverage set × 3
# (≈ $10 with judging). Results land in $BENCH_RESULTS (default target/bench)/<set>-<time>/:
# report.md, gaps.md, compare.md (vs --baseline, else the previous run of the same set), and a row
# per arm in $BENCH_RESULTS/history.csv. Exit status 2 when compare.py finds a regression.
#
# Needs ANTHROPIC_API_KEY (not with --dry), a python with requirements.txt ($BENCH_PYTHON), node,
# wasm-pack, bubblewrap and the duckdb CLI. $BENCH_DASHBOARD skips the CLI build, $BENCH_DATA the
# data build. Accuracy first (#1044): compare.md leads with the pass rate.
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/../.." && pwd)
SET=smoke REPS="" MODEL=claude-sonnet-5 ARMS=anofox-lean BASE="" DRY=""
while [ $# -gt 0 ]; do
  case "$1" in
    --set) SET=$2; shift 2 ;;
    --reps) REPS=$2; shift 2 ;;
    --model) MODEL=$2; shift 2 ;;
    --arms) ARMS=$2; shift 2 ;;
    --baseline) BASE=$2; shift 2 ;;
    --dry) DRY=1; shift ;;
    *) echo "unknown argument: $1" >&2; exit 64 ;;
  esac
done
PY=${BENCH_PYTHON:-python3}
RESULTS=${BENCH_RESULTS:-$ROOT/target/bench}
DATA=${BENCH_DATA:-$RESULTS/data}
export DATAZOO_DISABLE_TELEMETRY=1
[ -n "$DRY" ] || [ -n "${ANTHROPIC_API_KEY:-}" ] || [ -n "${ANTHROPIC_FEDERATION_RULE_ID:-}" ] || { echo "set ANTHROPIC_API_KEY, or the ANTHROPIC_FEDERATION_* variables (bench/viz-tokens/auth.py), or pass --dry" >&2; exit 64; }
case "$SET" in
  smoke) TASKS=$("$PY" -c "import json,sys; print(','.join(json.load(open(sys.argv[1]))['smoke']))" "$HERE/sets.json"); REPS=${REPS:-1} ;;
  full) TASKS=coverage; REPS=${REPS:-3} ;;
  *) echo "--set smoke|full" >&2; exit 64 ;;
esac
mkdir -p "$RESULTS"
ENGINE=$(git -C "$ROOT" rev-parse --short HEAD)
DASH=${BENCH_DASHBOARD:-}
if [ -z "$DASH" ]; then
  (cd "$ROOT" && cargo build -q --release --locked --features serve --bin dashboard)
  DASH=$ROOT/target/release/dashboard
fi
[ -f "$ROOT/web/pkg/anofox_visualization_bg.wasm" ] \
  || (cd "$ROOT" && wasm-pack build --target web --out-dir web/pkg --profile wasm --no-default-features --features wasm)
[ -d "$DATA" ] || "$PY" "$HERE/make-data.py" "$DATA"
OUT=$RESULTS/$SET-$MODEL-$(date +%Y%m%d-%H%M%S)
# the previous run of this set with this model, before this one exists
[ -n "$BASE" ] || BASE=$(find "$RESULTS" -maxdepth 1 -type d -name "$SET-$MODEL-*" | sort | tail -1)
echo "engine $ENGINE · set $SET · $REPS repeat(s) · model $MODEL · arms $ARMS · baseline ${BASE:-none}"
"$PY" "$HERE/harness.py" --tasks "$TASKS" --arms "$ARMS" --reps "$REPS" --model "$MODEL" --yes \
  --data "$DATA" --dashboard "$DASH" --pkg "$ROOT/web/pkg" --boundaries "$ROOT/web/data/boundaries" \
  --out "$OUT" ${DRY:+--dry} 2>&1 | tee "$OUT.log"
[ -n "$DRY" ] || "$PY" "$HERE/judge.py" --data "$DATA" "$OUT" 2>&1 | tee -a "$OUT.log"
"$PY" "$HERE/report.py" "$OUT"
"$PY" "$HERE/gaps.py" "$OUT" > "$OUT/gaps.md"
"$PY" "$HERE/compare.py" "$OUT" ${BASE:+--baseline "$BASE"} --history "$RESULTS/history.csv" \
  --set "$SET" --engine "$ENGINE" > /dev/null || status=$?
echo "report: $OUT/report.md · gaps: $OUT/gaps.md · compare: $OUT/compare.md"
exit "${status:-0}"
