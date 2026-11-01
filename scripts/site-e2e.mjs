// A4 (PR #429 review, #417): a published static site (`dashboard --site`),
// in a real browser, served under a GitHub-Pages-style sub-path (/repo/) by
// a plain static file server — no locked server, no builder, nothing but
// files on disk. Builds a small 3-dashboard fixture (a setup view,
// path_vars, a nav_picker, real parquet data), runs it through `dashboard
// --site` with the real `--app` web build, then checks:
//   - the nav + the nav_picker render;
//   - a nav-link click switches dashboards in place (a window marker set
//     before the click survives it — proof there was no full reload);
//   - a hard reload of /d/<id>/ and back/forward both work;
//   - the parquet data actually reads (a real rendered value from it);
//   - a `../x` path_vars value is dropped (#417 S2), not applied;
//   - there is no editor flash (the sidebar is never visible, even in the
//     DOM's own computed layout, not just by class name);
//   - #500: a table ::LINK and a chart-mark ::LINK, each written as
//     `/d/<id>?v.x={…}` (same as a locked server's own route), switch
//     dashboards in place under /repo/, never navigating to the domain
//     root, and pass the clicked value on; the served-nav and ::LINK hrefs
//     themselves resolve under /repo/ too (middle-click / new tab);
//     (the locked-server counterpart — those same hrefs staying plain
//     `/d/<id>` when `published` is false — is unit-tested directly at
//     the function level, not re-proven here; see tests/web/
//     site_href_test.mjs and nav_path_vars_test.mjs);
//   - no request leaves this site's own origin / the pinned CDN hosts
//     (the telemetry-e2e.mjs pattern, #334) — the exact list site_csp()
//     (src/bin/dashboard.rs) allows;
//   - axe (if --axe is given).
// Kept to one small fixture and one page load per check group — under 2
// minutes end to end on CI.
//
//   cargo build --bin dashboard
//   NODE_PATH=<dir with playwright>/node_modules node scripts/site-e2e.mjs \
//       [--dashboard target/debug/dashboard] [--duckdb duckdb] [--app web] \
//       [--port 8931] [--axe axe.min.js] [--shots DIR]
process.env.DATAZOO_DISABLE_TELEMETRY = "1"; // no usage telemetry from anofox extensions
import { createRequire } from "module";
import { execFileSync } from "child_process";
import { createServer } from "http";
import { existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "fs";
import { tmpdir } from "os";
import path from "path";
import { fileURLToPath } from "url";

const require = createRequire(import.meta.url);
let chromium;
try {
  ({ chromium } = require("playwright"));
} catch (e) {
  console.error("Playwright is not installed (NODE_PATH=<dir>/node_modules node " + path.basename(import.meta.url));
  process.exit(1);
}

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const arg = (k, d) => {
  const i = process.argv.indexOf(k);
  return i > 0 ? process.argv[i + 1] : d;
};
const DASH = arg("--dashboard", path.join(root, "target/debug/dashboard"));
const DUCKDB = arg("--duckdb", "duckdb");
const APP = arg("--app", path.join(root, "web"));
const PORT = Number(arg("--port", "8931"));
const axe = arg("--axe", "");
const shots = arg("--shots", "");
for (const [what, p] of [
  ["the dashboard CLI (cargo build --bin dashboard)", DASH],
  ["--app", APP],
]) {
  if (!existsSync(p)) {
    console.error(`site-e2e: ${what} missing: ${p}`);
    process.exit(2);
  }
}
if (shots) mkdirSync(shots, { recursive: true });

const fail = (m) => {
  console.error("FAIL: " + m);
  process.exitCode = 1;
};
const check = (ok, m) => (ok ? console.log("ok   " + m) : fail(m));
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// ---- 1. a small fixture: 2 parquet files + 3 dashboards + options ----
const work = mkdtempSync(path.join(tmpdir(), "dp-site-e2e-"));
const dataDir = path.join(work, "data");
const dashDir = path.join(work, "dash");
const outDir = path.join(work, "out");
mkdirSync(dataDir, { recursive: true });
mkdirSync(dashDir, { recursive: true });

execFileSync(
  DUCKDB,
  [
    ":memory:",
    "-c",
    `COPY (SELECT * FROM (VALUES ('north','widget',120),('north','gadget',80),('south','widget',95),
      ('south','gadget',150),('east','widget',60),('east','gadget',40)) t(region,product,units))
      TO '${path.join(dataDir, "sales.parquet")}' (FORMAT parquet);
     COPY (SELECT * FROM (VALUES ('north','North'),('south','South'),('east','East')) t(id,label))
      TO '${path.join(dataDir, "regions.parquet")}' (FORMAT parquet);`,
  ],
  { stdio: ["ignore", "pipe", "pipe"] }
);

writeFileSync(
  path.join(dashDir, "overview.sql"),
  `-- @title Sales overview\n-- @nav Overview\n-- @order 1\n` +
    `CREATE OR REPLACE TEMP VIEW sales AS SELECT * FROM read_parquet(getvariable('anofox_data') || '/sales.parquet')\n` +
    `  WHERE region = coalesce(getvariable('region'), region);\n` +
    `SELECT product::XAXIS, sum(units)::BARCHART, 'Units by product'::TITLE FROM sales GROUP BY 1 ORDER BY 1;\n` +
    // #500: a chart-mark ::LINK and a table ::LINK, each written as
    // '/d/<id>?v.x={…}' against the domain root, same as a locked server's
    // own route — drill through to by-product, pre-filtered by the
    // clicked region.
    `SELECT region::XAXIS, sum(units)::BARCHART, 'Units by region'::TITLE,\n` +
    `       '/d/by-product?v.region={x}'::LINK FROM sales GROUP BY 1 ORDER BY 1;\n` +
    `SELECT region::TABLE, sum(units) AS units,\n` +
    `       '/d/by-product?v.region={region}' AS detail ::LINK FROM sales GROUP BY 1 ORDER BY 1;\n`
);
writeFileSync(
  path.join(dashDir, "by-product.sql"),
  `-- @title Units by product and region\n-- @nav By product\n-- @order 2\n` +
    `CREATE OR REPLACE TEMP VIEW sales AS SELECT * FROM read_parquet(getvariable('anofox_data') || '/sales.parquet')\n` +
    `  WHERE region = coalesce(getvariable('region'), region);\n` +
    `SELECT product::TABLE, region, units FROM sales ORDER BY 1, 2;\n`
);
writeFileSync(
  path.join(dashDir, "trend.sql"),
  `-- @title Trend\n-- @nav Trend\n-- @order 3\n` +
    // #830: -- @requires resolves against anofox_data, now a local name:
    // a listed file shows its tab, a missing one hides it
    `-- @requires sales.parquet\nSELECT 'Has sales'::TAB;\n` +
    `SELECT 'nothing here but text — proves nav switches between differently-shaped dashboards'::MARKDOWN;\n` +
    // #655: a ::REGION map on the built-in boundaries, which --site must publish.
    `SELECT iso ::REGION, v AS "Units" ::BARCHART, 'world'::BOUNDARIES, 'Units by country'::TITLE\n` +
    `  FROM (VALUES ('DEU', 1.0), ('FRA', 2.0), ('BRA', 3.0)) t(iso, v);\n` +
    `-- @requires nosuch.parquet\nSELECT 'Never shown'::TAB;\nSELECT 'hidden'::MARKDOWN;\n` +
    `SELECT 'Last'::TAB;\nSELECT 'last tab'::MARKDOWN;\n`
);
const optionsFile = path.join(work, "locked.json");
writeFileSync(
  optionsFile,
  JSON.stringify({
    path_vars: ["region"],
    nav_picker: {
      var: "region",
      sql: "SELECT id, label FROM read_parquet(getvariable('anofox_data') || '/regions.parquet') ORDER BY label",
      label: "Region",
    },
    app: { name: "site-e2e" },
    home: "overview",
  })
);

// ---- 2. dashboard --site ----
execFileSync(
  DASH,
  ["--site", dashDir, outDir, "--data", dataDir, "--options", optionsFile, "--app", APP],
  { stdio: ["ignore", "pipe", "pipe"] }
);
check(existsSync(path.join(outDir, "site.json")), "dashboard --site wrote site.json");

// ---- 3. serve outDir under /repo/ (GitHub-Pages-style sub-path) ----
const MIME = {
  ".html": "text/html",
  ".js": "text/javascript",
  ".mjs": "text/javascript",
  ".css": "text/css",
  ".json": "application/json",
  ".wasm": "application/wasm",
  ".sql": "text/plain",
  ".parquet": "application/octet-stream",
};
const server = createServer((req, res) => {
  const u = new URL(req.url, "http://x");
  let rel = decodeURIComponent(u.pathname).replace(/^\/repo\/?/, "");
  if (rel === "" || rel.endsWith("/")) rel += "index.html";
  const p = path.join(outDir, rel);
  if (!p.startsWith(outDir)) {
    res.writeHead(403);
    return res.end();
  }
  try {
    const data = require("fs").readFileSync(p);
    res.writeHead(200, { "Content-Type": MIME[path.extname(p)] || "application/octet-stream", "Cache-Control": "no-store" });
    res.end(data);
  } catch (_) {
    res.writeHead(404);
    res.end("not found");
  }
});
await new Promise((resolve) => server.listen(PORT, "127.0.0.1", resolve));
const BASE = `http://127.0.0.1:${PORT}/repo/`;

const browser = await chromium.launch();
const allRequests = [];
async function openPage(url, { width = 1280 } = {}) {
  const ctx = await browser.newContext({ viewport: { width, height: 900 } });
  const p = await ctx.newPage();
  p.on("request", (r) => allRequests.push(r.url()));
  if (process.env.SITE_E2E_KEEP) p.on("console", (m) => console.log(`[console] ${m.text()}`));
  await p.goto(url, { waitUntil: "networkidle", timeout: 30000 });
  return p;
}
async function axeCheck(p, sel, what) {
  if (!axe || !existsSync(axe)) return;
  await p.addScriptTag({ path: axe });
  const r = await p.evaluate(async (s) => window.axe.run(s, { runOnly: { type: "tag", values: ["wcag2a", "wcag2aa"] } }), sel);
  for (const v of r.violations) console.log(`axe ${what} ${v.impact}: ${v.id} (${v.nodes.length}) ${v.help} — ${v.nodes.map((n) => n.target).join(" ")}`);
  check(!r.violations.some((v) => v.impact === "serious" || v.impact === "critical"), `axe ${what}: no serious violation (${r.passes.length} pass)`);
}

try {
  // ── 1. the nav + nav_picker render, no editor flash ──
  const p = await openPage(BASE);
  await p.waitForSelector(".served-nav a", { timeout: 60000 });
  const navLabels = await p.$$eval(".served-nav a[data-nav-id]", (as) => as.map((a) => a.textContent.trim()));
  check(JSON.stringify(navLabels) === JSON.stringify(["Overview", "By product", "Trend"]), `nav bar: ${navLabels.join(", ")}`);
  const picker = await p.$$eval(".served-nav-picker select option", (os) => os.map((o) => o.value));
  check(JSON.stringify(picker) === JSON.stringify(["east", "north", "south"]), `nav_picker options: ${picker.join(",")}`);
  const bodyClass = await p.evaluate(() => document.body.className);
  check(bodyClass.includes("published") && bodyClass.includes("embed") && bodyClass.includes("mode-view"), `body class: ${bodyClass}`);
  const sideVisible = await p.evaluate(() => {
    const side = document.querySelector(".side");
    if (!side) return false;
    const r = side.getBoundingClientRect();
    return getComputedStyle(side).display !== "none" && r.width > 0 && r.height > 0;
  });
  check(!sideVisible, "no editor flash: the sidebar is never visible (not just via class, via layout)");
  await p.waitForSelector(".dash .panel", { timeout: 60000 });
  await axeCheck(p, "body", "dashboard page");
  if (shots) await p.screenshot({ path: `${shots}/site-overview.png` });

  // ── 2. an in-place nav-link click: a window marker survives it, the URL changes ──
  await p.evaluate(() => (window.__e2eMarker = "still-here"));
  await p.click('.served-nav a[data-nav-id="by-product"]');
  await p.waitForSelector(".dash .panel", { timeout: 60000 });
  await sleep(500);
  const markerSurvived = await p.evaluate(() => window.__e2eMarker);
  check(markerSurvived === "still-here", "in-place nav switch: no full reload (a window marker survives)");
  check(p.url() === BASE + "d/by-product/", `in-place nav switch: URL is now ${p.url()}`);
  const tableText = await p.evaluate(() => document.querySelector(".dash")?.textContent || "");
  check(/150/.test(tableText) && /north/.test(tableText), "the parquet data actually reads (a real value renders)");
  // #830: a view over read_parquet used by three panels, then a second
  // dashboard over the same file: one fetch, not one HTTP read per panel
  const salesReads = allRequests.filter((u) => /\/data\/sales\.parquet$/.test(u)).length;
  check(salesReads === 1, `sales.parquet fetched once across two dashboards (#830), got ${salesReads}`);

  // ── 3. back/forward ──
  await p.goBack({ waitUntil: "networkidle" });
  await sleep(500);
  check(p.url() === BASE, `back: URL is ${p.url()}`);
  await p.goForward({ waitUntil: "networkidle" });
  await sleep(500);
  check(p.url() === BASE + "d/by-product/", `forward: URL is ${p.url()}`);

  // ── 4. a hard reload of /d/<id>/ lands on the right dashboard ──
  const p2 = await openPage(BASE + "d/by-product/");
  await p2.waitForSelector(".dash .panel", { timeout: 60000 });
  // #675: the first panel can be there before the table has rendered
  const loaded = await p2
    .waitForFunction(() => /Units per product|north|south|east/.test(document.querySelector(".dash")?.textContent || ""), null, { timeout: 30000 })
    .then(() => true)
    .catch(() => false);
  check(loaded, "a hard reload of /d/by-product/ loads the right dashboard");
  check((await p2.$$('.served-nav a[data-nav-id="by-product"].active')).length === 1, "the reloaded page's own nav shows it as active");

  // ── 5. S2: a `../x` path_vars value is dropped, never applied ──
  const p3 = await openPage(BASE + "d/by-product/?v.region=..%2Fetc");
  await p3.waitForSelector(".dash .panel", { timeout: 60000 });
  await sleep(500);
  const text3 = await p3.evaluate(() => document.querySelector(".dash")?.textContent || "");
  // unfiltered (the bad value dropped, `region` stays unset): every region's
  // rows still show, not just one — the same proof a server's 400 gives.
  check(/north/.test(text3) && /south/.test(text3) && /east/.test(text3), "a ../ path_vars value is dropped — the dashboard falls back to its default (unfiltered), not an error or a traversal");

  // ── 6. #500: a table ::LINK and a chart-mark ::LINK ('/d/<id>?v.x={…}')
  // switch in place under /repo/, never navigating to the domain root; the
  // hrefs themselves (served-nav, table ::LINK cell) resolve under /repo/
  // too, for middle-click / "open in new tab" / hover preview ──
  const p4 = await openPage(BASE);
  await p4.waitForSelector(".dash .panel", { timeout: 60000 });
  const navHref = await p4.$eval('.served-nav a[data-nav-id="by-product"]', (a) => a.getAttribute("href"));
  check(navHref.startsWith("/repo/d/by-product/"), `served-nav href resolves under /repo/: ${navHref}`);
  const tableLinkHref = await p4.$eval(".dash .dp-link", (a) => a.getAttribute("href"));
  check(tableLinkHref.startsWith("/repo/d/by-product/"), `table ::LINK href resolves under /repo/: ${tableLinkHref}`);

  await p4.evaluate(() => (window.__e2eMarker = "still-here"));
  await p4.click(".dash .dp-link"); // the first row: region = 'east' (units GROUP BY region ORDER BY region)
  await p4.waitForSelector(".dash .panel", { timeout: 60000 });
  await sleep(500);
  check((await p4.evaluate(() => window.__e2eMarker)) === "still-here", "table ::LINK click: no full reload (a window marker survives)");
  check(p4.url().startsWith(BASE + "d/by-product/"), `table ::LINK click: stays under /repo/ (${p4.url()})`);
  const afterTableLink = await p4.evaluate(() => document.querySelector(".dash")?.textContent || "");
  check(
    /60/.test(afterTableLink) && /40/.test(afterTableLink) && !/120/.test(afterTableLink) && !/95/.test(afterTableLink),
    "table ::LINK click: the target got the clicked row's value (region='east' filter applied)"
  );

  const p5 = await openPage(BASE);
  await p5.waitForSelector(".panel-svg > svg .dp-hit", { timeout: 60000 });
  await p5.evaluate(() => (window.__e2eMarker = "still-here"));
  const bar = await p5.evaluate(() => {
    const m = [...document.querySelectorAll(".panel-svg > svg .dp-hit")].find((e) => e.getAttribute("data-x") === "south");
    const b = m.getBoundingClientRect();
    return { x: b.left + b.width / 2, y: b.top + Math.min(b.height / 2, 20) };
  });
  await p5.mouse.click(bar.x, bar.y);
  await p5.waitForSelector(".dash .panel", { timeout: 60000 });
  await sleep(500);
  check((await p5.evaluate(() => window.__e2eMarker)) === "still-here", "chart ::LINK click: no full reload (a window marker survives)");
  check(p5.url().startsWith(BASE + "d/by-product/"), `chart ::LINK click: stays under /repo/ (${p5.url()})`);
  const afterChartLink = await p5.evaluate(() => document.querySelector(".dash")?.textContent || "");
  check(
    /95/.test(afterChartLink) && /150/.test(afterChartLink) && !/60/.test(afterChartLink) && !/120/.test(afterChartLink),
    "chart ::LINK click: the target got the clicked mark's value (region='south' filter applied)"
  );

  // ── 6b. #655: the ::REGION map draws from the published boundary layer ──
  check(existsSync(path.join(outDir, "data/boundaries/world.csv")), "--site published data/boundaries/world.csv");
  const p6 = await openPage(BASE + "d/trend/");
  await p6.waitForSelector(".dash .panel svg path", { timeout: 60000 }).catch(() => {});
  const mapText = await p6.evaluate(() => document.querySelector(".dash")?.textContent || "");
  check(!/couldn't fetch/.test(mapText), "::REGION map: the boundary layer was fetched (no 404)");
  const mapPaths = await p6.$$eval(".dash .panel svg path", (ps) => ps.length);
  check(mapPaths >= 3, `::REGION map: the three countries drawn (${mapPaths} paths)`);
  if (shots) await p6.screenshot({ path: `${shots}/site-map.png` });
  const tabs = await p6.$$eval(".dash > .tabbar > .tab-btn", (bs) => bs.filter((b) => !b.hidden && getComputedStyle(b).display !== "none").map((b) => b.textContent.trim()));
  check(JSON.stringify(tabs) === JSON.stringify(["Has sales", "Last"]), `-- @requires against the site's data (#830): tabs ${tabs.join(", ")}`);

  // (#500's locked-server guarantee — siteHref()/servedNavHref() are a
  // no-op when `published` is false, so a locked server's own /d/<id>
  // route is untouched — is unit-tested directly at the function level:
  // tests/web/site_href_test.mjs's "not published" case and
  // tests/web/nav_path_vars_test.mjs's existing published=false suite.
  // A real locked server here would need the native extension loaded via
  // anofox_serve_dashboards (scripts/served-e2e.mjs's setup), which this
  // script doesn't build; duplicating that heavier harness just to re-prove
  // a one-line early return isn't worth the extra CI time.)

  // ── 7. no request leaves this site's own origin / the pinned CDN hosts ──
  const allowedHosts = new Set([
    `127.0.0.1:${PORT}`,
    "cdn.jsdelivr.net",
    "extensions.duckdb.org",
    "fonts.googleapis.com",
    "fonts.gstatic.com",
  ]);
  const offHost = [...new Set(allRequests)].filter((u) => {
    if (u.startsWith("blob:") || u.startsWith("data:")) return false; // same-origin Worker/inline, never a network request
    try {
      return !allowedHosts.has(new URL(u).host);
    } catch (_) {
      return false;
    }
  });
  check(offHost.length === 0, `no request to an unexpected host${offHost.length ? ": " + offHost.slice(0, 5).join(", ") : ""}`);
  // #832: with a local DuckDB-Wasm in --app (scripts/fetch-duckdb-wasm.sh),
  // the site loads it from its own origin, never from jsdelivr
  if (existsSync(path.join(APP, "vendor/duckdb-wasm/duckdb-eh.wasm"))) {
    const cdnDuck = allRequests.filter((u) => /cdn\.jsdelivr\.net\/npm\/(@duckdb|apache-arrow|flatbuffers|tslib)/.test(u));
    check(cdnDuck.length === 0, `DuckDB-Wasm from the site itself, not jsdelivr (#832)${cdnDuck.length ? ": " + cdnDuck.slice(0, 3).join(", ") : ""}`);
    check(allRequests.some((u) => u.endsWith("/vendor/duckdb-wasm/duckdb-eh.wasm")), "the site's own duckdb-eh.wasm was loaded (#832)");
  } else console.log("note: no web/vendor/duckdb-wasm in --app — DuckDB-Wasm from jsdelivr (#832 check skipped)");

  // ── 8. #833: a budget, every dashboard in one page (in-place switches) —
  // no data file fetched twice, and each switch done within SWITCH_MS ──
  {
    const SWITCH_MS = 15000;
    const pb = await openPage(BASE);
    await pb.waitForSelector(".dash .panel", { timeout: 60000 });
    const reqs = [];
    pb.on("request", (r) => reqs.push(r.url()));
    for (const id of ["by-product", "trend", "overview", "by-product"]) {
      const t0 = Date.now();
      await pb.click(`.served-nav a[data-nav-id="${id}"]`);
      // siteGo marks the link active once the dashboard's run is done
      await pb
        .waitForFunction((want) => !!document.querySelector(`.served-nav a[data-nav-id="${want}"].active`) && !!document.querySelector(".dash .panel"), id, { timeout: SWITCH_MS })
        .catch(() => {});
      const ms = Date.now() - t0;
      check(ms < SWITCH_MS, `budget: switch to ${id} in ${ms} ms (< ${SWITCH_MS})`);
    }
    const data = reqs.filter((u) => /\/data\/[^/]+\.(parquet|csv|json)$/.test(u));
    const twice = data.filter((u, i) => data.indexOf(u) !== i);
    check(twice.length === 0, `budget: no data file fetched twice across four switches${twice.length ? ": " + [...new Set(twice)].join(", ") : ""}`);
    await pb.context().close();
  }
} finally {
  await browser.close();
  server.close();
  if (process.env.SITE_E2E_KEEP) console.log(`kept ${work}`);
  else rmSync(work, { recursive: true, force: true });
}

if (process.exitCode) console.error("site-e2e: FAILED");
else console.log("site-e2e: ok");
