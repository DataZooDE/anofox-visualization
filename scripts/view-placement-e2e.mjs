// Opt-in ::VIEW switcher placement (#534) in a real browser: `'header'::
// VIEW_PLACEMENT` right before a ::VIEW group's first marker moves its
// segmented switcher into the shown alternative's own card title (right-
// aligned) instead of its own row above the card — only when every
// alternative is exactly one panel; otherwise it stays 'row' (the default,
// unaffected by this feature at all). Below a minimum card width the
// buttons fold into a labelled <select>. placeViewHeaderBar() /
// foldViewHeaderBar() in web/app.js are what this exercises — the layout
// and fold decision are measured in the real DOM, so no Node unit test
// covers them (see tests/web/views_test.mjs's structural source checks for
// the pure parts: the role, the class toggle, the constant).
//
//   wasm-pack build --target web --out-dir web/pkg --profile wasm --no-default-features --features wasm
//   python3 serve_nocache.py 8797 &
//   NODE_PATH=<dir with playwright>/node_modules node scripts/view-placement-e2e.mjs [--url http://127.0.0.1:8797/] [--shots <dir>]
import { createRequire } from "module";

const require = createRequire(import.meta.url);
const { chromium } = require("playwright");
const arg = (k, d) => {
  const i = process.argv.indexOf(k);
  return i > 0 ? process.argv[i + 1] : d;
};
const base = arg("--url", "http://127.0.0.1:8797/");
const shots = arg("--shots", "");
const b64 = (s) => Buffer.from(s, "utf8").toString("base64");
let bad = 0;
const check = (ok, m) => (ok ? console.log("ok   " + m) : (bad++, console.error("FAIL " + m)));

const LONG_TITLE = "Weekly revenue, every channel and region combined — a long card title";
const SQL = `CREATE TABLE weekly AS
SELECT 'W' || lpad((w + 1)::VARCHAR, 2, '0') AS week,
       (900 + 40 * w + (hash(w) % 60)::INTEGER) AS revenue
FROM range(10) r(w);

-- a plain chart beside the header-placed group, same row, to check the
-- two cards' tops line up (the title row must not grow taller above it).
SELECT 6::COL;
SELECT week::XAXIS, revenue::LINECHART, 'Neighbour'::TITLE FROM weekly ORDER BY week;

SELECT 6::COL; SELECT 'header'::VIEW_PLACEMENT;
SELECT 'Lines' AS layout ::VIEW;
SELECT week::XAXIS, revenue::LINECHART, '${LONG_TITLE}' AS t ::TITLE FROM weekly ORDER BY week;
SELECT 'Bars' AS layout ::VIEW;
SELECT week::XAXIS, revenue::BARCHART, '${LONG_TITLE}' AS t ::TITLE FROM weekly ORDER BY week;
SELECT 'Table' AS layout ::VIEW;
SELECT 'none'::TOTALS;
SELECT week AS "Week" ::TABLE, revenue AS "Revenue" ::MONEY FROM weekly ORDER BY week;
SELECT 1::ENDVIEW;

-- a group that asked for 'header' but doesn't qualify (one alternative
-- holds two panels) — views.rs falls back to 'row'; this must render
-- exactly like it always has, untouched by this feature.
SELECT 'header'::VIEW_PLACEMENT;
SELECT 'A' AS multi ::VIEW;
SELECT week::XAXIS, revenue::LINECHART, 'Multi A1'::TITLE FROM weekly ORDER BY week;
SELECT week::XAXIS, revenue::BARCHART, 'Multi A2'::TITLE FROM weekly ORDER BY week;
SELECT 'B' AS multi ::VIEW;
SELECT week::XAXIS, revenue::BARCHART, 'Multi B'::TITLE FROM weekly ORDER BY week;
SELECT 1::ENDVIEW;

-- an ordinary group with no ::VIEW_PLACEMENT at all — today's default,
-- must also render exactly as it always has.
SELECT 'X' AS plain ::VIEW;
SELECT week::XAXIS, revenue::LINECHART, 'Plain X'::TITLE FROM weekly ORDER BY week;
SELECT 'Y' AS plain ::VIEW;
SELECT week::XAXIS, revenue::BARCHART, 'Plain Y'::TITLE FROM weekly ORDER BY week;
SELECT 1::ENDVIEW;
`;

const browser = await chromium.launch();
const settled = (page) =>
  page.waitForFunction(
    () => /\d+ panels?/.test(document.getElementById("status")?.textContent || "") && !document.body.classList.contains("loading"),
    null,
    { timeout: 120000 }
  );
// Only a VISIBLE pane's lazy jobs ever fire — a hidden ::VIEW alternative
// (or ::TAB) defers its own until shown, so a blanket ".dash .dp-lazy"
// check would wait forever once this script's dashboard has more than one
// alternative. Ignore a .dp-lazy under any ancestor with inline
// `display: none` (a hidden .dp-viewpane/.tabpane).
const paneReady = (page) =>
  page
    .waitForFunction(
      () => {
        const hidden = (el) => {
          for (let n = el; n; n = n.parentElement) if (n.style && n.style.display === "none") return true;
          return false;
        };
        return ![...document.querySelectorAll(".dash .dp-lazy")].some((el) => !hidden(el));
      },
      null,
      { timeout: 120000 }
    )
    .then(() => page.waitForTimeout(300));
async function open(viewport, query = "", theme = "") {
  const page = await browser.newPage({ viewport, reducedMotion: "reduce", colorScheme: theme || "light" });
  const errs = [];
  page.on("pageerror", (e) => errs.push(String(e)));
  await page.goto(`${base}${query}#sql=${b64(SQL)}`);
  await settled(page);
  await paneReady(page);
  return { page, errs };
}

// What the checks read from the page, for a given ::VIEW group id.
const measureGroup = (page, id) =>
  page.evaluate((id) => {
    const bar = document.querySelector(`.dp-viewbar[data-view-id="${id}"]`);
    const sec = bar?.closest(".dp-views");
    if (!bar || !sec) return null;
    const header = sec.classList.contains("dp-views-header");
    const title = bar.closest(".panel-title");
    const panel = bar.closest(".panel");
    const barR = bar.getBoundingClientRect();
    const panelR = panel ? panel.getBoundingClientRect() : null;
    // The title's own CONTENT-box right edge, not the panel's: a chart's
    // hover toolbox corner button (unrelated to #534) can reserve a strip
    // of ::panel-title padding-right (.dp-narrow) — the switcher is right-
    // aligned within whatever content width the title actually has, not
    // necessarily flush with the card's own edge.
    const titleContentRight = title
      ? title.getBoundingClientRect().right - parseFloat(getComputedStyle(title).paddingRight || "0")
      : null;
    // Only the title's OWN text node (mkTitle's first child, the switcher
    // bar appended after it) — a range over the whole figcaption would also
    // count the bar's nested buttons as extra "lines" via their own rects,
    // even when the text itself sits on one line.
    const titleRange = title?.firstChild && document.createRange();
    if (titleRange) titleRange.selectNodeContents(title.firstChild);
    return {
      header,
      inOwnRow: !!sec.querySelector(":scope > .dp-viewbar") && !title,
      inTitle: !!title,
      panelTop: panelR?.top,
      barRight: barR.right,
      panelRight: panelR?.right,
      titleContentRight,
      barTop: barR.top,
      titleTop: title?.getBoundingClientRect().top,
      titleLines: titleRange ? new Set([...titleRange.getClientRects()].map((r) => Math.round(r.top))).size : 0,
      folded: bar.classList.contains("dp-viewbar-folded"),
      selectVisible: !!bar.querySelector(".dp-viewbar-select") && getComputedStyle(bar.querySelector(".dp-viewbar-select")).display !== "none",
      tabsVisible: [...bar.querySelectorAll('[role="tab"]')].filter((b) => getComputedStyle(b).display !== "none").length,
      activeLabel: bar.querySelector('[role="tab"][aria-selected="true"]')?.textContent,
    };
  }, id);

{
  const { page, errs } = await open({ width: 1920, height: 1080 });

  // (a) the header-placed group's bar is inside the shown card's own
  // title, right-aligned, not a separate row above it.
  const m = await measureGroup(page, "layout");
  check(m.header, "(a) the 'header'-placed group is marked dp-views-header");
  check(m.inTitle && !m.inOwnRow, `(a) its switcher lives in the card's panel-title, not its own row (${JSON.stringify(m)})`);
  check(Math.abs(m.barRight - m.titleContentRight) < 2, `(a) the switcher is right-aligned within the title's own content width (bar right ${m.barRight}, title content right ${m.titleContentRight})`);
  check(m.titleLines === 1, `(a) at 1920px the long title still fits one line, unwrapped (${m.titleLines})`);
  check(Math.abs(m.barTop - m.titleTop) < 4, `(a) the switcher stays on the title's first line (bar top ${m.barTop}, title top ${m.titleTop})`);

  // (b) the neighbouring plain card's top lines up with the header card's.
  const tops = await page.evaluate(() => {
    const panels = [...document.querySelectorAll(".dash > .grid > .panel, .dash > .grid > .dp-views .panel")];
    const neighbour = panels.find((p) => p.querySelector(".panel-title")?.textContent === "Neighbour");
    const header = panels.find((p) => p.querySelector(".panel-title")?.textContent?.includes("Weekly revenue"));
    return { n: neighbour?.getBoundingClientRect().top, h: header?.getBoundingClientRect().top };
  });
  check(tops.n != null && tops.h != null && Math.abs(tops.n - tops.h) < 1, `(b) the two neighbouring cards' tops line up (${JSON.stringify(tops)})`);

  // (c) the two "doesn't qualify"/"no placement" groups are untouched —
  // still their own row above the card, never moved into a title.
  for (const id of ["multi", "plain"]) {
    const g = await measureGroup(page, id);
    check(g && !g.header && g.inOwnRow && !g.inTitle, `(c) group "${id}" stays 'row' (unaffected): ${JSON.stringify(g)}`);
  }

  // (d) still a tablist with arrow-key navigation when not folded.
  await page.focus('.dp-viewbar[data-view-id="layout"] [role="tab"][aria-selected="true"]');
  await page.keyboard.press("ArrowRight");
  const afterArrow = await measureGroup(page, "layout");
  check(afterArrow.activeLabel === "Bars", `(d) ArrowRight moves the active tab (now "${afterArrow.activeLabel}")`);
  const url1 = new URL(page.url());
  check(url1.searchParams.get("view.layout") === "Bars", "(d) the URL reflects the new alternative (?view.layout=Bars)");

  // (e) focus stays sensible when the card swaps (the clicked tab inside
  // the NOW-visible card still has focus, not lost to <body>).
  const focusTag = await page.evaluate(() => document.activeElement?.getAttribute("role"));
  check(focusTag === "tab", `(e) focus stays on a tab button after the card swap (was ${focusTag})`);
  const focusInVisiblePane = await page.evaluate(() => {
    const el = document.activeElement;
    const pane = el?.closest(".dp-viewpane");
    return !!pane && pane.style.display !== "none";
  });
  check(focusInVisiblePane, "(e) the focused tab is inside the now-visible pane");

  check(!errs.length, `no page errors: ${errs.join(" | ")}`);
  await page.close();
}

// (f) below a minimum card width the buttons fold into a labelled <select>.
{
  const { page, errs } = await open({ width: 390, height: 900 });
  const m = await measureGroup(page, "layout");
  check(m.folded, `(f) at 390px the header switcher is folded (${JSON.stringify(m)})`);
  check(m.selectVisible && m.tabsVisible === 0, `(f) the <select> shows, the segmented buttons are hidden (visible tabs ${m.tabsVisible})`);
  check(m.titleLines > 1, `(f) the title wraps onto more than one line before the folded select does (${m.titleLines})`);
  const label = await page.evaluate(() => document.querySelector('.dp-viewbar[data-view-id="layout"] select')?.getAttribute("aria-label"));
  check(!!label, `(f) the folded <select> has an aria-label ("${label}")`);
  // selecting an option still switches the pane and the URL, like a click.
  await page.selectOption('.dp-viewbar[data-view-id="layout"] select', { label: "Table" });
  await page.waitForTimeout(150);
  const after = await measureGroup(page, "layout");
  check(after.activeLabel === "Table", `(f) picking an option in the <select> switches the active alternative (now "${after.activeLabel}")`);
  check(new URL(page.url()).searchParams.get("view.layout") === "Table", "(f) …and updates the URL (?view.layout=Table)");
  const noHscroll = await page.evaluate(() => document.documentElement.scrollWidth <= document.documentElement.clientWidth + 1);
  check(noHscroll, "(f) no horizontal scroll at 390px");
  check(!errs.length, `(f) no page errors: ${errs.join(" | ")}`);
  await page.close();
}

// (g) a window resize re-folds without reloading (same bar, same group).
{
  const { page, errs } = await open({ width: 1920, height: 1080 });
  const wide = await measureGroup(page, "layout");
  check(!wide.folded, `(g) wide: not folded (${JSON.stringify(wide)})`);
  await page.setViewportSize({ width: 390, height: 900 });
  await page.waitForTimeout(500); // the 150ms debounce + redraw
  const narrow = await measureGroup(page, "layout");
  check(narrow.folded, "(g) narrow after resize: folded");
  check(!errs.length, `(g) no page errors: ${errs.join(" | ")}`);
  await page.close();
}

// (h) the switcher survives a cross-filter re-run of the active alternative
// (an input change re-queries it — a real DOM swap, not just new data in
// place; the relocated bar must not be thrown away with the old panel).
{
  const reSQL = `CREATE TABLE rerun_weekly AS SELECT w, 900 + w * 10 AS revenue FROM range(10) r(w);
SELECT DISTINCT 'A' AS region ::DROPDOWN UNION ALL SELECT 'B';
SELECT 'header'::VIEW_PLACEMENT;
SELECT 'Lines' AS layout ::VIEW;
SELECT w AS week::XAXIS, revenue::LINECHART, 'Chart ' || getvariable('region') AS t ::TITLE FROM rerun_weekly;
SELECT 'Bars' AS layout ::VIEW;
SELECT w AS week::XAXIS, revenue::BARCHART, 'Chart ' || getvariable('region') AS t ::TITLE FROM rerun_weekly;
SELECT 1::ENDVIEW;
`;
  const page = await browser.newPage({ viewport: { width: 1920, height: 1080 }, reducedMotion: "reduce" });
  const errs = [];
  page.on("pageerror", (e) => errs.push(String(e)));
  await page.goto(`${base}#sql=${b64(reSQL)}`);
  await settled(page);
  await paneReady(page);
  const before = await measureGroup(page, "layout");
  check(before.inTitle, `(h) before the re-run: still in the title (${JSON.stringify(before)})`);
  await page.selectOption(".dash select", "B");
  await page.waitForTimeout(1200);
  const after = await measureGroup(page, "layout");
  check(after?.inTitle, `(h) a cross-filter re-run of the active alternative keeps the switcher in the title (${JSON.stringify(after)})`);
  const titleText = await page.evaluate(() => document.querySelector(".panel-title")?.textContent);
  check(titleText?.includes("Chart B"), `(h) …and the panel itself did re-render with the new value ("${titleText}")`);
  check(!errs.length, `(h) no page errors: ${errs.join(" | ")}`);
  await page.close();
}

// screenshots: desktop (1920) and a phone width (390), light and dark.
if (shots) {
  for (const [tag, vp] of [["1920", { width: 1920, height: 1080 }], ["390", { width: 390, height: 900 }]]) {
    for (const theme of ["light", "dark"]) {
      const { page } = await open(vp, "", theme);
      await page.screenshot({ path: `${shots}/view-placement-${tag}-${theme}.png` });
      console.log(`shot ${shots}/view-placement-${tag}-${theme}.png`);
      await page.close();
    }
  }
}

await browser.close();
console.log(`view-placement-e2e: ${bad} failed`);
if (bad) process.exitCode = 1;
