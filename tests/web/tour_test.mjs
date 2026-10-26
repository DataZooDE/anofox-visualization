// Node test for guided tours (#518 PR B): extracts the two pure helpers,
// readTourParam and placePop, from web/app.js (same convention as
// tests/web/views_test.mjs) and checks their validation / placement rules
// directly — no DOM, no wasm.
import { readFileSync } from "fs";
import assert from "assert/strict";

const src = readFileSync(new URL("../../web/app.js", import.meta.url), "utf8");
const grab = (name) => {
  const m = src.match(new RegExp(`^function ${name}\\([\\s\\S]*?^}`, "m"));
  assert.ok(m, `function ${name} not found in app.js`);
  return m[0];
};
const idRe = src.match(/^const TOUR_ID_RE = .*$/m)[0];
const { readTourParam, placePop } = new Function(
  `${idRe}\n${grab("readTourParam")}\n${grab("placePop")}\nreturn { readTourParam, placePop };`
)();

// ---- readTourParam: `?tour=<id>`, validated (same shape as a panel's
// `-- @name`/a tour's `-- @tour <id>`) ----
assert.equal(readTourParam("?tour=cockpit_intro"), "cockpit_intro");
assert.equal(readTourParam("?a=1&tour=my-tour_2"), "my-tour_2");
assert.equal(readTourParam(""), null);
assert.equal(readTourParam("?tour="), null);
// not a well-formed id: starts with a digit, a space, a dot, too long
assert.equal(readTourParam("?tour=1bad"), null);
assert.equal(readTourParam("?tour=has space"), null);
assert.equal(readTourParam("?tour=has.dot"), null);
assert.equal(readTourParam("?tour=" + "a".repeat(65)), null);
assert.equal(readTourParam("?tour=" + "a".repeat(64)), "a".repeat(64));
// garbage / unrelated query: nothing starts
assert.equal(readTourParam("?foo=bar"), null);
assert.equal(readTourParam("not a query string"), null);
assert.equal(readTourParam("?tour=;DROP TABLE x"), null);
// repeated `?tour=` keys: URLSearchParams.get answers the first, like every
// other deep-link param this app reads (?tab=, ?view.*)
assert.equal(readTourParam("?tour=first&tour=second"), "first");

// ---- placePop: below / above / beside / clamped / the <600px sheet ----
const vp = { w: 1440, h: 900 };
const pop = { width: 320, height: 160 };

// Below the target when there's room.
{
  const r = placePop({ top: 100, bottom: 140, left: 200, right: 300, width: 100, height: 40 }, pop, vp);
  assert.equal(r.side, "below");
  assert.equal(r.top, 150); // target.bottom(140) + the 10px gap
}
// Flips above when below doesn't fit but above does.
{
  const target = { top: 820, bottom: 860, left: 200, right: 300, width: 100, height: 40 };
  const r = placePop(target, pop, vp);
  assert.equal(r.side, "above");
  assert.equal(r.top, 650); // target.top(820) - pop.height(160) - the 10px gap
}
// Beside the target when neither above nor below has room (a short viewport).
{
  const short = { w: 1440, h: 220 };
  const target = { top: 0, bottom: 220, left: 100, right: 300, width: 200, height: 220 };
  const r = placePop(target, pop, short);
  assert.equal(r.side, "beside");
}
// Clamped at the viewport's right edge — never off-screen.
{
  const target = { top: 100, bottom: 140, left: 1400, right: 1430, width: 30, height: 40 };
  const r = placePop(target, pop, vp);
  assert.equal(r.left, vp.w - pop.width - 10); // 1440 - 320 - 10
  assert.ok(r.left + pop.width <= vp.w);
}
// Clamped at the viewport's top edge.
{
  const target = { top: 0, bottom: 10, left: 200, right: 300, width: 100, height: 10 };
  const r = placePop(target, pop, vp);
  assert.ok(r.top >= 10, "never above the 10px screen gap");
}
// Under 600px wide: a bottom sheet — no top/left computed, the CSS media
// query positions it instead (decision 5).
{
  const r = placePop({ top: 100, bottom: 140, left: 10, right: 100, width: 90, height: 40 }, pop, { w: 390, h: 800 });
  assert.equal(r.side, "sheet");
  assert.equal(r.top, null);
  assert.equal(r.left, null);
}

console.log("tour_test.mjs: ok");
