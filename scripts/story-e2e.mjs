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

// (a2) a wide screen: the contents are a column on the right that stays in
// view while the article scrolls
{
  const page = await browser.newPage({ viewport: { width: 1600, height: 800 }, reducedMotion: "reduce" });
  await page.goto(`${base}?mode=view#sql=${b64(`-- @title Returns\n-- @layout story\n${BODY}`)}`);
  await page.waitForFunction(
    () => /\d+ panels?/.test(document.getElementById("status")?.textContent || "") && !document.body.classList.contains("loading"),
    null,
    { timeout: 120000 }
  );
  const rail = async () =>
    page.evaluate(() => {
      const bar = document.querySelector(".dash > .tabbar").getBoundingClientRect();
      const text = document.querySelector(".dash > .tabwrap").getBoundingClientRect();
      return { right: bar.left >= text.right, top: Math.round(bar.top), visible: bar.bottom > 0 && bar.top < innerHeight };
    });
  const r0 = await rail();
  check(r0.right, "(a2) the contents sit right of the article");
  await page.evaluate(() => document.querySelectorAll(".dash > .tabbar > .tab-btn")[1].click());
  await page.waitForTimeout(400);
  const r1 = await rail();
  check(r1.visible, `(a2) …and stay in view after scrolling (top ${r0.top} → ${r1.top})`);
  await page.close();
}

// (a3) the reading aids (#1375–#1382): a part link, the heading link and
// ?tab=, the progress bar, "How this is computed", a -- @term, an input
// inside a sentence, a ::STORY chart that steps while scrolling, a phone
{
  const UX = `-- @title Returns
-- @layout story
-- @term wave: the returns of one sale, spread over weeks
CREATE OR REPLACE TEMP TABLE sales AS
  SELECT DATE '2026-01-01' + INTERVAL (i) DAY AS day, 50 + 20 * sin(i / 5.0) + (i % 7) AS units FROM range(60) t(i);
SELECT 'Jump to [the end](#part-2).'::MARKDOWN;
SELECT 'Start'::TAB;
SELECT 'A sale comes back as a wave, not a peak.'::MARKDOWN;
SELECT day::XAXIS, units::LINECHART, CASE WHEN day IN (DATE '2026-01-10', DATE '2026-02-10') THEN 'Step on ' || strftime(day, '%-d %B') END ::STORY FROM sales ORDER BY day;
SELECT 'End'::TAB;
SELECT 3 AS k ::NUMBER, 'Days'::LABEL;
SELECT 'Look back {{@k}} days: {{n}} units.'::MARKDOWN, round(sum(units)) AS n FROM sales WHERE day >= DATE '2026-03-01' - INTERVAL (coalesce(getvariable('k'), 3)) DAY;
SELECT day::XAXIS, units::BARCHART FROM sales;
SELECT '$$\\text{receipts}_t \\;=\\; \\sum_{s=1}^{n} \\text{sales}_s \\cdot p_s \\sum_{k=0}^{60} f_{\\text{ask}}(k)\\, f_{\\text{travel}}(t - s - k) \\;+\\; \\text{storm}_t$$'::MARKDOWN;
${Array.from({ length: 12 }, (_, i) => `SELECT 'Filler paragraph ${i} so the page scrolls.'::MARKDOWN;`).join("\n")}
`;
  const ctx = await browser.newContext({ viewport: { width: 1280, height: 800 }, reducedMotion: "reduce", permissions: ["clipboard-read", "clipboard-write"] });
  const page = await ctx.newPage();
  const errs = [];
  page.on("pageerror", (e) => errs.push(String(e)));
  const ready = () =>
    page.waitForFunction(() => /\d+ panels?/.test(document.getElementById("status")?.textContent || "") && !document.body.classList.contains("loading"), null, { timeout: 120000 });
  await page.goto(`${base}?mode=view#sql=${b64(UX)}`);
  await ready();
  await page.waitForTimeout(500);
  check(await page.evaluate(() => !!document.querySelector(".dash > .story-progress")), "(a3) a reading-progress bar");
  await page.click('.dash a[href="#part-2"]');
  await page.waitForTimeout(500);
  const jumped = await page.evaluate(() => ({ hash: location.hash.slice(0, 12), top: Math.round(document.getElementById("part-2").getBoundingClientRect().top) }));
  check(!/part-2/.test(jumped.hash) && jumped.top < 200, `(a3) a #part-N link scrolls there, the address keeps its own fragment (top ${jumped.top})`);
  await page.hover("#part-2 > h2.story-h");
  await page.click("#part-2 .story-anchor");
  await page.waitForTimeout(200);
  const copied = await page.evaluate(() => navigator.clipboard.readText());
  check(/[?&]tab=End(&|$)/.test(copied), `(a3) the heading link copies ?tab=End (${copied.slice(0, 80)}…)`);
  const folds = await page.evaluate(() => [...document.querySelectorAll(".panel .story-sql code")].map((c) => c.textContent));
  check(folds.length === 2 && folds.every((t) => /::(LINECHART|BARCHART)/.test(t) && /FROM sales/.test(t)), `(a3) each chart has its SQL folded (${folds.length})`);
  check(await page.evaluate(() => /sales/.test(document.querySelector(".story-sql-reads")?.textContent || "")), "(a3) …naming the setup table it reads");
  const term = await page.evaluate(() => [...document.querySelectorAll(".story-term")].map((t) => t.textContent));
  check(term.length === 1 && term[0] === "wave", `(a3) the -- @term is marked once in its part (${term})`);
  await page.focus(".story-term");
  check(await page.evaluate(() => { const p = document.querySelector(".story-term-pop"); return !!p && !p.hidden && /spread over weeks/.test(p.textContent); }), "(a3) …and explains itself on focus");
  const inl = await page.evaluate(() => { const b = [...document.querySelectorAll(".md-body")].find((x) => /Look back/.test(x.textContent)); return { ctl: !!b?.querySelector(".story-inline input"), raw: /\{\{@/.test(b?.textContent || "") }; });
  check(inl.ctl && !inl.raw, "(a3) {{@k}} puts the input of k inside the sentence");
  const before = await page.evaluate(() => [...document.querySelectorAll(".md-body")].find((x) => /Look back/.test(x.textContent)).textContent);
  await page.fill(".story-inline input", "10");
  await page.press(".story-inline input", "Enter");
  await page.waitForTimeout(800);
  await ready();
  const after = await page.evaluate(() => [...document.querySelectorAll(".md-body")].find((x) => /Look back/.test(x.textContent))?.textContent);
  check(after && after !== before && !!(await page.$(".story-inline input")), "(a3) …and changing it updates the sentence, the input staying in it");
  // the ::STORY chart steps while scrolling
  const box = await page.$(".story-scrolly");
  check(!!box, "(a3) a ::STORY chart scrolls (cards beside a sticky chart)");
  if (box) {
    const top = await box.evaluate((e) => e.getBoundingClientRect().top + scrollY);
    const h = await box.evaluate((e) => e.offsetHeight);
    const seen = new Set();
    for (let dy = 0; dy < h; dy += 150) {
      await page.evaluate((y) => scrollTo(0, y), top + dy);
      await page.waitForTimeout(120);
      seen.add(await box.evaluate((b) => b.querySelector(".panel")?._story?.step?.()));
    }
    check(seen.has(0) && seen.has(1) && seen.has(2), `(a3) …stepping 0 → 1 → 2 as it scrolls (${[...seen].join(",")})`);
    // the strip above the sticky chart is covered (a colour, not the canvas' background list)
    const shadow = await box.evaluate((b) => getComputedStyle(b.querySelector(".panel")).boxShadow);
    check(shadow && shadow !== "none", `(a3) …the strip above the sticky chart is covered (${shadow})`);
  }
  check(!errs.length, "(a3) no page errors: " + errs.join(" | "));
  await ctx.close();
  // a phone: no sideways scrolling, the contents one row
  const phone = await browser.newPage({ viewport: { width: 390, height: 844 }, reducedMotion: "reduce" });
  await phone.goto(`${base}?mode=view#sql=${b64(UX)}`);
  await phone.waitForFunction(() => /\d+ panels?/.test(document.getElementById("status")?.textContent || ""), null, { timeout: 120000 });
  const ph = await phone.evaluate(() => ({ sw: document.documentElement.scrollWidth, w: innerWidth, rows: new Set([...document.querySelectorAll(".dash > .tabbar > .tab-btn")].map((b) => Math.round(b.getBoundingClientRect().top))).size }));
  check(ph.sw <= ph.w && ph.rows === 1, `(a3) on a phone: no sideways scroll, contents in one row (${JSON.stringify(ph)})`);
  // only the chart stays sticky; its "How this is computed" shows once after the steps
  await phone.evaluate(() => scrollTo(0, document.body.scrollHeight));
  await phone.waitForTimeout(400);
  const pf = await phone.evaluate(() => {
    const sticky = document.querySelector(".story-scrolly > .panel .story-sql");
    const foot = document.querySelector(".story-scrolly + .story-scrolly-foot");
    const m = document.querySelector(".md-math-display.md-math-done");
    return {
      stickySql: sticky ? getComputedStyle(sticky).display : "absent",
      foot: foot ? getComputedStyle(foot).display : "absent",
      footSql: !!foot?.querySelector(".story-sql"),
      mathFits: !!m && (m.scrollWidth <= m.clientWidth + 1 || m.classList.contains("dp-math-more")),
      mathSmaller: !!m && parseFloat(m.style.fontSize || "100") < 100,
    };
  });
  check(pf.stickySql === "none" && pf.foot === "block" && pf.footSql, `(a3) on a phone: the sticky chart without its SQL fold, which follows the steps (${JSON.stringify(pf)})`);
  check(pf.mathFits && pf.mathSmaller, `(a3) on a phone: a wide formula is drawn smaller, a cut one fades (${JSON.stringify(pf)})`);
  await phone.evaluate(() => scrollTo(0, 0));
  await phone.waitForTimeout(600);
  const left = await phone.evaluate(() => document.querySelector(".dash > .tabbar").scrollLeft);
  check(left === 0, `(a3) on a phone: back at the opening, the contents start at part 1 (${left})`);
  await phone.close();
}

// (a4) the HTML export of a story (#1368): it stays a story (every part
// shown, contents scroll), and a few-valued input keeps working offline
{
  const ctx = await browser.newContext({ viewport: { width: 1280, height: 800 }, reducedMotion: "reduce", acceptDownloads: true });
  const page = await ctx.newPage();
  await page.goto(`${base}?mode=view#sql=${b64(`-- @title Returns\n-- @layout story\n${BODY}`)}`);
  await page.waitForFunction(() => /\d+ panels?/.test(document.getElementById("status")?.textContent || "") && !document.body.classList.contains("loading"), null, { timeout: 120000 });
  const [dl] = await Promise.all([page.waitForEvent("download", { timeout: 180000 }), page.click("#dlhtml")]);
  const html = await (await import("fs")).promises.readFile(await dl.path(), "utf8");
  const off = await ctx.newPage();
  const errs = [];
  off.on("pageerror", (e) => errs.push(String(e)));
  await off.setContent(html, { waitUntil: "load" });
  await off.waitForTimeout(500);
  const s0 = await off.evaluate(() => ({
    story: document.querySelector(".dash")?.classList.contains("dp-story"),
    shown: [...document.querySelectorAll(".dash > .tabwrap > .tabpane")].filter((p) => getComputedStyle(p).display !== "none").length,
    vars: document.querySelector(".dp-variants")?.dataset.vars,
    chart: document.querySelector('.dash .tabpane .panel .panel-svg svg[aria-label], .dash .tabpane .panel .panel-svg svg')?.outerHTML.length,
  }));
  check(s0.story && s0.shown === 2, `(a4) the export is still a story, every part shown (${JSON.stringify(s0)})`);
  check(s0.vars === "channel", "(a4) the 2-value dropdown is kept live (channel)");
  const bar = () => off.evaluate(() => [...document.querySelectorAll(".dash .panel")].find((p) => p.querySelector("svg") && p.closest("#part-2, .tabpane:nth-child(2)"))?.querySelector("svg")?.outerHTML);
  const before = await bar();
  await off.evaluate(() => { const s = document.querySelector('[data-varname="channel"] select'); s.value = "store"; s.dispatchEvent(new Event("change", { bubbles: true })); });
  await off.waitForTimeout(200);
  check(before && (await bar()) !== before, "(a4) …and changing it offline swaps in that value's chart");
  check(!errs.length, "(a4) no errors in the export: " + errs.join(" | "));
  await ctx.close();
}

// (a5) two button inputs in two parts, the first inside a sentence: a click
// on one leaves the other's single control in its sentence and its state
// (no stale second copy), each part's panels follow their own input — live
// and in the export, where the two combine (each input's variants hold only
// the panels it changes) and the sentence keeps its input
{
  const TWO = `-- @title Two inputs
-- @layout story
CREATE OR REPLACE TEMP TABLE t AS SELECT range AS x, range % 5 AS y FROM range(30);
SELECT 'Part one'::TAB;
SELECT kind ::BUTTONS FROM (VALUES ('low'), ('high')) v(kind);
SELECT 'Follow a {{@kind}} day: factor {{f}}.'::MARKDOWN, CASE coalesce(getvariable('kind'), 'low') WHEN 'high' THEN 3 ELSE 1 END AS f;
SELECT x::XAXIS, (y * CASE coalesce(getvariable('kind'), 'low') WHEN 'high' THEN 3 ELSE 1 END)::LINECHART FROM t ORDER BY x;
SELECT 'Part two'::TAB;
SELECT scen ::BUTTONS FROM (VALUES ('calm'), ('storm')) v(scen);
SELECT 'Scenario: {{s}}.'::MARKDOWN, coalesce(getvariable('scen'), 'calm') AS s;
SELECT x::XAXIS, (y + CASE coalesce(getvariable('scen'), 'calm') WHEN 'storm' THEN 10 ELSE 0 END)::BARCHART FROM t ORDER BY x;
`;
  const look = (pg) =>
    pg.evaluate(() => {
      const ctls = (v) => [...document.querySelectorAll(`.dash [data-varname="${v}"]`)];
      const pick = (c) => c.querySelector('[aria-checked="true"], [aria-pressed="true"]')?.textContent.trim();
      const kind = ctls("kind");
      return {
        kinds: kind.length,
        inline: kind.length === 1 && !!kind[0].closest(".story-inline-slot") && !!kind[0].closest("p"),
        kind: kind.map(pick).join(","),
        scen: ctls("scen").map(pick).join(","),
        factor: (document.querySelector(".dash").innerText.match(/factor (\d)/) || [])[1],
        scenario: (document.querySelector(".dash").innerText.match(/Scenario: (\w+)/) || [])[1],
      };
    });
  const ctx = await browser.newContext({ viewport: { width: 1280, height: 800 }, reducedMotion: "reduce", acceptDownloads: true });
  const page = await ctx.newPage();
  const errs = [];
  page.on("pageerror", (e) => errs.push(String(e)));
  await page.goto(`${base}?mode=view#sql=${b64(TWO)}`);
  await page.waitForFunction(() => /\d+ panels?/.test(document.getElementById("status")?.textContent || "") && !document.body.classList.contains("loading"), null, { timeout: 120000 });
  const settle = () => page.waitForFunction(() => !document.body.classList.contains("loading") && !document.querySelector(".dash .dp-pending"), null, { timeout: 30000 }).then(() => page.waitForTimeout(400));
  const click = async (v, label) => {
    await page.locator(`.dash [data-varname="${v}"] [role="radio"]`, { hasText: label }).click();
    await settle();
  };
  await click("kind", "high");
  let l = await look(page);
  check(l.kinds === 1 && l.inline && l.kind === "high" && l.factor === "3", `(a5) the input in the sentence drives its part (${JSON.stringify(l)})`);
  await click("scen", "storm");
  l = await look(page);
  check(l.kinds === 1 && l.inline, `(a5) after the other part's input: still one control in the sentence, no second copy (${l.kinds})`);
  check(l.kind === "high" && l.factor === "3" && l.scen === "storm" && l.scenario === "storm", `(a5) …each part keeps its own value (${JSON.stringify(l)})`);
  await click("kind", "low");
  l = await look(page);
  check(l.kinds === 1 && l.inline && l.kind === "low" && l.factor === "1" && l.scenario === "storm", `(a5) the sentence's control is the live one (${JSON.stringify(l)})`);
  check(!errs.length, "(a5) no page errors: " + errs.join(" | "));
  // the export, from the first values
  await click("scen", "calm");
  const [dl] = await Promise.all([page.waitForEvent("download", { timeout: 180000 }), page.click("#dlhtml")]);
  const html = await (await import("fs")).promises.readFile(await dl.path(), "utf8");
  const vars = [...html.matchAll(/<template data-var="(\w+)" data-val="[^"]*" data-stmt="(\d+)"/g)].map((m) => [m[1], m[2]]);
  const stmtsOf = (v) => new Set(vars.filter((x) => x[0] === v).map((x) => x[1]));
  const shared = [...stmtsOf("kind")].filter((x) => stmtsOf("scen").has(x));
  check(stmtsOf("kind").size > 0 && stmtsOf("scen").size > 0 && !shared.length, `(a5) export: each input's variants hold only its own panels (shared: ${shared})`);
  const off = await ctx.newPage();
  const oerrs = [];
  off.on("pageerror", (e) => oerrs.push(String(e)));
  await off.setContent(html, { waitUntil: "load" });
  await off.waitForTimeout(500);
  l = await look(off);
  check(l.kinds === 1 && l.inline, "(a5) export: the input stays inside its sentence");
  await off.locator('[data-varname="kind"] [role="radio"]', { hasText: "high" }).click();
  await off.locator('[data-varname="scen"] [role="radio"]', { hasText: "storm" }).click();
  await off.waitForTimeout(200);
  l = await look(off);
  check(l.kind === "high" && l.factor === "3" && l.scen === "storm" && l.scenario === "storm", `(a5) export: the two inputs combine (${JSON.stringify(l)})`);
  check(!oerrs.length, "(a5) no errors in the export: " + oerrs.join(" | "));
  await ctx.close();
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
