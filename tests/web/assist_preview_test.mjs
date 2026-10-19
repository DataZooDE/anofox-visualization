// #244, C-1: the preview's render loop lives in ONE guarded engine session
// (web/lint.js's scratchSession, web/assist.js's wasmTools().previewSession),
// never on the page's own DuckDB. This is the dedicated, exhaustive test of
// that session: SET VARIABLEs/TEMP objects carry across calls (D1), the
// guard still refuses a class-3 statement mid-session, the lock re-check
// still rebuilds, and — H-1 for the render loop specifically — the page's
// own connection sees *nothing* while a preview runs. Node's blocking
// DuckDB-Wasm throughout (no document, no CSP to prove: buildEngine is
// called with `doc: undefined`), same setup as tests/web/assist_engine_test.mjs.
import { createRequire } from "module";
import { existsSync, readFileSync } from "fs";
import path from "path";
import assert from "assert/strict";
import * as A from "../../web/assist.js";
import { rowsJson, scratchSession, splitStatements } from "../../web/lint.js";
import { buildEngine, engineLocked, pageAdapter } from "../../web/assist-engine.js";

const here = path.dirname(new URL(import.meta.url).pathname);
const root = path.resolve(here, "../..");
const require = createRequire(path.join(here, "package.json"));

let duckdb = null;
try {
  duckdb = require("@duckdb/duckdb-wasm/dist/duckdb-node-blocking.cjs");
} catch (_) {}
const wasmPkg = path.join(root, "web/pkg");
const haveWasm = existsSync(path.join(wasmPkg, "anofox_visualization_bg.wasm"));
if (!duckdb || !haveWasm) {
  // No quiet skip (CI rule: never print "skip") — this test's whole point is
  // the preview session, so a missing web/pkg or DuckDB-Wasm install is a
  // hard failure.
  console.error("assist_preview_test: FAILED — needs web/pkg (wasm-pack build) and npm install --prefix tests/web");
  process.exit(1);
}
const wasm = await import(path.join(wasmPkg, "anofox_visualization.js"));
wasm.initSync({ module: readFileSync(path.join(wasmPkg, "anofox_visualization_bg.wasm")) });

async function rejects(fn, re, msg) {
  let threw = false,
    text = "";
  try {
    await fn();
  } catch (e) {
    threw = true;
    text = String(e?.message || e);
  }
  assert.ok(threw, msg || "expected to reject");
  if (re) assert.match(text, re, msg || text);
}

const dist = path.dirname(require.resolve("@duckdb/duckdb-wasm/dist/duckdb-node-blocking.cjs"));
async function rawDb() {
  const d = await duckdb.createDuckDB({ eh: { mainModule: path.join(dist, "duckdb-eh.wasm"), mainWorker: path.join(dist, "duckdb-node-eh.worker.cjs") } }, new duckdb.VoidLogger(), duckdb.NODE_RUNTIME);
  await d.instantiate();
  return d;
}

// ---- the page: sales-setup.sql's table ----
const FIX = path.join(root, "scripts/e2e-fixtures/assist");
const SALES_SETUP = readFileSync(path.join(FIX, "sales-setup.sql"), "utf8");
const CLEAN = readFileSync(path.join(FIX, "proposal-clean.sql"), "utf8");

const pageDb = await rawDb();
pageDb.open({});
const pageMainConn = pageDb.connect();
pageMainConn.query("INSTALL json; LOAD json;");
for (const s of splitStatements(SALES_SETUP)) pageMainConn.query(s);

const snapshotSeen = [];
const spyPageDb = {
  connect: async () => {
    const c = pageDb.connect();
    return new Proxy(c, {
      get: (o, k) => (k === "query" || k === "send" || k === "prepare" ? (s) => (snapshotSeen.push(String(s)), o[k](s)) : typeof o[k] === "function" ? o[k].bind(o) : o[k]),
    });
  },
  copyFileToBuffer: (n) => pageDb.copyFileToBuffer(n),
};

const engine = await buildEngine({
  makeDb: rawDb,
  page: pageAdapter(spyPageDb, { handles: new Map() }),
  extensions: ["json", "parquet", "icu"],
  files: [],
  doc: undefined,
});
assert.ok(await engineLocked(engine.db), "the engine is locked");
snapshotSeen.length = 0; // reset: only the preview session below is spied on

const tools = A.wasmTools({
  db: engine.db,
  lintStep: wasm.lint_step,
  plan: wasm.plan,
  pageSql: wasm.page_sql,
  snapshot: new Map(engine.tables.map((t) => [t.fq, t])),
});

// ---- 1. SET VARIABLE / TEMP across calls (D1) ----
{
  const s = tools.previewSession(CLEAN);
  await s.run("SET VARIABLE region = 'EU'");
  const r1 = JSON.parse(await s.run("SELECT getvariable('region') AS r"));
  assert.deepEqual(r1, [{ r: "EU" }], "a variable set in an earlier call is visible in a later one");
  await s.run("CREATE OR REPLACE TEMP TABLE t AS SELECT 1 AS x");
  const r2 = JSON.parse(await s.run("SELECT * FROM t"));
  assert.deepEqual(r2, [{ x: 1 }], "a temp table made in an earlier call is usable in a later one");
  await s.close();
}

// ---- 2. the guard still refuses a class-3 statement mid-session ----
{
  const s = tools.previewSession(CLEAN);
  await rejects(() => s.run("SELECT * FROM read_csv('https://example.invalid/x.csv')"), /not run/, "a remote read is refused");
  const ok = JSON.parse(await s.run("SELECT 1 AS a"));
  assert.deepEqual(ok, [{ a: 1 }], "the session keeps working after a refusal");
  await s.close();
}

// ---- 3. a temp macro from call 1 is usable in call 3 (guard state persists) ----
{
  const s = tools.previewSession(CLEAN);
  await s.run("CREATE OR REPLACE TEMP MACRO dbl(x) AS x * 2");
  await s.run("SELECT 1 AS a");
  const r = JSON.parse(await s.run("SELECT dbl(21) AS y"));
  assert.deepEqual(r, [{ y: 42 }], "a temp macro made earlier is callable later");
  await s.close();
}

// ---- 4 + 5. the render sequence equals a plain page connection's, and the
// page's own connection sees zero activity while it runs in the session ----
function isInputStmt(p) {
  const INPUT = new Set(["DROPDOWN", "MULTISELECT", "NUMBER", "DATE", "TEXT", "DATERANGE", "PERIOD", "BUTTONS", "TOGGLE"]);
  return !p.setup && (p.roles || []).some((r) => INPUT.has(r[1]));
}
// A minimal render pass over `run(sql)`: setup statements for effect, input
// statements seed their variable from the first option, every other
// non-chrome statement runs for its rows. Collects every result in order.
async function renderOnce(run, planned) {
  const CHROME = new Set(["THEME", "LOCALE", "DASHBOARD_TITLE", "BRAND", "LOGO", "FAVICON", "ASOF", "TAB", "GROUP", "ENDGROUP", "SPAN", "DRILL"]);
  const isChrome = (p) => !p.setup && (p.roles || []).every((r) => CHROME.has(r[1]));
  const out = [];
  for (const p of planned) {
    if (p.setup) {
      out.push(await run(p.sql));
      continue;
    }
    if (isChrome(p)) {
      out.push(await run(p.sql));
      continue;
    }
    if (isInputStmt(p)) {
      const rows = JSON.parse(await run(p.sql));
      out.push(JSON.stringify(rows));
      if (rows.length) {
        const varname = Object.keys(rows[0])[0];
        const first = String(rows[0][varname]);
        await run(`SET VARIABLE ${varname} = '${first.replace(/'/g, "''")}'`);
      }
      continue;
    }
    out.push(await run(p.sql));
  }
  return out;
}

{
  const planned = JSON.parse(wasm.plan(CLEAN));
  const s = tools.previewSession(CLEAN);
  const sessionRun = (sql) => s.run(sql);
  const sessionResults = await renderOnce(sessionRun, planned);
  await s.close();

  // the same sequence against a scratch connection straight on the page —
  // no gate, no lock: just proving the *results* match, item 4's point.
  const pageConn = pageDb.connect();
  const home = String(pageConn.query("SELECT current_database() AS d").toArray()[0].toJSON().d);
  pageConn.query(`ATTACH ':memory:' AS cmp; USE cmp; SET search_path = 'cmp.main,${home}.main';`);
  const pageRun = async (sql) => rowsJson(pageConn.query(sql));
  const pageResults = await renderOnce(pageRun, planned);
  assert.deepEqual(sessionResults.map((x) => JSON.parse(x || "[]")), pageResults.map((x) => JSON.parse(x || "[]")), "the preview session's render results equal a plain connection's");
  pageConn.close();
}

assert.deepEqual(snapshotSeen, [], "zero page activity during the render sequence and the earlier guard/TEMP calls (H-1/C-1)");

// ---- 6. concurrency: calls resolve in call order ----
{
  const s = tools.previewSession(CLEAN);
  const order = [];
  await Promise.all(
    [1, 2, 3, 4, 5, 6].map((n) =>
      s.run(`SELECT ${n} AS n`).then((r) => order.push(JSON.parse(r)[0].n))
    )
  );
  assert.deepEqual(order, [1, 2, 3, 4, 5, 6], "serialised: calls settle in the order they were made");
  await s.close();
}

// ---- 7. timeout: a session with a short checkQueryMs rejects a query past
// its budget, then keeps working. Node's *blocking* DuckDB-Wasm driver runs
// a query synchronously (the same Node-harness limitation
// tests/web/assist_engine_test.mjs documents for a stuck extension load):
// the event loop can't fire a timer until a real heavy query returns
// control, so — as tests/web/assist_test.mjs's own "M1" timeout test does —
// a thin proxy around a real connection stands in for a query that never
// answers, to prove the race and the cancelSent() call deterministically.
{
  let cancelCalls = 0;
  const hangDb = {
    connect: async () => {
      const real = await engine.db.connect();
      return new Proxy(real, {
        get: (o, k) => {
          if (k === "send") return (sql) => (String(sql).trim() === "SELECT 1 AS heavy_marker" ? new Promise(() => {}) : o.send(sql));
          if (k === "cancelSent") return async (...a) => (cancelCalls++, o.cancelSent ? o.cancelSent(...a) : true);
          return typeof o[k] === "function" ? o[k].bind(o) : o[k];
        },
      });
    },
  };
  const shortTools = A.wasmTools({
    db: hangDb,
    lintStep: wasm.lint_step,
    plan: wasm.plan,
    pageSql: wasm.page_sql,
    snapshot: new Map(engine.tables.map((t) => [t.fq, t])),
    checkQueryMs: 200,
  });
  const s = shortTools.previewSession(CLEAN);
  await rejects(() => s.run("SELECT 1 AS heavy_marker"), /took longer/, "a query past its budget is cancelled");
  assert.ok(cancelCalls >= 1, "cancelSent() was called");
  const ok = JSON.parse(await s.run("SELECT 1 AS a"));
  assert.deepEqual(ok, [{ a: 1 }], "the session works again after a timeout");
  await s.close();
}

// ---- 8. the lock re-check: a state change on an engine connection makes the
// next session call reject and name a rebuild ----
{
  const e2 = await buildEngine({
    makeDb: rawDb,
    page: pageAdapter(spyPageDb, { handles: new Map() }),
    extensions: ["json"],
    files: [],
    doc: undefined,
  });
  const t2 = A.wasmTools({ db: e2.db, lintStep: wasm.lint_step, plan: wasm.plan, pageSql: wasm.page_sql, snapshot: new Map(e2.tables.map((t) => [t.fq, t])) });
  const s = t2.previewSession("SELECT 1 ::METRIC;");
  const c = await e2.db.connect();
  await c.query("CALL system.main.enable_logging(storage='memory')");
  await c.close();
  await rejects(() => s.run("SELECT 1 AS a"), /changed state; rebuilding/);
  e2.close();
}

// ---- 9. an unlocked (raw page) db is refused outright ----
{
  const unlockedTools = A.wasmTools({ db: pageDb, lintStep: wasm.lint_step, plan: wasm.plan, pageSql: wasm.page_sql });
  const s = unlockedTools.previewSession(CLEAN);
  await rejects(() => s.run("SELECT 1"), /changed state|locked/);
}

// ---- 10. close(): further calls reject; no anofox_lint_* database left ----
{
  const s = tools.previewSession(CLEAN);
  await s.run("SELECT 1 AS a");
  await s.close();
  await rejects(() => s.run("SELECT 1"), /closed/);
  const leftConn = await engine.db.connect();
  const left = (await leftConn.query("SELECT count(*) AS n FROM duckdb_databases() WHERE database_name LIKE 'anofox_lint_%'")).toArray().map((r) => r.toJSON());
  await leftConn.close();
  assert.equal(Number(left[0].n), 0, "no anofox_lint_* scratch database left on the engine");
}

// ---- 11. scratchSession has no `prepare` option ----
{
  const src = readFileSync(path.join(root, "web/lint.js"), "utf8");
  const m = /export function scratchSession\(db, opts = \{\}\) \{\s*const \{([^}]*)\} = ownOpts\(opts\);/.exec(src);
  assert.ok(m, "scratchSession's destructure is findable");
  assert.doesNotMatch(m[1], /\bprepare\b/, "scratchSession takes no prepare option (#244 §6.1)");
}

engine.close();
console.log("assist_preview_test: ok");
