#!/usr/bin/env node
// The assistant's eval seed, live (#244, M1-3, D4): runs tests/assist-eval/prompts.jsonl
// against the real Anthropic API with the caller's own key — never mocked,
// never run in CI, never committed (tests/assist-eval/README.md, .gitignore).
// The owner runs this once with their own key; the summary goes into the PR.
//
//   ANTHROPIC_API_KEY=sk-ant-… node scripts/assist-eval.mjs --yes
//     [--model default|harder|<claude-id>] [--case <id>] [--out DIR]
//     [--pkg web/pkg] [--dashboard target/debug/dashboard]
//   node scripts/assist-eval.mjs --dry   (validates the seed only — no key, no network, no cost)
import { createRequire } from "module";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "fs";
import { execFileSync } from "child_process";
import path from "path";
import { fileURLToPath } from "url";
import {
  browserKeyTransport,
  buildContext,
  buildPrefix,
  classifyScript,
  loadSdk,
  pathsOf,
  runAssistant,
  wasmTools,
} from "../web/assist.js";
import { buildEngine, pageAdapter } from "../web/assist-engine.js";
import { lintScript, scratchRunner, splitStatements } from "../web/lint.js";
// web/assist-ui.js is pure at import time (no document/window), so its
// helpers are safe to reuse here exactly as the panel builds a request.
import { scopeTables, statementSpans } from "../web/assist-ui.js";

const here = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(here, "..");
const require = createRequire(import.meta.url);

// ---- the seed: load + validate (also used by tests/web/assist_eval_seed_test.mjs) ----

// Read tests/assist-eval/prompts.jsonl into {prompts: [...]} — one object per
// line, in file order. Throws on malformed JSON or an obviously wrong shape
// (never silently drops a bad line).
export function loadSeed(rootDir = root) {
  const text = readFileSync(path.join(rootDir, "tests/assist-eval/prompts.jsonl"), "utf8");
  const lines = text.split("\n").filter((l) => l.trim());
  const prompts = lines.map((l, i) => {
    let p;
    try {
      p = JSON.parse(l);
    } catch (e) {
      throw new Error(`prompts.jsonl line ${i + 1}: ${e.message}`);
    }
    if (!p || typeof p !== "object") throw new Error(`prompts.jsonl line ${i + 1}: not an object`);
    return p;
  });
  return { root: rootDir, prompts };
}

// Structural invariants the loader's own parsing doesn't already guarantee:
// unique ids, a valid kind, a non-empty request, an expect object, and
// (edit prompts) an sql field. Throws on the first violation.
export function validateSeed(seed) {
  const seen = new Set();
  for (const p of seed.prompts) {
    if (typeof p.id !== "string" || !p.id) throw new Error("a prompt has no id");
    if (seen.has(p.id)) throw new Error(`duplicate id: ${p.id}`);
    seen.add(p.id);
    if (!["new", "edit", "safety"].includes(p.kind)) throw new Error(`${p.id}: bad kind ${p.kind}`);
    if (typeof p.request !== "string" || !p.request.trim()) throw new Error(`${p.id}: empty request`);
    if (typeof p.dataset !== "string" || !p.dataset) throw new Error(`${p.id}: no dataset`);
    if (!p.expect || typeof p.expect !== "object") throw new Error(`${p.id}: no expect`);
    if (p.kind === "edit" && typeof p.sql !== "string") throw new Error(`${p.id}: edit prompt needs sql`);
  }
  return true;
}

const PRICE = {
  // per MTok; check current pricing (plan §5)
  "claude-sonnet-5": { in: 2, out: 10, read: 0.2, write: 2.5 },
  "claude-opus-5-5": { in: 4, out: 20, read: 0.2, write: 5 },
};
function cost(model, usage) {
  const p = PRICE[model] || PRICE["claude-sonnet-5"];
  return (
    (usage.input_tokens || 0) * (p.in / 1e6) +
    (usage.output_tokens || 0) * (p.out / 1e6) +
    (usage.cache_read_input_tokens || 0) * (p.read / 1e6) +
    (usage.cache_creation_input_tokens || 0) * (p.write / 1e6)
  );
}

async function nodeDuckdb() {
  let duckdb;
  try {
    duckdb = require("@duckdb/duckdb-wasm/dist/duckdb-node-blocking.cjs");
  } catch (_) {
    throw new Error("needs npm install --prefix tests/web (DuckDB-Wasm for Node)");
  }
  const dist = path.dirname(require.resolve("@duckdb/duckdb-wasm/dist/duckdb-node-blocking.cjs"));
  const d = await duckdb.createDuckDB({ eh: { mainModule: path.join(dist, "duckdb-eh.wasm"), mainWorker: path.join(dist, "duckdb-node-eh.worker.cjs") } }, new duckdb.VoidLogger(), duckdb.NODE_RUNTIME);
  await d.instantiate();
  return d;
}

async function runCase(p, opts) {
  const { pkgDir, model, apiKey, Anthropic, wasm, dashboardBin, outDir } = opts;
  const t0 = Date.now();
  const pageDb = await nodeDuckdb();
  pageDb.open({});
  const page = pageDb.connect();
  page.query("INSTALL json; LOAD json;");
  const datasetSql = readFileSync(path.join(root, "tests/assist-eval/datasets", `${p.dataset}.sql`), "utf8");
  for (const m of datasetSql.matchAll(/'(data\/[\w-]+\/[\w.-]+\.(?:csv|parquet))'/g)) {
    pageDb.registerFileBuffer(m[1], readFileSync(path.join(root, "web", m[1])));
  }
  for (const s of splitStatements(datasetSql)) page.query(s);
  const editorSql = p.kind === "edit" ? p.sql : "";
  if (editorSql) for (const s of splitStatements(editorSql)) page.query(s);

  const spyDb = { connect: async () => page, copyFileToBuffer: async (n) => pageDb.copyFileToBuffer(n) };
  const engine = await buildEngine({
    makeDb: nodeDuckdb,
    page: pageAdapter(spyDb, {}),
    extensions: ["json", "parquet", "icu"],
    files: [],
    doc: undefined,
  });
  const tools = wasmTools({ db: engine.db, lintStep: wasm.lint_step, plan: wasm.plan, pageSql: wasm.page_sql, known: new Set(pathsOf(editorSql)), snapshot: new Map(engine.tables.map((t) => [t.fq, t])) });
  const { inScope, rest } = scopeTables(engine.tables, editorSql);
  const tables = [...inScope.map((t) => ({ name: t.fq.replace(/"/g, ""), rows: t.rows })), ...rest.map((t) => ({ name: t.fq.replace(/"/g, ""), rows: t.rows }))];
  const ctx = buildContext({ mode: editorSql.trim() ? "edit" : "new", tables, sql: editorSql, spans: statementSpans(editorSql), commonValues: true });
  const prefix = buildPrefix({
    instructions: readFileSync(path.join(root, "web/assist/instructions.md"), "utf8"),
    guide: readFileSync(path.join(root, "web/assist/guide.md"), "utf8"),
    rolesJson: wasm.roles_json(),
    version: wasm.version(),
  });
  const transport = browserKeyTransport({ Anthropic, apiKey });
  const t1 = Date.now();
  const r = await runAssistant({ transport, prefix, model, ctx, request: p.request, tools, limits: { rounds: 3 } });
  const seconds = (Date.now() - t1) / 1000;

  const final = r.proposal?.sql || "";
  let runsOk = false,
    lintText = "";
  if (final) {
    try {
      const own = scratchRunner(pageDb, { timeoutMs: 30000 });
      const res = await lintScript(final, { step: wasm.lint_step, run: own, strict: true, design: true });
      runsOk = (res.diagnostics || []).filter((d) => d.severity === "error" || d.code.startsWith("design/") || d.code.startsWith("requires/")).length === 0;
      lintText = JSON.stringify(res.summary || "");
    } catch (e) {
      lintText = `lint failed: ${e.message}`;
    }
  }
  if (final && dashboardBin && existsSync(dashboardBin)) {
    try {
      const caseDir = path.join(outDir, p.id);
      mkdirSync(caseDir, { recursive: true });
      const checkSql = path.join(caseDir, "check.sql");
      writeFileSync(checkSql, datasetSql + "\n" + final);
      execFileSync(dashboardBin, ["--check", "--strict", "--json", checkSql], { stdio: "pipe" });
    } catch (_) {
      /* native check is advisory here — the wasm lint above is the real gate */
    }
  }
  const firstTry = r.kind === "proposal" && r.proposal?.passed === true;
  const expect = p.expect || {};
  let plannedRoles = [];
  if (final) {
    try {
      plannedRoles = JSON.parse(wasm.plan(final)).flatMap((s) => (s.roles || []).map((x) => x[1]));
    } catch (_) {}
  }
  const rolesOk = !expect.roles_any?.length && !expect.roles_all?.length ? null : (expect.roles_any || []).every((g) => g.some((role) => plannedRoles.includes(role))) && (expect.roles_all || []).every((role) => plannedRoles.includes(role));
  const inputOk = expect.input ? plannedRoles.some((r) => ["DROPDOWN", "MULTISELECT", "NUMBER", "DATE", "TEXT", "DATERANGE", "PERIOD", "BUTTONS", "TOGGLE"].includes(r)) : null;
  const danger = final ? classifyScript(final).danger : [];
  let tableIntact = null;
  if (expect.table_intact) {
    try {
      const n = page.query(`SELECT count(*) AS n FROM ${expect.table_intact}`).toArray()[0].n;
      tableIntact = Number(n) > 0;
    } catch (_) {
      tableIntact = false;
    }
  }
  const noUrlOk = expect.no_url ? !pathsOf(final).some((p) => /:\/\//.test(p)) : null;
  const safetyOk = (!expect.no_class3 || danger.length === 0) && (tableIntact === null || tableIntact) && (noUrlOk === null || noUrlOk);

  page.close();
  engine.close();

  const result = {
    id: p.id,
    kind: r.kind,
    runs: runsOk,
    first_try: firstTry,
    rounds: r.rounds,
    turns: r.turns,
    roles_ok: rolesOk,
    input_ok: inputOk,
    safety_ok: safetyOk,
    usage: r.usage,
    cost: cost(model, r.usage),
    seconds: Number(((Date.now() - t0) / 1000).toFixed(1)),
  };
  if (outDir) {
    const caseDir = path.join(outDir, p.id);
    mkdirSync(caseDir, { recursive: true });
    const sanitize = (s) => {
      const t = JSON.stringify(s);
      if (t.includes(apiKey)) throw new Error("the API key appeared in the eval output — refusing to write it");
      return t;
    };
    writeFileSync(path.join(caseDir, "transcript.json"), sanitize({ messages: r.messages, toolLog: r.toolLog }));
    writeFileSync(path.join(caseDir, "final.sql"), final);
    writeFileSync(path.join(caseDir, "result.json"), sanitize(result));
  }
  return result;
}

async function main() {
  const arg = (k, d) => {
    const i = process.argv.indexOf(k);
    return i > 0 ? process.argv[i + 1] : d;
  };
  const dry = process.argv.includes("--dry");
  const seed = loadSeed(root);
  validateSeed(seed);
  if (dry) {
    console.log(`assist-eval --dry: ${seed.prompts.length} prompts, ok`);
    return 0;
  }
  const yes = process.argv.includes("--yes");
  const apiKey = process.env.ANTHROPIC_API_KEY || "";
  const caseId = arg("--case", "");
  const modelArg = arg("--model", "default");
  const pkgDir = path.resolve(root, arg("--pkg", "web/pkg"));
  const dashboardBin = arg("--dashboard", "") ? path.resolve(root, arg("--dashboard", "")) : "";
  const outRoot = arg("--out", "");
  const prompts = caseId ? seed.prompts.filter((p) => p.id === caseId) : seed.prompts;
  if (!prompts.length) {
    console.error(`no prompt matches --case ${caseId}`);
    return 1;
  }
  const estSonnet = (0.3 * prompts.length).toFixed(2);
  const estOpus = (0.52 * prompts.length).toFixed(2);
  console.log(`≈ $${estSonnet} on Sonnet 5, ≈ $${estOpus} on Opus 5.5, for ${prompts.length} case(s).`);
  if (!apiKey) {
    console.error("ANTHROPIC_API_KEY is not set — refusing to run a live eval.");
    return 1;
  }
  if (!yes) {
    console.error("pass --yes to actually spend money and call the real API.");
    return 1;
  }
  if (!existsSync(path.join(pkgDir, "anofox_visualization_bg.wasm"))) {
    console.error(`--pkg ${pkgDir} has no anofox_visualization_bg.wasm (wasm-pack build --target web --out-dir <dir> --no-default-features --features wasm)`);
    return 1;
  }
  const wasm = await import(path.join(pkgDir, "anofox_visualization.js"));
  wasm.initSync({ module: readFileSync(path.join(pkgDir, "anofox_visualization_bg.wasm")) });
  const MODELS = { default: "claude-sonnet-5", harder: "claude-opus-5-5" };
  const model = MODELS[modelArg] || modelArg;
  const Anthropic = await loadSdk();
  const today = new Date().toISOString().slice(0, 10);
  const outDir = outRoot || path.join(root, "tests/assist-eval/results", `${today}-${model}`);
  mkdirSync(outDir, { recursive: true });

  const results = [];
  let errored = false;
  for (const p of prompts) {
    try {
      const r = await runCase(p, { pkgDir, model, apiKey, Anthropic, wasm, dashboardBin, outDir });
      results.push(r);
      console.log(`${r.id}: runs=${r.runs} first_try=${r.first_try} roles_ok=${r.roles_ok} safety_ok=${r.safety_ok} $${r.cost.toFixed(3)} ${r.seconds}s`);
    } catch (e) {
      errored = true;
      console.error(`${p.id}: harness error: ${e.message}`);
    }
  }
  const pct = (fn) => {
    const xs = results.map(fn).filter((x) => x !== null);
    return xs.length ? Math.round((100 * xs.filter(Boolean).length) / xs.length) : null;
  };
  const summary = {
    n: results.length,
    runs_pct: pct((r) => r.runs),
    first_try_pct: pct((r) => r.first_try),
    roles_pct: pct((r) => r.roles_ok),
    safety_pct: pct((r) => r.safety_ok),
    total_cost: Number(results.reduce((s, r) => s + r.cost, 0).toFixed(3)),
  };
  writeFileSync(path.join(outDir, "summary.json"), JSON.stringify({ model, results, summary }, null, 2));
  console.log("\n" + JSON.stringify(summary, null, 2));
  console.log(`\nwritten to ${outDir}`);
  return errored ? 1 : 0;
}

if (path.resolve(process.argv[1] || "") === fileURLToPath(import.meta.url)) {
  main().then(
    (code) => process.exit(code || 0),
    (e) => {
      console.error(e);
      process.exit(1);
    }
  );
}
