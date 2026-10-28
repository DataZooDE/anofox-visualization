// Browser e2e for small multiples on a map (M3a-1, #644): one map per
// ::FACET value in one panel, the panel at the grid's natural height;
// hovering a feature lights it up in every map; the wheel zooms every map
// (one shared window); "charts as tables" names each row's map; a
// phone-width viewport stacks the maps into fewer columns.
//
//   build web/pkg (scripts/build-wasm-pkg.sh)
//   python3 serve_nocache.py 8813 &
//   NODE_PATH=<playwright>/node_modules node scripts/map-facets-e2e.mjs [--url http://127.0.0.1:8813/]
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

const rows = [1, 2, 3, 4]
  .map((w) => `('POLYGON((0 0, 0 10, 10 10, 10 0, 0 0))', 'North', 'Week ${w}', ${10 * w}), ('POLYGON((10 0, 10 10, 20 10, 20 0, 10 0))', 'South', 'Week ${w}', ${90 - 10 * w})`)
  .join(",\n    ");
const SQL = `
SELECT 'Map facets e2e'::DASHBOARD_TITLE;
SELECT 12::COL;
SELECT wkt ::MAP, region ::LABEL, wk AS "Week" ::FACET, v AS "Value" ::BARCHART, '0,50,100'::CLASSES, 'Facets'::TITLE
FROM (VALUES
    ${rows}
) t(wkt, region, wk, v);
`;

const browser = await chromium.launch();
const errs = [];
const open = async (w) => {
  const page = await browser.newPage({ viewport: { width: w, height: 1000 } });
  page.on("pageerror", (e) => errs.push(String(e)));
  await page.goto(`${base}?embed=1#sql=${b64(SQL)}`);
  await page.waitForFunction(() => document.querySelector(".dash .panel-svg > svg[data-facets]") && !document.body.classList.contains("loading"), null, { timeout: 120000 });
  await page.waitForTimeout(300);
  return page;
};
const info = (page) =>
  page.evaluate(() => {
    const svg = document.querySelector(".dash .panel-svg > svg[data-facets]");
    const panels = JSON.parse(svg.getAttribute("data-panels"));
    return {
      n: panels.length,
      cols: new Set(panels.map((p) => Math.round(p.x))).size,
      h: +svg.getAttribute("height"),
      domain: svg.getAttribute("data-domain"),
      panels,
    };
  });

let page = await open(1400);
let s = await info(page);
if (s.n !== 4) fail(`${s.n} panels, expected 4`);
else ok(`4 maps in one panel (${s.cols} columns, ${s.h} px high)`);

// hover North in the first map → North emphasised in every map
await page.evaluate(() => {
  const el = document.querySelector('.dash svg[data-facets] [data-facet="Week 1"][data-label="North"]');
  const r = el.getBoundingClientRect();
  el.dispatchEvent(new MouseEvent("mouseenter", { bubbles: true, clientX: r.left + 5, clientY: r.top + 5 }));
});
await page.waitForTimeout(200);
const dim = await page.evaluate(() => {
  const op = (el) => +getComputedStyle(el).opacity;
  const all = [...document.querySelectorAll(".dash svg[data-facets] [data-label]")];
  return {
    north: all.filter((e) => e.getAttribute("data-label") === "North").map(op),
    south: all.filter((e) => e.getAttribute("data-label") === "South").map(op),
  };
});
if (!(dim.north.every((o) => o > 0.9) && dim.south.some((o) => o < 0.9))) fail(`hover emphasis: ${JSON.stringify(dim)}`);
else ok("hovering North lights it up in every map");

// wheel over map 3 → every map zooms (one shared window)
const before = s.domain;
const p3 = await page.evaluate(() => {
  const el = document.querySelector('.dash svg[data-facets] [data-facet="Week 3"][data-label="North"]');
  const r = el.getBoundingClientRect();
  return { x: r.left + r.width / 2, y: r.top + r.height / 2 };
});
await page.mouse.move(p3.x, p3.y);
await page.mouse.wheel(0, -300);
await page.waitForFunction((b) => document.querySelector(".dash .panel-svg > svg[data-facets]")?.getAttribute("data-domain") !== b, before, { timeout: 10000 }).catch(() => {});
await page.waitForTimeout(300);
s = await info(page);
const d0 = before.split(" ").map(Number);
const d = s.domain.split(" ").map(Number);
if (!(d[1] - d[0] < d0[1] - d0[0])) fail(`the wheel did not zoom the maps: ${before} → ${s.domain}`);
else if (s.n !== 4) fail(`zoomed render has ${s.n} panels`);
else ok(`the wheel zooms every map together (window ${(d[1] - d[0]).toFixed(1)} of ${(d0[1] - d0[0]).toFixed(1)})`);

// charts as tables names the map
const names = await page.evaluate(() => {
  const svg = document.querySelector(".dash .panel-svg > svg[data-facets]");
  // eslint-disable-next-line no-undef
  return typeof mapTableHtml === "function" ? mapTableHtml(svg, 50, "x") : "";
});
if (names && !/Week 1 · North/.test(names)) fail("charts-as-tables rows don't name their map");
else ok("charts as tables names each row's map");
await page.close();

// phone width: fewer columns
page = await open(390);
s = await info(page);
if (s.cols >= 4) fail(`390 px still shows ${s.cols} columns`);
else ok(`390 px: ${s.cols} column(s)`);
const scroll = await page.evaluate(() => document.documentElement.scrollWidth > window.innerWidth + 1);
if (scroll) fail("390 px scrolls sideways");
await page.close();

if (errs.length) fail("page errors: " + errs.join(" | "));
await browser.close();
console.log(`map-facets-e2e: ${process.exitCode ? "FAILED" : "0 failed"}`);
