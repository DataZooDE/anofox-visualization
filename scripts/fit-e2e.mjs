// Fit-to-screen (::FIT, #400) in a real browser: a ::TAB with 'fit'::FIT
// fills the window with no page scroll, its charts drawn again by ggplot-rs
// at their fitted pixel size; a tab without it scrolls as before; ?fit=0
// turns it off; a ::FIT tab with too many panels falls back to scrolling
// with a notice. fitScreen() in web/app.js is what this exercises — the
// layout is all measured in the DOM, so no unit test covers it.
//
//   wasm-pack build --target web --out-dir web/pkg --profile wasm --no-default-features --features wasm
//   python3 serve_nocache.py 8797 &
//   NODE_PATH=<dir with playwright>/node_modules node scripts/fit-e2e.mjs [--url http://127.0.0.1:8797/] [--shots <dir>]
//
// At 1920×1080, 1366×768 and 3840×2160:
// (a) the ::FIT tab: no page scrollbar, every panel inside the viewport, each
//     chart's <svg> drawn at its panel's size (its height attribute = the
//     shown height, no CSS transform), a table scrolls inside its panel, the
//     KPI tiles keep their minimum height;
// (b) the tab without ::FIT still scrolls;
// (c) the overloaded ::FIT tab scrolls and shows the notice;
// then (d) a window resize re-fits (panel sizes change, still no scroll),
// (e) ?fit=0 leaves the ::FIT tab scrolling, (f) a cross-filter re-run keeps
// the fitted sizes. --shots <dir> also writes the fitted tab at 1920×1080 in
// the light and dark theme (fit-light-1920.png, fit-dark-1920.png).
import { createRequire } from "module";

const require = createRequire(import.meta.url);
const { chromium } = require("playwright");
const arg = (k, d) => {
  const i = process.argv.indexOf(k);
  return i > 0 ? process.argv[i + 1] : d;
};
const base = arg("--url", "http://127.0.0.1:8797/");
const shots = arg("--shots", "");
const b64 = (s) => Buffer.from(s, "utf8").toString("base64");
let bad = 0;
const check = (ok, m) => (ok ? console.log("ok   " + m) : (bad++, console.error("FAIL " + m)));

const chart = (n) =>
  `SELECT 'Series ${n}' AS t ::TITLE, i AS x ::XAXIS, round(50 + 30 * sin(i / 2.0 + ${n}), 1) AS y ::LINECHART FROM range(12) r(i);\n`;
const bars = (n) =>
  `SELECT 'Region ${n}' AS t ::TITLE, r AS region ::XAXIS, v ::BARCHART FROM (VALUES ('North', ${3 + n}), ('South', 5), ('East', 2), ('West', 4)) t(r, v);\n`;
const SQL = `CREATE TABLE fit_orders AS SELECT i AS id, ['North', 'South', 'East', 'West'][1 + i % 4] AS region, 20 + (i * 37) % 90 AS amount FROM range(40) t(i);
SELECT 'All' AS fit_region ::DROPDOWN UNION ALL SELECT DISTINCT region FROM fit_orders;
SELECT 'Overview'::TAB;
SELECT 'fit'::FIT;
SELECT sum(amount) ::MONEY, 'Revenue'::LABEL FROM fit_orders WHERE getvariable('fit_region') IN ('All', region);
SELECT count(*) ::METRIC, 'Orders'::LABEL FROM fit_orders WHERE getvariable('fit_region') IN ('All', region);
SELECT round(avg(amount), 1) ::METRIC, 'Avg order'::LABEL FROM fit_orders WHERE getvariable('fit_region') IN ('All', region);
SELECT 'Revenue by region' AS t ::TITLE, region ::XAXIS, sum(amount) AS revenue ::BARCHART FROM fit_orders WHERE getvariable('fit_region') IN ('All', region) GROUP BY ALL ORDER BY 1;
SELECT 360::HEIGHT;
${chart(1)}SELECT 'Orders' AS t ::TITLE, id ::TABLE, region, amount FROM fit_orders WHERE getvariable('fit_region') IN ('All', region) ORDER BY id;
${bars(2)}SELECT 'Details'::TAB;
${[3, 4, 5, 6, 7, 8, 9, 10].map(chart).join("")}SELECT 'Crowded'::TAB;
SELECT 'fit'::FIT;
SELECT 120::HEIGHT;
${Array.from({ length: 36 }, (_, k) => (k ? "SELECT 120::HEIGHT;\n" : "") + bars(k)).join("")}`;

const browser = await chromium.launch();
const settled = (page) =>
  page.waitForFunction(
    () => /\d+ panels?/.test(document.getElementById("status")?.textContent || "") && !document.body.classList.contains("loading"),
    null,
    { timeout: 120000 }
  );
// The shown pane's lazily rendered panels have all arrived, and the debounced
// re-fit has run.
const paneReady = (page) =>
  page.waitForFunction(() => !document.querySelector(".dash .tabpane:not([style*='none']) .dp-lazy"), null, { timeout: 120000 }).then(() => page.waitForTimeout(400));
async function open(viewport, query = "", theme = "") {
  const page = await browser.newPage({ viewport, reducedMotion: "reduce", colorScheme: theme || "light" });
  const errs = [];
  page.on("pageerror", (e) => errs.push(String(e)));
  await page.goto(`${base}${query}#sql=${b64(SQL)}`);
  await settled(page);
  await paneReady(page);
  return { page, errs };
}
async function tab(page, name) {
  await page.click(`.tabbar .tab-btn:text-is('${name}')`);
  await paneReady(page);
}
// What the checks read from the page.
const measure = (page) =>
  page.evaluate(() => {
    const vh = window.innerHeight,
      vw = window.innerWidth;
    const root = document.documentElement;
    const panels = [...document.querySelectorAll(".dash .panel")].filter((p) => p.getClientRects().length && !p.parentElement.closest(".panel"));
    const boxes = panels.map((p) => {
      const r = p.getBoundingClientRect();
      return { top: r.top + window.scrollY, bottom: r.bottom + window.scrollY, left: r.left, right: r.right, h: r.height, kpi: p.matches(".metric"), table: !!p.querySelector(".dp-table") };
    });
    const charts = panels
      .map((p) => {
        const holder = p.querySelector(":scope > .panel-svg");
        const svg = holder && holder.querySelector(":scope > svg");
        if (!svg) return null;
        const hr = holder.getBoundingClientRect();
        const sr = svg.getBoundingClientRect();
        return {
          attrH: parseFloat(svg.getAttribute("height")),
          attrW: parseFloat(svg.getAttribute("width")),
          vbH: svg.viewBox.baseVal.height,
          vbW: svg.viewBox.baseVal.width,
          shownH: sr.height,
          shownW: sr.width,
          holderW: hr.width,
          transform: getComputedStyle(svg).transform,
          inlineTransform: svg.style.transform || "",
        };
      })
      .filter(Boolean);
    const table = panels.find((p) => p.querySelector(".dp-table"));
    return {
      vh,
      vw,
      scroll: root.scrollHeight > vh + 1,
      scrollH: root.scrollHeight,
      state: document.body.dataset.dpFit || "",
      note: !!document.querySelector("#dp-fit-note:not([hidden])"),
      boxes,
      charts,
      // the table's panel scrolls inside itself (overflow: auto); `tableOver`:
      // its rows are taller than the panel (so it does scroll)
      tableScrolls: table ? getComputedStyle(table).overflowY === "auto" : null,
      tableOver: table ? table.scrollHeight > table.clientHeight + 1 : null,
    };
  });
const inView = (m) => m.boxes.every((b) => b.top >= -0.5 && b.bottom <= m.vh + 0.5 && b.left >= -0.5 && b.right <= m.vw + 0.5);
function checkFitted(m, tag) {
  check(m.state === "on", `${tag} the ::FIT tab is fitted (state ${m.state})`);
  check(!m.scroll, `${tag} no page scrollbar (scrollHeight ${m.scrollH} vs ${m.vh})`);
  check(inView(m), `${tag} every panel is inside the viewport`);
  check(m.charts.length >= 3, `${tag} charts on the tab (${m.charts.length})`);
  // drawn at the panel's real size by ggplot-rs: the <svg>'s own height
  // attribute / viewBox is the height it is shown at (not a width-scaled or
  // stretched picture), and no CSS transform scales it
  const real = m.charts.every((c) => Math.abs((c.attrH || c.vbH) - c.shownH) <= 2 && Math.abs(c.vbH - c.shownH) <= 2 && Math.abs(c.vbW - c.holderW) <= 2);
  check(real, `${tag} each chart's svg height/viewBox matches its shown size: ${JSON.stringify(m.charts.map((c) => [c.attrH, c.vbH, Math.round(c.shownH), c.vbW, Math.round(c.holderW)]))}`);
  check(m.charts.every((c) => (c.transform === "none" || c.transform === "") && !/scale/.test(c.inlineTransform)), `${tag} no CSS transform on the charts`);
  check(m.boxes.filter((b) => b.kpi).every((b) => b.h >= 96 - 0.5), `${tag} KPI tiles keep their minimum height`);
  check(m.boxes.filter((b) => !b.kpi).every((b) => b.h >= 120 - 0.5), `${tag} no panel under 120 px`);
  check(m.tableScrolls === true, `${tag} the table's panel scrolls inside itself`);
}

for (const vp of [
  { width: 1920, height: 1080 },
  { width: 1366, height: 768 },
  { width: 3840, height: 2160 },
]) {
  const tag = `[${vp.width}×${vp.height}]`;
  const { page, errs } = await open(vp);
  // (a)
  const m = await measure(page);
  checkFitted(m, tag + " (a)");
  if (vp.height <= 1080) check(m.tableOver === true, `${tag} (a) …and its rows do overflow it (scrolled, not clipped or squeezed)`);
  // (b)
  await tab(page, "Details");
  const d = await measure(page);
  check(d.state === "off", `${tag} (b) the tab without ::FIT is not fitted (${d.state})`);
  check(d.scroll, `${tag} (b) …and scrolls as before`);
  // (c)
  await tab(page, "Crowded");
  const c = await measure(page);
  check(c.state === "skipped", `${tag} (c) the overloaded ::FIT tab falls back (${c.state})`);
  check(c.scroll, `${tag} (c) …to scrolling`);
  check(c.note, `${tag} (c) …with the notice`);
  // back: fitted again, the notice gone
  await tab(page, "Overview");
  const back = await measure(page);
  check(back.state === "on" && !back.scroll && !back.note, `${tag} back on the ::FIT tab: fitted again, no notice`);
  check(!errs.length, `${tag} no page errors: ${errs.join(" | ")}`);
  await page.close();
}

// (d) a resize re-fits
{
  const { page, errs } = await open({ width: 1920, height: 1080 });
  const before = await measure(page);
  await page.setViewportSize({ width: 1500, height: 820 });
  await page.waitForTimeout(700); // the 200 ms debounce + redraw
  const after = await measure(page);
  const changed = before.boxes.some((b, k) => after.boxes[k] && Math.abs(after.boxes[k].h - b.h) > 4);
  check(changed, "(d) resizing the window re-fits: the panels' heights change");
  checkFitted(after, "(d) after the resize:");
  check(!errs.length, `(d) no page errors: ${errs.join(" | ")}`);
  await page.close();
}

// (e) ?fit=0
{
  const { page, errs } = await open({ width: 1366, height: 768 }, "?fit=0");
  const m = await measure(page);
  check(m.state === "off", `(e) ?fit=0: the ::FIT tab is not fitted (${m.state})`);
  check(m.scroll, "(e) ?fit=0: …the page scrolls");
  check(!errs.length, `(e) no page errors: ${errs.join(" | ")}`);
  await page.close();
}

// (e') ?fit=1 fits the tab without ::FIT
{
  const { page, errs } = await open({ width: 1920, height: 1080 }, "?fit=1&tab=Details");
  const m = await measure(page);
  check(m.state === "on" && !m.scroll, `(e') ?fit=1 fits a tab without ::FIT (${m.state}, scroll ${m.scroll})`);
  check(!errs.length, `(e') no page errors: ${errs.join(" | ")}`);
  await page.close();
}

// (e'') a phone never fits
{
  const { page, errs } = await open({ width: 420, height: 860 });
  const m = await measure(page);
  check(m.state === "off" && m.scroll, `(e'') under 600 px ::FIT is ignored (${m.state})`);
  check(!errs.length, `(e'') no page errors: ${errs.join(" | ")}`);
  await page.close();
}

// (f) a cross-filter re-run keeps the fitted sizes
{
  const { page, errs } = await open({ width: 1920, height: 1080 });
  const before = await measure(page);
  await page.selectOption(".dash select", "North");
  await settled(page);
  await page.waitForTimeout(500);
  const after = await measure(page);
  check(after.state === "on", "(f) still fitted after a cross-filter re-run");
  const same = before.boxes.length === after.boxes.length && before.boxes.every((b, k) => Math.abs(after.boxes[k].h - b.h) <= 2);
  check(same, "(f) …at the same panel sizes");
  checkFitted(after, "(f) after the re-run:");
  check(!errs.length, `(f) no page errors: ${errs.join(" | ")}`);
  await page.close();
}

// screenshots of the fitted tab (light / dark)
if (shots) {
  for (const theme of ["light", "dark"]) {
    const { page } = await open({ width: 1920, height: 1080 }, "", theme);
    await page.screenshot({ path: `${shots}/fit-${theme}-1920.png` });
    console.log(`shot ${shots}/fit-${theme}-1920.png (${await page.evaluate(() => document.body.dataset.dpFit)})`);
    await page.close();
  }
}

await browser.close();
console.log(`fit-e2e: ${bad} failed`);
if (bad) process.exitCode = 1;
