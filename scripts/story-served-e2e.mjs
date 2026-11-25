// `-- @layout story` (#1367) on a real locked server: the served page is a
// story too (opening, every part shown, contents, a ::STORY chart that scrolls,
// a term, an input inside a sentence that still drives its chart) — and it
// shows no SQL: "How this is computed" is left out, as the server's viewers
// get the dashboard, not its source.
//
//   node scripts/story-served-e2e.mjs \
//       --ext path/to/anofox_visualization.duckdb_extension [--port 8968] [--duckdb duckdb]
import { createRequire } from "module";
import { spawn } from "child_process";
import { mkdtempSync, mkdirSync, writeFileSync, rmSync, existsSync } from "fs";
import path from "path";
import crypto from "crypto";

const require = createRequire(import.meta.url);
const { chromium } = require("playwright");
const arg = (k, d) => {
  const i = process.argv.indexOf(k);
  return i > 0 ? process.argv[i + 1] : d;
};
const ext = arg("--ext", "");
const port = Number(arg("--port", "8968"));
const duckdb = arg("--duckdb", "duckdb");
if (!ext || !existsSync(ext)) {
  console.error("usage: story-served-e2e.mjs --ext <anofox_visualization.duckdb_extension>");
  process.exit(2);
}
let bad = 0;
const check = (ok, m) => (ok ? console.log("ok   " + m) : (bad++, console.error("FAIL " + m)));
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const sqlStr = (s) => `'${s.replace(/'/g, "''")}'`;
const env = { ...process.env, DATAZOO_DISABLE_TELEMETRY: "1" };

const W = mkdtempSync(path.join(path.dirname(path.resolve(ext)), "story-served-e2e-"));
const dashDir = path.join(W, "dash");
mkdirSync(dashDir, { recursive: true });
writeFileSync(
  path.join(dashDir, "story.sql"),
  `-- @title Returns
-- @description A short story on a locked server.
-- @layout story
-- @vars k
-- @term wave: the returns of one sale, spread over weeks
SELECT 'Start'::TAB;
SELECT 'A sale comes back as a wave, not a peak.'::MARKDOWN;
SELECT day::XAXIS, units::LINECHART, CASE WHEN day IN (2, 40) THEN 'Step on day ' || day END ::STORY FROM sales ORDER BY day;
SELECT 'End'::TAB;
SELECT 3 AS k ::NUMBER, 'Days'::LABEL;
SELECT 'Look back {{@k}} days: {{n}} units.'::MARKDOWN, sum(units)::INT AS n FROM sales WHERE day > 60 - coalesce(getvariable('k'), 3);
SELECT day::XAXIS, units::BARCHART FROM sales WHERE day > 60 - coalesce(getvariable('k'), 3) ORDER BY day;
${Array.from({ length: 10 }, (_, i) => `SELECT 'Filler paragraph ${i} so the page scrolls.'::MARKDOWN;`).join("\n")}
`
);
const SETUP = `CREATE TABLE sales AS SELECT range AS day, 50 + (range % 7) * 3 AS units FROM range(1, 61);`;
const sec = crypto.randomBytes(24).toString("hex");
const opts = { auth: { mode: "proxy", shared_secret: sec, trusted_proxies: ["127.0.0.1"] }, policy: "none" };
const BASE = `http://127.0.0.1:${port}`;

let proc = null;
let log = "";
let browser = null;
try {
  proc = spawn(duckdb, ["-unsigned", "-batch", ":memory:"], { stdio: ["pipe", "pipe", "pipe"], env });
  proc.stdout.on("data", (d) => (log += d));
  proc.stderr.on("data", (d) => (log += d));
  proc.stdin.write(`LOAD ${sqlStr(ext)};\n${SETUP}\nSELECT anofox_serve_dashboards(${sqlStr(dashDir)}, ${port}, ${sqlStr(JSON.stringify(opts))});\n`);
  let up = false;
  for (let i = 0; i < 100 && !up; i++) {
    try {
      up = (await fetch(`${BASE}/healthz`)).ok;
    } catch (_) {}
    if (proc.exitCode != null) break;
    if (!up) await sleep(100);
  }
  if (!up) throw new Error(`the server did not start:\n${log}`);

  browser = await chromium.launch();
  const ctx = await browser.newContext({ viewport: { width: 1400, height: 800 }, reducedMotion: "reduce" });
  await ctx.setExtraHTTPHeaders({ "X-Anofox-Proxy-Secret": sec, "X-Forwarded-User": "alice" });
  const page = await ctx.newPage();
  const errs = [];
  page.on("pageerror", (e) => errs.push(String(e)));
  await page.goto(`${BASE}/d/story/`);
  await page.waitForFunction(() => !document.body.classList.contains("loading") && document.querySelectorAll(".dash .panel-svg svg").length >= 2, null, { timeout: 30000 });
  await page.waitForTimeout(800);
  const s = await page.evaluate(() => ({
    story: document.querySelector(".dash")?.classList.contains("dp-story"),
    title: document.querySelector(".story-hero .story-title")?.textContent,
    shown: [...document.querySelectorAll(".dash > .tabwrap > .tabpane")].filter((p) => getComputedStyle(p).display !== "none").length,
    scrolly: document.querySelectorAll(".story-scrolly").length,
    terms: document.querySelectorAll(".story-term").length,
    inline: !!document.querySelector(".story-inline input"),
    sql: document.querySelectorAll(".story-sql").length,
  }));
  check(s.story && s.title === "Returns", `the served page is a story with its opening (${s.title})`);
  check(s.shown === 2, `every part shown (${s.shown})`);
  check(s.scrolly === 1 && s.terms === 1 && s.inline, `scrolling ::STORY chart, term and inline input (${JSON.stringify(s)})`);
  check(s.sql === 0, `no "How this is computed" — a locked server shows no SQL (${s.sql})`);
  const text = () => page.evaluate(() => [...document.querySelectorAll(".md-body")].find((x) => /Look back/.test(x.textContent))?.textContent);
  const before = await text();
  await page.fill(".story-inline input", "10");
  await page.press(".story-inline input", "Enter");
  await page.waitForFunction((b) => {
    const t = [...document.querySelectorAll(".md-body")].find((x) => /Look back/.test(x.textContent))?.textContent;
    return t && t !== b;
  }, before, { timeout: 20000 });
  check((await text()) !== before, "the inline input drives the server's panels");
  check(!errs.length, "no page errors: " + errs.join(" | "));
} catch (e) {
  bad++;
  console.error("FAIL " + (e.stack || e));
  console.error(log);
} finally {
  if (browser) await browser.close();
  if (proc) {
    proc.stdin.end();
    proc.kill(); // our own child (pid known), never by name
  }
  rmSync(W, { recursive: true, force: true });
}
console.log(`story-served-e2e: ${bad} failed`);
process.exit(bad ? 1 : 0);
