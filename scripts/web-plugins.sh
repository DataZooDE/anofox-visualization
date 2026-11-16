#!/usr/bin/env bash
# #1218: the plugins a web build ships (modules/<name>/, today factor-graph),
# built for wasm32 into <web>/modules/<name>/plugin.wasm and listed in
# <web>/plugins.json, which the browser builder loads before it reads the
# roles (web/plugins.js). For the static web builds only (GitHub Pages, the
# release's web tarball, the browser e2e): the servers write their own
# plugins.json from --plugin, so web/ itself (embedded in the servers)
# stays without them.
#   scripts/web-plugins.sh [<web dir>]      (default: web)
#   env: CARGO_TARGET_DIR (default: target/web-plugins)
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WEB="${1:-$ROOT/web}"
TARGET="${CARGO_TARGET_DIR:-$ROOT/target/web-plugins}"
list=()
for manifest in "$ROOT"/modules/*/Cargo.toml; do
  dir="$(dirname "$manifest")"
  name="$(basename "$dir")"
  crate="$(sed -n 's/^name = "\(.*\)"$/\1/p' "$manifest" | head -1)"
  echo "== plugin $name ($crate) =="
  cargo build --locked --release --target wasm32-unknown-unknown \
    --manifest-path "$manifest" --target-dir "$TARGET"
  mkdir -p "$WEB/modules/$name"
  cp -f "$TARGET/wasm32-unknown-unknown/release/${crate//-/_}.wasm" "$WEB/modules/$name/plugin.wasm"
  list+=("\"modules/$name/plugin.wasm\"")
done
(IFS=,; echo "[${list[*]}]") > "$WEB/plugins.json"
echo "== $WEB/plugins.json: $(cat "$WEB/plugins.json") =="
