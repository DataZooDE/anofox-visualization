#!/usr/bin/env bash
# #1341: the stored results of "Does one model beat the M5 benchmarks on every
# Walmart item?" (web/examples/m5-daily-forecast.sql). The whole M5 setting,
# natively: anofox_forecast (series features, baselines), anofox_ml (one
# global PerpetualRegressor), anofox_statistics (tests). Writes web/data/m5/.
#
#   scripts/gen-m5-case.sh M5_DIR [duckdb] [work.duckdb]
#
# M5_DIR holds the competition's CSVs: calendar.csv, sell_prices.csv,
# sales_train_evaluation.csv and sales_test_evaluation.csv (the labels of the
# test days). Needs DuckDB 1.5.6 with anofox_ml, anofox_forecast and
# anofox_statistics (INSTALL … FROM community). About 2½ hours on 16 cores,
# ~25 GB of memory and ~40 GB of disk for the work database.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
M5_DIR="$(cd "${1:?usage: gen-m5-case.sh M5_DIR [duckdb] [work.duckdb]}" && pwd)"
DUCKDB="${2:-duckdb}"
WORK="${3:-$ROOT/m5-work.duckdb}"
export DATAZOO_DISABLE_TELEMETRY=1
SQL="$ROOT/scripts/m5-case"
# EXT_DIR: an optional extension_directory (else DuckDB's default)
PRE="SET VARIABLE m5_dir = '$M5_DIR';${EXT_DIR:+ SET extension_directory = '$EXT_DIR';}"
run() { (cd "$SQL" && "$DUCKDB" -bail "$WORK" -c "$PRE" "$@"); }

run -c ".read load.sql"
run -c ".read eval.sql"
run -c ".read base.sql"
run -c ".read base_cal.sql"
run -c ".read features.sql"
run -c "CREATE OR REPLACE TABLE tree_fc (cutoff INT, seed INT, id VARCHAR, t INT, h INT, f DOUBLE);
        CREATE OR REPLACE TABLE models (cutoff INT, seed INT, m BLOB);"
for c in 1913 1941; do
  for s in 7 8 9 10 11; do run -c "SET VARIABLE cutoff = $c; SET VARIABLE seed = $s;" -c ".read model.sql"; done
done
run -c ".read pf.sql"
run -c ".read assess.sql"
run -c ".read importance.sql"
mkdir -p "$ROOT/web/data/m5"
(cd "$ROOT/web" && "$DUCKDB" -bail "$WORK" -c "$PRE" \
  -c "LOAD anofox_ml; LOAD anofox_forecast; LOAD anofox_statistics; SET VARIABLE official = '$SQL/m5-official.csv';" \
  -c ".read $SQL/export.sql")
ls -la "$ROOT/web/data/m5"
