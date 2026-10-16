// Explore data's starter dashboards (issue #97) pass `dashboard --check
// --strict` for every sample table: builds one DuckDB file with the tables of
// the examples (their CREATE statements, web/data/**, web/m5_monthly.parquet)
// plus edge cases (every type, empty, one row, wide, all-null), profiles each
// table with web/explore.js exactly as the Explore page does (the ::PAGED SQL
// from the wasm build), writes its starter dashboard and lints it against the
// file. Exit 1 if any starter has an error or a design/* warning.
//
//   wasm-pack build --target web --out-dir web/pkg --no-default-features --features wasm
//   cargo build --bin dashboard
//   node scripts/explore-starters.mjs [--dashboard target/debug/dashboard] [--duckdb duckdb] [--out DIR]
import { execFileSync } from "child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, statSync, writeFileSync } from "fs";
import os from "os";
import path from "path";
import { CATALOG_SQL, profileTable, qid, starterDashboard } from "../web/explore.js";

const root = path.resolve(path.dirname(new URL(import.meta.url).pathname), "..");
const arg = (k, d) => {
  const i = process.argv.indexOf(k);
  return i > 0 ? process.argv[i + 1] : d;
};
const DUCKDB = arg("--duckdb", "duckdb");
const DASH = arg("--dashboard", path.join(root, "target/debug/dashboard"));
const out = arg("--out", mkdtempSync(path.join(os.tmpdir(), "explore-starters-")));
mkdirSync(out, { recursive: true });
for (const [what, p] of [["web/pkg (wasm-pack build)", path.join(root, "web/pkg/anofox_visualization_bg.wasm")], ["the dashboard CLI (cargo build --bin dashboard)", DASH]])
  if (!existsSync(p)) {
    console.error(`explore-starters: ${what} missing: ${p}`);
    process.exit(2);
  }

const wasm = await import(path.join(root, "web/pkg/anofox_visualization.js"));
wasm.initSync({ module: readFileSync(path.join(root, "web/pkg/anofox_visualization_bg.wasm")) });

const dbFile = path.join(out, "samples.duckdb");
rmSync(dbFile, { force: true });
const duck = (sql, { readonly = false } = {}) =>
  execFileSync(DUCKDB, [...(readonly ? ["-readonly"] : []), dbFile, "-json", "-c", sql], {
    cwd: path.join(root, "web"), // examples read 'data/…' relative to web/
    encoding: "utf8",
    stdio: ["ignore", "pipe", "pipe"],
    maxBuffer: 1 << 28,
  });
const rows = (sql) => {
  const s = duck(sql, { readonly: true }).trim();
  return s ? JSON.parse(s.replace(/\bNaN\b|-?\bInfinity\b/g, "null")) : [];
};

// 1. the examples' tables: each CREATE statement in order (one that needs an
// extension or a dashboard variable simply fails and is left out)
const creates = (text) =>
  text
    .split(/;[ \t]*(?:--[^\n]*)?\n/)
    .map((s) => s.replace(/^(\s*--[^\n]*\n)+/, "").trim())
    .filter((s) => /^CREATE\s+(OR\s+REPLACE\s+)?(TEMP\s+)?(TABLE|VIEW|MACRO)\b/i.test(s) && !/getvariable/i.test(s));
const appSrc = readFileSync(path.join(root, "web/app.js"), "utf8");
const sources = [
  ["app.js", [...appSrc.matchAll(/`(CREATE OR REPLACE TABLE[\s\S]*?;)`/g)].map((m) => m[1]).join("\n")],
  ...readdirSync(path.join(root, "web/examples"))
    .filter((f) => f.endsWith(".sql"))
    .map((f) => [f, readFileSync(path.join(root, "web/examples", f), "utf8")]),
];
let made = 0,
  skipped = 0;
for (const [, text] of sources)
  for (const st of creates(text)) {
    try {
      duck(st.replace(/^CREATE\s+(OR\s+REPLACE\s+)?TEMP\s+/i, "CREATE OR REPLACE "));
      made++;
    } catch (_) {
      skipped++;
    }
  }
// 2. the stored showcase data and the M5 sample
const walk = (d) => readdirSync(d).flatMap((f) => (statSync(path.join(d, f)).isDirectory() ? walk(path.join(d, f)) : [path.join(d, f)]));
for (const f of [...walk(path.join(root, "web/data")), path.join(root, "web/m5_monthly.parquet")]) {
  const rel = path.relative(path.join(root, "web"), f);
  const name = "file_" + rel.replace(/\.[^.]+$/, "").replace(/[^A-Za-z0-9]+/g, "_").toLowerCase();
  const reader = f.endsWith(".parquet") ? "read_parquet" : "read_csv_auto";
  duck(`CREATE OR REPLACE TABLE ${qid(name)} AS SELECT * FROM ${reader}('${rel}')`);
  made++;
}
// 3. edge cases
duck(`
CREATE OR REPLACE TABLE edge_types AS
SELECT i AS id, 'c' || (i % 4) AS cat, i % 2 = 0 AS flag, (i * 1.5)::DOUBLE AS amount, (i % 7)::DECIMAL(10, 2) AS price,
       DATE '2024-01-01' + i::INT AS day, TIMESTAMP '2024-01-01 08:00:00' + INTERVAL (i) HOUR AS ts, [i, i + 1] AS tags,
       {'a': i} AS obj, NULL::VARCHAR AS empty_col, 'same' AS constant, CASE WHEN i % 3 = 0 THEN 'x' ELSE i::VARCHAR END AS mixed,
       CASE WHEN i = 50 THEN 1e9 ELSE i END AS spiky, uuid() AS uid
FROM range(200) t(i);
CREATE OR REPLACE TABLE edge_empty (id INTEGER, name VARCHAR, day DATE, v DOUBLE);
CREATE OR REPLACE TABLE edge_one AS SELECT 1 AS id, 'only' AS name, DATE '2025-05-05' AS day, 3.5 AS v;
CREATE OR REPLACE TABLE "edge ""quoted"" names" AS SELECT i AS "order id", 'k' || (i % 3) AS "it's a category", i * 2.0 AS "value ($)", DATE '2023-01-01' + (i * 30)::INT AS "first day" FROM range(40) t(i);
CREATE OR REPLACE TABLE edge_wide AS SELECT ${Array.from({ length: 40 }, (_, k) => `i * ${k + 1} AS m${k}, 'g' || ((i + ${k}) % 5) AS g${k}`).join(", ")} FROM range(100) t(i);
CREATE OR REPLACE TABLE edge_dates_only AS SELECT DATE '2020-01-01' + i::INT AS d FROM range(1000) t(i);
CREATE OR REPLACE TABLE edge_many_cats AS SELECT 'p' || (i % 400) AS product, i % 9 AS bucket, i * 0.1 AS v FROM range(2000) t(i);
CREATE OR REPLACE VIEW edge_view AS SELECT cat, sum(amount) AS amount FROM edge_types GROUP BY cat;
`);
made += 8;

// Profile every table and view, write its starter, lint it.
const tables = rows(CATALOG_SQL).filter((r) => r.schema_name === "main");
const pageSql = (base, page) => wasm.page_sql(base, page);
const failures = [];
for (const t of tables) {
  const fq = `${qid(t.database_name)}.${qid(t.schema_name)}.${qid(t.table_name)}`;
  let sql;
  try {
    const prof = await profileTable(async (q) => rows(q), pageSql, fq);
    sql = starterDashboard({ name: t.table_name, fq, cards: prof.cards });
  } catch (e) {
    failures.push(`${t.table_name}: profile failed: ${String(e.stderr || e.message || e).slice(0, 400)}`);
    continue;
  }
  const file = path.join(out, t.table_name.replace(/[^A-Za-z0-9_]+/g, "_") + ".sql");
  writeFileSync(file, sql);
  let res;
  try {
    res = JSON.parse(execFileSync(DASH, ["--check", file, "--strict", "--json", "--db", dbFile], { encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] }));
  } catch (e) {
    res = e.stdout ? JSON.parse(e.stdout) : { ok: false, diagnostics: [{ severity: "error", code: "cli", message: String(e.stderr || e) }] };
  }
  const bad = res.diagnostics.filter((d) => d.severity === "error" || String(d.code).startsWith("design/"));
  const note = res.diagnostics.filter((d) => !bad.includes(d)).map((d) => d.code);
  if (!res.ok || bad.length)
    failures.push(`${t.table_name} (${file}):\n` + bad.map((d) => `    ${d.severity} [stmt ${d.stmt}] ${d.code}: ${d.message}\n      ${d.sql}`).join("\n"));
  else console.log(`ok  ${t.table_name}${note.length ? `  (notes: ${[...new Set(note)].join(", ")})` : ""}`);
}
console.log(`\n${tables.length} tables (${made} created, ${skipped} example statements left out); starters in ${out}`);
if (failures.length) {
  console.error(`\n${failures.length} starter dashboard(s) failed --check --strict:\n` + failures.join("\n"));
  process.exit(1);
}
