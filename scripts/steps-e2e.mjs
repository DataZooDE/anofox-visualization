// `::GUIDE_STEP` (#516) in a real browser, on web/examples/onboarding-tutorial.sql
// (step variable `lesson`, 4 steps — "The three doors", "Check your
// understanding" (gated on its own quiz, `q2`), "Drift sandbox", "Wrap-up"):
// (a) the strip reads "Step 1 of 4", Back is hidden, Next is enabled;
// (b) Next goes to step 2, the URL gains ?v.lesson=2, focus lands on the
//     step heading, and the live region text changes;
// (c) on step 2 Next is aria-disabled with the gate message visible;
// answering the quiz correctly enables it; (d) reloading ?v.lesson=4 with
// the gate still closed clamps back to step 2; (e) a step-list button jumps
// back to step 1; (f) the keyboard alone (Tab, Enter) completes the
// tutorial; (g) at 390px no horizontal scroll, the step list is hidden and
// the footer buttons are full width; (h) axe (--axe path/to/axe.min.js) has
// no violations on the step section; (i) the ⤓ HTML export contains every
// step's heading. conditionHolds()/clampStep() in web/app.js is what this
// exercises end to end — see tests/web/steps_test.mjs for the pure helpers.
//
//   wasm-pack build --target web --out-dir web/pkg --profile wasm --no-default-features --features wasm
//   python3 serve_nocache.py 8798 &
//   NODE_PATH=<dir with playwright>/node_modules node scripts/steps-e2e.mjs \
//       [--url http://127.0.0.1:8798/] [--shots <dir>] [--axe axe.min.js]
import { createRequire } from "module";
import { readFileSync, existsSync } from "fs";

const require = createRequire(import.meta.url);
const { chromium } = require("playwright");
const arg = (k, d) => {
  const i = process.argv.indexOf(k);
  return i > 0 ? process.argv[i + 1] : d;
};
const base = arg("--url", "http://127.0.0.1:8798/");
const shots = arg("--shots", "");
const axe = arg("--axe", "");
let bad = 0;
const check = (ok, m) => (ok ? console.log("ok   " + m) : (bad++, console.error("FAIL " + m)));

const sql = readFileSync(new URL("../web/examples/onboarding-tutorial.sql", import.meta.url), "utf8");
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
const progressText = (page) => page.locator(".dp-step-progress-text").first().innerText();
const next = (page) => page.locator(".dp-step-next").first();
const back = (page) => page.locator(".dp-step-back").first();
const scroll = (page) => page.evaluate(() => document.documentElement.scrollWidth > document.documentElement.clientWidth + 1);

// (a) on load: "Step 1 of 4", Back hidden, Next enabled (step 1 has no gate).
{
  const { page, errs } = await open({ width: 1440, height: 1000 });
  check((await progressText(page)).includes("Step 1 of 4"), "(a) the strip reads Step 1 of 4");
  check((await progressText(page)).includes("The three doors"), "(a) the strip names step 1");
  check((await back(page).count()) === 0, "(a) Back is hidden on step 1");
  check(await next(page).isVisible(), "(a) Next is visible");
  check((await next(page).getAttribute("aria-disabled")) === null, "(a) Next is enabled (no gate on step 1)");
  check(!errs.length, `(a) no page errors: ${errs.join(" | ")}`);
  await page.close();
}

// (b) Next goes to step 2; the URL gains ?v.lesson=2; focus lands on the new
// step's heading; the live progress text changes.
{
  const { page, errs } = await open({ width: 1440, height: 1000 });
  await next(page).focus();
  await next(page).click();
  await page.waitForFunction(() => document.querySelector(".dp-step-progress-text")?.textContent?.includes("Step 2 of 4"), null, { timeout: 15000 });
  const url = await page.evaluate(() => location.href);
  check(/[?&]v\.lesson=2(&|#|$)/.test(url), `(b) the URL carries ?v.lesson=2, got ${url}`);
  check((await progressText(page)).includes("Check your understanding"), "(b) the live region names step 2");
  const focused = await page.evaluate(() => document.activeElement?.classList.contains("dp-step-title") && document.activeElement?.textContent === "Check your understanding");
  check(focused, "(b) focus lands on the new step's heading");
  check(!errs.length, `(b) no page errors: ${errs.join(" | ")}`);
  await page.close();
}

// (c) on step 2, Next is aria-disabled with the gate message visible;
// answering the quiz correctly enables it (no page errors either side).
{
  const { page, errs } = await open({ width: 1440, height: 1000 }, "?v.lesson=2");
  check((await next(page).getAttribute("aria-disabled")) === "true", "(c) Next is aria-disabled before the quiz is answered");
  check(await page.locator(".dp-step-message").first().isVisible(), "(c) the gate message is visible");
  check((await page.locator(".dp-step-message").first().innerText()).includes("Answer correctly"), "(c) the gate message text shows");
  const sel = page.locator('select[aria-label="Which check sees concept drift?"]');
  await sel.focus();
  await sel.selectOption({ label: "measured error" });
  await page.waitForFunction(() => document.querySelector(".dp-step-next")?.getAttribute("aria-disabled") !== "true", null, { timeout: 15000 });
  check((await next(page).getAttribute("aria-disabled")) === null, "(c) Next is enabled once answered correctly");
  check(!(await page.locator(".dp-step-message").first().isVisible()), "(c) the gate message hides once Next is enabled");
  check(!errs.length, `(c) no page errors: ${errs.join(" | ")}`);
  await page.close();
}

// (d) reloading ?v.lesson=4 with the gate still closed (q2 unanswered)
// clamps back to step 2 — the step whose own gate is closed.
{
  const { page, errs } = await open({ width: 1440, height: 1000 }, "?v.lesson=4");
  check((await progressText(page)).includes("Step 2 of 4"), `(d) clamps to step 2, got: ${await progressText(page)}`);
  const url = await page.evaluate(() => location.href);
  check(/[?&]v\.lesson=2(&|#|$)/.test(url), `(d) the URL is rewritten to ?v.lesson=2, got ${url}`);
  check(!errs.length, `(d) no page errors: ${errs.join(" | ")}`);
  await page.close();
}

// (e) a step-list button jumps back to step 1.
{
  const { page, errs } = await open({ width: 1440, height: 1000 }, "?v.lesson=3&v.q2=measured+error");
  check((await progressText(page)).includes("Step 3 of 4"), `(e) starts on step 3, got: ${await progressText(page)}`);
  await page.locator(".dp-step-list button", { hasText: "1" }).first().click();
  await page.waitForFunction(() => document.querySelector(".dp-step-progress-text")?.textContent?.includes("Step 1 of 4"), null, { timeout: 15000 });
  check((await progressText(page)).includes("The three doors"), "(e) the list button jumps back to step 1");
  check(!errs.length, `(e) no page errors: ${errs.join(" | ")}`);
  await page.close();
}

// (f) the keyboard alone (Tab, Enter) completes the tutorial: focus Next,
// press Enter 3 times (4 steps), answering the quiz along the way with the
// keyboard too.
{
  const { page, errs } = await open({ width: 1440, height: 1000 });
  await next(page).focus();
  await page.keyboard.press("Enter"); // -> step 2
  await page.waitForFunction(() => document.querySelector(".dp-step-progress-text")?.textContent?.includes("Step 2 of 4"), null, { timeout: 15000 });
  const sel = page.locator('select[aria-label="Which check sees concept drift?"]');
  await sel.focus();
  await sel.selectOption({ label: "measured error" });
  await page.waitForFunction(() => document.querySelector(".dp-step-next")?.getAttribute("aria-disabled") !== "true", null, { timeout: 15000 });
  await next(page).focus();
  await page.keyboard.press("Enter"); // -> step 3
  await page.waitForFunction(() => document.querySelector(".dp-step-progress-text")?.textContent?.includes("Step 3 of 4"), null, { timeout: 15000 });
  await next(page).focus();
  await page.keyboard.press("Enter"); // -> step 4
  await page.waitForFunction(() => document.querySelector(".dp-step-progress-text")?.textContent?.includes("Step 4 of 4"), null, { timeout: 15000 });
  check((await next(page).count()) === 0, "(f) Next is hidden on the last step");
  check(!errs.length, `(f) no page errors: ${errs.join(" | ")}`);
  await page.close();
}

// (g) at 390px: no horizontal scroll, the step list is hidden, the footer
// buttons are full width.
{
  const { page, errs } = await open({ width: 390, height: 844 }, "?v.lesson=2");
  check(!(await scroll(page)), "(g) no horizontal scroll at 390px");
  check(!(await page.locator(".dp-step-list").first().isVisible()), "(g) the step-list pills are hidden");
  const backBox = await back(page).boundingBox();
  const nextBox = await next(page).boundingBox();
  check(!!backBox && !!nextBox && Math.abs(backBox.width - nextBox.width) < 2, "(g) Back/Next are roughly 50/50 width");
  if (shots) await page.screenshot({ path: `${shots}/steps-390.png` });
  check(!errs.length, `(g) no page errors: ${errs.join(" | ")}`);
  await page.close();
}

// (h) axe: no serious/critical violation on the step section (only with --axe).
if (axe && existsSync(axe)) {
  const { page } = await open({ width: 1440, height: 1000 }, "?v.lesson=2");
  await page.addScriptTag({ path: axe });
  const res = await page.evaluate(async () => window.axe.run(".dp-steps", { runOnly: { type: "tag", values: ["wcag2a", "wcag2aa"] } }));
  for (const v of res.violations) console.log(`axe .dp-steps ${v.impact}: ${v.id} (${v.nodes.length}) ${v.help}`);
  const serious = res.violations.filter((v) => v.impact === "serious" || v.impact === "critical");
  check(!serious.length, `(h) axe: no serious/critical violation on .dp-steps (${serious.map((v) => v.id).join(", ")})`);
  await page.close();
} else {
  console.log("(h) skipped: no --axe axe.min.js given");
}

// (i) the ⤓ HTML export contains every step's heading, flattened in order.
{
  const { page } = await open({ width: 1440, height: 1000 });
  const [dl] = await Promise.all([page.waitForEvent("download"), page.click("#dlhtml")]);
  const html = readFileSync(await dl.path(), "utf8");
  for (let n = 1; n <= 4; n++) check(html.includes(`Step ${n} of 4`), `(i) export carries the Step ${n} of 4 heading`);
  check(!html.includes('class="dp-step-foot"'), "(i) export drops the Back/Next footer");
  check(!html.includes('class="dp-step-head"'), "(i) export drops the progress/step-list nav");
  await page.close();
}

// Screenshots: step 1, step 2 (gate closed, with message), step 2 (open),
// light and dark, at 1440px and 390px (step 2 only), per the plan.
if (shots) {
  for (const theme of ["light", "dark"]) {
    const { page: p1 } = await open({ width: 1440, height: 1000 }, "", theme);
    await p1.screenshot({ path: `${shots}/steps-1-${theme}-1440.png` });
    await p1.close();
    const { page: p2 } = await open({ width: 1440, height: 1000 }, "?v.lesson=2", theme);
    await p2.screenshot({ path: `${shots}/steps-2-closed-${theme}-1440.png` });
    const sel = p2.locator('select[aria-label="Which check sees concept drift?"]');
    await sel.selectOption({ label: "measured error" });
    await p2.waitForFunction(() => document.querySelector(".dp-step-next")?.getAttribute("aria-disabled") !== "true", null, { timeout: 15000 });
    await p2.screenshot({ path: `${shots}/steps-2-open-${theme}-1440.png` });
    await p2.close();
    const { page: p390 } = await open({ width: 390, height: 844 }, "?v.lesson=2", theme);
    await p390.screenshot({ path: `${shots}/steps-2-${theme}-390.png` });
    await p390.close();
  }
  console.log(`shots written to ${shots}`);
}

await browser.close();
console.log(`steps-e2e: ${bad} failed`);
if (bad) process.exitCode = 1;
