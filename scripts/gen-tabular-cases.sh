#!/usr/bin/env bash
# Regenerate the stored results of the anofox-tabular use-case dashboards
# (anofox_tabular has no DuckDB-Wasm build, so the browser demo reads stored
# tables). Runs examples/tabular-<case>/compute.sql natively and writes the
# result tables to web/data/tabular-<case>/*.csv, which
# web/examples/tabular-<case>.sql reads, plus provenance.csv with the
# extension version.
#
# usage: scripts/gen-tabular-cases.sh <feed|dedup>...
#   DUCKDB=/path/to/duckdb (v1.5.6; default: duckdb on PATH), started -unsigned.
#   The extension comes from get.erpl.io:  INSTALL anofox_tabular FROM 'https://get.erpl.io'
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DUCKDB="${DUCKDB:-duckdb}"
cd "$ROOT"
for case in "$@"; do
  case "$case" in
    feed)  tables="fm_checks fm_fill fm_schema fm_profile fm_price_moves fm_deliveries" ;;
    dedup) tables="dd_records dd_pairs dd_clusters dd_key_checks dd_key_quality" ;;
    *) echo "unknown case: $case" >&2; exit 2 ;;
  esac
  out="web/data/tabular-$case"
  mkdir -p "$out"
  copy="COPY (SELECT extension_name, extension_version, (SELECT library_version FROM pragma_version()) AS duckdb_version
         FROM duckdb_extensions() WHERE extension_name = 'anofox_tabular') TO '$out/provenance.csv' (HEADER);"
  for t in $tables; do
    copy+="COPY (FROM $t ORDER BY ALL) TO '$out/$t.csv' (HEADER);"
  done
  echo "== tabular-$case → $out"
  "$DUCKDB" -unsigned -c ".read examples/tabular-$case/compute.sql" -c "$copy" 2>&1 | grep -v '^\[anofox\]' || true
  ls -l "$out"
done
