#!/usr/bin/env bash
# Smoke-test a packaged full-toolkit extension with the DuckDB CLI:
# LOAD (unsigned), render an SVG, NULL -> NULL, a bad spec raises an error,
# anofox_contract_version(), the serving functions are registered, the
# extension reports the release version, and the builder UI (web/pkg) is
# embedded unless SKIP_WASM=1.
#   usage: [DUCKDB=duckdb] scripts/release/smoke-extension.sh <file.duckdb_extension>
set -euo pipefail
# shellcheck source=scripts/release/common.sh
. "$(dirname "$0")/common.sh"

EXT="${1:?usage: smoke-extension.sh <file.duckdb_extension>}"
# A bare relative name would be looked up on the dynamic-loader path.
case "$EXT" in /*|./*|../*|[A-Za-z]:*) ;; *) EXT="./$EXT" ;; esac
DUCKDB="${DUCKDB:-$("$ROOT/scripts/release/install-duckdb-cli.sh")}"
fail() { echo "FAIL: $*" >&2; exit 1; }
q() { "$DUCKDB" -unsigned -noheader -list -c "LOAD '$EXT'; $1" | tr -d '\r'; }

echo "== smoke: $EXT with $("$DUCKDB" --version | tr -d '\r') =="

svg="$(q "WITH d AS (SELECT * FROM (VALUES ('app',30),('web',22),('api',12)) t(ch,n))
SELECT anofox_render(json_object(
  'rows',  (SELECT to_json(list({c0: ch, c1: n})) FROM d),
  'roles', json('[[0,\"XAXIS\"],[1,\"BARCHART\"]]'),
  'width', 400, 'height', 260));")"
case "$svg" in "<svg"*"<rect"*) echo "OK: anofox_render -> ${#svg} byte SVG" ;; *) fail "anofox_render: ${svg:0:120}" ;; esac

[ "$(q "SELECT anofox_render(NULL) IS NULL;")" = "true" ] || fail "anofox_render(NULL) must be NULL"
echo "OK: anofox_render(NULL) -> NULL"

if err="$("$DUCKDB" -unsigned -c "LOAD '$EXT'; SELECT anofox_render('not json');" 2>&1)"; then
  fail "a bad spec must raise an error"
fi
echo "OK: bad spec raises: $(echo "$err" | tr -d '\r' | grep -m1 -i error)"

cv="$(q "SELECT anofox_contract_version();")"
[ -n "$cv" ] || fail "anofox_contract_version() is empty"
echo "OK: anofox_contract_version() = $cv"

fns="$(q "SELECT string_agg(DISTINCT function_name, ',' ORDER BY function_name) FROM duckdb_functions()
          WHERE function_name IN ('anofox_serve','anofox_serve_dashboards','anofox_serve_refresh','anofox_serve_stop','anofox_bar');")"
[ "$fns" = "anofox_bar,anofox_serve,anofox_serve_dashboards,anofox_serve_refresh,anofox_serve_stop" ] || fail "functions: $fns"
echo "OK: functions registered: $fns"

ver="$(q "SELECT extension_version FROM duckdb_extensions() WHERE extension_name = 'anofox_visualization';")"
[ "$ver" = "v$VERSION" ] || fail "extension_version = '$ver', expected v$VERSION"
echo "OK: extension_version = $ver"

if [ "${SKIP_WASM:-0}" != "1" ]; then
  grep -aq "pkg/anofox_visualization_bg.wasm" "$EXT" || fail "builder UI not embedded (web/pkg missing at build time)"
  echo "OK: builder UI (web/pkg) embedded"
fi
echo "== smoke passed =="
