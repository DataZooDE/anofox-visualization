// Browser e2e for a map drill-down (#954): the Examples library's "Where
// does our revenue come from, from country down to district?" in the
// builder. Countries first (world layer), a click on Germany draws its 16
// states (de-states), a click on Bavaria its districts (de-districts);
// the breadcrumb goes back, the path is in the URL. France drills to its
// départements from world-states (#956), which the page fetches only then,
// and a département (no level below) shows the example's note. A ::TREEMAP
// under the same drill path moves in step (#1002). No page errors.
//
//   python3 serve_nocache.py 8792 &     # serves web/ (after the wasm build)
//   NODE_PATH=tests/web/node_modules node scripts/map-drill-e2e.mjs [--url http://127.0.0.1:8792/]
import { createRequire } from "module";
import { readFileSync } from "fs";
import path from "path";

const require = createRequire(import.meta.url);
const { chromium } = require("playwright");
const arg = (k, d) => {
  const i = process.argv.indexOf(k);
  return i > 0 ? process.argv[i + 1] : d;
};
const base = arg("--url", "http://127.0.0.1:8792/");
let failed = 0;
const check = (cond, m) => (cond ? console.log("ok   " + m) : (failed++, console.error("FAIL " + m)));
const b64 = (s) => Buffer.from(s, "utf8").toString("base64");
const SQL = readFileSync(path.join(path.dirname(new URL(import.meta.url).pathname), "..", "web", "examples", "drill-revenue-map.sql"), "utf8");

const browser = await chromium.launch();
try {
  const page = await browser.newPage({ viewport: { width: 1280, height: 900 } });
  const errs = [];
  page.on("pageerror", (e) => errs.push(String(e)));
  const fetched = [];
  page.on("request", (r) => fetched.push(new URL(r.url()).pathname));
  const statesFetched = () => fetched.some((p) => p.endsWith("/data/boundaries/world-states.csv"));
  await page.goto(`${base}?embed=1#sql=${b64(SQL)}`);
  await page.waitForFunction(() => !document.body.classList.contains("loading"), null, { timeout: 120000 });

  // the map's features, by their region code (their row's ::REGION value)
  const CODES = `(() => {
    const svg = document.querySelector(".dash svg[data-map]");
    const dp = svg?.closest(".panel")?._dp;
    if (!dp) return [];
    const ri = dp.roles.find((r) => r[1] === "REGION")?.[0];
    const rows = JSON.parse(dp.rows);
    return [...svg.querySelectorAll("path[data-row]")].map((p) => [String(rows[+p.getAttribute("data-row")]?.["c" + ri] ?? ""), p]);
  })()`;
  const keys = () => page.evaluate((c) => eval(c).map(([k]) => k), CODES);
  const title = () => page.evaluate(() => [...document.querySelectorAll(".dash .panel")].map((p) => p.textContent).find((t) => /Revenue by/.test(t)) || "");
  const levelIs = (re) =>
    page
      .waitForFunction(
        ([c, src]) => {
          const k = eval(c).map(([k]) => k);
          return k.length > 0 && k.every((x) => new RegExp(src).test(x));
        },
        [CODES, re.source],
        { timeout: 30000 }
      )
      .then(() => true, () => false);
  const click = (key) =>
    page.evaluate(([c, k]) => eval(c).find(([x]) => x === k)?.[1].dispatchEvent(new MouseEvent("click", { bubbles: true })), [CODES, key]);

  // 1. countries
  check(await levelIs(/^[A-Z]{3}$/), "the first level draws countries by their ISO code");
  check(!statesFetched(), "the states layer isn't fetched for the country level");
  const c = await keys();
  check(c.length === 10 && c.includes("DEU") && c.includes("FRA"), `ten countries (${c.join(", ")})`);
  check(/Revenue by country/.test(await title()), "titled by country");

  // 2. Germany → its states
  await click("DEU");
  check(await levelIs(/^DE-[A-Z]{2}$/), "a click on Germany draws its states");
  check((await keys()).length === 16, `16 states (${(await keys()).length})`);
  check(/Revenue by state/.test(await title()), "titled by state");
  // #975: the URL follows the run's end, which can come a moment after the
  // map is drawn (a panel shows as soon as it is ready, #938) — wait for it
  const inUrl = await page.waitForFunction(() => /v\.geo_path=DEU/.test(location.href), null, { timeout: 5000 }).then(() => true, () => false);
  check(inUrl, `the path is in the URL (${page.url().split("?")[1] || ""})`);
  const crumbs = () => page.evaluate(() => document.querySelector('.dash .dp-crumbs[data-drill="geo"]')?.textContent || "");
  check(/DEU/.test(await crumbs()), `a breadcrumb names the drill (${await crumbs()})`);

  // 3. Bavaria → its districts
  await click("DE-BY");
  check(await levelIs(/^09\d{3}$/), "a click on Bavaria draws its districts (codes 09…)");
  const d = (await keys()).length;
  check(d > 50 && d < 120, `Bavaria's districts (${d})`);
  check(/Revenue by district/.test(await title()), "titled by district");
  // the deepest level: another click goes nowhere
  const first = (await keys())[0];
  await click(first);
  await page.waitForTimeout(500);
  check(/^09\d{3}$/.test((await keys())[0] || ""), "the last level doesn't drill further");

  // 4. the breadcrumb goes back to the states
  await page.locator('.dash .dp-crumbs[data-drill="geo"] button.dp-crumb').last().click();
  check(await levelIs(/^DE-[A-Z]{2}$/), "the breadcrumb goes back to the states");
  await page.locator('.dash .dp-crumbs[data-drill="geo"] button.dp-crumb').first().click();
  check(await levelIs(/^[A-Z]{3}$/), "…and to the countries");

  // 4b. #1002: the ::TREEMAP under the same 'geo: …' drill — tiles keyed by
  // the code (::XAXIS) and showing the name (::LABEL); a tile click drills
  // the map too, and the map's level moves the treemap.
  // the live treemap svg's tiles (not a drill motion's fading .dp-ghost copy)
  const TILES = `[...([...document.querySelectorAll(".dash .panel")].find((p) => /Share of revenue by/.test(p.textContent))?.querySelectorAll(".panel-svg > svg [data-x]") || [])].filter((e) => !e.closest(".dp-ghost"))`;
  await page.waitForFunction((t) => { const k = [...eval(t)].map((e) => e.getAttribute("data-x")); return k.length > 0 && k.every((x) => /^[A-Z]{3}$/.test(x)); }, TILES, { timeout: 30000 }).catch(() => {});
  const tiles = () => page.evaluate((t) => [...eval(t)].map((e) => e.getAttribute("data-x")), TILES);
  const tk = await tiles();
  check(tk.length === 10 && tk.includes("DEU") && tk.every((k) => /^[A-Z]{3}$/.test(k)), `treemap tiles keyed by country code (${tk.join(", ")})`);
  const shown = await page.evaluate((t) => [...eval(t)].map((e) => [...e.closest("svg").querySelectorAll("text")].map((t) => t.textContent).join(" ")).join(" "), TILES);
  check(/Germany/.test(shown) && !/\bDEU\b/.test(shown), "…showing the names, not the codes");
  await page.evaluate((t) => [...eval(t)].find((e) => e.getAttribute("data-x") === "DEU")?.dispatchEvent(new MouseEvent("click", { bubbles: true })), TILES);
  check(await levelIs(/^DE-[A-Z]{2}$/), "a treemap tile click drills the map to Germany's states");
  const tStates = await page
    .waitForFunction((t) => { const k = [...eval(t)].map((e) => e.getAttribute("data-x")); return k.length > 0 && k.every((x) => /^DE-[A-Z]{2}$/.test(x)); }, TILES, { timeout: 30000 })
    .then(() => true, () => false);
  check(tStates, "…and the treemap to the same states");
  await page.locator('.dash .dp-crumbs[data-drill="geo"] button.dp-crumb').first().click();
  check(await levelIs(/^[A-Z]{3}$/), "the breadcrumb takes both back to the countries");

  // 5. France: its départements from world-states, fetched now
  await click("FRA");
  check(await levelIs(/^FR-[0-9A-Z]{1,3}$/), "a click on France draws its départements (world-states)");
  const fr = (await keys()).length;
  check(fr >= 90 && fr <= 101, `France's départements (${fr})`);
  check(statesFetched(), "…from the world-states layer, fetched only now");
  // a département: no level below in the sample
  await click("FR-75");
  const empty = await page
    .waitForFunction(() => /Nothing below this level in the sample/.test(document.querySelector(".dash")?.textContent || "") && !document.querySelector(".dash svg[data-map] path[data-row]"), null, { timeout: 30000 })
    .then(() => true, () => false);
  check(empty, "a département drills to the example's note (no districts outside Germany)");

  check(errs.length === 0, `no page errors (${errs.join(" | ")})`);
} finally {
  await browser.close();
}
console.log(`map-drill-e2e: ${failed} failed`);
process.exit(failed ? 1 : 0);
