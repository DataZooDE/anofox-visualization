// Node test for #407 part 2 (web/app.js pathVarValue / servedNavHref; the
// server side is duckext/src/serve.rs served_index's `pathVars` +
// check_path_values, and docs/secure-serving.md "The served nav keeps
// path_vars across dashboards"): a served-nav link must carry the *current*
// value of every server-declared `pathVars` variable, URL-encoded — and
// only those, never another variable the render loop happens to hold (a
// selection, a filter, …).
//
// A `pathVars` variable is typically *headless* — read only through
// getvariable(), with no ::DROPDOWN of its own on the dashboard (the whole
// reason nav_picker exists, #407 part 3) — so its value lives in `dpXf`
// (an unclaimed `?v.<name>=` URL seed, or the picker's own onchange, both
// go through the same store as a named selection), not in `dpVars` (which
// holds only a variable bound to a declared input). `pathVarValue` picks
// the right one; this was a real bug (dpVars only) caught by
// scripts/served-e2e.mjs §5 before this test existed.
//
// No jsdom: both functions are pure (string in, string out, no DOM),
// extracted from app.js like served_nav_test.mjs does, and run with
// stand-in state closed over them.
import { readFileSync } from "fs";
import assert from "assert/strict";

const src = readFileSync(new URL("../../web/app.js", import.meta.url), "utf8");
const fn = (name) => {
  const m = src.match(new RegExp(`^function ${name}\\([\\s\\S]*?^}`, "m"));
  assert.ok(m, `function ${name} not found in app.js`);
  return m[0];
};
// pathVarValue/servedNavHref close over the module-level dpVarDefault,
// dpVars, dpXf; bind them as the factory's own parameters so each assertion
// gets its own stand-in state, never a shared mutable global.
const makeHref = new Function(
  "dpVarDefault",
  "dpVars",
  "dpXf",
  `${fn("pathVarValue")}\n${fn("servedNavHref")}\nreturn { pathVarValue, servedNavHref };`
);

// -- pathVarValue: dpXf for a headless variable (no declared input) --
{
  const { pathVarValue } = makeHref({}, {}, { model: "car-price" });
  assert.equal(pathVarValue("model"), "car-price");
}
// -- pathVarValue: dpVars for one bound to a declared input on *this*
// dashboard (name in dpVarDefault) — even if dpXf also holds something for
// the same name (stale, or a seed that arrived before the input claimed it).
{
  const { pathVarValue } = makeHref({ model: '"x"' }, { model: "car-price" }, { model: "stale" });
  assert.equal(pathVarValue("model"), "car-price");
}
// -- pathVarValue: neither set --
{
  const { pathVarValue } = makeHref({}, {}, {});
  assert.equal(pathVarValue("model"), undefined);
}

// -- servedNavHref --
const href = (dpXf, id, served) => makeHref({}, {}, dpXf).servedNavHref(id, served);

// No pathVars at all (or an empty list): a plain /d/<id> link.
assert.equal(href({}, "monitor", {}), "/d/monitor");
assert.equal(href({ model: "car-price" }, "monitor", { pathVars: [] }), "/d/monitor");

// One pathVars value, currently set (headless — the realistic nav_picker
// case): carried as ?v.<name>=<value>.
assert.equal(
  href({ model: "car-price" }, "monitor_features", { pathVars: ["model"] }),
  "/d/monitor_features?v.model=car-price"
);

// Not yet set (undefined, null, or the empty string): left off the link —
// the target dashboard falls back to its own default, same as a fresh open.
for (const v of [undefined, null, ""]) {
  assert.equal(href({ model: v }, "monitor", { pathVars: ["model"] }), "/d/monitor");
}

// Several pathVars, each carried, in the server's configured order; a
// variable NOT listed in pathVars (a plain viewer filter/selection) is never
// carried even though dpXf holds a value for it too.
assert.equal(
  href({ model: "car-price", run: "2026-10-01", region: "EU" }, "monitor", {
    pathVars: ["model", "run"],
  }),
  "/d/monitor?v.model=car-price&v.run=2026-10-01"
);

// A numeric path_vars value (DuckDB accepts a non-negative integer too):
// String()-ed, not left as a number.
assert.equal(href({ run: 7 }, "monitor", { pathVars: ["run"] }), "/d/monitor?v.run=7");

// Defense in depth: the dashboard id and every value are URL-encoded. A
// value matching the server's own plain-name rule (checked independently,
// server-side, on every request) never actually needs this, but a client
// bug upstream of this function must not turn into a broken or hijacked
// link.
assert.equal(
  href({ model: "a b&c=d#e" }, "my dash", { pathVars: ["model"] }),
  "/d/my%20dash?v.model=a%20b%26c%3Dd%23e"
);

console.log("nav_path_vars_test: ok");
