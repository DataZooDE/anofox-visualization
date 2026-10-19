// A locked server in a real browser (test-locked-server.sh, section 6b):
//   * #214 — only the active alternative of a ::VIEW group is ever asked for;
//     switching to another alternative asks exactly its own panel(s), once;
//     switching back is a cache hit (no new request at all);
//   * `?view.<id>=<label>` seeds the active alternative; an unknown label
//     falls back to the default (the first SHOWN alternative);
//   * an alternative whose `-- @requires` input is missing has no button, is
//     never asked for, and a raw request for its panel gets 400;
//   * a re-run (a control change) keeps the chosen alternative and only
//     re-asks the currently visible panel;
//   * the standalone HTML export keeps the switcher's markup and script, and
//     it still works with no server at all.
//
//   NODE_PATH=<dir with playwright>/node_modules node duckext/scripts/views-e2e.mjs <base url>
import { createRequire } from "module";
import fs from "node:fs";

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

// Statements of views.sql (test-locked-server.sh, section 6b): 0/1 the run
// dropdown / boost number, 2 the setup (rd); group "yoy" — 3/4 Lines
// marker/chart (needs fc.parquet, r1 only), 5-7 Bars (no requires, always
// shown), 8-10 Table (no requires, always shown), 11 ::ENDVIEW; group "2" —
// 12/13 Alpha marker/KPI (needs zx.parquet, r1 only), 14/15 Beta marker/KPI
// (needs zy.parquet, r1 only) — on r2 every alternative of group "2" is
// hidden, so the whole group is.
const browser = await chromium.launch();

// A ::VIEW pane never idle-prefetches (D3 in web/app.js): the only `.dp-lazy`
// placeholders left once a page has "settled" are the ones sitting in a
// currently-hidden `.dp-viewpane` — everything else must have resolved.
async function waitSettled(page) {
  await page.waitForFunction(
    () => {
      if (!document.querySelector(".dp-viewbar")) return false;
      const lazies = [...document.querySelectorAll(".dp-lazy")];
      return lazies.every((el) => {
        const vp = el.closest(".dp-viewpane");
        return vp && vp.style.display === "none";
      });
    },
    null,
    { timeout: 60000 }
  );
  await page.waitForTimeout(300); // let any in-flight request settle before reading `asked`
}

async function open(path) {
  const page = await browser.newPage({ viewport: { width: 1200, height: 900 } });
  const asked = [];
  page.on("request", (r) => r.url().endsWith("/api/panel") && asked.push(JSON.parse(r.postData() || "{}").panel));
  const errs = [];
  page.on("pageerror", (e) => errs.push(String(e)));
  await page.goto(base + path);
  await waitSettled(page);
  return { page, asked, errs };
}

const state = (page) =>
  page.evaluate(() => {
    const bar = (id) => document.querySelector(`.dp-viewbar[data-view-id="${id}"]`);
    const labels = (id) => (bar(id) ? [...bar(id).querySelectorAll('[role="tab"]')].map((b) => b.textContent) : null);
    const active = (id) => bar(id)?.querySelector('[aria-selected="true"]')?.textContent ?? null;
    return {
      bars: document.querySelectorAll(".dp-viewbar").length,
      yoy: labels("yoy"),
      yoyActive: active("yoy"),
      g2: labels("2"),
      g2Active: active("2"),
      errcards: document.querySelectorAll(".dp-errcard").length,
      na: document.querySelectorAll(".dp-na").length,
    };
  });

const clickView = (page, barId, label) =>
  page.evaluate(
    ([id, l]) => {
      const bar = document.querySelector(`.dp-viewbar[data-view-id="${id}"]`);
      const btn = [...bar.querySelectorAll('[role="tab"]')].find((b) => b.textContent === l);
      btn.click();
    },
    [barId, label]
  );

// 1. First load (run=r1): group "yoy" defaults to Lines (shown), group "2"
// defaults to Alpha (shown) — only their own content is asked, no sibling's.
{
  const { page, asked, errs } = await open("/d/views?v.run=r1");
  const s = await state(page);
  check(s.bars === 2, `two ::VIEW groups on the page: ${JSON.stringify(s)}`);
  check(JSON.stringify(s.yoy) === '["Lines","Bars","Table"]' && s.yoyActive === "Lines", `"yoy" defaults to Lines: ${JSON.stringify(s)}`);
  check(JSON.stringify(s.g2) === '["Alpha","Beta"]' && s.g2Active === "Alpha", `group "2" defaults to Alpha: ${JSON.stringify(s)}`);
  check(asked.includes(3) && asked.includes(4), `the default alternative is asked (marker + content): ${JSON.stringify(asked)}`);
  check(asked.includes(12) && asked.includes(13) && asked.includes(14), `group "2"'s markers (and Alpha's content) are asked: ${JSON.stringify(asked)}`);
  check(![6, 7, 9, 10, 15].some((i) => asked.includes(i)), `the other alternatives' content is never asked up front: ${JSON.stringify(asked)}`);
  check(s.errcards === 0 && s.na === 0, `no error / Not available cards: ${JSON.stringify(s)}`);
  check(!errs.length, `no page errors: ${errs.join(" | ")}`);

  // 2. Click "Bars": asks exactly its own panel(s) (6, 7), once.
  const before = asked.length;
  await clickView(page, "yoy", "Bars");
  await page.waitForFunction(() => document.querySelector('.dp-viewbar[data-view-id="yoy"] [aria-selected="true"]')?.textContent === "Bars");
  await waitSettled(page);
  const afterBars = asked.slice(before);
  // (420::HEIGHT, statement 6, is a bare literal — no query, so no request;
  // the chart, statement 7, is the only one with an actual query to run.)
  check(afterBars.includes(7), `switching to Bars asks its own panel(s): ${JSON.stringify(afterBars)}`);
  check(!afterBars.includes(4) && !afterBars.includes(9) && !afterBars.includes(10), `switching to Bars asks nothing of Lines/Table: ${JSON.stringify(afterBars)}`);
  const dupes = afterBars.filter((p) => p === 7);
  check(dupes.length === 1, `panel 7 (Bars' chart) was asked exactly once: ${JSON.stringify(afterBars)}`);

  // 3. Click back to "Lines": a cache hit — no new request at all.
  const beforeBack = asked.length;
  await clickView(page, "yoy", "Lines");
  await page.waitForFunction(() => document.querySelector('.dp-viewbar[data-view-id="yoy"] [aria-selected="true"]')?.textContent === "Lines");
  await page.waitForTimeout(300);
  check(asked.length === beforeBack, `switching back to Lines is a cache hit, no new /api/panel request: ${JSON.stringify(asked.slice(beforeBack))}`);
  await page.close();
}

// 4. `?view.yoy=Table` deep-links straight to it: only Table's content is asked.
{
  const { page, asked } = await open("/d/views?v.run=r1&view.yoy=Table");
  const s = await state(page);
  check(s.yoyActive === "Table", `?view.yoy=Table lands on Table: ${JSON.stringify(s)}`);
  // ('all'::TOTALS, statement 9, is a bare literal — no query, so no request.)
  check(asked.includes(10), `Table's own panel is asked: ${JSON.stringify(asked)}`);
  check(![4, 6, 7].some((i) => asked.includes(i)), `Lines/Bars are never asked when Table is the deep-linked default: ${JSON.stringify(asked)}`);
  await page.close();
}

// 5. An unknown/garbage label falls back to the default (first shown).
{
  const { page } = await open("/d/views?v.run=r1&view.yoy=Zzzz");
  const s = await state(page);
  check(s.yoyActive === "Lines", `an unknown ?view.yoy= value falls back to the default: ${JSON.stringify(s)}`);
  await page.close();
}

// 6. run=r2: Lines (needs fc.parquet) is hidden — no button, never asked for
// its content; the default moves to Bars (next shown). Group "2" (needs
// zx/zy.parquet) has every alternative hidden: no bar for it at all.
{
  const { page, asked, errs } = await open("/d/views?v.run=r2");
  const s = await state(page);
  check(s.bars === 1, `group "2" (every alternative hidden) has no view bar at all: ${JSON.stringify(s)}`);
  check(JSON.stringify(s.yoy) === '["Bars","Table"]' && s.yoyActive === "Bars", `Lines has no button; the default moved to Bars: ${JSON.stringify(s)}`);
  check(asked.includes(3), `Lines' marker is still asked, to learn it is hidden: ${JSON.stringify(asked)}`);
  check(!asked.includes(4), `Lines' content is never asked once it is known hidden: ${JSON.stringify(asked)}`);
  check(asked.includes(12) && asked.includes(14), `group "2"'s markers are asked, to learn the group is hidden: ${JSON.stringify(asked)}`);
  check(![13, 15].some((i) => asked.includes(i)), `group "2"'s content is never asked: ${JSON.stringify(asked)}`);
  check(s.errcards === 0 && s.na === 0, `no error / Not available cards: ${JSON.stringify(s)}`);
  check(!errs.length, `no page errors: ${errs.join(" | ")}`);

  // A raw request for the hidden alternative's panel: 400, bypassing the UI.
  const direct = await page.evaluate(
    async (base) => {
      const r = await fetch(base + "/api/panel", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ dashboard: "views", panel: 4, vars: { run: "r2" } }),
      });
      return { status: r.status, body: await r.text() };
    },
    base
  );
  check(direct.status === 400 && direct.body.includes("no files found"), `a raw request for the hidden alternative's panel: ${JSON.stringify(direct)}`);

  // 7. A re-run (changing `boost`) keeps Bars active and only re-asks its
  // own panel — not Lines' (hidden) or Table's (shown but not active).
  const before = asked.length;
  // The ::NUMBER control ("boost"): its <label class="control"> wraps the
  // <input>, so it is natively associated — getByLabel finds it directly.
  await page.getByLabel("boost").fill("20");
  await page.keyboard.press("Tab"); // blur: the control's onchange fires the re-run
  await page.waitForTimeout(600);
  await waitSettled(page);
  const afterBoost = asked.slice(before);
  // A full re-run re-checks every ::VIEW marker's hidden status fresh (it
  // clears the client cache outright) — 3/12/14 are expected again; what
  // matters is that no CONTENT statement of the hidden or inactive
  // alternatives is ever asked.
  check(afterBoost.includes(7), `the re-run re-asks the active (Bars) panel: ${JSON.stringify(afterBoost)}`);
  check(![4, 9, 10, 13, 15].some((i) => afterBoost.includes(i)), `the re-run asks nothing for the hidden view's or the inactive one's content: ${JSON.stringify(afterBoost)}`);
  const s2 = await state(page);
  check(s2.yoyActive === "Bars", `the re-run keeps the currently-selected view: ${JSON.stringify(s2)}`);
  await page.close();
}

// 8. Standalone HTML export keeps the switcher's markup + script, and it
// still works with no server at all (best-effort: no existing e2e script in
// this repo tests HTML export, so this is a minimal, self-contained check).
{
  const page = await browser.newPage({ viewport: { width: 1200, height: 900 }, acceptDownloads: true });
  await page.goto(base + "/d/views?v.run=r1");
  await waitSettled(page);
  const [download] = await Promise.all([page.waitForEvent("download"), page.evaluate(() => document.getElementById("dlhtml").click())]);
  const dlPath = await download.path();
  const html = dlPath ? fs.readFileSync(dlPath, "utf8") : "";
  check(html.includes('class="dp-views"') && html.includes("dp-viewbar") && html.includes("dp-viewpane"), "the exported HTML keeps the ::VIEW switcher's markup");
  check(html.includes("viewActivate"), "the exported HTML keeps the ::VIEW switcher's standalone script");

  // Copy it out before closing the page — closing it tears down its
  // (per-page) browser context, which deletes the download's temp file.
  let p = null;
  let dir = null;
  if (html) {
    dir = fs.mkdtempSync((process.env.TMPDIR || "/tmp") + "/views-e2e-");
    p = dir + "/dashboard.html";
    fs.writeFileSync(p, html);
  }
  await page.close();

  if (p) {
    const page2 = await browser.newPage();
    await page2.goto("file://" + p);
    const before = await page2.evaluate(() => document.querySelector('.dp-viewbar[data-view-id="yoy"] [aria-selected="true"]')?.textContent);
    await page2.evaluate(() => {
      const bar = document.querySelector('.dp-viewbar[data-view-id="yoy"]');
      const btn = [...bar.querySelectorAll('[role="tab"]')].find((b) => b.textContent === "Bars");
      btn.click();
    });
    const after = await page2.evaluate(() => document.querySelector('.dp-viewbar[data-view-id="yoy"] [aria-selected="true"]')?.textContent);
    check(before === "Lines" && after === "Bars", `the standalone export's switcher still works offline: ${before} -> ${after}`);
    await page2.close();
    fs.rmSync(dir, { recursive: true, force: true });
  }
}

await browser.close();
if (failed) {
  console.error(`${failed} check(s) failed`);
  process.exit(1);
}
console.log("views-e2e: ok");
