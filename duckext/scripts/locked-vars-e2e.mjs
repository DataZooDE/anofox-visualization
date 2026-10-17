// A locked server in a real browser (test-locked-server.sh, section 4):
//   * #151 — a dashboard's own `SET VARIABLE x = coalesce(nullif(getvariable('x'),
//     ''), (SELECT …))` is never sent as a viewer value: the default applies,
//     and a viewer's choice (a dropdown, `?v.run=`) survives the setup re-run;
//   * #152 — the ::INFO column of a ::PAGED table is the ⓘ definition, not a
//     column, and the page requests leave it out (`omit`).
//
//   NODE_PATH=<dir with playwright>/node_modules node duckext/scripts/locked-vars-e2e.mjs <base url>
import { createRequire } from "module";

const require = createRequire(import.meta.url);
const { chromium } = require("playwright");
const base = process.argv[2] || "http://127.0.0.1:8983";
let failed = 0;
const check = (cond, m) => {
  if (cond) console.log("ok   " + m);
  else {
    console.error("FAIL: " + m);
    failed++;
  }
};

const browser = await chromium.launch();
async function open(path) {
  const page = await browser.newPage({ viewport: { width: 1200, height: 900 } });
  const bodies = [];
  page.on("request", (r) => r.url().endsWith("/api/panel") && bodies.push(JSON.parse(r.postData() || "{}")));
  const errs = [];
  page.on("pageerror", (e) => errs.push(String(e)));
  await page.goto(base + path);
  await page.waitForFunction(() => document.querySelector(".metric-value"), null, { timeout: 60000 });
  await page.waitForTimeout(800); // the remaining panels
  return { page, bodies, errs };
}
const kpi = (page) =>
  page.evaluate(() => ({
    n: document.querySelector(".metric-value")?.textContent.trim(),
    cap: document.querySelector(".metric-cap")?.textContent.trim(),
  }));

// No input: the dashboard's default, and `run` is never in a request.
{
  const { page, bodies, errs } = await open("/d/runs");
  const k = await kpi(page);
  check(k.n === "1" && k.cap === "run c", `default from the dashboard's own SET: ${JSON.stringify(k)}`);
  check(bodies.length > 0 && bodies.every((b) => !("run" in (b.vars || {}))), `no request sends the setup variable: ${JSON.stringify(bodies.map((b) => b.vars))}`);
  // #152: the ::PAGED table's ::INFO is its ⓘ, not a column.
  const t = await page.evaluate(() => {
    const fig = [...document.querySelectorAll("figure.panel")].find((f) => f.querySelector(".tbl-search"));
    return {
      heads: fig ? [...fig.querySelectorAll("thead th")].map((th) => th.textContent.trim()) : null,
      info: fig?.querySelector(".dp-info-pop")?.textContent,
    };
  });
  check(JSON.stringify(t.heads?.map((h) => h.replace(/[↕↑↓]/g, "").trim())) === '["Run"]', `the ::INFO column is no table column: ${JSON.stringify(t.heads)}`);
  check(t.info === "Runs of the forecast pipeline", `the ::PAGED table has the ⓘ definition: ${JSON.stringify(t.info)}`);
  const paged = bodies.filter((b) => b.page);
  check(paged.some((b) => b.page.stats) && paged.filter((b) => !(b.page.limit === 1 && !b.page.omit)).every((b) => JSON.stringify(b.page.omit) === '["about"]'), `the page requests omit the ::INFO column: ${JSON.stringify(paged.map((b) => b.page))}`);
  check(!errs.length, `no page errors: ${errs.join(" | ")}`);
  await page.close();
}

// A dropdown: its value survives the dashboard's defaulting SET (re-run per request).
{
  const { page, errs } = await open("/d/pick");
  let k = await kpi(page);
  check(k.n === "1" && k.cap === "run a", `the dropdown's first option, not the SET's source text: ${JSON.stringify(k)}`);
  await page.selectOption(`select[aria-label="run"]`, "b");
  await page.waitForFunction(() => document.querySelector(".metric-cap")?.textContent.trim() === "run b", null, { timeout: 20000 }).catch(() => {});
  k = await kpi(page);
  check(k.n === "1" && k.cap === "run b", `a picked value: ${JSON.stringify(k)}`);
  check(!errs.length, `no page errors: ${errs.join(" | ")}`);
  await page.close();
}
{
  const { page } = await open("/d/pick?v.run=c");
  const k = await kpi(page);
  check(k.n === "1" && k.cap === "run c", `a ?v.run= value: ${JSON.stringify(k)}`);
  await page.close();
}

await browser.close();
if (failed) {
  console.error(`${failed} failure(s)`);
  process.exit(1);
}
