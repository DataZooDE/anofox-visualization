// The assistant's locked DuckDB-Wasm engine (#204, web/assist-engine.js):
// the lock holds (plan-204 §1.2), the snapshot carries the page's own data
// with its types intact and nothing executable, the page is touched only
// through pageAdapter's closed set of fixed statements, and — H-1 — the
// page sees *nothing* of a tool-use loop or a proposal preview run against
// the locked engine through web/assist.js's wasmTools. Node's blocking
// DuckDB-Wasm throughout (no document, no CSP to prove: buildEngine is
// called with `doc: undefined`).
import { createRequire } from "module";
import { existsSync, mkdtempSync, readFileSync, rmSync } from "fs";
import os from "os";
import path from "path";
import assert from "assert/strict";
import * as E from "../../web/assist-engine.js";
import * as A from "../../web/assist.js";

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
  // No quiet skip: this test's whole point is the locked engine, so a
  // missing web/pkg or DuckDB-Wasm install is a hard failure, not a note.
  console.error("assist_engine_test: FAILED — needs web/pkg (wasm-pack build) and npm install --prefix tests/web");
  process.exit(1);
}
const wasm = await import(path.join(wasmPkg, "anofox_visualization.js"));
wasm.initSync({ module: readFileSync(path.join(wasmPkg, "anofox_visualization_bg.wasm")) });

// A plain try/catch, not assert.rejects: a few of these ultimately throw a
// raw wasm-boundary exception that assert.rejects' own promise wrapping
// does not reliably survive in the Node blocking driver. `fn` is a thunk
// (deferred invocation), so a synchronous throw is caught the same way an
// async rejection would be.
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

// ---- the page: sales/customers, a view, an attached db, the type matrix,
// hostile names, two registered files ----
const pageDb = await rawDb();
pageDb.open({});
const pageMainConn = pageDb.connect(); // never touched by the engine build (spy #2)
pageMainConn.query("INSTALL json; LOAD json; INSTALL icu; LOAD icu;");
pageMainConn.query("CREATE TABLE sales AS SELECT i AS id, ['EU','US','APAC'][1+i%3] AS region, (DATE '2026-01-01' + INTERVAL (i) DAY)::DATE AS day, (i*1.5)::DOUBLE AS amount FROM range(0, 600) t(i)");
pageMainConn.query("CREATE TABLE customers AS SELECT 'alice@x.de' AS email UNION ALL SELECT 'bob@y.de'");
pageMainConn.query("CREATE VIEW v AS SELECT * FROM sales WHERE amount > 0");
pageMainConn.query("CREATE SCHEMA s2; CREATE TABLE s2.t2 AS SELECT 1 AS x;");
pageMainConn.query("ATTACH ':memory:' AS other; CREATE TABLE other.main.t3 AS SELECT 1 AS x;");
pageMainConn.query(
  "CREATE TABLE matrix AS SELECT " +
    "1::INTEGER AS i_, 1::BIGINT AS bi, 170141183460469231731687303715884105727::HUGEINT AS hi, " +
    "2.5::DOUBLE AS d, 2.5::FLOAT AS f, 12.345::DECIMAL(18,3) AS dc, 200::UTINYINT AS ut, " +
    "NULL::VARCHAR AS v, DATE '2026-01-01' AS dt, TIME '01:02:03' AS tm, TIMESTAMP '2026-01-01 00:00:00' AS ts, " +
    "TIMESTAMPTZ '2026-01-01 00:00:00+00' AS tstz, [1,2,3] AS lst, [1,2]::INTEGER[2] AS fixedarr, " +
    "{'a': NULL, 'b': 1} AS strct, map([1],['x']) AS mp, uuid() AS u, 'abc'::BLOB AS bl, '101'::BIT AS bitcol, " +
    "INTERVAL '1 day' AS iv"
);
// I-6: hostile ENUM label and STRUCT field name — DuckDB escapes them in
// DESCRIBE output; the engine must get exactly this table and nothing else.
pageMainConn.query("CREATE TYPE hostile_enum AS ENUM ('a'')); CREATE MACRO pwn() AS 1; --', 'b')");
pageMainConn.query('CREATE TABLE hostile (e hostile_enum, s STRUCT("q"")) ; DROP TABLE x; --" VARCHAR))');
pageMainConn.query("INSERT INTO hostile VALUES ('a'')); CREATE MACRO pwn() AS 1; --', ROW('v'))");
pageDb.registerFileText("data/x/y.csv", "a,b\n1,2\n");
pageDb.registerFileText("other.csv", "c\n3\n");

// A spy wrapping one dedicated page connection (what pageAdapter opens) and
// a second, independent spy on the page's *main* connection (pageMainConn,
// used by the loop test below) — both must see nothing during the loop and
// the preview (H-1).
const snapshotSeen = [];
const loopSeen = [];
const handles = new Map();
const spyPageDb = {
  connect: async () => {
    const c = pageDb.connect();
    return new Proxy(c, {
      get: (o, k) => (k === "query" || k === "send" || k === "prepare" ? (s) => (snapshotSeen.push(String(s)), o[k](s)) : typeof o[k] === "function" ? o[k].bind(o) : o[k]),
    });
  },
  copyFileToBuffer: (n) => pageDb.copyFileToBuffer(n),
};

const engine = await E.buildEngine({
  makeDb: rawDb,
  page: E.pageAdapter(spyPageDb, { handles }),
  // anofox_forecast/anofox_statistics are deliberately left out of this
  // build: DuckDB-Wasm's Node *blocking* driver hangs (not fails) trying to
  // LOAD a custom extension from a file:// custom_extension_repository —
  // a Node-test-harness artifact (the main thread never yields, so even a
  // bounded race never fires), not a #204 design issue: the browser's
  // AsyncDuckDB runs the engine in its own Worker, where a stuck LOAD can't
  // block the main thread, and LOCALEXT there is always an http(s):// URL.
  // Item 10's "extension not loaded -> clean error" is covered below with
  // `spatial`/`st_astext`, which needs no network fetch to prove the point.
  extensions: ["json", "parquet", "icu"],
  files: ["data/x/y.csv", "other.csv"],
  limits: { ...E.SNAPSHOT_LIMITS, tableRows: 100 },
  doc: undefined,
});

// ---- 1. locked ----
assert.equal(await E.engineLocked(engine.db), true, "the engine is locked");
{
  // L-1: a throwaway engine — corrupting the lock state must not leak into
  // the shared `engine` the rest of this file uses.
  const e1 = await E.buildEngine({ makeDb: rawDb, page: E.pageAdapter(spyPageDb, {}), extensions: ["json"], doc: undefined });
  const c = await e1.db.connect();
  await c.query("CALL system.main.enable_logging(storage='memory')");
  assert.equal(await E.engineLocked(e1.db), false, "enable_logging escapes lock_configuration (L-1) — caught by the re-check");
  await c.close();
  e1.close();
}

// ---- 2. the lock: a representative subset of plan-204 §1.2 (fresh engine) ----
// sec-review-256 L-1: run from a scratch cwd with no `data/` directory, so
// a normalised spelling that happens to fall through to the host
// filesystem can't silently find a same-named file there and hide the bug
// the way the repo's own cwd (which has no top-level data/ either, but a
// reviewer's or a future test's cwd might) would.
const prevCwd = process.cwd();
const noDataCwd = mkdtempSync(path.join(os.tmpdir(), "assist-engine-nodata-"));
try {
  process.chdir(noDataCwd);
  const e2 = await E.buildEngine({ makeDb: rawDb, page: E.pageAdapter(spyPageDb, {}), extensions: ["json"], files: ["data/x/y.csv"], doc: undefined });
  const c = await e2.db.connect();
  const throws = (sql, re, msg) => rejects(() => c.query(sql), re, msg || sql);
  const ok = async (sql, msg) => {
    await c.query(sql);
  };
  const rowsOf = async (sql) => (await c.query(sql)).toArray().map((r) => r.toJSON());
  await throws("SELECT * FROM read_csv('http://127.0.0.1:1/x.csv')", /Permission|external access/i, "URL reader");
  await throws("SELECT * FROM glob('*')", /Permission|external access/i, "glob");
  await throws("SELECT * FROM glob('data/x/*')", /Permission|external access/i, "glob of a dir");
  await throws("SELECT * FROM read_text('/etc/hostname')", /Permission|external access/i, "absolute path");
  await throws("SELECT * FROM read_text('other.csv')", /Permission|external access/i, "a registered file not allow-listed");
  await throws("SELECT * FROM read_text('DATA/X/Y.CSV')", /Permission|external access/i, "an allowed file, wrong case (still refused)");
  // sec-review-256 L-1 (correcting the earlier, wrong claim): DuckDB's
  // *permission check* normalises './x', 'a/../a', a doubled '/' and a
  // trailing '/' before comparing against allowed_paths, so the check
  // itself passes them — but the file is then opened by its **raw**,
  // unnormalised spelling, which is not a name DuckDB-Wasm's registry
  // knows (only the exact string 'data/x/y.csv' is registered). So these
  // never read the registered bytes. They read nothing at all here
  // (Chromium: confirmed the same way; zero rows, no request recorded).
  // With a `data/x/y.csv` file actually sitting on the *host* disk at the
  // raw spelling's resolved path, the Node build's DuckDB falls through to
  // the real filesystem and reads (or, via COPY, overwrites) that file —
  // which is exactly why this test runs from a cwd with no `data/` at all.
  const exact = await rowsOf("SELECT * FROM read_text('data/x/y.csv')");
  assert.equal(exact.length, 1, "the exact allowed name reads one row");
  assert.match(String(exact[0].content ?? exact[0]), /a,b/, "… and it is the registered bytes");
  for (const spelling of ["./data/x/y.csv", "data/x/../x/y.csv", "data//x/y.csv", "data/x/y.csv/"]) {
    const got = await rowsOf(`SELECT * FROM read_text('${spelling}')`);
    assert.equal(got.length, 0, `a normalised spelling must not read the registered file's bytes: ${spelling}`);
  }
  // COPY ... (USE_TMP_FILE false) is a write to an allowed path: the gate
  // (web/assist.js's classifier) already denies COPY outright as class 3,
  // so the model can never reach this — but run directly on the engine
  // connection (as this vector table does, with no gate), it succeeds and
  // overwrites the engine's own *registered buffer*, not anything on the
  // page or the host.
  await ok("COPY (SELECT 'W2' AS w) TO 'data/x/y.csv' (FORMAT csv, USE_TMP_FILE false)", "COPY to an allowed path overwrites the engine's own copy");
  const afterCopy = await rowsOf("SELECT * FROM read_text('data/x/y.csv')");
  assert.match(String(afterCopy[0].content ?? afterCopy[0]), /W2/, "the engine's own buffer changed");
  assert.equal(new TextDecoder().decode(await pageDb.copyFileToBuffer("data/x/y.csv")), "a,b\n1,2\n", "the page's own buffer is unchanged");
  await throws("COPY (SELECT 1) TO 'new.csv'", /Permission|external access/i, "COPY to a new file");
  await throws("ATTACH 'new.duckdb'", /disabled|Permission/i, "ATTACH a new file");
  await throws("ATTACH 'md:'", /disabled|external/i, "ATTACH md:");
  await throws("CREATE SECRET s (TYPE s3)", /Permission|disabled/i, "CREATE SECRET");
  await throws("LOAD inet", /disabled|not loaded|Permission/i, "LOAD a never-loaded extension");
  await ok("LOAD json", "LOAD of an already-loaded extension is a no-op");
  await ok("INSTALL httpfs", "INSTALL alone makes no request and is not refused");
  await throws("LOAD httpfs", /disabled|not loaded|Permission/i, "the LOAD that follows INSTALL is still refused");
  await ok("ATTACH ':memory:' AS zz; DETACH zz", "ATTACH :memory: is harmless");
  for (const s of ["SET enable_external_access=true", "SET lock_configuration=false", "SET allowed_paths=['/etc']", "PRAGMA enable_external_access=true"])
    await throws(s, /lock|Permission|Cannot enable/i, s);
  await ok("PRAGMA enable_profiling; PRAGMA disable_profiling", "the bare PRAGMA enable_profiling is allowed (it just prints to the worker console)");
  await throws("PRAGMA enable_profiling='no_output'", /locked|Cannot change/i, "the =value form is a setting, and is locked");
  await c.close();
  e2.close();
} finally {
  process.chdir(prevCwd);
  rmSync(noDataCwd, { recursive: true, force: true });
}

// ---- 3. the snapshot: names, counts, types, hostile names ----
{
  const fqSales = `"memory"."main"."sales"`;
  const fqCustomers = `"memory"."main"."customers"`;
  const fqView = `"memory"."main"."v"`;
  const fqS2 = `"memory"."s2"."t2"`;
  const fqOther = `"other"."main"."t3"`;
  const fqMatrix = `"memory"."main"."matrix"`;
  const byFq = new Map(engine.tables.map((t) => [t.fq, t]));
  assert.ok(byFq.has(fqSales), JSON.stringify(engine.tables));
  assert.ok(byFq.has(fqCustomers));
  assert.ok(byFq.has(fqView));
  assert.ok(byFq.has(fqS2));
  assert.ok(byFq.has(fqOther));
  assert.equal(byFq.get(fqView).kind, "view");
  assert.equal(byFq.get(fqCustomers).rows, 2);
  // tableRows: 100 forces sampling of the 600-row sales table
  assert.equal(byFq.get(fqSales).sampled, true);
  assert.equal(byFq.get(fqSales).rows, 100);

  const c = await engine.db.connect();
  const count = async (fq) => Number((await c.query(`SELECT count(*) AS n FROM ${fq}`)).toArray()[0].n);
  assert.equal(await count(fqCustomers), 2);
  assert.equal(await count(fqMatrix), 1);
  const types = (await c.query(`DESCRIBE SELECT * FROM ${fqMatrix}`)).toArray().map((r) => r.toJSON());
  const byCol = Object.fromEntries(types.map((t) => [t.column_name, t.column_type]));
  assert.match(byCol.hi, /HUGEINT/);
  assert.match(byCol.lst, /\[\]$/);
  assert.match(byCol.fixedarr, /\[2\]$/);
  assert.match(byCol.strct, /STRUCT/);
  assert.match(byCol.mp, /MAP/);
  assert.match(byCol.bitcol, /BIT/);
  assert.match(byCol.iv, /INTERVAL/);

  // hostile names: nothing extra was created
  const fns = (await c.query("SELECT DISTINCT function_name AS n FROM duckdb_functions() WHERE NOT internal")).toArray().map((r) => r.n);
  assert.deepEqual(fns, ["enable_logging"], "only the LOGGING_SHADOW macro; no pwn()");
  const tbls = (await c.query("SELECT database_name||'.'||schema_name||'.'||table_name AS t FROM duckdb_tables() WHERE NOT internal ORDER BY 1")).toArray().map((r) => r.t);
  assert.ok(tbls.includes("memory.main.hostile"), JSON.stringify(tbls));
  assert.ok(!tbls.some((t) => /\bx\b/.test(t) && t !== "memory.main.matrix"), "no DROP TABLE x ran, and nothing named x exists");
  await c.close();
}

// ---- 4. caps (L-2, plan-204 §5.1 item 4; sec-review-256 M-2) ----
{
  // totalRows: the budget runs out after the first 10-row table, so the
  // rest are excluded — not sampled, not partially copied.
  const capPage = await rawDb();
  capPage.open({});
  const cp = capPage.connect();
  cp.query("CREATE TABLE a10 AS SELECT i FROM range(10) t(i)");
  cp.query("CREATE TABLE b10 AS SELECT i FROM range(10) t(i)");
  cp.query("CREATE TABLE c10 AS SELECT i FROM range(10) t(i)");
  const eRows = await E.buildEngine({ makeDb: rawDb, page: E.pageAdapter(capPage, {}), extensions: ["json"], limits: { ...E.SNAPSHOT_LIMITS, totalRows: 10 }, doc: undefined });
  const byRows = Object.fromEntries(eRows.tables.map((t) => [t.fq, t]));
  assert.equal(byRows['"memory"."main"."a10"'].rows, 10);
  assert.equal(byRows['"memory"."main"."a10"'].excluded, undefined);
  assert.equal(byRows['"memory"."main"."b10"'].excluded, "too large", "totalRows ran out after a10");
  assert.equal(byRows['"memory"."main"."c10"'].excluded, "too large");
  eRows.close();

  // totalBytes: a tiny budget excludes even the first table.
  const eBytes = await E.buildEngine({ makeDb: rawDb, page: E.pageAdapter(capPage, {}), extensions: ["json"], limits: { ...E.SNAPSHOT_LIMITS, totalBytes: 8 }, doc: undefined });
  const byBytes = Object.fromEntries(eBytes.tables.map((t) => [t.fq, t]));
  assert.equal(byBytes['"memory"."main"."a10"'].excluded, "too large");
  eBytes.close();
  capPage.terminate?.();

  // Remote database exclusion: a MotherDuck-typed database injected
  // alongside the real page's own — its table is excluded, and `rows()`
  // (the expensive part) is never called for it. The real tables still
  // snapshot normally through the same, otherwise-unmodified pageAdapter.
  {
    const realPage = E.pageAdapter(spyPageDb, { handles });
    let cloudRowsCalls = 0;
    const withCloud = {
      ...realPage,
      async databases(ms) {
        return [...(await realPage.databases(ms)), { database_name: "cloud", path: "md:mydb", type: "motherduck" }];
      },
      async catalog(ms) {
        return [...(await realPage.catalog(ms)), { database_name: "cloud", schema_name: "main", table_name: "secret", kind: "table", n: 1, cols: 1 }];
      },
      async rows(fq, kind, n, ms) {
        if (fq.includes("cloud")) cloudRowsCalls++;
        return realPage.rows(fq, kind, n, ms);
      },
    };
    const eCloud = await E.buildEngine({ makeDb: rawDb, page: withCloud, extensions: ["json"], doc: undefined });
    const byFq = Object.fromEntries(eCloud.tables.map((t) => [t.fq, t]));
    assert.equal(byFq['"cloud"."main"."secret"']?.excluded, "remote database");
    assert.equal(cloudRowsCalls, 0, "rows() is never called for an excluded remote database");
    assert.ok(byFq['"memory"."main"."sales"'] && !byFq['"memory"."main"."sales"'].excluded, "the real tables still snapshot normally alongside it");
    eCloud.close();
  }

  // The timeout: a fake connection whose send() yields batches slowly (a
  // real async delay, not a microtask, the same reason M-1's unit test
  // needs one) — pageAdapter.rows() must reject around `ms`, call
  // cancelSent(), and never wait for the rest of the batches.
  {
    class FakeTable {
      constructor(batches) {
        this.batches = batches;
        this.numRows = batches.length;
      }
      toArray() {
        return [];
      }
      slice() {
        return this;
      }
    }
    function fakeSlowConn() {
      let cancelled = false;
      let cancelCalls = 0;
      return {
        query: async () => ({ constructor: FakeTable, toArray: () => [] }), // the tableCtor() probe only
        send: async () => ({
          [Symbol.asyncIterator]() {
            let yielded = 0;
            return {
              async next() {
                if (cancelled || yielded++ >= 500) return { done: true, value: undefined };
                await new Promise((r) => setTimeout(r, 50));
                return { done: false, value: { data: { byteLength: 10 } } };
              },
            };
          },
        }),
        cancelSent: async () => {
          cancelled = true;
          cancelCalls++;
          return true;
        },
        get cancelCalls() {
          return cancelCalls;
        },
        close: async () => {},
        prepare: async () => ({ close: async () => {} }),
      };
    }
    const conn = fakeSlowConn();
    const fakeDb = { connect: async () => conn, copyFileToBuffer: async () => new Uint8Array() };
    const slowPage = E.pageAdapter(fakeDb, {});
    const t0 = Date.now();
    await rejects(() => slowPage.rows('"memory"."main"."heavy"', "view", 10, 200), /took longer than 0\.2 s and was cancelled/);
    const dt = Date.now() - t0;
    assert.ok(dt < 1000, `the timeout actually bounded the wait, not the 500×50 ms of slow batches (took ${dt} ms)`);
    assert.equal(conn.cancelCalls, 1, "cancelSent() was called on expiry");
    // The connection is left usable (not permanently wedged by the abort):
    // a later request on the very same connection object still answers.
    const again = await slowPage.describe('"memory"."main"."heavy"', 0);
    assert.deepEqual(again, []);
  }

  // sec-review-256 "last check": `limits` is merged over SNAPSHOT_LIMITS
  // through ownOpts, and every numeric field is capped at its own
  // default — a caller (a polluted Object.prototype or not) can only ever
  // make a cap tighter, never drop it (a partial object) or raise it past
  // the reviewed maximum. Proven directly: SNAPSHOT_LIMITS.totalRows is
  // shrunk to a tiny, test-only value, so capping to "the real default" is
  // fast and exact to check, rather than needing millions of rows.
  {
    const capsPage = await rawDb();
    capsPage.open({});
    const ccp = capsPage.connect();
    ccp.query("CREATE TABLE d1 AS SELECT i FROM range(5) t(i)");
    ccp.query("CREATE TABLE d2 AS SELECT i FROM range(5) t(i)");
    // Pre-instantiate the three engine dbs this block needs *before*
    // polluting Object.prototype: DuckDB-Wasm's own dynamic linker
    // (relocateExports/updateGOT, a plain `for…in` over its GOT object)
    // instantiates fresh on every `rawDb()`/buildEngine call, and a `for…in`
    // picks up inherited enumerable properties too — a large numeric value
    // on Object.prototype crashes *that* unrelated code
    // ("Cannot create property 'value' on number …"), not anything this PR
    // touches. Building the dbs first and handing buildEngine a `makeDb`
    // that just returns one keeps the pollution window around ownOpts'
    // merge/cap logic only, which is what this test is actually about.
    const preDbs = [await rawDb(), await rawDb(), await rawDb()];
    const nextDb = () => preDbs.shift();
    const realTotalRows = E.SNAPSHOT_LIMITS.totalRows;
    E.SNAPSHOT_LIMITS.totalRows = 3; // tiny, test-only ceiling
    const dirty = { totalRows: 1e12 };
    for (const k of Object.keys(dirty)) Object.prototype[k] = dirty[k];
    try {
      // No `limits` at all: a polluted Object.prototype.totalRows must
      // never be read — the merge is `{ ...SNAPSHOT_LIMITS, ...ownOpts(limits) }`,
      // and ownOpts(undefined) is an empty, null-prototype object
      // regardless of what Object.prototype carries.
      const eNone = await E.buildEngine({ makeDb: nextDb, page: E.pageAdapter(capsPage, {}), extensions: ["json"], doc: undefined });
      const byNone = Object.fromEntries(eNone.tables.map((t) => [t.fq, t]));
      assert.equal(byNone['"memory"."main"."d1"'].rows, 3, "the tiny real default applies with no limits object at all, pollution notwithstanding");
      assert.equal(byNone['"memory"."main"."d2"'].excluded, "too large");
      eNone.close();

      // An explicit, oversized limits.totalRows (1e12, the same value
      // sitting on the polluted prototype): still capped at the real
      // default, not accepted as given.
      const eHuge = await E.buildEngine({ makeDb: nextDb, page: E.pageAdapter(capsPage, {}), extensions: ["json"], limits: { totalRows: 1e12 }, doc: undefined });
      const byHuge = Object.fromEntries(eHuge.tables.map((t) => [t.fq, t]));
      assert.equal(byHuge['"memory"."main"."d1"'].rows, 3, "an oversized limits.totalRows is capped at the real default, not accepted as given");
      assert.equal(byHuge['"memory"."main"."d2"'].excluded, "too large");
      eHuge.close();

      // A partial limits object (another field only): totalRows, which it
      // doesn't mention, still falls back to the real default rather than
      // being dropped (the old key-by-key read made it `undefined`, and
      // `undefined <= 0` is `false` — the row cap silently vanished).
      const ePartial = await E.buildEngine({ makeDb: nextDb, page: E.pageAdapter(capsPage, {}), extensions: ["json"], limits: { queryMs: 5000 }, doc: undefined });
      const byPartial = Object.fromEntries(ePartial.tables.map((t) => [t.fq, t]));
      assert.equal(byPartial['"memory"."main"."d1"'].rows, 3, "totalRows still falls back to the real default when a partial limits object omits it");
      ePartial.close();
    } finally {
      for (const k of Object.keys(dirty)) delete Object.prototype[k];
      E.SNAPSHOT_LIMITS.totalRows = realTotalRows;
    }
    capsPage.terminate?.();
  }
}

// ---- 5. the page is read only through pageAdapter (fixed statement shapes) ----
{
  // "SELECT 1" is pageAdapter's own one-time Table-class probe (M-2's
  // withTimeout fix: a fixed, dataless bookkeeping query, not a read of
  // anything page-owned) — part of the same closed set, so it's allowed
  // here too.
  const shape = /^(SELECT 1|SELECT database_name, path, type FROM duckdb_databases\(\)|SELECT database_name, schema_name, table_name|SELECT sql FROM duckdb_views\(\)|DESCRIBE SELECT \* FROM "[^"]*"\."[^"]*"\."[^"]*"|SELECT \* FROM "[^"]*"\."[^"]*"\."[^"]*" (LIMIT \d+|USING SAMPLE reservoir\(\d+ ROWS\) REPEATABLE \(42\)))/;
  for (const s of snapshotSeen) assert.match(s, shape, s);
  assert.ok(snapshotSeen.length > 0);
}

// ---- 6. safeFileName ----
for (const bad of ["http://x", "s3://b/k", "C:x", "/etc/passwd", "~/x", "a/../b", "a//b", "*.csv", "x\0y", "", "/x"]) assert.equal(E.safeFileName(bad), false, bad);
for (const good of ["data/x/y.csv", "explore/a b.csv", "m5_monthly.parquet"]) assert.equal(E.safeFileName(good), true, good);

// ---- 10. extensions (M-1) ----
assert.deepEqual(E.engineExtensions({}), ["json", "parquet", "icu", "anofox_forecast", "anofox_statistics"]);
assert.ok(E.engineExtensions({ pageLoaded: ["spatial"] }).includes("spatial"));
assert.ok(E.engineExtensions({ editorNeeds: ["spatial"] }).includes("spatial"));
await rejects(() => E.buildEngine({ makeDb: rawDb, page: E.pageAdapter(spyPageDb, {}), extensions: ["inet"], doc: undefined }), /not an engine extension/);

// ---- sec-review-256 M-1: a timed-out extension load fails closed ----
// A fake db whose query() has a *real* async delay (setTimeout, not a
// microtask) for LOAD/INSTALL — the only way a Node unit test can see the
// race the real browser hits (the Node *blocking* driver answers
// synchronously, so it never raced anything, which is exactly how M-1 got
// through review the first time).
{
  const emptyPage = { databases: async () => [], catalog: async () => [], viewSql: async () => "", describe: async () => [], rows: async () => ({ table: { numRows: 0, toArray: () => [], batches: [] }, sampled: false, truncated: false }), files: async () => [], handle: () => undefined, copyFile: async () => new Uint8Array() };
  function fakeDelayedDb(delayMs) {
    let terminated = false;
    const conn = {
      query: async (sql) => {
        if (/^(LOAD|INSTALL)\b/i.test(sql)) await new Promise((r) => setTimeout(r, delayMs));
        if (/current_setting/i.test(sql)) return { toArray: () => [{ toJSON: () => ({ e: false, l: true, a: false, i: false, g: false }) }] };
        return { toArray: () => [] };
      },
      send: async () => {
        throw new Error("fakeDelayedDb: send() unused in this fixture");
      },
      prepare: async () => ({ close: async () => {} }),
      close: async () => {},
      cancelSent: async () => true,
    };
    return {
      open: async () => {},
      connect: async () => conn,
      terminate: async () => {
        terminated = true;
      },
      get terminated() {
        return terminated;
      },
    };
  }
  // extMs (1 ms) shorter than the fake LOAD's delay (30 ms): the build must
  // reject — never resolve with engine.extensions silently missing json.
  {
    const db = fakeDelayedDb(30);
    await rejects(
      () => E.buildEngine({ makeDb: async () => db, page: emptyPage, extensions: ["json"], limits: { ...E.SNAPSHOT_LIMITS, extMs: 1 }, doc: undefined }),
      /extension "json" did not load within 1 ms/
    );
    assert.equal(db.terminated, true, "the Worker is terminated on a timed-out load, not left running behind the lock");
  }
  // extMs comfortably longer than the delay: succeeds normally, and
  // engine.extensions correctly reports what was actually loaded.
  {
    const db = fakeDelayedDb(5);
    const engineFast = await E.buildEngine({ makeDb: async () => db, page: emptyPage, extensions: ["json"], limits: { ...E.SNAPSHOT_LIMITS, extMs: 500 }, doc: undefined });
    assert.deepEqual(engineFast.extensions, ["json"]);
    engineFast.close();
  }
}

// ---- 11. the fingerprint ----
{
  const fp1 = await E.pageFingerprint(E.pageAdapter(spyPageDb, { handles }), { extensions: engine.extensions, files: engine.files });
  const fp2 = await E.pageFingerprint(E.pageAdapter(spyPageDb, { handles }), { extensions: engine.extensions, files: engine.files });
  assert.equal(fp1, fp2, "stable with nothing changed");
  pageMainConn.query("CREATE TABLE extra_fp_table AS SELECT 1");
  const fp3 = await E.pageFingerprint(E.pageAdapter(spyPageDb, { handles }), { extensions: engine.extensions, files: engine.files });
  assert.notEqual(fp1, fp3, "changes on a new table");
  const fp4 = await E.pageFingerprint(E.pageAdapter(spyPageDb, { handles }), { extensions: [...engine.extensions, "spatial"], files: engine.files });
  assert.notEqual(fp1, fp4, "changes on a changed extension set");
}

// ---- 7/8/9: H-1 (zero leakage) + I-2 (quiet errors) + D3 (unchecked) via
// web/assist.js's wasmTools, over the real locked engine ----
{
  loopSeen.length = 0;
  const spyMain = new Proxy(pageMainConn, {
    get: (o, k) => (k === "query" || k === "send" || k === "prepare" ? (s) => (loopSeen.push(String(s)), o[k](s)) : typeof o[k] === "function" ? o[k].bind(o) : o[k]),
  });
  const snapshot = new Map(engine.tables.map((t) => [t.fq, t]));
  const tools = A.wasmTools({ db: engine.db, lintStep: wasm.lint_step, plan: wasm.plan, pageSql: wasm.page_sql, snapshot, extensions: engine.extensions, sampleRows: () => false });
  const lt = JSON.parse(await tools.list_tables({ filter: "" }));
  assert.ok(lt.find((t) => t.name === "sales" && t.sampled === true), JSON.stringify(lt));
  await tools.profile_table({ table: "sales" });
  const rs = JSON.parse(await tools.run_sql({ sql: "SELECT region, count(*) AS n FROM sales GROUP BY ALL", purpose: "x" }));
  assert.ok(rs.rows > 0);
  // Item 10 (M-1): a function from an extension the engine never loaded
  // (spatial wasn't requested) gives a clean "not in the assistant's
  // engine" error, not DuckDB's raw Catalog Error. st_point/st_astext are
  // spatial scalars the gate's parse-tree check lets through by name
  // (SPATIAL_SCALAR) without needing the extension loaded to pass the gate.
  await rejects(() => tools.run_sql({ sql: "SELECT st_astext(st_point(1,2)) AS g", purpose: "x" }), /not in the assistant's engine: spatial/);
  const chk = await tools.check("SELECT region ::XAXIS, sum(amount) ::BARCHART, 'Revenue by region'::TITLE FROM sales GROUP BY ALL;");
  assert.equal(chk.ok, true, chk.text);
  const gt = await tools.gate("SELECT 1 ::METRIC;");
  assert.equal(gt.allowed, true);
  const preview = tools.previewRunner("SELECT region ::XAXIS, sum(amount) ::BARCHART, 'Revenue by region'::TITLE FROM sales GROUP BY ALL;");
  const pv = await preview("SELECT region, sum(amount) AS s FROM sales GROUP BY ALL ORDER BY 2 DESC");
  assert.ok(JSON.parse(pv).length > 0);
  assert.deepEqual(loopSeen, [], "nothing of the tool loop or the preview reached the page's main connection");

  // M-1: prepare is refused under requireLocked (the default)
  assert.throws(() => A.wasmTools({ db: engine.db, prepare: () => {}, lintStep: wasm.lint_step, plan: wasm.plan, pageSql: wasm.page_sql }), /prepare is not allowed/);
  // the default rejects an unlocked db
  const unlockedTools = A.wasmTools({ db: pageDb, lintStep: wasm.lint_step, plan: wasm.plan, pageSql: wasm.page_sql });
  await rejects(() => unlockedTools.list_tables({ filter: "" }), /locked assistant engine|changed state/);
  const unlockedPreview = unlockedTools.previewRunner("SELECT 1 ::METRIC;");
  await rejects(() => unlockedPreview("SELECT 1"), /locked assistant engine|changed state/);

  // I-2: the gate already refuses this (two statements; a computed path),
  // so this bypasses it on purpose — a runner with no guard, as if some
  // future gate bug let it through — to prove the *error text itself*
  // (quietError, the tools' own say() path) still never carries the value.
  // With sample rows off, "alice@x.de" is in neither the SQL nor the
  // catalog, so namesIn would say no — simulated here as () => false.
  const scratch = (await import("../../web/lint.js")).scratchRunner(engine.db);
  let raw = null;
  try {
    await scratch("SET VARIABLE p=(SELECT min(email) FROM customers); SELECT * FROM read_csv(getvariable('p'))");
  } catch (e) {
    raw = String(e?.message || e);
  }
  assert.ok(raw, "the statement did throw");
  assert.match(raw, /alice/, "the raw DuckDB error does carry it (that's exactly why it must never reach the model unmasked)");
  const masked = A.quietError(raw, () => false);
  assert.doesNotMatch(masked, /alice|bob/, masked);

  // D3: a *known* remote path the statement itself reads literally is
  // "unchecked" (it runs on Apply, not here — the engine never fetches
  // remote files at all), not a failed check; any other Permission Error
  // (an unknown path) stays a real error (I-2 above already covers that).
  const knownUrl = "https://example.org/known.csv";
  const d3tools = A.wasmTools({ db: engine.db, lintStep: wasm.lint_step, plan: wasm.plan, pageSql: wasm.page_sql, snapshot, extensions: engine.extensions, known: new Set([knownUrl]) });
  const d3 = await d3tools.check(`SELECT a ::XAXIS, count(*) ::BARCHART, 'Known remote read'::TITLE FROM read_csv('${knownUrl}') GROUP BY ALL;`);
  assert.equal(d3.statements[0].unchecked, "reads a remote file; it runs on Apply", JSON.stringify(d3.statements));
  assert.equal(d3.statements[0].error, undefined);
  assert.ok(!d3.issues.some((i) => i.stmt === 1 && i.code === "sql-error"), JSON.stringify(d3.issues));
  assert.equal(d3.diagnostics.find((x) => x.stmt === 1 && x.code === "sql-error")?.severity, "info");
}

// ---- static greps (I-3, I-4, L-3; comments stripped — the header comments
// name every forbidden API on purpose, to say it isn't used) ----
const noComments = (s) => s.replace(/\/\*[\s\S]*?\*\//g, " ").replace(/\/\/.*$/gm, "");
{
  const src = noComments(readFileSync(path.join(root, "web/assist-engine.js"), "utf8"));
  for (const re of [/fetch\(/, /XMLHttpRequest/, /registerFileURL/, /\bopfs\b/i, /document\.baseURI/, /\bimport\(/]) assert.doesNotMatch(src, re, re.toString());
  assert.match(src, /import\.meta\.url/, "LOCALEXT comes from import.meta.url");
  for (const f of ["web/app.js", "web/assist.js", "web/assist-csp.js", "web/assist-engine.js"]) {
    const s = noComments(readFileSync(path.join(root, f), "utf8"));
    assert.doesNotMatch(s, /window\.assistDeps/, f);
  }
  for (const f of ["web/app.js"]) {
    const s = noComments(readFileSync(path.join(root, f), "utf8"));
    assert.doesNotMatch(s, /requireLocked\s*:/, `${f}: never overrides requireLocked`);
    assert.doesNotMatch(s, /__testing/, f);
    assert.doesNotMatch(s, /wasmTools\([^)]*\bprepare\s*:/, `${f}: never passes prepare to wasmTools`);
  }
}

engine.close();
console.log("assist_engine_test: ok");
