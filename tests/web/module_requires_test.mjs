// #1156: a dashboard that declares a module this build lacks
// ('name'::REQUIRES) — the wasm build says so (missing_modules), the app
// shows that message instead of running; with nothing missing it is empty.
import { readFileSync, existsSync } from "fs";
import assert from "assert/strict";

const src = readFileSync(new URL("../../web/app.js", import.meta.url), "utf8");
assert.match(src, /const needs = missing_modules\(/, "run() asks for missing modules first");

const pkg = new URL("../../web/pkg/anofox_visualization.js", import.meta.url);
const wasm = new URL("../../web/pkg/anofox_visualization_bg.wasm", import.meta.url);
// The wasm side, when built (the sharded web tests always build it).
if (!existsSync(pkg) || !existsSync(wasm)) {
  console.log("module_requires_test: ok (app.js only; no web/pkg)");
  process.exit(0);
}
const m = await import(pkg.href);
m.initSync({ module: readFileSync(wasm) });
assert.equal(
  m.missing_modules("SELECT 'hello, maps'::REQUIRES;\nSELECT 1::BARCHART, 'a'::XAXIS;"),
  "This dashboard needs the modules 'hello', 'maps', which this build does not include."
);
assert.equal(m.missing_modules("SELECT 1::BARCHART, 'a'::XAXIS;"), "");
// ::REQUIRES is a directive: in the browser's directive set, and a no-op
// branch in run(), so it draws no panel of its own
assert.ok(JSON.parse(m.roles_json()).sets.directives.includes("REQUIRES"));
assert.match(src, /\} else if \(d === "REQUIRES"\) \{/);
console.log("module_requires_test: ok");
