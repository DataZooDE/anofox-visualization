// #740: the assistant engine on an authoring server. The page's tables live
// in the server's session; `livePageAdapter` copies them through `/query`
// (here: a second DuckDB-Wasm standing in for the server, answering JSON text
// the way the CLI's -json output does) into the locked engine. The engine is
// locked, values keep their types (a BIGINT past 2^53 included), hostile
// text stays data, the server sees only the adapter's fixed statements, and
// the live limits sample a large table.
import { createRequire } from "module";
import { existsSync } from "fs";
import path from "path";
import assert from "assert/strict";
import * as E from "../../web/assist-engine.js";
import { CATALOG_SQL } from "../../web/explore.js";

const here = path.dirname(new URL(import.meta.url).pathname);
const require = createRequire(path.join(here, "package.json"));
let duckdb = null;
try {
  duckdb = require("@duckdb/duckdb-wasm/dist/duckdb-node-blocking.cjs");
} catch (_) {}
if (!duckdb) {
  console.error("assist_live_engine_test: FAILED — needs npm install --prefix tests/web");
  process.exit(1);
}
const dist = path.dirname(require.resolve("@duckdb/duckdb-wasm/dist/duckdb-node-blocking.cjs"));
async function rawDb() {
  const d = await duckdb.createDuckDB({ eh: { mainModule: path.join(dist, "duckdb-eh.wasm"), mainWorker: path.join(dist, "duckdb-node-eh.worker.cjs") } }, new duckdb.VoidLogger(), duckdb.NODE_RUNTIME);
  await d.instantiate();
  return d;
}
assert.ok(existsSync(path.join(dist, "duckdb-eh.wasm")));

// ---- the "server": its own database, answering /query with JSON text ----
const server = await rawDb();
server.open({});
const sc = server.connect();
sc.query("INSTALL json; LOAD json;");
sc.query(
  "CREATE TABLE typed AS SELECT 1::INTEGER AS i, DATE '2026-03-01' AS d, TIMESTAMP '2026-03-01 10:00:00' AS ts, " +
    "12.345::DECIMAL(18,3) AS m, [1,2] AS l, {'a': 1, 'b': 'x'} AS s, NULL::VARCHAR AS n, " +
    "'x''); DROP TABLE typed; --' AS q, 9007199254740993::BIGINT AS big"
);
sc.query("CREATE TABLE big AS SELECT i AS id, i % 7 AS k FROM range(0, 250) t(i)");
sc.query("CREATE VIEW v AS SELECT id FROM big WHERE k = 0");
sc.query("CREATE SCHEMA s2; CREATE TABLE s2.t2 AS SELECT 'z' AS z");

// What /query on an authoring server returns: the rows as a JSON array, as
// text. The server's own JSON writer, so numbers stay exact.
const seen = [];
async function query(sql) {
  seen.push(sql);
  const r = sc.query(`SELECT coalesce(to_json(list(__row))::VARCHAR, '[]') AS j FROM (${sql}) __row`);
  return r.toArray()[0].toJSON().j;
}

const engine = await E.buildEngine({
  makeDb: rawDb,
  page: E.livePageAdapter({ query }),
  extensions: ["json"],
  limits: { ...E.LIVE_LIMITS, tableRows: 100 },
  doc: undefined,
});

// ---- the engine is locked, and has the server's tables ----
assert.equal(await E.engineLocked(engine.db), true, "the engine is locked");
const byFq = Object.fromEntries(engine.tables.map((t) => [t.fq, t]));
const fq = (db, t, s = "main") => `"${db}"."${s}"."${t}"`;
const dbName = Object.keys(byFq)[0].split(".")[0].replace(/"/g, "");
assert.equal(byFq[fq(dbName, "typed")].rows, 1, JSON.stringify(engine.tables));
assert.equal(byFq[fq(dbName, "big")].rows, 100);
assert.equal(byFq[fq(dbName, "big")].sampled, true, "a table over the limit is sampled");
assert.equal(byFq[fq(dbName, "v")].rows, 36, "a view is read with LIMIT, here all 36 rows");
assert.equal(byFq[fq(dbName, "t2", "s2")].rows, 1, "other schemas come along");

const ec = engine.db.connect();
const row = ec.query(`SELECT i, d::VARCHAR AS d, ts::VARCHAR AS ts, m::VARCHAR AS m, l, s.b AS sb, n, q, big::VARCHAR AS big, typeof(m) AS tm, typeof(big) AS tb FROM ${fq(dbName, "typed")}`).toArray()[0].toJSON();
assert.equal(row.i, 1);
assert.equal(row.d, "2026-03-01");
assert.equal(row.ts, "2026-03-01 10:00:00");
assert.equal(row.m, "12.345");
assert.equal(row.tm, "DECIMAL(18,3)", "the described type, not a JSON guess");
assert.equal(row.sb, "x");
assert.equal(row.n, null);
assert.equal(row.q, "x'); DROP TABLE typed; --", "hostile text is data");
assert.equal(row.big, "9007199254740993", "a BIGINT past 2^53 is exact (never a JS double)");
assert.equal(row.tb, "BIGINT");
ec.close();

// ---- the server saw only the adapter's fixed statements ----
const fixed = [
  /^SELECT database_name, path, type FROM duckdb_databases\(\)$/,
  new RegExp("^" + CATALOG_SQL.replace(/[.*+?^${}()|[\]\\]/g, "\\$&") + "$"),
  /^SELECT sql FROM duckdb_views\(\) WHERE database_name='[^']*' AND schema_name='[^']*' AND view_name='[^']*'$/,
  /^DESCRIBE SELECT \* FROM "[^"]+"\."[^"]+"\."[^"]+"$/,
  /^SELECT count\(\*\) AS n FROM "[^"]+"\."[^"]+"\."[^"]+"$/,
  /^SELECT \* FROM "[^"]+"\."[^"]+"\."[^"]+"( LIMIT \d+| USING SAMPLE reservoir\(\d+ ROWS\) REPEATABLE \(42\))?$/,
];
for (const s of seen) assert.ok(fixed.some((re) => re.test(s)), `not one of the adapter's statements: ${s}`);
assert.ok(!seen.some((s) => /INSERT|CREATE|DROP|ATTACH|COPY|SET /i.test(s.replace(/'[^']*'/g, ""))), "nothing that writes");

// ---- the byte budget refuses rather than copies ----
{
  const e2 = await E.buildEngine({ makeDb: rawDb, page: E.livePageAdapter({ query }), extensions: ["json"], limits: { ...E.LIVE_LIMITS, totalBytes: 50 }, doc: undefined });
  assert.ok(e2.tables.some((t) => t.excluded === "too large"), JSON.stringify(e2.tables));
  e2.close();
}

// ---- an adapter without a query function is refused ----
assert.throws(() => E.livePageAdapter({}), /needs query/);
assert.equal((await E.livePageAdapter({ query }).files()).length, 0, "a live page has no files");

engine.close();
console.log("assist_live_engine_test: ok");
