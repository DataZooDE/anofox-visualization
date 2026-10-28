// KPI tiles in the browser: kpiSpark (app.js) draws a ::SPARKLINE list through
// the wasm render_cell, and the change / goal texts come from the wasm export
// kpi_parts (src/kpi.rs, the same texts as the headless tile). Skips without
// web/pkg.
import { readFileSync, existsSync } from "fs";
import assert from "assert/strict";

const src = readFileSync(new URL("../../web/app.js", import.meta.url), "utf8");
const grab = (name) => {
  const m = src.match(new RegExp(`^function ${name}\\([\\s\\S]*?^}`, "m"));
  assert.ok(m, `function ${name} not found in app.js`);
  return m[0];
};
const escapeHtml = (s) => String(s).replace(/[&<>"']/g, (c) => `&#${c.charCodeAt(0)};`);
const fmtNum = (n) => String(n);
const pkg = new URL("../../web/pkg/", import.meta.url);
if (!existsSync(new URL("anofox_visualization_bg.wasm", pkg))) {
  console.log("web/pkg not built — skipping the wasm part of kpi_test");
  process.exit(0);
}
const wasm = await import(new URL("anofox_visualization.js", pkg));
wasm.initSync({ module: readFileSync(new URL("anofox_visualization_bg.wasm", pkg)) });
const parts = (req) => JSON.parse(wasm.kpi_parts(JSON.stringify(req)));

// kpiSpark draws through the wasm render_cell (render_sparkline, #665)
const renderCtx = () => JSON.stringify({ theme: "", variant: "light" });
const { kpiSpark } = new Function(
  "escapeHtml",
  "fmtNum",
  "render_cell",
  "renderCtx",
  `const SPARK_FLOOR = 50;\n${grab("sparkRequestSize")}\n${grab("kpiSpark")}\nreturn { kpiSpark };`
)(escapeHtml, fmtNum, wasm.render_cell, renderCtx);
// a JS array (DuckDB-Wasm) or a JSON string; nulls skipped; < 2 numbers → ""
const svg = kpiSpark([1, null, 3, 2]);
assert.match(svg, /^<svg class="metric-spark" preserveAspectRatio="xMinYMid meet" role="img" aria-label="1 → 2 · min 1 · max 3 · 3 points"/);
assert.match(svg, /^<svg[^>]*><title>1 → 2 · min 1 · max 3 · 3 points<\/title>/);
assert.match(svg, /data-geom="sparkline"|gg-line/); // ggplot-rs marks, not a hand-written polyline
assert.ok(!svg.includes("<polyline points=\"0.0,"), "no hand-drawn polyline");
// the viewBox keeps the requested 320:34 aspect (the end dot stays round)
const vb = svg.match(/viewBox="0 0 ([\d.]+) ([\d.]+)"/);
assert.ok(vb && Math.abs(vb[1] / vb[2] - 320 / 34) < 0.05, vb && vb[0]);
assert.ok(kpiSpark("[4, 5]").startsWith("<svg"));
assert.equal(kpiSpark([7]), "");
assert.equal(kpiSpark(null), "");
assert.equal(kpiSpark("12"), "");

let p = parts({ value: 1100, prev: 1000, fmt: "MONEY", delta_format: "both" });
assert.deepEqual(p.delta, { up: true, good: true, arrow: "▲", pct: "10%", abs: "+$100", text: "10% (+$100)" });
assert.equal(p.goal, null);
p = parts({ value: "54.2", prev: "56.5", fmt: "PERCENT", delta_format: "abs", lower_good: true });
assert.equal(p.delta.text, "−2.3 pp");
assert.equal(p.delta.good, true);
// NULL comparison (::PERIOD "No comparison") → no change line
assert.equal(parts({ value: 5, prev: null, fmt: "METRIC" }).delta, null);
// prev = 0 → the absolute part only
assert.equal(parts({ value: 250, prev: 0, fmt: "MONEY" }).delta.text, "+$250");
// unknown format: flagged, shown as a %
p = parts({ value: 2, prev: 1, fmt: "METRIC", delta_format: "percentage" });
assert.equal(p.delta_format_ok, false);
assert.equal(p.delta.text, "100%");
// goal
p = parts({ value: 984000, goal: 1200000, fmt: "MONEY" });
assert.deepEqual(p.goal, { frac: 0.82, reached: false, text: "82% of goal $1.2M" });
// locale
assert.equal(parts({ value: 1100, prev: 1000, fmt: "MONEY", delta_format: "abs", locale: "de-DE" }).delta.text, "+100 €");
console.log("kpi_test ok");
