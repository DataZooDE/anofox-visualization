// Node test (no browser) for wireTabs — the exported snapshot's tab/sub-tab
// switcher (web/app.js's SNAPSHOT_JS, #381). Extracts the function (it lives
// inside a template literal, not a top-level declaration, so this doesn't
// use the `grab` convention of urlstate_test.mjs/interaction_test.mjs — a
// brace-balanced slice instead) and drives it against tiny fake bar/wrap/
// button/pane objects (no jsdom in this repo's Node test deps).
//
// Before #381's fix, the export wired ONE flat list of every `.tab-btn` in
// the document (top-level AND sub-tab buttons share that class) against a
// SINGLE top-level `.tabwrap`'s panes by position — so a sub-tab button's
// index ran past the top-level panes array. This checks the actual
// replacement: each call is independently scoped to its own bar/wrap pair.
import { readFileSync } from "fs";
import assert from "assert/strict";

const src = readFileSync(new URL("../../web/app.js", import.meta.url), "utf8");
const start = src.indexOf("function wireTabs(");
assert.ok(start >= 0, "wireTabs not found in web/app.js (SNAPSHOT_JS)");
let depth = 0,
  end = -1;
for (let i = start; i < src.length; i++) {
  if (src[i] === "{") depth++;
  else if (src[i] === "}") {
    depth--;
    if (depth === 0) {
      end = i + 1;
      break;
    }
  }
}
assert.ok(end > start, "wireTabs's body braces did not balance");
const { wireTabs } = new Function(`${src.slice(start, end)}\nreturn { wireTabs };`)();

// A fake button: classList + addEventListener/_fire (the test's stand-in for
// a real click), nothing more — wireTabs never reads anything else off it.
function fakeBtn() {
  const classes = new Set();
  const handlers = [];
  return {
    classList: { add: (c) => classes.add(c), remove: (c) => classes.delete(c), contains: (c) => classes.has(c) },
    addEventListener: (ev, fn) => ev === "click" && handlers.push(fn),
    _fire: () => handlers.forEach((fn) => fn({ stopPropagation() {} })),
  };
}
const fakePane = () => ({ style: { display: "none" } });
// bar.children + wrap.querySelectorAll(sel) are the only two DOM reads.
const fakeBar = (btns) => ({ children: btns });
const fakeWrap = (panes) => ({ querySelectorAll: () => panes });

// ---- one bar/wrap pair: clicking button i shows only pane i ----
{
  const btns = [fakeBtn(), fakeBtn(), fakeBtn()];
  const panes = [fakePane(), fakePane(), fakePane()];
  wireTabs(fakeBar(btns), fakeWrap(panes), ".tabpane");
  btns[1]._fire();
  assert.deepEqual(panes.map((p) => p.style.display), ["none", "", "none"], "clicking button 1 shows only pane 1");
  assert.equal(btns[1].classList.contains("active"), true, "the clicked button is marked active");
  assert.equal(btns[0].classList.contains("active"), false, "the others are not");

  btns[2]._fire();
  assert.deepEqual(panes.map((p) => p.style.display), ["none", "none", ""], "a later click moves to pane 2");
  assert.equal(btns[1].classList.contains("active"), false, "…and clears the previous active button");
}

// ---- two independent bar/wrap pairs never cross-touch (the actual #381
// bug: one flat button list against one wrap's panes) ----
{
  const topBtns = [fakeBtn(), fakeBtn()];
  const topPanes = [fakePane(), fakePane()];
  const subBtns = [fakeBtn(), fakeBtn(), fakeBtn()];
  const subPanes = [fakePane(), fakePane(), fakePane()];
  wireTabs(fakeBar(topBtns), fakeWrap(topPanes), ".tabpane");
  wireTabs(fakeBar(subBtns), fakeWrap(subPanes), ".subtabpane");

  topBtns[1]._fire();
  subBtns[2]._fire();
  assert.deepEqual(topPanes.map((p) => p.style.display), ["none", ""], "the top pair's own panes");
  assert.deepEqual(subPanes.map((p) => p.style.display), ["none", "none", ""], "the sub pair's own panes, unaffected by the top click");

  // A sub-tab click (index 2, past the top pair's 2 panes) must never index
  // into the top pair's panes array — exactly the old bug's TypeError path.
  assert.equal(topPanes.length, 2, "sanity: fewer top panes than sub buttons (the shape that broke the old global index)");
}

// ---- a button count past its own wrap's pane count never throws (defensive
// — real bar/wrap pairs always match 1:1, but the fix guards it anyway) ----
{
  const btns = [fakeBtn(), fakeBtn()];
  const panes = [fakePane()];
  wireTabs(fakeBar(btns), fakeWrap(panes), ".tabpane");
  assert.doesNotThrow(() => btns[1]._fire(), "an out-of-range index is a no-op, not a TypeError");
  assert.equal(panes[0].style.display, "none", "…and leaves the lone pane as the (irrelevant) click found it");
}

console.log("subtabs_export_test: ok");
