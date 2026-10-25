// `-- @name` (#518 PR A) in the browser: `tagName` (web/app.js) sets
// `data-dp-name` once and never overwrites an already-tagged element; when
// the wasm bundle is built (web/pkg, see tests/web/contract_test.mjs for how
// it's loaded) wasm `plan()` carries `Panel::name` through on the statements
// that have a `-- @name` line, and leaves it `null` on the ones that don't.
import { readFileSync, existsSync } from "fs";
import assert from "assert/strict";

const src = readFileSync(new URL("../../web/app.js", import.meta.url), "utf8");
const grab = (name) => {
  const m = src.match(new RegExp(`^function ${name}\\([\\s\\S]*?^}`, "m"));
  assert.ok(m, `function ${name} not found in app.js`);
  return m[0];
};
const { tagName } = new Function(`${grab("tagName")}\nreturn { tagName };`)();

// ---- tagName ----
{
  const el = { dataset: {} };
  tagName(el, { name: "kpi" });
  assert.equal(el.dataset.dpName, "kpi");

  // No name on the statement: untouched.
  const el2 = { dataset: {} };
  tagName(el2, { name: null });
  assert.equal(el2.dataset.dpName, undefined);
  tagName(el2, {});
  assert.equal(el2.dataset.dpName, undefined);

  // A re-run reaching the same element again: never overwritten.
  const el3 = { dataset: { dpName: "first" } };
  tagName(el3, { name: "second" });
  assert.equal(el3.dataset.dpName, "first");

  // No element (nothing matched this statement's slot): no throw.
  tagName(null, { name: "kpi" });
  tagName(undefined, { name: "kpi" });
}

// ---- wasm plan(): Panel::name ----
const pkg = new URL("../../web/pkg/anofox_visualization.js", import.meta.url);
const wasm = new URL("../../web/pkg/anofox_visualization_bg.wasm", import.meta.url);
if (existsSync(pkg) && existsSync(wasm)) {
  const m = await import(pkg.href);
  m.initSync({ module: readFileSync(wasm) });
  const plan = JSON.parse(
    m.plan(
      "-- @name kpi\nSELECT 1::METRIC;\n" +
        "SELECT 2::METRIC;\n" +
        "-- @name box\nSELECT 'Filters'::GROUP;\n" +
        "SELECT 1::ENDGROUP;\n" +
        "-- @name t1\nSELECT 'Overview'::TAB;"
    )
  );
  assert.equal(plan.length, 5);
  assert.equal(plan[0].name, "kpi");
  assert.equal(plan[1].name, null);
  assert.equal(plan[2].name, "box");
  assert.equal(plan[3].name, null);
  assert.equal(plan[4].name, "t1");
  console.log("web names tests: ok (with wasm)");
} else {
  console.log("web names tests: ok (web/pkg not built — wasm part skipped)");
}
