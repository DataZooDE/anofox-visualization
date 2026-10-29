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

await browser.close();
console.log(`figure-e2e: ${process.exitCode ? "failed" : "0 failed"}`);
