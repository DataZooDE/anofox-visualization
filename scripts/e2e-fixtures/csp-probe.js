// Served helper for scripts/assist-sandbox-e2e.mjs (never injected through
// page.evaluate string eval — page.evaluate's eval is exempt from the
// page's CSP, a Playwright/CDP artefact (plan-204 §1.5), so anything that
// must prove the CSP's own effect runs from a file the page loads instead).
// Copied next to web/index.html for the run (same origin, so relative
// imports of web/assist-csp.js, web/assist-engine.js, web/assist.js and
// web/pkg/ resolve) and removed afterwards.
//
// Exposes window.__e2e: small, composable async functions the e2e script
// calls through page.evaluate (calling an already-defined function is not
// the eval the reviewer flagged; writing new code as a string is).
import { applyAssistCsp, assistCspProven } from "../assist-csp.js";
import { buildEngine, engineExtensions, engineLocked, pageAdapter, __testing } from "../assist-engine.js";
import { wasmTools } from "../assist.js";
import init, { plan as wasmPlan, lint_step as wasmLintStep, page_sql as wasmPageSql } from "../pkg/anofox_visualization.js";

await init(); // the core wasm (plan/lint_step) — harmless to the CSP: same-origin, part of appBase

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
    class EmptyTable {
      constructor() {}
      toArray() {
        return [];
      }
      slice() {
        return this;
      }
    }
    const page = pageAdapter(
      {
        connect: async () => ({
          query: async () => new EmptyTable(),
          send: async () => ({ schema: {}, [Symbol.asyncIterator]: () => ({ next: async () => ({ done: true }) }) }),
          cancelSent: async () => true,
        }),
      },
      {}
    );
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
    engine.close();
    return { locked, tables };
  },
  // sec-review-256 L-1: a normalised spelling of an allowed file must
  // never read the registered bytes — it must read *nothing* (not fall
  // through to some other file), and the attempt must never touch the
  // network. The exact name does read the registered bytes.
  async pathSpellings() {
    const bytes = new TextEncoder().encode("a,b\n1,2\n");
    await pageDb.registerFileBuffer("data/x/y.csv", bytes);
    const page = pageAdapter(pageDb, { handles: new Map() });
    const engine = await buildEngine({ makeDb, page, extensions: ["json"], files: ["data/x/y.csv"], doc: document });
    try {
      const c = await engine.db.connect();
      const read = async (p) => {
        try {
          const rows = (await c.query(`SELECT * FROM read_text('${p}')`)).toArray().map((r) => r.toJSON());
          return { rows: rows.length, content: rows[0]?.content };
        } catch (e) {
          return { error: String(e?.message || e) };
        }
      };
      const exact = await read("data/x/y.csv");
      const spellings = {};
      for (const p of ["./data/x/y.csv", "data/x/../x/y.csv", "data//x/y.csv", "data/x/y.csv/", "DATA/X/Y.CSV"]) spellings[p] = await read(p);
      return { exact, spellings };
    } finally {
      engine.close();
    }
  },
  // sec-review-256 M-1: the full fixed extension set, spatial included —
  // engine.extensions must equal what the engine's own catalog says is
  // actually loaded (minus DuckDB's always-on core_functions), and a
  // spatial scalar must work. The old e2e only ever requested ["json"],
  // which is exactly why the 0 ms race went unnoticed.
  async buildLockedWithSpatial() {
    const page = pageAdapter(pageDb, {});
    const requested = engineExtensions({ pageLoaded: ["spatial"] });
    const engine = await buildEngine({ makeDb, page, extensions: requested, doc: document });
    try {
      const c = await engine.db.connect();
      const loadedRows = (await c.query("SELECT extension_name AS n FROM duckdb_extensions() WHERE loaded")).toArray().map((r) => r.n);
      const point = (await c.query("SELECT ST_AsText(ST_Point(1,2)) AS g")).toArray()[0].toJSON().g;
      return { requested, reported: engine.extensions, loadedRows, point };
    } finally {
      engine.close();
    }
  },
  // sec-review-256 M-2: a dashboard view that would run (near-)forever —
  // the engine build must still finish close to queryMs, the view must be
  // excluded as "timed out", and the *page's own* DuckDB (the one thing a
  // viewer is actually using) must answer promptly once the build is done,
  // not after the view would have finished on its own.
  async heavyViewTimeout() {
    const c = await pageDb.connect();
    await c.query("CREATE OR REPLACE VIEW heavy AS SELECT count(*) n FROM range(200000000) a, range(1000) b");
    const page = pageAdapter(pageDb, {});
    const t0 = performance.now();
    const engine = await buildEngine({ makeDb, page, extensions: ["json"], limits: { tableRows: 1_000_000, totalRows: 3_000_000, totalBytes: 512 << 20, fileBytes: 256 << 20, memory: "512MB", queryMs: 300, catalogMs: 300, extMs: 30_000 }, doc: document });
    const buildMs = performance.now() - t0;
    const heavy = engine.tables.find((t) => /"heavy"$/.test(t.fq));
    engine.close();
    const t1 = performance.now();
    await c.query("SELECT 1");
    const pageAnsweredMs = performance.now() - t1;
    await c.close();
    return { buildMs, excluded: heavy?.excluded, pageAnsweredMs };
  },
  // plan-204 §5.3 step 8 / sec-review-256 L-3, implemented as planned: a
  // test-only gate bypass (__testing.admitAll), a preview statement that
  // reads the recorder, a markdown panel's <img> pointed at the recorder,
  // and a spy on the page connection — all three must show nothing
  // reached the page or the network outside the engine's own
  // (CSP-blocked) attempt.
  async previewGateBypass(recUrl) {
    const pageSeen = [];
    const spiedPageDb = {
      connect: async () => {
        const c = await pageDb.connect();
        return new Proxy(c, {
          get: (o, k) => (k === "query" || k === "send" || k === "prepare" ? (s) => (pageSeen.push(String(s)), o[k](s)) : typeof o[k] === "function" ? o[k].bind(o) : o[k]),
        });
      },
      copyFileToBuffer: (n) => pageDb.copyFileToBuffer(n),
    };
    const page = pageAdapter(spiedPageDb, {});
    const engine = await buildEngine({ makeDb, page, extensions: ["json"], doc: document });
    try {
      pageSeen.length = 0; // only the preview below is spied on
      const tools = wasmTools({ db: engine.db, lintStep: wasmLintStep, plan: wasmPlan, pageSql: wasmPageSql, snapshot: new Map(engine.tables.map((t) => [t.fq, t])), __testing: { admitAll: true } });
      const preview = tools.previewRunner(`SELECT * ::TABLE FROM read_csv('${recUrl}/preview.csv');`);
      let previewOk = false,
        previewError = null;
      try {
        await preview(`SELECT * FROM read_csv('${recUrl}/preview.csv')`);
        previewOk = true; // the lock must refuse this before it ever reaches the network
      } catch (e) {
        previewError = String(e?.message || e);
      }
      // The markdown panel: a `::MARKDOWN` row's text is rendered to HTML
      // client-side (app.js's renderMarkdown, not render_panel's SVG — a
      // model-shaped row produces exactly the same `<img src>` a renderer
      // would), inserted into the *live* document (it must actually be
      // connected to trigger a fetch at all) so the ratchet's img-src has
      // something real to block. render_panel itself only ever gets rows
      // (confirmed above: the preview's own data never touches it either).
      const img = document.createElement("img");
      img.src = `${recUrl}/pimg`;
      document.body.appendChild(img);
      await new Promise((resolve) => {
        img.onload = img.onerror = resolve;
        setTimeout(resolve, 500);
      });
      img.remove();
      return { previewOk, previewError, pageSeen };
    } finally {
      engine.close();
    }
  },
  // sec-review-256 Info: the GDAL VSI-prefixed Permission Error rewrite
  // ("not available to the assistant: …", never DuckDB's raw message).
  async gdalRewrite(recUrl) {
    const page = pageAdapter(pageDb, {});
    const vsicurl = `/vsicurl/${recUrl}/x.geojson`;
    const engine = await buildEngine({ makeDb, page, extensions: ["json", "spatial"], doc: document });
    try {
      const tools = wasmTools({ db: engine.db, lintStep: wasmLintStep, plan: wasmPlan, pageSql: wasmPageSql, snapshot: new Map(engine.tables.map((t) => [t.fq, t])), known: new Set([vsicurl]) });
      try {
        await tools.run_sql({ sql: `SELECT * FROM st_read('${vsicurl}')`, purpose: "x" });
        return { ok: true };
      } catch (e) {
        return { ok: false, message: String(e?.message || e) };
      }
    } finally {
      engine.close();
    }
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
