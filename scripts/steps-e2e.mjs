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
// no violations on the step section; (i) the ⤓ HTML export (#523) keeps an
// offline Back/Next/step-list/announcer, drops the gate with a note instead
// of freezing it, and still flattens every step, numbered, when printed.
// conditionHolds()/clampStep() in web/app.js is what this exercises end to
// end — see tests/web/steps_test.mjs for the pure helpers.
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

// (i) the ⤓ HTML export (#523): keeps an offline stepper (Back/Next, the
// progress strip, the step list, a live announcer) wired up by
// SNAPSHOT_JS — dropping the gate (there's no query engine offline to
// re-check it: every step becomes reachable, with a note saying so) rather
// than freezing whatever was disabled on the live page. Downloaded from
// step 2 with its gate still closed (q2 unanswered), to prove the export
// doesn't carry that frozen-closed state over. Its print stylesheet still
// flattens every step, numbered, like the pre-#523 full export did.
{
  const { page } = await open({ width: 1440, height: 1000 }, "?v.lesson=2");
  const [dl] = await Promise.all([page.waitForEvent("download"), page.click("#dlhtml")]);
  const html = readFileSync(await dl.path(), "utf8");
  check(html.includes('class="dp-step-foot"'), "(i) export keeps the Back/Next footer");
  check(html.includes('class="dp-step-head"'), "(i) export keeps the progress/step-list nav");
  check(!html.includes("Answer correctly above to continue"), "(i) export drops the gate message");
  check(html.includes("Checks are not active in this offline copy."), "(i) export adds the offline note (this sequence has a gate)");
  await page.close();

  // Open the export cold, offline, with no server and no live app.js — same
  // isolation as scripts/subtabs-export-e2e.mjs.
  const off = await browser.newContext({ offline: true, viewport: { width: 1440, height: 1000 }, reducedMotion: "reduce" });
  const op = await off.newPage();
  const exportErrs = [];
  op.on("pageerror", (e) => exportErrs.push(String(e)));
  await op.route("**/*", (r) => (r.request().url().startsWith("http") ? (r.abort(), undefined) : r.continue()));
  await op.setContent(html, { waitUntil: "load" });

  const offProgress = () => op.locator(".dp-step-progress-text").first().innerText();
  const offNext = () => op.locator(".dp-step-next").first();
  const offBack = () => op.locator(".dp-step-back").first();

  check((await offProgress()).includes("Step 2 of 4"), "(i) export opens on the step it was downloaded from");
  check((await offNext().getAttribute("aria-disabled")) === null, "(i) export: Next is enabled even though its gate was closed when downloaded");

  await offNext().focus();
  await op.keyboard.press("Enter"); // keyboard-only -> step 3
  await op.waitForFunction(() => document.querySelector(".dp-step-progress-text")?.textContent?.includes("Step 3 of 4"), null, { timeout: 15000 });
  check((await offProgress()).includes("Drift sandbox"), "(i) export: keyboard Enter on Next advances to step 3");
  const focusedOffline = await op.evaluate(() => document.activeElement?.classList.contains("dp-step-title"));
  check(focusedOffline, "(i) export: focus lands on the new step's heading offline too");

  await op.locator(".dp-step-list button", { hasText: "1" }).first().click();
  await op.waitForFunction(() => document.querySelector(".dp-step-progress-text")?.textContent?.includes("Step 1 of 4"), null, { timeout: 15000 });
  check((await offProgress()).includes("Step 1 of 4"), "(i) export: a step-list button jumps back to step 1");
  check(!(await offBack().isVisible()), "(i) export: Back is hidden again on step 1");

  const announced = await op.evaluate(() =>
    [...document.body.children].some(
      (el) => el.getAttribute("aria-live") === "polite" && el.getAttribute("role") === "status" && el.textContent.includes("Step 1 of 4")
    )
  );
  check(announced, "(i) export: the persistent announcer exists offline and carries the current step");
  check(!exportErrs.length, `(i) export: no page errors navigating the offline stepper: ${exportErrs.join(" | ")}`);

  // Print: flattened back to every step, numbered, nav/footer hidden.
  await op.emulateMedia({ media: "print" });
  const printVisible = await op.evaluate(() =>
    [...document.querySelectorAll(".dp-steppane")].every((p) => getComputedStyle(p).display !== "none")
  );
  check(printVisible, "(i) export print: every step pane is visible");
  const printHeadings = await op.evaluate(() => [...document.querySelectorAll(".dp-step-title")].map((h2) => getComputedStyle(h2, "::before").content));
  for (let n = 1; n <= 4; n++) check(printHeadings.some((c) => c.includes(`Step ${n} of 4`)), `(i) export print: a heading reads Step ${n} of 4`);
  const navHidden = await op.evaluate(() => {
    const head = document.querySelector(".dp-step-head"),
      foot = document.querySelector(".dp-step-foot");
    return getComputedStyle(head).display === "none" && getComputedStyle(foot).display === "none";
  });
  check(navHidden, "(i) export print: the nav bar and footer stay hidden");

  await op.close();
  await off.close();
}

// (j) review-537 re-review: the persistent announcer's text is only
// touched on an actual change — an unrelated re-run (the ::BUTTONS
// chart/table switch on step 3, which never touches the step variable or
// any gate) must not re-mutate it, or NVDA/JAWS would re-announce
// "Step n of N" on every cross-filter.
{
  const { page, errs } = await open({ width: 1440, height: 1000 }, "?v.lesson=3&v.q2=measured+error");
  await page.waitForSelector(".dp-step-progress-text");
  const mutations = await page.evaluate(() => {
    window.__muts = 0;
    const announcer = [...document.body.children].find((el) => el.getAttribute("aria-live") === "polite" && el.getAttribute("role") === "status");
    if (!announcer) return null;
    new MutationObserver(() => window.__muts++).observe(announcer, { characterData: true, childList: true, subtree: true });
    return true;
  });
  check(mutations === true, "(j) the persistent announcer exists on step 3");
  await page.click(".dp-seg button:text-is('Table')");
  await page.waitForFunction(() => document.querySelector(".dash .dp-table"), null, { timeout: 15000 });
  const count = await page.evaluate(() => window.__muts);
  check(count === 0, `(j) an unrelated re-run (chart -> table) does not re-touch the announcer (${count} mutation(s))`);
  check(!errs.length, `(j) no page errors: ${errs.join(" | ")}`);
  await page.close();
}

// (k) a guide-step sequence inside a currently-hidden ::TAB never gets a
// live announcer at all (it would announce a step change nobody can see) —
// but it does once that tab is the one active during a render pass (a
// plain tab click never re-runs/re-finalises, so this opens straight on
// it via `?tab=`, the same deep link a reload after clicking it would
// carry — see `setTabParam`).
{
  const sql =
    "SELECT 'Shown'::TAB;\nSELECT 'visible'::MARKDOWN;\n" +
    "SELECT 'Hidden'::TAB;\nSELECT 'S1' AS hidden_lesson ::GUIDE_STEP;\nSELECT 'x'::MARKDOWN;\nSELECT 'S2'::GUIDE_STEP;\nSELECT 'y'::MARKDOWN;\n";
  const hasAnnouncer = (page) =>
    page.evaluate(() => [...document.body.children].some((el) => el.getAttribute("aria-live") === "polite" && el.getAttribute("role") === "status"));
  {
    const page = await browser.newPage({ viewport: { width: 1440, height: 1000 }, reducedMotion: "reduce" });
    const errs = [];
    page.on("pageerror", (e) => errs.push(String(e)));
    await page.goto(`${base}#sql=${b64(sql)}`);
    await settled(page);
    check(!(await hasAnnouncer(page)), "(k) a guide-step sequence inside a hidden ::TAB creates no live announcer");
    check(!errs.length, `(k) no page errors (hidden tab): ${errs.join(" | ")}`);
    await page.close();
  }
  {
    const page = await browser.newPage({ viewport: { width: 1440, height: 1000 }, reducedMotion: "reduce" });
    const errs = [];
    page.on("pageerror", (e) => errs.push(String(e)));
    await page.goto(`${base}?tab=Hidden#sql=${b64(sql)}`);
    await settled(page);
    await page.waitForSelector(".dp-step-progress-text");
    check(await hasAnnouncer(page), "(k) the same sequence gets an announcer once its tab is the active one");
    check(!errs.length, `(k) no page errors (shown tab): ${errs.join(" | ")}`);
    await page.close();
  }
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
