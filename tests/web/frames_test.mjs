// Node test for web/anim.js's ::FRAME player (M2f-A, #149), run by
// tests/web_js.rs. Pure helpers (lerpRgb/parseHex) are checked directly; the
// player's control flow (clamping, coalescing, Play) is checked against a
// hand-rolled fake DOM — this repo's tests/web convention is no jsdom, and a
// `data-map` fixture isn't needed here since the generic stepping logic
// (what frame is drawn, when, from which button) doesn't depend on the
// per-mark colour/geometry tween exercised separately in anim_test.mjs.
import assert from "assert/strict";
import { animKit } from "../../web/anim.js";

// ---- a minimal fake element/document/window, just enough for frames() ----
function fakeEl() {
  const attrs = {};
  const listeners = {};
  return {
    className: "",
    textContent: "",
    value: "",
    isConnected: true,
    children: [],
    setAttribute(k, v) {
      attrs[k] = String(v);
    },
    getAttribute(k) {
      return k in attrs ? attrs[k] : null;
    },
    hasAttribute(k) {
      return k in attrs;
    },
    removeAttribute(k) {
      delete attrs[k];
    },
    append(...els) {
      this.children.push(...els);
    },
    after() {},
    querySelectorAll() {
      return [];
    },
    querySelector() {
      return null;
    },
    matches() {
      return false;
    },
    addEventListener(type, fn) {
      listeners[type] = fn;
    },
    fire(type, ev) {
      listeners[type] && listeners[type](ev);
    },
    byClass(cls) {
      return this.children.find((c) => (c.className || "").includes(cls));
    },
  };
}

function fakeWin({ reducedMotion = false } = {}) {
  let raf = null;
  let rafCalls = 0;
  let timerFn = null;
  let timerMs = null;
  const docListeners = {};
  const doc = {
    hidden: false,
    createElement: () => fakeEl(),
    addEventListener(type, fn) {
      docListeners[type] = fn;
    },
    fireVisibility() {
      docListeners.visibilitychange && docListeners.visibilitychange();
    },
  };
  const win = {
    document: doc,
    matchMedia: () => ({ matches: reducedMotion }),
    requestAnimationFrame: (fn) => {
      raf = fn;
      rafCalls++;
      return rafCalls; // must be truthy, like a real browser's id, for frames()'s own "already scheduled" guard
    },
    setInterval: (fn, ms) => {
      timerFn = fn;
      timerMs = ms;
      return 1;
    },
    clearInterval: () => {
      timerFn = null;
    },
    performance: { now: () => 0 },
    runRaf: () => {
      const fn = raf;
      raf = null;
      if (fn) fn(0);
    },
    tick: () => timerFn && timerFn(),
    rafCalls: () => rafCalls,
    timerMs: () => timerMs,
    timerRunning: () => !!timerFn,
  };
  return win;
}

function fixture(win, labels) {
  const svg = fakeEl();
  svg.setAttribute("data-frames", JSON.stringify(labels));
  svg.setAttribute("data-frame", String(labels.length - 1));
  svg.setAttribute("data-frame-name", "Week");
  const holder = fakeEl();
  let bar = null;
  holder.after = (el) => (bar = el);
  svg.parentElement = holder;
  const panel = fakeEl();
  return { svg, panel, bar: () => bar };
}

// ---- lerpRgb / parseHex (pure, D24) ---------------------------------------
{
  const { lerpRgb, parseHex } = animKit({ document: {} });
  assert.deepEqual(parseHex("#000000"), [0, 0, 0]);
  assert.deepEqual(parseHex("#ffffff"), [255, 255, 255]);
  assert.equal(parseHex("url(#hatch)"), null);
  assert.equal(parseHex("none"), null);
  assert.equal(parseHex("red"), null);
  assert.equal(lerpRgb([0, 0, 0], [255, 255, 255], 0), "#000000");
  assert.equal(lerpRgb([0, 0, 0], [255, 255, 255], 1), "#ffffff");
  assert.equal(lerpRgb([0, 0, 0], [255, 255, 255], 0.5), "#808080");
  assert.equal(lerpRgb([10, 20, 30], [10, 20, 30], 0.5), "#0a141e"); // unchanged ends: stays put
}

// ---- start index clamps to the label range --------------------------------
{
  const win = fakeWin();
  const { frames } = animKit(win);
  const { svg, panel } = fixture(win, ["W1", "W2", "W3"]);
  let drawn = [];
  const draw = (k) => (drawn.push(k), svg);
  const ctl = frames(panel, svg, 99, draw); // way past the end
  assert.equal(ctl.step(), 2); // clamped to the last label
  assert.deepEqual(drawn, []); // already what the svg shows: no jump needed
}

// ---- …unless the persisted step really differs from what's on screen ------
{
  const win = fakeWin();
  const { frames } = animKit(win);
  const { svg, panel } = fixture(win, ["W1", "W2", "W3"]); // data-frame = "2" (the last)
  let drawn = [];
  const draw = (k) => (drawn.push(k), svg);
  const ctl = frames(panel, svg, 0, draw); // a re-run's persisted step: back to the first frame
  assert.equal(ctl.step(), 0);
  assert.deepEqual(drawn, [0]); // jumped once, instantly (no rAF/tween involved)
}

// ---- coalescing: three show() calls in one tick → one draw, the last k ---
{
  const win = fakeWin();
  const { frames } = animKit(win);
  const { svg, panel } = fixture(win, ["W1", "W2", "W3", "W4"]);
  let drawn = [];
  const draw = (k) => (drawn.push(k), svg);
  const ctl = frames(panel, svg, 3, draw); // starts at the last label, already shown: no initial draw
  assert.deepEqual(drawn, []);
  ctl.show(0);
  ctl.show(1);
  ctl.show(2); // scrubbing fast: only the latest value should ever draw
  assert.equal(win.rafCalls(), 1); // one rAF scheduled for the whole burst
  win.runRaf();
  assert.deepEqual(drawn, [2]); // exactly one draw, at the last requested frame
  assert.equal(ctl.step(), 2);
}

// ---- Play advances, then stops at the last frame (no autoplay beyond it) -
{
  const win = fakeWin();
  const { frames } = animKit(win);
  const { svg, panel, bar } = fixture(win, ["W1", "W2", "W3"]);
  let drawn = [];
  const draw = (k) => (drawn.push(k), svg);
  const ctl = frames(panel, svg, 0, draw); // data-frame starts at "2": the initial jump to 0 draws once
  drawn = [];
  const playBtn = bar().byClass("dp-frames-play");
  playBtn.onclick(); // Play: jumps to frame 1 (from 0) at once, then starts the timer
  win.runRaf();
  assert.deepEqual(drawn, [1]);
  assert.equal(playBtn.getAttribute("aria-pressed"), "true");
  assert.equal(win.timerRunning(), true);
  assert.equal(win.timerMs(), 1000); // no prefers-reduced-motion here
  win.tick(); // → frame 2, the last label
  win.runRaf();
  assert.deepEqual(drawn, [1, 2]);
  assert.equal(win.timerRunning(), true); // still at the last frame, Play keeps running
  win.tick(); // the next tick sees k at the end already: stop, don't wrap to 0
  assert.equal(win.timerRunning(), false);
  assert.equal(playBtn.getAttribute("aria-pressed"), "false");
  assert.deepEqual(drawn, [1, 2]); // no further draw past the last frame
  assert.equal(ctl.step(), 2);
  // Pressing Play again, sitting at the last frame, restarts from the top.
  playBtn.onclick();
  win.runRaf();
  assert.deepEqual(drawn, [1, 2, 0]);
}

// ---- aria-live is off while playing (not flooded), back to polite on Pause
{
  const win = fakeWin();
  const { frames } = animKit(win);
  const { svg, panel, bar } = fixture(win, ["W1", "W2"]);
  frames(panel, svg, 1, () => svg);
  const pos = bar().children[4];
  assert.equal(pos.getAttribute("aria-live"), "polite");
  bar()
    .byClass("dp-frames-play")
    .onclick();
  assert.equal(pos.getAttribute("aria-live"), "off");
  bar()
    .byClass("dp-frames-play")
    .onclick(); // Pause
  assert.equal(pos.getAttribute("aria-live"), "polite");
}

// ---- reduced motion: Play stays (D14), steps 1.5s apart --------------------
{
  const win = fakeWin({ reducedMotion: true });
  const { frames } = animKit(win);
  const { svg, panel, bar } = fixture(win, ["W1", "W2"]);
  frames(panel, svg, 1, () => svg);
  bar()
    .byClass("dp-frames-play")
    .onclick();
  assert.equal(win.timerMs(), 1500);
}

// ---- the tab/view going hidden stops Play ----------------------------------
{
  const win = fakeWin();
  const { frames } = animKit(win);
  const { svg, panel, bar } = fixture(win, ["W1", "W2", "W3"]);
  frames(panel, svg, 0, () => svg);
  bar()
    .byClass("dp-frames-play")
    .onclick();
  assert.equal(win.timerRunning(), true);
  win.document.hidden = true;
  win.document.fireVisibility();
  assert.equal(win.timerRunning(), false);
}

// ---- #664 ::SYNC_FRAME: onStep on own steps, follow() by label, quietly ---
{
  const win = fakeWin();
  const { frames } = animKit(win);
  const a = fixture(win, ["W1", "W2", "W3"]);
  const b = fixture(win, ["W2", "W3"]); // a map missing W1
  const told = [];
  const ctlA = frames(a.panel, a.svg, 2, () => a.svg, (label) => told.push(label));
  let drawnB = [];
  const ctlB = frames(b.panel, b.svg, 1, (k) => (drawnB.push(k), b.svg));
  ctlA.show(1); // a step of A tells the group its label
  assert.deepEqual(told, ["W2"]);
  ctlB.follow("W2"); // B follows by label (index 0 in B)
  win.runRaf();
  assert.deepEqual(drawnB, [0]);
  assert.equal(ctlB.label(), "W2");
  ctlB.follow("W1"); // a label B doesn't have: stays put
  win.runRaf();
  assert.deepEqual(drawnB, [0]);
  told.length = 0;
  ctlA.follow("W3"); // a follow never re-broadcasts
  win.runRaf();
  assert.deepEqual(told, []);
  assert.equal(ctlA.step(), 2);
  // a following map stops its own Play
  const playB = b.bar().byClass("dp-frames-play");
  ctlB.show(0);
  win.runRaf();
  playB.onclick();
  assert.equal(win.timerRunning(), true);
  ctlB.follow("W2");
  assert.equal(playB.getAttribute("aria-pressed"), "false");
}

console.log("frames_test: ok");
