// #210 measurement: how many /api/panel requests one click on a locked
// dashboard sends, before and after the locked server keys its cache (and
// re-requests) by the variables a panel actually depends on, not every
// variable. Not run in CI — it's a before/after measurement, run once
// against the extension built from each side (unlike scripts/served-e2e.mjs
// / scripts/input-options-order-served-e2e.mjs, which are plain pass/fail
// checks and do run in CI's `web-e2e` job, #590).
//
//   node scripts/varscope-e2e.mjs --ext path/to/anofox_visualization.duckdb_extension \
//       [--port 8931] [--duckdb duckdb] [--expect-max N] [--expect-last N]
import { createRequire } from "module";
import { spawn, spawnSync } from "child_process";
import { mkdtempSync, mkdirSync, writeFileSync, rmSync, existsSync } from "fs";
import { tmpdir } from "os";
import path from "path";

const require = createRequire(import.meta.url);
const { chromium } = require("playwright");
const arg = (k, d) => {
  const i = process.argv.indexOf(k);
  return i > 0 ? process.argv[i + 1] : d;
};
const ext = arg("--ext", "");
const port = Number(arg("--port", "8931"));
const duckdb = arg("--duckdb", "duckdb");
const expectMax = arg("--expect-max", null);
const expectLast = arg("--expect-last", null);
if (!ext || !existsSync(ext)) {
  console.error("usage: varscope-e2e.mjs --ext <anofox_visualization.duckdb_extension> (build it with duckext/scripts/build-native.sh)");
  process.exit(2);
}
const fail = (m) => {
  console.error("FAIL: " + m);
  process.exitCode = 1;
};
const check = (ok, m) => (ok ? console.log("ok   " + m) : fail(m));
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const sqlStr = (s) => `'${s.replace(/'/g, "''")}'`;
const env = { ...process.env, DATAZOO_DISABLE_TELEMETRY: "1" };

// 1. A deterministic, fake dataset: 20 series, 200 points each.
const dir = mkdtempSync(path.join(tmpdir(), "dp-varscope-"));
const db = path.join(dir, "data.duckdb");
const DATA =
  "CREATE TABLE s AS SELECT 'S' || lpad((i % 20)::VARCHAR, 2, '0') AS series, i AS t, ((i * 7919) % 1000) / 10.0 AS y FROM range(4000) r(i);";
{
  const r = spawnSync(duckdb, [db, "-c", DATA], { env });
  if (r.status !== 0) {
    console.error((r.stdout || "").toString(), (r.stderr || "").toString());
    throw new Error("could not build the fake dataset");
  }
}

// 2. The dashboard: an Overview tab (five panels that read no variable) and a
// Detail tab whose panels read the clicked row's series — some directly
// (::ROWSELECT), one through a `SET VARIABLE sel` closure.
const dashDir = path.join(dir, "dash");
mkdirSync(dashDir, { recursive: true });
writeFileSync(
  path.join(dashDir, "vs.sql"),
  `SELECT 'Overview'::TAB;
SELECT count(*)::BIGINT AS n ::METRIC, 'Rows'::LABEL FROM s;
SELECT count(DISTINCT series)::BIGINT AS n ::METRIC, 'Series'::LABEL FROM s;
SELECT round(avg(y), 1) AS a ::METRIC, 'Mean'::LABEL FROM s;
SELECT max(y) AS m ::METRIC, 'Max'::LABEL FROM s;
SELECT series ::XAXIS, round(sum(y), 1) AS total ::BARCHART FROM s GROUP BY series ORDER BY series;
SELECT 'Detail'::TAB;
SET VARIABLE sel = coalesce(nullif(getvariable('series'), ''), 'S00');
SELECT 'single'::ROWSELECT;
SELECT series ::TABLE, count(*)::BIGINT AS n, round(sum(y), 1) AS total FROM s GROUP BY series ORDER BY series;
SELECT count(*)::BIGINT AS n ::METRIC, 'Rows of ' || getvariable('sel') ::LABEL FROM s WHERE series = getvariable('sel');
SELECT round(avg(y), 1) AS a ::METRIC, 'Mean of ' || getvariable('sel') ::LABEL FROM s WHERE series = getvariable('sel');
SELECT t ::XAXIS, y ::LINECHART FROM s WHERE series = getvariable('sel') ORDER BY t;
SELECT series AS "Series" ::PAGED, t AS "t", y AS "y" FROM s ORDER BY series, t;
SELECT t % 10 AS bucket ::XAXIS, count(*)::BIGINT AS n ::BARCHART FROM s GROUP BY 1 ORDER BY 1;
`
);

// 3. Start the locked server on the fake dataset (a DuckDB process we own;
// stopped by PID).
async function serve() {
  const p = spawn(duckdb, ["-unsigned", "-batch", db], { stdio: ["pipe", "pipe", "pipe"], env });
  let log = "";
  p.stdout.on("data", (d) => (log += d));
  p.stderr.on("data", (d) => (log += d));
  p.stdin.write(`LOAD ${sqlStr(ext)};\nSELECT anofox_serve_dashboards(${sqlStr(dashDir)}, ${port});\n`);
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

let srv = null;
let browser = null;
try {
  srv = await serve();
  browser = await chromium.launch();
  const ctx = await browser.newContext({ viewport: { width: 1280, height: 900 }, reducedMotion: "reduce" });
  const page = await ctx.newPage();

  // 4. Track /api/panel requests: a counter, an in-flight count and the time
  // of the last event. "Quiet": in-flight 0 and nothing happened for 2500 ms
  // (covers the idle prefetch of hidden tabs, requestIdleCallback timeout
  // 1 s), with a 30 s timeout that fails the run.
  let count = 0;
  let inFlight = 0;
  let last = Date.now();
  const isPanel = (req) => req.url().endsWith("/api/panel");
  page.on("request", (req) => {
    if (!isPanel(req)) return;
    count++;
    inFlight++;
    last = Date.now();
  });
  page.on("requestfinished", (req) => {
    if (!isPanel(req)) return;
    inFlight--;
    last = Date.now();
  });
  page.on("requestfailed", (req) => {
    if (!isPanel(req)) return;
    inFlight--;
    last = Date.now();
  });
  async function quiet() {
    const deadline = Date.now() + 30000;
    for (;;) {
      if (inFlight <= 0 && Date.now() - last >= 2500) return;
      if (Date.now() > deadline) throw new Error(`timed out waiting for quiet (in-flight ${inFlight})`);
      await sleep(100);
    }
  }

  await page.goto(`http://127.0.0.1:${port}/d/vs`);
  await quiet();

  // 5. Switch to the Detail tab.
  await page.click('.tabbar > .tab-btn:text-is("Detail")');
  await quiet();

  // 6. Click three rows (rows 3, 5, 3 again), each time recording how many
  // /api/panel requests that one click sent, and checking the visible pane
  // shows the clicked series (never a stale one).
  const rows = () => page.locator(".tabpane:not([style*='none']) tr.clickable");
  const visiblePane = () => page.locator(".tabpane:not([style*='none'])");
  const counts = [];
  for (const i of [3, 5, 3]) {
    const series = (await rows().nth(i).locator("td").first().textContent()).trim();
    count = 0;
    await rows().nth(i).click();
    await quiet();
    const text = await visiblePane().first().textContent();
    check(text.includes(`Rows of ${series}`), `click row ${i} (${series}): the panel shows the clicked series (${count} requests)`);
    counts.push(count);
  }

  console.log(`clicks: ${counts.join(", ")} /api/panel requests`);
  if (expectMax != null) {
    check(counts.every((c) => c <= Number(expectMax)), `every click <= ${expectMax} requests (${counts.join(",")})`);
  }
  if (expectLast != null) {
    check(counts[counts.length - 1] <= Number(expectLast), `the last click <= ${expectLast} requests (${counts[counts.length - 1]})`);
  }
  console.log(JSON.stringify({ clicks: counts }));
} finally {
  if (srv) stop(srv);
  if (browser) await browser.close();
  rmSync(dir, { recursive: true, force: true });
}
if (!process.exitCode) console.log("varscope-e2e: ok");
