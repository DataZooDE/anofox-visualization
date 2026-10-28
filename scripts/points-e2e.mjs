// Browser e2e for line/area markers on dense series (#631): a 7-day series at
// 15-minute resolution (672 points; one and two series) draws its markers
// uninked (ggplot-rs PointVisibility) — they stay hover/click targets: the
// crosshair tooltip still shows and inks the points under it, hovering a
// point inks it, and a ::LINK click on the chart still drills through. A
// 12-point series keeps visible markers; 'always'::POINTS / 'none'::POINTS
// override the automatic rule both ways.
//
//   wasm-pack build --target web --out-dir web/pkg --profile wasm --no-default-features --features wasm
//   python3 serve_nocache.py 8813 &
//   NODE_PATH=<playwright>/node_modules node scripts/points-e2e.mjs [--url http://127.0.0.1:8813/]
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

const DENSE = `SELECT TIMESTAMP '2026-10-01' + INTERVAL 15 MINUTE * i AS t, (i * 37) % 101 AS v FROM range(0, 672) r(i)`;
const SQL = `
SELECT 'Points e2e'::DASHBOARD_TITLE;
SELECT 'Main'::TAB;
SELECT t::XAXIS, v::LINECHART, '#Other'::LINK, 'Dense'::TITLE FROM (${DENSE});
SELECT t::XAXIS, s::CATEGORY, v::LINECHART, 'Dense2'::TITLE FROM (SELECT t, s, v + CASE s WHEN 'b' THEN 50 ELSE 0 END AS v FROM (${DENSE}), (VALUES ('a'), ('b')) c(s)) ORDER BY s, t;
SELECT m::XAXIS, v::LINECHART, 'Sparse'::TITLE FROM (SELECT DATE '2026-01-01' + INTERVAL 1 MONTH * i AS m, (i * 7) % 11 AS v FROM range(0, 12) r(i));
SELECT t::XAXIS, v::LINECHART, 'always'::POINTS, 'Always'::TITLE FROM (${DENSE});
SELECT m::XAXIS, v::AREACHART, 'none'::POINTS, 'None'::TITLE FROM (SELECT DATE '2026-01-01' + INTERVAL 1 MONTH * i AS m, (i * 7) % 11 AS v FROM range(0, 12) r(i));
SELECT 'Other'::TAB;
SELECT 'Arrived'::MARKDOWN;
`;

const browser = await chromium.launch();
const page = await browser.newPage({ viewport: { width: 1400, height: 2400 } });
const errs = [];
page.on("pageerror", (e) => errs.push(String(e)));
await page.goto(`${base}?embed=1#sql=${b64(SQL)}`);
await page.waitForFunction(
  () => document.querySelectorAll(".dash .panel-svg > svg[aria-label]").length >= 5 && !document.body.classList.contains("loading"),
  null,
  { timeout: 120000 }
);
await page.waitForTimeout(300);

// Per panel: the data points and how many are inked (computed fill-opacity).
const stats = (title) =>
  page.evaluate((title) => {
    const panel = [...document.querySelectorAll(".dash .panel")].find((p) => p.querySelector(".panel-title")?.textContent.trim() === title);
    const pts = [...(panel?.querySelectorAll('.panel-svg > svg circle[data-geom="point"]') || [])];
    return {
      n: pts.length,
      inked: pts.filter((c) => +getComputedStyle(c).fillOpacity > 0).length,
      titled: pts.filter((c) => c.querySelector("title") || c.hasAttribute("data-tip")).length,
    };
  }, title);

for (const t of ["Dense", "Dense2", "Always", "Sparse", "None"]) {
  const s = await stats(t);
  const wantInked = t === "Always" || t === "Sparse";
  if (!s.n) fail(`${t}: no point marks`);
  else if (wantInked ? s.inked !== s.n : s.inked !== 0) fail(`${t}: ${s.inked}/${s.n} markers inked`);
  else if (s.titled !== s.n) fail(`${t}: ${s.titled}/${s.n} markers keep their hover metadata`);
  else ok(`${t}: ${s.inked}/${s.n} inked, every marker a hover target`);
}

// The crosshair over the dense chart: a tooltip, and the column's point inked.
const box = await page.evaluate(() => {
  const panel = [...document.querySelectorAll(".dash .panel")].find((p) => p.querySelector(".panel-title")?.textContent.trim() === "Dense");
  panel.scrollIntoView({ block: "center" });
  const r = panel.querySelector(".panel-svg > svg").getBoundingClientRect();
  return { x: r.left + r.width / 2, y: r.top + r.height / 2 };
});
await page.mouse.move(box.x - 5, box.y);
await page.mouse.move(box.x, box.y);
await page.waitForTimeout(150);
const cross = await page.evaluate(() => {
  const panel = [...document.querySelectorAll(".dash .panel")].find((p) => p.querySelector(".panel-title")?.textContent.trim() === "Dense");
  const inked = [...panel.querySelectorAll('circle[data-geom="point"]')].filter((c) => +getComputedStyle(c).fillOpacity > 0).length;
  const tip = document.getElementById("dp-tip");
  return { tip: !!tip?.classList.contains("show") && tip.textContent.trim().length > 0, inked };
});
if (!cross.tip) fail("dense chart: no crosshair tooltip on hover");
else if (cross.inked < 1) fail("dense chart: the crosshair inks no point");
else ok(`dense chart: crosshair tooltip, ${cross.inked} point(s) inked under it`);

// Leaving clears the ink again.
await page.mouse.move(5, 5);
await page.waitForTimeout(100);
if ((await stats("Dense")).inked !== 0) fail("dense chart: points stay inked after the pointer leaves");

// A click on an (uninked) marker still follows the chart's ::LINK.
const hit = await page.evaluate(() => {
  const panel = [...document.querySelectorAll(".dash .panel")].find((p) => p.querySelector(".panel-title")?.textContent.trim() === "Dense");
  const pts = [...panel.querySelectorAll('circle[data-geom="point"]')];
  const c = pts[Math.floor(pts.length / 2)];
  const r = c.getBoundingClientRect();
  return { x: r.left + r.width / 2, y: r.top + r.height / 2 };
});
await page.mouse.click(hit.x, hit.y);
await page.waitForFunction(() => !document.body.classList.contains("loading"), null, { timeout: 120000 });
await page.waitForTimeout(300);
const tab = await page.evaluate(() => document.querySelector(".dash > .tabbar > .tab-btn.active")?.textContent.trim() || "");
if (tab !== "Other") fail(`::LINK click on a dense marker did not drill through (active tab: ${tab})`);
else ok("::LINK click on an uninked marker drills through");

if (errs.length) fail("page errors: " + errs.join(" | "));
await browser.close();
console.log(`points-e2e: ${process.exitCode ? "FAILED" : "0 failed"}`);
