#!/usr/bin/env bash
# #1341: the stored results of "Which items need one model, which need many?"
# (web/examples/ml-global-forecast.sql): anofox_ml fits natively, the page
# reads web/data/ml-global/*.csv. Needs a DuckDB CLI with anofox_ml
# (INSTALL anofox_ml FROM community).
#   scripts/gen-ml-global-case.sh [duckdb]
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DUCKDB="${1:-duckdb}"
export DATAZOO_DISABLE_TELEMETRY=1
mkdir -p "$ROOT/web/data/ml-global"
cd "$ROOT/web"   # the SQL reads m5_monthly.parquet and writes data/ml-global/ relative to web/
"$DUCKDB" :memory: -c ".read $ROOT/scripts/ml-global-case.sql"
ls -la "$ROOT/web/data/ml-global"
