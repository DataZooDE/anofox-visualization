// #950: `-- @inputs auto` places each loose input by what it controls — the
// page filter in a bar above the tabs (after the intro text), a tab's filter
// at the top of that tab, a chart's own switch in that chart's title — and
// a re-run (a changed filter, a clicked switch) keeps every bar where it is,
// once.
//   NODE_PATH=tests/web/node_modules node scripts/input-placement-e2e.mjs [--url http://127.0.0.1:8947/]
import { createRequire } from "module";
const require = createRequire(import.meta.url);
const { chromium } = require("playwright");
const arg = (k, d) => {
  const i = process.argv.indexOf(k);
  return i > 0 ? process.argv[i + 1] : d;
};
const base = arg("--url", "http://127.0.0.1:8947/");
let bad = 0;
const check = (ok, m) => (ok ? console.log("ok   " + m) : (bad++, console.error("FAIL " + m)));
const b64 = (s) => Buffer.from(s, "utf8").toString("base64");
// written in the "wrong" places on purpose: the switch first, the page
// filter on tab B, the tab filter below nothing yet
const SQL = `-- @inputs auto
CREATE TABLE t AS SELECT * FROM (VALUES ('EU', 'A', 1), ('EU', 'B', 2), ('US', 'A', 3), ('US', 'B', 4)) v(region, ch, v);
SELECT '# Sales' ::MARKDOWN;
SELECT m AS measure ::BUTTONS FROM (VALUES ('Units'), ('Double')) x(m);
SELECT 'A' ::TAB;
SELECT DISTINCT ch AS ch ::DROPDOWN FROM t ORDER BY 1;
SELECT 'By region' ::TITLE, region ::XAXIS, sum(v) ::BARCHART FROM t WHERE region = getvariable('region') AND ch = getvariable('ch') GROUP BY region;
SELECT 'Measure' ::TITLE, region ::XAXIS, sum(CASE getvariable('measure') WHEN 'Double' THEN 2 * v ELSE v END) ::BARCHART FROM t WHERE ch = getvariable('ch') GROUP BY region ORDER BY region;
SELECT 'B' ::TAB;
SELECT DISTINCT region AS region ::DROPDOWN FROM t ORDER BY 1;
SELECT 'Units' ::TITLE, ch ::XAXIS, sum(v) ::BARCHART FROM t WHERE region = getvariable('region') GROUP BY ch;
`;
const browser = await chromium.launch();
const page = await browser.newPage({ viewport: { width: 1200, height: 900 } });
const errs = [];
page.on("pageerror", (e) => errs.push(String(e)));
await page.goto(`${base}?mode=view#sql=${b64(SQL)}`);
const ready = () =>
  page.waitForFunction(() => /[1-9]\d* panels?/.test(document.getElementById("status")?.textContent || "") && !document.body.classList.contains("loading") && !document.querySelector(".dash .dp-pending, .dash .dp-lazy"), null, { timeout: 120000 });
await ready();
const layout = () =>
  page.evaluate(() => {
    const dash = document.querySelector(".dash");
    const bars = [...dash.querySelectorAll(".dp-autobar")];
    const page = bars.filter((b) => b.dataset.at === "page");
    const sec = bars.filter((b) => b.dataset.at === "section");
    const pan = bars.filter((b) => b.dataset.at === "panel");
    const grid = dash.querySelector(":scope > .grid");
    const kids = grid ? [...grid.children] : [];
    const md = kids.findIndex((k) => k.textContent.includes("Sales"));
    const pane = sec[0]?.closest(".tabpane");
    const paneKids = pane ? [...pane.children] : [];
    const firstPanel = paneKids.findIndex((k) => k.matches(".panel"));
    const title = pan[0]?.closest(".panel-title");
    return {
      bars: bars.length,
      errors: dash.querySelectorAll(".dp-errcard").length,
      page: page.length,
      pageHasRegion: !!page[0]?.querySelector("select") && page[0].textContent.toLowerCase().includes("region"),
      pageAfterIntro: md >= 0 && kids.indexOf(page[0]) > md,
      pageAboveTabs: !!page[0] && page[0].closest(".tabwrap") === null,
      sec: sec.length,
      secTopOfA: !!pane && paneKids.indexOf(sec[0]) >= 0 && paneKids.indexOf(sec[0]) < firstPanel,
      pan: pan.length,
      panInTitle: !!title && title.textContent.includes("Measure"),
      buttons: pan[0] ? pan[0].querySelectorAll("button").length : 0,
    };
  });
const expect = (l, when) => {
  check(l.page === 1 && l.pageHasRegion, `${when}: one page bar, holding the region filter (${JSON.stringify(l)})`);
  check(l.pageAfterIntro && l.pageAboveTabs, `${when}: the page bar stands after the intro text, above the tabs`);
  check(l.sec === 1 && l.secTopOfA, `${when}: tab A's filter at the top of tab A`);
  check(l.pan === 1 && l.panInTitle && l.buttons === 2, `${when}: the measure switch in its chart's title`);
  check(l.bars === 3, `${when}: three bars, no copies`);
  check(l.errors === 0, `${when}: every panel drew`);
};
expect(await layout(), "start");
if (arg("--shot")) await page.screenshot({ path: arg("--shot"), fullPage: true });

// a page filter change re-runs everything
const region = page.locator('.dash .dp-autobar[data-at="page"] select');
await region.selectOption("US");
await ready();
expect(await layout(), "region=US");

// the switch in the title still works, and stays there
await page.locator('.dash .panel-title .dp-autobar button:has-text("Double")').click();
await ready();
expect(await layout(), "measure=Double");
const pressed = await page.evaluate(() => [...document.querySelectorAll(".dash .panel-title .dp-autobar button")].map((b) => b.getAttribute("aria-pressed") || b.getAttribute("aria-checked")));
check(pressed.includes("true"), `the clicked switch reads as chosen (${JSON.stringify(pressed)})`);

// without the header switch, inputs stay where they stand
await page.goto(`${base}?mode=view#sql=${b64(SQL.replace("-- @inputs auto\n", ""))}`);
await ready();
check((await page.locator(".dash .dp-autobar").count()) === 0, "no @inputs auto: no automatic bars");

check(errs.length === 0, `no page errors (${errs.join(" | ")})`);
await browser.close();
console.log(bad ? `${bad} failed` : "all ok");
process.exit(bad ? 1 : 0);
