#!/usr/bin/env bash
# Build + package + smoke-test the native anofox-visualization DuckDB extension
# (the C-API "full toolkit" build: rendering + anofox_serve*).
# Produces $OUT (default /tmp/anofox_visualization.duckdb_extension) for
# `duckdb -unsigned`. Extra args go to `cargo build` (e.g. --release).
# Any OUT name builds and smoke-tests, but `LOAD '<file>'` needs the name
# anofox_visualization.duckdb_extension or anofox_visualization.<x>.duckdb_extension
# (DuckDB takes the entry point from the file name up to the first dot, #201).
set -euo pipefail
HERE="$(cd "$(dirname "$0")/.." && pwd)"
CORE="$(cd "$HERE/.." && pwd)"
DUCKDB="${DUCKDB:-$(command -v duckdb || echo "$HOME/.local/bin/duckdb")}"
OUT="${OUT:-/tmp/anofox_visualization.duckdb_extension}"
# Relative to the caller's directory (the build below runs in $CORE).
case "$OUT" in /*|[A-Za-z]:*) ;; *) OUT="$PWD/$OUT" ;; esac
PLATFORM="${PLATFORM:-$("$DUCKDB" -noheader -list -c 'PRAGMA platform;')}"
# Single version source of truth: [workspace.package] in the root Cargo.toml.
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' "$CORE/Cargo.toml" | head -1)"
TARGET_DIR="${CARGO_TARGET_DIR:-$CORE/target}"

# The extension embeds web/ at compile time (include_dir!) so `SELECT
# anofox_serve(port)` can serve the browser builder. Build the wasm module first
# so web/pkg is present (REQUIRED unless SKIP_WASM=1, which builds a server
# whose UI cannot render).
if [ "${SKIP_WASM:-0}" != "1" ]; then
  echo "== build web/pkg (wasm) — embedded UI for anofox_serve =="
  # No wasm-pack (#552): this box's wasm-pack (0.13.1) always adds an
  # implicit --release alongside a custom --profile, which cargo then
  # rejects ("the argument '--release' cannot be used with '--profile
  # <PROFILE-NAME>'") — `--profile wasm` never got past `cargo build`
  # locally, on any wasm-pack version with this regression. Do by hand what
  # `wasm-pack build --target web` does under the hood instead, matching
  # CI's own wasm step exactly (.github/workflows/ci.yml and pages.yml both
  # run `wasm-pack build --target web --out-dir web/pkg --profile wasm
  # --no-default-features --features wasm`): compile to
  # wasm32-unknown-unknown with that same profile/features, run
  # wasm-bindgen for the `--target web` JS/TS glue, then wasm-opt (wasm-pack
  # bundles/fetches its own copy of that for the same effect; CI has none
  # on PATH beyond what wasm-pack brings either).
  command -v wasm-bindgen >/dev/null || { echo "wasm-bindgen (the CLI, not just the crate) is required (or SKIP_WASM=1)"; exit 1; }
  command -v wasm-opt >/dev/null || { echo "wasm-opt is required (or SKIP_WASM=1)"; exit 1; }
  rustup target add wasm32-unknown-unknown >/dev/null 2>&1 || true
  # The wasm-bindgen CLI must be the exact version of the `wasm-bindgen`
  # crate this links against (Cargo.lock) — a mismatch fails at runtime
  # (the generated glue panics on load), not at build time.
  WB_CRATE_VERSION="$(awk '/^name = "wasm-bindgen"$/{f=1;next} f && /^version = /{print; exit}' "$CORE/Cargo.lock" | sed 's/^version = "\(.*\)"/\1/')"
  WB_CLI_VERSION="$(wasm-bindgen --version | awk '{print $2}')"
  [ "$WB_CLI_VERSION" = "$WB_CRATE_VERSION" ] || {
    echo "wasm-bindgen CLI ($WB_CLI_VERSION) must match the wasm-bindgen crate pinned in Cargo.lock ($WB_CRATE_VERSION)"
    echo "  cargo install wasm-bindgen-cli --version $WB_CRATE_VERSION --locked"
    exit 1
  }
  ( cd "$CORE" && CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-8}" cargo build --locked --lib --target wasm32-unknown-unknown --profile wasm --no-default-features --features wasm )
  rm -rf "$CORE/web/pkg"
  wasm-bindgen --target web --out-dir "$CORE/web/pkg" --out-name anofox_visualization \
    "$TARGET_DIR/wasm32-unknown-unknown/wasm/anofox_visualization.wasm"
  wasm-opt -O "$CORE/web/pkg/anofox_visualization_bg.wasm" -o "$CORE/web/pkg/anofox_visualization_bg.wasm"
  touch "$HERE/src/serve.rs"   # force include_dir! to re-embed the fresh web/pkg
else
  echo "== SKIP_WASM=1 — anofox_serve UI won't include the renderer =="
fi

echo "== build cdylib (v$VERSION) =="
cd "$CORE"
CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-8}" cargo build --locked -p anofox-visualization-duckdb --lib "$@"
PROFILE=debug
for a in "$@"; do [ "$a" = "--release" ] && PROFILE=release; done
LIB=""
for f in libanofox_visualization_ext.so libanofox_visualization_ext.dylib anofox_visualization_ext.dll; do
  [ -f "$TARGET_DIR/$PROFILE/$f" ] && LIB="$TARGET_DIR/$PROFILE/$f"
done
[ -n "$LIB" ] || { echo "built library not found in $TARGET_DIR/$PROFILE"; exit 1; }
echo "   $LIB"

echo "== package (C_STRUCT metadata footer, C-API v1.2.0, platform $PLATFORM) =="
"${PYTHON:-python3}" "$HERE/scripts/append_extension_metadata.py" -l "$LIB" -n anofox_visualization \
  -p "$PLATFORM" -dv v1.2.0 -ev "v$VERSION" --abi-type C_STRUCT -o "$OUT" | grep -i "output file"

echo "== smoke test =="
# LOAD derives <name>_init_c_api from the file name up to the first dot and the
# library exports anofox_visualization_init_c_api only: test any other OUT name
# through a symlink with the canonical name (#201).
case "$(basename "$OUT")" in
  anofox_visualization.duckdb_extension|anofox_visualization.*.duckdb_extension)
    EXT="$OUT"; RENAMED= ;;
  *)
    SMOKE_DIR="$(mktemp -d)"; trap 'rm -rf "$SMOKE_DIR"' EXIT
    EXT="$SMOKE_DIR/anofox_visualization.duckdb_extension"; ln -s "$OUT" "$EXT"; RENAMED=1 ;;
esac
SMOKE="$("$DUCKDB" -unsigned -noheader -list -c "
LOAD '$EXT';
WITH d AS (SELECT * FROM (VALUES ('app',30),('web',22),('api',12)) t(ch,n))
SELECT CASE WHEN anofox_render(json_object(
  'rows',  (SELECT to_json(list({c0: ch, c1: n})) FROM d),
  'roles', json('[[0,\"XAXIS\"],[1,\"BARCHART\"]]'),
  'width', 400, 'height', 260)) LIKE '<svg%<rect%'
  THEN 'OK: rendered a bar chart' ELSE 'FAIL' END;" | tr -d '\r')"
echo "$SMOKE"
[ "$SMOKE" = "OK: rendered a bar chart" ] || { echo "FAIL: the bar-chart smoke render returned '$SMOKE'" >&2; exit 1; }
if "$DUCKDB" -unsigned -c "LOAD '$EXT'; SELECT anofox_render('not json');" >/dev/null 2>&1; then
  echo "FAIL: a bad spec must raise a SQL error"; exit 1
fi
echo "OK: bad spec raises a SQL error"
# #333/#335: DATAZOO_DISABLE_TELEMETRY defaults to "1" on LOAD, whatever this
# script's own environment has (CI sets it; strip it here so the check is of
# the extension's default, not an inherited value).
case "$PLATFORM" in
  windows_*)
    # DuckDB's SQL getenv() cannot prove this on Windows: it runs inside
    # duckdb.exe, whose CRT (statically linked for the official CLI release
    # and for every extension built via the x64-windows-static-release vcpkg
    # triplet -- confirmed from vcpkg's own triplet file, VCPKG_CRT_LINKAGE
    # static) snapshots the OS environment block into its own private table
    # at process startup, before any LOAD runs -- so it can never reflect a
    # write any extension makes afterwards, fixed or not (see
    # src/host/telemetry.rs's `windows_crt` module docs for the full CRT
    # analysis). Ask the extension itself instead, through the
    # diagnostic-only anofox_telemetry_probe() it registers: it reports
    # both the OS environment block (std::env::var / SetEnvironmentVariableW
    # -- what a static-CRT module LOADed after ours would snapshot) and this
    # process's own CRT cache (getenv / _wgetenv_s -- what this module's own
    # getenv(), and any sibling sharing a dynamic UCRT with it, would see).
    # Both must read "1" for the Windows fix to be doing its job.
    TELEMETRY="$(env -u DATAZOO_DISABLE_TELEMETRY "$DUCKDB" -unsigned -noheader -list -c "
LOAD '$EXT'; SELECT anofox_telemetry_probe();" | tr -d '\r')"
    [ "$TELEMETRY" = "os=1 crt=1" ] || { echo "FAIL: DATAZOO_DISABLE_TELEMETRY should default to 1 on LOAD (os+crt), saw '$TELEMETRY'" >&2; exit 1; }
    echo "OK: DATAZOO_DISABLE_TELEMETRY defaults to 1 on LOAD ($TELEMETRY)"
    ;;
  *)
    # getenv() is a DuckDB-CLI-only function; on Linux/macOS there is one
    # process-wide environment (no separate CRT table to diverge, see the
    # module docs above), so DuckDB's own getenv() through the duckdb binary
    # is already a direct check of what matters.
    TELEMETRY="$(env -u DATAZOO_DISABLE_TELEMETRY "$DUCKDB" -unsigned -noheader -list -c "
LOAD '$EXT'; SELECT getenv('DATAZOO_DISABLE_TELEMETRY');" | tr -d '\r')"
    [ "$TELEMETRY" = "1" ] || { echo "FAIL: DATAZOO_DISABLE_TELEMETRY should default to 1 on LOAD, saw '$TELEMETRY'" >&2; exit 1; }
    echo "OK: DATAZOO_DISABLE_TELEMETRY defaults to 1 on LOAD"
    ;;
esac
# #179: after USE <other db> the macros are reached qualified or via search_path (BUILD.md, "macro caveat").
USE_OK="$("$DUCKDB" -unsigned -noheader -list -c "
LOAD '$EXT';
ATTACH ':memory:' AS other; USE other;
CREATE TABLE s AS SELECT 'A' AS model_id, 'mae' AS metric, 1.0 AS value;
SELECT CASE WHEN memory.anofox_plot_summary(s) LIKE '<svg%' THEN 'qualified' ELSE 'FAIL' END FROM s;
SET search_path = 'other.main,memory.main';
SELECT CASE WHEN anofox_plot_summary(s) LIKE '<svg%' THEN 'search_path' ELSE 'FAIL' END FROM s;" | tr -d '\r' | tr '\n' ' ')"
[ "$USE_OK" = "qualified search_path " ] || { echo "FAIL: plot macros after USE: '$USE_OK'" >&2; exit 1; }
echo "OK: plot macros after USE (qualified, search_path)"
echo "== done → $OUT =="
if [ -n "$RENAMED" ]; then
  echo "NOTE: LOAD '$OUT' fails (DuckDB looks for $(basename "$OUT" | cut -d. -f1)_init_c_api)."
  echo "      Rename it to anofox_visualization.duckdb_extension, or LOAD a symlink with that name."
fi
