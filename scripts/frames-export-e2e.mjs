// Browser e2e for the ::FRAME export (M2f-B, #149): downloadHtml() embeds
// every frame of a time map (within the 3 MB/map budget, plan §M2f-B) as
// sibling <template data-frame="k"> elements next to its .panel-svg, and
// the exported file's own snapshotMotion() wires a swap-only player
// (anim.js's frames()/tween(), app.js's wireHits() re-run on the fresh
// marks) — so the export's time map still plays offline, not just a
// "N frames — Play in the live dashboard" note (M2f-A's own fallback,
// kept here for a map over budget).
//
//   cargo build --lib --target wasm32-unknown-unknown --profile wasm --no-default-features --features wasm
//   wasm-bindgen --target web --out-dir web/pkg <target>/wasm32-unknown-unknown/wasm/anofox_visualization.wasm
//   wasm-opt -O web/pkg/anofox_visualization_bg.wasm -o web/pkg/anofox_visualization_bg.wasm
//   python3 serve_nocache.py 8793 &
//   NODE_PATH=<dir with playwright>/node_modules node scripts/frames-export-e2e.mjs \
//       [--url http://127.0.0.1:8793/]
import { createRequire } from "module";
import { readFileSync } from "fs";
import path from "path";

const require = createRequire(import.meta.url);
const { chromium } = require("playwright");
const arg = (k, d) => {
  const i = process.argv.indexOf(k);
  return i > 0 ? process.argv[i + 1] : d;
};
const base = arg("--url", "http://127.0.0.1:8793/");
const fail = (m) => {
  console.error("FAIL: " + m);
  process.exitCode = 1;
};
const ok = (m) => console.log("ok: " + m);
const b64 = (s) => Buffer.from(s, "utf8").toString("base64");
function check(c, m) {
  if (c) ok(m);
  else fail(m);
}

const sqlPath = path.join(path.dirname(new URL(import.meta.url).pathname), "..", "web", "examples", "delivery-coverage.sql");
const SQL = readFileSync(sqlPath, "utf8");

async function openMap(page) {
  await page.locator(".tabbar button.tab-btn", { hasText: "On time" }).click();
  await page.waitForTimeout(150);
  await page.locator('.dp-viewbar [role="tab"]', { hasText: "Week by week" }).click();
  await page.waitForTimeout(150);
  await page.waitForFunction(() => !!document.querySelector(".dash .panel-svg > svg[data-frames]"), null, { timeout: 60000 });
}

// Trigger the real downloadHtml() via #dlhtml (hidden by ?embed=1, needed so
// the dashboard auto-runs from the #sql hash — clicked programmatically
// rather than requiring it visible) and return the exported file's text.
async function exportHtml(page) {
  const [dl] = await Promise.all([page.waitForEvent("download"), page.evaluate(() => document.getElementById("dlhtml").click())]);
  return readFileSync(await dl.path(), "utf8");
}

// Open an export cold, offline, with no server, no live app.js and no wasm.
async function openOffline(browser, html, viewport = { width: 1200, height: 900 }, colorScheme) {
  const off = await browser.newContext({ offline: true, viewport, colorScheme });
  const op = await off.newPage();
  const errs = [];
  op.on("pageerror", (e) => errs.push(String(e)));
  await op.route("**/*", (r) => (r.request().url().startsWith("http") ? (r.abort(), undefined) : r.continue()));
  await op.setContent(html, { waitUntil: "load" });
  return { op, errs };
}

async function run() {
  const browser = await chromium.launch();
  const page = await browser.newPage({ viewport: { width: 1200, height: 900 }, acceptDownloads: true });
  const liveErrs = [];
  page.on("pageerror", (e) => liveErrs.push(String(e)));
  await page.goto(`${base}?embed=1#sql=${b64(SQL)}`);
  await page.waitForFunction(() => !document.body.classList.contains("loading"), null, { timeout: 120000 });
  await openMap(page);
  check(!liveErrs.length, "no page errors building the live dashboard " + liveErrs.join(" | "));

  const subtitleOf = (p) =>
    p.evaluate(() => {
      const svg = document.querySelector(".dash svg[data-frames]");
      const name = svg?.getAttribute("data-frame-name") || "";
      return [...(svg?.querySelectorAll("text") || [])].find((t) => t.textContent.startsWith(name + ":"))?.textContent || "";
    });
  const liveLabel = await subtitleOf(page);
  ok(`live: subtitle reads "${liveLabel}" (last frame, D12)`);

  const html = await exportHtml(page);

  // 15 states × 12 weeks is well under the 3 MB budget: every frame must be
  // embedded as a <template>, no fallback note for this panel.
  const tplCount = (html.match(/<template data-frame="\d+">/g) || []).length;
  check(tplCount === 12, `export embeds 12 <template data-frame> frames (found ${tplCount})`);
  check(!/<p class="dp-frames-note">/.test(html), "export has no fallback note when the map is within budget");
  check(!/<div class="dp-frames"/.test(html), "the export's markup carries no live bar of its own (built offline, by snapshotMotion())");

  const { op, errs: exportErrs } = await openOffline(browser, html);

  const bar = await op.$(".dash .dp-frames");
  check(!!bar, "export: the offline player bar built itself (snapshotMotion()'s K.frames)");
  const pos0 = await op.locator(".dp-frames-pos").textContent();
  check(/12 of 12/.test(pos0 || ""), `export: starts on the last frame ("${pos0}")`);

  const fillBefore = await op.evaluate(() => document.querySelector(".dash svg[data-frames] path[data-x]")?.getAttribute("fill"));
  const titleBefore = await subtitleOf(op);
  await op.click(".dp-frames-prev");
  await op.waitForTimeout(400); // the step's own swap + tween
  const pos1 = await op.locator(".dp-frames-pos").textContent();
  check(/11 of 12/.test(pos1 || ""), `export: ‹ steps back offline (position "${pos1}")`);
  const titleAfter = await subtitleOf(op);
  check(titleBefore !== titleAfter, `export: the subtitle changed on step (before "${titleBefore}", after "${titleAfter}")`);
  const fillAfter = await op.evaluate(() => document.querySelector(".dash svg[data-frames] path[data-x]")?.getAttribute("fill"));
  check(fillBefore !== fillAfter || titleBefore !== titleAfter, "export: the swap actually replaced the map's own markup, not a no-op");

  // The legend's class boundaries (D21) must still be the shared ones
  // baked into every embedded frame — comparing week 12's (first loaded)
  // against week 11's (just swapped in) must show the same breaks.
  const classesOf = async () =>
    op.evaluate(() => [...document.querySelectorAll(".dash svg[data-frames] text")].map((t) => t.textContent).filter((t) => /–|—|-/.test(t) && /\(\d+\)/.test(t)).map((t) => t.replace(/\(\d+\)/, "")));
  const classesAfter = await classesOf();

  // Hover still works on the swapped-in marks (wireHits() re-run per step,
  // app.js) — a .dp-hit element on the fresh svg carries a working tooltip.
  // review of #602 S1: wireHits() used to only convert path[data-row]
  // titles (areas) — this map is a choropleth, so it alone wouldn't catch
  // a regression there (see runPointMap() below for the circle-mark case).
  const hoverWorks = await op.evaluate(() => {
    const mark = document.querySelector(".dash svg[data-frames] .dp-hit");
    if (!mark) return null;
    mark.dispatchEvent(new MouseEvent("mouseenter"));
    const shown = document.getElementById("dp-tip")?.classList.contains("show");
    mark.dispatchEvent(new MouseEvent("mouseleave"));
    return shown;
  });
  check(hoverWorks === true, `export: hover re-wires on the swapped-in marks (dp-tip shown: ${hoverWorks})`);

  // Slider to frame 0, Play/Pause, same shape as the live player (anim.js's
  // frames() itself — this just confirms it still runs with no wasm).
  const range = await op.$(".dp-frames input[type=range]");
  await range.fill("0");
  await range.dispatchEvent("input");
  await op.waitForTimeout(400);
  const pos2 = await op.locator(".dp-frames-pos").textContent();
  check(/1 of 12/.test(pos2 || ""), `export: slider → 0 (position "${pos2}")`);
  const classes0 = await classesOf();
  check(JSON.stringify(classes0) === JSON.stringify(classesAfter), "export: legend class boundaries unchanged between embedded frames (D21)");

  await op.click(".dp-frames-play");
  await op.waitForTimeout(2200);
  const posPlaying = await op.locator(".dp-frames-pos").textContent();
  check(posPlaying !== pos2, "export: Play advances offline too");
  await op.click(".dp-frames-play"); // Pause before closing

  check(!exportErrs.length, "export: no page errors stepping through the offline player " + exportErrs.join(" | "));
  await browser.close();
}

// review of #602 S3: a point/symbol map's own marks are <circle>, not
// <path> — wireHits()'s old selector (svg[data-map] path[data-row] title)
// never matched them, so hover silently broke after the first offline
// step. This is the one existing scenario (above) couldn't catch, since
// delivery-coverage's "Week by week" map is a choropleth (path marks).
async function runPointMap() {
  const SQL = `SELECT lat::LAT, lon::LON, week AS "Week" ::FRAME, v ::BARCHART
FROM (SELECT 48.0 + 0.05 * i AS lat, 9.0 + 0.05 * i AS lon, (1 + i % 3)::VARCHAR AS week, i AS v FROM range(15) t(i));`;
  const browser = await chromium.launch();
  const page = await browser.newPage({ viewport: { width: 900, height: 700 }, acceptDownloads: true });
  await page.goto(`${base}?embed=1#sql=${b64(SQL)}`);
  await page.waitForFunction(() => !document.body.classList.contains("loading"), null, { timeout: 120000 });
  await page.waitForFunction(() => !!document.querySelector(".dash .panel-svg > svg[data-frames]"), null, { timeout: 60000 });
  const html = await exportHtml(page);
  const { op, errs } = await openOffline(browser, html);
  await op.click(".dp-frames-prev"); // step away from the last (server-rendered) frame
  await op.waitForTimeout(400);
  const mark = await op.$(".dash svg[data-frames] circle[data-row]");
  check(!!mark, "point map export: the swapped-in frame still draws circle marks");
  // A circle with no value this frame (NA) correctly gets no tooltip at
  // all (attachHover's own live rule) — pick one that actually carries a
  // value (.dp-hit, the class wireHits() adds), same as the choropleth
  // case above, not just "the first circle[data-row]".
  const hoverWorks = await op.evaluate(() => {
    const el = document.querySelector(".dash svg[data-frames] circle.dp-hit[data-row]");
    if (!el) return "not-wired";
    el.dispatchEvent(new MouseEvent("mouseenter"));
    const shown = document.getElementById("dp-tip")?.classList.contains("show");
    el.dispatchEvent(new MouseEvent("mouseleave"));
    return shown;
  });
  check(hoverWorks === true, `point map export: hover works on a swapped-in circle mark after a step (${hoverWorks})`);
  check(!errs.length, "point map export: no page errors " + errs.join(" | "));
  await browser.close();
}

// review of #602 S3: a ::FIT export's scaleSvg() (SNAPSHOT_JS) sizes the
// map's svg to the fitted panel height on load; stepping must not drop
// back to the authored (unfitted) height (S2's own fix).
async function runFit() {
  const SQL = `SELECT 'Main'::TAB;
SELECT 'fit'::FIT;
SELECT 900::HEIGHT;
SELECT lat::LAT, lon::LON, week AS "Week" ::FRAME, v ::BARCHART
FROM (SELECT 48.0 + 0.05 * i AS lat, 9.0 + 0.05 * i AS lon, (1 + i % 3)::VARCHAR AS week, i AS v FROM range(15) t(i));`;
  const browser = await chromium.launch();
  // A viewport short enough, against the authored 900px panel height, that
  // fitDash() actually has to shrink it (otherwise scaleSvg() has nothing
  // to do and this test would pass vacuously).
  const page = await browser.newPage({ viewport: { width: 900, height: 560 }, acceptDownloads: true });
  await page.goto(`${base}?embed=1#sql=${b64(SQL)}`);
  await page.waitForFunction(() => !document.body.classList.contains("loading"), null, { timeout: 120000 });
  await page.waitForFunction(() => !!document.querySelector(".dash .panel-svg > svg[data-frames]"), null, { timeout: 60000 });
  const html = await exportHtml(page);
  const { op, errs } = await openOffline(browser, html, { width: 900, height: 560 });
  const before = await op.evaluate(() => {
    const s = document.querySelector(".dash svg[data-frames]");
    return s && { h: s.getAttribute("height"), scaled: s.hasAttribute("data-dp-scaled") };
  });
  check(!!before && before.scaled, `::FIT export: the map loads already scaled (${JSON.stringify(before)})`);
  await op.click(".dp-frames-prev");
  await op.waitForTimeout(400);
  const after = await op.evaluate(() => {
    const s = document.querySelector(".dash svg[data-frames]");
    return s && { h: s.getAttribute("height"), scaled: s.hasAttribute("data-dp-scaled") };
  });
  check(
    !!after && after.scaled && after.h === before.h,
    `::FIT export: the scaled height survives a step (before ${JSON.stringify(before)}, after ${JSON.stringify(after)})`
  );
  check(!errs.length, "::FIT export: no page errors " + errs.join(" | "));
  await browser.close();
}

// A dark export's embedded frames must stay dark after a step — frames are
// pre-rendered through ctxWith({frame:k}), which already carries the
// current theme variant (renderCtx()); this just confirms the swap doesn't
// somehow lose it.
async function runDark() {
  const browser = await chromium.launch();
  const page = await browser.newPage({ viewport: { width: 1200, height: 900 }, acceptDownloads: true, colorScheme: "dark" });
  await page.goto(`${base}?embed=1&theme=dark#sql=${b64(SQL)}`);
  await page.waitForFunction(() => !document.body.classList.contains("loading"), null, { timeout: 120000 });
  await openMap(page);
  const html = await exportHtml(page);
  check(/data-ax-variant="dark"/.test(html), "dark export: the exported page itself is marked dark");
  const { op, errs } = await openOffline(browser, html, { width: 1200, height: 900 }, "dark");
  const bg = (p) => p.evaluate(() => document.querySelector(".dash svg[data-frames] rect")?.getAttribute("fill"));
  const before = await bg(op);
  await op.click(".dp-frames-prev");
  await op.waitForTimeout(400);
  const after = await bg(op);
  check(!!before && before === after, `dark export: the panel background stays the same dark fill across a step (before ${before}, after ${after})`);
  check(!errs.length, "dark export: no page errors " + errs.join(" | "));
  await browser.close();
}

// Stepping the live map before exporting must carry that frame into the
// export (the server-rendered svg's own data-frame, D12's "else the last").
async function runNonLastStart() {
  const browser = await chromium.launch();
  const page = await browser.newPage({ viewport: { width: 1200, height: 900 }, acceptDownloads: true });
  await page.goto(`${base}?embed=1#sql=${b64(SQL)}`);
  await page.waitForFunction(() => !document.body.classList.contains("loading"), null, { timeout: 120000 });
  await openMap(page);
  await page.click(".dp-frames-prev");
  await page.click(".dp-frames-prev");
  await page.waitForTimeout(400);
  const livePos = await page.locator(".dp-frames-pos").textContent();
  check(/10 of 12/.test(livePos || ""), `live: stepped back twice to frame 10 ("${livePos}")`);
  const html = await exportHtml(page);
  const { op, errs } = await openOffline(browser, html);
  const exportPos = await op.locator(".dp-frames-pos").textContent();
  check(/10 of 12/.test(exportPos || ""), `export: opens on the frame the live dashboard was stepped to, not always the last ("${exportPos}")`);
  check(!errs.length, "non-last-start export: no page errors " + errs.join(" | "));
  await browser.close();
}

// The plan's own "a synthetic 52 x 401 map falls back to the note" case
// (§M2f-B Tests): a map whose embedded frames would exceed the 3 MB/map
// budget gets no templates at all, just M2f-A's static note.
async function runOverBudget() {
  const SQL = `SELECT lat::LAT, lon::LON, week AS "Week" ::FRAME, v ::BARCHART
FROM (SELECT 48.0 + (i % 200) * 0.005 AS lat, 9.0 + ((i / 200) % 10) * 0.02 AS lon, (1 + i % 40)::VARCHAR AS week, i AS v FROM range(80000) t(i));`;
  const browser = await chromium.launch();
  const page = await browser.newPage({ viewport: { width: 1200, height: 900 }, acceptDownloads: true });
  await page.goto(`${base}?embed=1#sql=${b64(SQL)}`);
  await page.waitForFunction(() => !document.body.classList.contains("loading"), null, { timeout: 180000 });
  await page.waitForFunction(() => !!document.querySelector(".dash .panel-svg > svg[data-frames]"), null, { timeout: 60000 });
  const t0 = Date.now();
  const html = await exportHtml(page);
  const ms = Date.now() - t0;
  const tplCount = (html.match(/<template data-frame="\d+">/g) || []).length;
  check(tplCount === 0, `over-budget export: no frames embedded (found ${tplCount})`);
  check(/<p class="dp-frames-note">40 frames — Play in the live dashboard<\/p>/.test(html), "over-budget export: the static note is present");
  check(!/<div class="dp-frames"/.test(html), "over-budget export: no live bar markup either");
  ok(`over-budget export: took ${ms} ms (budget check stops the render loop early, not a full 40 frames)`);
  await browser.close();
}

await run();
await runPointMap();
await runFit();
await runDark();
await runNonLastStart();
await runOverBudget();
if (!process.exitCode) console.log("frames-export-e2e: ok");
