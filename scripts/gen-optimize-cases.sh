#!/usr/bin/env bash
# Regenerate the stored results of the anofox-optimize use-case dashboards
# (web/examples/optimize-<case>.sql). anofox_optimize has no loadable
# DuckDB-Wasm build yet, so each examples/optimize-<case>/compute.sql runs
# natively and its result tables are written to web/data/optimize-<case>/*.csv,
# plus provenance.csv (the version of every extension the case used).
#
# usage: scripts/gen-optimize-cases.sh <production|delivery|replenishment>...
#   DUCKDB=/path/to/duckdb (v1.5.6; default: duckdb on PATH), started -unsigned.
#   The extensions come from get.erpl.io:
#     INSTALL anofox_optimize FROM 'https://get.erpl.io'
#     INSTALL anofox_forecast FROM 'https://get.erpl.io'   # replenishment only
set -euo pipefail
export DATAZOO_DISABLE_TELEMETRY=1  # no usage telemetry from anofox extensions, whatever was inherited
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DUCKDB="${DUCKDB:-duckdb}"
cd "$ROOT"
for case in "$@"; do
  case "$case" in
    production)    exts="'anofox_optimize'"
                   tables="pp_orders pp_scenarios pp_algos pp_gantt pp_matrix" ;;
    delivery)      exts="'anofox_optimize'"
                   tables="dr_stops dr_summary dr_vans dr_route dr_legs" ;;
    replenishment) exts="'anofox_forecast', 'anofox_optimize'"
                   tables="ir_skus ir_sales ir_forecast ir_backtest ir_need ir_lots ir_plan ir_summary" ;;
    *) echo "unknown case: $case" >&2; exit 2 ;;
  esac
  out="web/data/optimize-$case"
  mkdir -p "$out"
  copy="COPY (SELECT extension_name, extension_version, (SELECT library_version FROM pragma_version()) AS duckdb_version
         FROM duckdb_extensions() WHERE extension_name IN ($exts) ORDER BY 1) TO '$out/provenance.csv' (HEADER);"
  for t in $tables; do
    copy+="COPY (FROM $t ORDER BY ALL) TO '$out/$t.csv' (HEADER);"
  done
  echo "== $case → $out"
  "$DUCKDB" -unsigned -c ".read examples/optimize-$case/compute.sql" -c "$copy" 2>&1 | grep -v '^\[anofox\]' || true
  ls -l "$out"
done
