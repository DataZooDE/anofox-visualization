// The lint in the browser (#145): the diagnostics `dashboard --check` gives,
// over DuckDB-Wasm. The lint (src/lint.rs) runs its queries synchronously, so
// the wasm export `lint_step(script, known, opts)` is resumable: it answers
// each query from `known` and lists the ones it has no answer for in `need`.
// lintScript runs those, adds the rows (or the error) and calls again until
// `done`. Each query is `prelude + statement` and runs in a fresh session
// (scratchRunner), as the CLI runs each one in a fresh `duckdb`.
//
// A pure ES module (no DOM): web/app.js uses it for the Check button, and
// tests/web/lint_test.mjs runs it under Node against `dashboard --check`.

// lint_step usually settles in a step or two (one more per input whose default
// later panels read); this bounds a script that never would.
export const MAX_STEPS = 40;

// Lint `script`. `step` is the wasm lint_step; `run(sql)` resolves to the
// rows as JSON text (as render_panel takes them) or rejects with the DuckDB
// error. `strict` and `design` are `--strict` and the inverse of
// `--no-design`. `onStep(n, queries)` reports progress; `cancelled()` stops
// before the next query. Resolves to {diagnostics, ok, summary, spans, steps,
// queries, answers} (answers: query -> {rows} | {error}), or null when
// cancelled.
export async function lintScript(script, { step, run, strict = false, design = true, onStep, cancelled }) {
  const opts = JSON.stringify({ strict, design });
  const known = new Map(); // query -> {rows} | {error}
  for (let n = 1; n <= MAX_STEPS; n++) {
    const res = JSON.parse(step(script, knownJson(known), opts));
    if (res.error) throw new Error(res.error);
    if (res.done) return { ...res, steps: n, queries: known.size, answers: known };
    onStep?.(n, known.size + res.need.length);
    for (const sql of res.need) {
      if (cancelled?.()) return null;
      try {
        known.set(sql, { rows: await run(sql) });
      } catch (e) {
        known.set(sql, { error: String((e && e.message) || e) });
      }
    }
  }
  throw new Error(`the check did not settle after ${MAX_STEPS} steps`);
}

function knownJson(known) {
  const o = {};
  for (const [k, v] of known) o[k] = v;
  return JSON.stringify(o);
}

// A runner for lintScript: each query in a fresh session — a new connection
// with its own in-memory database (ATTACH ':memory:' + USE), so the
// dashboard's setup never touches the page's tables and temp objects and
// variables start empty. Tables of the page's database stay readable (they
// follow on the search path), so a script over an added file still lints.
// `prepare(sql)` readies what a statement needs first (extensions, files);
// `toRows(result)` turns a DuckDB-Wasm result into the rows' JSON text.
// Works with the async (browser) and the blocking (Node) DuckDB-Wasm alike.
export function scratchRunner(db, { prepare, toRows = rowsJson } = {}) {
  let n = 0;
  return async (sql) => {
    if (prepare) await prepare(sql);
    const c = await db.connect();
    const name = `anofox_lint_${++n}`;
    let home = null;
    try {
      home = `"${String((await c.query("SELECT current_database() AS d")).toArray()[0].toJSON().d).replace(/"/g, '""')}"`;
      await c.query(`ATTACH ':memory:' AS ${name}`);
      await c.query(`USE ${name}`);
      await c.query(`SET search_path = '${name}.main,${home.replace(/'/g, "''")}.main'`);
      return toRows(await c.query(sql));
    } finally {
      try {
        if (home) {
          await c.query(`USE ${home}`);
          await c.query(`DETACH DATABASE IF EXISTS ${name}`);
        }
      } catch (_) {
        /* the connection is closed next anyway */
      }
      await c.close();
    }
  };
}

// A DuckDB-Wasm result as rows' JSON text ([{col: value, …}, …]) — what
// web/app.js hands to render_panel. DuckDB-Wasm returns DATE / TIMESTAMP as
// epoch numbers (back to ISO strings, so date variables and axes read as
// YYYY-MM-DD) and DECIMAL as the unscaled mantissa (scaled back); BIGINTs
// become numbers.
export function rowsJson(res) {
  const dateCols = new Map(); // name -> "date" | "time"
  const decCols = new Map(); // name -> scale
  try {
    for (const f of res.schema.fields) {
      const t = String(f.type);
      if (/date/i.test(t)) dateCols.set(f.name, "date");
      else if (/timestamp/i.test(t)) dateCols.set(f.name, "time");
      else if (/decimal/i.test(t)) decCols.set(f.name, Number(f.type.scale) || 0);
    }
  } catch (_) {}
  const rows = res.toArray().map((row) => {
    const o = row.toJSON();
    for (const [c, kind] of dateCols) {
      if (o[c] != null) o[c] = toIso(o[c], kind === "date");
    }
    for (const [c, scale] of decCols) {
      if (o[c] != null && scale > 0) o[c] = Number(o[c]) / 10 ** scale;
    }
    return o;
  });
  return JSON.stringify(rows, (_, v) => (typeof v === "bigint" ? Number(v) : v));
}

// Normalise a DuckDB-Wasm date/time value (Date, or epoch as days/ms/µs) to an
// ISO string — YYYY-MM-DD for dates, "YYYY-MM-DD HH:MM:SS" for timestamps.
export function toIso(v, dateOnly) {
  let d;
  if (v instanceof Date) d = v;
  else {
    let n = Number(v);
    if (!isFinite(n)) return String(v);
    if (Math.abs(n) < 1e11) n *= 86400000; // days -> ms
    else if (Math.abs(n) > 1e14) n = Math.round(n / 1000); // µs -> ms
    d = new Date(n);
  }
  if (isNaN(d.getTime())) return String(v);
  const iso = d.toISOString();
  return dateOnly ? iso.slice(0, 10) : iso.replace("T", " ").slice(0, 19);
}

// The diagnostics in statement order (a statement-0 one — the whole script —
// first), errors before warnings before notes within a statement.
const RANK = { error: 0, warning: 1, note: 2 };
export function sortDiagnostics(diags) {
  return diags
    .map((d, i) => [d, i])
    .sort(([a, i], [b, j]) => a.stmt - b.stmt || (RANK[a.severity] ?? 3) - (RANK[b.severity] ?? 3) || i - j)
    .map(([d]) => d);
}

// "2 errors, 1 warning" — the counts for the status line ("" for none).
export function countsText(diags) {
  const n = (s) => diags.filter((d) => d.severity === s).length;
  const part = (k, word) => (k ? `${k} ${word}${k === 1 ? "" : "s"}` : "");
  return [part(n("error"), "error"), part(n("warning"), "warning"), part(n("note"), "note")].filter(Boolean).join(", ");
}
