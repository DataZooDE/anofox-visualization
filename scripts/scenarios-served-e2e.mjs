// Scenarios on a real locked server (#1243 S1): anofox-scenario's branches
// attached read-only, the ::SCENARIO picker in the page (main + every
// scenario), a ?v.scenario= seed that makes unqualified names read the
// branch (picking in the page: #1271), the per-scenario rows and the diff helpers, and a hard reload on
// main. Needs a native extension build with the real UI embedded (CI's
// `web-e2e` job builds it, #590) and anofox_scenario from the community
// repository (skipped, exit 0, when it can't be installed).
//
//   node scripts/scenarios-served-e2e.mjs \
//       --ext path/to/anofox_visualization.duckdb_extension [--port 8956] [--duckdb duckdb]
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
const port = Number(arg("--port", "8956"));
const duckdb = arg("--duckdb", "duckdb");
if (!ext || !existsSync(ext)) {
  console.error("usage: scenarios-served-e2e.mjs --ext <anofox_visualization.duckdb_extension> (build it with duckext/scripts/build-native.sh)");
  process.exit(2);
}
let bad = 0;
const check = (ok, m) => (ok ? console.log("ok   " + m) : (bad++, console.error("FAIL " + m)));
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const sqlStr = (s) => `'${s.replace(/'/g, "''")}'`;
const env = { ...process.env, DATAZOO_DISABLE_TELEMETRY: "1" };

const dir = mkdtempSync(path.join(tmpdir(), "dp-scenarios-served-"));
const db = path.join(dir, "plan.duckdb");
// 1. A database with a plan and one scenario of it.
const made = spawnSync(duckdb, [db, "-c", `INSTALL anofox_scenario FROM community; LOAD anofox_scenario;
  CREATE TABLE plan (sku VARCHAR PRIMARY KEY, qty INTEGER);
  INSERT INTO plan VALUES ('A', 10), ('B', 20), ('C', 30);
  CALL scenario_create('demand_up', 'demand +10 %');
  ATTACH 'demand_up' AS s (TYPE scenario);
  UPDATE s.plan SET qty = qty + 1 WHERE sku = 'A';
  INSERT INTO s.plan VALUES ('D', 5);`], { env, encoding: "utf8" });
if (made.status !== 0) {
  console.log(`NOTE: anofox_scenario could not be installed or set up; skipped (${(made.stderr || "").split("\n")[0]})`);
  rmSync(dir, { recursive: true, force: true });
  process.exit(0);
}
const dashDir = path.join(dir, "dash");
mkdirSync(dashDir, { recursive: true });
writeFileSync(
  path.join(dashDir, "plan.sql"),
  `SELECT name AS scenario ::SCENARIO FROM anofox_scenarios();
SELECT sum(qty)::BIGINT AS q ::METRIC, 'Planned' ::LABEL FROM plan;
SELECT scenario ::XAXIS, sum(qty)::BIGINT AS "Planned" ::BARCHART FROM anofox_scenario_rows('plan') GROUP BY 1 ORDER BY 1;
SELECT sku ::TABLE, change_type, old_value, new_value FROM anofox_scenario_diff('demand_up', 'plan') ORDER BY sku;
`
);

// 2. The locked server (a DuckDB process we own; stopped by PID).
async function serve() {
  const p = spawn(duckdb, ["-unsigned", "-batch"], { stdio: ["pipe", "pipe", "pipe"], env });
  let log = "";
  p.stdout.on("data", (d) => (log += d));
  p.stderr.on("data", (d) => (log += d));
  const opts = JSON.stringify({ search_path: "warehouse", scenarios: { database: "warehouse", tables: ["plan"] } });
  p.stdin.write(`LOAD ${sqlStr(ext)};\nLOAD anofox_scenario;\nATTACH ${sqlStr(db)} AS warehouse;\nSELECT anofox_serve_dashboards(${sqlStr(dashDir)}, ${port}, ${sqlStr(opts)});\n`);
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
  const page = await browser.newPage({ viewport: { width: 1100, height: 800 }, reducedMotion: "reduce" });
  const errs = [];
  page.on("pageerror", (e) => errs.push(String(e)));
  const settled = async () => {
    await sleep(300);
    await page.waitForFunction(
      () => !document.body.classList.contains("loading") && !!document.querySelector(".metric-value"),
      null,
      { timeout: 15000 }
    );
  };
  const kpi = () => page.locator(".metric-value").first().textContent();
  const options = () => page.locator(".dash select[aria-label=scenario] option").allTextContents();

  await page.goto(`http://127.0.0.1:${port}/d/plan`);
  await settled();
  check((await kpi()) === "60", `main by default (${await kpi()})`);
  const opts = await options();
  check(opts.includes("main") && opts.includes("demand_up"), `the picker lists main and the scenario (${opts})`);
  const grid = await page.locator(".grid").first().textContent();
  check(/demand_up/.test(grid) && /Dadded|D\s+added/.test(grid) && /modified/.test(grid), `rows per scenario and the diff (D added) are drawn (${grid.slice(0, 300)})`);

  // Picking in the page doesn't re-run the panels yet (#1271): the seed below
  // is what reads the branch.

  await page.goto(`http://127.0.0.1:${port}/d/plan?v.scenario=demand_up`);
  await settled();
  await page.waitForFunction(() => document.querySelector(".metric-value")?.textContent === "66", null, { timeout: 15000 }).catch(() => {});
  check((await kpi()) === "66", `a ?v.scenario= seed reads the branch on a hard reload (${await kpi()})`);

  await page.goto(`http://127.0.0.1:${port}/d/plan?v.scenario=${encodeURIComponent("x'; DROP TABLE plan; --")}`);
  await settled();
  check((await kpi()) === "60", `an unknown value reads main (${await kpi()})`);

  check(errs.length === 0, `no page errors${errs.length ? ": " + errs.join("; ") : ""}`);
} finally {
  if (srv) stop(srv);
  if (browser) await browser.close();
  rmSync(dir, { recursive: true, force: true });
}
console.log(`scenarios-served-e2e: ${bad} failed`);
if (bad) process.exit(1);
