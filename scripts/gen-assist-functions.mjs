// The functions the assistant's SQL may call (#145, sec-review-192-rereview2):
// web/assist/functions.js, from duckdb_functions() of the DuckDB-Wasm the
// tests use (tests/web) with json, anofox_statistics and anofox_forecast
// (web/localext) loaded. Every scalar, aggregate and macro is allowed except
// the reviewed denials below; a function this list has not seen (another
// extension, a newer DuckDB) is refused by the assistant's gate. After a
// DuckDB-Wasm or extension upgrade: run it, review what it prints, commit.
//
//   node scripts/gen-assist-functions.mjs           write web/assist/functions.js
//   node scripts/gen-assist-functions.mjs --check   exit 1 if it is stale
process.env.DATAZOO_DISABLE_TELEMETRY = "1"; // no usage telemetry from anofox extensions
process.env.DATAZOO_NO_BANNER = "1";
import { createRequire } from "module";
import { spawn } from "child_process";
import { existsSync, readFileSync, writeFileSync } from "fs";
import path from "path";
import { fileURLToPath } from "url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
export const OUT = "web/assist/functions.js";
// Reviewed denials: they reach outside the session, change its state, or carry
// a value into an error message.
const DENIED = {
  json_serialize_plan: "binds the query held in its string: reads files and URLs",
  json_execute_serialized_sql: "runs the query held in its string",
  current_setting: "reads the session's settings",
  nextval: "advances a sequence of the page's database",
  currval: "reads a sequence of the page's database",
  setseed: "changes the session's random state",
  write_log: "writes to the database's log",
  error: "its message is its argument",
  make_type: "binds a type named by a string; its error echoes the string",
  sleep_ms: "holds the worker",
  pg_sleep: "holds the worker",
};
// Reviewed and allowed: named like SQL text, paths, settings or state, but they
// only compute on their arguments.
const REVIEWED = new Set(
  "aft_survival cast_to_type current_catalog current_query current_query_id elasticnet_path_agg format_pg_type format_type formatreadabledecimalsize formatreadablesize get_type has_sequence_privilege in_search_path json_deserialize_sql json_extract_path json_extract_path_text json_serialize_sql json_type kaplan_meier_agg lars_path_agg log log10 log2 log_loss_agg log_rank_test_agg logistic_augment_agg logistic_fit_agg logistic_partial_residuals_agg normalized_interval parse_dirpath parse_duckdb_log_message parse_filename parse_path pg_conf_load_time replace_type ridge_path_agg ts_matrix_profile_period url_decode url_encode vector_type".split(" ")
);
const LOOKS = /(sql|query|plan|path|file|url|http|read|load|exec|setting|env|secret|log|seq|seed|sleep|val$|write|error|type$|attach|extension)/;
// Table functions the parser also emits as a scalar call (SELECT unnest(l)).
const PARSER_EXTRA = ["unnest", "unlist"];
// Native table functions whose VARCHAR first argument names a relation (the
// table macros' relation arguments come from their bodies).
const NATIVE_RELATION = { pragma_table_info: [[0, "col0"]], ts_fill_forward_operator: [[0, "col0"]], anofox_fcst_ts_fill_forward_operator: [[0, "col0"]] };

// The DuckDB-Wasm connection the browser (and the tests) use, with json,
// anofox_statistics and anofox_forecast loaded from web/localext, served
// locally (the blocking bundle fetches extensions synchronously).
async function collect() {
  const require = createRequire(path.join(root, "tests/web/package.json"));
  const duckdb = require("@duckdb/duckdb-wasm/dist/duckdb-node-blocking.cjs");
  const dist = path.dirname(require.resolve("@duckdb/duckdb-wasm/dist/duckdb-node-blocking.cjs"));
  const db = await duckdb.createDuckDB(
    { eh: { mainModule: path.join(dist, "duckdb-eh.wasm"), mainWorker: path.join(dist, "duckdb-node-eh.worker.cjs") } },
    new duckdb.VoidLogger(),
    duckdb.NODE_RUNTIME
  );
  await db.instantiate();
  db.open({ allowUnsignedExtensions: true });
  const c = db.connect();
  const rows = (s) => JSON.parse(JSON.stringify(c.query(s).toArray().map((r) => r.toJSON()), (_, v) => (typeof v === "bigint" ? Number(v) : v)));
  const port = 19500 + (process.pid % 400);
  const server = spawn("python3", ["-u", "-m", "http.server", String(port), "--bind", "127.0.0.1"], {
    cwd: path.join(root, "web/localext"),
    stdio: ["ignore", "pipe", "ignore"],
  });
  await new Promise((ok) => server.stdout.once("data", ok));
  try {
    c.query("INSTALL json; LOAD json");
    c.query(`SET custom_extension_repository='http://127.0.0.1:${port}'`);
    for (const e of ["anofox_statistics", "anofox_forecast"]) {
      c.query(`FORCE INSTALL ${e}`);
      c.query(`LOAD ${e}`);
    }
    c.query("RESET custom_extension_repository");
    const version = rows("SELECT version() AS v")[0].v;
    const exts = rows("SELECT extension_name AS n, extension_version AS v FROM duckdb_extensions() WHERE loaded ORDER BY 1");
    const fns = rows("SELECT lower(function_name) AS f, function_type AS t, parameters AS p, macro_definition AS m FROM duckdb_functions() WHERE internal");
    return { version, exts, fns };
  } finally {
    server.kill();
  }
}

// {scalars, denied (Map), builtin, rel, review} from one duckdb_functions() rowset.
function build(fns) {
  const scalarTypes = new Set(["scalar", "aggregate", "macro"]);
  const denied = new Map(Object.entries(DENIED));
  for (const r of fns) if (r.t === "macro" && /\bFROM\s+(?!duckdb_)[a-z_][a-z0-9_]*\s*\(/i.test(r.m || "")) denied.set(r.f, "its body reads a table function");
  const scalars = [...new Set([...fns.filter((r) => scalarTypes.has(r.t)).map((r) => r.f), ...PARSER_EXTRA])].filter((f) => !denied.has(f)).sort();
  const review = scalars.filter((f) => LOOKS.test(f) && !REVIEWED.has(f));
  const builtin = [...new Set(fns.map((r) => r.f))].sort();
  const relNames = new Set();
  for (const r of fns) if (r.t === "table_macro") for (const m of (r.m || "").matchAll(/query_table\(\s*CAST\(\s*"?([a-z_][a-z0-9_]*)"?\s+AS\s+VARCHAR/gi)) relNames.add(m[1].toLowerCase());
  const rel = { ...NATIVE_RELATION };
  for (const r of fns)
    if (r.t === "table_macro") {
      const pairs = (r.p || []).map((p, i) => [i, String(p).toLowerCase()]).filter(([, p]) => relNames.has(p));
      if (!pairs.length) continue;
      const cur = rel[r.f] || [];
      for (const x of pairs) if (!cur.some(([i, p]) => i === x[0] && p === x[1])) cur.push(x);
      rel[r.f] = cur.sort((a, b) => a[0] - b[0] || (a[1] < b[1] ? -1 : 1));
    }
  return { scalars, denied, builtin, rel, review };
}

// ---- rendering web/assist/functions.js ----

// Space-joined names wrapped at `width` characters, one chunk per line.
function wrapWords(words, width = 96) {
  const lines = [];
  let cur = "";
  for (const w of words) {
    const piece = `${w} `;
    if (cur && cur.length + piece.length > width) {
      lines.push(cur);
      cur = piece;
    } else {
      cur += piece;
    }
  }
  if (cur) lines.push(cur);
  if (lines.length) lines[lines.length - 1] = lines[lines.length - 1].replace(/ $/, "");
  return lines.length ? lines : [""];
}

function renderNameSet(varName, words) {
  const lines = wrapWords(words);
  const out = [`export const ${varName} = new Set(`];
  if (lines.length === 1) {
    out.push(`  ${JSON.stringify(lines[0])}.split(" ")`);
  } else {
    lines.forEach((l, i) => {
      const prefix = i === 0 ? "  (" : "   ";
      const suffix = i < lines.length - 1 ? " +" : `).split(" ")`;
      out.push(`${prefix}${JSON.stringify(l)}${suffix}`);
    });
  }
  out.push(");");
  return out.join("\n") + "\n";
}

function renderDeniedMap(denied) {
  const sorted = [...denied.entries()].sort((a, b) => (a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : 0));
  const lines = sorted.map(([k, v]) => `  [${JSON.stringify(k)}, ${JSON.stringify(v)}],`);
  return `export const SCALAR_DENIED = new Map([\n${lines.join("\n")}\n]);\n`;
}

function renderRelationArgs(rel) {
  const keys = Object.keys(rel).sort();
  const lines = keys.map((k) => `  ${JSON.stringify(k)}: ${JSON.stringify(rel[k])},`);
  return `export const RELATION_ARGS = {\n${lines.join("\n")}\n};\n`;
}

// What the four exports are: the allow-list a scalar/aggregate/window call is
// checked against (SCALAR_ALLOWED), the reviewed denials kept apart for their
// reasons (SCALAR_DENIED), every function name of every type so a temp macro
// can be refused when it shadows one (BUILTIN), and which argument of a table
// macro must name a relation of the catalog (RELATION_ARGS).
const EXPORTS_COMMENT = `// SCALAR_ALLOWED: the scalar, aggregate and window functions the assistant's
// SQL may call directly. SCALAR_DENIED: reviewed denials, with why, kept out
// of SCALAR_ALLOWED (defence in depth with web/assist.js's ALWAYS_DENIED).
// BUILTIN: every function name of every type (a temp macro may not take one).
// RELATION_ARGS: per table macro, the [index, parameter name] pairs whose
// argument must name a table of the catalog (the macro's body reads it
// through query_table()).
`;

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const { version, exts, fns } = await collect();
  const { scalars, denied, builtin, rel, review } = build(fns);
  console.log(`scalars: ${scalars.length}, denied: ${denied.size}, builtin: ${builtin.length}, relation keys: ${Object.keys(rel).length}`);
  console.log(`review: ${JSON.stringify(review)}`);
  if (review.length) {
    console.error(`review is not empty: resolve each name (add to DENIED or REVIEWED in ${path.relative(root, fileURLToPath(import.meta.url))}) before writing.`);
    process.exit(2);
  }
  const wasmVersion = JSON.parse(readFileSync(path.join(root, "tests/web/package.json"), "utf8")).dependencies["@duckdb/duckdb-wasm"];
  const extsText = exts.map((e) => `${e.n} ${e.v}`).join(", ");
  const header = `// Generated by scripts/gen-assist-functions.mjs from duckdb_functions() of DuckDB ${version} (DuckDB-Wasm ${wasmVersion}) with ${extsText}; do not edit.\n`;
  const body = [header, EXPORTS_COMMENT, renderNameSet("SCALAR_ALLOWED", scalars), renderDeniedMap(denied), renderNameSet("BUILTIN", builtin), renderRelationArgs(rel)].join("\n");
  const file = path.join(root, OUT);
  if (process.argv.includes("--check")) {
    if (!existsSync(file) || readFileSync(file, "utf8") !== body) {
      console.error(`${OUT} is stale: run node scripts/gen-assist-functions.mjs`);
      process.exit(1);
    }
    console.log(`${OUT}: current`);
  } else {
    writeFileSync(file, body);
    console.log(`wrote ${OUT} (${body.length} bytes)`);
  }
}
