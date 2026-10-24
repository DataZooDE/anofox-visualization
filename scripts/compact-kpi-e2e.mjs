// Compact KPI tiles (::KPI_SIZE 'compact', #405) in a real browser: a
// ::GROUP's 'compact'::KPI_SIZE packs every tile in the box onto one line
// (label, value, change) at well under the normal ~96 px floor — a tile's
// own ::KPI_SIZE (either value) overrides the box's. ::THRESHOLDS colouring
// still applies; ::SPARKLINE / ::GOAL are dropped (no room). Also checks the
// lower ::FIT floor (#400) a compact tile gets.
//
//   cargo build --lib --target wasm32-unknown-unknown --profile wasm --no-default-features --features wasm
//   wasm-bindgen --target web --out-dir web/pkg --out-name anofox_visualization <target>/wasm32-unknown-unknown/wasm/anofox_visualization.wasm
//   python3 serve_nocache.py 8797 &
//   NODE_PATH=<dir with playwright>/node_modules node scripts/compact-kpi-e2e.mjs [--url http://127.0.0.1:8797/] [--shots <dir>]
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

// The issue's repro (#405 comment): a 4-tile status strip, ::THRESHOLDS on
// two of them (one 'warn', one 'bad', so both non-good statuses are
// exercised) — plus two single-tile boxes that isolate the override rule
// (a tile's own ::KPI_SIZE, either value, wins over its box's) from the
// *shared-row* "sized to the tallest tile" behaviour a mismatched compact
// tile would otherwise show when it shares a row with a normal one (by
// design: KPIs in a box always share one row height).
const SQL = `SELECT 'Status'::GROUP, 'compact'::KPI_SIZE;
SELECT 42::METRIC, 'Latest value'::LABEL;
SELECT 7::METRIC, 'Alerts'::LABEL, '<1:good; <4:warn; bad'::THRESHOLDS;
SELECT 2::METRIC, 'Drifting inputs'::LABEL, '<1:good; <3:warn; bad'::THRESHOLDS;
SELECT 91::PERCENT, 'Coverage'::LABEL;
SELECT 1::ENDGROUP;
SELECT 'OwnCompact'::GROUP;
SELECT 55::METRIC, 'Own compact'::LABEL, 'compact'::KPI_SIZE;
SELECT 1::ENDGROUP;
SELECT 'OwnNormal'::GROUP, 'compact'::KPI_SIZE;
SELECT 20::METRIC, 'Overridden'::LABEL, 'normal'::KPI_SIZE;
SELECT 1::ENDGROUP;
`;

// A separate ::FIT scope: under fit-to-screen, a row shares the window with
// the other rows in the *ratio* of its weight (::HEIGHT, else the default —
// a KPI row's weight is its own minimum), so there's usually slack to grow
// into. At 1366×700 with one dominant 400::HEIGHT chart, there is exactly
// enough slack to pin the KPI row at its own floor and no more — whether
// the box carries 'compact'::KPI_SIZE decides which floor that is.
const fitSql = (kpiSize) => `SELECT 'fit'::FIT;
SELECT 'Status'::GROUP${kpiSize ? `, 'compact'::KPI_SIZE` : ""};
SELECT 42::METRIC, 'Latest value'::LABEL;
SELECT 7::METRIC, 'Alerts'::LABEL, '<1:good; <4:warn; bad'::THRESHOLDS;
SELECT 2::METRIC, 'Drifting inputs'::LABEL, '<1:good; <3:warn; bad'::THRESHOLDS;
SELECT 91::PERCENT, 'Coverage'::LABEL;
SELECT 1::ENDGROUP;
SELECT 400::HEIGHT;
SELECT region::XAXIS, amount::BARCHART FROM (VALUES ('North', 10), ('South', 5), ('East', 2), ('West', 4)) t(region, amount);
`;

const browser = await chromium.launch();
const settled = (page) =>
  page.waitForFunction(
    () => /\d+ panels?/.test(document.getElementById("status")?.textContent || "") && !document.body.classList.contains("loading"),
    null,
    { timeout: 120000 }
  );
async function open(viewport, theme = "", query = "", sql = SQL) {
  const page = await browser.newPage({ viewport, reducedMotion: "reduce", colorScheme: theme || "light" });
  const errs = [];
  page.on("pageerror", (e) => errs.push(String(e)));
  await page.goto(`${base}${query}#sql=${b64(sql)}`);
  await settled(page);
  await page.waitForTimeout(300);
  return { page, errs };
}

// Every ::GROUP box, its tiles and their measured geometry / classes.
const measure = (page) =>
  page.evaluate(() => {
    const groups = [...document.querySelectorAll(".dash .group")].map((box) => {
      const title = box.querySelector(".group-title")?.textContent || "";
      const tiles = [...box.querySelectorAll(".metric")].map((m) => {
        const r = m.getBoundingClientRect();
        const val = m.querySelector(".metric-value");
        return {
          top: r.top,
          bottom: r.bottom,
          h: r.height,
          compact: m.classList.contains("metric-size-compact"),
          normal: m.classList.contains("metric-size-normal"),
          cap: m.querySelector(".metric-cap")?.textContent || "",
          value: val?.textContent || "",
          status: [...(val?.classList || [])].find((c) => c.startsWith("th-")) || "",
          hasSpark: !!m.querySelector("svg.metric-spark"),
          hasGoal: !!m.querySelector(".metric-goal"),
        };
      });
      return { title, compactBox: box.classList.contains("group-compact"), tiles };
    });
    return { groups, scroll: document.documentElement.scrollHeight > window.innerHeight + 1, state: document.body.dataset.dpFit || "" };
  });

{
  const { page, errs } = await open({ width: 1920, height: 1080 });
  const m = await measure(page);
  const status = m.groups.find((g) => g.title === "Status");
  check(!!status && status.compactBox, `the 'Status' box is marked compact (${status && status.compactBox})`);
  check(status?.tiles.length === 4, `4 tiles in the 'Status' box (${status?.tiles.length})`);
  const rowH = status ? Math.max(...status.tiles.map((t) => t.bottom)) - Math.min(...status.tiles.map((t) => t.top)) : Infinity;
  check(rowH <= 40.5, `the compact strip's row is <= 40px tall (measured ${rowH.toFixed(1)}px)`);
  check(status?.tiles.every((t) => t.h <= 40.5), `every compact tile is <= 40px tall (${status?.tiles.map((t) => t.h.toFixed(1))})`);
  check(status?.tiles.every((t) => !t.hasSpark && !t.hasGoal), "compact tiles drop ::SPARKLINE / ::GOAL");
  // ::THRESHOLDS still colours the value: 'Alerts' (7, rule '<1:good;<4:warn;bad') -> bad;
  // 'Drifting inputs' (2, rule '<1:good;<3:warn;bad') -> warn
  const alerts = status?.tiles.find((t) => t.cap === "Alerts");
  const drifting = status?.tiles.find((t) => t.cap === "Drifting inputs");
  check(alerts?.status === "th-bad", `::THRESHOLDS colours 'Alerts' bad in a compact tile (${alerts?.status})`);
  check(drifting?.status === "th-warn", `::THRESHOLDS colours 'Drifting inputs' warn in a compact tile (${drifting?.status})`);
  // label, value and (for the thresholded tiles) nothing else on the one line
  check(status?.tiles.every((t) => t.cap && t.value), "every compact tile still shows its label and value");

  // 'OwnCompact': a box that is NOT itself compact, with one tile that sets
  // its own 'compact'::KPI_SIZE — alone in the box, so the row-sharing rule
  // ("sized to the tallest tile") can't mask the override either way.
  const ownCompactBox = m.groups.find((g) => g.title === "OwnCompact");
  check(!!ownCompactBox && !ownCompactBox.compactBox, "the 'OwnCompact' box is not itself compact");
  const ownCompact = ownCompactBox?.tiles.find((t) => t.cap === "Own compact");
  check(ownCompact?.compact === true && ownCompact.h <= 40.5, `a tile's own 'compact'::KPI_SIZE applies even in a normal box (compact=${ownCompact?.compact}, h=${ownCompact?.h.toFixed(1)})`);

  // 'OwnNormal': a compact box with its one tile overriding back to 'normal'.
  const ownNormalBox = m.groups.find((g) => g.title === "OwnNormal");
  check(!!ownNormalBox && ownNormalBox.compactBox, "the 'OwnNormal' box is itself compact");
  const overridden = ownNormalBox?.tiles.find((t) => t.cap === "Overridden");
  check(overridden?.normal === true && overridden.h > 45, `a tile's own 'normal'::KPI_SIZE wins over its compact box (normal=${overridden?.normal}, h=${overridden?.h.toFixed(1)})`);

  check(!errs.length, `no page errors: ${errs.join(" | ")}`);
  if (shots) {
    await page.screenshot({ path: `${shots}/compact-kpi-light-1920.png` });
    console.log(`shot ${shots}/compact-kpi-light-1920.png`);
  }
  await page.close();
}

// ::FIT (#400): at the same viewport and layout, a compact KPI row pins at
// its own ~36px floor (MIN_KPI_COMPACT) where a normal one pins at the
// usual ~96px one (MIN_KPI) — both still fit (no fallback to scrolling).
for (const [tag, kpiSize, want] of [
  ["compact", true, 36],
  ["normal", false, 96],
]) {
  const { page, errs } = await open({ width: 1366, height: 700 }, "", "", fitSql(kpiSize));
  const m = await page.evaluate(() => ({
    state: document.body.dataset.dpFit,
    scroll: document.documentElement.scrollHeight > window.innerHeight + 1,
    heights: [...document.querySelectorAll(".dash .group .metric")].map((p) => p.getBoundingClientRect().height),
  }));
  check(m.state === "on" && !m.scroll, `(fit ${tag}) the tab fits, no scroll (${m.state}, scroll ${m.scroll})`);
  check(m.heights.length === 4 && m.heights.every((h) => Math.abs(h - want) <= 0.5), `(fit ${tag}) the KPI row pins at its own floor (~${want}px): ${m.heights.map((h) => h.toFixed(1))}`);
  check(!errs.length, `(fit ${tag}) no page errors: ${errs.join(" | ")}`);
  await page.close();
}

// Dark theme + a phone width, for a visual check (not asserted — screenshots only).
if (shots) {
  for (const [theme, vp, tag] of [
    ["dark", { width: 1920, height: 1080 }, "dark-1920"],
    ["light", { width: 390, height: 844 }, "light-390"],
  ]) {
    const { page } = await open(vp, theme);
    await page.screenshot({ path: `${shots}/compact-kpi-${tag}.png` });
    console.log(`shot ${shots}/compact-kpi-${tag}.png`);
    await page.close();
  }
}

await browser.close();
console.log(`compact-kpi-e2e: ${bad} failed`);
if (bad) process.exitCode = 1;
