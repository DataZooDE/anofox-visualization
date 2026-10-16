// ::PERIOD in the browser: the control's date math is the wasm export
// period_resolve (src/period.rs) — checked here against the same vectors as
// the Rust side (tests/fixtures/period_vectors.json, tests/period.rs) — and
// app.js only binds the result (periodResolve / periodSets). Skips the wasm
// part without web/pkg (`wasm-pack build --target web --out-dir web/pkg
// --no-default-features --features wasm`).
import { readFileSync, existsSync } from "fs";
import assert from "assert/strict";

const src = readFileSync(new URL("../../web/app.js", import.meta.url), "utf8");
const grab = (name) => {
  const m = src.match(new RegExp(`^function ${name}\\([\\s\\S]*?^}`, "m"));
  assert.ok(m, `function ${name} not found in app.js`);
  return m[0];
};

// periodSets: unquoted names (the served mode captures `SET VARIABLE name = lit`),
// quotes doubled, NULL for a missing comparison.
const { periodSets } = new Function(`${grab("periodSets")}\nreturn { periodSets };`)();
assert.deepEqual(periodSets("p", { from: "2026-09-01", prev_from: null, label: "it's" }), [
  "SET VARIABLE p_from = '2026-09-01'",
  "SET VARIABLE p_prev_from = NULL",
  "SET VARIABLE p_label = 'it''s'",
]);
// …and the served-mode capture regex in runSql accepts every one of them.
const capture = /^\s*SET\s+VARIABLE\s+([A-Za-z_]\w*)\s*=\s*([\s\S]+?);?\s*$/i;
for (const q of periodSets("period", { from: "2026-09-01", prev_to: null, prev_label: "vs Aug 2 – 31, 2026" }))
  assert.ok(capture.test(q), q);


// The URL state (#25): a ::PERIOD's choice as `v.<prefix>` (preset or
// `from..to`) + `_grain` + `_compare`, only what differs from the default.
const { periodUrlParams } = new Function(`${grab("periodUrlParams")}\nreturn { periodUrlParams };`)();
const def = { preset: "30d", compare: "prev" };
assert.deepEqual(periodUrlParams("p", undefined, def), []);
assert.deepEqual(periodUrlParams("p", { preset: "30d", compare: "prev" }, def), []);
assert.deepEqual(periodUrlParams("p", { preset: "ytd", grain: "month", compare: "yoy" }, def), [
  ["p", "ytd"],
  ["p_grain", "month"],
  ["p_compare", "yoy"],
]);
assert.deepEqual(periodUrlParams("p", { preset: "custom", from: "2026-01-01", to: "2026-03-31" }, def), [
  ["p", "2026-01-01..2026-03-31"],
]);

const pkg = new URL("../../web/pkg/", import.meta.url);
if (!existsSync(new URL("anofox_visualization_bg.wasm", pkg))) {
  console.log("web/pkg not built — skipping the wasm part of period_test");
  process.exit(0);
}
const wasm = await import(new URL("anofox_visualization.js", pkg));
wasm.initSync({ module: readFileSync(new URL("anofox_visualization_bg.wasm", pkg)) });

const fx = JSON.parse(readFileSync(new URL("../fixtures/period_vectors.json", import.meta.url), "utf8"));
for (const c of fx.cases) {
  const r = JSON.parse(wasm.period_resolve(c.spec, JSON.stringify(c.state)));
  assert.deepEqual(r.vars, c.vars, c.name);
  assert.equal(r.ok, c.ok, c.name);
  assert.equal(r.preset, c.preset, c.name);
}

// app.js's periodResolve: the viewer's local date + the column → prefix.
let today = null;
const { periodResolve } = new Function(
  "period_resolve",
  "localToday",
  `${grab("periodResolve")}\nreturn { periodResolve };`
)(wasm.period_resolve, () => today);
today = "2026-03-31";
let r = periodResolve("default=mtd", "p", {});
assert.equal(r.prefix, "p");
assert.equal(r.vars.from, "2026-03-01");
assert.equal(r.vars.prev_to, "2026-02-28");
assert.equal(r.presets.at(-1).id, "custom");
assert.deepEqual(r.grains, ["day", "week", "month", "quarter"]);
assert.equal(r.compares.length, 3);
r = periodResolve("", "'presets=…'", { preset: "custom", from: "2026-01-01", to: "2026-01-31", grain: "week", compare: "none" });
assert.equal(r.prefix, "period", "an unaliased column falls back to the prefix `period`");
assert.equal(r.vars.grain, "week");
assert.equal(r.vars.prev_from, null);
assert.equal(r.vars.label, "Jan 2026");
r = periodResolve("presets=fortnight", "p", {});
assert.equal(r.ok, false);
assert.ok(r.errors[0].includes("fortnight"));
assert.ok(r.vars, "a bad spec still resolves on the defaults");

// validPeriodSeed: each URL part is checked against the input's own spec via
// period_resolve; an invalid part is dropped, the rest still applies.
const { validPeriodSeed, readUrlSeed } = new Function(
  `const URL_VAR_RE = /^[A-Za-z_][A-Za-z0-9_]{0,63}$/;\n${grab("readUrlSeed")}\n${grab("isIsoDate")}\n${grab("validPeriodSeed")}\nreturn { validPeriodSeed, readUrlSeed };`
)();
const spec = "presets=7d,30d,ytd; default=30d; grains=week,month; today=2026-09-30";
const res = (st) => periodResolve(spec, "p", st);
const seed = (qs) => validPeriodSeed(readUrlSeed(qs), "p", res);
assert.deepEqual(seed("?v.p=ytd&v.p_grain=month&v.p_compare=yoy"), { preset: "ytd", grain: "month", compare: "yoy" });
assert.deepEqual(seed("?v.p=2026-01-01..2026-03-31"), { preset: "custom", from: "2026-01-01", to: "2026-03-31" });
assert.equal(seed("?v.other=1"), undefined);
for (const bad of ["qtd", "fortnight", "2026-02-30..2026-03-01", "2026-01-01..", "custom", "ytd'; DROP"])
  assert.equal(seed("?v.p=" + encodeURIComponent(bad)), undefined, bad);
assert.deepEqual(seed("?v.p=nope&v.p_grain=day&v.p_compare=none"), { compare: "none" }, "day is not offered");
assert.equal(seed("?v.p=ytd&v.p=7d"), undefined, "one value only");
// round trip: what periodUrlParams writes, validPeriodSeed reads back.
const st = { preset: "custom", from: "2026-02-01", to: "2026-02-28", grain: "week", compare: "yoy" };
const qs = "?" + new URLSearchParams(periodUrlParams("p", st, res({})).map(([k, v]) => ["v." + k, v]));
assert.deepEqual(seed(qs), st);
console.log("period_test ok");
