// Drill motion check in a real browser (the "Selections & drill" example):
// a drill-down click, a breadcrumb back-up and a drill-through click must each
// feel like one continuous move. Per action it records every frame (rAF) and
// DOM change and fails on
//   - layout shift (CLS, every layout-shift entry — the click's own run counts)
//   - a panel collapsing to a skeleton, or the drilled panel shrinking
//   - a scroll jump
//   - the entrance animation replaying (.dp-in, clip reveals)
//   - panels the action does not touch being rebuilt (their DOM must stay)
//   - focus lost to <body>
//   - more than one run per click, a frame without the panels (white flash)
//   - the run (click → data on screen) above --bound ms, or the motion after
//     it (run end → stable) above --settle ms: the drill animations are
//     ≤ 400 ms, so a slow settle is the page, not the query or machine load
// and prints the numbers. --frames DIR saves a frame strip per action (CDP
// screencast, montage'd when ImageMagick is on the PATH); --axe runs axe-core
// on the drilled chart.
//
//   python3 serve_nocache.py 8791 &
//   NODE_PATH=<dir with playwright>/node_modules node scripts/drill-e2e.mjs \
//       [--url http://127.0.0.1:8791/] [--motion reduce] [--frames dir] [--bound 1500] [--settle 900] [--width 390] [--axe axe.min.js]
import { createRequire } from "module";
import { existsSync, mkdirSync, writeFileSync } from "fs";
import { execFileSync } from "child_process";
import path from "path";
import os from "os";

const require = createRequire(import.meta.url);
const { chromium } = require("playwright");
const arg = (k, d) => {
  const i = process.argv.indexOf(k);
  return i > 0 ? process.argv[i + 1] : d;
};
const base = arg("--url", "http://127.0.0.1:8791/");
const motion = arg("--motion", "no-preference");
const frames = arg("--frames", "");
// The run time is the query + render: on a machine busier than its cores
// (load average / cores > 1) it stretches with the queue, so its bound does
// too — printed with every run. The settle bound does not scale.
const load = os.loadavg()[0] / Math.max(1, os.cpus().length);
const bound = Math.round(Number(arg("--bound", "1500")) * Math.max(1, load));
console.log(`run bound ${bound} ms (load ${load.toFixed(2)} per core)`);
const settleBound = Number(arg("--settle", "900"));
const axe = arg("--axe", "");
const fail = (m) => {
  console.error("FAIL: " + m);
  process.exitCode = 1;
};

const browser = await chromium.launch();
// --width 390: a phone, where charts are redrawn at their shown width (fitCharts)
const page = await browser.newPage({ viewport: { width: Number(arg("--width", "1280")), height: 640 }, reducedMotion: motion });
const errs = [];
page.on("pageerror", (e) => errs.push(String(e)));
await page.addInitScript(() => {
  // Collected for the whole page life; each action reads the slice after its start.
  window.__shifts = [];
  window.__long = [];
  try {
    new PerformanceObserver((l) => l.getEntries().forEach((e) => window.__shifts.push({ t: e.startTime, v: e.value, src: (e.sources || []).map((s) => s.node?.className?.baseVal ?? s.node?.className ?? s.node?.nodeName ?? "") }))).observe({ type: "layout-shift", buffered: true });
    new PerformanceObserver((l) => l.getEntries().forEach((e) => window.__long.push({ t: e.startTime, d: e.duration }))).observe({ type: "longtask", buffered: true });
  } catch (_) {}
});
await page.goto(`${base}?embed=1&dashboard=${encodeURIComponent("Selections & drill")}&tab=${encodeURIComponent("Drill-down")}`);
const settled = () =>
  page.waitForFunction(
    () => /\d+ panels?/.test(document.getElementById("status")?.textContent || "") && !document.body.classList.contains("loading"),
    null,
    { timeout: 120000 }
  );
await settled();
await page.waitForFunction(() => document.querySelector(".dash .dp-crumbs"), null, { timeout: 60000 });
await page.waitForTimeout(1200); // the entrance animation of the first load

let cdp = null;
let shots = [];
if (frames) {
  mkdirSync(frames, { recursive: true });
  cdp = await page.context().newCDPSession(page);
  cdp.on("Page.screencastFrame", (f) => {
    shots.push({ t: f.metadata.timestamp, data: f.data });
    cdp.send("Page.screencastFrameAck", { sessionId: f.sessionId }).catch(() => {});
  });
}

// Start the in-page recorder: one sample per animation frame + a mutation log.
// `keep` = panels the action must not rebuild, `watch` = the panel whose height
// must hold.
async function record(tabSwitch = false) {
  await page.evaluate((tabSwitch) => {
    const R = (window.__rec = { t0: performance.now(), frames: [], muts: [], runs: 0, lastMut: 0, entrance: 0, stop: false, tabSwitch });
    const visPanels = () => [...document.querySelectorAll(".dash .panel")].filter((p) => p.offsetParent && p.getBoundingClientRect().height > 0);
    let n = 0;
    for (const p of document.querySelectorAll(".dash .panel")) p.__id = ++n;
    R.before = visPanels().map((p) => p.__id);
    R.watch = document.querySelector(".dash .dp-crumbs")?.closest(".panel") || null;
    R.watchH = R.watch ? R.watch.getBoundingClientRect().height : 0;
    R.y0 = scrollY;
    const mo = new MutationObserver((list) => {
      const t = performance.now() - R.t0;
      for (const m of list) {
        if (m.target.id === "dp-tip" || m.target.closest?.("#dp-tip")) continue;
        if (m.type === "attributes" && m.target === document.body && m.attributeName === "class") {
          if (document.body.classList.contains("loading") && !m.oldValue?.includes("loading")) R.runs++;
          else if (!document.body.classList.contains("loading") && m.oldValue?.includes("loading")) R.runEnd = performance.now() - R.t0;
          continue;
        }
        if (R.tabSwitch && !R.switchAt) {
          if (m.type === "attributes" && m.attributeName === "class" && m.target.classList?.contains("tab-btn") && m.target.classList.contains("active")) R.switchAt = performance.now();
          for (const a of m.addedNodes || []) if (a.nodeType === 1 && a.matches?.(".tabbar")) R.switchAt = performance.now();
        }
        if (m.type === "attributes" && m.attributeName === "class" && m.target.classList?.contains("dp-in") && !String(m.oldValue || "").includes("dp-in")) R.entrance++;
        for (const a of m.addedNodes || []) if (a.nodeType === 1 && (a.matches?.("clipPath.dp-clip") || a.querySelector?.("clipPath.dp-clip"))) R.entrance++;
        if (m.type !== "attributes" || !["style", "fill-opacity", "opacity", "points", "x", "y", "width", "height", "transform"].includes(m.attributeName)) R.lastMut = t;
        R.muts.push([Math.round(t), m.type, m.attributeName || "", m.target.nodeName, (m.addedNodes || []).length, (m.removedNodes || []).length]);
      }
    });
    mo.observe(document.body, { subtree: true, childList: true, attributes: true, attributeOldValue: true });
    R.mo = mo;
    const tick = () => {
      if (R.stop) return;
      const vis = visPanels();
      R.frames.push({
        t: performance.now() - R.t0,
        y: scrollY,
        skel: document.querySelectorAll(".dash .dp-skel").length,
        pending: document.querySelectorAll(".dash .dp-pending").length,
        vis: vis.length,
        watchH: R.watch && R.watch.isConnected ? R.watch.getBoundingClientRect().height : (document.querySelector(".dash .dp-crumbs")?.closest(".panel")?.getBoundingClientRect().height ?? 0),
        focus: document.activeElement === document.body ? "body" : (document.activeElement?.nodeName || "") + "." + (document.activeElement?.getAttribute?.("class") || ""),
      });
      requestAnimationFrame(tick);
    };
    requestAnimationFrame(tick);
  }, tabSwitch);
  if (cdp) {
    shots = [];
    await cdp.send("Page.startScreencast", { format: "jpeg", quality: 60, everyNthFrame: 1 });
  }
}

// Wait until the run is done, no DOM change for 400 ms and no animation runs.
async function stable(label, opts = {}) {
  await page.waitForFunction(
    () => {
      const R = window.__rec;
      const now = performance.now() - R.t0;
      const anims = document.getAnimations().filter((a) => a.playState === "running" && a.effect?.target?.closest?.(".dash") && !/dp-shimmer|dp-pulse/.test(a.animationName || ""));
      if (anims.length) R.lastMut = Math.max(R.lastMut, now);
      return R.runs > 0 && !document.body.classList.contains("loading") && now - R.lastMut > 400;
    },
    null,
    { timeout: 60000, polling: 50 }
  );
  if (cdp) await cdp.send("Page.stopScreencast");
  const r = await page.evaluate(() => {
    const R = window.__rec;
    R.stop = true;
    R.mo.disconnect();
    const t0abs = R.t0;
    // (the tooltip is position:fixed: its moves are no layout shift of the page)
    const shifts = window.__shifts.filter((s) => s.t >= t0abs && !s.src.every((c) => /dp-tip/.test(c)));
    const long = window.__long.filter((s) => s.t >= t0abs);
    const vis = [...document.querySelectorAll(".dash .panel")].filter((p) => p.offsetParent);
    const kept = R.before.filter((id) => vis.some((p) => p.__id === id));
    const focus = document.activeElement;
    // A tab switch moves everything below the tabs by design (like any tab
    // click): count the shifts after its first frame.
    const after = R.switchAt ? shifts.filter((s) => s.t > R.switchAt + 34) : shifts;
    return {
      cls: after.reduce((a, s) => a + s.v, 0),
      switchMs: R.switchAt ? Math.round(R.switchAt - R.t0) : null,
      shiftSrc: [...new Set(shifts.flatMap((s) => s.src))].slice(0, 6),
      long: long.length,
      longMs: Math.round(long.reduce((a, s) => a + s.d, 0)),
      runs: R.runs,
      stableMs: Math.round(R.lastMut),
      runMs: Math.round(R.runEnd ?? R.lastMut),
      settleMs: Math.round(Math.max(0, R.lastMut - (R.runEnd ?? R.lastMut))),
      skel: Math.max(0, ...R.frames.map((f) => f.skel)),
      skelMs: (() => {
        const on = R.frames.filter((f) => f.skel);
        return on.length ? Math.round(on.at(-1).t - on[0].t) : 0;
      })(),
      pendingFrames: R.frames.filter((f) => f.pending).length,
      blankFrames: R.frames.filter((f) => f.vis === 0).length,
      frames: R.frames.length,
      minWatchRatio: R.watchH ? Math.min(...R.frames.map((f) => f.watchH)) / R.watchH : 1,
      scrollJump: Math.max(0, ...R.frames.map((f) => Math.abs(f.y - R.y0))),
      entrance: R.entrance,
      before: R.before.length,
      kept: kept.length,
      lostFocusFrames: R.frames.filter((f) => f.focus === "body").length,
      focus: focus === document.body ? "body" : `${focus.nodeName.toLowerCase()}${focus.getAttribute("class") ? "." + focus.getAttribute("class").split(" ")[0] : ""} "${(focus.getAttribute("aria-label") || focus.textContent || "").slice(0, 40)}"`,
      focusInDrill: !!focus.closest?.(".panel")?.querySelector(".dp-crumbs") || !!focus.closest?.(".dp-crumbs"),
      muts: R.muts.length,
    };
  });
  if (cdp && shots.length) {
    const dir = path.join(frames, label);
    mkdirSync(dir, { recursive: true });
    const t0 = shots[0].t;
    const names = [];
    shots.forEach((s, k) => {
      const f = path.join(dir, `${String(k).padStart(3, "0")}_${Math.round((s.t - t0) * 1000)}ms.jpg`);
      writeFileSync(f, Buffer.from(s.data, "base64"));
      names.push(f);
    });
    try {
      const pick = names.filter((_, k) => k % Math.max(1, Math.ceil(names.length / 12)) === 0).slice(0, 12);
      execFileSync("montage", [...pick, "-tile", "4x", "-geometry", "480x+4+4", "-label", "%t", path.join(frames, `${label}-strip.jpg`)]);
    } catch (_) {}
    r.shots = shots.length;
  }
  console.log(`${label}: ${JSON.stringify(r)}`);
  return r;
}

function check(label, r, { keepAll = true, focus = true } = {}) {
  if (r.cls >= 0.01) fail(`${label}: layout shift ${r.cls.toFixed(4)} (${r.shiftSrc.join(", ")})`);
  if (r.skel) fail(`${label}: a panel collapsed to a skeleton`);
  if (r.minWatchRatio < 0.98) fail(`${label}: the drilled panel shrank to ${(r.minWatchRatio * 100).toFixed(0)}% of its height`);
  if (r.scrollJump > 2) fail(`${label}: the page scrolled by ${r.scrollJump}px`);
  if (r.entrance) fail(`${label}: the entrance animation replayed (${r.entrance}×)`);
  if (r.runs !== 1) fail(`${label}: ${r.runs} runs for one click`);
  if (r.blankFrames) fail(`${label}: ${r.blankFrames} frame(s) without any panel`);
  if (keepAll && r.kept < r.before - 2) fail(`${label}: ${r.before - r.kept} of ${r.before} panels rebuilt (only the drilled chart and its follower may change)`);
  if (focus && (r.focus === "body" || !r.focusInDrill)) fail(`${label}: focus lost (${r.focus})`);
  if (r.runMs > bound) fail(`${label}: the run took ${r.runMs} ms (bound ${bound} ms)`);
  if (r.settleMs > settleBound) fail(`${label}: still moving ${r.settleMs} ms after the run (bound ${settleBound} ms)`);
}

// The drill chart, scrolled a little down so a scroll jump shows.
await page.evaluate(() => {
  const p = document.querySelector(".dash .dp-crumbs").closest(".panel");
  window.scrollTo(0, Math.max(0, p.getBoundingClientRect().top + scrollY - 80));
});
await page.waitForTimeout(200);
const markAt = (sel, k = 0) =>
  page.evaluate(
    ([sel, k]) => {
      const m = [...document.querySelector(".dash .dp-crumbs").closest(".panel").querySelectorAll(sel)].filter((e) => e.getBoundingClientRect().width > 0)[k];
      const b = m.getBoundingClientRect();
      return { x: b.left + b.width / 2, y: b.top + Math.min(b.height / 2, 20), x0: m.getAttribute("data-x") };
    },
    [sel, k]
  );

// (a) drill-down: click the first region bar.
const bar = await markAt(".panel-svg > svg .dp-hit");
await record();
await page.mouse.click(bar.x, bar.y);
const a = await stable("drill-down");
const crumbs = await page.$eval(".dash .dp-crumbs", (n) => n.textContent);
if (!crumbs.includes(bar.x0)) fail(`drill-down: the breadcrumb does not show ${bar.x0} (${crumbs})`);
const drilled = await page.evaluate(() => new URL(location.href).searchParams.get("v.drill_path"));
if (drilled !== bar.x0) fail(`drill-down: v.drill_path=${drilled}, expected ${bar.x0}`);
check("drill-down", a);

// (a2) keyboard drill one level deeper: Enter on the focused mark.
await page.evaluate(() => {
  const svg = document.querySelector(".dash .dp-crumbs").closest(".panel").querySelector(".panel-svg > svg");
  svg.querySelector(".dp-hit").focus();
});
await record();
await page.keyboard.press("Enter");
const a2 = await stable("drill-down-key");
check("drill-down-key", a2);

// The drilled chart and its breadcrumb pass axe.
if (axe && existsSync(axe)) {
  await page.addScriptTag({ path: axe });
  const res = await page.evaluate(() =>
    window.axe.run(".dash", { runOnly: { type: "tag", values: ["wcag2a", "wcag2aa"] }, rules: { "color-contrast": { enabled: false } } })
  );
  for (const v of res.violations) console.log(`axe ${v.impact}: ${v.id} (${v.nodes.length}) ${v.help}`);
  const bad = res.violations.filter((v) => v.impact === "serious" || v.impact === "critical");
  if (bad.length) fail(`axe: ${bad.map((v) => v.id).join(", ")}`);
}

// A focused chart keeps the focus across a re-run on both swap paths: kept
// (a click on the last level only cross-filters `selected`, which the chart
// does not read: its DOM stays) and replaced (a drill redraws it).
const focusKept = async (label) => {
  const r = await page.evaluate(() => {
    const ae = document.activeElement;
    return { onChart: !!ae?.matches?.(".panel-svg > svg") && ae.isConnected && !!ae.closest(".panel")?.querySelector(".dp-crumbs"), same: ae === window.__focusSvg };
  });
  if (!r.onChart) fail(`${label}: the focused chart lost the focus`);
  return r;
};
await page.evaluate(() => {
  const svg = document.querySelector(".dash .dp-crumbs").closest(".panel").querySelector(".panel-svg > svg");
  window.__focusSvg = svg;
  svg.focus();
  svg.querySelector(".dp-hit").dispatchEvent(new MouseEvent("click", { bubbles: true }));
});
await settled();
await page.waitForTimeout(300);
const kept = await focusKept("kept panel");
if (!kept.same) fail("kept panel: a re-run that does not change the chart rebuilt it");
await page.evaluate(() => document.activeElement.querySelector(".dp-hit").dispatchEvent(new MouseEvent("click", { bubbles: true }))); // clear
await settled();
await page.waitForTimeout(300);

// (b) back up with the breadcrumb ("All").
const all = await page.$eval(".dash .dp-crumbs .dp-crumb", (b) => {
  const r = b.getBoundingClientRect();
  return { x: r.left + r.width / 2, y: r.top + r.height / 2 };
});
await record();
await page.mouse.click(all.x, all.y);
const b = await stable("back-up");
check("back-up", b);
// Replaced: focus the chart, drill by a click that does not move the focus.
await page.evaluate(() => {
  const svg = document.querySelector(".dash .dp-crumbs").closest(".panel").querySelector(".panel-svg > svg");
  window.__focusSvg = svg;
  svg.focus();
  svg.querySelector(".dp-hit").dispatchEvent(new MouseEvent("click", { bubbles: true }));
});
await settled();
await page.waitForTimeout(300);
const repl = await focusKept("replaced panel");
if (repl.same) fail("replaced panel: the drill did not redraw the chart");
await page.$eval(".dash .dp-crumbs .dp-crumb", (x) => x.click()); // back to the top
await settled();
await page.waitForTimeout(300);
const path0 = await page.evaluate(() => new URL(location.href).searchParams.get("v.drill_path"));
if (path0) fail(`back-up: the URL still has v.drill_path=${path0}`);

// (c) drill-through: on the Drill-through tab, click a city bar (#City detail?v.pick_city=…).
await page.click(".dash .tabbar .tab-btn:has-text('Drill-through')");
await page.waitForTimeout(600);
await page.evaluate(() => window.scrollTo(0, 60));
await page.waitForTimeout(200);
const city = await page.evaluate(() => {
  const pane = [...document.querySelectorAll(".dash .tabpane")].find((p) => p.style.display !== "none");
  // not the dropdown's default (Berlin): the link must change the city
  const m = [...pane.querySelectorAll(".panel.dp-linked .panel-svg > svg .dp-hit")].find((e) => e.getAttribute("data-x") !== "Berlin");
  const r = m.getBoundingClientRect();
  return { x: r.left + r.width / 2, y: r.top + Math.min(r.height / 2, 20), name: m.getAttribute("data-x") };
});
await record(true);
await page.mouse.click(city.x, city.y);
const c = await stable("drill-through");
const after = await page.evaluate(() => ({
  tab: document.querySelector(".dash .tabbar .tab-btn.active")?.textContent,
  pick: new URL(location.href).searchParams.get("v.pick_city"),
  focus: document.activeElement?.textContent?.slice(0, 30),
}));
if (after.tab !== "City detail") fail(`drill-through: the active tab is ${after.tab}`);
if (after.pick !== city.name) fail(`drill-through: v.pick_city=${after.pick}, expected ${city.name}`);
// A different tab: the panels on screen are all new; focus lands on its
// first panel title.
check("drill-through", c, { keepAll: false, focus: false });
const onTitle = () =>
  page.evaluate(() => {
    const ae = document.activeElement;
    const pane = [...document.querySelectorAll(".dash > .tabwrap > .tabpane")].find((p) => p.style.display !== "none");
    return pane?.contains(ae) && ae.matches(".panel-title, .group-title") ? ae.textContent : "";
  });
if (!(await onTitle())) fail(`drill-through: focus not on the target tab's title (${c.focus})`);

// (c2) keyboard drill-through: Enter on a city mark opens the tab with the
// focus on its title; Esc goes back to the mark.
await page.click(".dash .tabbar .tab-btn:has-text('Drill-through')");
await page.waitForTimeout(400);
const origin = await page.evaluate(() => {
  const pane = [...document.querySelectorAll(".dash > .tabwrap > .tabpane")].find((p) => p.style.display !== "none");
  const m = [...pane.querySelectorAll(".panel.dp-linked .panel-svg > svg .dp-hit")].find((e) => !["Berlin", "Lyon"].includes(e.getAttribute("data-x")));
  m.focus();
  return m.getAttribute("aria-label");
});
await record(true);
await page.keyboard.press("Enter");
const c2 = await stable("drill-through-key");
check("drill-through-key", c2, { keepAll: false, focus: false });
const t2 = await onTitle();
if (!t2) fail(`drill-through-key: focus not on the target tab's title (${c2.focus})`);
await page.keyboard.press("Escape");
await page.waitForTimeout(300);
const ret = await page.evaluate(() => ({ tab: document.querySelector(".dash > .tabbar > .tab-btn.active")?.textContent, label: document.activeElement.getAttribute("aria-label") }));
console.log(`drill-through-key: focus "${t2}", Esc → ${ret.tab} / ${ret.label}`);
if (ret.tab !== "Drill-through" || ret.label !== origin) fail(`drill-through-key: Esc did not return to the mark (${ret.tab}, ${ret.label})`);

if (errs.length) fail("page errors: " + errs.join(" | "));
await browser.close();
console.log(process.exitCode ? "drill-e2e: FAILED" : "drill-e2e: ok");
