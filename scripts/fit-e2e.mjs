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
// (g) a ::FIT inside a ::SUBTAB fits only that sub-tab: the parent tab's own
//     panels above it keep their authored height, the sub-tab's panels fill
//     the window under the app bar and the sub-tab bar (the page may scroll
//     to reach them; fit-subtab-1920.png). (h) the downloaded HTML (⤓ HTML)
//     fits itself offline, scaling its pre-rendered SVGs by viewBox (no wasm
//     to redraw them), and falls back on the overloaded tab.
import { createRequire } from "module";
import { readFileSync } from "fs";

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

const SUB_SQL = `SELECT 'Main'::TAB;
${chart(11)}${chart(12)}SELECT 'Fitted'::SUBTAB;
SELECT 'fit'::FIT;
${bars(13)}SELECT 360::HEIGHT;
${chart(14)}${bars(15)}${chart(16)}SELECT 'Other'::SUBTAB;
${chart(17)}SELECT 'Second'::TAB;
${chart(18)}`;

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
async function open(viewport, query = "", theme = "", sql = SQL) {
  const page = await browser.newPage({ viewport, reducedMotion: "reduce", colorScheme: theme || "light", acceptDownloads: true });
  const errs = [];
  page.on("pageerror", (e) => errs.push(String(e)));
  await page.goto(`${base}${query}#sql=${b64(sql)}`);
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

// (g) a ::FIT sub-tab: only its own panels fit
{
  const vp = { width: 1920, height: 1080 };
  // the parent tab's panels (in the tab pane, outside the sub-tab wrap)
  const parentHeights = (page) =>
    page.evaluate(() =>
      [...document.querySelectorAll(".dash .tabpane:not([style*='none']) .panel")].filter((p) => !p.closest(".subtabwrap")).map((p) => p.getBoundingClientRect().height)
    );
  const base0 = await open(vp, "?fit=0", "", SUB_SQL);
  const before = await parentHeights(base0.page);
  await base0.page.close();
  const { page, errs } = await open(vp, "", "", SUB_SQL);
  const after = await parentHeights(page);
  check(before.length === 2 && after.length === 2 && before.every((h, k) => Math.abs(h - after[k]) < 0.5), `(g) the parent tab's panels keep their authored height (${before.map(Math.round)} vs ${after.map(Math.round)})`);
  const g = await page.evaluate(() => {
    const pane = document.querySelector(".subtabpane:not([style*='none'])");
    const bar = pane.parentElement.previousElementSibling;
    const head = document.querySelector("body > header").getBoundingClientRect().height;
    const pad = parseFloat(getComputedStyle(document.querySelector(".dash")).paddingBottom);
    const bcs = getComputedStyle(bar);
    const region = pane.getBoundingClientRect().bottom - bar.getBoundingClientRect().top + parseFloat(bcs.marginTop);
    const panels = [...pane.querySelectorAll(".panel")];
    const charts = panels.map((p) => {
      const holder = p.querySelector(":scope > .panel-svg");
      const svg = holder.querySelector(":scope > svg");
      return { vbH: svg.viewBox.baseVal.height, shownH: svg.getBoundingClientRect().height, vbW: svg.viewBox.baseVal.width, holderW: holder.getBoundingClientRect().width, over: p.scrollHeight > p.clientHeight + 1 };
    });
    return { state: document.body.dataset.dpFit, region, room: innerHeight - head - pad, n: panels.length, charts, fitted: panels.every((p) => p.classList.contains("dp-fit")), parentFitted: [...document.querySelectorAll(".tabpane .panel.dp-fit")].filter((p) => !p.closest(".subtabwrap")).length };
  });
  check(g.state === "on" && g.fitted && g.n === 4, `(g) the ::FIT sub-tab is fitted (${g.state}, ${g.n} panels)`);
  check(g.parentFitted === 0, "(g) no parent-tab panel is resized");
  check(g.region <= g.room + 1 && g.region >= g.room - 24, `(g) the sub-tab bar + its panels fill the window under the app bar (${Math.round(g.region)} of ${Math.round(g.room)} px)`);
  check(g.charts.every((c) => Math.abs(c.vbH - c.shownH) <= 2 && Math.abs(c.vbW - c.holderW) <= 2 && !c.over), "(g) its charts are redrawn at their panels' size, nothing overflows");
  // scrolled to the sub-tab: every one of its panels is on screen
  await page.evaluate(() => {
    const bar = document.querySelector(".subtabbar");
    window.scrollTo(0, bar.getBoundingClientRect().top + window.scrollY - document.querySelector("body > header").getBoundingClientRect().height);
  });
  const vis = await page.evaluate(() =>
    [...document.querySelectorAll(".subtabpane:not([style*='none']) .panel")].every((p) => {
      const r = p.getBoundingClientRect();
      return r.top >= 0 && r.bottom <= innerHeight + 1;
    })
  );
  check(vis, "(g) scrolled to the sub-tab, all of its panels are inside the window");
  if (shots) await page.screenshot({ path: `${shots}/fit-subtab-1920.png` });
  // the other sub-tab (no ::FIT) is not fitted
  await page.click(".subtabbar .subtab-btn:text-is('Other')");
  await page.waitForTimeout(400);
  check((await page.evaluate(() => document.body.dataset.dpFit)) === "off", "(g) the sibling sub-tab without ::FIT is not fitted");
  check(!errs.length, `(g) no page errors: ${errs.join(" | ")}`);
  await page.close();
}

// (h) the downloaded HTML fits itself offline
{
  const { page, errs } = await open({ width: 1920, height: 1080 });
  const [dl] = await Promise.all([page.waitForEvent("download"), page.click("#dlhtml")]);
  const html = readFileSync(await dl.path(), "utf8");
  await page.close();
  for (const vp of [
    { width: 1366, height: 768 },
    { width: 1920, height: 1080 },
  ]) {
    const tag = `(h) [export ${vp.width}×${vp.height}]`;
    const off = await browser.newContext({ offline: true, viewport: vp, reducedMotion: "reduce" });
    const op = await off.newPage();
    const exportErrs = [];
    op.on("pageerror", (e) => exportErrs.push(String(e)));
    await op.route("**/*", (r) => (r.request().url().startsWith("http") ? (r.abort(), undefined) : r.continue()));
    await op.setContent(html, { waitUntil: "load" });
    await op.waitForTimeout(300);
    const m = await measure(op);
    check(m.state === "on" && !m.scroll && inView(m), `${tag} the ::FIT tab fits (${m.state}, scroll ${m.scroll})`);
    const svgs = await op.evaluate(() =>
      [...document.querySelectorAll(".dash .panel.dp-fit")].filter((p) => p.getClientRects().length).map((p) => {
        const holder = p.querySelector(":scope > .panel-svg");
        const svg = holder && holder.querySelector(":scope > svg");
        if (!svg) return null;
        const r = svg.getBoundingClientRect();
        return { scaled: svg.hasAttribute("data-dp-scaled"), attrH: +svg.getAttribute("height"), shownH: r.height, par: svg.getAttribute("preserveAspectRatio") || "", transform: getComputedStyle(svg).transform, fits: r.bottom <= p.getBoundingClientRect().bottom + 0.5 };
      }).filter(Boolean)
    );
    check(svgs.length >= 3 && svgs.every((v) => v.scaled && Math.abs(v.attrH - v.shownH) <= 1 && v.fits), `${tag} each chart's SVG is re-scaled to its fitted panel by its height / viewBox: ${JSON.stringify(svgs.map((v) => [v.attrH, Math.round(v.shownH)]))}`);
    check(svgs.every((v) => !/none/.test(v.par) && (v.transform === "none" || v.transform === "")), `${tag} uniformly (no preserveAspectRatio="none"), no CSS transform`);
    await op.getByRole("button", { name: "Crowded", exact: true }).click();
    await op.waitForTimeout(300);
    const c = await measure(op);
    check(c.state === "skipped" && c.scroll && c.note, `${tag} the overloaded tab falls back to scrolling with the notice`);
    await op.getByRole("button", { name: "Details", exact: true }).click();
    await op.waitForTimeout(300);
    check((await measure(op)).state === "off", `${tag} the tab without ::FIT is not fitted`);
    check(!exportErrs.length, `${tag} no page errors: ${exportErrs.join(" | ")}`);
    await off.close();
  }
  check(!errs.length, `(h) no page errors: ${errs.join(" | ")}`);
}

// (i) #418: a ::GROUP KPI strip keeps its own natural height under ::FIT —
// it is taken out of the row-share entirely (never stretched to fill the
// window), while chart rows share the rest; a ::HEIGHT right before the
// ::GROUP instead sets that row's share, like it does for a chart row.
{
  const vp = { width: 1920, height: 1080 };
  const sql = (heightLine = "") => `SELECT 'fit'::FIT;\n${heightLine}SELECT ''::GROUP;\n` + `SELECT count(*) ::METRIC, 'Extensions'::LABEL FROM range(5) t(i);\n` + `SELECT sum(i) ::METRIC, 'Downloads'::LABEL FROM range(5) t(i);\n` + `SELECT 1::ENDGROUP;\n` + `SELECT 6::COL; SELECT 320::HEIGHT;\n${chart(30)}SELECT 6::COL; SELECT 320::HEIGHT;\n${chart(31)}${bars(32)}`;
  // the tiles' authored (un-fitted) height — what "natural" means here
  const nat = await open(vp, "?fit=0", "", sql());
  const natH = await nat.page.evaluate(() => [...document.querySelectorAll(".metric")].map((p) => p.getBoundingClientRect().height));
  await nat.page.close();
  // no ::HEIGHT before the ::GROUP: fitted tiles stay at ~natH (at least the
  // tile's own minimum — 96 px plain, 36 px ::KPI_SIZE 'compact' — #405/#400),
  // not stretched to share the window with the chart rows.
  const { page, errs } = await open(vp, "", "", sql());
  const m = await measure(page);
  check(m.state === "on" && !m.scroll, `(i) the ::FIT tab fits, no scroll (${m.state}, scroll ${m.scroll})`);
  const fitH = await page.evaluate(() => [...document.querySelectorAll(".metric")].map((p) => p.getBoundingClientRect().height));
  const wantH = natH.map((h) => Math.max(h, 96));
  check(
    natH.length === 2 && fitH.length === 2 && fitH.every((h, k) => Math.abs(h - wantH[k]) <= 4),
    `(i) a ::GROUP KPI strip keeps its natural height under ::FIT, not stretched (natural ${natH.map(Math.round)}, wanted ${wantH.map(Math.round)}, fitted ${fitH.map(Math.round)})`
  );
  check(inView(m), "(i) every panel is inside the viewport");
  check(m.charts.length >= 2, `(i) charts on the tab (${m.charts.length})`);
  check(
    m.charts.every((c) => Math.abs((c.attrH || c.vbH) - c.shownH) <= 2 && Math.abs(c.vbW - c.holderW) <= 2),
    `(i) charts fill the rest, redrawn at their fitted size: ${JSON.stringify(m.charts.map((c) => [c.attrH, Math.round(c.shownH)]))}`
  );
  check(!errs.length, `(i) no page errors: ${errs.join(" | ")}`);
  await page.close();
  // a ::HEIGHT right before the ::GROUP sets its share instead — no longer
  // ignored: a much taller authored weight grows the row well past natural.
  const { page: hp, errs: hErrs } = await open(vp, "", "", sql("SELECT 420::HEIGHT;\n"));
  const hm = await measure(hp);
  const grown = hm.boxes.find((b) => b.kpi).h;
  check(grown > wantH[0] + 40, `(i) ::HEIGHT right before a ::GROUP sets its ::FIT share, not ignored (natural ${Math.round(wantH[0])}px, 420::HEIGHT -> ${Math.round(grown)}px)`);
  check(hm.state === "on" && !hm.scroll && inView(hm), `(i, ::HEIGHT) the tab fits, no scroll, every panel in the viewport (${hm.state}, scroll ${hm.scroll})`);
  check(!hErrs.length, `(i, ::HEIGHT) no page errors: ${hErrs.join(" | ")}`);
  await hp.close();
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
