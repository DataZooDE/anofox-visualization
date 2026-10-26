#!/usr/bin/env bash
# Build web/pkg — the wasm module + JS/TS glue the builder UI (anofox_serve,
# the standalone `serve` CLI, `dashboard --site`) loads — straight from
# `cargo build`, never wasm-pack (#552): this box's wasm-pack (0.13.1) always
# adds an implicit --release alongside a custom --profile, which cargo then
# rejects ("the argument '--release' cannot be used with '--profile
# <PROFILE-NAME>'") — `--profile wasm` never got past `cargo build` locally,
# on any wasm-pack version with this regression. Does by hand what `wasm-pack
# build --target web` does under the hood instead (CI's own wasm-pack step
# — `wasm-pack build --target web --out-dir web/pkg --profile wasm
# --no-default-features --features wasm`, ci.yml/pages.yml — isn't hit by
# this and is left alone, #566): compile to wasm32-unknown-unknown with that
# same profile/features, run wasm-bindgen for the `--target web` JS/TS glue,
# then wasm-opt (wasm-pack bundles/fetches its own copy of that for the same
# effect).
#
# One script so duckext/scripts/build-native.sh and scripts/release/
# common.sh's ensure_web_pkg() don't each carry their own copy (#566 — #565
# added this to build-native.sh only; common.sh's ensure_web_pkg() still
# shelled out to wasm-pack). This script always (re)builds — callers decide
# whether to call it at all (SKIP_WASM=1, or web/pkg already present) and
# touch their own serve.rs embedding it (duckext/src/serve.rs, src/bin/
# serve.rs) afterwards.
#   env: CARGO_TARGET_DIR, CARGO_BUILD_JOBS
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"

echo "== build web/pkg (wasm) =="
command -v wasm-bindgen >/dev/null || { echo "wasm-bindgen (the CLI, not just the crate) is required (or SKIP_WASM=1)" >&2; exit 1; }
command -v wasm-opt >/dev/null || { echo "wasm-opt is required (or SKIP_WASM=1)" >&2; exit 1; }
rustup target add wasm32-unknown-unknown >/dev/null 2>&1 || true

# The wasm-bindgen CLI must be the exact version of the `wasm-bindgen` crate
# this links against (Cargo.lock) — a mismatch fails at runtime (the
# generated glue panics on load), not at build time.
WB_CRATE_VERSION="$(awk '/^name = "wasm-bindgen"$/{f=1;next} f && /^version = /{print; exit}' "$ROOT/Cargo.lock" | sed 's/^version = "\(.*\)"/\1/')"
WB_CLI_VERSION="$(wasm-bindgen --version | awk '{print $2}')"
[ "$WB_CLI_VERSION" = "$WB_CRATE_VERSION" ] || {
  echo "wasm-bindgen CLI ($WB_CLI_VERSION) must match the wasm-bindgen crate pinned in Cargo.lock ($WB_CRATE_VERSION)" >&2
  echo "  cargo install wasm-bindgen-cli --version $WB_CRATE_VERSION --locked" >&2
  exit 1
}

( cd "$ROOT" && CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-8}" cargo build --locked --lib --target wasm32-unknown-unknown --profile wasm --no-default-features --features wasm )
rm -rf "$ROOT/web/pkg"
wasm-bindgen --target web --out-dir "$ROOT/web/pkg" --out-name anofox_visualization \
  "$TARGET_DIR/wasm32-unknown-unknown/wasm/anofox_visualization.wasm"
wasm-opt -O "$ROOT/web/pkg/anofox_visualization_bg.wasm" -o "$ROOT/web/pkg/anofox_visualization_bg.wasm"
