// Browser e2e for tooltip trends on a map (M3b, #642): a list ::SPARKLINE
// next to a map's input is not its measure — the map still draws (full width,
// classed fill), and hovering a feature shows a mini chart drawn by
// render_cell plus the Rust-written data-trend line. A ::SPARKBAR list draws
// bars the same way; a time map's tooltip follows the frame shown.
//
//   build web/pkg (scripts/build-wasm-pkg.sh)
//   python3 serve_nocache.py 8813 &
//   NODE_PATH=<playwright>/node_modules node scripts/map-spark-e2e.mjs [--url http://127.0.0.1:8813/]
import { createRequire } from "module";

const require = createRequire(import.meta.url);
const { chromium } = require("playwright");
const arg = (k, d) => {
  const i = process.argv.indexOf(k);
  return i > 0 ? process.argv[i + 1] : d;
};
const base = arg("--url", "http://127.0.0.1:8813/");
const fail = (m) => {
  console.error("FAIL: " + m);
  process.exitCode = 1;
};
const ok = (m) => console.log("ok: " + m);
const b64 = (s) => Buffer.from(s, "utf8").toString("base64");

const SQL = `
SELECT 'Map spark e2e'::DASHBOARD_TITLE;
SELECT 6::COL;
SELECT w AS "Weekly" ::SPARKLINE, wkt ::MAP, v AS "On time" ::BARCHART, '0,50,100'::CLASSES, region ::LABEL, 'Lines'::TITLE
FROM (VALUES
    ('POLYGON((0 0, 0 10, 10 10, 10 0, 0 0))', 'North', 40, [12, 9, 21, 18]),
    ('POLYGON((10 0, 10 10, 20 10, 20 0, 10 0))', 'South', 80, [5, 6, 7, 8])
) t(wkt, region, v, w);
SELECT 6::COL;
SELECT wkt ::MAP, v AS "On time" ::BARCHART, region ::LABEL, w AS "Weekly" ::SPARKBAR, 'Bars'::TITLE
FROM (VALUES
    ('POLYGON((0 0, 0 10, 10 10, 10 0, 0 0))', 'North', 40, [1, 3, 2]),
    ('POLYGON((10 0, 10 10, 20 10, 20 0, 10 0))', 'South', 80, [4, 4, 6])
) t(wkt, region, v, w);
SELECT 6::COL;
SELECT wkt ::MAP, wk AS "Week" ::FRAME, v AS "On time" ::BARCHART, region ::LABEL, w AS "Weekly" ::SPARKLINE, 'Frames'::TITLE
FROM (VALUES
    ('POLYGON((0 0, 0 10, 10 10, 10 0, 0 0))', 'North', 1, 40, [1, 2]),
    ('POLYGON((0 0, 0 10, 10 10, 10 0, 0 0))', 'North', 2, 60, [10, 30]),
    ('POLYGON((10 0, 10 10, 20 10, 20 0, 10 0))', 'South', 1, 80, [3, 4]),
    ('POLYGON((10 0, 10 10, 20 10, 20 0, 10 0))', 'South', 2, 90, [30, 40])
) t(wkt, region, wk, v, w) ORDER BY wk, region;
`;

const browser = await chromium.launch();
const page = await browser.newPage({ viewport: { width: 1400, height: 1400 } });
const errs = [];
page.on("pageerror", (e) => errs.push(String(e)));
await page.goto(`${base}?embed=1#sql=${b64(SQL)}`);
await page.waitForFunction(
  () => document.querySelectorAll(".dash .panel-svg > svg[data-map]").length >= 3 && !document.body.classList.contains("loading"),
  null,
  { timeout: 120000 }
);
await page.waitForTimeout(300);

// Hover the feature labelled `label` in panel `title`; return the tooltip parts.
const hover = async (title, label) => {
  const box = await page.evaluate(
    ({ title, label }) => {
      const panel = [...document.querySelectorAll(".dash .panel")].find((p) => p.querySelector(".panel-title")?.textContent.trim() === title);
      const el = [...panel.querySelectorAll("svg[data-map] [data-row]")].find((e) => e.getAttribute("data-label") === label);
      if (!el) return null;
      el.scrollIntoView({ block: "center" });
      const r = el.getBoundingClientRect();
      return { x: r.left + r.width / 2, y: r.top + r.height / 2, trend: el.getAttribute("data-trend") };
    },
    { title, label }
  );
  if (!box) return null;
  await page.mouse.move(5, 5);
  await page.mouse.move(box.x, box.y);
  await page.waitForTimeout(120);
  const tip = await page.evaluate(() => {
    const t = document.getElementById("dp-tip");
    return {
      show: t.classList.contains("show"),
      head: t.querySelector(".tip-head")?.textContent || "",
      chart: !!t.querySelector(".tip-spark svg.tip-spark-svg"),
      polylines: t.querySelectorAll(".tip-spark svg polyline, .tip-spark svg path").length,
      bars: t.querySelectorAll(".tip-spark svg rect[data-geom]").length,
      trend: t.querySelector(".tip-trend")?.textContent || "",
    };
  });
  return { ...tip, attr: box.trend };
};

// The map is a map (not a 460 px sparkline strip) and keeps its classed measure.
const shape = await page.evaluate(() => {
  const panel = [...document.querySelectorAll(".dash .panel")].find((p) => p.querySelector(".panel-title")?.textContent.trim() === "Lines");
  const svg = panel.querySelector(".panel-svg > svg");
  return { w: +svg.getAttribute("width"), classed: svg.querySelectorAll("[data-class]").length };
});
if (shape.w <= 460 || shape.classed < 2) fail(`the spark column took over the map: width ${shape.w}, classed marks ${shape.classed}`);
else ok(`map keeps its measure (width ${shape.w}, ${shape.classed} classed features)`);

let t = await hover("Lines", "North");
if (!t || !t.show) fail("Lines: no tooltip on hover");
else if (!t.chart) fail("Lines: no mini chart in the tooltip");
else if (t.trend !== t.attr || !t.trend.startsWith("Trend: 12 → 18")) fail(`Lines: trend "${t.trend}" vs data-trend "${t.attr}"`);
else ok(`Lines: mini chart + "${t.trend}"`);

t = await hover("Bars", "South");
if (!t || !t.chart) fail("Bars: no mini chart");
else if (t.bars < 3) fail(`Bars: ${t.bars} bars in the tooltip chart`);
else ok(`Bars: ${t.bars} bars + "${t.trend}"`);

// a time map shows the last frame (week 2): North's list is [10, 30]
t = await hover("Frames", "North");
if (!t || !t.trend.startsWith("Trend: 10 → 30")) fail(`Frames: trend "${t && t.trend}" is not week 2's`);
else ok(`Frames: the shown frame's trend ("${t.trend}")`);

if (errs.length) fail("page errors: " + errs.join(" | "));
await browser.close();
console.log(`map-spark-e2e: ${process.exitCode ? "FAILED" : "0 failed"}`);
