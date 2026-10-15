# shellcheck shell=bash
# shellcheck disable=SC2034  # the variables are used by the scripts sourcing this
# Shared settings for the release scripts (sourced, not executed).
#
#   ROOT            repository root
#   VERSION         extension/CLI version: [workspace.package] version in Cargo.toml
#   TAG             release tag (RELEASE_TAG, default v$VERSION)
#   DUCKDB_VERSION  the DuckDB release we build/test against: .duckdb-version
#   PLATFORM        DuckDB platform string (linux_amd64, osx_arm64, windows_amd64, ...)
#   PYTHON          python interpreter (python3, or python on Windows runners)
#   EXE             ".exe" on Windows, "" elsewhere
#   DIST            output directory for release assets (default $ROOT/dist)
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
# Git Bash on Windows: use C:/... paths, which bash and native Windows programs
# (duckdb.exe reading `LOAD '<path>'`, python) both understand.
if command -v cygpath >/dev/null 2>&1; then ROOT="$(cygpath -m "$ROOT")"; fi
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' "$ROOT/Cargo.toml" | head -1)"
TAG="${RELEASE_TAG:-v$VERSION}"
DUCKDB_VERSION="${DUCKDB_VERSION:-$(tr -d '[:space:]' < "$ROOT/.duckdb-version")}"
DIST="${DIST:-$ROOT/dist}"

if [ -z "${PYTHON:-}" ]; then
  if command -v python3 >/dev/null 2>&1 && python3 -c '' >/dev/null 2>&1; then PYTHON=python3; else PYTHON=python; fi
fi

detect_platform() {
  local os arch
  os="$(uname -s)"; arch="$(uname -m)"
  case "$arch" in
    x86_64|amd64) arch=amd64 ;;
    aarch64|arm64) arch=arm64 ;;
    *) echo "unsupported architecture: $arch" >&2; return 1 ;;
  esac
  case "$os" in
    Linux) echo "linux_$arch" ;;
    Darwin) echo "osx_$arch" ;;
    MINGW*|MSYS*|CYGWIN*) echo "windows_$arch" ;;
    *) echo "unsupported OS: $os" >&2; return 1 ;;
  esac
}
PLATFORM="${PLATFORM:-$(detect_platform)}"
case "$PLATFORM" in windows_*) EXE=.exe ;; *) EXE= ;; esac

# The extension asset name: which release, which DuckDB it was tested
# against, which platform. DuckDB derives the extension name (and so the
# `<name>_init_c_api` entry point) from the file name up to the FIRST dot, so
# the name must start with `anofox_visualization.` and use dots as separators:
#   anofox_visualization.v2026.10.15.duckdb-v1.5.6.linux_amd64.duckdb_extension
ext_asset_name() {
  echo "anofox_visualization.$TAG.duckdb-$DUCKDB_VERSION.$PLATFORM.duckdb_extension"
}

# The Rust host target triple (x86_64-unknown-linux-gnu, aarch64-apple-darwin, ...).
rust_target() { rustc -vV | sed -n 's/^host: //p' | tr -d '\r'; }

# Build web/pkg (the wasm renderer the builder UI loads) unless it is already
# there or SKIP_WASM=1.
ensure_web_pkg() {
  if [ "${SKIP_WASM:-0}" = "1" ] || [ -f "$ROOT/web/pkg/anofox_visualization_bg.wasm" ]; then return 0; fi
  command -v wasm-pack >/dev/null || { echo "wasm-pack is required (or SKIP_WASM=1)" >&2; return 1; }
  ( cd "$ROOT" && wasm-pack build --target web --out-dir web/pkg --no-default-features --features wasm )
}
