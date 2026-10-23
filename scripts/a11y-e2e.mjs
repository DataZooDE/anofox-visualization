// Keyboard + screen-reader check of the charts in a real browser (issue #37):
// Tab into a chart, walk its marks with the arrow keys, Enter selects (the
// dashboard re-runs and the focus comes back), Esc leaves; every chart SVG has
// a summary; "Charts as tables" swaps each chart for a table. With axe-core on
// disk (--axe path/to/axe.min.js) it also runs axe on the dashboard, on the
// sidebar's examples library and on Explore data (a table's cards, a chart preview, a query result). A
// ::STORY chart's first step must light some marks, dim the rest and draw
// its connector.
//
// #244: the assistant panel's own a11y (the key/sent/diff dialogs, the
// phone sheet, the preview bar) lives in scripts/assist-e2e.mjs, not here —
// --assist <pkg dir> spawns it (--only a11y,phone) as a subprocess.
//
//   python3 serve_nocache.py 8791 &
//   NODE_PATH=<dir with playwright>/node_modules node scripts/a11y-e2e.mjs \
//       [--url http://127.0.0.1:8791/] [--dashboard "Selections & drill"] [--axe axe.min.js]
//       [--sql file.sql]   (a dashboard from a file, passed in the #sql hash)
//       [--assist <wasm pkg dir>]   (also runs the assistant's a11y, scripts/assist-e2e.mjs)
import { createRequire } from "module";
import { existsSync, readFileSync } from "fs";
import { spawnSync } from "child_process";
import path from "path";
import { fileURLToPath } from "url";

const require = createRequire(import.meta.url);
const { chromium } = require("playwright");
const arg = (k, d) => {
  const i = process.argv.indexOf(k);
  return i > 0 ? process.argv[i + 1] : d;
};
const base = arg("--url", "http://127.0.0.1:8791/");
const dash = arg("--dashboard", "Overview");
const axe = arg("--axe", "");
const sqlFile = arg("--sql", "");
const assistPkg = arg("--assist", "");
const fail = (m) => {
  console.error("FAIL: " + m);
  process.exitCode = 1;
};

if (assistPkg) {
  const here = path.dirname(fileURLToPath(import.meta.url));
  const assistArgs = ["scripts/assist-e2e.mjs", "--pkg", assistPkg, "--only", "a11y,phone"];
  if (axe) assistArgs.push("--axe", axe);
  const r = spawnSync(process.execPath, assistArgs, { cwd: path.resolve(here, ".."), stdio: "inherit", env: process.env });
  if (r.status !== 0) fail("the assistant's own a11y (scripts/assist-e2e.mjs --only a11y,phone) failed");
}

const browser = await chromium.launch();
const page = await browser.newPage({ viewport: { width: 1400, height: 1000 }, reducedMotion: "reduce" });
const errs = [];
page.on("pageerror", (e) => errs.push(String(e)));
if (sqlFile) await page.goto(`${base}?embed=1#sql=${Buffer.from(readFileSync(sqlFile, "utf8"), "utf8").toString("base64")}`);
else await page.goto(`${base}?embed=1&dashboard=${encodeURIComponent(dash)}`);
// Loaded: a chart with its summary, or (a dashboard without charts) panels and
// no loading skeletons left.
await page.waitForFunction(
  () =>
    document.querySelector(".dash .panel-svg > svg[aria-label]") ||
    (document.querySelector(".dash .panel") && !document.querySelector(".dash .dp-skel")),
  null,
  { timeout: 120000 }
);
await page.waitForTimeout(500);
// No run in flight (a re-run replaces every panel when it lands).
const idle = () => page.waitForFunction(() => !document.body.classList.contains("loading"), null, { timeout: 120000 });

// Every chart has a one-sentence summary (decorative overlays such as the
// ::STORY connector are aria-hidden and skipped).
const svgs = await page.$$eval(".dash .panel-svg > svg:not([aria-hidden=\"true\"])", (s) => s.map((e) => [e.getAttribute("role"), e.getAttribute("aria-label") || ""]));
console.log(`${svgs.length} charts`);
for (const [role, label] of svgs) {
  if (!role || !/\.$/.test(label)) fail(`chart without a summary: role=${role} label=${label}`);
}
console.log("summary:", svgs[0]?.[1]);

// Each ::STORY chart's first step lights the marks at its key (some lit, the
// rest dimmed) and draws the connector; then back to the overview. Only
// charts on screen: one on a hidden tab has no layout to draw a connector in.
const stories = await page.$$eval(".dash .panel", (ps) =>
  ps.flatMap((p, i) => {
    if (!p.querySelector(".dp-story-next") || !p.offsetWidth) return [];
    p.querySelector(".dp-story-next").click();
    const svg = p.querySelector(".panel-svg > svg[data-story]");
    const marks = [...svg.querySelectorAll("[data-x]")].filter((e) => !e.hasAttribute("data-rule") && !e.closest(".dp-ghost"));
    const lit = marks.filter((e) => !e.style.opacity);
    const out = {
      i,
      key: JSON.parse(svg.getAttribute("data-story"))[0].k,
      lit: [...new Set(lit.map((e) => e.getAttribute("data-x")))].slice(0, 4),
      dimmed: marks.length - lit.length,
      link: !!p.querySelector(".dp-story-link path"),
    };
    p.querySelector(".dp-story-prev").click(); // step 1 → the overview
    return [out];
  })
);
for (const s of stories) {
  console.log("story step 1:", JSON.stringify(s));
  if (!s.lit.length || !s.dimmed || !s.link) fail(`story in panel ${s.i}: step 1 (${s.key}) lit no marks or drew no connector`);
}

// A reference rule (data-rule: ::XLINE "Today", ::REFLINE) shows its tooltip
// under the pointer (on a crosshair chart as a "Reference line" row) but is no
// value: clicking it neither selects nor cross-filters.
// The first rule on screen: one in a hidden tab has no box to hover.
const rules = await page.$$(".dash .panel-svg > svg [data-rule]");
let rule = null;
for (const r of rules) {
  if (await r.evaluate((e) => !!e.getClientRects().length && e.getBoundingClientRect().width + e.getBoundingClientRect().height > 0)) {
    rule = r;
    break;
  }
}
if (rule) {
  const at = await rule.evaluate((e) => {
    e.closest(".panel").scrollIntoView({ block: "center" });
    const r = e.getBoundingClientRect();
    const cross = !!e.closest(".has-axis-pointer");
    return { x: r.left + r.width / 2, y: r.top + r.height / 2, want: cross ? "Reference line" : e.getAttribute("data-tip") };
  });
  await page.mouse.move(at.x, at.y, { steps: 3 });
  await page.waitForTimeout(300);
  const hover = await page.evaluate(() => {
    const t = document.getElementById("dp-tip");
    return t?.classList.contains("show") ? t.textContent : "";
  });
  console.log("rule hover:", hover);
  if (!hover.includes(at.want)) fail("hovering a reference rule shows no tooltip for it");
  await page.mouse.move(0, 0);
  await rule.dispatchEvent("click");
  await page.waitForTimeout(1000);
  const sel = await page.evaluate(() => [...new URL(location.href).searchParams.keys()].filter((k) => k === "v.selected"));
  console.log("rule click selects:", sel.length > 0);
  if (sel.length) fail("clicking a reference rule set a selection");
} else if (rules.length) console.log("rule hover: no reference rule on screen (only in other tabs): skipped");

// Tab until a chart has focus (a dashboard without charts skips this part).
let onChart = false;
for (let i = 0; i < 80 && !onChart && svgs.length; i++) {
  await page.keyboard.press("Tab");
  onChart = await page.evaluate(() => document.activeElement?.matches?.(".panel-svg > svg[tabindex]"));
}
if (!svgs.length) console.log("no charts: keyboard and table-switch checks skipped");
else if (!onChart) fail("Tab never reached a chart");
else {
  const ring = await page.evaluate(() => getComputedStyle(document.activeElement).outlineStyle);
  if (ring === "none") fail("no visible focus ring on the chart");
  await page.keyboard.press("ArrowRight");
  const a = await page.evaluate(() => document.activeElement.getAttribute("aria-label"));
  await page.keyboard.press("ArrowRight");
  const b = await page.evaluate(() => document.activeElement.getAttribute("aria-label"));
  console.log("marks:", a, "→", b);
  if (!a || !b) fail("arrow keys did not focus a mark");
  const tip = await page.evaluate(() => document.getElementById("dp-tip")?.classList.contains("show"));
  if (!tip) fail("the focused mark shows no tooltip");
  // A mark whose ::LINK opens another tab (drill-through) hands the focus to
  // that tab's first panel title; Esc there returns to the mark. Any other
  // Enter re-runs in place and the focus stays on the chart.
  const tab0 = await page.evaluate(() => document.querySelector(".dash > .tabbar > .tab-btn.active")?.textContent || "");
  const linked = await page.evaluate(() => !!document.activeElement.closest(".panel.dp-linked"));
  await page.keyboard.press("Enter");
  await idle();
  const tab1 = await page.evaluate(() => document.querySelector(".dash > .tabbar > .tab-btn.active")?.textContent || "");
  if (linked && tab1 !== tab0) {
    const at = await page.evaluate(() => {
      const ae = document.activeElement;
      const pane = [...document.querySelectorAll(".dash > .tabwrap > .tabpane")].find((p) => p.style.display !== "none");
      return { inPane: !!pane?.contains(ae), title: ae.matches(".panel-title, .group-title"), text: ae.textContent.slice(0, 50) };
    });
    console.log(`drill-through ${tab0} → ${tab1}: focus on "${at.text}"`);
    if (!at.inPane || !at.title) fail(`drill-through: focus not on the target tab's title (${at.text})`);
    await page.keyboard.press("Escape");
    await page.waitForTimeout(200);
    const ret = await page.evaluate(() => ({ tab: document.querySelector(".dash > .tabbar > .tab-btn.active")?.textContent, label: document.activeElement.getAttribute("aria-label") }));
    if (ret.tab !== tab0 || ret.label !== b) fail(`Esc did not return to the linked mark (tab ${ret.tab}, focus ${ret.label})`);
    await page.keyboard.press("Escape"); // the mark → its chart
  } else {
    const back = await page.evaluate(() => !!document.activeElement?.closest?.(".panel-svg"));
    if (!back) fail("focus lost after Enter (re-run)");
    await page.keyboard.press("Escape");
  }
  const out = await page.evaluate(() => document.activeElement?.matches?.(".panel-svg > svg"));
  if (!out) fail("Esc did not return to the chart");
  // Focus moved on while the re-run is in flight (Enter, then at once Esc back
  // to the chart): the redrawn chart gets it, not the mark Enter was on.
  await page.keyboard.press("ArrowRight");
  await page.keyboard.press("Enter");
  await page.keyboard.press("Escape");
  await idle();
  // (on a linked chart Enter left for another tab and Esc came back to the mark)
  const kept = await page.evaluate((linked) => {
    const ae = document.activeElement;
    return (linked ? !!ae?.closest?.(".panel-svg") : ae?.matches?.(".panel-svg > svg")) && ae.isConnected;
  }, linked && tab1 !== tab0);
  if (!kept) fail("a re-run took the focus off the chart");
}

// The toolbox line/bar switch redraws a chart in place: after each switch it
// keeps its summary and keyboard marks, and a ::STORY chart its stepper,
// caption and connector, on the step it was on.
const swapIdx = await page.$$eval(".dash .panel", (ps) => {
  const sw = (p) => p.offsetWidth && p.querySelector('.dp-tool[title="Line / bar"]'); // on screen
  const i = ps.findIndex((p) => sw(p) && p.querySelector(".dp-story")); // a ::STORY chart first
  return i >= 0 ? i : ps.findIndex(sw);
});
if (swapIdx < 0) console.log("no line/bar toolbox switch: redraw check skipped");
else {
  const storyStep = () =>
    page.evaluate((i) => {
      const p = document.querySelectorAll(".dash .panel")[i];
      p.querySelector(".dp-story-next")?.click();
      return p.querySelector(".dp-story-pos")?.textContent || "";
    }, swapIdx);
  const story = await page.evaluate((i) => !!document.querySelectorAll(".dash .panel")[i].querySelector(".dp-story"), swapIdx);
  const before = story ? await storyStep() : "";
  for (const n of [1, 2]) {
    const s = await page.evaluate((i) => {
      const p = document.querySelectorAll(".dash .panel")[i];
      p.querySelector('.dp-tool[title="Line / bar"]').click();
      const svg = p.querySelector(".panel-svg > svg");
      const h = p.querySelector(".panel-svg");
      return {
        role: svg.getAttribute("role"),
        label: svg.getAttribute("aria-label") || "",
        tab: svg.getAttribute("tabindex"),
        marks: svg.querySelectorAll('.dp-hit[tabindex="-1"]').length,
        bars: p.querySelectorAll(".dp-story").length,
        cap: !!h.querySelector(":scope > .dp-story-cap:not([hidden])"),
        link: !!h.querySelector(":scope > .dp-story-link path"),
        pos: p.querySelector(".dp-story-pos")?.textContent || "",
      };
    }, swapIdx);
    console.log(`switch ${n}:`, JSON.stringify({ ...s, label: s.label.slice(0, 40) }));
    if (!s.role || !/\.$/.test(s.label)) fail(`switch ${n}: chart without a summary`);
    if (s.tab !== "0" || !s.marks) fail(`switch ${n}: marks not keyboard reachable`);
    if (story) {
      if (s.bars !== 1 || !s.cap || !s.link) fail(`switch ${n}: story stepper, caption or connector missing`);
      if (s.pos !== before) fail(`switch ${n}: story step not kept (${s.pos} vs ${before})`);
    }
  }
  await idle();
  await page.evaluate((i) => document.querySelectorAll(".dash .panel")[i].querySelector(".panel-svg > svg").focus(), swapIdx);
  await page.keyboard.press("ArrowRight");
  const mark = await page.evaluate(() => document.activeElement.getAttribute("aria-label"));
  if (!mark) fail("arrow keys did not reach a mark after the switch");
  if (story) {
    const after = await storyStep();
    console.log("story:", before, "→", after);
    if (after === before) fail("story stepping stopped after the switch");
  }
}

// "Charts as tables" (#401: off by default, no in-page bar unless the
// dashboard's own header says `-- @astables on` — this one doesn't, so
// `.dp-a11y-bar` must be absent). The toolbar's menu item reaches the
// feature either way; it sits in `header .actions`, which `?embed=1` hides
// with `display:none` — invisible to Playwright's own actionability click,
// but a plain DOM .click() still fires its handler, so the toggle is
// exercised the same way a non-embed viewer would reach it from the ⋯ menu.
const hasBar = await page.evaluate(() => !!document.querySelector(".dp-a11y-bar"));
if (hasBar) fail("the Charts-as-tables bar showed without `-- @astables on`");
await page.evaluate(() => document.getElementById("astables-toggle").click());
const swapped = await page.evaluate(() => {
  const charts = [...document.querySelectorAll(".dash .panel")].filter((f) => f.querySelector(".panel-svg") && f._dp);
  return { charts: charts.length, tables: charts.filter((f) => f.querySelector(".dp-astable-view:not([hidden]) table")).length, hidden: charts.every((f) => f.querySelector(".panel-svg").hidden) };
});
console.log("as tables:", JSON.stringify(swapped));
if (svgs.length && (!swapped.charts || swapped.tables !== swapped.charts || !swapped.hidden)) fail("charts not swapped for tables");
await page.evaluate(() => document.getElementById("astables-toggle").click());

// Resizable sidebar (issue #59), in the builder (not embed): a focusable
// separator with its bounds; ←/→ step, the width survives a reload,
// double-click resets, charts follow, and phones get a closed drawer instead.
// Always on "Overview" (it has charts), whatever --dashboard is.
{
  const bp = await browser.newPage({ viewport: { width: 1400, height: 1000 }, reducedMotion: "reduce" });
  bp.on("pageerror", (e) => errs.push(String(e)));
  await bp.goto(`${base}?dashboard=Overview`);
  await bp.waitForSelector(".dash .panel-svg > svg", { timeout: 120000 });
  // Settle first: under reduced motion every property has a .01ms transition,
  // so a new width lands a frame (or, under load, several) later. Wait until
  // the sidebar has the handle's width (238px on a phone) and the chart's
  // width holds for two frames.
  const side = () =>
    bp.evaluate(async () => {
      const frame = () => new Promise(requestAnimationFrame);
      const sb = document.getElementById("sidebar");
      const target = () => (matchMedia("(max-width:820px)").matches ? 238 : +document.getElementById("side-resizer").getAttribute("aria-valuenow"));
      const cw = () => Math.round(document.querySelector(".dash .panel-svg > svg").getBoundingClientRect().width);
      let last = -1;
      for (const t0 = performance.now(); performance.now() - t0 < 5000; ) {
        await frame();
        const c = cw();
        if (Math.round(sb.getBoundingClientRect().width) === target() && c === last) break;
        last = c;
      }
      const h = document.getElementById("side-resizer");
      return {
        w: Math.round(sb.getBoundingClientRect().width),
        now: +h.getAttribute("aria-valuenow"),
        role: h.getAttribute("role"),
        min: +h.getAttribute("aria-valuemin"),
        max: +h.getAttribute("aria-valuemax"),
        tab: h.tabIndex,
        chart: cw(),
        handle: getComputedStyle(h).display,
      };
    });
  const s0 = await side();
  if (s0.role !== "separator" || s0.tab !== 0 || s0.min !== 180 || s0.max !== 480 || s0.w !== 238 || s0.now !== 238) fail(`sidebar handle: ${JSON.stringify(s0)}`);
  await bp.focus("#side-resizer");
  await bp.keyboard.press("ArrowRight");
  await bp.keyboard.press("ArrowRight");
  const s1 = await side();
  if (s1.w !== s0.now + 32 || s1.now !== s1.w) fail(`sidebar ←/→: ${JSON.stringify(s1)}`);
  await bp.keyboard.press("End");
  if ((await side()).now !== 480) fail("sidebar End: not at the max");
  await bp.keyboard.press("Home");
  await bp.keyboard.press("ArrowLeft");
  if ((await side()).w !== 180) fail("sidebar: below the min");
  // Pointer drag to the right; charts get narrower with their panel.
  const hb = await bp.locator("#side-resizer").boundingBox();
  await bp.mouse.move(hb.x + hb.width / 2, hb.y + 200);
  await bp.mouse.down();
  await bp.mouse.move(hb.x + hb.width / 2 + 200, hb.y + 200, { steps: 5 });
  await bp.mouse.up();
  await bp.waitForTimeout(200);
  const s2 = await side();
  if (s2.w !== 380) fail(`sidebar drag: ${JSON.stringify(s2)}`);
  if (!(s2.chart < s0.chart)) fail(`charts did not re-layout: ${s0.chart} -> ${s2.chart}`);
  await bp.reload();
  await bp.waitForSelector(".dash .panel-svg > svg", { timeout: 120000 });
  if ((await side()).w !== 380) fail("sidebar width not remembered across a reload");
  await bp.dblclick("#side-resizer");
  const s3 = await side();
  if (s3.w !== 238) fail(`sidebar double-click reset: ${JSON.stringify(s3)}`);
  // Phone width: the sidebar is a closed drawer, min(85vw, 320px) wide (the
  // stored width ignored, the handle gone); ☰ opens it as a modal dialog with
  // focus inside, Esc closes it back onto ☰.
  await bp.focus("#side-resizer");
  await bp.keyboard.press("End");
  await bp.setViewportSize({ width: 390, height: 844 });
  await bp.waitForTimeout(300); // the media-query change event
  const drawer = () =>
    bp.evaluate(() => ({
      w: Math.round(document.getElementById("sidebar").getBoundingClientRect().width),
      shown: getComputedStyle(document.getElementById("sidebar")).visibility === "visible",
      handle: getComputedStyle(document.getElementById("side-resizer")).display,
      now: Number(document.getElementById("side-resizer").getAttribute("aria-valuenow")),
      expanded: document.getElementById("side-toggle").getAttribute("aria-expanded"),
      focus: document.activeElement.id,
      modal: document.getElementById("sidebar").getAttribute("aria-modal"),
    }));
  const phone = await drawer();
  if (phone.now !== 480 || phone.w !== Math.min(Math.round(390 * 0.85), 320) || phone.shown || phone.handle !== "none" || phone.expanded !== "false")
    fail(`sidebar on a phone: ${JSON.stringify(phone)}`);
  await bp.click("#side-toggle");
  await bp.waitForTimeout(300);
  const opened = await drawer();
  if (!opened.shown || opened.expanded !== "true" || opened.modal !== "true" || opened.focus !== "side-new") fail(`phone drawer open: ${JSON.stringify(opened)}`);
  await bp.keyboard.press("Escape");
  await bp.waitForTimeout(300);
  const closed = await drawer();
  if (closed.shown || closed.expanded !== "false" || closed.focus !== "side-toggle") fail(`phone drawer Esc: ${JSON.stringify(closed)}`);
  console.log("sidebar:", s0.w, "→", s1.w, "→ drag", s2.w, "→ reset", s3.w, "· chart", s0.chart, "→", s2.chart, "· phone", JSON.stringify(phone));
  await bp.close();
}

// The examples library (sidebar): one "Examples" section grouped by business
// area, open on a first visit, "How it works" closed. Its toggles are buttons
// with aria-expanded that keep the focus and survive a reload; Hide moves the
// focus to "Show examples", which brings the section back; a visit without a
// link reopens the dashboard open last. With --axe, axe
// checks the sidebar (colour contrast included), light and dark.
for (const scheme of ["light", "dark"]) {
  const lp = await browser.newPage({ viewport: { width: 1400, height: 1000 }, colorScheme: scheme, reducedMotion: "reduce" });
  lp.on("pageerror", (e) => errs.push(String(e)));
  await lp.goto(`${base}?dashboard=Overview`);
  await lp.waitForSelector("#side-ex-toggle", { timeout: 60000 });
  const lib = () =>
    lp.evaluate(() => ({
      open: document.getElementById("side-ex-toggle")?.getAttribute("aria-expanded"),
      listHidden: document.getElementById("side-ex")?.hidden,
      areas: [...document.querySelectorAll(".side-area")].map((b) => [
        b.querySelector(".g-name").textContent,
        b.getAttribute("aria-expanded"),
        document.getElementById(b.getAttribute("aria-controls"))?.hidden,
      ]),
      focus: document.activeElement?.id,
      active: document.querySelector(".side-item.active")?.dataset.key,
      show: !!document.getElementById("side-show-ex"),
    }));
  const tag = `examples library (${scheme})`;
  let l = await lib();
  if (l.open !== "true" || l.listHidden || l.active !== "ex:where-do-our-sessions-come-from") fail(`${tag}: first visit ${JSON.stringify(l)}`);
  if (l.areas[0]?.[0] !== "Start here" || l.areas.at(-1)?.[0] !== "How it works") fail(`${tag}: area order ${JSON.stringify(l.areas)}`);
  for (const [name, exp, hidden] of l.areas)
    if (exp !== String(name !== "How it works") || hidden !== (exp === "false")) fail(`${tag}: area ${name} expanded=${exp} hidden=${hidden}`);
  await lp.focus("#side-area-how-it-works");
  await lp.keyboard.press("Enter");
  l = await lib();
  if (l.areas.at(-1)[1] !== "true" || l.areas.at(-1)[2] || l.focus !== "side-area-how-it-works") fail(`${tag}: Enter on an area ${JSON.stringify(l)}`);
  await lp.focus("#side-ex-toggle");
  await lp.keyboard.press("Space");
  l = await lib();
  if (l.open !== "false" || !l.listHidden || l.focus !== "side-ex-toggle") fail(`${tag}: Space on the library ${JSON.stringify(l)}`);
  // (without ?dashboard=: a linked example opens its library and area for that page)
  await lp.goto(base);
  await lp.waitForSelector("#side-ex-toggle", { timeout: 60000 });
  l = await lib();
  if (l.open !== "false" || l.areas.at(-1)[1] !== "true") fail(`${tag}: choices not kept across a reload ${JSON.stringify(l)}`);
  await lp.focus("#side-ex-toggle");
  await lp.keyboard.press("Enter");
  await lp.focus("#side-hide-ex");
  await lp.keyboard.press("Enter");
  l = await lib();
  if (l.open !== undefined || !l.show || l.focus !== "side-show-ex") fail(`${tag}: Hide ${JSON.stringify(l)}`);
  await lp.reload();
  await lp.waitForSelector("#side-show-ex", { state: "attached", timeout: 60000 });
  await lp.focus("#side-show-ex");
  await lp.keyboard.press("Enter");
  l = await lib();
  if (l.open !== "true" || l.show || l.focus !== "side-ex-toggle") fail(`${tag}: Show examples ${JSON.stringify(l)}`);
  // Without a link, the next visit opens the dashboard open last.
  await lp.click('.side-item[data-key="ex:why-do-customers-cancel"]');
  await lp.goto(base);
  await lp.waitForSelector("#side-ex-toggle", { timeout: 60000 });
  l = await lib();
  const nm = await lp.inputValue("#dash-name");
  if (l.active !== "ex:why-do-customers-cancel" || nm !== "Why do customers cancel?") fail(`${tag}: last dashboard not reopened (${l.active}, ${nm})`);
  if (axe && existsSync(axe)) {
    await lp.addScriptTag({ path: axe });
    const res = await lp.evaluate(async () => window.axe.run("#sidebar", { runOnly: { type: "tag", values: ["wcag2a", "wcag2aa"] } }));
    for (const v of res.violations) console.log(`axe sidebar ${scheme} ${v.impact}: ${v.id} (${v.nodes.length}) ${v.help} — ${v.nodes[0]?.target}`);
    const bad = res.violations.filter((v) => v.impact === "serious" || v.impact === "critical");
    if (bad.length) fail(`axe sidebar ${scheme}: ${bad.map((v) => v.id).join(", ")}`);
    else console.log(`axe sidebar ${scheme}: ${res.passes.length} rules pass, no serious violation`);
  }
  console.log(`${tag}: ${l.areas.map((a) => a[0]).join(" · ")}`);
  await lp.close();
}

// The inline "Layout" example's ::VIEW switcher (#214: Revenue → By week →
// Bars/Lines/Table, added in web/app.js's "examples: Revenue → By week as a
// ::VIEW switcher" commit) — always checked here, whatever --dashboard was
// asked for on the command line: axe, keyboard (Arrow/Home/End move +
// activate, aria-selected updates, focus stays on the tab — which is exactly
// what a document-level handler such as dpNav intercepting the key would
// break, so this also shows it never does), phone width 390 (no horizontal
// overflow, the view bar visible and tappable, the panel full width), and
// reduced motion (no .dp-switching animation class, the same
// reducedMotion: "reduce" simulation this script already uses elsewhere).
{
  const vp = await browser.newPage({ viewport: { width: 1400, height: 1000 }, reducedMotion: "reduce" });
  vp.on("pageerror", (e) => errs.push(String(e)));
  await vp.goto(`${base}?dashboard=${encodeURIComponent("How is revenue split by week and channel?")}`);
  await vp.waitForSelector(".dp-viewbar [role='tab']", { timeout: 60000 });
  await vp.waitForFunction(() => document.querySelector(".dash .panel") && !document.body.classList.contains("loading"), null, { timeout: 60000 });
  const viewTabs = () => vp.$$eval(".dp-viewbar [role='tab']", (l) => l.map((b) => b.textContent));
  const selected = () => vp.$$eval(".dp-viewbar [role='tab']", (l) => l.map((b) => b.getAttribute("aria-selected")));
  if (JSON.stringify(await viewTabs()) !== '["Bars","Lines","Table"]') fail(`::VIEW switcher tabs: ${JSON.stringify(await viewTabs())}`);

  await vp.focus(".dp-viewbar [role='tab']:first-child");
  await vp.keyboard.press("ArrowRight");
  // activateView's refocus() moves the focus on the next animation frame
  // (the new controls attach once the run has rendered), not synchronously.
  await vp.waitForFunction(() => document.activeElement === document.querySelectorAll(".dp-viewbar [role='tab']")[1], null, { timeout: 5000 });
  let st = await vp.evaluate(() => {
    const btns = [...document.querySelectorAll(".dp-viewbar [role='tab']")];
    const pane = document.getElementById(btns[1].getAttribute("aria-controls"));
    return { sel: btns.map((b) => b.getAttribute("aria-selected")), focused: document.activeElement === btns[1], shown: pane?.style.display !== "none" };
  });
  if (st.sel[1] !== "true" || !st.focused || !st.shown) fail(`::VIEW switcher ArrowRight: ${JSON.stringify(st)}`);
  await vp.keyboard.press("End");
  if ((await selected())[2] !== "true") fail(`::VIEW switcher End: ${JSON.stringify(await selected())}`);
  await vp.keyboard.press("Home");
  if ((await selected())[0] !== "true") fail(`::VIEW switcher Home: ${JSON.stringify(await selected())}`);
  console.log("::VIEW switcher keyboard: Bars →(ArrowRight) Lines →(End) Table →(Home) Bars, focus kept on the tab throughout");

  if (axe && existsSync(axe)) {
    await vp.addScriptTag({ path: axe });
    const res = await vp.evaluate(async () => window.axe.run(".dp-views", { runOnly: { type: "tag", values: ["wcag2a", "wcag2aa"] } }));
    for (const v of res.violations) console.log(`axe ::VIEW switcher ${v.impact}: ${v.id} (${v.nodes.length}) ${v.help}`);
    const bad = res.violations.filter((v) => v.impact === "serious" || v.impact === "critical");
    if (bad.length) fail(`axe ::VIEW switcher: ${bad.map((v) => v.id).join(", ")}`);
    else console.log(`axe ::VIEW switcher: ${res.passes.length} rules pass, no serious violation`);
  }

  // Reduced motion: switching to a different alternative applies no
  // .dp-switching class (this context's reducedMotion: "reduce" makes
  // dpAnim.still() true — see web/app.js's activateView).
  await vp.click(".dp-viewbar [role='tab']:nth-child(2)"); // Bars → Lines
  await vp.waitForTimeout(100);
  const switching = await vp.evaluate(() => !!document.querySelector(".dp-switching"));
  if (switching) fail("reduced motion: .dp-switching was applied on a view switch");

  // Phone width (390): no horizontal overflow, the view bar visible and
  // tappable, the active pane's panel full width.
  await vp.setViewportSize({ width: 390, height: 844 });
  await vp.waitForTimeout(300);
  const phone = await vp.evaluate(() => {
    const bar = document.querySelector(".dp-viewbar");
    const btn = bar?.querySelector('[role="tab"][aria-selected="true"]');
    const pane = btn && document.getElementById(btn.getAttribute("aria-controls"));
    const br = btn?.getBoundingClientRect();
    const pr = pane?.getBoundingClientRect();
    return {
      overflow: document.documentElement.scrollWidth - document.documentElement.clientWidth,
      barVisible: !!bar && getComputedStyle(bar).display !== "none" && bar.getBoundingClientRect().width > 0,
      tapSize: br ? Math.min(br.width, br.height) : 0,
      paneWidth: pr?.width ?? 0,
    };
  });
  console.log("::VIEW switcher phone (390):", JSON.stringify(phone));
  if (phone.overflow > 1) fail(`::VIEW switcher phone 390: horizontal overflow ${phone.overflow}px`);
  if (!phone.barVisible) fail("::VIEW switcher phone 390: the view bar is not visible");
  if (phone.tapSize < 24) fail(`::VIEW switcher phone 390: the tab is too small to tap (${phone.tapSize}px)`);
  if (phone.paneWidth < 300) fail(`::VIEW switcher phone 390: the panel is not full width (${phone.paneWidth}px)`);
  await vp.close();
}

if (axe && existsSync(axe)) {
  await page.addScriptTag({ path: axe });
  const res = await page.evaluate(async () =>
    window.axe.run(".dash", { runOnly: { type: "tag", values: ["wcag2a", "wcag2aa"] }, rules: { "color-contrast": { enabled: false } } })
  );
  for (const v of res.violations) console.log(`axe ${v.impact}: ${v.id} (${v.nodes.length}) ${v.help}`);
  const bad = res.violations.filter((v) => /^(aria-|svg-img-alt|role-img-alt|nested-interactive|button-name)/.test(v.id));
  if (bad.length) fail(`axe: ${bad.map((v) => v.id).join(", ")}`);
  // Explore data (#97): the catalog, a table's column cards, a chart preview
  // and a query result, keyboard-reachable and labelled — in light and dark.
  for (const scheme of ["light", "dark"]) {
    const xp = await browser.newPage({ viewport: { width: 1400, height: 1000 }, colorScheme: scheme, reducedMotion: "reduce" });
    xp.on("pageerror", (e) => errs.push(String(e)));
    await xp.goto(`${base}?dashboard=${encodeURIComponent("Period & KPIs")}`);
    await xp.waitForFunction(() => document.querySelector(".dash .panel") && !document.body.classList.contains("loading"), null, { timeout: 120000 });
    await xp.click("#side-explore");
    await xp.waitForSelector('.cat-table[data-fq$=\'"shop"\']');
    // keyboard: the table is a button in the catalog list
    await xp.focus('.cat-table[data-fq$=\'"shop"\']');
    await xp.keyboard.press("Enter");
    await xp.waitForFunction(() => document.querySelector(".xp-progress.done") && !document.querySelector("#xq-detail[aria-busy]"), null, { timeout: 120000 });
    await xp.focus('.xp-card[data-col="orders"] .xp-chartbtn');
    await xp.keyboard.press("Enter");
    await xp.waitForSelector(".xp-chart-svg > svg[aria-label]", { timeout: 60000 });
    if (!(await xp.evaluate(() => document.activeElement?.closest(".xp-chart")))) fail("explore: the chart preview does not take the focus");
    await xp.click("#xq-run");
    await xp.waitForSelector("#xq-tools button", { timeout: 60000 });
    // a comparison of two time ranges (#111)
    await xp.click("button:has-text('Compare…')");
    await xp.check(".xp-cmp-mode input[value=range]");
    await xp.click(".xp-cmp-form button[type=submit]");
    await xp.waitForSelector("table.xp-cmp", { timeout: 60000 });
    await xp.addScriptTag({ path: axe });
    const xres = await xp.evaluate(async () => window.axe.run("#explore", { runOnly: { type: "tag", values: ["wcag2a", "wcag2aa"] } }));
    for (const v of xres.violations) console.log(`axe explore ${scheme} ${v.impact}: ${v.id} (${v.nodes.length}) ${v.help} — ${v.nodes[0]?.target}`);
    const xbad = xres.violations.filter((v) => v.impact === "serious" || v.impact === "critical");
    if (xbad.length) fail(`axe explore ${scheme}: ${xbad.map((v) => v.id).join(", ")}`);
    else console.log(`axe explore ${scheme}: ${xres.passes.length} rules pass, no serious violation`);
    await xp.close();
  }
} else console.log("axe-core not given (--axe): skipped");
if (errs.length) fail("page errors: " + errs.join(" | "));
await browser.close();
if (!process.exitCode) console.log("a11y-e2e: ok");
