// #844 (M3-2): sample rows are an opt-in per conversation. For every eval
// dataset (tests/assist-eval/datasets/) and a fixture with personal columns,
// a scripted conversation profiles every table and runs SELECT * on it; the
// serialised requests are scanned for the data's own text values — every
// value the profile doesn't send as a common value. Without the opt-in, none
// may appear; with it, rows do (a few per table, the first rows of a
// query), and a column that looks personal is still withheld.
import { createRequire } from "module";
import { readFileSync, readdirSync } from "fs";
import path from "path";
import assert from "assert/strict";

const here = path.dirname(new URL(import.meta.url).pathname);
const root = path.resolve(here, "../..");
const require = createRequire(path.join(here, "package.json"));
const A = await import(path.join(root, "web/assist.js"));
const { splitStatements } = await import(path.join(root, "web/lint.js"));
const { datasetFiles } = await import(path.join(root, "scripts/assist-eval.mjs"));
const wasm = await import(path.join(root, "web/pkg/anofox_visualization.js"));
wasm.initSync({ module: readFileSync(path.join(root, "web/pkg/anofox_visualization_bg.wasm")) });
const duckdb = require("@duckdb/duckdb-wasm/dist/duckdb-node-blocking.cjs");
const dist = path.dirname(require.resolve("@duckdb/duckdb-wasm/dist/duckdb-node-blocking.cjs"));
async function rawDb() {
  const d = await duckdb.createDuckDB({ eh: { mainModule: path.join(dist, "duckdb-eh.wasm"), mainWorker: path.join(dist, "duckdb-node-eh.worker.cjs") } }, new duckdb.VoidLogger(), duckdb.NODE_RUNTIME);
  await d.instantiate();
  d.open({});
  return d;
}

// ---- the pure parts ----
{
  const s = A.sampleOf(
    [["region", "VARCHAR"], ["email", "VARCHAR"], ["note", "VARCHAR"], ["n", "INTEGER"]],
    [
      { region: "EU", email: "ann@example.org", note: "x".repeat(300), n: 1 },
      { region: "US", email: "bob@example.org", note: "ok", n: 2 },
      { region: "US", email: "cy@example.org", note: "ok", n: 3 },
    ],
    2
  );
  assert.deepEqual(s.columns, ["region", "note", "n"]);
  assert.deepEqual(s.withheld, ["email"]);
  assert.equal(s.rows.length, 2);
  assert.equal(s.rows[0][1].length, 201, "long text is cut");
  // a value that looks personal withholds its column, whatever its name
  assert.deepEqual(A.sampleOf([["contact_info", "VARCHAR"]], [{ contact_info: "+49 30 1234567" }], 5).withheld, ["contact_info"]);
  // the context carries rows only with the flag
  const t = [{ name: "x", rows: 1, cards: [], sample: { columns: ["a"], rows: [["v"]] } }];
  assert.equal(A.buildContext({ tables: t }).tables[0].sample_rows, undefined);
  assert.equal(A.buildContext({ tables: t }).sample_rows, false);
  assert.deepEqual(A.buildContext({ tables: t, sampleRows: true }).tables[0].sample_rows, { columns: ["a"], rows: [["v"]] });
  assert.match(A.payloadPreview({ prefix: A.buildPrefix({ instructions: "i", guide: "g", rolesJson: wasm.roles_json() }), ctx: A.buildContext({}), request: "r", sampleRows: true }).note, /first 20 rows/);
}

// ---- the scan, per dataset ----
const prefix = A.buildPrefix({ instructions: "You build dashboards.", guide: "Guide.", rolesJson: wasm.roles_json(), version: "test" });
const PEOPLE = `CREATE TABLE people AS SELECT 'Customer ' || i || ' Lastname' || i AS customer_name, 'person' || i || '@mail.example' AS email,
  ['EU','US'][1 + i % 2] AS region, 'free text note number ' || i || ' about the order' AS note, i AS n FROM range(0, 80) t(i);`;
const datasets = readdirSync(path.join(root, "tests/assist-eval/datasets"))
  .filter((f) => f.endsWith(".sql"))
  .sort()
  .map((f) => ({ name: f, sql: readFileSync(path.join(root, "tests/assist-eval/datasets", f), "utf8") }));
datasets.push({ name: "people (fixture)", sql: PEOPLE });

let ids = 0;
const use = (name, input) => ({ type: "tool_use", id: `toolu_${++ids}`, name, input });
const turn = (content, stop = "tool_use") => ({ role: "assistant", content, stop_reason: stop, usage: { input_tokens: 10, output_tokens: 5 } });
function scripted(turns) {
  const sent = [];
  return {
    sent,
    name: "scripted",
    destination: "test",
    async send(req) {
      sent.push(JSON.stringify(req));
      const t = turns.shift();
      if (!t) throw new Error("no more turns");
      return t;
    },
  };
}

async function conversation(db, sampleOn) {
  const tools = A.wasmTools({ db, lintStep: wasm.lint_step, plan: wasm.plan, pageSql: wasm.page_sql, requireLocked: false, sampleRows: () => sampleOn });
  const list = JSON.parse(await tools.list_tables({ filter: "" }));
  const tables = [];
  for (const t of list) {
    await tools.profile_table({ table: t.name });
    tables.push({ name: t.name, rows: t.rows, cards: undefined, sample: sampleOn ? await tools.sampleTable({ table: t.name }) : undefined });
  }
  if (!sampleOn) await assert.rejects(() => tools.sampleTable({ table: list[0].name }), /sample rows are off/);
  const ctx = A.buildContext({ mode: "new", tables, sampleRows: sampleOn });
  const tr = scripted([turn(list.flatMap((t) => [use("profile_table", { table: t.name }), use("run_sql", { sql: `SELECT * FROM ${t.name} LIMIT 50`, purpose: "look at the rows" })])), turn([{ type: "text", text: "Done." }], "end_turn")]);
  await A.runAssistant({ transport: tr, prefix, ctx, request: "Build a dashboard", tools, limits: { callsPerTurn: 20 } });
  const sent = tr.sent.join("\n");
  // every call ran (a refused call would scan nothing)
  const bad = /INVALID_INPUT|\\"is_error\\":true|"is_error":true/.exec(sent);
  assert.ok(!bad, `the scripted calls ran: ${bad ? sent.slice(bad.index - 300, bad.index + 300) : ""}`);
  return { sent, tools, list };
}

// The data's own text values the profile doesn't send as common values.
async function canaries(db, tools, list) {
  const c = db.connect();
  const out = new Map(); // value -> column
  for (const t of list) {
    const p = JSON.parse(await tools.profile_table({ table: t.name }));
    for (const col of p.columns.filter((x) => x.kind === "text" && !x.common_values)) {
      const rows = c.query(`SELECT DISTINCT CAST("${col.name}" AS VARCHAR) AS v FROM ${t.name} WHERE length(CAST("${col.name}" AS VARCHAR)) >= 5 LIMIT 300`).toArray();
      for (const r of rows) out.set(r.toJSON().v, `${t.name}.${col.name}`);
    }
  }
  c.close();
  return out;
}

let scanned = 0;
for (const d of datasets) {
  const db = await rawDb();
  const c = db.connect();
  c.query("INSTALL json; LOAD json;");
  // the eval's own rule for a dataset's files (data/…/x.csv, m5_monthly.parquet)
  for (const f of datasetFiles(d.sql)) db.registerFileBuffer(f, new Uint8Array(readFileSync(path.join(root, "web", f))));
  for (const s of splitStatements(d.sql)) c.query(s);
  c.close();

  const off = await conversation(db, false);
  const values = await canaries(db, off.tools, off.list);
  const leaked = [...values.keys()].filter((v) => off.sent.includes(JSON.stringify(v).slice(1, -1)));
  assert.deepEqual(leaked, [], `${d.name}: row values in a request without the opt-in (${leaked.slice(0, 3).map((v) => values.get(v)).join(", ")})`);
  scanned += values.size;

  const on = await conversation(db, true);
  // the context is JSON inside the message text: its quotes are escaped
  assert.ok(on.sent.includes('\\"sample_rows\\":true'), `${d.name}: the context says sample rows are on`);
  if (d.name.startsWith("people")) {
    assert.ok(on.sent.includes("free text note number 3 about the order"), "people: with the opt-in, the context has rows");
    const i = on.sent.indexOf("tool_result");
    assert.ok(on.sent.includes("free text note number 15 about the order"), `people: with the opt-in, run_sql returns rows: ${on.sent.slice(i, i + 1500)}`);
    for (const v of values.keys()) {
      const col = values.get(v);
      if (col.endsWith(".email") || col.endsWith(".customer_name")) assert.ok(!on.sent.includes(v), `people: ${col} is personal: withheld even with the opt-in (${v})`);
    }
    assert.match(on.sent, /\\"withheld\\":\[\\"customer_name\\",\\"email\\"\]/, "people: the withheld columns are named");
  }
  db.terminate?.();
}
assert.ok(scanned > 100, `enough values scanned (${scanned})`);

console.log(`assist_sample_rows_test: ok (${datasets.length} datasets, ${scanned} values scanned)`);
