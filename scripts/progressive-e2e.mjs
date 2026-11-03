// #390: a re-run shows each changed panel as soon as it is ready — after an
// input change the fast panel shows its new value while the slow one is still
// pending; at the end every statement has exactly one panel, none pending, and
// a second change works the same way.
//   NODE_PATH=tests/web/node_modules node scripts/progressive-e2e.mjs [--url http://127.0.0.1:8947/]
import { createRequire } from "module";
const require = createRequire(import.meta.url);
const { chromium } = require("playwright");
const arg = (k, d) => {
  const i = process.argv.indexOf(k);
  return i > 0 ? process.argv[i + 1] : d;
};
const base = arg("--url", "http://127.0.0.1:8947/");
let bad = 0;
const check = (ok, m) => (ok ? console.log("ok   " + m) : (bad++, console.error("FAIL " + m)));
const b64 = (s) => Buffer.from(s, "utf8").toString("base64");
const SQL = `SELECT k ::DROPDOWN FROM (VALUES (1), (2), (3)) t(k);
SELECT getvariable('k') * 10 AS fast ::METRIC, 'Fast' ::LABEL;
SELECT count(*) + getvariable('k') AS slow ::METRIC, 'Slow' ::LABEL FROM range(60000000) r(i) WHERE i % 7 = 0;
`;
const browser = await chromium.launch();
const page = await browser.newPage({ viewport: { width: 1200, height: 900 } });
const errs = [];
page.on("pageerror", (e) => errs.push(String(e)));
await page.goto(`${base}?mode=view#sql=${b64(SQL)}`);
const ready = () =>
  page.waitForFunction(() => /[1-9]\d* panels?/.test(document.getElementById("status")?.textContent || "") && !document.body.classList.contains("loading") && !document.querySelector(".dash .dp-pending"), null, { timeout: 120000 });
await ready();
const value = (label) =>
  page.evaluate((label) => {
    const p = [...document.querySelectorAll(".dash .panel.metric")].find((x) => x.textContent.includes(label));
    return p ? { v: p.querySelector(".metric-value")?.textContent, pending: p.classList.contains("dp-pending") } : null;
  }, label);
check((await value("Fast"))?.v === "10", `start: fast = 10 (${JSON.stringify(await value("Fast"))})`);

for (const [k, fast] of [["2", "20"], ["3", "30"]]) {
  await page.selectOption(".dash select", k);
  // the fast panel updates while the slow one still runs
  const seen = await page
    .waitForFunction(
      (fast) => {
        const ps = [...document.querySelectorAll(".dash .panel.metric")];
        const f = ps.find((x) => x.textContent.includes("Fast"));
        const s = ps.find((x) => x.textContent.includes("Slow"));
        return f?.querySelector(".metric-value")?.textContent === fast && s?.classList.contains("dp-pending");
      },
      fast,
      { timeout: 60000, polling: 20 }
    )
    .then(() => true, () => false);
  check(seen, `k=${k}: the fast panel shows ${fast} while the slow one is still pending`);
  await ready();
  const counts = await page.evaluate(() => {
    const c = {};
    for (const p of document.querySelectorAll(".dash .panel[data-stmt]")) c[p.dataset.stmt] = (c[p.dataset.stmt] || 0) + 1;
    return c;
  });
  check(Object.values(counts).every((n) => n === 1), `k=${k}: one panel per statement (${JSON.stringify(counts)})`);
  check((await value("Fast"))?.v === fast && !(await value("Slow"))?.pending, `k=${k}: both done, fast = ${fast}`);
}
check(!errs.length, "no page errors: " + errs.join(" | "));
await browser.close();
console.log(`progressive-e2e: ${bad} failed`);
process.exit(bad ? 1 : 0);
