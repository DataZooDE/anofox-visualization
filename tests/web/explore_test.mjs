// Explore data (issue #97): web/explore.js — the profiling SQL, the column
// cards, data-quality hints, chart SQL, the starter dashboard and the SQL
// edits — plus, with the wasm build (web/pkg) and DuckDB-Wasm (npm install
// --prefix tests/web), a real profile of sample tables in DuckDB-Wasm, every
// starter statement run there, and a time bound for profiling 1 M rows.
// (`dashboard --check --strict` of the starters: scripts/explore-starters.mjs.)
process.env.DATAZOO_DISABLE_TELEMETRY ??= "1"; // no usage telemetry from anofox extensions (child processes inherit it)
import { existsSync, readFileSync } from "fs";
import { createRequire } from "module";
import path from "path";
import assert from "assert/strict";
import * as xp from "../../web/explore.js";

// ---- kinds, the sample threshold, the SQL text ----
assert.deepEqual(
  ["INTEGER", "DECIMAL(18,3)", "DOUBLE", "DATE", "TIMESTAMP WITH TIME ZONE", "BOOLEAN", "VARCHAR", "ENUM('a', 'b')", "INTEGER[]", "STRUCT(a INTEGER)", "BLOB", "INTERVAL", "UUID"].map(xp.colKind),
  ["num", "num", "num", "date", "time", "bool", "text", "text", "other", "other", "other", "other", "text"]
);
assert.equal(xp.profileBase('"t"', 1000).sampled, false);
const big = xp.profileBase('"t"', 48e6);
assert.ok(big.sampled && big.rows === xp.SAMPLE_ROWS && /USING SAMPLE reservoir\(100000 ROWS\)/.test(big.base), big.base);
const ex = xp.extrasSql("SELECT * FROM t", [
  { name: "amount", kind: "num" },
  { name: "day", kind: "date" },
  { name: "customer_id", kind: "num" },
  { name: "note", kind: "text" },
]);
assert.match(ex, /quantile_cont\(\(CASE WHEN isfinite\(TRY_CAST\("amount" AS DOUBLE\)\)/, "percentiles count finite values only (#74)");
assert.match(ex, /CAST\(min\("day"\) AS VARCHAR\) AS t0_1/);
assert.match(ex, /count\(DISTINCT "customer_id"\) AS dx_2/);
assert.match(ex, /count\(TRY_CAST\("note" AS DOUBLE\)\) AS tn_3/);
assert.equal(xp.timeGrain("2026-01-01", "2026-02-01"), "day");
assert.equal(xp.timeGrain("2025-01-01", "2026-06-01"), "week");
assert.equal(xp.timeGrain("2010-01-01", "2026-06-01"), "month");
assert.equal(xp.timeGrain("1900-01-01", "2026-06-01"), "year");
assert.equal(xp.timeGrain(null, "2026-06-01"), null);
assert.match(xp.seriesSql("SELECT * FROM t", [{ i: 1, name: "day", grain: "week" }]), /date_trunc\('week', "day"\)/);

// ---- cards and hints ----
const cols = [
  { name: "amount", type: "DOUBLE" },
  { name: "region", type: "VARCHAR" },
  { name: "day", type: "DATE" },
  { name: "order_id", type: "INTEGER" },
  { name: "tags", type: "VARCHAR[]" },
];
const cards = xp.buildCards(cols, {
  stats: { n: 100, n0: 90, d0: 80, lo0: 1, hi0: 1000, m0: 50, md0: 40, s0: 4500, n1: 100, d1: 3, n2: 100, d2: 30, n3: 100, d3: 98, lo3: 1, hi3: 100, n4: 40, d4: 12 },
  prof: [
    { c: 0, t: "h", k: 0, n: 80 },
    { c: 0, t: "h", k: 9, n: 10 },
    { c: 1, t: "t", v: "EU", n: 60 },
    { c: 1, t: "t", v: "US", n: 40 },
  ],
  extras: { n: 100, q05_0: 2, q95_0: 60, t0_2: "2026-01-01", t1_2: "2026-01-30", dx_3: 95 },
  series: [
    { c: 2, b: "2026-01-01", n: 3 },
    { c: 2, b: "2026-01-02", n: 5 },
  ],
});
const [amount, region, day, oid, tags] = cards;
assert.equal(amount.nullPct, 10);
assert.deepEqual([amount.min, amount.p05, amount.median, amount.p95, amount.max], [1, 2, 40, 60, 1000]);
assert.equal(amount.hist.length, 10);
assert.equal(amount.hist[0] + amount.hist[9], 90);
assert.deepEqual(region.top.map((t) => t.v), ["EU", "US"]);
assert.equal(day.grain, "day");
assert.equal(day.series.length, 2);
assert.equal(oid.distinct, 95, "a key column's distinct count is exact");
assert.equal(tags.kind, "other");
assert.equal(tags.nullPct, 60);
const codes = (c) => (c.hints || xp.qualityHints(c)).map((h) => h.code);
assert.ok(codes(amount).includes("outliers"), "1000 is far beyond p95 60");
assert.ok(codes(oid).includes("duplicate-keys"));
assert.ok(codes(tags).includes("mostly-null"));
assert.deepEqual(codes(region), []);
assert.deepEqual(codes({ name: "c", kind: "text", n: 10, nonnull: 10, nullPct: 0, distinct: 1 }), ["constant"]);
assert.deepEqual(codes({ name: "c", kind: "text", n: 10, nonnull: 10, nullPct: 0, distinct: 5, numericText: 4 }), ["mixed-types"]);
assert.deepEqual(codes({ name: "c", kind: "num", n: 10, nonnull: 0, nullPct: 100, distinct: 0 }), ["all-null"]);
assert.match(xp.cardSummary(amount), /^amount: DOUBLE; 10% empty; .*from 1 to 1,000, median 40\.$/);
assert.equal(xp.fmtCount(48_123_456), "48 M");
assert.equal(xp.fmtCount(1_000_000), "1.0 M");
assert.equal(xp.fmtStat(0.000012), "1.20e-5");

// ---- chart SQL ----
assert.deepEqual(xp.chartChoices(amount, cards).map((c) => c.id), ["hist", "box", "line"]);
assert.deepEqual(xp.chartChoices(region, cards).map((c) => c.id), ["bars", "line"]);
assert.deepEqual(xp.chartChoices(tags, cards), []);
assert.match(xp.columnChartSql('"t"', amount, "hist", cards), /"amount"::HISTOGRAM/);
assert.match(xp.columnChartSql('"t"', amount, "hist", cards), /\) _s USING SAMPLE reservoir\(5000 ROWS\)/, "sampled after the NOT NULL filter");
assert.match(xp.columnChartSql('"t"', region, "bars", cards), /ORDER BY 2 DESC, 1 LIMIT 15;$/);
assert.match(xp.columnChartSql('"t"', amount, "line", cards), /date_trunc\('day', "day"\)/);
assert.equal(xp.columnChartSql('"t"', amount, "scatter", cards), "", "no other measure to plot against");
assert.equal(
  xp.resultChartSql("SELECT a, b FROM t;", "bar", "a", "b"),
  `SELECT "a"::XAXIS, "b"::BARCHART, 'b by a'::TITLE\nFROM (\nSELECT a, b FROM t\n) _q;`
);
assert.deepEqual(xp.resultAxes(["day", "n"], [{ day: "2026-01-01", n: 3 }]), { x: "day", y: "n", nums: ["n"] });

// ---- SQL edits, errors, history, files ----
assert.equal(xp.addWhere("SELECT * FROM t LIMIT 100", `"a" = 1`), 'SELECT * FROM t\nWHERE "a" = 1\nLIMIT 100');
assert.equal(xp.addWhere("SELECT * FROM t WHERE b > 2 ORDER BY 1;", `"a" = 1`), 'SELECT * FROM t WHERE (b > 2) AND "a" = 1\nORDER BY 1');
assert.equal(xp.addWhere("SELECT * FROM (SELECT * FROM u WHERE x LIMIT 3) s", "y"), "SELECT * FROM (SELECT * FROM u WHERE x LIMIT 3) s\nWHERE y");
assert.equal(xp.addWhere("SELECT 'where limit' AS w FROM t", "y"), "SELECT 'where limit' AS w FROM t\nWHERE y");
assert.equal(xp.addWhere("SELECT 1 UNION ALL SELECT 2", "y"), "SELECT * FROM (\nSELECT 1 UNION ALL SELECT 2\n) _q\nWHERE y");
assert.equal(xp.valueCond({ name: "it's", kind: "text" }, "O'Neil"), `"it's" = 'O''Neil'`);
assert.equal(xp.valueCond({ name: "n", kind: "num" }, "42"), `"n" = 42`);
assert.equal(xp.valueCond({ name: "d", kind: "date" }, "2026-01-01"), `CAST("d" AS VARCHAR) = '2026-01-01'`);
const msg = 'Binder Error: Referenced column "nosuch" not found\n\nLINE 2:   nosuch + 1 FROM t\n          ^';
assert.deepEqual(xp.errorLocation(msg), { line: 2, col: 3 });
assert.equal(xp.offsetOf("SELECT 1,\n  nosuch + 1 FROM t", 2, 3), 12);
assert.equal(xp.errorLocation("Catalog Error: Table x does not exist"), null);
assert.deepEqual(xp.pushHistory(["b", "a"], "a"), ["a", "b"]);
assert.equal(xp.pushHistory(Array.from({ length: 30 }, (_, i) => String(i)), "new").length, 20);
assert.deepEqual(xp.fileView("C:\\data\\Sales 2026.csv"), { view: "sales_2026", reader: "read_csv_auto", ext: "csv" });
assert.deepEqual(xp.fileView("2026.parquet"), { view: "file_2026", reader: "read_parquet", ext: "parquet" });
assert.equal(xp.fileView("notes.docx").reader, null);
assert.equal(xp.varName("Order Status", new Set(["f_order_status"])), "f_order_status_2");

// ---- joins ----
const jc = xp.joinCandidates(
  { fq: "o", name: "orders", cols: [{ name: "customer_id", kind: "num" }, { name: "region", kind: "text" }, { name: "amount", kind: "num" }] },
  [{ fq: "c", name: "customers", cols: [{ name: "id", kind: "num" }, { name: "region", kind: "text" }] }]
);
assert.deepEqual(jc.map((j) => [j.a, j.b]), [["customer_id", "id"], ["region", "region"]]);
assert.match(xp.joinSql('"o"', "customer_id", '"c"', "id"), /JOIN "c" b ON a\."customer_id" = b\."id"/);

// ---- the starter dashboard ----
const starter = xp.starterDashboard({ name: "orders", fq: '"memory"."main"."orders"', cards });
for (const re of [/::ASOF/, / AS period ::PERIOD/, /::BUTTONS/, /getvariable\('period_from'\)/, /'At a glance'::GROUP/, /::LINECHART/, /::BARCHART/, /::HISTOGRAM/, /::PAGED/])
  assert.match(starter, re);
assert.doesNotMatch(starter, /"tags"/, "lists stay out of the paged table");
assert.doesNotMatch(starter, /sum\("order_id"\)/, "a key is no measure");
// no date, no category: no filters, no macro — the table itself
const plain = xp.starterDashboard({ name: "m", fq: '"m"', cards: [amount] });
assert.doesNotMatch(plain, /PERIOD|getvariable|BUTTONS|DROPDOWN/);
assert.match(plain, /FROM "m"/);
// odd names are quoted, their words escaped
const odd = xp.buildCards([{ name: 'it\'s "x"', type: "VARCHAR" }, { name: "value ($)", type: "DOUBLE" }], {
  stats: { n: 10, n0: 10, d0: 3, n1: 10, d1: 9, lo1: 1, hi1: 9 },
});
const os = xp.starterDashboard({ name: "odd", fq: '"odd"', cards: odd });
assert.match(os, /"it's ""x"""/);
assert.match(os, /'It''s "x"'::LABEL/);

// ---- anofox_tabular checks and comparisons (#111) ----
const dupHint = { code: "duplicate-keys" };
assert.deepEqual(xp.hintCheck('"t"', oid, dupHint), { table: '"t"', column: "order_id", params: null, lower: null, upper: 0, type: "duplicate_count" });
assert.equal(xp.hintCheck('"t"', amount, { code: "outliers" }).params.expression, `"amount" IS NULL OR (CASE WHEN isfinite(TRY_CAST("amount" AS DOUBLE)) THEN TRY_CAST("amount" AS DOUBLE) END) BETWEEN -172 AND 234`);
assert.equal(xp.hintCheck('"t"', region, { code: "nope" }), null);
const ts = xp.tabularChecksSql([xp.hintCheck('"t"', oid, dupHint)], true);
assert.match(ts, /^LOAD anofox_tabular;\nATTACH IF NOT EXISTS ':memory:' AS xp_checks \(READ_WRITE\);/);
assert.match(ts, /\('1 duplicate_count order_id', 'duplicate_count', '"t"', 'order_id', CAST\(NULL AS VARCHAR\), CAST\(NULL AS DOUBLE\), CAST\(0 AS DOUBLE\), false/);
assert.match(ts, /FROM run_checks\('xp_checks\.main\.checks'\)/);
assert.deepEqual(xp.halves("2026-01-01", "2026-01-31"), [["2026-01-01", "2026-01-16"], ["2026-01-17", "2026-01-31"]]);
assert.equal(xp.halves("2026-01-01", "2026-01-01"), null);
assert.match(xp.rangeSide('"t"', "day", "2026-01-01", "2026-01-16"), /^\(SELECT \* FROM "t" WHERE CAST\("day" AS DATE\) BETWEEN DATE '2026-01-01' AND DATE '2026-01-16'\) _r$/);
const cA = [
  { name: "id", type: "INTEGER", kind: "num", n: 10, nonnull: 10, distinct: 10, nullPct: 0, mean: 5, median: 5 },
  { name: "v", type: "INTEGER", kind: "num", n: 10, nonnull: 10, distinct: 4, nullPct: 0, mean: 10, median: 10 },
  { name: "gone", type: "VARCHAR", kind: "text", n: 10, nonnull: 10, distinct: 2, nullPct: 0 },
];
const cB = [
  { name: "id", type: "INTEGER", kind: "num", n: 12, nonnull: 12, distinct: 12, nullPct: 0, mean: 6, median: 6 },
  { name: "v", type: "DOUBLE", kind: "num", n: 12, nonnull: 6, distinct: 4, nullPct: 50, mean: 20, median: 10 },
  { name: "new", type: "DATE", kind: "date", n: 12, nonnull: 12, distinct: 12, nullPct: 0 },
];
const cc = xp.compareCards(cA, cB);
assert.deepEqual(cc.map((r) => [r.name, r.change]), [["id", "same"], ["v", "retyped"], ["gone", "removed"], ["new", "added"]]);
assert.deepEqual(cc[1].shifts, ["empty 0% → 50%", "mean 10 → 20 (+100%)"]);
assert.equal(xp.compareKey(cA, cB), "id");
assert.equal(xp.compareKey(cA.slice(1), cB.slice(1)), null);
const kd = xp.keyDiffSql('"a"', '"b"', "id", [{ name: "id" }, { name: "v", num: true }]);
assert.match(kd, /md5\(CAST\(row\(CAST\("v" AS DOUBLE\)\) AS VARCHAR\)\)/, "numbers compare as numbers across a retype");
assert.match(xp.overlaySql("SELECT * FROM a", "SELECT * FROM b", [{ i: 1, name: "v", lo: 0, hi: 20 }]), /SELECT 1 AS c, 'a' AS s, least\(9, floor\(\(_x - 0\) \/ 20 \* 10\)\)::INTEGER AS k/);

console.log("explore_test (pure): ok");

// ---- in DuckDB-Wasm: real profiles, the starters run, 1 M rows in time ----
const pkg = new URL("../../web/pkg/", import.meta.url);
const here = path.dirname(new URL(import.meta.url).pathname);
const require = createRequire(path.join(here, "package.json"));
let duckdb = null;
try {
  duckdb = require("@duckdb/duckdb-wasm/dist/duckdb-node-blocking.cjs");
} catch (_) {}
if (!existsSync(new URL("anofox_visualization_bg.wasm", pkg)) || !duckdb) {
  console.log("explore_test: DuckDB-Wasm part skipped (needs web/pkg and npm install --prefix tests/web)");
  process.exit(0);
}
const wasm = await import(new URL("anofox_visualization.js", pkg));
wasm.initSync({ module: readFileSync(new URL("anofox_visualization_bg.wasm", pkg)) });
const dist = path.dirname(require.resolve("@duckdb/duckdb-wasm/dist/duckdb-node-blocking.cjs"));
const db = await duckdb.createDuckDB(
  { mvp: { mainModule: path.join(dist, "duckdb-mvp.wasm"), mainWorker: path.join(dist, "duckdb-node-mvp.worker.cjs") } },
  new duckdb.VoidLogger(),
  duckdb.NODE_RUNTIME
);
await db.instantiate();
const conn = db.connect();
const q = (s) =>
  conn
    .query(s)
    .toArray()
    .map((r) => JSON.parse(JSON.stringify(r.toJSON(), (_, v) => (typeof v === "bigint" ? Number(v) : v))));
const run = async (s) => q(s);
const pageSql = (b, p) => wasm.page_sql(b, p);
q(`CREATE TABLE orders AS SELECT i AS order_id, ['EU','US','APAC'][1 + i % 3] AS region, CASE WHEN i % 10 = 0 THEN NULL ELSE i * 1.5 END AS amount,
     DATE '2026-01-01' + (i % 60)::INT AS day, 'k' || i AS k, i % 7 AS v FROM range(1, 1001) t(i)`);
const p = await xp.profileTable(run, pageSql, '"memory"."main"."orders"');
assert.equal(p.total, 1000);
assert.equal(p.sampled, false);
const by = Object.fromEntries(p.cards.map((c) => [c.name, c]));
assert.equal(by.amount.nullPct, 10);
assert.equal(by.amount.min, 1.5);
assert.equal(by.amount.max, 1498.5);
assert.ok(Math.abs(by.amount.p05 - 0.05 * 1497 - 1.5) < 20, `p05 ${by.amount.p05}`);
assert.equal(by.region.top.length, 3);
assert.equal(by.day.tmin, "2026-01-01");
assert.equal(by.day.tmax, "2026-03-01");
assert.equal(by.day.grain, "day");
assert.equal(by.day.series.length, 60);
assert.equal(by.order_id.distinct, 1000);
assert.equal(by.v.top.length, 3, "a column named like the profile's aliases (v) still profiles");
// every statement of its starter runs in DuckDB-Wasm (the period's defaults set)
const sql = xp.starterDashboard({ name: "orders", fq: '"memory"."main"."orders"', cards: p.cards });
const stmts = JSON.parse(wasm.plan(sql));
for (const s of stmts) {
  const rows = q(s.sql);
  if (s.roles.some((r) => r[1] === "PERIOD")) {
    const res = JSON.parse(wasm.period_resolve(String(Object.values(rows[0])[0]), JSON.stringify({ column: "period" })));
    for (const [k, v] of Object.entries(res.vars)) q(`SET VARIABLE ${res.prefix}_${k} = ${v == null ? "NULL" : `'${v}'`}`);
  }
  if (s.roles.some((r) => r[1] === "BUTTONS" || r[1] === "DROPDOWN")) q(`SET VARIABLE ${Object.keys(rows[0])[0]} = 'All'`);
}
const kpi = stmts.find((s) => s.roles.some((r) => r[1] === "COMPACT"));
assert.equal(Object.values(q(kpi.sql)[0])[0], 1000, "the default period covers every row");
q("RESET VARIABLE period_from");
assert.equal(Object.values(q(kpi.sql)[0])[0], 1000, "an unset variable filters nothing");

// 1 M rows: profiled on a sample, within the bound (DuckDB-Wasm, one thread)
q(`CREATE TABLE big AS SELECT i AS id, 'k' || (i % 50) AS cat, (i % 997) * 1.25 AS amount, DATE '2020-01-01' + (i % 2000)::INT AS day FROM range(1000000) t(i)`);
const bound = Number(process.env.EXPLORE_PERF_MS || 15000);
const t0 = Date.now();
const pb = await xp.profileTable(run, pageSql, '"memory"."main"."big"');
const ms = Date.now() - t0;
assert.equal(pb.total, 1e6);
assert.ok(pb.sampled, "a 1 M-row table is profiled on a sample");
assert.ok(ms < bound, `profiling 1 M rows took ${ms} ms (bound ${bound} ms)`);
// a superseded profile stops before its next query
let calls = 0;
await assert.rejects(
  xp.profileTable(async (s) => (calls++, q(s)), pageSql, '"memory"."main"."big"', { alive: () => calls < 2 }),
  (e) => e.name === "Superseded"
);
assert.equal(calls, 2);
// a comparison by key, in DuckDB-Wasm, and its dashboard runs there
q(`CREATE TABLE v1 AS SELECT i AS customer_id, 'n' || i AS name, i * 10 AS score FROM range(1, 301) t(i)`);
q(`CREATE TABLE v2 AS SELECT i AS customer_id, CASE WHEN i % 25 = 0 THEN 'x' || i ELSE 'n' || i END AS name, (i * 10)::DOUBLE AS score, i % 2 = 0 AS active FROM range(20, 341) t(i)`);
const sideA = { src: '"memory"."main"."v1"', label: "v1" },
  sideB = { src: '"memory"."main"."v2"', label: "v2" };
const cmp = await xp.compareTables(run, pageSql, sideA, sideB);
assert.equal(cmp.key, "customer_id");
assert.deepEqual([cmp.diff.added, cmp.diff.removed, cmp.diff.changed], [40, 19, 12]);
assert.deepEqual(cmp.cols.filter((r) => r.change !== "same").map((r) => `${r.name} ${r.change}`), ["score retyped", "active added"]);
const ov = cmp.overlay.get("score");
assert.equal(ov.a.reduce((x, y) => x + y), 300);
assert.equal(ov.b.reduce((x, y) => x + y), 321);
for (const s of JSON.parse(wasm.plan(xp.compareDashboard({ a: sideA, b: sideB, cmp })))) q(s.sql);
console.log(`explore_test (DuckDB-Wasm): ok — 1 M rows profiled in ${ms} ms`);
