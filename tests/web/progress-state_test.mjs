// Node test for #525 (tours/steps remember progress for a signed-in
// viewer): extracts the pure helper tourSeenViaState from web/app.js (same
// convention as tests/web/tour_test.mjs) and checks it against stub
// window/document/servedPlan objects — no DOM, no wasm, no server.
import { readFileSync } from "fs";
import assert from "assert/strict";

const src = readFileSync(new URL("../../web/app.js", import.meta.url), "utf8");
const grab = (name) => {
  const m = src.match(new RegExp(`^function ${name}\\([\\s\\S]*?^}`, "m"));
  assert.ok(m, `function ${name} not found in app.js`);
  return m[0];
};
// window/document/servedPlan are passed in as parameters, so the grabbed
// function's references to them (its usual module-level closures) resolve
// to whichever stub this call supplies.
const call = new Function(
  "window",
  "document",
  "servedPlan",
  "id",
  `${grab("tourSeenViaState")}\nreturn tourSeenViaState(id);`
);

const notServed = {};
const servedNoState = { __served: { state: { enabled: false } } };
const servedEnabled = { __served: { state: { enabled: true } } };

// Not served at all: can't tell.
assert.equal(call(notServed, {}, null, "cockpit_intro"), null);
// Served, but options.state isn't on: can't tell.
assert.equal(call(servedNoState, {}, [{ roles: [] }], "cockpit_intro"), null);
// Served + enabled, but the plan hasn't been parsed yet (servedRequest
// primes it lazily): can't tell.
assert.equal(call(servedEnabled, {}, null, "cockpit_intro"), null);
// Served + enabled, plan parsed, but no ::SUBMIT anywhere carries this id:
// can't tell (falls back to localStorage at the call site).
assert.equal(
  call(servedEnabled, {}, [{ roles: [[0, "METRIC"]] }, { roles: [[0, "SUBMIT", "other_tour"]] }], "cockpit_intro"),
  null
);
// A matching ::SUBMIT exists and its panel IS in the DOM (1 row: not yet
// recorded) — not seen.
{
  const plan = [{ roles: [[0, "SUBMIT", "cockpit_intro"]] }];
  const doc = { querySelector: (sel) => (sel.includes('data-stmt="0"') ? {} : null) };
  assert.equal(call(servedEnabled, doc, plan, "cockpit_intro"), false);
}
// A matching ::SUBMIT exists and its panel is ABSENT from the DOM (zero
// rows: the author's "record once" gate already fired) — seen.
{
  const plan = [{ roles: [[0, "SUBMIT", "cockpit_intro"]] }];
  const doc = { querySelector: () => null };
  assert.equal(call(servedEnabled, doc, plan, "cockpit_intro"), true);
}
// The matching ::SUBMIT isn't the first statement — its own plan index is
// still the one looked up in the DOM (data-stmt="1", not "0").
{
  const plan = [{ roles: [[0, "METRIC"]] }, { roles: [[0, "SUBMIT", "cockpit_intro"]] }];
  const doc = { querySelector: (sel) => assert.ok(sel.includes('data-stmt="1"')) || null };
  call(servedEnabled, doc, plan, "cockpit_intro");
}

console.log("progress-state_test.mjs: ok");
