#!/usr/bin/env bash
# #1266: the embedding build (`default-features = false`) stays lean — no
# server, TLS, auth, plugin runtime, raster or PDF stack. Fails when one of
# them joins the default-off dependency tree.
set -euo pipefail
cd "$(dirname "$0")/.."
tree=$(cargo tree --locked -p anofox-visualization --no-default-features -e normal --prefix none \
  | sed 's/ v[0-9].*//' | sort -u)
bad=""
for d in svg2pdf resvg usvg tiny-skia rustybuzz ttf-parser png tiny_http rustls ureq aws-lc-rs \
         jsonwebtoken wasmi wasm-bindgen include_dir open; do
  if grep -qx "$d" <<<"$tree"; then bad="$bad $d"; fi
done
if [ -n "$bad" ]; then
  echo "error: the default-features = false build now pulls in:$bad" >&2
  echo "(keep it lean, docs/embedding.md; put new heavy dependencies behind a feature)" >&2
  exit 1
fi
echo "ok: the embedding build depends on: $(tr '\n' ' ' <<<"$tree")"
