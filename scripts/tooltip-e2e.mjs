// #751: chart tooltips from SQL — ::TOOLTIP columns, ::TOOLTIP_FORMAT and
// ::TOOLTIP_HEADER templates — in a real browser. Hover a bar and a line
// point: the tooltip shows the filled, formatted template and the extra
// column; a value with markup stays text; a chart without these roles keeps
// its default tooltip.
//
//   NODE_PATH=tests/web/node_modules node scripts/tooltip-e2e.mjs --url http://127.0.0.1:8947/
import { createRequire } from "module";
const require = createRequire(import.meta.url);
const { chromium } = require("playwright");
const arg = (k, d) => {
  const i = process.argv.indexOf(k);
  return i > 0 ? process.argv[i + 1] : d;
};
const BASE = arg("--url", "http://127.0.0.1:8947/");
let bad = 0;
const check = (ok, m) => (ok ? console.log("ok   " + m) : (bad++, console.error("FAIL " + m)));
const b64 = (s) => Buffer.from(s, "utf8").toString("base64");

const SQL = `CREATE OR REPLACE TEMP TABLE s AS SELECT * FROM (VALUES
  ('EU', 1234.5, 12, DATE '2026-03-01', '<b>x</b>'),
  ('US', 2000.0, 20, DATE '2026-03-02', 'plain')) v(region, revenue, units, day, note);
SELECT region ::XAXIS, revenue ::BARCHART, '€'::YFORMAT,
       units AS "Units" ::TOOLTIP, note AS "Note" ::TOOLTIP,
       '{x}: {y} for {Units} units'::TOOLTIP_FORMAT,
       'Region {x}'::TOOLTIP_HEADER,
       'Revenue by region'::TITLE
FROM s ORDER BY region;
SELECT day ::XAXIS, revenue ::LINECHART, 'always'::POINTS,
       '{y:.0f} on {x:%d %b %Y}'::TOOLTIP_FORMAT,
       'Revenue per day'::TITLE
FROM (SELECT day, revenue FROM s UNION ALL SELECT DATE '2026-03-03', 1500.0) ORDER BY day;
SELECT region ::XAXIS, units ::BARCHART, 'Units by region'::TITLE FROM s ORDER BY region;
`;

const browser = await chromium.launch();
try {
  const page = await browser.newPage({ viewport: { width: 1300, height: 1000 } });
  const errs = [];
  page.on("pageerror", (e) => errs.push(String(e)));
  await page.goto(`${BASE}?mode=view#sql=${b64(SQL)}`);
  await page.waitForFunction(() => document.querySelectorAll(".dash .panel svg").length >= 3 && !document.body.classList.contains("loading"), null, { timeout: 120000 });
  const tipText = async (sel) => {
    await (typeof sel === "string" ? page.locator(sel).first() : sel).hover({ force: true });
    await page.waitForFunction(() => [...document.querySelectorAll("#dp-tip")].some((x) => x.classList.contains("show")), null, { timeout: 3000 }).catch(() => {});
    return page.evaluate(() => {
      const t = [...document.querySelectorAll(".dp-tip, #dp-tip, .tip")].find((x) => x.classList.contains("show"));
      return t ? { text: t.textContent, html: t.innerHTML } : null;
    });
  };
  const panels = page.locator(".dash .panel");
  const bar = panels.nth(0).locator('[data-row="0"]').first();
  check((await page.locator('.dash .panel [data-row]').count()) >= 4, "chart marks with a tooltip role carry data-row");
  const t1 = await tipText(bar);
  check(!!t1 && /Region EU/.test(t1.text), `the header template (${t1?.text})`);
  check(!!t1 && /EU: €1,234\.50 for 12 units/.test(t1.text), "the line template, formatted by ::YFORMAT, with a ::TOOLTIP column");
  check(!!t1 && /Note/.test(t1.text) && t1.text.includes("<b>x</b>") && !t1.html.includes("<b>x</b>"), "an unnamed ::TOOLTIP column is listed, its markup stays text");
  // the line's crosshair (3+ points): move over the plot at the second point
  const pt = await panels.nth(1).locator('circle[data-row="1"]').boundingBox();
  for (const dy of [40, 20, 0]) await page.mouse.move(pt.x + pt.width / 2, pt.y + pt.height / 2 + dy, { steps: 3 });
  await page.waitForFunction(() => document.getElementById("dp-tip")?.classList.contains("show"), null, { timeout: 3000 }).catch(() => {});
  const line = await page.evaluate(() => ({ text: document.getElementById("dp-tip")?.textContent || "" }));
  check(!!line && /2,000 on 02 Mar 2026/.test(line.text), `formats in the template: .0f and a strftime date (${line?.text})`);
  const plain = await tipText(panels.nth(2).locator("svg .dp-hit").first());
  check(!!plain && !(await panels.nth(2).locator("[data-row]").count()), "a chart without tooltip roles has no data-row");
  check(!!plain && /12|20/.test(plain.text), `…and its default tooltip (${plain?.text})`);
  check(errs.length === 0, `no page errors (${errs.join(" | ")})`);
} catch (e) {
  bad++;
  console.error("FAIL " + (e.stack || e));
} finally {
  await browser.close();
}
console.log(bad ? `tooltip-e2e: ${bad} failed` : "tooltip-e2e: ok");
process.exit(bad ? 1 : 0);
