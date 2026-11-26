// #1391 (4): load on a locked server — VIEWERS viewers (default 50), each its
// own login, read a story with scenario buttons at the same time: open the
// page (every panel), then press each scenario button in turn. One browser
// records the page's own panel requests per step; the viewers replay them
// concurrently, as the page sends them (all panels of a step at once, a 503
// retried after its Retry-After, twice at most).
//
// --shared: no access policy, so the panels (which don't read the login) are
// answered once for everybody — the best case. Default: a row policy gives
// every viewer their own rows (a region each), so no answer is shared — the
// worst case.
//
// Reports per step the wait of a viewer (p50 / p95 / max), the requests by
// status, and the server's memory (VmRSS before, peak during, after).
// Fails on any answer that is not 200 after the retries, a p95 above
// --p95-max seconds (default 10), or a peak RSS above --rss-max MB (2048).
//
//   node scripts/load-served.mjs --ext path/to/anofox_visualization.duckdb_extension \
//       [--viewers 50] [--rows 2000000] [--shared] [--port 8974] [--duckdb duckdb]
import { createRequire } from "module";
import { spawn } from "child_process";
import { mkdtempSync, mkdirSync, writeFileSync, readFileSync, rmSync, existsSync } from "fs";
import path from "path";
import crypto from "crypto";

const require = createRequire(import.meta.url);
const { chromium } = require("playwright");
const arg = (k, d) => {
  const i = process.argv.indexOf(k);
  return i > 0 ? process.argv[i + 1] : d;
};
const ext = arg("--ext", "");
const port = Number(arg("--port", "8974"));
const duckdb = arg("--duckdb", "duckdb");
const VIEWERS = Number(arg("--viewers", "50"));
const ROWS = Number(arg("--rows", "2000000"));
const P95_MAX = Number(arg("--p95-max", "10"));
const RSS_MAX = Number(arg("--rss-max", "2048"));
const SHARED = process.argv.includes("--shared");
if (!ext || !existsSync(ext)) {
  console.error("usage: load-served.mjs --ext <anofox_visualization.duckdb_extension>");
  process.exit(2);
}
let bad = 0;
const check = (ok, m) => (ok ? console.log("ok   " + m) : (bad++, console.error("FAIL " + m)));
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const sqlStr = (s) => `'${s.replace(/'/g, "''")}'`;
const env = { ...process.env, DATAZOO_DISABLE_TELEMETRY: "1" };

const W = mkdtempSync(path.join(path.dirname(path.resolve(ext)), "load-served-"));
const dashDir = path.join(W, "dash");
mkdirSync(dashDir, { recursive: true });
// A story of three parts; the scenario (::BUTTONS) changes the numbers of
// every part, so each press re-runs most panels over the whole table.
writeFileSync(
  path.join(dashDir, "story.sql"),
  `-- @title Returns under load
-- @description A story with scenario buttons, read by many viewers at once.
-- @layout story
-- @vars sc
SELECT 'Sales'::TAB;
SELECT sc ::BUTTONS, 'Scenario'::LABEL FROM (VALUES ('base'), ('promo'), ('storm')) t(sc);
SELECT 'Every sale may come back; the scenario sets how many.'::MARKDOWN;
SELECT sum(units * f)::BIGINT AS returned ::METRIC, 'Returned units'::LABEL FROM sales, (SELECT CASE coalesce(getvariable('sc'), 'base') WHEN 'promo' THEN 0.32 WHEN 'storm' THEN 0.25 ELSE 0.2 END AS f);
SELECT day::XAXIS, sum(units)::LINECHART, 'Units sold per day'::TITLE FROM sales GROUP BY day ORDER BY day;
SELECT 'Returns'::TAB;
SELECT day::XAXIS, (sum(units) * CASE coalesce(getvariable('sc'), 'base') WHEN 'promo' THEN 0.32 WHEN 'storm' THEN 0.25 ELSE 0.2 END)::BARCHART, 'Returns per day'::TITLE FROM sales GROUP BY day ORDER BY day;
SELECT region::XAXIS, (sum(units) * CASE coalesce(getvariable('sc'), 'base') WHEN 'promo' THEN 0.32 ELSE 0.2 END)::BARCHART, 'Returns by region'::TITLE FROM sales GROUP BY region ORDER BY region;
SELECT 'Warehouse'::TAB;
SELECT region ::TABLE, count(*) AS orders, sum(units) AS units, round(avg(units) * CASE coalesce(getvariable('sc'), 'base') WHEN 'storm' THEN 1.4 ELSE 1 END, 2) AS load FROM sales GROUP BY region ORDER BY region;
${Array.from({ length: 6 }, (_, i) => `SELECT 'Paragraph ${i} of the story.'::MARKDOWN;`).join("\n")}
`
);
const SETUP = `CREATE TABLE sales AS SELECT (range % 120) AS day, ['north','south','east','west','centre'][1 + (range % 5)::INT] AS region, 1 + (hash(range) % 9)::INT AS units FROM range(${ROWS});
CREATE TABLE entitlements AS SELECT 'viewer-' || range AS viewer, ['north','south','east','west','centre'][1 + (range % 5)::INT] AS region FROM range(${VIEWERS})
  UNION ALL SELECT 'recorder', 'north';`;
const policyFile = path.join(W, "policy.json");
writeFileSync(
  policyFile,
  JSON.stringify({
    tables: {
      sales: { rows: "region IN (SELECT region FROM entitlements WHERE viewer = getvariable('__claim_user'))" },
      entitlements: { rows: "viewer = getvariable('__claim_user')" },
    },
  })
);
const sec = crypto.randomBytes(24).toString("hex");
const opts = { auth: { mode: "proxy", shared_secret: sec, trusted_proxies: ["127.0.0.1"] }, policy: SHARED ? "none" : policyFile };
const BASE = `http://127.0.0.1:${port}`;

const rssMB = (pid) => {
  try {
    const s = readFileSync(`/proc/${pid}/status`, "utf8");
    return Number(/VmRSS:\s+(\d+)/.exec(s)[1]) / 1024;
  } catch (_) {
    return NaN;
  }
};
const pct = (xs, p) => {
  const s = [...xs].sort((a, b) => a - b);
  return s[Math.min(s.length - 1, Math.floor((p / 100) * s.length))];
};

let proc = null;
let log = "";
let browser = null;
try {
  proc = spawn(duckdb, ["-unsigned", "-batch", ":memory:"], { stdio: ["pipe", "pipe", "pipe"], env });
  proc.stdout.on("data", (d) => (log = (log + d).slice(-20000)));
  proc.stderr.on("data", (d) => (log = (log + d).slice(-20000)));
  proc.stdin.write(`LOAD ${sqlStr(ext)};\n${SETUP}\nSELECT anofox_serve_dashboards(${sqlStr(dashDir)}, ${port}, ${sqlStr(JSON.stringify(opts))});\n`);
  let up = false;
  for (let i = 0; i < 600 && !up; i++) {
    try {
      up = (await fetch(`${BASE}/healthz`)).ok;
    } catch (_) {}
    if (proc.exitCode != null) break;
    if (!up) await sleep(100);
  }
  if (!up) throw new Error(`the server did not start:\n${log}`);

  // 1. record the page's panel requests: the opening, then each button
  browser = await chromium.launch();
  const ctx = await browser.newContext({ viewport: { width: 1300, height: 900 } });
  await ctx.setExtraHTTPHeaders({ "X-Anofox-Proxy-Secret": sec, "X-Forwarded-User": "recorder" });
  const page = await ctx.newPage();
  let cur = [];
  page.on("request", (r) => r.url().endsWith("/api/panel") && cur.push(r.postData()));
  const settle = async () => {
    await page.waitForFunction(() => !document.body.classList.contains("loading") && !document.querySelector(".dash .panel.loading, .dash [aria-busy=true]"), null, { timeout: 60000 });
    await page.waitForTimeout(1500);
  };
  await page.goto(`${BASE}/d/story/`);
  await page.waitForFunction(() => document.querySelectorAll(".dash .panel-svg svg").length >= 3, null, { timeout: 60000 });
  await settle();
  const steps = [{ name: "open", bodies: cur }];
  // (back to base needs no request: the page still holds those answers)
  for (const v of ["promo", "storm"]) {
    cur = [];
    await page.locator(`.dash button`, { hasText: new RegExp(`^${v}$`) }).first().click();
    await settle();
    steps.push({ name: `press ${v}`, bodies: cur });
  }
  await browser.close();
  browser = null;
  check(steps.every((s) => s.bodies.length > 0), `recorded the page's requests: ${steps.map((s) => `${s.name} ${s.bodies.length}`).join(", ")}`);

  // 2. the viewers
  const statuses = {};
  const post = async (user, body) => {
    for (let attempt = 1; ; attempt++) {
      const r = await fetch(`${BASE}/api/panel`, {
        method: "POST",
        headers: { "Content-Type": "application/json", "X-Anofox-Proxy-Secret": sec, "X-Forwarded-User": user },
        body,
      });
      await r.arrayBuffer();
      statuses[r.status] = (statuses[r.status] || 0) + 1;
      if (r.status !== 503 || attempt > 2) return r.status;
      await sleep(Math.min(5, Number(r.headers.get("Retry-After")) || 1) * attempt * 1000);
    }
  };
  const before = rssMB(proc.pid);
  let peak = before;
  const sampler = setInterval(() => (peak = Math.max(peak, rssMB(proc.pid))), 100);
  const waits = steps.map(() => []);
  const failed = [];
  const t0 = Date.now();
  await Promise.all(
    Array.from({ length: VIEWERS }, async (_, n) => {
      const user = `viewer-${n}`;
      await sleep(Math.random() * 2000); // they don't all arrive in the same millisecond
      for (const [i, s] of steps.entries()) {
        const t = performance.now();
        const got = await Promise.all(s.bodies.map((b) => post(user, b)));
        waits[i].push((performance.now() - t) / 1000);
        got.filter((x) => x !== 200).forEach((x) => failed.push(`${user} ${s.name}: ${x}`));
        await sleep(500 + Math.random() * 1500); // reading
      }
    })
  );
  clearInterval(sampler);
  const total = (Date.now() - t0) / 1000;
  await sleep(2000);
  const after = rssMB(proc.pid);

  console.log(`\n${VIEWERS} viewers, ${ROWS.toLocaleString("en")} rows, ${SHARED ? "shared answers (no policy)" : "a row policy per viewer (nothing shared)"}, ${total.toFixed(1)} s in all`);
  console.log("step            requests   p50 s   p95 s   max s");
  for (const [i, s] of steps.entries())
    console.log(
      `${s.name.padEnd(16)}${String(s.bodies.length).padStart(8)}${pct(waits[i], 50).toFixed(2).padStart(8)}${pct(waits[i], 95).toFixed(2).padStart(8)}${Math.max(...waits[i]).toFixed(2).padStart(8)}`
    );
  console.log(`requests by status: ${JSON.stringify(statuses)}`);
  console.log(`server memory (VmRSS): ${before.toFixed(0)} MB before, ${peak.toFixed(0)} MB peak, ${after.toFixed(0)} MB after\n`);

  check(!failed.length, `every answer 200 after the page's retries (${failed.length} not: ${failed.slice(0, 5).join("; ")})`);
  const p95 = Math.max(...waits.map((w) => pct(w, 95)));
  check(p95 <= P95_MAX, `a viewer's wait per step: p95 ${p95.toFixed(2)} s ≤ ${P95_MAX} s`);
  check(peak <= RSS_MAX, `server peak memory ${peak.toFixed(0)} MB ≤ ${RSS_MAX} MB`);
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
console.log(`load-served: ${bad} failed`);
process.exit(bad ? 1 : 0);
