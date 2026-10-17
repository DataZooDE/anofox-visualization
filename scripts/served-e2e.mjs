// A served folder on the locked server (anofox_serve_dashboards), in a real
// browser (sidebar plan, PR 4): the `/` list groups the dashboards by
// `-- @group` and shows the owner, "✓ certified" and the tags; the nav on a
// dashboard page groups its links, and above 8 dashboards becomes one button
// with a searchable, grouped list (keyboard: Enter opens, the search takes
// the focus, Esc closes back onto the button). Runs two folders (4 and 10
// dashboards), each on its own server it starts and stops.
//
//   NODE_PATH=<dir with playwright>/node_modules node scripts/served-e2e.mjs \
//       --ext path/to/anofox_visualization.duckdb_extension [--port 8930] \
//       [--duckdb duckdb] [--theme dark|light] [--width 390] [--axe axe.min.js] [--shots DIR]
import { createRequire } from "module";
import { spawn } from "child_process";
import { mkdirSync, mkdtempSync, writeFileSync, rmSync, existsSync } from "fs";
import { tmpdir } from "os";
import path from "path";

const require = createRequire(import.meta.url);
const { chromium } = require("playwright");
const arg = (k, d) => {
  const i = process.argv.indexOf(k);
  return i > 0 ? process.argv[i + 1] : d;
};
const ext = arg("--ext", "");
const port0 = Number(arg("--port", "8930"));
const duckdb = arg("--duckdb", "duckdb");
const theme = arg("--theme", "");
const width = Number(arg("--width", "1280"));
const axe = arg("--axe", "");
const shots = arg("--shots", "");
if (!ext || !existsSync(ext)) {
  console.error("usage: served-e2e.mjs --ext <anofox_visualization.duckdb_extension> (build it with duckext/scripts/build-native.sh)");
  process.exit(2);
}
if (shots) mkdirSync(shots, { recursive: true });
const tag = `${width <= 820 ? "phone" : "desktop"}${theme ? "-" + theme : ""}`;
const fail = (m) => {
  console.error("FAIL: " + m);
  process.exitCode = 1;
};
const check = (ok, m) => (ok ? console.log("ok   " + m) : fail(m));
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const sqlStr = (s) => `'${s.replace(/'/g, "''")}'`;

// A folder of small dashboards: [id, header lines].
function folder(entries) {
  const dir = mkdtempSync(path.join(tmpdir(), "dp-served-"));
  for (const [id, head] of entries) writeFileSync(path.join(dir, `${id}.sql`), `${head}\nSELECT 42 AS n ::METRIC, 'Orders'::LABEL;\n`);
  return dir;
}
// Start the locked server on `dir` (a DuckDB process we own; stopped by PID).
async function serve(dir, port) {
  const p = spawn(duckdb, ["-unsigned", "-batch", ":memory:"], { stdio: ["pipe", "pipe", "pipe"] });
  let log = "";
  p.stdout.on("data", (d) => (log += d));
  p.stderr.on("data", (d) => (log += d));
  p.stdin.write(`LOAD ${sqlStr(ext)};\nSELECT anofox_serve_dashboards(${sqlStr(dir)}, ${port});\n`);
  for (let i = 0; i < 100; i++) {
    try {
      if ((await fetch(`http://127.0.0.1:${port}/healthz`)).ok) return { proc: p, log: () => log };
    } catch (_) {}
    if (p.exitCode != null) break;
    await sleep(100);
  }
  p.kill();
  throw new Error(`the server did not start:\n${log}`);
}
const stop = (s) => {
  s.proc.stdin.end();
  s.proc.kill(); // our own child (pid known), never by name
};

const browser = await chromium.launch();
const errs = [];
async function page(url) {
  const ctx = await browser.newContext({ viewport: { width, height: 900 }, reducedMotion: "reduce", colorScheme: theme === "dark" ? "dark" : "light" });
  const p = await ctx.newPage();
  p.on("pageerror", (e) => errs.push(String(e)));
  await p.goto(url + (theme ? `?theme=${theme}` : ""));
  return p;
}
async function axeCheck(p, sel, what) {
  if (!axe || !existsSync(axe)) return;
  await p.addScriptTag({ path: axe });
  const r = await p.evaluate(async (s) => window.axe.run(s, { runOnly: { type: "tag", values: ["wcag2a", "wcag2aa"] } }), sel);
  for (const v of r.violations) console.log(`axe ${what} ${v.impact}: ${v.id} (${v.nodes.length}) ${v.help} — ${v.nodes[0]?.target}`);
  check(!r.violations.some((v) => v.impact === "serious" || v.impact === "critical"), `axe ${what}: no serious violation (${r.passes.length} pass)`);
}

// ── 1. Four dashboards: a grouped nav bar ──
const small = folder([
  ["weekly", "-- @title Weekly sales\n-- @group Sales\n-- @tags kpi, weekly\n-- @owner Sales controlling\n-- @certified 2026-10-15"],
  ["margin", "-- @title Margin bridge\n-- @group Sales\n-- @owner Finance"],
  ["stock", "-- @title Stock outlook\n-- @group Ops\n-- @certified 2026-02-30"],
  ["start", "-- @title Start here"],
]);
let srv = await serve(small, port0);
try {
  const list = await (await fetch(`http://127.0.0.1:${port0}/`)).text();
  const at = (s) => list.indexOf(s);
  check(at(">Start here<") > 0 && at(">Start here<") < at(">Ops</h2>") && at(">Ops</h2>") < at(">Sales</h2>"), "/ list: ungrouped first, then the groups by name");
  check(at(">Sales</h2>") < at(">Weekly sales<") && at(">Sales</h2>") < at(">Margin bridge<"), "/ list: dashboards under their group");
  check((list.match(/✓ certified/g) || []).length === 1, "/ list: one ✓ certified (a bad date is no certification)");
  check(list.includes(">Sales controlling<") && list.includes(">kpi<"), "/ list: owner and tags");
  check(!list.includes('id="dash-q"'), "/ list: no search for 4 dashboards");

  const p = await page(`http://127.0.0.1:${port0}/d/weekly`);
  await p.waitForSelector(".served-nav a", { timeout: 60000 });
  const bar = await p.$$eval(".served-nav a, .served-nav .served-nav-group", (l) => l.map((e) => (e.tagName === "A" ? "a:" + e.textContent.trim() : "g:" + e.textContent)));
  check(JSON.stringify(bar) === JSON.stringify(["a:Start here", "g:Ops", "a:Stock outlook", "g:Sales", "a:Margin bridge", "a:Weekly sales ✓"]), `nav bar grouped: ${bar.join(" | ")}`);
  check((await p.getAttribute(".served-nav a.active", "aria-current")) === "page", "the open dashboard is aria-current");
  check(/Owner: Sales controlling/.test(await p.getAttribute(".served-nav a.active", "title")), "owner in the link's tooltip");
  await p.waitForSelector(".dash .panel", { timeout: 60000 });
  await axeCheck(p, ".served-nav", "nav bar");
  if (shots) await p.screenshot({ path: `${shots}/${tag}-served-bar.png` });
  if (shots) {
    const lp = await page(`http://127.0.0.1:${port0}/`);
    await lp.screenshot({ path: `${shots}/${tag}-served-list.png`, fullPage: true });
  }
} finally {
  stop(srv);
}

// ── 2. Ten dashboards: one button, a grouped searchable list ──
const big = folder(
  Array.from({ length: 10 }, (_, i) => [
    `d${i}`,
    `-- @title Report ${i}${i % 3 === 0 ? "\n-- @group Finance" : i % 3 === 1 ? "\n-- @group Ops / Plants" : ""}${i === 4 ? "\n-- @owner Plant team\n-- @certified 2026-10-01" : ""}`,
  ])
);
const port1 = port0 + 1;
srv = await serve(big, port1);
try {
  const list = await (await fetch(`http://127.0.0.1:${port1}/`)).text();
  check(list.includes('id="dash-q"') && list.includes(">Plants</h3>"), "/ list: a search field above 8, `A / B` as a sub-heading");
  const p = await page(`http://127.0.0.1:${port1}/d/d4`);
  await p.waitForSelector("#served-nav-pick", { timeout: 60000 });
  check((await p.$$(".served-nav > a")).length === 0, "no row of 10 links: one button");
  check((await p.textContent("#served-nav-pick")).startsWith("Report 4"), "the button names the open dashboard");
  await p.focus("#served-nav-pick");
  await p.keyboard.press("Enter");
  check((await p.getAttribute("#served-nav-pick", "aria-expanded")) === "true", "Enter opens the list");
  check(await p.evaluate(() => document.activeElement.classList.contains("served-nav-q")), "the search takes the focus");
  const heads = await p.$$eval("#served-nav-menu h3", (l) => l.map((h) => h.textContent));
  check(JSON.stringify(heads) === JSON.stringify(["Finance", "Ops / Plants"]), `groups: ${heads.join(", ")}`);
  await p.keyboard.type("plant");
  const shown = await p.$$eval("#served-nav-menu li", (l) => l.filter((li) => !li.hidden).map((li) => li.querySelector("a").textContent.trim()));
  check(shown.length === 3 && shown.every((t) => /Report (1|4|7)/.test(t)), `search "plant": ${shown.join(", ")}`);
  await axeCheck(p, ".served-nav", "nav menu");
  if (shots) await p.screenshot({ path: `${shots}/${tag}-served-menu.png` });
  await p.keyboard.press("Escape");
  check((await p.getAttribute("#served-nav-pick", "aria-expanded")) === "false" && (await p.evaluate(() => document.activeElement.id)) === "served-nav-pick", "Esc closes, focus back on the button");
  await p.click("#served-nav-pick");
  await p.click('#served-nav-menu a[href="/d/d7"]');
  await p.waitForURL(/\/d\/d7$/);
  check(true, "a link in the list opens that dashboard");
} finally {
  stop(srv);
}

check(!errs.length, `no page errors ${errs.join(" | ")}`);
await browser.close();
for (const d of [small, big]) rmSync(d, { recursive: true, force: true });
if (!process.exitCode) console.log(`served-e2e (${tag}): ok`);
