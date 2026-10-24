// Browser e2e for #463: the three remaining hand-drawn browser mini charts
// — Explore data's card histogram (`xpMini`, both its pre-binned-histogram
// and rows-over-time-line branches), its A/B distribution overlay
// (`xpOverlay`), and the table's own column-profile histogram (`st-hist`)
// — all now draw through the wasm renderer's render_mini_hist/
// render_mini_overlay/render_cell exports (owner rule #318 — one renderer,
// ggplot-rs — the #425/#453/#454 migration), not hand-written SVG. Checks,
// in a real browser:
//   - every chart's marks are ggplot-rs output (gg-mark, data-geom), not
//     hand-drawn SVG.
//   - xpMini's histogram branch: the right bar count, opaque fill (1.0,
//     its own previous hand-drawn look).
//   - xpMini's line branch (rows over time, >= 3 periods): a line +
//     endpoint point mark (render_sparkline's own, richer than the old
//     bare polyline — #454's precedent), whose own bounding box is square
//     (not flattened into an ellipse by the fluid-width card).
//   - xpOverlay: A (outline — zero fill-opacity, a visible stroke) over B
//     (filled, translucent), on one shared y scale.
//   - st-hist: per-bar hover tips (`"{range}: {count}"`), the CSS
//     `:hover{fill-opacity:1}` highlight still wins over the baked
//     presentation attribute.
//
//   cargo build --lib --target wasm32-unknown-unknown --profile wasm --no-default-features --features wasm
//   wasm-bindgen --target web --out-dir web/pkg --out-name anofox_visualization <target>/wasm32-unknown-unknown/wasm/anofox_visualization.wasm
//   python3 serve_nocache.py 8947 &
//   NODE_PATH=<dir with playwright>/node_modules node scripts/mini-charts-e2e.mjs [--url http://127.0.0.1:8947/] [--shots <dir>]
import { createRequire } from "module";
import fs from "fs";

const require = createRequire(import.meta.url);
const { chromium } = require("playwright");
const arg = (k, d) => {
  const i = process.argv.indexOf(k);
  return i > 0 ? process.argv[i + 1] : d;
};
const base = arg("--url", "http://127.0.0.1:8947/");
const shots = arg("--shots", "");
if (shots) fs.mkdirSync(shots, { recursive: true });
const b64 = (s) => Buffer.from(s, "utf8").toString("base64");
let bad = 0;
const check = (ok, m) => (ok ? console.log("ok   " + m) : (bad++, console.error("FAIL " + m)));

const browser = await chromium.launch();
const errs = [];

// ---- xpMini + xpOverlay: Explore data ----
{
  const page = await browser.newPage({ viewport: { width: 1400, height: 1000 }, reducedMotion: "reduce" });
  page.on("pageerror", (e) => errs.push(String(e)));
  const dash = "How are sales doing this period against the last?";
  await page.goto(`${base}?dashboard=${encodeURIComponent(dash)}`);
  await page.waitForFunction((d) => document.getElementById("dash-name")?.value === d, dash, { timeout: 60000 });
  await page.waitForFunction(() => document.querySelector(".dash .panel") && !document.body.classList.contains("loading"), null, { timeout: 60000 });
  await page.click("#side-explore");
  await page.waitForFunction(() => document.body.classList.contains("mode-explore"), null, { timeout: 20000 });
  const sql = async (q) => {
    await page.fill("#xq-sql", q);
    await page.click("#xq-run");
    await page.waitForFunction(() => /rows? · /.test(document.getElementById("xq-info").textContent) || !document.getElementById("xq-err").hidden, null, { timeout: 60000 });
  };
  const settled = () => page.waitForFunction(() => document.querySelector(".xp-progress.done") && !document.querySelector("#xq-detail[aria-busy]"), null, { timeout: 60000 });

  // A numeric column (xpMini's histogram branch) and a date column with
  // >= 3 distinct periods (xpMini's line branch).
  await sql(`CREATE OR REPLACE TABLE mini463 AS
SELECT i AS id,
       (20 + 15 * sin(i / 5.0) + (i % 7))::DOUBLE AS amount,
       (DATE '2026-01-01' + INTERVAL (i % 40) DAY)::DATE AS day
FROM range(1, 301) t(i)`);
  await page.waitForSelector('.cat-table[data-fq$=\'"mini463"\']', { timeout: 60000 });
  await page.click('.cat-table[data-fq$=\'"mini463"\']');
  await settled();
  await page.waitForTimeout(300);

  const cards = await page.evaluate(() => {
    const card = (col) => document.querySelector(`.xp-card[data-col="${col}"] svg.xp-mini`);
    const info = (svg) => {
      if (!svg) return null;
      const rects = [...svg.querySelectorAll("rect")];
      const circ = svg.querySelector("circle");
      const circBox = circ ? circ.getBoundingClientRect() : null;
      return {
        ariaLabel: svg.getAttribute("aria-label") || "",
        rectCount: rects.length,
        everyRectIsGgMark: rects.every((r) => r.classList.contains("gg-mark")),
        hasLine: !!svg.querySelector('[data-geom="line"]'),
        circBox: circBox && { w: circBox.width, h: circBox.height },
        firstRectFillOpacity: rects[0]?.getAttribute("fill-opacity"),
      };
    };
    return { amount: info(card("amount")), day: info(card("day")) };
  });

  const hist = cards.amount;
  check(!!hist, "xpMini's histogram branch (amount) rendered a chart");
  if (hist) {
    check(hist.rectCount === 10, `xpMini histogram has 10 bars, found ${hist.rectCount}`);
    check(hist.everyRectIsGgMark, "xpMini histogram's bars are ggplot-rs marks (gg-mark), not hand-drawn SVG");
    check(hist.firstRectFillOpacity == null, `xpMini histogram bars are opaque (no fill-opacity attr, alpha 1.0), found ${JSON.stringify(hist.firstRectFillOpacity)}`);
    check(/^Histogram of amount/.test(hist.ariaLabel), `xpMini histogram's own aria-label summary survived, found ${JSON.stringify(hist.ariaLabel)}`);
  }

  const line = cards.day;
  check(!!line, "xpMini's line branch (day, rows over time) rendered a chart");
  if (line) {
    check(line.hasLine, "xpMini's line branch draws through ggplot-rs's line geom");
    check(/^Rows per day/.test(line.ariaLabel), `xpMini line's own aria-label summary survived, found ${JSON.stringify(line.ariaLabel)}`);
    check(
      line.circBox && Math.abs(line.circBox.w - line.circBox.h) < 1,
      `xpMini line's endpoint marker is round, not flattened into an ellipse by the fluid-width card (its own bbox is ${JSON.stringify(line.circBox)})`
    );
  }

  if (shots) await page.locator(".xp-cards").screenshot({ path: `${shots}/mini-charts-xpcards.png` });

  // xpOverlay via Compare: A (outline) over B (filled) on one shared scale.
  await sql("CREATE OR REPLACE TABLE ov463_v1 AS SELECT i AS id, (pow(i % 100 / 100.0, 2.2) * 100)::DOUBLE AS score FROM range(1, 1001) t(i)");
  await sql("CREATE OR REPLACE TABLE ov463_v2 AS SELECT i AS id, (100 - pow(i % 100 / 100.0, 2.2) * 100)::DOUBLE AS score FROM range(1, 1001) t(i)");
  await page.waitForSelector('.cat-table[data-fq$=\'"ov463_v2"\']', { timeout: 60000 });
  await page.click('.cat-table[data-fq$=\'"ov463_v1"\']');
  await settled();
  await page.click("button:has-text('Compare…')");
  await page.selectOption(".xp-cmp-form select", { label: "ov463_v2" });
  await page.click(".xp-cmp-form button[type=submit]");
  await page.waitForSelector("table.xp-cmp", { timeout: 60000 });
  await page.waitForTimeout(300);

  const ov = await page.evaluate(() => {
    const svg = document.querySelector('table.xp-cmp tbody tr:nth-child(2) svg.xp-ov'); // "score" row
    if (!svg) return null;
    const rects = [...svg.querySelectorAll("rect")];
    return {
      ariaLabel: svg.getAttribute("aria-label") || "",
      rectCount: rects.length,
      everyRectIsGgMark: rects.every((r) => r.classList.contains("gg-mark")),
      hasOutline: rects.some((r) => r.getAttribute("fill-opacity") === "0.000"),
      hasFilled: rects.some((r) => r.getAttribute("fill-opacity") === "0.550"),
    };
  });
  check(!!ov, "xpOverlay (score row) rendered a chart");
  if (ov) {
    check(ov.rectCount === 20, `xpOverlay has 20 bars (10 A + 10 B), found ${ov.rectCount}`);
    check(ov.everyRectIsGgMark, "xpOverlay's bars are ggplot-rs marks (gg-mark), not hand-drawn SVG");
    check(ov.hasOutline, "xpOverlay draws A as an outline (zero fill-opacity)");
    check(ov.hasFilled, "xpOverlay draws B filled (0.55 fill-opacity)");
    check(/^Distribution of score/.test(ov.ariaLabel), `xpOverlay's own aria-label summary survived, found ${JSON.stringify(ov.ariaLabel)}`);
  }
  if (shots) await page.locator("table.xp-cmp tbody tr:nth-child(2)").screenshot({ path: `${shots}/mini-charts-overlay.png` });

  await page.close();
}

// ---- st-hist: the table's own column-profile row ----
{
  const page = await browser.newPage({ viewport: { width: 1200, height: 700 }, reducedMotion: "reduce" });
  page.on("pageerror", (e) => errs.push(String(e)));
  const SQL = `SELECT f AS feature ::TABLE, x AS amount
FROM (SELECT i AS f, (20 + 15 * sin(i / 5.0) + (i % 7))::DOUBLE AS x FROM range(1, 301) t(i));
`;
  await page.goto(`${base}?embed=1#sql=${b64(SQL)}`);
  await page.waitForFunction(() => !document.body.classList.contains("loading") && document.querySelectorAll(".dash table tbody tr").length > 0, null, { timeout: 60000 });
  await page.waitForTimeout(300);
  await page.click("button:has-text('Stats')");
  await page.waitForSelector("tr.tbl-stats svg.st-hist", { timeout: 20000 });
  await page.waitForTimeout(300);

  const sh = await page.evaluate(() => {
    const svgs = [...document.querySelectorAll("tr.tbl-stats svg.st-hist")];
    return svgs.map((svg) => {
      const rects = [...svg.querySelectorAll("rect")];
      return {
        ariaLabel: svg.getAttribute("aria-label") || "",
        rectCount: rects.length,
        everyRectIsGgMark: rects.every((r) => r.classList.contains("gg-mark")),
        firstTitle: rects[0]?.querySelector("title")?.textContent || "",
      };
    });
  });
  check(sh.length === 2, `two st-hist cells (feature + amount), found ${sh.length}`);
  for (const [i, s] of sh.entries()) {
    check(s.rectCount === 10, `st-hist[${i}] has 10 bars, found ${s.rectCount}`);
    check(s.everyRectIsGgMark, `st-hist[${i}]'s bars are ggplot-rs marks (gg-mark), not hand-drawn SVG`);
    check(/^distribution of/.test(s.ariaLabel), `st-hist[${i}]'s own aria-label summary survived, found ${JSON.stringify(s.ariaLabel)}`);
    check(/: \d/.test(s.firstTitle), `st-hist[${i}]'s first bar carries a "{range}: {count}" tip, found ${JSON.stringify(s.firstTitle)}`);
  }

  // The CSS :hover highlight (fill-opacity:1) must still win over ggplot-rs's
  // own baked fill-opacity:0.55 (a CSS selector rule beats a presentation
  // attribute regardless of what alpha the renderer sets).
  const rect = page.locator("tr.tbl-stats svg.st-hist rect").nth(3);
  const before = await rect.evaluate((el) => getComputedStyle(el).fillOpacity);
  await rect.hover();
  await page.waitForTimeout(100);
  const after = await rect.evaluate((el) => getComputedStyle(el).fillOpacity);
  check(before === "0.55" && after === "1", `st-hist's hover highlight still works (fill-opacity ${before} -> ${after})`);

  if (shots) await page.locator(".dash table").first().screenshot({ path: `${shots}/mini-charts-sthist.png` });

  await page.close();
}

for (const e of errs) check(false, `page error: ${e}`);

await browser.close();
console.log(`mini-charts-e2e: ${bad} failed`);
if (bad) process.exit(1);
