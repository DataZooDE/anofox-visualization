#!/usr/bin/env bash
# Build the wasm extension, package it (metadata custom section), serve a local
# extension repo, and load it in DuckDB-Wasm via headless Chromium — printing the
# result. Requires: emsdk at ~/emsdk, and Playwright in ../../ggplot-rs/web/e2e.
set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
GG=/home/simonm/projects/rust/ggplot-rs
source ~/emsdk/emsdk_env.sh 2>/dev/null

echo "== build (emscripten side-module) =="
cd "$HERE"
rm -rf target/wasm32-unknown-emscripten
RUSTFLAGS="-C link-arg=-sSIDE_MODULE=1" cargo build --target wasm32-unknown-emscripten \
  | grep -iE "error|Finished" || true
wasm-opt -Oz --strip-debug target/wasm32-unknown-emscripten/debug/anofox_visualization.wasm -o /tmp/ggext.wasm

echo "== package (duckdb_signature custom section) =="
REPO="$GG/web/extrepo/v1.1.1/wasm_eh"; mkdir -p "$REPO"
python3 "$HERE/scripts/append_extension_metadata.py" -l /tmp/ggext.wasm -n anofox_visualization \
  -o "$REPO/anofox_visualization.duckdb_extension.wasm" -p wasm_eh -dv v0.0.1 -ev v0.1.0 >/dev/null
python3 -c "import gzip;d=open('$REPO/anofox_visualization.duckdb_extension.wasm','rb').read();open('$REPO/anofox_visualization.duckdb_extension.wasm.gz','wb').write(gzip.compress(d))"

echo "== serve + load in DuckDB-Wasm (headless Chromium) =="
pkill -f "http.server -d $GG/web 8130" 2>/dev/null || true
( python3 -m http.server -d "$GG/web" 8130 >/dev/null 2>&1 & ) ; sleep 2
cd "$GG/web/e2e"
cat > _wasmtest.mjs <<'EOF'
import { chromium } from "playwright";
const b = await chromium.launch({ headless: true }); const p = await b.newPage();
await p.goto("http://localhost:8130/", { waitUntil: "load", timeout: 60000 });
const r = await p.evaluate(async () => {
  const duckdb = await import("https://cdn.jsdelivr.net/npm/@duckdb/duckdb-wasm@1.29.0/+esm");
  const bundle = await duckdb.selectBundle(duckdb.getJsDelivrBundles());
  const w = URL.createObjectURL(new Blob([`importScripts("${bundle.mainWorker}");`],{type:"text/javascript"}));
  const db = new duckdb.AsyncDuckDB(new duckdb.ConsoleLogger(), new Worker(w));
  await db.instantiate(bundle.mainModule, bundle.pthreadWorker);
  await db.open({ path: ":memory:", allowUnsignedExtensions: true });
  const c = await db.connect(); const s = {};
  try {
    await c.query("SET custom_extension_repository='http://localhost:8130/extrepo';");
    await c.query("INSTALL anofox_visualization;"); await c.query("LOAD anofox_visualization;"); s.load = "ok";
    s.result = (await c.query("SELECT length(anofox_render()) AS n")).toArray().map(r=>r.toJSON());
  } catch (e) { s.error = String(e); }
  return s;
});
console.log("RESULT:", JSON.stringify(r));
await b.close();
EOF
node _wasmtest.mjs; rm -f _wasmtest.mjs
pkill -f "http.server -d $GG/web 8130" 2>/dev/null || true
rm -rf "$GG/web/extrepo"
