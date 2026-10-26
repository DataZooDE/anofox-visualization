// Node test for `::GUIDE_STEP`'s pure helpers (#516): extracts readStepSeed
// and clampStep from web/app.js (same `grab` convention as
// tests/web/show_if_test.mjs / views_test.mjs). The render loop's own
// GUIDE_STEP branch and the post-loop bar/footer finalisation are nested
// closures over `runOnce`'s state (stmts, dpVars, stepGroups, …) — like
// those other cases, they have no standalone pure form this convention can
// isolate, so they are covered instead by the browser e2e
// (scripts/steps-e2e.mjs) and the Rust-side dashboard.rs test
// (tests/guide_steps_516.rs) rather than here.
import { readFileSync } from "fs";
import assert from "assert/strict";

const src = readFileSync(new URL("../../web/app.js", import.meta.url), "utf8");
const grab = (name) => {
  const m = src.match(new RegExp(`^function ${name}\\([\\s\\S]*?^}`, "m"));
  assert.ok(m, `function ${name} not found in app.js`);
  return m[0];
};
const { readStepSeed, clampStep } = new Function(`${grab("readStepSeed")}\n${grab("clampStep")}\nreturn { readStepSeed, clampStep };`)();

// readStepSeed: garbage, 0 and negatives become undefined (the caller falls
// back to 1); a well-formed positive integer (as a string, from the URL) passes.
assert.equal(readStepSeed(undefined), undefined);
assert.equal(readStepSeed([]), undefined);
assert.equal(readStepSeed(["0"]), undefined);
assert.equal(readStepSeed(["-3"]), undefined);
assert.equal(readStepSeed(["abc"]), undefined);
assert.equal(readStepSeed(["3"]), 3);
assert.equal(readStepSeed(["3.7"]), 3); // parseInt truncates
assert.equal(readStepSeed(["007"]), 7);

// clampStep: no gates (or every gate open) just clamps to [1, count].
assert.equal(clampStep(1, [], 5), 1);
assert.equal(clampStep(5, [], 5), 5);
assert.equal(clampStep(0, [], 5), 1, "garbage/below range -> 1");
assert.equal(clampStep(-9, [], 5), 1);
assert.equal(clampStep(99, [], 5), 5, "above range -> N");
assert.equal(clampStep(3, [true, true, true, true], 5), 3);

// A closed gate on step i (0-based `gates[i]` = step i+1's own gate) caps
// the furthest reachable step at i+1 — a request past it clamps back.
assert.equal(clampStep(5, [true, false, true, true], 5), 2, "gate closed on step 2: furthest is 2");
assert.equal(clampStep(1, [true, false, true, true], 5), 1, "below the gate is unaffected");
assert.equal(clampStep(2, [true, false, true, true], 5), 2, "exactly at the gate's own step is fine");
// The FIRST closed gate wins (an earlier one blocks reaching a later one).
assert.equal(clampStep(5, [false, false, true, true], 5), 1);
// A closed gate on the last step changes nothing (nothing is past it).
assert.equal(clampStep(5, [true, true, true, true, false], 5), 5);

console.log("steps_test: ok");
