// "Charts as tables" opt-in (#401) in a real browser: off by default (no
// `.dp-a11y-bar`); the toolbar's ⋯ menu item (#astables-toggle) toggles the
// feature either way, bar or no bar, and keeps `aria-pressed` in sync; a
// dashboard with `-- @astables on` shows the bar for every viewer, and
// `-- @astables off` (or any other value) does not.
//
//   wasm-pack build --target web --out-dir web/pkg --profile wasm --no-default-features --features wasm
//   python3 serve_nocache.py 8798 &
//   NODE_PATH=<dir with playwright>/node_modules node scripts/astables-e2e.mjs [--url http://127.0.0.1:8798/]
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

const browser = await chromium.launch();
async function open(sql) {
  const page = await browser.newPage({ viewport: { width: 1200, height: 1000 }, reducedMotion: "reduce" });
  const errs = [];
  page.on("pageerror", (e) => errs.push(String(e)));
  await page.goto(`${base}?mode=view#sql=${b64(sql)}`);
  await page.waitForFunction(
    () => /\d+ panels?/.test(document.getElementById("status")?.textContent || "") && !document.body.classList.contains("loading"),
    null,
    { timeout: 120000 }
  );
  return { page, errs };
}
const CHART = `SELECT region::XAXIS, revenue::BARCHART FROM (VALUES ('North', 10), ('South', 20)) t(region, revenue);`;
const state = (page) =>
  page.evaluate(() => ({
    bar: !!document.querySelector(".dp-a11y-bar"),
    tables: document.body.classList.contains("dp-as-tables"),
    menuPressed: document.getElementById("astables-toggle")?.getAttribute("aria-pressed"),
  }));

// (a) no header, no install default: off, no bar, menu starts unpressed.
{
  const { page, errs } = await open(CHART);
  const s = await state(page);
  check(!s.bar, "(a) no bar without -- @astables on");
  check(!s.tables, "(a) tables off by default");
  check(s.menuPressed === "false", "(a) menu item starts aria-pressed=false");
  // the menu toggle switches charts to tables, and back, with no bar ever
  // appearing — the feature must be reachable even on a bar-less dashboard.
  await page.evaluate(() => document.getElementById("astables-toggle").click());
  const on = await state(page);
  check(on.tables, "(a) menu toggle switches charts to tables");
  check(!on.bar, "(a) …without summoning the bar");
  check(on.menuPressed === "true", "(a) menu item reflects aria-pressed=true");
  const chartHidden = await page.evaluate(() => {
    const fig = document.querySelector(".dash .panel");
    return !!fig && fig.querySelector(".panel-svg").hidden && !!fig.querySelector(".dp-astable-view table");
  });
  check(chartHidden, "(a) the chart panel itself is swapped for its data table");
  await page.evaluate(() => document.getElementById("astables-toggle").click());
  const off = await state(page);
  check(!off.tables, "(a) the menu toggle switches back to charts");
  check(off.menuPressed === "false", "(a) …and back to aria-pressed=false");
  check(!errs.length, "(a) no page errors: " + errs.join(" | "));
  await page.close();
}

// (b) `-- @astables on`: the bar shows, and its own button also toggles the
// same remembered state as the menu item (they stay in sync).
{
  const { page, errs } = await open(`-- @astables on\n${CHART}`);
  const s = await state(page);
  check(s.bar, "(b) -- @astables on shows the bar");
  check(!s.tables, "(b) still off until switched on");
  await page.click(".dp-astable-btn");
  const on = await state(page);
  check(on.tables, "(b) the bar's own button switches to tables");
  check(on.menuPressed === "true", "(b) the menu item follows the bar button");
  await page.click(".dp-astable-btn");
  check(!errs.length, "(b) no page errors: " + errs.join(" | "));
  await page.close();
}

// (c) `-- @astables off` (explicit) and an unrecognized value: same as no
// header at all — no bar.
for (const v of ["off", "maybe"]) {
  const { page, errs } = await open(`-- @astables ${v}\n${CHART}`);
  const s = await state(page);
  check(!s.bar, `(c) -- @astables ${v} shows no bar`);
  check(!errs.length, `(c/${v}) no page errors: ` + errs.join(" | "));
  await page.close();
}

await browser.close();
console.log(`astables-e2e: ${bad} failed`);
if (bad) process.exitCode = 1;
