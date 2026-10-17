// A locked server in a real browser (test-locked-server.sh, section 6):
//   * #162 — a ::TRACK strip whose `-- @requires` input is missing is left
//     out of its chart: no error card, no "Not available" card of its own;
//   * #163 — a ::TAB / ::SUBTAB whose `-- @requires` input is missing has no
//     button and no pane, its panels are never requested, and `?tab=` to it
//     falls back to the first shown tab.
//
//   NODE_PATH=<dir with playwright>/node_modules node duckext/scripts/requires-tabs-e2e.mjs <base url>
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

// Statements of tabs.sql (test-locked-server.sh): 7 = the Forecastability
// ::TAB, 8–13 its panels and sub-tabs, 10 = the Detail ::SUBTAB.
const browser = await chromium.launch();
async function open(path) {
  const page = await browser.newPage({ viewport: { width: 1200, height: 900 } });
  const asked = [];
  page.on("request", (r) => r.url().endsWith("/api/panel") && asked.push(JSON.parse(r.postData() || "{}").panel));
  const errs = [];
  page.on("pageerror", (e) => errs.push(String(e)));
  await page.goto(base + path);
  await page.waitForFunction(() => document.querySelector(".tabbar .tab-btn") && !document.querySelector(".dp-lazy"), null, { timeout: 60000 });
  await page.waitForTimeout(800); // idle prefetch of the other panes
  return { page, asked, errs };
}
const state = (page) =>
  page.evaluate(() => ({
    tabs: [...document.querySelectorAll(".tabbar > .tab-btn")].map((b) => b.textContent),
    active: document.querySelector(".tabbar > .tab-btn.active")?.textContent,
    panes: document.querySelectorAll(".tabwrap > .tabpane").length,
    subs: [...document.querySelectorAll(".subtab-btn")].map((b) => b.textContent),
    tracks: [...document.querySelectorAll(".panel-svg")].map((h) => (h._tracks || []).length).filter((n) => n),
    errcards: document.querySelectorAll(".dp-errcard").length,
    na: document.querySelectorAll(".dp-na").length,
  }));

// A run without the tab's file: no Forecastability tab, none of its panels asked.
{
  const { page, asked, errs } = await open("/d/tabs?v.run=r2");
  const s = await state(page);
  check(JSON.stringify(s.tabs) === '["Overview"]' && s.panes === 1, `the hidden tab has no button and no pane: ${JSON.stringify(s)}`);
  check(asked.includes(7) && ![8, 9, 10, 11, 12, 13].some((i) => asked.includes(i)), `only the tab's own statement is asked, none of its panels: ${JSON.stringify(asked)}`);
  check(JSON.stringify(s.tracks) === "[1]" && s.errcards === 0 && s.na === 0, `the missing strip is left out, no card: ${JSON.stringify(s)}`);
  check(!errs.length, `no page errors: ${errs.join(" | ")}`);
  await page.close();
}
// `?tab=` to the hidden tab: the first shown tab.
{
  const { page } = await open("/d/tabs?v.run=r2&tab=Forecastability");
  const s = await state(page);
  check(s.active === "Overview", `?tab= to a hidden tab falls back to the first: ${JSON.stringify(s)}`);
  await page.close();
}
// A run with it: the tab is there; its Detail sub-tab (no sub.parquet) is not.
{
  const { page, asked, errs } = await open("/d/tabs?v.run=r1&tab=Forecastability");
  await page.waitForFunction(() => !document.querySelector(".dp-lazy"), null, { timeout: 30000 }).catch(() => {});
  const s = await state(page);
  check(JSON.stringify(s.tabs) === '["Overview","Forecastability"]' && s.active === "Forecastability", `the tab is shown: ${JSON.stringify(s)}`);
  check(JSON.stringify(s.subs) === '["Other"]', `the hidden sub-tab has no button: ${JSON.stringify(s)}`);
  check(!asked.includes(11) && asked.includes(13), `the hidden sub-tab's panel is never asked: ${JSON.stringify(asked)}`);
  check(s.errcards === 0 && s.na === 0, `no error / Not available cards: ${JSON.stringify(s)}`);
  // keyboard: the tab buttons stay plain buttons in order (Tab moves between them)
  const order = await page.evaluate(() => [...document.querySelectorAll(".tabbar > .tab-btn, .subtabbar > .subtab-btn")].map((b) => b.tagName + ":" + b.tabIndex));
  check(order.every((o) => o === "BUTTON:0"), `tab buttons are focusable buttons: ${JSON.stringify(order)}`);
  check(!errs.length, `no page errors: ${errs.join(" | ")}`);
  await page.close();
}
await browser.close();
if (failed) {
  console.error(`${failed} check(s) failed`);
  process.exit(1);
}
console.log("requires-tabs-e2e: ok");
