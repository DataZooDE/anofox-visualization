// #940: a relative ::HEIGHT in the page — heightOf / relHeight (web/app.js)
// over the renderer's own parser (wasm panel_height, crate::height). Pixels
// stay plain numbers; a ratio or a window share is a Number holding its
// pixel estimate (every existing use of nextHeight reads that) with `.rel`
// attached, which the chart path and fitCharts size by.
import { existsSync, readFileSync } from "fs";
import path from "path";
import assert from "assert/strict";

const here = path.dirname(new URL(import.meta.url).pathname);
const root = path.resolve(here, "../..");
const wasmPkg = path.join(root, "web/pkg");
if (!existsSync(path.join(wasmPkg, "anofox_visualization_bg.wasm"))) {
  console.error("height_940_test: FAILED — needs web/pkg (wasm-pack build)");
  process.exit(1);
}
const wasm = await import(path.join(wasmPkg, "anofox_visualization.js"));
wasm.initSync({ module: readFileSync(path.join(wasmPkg, "anofox_visualization_bg.wasm")) });

// The two functions, sliced out of web/app.js by their braces.
const src = readFileSync(path.join(root, "web/app.js"), "utf8");
function grab(name) {
  const start = src.indexOf(`function ${name}(`);
  assert.ok(start >= 0, `${name} not found in web/app.js`);
  let depth = 0;
  for (let i = src.indexOf("{", start); i < src.length; i++) {
    if (src[i] === "{") depth++;
    else if (src[i] === "}" && --depth === 0) return src.slice(start, i + 1);
  }
  throw new Error(`${name}: unbalanced braces`);
}
const innerHeight = 1000;
const { heightOf, relHeight } = new Function(
  "panel_height",
  "innerHeight",
  `${grab("heightOf")}\n${grab("relHeight")}\nreturn { heightOf, relHeight };`
)(wasm.panel_height, innerHeight);

// pixels: a plain number, as before
assert.equal(heightOf("320", 6, 1), 320);
assert.equal(heightOf(250, 6, 1), 250);
// not a height: 0 (the default height; the lint says layout/bad-height)
for (const bad of ["", "tall", "0", "120vh", "16:0", null]) assert.equal(heightOf(bad, 6, 1), 0, String(bad));

// a ratio of the width: the estimate at the render width (span × 100)
const r = heightOf("16:9", 6, 1);
assert.equal(typeof r, "object");
assert.equal(+r, Math.round(600 * 9 / 16));
assert.equal(r + "px", "338px", "plain uses read the estimate");
assert.ok(r > 0 && !!r, "truthy like a height");
assert.deepEqual(r.rel, { ratio: 9 / 16 });
assert.equal(relHeight(r.rel, 1200), 675, "it follows the drawn width");
assert.equal(+heightOf("0.5", 12, 1), 600);

// a share of the window
const v = heightOf("40vh", 4, 1);
assert.equal(+v, 400);
assert.deepEqual(v.rel, { vh: 0.4 });
assert.equal(relHeight(v.rel, 123), 400, "the width doesn't matter");

// bounds
assert.equal(relHeight({ ratio: 0.05 }, 100), 40);
assert.equal(relHeight({ ratio: 4 }, 2000), 5000);

// the chart path and the re-fit read .rel
assert.match(src, /hRel: boxH\?\.rel/);
assert.match(src, /dp\.hRel \? relHeight\(dp\.hRel, w\)/);
console.log("height_940_test: ok");
