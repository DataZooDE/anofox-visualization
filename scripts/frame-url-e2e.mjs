// Browser e2e for a named time map's frame in the URL (M3e-2, #647): stepping
// a `-- @name` ::FRAME map writes `?frame.<name>=<label>` (not for the last
// frame, the default, and never for an unnamed map); reloading that URL opens
// the map on that frame; an unknown label is ignored. Then ::SYNC_FRAME
// (#664): maps of one group step and play together.
//
//   build web/pkg (scripts/build-wasm-pkg.sh)
//   python3 serve_nocache.py 8813 &
//   NODE_PATH=<playwright>/node_modules node scripts/frame-url-e2e.mjs [--url http://127.0.0.1:8813/]
import { createRequire } from "module";

const require = createRequire(import.meta.url);
const { chromium } = require("playwright");
const arg = (k, d) => {
  const i = process.argv.indexOf(k);
  return i > 0 ? process.argv[i + 1] : d;
};
const base = arg("--url", "http://127.0.0.1:8813/");
const fail = (m) => {
  console.error("FAIL: " + m);
  process.exitCode = 1;
};
const ok = (m) => console.log("ok: " + m);
const b64 = (s) => Buffer.from(s, "utf8").toString("base64");

const rows = (v) =>
  [1, 2, 3, 4]
    .map((w) => `('POLYGON((0 0, 0 10, 10 10, 10 0, 0 0))', 'North', 'Week ${w}', ${v + w}), ('POLYGON((10 0, 10 10, 20 10, 20 0, 10 0))', 'South', 'Week ${w}', ${v + 10 * w})`)
    .join(",\n    ");
const SQL = `
SELECT 'Frame URL e2e'::DASHBOARD_TITLE;
SELECT 6::COL;
-- @name weekly
SELECT wkt ::MAP, wk AS "Week" ::FRAME, v AS "On time" ::BARCHART, region ::LABEL, 'Named'::TITLE
FROM (VALUES
    ${rows(10)}
) t(wkt, region, wk, v) ORDER BY wk, region;
SELECT 6::COL;
SELECT wkt ::MAP, wk AS "Week" ::FRAME, v AS "On time" ::BARCHART, region ::LABEL, 'Unnamed'::TITLE
FROM (VALUES
    ${rows(20)}
) t(wkt, region, wk, v) ORDER BY wk, region;
`;

const browser = await chromium.launch();
const page = await browser.newPage({ viewport: { width: 1400, height: 1000 } });
const errs = [];
page.on("pageerror", (e) => errs.push(String(e)));
const open = async (url) => {
  // via about:blank: a goto to the same URL (only the #sql hash) wouldn't reload
  await page.goto("about:blank");
  await page.goto(url);
  await page.waitForFunction(
    () => document.querySelectorAll(".dash .dp-frames").length >= 2 && !document.body.classList.contains("loading"),
    null,
    { timeout: 120000 }
  );
  await page.waitForTimeout(700); // the seeded mount's own throttled URL sync
};
const pos = (title) =>
  page.evaluate((t) => {
    const panel = [...document.querySelectorAll(".dash .panel")].find((p) => p.querySelector(".panel-title")?.textContent.trim() === t);
    return panel?.querySelector(".dp-frames-pos")?.textContent.trim() || "";
  }, title);
const step = async (title, cls) => {
  await page.evaluate(
    ({ t, cls }) => {
      const panel = [...document.querySelectorAll(".dash .panel")].find((p) => p.querySelector(".panel-title")?.textContent.trim() === t);
      panel.querySelector("." + cls).click();
    },
    { t: title, cls }
  );
  await page.waitForTimeout(700); // ≥ the 500 ms throttle
};
const frameParams = () => page.evaluate(() => [...new URL(location.href).searchParams].filter(([k]) => k.startsWith("frame.")));

const start = `${base}?embed=1#sql=${b64(SQL)}`;
await open(start);
if ((await frameParams()).length) fail(`the default (last) frame wrote a URL param: ${JSON.stringify(await frameParams())}`);
else ok("no frame param on the default frame");

await step("Named", "dp-frames-prev");
await step("Named", "dp-frames-prev");
let fp = await frameParams();
if (JSON.stringify(fp) !== JSON.stringify([["frame.weekly", "Week 2"]])) fail(`after two steps back: ${JSON.stringify(fp)}`);
else ok("stepping a named map writes frame.weekly=Week 2");

await step("Unnamed", "dp-frames-prev");
fp = await frameParams();
if (fp.length !== 1) fail(`an unnamed map wrote a param: ${JSON.stringify(fp)}`);
else ok("an unnamed map never writes one");

// reload the shared link: the named map opens on week 2, the other on its last
const shared = await page.evaluate(() => location.href);
await open(shared);
const p1 = await pos("Named");
const p2 = await pos("Unnamed");
if (!/Week 2/.test(p1) && !/2 of 4/.test(p1)) fail(`reloaded named map shows "${p1}"`);
else ok(`reloaded link opens the named map on "${p1}"`);
if (!/4 of 4|Week 4/.test(p2)) fail(`reloaded unnamed map shows "${p2}"`);

// back to the last frame: the param goes away
await step("Named", "dp-frames-next");
await step("Named", "dp-frames-next");
if ((await frameParams()).length) fail(`back on the last frame, still ${JSON.stringify(await frameParams())}`);
else ok("the param goes away on the last frame");

// an unknown label is ignored (last frame)
const bogus = new URL(start);
bogus.searchParams.set("frame.weekly", "Week 99");
await open(bogus.href);
if (!/4 of 4|Week 4/.test(await pos("Named"))) fail(`an unknown label moved the map: "${await pos("Named")}"`);
else ok("an unknown label is ignored");

// #664 ::SYNC_FRAME: two maps of one group step together; a third (no
// group) stays; Play on one drives the other
const SYNC = `
SELECT 4::COL;
SELECT wkt ::MAP, wk AS "Week" ::FRAME, v AS "On time" ::BARCHART, region ::LABEL, 'weeks'::SYNC_FRAME, 'A'::TITLE
FROM (VALUES
    ${rows(10)}
) t(wkt, region, wk, v) ORDER BY wk, region;
SELECT 4::COL;
SELECT wkt ::MAP, wk AS "Week" ::FRAME, v AS "Late" ::BARCHART, region ::LABEL, 'weeks'::SYNC_FRAME, 'B'::TITLE
FROM (VALUES
    ${rows(30)}
) t(wkt, region, wk, v) ORDER BY wk, region;
SELECT 4::COL;
SELECT wkt ::MAP, wk AS "Week" ::FRAME, v AS "Other" ::BARCHART, region ::LABEL, 'C'::TITLE
FROM (VALUES
    ${rows(50)}
) t(wkt, region, wk, v) ORDER BY wk, region;
`;
await page.goto("about:blank");
await page.goto(`${base}?embed=1#sql=${b64(SYNC)}`);
await page.waitForFunction(() => document.querySelectorAll(".dash .dp-frames").length >= 3 && !document.body.classList.contains("loading"), null, { timeout: 120000 });
await page.waitForTimeout(500);
await step("A", "dp-frames-prev");
let [a, b, c] = [await pos("A"), await pos("B"), await pos("C")];
if (!/3 of 4/.test(a) || !/3 of 4/.test(b)) fail(`::SYNC_FRAME: A "${a}", B "${b}" (both should be 3 of 4)`);
else ok("a step of one synced map moves the other");
if (!/4 of 4/.test(c)) fail(`::SYNC_FRAME: the unsynced map moved: "${c}"`);
else ok("a map outside the group stays");
// the drawn frame follows too, not just the label: B's svg shows frame 2
const bFrame = await page.evaluate(() => {
  const panel = [...document.querySelectorAll(".dash .panel")].find((p) => p.querySelector(".panel-title")?.textContent.trim() === "B");
  return panel?.querySelector(":scope > .panel-svg > svg")?.querySelector("[data-value]")?.getAttribute("data-value");
});
if (bFrame !== String(30 + 3)) fail(`::SYNC_FRAME: B draws value ${bFrame}, expected 33 (Week 3, North)`);
else ok("the follower draws the same frame");
await step("B", "dp-frames-play");
await page.waitForTimeout(1300);
[a, b] = [await pos("A"), await pos("B")];
if (a.replace(/ ·.*/, "") !== b.replace(/ ·.*/, "")) fail(`::SYNC_FRAME Play: A "${a}" vs B "${b}"`);
else ok(`Play on one map drives the other (${a})`);

if (errs.length) fail("page errors: " + errs.join(" | "));
await browser.close();
console.log(`frame-url-e2e: ${process.exitCode ? "FAILED" : "0 failed"}`);
