// Browser e2e for #425 (::SPARKBAR in-cell bar histogram) + its own #457
// follow-up (ggplot-rs's 50px panel floor): a table's ::SPARKBAR cell draws
// through the wasm renderer's render_cell export (owner rule #318 — one
// renderer, ggplot-rs — review on #453), not hand-written SVG. Checks, in a
// real browser:
//   - the cell's SVG is ggplot-rs output: gg-mark rects (data-geom="col"),
//     the expected bar count for a single series and a ref/cur pair.
//   - the cell's drawn content fits inside its own bounding box, and does
//     so via a UNIFORM downscale, not a stretch: the render_sparkbar panel
//     floor clamp (50px, same as render_sparkline's #457 fix) means the
//     returned svg's own viewBox is taller than this cell's own fixed CSS
//     box, so cellSparkbar requests a WIDER panel too, matched to the
//     cell's own aspect ratio (`sparkRequestSize`, web/app.js, #476
//     review) — no `preserveAspectRatio="none"` override: that would scale
//     the two axes unevenly (fine for a rect, but the same request size
//     this function shares with cellSpark's circular endpoint marker needs
//     a uniform scale, so neither cell overrides it).
//
//   cargo build --lib --target wasm32-unknown-unknown --profile wasm --no-default-features --features wasm
//   wasm-bindgen --target web --out-dir web/pkg --out-name anofox_visualization <target>/wasm32-unknown-unknown/wasm/anofox_visualization.wasm
//   python3 serve_nocache.py 8947 &
//   NODE_PATH=<dir with playwright>/node_modules node scripts/sparkbar-e2e.mjs [--url http://127.0.0.1:8947/] [--shots <dir>]
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

// One single-series bucket list, one ref/cur comparison list — the two
// ::SPARKBAR cell shapes `cellSparkbar` (web/app.js) and `render_sparkbar`
// (src/lib.rs) both handle.
// Bare decimal literals (0.1, …, DuckDB's own DECIMAL inference) — #468
// fixed rowsJson's unscale step to walk a DECIMAL nested inside a LIST/
// STRUCT/MAP, so this no longer needs a ::DOUBLE cast to route around it.
const SQL = `SELECT f AS feature ::TABLE,
       list(share ORDER BY bucket) AS "Single" ::SPARKBAR,
       list({'ref': r, 'cur': c} ORDER BY bucket) AS "RefCur" ::SPARKBAR
FROM (VALUES
    ('temp', 1, 0.1, 0.1, 0.3),
    ('temp', 2, 0.2, 0.3, 0.3),
    ('temp', 3, 0.3, 0.4, 0.2),
    ('temp', 4, 0.4, 0.2, 0.2)
) t(f, bucket, share, r, c)
GROUP BY f;
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

// ---- the cell draws through render_cell (ggplot-rs), not hand-written SVG ----
const cells = await page.evaluate(() => {
  const spark = [...document.querySelectorAll("td.spark-cell svg.spark-bar")];
  return spark.map((svg) => {
    const rects = [...svg.querySelectorAll("rect")];
    const cellBox = svg.closest("td").getBoundingClientRect();
    const svgBox = svg.getBoundingClientRect();
    return {
      rectCount: rects.length,
      everyRectIsGgMark: rects.every((r) => r.classList.contains("gg-mark") && r.getAttribute("data-geom") === "col"),
      preserveAspectRatio: svg.getAttribute("preserveAspectRatio"),
      cellBox: { top: cellBox.top, bottom: cellBox.bottom, left: cellBox.left, right: cellBox.right },
      svgBox: { top: svgBox.top, bottom: svgBox.bottom, left: svgBox.left, right: svgBox.right },
    };
  });
});

check(cells.length === 2, `two ::SPARKBAR cells rendered (one row), found ${cells.length}`);

const single = cells[0];
const refCur = cells[1];

if (single) {
  check(single.rectCount === 4, `single-series cell draws 4 bars (one per bucket), found ${single.rectCount}`);
  check(single.everyRectIsGgMark, "single-series cell's bars are ggplot-rs marks (gg-mark gg-bar, data-geom=\"col\"), not hand-drawn SVG");
  check(
    single.preserveAspectRatio !== "none",
    `single-series cell's svg scales uniformly (no preserveAspectRatio="none"), found ${JSON.stringify(single.preserveAspectRatio)}`
  );
} else {
  check(false, "no single-series ::SPARKBAR cell found");
}

if (refCur) {
  check(refCur.rectCount === 8, `ref/cur cell draws 8 bars (4 ref + 4 cur), found ${refCur.rectCount}`);
  check(refCur.everyRectIsGgMark, "ref/cur cell's bars are ggplot-rs marks (gg-mark gg-bar, data-geom=\"col\"), not hand-drawn SVG");
  check(
    refCur.preserveAspectRatio !== "none",
    `ref/cur cell's svg scales uniformly (no preserveAspectRatio="none"), found ${JSON.stringify(refCur.preserveAspectRatio)}`
  );
} else {
  check(false, "no ref/cur ::SPARKBAR cell found");
}

// ---- the cell's own bounding box fits the table cell (#457's bug: a
// taller internal viewBox than the cell, un-stretched, either clips or
// floats centred with blank margins) ----
const EPS = 1.5; // sub-pixel rounding
for (const [label, c] of [["single-series", single], ["ref/cur", refCur]]) {
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
  await page.screenshot({ path: `${shots}/sparkbar-e2e.png` });
  const table = await page.locator(".dash table").first();
  await table.screenshot({ path: `${shots}/sparkbar-e2e-table.png` });
}

check(errs.length === 0, `no page errors${errs.length ? ": " + errs.join("; ") : ""}`);

await browser.close();
console.log(`sparkbar-e2e: ${bad} failed`);
if (bad) process.exit(1);
