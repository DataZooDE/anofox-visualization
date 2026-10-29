// ::FIGURE … ::ENDFIGURE (#661) in a real browser: the cells render as ONE
// card (one SVG with a nested <svg data-panel> per cell, tagged A, B, C),
// titled by the ::FIGURE value; the cells are not drawn again as panels of
// their own; a mark's tooltip reads its own cell's data, and the
// x-crosshair works per cell (#695); dark mode and a narrow window
// re-render it; no page errors.
//
//   scripts/build-wasm-pkg.sh
//   python3 serve_nocache.py 8947 &
//   NODE_PATH=<dir with playwright>/node_modules node scripts/figure-e2e.mjs \
//       [--url http://127.0.0.1:8947/] [--shots <dir>]
import { createRequire } from "module";

const require = createRequire(import.meta.url);
const { chromium } = require("playwright");
const arg = (k, d) => {
  const i = process.argv.indexOf(k);
  return i > 0 ? process.argv[i + 1] : d;
};
const base = arg("--url", "http://127.0.0.1:8947/");
const shots = arg("--shots", "");
const fail = (m) => {
  console.error("FAIL: " + m);
  process.exitCode = 1;
};
const check = (ok, m) => (ok ? console.log("ok   " + m) : fail(m));
const b64 = (s) => Buffer.from(s, "utf8").toString("base64");

const SQL = `-- @title Figure e2e (#661)
CREATE OR REPLACE TEMP TABLE pts AS SELECT i, CASE WHEN i % 2 = 0 THEN 'reference' ELSE 'now' END AS period, (i % 50) / 5.0 AS x, 3 + 0.8 * ((i % 50) / 5.0) AS y FROM range(200) t(i);
SELECT 'Data drift: the inputs move, the rule holds' ::FIGURE, '(hist / scatter) | rule' ::LAYOUT, '1,4' ::HEIGHTS, '2,1' ::WIDTHS, 'x' ::AXES_COLLECT, true ::COLLECT_LEGENDS, 'A' ::TAG_LEVELS;
-- @name hist
SELECT x ::HISTOGRAM, period ::CATEGORY FROM pts;
-- @name scatter
SELECT x ::XAXIS, y ::SCATTER, period ::CATEGORY FROM pts;
-- @name rule
SELECT x ::XAXIS, avg(y) + 100 AS y ::LINECHART FROM pts GROUP BY ALL ORDER BY 1;
SELECT 1 ::ENDFIGURE;
SELECT x ::XAXIS, y ::LINECHART, 'After the figure' ::TITLE FROM (VALUES (1, 2), (2, 3), (3, 5)) v(x, y);
`;

const ready = (page) =>
  page.waitForFunction(
    () => /\d+ panels?/.test(document.getElementById("status")?.textContent || "") && !document.body.classList.contains("loading"),
    null,
    { timeout: 120000 }
  );

const browser = await chromium.launch();
const page = await browser.newPage({ viewport: { width: 1440, height: 1000 }, reducedMotion: "reduce" });
const errs = [];
page.on("pageerror", (e) => errs.push(String(e)));
await page.goto(`${base}?mode=view#sql=${b64(SQL)}`);
await ready(page);

const info = () =>
  page.evaluate(() =>
    [...document.querySelectorAll(".dash .panel")].map((p) => ({
      cls: p.className,
      title: p.querySelector(".panel-title")?.textContent || "",
      cells: [...p.querySelectorAll("svg[data-panel]")].map((s) => s.getAttribute("data-panel")),
      svgW: p.querySelector(".panel-svg > svg")?.getAttribute("width") || "",
    }))
  );
const panels = await info();
check(!errs.length, "no page errors: " + errs.join(" | "));
check(panels.length === 2, `2 panels: the figure + the chart after it (${panels.length})`);
const fig = panels[0];
check(/\bdp-figure\b/.test(fig.cls), `the figure is one card (${fig.cls})`);
check(fig.title === "Data drift: the inputs move, the rule holds", `titled by the ::FIGURE value (${fig.title})`);
check(fig.cells.join(",") === "A,B,C", `three tagged cells in one SVG (${fig.cells})`);
check(panels[1].title === "After the figure", `the next chart is its own card (${panels[1].title})`);

// A mark's tooltip is its own cell's: the line cell's y is avg + 100.
const tipOver = async (sel) => {
  const el = await page.$(`.dash .dp-figure ${sel}`);
  if (!el) return null;
  const box = await el.boundingBox();
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
  await page.waitForTimeout(250);
  return page.evaluate(() => document.querySelector(".dp-tip.show")?.textContent || "");
};
const lineTip = await tipOver('svg[data-panel="C"] circle');
check(/10[0-9]/.test(lineTip || ""), `a line point's tooltip reads its own cell (${lineTip})`);
const scatterTip = await tipOver('svg[data-panel="B"] circle');
check(scatterTip && !/10[0-9]\./.test(scatterTip), `a scatter point's tooltip reads its own cell (${scatterTip})`);
// #695: the x-crosshair works per cell — over the line cell it shows that
// cell's column; over the scatter cell (no shared-x series) it stays off.
const crossShown = () => page.evaluate(() => getComputedStyle(document.getElementById("dp-cross")).display !== "none");
const lineCell = await page.$('.dash .dp-figure svg[data-panel="C"]');
const lb = await lineCell.boundingBox();
await page.mouse.move(lb.x + lb.width * 0.55, lb.y + lb.height * 0.5);
await page.waitForTimeout(250);
const crossTip = await page.evaluate(() => document.querySelector(".dp-tip.show")?.textContent || "");
check((await crossShown()) && /10[0-9]/.test(crossTip), `the crosshair works in the line cell (${crossTip})`);
const crossX = await page.evaluate(() => parseFloat(document.getElementById("dp-cross").style.left));
check(crossX >= lb.x && crossX <= lb.x + lb.width, `…inside that cell (${crossX} in ${lb.x}–${lb.x + lb.width})`);
const scatterTip2 = await tipOver('svg[data-panel="B"] circle');
check(!(await crossShown()) && scatterTip2 && !/10[0-9]\./.test(scatterTip2), `no crosshair over the scatter cell, its point keeps its tooltip (${scatterTip2})`);
// …and on a plain chart the dashed line is actually drawn (it stayed hidden
// by the stylesheet's .dp-cross{display:none} since 10-15)
const plain = await (await page.$(".dash .panel:not(.dp-figure) .panel-svg svg")).boundingBox();
await page.mouse.move(plain.x + plain.width / 2, plain.y + plain.height / 2);
await page.waitForTimeout(250);
check(await crossShown(), "the crosshair line shows on a plain line chart");
await page.mouse.move(5, 5);
check(
  (await page.evaluate(() => [...document.querySelectorAll('.dash .dp-figure g[data-cell]')].map((g) => g.getAttribute("data-dp-name")).join(","))) === "hist,scatter,rule",
  "each cell carries its -- @name (data-dp-name)"
);
if (shots) await page.screenshot({ path: `${shots}/figure-1440-light.png`, fullPage: true });

// #695: the data view / charts-as-tables show each cell's rows, not the
// figure statement's own (title-only) row.
await page.mouse.move(5, 5);
await page.locator(".dash .dp-figure").hover();
await page.locator('.dash .dp-figure .dp-tool[title="Data view"]').click({ force: true });
const dv = await page.evaluate(() => [...document.querySelectorAll(".dp-modal .dp-dv-cap")].map((c) => c.textContent).join(","));
const dvRows = await page.evaluate(() => [...document.querySelectorAll(".dp-modal table.dp-dv")].map((t) => t.querySelectorAll("tbody tr").length));
check(dv === "hist,scatter,rule" && dvRows.every((n) => n > 0), `data view: a table per cell (${dv}; ${dvRows})`);
await page.click(".dp-modal-x");

// Dark mode re-renders the figure (same cells).
const before = await page.evaluate(() => document.querySelector(".dash .dp-figure .panel-svg").innerHTML.length);
await page.click("#dark");
await page.waitForFunction(() => document.documentElement.dataset.axVariant === "dark", null, { timeout: 15000 });
await ready(page);
const dark = await info();
check(dark[0].cells.join(",") === "A,B,C", `dark mode keeps the three cells (${dark[0].cells})`);
if (shots) await page.screenshot({ path: `${shots}/figure-1440-dark.png`, fullPage: true });
check(before > 0, "the figure SVG has content");

// A narrow window: no horizontal page overflow.
await page.setViewportSize({ width: 390, height: 1200 });
await page.waitForTimeout(800);
const overflow = await page.evaluate(() => document.documentElement.scrollWidth > document.documentElement.clientWidth + 1);
check(!overflow, "no horizontal overflow at 390px");
if (shots) await page.screenshot({ path: `${shots}/figure-390-dark.png`, fullPage: true });
check(!errs.length, "still no page errors: " + errs.join(" | "));

// #695: a cell's own ::LINK follows from its marks (the cell's rows); a
// cell without one keeps the default click.
const LINK_SQL = `-- @title Figure link e2e (#695)
SELECT 'Main' ::TAB;
SELECT 'Linked cells' ::FIGURE;
SELECT x ::XAXIS, y ::LINECHART, '#Detail' AS go ::LINK FROM (VALUES (1, 2), (2, 3), (3, 5)) v(x, y);
SELECT x ::XAXIS, y ::LINECHART FROM (VALUES (1, 4), (2, 1), (3, 2)) v(x, y);
SELECT 1 ::ENDFIGURE;
SELECT 'Detail' ::TAB;
SELECT 'The detail page.' ::MARKDOWN;
`;
{
  const p2 = await browser.newPage({ viewport: { width: 1200, height: 900 }, reducedMotion: "reduce" });
  const e2 = [];
  p2.on("pageerror", (e) => e2.push(String(e)));
  await p2.goto(`${base}?mode=view#sql=${b64(LINK_SQL)}`);
  await ready(p2);
  const active = () => p2.evaluate(() => document.querySelector(".tabbar .tab-btn.active, .tabbar .tab-btn[aria-selected='true']")?.textContent?.trim());
  const cellMark = (k) => p2.locator(`.dash .dp-figure g[data-cell="${k}"] circle.dp-hit`).first();
  await cellMark(1).click({ force: true });
  await p2.waitForTimeout(600);
  check((await active()) === "Main", `a cell without ::LINK doesn't navigate (${await active()})`);
  await cellMark(0).click({ force: true });
  await p2.waitForFunction(() => /Detail/.test(document.querySelector(".tabbar .tab-btn.active, .tabbar .tab-btn[aria-selected='true']")?.textContent || ""), null, { timeout: 15000 }).catch(() => {});
  check((await active()) === "Detail", `a cell's ::LINK follows from its mark (${await active()})`);
  check(!e2.length, "link figure: no page errors: " + e2.join(" | "));
  await p2.close();
}

// #695: a ::SELECT_AS right before a cell inside the block is that cell's:
// a click on its mark sets the named selection (read by the KPI after the
// figure); it is not drawn as a cell, nor applied to the next panel.
const SEL_SQL = `-- @title Figure select e2e (#695)
SELECT 'Picked' ::FIGURE;
SELECT 'region' ::SELECT_AS;
SELECT r ::XAXIS, n ::BARCHART FROM (VALUES ('north', 3), ('south', 5)) v(r, n);
SELECT x ::XAXIS, y ::LINECHART FROM (VALUES (1, 2), (2, 3), (3, 1)) v(x, y);
SELECT 1 ::ENDFIGURE;
SELECT coalesce(getvariable('region'), 'none') AS picked ::TEXT_SMALL;
`;
{
  const p3 = await browser.newPage({ viewport: { width: 1200, height: 900 }, reducedMotion: "reduce" });
  const e3 = [];
  p3.on("pageerror", (e) => e3.push(String(e)));
  await p3.goto(`${base}?mode=view#sql=${b64(SEL_SQL)}`);
  await ready(p3);
  const cells = await p3.evaluate(() => document.querySelectorAll(".dash .dp-figure g[data-cell]").length);
  check(cells === 2, `the ::SELECT_AS is no cell (${cells} cells)`);
  await p3.locator('.dash .dp-figure g[data-cell="0"] .dp-hit').first().click({ force: true });
  await p3.waitForFunction(() => /north|south/.test(document.querySelector(".dash .textcard .text-value")?.textContent || ""), null, { timeout: 15000 }).catch(() => {});
  const picked = await p3.evaluate(() => document.querySelector(".dash .textcard .text-value")?.textContent);
  check(/north|south/.test(picked || ""), `a click on the cell sets its named selection (${picked})`);
  check(!e3.length, "select figure: no page errors: " + e3.join(" | "));
  await p3.close();
}

// #695: zoom (?zoom=1) follows ::AXES_COLLECT 'x' — scrolling in the
// scatter zooms it and the histogram stacked above it (one shared x); the
// rule beside them keeps its own x; double-click resets.
{
  const zp = await browser.newPage({ viewport: { width: 1440, height: 1000 }, reducedMotion: "reduce" });
  const zerrs = [];
  zp.on("pageerror", (e) => zerrs.push(String(e)));
  await zp.goto(`${base}?mode=view&zoom=1#sql=${b64(SQL)}`);
  await zp.waitForFunction(() => /\d+ panels?/.test(document.getElementById("status")?.textContent || "") && document.querySelector(".dp-figure svg[data-panel]"), null, { timeout: 120000 });
  const cellsNow = () =>
    zp.evaluate(() =>
      [...document.querySelectorAll(".dp-figure svg[data-panel]")].map((s) => {
        const g = s.querySelector(":scope > g[data-cell]");
        return { k: +g.getAttribute("data-cell"), share: g.getAttribute("data-xshare"), x: s.getAttribute("data-xdomain") };
      }).sort((a, b) => a.k - b.k)
    );
  const before = await cellsNow();
  check(before[0].share === "0" && before[1].share === "0" && before[2].share === null, `the stacked hist and scatter share x, the rule doesn't (${JSON.stringify(before.map((c) => c.share))})`);
  const box = await zp.evaluate(() => {
    const s = [...document.querySelectorAll(".dp-figure svg[data-panel]")].find((s) => s.querySelector(':scope > g[data-cell="1"]'));
    const b = s.getBoundingClientRect();
    return { x: b.left + b.width / 2, y: b.top + b.height / 2 };
  });
  await zp.mouse.move(box.x, box.y);
  for (let i = 0; i < 3; i++) {
    await zp.mouse.wheel(0, -200);
    await zp.waitForTimeout(150);
  }
  await zp.waitForTimeout(300);
  const after = await cellsNow();
  const width = (x) => {
    const [a, b] = x.split(" ").map(Number);
    return b - a;
  };
  check(width(after[1].x) < width(before[1].x) * 0.8, `scrolling zooms the scatter's x (${before[1].x} → ${after[1].x})`);
  check(after[0].x === after[1].x, `the histogram above it takes the same x (${after[0].x} vs ${after[1].x})`);
  check(after[2].x === before[2].x, `the rule beside them keeps its own (${after[2].x})`);
  await zp.mouse.dblclick(box.x, box.y);
  await zp.waitForTimeout(300);
  const reset = await cellsNow();
  check(reset[1].x === before[1].x && reset[0].x === before[0].x, `double-click resets the figure (${reset[1].x})`);
  check(!zerrs.length, "zoom: no page errors: " + zerrs.join(" | "));
  await zp.close();
}

await browser.close();
console.log(`figure-e2e: ${process.exitCode ? "failed" : "0 failed"}`);
