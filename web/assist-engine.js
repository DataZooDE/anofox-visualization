// The assistant's own DuckDB-Wasm instance (#204): a second `AsyncDuckDB`, in
// its own Worker, created only after the tab's CSP is proven enforced
// (assist-csp.js). Its extensions are fixed before the lock; its files are
// registered under exact names before the lock; then it is locked
// (`enable_external_access=false`, autoload/autoinstall/community off, an
// exact `allowed_paths`, `lock_configuration=true`) and given an Arrow
// snapshot of the page's own tables.
//
// The contract (plan-204 §2.2): before Apply, the page's own DuckDB never
// runs a statement whose text the model influenced. Every tool, the lint's
// queries, the context profiles and the proposal preview all run here, in
// the locked engine; the page is read only through `pageAdapter`'s closed
// set of fixed statements (the snapshot). The lock is proven once at build
// time (`engineLocked`) and re-checked before every tool call and preview
// batch (web/assist.js), because one DuckDB setting — `enable_logging` —
// escapes `lock_configuration` (a `LOGGING_SHADOW` macro catches the common
// unqualified spelling; the re-check catches the rest and triggers a
// rebuild).
//
// The lock comes before the data: extensions and files are loaded while the
// engine can still reach the network (the CSP already blocks the document,
// but LOAD/INSTALL of the fixed set still needs those same-origin/CDN
// fetches); once locked, nothing loaded afterwards can ever add a new
// reader, a new network path or a new unsigned extension — only the fixed,
// reviewed surface loaded before the lock exists for the rest of this
// engine's life.
//
// A pure ES module (no DOM beyond `doc` for the CSP proof):
// tests/web/assist_engine_test.mjs drives it under Node (`doc: undefined`).
//
// No network calls, no registerFileURL, no OPFS, no document.baseURI and no
// dynamic module loading here (a static grep test; see
// tests/web/assist_engine_test.mjs item 12).
import { CATALOG_SQL, qid, sqlStr } from "./explore.js";
import { assistCspProven } from "./assist-csp.js";

// L-3: never `document.baseURI` — a `<base>` injection (an XSS before the
// ratchet, or a sibling repo on the shared github.io origin) must not move
// where the engine's unsigned anofox extensions come from.
export const LOCALEXT = new URL("./localext", import.meta.url).href;

// D2 + L-2: the snapshot's limits. `tableRows`/`totalRows` bound what is
// copied; `totalBytes` is the Arrow-encoded byte budget across the whole
// snapshot; `fileBytes` bounds one registered file; `queryMs`/`catalogMs`
// bound one page query (cancelled, not just abandoned, on expiry).
export const SNAPSHOT_LIMITS = {
  tableRows: 1_000_000,
  totalRows: 3_000_000,
  totalBytes: 512 << 20,
  fileBytes: 256 << 20,
  memory: "1.5GB",
  queryMs: 20_000,
  catalogMs: 5_000,
  extMs: 30_000,
};

// Fixed before the lock (M-1): json, parquet, icu (today()/get_current_time(),
// #225) and both anofox extensions always load (same-origin web/localext/,
// ~5 MB each, HTTP-cached); spatial (large, from extensions.duckdb.org) only
// when the page already has it loaded or the current *editor* SQL needs it —
// never the model's SQL, which the engine never prepares against (M-1).
export const ENGINE_EXTENSIONS = ["json", "parquet", "icu", "anofox_forecast", "anofox_statistics", "spatial"];
export function engineExtensions({ pageLoaded = [], editorNeeds = [] } = {}) {
  const base = ["json", "parquet", "icu", "anofox_forecast", "anofox_statistics"];
  const extra = [...pageLoaded, ...editorNeeds].includes("spatial") ? ["spatial"] : [];
  return [...base, ...extra];
}

// The lock sequence (plan-204 §1.2, verified against DuckDB-Wasm 1.33.1-dev65.0
// / DuckDB v1.5.6): `allowed_directories` first (so `allowed_paths` below
// lands while it's still settable), then the extension and access switches,
// `lock_configuration` last.
export const ENGINE_LOCK_SQL = [
  "SET allowed_directories=[]",
  "SET autoinstall_known_extensions=false",
  "SET autoload_known_extensions=false",
  "SET allow_community_extensions=false",
  "SET enable_external_access=false",
  "SET lock_configuration=true",
];

// L-1: `enable_logging` is outside `lock_configuration` (reported upstream —
// see the PR body). The unqualified spelling the model's gate would route a
// CALL through (the gate denies CALL as class 3 anyway; this is belt and
// braces) resolves to this macro instead of the built-in table function.
// `system.main.enable_logging` still reaches the real one: `engineLocked`'s
// per-call re-check is the control that actually holds (§2.4).
const LOGGING_SHADOW =
  "CREATE MACRO enable_logging(level := NULL, storage := NULL, storage_path := NULL, storage_config := NULL, storage_buffer_size := NULL) AS TABLE SELECT 'refused' AS enable_logging";

// A name safe to register with DuckDB-Wasm and put on `allowed_paths`: no
// scheme, no leading '/' or '~' (I-6), no '..' segment, no doubled '/', none
// of the glob/shell metacharacters DuckDB's own path matching treats
// specially, and a sane length.
export function safeFileName(n) {
  return (
    typeof n === "string" &&
    n.length > 0 &&
    n.length < 512 &&
    !/^[a-z][a-z0-9+.-]*:/i.test(n) &&
    !/^[\/~]/.test(n) &&
    !n.split("/").includes("..") &&
    !n.includes("//") &&
    !/[*?[\]{}\\\0]/.test(n)
  );
}

async function tryQ(c, sql) {
  // Swallows errors for `memory_limit`/`preserve_insertion_order` only: some
  // DuckDB-Wasm builds don't support one or the other.
  try {
    await c.query(sql);
  } catch (_) {}
}

// Loads one engine extension; never throws on its own (an offline test
// run, a build with no network to extensions.duckdb.org, or a get.erpl.io
// outage must still produce a usable, if smaller, engine — the error
// shapes in web/assist.js turn a missing extension into "not in the
// assistant's engine: <ext>" for the one tool call that needed it, not a
// build failure). `loadExt` below bounds it with a real timeout.
async function loadExtInner(c, name) {
  try {
    if (name === "json" || name === "parquet" || name === "icu") {
      try {
        await c.query(`LOAD ${name}`);
      } catch (_) {
        await c.query(`INSTALL ${name}`);
        await c.query(`LOAD ${name}`);
      }
      return true;
    }
    if (name === "anofox_forecast" || name === "anofox_statistics") {
      await c.query(`SET custom_extension_repository=${sqlStr(LOCALEXT)}`);
      try {
        await c.query(`INSTALL ${name}`);
        await c.query(`LOAD ${name}`);
      } finally {
        await c.query("RESET custom_extension_repository");
      }
      return true;
    }
    if (name === "spatial") {
      await c.query("INSTALL spatial");
      await c.query("LOAD spatial");
      return true;
    }
  } catch (_) {
    return false;
  }
  throw new Error(`not an engine extension: ${name}`);
}

// sec-review-256 M-1: bounds `loadExtInner` with a timeout that actually
// stops it when it fires, rather than a race that lets its queries keep
// running on `c` in the background (interleaved with the next extension's,
// the file registration, the lock and the snapshot — see the call site).
// There is no cancelSent()-style recovery for a stuck LOAD/INSTALL either
// (M-2's finding applies here too: `cancelSent()` cannot interrupt a plain
// `query()` message) — terminating the Worker is the only thing that
// does, and that leaves `c`/`db` unusable afterwards, so a timeout here
// always throws rather than returning `false`.
async function loadExt(c, db, name, ms) {
  let timedOut = false;
  const inner = loadExtInner(c, name);
  inner.catch(() => {}); // loadExtInner never actually rejects; defensive only
  const result = await new Promise((resolve) => {
    const t = setTimeout(() => {
      timedOut = true;
      resolve(false);
    }, ms);
    inner.then(
      (r) => {
        clearTimeout(t);
        resolve(r);
      },
      () => {
        clearTimeout(t);
        resolve(false);
      }
    );
  });
  if (timedOut) {
    try {
      await db.terminate?.();
    } catch (_) {}
    throw new Error(`assistant engine: extension "${name}" did not load within ${ms} ms`);
  }
  return result;
}

// A query with a cancelling timeout (L-2) that actually bounds the work in
// a browser (sec-review-256 M-2): `c.query(sql)` is one `RUN_QUERY` message,
// and DuckDB-Wasm's `cancelSent()` only cancels a query that was *sent* —
// racing `query()` against a timer does nothing: the Worker keeps running
// the original query to completion regardless (confirmed: a 300 ms budget
// against a ~200M-row cross-join view still returned normally after 13+s).
// `c.send(sql)` + reading its own batches is a query `cancelSent()` can
// actually interrupt — confirmed the same way: the identical query, raced
// the identical way, rejects within the budget and leaves the connection
// free for the next query. This mirrors web/lint.js's `runOne`/
// `scratchRunner`, which already uses `send()` for exactly this reason.
//
// `TableCtor` rebuilds a real Arrow Table from the batches collected before
// the cutoff (`new TableCtor(batches)`): the same class `c.query()` itself
// returns (grabbed once, from a trivial query, since DuckDB-Wasm doesn't
// export the Table class directly) — `insertArrowTable`, `.numRows`,
// `.toArray()` and `.slice()` all work on it exactly as on a `query()`
// result.
async function withTimeout(c, sql, ms, TableCtor) {
  if (!ms) return c.query(sql);
  // `c.send(sql)` itself must be *inside* the race, not awaited before it:
  // for a global aggregate with no intermediate rows (count(*), no GROUP
  // BY), DuckDB produces nothing — not even the reader's first batch —
  // until the whole thing has been computed. An earlier version of this
  // function awaited `c.send(sql)` before starting the timer, so the
  // timeout never had a chance to fire until `send()` itself resolved
  // (sec-review-256 M-2: a 300 ms budget against a ~200M-row cross-join
  // view still took 16+ s). `Promise.race` abandons `work` the moment
  // `late` wins — it does not wait for `work` to notice anything — which
  // is what actually bounds the wall-clock time; `cancelSent()` only
  // stops the abandoned work from wasting the Worker's time afterward.
  let timer;
  let aborted = false;
  const work = (async () => {
    const reader = await c.send(sql);
    const batches = [];
    for await (const b of reader) {
      if (aborted) break;
      batches.push(b);
    }
    return new TableCtor(batches);
  })();
  work.catch(() => {}); // the loser's eventual settlement must not be an unhandled rejection
  const late = new Promise((_, reject) => {
    timer = setTimeout(() => {
      aborted = true;
      Promise.resolve(c.cancelSent?.())
        .catch(() => {})
        .finally(() => reject(new Error(`the query took longer than ${ms / 1000} s and was cancelled`)));
    }, ms);
  });
  try {
    return await Promise.race([work, late]);
  } finally {
    clearTimeout(timer);
  }
}

// The total encoded bytes of an Arrow result (checked against the remaining
// `totalBytes` budget before it is copied into the engine). I-R2-2: this
// runs once the table is already in the main thread — a wide, 1M-row table
// is fully fetched before the cap can refuse the *insert*. That is an
// accepted, documented limit (plan-204 rev2 re-check): the per-query timeout
// already bounds the slow case, and streaming the cap in during `send()`
// would need an Arrow-batch-by-batch budget check this release doesn't add.
export function arrowBytes(t) {
  try {
    return (t.batches || []).reduce((s, b) => s + (b.data?.byteLength ?? 0), 0);
  } catch (_) {
    return 0;
  }
}

// FNV-1a, 32-bit, hex. Good enough for "did the page's shape change", not a
// security boundary.
function fnv1a(str) {
  let h = 0x811c9dc5;
  for (let i = 0; i < str.length; i++) {
    h ^= str.charCodeAt(i);
    h = Math.imul(h, 0x01000193);
  }
  return (h >>> 0).toString(16).padStart(8, "0");
}

// The read-only surface `buildEngine` uses on the *page*'s DuckDB: a closed
// set of fixed statements, `fq` always rebuilt from catalog rows with `qid`
// (never a name the caller hands in directly). Opens its own connection (so
// a snapshot timeout's `cancelSent` never cancels the page's own panels).
export function pageAdapter(db, { handles = new Map() } = {}) {
  let connP = null;
  const conn = () => (connP ||= Promise.resolve(db.connect()));
  // Grabbed once, from a trivial query on this connection (always fast —
  // it never needs its own timeout): the Table class `withTimeout` rebuilds
  // a cancelled query's partial-then-complete batches into.
  let tableCtorP = null;
  const tableCtor = async () => {
    if (!tableCtorP) {
      const c = await conn();
      tableCtorP = Promise.resolve(c.query("SELECT 1")).then((t) => t.constructor);
    }
    return tableCtorP;
  };
  const rows = async (sql, ms) => {
    const c = await conn();
    const r = await withTimeout(c, sql, ms, ms ? await tableCtor() : undefined);
    return r.toArray().map((row) => row.toJSON());
  };
  return {
    // M-2: every page query gets `catalogMs`/`queryMs`, not just `rows()` —
    // a crafted `duckdb_views().sql` or a huge catalog is bounded the same
    // way a huge view's data is.
    async databases(ms = 0) {
      return rows("SELECT database_name, path, type FROM duckdb_databases()", ms);
    },
    async catalog(ms = 0) {
      return rows(CATALOG_SQL, ms);
    },
    async viewSql(dbName, schema, name, ms = 0) {
      const r = await rows(`SELECT sql FROM duckdb_views() WHERE database_name=${sqlStr(dbName)} AND schema_name=${sqlStr(schema)} AND view_name=${sqlStr(name)}`, ms);
      return r[0]?.sql || "";
    },
    async describe(fq, ms = 0) {
      return rows(`DESCRIBE SELECT * FROM ${fq}`, ms);
    },
    // Tables: `LIMIT n+1`; with more than `n` rows, a reservoir sample of
    // exactly `n` (`sampled:true`). Views: `LIMIT n+1` only, never a
    // reservoir sample (a view body can be expensive; a full scan for a
    // sample would run it twice) — kept to `n` rows with `truncated:true`.
    async rows(fq, kind, n, ms = 0) {
      const c = await conn();
      const Ctor = ms ? await tableCtor() : undefined;
      const first = await withTimeout(c, `SELECT * FROM ${fq} LIMIT ${n + 1}`, ms, Ctor);
      const got = first.numRows ?? first.toArray().length;
      if (got <= n) return { table: first, sampled: false, truncated: false };
      if (kind === "view") return { table: first.slice(0, n), sampled: false, truncated: true };
      const sampled = await withTimeout(c, `SELECT * FROM ${fq} USING SAMPLE reservoir(${n} ROWS) REPEATABLE (42)`, ms, Ctor);
      return { table: sampled, sampled: true, truncated: false };
    },
    // The page's registered file names (xpHandles; `app.js` keeps it in
    // sync with every successful `registerFileHandle`).
    async files() {
      return [...handles.keys()];
    },
    handle(name) {
      return handles.get(name);
    },
    async copyFile(name, max) {
      const bytes = await db.copyFileToBuffer(name);
      if (bytes.byteLength > max) throw new Error(`${name}: too large for the assistant`);
      return bytes;
    },
  };
}

// A database the snapshot must never touch: a scheme or `md:` path, a
// non-`duckdb` type (MotherDuck, sqlite, …), or one of DuckDB-Wasm's/the
// lint's own housekeeping databases (not reported — they aren't the user's).
const REMOTE_SCHEME = /^[a-z][a-z0-9+.-]*:\/\//i;
function dbStatus(row) {
  if (/^(temp|system)$/.test(row.database_name) || /^anofox_lint_\d+$/.test(row.database_name)) return "skip";
  const path = String(row.path || "");
  if (REMOTE_SCHEME.test(path) || /^md:/i.test(path)) return "remote database";
  if (row.type && row.type !== "duckdb") return "remote database";
  return null;
}

// The snapshot (L-2, D2, D3): an Arrow copy of the page's own tables and
// views into the locked engine, in catalog order, bounded by `limits`.
// Remote-attached and non-`duckdb` databases — and any view whose body names
// one as an identifier — are excluded rather than pulled: an
// over-approximation (a false exclusion is safe; a false inclusion is not).
export async function snapshotTables(c, page, limits = SNAPSHOT_LIMITS, { signal, onProgress } = {}) {
  const dbRows = await page.databases(limits.catalogMs);
  const excludedDbs = new Map();
  const includedDbs = new Set();
  for (const r of dbRows) {
    const why = dbStatus(r);
    if (why === "skip") continue;
    if (why) excludedDbs.set(r.database_name, why);
    else includedDbs.add(r.database_name);
  }
  const excludedNameRe = [...excludedDbs.keys()].map((d) => new RegExp(`(^|[^\\w$])${d.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}([^\\w$]|$)`, "i"));
  const catalogRows = await page.catalog(limits.catalogMs);
  const out = [];
  let totalRows = limits.totalRows;
  let totalBytes = limits.totalBytes;
  const attached = new Set(["memory"]);
  for (const row of catalogRows) {
    if (signal?.aborted) break;
    const dbName = row.database_name,
      schema = row.schema_name,
      name = row.table_name,
      kind = row.kind;
    const fq = `${qid(dbName)}.${qid(schema)}.${qid(name)}`;
    if (excludedDbs.has(dbName)) {
      out.push({ fq, kind, rows: 0, excluded: excludedDbs.get(dbName) });
      continue;
    }
    if (!includedDbs.has(dbName)) continue; // a housekeeping database: not part of the snapshot at all
    if (kind === "view" && excludedNameRe.length) {
      const sql = await page.viewSql(dbName, schema, name, limits.catalogMs).catch(() => "");
      if (excludedNameRe.some((re) => re.test(sql))) {
        out.push({ fq, kind, rows: 0, excluded: "reads a remote database" });
        continue;
      }
    }
    if (totalRows <= 0 || totalBytes <= 0) {
      out.push({ fq, kind, rows: 0, excluded: "too large" });
      continue;
    }
    onProgress?.({ fq });
    try {
      if (!attached.has(dbName)) {
        await c.query(`ATTACH ':memory:' AS ${qid(dbName)}`);
        attached.add(dbName);
      }
      await c.query(`USE ${qid(dbName)}`);
      if (schema !== "main") await c.query(`CREATE SCHEMA IF NOT EXISTS ${qid(schema)}`);
      const descRows = await page.describe(fq, limits.catalogMs);
      const ddl = `CREATE TABLE ${qid(schema)}.${qid(name)} (${descRows.map((d) => `${qid(d.column_name)} ${d.column_type}`).join(", ")})`;
      // I-6: exactly one statement is prepared before anything runs.
      let createFlag = false,
        typesChanged = false;
      try {
        const st = await c.prepare(ddl);
        await st.close();
        await c.query(ddl);
      } catch (_) {
        createFlag = true;
        typesChanged = true;
      }
      const n = Math.min(limits.tableRows, totalRows);
      const r = await page.rows(fq, kind, n, limits.queryMs);
      const bytes = arrowBytes(r.table);
      if (bytes > totalBytes) {
        out.push({ fq, kind, rows: 0, excluded: "too large" });
        await c.query("USE memory");
        continue;
      }
      await c.insertArrowTable(r.table, { schema, name, create: createFlag });
      const got = r.table.numRows ?? r.table.toArray().length;
      totalRows -= got;
      totalBytes -= bytes;
      await c.query("USE memory");
      out.push({ fq, kind, rows: got, sampled: r.sampled || undefined, truncated: r.truncated || undefined, types_changed: typesChanged || undefined });
    } catch (e) {
      try {
        await c.query("USE memory");
      } catch (_) {}
      const msg = String(e?.message || e);
      out.push({ fq, kind, rows: 0, excluded: /took longer than|cancelled/i.test(msg) ? "timed out" : e?.name || "error" });
    }
  }
  return out;
}

// A fingerprint of what the engine was built from: changes on a new table, a
// new registered file, a new attached db or a changed extension set; stable
// on a rebuild with nothing changed. M1-3 recomputes this before each
// request and rebuilds the engine when it differs.
export async function pageFingerprint(page, { extensions = [], files = [], limits = SNAPSHOT_LIMITS } = {}) {
  const dbs = await page.databases(limits.catalogMs);
  const cat = await page.catalog(limits.catalogMs);
  // DuckDB-Wasm's blocking (Node) connection returns BigInt for count-like
  // columns; JSON.stringify doesn't know BigInt, so numbers are normalised
  // the same way web/lint.js's rowsJson does.
  const bigJson = (v) => JSON.stringify(v, (_, x) => (typeof x === "bigint" ? Number(x) : x));
  const parts = [
    bigJson(cat.map((r) => [r.database_name, r.schema_name, r.table_name, r.kind, r.n ?? null, r.cols ?? null])),
    bigJson(dbs.map((r) => [r.database_name, r.path, r.type])),
    bigJson([...files].sort()),
    bigJson([...extensions].sort()),
  ];
  return fnv1a(parts.join("\u0001"));
}

// Settled, independent of how it got there: every lock switch at the value
// the lock demands, `enable_logging` included (L-1 — the real control, not
// the `LOGGING_SHADOW` macro, which only catches the unqualified spelling).
export async function engineLocked(db) {
  const c = await db.connect();
  try {
    const r = (
      await c.query(
        "SELECT current_setting('enable_external_access')::BOOLEAN AS e, current_setting('lock_configuration')::BOOLEAN AS l, " +
          "current_setting('autoload_known_extensions')::BOOLEAN AS a, current_setting('autoinstall_known_extensions')::BOOLEAN AS i, " +
          "current_setting('enable_logging')::BOOLEAN AS g"
      )
    )
      .toArray()[0]
      .toJSON();
    return r.e === false && r.l === true && r.a === false && r.i === false && r.g === false;
  } catch (_) {
    return false; // e.g. after enable_logging(storage='file') every query fails
  } finally {
    await c.close();
  }
}

// Build the engine: a fresh `AsyncDuckDB` (`makeDb`), its fixed extensions,
// its allow-listed files, the lock, then the snapshot. `doc` is the document
// whose CSP must already be proven enforced (skip in Node: no document).
// `lock: false` is reachable only through `__testing` (e2e step 7 and the
// H-1 preview fixture need an unlocked engine to show the CSP alone is not
// enough; app.js and assist*.js never import `__testing` and never pass
// `lock:` — a static grep test).
export async function buildEngine({ makeDb, page, extensions = [], files = [], limits = SNAPSHOT_LIMITS, doc = globalThis.document, signal, onProgress, lock = true }) {
  if (doc) await assistCspProven(doc); // L-4: rejects unless enforcement was proven
  const db = await makeDb();
  try {
    try {
      await db.open({ path: ":memory:", allowUnsignedExtensions: true }); // anofox_* are unsigned; irrelevant after the lock
    } catch (_) {
      await db.open({ path: ":memory:" });
    }
    const c = await db.connect();
    await tryQ(c, `SET memory_limit='${limits.memory}'`);
    await tryQ(c, "SET preserve_insertion_order=false");
    const loaded = [];
    for (const e of extensions) {
      if (!ENGINE_EXTENSIONS.includes(e)) throw new Error(`not an engine extension: ${e}`);
      // sec-review-256 M-1: a merely *raced* timeout does not stop
      // anything — `loadExt`'s queries kept running on `c` in the
      // background, interleaved with the next extension's, the file
      // registration, the lock and the snapshot (the lock still held when
      // that happened, but which extensions ended up loaded became a
      // function of network timing, and `engine.extensions` could lie).
      // There is no cancelSent()-style recovery for a stuck LOAD either
      // (M-2: that doesn't stop a plain `query()`) — the only thing that
      // actually stops it is terminating the Worker outright, which makes
      // `c` unusable. So a timed out load fails the whole build (fail
      // closed) rather than continuing with a partly loaded, racy
      // extension set.
      if (await loadExt(c, db, e, limits.extMs ?? SNAPSHOT_LIMITS.extMs)) loaded.push(e);
    }
    const allowed = [];
    for (const name of files) {
      if (!safeFileName(name)) continue;
      try {
        const h = page.handle?.(name);
        if (h) await db.registerFileHandle(name, h, 2 /* DuckDBDataProtocol.BROWSER_FILEREADER */, true);
        else await db.registerFileBuffer(name, await page.copyFile(name, limits.fileBytes));
        allowed.push(name);
      } catch (_) {
        /* a file that can't be read is simply not allow-listed */
      }
    }
    if (lock) {
      for (const s of ENGINE_LOCK_SQL) {
        await c.query(s);
        if (s === "SET allowed_directories=[]" && allowed.length) await c.query(`SET allowed_paths=[${allowed.map(sqlStr).join(",")}]`);
      }
      await c.query(LOGGING_SHADOW);
      if (!(await engineLocked(db))) throw new Error("assistant engine: not locked");
    }
    const tables = await snapshotTables(c, page, limits, { signal, onProgress });
    await c.close();
    return {
      db,
      extensions: loaded,
      files: allowed,
      tables,
      fingerprint: await pageFingerprint(page, { extensions: loaded, files: allowed, limits }),
      close: () => db.terminate?.(),
    };
  } catch (e) {
    try {
      await db.terminate?.();
    } catch (_) {}
    throw e;
  }
}

// e2e step 7 / the H-1 preview fixture only — never imported by app.js or
// assist*.js (a static grep test, tests/web/assist_engine_test.mjs item 12).
export const __testing = { unlocked: (o) => buildEngine({ ...o, lock: false }) };
