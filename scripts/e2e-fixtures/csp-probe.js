// Served helper for scripts/assist-sandbox-e2e.mjs (never injected through
// page.evaluate string eval — page.evaluate's eval is exempt from the
// page's CSP, a Playwright/CDP artefact (plan-204 §1.5), so anything that
// must prove the CSP's own effect runs from a file the page loads instead).
// Copied next to web/index.html for the run (same origin, so relative
// imports of web/assist-csp.js, web/assist-engine.js and web/assist.js
// resolve) and removed afterwards.
//
// Exposes window.__e2e: small, composable async functions the e2e script
// calls through page.evaluate (calling an already-defined function is not
// the eval the reviewer flagged; writing new code as a string is).
import { applyAssistCsp, assistCspProven } from "../assist-csp.js";
import { buildEngine, engineLocked, pageAdapter, __testing } from "../assist-engine.js";
import { wasmTools } from "../assist.js";

// The harness's own "page": a tiny unlocked AsyncDuckDB, imported once at
// module load (before any ratchet in this page ever runs) — I-4, never
// import() DuckDB-Wasm again afterwards.
const duckdbMod = await import("https://cdn.jsdelivr.net/npm/@duckdb/duckdb-wasm@1.33.1-dev65.0/+esm");
async function makeDb() {
  const bundle = await duckdbMod.selectBundle(duckdbMod.getJsDelivrBundles());
  const url = URL.createObjectURL(new Blob([`importScripts("${bundle.mainWorker}");`], { type: "text/javascript" }));
  const d = new duckdbMod.AsyncDuckDB(new duckdbMod.ConsoleLogger(), new Worker(url));
  await d.instantiate(bundle.mainModule, bundle.pthreadWorker);
  URL.revokeObjectURL(url);
  return d;
}

let pageDb = null;
window.__e2e = {
  // Created once, before the ratchet (the e2e script calls this first) —
  // R1: this Worker must keep its network after the ratchet.
  async makePageDb() {
    pageDb = await makeDb();
    await pageDb.open({ path: ":memory:" });
    const c = await pageDb.connect();
    await c.query("CREATE TABLE sales AS SELECT ['EU','US'][1+i%2] AS region, i*1.0 AS amount FROM range(0,50) t(i)");
    await c.close();
  },
  async pageReadsUrl(url) {
    const c = await pageDb.connect();
    try {
      await c.query(`SELECT * FROM read_csv('${url}')`);
      return { ok: true };
    } catch (e) {
      return { ok: false, error: String(e?.message || e) };
    } finally {
      await c.close();
    }
  },
  async ratchet(imgOrigins = []) {
    try {
      await applyAssistCsp(document, { imgOrigins });
      return { ok: true };
    } catch (e) {
      return { ok: false, error: String(e?.message || e) };
    }
  },
  async proven() {
    try {
      await assistCspProven(document);
      return true;
    } catch (_) {
      return false;
    }
  },
  // The CSP backstop alone (no lock): proves the engine Worker, created
  // after the ratchet, has no network even unlocked (plan-204 §1.4/§1.5).
  async unlockedRead(url) {
    const page = pageAdapter({ connect: async () => ({ query: async () => ({ toArray: () => [] }) }) }, {});
    const engine = await __testing.unlocked({ makeDb, page, extensions: [], doc: document });
    try {
      const c = await engine.db.connect();
      await c.query(`SELECT * FROM read_csv('${url}')`);
      return { ok: true };
    } catch (e) {
      return { ok: false, error: String(e?.message || e) };
    } finally {
      engine.close();
    }
  },
  // The real, locked engine over the harness's own page db: buildEngine end
  // to end in a real browser, under the CSP already in place.
  async buildLocked() {
    const page = pageAdapter(pageDb, {});
    const engine = await buildEngine({ makeDb, page, extensions: ["json"], doc: document });
    const locked = await engineLocked(engine.db);
    const tables = engine.tables;
    return { locked, tables, engine };
  },
  // H-1: the proposal preview runs in the locked engine, never on the
  // page — a legitimate proposal (reads only `sales`, which the gate
  // allows) is previewed while a spy wraps the harness's page connection;
  // the e2e script checks the recorder and the spy separately.
  async previewOnLockedEngine() {
    const page = pageAdapter(pageDb, {});
    const engine = await buildEngine({ makeDb, page, extensions: ["json"], doc: document });
    try {
      const tools = wasmTools({ db: engine.db, lintStep: () => "{}", plan: () => "[]", pageSql: () => "", snapshot: new Map(engine.tables.map((t) => [t.fq, t])) });
      const preview = tools.previewRunner("SELECT region, sum(amount) AS s FROM sales GROUP BY ALL;");
      const rows = await preview("SELECT region, sum(amount) AS s FROM sales GROUP BY ALL ORDER BY 2 DESC");
      return { ok: true, rows };
    } catch (e) {
      return { ok: false, error: String(e?.message || e) };
    } finally {
      engine.close();
    }
  },
};
