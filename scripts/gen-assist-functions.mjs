// The functions the assistant's SQL may call (#145, sec-review-192-rereview2):
// web/assist/functions.js, from duckdb_functions() of the DuckDB-Wasm the
// tests use (tests/web) with json, anofox_statistics and anofox_forecast
// (web/localext) loaded. Every scalar, aggregate and macro is allowed except
// the reviewed denials below; a function this list has not seen (another
// extension, a newer DuckDB) is refused by the assistant's gate. After a
// DuckDB-Wasm or extension upgrade: run it, review what it prints, commit.
// CI's web job runs --check with the extensions scripts/fetch-wasm-extensions.sh
// fetches (#224), so a new get.erpl.io build fails it until the list is
// regenerated.
//
//   node scripts/gen-assist-functions.mjs           write web/assist/functions.js
//   node scripts/gen-assist-functions.mjs --check   exit 1 if it is stale
process.env.DATAZOO_DISABLE_TELEMETRY = "1"; // no usage telemetry from anofox extensions
process.env.DATAZOO_NO_BANNER = "1";
import { createRequire } from "module";
import { spawn } from "child_process";
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "fs";
import os from "os";
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
// Table functions the parser also emits as a scalar call (SELECT unnest(l)),
// and icu's today() and get_current_time(): the browser autoloads icu, the
// Node DuckDB-Wasm this list is made from has no icu to load (#225).
const PARSER_EXTRA = ["unnest", "unlist", "today", "get_current_time"];
// Native table functions whose VARCHAR first argument names a relation (the
// table macros' relation arguments come from their bodies).
const NATIVE_RELATION = { pragma_table_info: [[0, "col0"]] };
// The anofox extensions' table function families (anofox_forecast's ts_* /
// anofox_fcst_ts_*, anofox_statistics' models and curves): their table macros
// go into ANOFOX_TABLES, their native table functions only when reviewed.
const ANOFOX_FAMILY =
  /^((anofox_fcst_)?ts_[a-z0-9_]+|(ols|wls|ridge|elasticnet|huber|quantile|logistic|poisson|binomial|gamma|negbinom|tweedie|theil_sen|ransac|bls|rls|pls|nnls|lars|isotonic|alm|aft|glmm)_[a-z_]+_by|(acf|pacf|roc_curve|pr_curve|calibration_curve|kaplan_meier|eb_shrink|tukey_hsd|dunn_test|pairwise_t_test|aid|aid_anomaly)_by)$/;
// Reviewed native table functions of those families: they read a subquery
// argument and run no SQL of their own.
const NATIVE_TABLE_ALLOWED = new Set(["ts_aggregate_hierarchy", "ts_combine_keys", "ts_split_keys", "ts_validate_separator"]);
// Reviewed and left out, with why.
const NATIVE_TABLE_DENIED = {
  ts_features_list: "traps DuckDB-Wasm (memory access out of bounds)",
  ts_features_config_template: "traps DuckDB-Wasm (memory access out of bounds)",
};
// A native *_operator is left out without a review: anofox_forecast's run SQL
// built from their string arguments, and trap DuckDB-Wasm (#223).
const NATIVE_OPERATOR = /_operator$/;
// Every query_table() / query() call of a macro body, and the one shape whose
// relation argument build() reads: query_table(CAST(<parameter> AS VARCHAR)).
// A quoted call ("query_table"(…)) is counted too (#239 N-1); QUERY_TABLE_PARAM
// stays unquoted-only, so a quoted query_table() ends up in review.
const QUERY_CALL = /\bquery(?:_table)?"?\s*\(/gi;
const QUERY_TABLE_PARAM = /\bquery_table\(\s*CAST\(\s*"?([a-z_][a-z0-9_]*)"?\s+AS\s+VARCHAR\s*\)\s*\)/gi;
// A call in a macro body: a plain or double-quoted name before "(" (#239 N-1).
const CALL = /(?:"([A-Za-z_][A-Za-z0-9_]*)"|\b([A-Za-z_][A-Za-z0-9_]*))\s*\(/g;
const callsOf = (body) => new Set([...String(body || "").matchAll(CALL)].map((m) => (m[1] || m[2]).toLowerCase()));
// File and URL readers (the gate's READER in web/assist.js).
const READER_CALL = /^(read_\w+|\w+_scan|glob|sniff_csv|parquet_\w+|st_read\w*|iceberg_\w+|delta_\w+)$/i;
// Core table functions a family macro may call (query/query_table: their own rule).
const CORE_TABLE_OK = new Set(["range", "generate_series", "unnest", "unlist", "query_table", "query"]);
// Reviewed natives outside the families that their table macros wrap (#239
// N-1): table in-out functions whose TABLE argument is the macro's relation and
// whose other arguments are column names or options; no SQL of their own.
const NATIVE_HELPER_ALLOWED = new Set(
  (
    "_ts_coverage_native _ts_cv_folds_native _ts_cv_forecast_native _ts_cv_hydrate_native " +
    "_ts_cv_split_native _ts_detect_changepoints_by_native _ts_features_native _ts_fill_forward_native _ts_fill_gaps_native " +
    "_ts_forecast_panel_native _ts_forecast_var_native _ts_mase_native _ts_metrics_native _ts_mstl_decomposition_native " +
    "_ts_quantile_loss_native _ts_rmae_native _ts_stats_by_native"
  ).split(" ")
);

// The DuckDB-Wasm connection the browser (and the tests) use, with json,
// anofox_statistics and anofox_forecast loaded from web/localext, served
// locally (the blocking bundle fetches extensions synchronously).
async function collect() {
  // #239 N-3: a fresh HOME, so DuckDB-Wasm's extension cache
  // (~/.duckdb/extensions/127.0.0.1:<port>/) can't serve an old build.
  const home = mkdtempSync(path.join(os.tmpdir(), "gen-assist-home-"));
  const oldHome = process.env.HOME;
  process.env.HOME = home;
  try {
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
  } finally {
    process.env.HOME = oldHome;
    rmSync(home, { recursive: true, force: true });
  }
}

// {scalars, denied (Map), builtin, rel, tables, left, review} from one
// duckdb_functions() rowset; a non-empty `review` stops the write.
export function build(fns) {
  const scalarTypes = new Set(["scalar", "aggregate", "macro"]);
  const denied = new Map(Object.entries(DENIED));
  const review = [];
  for (const r of fns) if (r.t === "macro" && /\bFROM\s+(?!duckdb_)[a-z_][a-z0-9_]*\s*\(/i.test(r.m || "")) denied.set(r.f, "its body reads a table function");
  // A macro body reads a relation by name only as query_table(CAST(<one of
  // its parameters> AS VARCHAR)), the shape RELATION_ARGS is read from: a
  // table macro with any other query_table() / query() stops the write
  // (#224), a scalar macro with one is denied.
  const relNames = new Set();
  for (const r of fns) {
    const body = r.m || "";
    const calls = (body.match(QUERY_CALL) || []).length;
    if (!calls) continue;
    if (r.t !== "table_macro") {
      if (r.t === "macro") denied.set(r.f, "its body reads a table function");
      continue;
    }
    const params = new Set((r.p || []).map((p) => String(p).toLowerCase()));
    const read = [...body.matchAll(QUERY_TABLE_PARAM)].map((m) => m[1].toLowerCase());
    if (read.length !== calls) review.push(`${r.f}: query_table() or query() in a shape this script does not read`);
    for (const p of read) {
      if (!params.has(p)) review.push(`${r.f}: query_table() of ${p}, which is not one of its parameters`);
      relNames.add(p);
    }
  }
  const scalars = [...new Set([...fns.filter((r) => scalarTypes.has(r.t)).map((r) => r.f), ...PARSER_EXTRA])].filter((f) => !denied.has(f)).sort();
  for (const f of scalars) if (LOOKS.test(f) && !REVIEWED.has(f)) review.push(f);
  const builtin = [...new Set([...fns.map((r) => r.f), ...PARSER_EXTRA])].sort();
  // The anofox families' table functions (#223): their table macros and the
  // reviewed native ones. Any other native one is left out (an *_operator
  // without a review; one not reviewed yet stops the write), and so is a table
  // macro whose body calls one.
  const left = new Set();
  for (const r of fns)
    if (r.t === "table" && ANOFOX_FAMILY.test(r.f) && !NATIVE_TABLE_ALLOWED.has(r.f)) {
      left.add(r.f);
      if (!NATIVE_OPERATOR.test(r.f) && !Object.hasOwn(NATIVE_TABLE_DENIED, r.f)) review.push(`${r.f}: a native table function (add it to NATIVE_TABLE_ALLOWED or NATIVE_TABLE_DENIED)`);
    }
  const kinds = new Map();
  for (const r of fns) (kinds.get(r.f) || kinds.set(r.f, new Set()).get(r.f)).add(r.t);
  for (const r of fns)
    if (r.t === "table_macro" && ANOFOX_FAMILY.test(r.f))
      for (const c of callsOf(r.m)) {
        if (ANOFOX_FAMILY.test(c) || CORE_TABLE_OK.has(c)) continue;
        if (READER_CALL.test(c)) review.push(`${r.f}: calls ${c}(), a file or URL reader`);
        else if (kinds.get(c)?.has("table") && !NATIVE_HELPER_ALLOWED.has(c))
          review.push(`${r.f}: calls ${c}(), a native table function outside its family (review it into NATIVE_HELPER_ALLOWED)`);
        else if (kinds.get(c)?.has("table_macro")) review.push(`${r.f}: calls ${c}(), a table macro outside its family`);
      }
  // A table macro that calls a left-out function (quoted or not, directly or
  // through another macro) is left out too.
  for (let grew = true; grew; ) {
    grew = false;
    for (const r of fns)
      if (r.t === "table_macro" && !left.has(r.f) && [...callsOf(r.m)].some((c) => left.has(c))) {
        left.add(r.f);
        grew = true;
      }
  }
  const tables = [...new Set(fns.filter((r) => ANOFOX_FAMILY.test(r.f) && (r.t === "table_macro" || (r.t === "table" && NATIVE_TABLE_ALLOWED.has(r.f)))).map((r) => r.f))]
    .filter((f) => !left.has(f))
    .sort();
  const rel = { ...NATIVE_RELATION };
  for (const r of fns)
    if (r.t === "table_macro") {
      const pairs = (r.p || []).map((p, i) => [i, String(p).toLowerCase()]).filter(([, p]) => relNames.has(p));
      if (!pairs.length) continue;
      const cur = rel[r.f] || [];
      for (const x of pairs) if (!cur.some(([i, p]) => i === x[0] && p === x[1])) cur.push(x);
      rel[r.f] = cur.sort((a, b) => a[0] - b[0] || (a[1] < b[1] ? -1 : 1));
    }
  return { scalars, denied, builtin, rel, tables, left: [...left].sort(), review: [...new Set(review)].sort() };
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

// What the five exports are: the allow-list a scalar/aggregate/window call is
// checked against (SCALAR_ALLOWED), the reviewed denials kept apart for their
// reasons (SCALAR_DENIED), every function name of every type so a temp macro
// can be refused when it shadows one (BUILTIN), the anofox table functions
// the assistant may call (ANOFOX_TABLES), and which argument of a table macro
// must name a relation of the catalog (RELATION_ARGS).
const EXPORTS_COMMENT = `// SCALAR_ALLOWED: the scalar, aggregate and window functions the assistant's
// SQL may call directly. SCALAR_DENIED: reviewed denials, with why, kept out
// of SCALAR_ALLOWED (defence in depth with web/assist.js's ALWAYS_DENIED).
// BUILTIN: every function name of every type (a temp macro may not take one).
// ANOFOX_TABLES: the anofox extensions' table functions the assistant may
// call: their families' table macros and reviewed native ones, never a native
// *_operator.
// RELATION_ARGS: per table macro, the [index, parameter name] pairs whose
// argument must name a table of the catalog (the macro's body reads it
// through query_table()).
`;

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const { version, exts, fns } = await collect();
  const { scalars, denied, builtin, rel, tables, left, review } = build(fns);
  console.log(`scalars: ${scalars.length}, denied: ${denied.size}, builtin: ${builtin.length}, anofox tables: ${tables.length}, relation keys: ${Object.keys(rel).length}`);
  console.log(`left out: ${JSON.stringify(left)}`);
  console.log(`review: ${JSON.stringify(review)}`);
  if (review.length) {
    console.error(
      `review is not empty: resolve each entry in ${path.relative(root, fileURLToPath(import.meta.url))} before writing (a scalar: DENIED or REVIEWED; a native table function: NATIVE_TABLE_ALLOWED or NATIVE_TABLE_DENIED; a macro body: teach build() its shape).`
    );
    process.exit(2);
  }
  const wasmVersion = JSON.parse(readFileSync(path.join(root, "tests/web/package.json"), "utf8")).dependencies["@duckdb/duckdb-wasm"];
  const extsText = exts.map((e) => `${e.n} ${e.v}`).join(", ");
  const header = `// Generated by scripts/gen-assist-functions.mjs from duckdb_functions() of DuckDB ${version} (DuckDB-Wasm ${wasmVersion}) with ${extsText}; do not edit.\n`;
  const body = [header, EXPORTS_COMMENT, renderNameSet("SCALAR_ALLOWED", scalars), renderDeniedMap(denied), renderNameSet("BUILTIN", builtin), renderNameSet("ANOFOX_TABLES", tables), renderRelationArgs(rel)].join("\n");
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
