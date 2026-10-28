// Browser e2e for the map zoom tools (M3d, #643): the wheel keeps the map
// point under the pointer (exact data-viewport mapping); double-click zooms
// to a feature (keeping the first click's selection) and resets on empty
// space; the reset chip; zoom to selection; find a place; + / − / 0 / z keys;
// zoom limits; a ::SYNC_ZOOM partner follows.
//
//   build web/pkg (scripts/build-wasm-pkg.sh)
//   python3 serve_nocache.py 8813 &
//   NODE_PATH=<playwright>/node_modules node scripts/map-tools-e2e.mjs [--url http://127.0.0.1:8813/]
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

const POLYS = `(VALUES
    ('POLYGON((0 0, 0 10, 10 10, 10 0, 0 0))', 'North', 1),
    ('POLYGON((10 0, 10 10, 20 10, 20 0, 10 0))', 'South', 2),
    ('POLYGON((20 0, 20 10, 40 10, 40 0, 20 0))', 'East', 3),
    ('POLYGON((0 10, 0 12, 2 12, 2 10, 0 10))', 'Tiny', 4)
) t(wkt, region, v)`;
const SQL = `
SELECT 'Map tools e2e'::DASHBOARD_TITLE;
SELECT 6::COL;
SELECT wkt ::MAP, region ::LABEL, v AS "Value" ::BARCHART, 'g'::SYNC_ZOOM, 'Main'::TITLE FROM ${POLYS};
SELECT 6::COL;
SELECT wkt ::MAP, region ::LABEL, 'g'::SYNC_ZOOM, 'Partner'::TITLE FROM ${POLYS};
SELECT 12::COL;
SELECT region AS "Region" ::TABLE, 'Picked'::TITLE FROM (VALUES ('North'), ('South'), ('East'), ('Tiny')) t(region)
WHERE coalesce(getvariable('selected'), '') IN ('', region);
`;

const browser = await chromium.launch();
const page = await browser.newPage({ viewport: { width: 1400, height: 1100 } });
const errs = [];
page.on("pageerror", (e) => errs.push(String(e)));
await page.goto(`${base}?embed=1#sql=${b64(SQL)}`);
await page.waitForFunction(
  () => document.querySelectorAll(".dash .panel-svg > svg[data-map]").length >= 2 && document.querySelector(".dash table") && !document.body.classList.contains("loading"),
  null,
  { timeout: 120000 }
);
await page.waitForTimeout(400);
const settle = async () => {
  await page.waitForFunction(() => !document.body.classList.contains("loading"), null, { timeout: 120000 });
  await page.evaluate(() => new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r))));
  await page.waitForTimeout(150);
};

// A panel's rendered window and viewport, plus helpers, in the page.
const state = (title) =>
  page.evaluate((t) => {
    const panel = [...document.querySelectorAll(".dash .panel")].find((p) => p.querySelector(".panel-title")?.textContent.trim() === t);
    const svg = panel.querySelector(".panel-svg > svg");
    const d = svg.getAttribute("data-domain").split(" ").map(Number);
    const vp = svg.getAttribute("data-viewport").split(" ").map(Number);
    const chip = panel.querySelector(".dp-map-reset");
    return { d, vp, w: d[1] - d[0], chip: !!chip && !chip.hidden };
  }, title);
// Client point of map coordinate (x, y) in panel `title`.
const at = (title, x, y) =>
  page.evaluate(
    ({ t, x, y }) => {
      const panel = [...document.querySelectorAll(".dash .panel")].find((p) => p.querySelector(".panel-title")?.textContent.trim() === t);
      const svg = panel.querySelector(".panel-svg > svg");
      const d = svg.getAttribute("data-domain").split(" ").map(Number);
      const vp = svg.getAttribute("data-viewport").split(" ").map(Number);
      const ux = vp[0] + ((x - d[0]) / (d[1] - d[0])) * vp[2];
      const uy = vp[1] + ((d[3] - y) / (d[3] - d[2])) * vp[3];
      const p = new DOMPoint(ux, uy).matrixTransform(svg.getScreenCTM());
      return { x: p.x, y: p.y };
    },
    { t: title, x, y }
  );
const mapPointAt = (title, cx, cy) =>
  page.evaluate(
    ({ t, cx, cy }) => {
      const panel = [...document.querySelectorAll(".dash .panel")].find((p) => p.querySelector(".panel-title")?.textContent.trim() === t);
      const svg = panel.querySelector(".panel-svg > svg");
      const d = svg.getAttribute("data-domain").split(" ").map(Number);
      const vp = svg.getAttribute("data-viewport").split(" ").map(Number);
      const u = new DOMPoint(cx, cy).matrixTransform(svg.getScreenCTM().inverse());
      return [d[0] + ((u.x - vp[0]) / vp[2]) * (d[1] - d[0]), d[3] - ((u.y - vp[1]) / vp[3]) * (d[3] - d[2])];
    },
    { t: title, cx, cy }
  );
const contains = (s, x0, x1, y0, y1) => s.d[0] <= x0 + 1e-6 && s.d[1] >= x1 - 1e-6 && s.d[2] <= y0 + 1e-6 && s.d[3] >= y1 - 1e-6;
const picked = () => page.evaluate(() => [...document.querySelectorAll(".dash table tbody tr")].map((r) => r.textContent.trim()));

const home = await state("Main");
await page.evaluate(() => [...document.querySelectorAll(".dash .panel")].find((p) => p.querySelector(".panel-title")?.textContent.trim() === "Main").scrollIntoView({ block: "center" }));

// 1. wheel keeps the point under the pointer
let p = await at("Main", 15, 5);
const before = await mapPointAt("Main", p.x, p.y);
await page.mouse.move(p.x, p.y);
await page.mouse.wheel(0, -200);
await settle();
const after = await mapPointAt("Main", p.x, p.y);
let s = await state("Main");
const px = s.w / s.vp[2]; // map units per viewBox unit
if (!(s.w < home.w)) fail(`wheel did not zoom in (${s.w} vs ${home.w})`);
else if (Math.hypot(after[0] - before[0], after[1] - before[1]) > 2 * px * 2) fail(`wheel anchor drifted: ${before} → ${after}`);
else ok(`wheel zooms about the pointer (drift ${Math.hypot(after[0] - before[0], after[1] - before[1]).toFixed(3)} map units)`);
if (!s.chip) fail("no reset chip while zoomed");
// 8. the ::SYNC_ZOOM partner followed
const partner = await state("Partner");
if (Math.abs(partner.w - s.w) / s.w > 0.05) fail(`partner did not follow (${partner.w} vs ${s.w})`);
else ok("the ::SYNC_ZOOM partner follows");

// 4. the chip resets
await page.evaluate(() => [...document.querySelectorAll(".dash .panel")].find((p) => p.querySelector(".panel-title")?.textContent.trim() === "Main").querySelector(".dp-map-reset").click());
await settle();
s = await state("Main");
if (Math.abs(s.w - home.w) > 1e-6 || s.chip) fail(`chip did not reset (w ${s.w}, chip ${s.chip})`);
else ok("the reset chip returns home and hides");

// 2. double-click a feature: zoom to it, keep the first click's selection
p = await at("Main", 1, 11); // inside Tiny
await page.mouse.dblclick(p.x, p.y);
await settle();
s = await state("Main");
const rows = await picked();
if (!contains(s, 0, 2, 10, 12) || s.w > home.w / 2) fail(`double-click did not zoom to Tiny: ${s.d}`);
else ok(`double-click zooms to the feature (width ${s.w.toFixed(2)} of ${home.w.toFixed(2)})`);
if (rows.length !== 1 || !/Tiny/.test(rows[0])) fail(`the first click's selection was not kept: ${JSON.stringify(rows)}`);
else ok("the first click's selection stays (the second click is swallowed)");

// 3. double-click empty space: home
p = await page.evaluate(() => {
  const panel = [...document.querySelectorAll(".dash .panel")].find((p) => p.querySelector(".panel-title")?.textContent.trim() === "Main");
  const svg = panel.querySelector(".panel-svg > svg");
  const vp = svg.getAttribute("data-viewport").split(" ").map(Number);
  const q = new DOMPoint(vp[0] + vp[2] - 3, vp[1] + 3).matrixTransform(svg.getScreenCTM());
  return { x: q.x, y: q.y };
});
await page.mouse.dblclick(p.x, p.y);
await settle();
s = await state("Main");
if (Math.abs(s.w - home.w) > 1e-6) fail(`double-click on empty space did not reset (${s.w})`);
else ok("double-click on empty space resets");

// 5. zoom to selection: select Tiny (the empty-space click above cleared it)
p = await at("Main", 1, 11);
await page.mouse.click(p.x, p.y);
await settle();
await page.evaluate(() => {
  const panel = [...document.querySelectorAll(".dash .panel")].find((p) => p.querySelector(".panel-title")?.textContent.trim() === "Main");
  panel.querySelector('.dp-tool[title="Zoom to selection"]').click();
});
await settle();
s = await state("Main");
if (!contains(s, 0, 2, 10, 12) || s.w > home.w / 2) fail(`zoom to selection: ${s.d}`);
else ok("zoom to selection");

// 6. find a place
await page.evaluate(() => {
  const panel = [...document.querySelectorAll(".dash .panel")].find((p) => p.querySelector(".panel-title")?.textContent.trim() === "Main");
  panel.querySelector('.dp-tool[title="Find a place"]').click();
});
await page.waitForSelector(".dp-map-find input");
await page.fill(".dp-map-find input", "east");
await page.keyboard.press("Enter");
await settle();
s = await state("Main");
const said = await page.evaluate(() => [...document.querySelectorAll("[aria-live]")].map((e) => e.textContent).join(" | "));
if (!contains(s, 20, 40, 0, 10)) fail(`find "east" did not zoom to East: ${s.d}`);
else if (!/Zoomed to East/.test(said)) fail(`find did not announce: "${said}"`);
else ok('find a place zooms to "East" and announces it');
if (await page.$(".dp-map-find")) fail("the find box stayed open");

// 7. keys
await page.evaluate(() => [...document.querySelectorAll(".dash .panel")].find((p) => p.querySelector(".panel-title")?.textContent.trim() === "Main").querySelector(".panel-svg > svg").focus());
const w0 = (await state("Main")).w;
await page.keyboard.press("+");
await settle();
const w1 = (await state("Main")).w;
await page.keyboard.press("0");
await settle();
const w2 = (await state("Main")).w;
if (!(Math.abs(w1 / w0 - 0.8) < 0.02)) fail(`"+" zoomed by ${w1 / w0}`);
else if (Math.abs(w2 - home.w) > 1e-6) fail(`"0" did not reset (${w2})`);
else ok("keys: + zooms in ×0.8, 0 resets");

// 9. limits
p = await at("Main", 15, 5);
await page.mouse.move(p.x, p.y);
for (let i = 0; i < 40; i++) await page.mouse.wheel(0, -200);
await settle();
s = await state("Main");
if (s.w < home.w / 64 - 1e-6) fail(`zoomed in past the limit: ${s.w} < ${home.w / 64}`);
else ok(`zoom-in limit holds (width ${s.w.toFixed(3)}, limit ${(home.w / 64).toFixed(3)})`);
for (let i = 0; i < 60; i++) await page.mouse.wheel(0, 200);
await settle();
s = await state("Main");
if (s.w > home.w * 2 + 1e-6) fail(`zoomed out past the limit: ${s.w}`);
else ok("zoom-out limit holds");

if (errs.length) fail("page errors: " + errs.join(" | "));
await browser.close();
console.log(`map-tools-e2e: ${process.exitCode ? "FAILED" : "0 failed"}`);
