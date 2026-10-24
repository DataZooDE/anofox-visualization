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
// `initScript`, when given, runs before any page script (so a
// `window.__anofoxApp` it sets is in place when web/app.js's appBrandLoad()
// reads it) — used below for the #403 installation-default scenarios.
async function open(sql, initScript) {
  const page = await browser.newPage({ viewport: { width: 1200, height: 1000 }, reducedMotion: "reduce" });
  const errs = [];
  page.on("pageerror", (e) => errs.push(String(e)));
  if (initScript) await page.addInitScript(initScript);
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

// #403: an installation default (`window.__anofoxApp.charts_as_tables:
// true`, the same shape app-config.json/the serving process use) seeds a
// viewer's never-set initial "Charts as tables" choice. It's read after
// appBrandReady resolves (finishAssistBoot), which may be after the first
// paint — boot() never awaits it before the first run() (#244) — so these
// wait on the actual end state (the toggle's aria-pressed) rather than a
// fixed delay, the same as the "no flash" requirement: whenever it does
// resolve, the already-rendered first dashboard is swapped once, with no
// stuck-wrong state left behind.
const withInstallDefault = () => {
  window.__anofoxApp = { charts_as_tables: true };
};

// (d) the headline case: no explicit choice anywhere — the default wins,
// and the already-rendered dashboard is re-rendered as tables.
{
  const { page, errs } = await open(CHART, withInstallDefault);
  await page.waitForFunction(() => document.getElementById("astables-toggle")?.getAttribute("aria-pressed") === "true", null, { timeout: 5000 });
  const s = await state(page);
  check(s.tables, "(d) the install default switches charts to tables");
  check(s.menuPressed === "true", "(d) the menu item reflects it");
  check(!s.bar, "(d) the install default does not force the in-page bar on (#401 stays author/viewer opt-in)");
  const swapped = await page.evaluate(() => {
    const fig = document.querySelector(".dash .panel");
    return !!fig && fig.querySelector(".panel-svg").hidden && !!fig.querySelector(".dp-astable-view table");
  });
  check(swapped, "(d) the panel itself is swapped for its data table, not just the body class");
  check(!errs.length, "(d) no page errors: " + errs.join(" | "));
  await page.close();
}

// (e) an explicit `?tables=0` always wins over the install default.
{
  const page = await browser.newPage({ viewport: { width: 1200, height: 1000 }, reducedMotion: "reduce" });
  const errs = [];
  page.on("pageerror", (e) => errs.push(String(e)));
  await page.addInitScript(withInstallDefault);
  await page.goto(`${base}?mode=view&tables=0#sql=${b64(CHART)}`);
  await page.waitForFunction(
    () => /\d+ panels?/.test(document.getElementById("status")?.textContent || "") && !document.body.classList.contains("loading"),
    null,
    { timeout: 120000 }
  );
  await page.waitForTimeout(300); // give finishAssistBoot's continuation a chance to (wrongly) fire
  const s = await state(page);
  check(!s.tables, "(e) ?tables=0 keeps charts, even with the install default on");
  check(s.menuPressed === "false", "(e) the menu item stays unpressed");
  check(!errs.length, "(e) no page errors: " + errs.join(" | "));
  await page.close();
}

// (f) an explicit remembered choice (the toggle, before any install
// default is known) also wins — "never set" is the only case the default
// may seed, not "set, but now overridden".
{
  const { page, errs } = await open(CHART);
  await page.evaluate(() => document.getElementById("astables-toggle").click());
  await page.evaluate(() => document.getElementById("astables-toggle").click()); // back off — now explicitly "0" in localStorage
  const stored = await page.evaluate(() => localStorage.getItem("dp-as-tables"));
  check(stored === "0", "(f) the toggle recorded an explicit off");
  const storageState = await page.context().storageState();
  await page.close();
  const page2 = await browser.newPage({ viewport: { width: 1200, height: 1000 }, reducedMotion: "reduce", storageState });
  const errs2 = [];
  page2.on("pageerror", (e) => errs2.push(String(e)));
  await page2.addInitScript(withInstallDefault);
  await page2.goto(`${base}?mode=view#sql=${b64(CHART)}`);
  await page2.waitForFunction(
    () => /\d+ panels?/.test(document.getElementById("status")?.textContent || "") && !document.body.classList.contains("loading"),
    null,
    { timeout: 120000 }
  );
  await page2.waitForTimeout(300);
  const s2 = await state(page2);
  check(!s2.tables, "(f) a previously remembered off survives a fresh load with the install default on");
  check(!errs2.length, "(f) no page errors: " + errs2.join(" | "));
  await page2.close();
}

await browser.close();
console.log(`astables-e2e: ${bad} failed`);
if (bad) process.exitCode = 1;
