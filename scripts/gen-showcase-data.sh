#!/usr/bin/env bash
# Regenerate the stored results of the extension showcases whose extension
# can't run in the browser demo (no loadable DuckDB-Wasm build, or a model
# that needs a native runtime). Runs examples/<ext>-showcase/compute.sql
# natively and writes the result tables to web/data/<ext>/*.csv, which
# web/examples/<ext>-showcase.sql reads. Each run also records the extension
# version in web/data/<ext>/provenance.csv.
#
# usage: scripts/gen-showcase-data.sh <tabular|optimize|decide>...
#   DUCKDB=/path/to/duckdb (v1.5.6; default: duckdb on PATH), started -unsigned.
#   The extensions come from get.erpl.io:  INSTALL anofox_<ext> FROM 'https://get.erpl.io'
#   decide additionally needs:  CALL decide_download('laya-multilingual')
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DUCKDB="${DUCKDB:-duckdb}"
cd "$ROOT"
for ext in "$@"; do
  case "$ext" in
    optimize) tables="os_pack_algos os_pack_bins os_knap_curve os_knap_pick os_wave_algos os_wave_load
                      os_sched_algos os_sched_gantt os_seq_algos os_portfolio os_portfolio_weights" ;;
    tabular)  tables="tq_check_results tq_daily tq_profile tq_profile_summary tq_validity tq_anomaly
                      tq_outlier_tree tq_diff" ;;
    decide)   tables="dc_scored dc_metrics" ;;
    *) echo "unknown showcase: $ext" >&2; exit 2 ;;
  esac
  out="web/data/$ext"
  mkdir -p "$out"
  copy="COPY (SELECT extension_name, extension_version, (SELECT library_version FROM pragma_version()) AS duckdb_version
         FROM duckdb_extensions() WHERE extension_name = 'anofox_$ext') TO '$out/provenance.csv' (HEADER);"
  for t in $tables; do
    # the portfolio weight lists stay out of the CSVs (os_portfolio_weights has them)
    copy+="COPY (SELECT COLUMNS(lambda c: c <> 'weights')
           FROM $t ORDER BY ALL) TO '$out/$t.csv' (HEADER);"
  done
  echo "== $ext → $out"
  "$DUCKDB" -unsigned -c ".read examples/$ext-showcase/compute.sql" -c "$copy" 2>&1 | grep -v '^\[anofox\]' || true
  ls -l "$out"
done
