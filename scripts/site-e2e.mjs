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
    `SELECT product::XAXIS, sum(units)::BARCHART, 'Units by product'::TITLE FROM sales GROUP BY 1 ORDER BY 1;\n`
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
    `SELECT 'nothing here but text — proves nav switches between differently-shaped dashboards'::MARKDOWN;\n`
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
  const title2 = await p2.$eval(".dash", (el) => el.textContent).catch(() => "");
  check(/Units per product|north|south|east/.test(title2), "a hard reload of /d/by-product/ loads the right dashboard");
  check((await p2.$$('.served-nav a[data-nav-id="by-product"].active')).length === 1, "the reloaded page's own nav shows it as active");

  // ── 5. S2: a `../x` path_vars value is dropped, never applied ──
  const p3 = await openPage(BASE + "d/by-product/?v.region=..%2Fetc");
  await p3.waitForSelector(".dash .panel", { timeout: 60000 });
  await sleep(500);
  const text3 = await p3.evaluate(() => document.querySelector(".dash")?.textContent || "");
  // unfiltered (the bad value dropped, `region` stays unset): every region's
  // rows still show, not just one — the same proof a server's 400 gives.
  check(/north/.test(text3) && /south/.test(text3) && /east/.test(text3), "a ../ path_vars value is dropped — the dashboard falls back to its default (unfiltered), not an error or a traversal");

  // ── 6. no request leaves this site's own origin / the pinned CDN hosts ──
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
} finally {
  await browser.close();
  server.close();
  rmSync(work, { recursive: true, force: true });
}

if (process.exitCode) console.error("site-e2e: FAILED");
else console.log("site-e2e: ok");
