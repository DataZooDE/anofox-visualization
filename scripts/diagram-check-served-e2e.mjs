// ::DIAGRAM_CHECK (#699) on a real locked server (anofox_serve_dashboards):
// the check is one more declared statement — the server runs it on the
// edited graph (one string variable) and the page colours / lists what it
// returns; after an edit (Remove) it runs again. The viewer sends no SQL.
// Needs a native extension build with the real UI embedded (CI's `web-e2e`
// job builds it, #590).
//
//   node scripts/diagram-check-served-e2e.mjs \
//       --ext path/to/anofox_visualization.duckdb_extension [--port 8955] [--duckdb duckdb]
import { createRequire } from "module";
import { spawn } from "child_process";
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
const port = Number(arg("--port", "8955"));
const duckdb = arg("--duckdb", "duckdb");
if (!ext || !existsSync(ext)) {
  console.error("usage: diagram-check-served-e2e.mjs --ext <anofox_visualization.duckdb_extension> (build it with duckext/scripts/build-native.sh)");
  process.exit(2);
}
let bad = 0;
const check = (ok, m) => (ok ? console.log("ok   " + m) : (bad++, console.error("FAIL " + m)));
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const sqlStr = (s) => `'${s.replace(/'/g, "''")}'`;
const env = { ...process.env, DATAZOO_DISABLE_TELEMETRY: "1" };

// 1. The dashboard: an editable diagram and its check.
const dir = mkdtempSync(path.join(tmpdir(), "dp-diagram-check-served-"));
const dashDir = path.join(dir, "dash");
mkdirSync(dashDir, { recursive: true });
writeFileSync(
  path.join(dashDir, "net.sql"),
  `CREATE OR REPLACE TEMP TABLE network_node AS SELECT * FROM (VALUES
 ('sup', 'Supplier', 'supplier', NULL), ('wh', 'Warehouse', 'stock', ['sup']),
 ('orphan', 'Orphan', 'stock', NULL)) v(id, name, kind, upstream);
SELECT n->>'id' AS id ::DIAGRAM_CHECK, 'bad' AS status, 'not connected to anything' AS message
FROM (SELECT unnest((getvariable('graph')->'nodes')::JSON[]) AS n)
WHERE n->>'id' NOT IN (SELECT e->>'from' FROM (SELECT unnest((getvariable('graph')->'edges')::JSON[]) AS e)
                       UNION SELECT e->>'to' FROM (SELECT unnest((getvariable('graph')->'edges')::JSON[]) AS e));
SELECT id ::DIAGRAM, name ::LABEL, kind ::CATEGORY, upstream ::UPSTREAM, 'edit' AS graph ::DIAGRAM_MODE, 'Network' ::TITLE FROM network_node;
`
);

// 2. Start the locked server (a DuckDB process we own; stopped by PID).
async function serve() {
  const p = spawn(duckdb, ["-unsigned", "-batch"], { stdio: ["pipe", "pipe", "pipe"], env });
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
  const page = await browser.newPage({ viewport: { width: 900, height: 700 }, reducedMotion: "reduce" });
  const errs = [];
  page.on("pageerror", (e) => errs.push(String(e)));
  const settled = async () => {
    await sleep(300);
    await page.waitForFunction(() => !document.body.classList.contains("loading") && !!document.querySelector(".dp-edit-bar"), null, { timeout: 15000 });
  };
  const st = () =>
    page.evaluate(() => ({
      items: [...document.querySelectorAll(".dp-edit-checks li")].map((li) => li.textContent),
      orphan: document.querySelector('[data-node="orphan"][data-status]')?.getAttribute("data-status") ?? null,
    }));

  await page.goto(`http://127.0.0.1:${port}/d/net`);
  await settled();
  let s = await st();
  check(s.orphan === "bad" && s.items.length === 1 && /Orphan: not connected/.test(s.items[0]), `served: the server runs the check (${JSON.stringify(s)})`);
  await page.click(".dp-edit-checks .dp-check-at");
  await page.click('.dp-edit-bar > button:text-is("Remove")');
  await settled();
  await page.waitForFunction(() => !document.querySelector(".dp-edit-checks"), null, { timeout: 15000 }).catch(() => {});
  s = await st();
  check(s.orphan === null && s.items.length === 0, `served: the check runs again on the edited graph (${JSON.stringify(s)})`);

  check(errs.length === 0, `served: no page errors${errs.length ? ": " + errs.join("; ") : ""}`);
} finally {
  if (srv) stop(srv);
  if (browser) await browser.close();
  rmSync(dir, { recursive: true, force: true });
}
console.log(`diagram-check-served-e2e: ${bad} failed`);
if (bad) process.exit(1);
