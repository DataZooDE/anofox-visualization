// A table a little too wide for its card tightens its cells' padding and fits
// (#1026: a status-pill column at the right edge read as clipped under the
// "more columns" fade); one far too wide still scrolls sideways with the fade
// (#996).
//
//   python3 serve_nocache.py 8798 &
//   NODE_PATH=<dir with playwright>/node_modules node scripts/table-fit-e2e.mjs [--url http://127.0.0.1:8798/]
import { createRequire } from "module";

const require = createRequire(import.meta.url);
const { chromium } = require("playwright");
const arg = (k, d) => {
  const i = process.argv.indexOf(k);
  return i > 0 ? process.argv[i + 1] : d;
};
const base = arg("--url", "http://127.0.0.1:8798/");
const b64 = (s) => Buffer.from(s, "utf8").toString("base64");
let bad = 0;
const check = (ok, m) => (ok ? console.log("ok   " + m) : (bad++, console.error("FAIL " + m)));

// the website gallery's table, where #1026 was seen (1100 px window, a lone panel: half the row)
const STORES = `CREATE TABLE stores AS SELECT * FROM (VALUES
  ('Berlin', 1240000, 12.4, 4.1, 'on plan'), ('Hamburg', 980000, 9.8, -2.3, 'at risk'),
  ('Munich', 1410000, 14.1, 6.8, 'on plan'), ('Stuttgart', 760000, 7.6, -5.1, 'behind'),
  ('Cologne', 890000, 8.9, 1.2, 'on plan'), ('Düsseldorf', 640000, 6.4, -0.4, 'at risk')) t(store, revenue, margin, growth, status);`;
const NEAR = `${STORES}
SELECT store ::TABLE, revenue AS "Revenue" ::MONEY, margin AS "Margin %" ::COLORSCALE,
       growth AS "vs last year" ::TREND, status AS "Status" ::BADGE FROM stores ORDER BY revenue DESC;`;
const WIDE = `SELECT k ::TABLE, ${Array.from({ length: 14 }, (_, i) => `${i + 1} * 1234567.89 AS "Measure number ${i + 1}"`).join(", ")}
FROM (VALUES ('a'), ('b')) t(k);`;

const browser = await chromium.launch();
async function open(sql, width = 1100) {
  const page = await browser.newPage({ viewport: { width, height: 900 }, reducedMotion: "reduce" });
  await page.goto(`${base}?embed=1#sql=${b64(sql)}`); // the dashboard alone, as the gallery shot it
  await page.waitForSelector(".dash .tbl-scroll table.dp-table", { timeout: 120000 });
  await page.waitForTimeout(800);
  return page;
}
const state = (page) =>
  page.evaluate(() => {
    const s = document.querySelector(".dash .tbl-scroll");
    const t = s.querySelector("table.dp-table");
    const last = [...t.querySelectorAll("tbody tr:first-child td")].pop();
    return {
      tight: t.classList.contains("dp-tight"),
      fade: s.classList.contains("dp-more-r"),
      overflow: s.scrollWidth - s.clientWidth,
      lastInside: last.getBoundingClientRect().right <= s.getBoundingClientRect().right + 1,
    };
  });

{
  const page = await open(NEAR);
  const st = await state(page);
  check(st.tight, "a table a little too wide tightens its padding");
  check(st.overflow <= 2 && !st.fade, `…and fits without the fade (overflow ${st.overflow}px)`);
  check(st.lastInside, "…its last (badge) column is inside the card");
  await page.close();
}
{
  const page = await open(WIDE);
  const st = await state(page);
  check(st.overflow > 2 && st.fade, `a table far too wide still scrolls with the fade (overflow ${st.overflow}px)`);
  await page.close();
}
await browser.close();
if (bad) {
  console.error(`${bad} check(s) failed`);
  process.exit(1);
}
console.log("table fit: ok");
