// `-- @layout story` (#1367) in a real browser: the dashboard reads as one
// article — every tab is a section under its heading (all shown), the tab bar
// is their table of contents and scrolls to a section, sub-tabs are
// sub-sections (their bar hidden), ::MARKDOWN is plain, and an input sits in
// its section next to what it controls (`-- @inputs auto` by default), the
// opening shows the header's title, and the contents mark the section in
// view. The same SQL without the header keeps tabs.
//
//   bash scripts/build-wasm-pkg.sh
//   python3 serve_nocache.py 8798 &
//   NODE_PATH=<dir with playwright>/node_modules node scripts/story-e2e.mjs [--url http://127.0.0.1:8798/] [--shot out.png]
import { createRequire } from "module";

const require = createRequire(import.meta.url);
const { chromium } = require("playwright");
const arg = (k, d) => {
  const i = process.argv.indexOf(k);
  return i > 0 ? process.argv[i + 1] : d;
};
const base = arg("--url", "http://127.0.0.1:8798/");
const shot = arg("--shot", "");
const b64 = (s) => Buffer.from(s, "utf8").toString("base64");
let bad = 0;
const check = (ok, m) => (ok ? console.log("ok   " + m) : (bad++, console.error("FAIL " + m)));

const BODY = `CREATE OR REPLACE TEMP TABLE sales AS
  SELECT DATE '2026-01-01' + INTERVAL (i) DAY AS day, 50 + 20 * sin(i / 5.0) + (i % 7) AS units, CASE WHEN i % 2 = 0 THEN 'web' ELSE 'store' END AS channel
  FROM range(60) t(i);
SELECT 'The shortcut'::TAB;
SELECT 'The usual shortcut multiplies a sales forecast by a return rate. **In regular weeks it does fine.** It breaks when something changes: a promo, a storm, a new policy.'::MARKDOWN;
SELECT day::XAXIS, units::LINECHART FROM sales WHERE channel = 'web';
SELECT 'Why it breaks'::TAB;
SELECT 'Pick a channel; the chart below follows it.'::MARKDOWN;
SELECT channel::DROPDOWN FROM (VALUES ('web'), ('store')) t(channel);
SELECT day::XAXIS, units::BARCHART FROM sales WHERE channel = coalesce(getvariable('channel'), 'web');
SELECT 'Detail'::SUBTAB;
SELECT count(*) AS days ::METRIC FROM sales;
SELECT 'More'::SUBTAB;
SELECT sum(units) AS units ::METRIC FROM sales;
`;

const browser = await chromium.launch();
async function open(sql) {
  const page = await browser.newPage({ viewport: { width: 1280, height: 800 }, reducedMotion: "reduce" });
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
const state = (page) =>
  page.evaluate(() => {
    const dash = document.querySelector(".dash");
    const shown = (e) => !!e && getComputedStyle(e).display !== "none" && e.offsetParent !== null;
    const panes = [...document.querySelectorAll(".dash > .tabwrap > .tabpane")];
    const md = document.querySelector(".dash .panel .md-body")?.closest(".panel");
    const bar = document.querySelector(".dp-autobar");
    return {
      story: dash.classList.contains("dp-story"),
      panes: panes.length,
      shownPanes: panes.filter(shown).length,
      h2: [...document.querySelectorAll(".dash .tabpane > h2.story-h")].map((h) => h.textContent),
      h3: [...document.querySelectorAll(".dash .subtabpane > h3.story-h")].map((h) => h.textContent),
      subBarShown: shown(document.querySelector(".subtabbar")),
      shownSub: [...document.querySelectorAll(".subtabpane")].filter(shown).length,
      mdPlain: !!md && md.classList.contains("md-plain"),
      barInSection: !!bar && !!bar.closest(".tabpane") && bar.closest(".tabpane") === panes[1],
      width: dash.getBoundingClientRect().width,
    };
  });

// (a) the story
{
  const { page, errs } = await open(`-- @title Returns\n-- @layout story\n${BODY}`);
  const s = await state(page);
  check(s.story, "(a) .dash has dp-story");
  check(s.panes === 2 && s.shownPanes === 2, `(a) both sections shown (${s.shownPanes}/${s.panes})`);
  check(s.h2.join("|") === "The shortcut|Why it breaks", `(a) section headings (${s.h2.join("|")})`);
  check(s.h3.join("|") === "Detail|More" && s.shownSub === 2 && !s.subBarShown, `(a) sub-tabs are shown sub-sections, bar hidden (${s.h3.join("|")}, ${s.shownSub})`);
  check(s.mdPlain, "(a) ::MARKDOWN is plain by default");
  check(s.barInSection, "(a) the input sits in the section it controls");
  check(s.width <= 1040.5, `(a) a reading column (${Math.round(s.width)} px)`);
  const hero = await page.evaluate(() => ({
    title: document.querySelector(".dash > .story-hero .story-title")?.textContent,
    meta: document.querySelector(".dash > .story-hero .story-meta")?.textContent,
    before: document.querySelector(".dash > .story-hero")?.nextElementSibling?.matches(".grid"),
  }));
  check(hero.title === "Returns" && hero.before, `(a) the opening shows the -- @title above the page (${hero.title})`);
  check(/2 parts/.test(hero.meta || ""), `(a) …with the number of parts (${hero.meta})`);
  if (shot) await page.screenshot({ path: shot, fullPage: true });
  const y0 = await page.evaluate(() => scrollY);
  await page.evaluate(() => document.querySelectorAll(".dash > .tabbar > .tab-btn")[1].click());
  await page.waitForTimeout(300);
  const after = await page.evaluate(() => ({ y: scrollY, shown: [...document.querySelectorAll(".dash > .tabwrap > .tabpane")].filter((p) => p.style.display !== "none").length }));
  check(after.y > y0 && after.shown === 2, `(a) a contents entry scrolls to its section (${y0} → ${after.y}), nothing hides`);
  await page.waitForTimeout(300);
  const cur = await page.evaluate(() => document.querySelector(".dash > .tabbar > .tab-btn.active")?.textContent);
  check(cur === "Why it breaks", `(a) the contents mark the section in view (${cur})`);
  // the input still drives its chart
  await page.selectOption(".dp-autobar select", "store");
  await page.waitForFunction(() => !document.body.classList.contains("loading"), null, { timeout: 60000 });
  const s2 = await state(page);
  check(s2.story && s2.shownPanes === 2, "(a) after an input change, still a story with every section");
  check(!errs.length, "(a) no page errors: " + errs.join(" | "));
  await page.close();
}

// (b) without the header: tabs as before
{
  const { page, errs } = await open(`-- @title Returns\n${BODY}`);
  const s = await state(page);
  check(!s.story, "(b) no dp-story");
  check(s.shownPanes === 1 && !s.h2.length, `(b) one tab shown, no section headings (${s.shownPanes})`);
  check(!s.mdPlain, "(b) ::MARKDOWN keeps the callout default");
  check(!(await page.evaluate(() => !!document.querySelector(".story-hero"))), "(b) no story opening");
  check(!errs.length, "(b) no page errors: " + errs.join(" | "));
  await page.close();
}

// (c) `-- @markdown_style callout` and `-- @inputs manual` still win
{
  const { page, errs } = await open(`-- @title Returns\n-- @layout story\n-- @markdown_style callout\n-- @inputs manual\n${BODY}`);
  const s = await state(page);
  check(s.story && !s.mdPlain, "(c) -- @markdown_style callout wins over the story's plain");
  check(!(await page.evaluate(() => !!document.querySelector(".dp-autobar"))), "(c) -- @inputs manual keeps inputs where they stand");
  check(!errs.length, "(c) no page errors: " + errs.join(" | "));
  await page.close();
}

await browser.close();
console.log(`story-e2e: ${bad} failed`);
process.exit(bad ? 1 : 0);
