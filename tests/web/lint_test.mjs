// The lint in the browser (#145): web/lint.js drives the wasm lint_step over
// DuckDB-Wasm and must give what `dashboard --check --json` gives.
//   1. the fixed-point loop with a fake lint_step (no wasm needed);
//   2. with the wasm build (web/pkg), DuckDB-Wasm (npm install --prefix
//      tests/web), the wasm extensions (scripts/fetch-wasm-extensions.sh) and
//      the dashboard CLI (DASHBOARD, else target/debug/dashboard): every
//      web/examples/*.sql, the lint fixtures (tests/fixtures/lint/) and two
//      Explore starter dashboards, linted in DuckDB-Wasm and by the CLI.
// The CLI's errors come from its `duckdb` (on PATH), the browser's from
// DuckDB-Wasm: an sql-error's message is compared on its first line. A script
// whose `LOAD <extension>` fails in the CLI (not installed, or an outdated
// build in ~/.duckdb) has no native reference: it is named in the output and
// must lint without an error in the browser instead.
process.env.DATAZOO_DISABLE_TELEMETRY = "1"; // no usage telemetry from anofox extensions, whatever was inherited (child processes inherit it)
import { execFileSync, spawn } from "child_process";
import { existsSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from "fs";
import { createRequire } from "module";
import os from "os";
import path from "path";
import assert from "assert/strict";
import { countsText, lintScript, rowsJson, scratchRunner, sortDiagnostics } from "../../web/lint.js";

// ---- 1. the loop ----
{
  // A fake lint: needs q1, then (with q1's rows) q2; done once both are known.
  const calls = [];
  const step = (script, known, opts) => {
    const k = JSON.parse(known);
    calls.push([Object.keys(k), JSON.parse(opts)]);
    if (!k.q1) return JSON.stringify({ done: false, need: ["q1"] });
    if (!k.q2) return JSON.stringify({ done: false, need: ["q2", "q3"] });
    return JSON.stringify({ done: true, need: [], diagnostics: [{ severity: "error", stmt: 1, code: "sql-error", message: k.q3.error, sql: "" }], ok: false, summary: "1 error(s), 0 warning(s)", spans: [[0, 4]] });
  };
  const ran = [];
  const run = async (sql) => {
    ran.push(sql);
    if (sql === "q3") throw new Error("Binder Error: nope");
    return "[]";
  };
  const progress = [];
  const r = await lintScript("x", { step, run, strict: true, onStep: (n, q) => progress.push([n, q]) });
  assert.equal(r.steps, 3);
  assert.equal(r.queries, 3);
  assert.deepEqual(ran, ["q1", "q2", "q3"]);
  assert.deepEqual(progress, [[1, 1], [2, 3]]);
  assert.deepEqual(calls[0][1], { strict: true, design: true });
  assert.equal(r.diagnostics[0].message, "Binder Error: nope", "a failed query is answered with its error");
  // cancelled before the next query
  let n = 0;
  assert.equal(await lintScript("x", { step, run, cancelled: () => ++n > 1 }), null);
  // a lint that never settles, or a bad argument
  await assert.rejects(lintScript("x", { step: () => JSON.stringify({ done: false, need: [String(Math.random())] }), run }), /did not settle/);
  await assert.rejects(lintScript("x", { step: () => JSON.stringify({ error: "known: not a JSON object" }), run }), /not a JSON object/);
  // ordering and the status counts
  const d = sortDiagnostics([
    { stmt: 3, severity: "warning", code: "b" },
    { stmt: 3, severity: "error", code: "a" },
    { stmt: 0, severity: "warning", code: "h" },
    { stmt: 1, severity: "note", code: "n" },
  ]);
  assert.deepEqual(d.map((x) => x.code), ["h", "n", "a", "b"]);
  assert.equal(countsText(d), "1 error, 2 warnings, 1 note");
  assert.equal(countsText([]), "");
}

// ---- 2. DuckDB-Wasm against the CLI ----
const here = path.dirname(new URL(import.meta.url).pathname);
const root = path.resolve(here, "../..");
const web = path.join(root, "web");
const pkg = path.join(web, "pkg");
const DASH = process.env.DASHBOARD || path.join(root, "target/debug/dashboard");
const EXT = "v1.5.6";
const require = createRequire(path.join(here, "package.json"));
let duckdb = null;
try {
  duckdb = require("@duckdb/duckdb-wasm/dist/duckdb-node-blocking.cjs");
} catch (_) {}
const missing = [
  [existsSync(path.join(pkg, "anofox_visualization_bg.wasm")), "web/pkg (wasm-pack build)"],
  [duckdb, "DuckDB-Wasm (npm install --prefix tests/web)"],
  [existsSync(path.join(web, "localext", EXT, "wasm_eh", "anofox_forecast.duckdb_extension.wasm")), "web/localext (scripts/fetch-wasm-extensions.sh)"],
  [existsSync(DASH), `the dashboard CLI at ${DASH} (cargo build --bin dashboard)`],
].filter(([ok]) => !ok);
if (missing.length) {
  console.log(`lint_test: DuckDB-Wasm part skipped (needs ${missing.map(([, w]) => w).join(", ")})`);
  console.log("lint_test: ok");
  process.exit(0);
}
const wasm = await import(path.join(pkg, "anofox_visualization.js"));
wasm.initSync({ module: readFileSync(path.join(pkg, "anofox_visualization_bg.wasm")) });

// The extensions are fetched synchronously by the blocking bundle: serve them
// from another process (as tests/web/wasm_extensions_test.mjs does).
const port = 19000 + (process.pid % 1000);
const server = spawn("python3", ["-u", "-m", "http.server", String(port), "--bind", "127.0.0.1"], {
  cwd: path.join(web, "localext"),
  stdio: ["ignore", "pipe", "ignore"],
});
await new Promise((ok) => server.stdout.once("data", ok));
const tmp = mkdtempSync(path.join(os.tmpdir(), "lint-test-"));
try {
  // The eh bundle: the MVP one loses DuckDB's error messages.
  const dist = path.dirname(require.resolve("@duckdb/duckdb-wasm/dist/duckdb-node-blocking.cjs"));
  const db = await duckdb.createDuckDB(
    { eh: { mainModule: path.join(dist, "duckdb-eh.wasm"), mainWorker: path.join(dist, "duckdb-node-eh.worker.cjs") } },
    new duckdb.VoidLogger(),
    duckdb.NODE_RUNTIME
  );
  await db.instantiate();
  db.open({ allowUnsignedExtensions: true });
  const page = db.connect(); // the page's own connection (web/app.js `conn`)
  page.query("INSTALL json; LOAD json"); // as web/app.js does before an anofox extension
  page.query(`SET custom_extension_repository='http://127.0.0.1:${port}'`);
  for (const e of ["anofox_statistics", "anofox_forecast"]) {
    page.query(`FORCE INSTALL ${e}`);
    page.query(`LOAD ${e}`);
  }
  page.query("RESET custom_extension_repository");
  // What web/app.js registers on first use: the stored results under web/data
  // and the forecast example's parquet, by the names the SQL reads them by.
  const walk = (d) => readdirSync(path.join(web, d), { withFileTypes: true }).flatMap((e) => (e.isDirectory() ? walk(`${d}/${e.name}`) : [`${d}/${e.name}`]));
  for (const f of [...walk("data"), "m5_monthly.parquet"]) db.registerFileBuffer(f, new Uint8Array(readFileSync(path.join(web, f))));
  const run = scratchRunner(db);

  // Explore starters over two tables of an attached database, as Explore
  // profiles them; the CLI reads the same tables from a database file.
  const SAMPLES = [
    [
      "orders",
      `SELECT i AS order_id, (DATE '2026-01-01' + INTERVAL (i % 120) DAY)::DATE AS day,
              ['North', 'South', 'East', 'West'][1 + i % 4] AS region, round(20 + (i * 37) % 180 + i / 50.0, 2) AS amount
       FROM range(1, 2001) t(i)`,
    ],
    ["weeks", `SELECT * FROM read_csv('data/supply-chain/weeks.csv')`],
  ];
  const samplesDb = path.join(tmp, "samples.duckdb");
  execFileSync("duckdb", [samplesDb, "-c", SAMPLES.map(([t, q]) => `CREATE TABLE ${t} AS ${q};`).join("\n")], { cwd: web });
  page.query("ATTACH ':memory:' AS samples");
  for (const [t, q] of SAMPLES) page.query(`CREATE TABLE samples.main.${t} AS ${q}`);
  const xp = await import("../../web/explore.js");
  const rowsOf = async (q) => JSON.parse(await run(q));
  const scripts = [];
  for (const dir of ["web/examples", "tests/fixtures/lint"])
    for (const f of readdirSync(path.join(root, dir)).filter((f) => f.endsWith(".sql")).sort())
      scripts.push({ name: f, file: path.join(root, dir, f) });
  for (const [t] of SAMPLES) {
    const fq = `"samples"."main"."${t}"`;
    const prof = await xp.profileTable(rowsOf, wasm.page_sql, fq);
    const file = path.join(tmp, `starter-${t}.sql`);
    writeFileSync(file, xp.starterDashboard({ name: t, fq, cards: prof.cards }));
    scripts.push({ name: `starter-${t}.sql`, file, db: samplesDb });
  }

  const firstLine = (d) => (d.code === "sql-error" ? { ...d, message: d.message.split(/ (?:Did you mean|LINE \d+:)/)[0].trim() } : d);
  let steps = 0;
  for (const { name, file, db: dbFile } of scripts) {
    const script = readFileSync(file, "utf8");
    const t0 = Date.now();
    const got = await lintScript(script, { step: wasm.lint_step, run, strict: true });
    const ms = Date.now() - t0;
    let out;
    try {
      out = execFileSync(DASH, ["--check", file, "--json", "--strict", ...(dbFile ? ["--db", dbFile] : [])], { cwd: web, encoding: "utf8", maxBuffer: 1 << 26 });
    } catch (e) {
      out = e.stdout; // exit 1: the lint found errors (or --strict warnings)
    }
    const want = JSON.parse(out);
    const noLoad = want.diagnostics.find((d) => d.code === "sql-error" && /^LOAD\s/i.test(d.sql));
    if (noLoad) {
      const errors = got.diagnostics.filter((d) => d.severity === "error");
      assert.deepEqual(errors, [], `${name}: errors in the browser`);
      console.log(`lint_test: ${name}: no native reference (the CLI can't ${noLoad.sql}); clean in the browser: ${got.diagnostics.length} diagnostics, ${ms} ms`);
      continue;
    }
    assert.deepEqual(got.diagnostics.map(firstLine), want.diagnostics.map(firstLine), `${name}: the browser's lint differs from dashboard --check`);
    assert.equal(got.ok, want.ok, `${name}: ok`);
    steps += got.steps;
    console.log(`lint_test: ${name}: ${got.diagnostics.length} diagnostics, ${got.steps} steps, ${got.queries} queries, ${ms} ms`);
  }
  assert.ok(steps > scripts.length, "some script needed more than one step");

  // The scratch sessions leave the page's database alone.
  const tables = page.query("SELECT database_name, table_name FROM duckdb_tables() ORDER BY ALL").toArray().map((r) => `${r.database_name}.${r.table_name}`);
  assert.deepEqual(tables, ["samples.orders", "samples.weeks"], "no table from a linted script in the page's databases");
  const dbs = page.query("SELECT database_name FROM duckdb_databases() WHERE database_name LIKE 'anofox_lint_%'").toArray();
  assert.equal(dbs.length, 0, "every scratch database is detached");
  // rowsJson: dates and decimals as the CLI's JSON reads them
  const r = JSON.parse(rowsJson(page.query("SELECT DATE '2026-01-02' AS d, TIMESTAMP '2026-01-02 03:04:05' AS ts, 12.50::DECIMAL(5,2) AS m, 3::BIGINT AS b")));
  assert.deepEqual(r, [{ d: "2026-01-02", ts: "2026-01-02 03:04:05", m: 12.5, b: 3 }]);
  console.log("lint_test: ok");
} finally {
  server.kill();
  rmSync(tmp, { recursive: true, force: true });
}
