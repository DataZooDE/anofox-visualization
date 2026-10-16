// Node test for the browser glue of contract plot calls (run by
// tests/web_js.rs): the helpers web/app.js uses to turn a planned
// `SELECT anofox_plot_xxx(t) FROM … t` result row into a panel, and — when the
// wasm bundle is built (web/pkg, `wasm-pack build --target web --out-dir
// web/pkg --no-default-features --features wasm`) — plan() + render_contract().
import { readFileSync, existsSync } from "fs";
import assert from "assert/strict";

const src = readFileSync(new URL("../../web/app.js", import.meta.url), "utf8");
const grab = (name) => {
  const m = src.match(new RegExp(`^function ${name}\\([\\s\\S]*?^}`, "m"));
  assert.ok(m, `function ${name} not found in app.js`);
  return m[0];
};
const { contractCell, contractCaption, contractHeight } = new Function(
  `${grab("contractCell")}\n${grab("contractCaption")}\n${grab("contractHeight")}\n` +
    "return { contractCell, contractCaption, contractHeight };"
)();

// Payload cells: JSON text as-is (DuckDB-Wasm), nested values re-serialised
// (a native backend), missing → the empty default.
assert.equal(contractCell({ c1: '[{"a":1}]' }, 1, "[]"), '[{"a":1}]');
assert.equal(contractCell({ c1: [{ a: 1 }] }, 1, "[]"), '[{"a":1}]');
assert.equal(contractCell({ c2: { kind: "qq" } }, 2, "{}"), '{"kind":"qq"}');
assert.equal(contractCell({}, 2, "{}"), "{}");
assert.equal(contractCell(null, 2, "[]"), "[]");

// Captions: ::TITLE + GROUP BY keys.
const c = { macro: "anofox_plot_curve", rows: 1, args: 2, groups: [[0, "curve_type"]] };
assert.equal(contractCaption("Curves", c, { c0: "roc" }), "Curves — roc");
assert.equal(contractCaption(null, c, { c0: "roc" }), "roc");
assert.equal(contractCaption("Curves", { ...c, groups: [] }, { c0: "roc" }), "Curves");
assert.equal(contractCaption(null, { ...c, groups: [] }, {}), "");
assert.equal(
  contractCaption("T", { ...c, groups: [[0, "a"], [3, "b"]] }, { c0: "x", c3: 2 }),
  "T — x · 2"
);
assert.ok(contractHeight("anofox_plot_diagnostics") > contractHeight("anofox_plot_curve"));
assert.equal(contractHeight("anofox_plot_wibble"), 320);

// The wasm side, when built.
const pkg = new URL("../../web/pkg/anofox_visualization.js", import.meta.url);
const wasm = new URL("../../web/pkg/anofox_visualization_bg.wasm", import.meta.url);
if (existsSync(pkg) && existsSync(wasm)) {
  const m = await import(pkg.href);
  m.initSync({ module: readFileSync(wasm) });
  const plan = JSON.parse(
    m.plan(
      "CREATE TABLE t AS SELECT 1;\n" +
        "SELECT 6::COL;\n" +
        "SELECT model_id, anofox_plot_terms(c), 'Coefficients'::TITLE FROM tidy c GROUP BY model_id;\n" +
        "SELECT anofox_plot_obs(o, kind := 'qq') FROM aug o;"
    )
  );
  assert.equal(plan.length, 4);
  assert.equal(plan[0].setup, true);
  assert.equal(plan[1].contract, null);
  const p = plan[2];
  assert.equal(p.setup, false);
  assert.deepEqual(p.contract, { macro: "anofox_plot_terms", rows: 1, args: 2, groups: [[0, "model_id"]] });
  assert.deepEqual(p.roles, [[3, "TITLE", ""]]);
  assert.match(p.sql, /to_json\(list\("c" ORDER BY "c"\)\) AS c1/);
  assert.match(plan[3].sql, /json_object\('kind', 'qq'\) AS c1/);
  const rows = JSON.stringify([
    { model_id: "ols", term: "price", estimate: -17.5, conf_low: -19.2, conf_high: -15.9 },
    { model_id: "ols", term: "promo", estimate: 14.3, conf_low: 12.1, conf_high: 16.4 },
  ]);
  const svg = m.render_contract("anofox_plot_terms", rows, "{}", 600, 340, "");
  assert.match(svg, /^<svg/);
  assert.ok(svg.includes("promo"));
  // Producer spellings (statsmodels) are read as contract columns.
  const sm = JSON.stringify([
    { term: "price", "Coef.": -17.5, "P>|t|": 0.001, "[0.025": -19.2, "0.975]": -15.9 },
    { term: "promo", "Coef.": 14.3, "P>|t|": 0.002, "[0.025": 12.1, "0.975]": 16.4 },
  ]);
  const smSvg = m.render_contract("anofox_plot_terms", sm, "{}", 600, 340, "");
  assert.ok(smSvg.includes("promo: 14.3 [12.1, 16.4]"), smSvg.slice(0, 400));
  // A misnamed column: the error SVG suggests it.
  const bad = JSON.stringify([{ term: "a", estimat: 1 }]);
  assert.match(m.render_contract("anofox_plot_terms", bad, "{}", 600, 340, ""), /did you mean/);
  // A macro the build doesn't know yet → a note, not an error.
  assert.match(m.render_contract("anofox_plot_wibble", "[]", "{}", 600, 340, ""), /next anofox-visualization release/);
  // Bad input → an error SVG (never a throw).
  assert.match(m.render_contract("anofox_plot_terms", "{", "{}", 600, 340, ""), /^<svg/);
  console.log("web contract tests: ok (with wasm)");
} else {
  console.log("web contract tests: ok (web/pkg not built — wasm part skipped)");
}
