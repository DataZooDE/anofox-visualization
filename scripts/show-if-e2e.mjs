// `::SHOW_IF` (#515) in a real browser, on web/examples/onboarding-quiz.sql:
// (a) on load the explanation panel is absent (no blank slot); (b) choosing
// an answer shows it and the URL gains ?v.q1=…; (c) reloading that URL shows
// it straight away; (d) the ::BUTTONS switch swaps the chart/table groups;
// (e) focus stays on the dropdown after the change; (f) at 390px wide no
// horizontal scroll. conditionHolds()/shownBy() in web/app.js is what this
// exercises — the skip happens in the live render loop, so no unit test
// covers it end to end.
//
//   wasm-pack build --target web --out-dir web/pkg --profile wasm --no-default-features --features wasm
//   python3 serve_nocache.py 8797 &
//   NODE_PATH=<dir with playwright>/node_modules node scripts/show-if-e2e.mjs [--url http://127.0.0.1:8797/] [--shots <dir>]
import { createRequire } from "module";
import { readFileSync } from "fs";

const require = createRequire(import.meta.url);
const { chromium } = require("playwright");
const arg = (k, d) => {
  const i = process.argv.indexOf(k);
  return i > 0 ? process.argv[i + 1] : d;
};
const base = arg("--url", "http://127.0.0.1:8797/");
const shots = arg("--shots", "");
let bad = 0;
const check = (ok, m) => (ok ? console.log("ok   " + m) : (bad++, console.error("FAIL " + m)));

const sql = readFileSync(new URL("../web/examples/onboarding-quiz.sql", import.meta.url), "utf8");
const b64 = (s) => Buffer.from(s, "utf8").toString("base64");

const browser = await chromium.launch();
const settled = (page) =>
  page.waitForFunction(
    () => /\d+ panels?/.test(document.getElementById("status")?.textContent || "") && !document.body.classList.contains("loading"),
    null,
    { timeout: 60000 }
  );
async function open(viewport, query = "", theme = "light") {
  const page = await browser.newPage({ viewport, reducedMotion: "reduce", colorScheme: theme, acceptDownloads: true });
  const errs = [];
  page.on("pageerror", (e) => errs.push(String(e)));
  await page.goto(query && query.startsWith("http") ? query : `${base}${query}#sql=${b64(sql)}`);
  await settled(page);
  return { page, errs };
}
// Panel count + whether the panel holding `text` is present, and the panel
// grid's own left edges (used to check no gap is left for a hidden panel).
const measure = (page) =>
  page.evaluate(() => {
    const panels = [...document.querySelectorAll(".dash .panel")].filter((p) => p.getClientRects().length);
    return {
      n: panels.length,
      lefts: panels.map((p) => Math.round(p.getBoundingClientRect().left)),
      text: document.querySelector(".dash")?.textContent || "",
      scroll: document.documentElement.scrollWidth > document.documentElement.clientWidth + 1,
    };
  });

// (a) on load: no answer yet, so the explanation panel is absent (no blank
// slot in its place); the ::BUTTONS switch defaults to 'chart', so the
// chart group shows and the table group doesn't.
{
  const { page, errs } = await open({ width: 1440, height: 1000 });
  const m = await measure(page);
  check(!m.text.includes("Right.") && !m.text.includes("Not quite"), "(a) the explanation is absent before an answer");
  check(m.text.includes("choose"), "(a) the dropdown's placeholder is shown");
  check(m.text.includes("Input vs. outcome"), "(a) the default (chart) group shows");
  check(!(await page.$(".dash .dp-table")), "(a) the table group does not show");
  check(!errs.length, `(a) no page errors: ${errs.join(" | ")}`);
  await page.close();
}

// (b) choosing an answer shows the explanation, and the URL gains ?v.q1=…
{
  const { page, errs } = await open({ width: 1440, height: 1000 });
  const before = await measure(page);
  const sel = page.locator('select[aria-label="Your answer"]');
  // A real keyboard/mouse pick leaves the control focused when it fires its
  // change event — Playwright's selectOption() alone does not reliably
  // simulate that focus hand-off, so focus it first (same intent as tabbing
  // to it, then choosing).
  await sel.focus();
  await sel.selectOption({ label: "measured error" });
  await page.waitForFunction(() => document.querySelector(".dash")?.textContent?.includes("Right."), null, { timeout: 15000 });
  const after = await measure(page);
  check(after.n === before.n + 1, `(b) one more panel after answering (${before.n} -> ${after.n})`);
  check(after.text.includes("Right."), "(b) the correct explanation shows");
  const url = await page.evaluate(() => location.href);
  check(/[?&]v\.q1=measured(\+|%20)error(&|#|$)/.test(url), `(b) the URL carries ?v.q1=measured+error, got ${url}`);
  // (e) focus stays on the dropdown after the change.
  const focused = await page.evaluate(() => document.activeElement === document.querySelector('select[aria-label="Your answer"]'));
  check(focused, "(e) focus stays on the dropdown after the change");
  await page.close();
}

// (c) reloading that URL shows the explanation straight away.
{
  const { page, errs } = await open({ width: 1440, height: 1000 }, `?v.q1=${encodeURIComponent("measured error")}`);
  const m = await measure(page);
  check(m.text.includes("Right."), "(c) a reload with ?v.q1= shows the explanation immediately");
  check(!errs.length, `(c) no page errors: ${errs.join(" | ")}`);
  await page.close();
}

// (d) the ::BUTTONS switch swaps the chart/table groups.
{
  const { page, errs } = await open({ width: 1440, height: 1000 });
  const chartFirst = await measure(page);
  check(chartFirst.text.includes("Input vs. outcome"), "(d) the chart group shows by default");
  await page.click(".dp-seg button:text-is('Table')");
  await page.waitForFunction(() => document.querySelector(".dash .dp-table"), null, { timeout: 15000 });
  const tableShown = await measure(page);
  check(!tableShown.text.includes("Input vs. outcome"), "(d) the chart group is gone once Table is picked");
  check(!!(await page.$(".dash .dp-table")), "(d) the table group shows");
  await page.close();
}

// (f) at 390 px wide, no horizontal scroll.
{
  const { page, errs } = await open({ width: 390, height: 844 });
  const m = await measure(page);
  check(!m.scroll, "(f) no horizontal scroll at 390px");
  if (shots) await page.screenshot({ path: `${shots}/show-if-390.png` });
  await page.close();
}

// Screenshots: before/after answering, light and dark, at 1440 and 390.
if (shots) {
  for (const theme of ["light", "dark"]) {
    for (const vp of [
      { w: 1440, h: 1000, tag: "1440" },
      { w: 390, h: 844, tag: "390" },
    ]) {
      const { page } = await open({ width: vp.w, height: vp.h }, "", theme);
      await page.screenshot({ path: `${shots}/show-if-before-${theme}-${vp.tag}.png` });
      const sel = page.locator('select[aria-label="Your answer"]');
      await sel.selectOption({ label: "measured error" });
      await page.waitForFunction(() => document.querySelector(".dash")?.textContent?.includes("Right."), null, { timeout: 15000 });
      await page.screenshot({ path: `${shots}/show-if-after-${theme}-${vp.tag}.png` });
      await page.close();
    }
  }
  console.log(`shots written to ${shots}`);
}

await browser.close();
console.log(`show-if-e2e: ${bad} failed`);
if (bad) process.exitCode = 1;
