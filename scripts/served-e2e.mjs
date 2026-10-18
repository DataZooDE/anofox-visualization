// A served folder on the locked server (anofox_serve_dashboards), in a real
// browser (sidebar plan, PR 4): the `/` landing page shows the dashboards as
// cards grouped by `-- @group` (counts; title, description, tags, owner,
// "✓ certified", updated), with options.app as its header and, above 8
// dashboards, a search field, tag chips and "certified only"; options.home
// redirects `/`. The nav on a dashboard page groups its links, and above 8
// dashboards becomes one button with a searchable, grouped list (keyboard:
// Enter opens, the search takes the focus, Esc closes back onto the button).
// Runs three servers (4 and 10 dashboards, and one with options.home), each
// started and stopped by this script.
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
if (!/^anofox_visualization(\..+)?\.duckdb_extension$/.test(path.basename(ext))) {
  console.error(`--ext ${ext}: LOAD takes the entry point from the file name, so it must be named anofox_visualization.duckdb_extension (or anofox_visualization.<x>.duckdb_extension). Rename it, or: ln -s '${ext}' /tmp/anofox_visualization.duckdb_extension`);
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
async function serve(dir, port, options = null) {
  const p = spawn(duckdb, ["-unsigned", "-batch", ":memory:"], { stdio: ["pipe", "pipe", "pipe"] });
  let log = "";
  p.stdout.on("data", (d) => (log += d));
  p.stderr.on("data", (d) => (log += d));
  const opts = options ? `, ${sqlStr(JSON.stringify(options))}` : "";
  p.stdin.write(`LOAD ${sqlStr(ext)};\nSELECT anofox_serve_dashboards(${sqlStr(dir)}, ${port}${opts});\n`);
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
  for (const v of r.violations) console.log(`axe ${what} ${v.impact}: ${v.id} (${v.nodes.length}) ${v.help} — ${v.nodes.map((n) => n.target).join(" ")}`);
  check(!r.violations.some((v) => v.impact === "serious" || v.impact === "critical"), `axe ${what}: no serious violation (${r.passes.length} pass)`);
}

// ── 1. Four dashboards: a grouped nav bar ──
const small = folder([
  ["weekly", "-- @title Weekly sales\n-- @group Sales\n-- @tags kpi, weekly\n-- @owner Sales controlling\n-- @certified 2026-10-15\n-- @description Orders and revenue per store, week by week."],
  ["margin", "-- @title Margin bridge\n-- @group Sales\n-- @owner Finance"],
  ["stock", "-- @title Stock outlook\n-- @group Ops\n-- @certified 2026-02-30"],
  ["start", "-- @title Start here"],
]);
let srv = await serve(small, port0);
try {
  const list = await (await fetch(`http://127.0.0.1:${port0}/`)).text();
  const at = (s) => list.indexOf(s);
  check(at(">Start here<") > 0 && at(">Start here<") < at(">Ops <") && at(">Ops <") < at(">Sales <"), "landing: ungrouped first, then the groups by name");
  check(at(">Sales <") < at(">Weekly sales<") && at(">Sales <") < at(">Margin bridge<"), "landing: cards under their group");
  check(/>Sales <span class="lp-n"[^>]*>2</.test(list), "landing: a count per group");
  check((list.match(/✓ certified/g) || []).length === 1, "landing: one ✓ certified (a bad date is no certification)");
  check(list.includes(">Sales controlling<") && list.includes("<li>kpi</li>") && list.includes(">Orders and revenue per store, week by week.<"), "landing: owner, tags and description");
  check(/Updated <time datetime="\d{4}-\d\d-\d\d">/.test(list), "landing: updated (the file's date)");
  check(list.includes("<h1 class=\"lp-title\">Dashboards</h1>") && list.includes("4 dashboards in 2 groups"), "landing: a header without options.app");
  check(!list.includes('id="lp-q"'), "landing: no filters for 4 dashboards");

  const p = await page(`http://127.0.0.1:${port0}/d/weekly`);
  await p.waitForSelector(".served-nav a", { timeout: 60000 });
  const bar = await p.$$eval(".served-nav a, .served-nav .served-nav-group", (l) => l.map((e) => (e.tagName === "A" ? "a:" + e.textContent.trim() : "g:" + e.textContent)));
  check(JSON.stringify(bar) === JSON.stringify(["a:Start here", "g:Ops", "a:Stock outlook", "g:Sales", "a:Margin bridge", "a:Weekly sales ✓"]), `nav bar grouped: ${bar.join(" | ")}`);
  check((await p.getAttribute(".served-nav a.active", "aria-current")) === "page", "the open dashboard is aria-current");
  check(/Owner: Sales controlling/.test(await p.getAttribute(".served-nav a.active", "title")), "owner in the link's tooltip");
  await p.waitForSelector(".dash .panel", { timeout: 60000 });
  await axeCheck(p, ".served-nav", "nav bar");
  if (shots) await p.screenshot({ path: `${shots}/${tag}-served-bar.png` });
  const lp = await page(`http://127.0.0.1:${port0}/`);
  await axeCheck(lp, "body", "landing (4)");
  if (shots) await lp.screenshot({ path: `${shots}/${tag}-landing-small.png`, fullPage: true });
} finally {
  stop(srv);
}

// ── 2. Ten dashboards: one button, a grouped searchable list ──
const big = folder(
  Array.from({ length: 10 }, (_, i) => [
    `d${i}`,
    `-- @title Report ${i}${i % 3 === 0 ? "\n-- @group Finance" : i % 3 === 1 ? "\n-- @group Ops / Plants" : ""}${i === 4 ? "\n-- @owner Plant team\n-- @certified 2026-10-01" : ""}\n-- @tags ${i % 2 ? "weekly" : "monthly"}\n-- @description What report ${i} answers, in one line.`,
  ])
);
const port1 = port0 + 1;
srv = await serve(big, port1, { app: { name: "Bakery <BI>", tagline: "Sales & stock" } });
try {
  const list = await (await fetch(`http://127.0.0.1:${port1}/`)).text();
  check(list.includes('id="lp-q"') && list.includes(">Plants</h3>") && list.includes('id="lp-cert"'), "landing: filters above 8, `A / B` as a sub-heading");
  check(list.includes("<h1 class=\"lp-title\">Bakery &lt;BI&gt;</h1>") && list.includes("Sales &amp; stock · 10 dashboards"), "landing: options.app is the header (escaped)");
  // the filters, in the browser
  const lp = await page(`http://127.0.0.1:${port1}/`);
  await lp.waitForSelector("#lp-q");
  const shownCards = () => lp.$$eval(".lp-card", (l) => l.filter((c) => !c.hidden).map((c) => c.querySelector(".lp-link").textContent));
  await lp.fill("#lp-q", "report 4");
  check(JSON.stringify(await shownCards()) === JSON.stringify(["Report 4"]), "landing search");
  check(/1 of 10/.test(await lp.textContent("#lp-status")), "…announced (role=status)");
  await lp.click("#lp-clear");
  check((await shownCards()).length === 10 && (await lp.evaluate(() => document.activeElement.id)) === "lp-q", "Clear shows all, focus to the search");
  await lp.click('.lp-chip[data-tag="weekly"]');
  check((await shownCards()).length === 5 && (await lp.getAttribute('.lp-chip[data-tag="weekly"]', "aria-pressed")) === "true", "a tag chip filters (aria-pressed)");
  await lp.click('.lp-chip[data-tag="weekly"]');
  await lp.click("#lp-cert");
  check(JSON.stringify(await shownCards()) === JSON.stringify(["Report 4"]), "certified only");
  check((await lp.$$eval(".lp-group", (l) => l.filter((g) => !g.hidden).length)) === 1, "groups without a match hide");
  await lp.click("#lp-clear");
  await lp.focus("#lp-q");
  await lp.keyboard.press("Tab");
  check(await lp.evaluate(() => document.activeElement.classList.contains("lp-chip")), "Tab reaches the chips");
  await axeCheck(lp, "body", "landing (10)");
  if (shots) await lp.screenshot({ path: `${shots}/${tag}-landing.png`, fullPage: true });
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

// ── 3. options.home: / goes straight to that dashboard ──
const port2 = port0 + 2;
srv = await serve(small, port2, { home: "weekly" });
try {
  const r = await fetch(`http://127.0.0.1:${port2}/`, { redirect: "manual" });
  check(r.status === 302 && r.headers.get("location") === "/d/weekly", `options.home: / → ${r.status} ${r.headers.get("location")}`);
} finally {
  stop(srv);
}

check(!errs.length, `no page errors ${errs.join(" | ")}`);
await browser.close();
for (const d of [small, big]) rmSync(d, { recursive: true, force: true });
if (!process.exitCode) console.log(`served-e2e (${tag}): ok`);
