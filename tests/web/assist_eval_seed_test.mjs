// #244 M1-3, #845 M3-3: the assistant's eval seed (tests/assist-eval/) is
// well-formed — exactly 32 prompts in the plan's mix (§6, + #951's vague ones), unique ids, every dataset runs, every role the seed
// expects actually exists, and an edit prompt's `sql` plans cleanly. This is
// the only part of the eval the CI loop runs (no API key, no network, no
// cost) — the live run itself (scripts/assist-eval.mjs, D4) is manual.
import { createRequire } from "module";
import { existsSync, readFileSync, readdirSync } from "fs";
import path from "path";
import assert from "assert/strict";

const here = path.dirname(new URL(import.meta.url).pathname);
const root = path.resolve(here, "../..");
const require = createRequire(path.join(here, "package.json"));
const read = (p) => readFileSync(path.join(root, p), "utf8");

const wasmPkg = path.join(root, "web/pkg");
if (!existsSync(path.join(wasmPkg, "anofox_visualization_bg.wasm"))) {
  console.error("assist_eval_seed_test: FAILED — needs web/pkg (wasm-pack build)");
  process.exit(1);
}
const wasm = await import(path.join(wasmPkg, "anofox_visualization.js"));
wasm.initSync({ module: readFileSync(path.join(wasmPkg, "anofox_visualization_bg.wasm")) });

let duckdb = null;
try {
  duckdb = require("@duckdb/duckdb-wasm/dist/duckdb-node-blocking.cjs");
} catch (_) {}
if (!duckdb) {
  console.error("assist_eval_seed_test: FAILED — needs npm install --prefix tests/web");
  process.exit(1);
}
const dist = path.dirname(require.resolve("@duckdb/duckdb-wasm/dist/duckdb-node-blocking.cjs"));
async function rawDb() {
  const d = await duckdb.createDuckDB({ eh: { mainModule: path.join(dist, "duckdb-eh.wasm"), mainWorker: path.join(dist, "duckdb-node-eh.worker.cjs") } }, new duckdb.VoidLogger(), duckdb.NODE_RUNTIME);
  await d.instantiate();
  return d;
}

const { loadSeed, validateSeed, datasetFiles } = await import(path.join(root, "scripts/assist-eval.mjs"));

// ---- the seed's own shape ----
const seed = loadSeed(root);
assert.equal(seed.prompts.length, 32, `exactly 32 prompts, found ${seed.prompts.length}`);
{
  const n = (f) => seed.prompts.filter(f).length;
  assert.equal(n((p) => p.kind === "new" && !p.harder && !p.vague), 10, "10 new, simple");
  // #951: vague requests (2 new, the 2 Explore starts) must be asked about
  assert.equal(n((p) => p.vague), 4, "4 vague");
  for (const p of seed.prompts) assert.equal(!!p.vague, !!p.expect.asks, `${p.id}: vague ⇔ expect.asks`);
  assert.equal(n((p) => p.kind === "new" && p.harder), 8, "8 new, harder");
  assert.equal(n((p) => p.kind === "edit"), 7, "7 edits");
  assert.equal(n((p) => p.kind === "explore"), 2, "2 from Explore");
  assert.equal(n((p) => p.kind === "safety"), 3, "3 safety");
}
const ids = new Set();
for (const p of seed.prompts) {
  assert.ok(!ids.has(p.id), `duplicate id ${p.id}`);
  ids.add(p.id);
  assert.ok(["new", "edit", "explore", "safety"].includes(p.kind), `${p.id}: kind ${p.kind}`);
  assert.ok(typeof p.rubric === "string" && p.rubric.trim(), `${p.id}: a rubric for the judge`);
  assert.equal(typeof p.request, "string", `${p.id}: request`);
  assert.ok(p.request.trim(), `${p.id}: empty request`);
  assert.equal(typeof p.dataset, "string", `${p.id}: dataset`);
  assert.ok(existsSync(path.join(root, "tests/assist-eval/datasets", `${p.dataset}.sql`)), `${p.id}: dataset ${p.dataset} has no .sql file`);
  assert.ok(p.expect && typeof p.expect === "object", `${p.id}: expect`);
  if (p.kind === "edit") assert.equal(typeof p.sql, "string", `${p.id}: edit prompt needs sql`);
}
validateSeed(seed); // throws on anything the loader itself didn't already catch
// an Explore start names a table its dataset creates
for (const p of seed.prompts.filter((p) => p.kind === "explore")) {
  const sql = read(`tests/assist-eval/datasets/${p.dataset}.sql`);
  for (const t of p.scope) assert.match(sql, new RegExp(`CREATE (OR REPLACE )?TABLE ${t}\\b`, "i"), `${p.id}: ${t} is not in ${p.dataset}.sql`);
}

// ---- every role the seed expects exists in roles_json() ----
const roles = new Set(JSON.parse(wasm.roles_json()).roles.map((r) => r.token));
for (const p of seed.prompts) {
  const groups = [...(p.expect.roles_any || []), p.expect.roles_all || []];
  for (const g of groups) for (const r of g) assert.ok(roles.has(r), `${p.id}: role ${r} is not in roles_json()`);
}

// ---- every dataset runs in DuckDB-Wasm, registering the data/ files it names ----
for (const dataset of new Set(seed.prompts.map((p) => p.dataset))) {
  const sql = read(`tests/assist-eval/datasets/${dataset}.sql`);
  const db = await rawDb();
  db.open({});
  const c = db.connect();
  c.query("INSTALL json; LOAD json;");
  for (const file of datasetFiles(sql)) db.registerFileBuffer(file, new Uint8Array(readFileSync(path.join(root, "web", file))));
  try {
    c.query(sql);
  } catch (e) {
    assert.fail(`dataset ${dataset}.sql failed: ${e?.message || e}`);
  }
  c.close();
}

// ---- an edit prompt's sql plans without errors ----
for (const p of seed.prompts.filter((p) => p.kind === "edit")) {
  let planned;
  try {
    planned = JSON.parse(wasm.plan(p.sql));
  } catch (e) {
    assert.fail(`${p.id}: plan(sql) threw: ${e?.message || e}`);
  }
  assert.ok(Array.isArray(planned) && planned.length > 0, `${p.id}: plan(sql) is empty`);
}

// ---- scripts/assist-eval.mjs --dry logic (loadSeed/validateSeed importable and callable) ----
assert.equal(typeof loadSeed, "function");
assert.equal(typeof validateSeed, "function");

console.log("assist_eval_seed_test: ok");
