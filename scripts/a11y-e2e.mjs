// Keyboard + screen-reader check of the charts in a real browser (issue #37):
// Tab into a chart, walk its marks with the arrow keys, Enter selects (the
// dashboard re-runs and the focus comes back), Esc leaves; every chart SVG has
// a summary; "Charts as tables" swaps each chart for a table. With axe-core on
// disk (--axe path/to/axe.min.js) it also runs axe on the dashboard.
//
//   python3 serve_nocache.py 8791 &
//   NODE_PATH=<dir with playwright>/node_modules node scripts/a11y-e2e.mjs \
//       [--url http://127.0.0.1:8791/] [--dashboard "Selections & drill"] [--axe axe.min.js]
import { createRequire } from "module";
import { existsSync } from "fs";

const require = createRequire(import.meta.url);
const { chromium } = require("playwright");
const arg = (k, d) => {
  const i = process.argv.indexOf(k);
  return i > 0 ? process.argv[i + 1] : d;
};
const base = arg("--url", "http://127.0.0.1:8791/");
const dash = arg("--dashboard", "Overview");
const axe = arg("--axe", "");
const fail = (m) => {
  console.error("FAIL: " + m);
  process.exitCode = 1;
};

const browser = await chromium.launch();
const page = await browser.newPage({ viewport: { width: 1400, height: 1000 }, reducedMotion: "reduce" });
const errs = [];
page.on("pageerror", (e) => errs.push(String(e)));
await page.goto(`${base}?embed=1&dashboard=${encodeURIComponent(dash)}`);
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

// Every chart has a one-sentence summary (decorative overlays such as the
// ::STORY connector are aria-hidden and skipped).
const svgs = await page.$$eval(".dash .panel-svg > svg:not([aria-hidden=\"true\"])", (s) => s.map((e) => [e.getAttribute("role"), e.getAttribute("aria-label") || ""]));
console.log(`${svgs.length} charts`);
for (const [role, label] of svgs) {
  if (!role || !/\.$/.test(label)) fail(`chart without a summary: role=${role} label=${label}`);
}
console.log("summary:", svgs[0]?.[1]);

// A reference rule (data-rule: ::XLINE "Today", ::REFLINE) shows its tooltip
// but is no value: clicking it neither selects nor cross-filters.
const rule = await page.$(".dash .panel-svg > svg [data-rule]");
if (rule) {
  await rule.dispatchEvent("click");
  await page.waitForTimeout(1000);
  const sel = await page.evaluate(() => [...new URL(location.href).searchParams.keys()].filter((k) => k === "v.selected"));
  console.log("rule click selects:", sel.length > 0);
  if (sel.length) fail("clicking a reference rule set a selection");
}

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
  await page.keyboard.press("Enter");
  await page.waitForTimeout(1500);
  const back = await page.evaluate(() => !!document.activeElement?.closest?.(".panel-svg"));
  if (!back) fail("focus lost after Enter (re-run)");
  await page.keyboard.press("Escape");
  const out = await page.evaluate(() => document.activeElement?.matches?.(".panel-svg > svg"));
  if (!out) fail("Esc did not return to the chart");
}

// "Charts as tables".
await page.click(".dp-astable-btn");
const swapped = await page.evaluate(() => {
  const charts = [...document.querySelectorAll(".dash .panel")].filter((f) => f.querySelector(".panel-svg") && f._dp);
  return { charts: charts.length, tables: charts.filter((f) => f.querySelector(".dp-astable-view:not([hidden]) table")).length, hidden: charts.every((f) => f.querySelector(".panel-svg").hidden) };
});
console.log("as tables:", JSON.stringify(swapped));
if (svgs.length && (!swapped.charts || swapped.tables !== swapped.charts || !swapped.hidden)) fail("charts not swapped for tables");
await page.click(".dp-astable-btn");

if (axe && existsSync(axe)) {
  await page.addScriptTag({ path: axe });
  const res = await page.evaluate(async () =>
    window.axe.run(".dash", { runOnly: { type: "tag", values: ["wcag2a", "wcag2aa"] }, rules: { "color-contrast": { enabled: false } } })
  );
  for (const v of res.violations) console.log(`axe ${v.impact}: ${v.id} (${v.nodes.length}) ${v.help}`);
  const bad = res.violations.filter((v) => /^(aria-|svg-img-alt|role-img-alt|nested-interactive|button-name)/.test(v.id));
  if (bad.length) fail(`axe: ${bad.map((v) => v.id).join(", ")}`);
} else console.log("axe-core not given (--axe): skipped");
if (errs.length) fail("page errors: " + errs.join(" | "));
await browser.close();
if (!process.exitCode) console.log("a11y-e2e: ok");
