// Node test for `::SHOW_IF`'s condition value rule (#515): extracts
// condHolds from web/app.js (same `grab` convention as tests/web/views_test.mjs)
// and checks the one rule it and the native `sql::condition::holds*`
// (src/sql/condition.rs) agree on: `true` only, plus the `"true"` string
// exception (decision 7). `conditionHolds` (the async runner) is a nested
// closure over the render loop's own `stmts`/`pv` — like views_test.mjs's
// patchSlots/shapeOf cases, it has no standalone pure form this convention
// can isolate, so it is covered instead by the browser e2e
// (scripts/show-if-e2e.mjs) and the Rust-side dashboard.rs test
// (tests/show_if_515.rs) rather than here.
import { readFileSync } from "fs";
import assert from "assert/strict";

const src = readFileSync(new URL("../../web/app.js", import.meta.url), "utf8");
const grab = (name) => {
  const m = src.match(new RegExp(`^function ${name}\\([\\s\\S]*?^}`, "m"));
  assert.ok(m, `function ${name} not found in app.js`);
  return m[0];
};
const { condHolds } = new Function(`${grab("condHolds")}\nreturn { condHolds };`)();

assert.equal(condHolds(true), true);
assert.equal(condHolds("true"), true);
for (const v of [false, null, undefined, 1, "yes", "TRUE", "", 0, [], {}]) {
  assert.equal(condHolds(v), false, `condHolds(${JSON.stringify(v)})`);
}

console.log("show_if_test: ok");
