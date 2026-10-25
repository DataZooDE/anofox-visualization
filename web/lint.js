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

// sec-review-256 (final check): a null-prototype, *shallow* copy of `o`'s
// own enumerable properties — every other value (a Map, a Set, an array,
// a function, a service object such as a `pageAdapter()`, or a nested
// plain object) passes through by reference, untouched. Every function
// here and in web/assist.js, web/assist-engine.js and web/assist-csp.js
// that takes an options/config object runs it through this first, so a
// missing key is always `undefined` — never read off a polluted
// `Object.prototype`. A caller whose *nested* field is itself read
// key-by-key from untrusted input (`assistConfig`'s `a.models`,
// `runAssistant`'s `limits`) calls `ownOpts` again on that field
// specifically — shallow, not automatic, so this never silently reaches
// into (and needlessly clones) a service object such as `buildEngine`'s
// `page`, which is a plain object literal too but is never meant to be
// copied.
export function ownOpts(o) {
  const out = Object.create(null);
  if (!o || typeof o !== "object") return out;
  for (const k of Object.keys(o)) out[k] = o[k];
  return out;
}

// Lint `script`. `step` is the wasm lint_step; `run(sql)` resolves to the
// rows as JSON text (as render_panel takes them) or rejects with the DuckDB
// error. `strict` and `design` are `--strict` and the inverse of
// `--no-design`. `onStep(n, queries)` reports progress; `cancelled()` stops
// before the next query. Resolves to {diagnostics, ok, summary, spans, steps,
// queries, answers} (answers: query -> {rows} | {error}), or null when
// cancelled.
// sec-review-256 last check: the second parameter used to be destructured
// directly in the signature (`{ step, run, … }`), which reads *inherited*
// properties too — a polluted `Object.prototype.strict`/`.design` would
// apply to every caller. `ownOpts` first, as everywhere else.
export async function lintScript(script, opts = {}) {
  const { step, run, strict = false, design = true, onStep, cancelled } = ownOpts(opts);
  const stepOpts = JSON.stringify({ strict, design });
  const known = new Map(); // query -> {rows} | {error}
  for (let n = 1; n <= MAX_STEPS; n++) {
    const res = JSON.parse(step(script, knownJson(known), stepOpts));
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
// `guard(piece, {query, state})` sees every statement before anything of the
// query runs (prepare included) and throws to refuse it; `query(sql)` runs a
// helper query in the same scratch session, `state` lives for one call (the
// assistant's parse-tree check at the execution point).
// Works with the async (browser) and the blocking (Node) DuckDB-Wasm alike.
let scratchSeq = 0; // unique across runners: two may run at once
export function scratchRunner(db, opts = {}) {
  const { prepare, toRows = rowsJson, timeoutMs = 0, guard } = ownOpts(opts);
  return async (sql) => {
    const c = await db.connect();
    const name = `anofox_lint_${++scratchSeq}`;
    let home = null;
    let timer;
    // Cleanup always runs USE/DETACH on `c`, even after a timeout (#258).
    // In DuckDB-Wasm a new statement on a busy connection interrupts the
    // pending one instead of queueing behind it (USE+DETACH ~150 ms,
    // close() ~5 ms, measured), and cancelSent() resolving false is the
    // normal answer for an idle or fetching connection, so skipping the
    // DETACH on it would leak `anofox_lint_N` for the rest of the session.
    // If `c` itself fails the cleanup, DETACH again from a fresh connection:
    // ATTACH/DETACH are catalog-level, not per connection.
    const detachAndClose = async () => {
      try {
        if (home) {
          await c.query(`USE ${home}`);
          await c.query(`DETACH DATABASE IF EXISTS ${name}`);
        }
        await c.close();
      } catch (_) {
        await c.close().catch(() => {});
        if (home) {
          try {
            const c2 = await db.connect();
            try {
              await c2.query(`DETACH DATABASE IF EXISTS ${name}`);
            } finally {
              await c2.close();
            }
          } catch (_) {
            /* best effort: `name` stays attached if even this fails */
          }
        }
      }
    };
    try {
      home = `"${String((await c.query("SELECT current_database() AS d")).toArray()[0].toJSON().d).replace(/"/g, '""')}"`;
      await c.query(`ATTACH ':memory:' AS ${name}`);
      await c.query(`USE ${name}`);
      await c.query(`SET search_path = '${name}.main,${home.replace(/'/g, "''")}.main'`);
      const pieces = splitStatements(sql);
      if (guard) {
        const ctx = { query: async (q) => rowsJson(await runOne(c, q)), state: {} };
        for (const piece of pieces) await guard(piece, ctx);
      }
      if (prepare) await prepare(sql);
      const work = (async () => {
        let last = null;
        for (const piece of pieces) last = await runOne(c, piece);
        return toRows(last || { schema: { fields: [] }, toArray: () => [] });
      })();
      if (!timeoutMs) return await work;
      // A query past its time is cancelled in the worker (the page's later
      // queries would wait behind it), not just abandoned.
      const late = new Promise((_, reject) => {
        timer = setTimeout(async () => {
          try {
            await c.cancelSent?.();
          } catch (_) {}
          reject(new Error(`the query took longer than ${timeoutMs / 1000} s and was cancelled`));
        }, timeoutMs);
      });
      work.catch(() => {}); // (rejects after a cancel; the timeout error wins)
      return await Promise.race([work, late]);
    } finally {
      clearTimeout(timer);
      await detachAndClose();
    }
  };
}

// Run exactly one statement. DuckDB's own parser decides what one statement
// is: a text prepare() refuses as "multiple statements" never runs (a split
// that differs from DuckDB's, e.g. around E'\'' strings, can't hide a
// second statement). Sent, not queried, so a timeout can cancel it.
async function runOne(c, sql) {
  try {
    const st = await c.prepare(sql);
    await st.close();
  } catch (e) {
    const msg = String((e && e.message) || e);
    if (/multiple statements/i.test(msg)) throw new Error(`not run: DuckDB reads more than one statement in: ${sql.slice(0, 80)}`);
    if (/^Parser Error/i.test(msg)) throw e;
    // Any other prepare() error (a binder error, a missing table) is left to
    // send(), which reports it and also takes a single statement only.
  }
  const reader = await c.send(sql);
  const schema = reader.schema;
  const batches = [];
  for await (const b of reader) batches.push(b);
  return { schema: batches[0]?.schema || schema || { fields: [] }, toArray: () => batches.flatMap((b) => b.toArray()) };
}

// The statements of a script, split on `;` outside literals ('…', E'…' with
// backslash escapes, "…", $tag$…$tag$) and comments.
export function splitStatements(sql) {
  const out = [];
  let cur = "";
  let real = false; // the current statement has more than comments
  const s = String(sql);
  for (let i = 0; i < s.length; i++) {
    const c = s[i];
    const two = s.slice(i, i + 2);
    if (c === "'" || c === '"') {
      const esc = c === "'" && /[Ee]/.test(s[i - 1] || "") && !/[\w$]/.test(s[i - 2] || "");
      let j = i + 1;
      for (; j < s.length; j++) {
        if (esc && s[j] === "\\") j++;
        else if (s[j] === c) {
          if (s[j + 1] === c) j++;
          else break;
        }
      }
      cur += s.slice(i, j + 1);
      i = j;
      real = true;
    } else if (two === "--") {
      const j = s.indexOf("\n", i);
      const end = j < 0 ? s.length : j;
      cur += s.slice(i, end);
      i = end - 1;
    } else if (two === "/*") {
      const j = s.indexOf("*/", i + 2);
      const end = j < 0 ? s.length : j + 2;
      cur += s.slice(i, end);
      i = end - 1;
    } else if (c === "$" && /^\$[A-Za-z_]*\$/.test(s.slice(i)) && !/[\w$]/.test(s[i - 1] || "")) {
      const tag = s.slice(i).match(/^\$[A-Za-z_]*\$/)[0];
      const j = s.indexOf(tag, i + tag.length);
      const end = j < 0 ? s.length : j + tag.length;
      cur += s.slice(i, end);
      i = end - 1;
      real = true;
    } else if (c === ";") {
      if (real) out.push(cur.trim());
      cur = "";
      real = false;
    } else {
      cur += c;
      if (!/\s/.test(c)) real = true;
    }
  }
  if (real) out.push(cur.trim());
  return out;
}

// One long-lived scratch session (#244, C-1): scratchRunner's isolation — a
// fresh connection whose own in-memory database is first on the search path
// — kept open across calls, so a dashboard's SET VARIABLEs and TEMP objects
// carry from one statement to the next as on the page's own connection
// (scratchRunner's fresh session per call would lose them: inputs, drills
// and pages would break, and setup would have to re-run per panel). Calls
// are serialised (a promise tail): `before()` runs before every call (the
// caller's engine-lock re-check); `guard(piece, ctx)` before every
// statement of that call, sharing one `ctx.state` across the whole session
// (so a temp macro or variable a later call sees was set by an earlier
// one). No `prepare` parameter: under the locked engine there is nothing
// left to prepare (web/assist.js's wasmTools comment), and a `prepare`
// driven by the model's SQL would defeat the point of the lock.
export function scratchSession(db, opts = {}) {
  const { toRows = rowsJson, timeoutMs = 0, guard, before } = ownOpts(opts);
  let cP = null; // the connection, attached once on first use
  let name = null;
  let home = null;
  let closed = false;
  let tail = Promise.resolve(); // serialises calls over the one connection
  const state = {}; // shared across every call of this session (guard's ctx.state)
  const attach = async () => {
    const c = await db.connect();
    name = `anofox_lint_${++scratchSeq}`;
    home = `"${String((await c.query("SELECT current_database() AS d")).toArray()[0].toJSON().d).replace(/"/g, '""')}"`;
    await c.query(`ATTACH ':memory:' AS ${name}`);
    await c.query(`USE ${name}`);
    await c.query(`SET search_path = '${name}.main,${home.replace(/'/g, "''")}.main'`);
    return c;
  };
  const conn = () => (cP ||= attach());
  const runCall = async (sql) => {
    if (closed) throw new Error("the preview was closed");
    const c = await conn();
    if (before) await before();
    const pieces = splitStatements(sql);
    if (guard) {
      const ctx = { query: async (q) => rowsJson(await runOne(c, q)), state };
      for (const piece of pieces) await guard(piece, ctx);
    }
    const work = (async () => {
      let last = null;
      for (const piece of pieces) last = await runOne(c, piece);
      return toRows(last || { schema: { fields: [] }, toArray: () => [] });
    })();
    if (!timeoutMs) return await work;
    let timer;
    const late = new Promise((_, reject) => {
      timer = setTimeout(async () => {
        try {
          await c.cancelSent?.();
        } catch (_) {}
        reject(new Error(`the query took longer than ${timeoutMs / 1000} s and was cancelled`));
      }, timeoutMs);
    });
    work.catch(() => {});
    try {
      return await Promise.race([work, late]);
    } finally {
      clearTimeout(timer);
    }
  };
  return {
    // Serialised: each call waits for the previous one's settlement (success
    // or failure) before it starts, so two overlapping calls never interleave
    // statements on the one connection.
    run(sql) {
      const p = tail.then(() => runCall(sql), () => runCall(sql));
      tail = p.then(
        () => {},
        () => {}
      );
      return p;
    },
    async close() {
      closed = true;
      await tail.catch(() => {});
      if (!cP) return;
      const c = await cP;
      try {
        if (home) {
          await c.query(`USE ${home}`);
          await c.query(`DETACH DATABASE IF EXISTS ${name}`);
        }
      } catch (_) {
        /* the connection is closed next anyway */
      }
      await c.close();
    },
  };
}

// Arrow's logical type ids (apache-arrow's `Type` enum — already live on
// every Field/DataType object a DuckDB-Wasm result hands us, so reading
// `.typeId` isn't a new import; web/app.js's own CSP note on importing
// apache-arrow itself, after boot, doesn't apply here).
const T_DECIMAL = 7,
  T_DATE = 8,
  T_TIMESTAMP = 10,
  T_LIST = 12,
  T_STRUCT = 13,
  T_FIXED_SIZE_LIST = 16,
  T_MAP = 17;

// #468: `row.toJSON()` only resolves a row's own *top-level* columns to
// plain values — a List/Struct/Map's own elements stay live Arrow
// Vector/StructRow/MapRow objects, each needing the same per-type fixup a
// top-level column gets below (DECIMAL's unscaled mantissa, DATE/
// TIMESTAMP's epoch number), or a value such as `[0.1, 0.25]::DECIMAL(4,2)[]`
// reaches the renderer as `["10", "25"]` — the raw, unscaled mantissa.
// Walks `value` alongside its own (possibly nested) Arrow `type`, converting
// only the leaf kinds DuckDB-Wasm's default conversion gets wrong; every
// other leaf (string, bool, plain int, already-correct nested value) passes
// through untouched, same as it does today.
function fromArrow(value, type) {
  if (value == null) return null;
  switch (type.typeId) {
    case T_DECIMAL: {
      const scale = Number(type.scale) || 0;
      const n = Number(value);
      return scale > 0 ? n / 10 ** scale : n;
    }
    case T_DATE:
      return toIso(value, true);
    case T_TIMESTAMP:
      return toIso(value, false);
    case T_LIST:
    case T_FIXED_SIZE_LIST: {
      const itemType = type.children[0].type;
      return [...value].map((v) => fromArrow(v, itemType));
    }
    case T_STRUCT: {
      const out = {};
      for (const f of type.children) out[f.name] = fromArrow(value[f.name], f.type);
      return out;
    }
    case T_MAP: {
      // Physically List<Struct<key, value>> — `entries`' own two children.
      const entry = type.children[0].type;
      const keyType = entry.children[0].type;
      const valType = entry.children[1].type;
      const out = {};
      for (const [k, v] of value) out[fromArrow(k, keyType)] = fromArrow(v, valType);
      return out;
    }
    default:
      return typeof value === "bigint" ? Number(value) : value;
  }
}

// Whether `type`'s own structure (recursively) contains a DECIMAL, DATE or
// TIMESTAMP leaf anywhere — the only leaf kinds `fromArrow` needs to walk
// for. A column without one keeps going through DuckDB-Wasm's own (already
// correct) default conversion, untouched.
function needsFixup(type) {
  switch (type.typeId) {
    case T_DECIMAL:
    case T_DATE:
    case T_TIMESTAMP:
      return true;
    case T_LIST:
    case T_FIXED_SIZE_LIST:
      return needsFixup(type.children[0].type);
    case T_STRUCT:
      return type.children.some((f) => needsFixup(f.type));
    case T_MAP:
      return needsFixup(type.children[0].type.children[1].type);
    default:
      return false;
  }
}

// A DuckDB-Wasm result as rows' JSON text ([{col: value, …}, …]) — what
// web/app.js hands to render_panel. DuckDB-Wasm returns DATE / TIMESTAMP as
// epoch numbers (back to ISO strings, so date variables and axes read as
// YYYY-MM-DD) and DECIMAL as the unscaled mantissa (scaled back); BIGINTs
// become numbers — a column's own top-level value and, #468, any DECIMAL /
// DATE / TIMESTAMP nested inside a LIST / STRUCT / MAP (or FixedSizeList).
export function rowsJson(res) {
  const fixupCols = new Map(); // name -> field.type, for a column needing fromArrow's walk
  try {
    for (const f of res.schema.fields) {
      if (needsFixup(f.type)) fixupCols.set(f.name, f.type);
    }
  } catch (_) {}
  const rows = res.toArray().map((row) => {
    const o = row.toJSON();
    for (const [c, type] of fixupCols) o[c] = fromArrow(o[c], type);
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

// Check on a live backend (#199): `dashboard --check`'s diagnostics, run on
// the server's own data, for a backend with no DuckDB-Wasm to scratch-lint
// in (an authoring `anofox_serve(port)` / `serve <db>`). Unlike `lintScript`
// above, this is one round trip: the server runs the whole lint itself,
// synchronously, on a connection it keeps open and isolated for the purpose
// (ATTACH ':memory:' + USE, the same idea as `scratchRunner` — see
// duckext/src/serve.rs's `scratch_check`), so it never needs resuming and
// never touches what the page's own queries ("/query") see. Same auth as
// "/query" (the session's token cookie) — `fetchImpl` defaults to the global
// `fetch` (a test can pass a stub). Resolves to `{diagnostics, ok, summary,
// spans}`, or rejects with the server's error text.
export async function liveCheck(script, opts = {}) {
  const { strict = false, design = true, fetchImpl = fetch } = ownOpts(opts);
  const r = await fetchImpl("/check", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ script, strict, design }),
  });
  const text = await r.text();
  if (!r.ok) throw new Error(text);
  return JSON.parse(text);
}

// "2 errors, 1 warning" — the counts for the status line ("" for none).
export function countsText(diags) {
  const n = (s) => diags.filter((d) => d.severity === s).length;
  const part = (k, word) => (k ? `${k} ${word}${k === 1 ? "" : "s"}` : "");
  return [part(n("error"), "error"), part(n("warning"), "warning"), part(n("note"), "note")].filter(Boolean).join(", ");
}
