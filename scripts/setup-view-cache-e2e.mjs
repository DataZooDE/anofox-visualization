// #1304: a panel that reads a variable only through a TEMP VIEW the setup
// made (`FROM thr`, thr = getvariable('lrn')) re-runs when that variable
// changes — also after another input changed (before the fix its cached
// result came back: the cache key held only the panel's own variables).
//   NODE_PATH=<playwright>/node_modules node scripts/setup-view-cache-e2e.mjs --url http://127.0.0.1:8947/
import { createRequire } from "module";
const require = createRequire(import.meta.url);
const { chromium } = require("playwright");
const i = process.argv.indexOf("--url");
const base = i > 0 ? process.argv[i + 1] : "http://127.0.0.1:8947/";
let bad = 0;
const b64 = (s) => Buffer.from(s, "utf8").toString("base64");
const SQL = `CREATE OR REPLACE TEMP VIEW thr AS
SELECT coalesce(try_cast(getvariable('lrn')->>'threshold' AS DOUBLE), 0.5) AS t;
SELECT 'First'::TAB;
SELECT s AS pick ::DROPDOWN, 'Adjust for'::LABEL FROM (VALUES ('minimal'), ('nothing')) v(s);
SELECT coalesce(getvariable('pick'), 'minimal') AS v ::METRIC, 'picked'::LABEL;
SELECT 'Second'::TAB;
SELECT field AS lrn ::FORM, type, value, min, max, step, label
FROM (VALUES ('threshold', 'slider', '0.5', 0.0, 1.0, 0.05, 'Threshold')) f(field, type, value, min, max, step, label);
SELECT round(t, 2) AS t ::METRIC, 'threshold seen'::LABEL FROM thr;
`;
const b = await chromium.launch();
const run = async (changeDropdown) => {
  const p = await b.newPage({ viewport: { width: 1200, height: 900 } });
  await p.goto(`${base}#sql=${b64(SQL)}`);
  await p.waitForFunction(() => /\d+ panels?/.test(document.getElementById("status")?.textContent || "") && !document.body.classList.contains("loading"), null, { timeout: 120000 });
  await p.waitForTimeout(600);
  if (changeDropdown) {
    await p.locator('select:has(option:text-is("nothing"))').first().selectOption("nothing");
    await p.waitForTimeout(1500);
  }
  await p.click('.tab-btn:has-text("Second")');
  await p.waitForTimeout(1200);
  const kpi = () => p.evaluate(() => [...document.querySelectorAll(".dash .panel")].map((f) => f.textContent).find((t) => /threshold seen/.test(t)) || "");
  const before = await kpi();
  const slider = p.locator('input[type="range"]').first();
  await slider.evaluate((el) => { el.value = "0.25"; el.dispatchEvent(new Event("input", { bubbles: true })); el.dispatchEvent(new Event("change", { bubbles: true })); });
  await p.waitForTimeout(3000);
  const after = await kpi();
  const ok = /^0\.5/.test(before.trim()) && /^0\.25/.test(after.trim());
  if (!ok) bad++;
  console.log(`${ok ? "ok  " : "FAIL"} ${changeDropdown ? "after a dropdown change" : "fresh page"}: ${before.replace(/\s+/g, " ").trim()} -> ${after.replace(/\s+/g, " ").trim()}`);
  await p.close();
};
await run(false);
await run(true);
await b.close();
console.log(bad ? `${bad} FAILED` : "all ok");
process.exit(bad ? 1 : 0);
