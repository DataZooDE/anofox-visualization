// ::THRESHOLDS in the browser (#30): app.js's parseThresholds / thresholdRule
// over the wasm `threshold_rules` parser give the same statuses as the Rust
// rule set (src/thresholds.rs) that colours headless KPIs, gauges and charts.
// Skips without web/pkg.
import { readFileSync, existsSync } from "fs";
import assert from "assert/strict";

const pkg = new URL("../../web/pkg/", import.meta.url);
if (!existsSync(new URL("anofox_visualization_bg.wasm", pkg))) {
  console.log("web/pkg not built — skipping thresholds_test");
  process.exit(0);
}
const wasm = await import(new URL("anofox_visualization.js", pkg));
wasm.initSync({ module: readFileSync(new URL("anofox_visualization_bg.wasm", pkg)) });

const src = readFileSync(new URL("../../web/app.js", import.meta.url), "utf8");
const grab = (name) => {
  const m = src.match(new RegExp(`^function ${name}\\([\\s\\S]*?^}`, "m"));
  assert.ok(m, `function ${name} not found in app.js`);
  return m[0];
};
const { parseThresholds, thresholdRule } = new Function(
  "threshold_rules",
  `const cleanNum = (v) => (v == null || v === "" ? null : Number(String(v).replace(/[^0-9eE.+-]/g, "")));\n` +
    `${grab("parseThresholds")}\n${grab("thresholdRule")}\nreturn { parseThresholds, thresholdRule };`,
)(wasm.threshold_rules);

const status = (spec, v) => thresholdRule(parseThresholds(spec)[0].rules, v)?.status ?? null;

// first match wins; ranges are inclusive; a bare status catches the rest
for (const [v, want] of [[-1, "bad"], [0, "warn"], [0.05, "warn"], [0.2, "good"]])
  assert.equal(status("<0:bad; 0..0.05:warn; >0.05:good", v), want, `${v}`);
assert.equal(status("<80:bad; <95:warn; good", 94.9), "warn");
assert.equal(status(">=1.2M:ok; >=900k:warning; critical", 1e6), "warn");
assert.equal(status("<5%:good; bad", 0.04), "good");
assert.equal(status("<0:bad", "n/a"), null);
// table directive: one rule set per named column
const g = parseThresholds("Margin %: <10:bad; <15:warn; good | accuracy: <0.85:bad");
assert.deepEqual(g.map((x) => x.column), ["Margin %", "accuracy"]);
assert.equal(thresholdRule(g[0].rules, "12.5").status, "warn");
// malformed → no rules (dashboard --check reports thresholds/bad-rule)
assert.deepEqual(parseThresholds("<0:purple"), []);
console.log("thresholds_test ok");
