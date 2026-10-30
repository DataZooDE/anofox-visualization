// #751: chart tooltips from SQL — ::TOOLTIP columns, ::TOOLTIP_FORMAT and
// ::TOOLTIP_HEADER templates — in a real browser. Hover a bar and a line
// point: the tooltip shows the filled, formatted template and the extra
// column; a value with markup stays text; a chart without these roles keeps
// its default tooltip. #752: a log-log scatter with ::LABEL lists only the
// hovered point, headed by its label, x in data units; ::ALPHA fades the
// points; a point's ::LINK fills {column} / {label} from its own row.
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
SELECT d ::XAXIS, v ::LINECHART, 'none'::TOOLTIP_MODE, 'No tooltip'::TITLE FROM (VALUES (1, 1.0), (2, 3.0), (3, 2.0)) t(d, v);
SELECT d ::XAXIS, v ::LINECHART, 'single'::TOOLTIP_MODE, 'Single'::TITLE FROM (VALUES (1, 1.0), (2, 3.0), (3, 2.0)) t(d, v);
SELECT d ::XAXIS, v ::LINECHART, g ::CATEGORY, 'split'::TOOLTIP_MODE, '{series} {y:.1f}'::TOOLTIP_FORMAT, 'Split'::TITLE
FROM (VALUES (1, 1.0, 'a'), (2, 3.0, 'a'), (3, 2.0, 'a'), (1, 2.0, 'b'), (2, 4.0, 'b'), (3, 1.0, 'b')) t(d, v, g);
SELECT stars ::XAXIS, downloads ::SCATTER, 'log10'::XSCALE, 'log10'::YSCALE, kind ::CATEGORY,
       extension ::LABEL, 2::LABEL_TOP, '1,0'::ABLINE, 0.6::ALPHA,
       'https://example.com/x/{extension}?l={label}'::LINK, 'Stars'::TITLE
FROM (VALUES ('airport', 350, 1464, 'Near'), ('a2', 350, 877, 'Near'), ('bq', 1200, 90000, 'Above'), ('x3', 20, 50, 'Below')) t(extension, stars, downloads, kind);
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
  // ::TOOLTIP_MODE: 'none' shows no tooltip and has no crosshair; 'single' has no crosshair
  await page.mouse.move(5, 5);
  await page.evaluate(() => document.getElementById("dp-tip")?.classList.remove("show"));
  check(!(await panels.nth(3).evaluate((p) => p.classList.contains("has-axis-pointer"))), "'none': no crosshair");
  const box = await panels.nth(3).locator("svg").first().boundingBox();
  for (const fx of [0.3, 0.5, 0.7]) await page.mouse.move(box.x + box.width * fx, box.y + box.height * 0.5, { steps: 2 });
  await panels.nth(3).locator("circle.dp-hit").nth(1).hover({ force: true });
  await page.waitForTimeout(400);
  check(!(await page.evaluate(() => document.getElementById("dp-tip")?.classList.contains("show"))), "'none': no tooltip on hover");
  check(!(await panels.nth(4).evaluate((p) => p.classList.contains("has-axis-pointer"))), "'single': no crosshair (only the hovered mark)");
  check(await panels.nth(1).evaluate((p) => p.classList.contains("has-axis-pointer")), "a line chart without a mode keeps its crosshair");
  // 'split': one label per series beside its point
  const sp = await panels.nth(5).locator("circle.dp-hit").nth(1).boundingBox();
  for (const dy of [40, 20, 0]) await page.mouse.move(sp.x + sp.width / 2, sp.y + sp.height / 2 + dy, { steps: 2 });
  await page.waitForTimeout(300);
  const labels = await page.evaluate(() => [...document.querySelectorAll("#dp-tip-split .dp-tip-split")].map((d) => d.textContent));
  check(labels.length === 2 && labels.some((t) => /^a 3\.0$/.test(t.trim())) && labels.some((t) => /^b 4\.0$/.test(t.trim())), `'split': a label per series (${JSON.stringify(labels)})`);
  await page.mouse.move(2, 2);
  await page.waitForTimeout(200);
  check(!(await page.evaluate(() => document.querySelectorAll("#dp-tip-split .dp-tip-split").length)), "'split': the labels go with the pointer");
  // #752: the scatter
  await page.mouse.move(2, 2);
  const sc = panels.nth(6);
  check(!(await sc.evaluate((p) => p.classList.contains("has-axis-pointer"))), "a scatter (with an ::ABLINE) has no crosshair");
  const ap = sc.locator('circle.dp-hit[data-label="airport"]');
  check((await ap.getAttribute("fill-opacity")) === "0.600", "::ALPHA on the points");
  const st = await tipText(ap);
  check(!!st && /^airport/.test(st.text) && /stars\s*350/.test(st.text) && /downloads\s*1,464/.test(st.text) && /Near/.test(st.text), `headed by its ::LABEL, x in data units (${st?.text})`);
  check(!!st && !/877/.test(st.text) && !/a2/.test(st.text), "only the hovered point, not its x column");
  let url = "";
  await page.context().route("https://example.com/**", (r) => ((url = r.request().url()), r.abort()));
  const [pop] = await Promise.all([page.waitForEvent("popup", { timeout: 5000 }).catch(() => null), ap.click({ force: true })]);
  await page.waitForTimeout(300);
  check(/\/x\/airport\?l=airport/.test(url), `a point's ::LINK fills {column} and {label} from its row (${url})`);
  if (pop) await pop.close();
  check(errs.length === 0, `no page errors (${errs.join(" | ")})`);
} catch (e) {
  bad++;
  console.error("FAIL " + (e.stack || e));
} finally {
  await browser.close();
}
console.log(bad ? `tooltip-e2e: ${bad} failed` : "tooltip-e2e: ok");
process.exit(bad ? 1 : 0);
