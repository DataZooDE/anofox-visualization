// ::TOOLBAR in a real browser (#354 follow-up review): a table's toolbar is
// built by many small per-part decisions in renderTable (web/app.js), which
// no unit test exercises in the DOM — this does.
//
//   wasm-pack build --target web --out-dir web/pkg --profile wasm --no-default-features --features wasm
//   python3 serve_nocache.py 8797 &
//   NODE_PATH=<dir with playwright>/node_modules node scripts/toolbar-e2e.mjs [--url http://127.0.0.1:8797/]
//
// (a) 'none'::TOOLBAR + 'sum'::TOTALS: the totals row still shows (only its
//     toggle button is gone with the rest of the bar).
// (b) a table's saved search / column filter / hidden column survive an
//     edit that drops the SQL's preceding directive from 'none'::ROWSELECT
//     (no-op) to 'export'::TOOLBAR: every part excluded by the allow-list
//     must stop filtering/hiding, not just lose its button.
// (c) 'export'::TOOLBAR on an 11-row table: no Pages|Scroll segment (that
//     rides on the `density` part, not its own).
// (d) 'search'::TOOLBAR (export excluded) still gets the small hover ⤓ CSV
//     button — including on a table with a ::SPARKLINE column, whose
//     per-cell <svg> must not read as "this panel is a chart".
import { createRequire } from "module";

const require = createRequire(import.meta.url);
const { chromium } = require("playwright");
const arg = (k, d) => {
  const i = process.argv.indexOf(k);
  return i > 0 ? process.argv[i + 1] : d;
};
const base = arg("--url", "http://127.0.0.1:8797/");
const b64 = (s) => Buffer.from(s, "utf8").toString("base64");
let bad = 0;
const check = (ok, m) => (ok ? console.log("ok   " + m) : (bad++, console.error("FAIL " + m)));

const browser = await chromium.launch();

async function open(sql) {
  const page = await browser.newPage({ viewport: { width: 1200, height: 1400 }, reducedMotion: "reduce" });
  const errs = [];
  page.on("pageerror", (e) => errs.push(String(e)));
  await page.goto(`${base}?mode=edit#sql=${b64(sql)}`);
  await settled(page);
  return { page, errs };
}
const settled = (page) =>
  page.waitForFunction(
    () => /\d+ panels?/.test(document.getElementById("status")?.textContent || "") && !document.body.classList.contains("loading"),
    null,
    { timeout: 120000 }
  );
async function rerun(page, sql) {
  await page.fill("#sql", sql);
  await page.click("#run");
  await settled(page);
}
// The rows (VALUES 12 cities) shared by (b) and (c): enough rows (> 10) for
// `many` (search, Filters, Pages|Scroll), `region`'s 4 distinct values for
// the Filters row's dropdown, and 5 columns (> 4) for the Columns menu.
const CITIES = `(VALUES
  ('Aberdeen','North',100,10,'A'),('Bristol','South',200,20,'B'),('Cardiff','South',150,15,'C'),
  ('Derby','Midlands',120,12,'D'),('Exeter','South',90,9,'E'),('Fulham','South',110,11,'F'),
  ('Glasgow','North',300,30,'G'),('Hull','North',140,14,'H'),('Ipswich','East',95,9,'I'),
  ('Jarrow','North',60,6,'J'),('Kendal','North',55,5,'K'),('Leeds','North',250,25,'L')
) t(city, region, population, density, mayor)`;

// (a) ------------------------------------------------------------------
{
  const { page, errs } = await open(`
SELECT 'none'::TOOLBAR;
SELECT 'sum'::TOTALS;
SELECT name::TABLE, value::MONEY FROM (VALUES ('Alpha', 1000), ('Beta', 2000)) t(name, value);
`);
  const info = await page.evaluate(() => {
    const fig = document.querySelector(".dash .panel");
    const table = fig?.querySelector(".dp-table");
    return {
      bar: !!fig?.querySelector(".tbl-bar"),
      totalsRow: !!table?.querySelector("tfoot .tot-l"),
    };
  });
  check(!info.bar, "(a) 'none'::TOOLBAR drops the bar");
  check(info.totalsRow, "(a) the totals row still shows under 'none'::TOOLBAR + 'sum'::TOTALS");
  check(!errs.length, "(a) no page errors: " + errs.join(" | "));
  await page.close();
}

// (b) ------------------------------------------------------------------
{
  const { page, errs } = await open(`
SELECT 'none'::ROWSELECT;
SELECT city::TABLE, region, population, density, mayor FROM ${CITIES};
`);
  // search (q): matches only "Leeds" (North) — combined with the column
  // filter below (region = South) this leaves nothing, a clear "filtered"
  // signal to contrast with "every row shows" after the fix.
  await page.fill(".tbl-search", "Leeds");
  await page.waitForTimeout(250); // the search is debounced
  // Filters row -> region's dropdown (2nd column: city, region, population,
  // density — city also gets a dropdown, being all-distinct and <= 30
  // values, so pick by position, not just ".dp-colf") -> South (colEq).
  await page.click(".tbl-btn:text-is('Filters')");
  await page.selectOption(".tbl-frow th:nth-child(2) select.dp-colf", "South");
  // Columns -> hide "density" (hidden).
  await page.click(".tbl-cols:not(.tbl-export) summary");
  await page.click(".tbl-cols-pop label:has-text('Density') input");
  // "N of 12 rows" (filtered) / "12 rows" (not) — tbody keeps one <tr> for
  // the "No rows match the filters." placeholder, so counting <tr> alone
  // can't tell a real 0-row match from a dozen real rows.
  const before = await page.evaluate(() => ({
    count: document.querySelector(".dash .tbl-count")?.textContent || "",
    cols: document.querySelectorAll(".dash .dp-table thead tr:first-child th").length,
  }));
  check(before.count === "0 of 12 rows", "(b) setup: the contradictory search + filter leave 0 rows (" + before.count + ")");
  check(before.cols === 4, "(b) setup: Density is hidden (" + before.cols + " header cells)");

  // Same statement count/position (swap, not insert) so the table keeps its
  // saved UI-state key: only 'export' is now kept.
  await rerun(
    page,
    `
SELECT 'export'::TOOLBAR;
SELECT city::TABLE, region, population, density, mayor FROM ${CITIES};
`
  );
  const after = await page.evaluate(() => ({
    count: document.querySelector(".dash .tbl-count")?.textContent || "",
    cols: document.querySelectorAll(".dash .dp-table thead tr:first-child th").length,
    chips: document.querySelectorAll(".dash .tbl-chip").length,
    search: !!document.querySelector(".dash .tbl-search"),
  }));
  check(after.count === "12 rows", "(b) excluding search/filters/columns shows every row again (" + after.count + ")");
  check(after.cols === 5, "(b) … and every column again (" + after.cols + "/5)");
  check(after.chips === 0, "(b) no leftover filter chips");
  check(!after.search, "(b) no search box ('search' not in the allow-list)");
  check(!errs.length, "(b) no page errors: " + errs.join(" | "));
  await page.close();
}

// (c) ------------------------------------------------------------------
{
  const { page, errs } = await open(`
SELECT 'export'::TOOLBAR;
SELECT city::TABLE, region, population, density, mayor FROM ${CITIES};
`);
  const hasSeg = await page.evaluate(() => !!document.querySelector(".dash .tbl-seg"));
  check(!hasSeg, "(c) no Pages|Scroll segment on an 11+-row table with 'export' only");
  check(!errs.length, "(c) no page errors: " + errs.join(" | "));
  await page.close();
}

// (d) ------------------------------------------------------------------
{
  const { page, errs } = await open(`
SELECT 'search'::TOOLBAR;
SELECT name::TABLE, value FROM (VALUES ('Alpha', 1), ('Beta', 2)) t(name, value);
`);
  const info = await page.evaluate(() => {
    const fig = document.querySelector(".dash .panel");
    return { dl: !!fig?.querySelector(".dl"), exportMenu: !!fig?.querySelector(".tbl-export") };
  });
  check(info.dl, "(d) the hover ⤓ CSV button shows when 'export' is excluded");
  check(!info.exportMenu, "(d) …and the toolbar's own export menu does not");
  check(!errs.length, "(d) no page errors: " + errs.join(" | "));
  await page.close();
}

// (d, sparkline) ---------------------------------------------------------
{
  const { page, errs } = await open(`
CREATE TABLE spk_hist AS SELECT (i % 3) AS grp, i AS d, (i * 7) % 11 AS v FROM range(12) t(i);
SELECT 'search'::TOOLBAR;
SELECT DISTINCT fc.grp::TABLE,
       (SELECT list(v ORDER BY d) FROM spk_hist WHERE spk_hist.grp = fc.grp) AS trend ::SPARKLINE
FROM spk_hist fc;
`);
  const info = await page.evaluate(() => {
    const fig = document.querySelector(".dash .panel");
    return {
      dl: !!fig?.querySelector(".dl"),
      cellSvg: !!fig?.querySelector(".dp-table svg"),
    };
  });
  check(info.cellSvg, "(d/sparkline) setup: the table does have a per-cell <svg>");
  check(info.dl, "(d/sparkline) the hover ⤓ CSV button still shows (a sparkline's svg isn't mistaken for a chart)");
  check(!errs.length, "(d/sparkline) no page errors: " + errs.join(" | "));
  await page.close();
}

await browser.close();
console.log(`toolbar-e2e: ${bad} failed`);
if (bad) process.exitCode = 1;
