// Browser e2e for #454: a table's ::SPARKLINE cell draws through the wasm
// renderer's render_cell export (owner rule #318 — one renderer, ggplot-rs
// — the same migration #425/#453 did for ::SPARKBAR cells), not the
// hand-written SVG `cellSpark` used to build. Checks, in a real browser:
//   - the cell's SVG is ggplot-rs output: gg-mark marks (data-geom="line"/
//     "ribbon"/"point"), the expected point count.
//   - the cell's own whole-series summary tip (first → last / min / max /
//     count) survived the migration, as the returned svg's own <title> —
//     render_sparkline only puts a <title> on its endpoint marker, so
//     cellSpark (web/app.js) injects this one itself.
//   - the cell's drawn content fits inside its own bounding box —
//     render_sparkline's 50px panel floor (#457) paired with
//     preserveAspectRatio="none" on the returned SVG, so a taller internal
//     viewBox stretch-fills the cell's fixed CSS size instead of
//     letterboxing a narrower, centred chart or clipping a dip off the
//     bottom.
//
//   cargo build --lib --target wasm32-unknown-unknown --profile wasm --no-default-features --features wasm
//   wasm-bindgen --target web --out-dir web/pkg --out-name anofox_visualization <target>/wasm32-unknown-unknown/wasm/anofox_visualization.wasm
//   python3 serve_nocache.py 8947 &
//   NODE_PATH=<dir with playwright>/node_modules node scripts/sparkline-e2e.mjs [--url http://127.0.0.1:8947/] [--shots <dir>]
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
const b64 = (s) => Buffer.from(s, "utf8").toString("base64");
let bad = 0;
const check = (ok, m) => (ok ? console.log("ok   " + m) : (bad++, console.error("FAIL " + m)));

// A zigzag with real dips (the exact class of series #457 found clipped
// under the 50px floor before render_sparkline's own clamp), and a flat
// (constant) series — render_sparkline's own "no range to show" branch.
const SQL = `SELECT f AS feature ::TABLE,
       trend AS "Trend" ::SPARKLINE
FROM (VALUES
    ('zigzag', [1005,1010,1000,1012,1001,1013,1000]),
    ('flat', [10,10,10,10])
) t(f, trend);
`;

const browser = await chromium.launch();
const page = await browser.newPage({ viewport: { width: 900, height: 500 }, reducedMotion: "reduce" });
const errs = [];
page.on("pageerror", (e) => errs.push(String(e)));

const settled = () =>
  page.waitForFunction(
    () => !document.body.classList.contains("loading") && document.querySelectorAll(".dash table tbody tr").length > 0,
    null,
    { timeout: 60000 }
  );

await page.goto(`${base}?embed=1#sql=${b64(SQL)}`);
await settled();
await page.waitForTimeout(300);

const cells = await page.evaluate(() => {
  const spark = [...document.querySelectorAll("td.spark-cell svg.spark")];
  return spark.map((svg) => {
    const cellBox = svg.closest("td").getBoundingClientRect();
    const svgBox = svg.getBoundingClientRect();
    const points = [...svg.querySelectorAll("circle, polyline, polygon")];
    return {
      title: svg.querySelector(":scope > title")?.textContent || "",
      hasLine: !!svg.querySelector('[data-geom="line"]'),
      hasRibbon: !!svg.querySelector('[data-geom="ribbon"]'),
      hasPoint: !!svg.querySelector('[data-geom="point"]'),
      everyMarkIsGgMark: points.every((m) => m.classList.contains("gg-mark")),
      linePointCount: (svg.querySelector('polyline[data-geom="line"]')?.getAttribute("points") || "").trim().split(/\s+/).filter(Boolean).length,
      preserveAspectRatio: svg.getAttribute("preserveAspectRatio"),
      cellBox: { top: cellBox.top, bottom: cellBox.bottom, left: cellBox.left, right: cellBox.right },
      svgBox: { top: svgBox.top, bottom: svgBox.bottom, left: svgBox.left, right: svgBox.right },
    };
  });
});

check(cells.length === 2, `two ::SPARKLINE cells rendered (one per row), found ${cells.length}`);

const zigzag = cells[0];
const flat = cells[1];

if (zigzag) {
  check(zigzag.hasLine && zigzag.hasRibbon && zigzag.hasPoint, "zigzag cell draws line + ribbon (area) + endpoint point marks");
  check(zigzag.everyMarkIsGgMark, "zigzag cell's marks are ggplot-rs marks (gg-mark), not hand-drawn SVG");
  check(zigzag.linePointCount === 7, `zigzag cell's line has 7 points (one per series value), found ${zigzag.linePointCount}`);
  check(
    zigzag.title === "1K → 1K (−0%) · min 1K · max 1K · 7 points",
    `zigzag cell's whole-series summary tip survived the migration, found ${JSON.stringify(zigzag.title)}`
  );
  check(zigzag.preserveAspectRatio === "none", `zigzag cell's svg stretch-fills its box (preserveAspectRatio="none"), found ${JSON.stringify(zigzag.preserveAspectRatio)}`);
} else {
  check(false, "no zigzag ::SPARKLINE cell found");
}

if (flat) {
  check(flat.hasLine && flat.hasPoint, "flat (constant) cell still draws a line + endpoint point, not dropped");
  check(flat.everyMarkIsGgMark, "flat cell's marks are ggplot-rs marks (gg-mark), not hand-drawn SVG");
} else {
  check(false, "no flat ::SPARKLINE cell found");
}

// ---- the cell's own bounding box fits the table cell (#457's bug class: a
// taller internal viewBox than the cell, un-stretched, either clips or
// floats centred with blank margins) ----
const EPS = 1.5; // sub-pixel rounding
for (const [label, c] of [["zigzag", zigzag], ["flat", flat]]) {
  if (!c) continue;
  const fits =
    c.svgBox.top >= c.cellBox.top - EPS &&
    c.svgBox.bottom <= c.cellBox.bottom + EPS &&
    c.svgBox.left >= c.cellBox.left - EPS &&
    c.svgBox.right <= c.cellBox.right + EPS;
  check(fits, `${label} cell's svg bounding box fits inside its own cell (svg ${JSON.stringify(c.svgBox)}, cell ${JSON.stringify(c.cellBox)})`);
}

if (shots) {
  fs.mkdirSync(shots, { recursive: true });
  const table = await page.locator(".dash table").first();
  await table.screenshot({ path: `${shots}/sparkline-e2e-table.png` });
}

check(errs.length === 0, `no page errors${errs.length ? ": " + errs.join("; ") : ""}`);

await browser.close();
console.log(`sparkline-e2e: ${bad} failed`);
if (bad) process.exit(1);
