// Node test for web/anim.js's ::STORY key matching (run by tests/web_js.rs):
// a step's key lights the marks whose data-x is the same x, however each
// side spells it.
import assert from "assert/strict";
import { animKit } from "../../web/anim.js";

const { sameX } = animKit({ document: {} });

assert.ok(sameX("W43", "W43"));
assert.ok(!sameX("W43", "W44"));
assert.ok(!sameX(null, "W43"));
// numbers by value
assert.ok(sameX("2", "2.0"));
assert.ok(!sameX("2", "3"));
// dates: with or without a midnight time
assert.ok(sameX("2026-10-12", "2026-10-12 00:00:00"));
assert.ok(!sameX("2026-10-12", "2026-10-13"));
// an older SVG's epoch-seconds key against the marks' ISO date (time), in UTC
assert.ok(sameX("1788220800", "2026-09-01"));
assert.ok(sameX("2026-09-01", "1788220800"));
assert.ok(sameX("1788255000", "2026-09-01 09:30:00"));
assert.ok(!sameX("1788220800", "2026-09-02"));
// only a 9–11 digit number reads as epoch seconds
assert.ok(!sameX("123", "1970-01-01 00:02:03"));

// ---- D24 (M2f-A, #149): the re-run tween's colour lerp on a map's areas --
// A minimal fake <path>/<svg> — just enough of the Element interface for
// keyed()/geom()/tween()/play() to walk two map features by their data-x.
function fakePath(x, fill) {
  const attrs = { "data-x": x, fill };
  return {
    tagName: "path",
    getAttribute: (k) => (k in attrs ? attrs[k] : null),
    setAttribute: (k, v) => (attrs[k] = String(v)),
    hasAttribute: (k) => k in attrs,
    cloneNode: () => fakePath(x, attrs.fill), // a snapshot, not a live reference
  };
}
function fakeMapSvg(paths) {
  return {
    hasAttribute: (k) => k === "data-map",
    matches: () => false,
    querySelector: () => null,
    querySelectorAll: (sel) => (sel === "path[data-x]" ? paths : []),
  };
}
{
  const win = { requestAnimationFrame: () => {}, performance: { now: () => 0 }, matchMedia: () => ({ matches: false }) };
  const { geom, tween, lerpRgb } = animKit(win);
  let raf = null;
  win.requestAnimationFrame = (fn) => (raf = fn);
  const run = (t) => {
    const fn = raf;
    raf = null;
    fn && fn(t);
  };

  const oldSvg = fakeMapSvg([fakePath("A", "#000000")]);
  const old = geom(oldSvg);
  const newSvg = fakeMapSvg([fakePath("A", "#ffffff")]);
  tween(newSvg, old, 300);
  const path = newSvg.querySelectorAll("path[data-x]")[0];
  assert.equal(path.getAttribute("fill"), "#000000"); // set(0): starts at the old colour
  const eased = 1 - Math.pow(1 - 0.5, 3); // play()'s own ease-out, at half the duration
  run(150);
  assert.equal(path.getAttribute("fill"), lerpRgb([0, 0, 0], [255, 255, 255], eased));
  run(300); // the exact end state
  assert.equal(path.getAttribute("fill"), "#ffffff");
}
// A pattern end (the "no data" hatch, #206) swaps at the very end — no lerp,
// so the old solid colour holds all the way through the tween.
{
  const win = { requestAnimationFrame: () => {}, performance: { now: () => 0 }, matchMedia: () => ({ matches: false }) };
  const { geom, tween } = animKit(win);
  let raf = null;
  win.requestAnimationFrame = (fn) => (raf = fn);
  const run = (t) => {
    const fn = raf;
    raf = null;
    fn && fn(t);
  };

  const oldSvg = fakeMapSvg([fakePath("A", "#0a0a0a")]);
  const old = geom(oldSvg);
  const newSvg = fakeMapSvg([fakePath("A", "url(#hatch)")]);
  tween(newSvg, old, 300);
  const path = newSvg.querySelectorAll("path[data-x]")[0];
  assert.equal(path.getAttribute("fill"), "#0a0a0a");
  run(150);
  assert.equal(path.getAttribute("fill"), "#0a0a0a"); // still the old solid colour mid-tween
  run(300);
  assert.equal(path.getAttribute("fill"), "url(#hatch)"); // swaps only at the end
}

console.log("anim_test: ok");
