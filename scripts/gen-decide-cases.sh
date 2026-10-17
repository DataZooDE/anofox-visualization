#!/usr/bin/env bash
# Regenerate the stored results of the anofox-decide use-case dashboards
# (anofox_decide has no DuckDB-Wasm build and its local model needs ONNX
# Runtime). Runs examples/decide-<case>/compute.sql natively and writes the
# result tables to web/data/decide-<case>/*.csv, which
# web/examples/decide-<case>.sql reads, plus provenance.csv.
#
# usage: scripts/gen-decide-cases.sh <claims|leads>...
#   DUCKDB=/path/to/duckdb (v1.5.6; default: duckdb on PATH), started -unsigned.
#   Once:  INSTALL anofox_decide FROM 'https://get.erpl.io'
#          LOAD anofox_decide; CALL decide_download('laya-multilingual')
set -euo pipefail
export DATAZOO_DISABLE_TELEMETRY=1  # no usage telemetry from anofox extensions, whatever was inherited
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DUCKDB="${DUCKDB:-duckdb}"
cd "$ROOT"
for c in "$@"; do
  case "$c" in
    claims) tables="cl_scored cl_signals cl_metrics" ;;
    leads)  tables="ld_scored ld_metrics" ;;
    *) echo "unknown case: $c" >&2; exit 2 ;;
  esac
  out="web/data/decide-$c"
  mkdir -p "$out"
  copy="COPY (SELECT extension_name, extension_version, (SELECT library_version FROM pragma_version()) AS duckdb_version
         FROM duckdb_extensions() WHERE extension_name = 'anofox_decide') TO '$out/provenance.csv' (HEADER);"
  for t in $tables; do
    copy+="COPY (FROM $t ORDER BY ALL) TO '$out/$t.csv' (HEADER);"
  done
  echo "== decide-$c → $out"
  "$DUCKDB" -unsigned -c ".read examples/decide-$c/compute.sql" -c "$copy" 2>&1 | grep -v '^\[anofox\]' || true
  ls -l "$out"
done
