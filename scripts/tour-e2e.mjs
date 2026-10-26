// Guided tours (#518 PR B) in a real browser, on
// web/examples/supply-chain.sql's `amplify_tour` (4 steps: `page`,
// `chain_diagram` in "Where it amplifies", `stock_backlog_chart` in "Stock
// and service", `policy_table` in "What would stabilise it"; `-- @tour_
// autostart amplify_tour`):
// (a) `?tour=amplify_tour`: the callout reads "1 of 4", a centred `page`
//     step (no ring); (b) Next switches tabs as it reaches each step,
//     "?tab=" updates, the ring tracks the target; (c) Back goes back;
// (d) Esc ends the tour, removes `?tour`, focus returns to what had it;
// (e) the toolbar's Tour button starts it; (f) an unknown target is
// skipped (a small inline dashboard); (g) a cross-filter re-run (a
// ::BUTTONS click) while the tour runs keeps the ring on the re-rendered
// target; (h) axe (--axe path/to/axe.min.js) has no serious/critical
// violation with the tour open; (i) at 390px the callout is a bottom sheet,
// no horizontal scroll; (j) `-- @tour_autostart` fires once per browser,
// not again on a reload once closed; (k) `?tour=` with garbage does
// nothing. tourStart()/tourShowFrom()/revealTourAncestors()/placePop() in
// web/app.js is what this exercises end to end — see
// tests/web/tour_test.mjs for the pure helpers (readTourParam, placePop).
//
//   cargo build --lib --target wasm32-unknown-unknown --profile wasm --no-default-features --features wasm
//   wasm-bindgen --target web --out-dir web/pkg <target>/wasm32-unknown-unknown/wasm/anofox_visualization.wasm
//   python3 serve_nocache.py 8798 &
//   NODE_PATH=<dir with playwright>/node_modules node scripts/tour-e2e.mjs \
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

const sql = readFileSync(new URL("../web/examples/supply-chain.sql", import.meta.url), "utf8");
const b64 = (s) => Buffer.from(s, "utf8").toString("base64");

// A small dashboard with no `-- @name` at all: its tour's one step targets
// a name that doesn't exist — (f) and (k).
const unknownSql = `-- @tour t Unknown target\n--   no_such_panel: never shows\nSELECT 1 ::METRIC, 'Only panel'::LABEL;\n`;

const browser = await chromium.launch();
const settled = (page) =>
  page.waitForFunction(
    () => /\d+ panels?/.test(document.getElementById("status")?.textContent || "") && !document.body.classList.contains("loading"),
    null,
    { timeout: 60000 }
  );
// Each test gets its OWN browser context (and so its own localStorage) —
// `-- @tour_autostart` must never leak "seen" between tests, and (j) itself
// depends on a clean slate.
async function open(viewport, query = "", theme = "light", script = sql) {
  const ctx = await browser.newContext({ viewport, reducedMotion: "reduce", colorScheme: theme, acceptDownloads: true });
  const page = await ctx.newPage();
  const errs = [];
  page.on("pageerror", (e) => errs.push(String(e)));
  await page.goto(query && query.startsWith("http") ? query : `${base}${query}#sql=${b64(script)}`);
  await settled(page);
  return { ctx, page, errs };
}
const tourState = (page) =>
  page.evaluate(() => {
    const pop = document.querySelector(".dp-tour-pop");
    const ring = document.querySelector(".dp-tour-ring");
    return {
      hasPop: !!pop,
      count: document.querySelector(".dp-tour-count")?.textContent || "",
      body: document.querySelector(".dp-tour-body")?.textContent || "",
      tab: document.querySelector(".tab-btn.active")?.textContent || "",
      ringHidden: ring ? ring.hidden : null,
      ringRect: ring && !ring.hidden ? ring.getBoundingClientRect() : null,
      url: location.search,
      sheet: pop ? pop.classList.contains("dp-tour-sheet") : false,
    };
  });
const rectContains = (outer, inner) => outer.left <= inner.left + 1 && outer.top <= inner.top + 1 && outer.right >= inner.right - 1 && outer.bottom >= inner.bottom - 1;

// (a) `?tour=amplify_tour`: "1 of 4", a centred `page` step (no ring).
{
  const { ctx, page, errs } = await open({ width: 1440, height: 1000 }, "?tour=amplify_tour");
  const s = await tourState(page);
  check(s.hasPop, "(a) the callout shows");
  check(s.count === "1 of 4", `(a) reads "1 of 4", got "${s.count}"`);
  check(s.ringHidden === true, "(a) no ring on the `page` step");
  check(!errs.length, `(a) no page errors: ${errs.join(" | ")}`);
  await ctx.close();
}

// (b) Next steps through tabs; (c) Back goes back; the ring tracks a real
// target's rect.
{
  const { ctx, page } = await open({ width: 1440, height: 1000 }, "?tour=amplify_tour");
  await page.click(".dp-tour-next");
  await page.waitForTimeout(300);
  let s = await tourState(page);
  check(s.count === "2 of 4" && s.tab === "Where it amplifies", `(b) step 2 opens its tab, got ${JSON.stringify(s)}`);
  check(/[?&]tab=/.test(s.url) === false || /tab=Where/.test(decodeURIComponent(s.url)), "(b) ?tab= reflects the opened tab (or none needed on the default tab)");
  const diagramRect = await page.evaluate(() => document.querySelector('[data-dp-name="chain_diagram"]')?.getBoundingClientRect());
  check(!!s.ringRect && rectContains(s.ringRect, diagramRect), "(b) the ring's rect contains the target's rect");

  await page.click(".dp-tour-next");
  await page.waitForTimeout(300);
  s = await tourState(page);
  check(s.count === "3 of 4" && s.tab === "Stock and service", `(b) step 3 opens its own tab, got ${JSON.stringify(s)}`);

  // (c) Back returns to step 2, its tab re-opens.
  await page.click(".dp-tour-back");
  await page.waitForTimeout(300);
  s = await tourState(page);
  check(s.count === "2 of 4" && s.tab === "Where it amplifies", `(c) Back returns to step 2 and its tab, got ${JSON.stringify(s)}`);
  await ctx.close();
}

// (d) Esc ends the tour, removes `?tour`, focus returns to what had it —
// the toolbar's Tour button (e).
{
  const { ctx, page } = await open({ width: 1440, height: 1000 });
  await page.click("#dp-tour-btn"); // (e) the toolbar button starts it
  await page.waitForSelector(".dp-tour-pop");
  check(true, "(e) the toolbar's Tour button starts the tour");
  const focused = await page.evaluate(() => document.activeElement?.className || "");
  check(/dp-tour-next/.test(focused), `(d) focus moves to Next/Done on start, got "${focused}"`);
  await page.keyboard.press("Escape");
  await page.waitForTimeout(200);
  const s = await tourState(page);
  const focusedAfter = await page.evaluate(() => document.activeElement?.id || "");
  check(!s.hasPop, "(d) Esc ends the tour");
  check(s.url === "", `(d) Esc removes ?tour from the URL, got "${s.url}"`);
  check(focusedAfter === "dp-tour-btn", `(d) focus returns to the Tour button, got "${focusedAfter}"`);
  await ctx.close();
}

// (f) an unknown target is skipped — the tour's only step is `no_such_
// panel`, so it never resolves at all and the tour doesn't start.
{
  const { ctx, page, errs } = await open({ width: 1440, height: 1000 }, "?tour=t", "light", unknownSql);
  const s = await tourState(page);
  check(!s.hasPop, "(f) a tour whose only step is an unknown target never starts");
  check(!errs.length, `(f) no page errors: ${errs.join(" | ")}`);
  await ctx.close();
}

// (g) a cross-filter re-run (a ::BUTTONS click) while the tour runs keeps
// the ring on the re-rendered target.
{
  const { ctx, page } = await open({ width: 1440, height: 1000 }, "?tour=amplify_tour");
  await page.click(".dp-tour-next"); // step 2: chain_diagram
  await page.waitForTimeout(300);
  await page.click(".dp-seg button:text-is('Supplier outage')"); // the Scenario ::BUTTONS
  await settled(page);
  await page.waitForTimeout(300);
  const s = await tourState(page);
  check(s.hasPop && s.count === "2 of 4", `(g) the tour stays on step 2 through a cross-filter re-run, got ${JSON.stringify(s)}`);
  const diagramRect = await page.evaluate(() => document.querySelector('[data-dp-name="chain_diagram"]')?.getBoundingClientRect());
  check(!!s.ringRect && rectContains(s.ringRect, diagramRect), "(g) the ring still tracks the (re-rendered) target");
  await ctx.close();
}

// (h) axe: no serious/critical violation with the tour's callout open.
if (axe && existsSync(axe)) {
  const { ctx, page } = await open({ width: 1440, height: 1000 }, "?tour=amplify_tour");
  await page.addScriptTag({ path: axe });
  const res = await page.evaluate(async () => window.axe.run(".dp-tour-pop, .dp-tour-ring", { runOnly: { type: "tag", values: ["wcag2a", "wcag2aa"] } }));
  for (const v of res.violations) console.log(`axe tour ${v.impact}: ${v.id} (${v.nodes.length}) ${v.help}`);
  const serious = res.violations.filter((v) => v.impact === "serious" || v.impact === "critical");
  check(!serious.length, `(h) axe: no serious/critical violation on the tour (${serious.map((v) => v.id).join(", ")})`);
  await ctx.close();
} else {
  console.log("(h) skipped: no --axe axe.min.js given");
}

// (i) at 390px the callout is a bottom sheet, no horizontal scroll.
{
  const { ctx, page } = await open({ width: 390, height: 844 }, "?tour=amplify_tour");
  await page.click(".dp-tour-next"); // a real (non-`page`) step
  await page.waitForTimeout(300);
  const s = await tourState(page);
  const scroll = await page.evaluate(() => document.documentElement.scrollWidth > document.documentElement.clientWidth + 1);
  check(s.sheet, "(i) the callout is a bottom sheet at 390px");
  check(!scroll, "(i) no horizontal scroll at 390px");
  if (shots) await page.screenshot({ path: `${shots}/tour-390.png` });
  await ctx.close();
}

// (j) `-- @tour_autostart`: fires once, not again on a reload (localStorage
// "seen"), but only once the tour has actually been closed (so the URL no
// longer carries `?tour=` into the reload — see decision 6).
{
  const { ctx, page } = await open({ width: 1440, height: 1000 });
  let s = await tourState(page);
  check(s.hasPop && s.count === "1 of 4", `(j) autostart fires on a fresh visit, got ${JSON.stringify(s)}`);
  await page.keyboard.press("Escape");
  await page.waitForTimeout(200);
  await page.reload();
  await settled(page);
  await page.waitForTimeout(500);
  s = await tourState(page);
  check(!s.hasPop, "(j) autostart does not fire again on reload");
  await ctx.close();
}

// (k) `?tour=` with garbage does nothing (an id that doesn't parse, and one
// that parses but names no tour) — once autostart has already "fired", so
// only the URL path is exercised.
{
  const { ctx, page } = await open({ width: 1440, height: 1000 });
  await page.evaluate(() => localStorage.setItem("dp:tour-seen:untitled:amplify_tour", "1"));
  for (const bad of ["?tour=;DROP TABLE x", "?tour=no_such_tour", "?tour=" + "a".repeat(100)]) {
    await page.goto(`${base}${encodeURI(bad)}#sql=${b64(sql)}`);
    await settled(page);
    await page.waitForTimeout(300);
    const s = await tourState(page);
    check(!s.hasPop, `(k) garbage ?tour= (${bad}) starts nothing`);
  }
  await ctx.close();
}

// Screenshots: step 1 and a step in another tab, light and dark, 1440px.
if (shots) {
  for (const theme of ["light", "dark"]) {
    const { ctx, page } = await open({ width: 1440, height: 1000 }, "?tour=amplify_tour", theme);
    await page.screenshot({ path: `${shots}/tour-step1-${theme}.png` });
    await page.click(".dp-tour-next");
    await page.waitForTimeout(300);
    await page.screenshot({ path: `${shots}/tour-step2-${theme}.png` });
    await ctx.close();
  }
  console.log(`shots written to ${shots}`);
}

await browser.close();
console.log(`tour-e2e: ${bad} failed`);
if (bad) process.exitCode = 1;
