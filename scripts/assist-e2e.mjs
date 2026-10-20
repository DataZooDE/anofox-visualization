// The assistant panel (#244, M1-3), in a real browser: the full loop against
// a mocked Anthropic Messages API (never a real call — api.anthropic.com is
// intercepted at the browser network layer, scripts/e2e-fixtures/assist-mock.mjs),
// the preview's C-1 isolation (zero messages to the page's own DuckDB
// Worker while a proposal is previewed and the viewer changes inputs,
// drills or pages), the key never leaking anywhere (R3, the sk-ant- sweep),
// the CSP-before-everything order (R7/C-2), and the phone layout.
//
//   wasm-pack build --target web --out-dir <pkg> --no-default-features --features wasm
//   scripts/fetch-wasm-extensions.sh
//   NODE_PATH=<dir with playwright>/node_modules TMPDIR=<scratch>/tmp \
//     node scripts/assist-e2e.mjs --pkg <pkg dir> [--port 8793] [--rec-port 8794] \
//     [--axe axe.min.js] [--shots DIR] [--only main,phone,a11y,never,csp]
import { createRequire } from "module";
import { createServer } from "http";
import { readFileSync, existsSync, mkdirSync } from "fs";
import path from "path";
import { fileURLToPath } from "url";
import { installMock, sse, text, toolUse, workerSpyInit, violationInit, cfg } from "./e2e-fixtures/assist-mock.mjs";

const require = createRequire(import.meta.url);
let chromium;
try {
  ({ chromium } = require("playwright"));
} catch (e) {
  console.error("Playwright is not installed (NODE_PATH=<dir with playwright>/node_modules node scripts/assist-e2e.mjs)");
  process.exit(1);
}
const arg = (k, d) => {
  const i = process.argv.indexOf(k);
  return i > 0 ? process.argv[i + 1] : d;
};
const PKG = arg("--pkg", "");
const PORT = Number(arg("--port", "8793"));
const REC_PORT = Number(arg("--rec-port", "8794"));
const AXE = arg("--axe", "");
const SHOTS = arg("--shots", "");
const ONLY = arg("--only", "") ? arg("--only", "").split(",") : null;
const run = (tag) => !ONLY || ONLY.includes(tag);
if (!PKG || !existsSync(path.join(PKG, "anofox_visualization_bg.wasm"))) {
  console.error("usage: assist-e2e.mjs --pkg <wasm pkg dir> (wasm-pack build --target web --out-dir <dir> --no-default-features --features wasm)");
  process.exit(2);
}
if (SHOTS) mkdirSync(SHOTS, { recursive: true });

const here = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(here, "..");
const webDir = path.join(root, "web");
const fixDir = path.join(here, "e2e-fixtures", "assist");
const SALES_SETUP = readFileSync(path.join(fixDir, "sales-setup.sql"), "utf8");
const CLEAN = readFileSync(path.join(fixDir, "proposal-clean.sql"), "utf8");
const HOSTILE_TMPL = readFileSync(path.join(fixDir, "proposal-hostile.sql"), "utf8");
const REPLY_HOSTILE_TMPL = readFileSync(path.join(fixDir, "reply-hostile.txt"), "utf8");
const HOSTILE = HOSTILE_TMPL.replaceAll("${REC}", String(REC_PORT));
const REPLY_HOSTILE = REPLY_HOSTILE_TMPL.replaceAll("${REC}", String(REC_PORT));
const SETUP_AND_METRIC = `${SALES_SETUP}\nSELECT count(*) AS n ::METRIC FROM sales;\n`;
// M9b (sec-review-272 H-1): a proposal with exactly one class-3 statement
// that is harmless to actually run for real — SET memory_limit is refused
// (not on SETTABLE) but has no reader, no ATTACH/INSTALL/COPY, nothing that
// reaches the network or writes a file — so "Apply, don't run" then a real
// explicit Run proves the hold both holds (no dialog bypass needed to
// reach it: refused.length > 0 alone gets the three-button dialog) and
// lifts, without the test itself running anything dangerous for real.
const HOLD_CLEAN = `${SALES_SETUP}\nSET memory_limit='2GB';\nSELECT count(*) AS n ::METRIC FROM sales;\n`;
// H-1R (sec-review-272-rereview): a render queued by the proposal's own
// ::RELOAD timer (or any other trigger firing mid-render) can still be in
// flight when the viewer rejects the proposal — Discard, or Apply's
// "Apply, don't run". ATTACH/SET need no network to prove the race (no
// REC_PORT dependency): adapted from the re-review's own probe template,
// scratch-244/tmp/rr/probe3.mjs (`discard,norun`), which reproduced this
// 3 of 3 trials against the unfixed code.
const RACE = `SELECT 0.1::RELOAD;\nSELECT 'RACEMARK' AS m ::METRIC;\nSELECT count(*) AS n ::METRIC FROM range(60000000) t(x) WHERE x % 7 = 3;\nSELECT 'RACEMARK2' AS m2 ::METRIC;\nATTACH ':memory:' AS race_attach;\nSET memory_limit='3GB';\nSELECT 'RACEMARK3' AS m3 ::METRIC;\n`;
// M-2 (sec-review-272-rereview): a proposal with a refused SET/ATTACH and
// a remote read_csv, to prove "Check" can't run any of it on the page
// database while held.
const CHECK_HOLD = `SET memory_limit='3GB';\nATTACH ':memory:' AS check_attach;\nSELECT 'CHECKMARK' AS m ::METRIC;\nSELECT * FROM read_csv('http://127.0.0.1:${REC_PORT}/check.csv') ::TABLE;\n`;
// L-3 (sec-review-272-rereview): a ::VIEW group with a model-computed
// (not literal) second alternative's label — clicking it during the
// preview must never write into the address bar (setViewParam's missing
// `if (preview) return;` guard).
const VIEWP = `SELECT region ::XAXIS, ROUND(SUM(units*price),0) ::BARCHART, 'Revenue'::TITLE FROM sales GROUP BY 1 ORDER BY 2 DESC;\nSELECT 'v1' AS rrview ::VIEW;\nSELECT count(*) AS n ::METRIC FROM sales;\nSELECT (SELECT 'L-' || sum(units)::VARCHAR FROM sales) ::VIEW;\n`;
// H-1R2 (sec-review-272-rereview final check): a hidden ::TAB with a
// deferred CREATE and six slow prefetched panels — adapted verbatim from
// the reviewer's own probe template, scratch-244/tmp/rr/probe8.mjs. The
// idle prefetch starts the hidden tab's slow jobs while the preview sits
// there; the user then exits (typing, or "Apply, don't run") and, about
// 80 ms later — still inside the exit — presses Ctrl/⌘+Enter in the
// editor. None of the hidden tab's statements (including its own
// deferred CREATE) may reach the page database once that lazy work
// finally runs.
const LAZY_SLOW = (k) => `SELECT count(*) AS n${k} ::METRIC FROM range(400000000) t(x) WHERE x % 7 = ${k};`;
const LAZY_PROP = `SELECT 'Main'::TAB;\nSELECT 'RACEMARK' AS m ::METRIC;\nSELECT 'Hidden'::TAB;\nCREATE OR REPLACE TABLE lazymark_tbl AS SELECT 1 AS x;\n${[1, 2, 3, 4, 5, 6].map(LAZY_SLOW).join("\n")}\nSELECT 'LAZYMARK' AS q ::METRIC;\n`;
const LAZY_RE = /RACEMARK|LAZYMARK|lazymark_tbl/;

const results = [];
const record = (name, ok, note = "") => {
  results.push({ name, ok, note });
  console.log(`${ok ? "ok  " : "FAIL"} ${name}${note ? " — " + note : ""}`);
  if (!ok) process.exitCode = 1;
};
const b64 = (s) => Buffer.from(s, "utf8").toString("base64");

// ---- servers (loopback; stopped by us at the end) ----
const MIME = { ".html": "text/html", ".js": "text/javascript", ".mjs": "text/javascript", ".css": "text/css", ".json": "application/json", ".wasm": "application/wasm", ".md": "text/markdown", ".csv": "text/csv", ".parquet": "application/octet-stream" };
let liveMode = false;
let currentCfg = null; // the in-memory app-config.json body, or null (404)
const staticLog = [];
const staticServer = createServer((req, res) => {
  const u = new URL(req.url, "http://x");
  staticLog.push(u.pathname);
  if (u.pathname === "/app-config.json") {
    if (currentCfg == null) {
      res.writeHead(404);
      res.end();
      return;
    }
    res.writeHead(200, { "Content-Type": "application/json", "Cache-Control": "no-store" });
    res.end(JSON.stringify(currentCfg));
    return;
  }
  if (u.pathname === "/query" && req.method === "POST") {
    if (liveMode) {
      res.writeHead(200, { "Content-Type": "application/json" });
      res.end('[{"ok":1}]');
    } else {
      res.writeHead(404);
      res.end();
    }
    return;
  }
  if (u.pathname === "/__frame.html") {
    res.writeHead(200, { "Content-Type": "text/html" });
    res.end('<!doctype html><iframe src="/?mode=edit" width="1200" height="800"></iframe>');
    return;
  }
  // Same-origin axe-core (addScriptTag({url})) — once a key is stored the
  // assistant's own CSP (script-src with no 'unsafe-inline') blocks
  // addScriptTag({path})'s *inline* injection; a same-origin <script src>
  // is unaffected.
  if (u.pathname === "/__axe__/axe.min.js" && AXE && existsSync(AXE)) {
    res.writeHead(200, { "Content-Type": "text/javascript", "Cache-Control": "no-store" });
    res.end(readFileSync(AXE));
    return;
  }
  let p;
  if (u.pathname.startsWith("/pkg/")) p = path.join(PKG, u.pathname.slice("/pkg/".length));
  else p = path.join(webDir, decodeURIComponent(u.pathname));
  if (p.endsWith("/")) p = path.join(p, "index.html");
  try {
    const data = readFileSync(p);
    res.writeHead(200, { "Content-Type": MIME[path.extname(p)] || "application/octet-stream", "Cache-Control": "no-store" });
    res.end(data);
  } catch (_) {
    res.writeHead(404);
    res.end("not found");
  }
});
const recLog = [];
const recServer = createServer((req, res) => {
  recLog.push(`${req.method} ${req.url}`);
  res.writeHead(200, { "Content-Type": "text/plain", "Access-Control-Allow-Origin": "*" });
  res.end(/\.csv(\?|$)/.test(req.url) ? "a,b\n1,2\n" : "ok");
});
await new Promise((ok) => staticServer.listen(PORT, "127.0.0.1", ok));
await new Promise((ok) => recServer.listen(REC_PORT, "127.0.0.1", ok));
const BASE = `http://127.0.0.1:${PORT}/`;
const RECBASE = `http://127.0.0.1:${REC_PORT}`;
console.log(`static ${BASE}  recorder ${RECBASE}`);

const browser = await chromium.launch();

// Every context gets the worker spy + CSP-violation recorder; `cfgObj`
// (null: no app-config.json, 404) becomes the static server's answer for
// the lifetime of the context — tests never share one, so one case's
// config can never leak into another's.
async function newCtx(opts = {}) {
  const { viewport = { width: 1400, height: 1000 }, reducedMotion = "reduce", colorScheme = "light" } = opts;
  // Only reassigned when the caller actually passes `cfgObj` — otherwise a
  // `currentCfg = …` the caller already did (its own case-specific setup,
  // e.g. before an inner page.goto()) must not be clobbered back to the
  // default by this call.
  if (Object.prototype.hasOwnProperty.call(opts, "cfgObj")) currentCfg = opts.cfgObj;
  // ignoreHTTPSErrors: installMock() redirects api.anthropic.com to a local
  // HTTPS server with a throwaway self-signed cert (assist-mock.mjs).
  const context = await browser.newContext({ viewport, reducedMotion, colorScheme, ignoreHTTPSErrors: true });
  if (process.env.ASSIST_E2E_DEBUG) {
    context.on("page", (p) => {
      p.on("console", (m) => console.log("[console]", m.text()));
      p.on("pageerror", (e) => console.log("[pageerror]", String(e)));
      p.on("requestfinished", (r) => {
        if (/api\.anthropic\.com/.test(r.url())) console.log("[req]", r.method(), r.url());
      });
      p.on("requestfailed", (r) => {
        if (/api\.anthropic\.com/.test(r.url())) console.log("[reqfail]", r.method(), r.url(), r.failure()?.errorText);
      });
    });
  }
  await context.addInitScript(workerSpyInit);
  await context.addInitScript(violationInit);
  return context;
}
function settled(page, timeout = 120000) {
  return page.waitForFunction(() => /\d+ panels?/.test(document.getElementById("status")?.textContent || "") && !document.body.classList.contains("loading"), null, { timeout });
}
async function gotoEdit(page, sql, extra = "") {
  await page.goto(`${BASE}?mode=edit${extra}#sql=${b64(sql)}`);
  await settled(page);
}
const dbSpy = (page) => page.evaluate(() => window.__dbSpy);
const mark = async (page) => (await dbSpy(page)).msgs.length;
const since = async (page, n) => (await dbSpy(page)).msgs.slice(n);
const pageMsgs = (msgs) => msgs.filter((m) => m.w === 0);
// H-1R/M-2 (sec-review-272-rereview): page-worker messages since mark `n0`
// whose SQL matches `re` — a tighter check than pageMsgs() alone when
// other, legitimate page-DB traffic (restoring the page's own dashboard)
// is expected around the same window; this isolates the PROPOSAL's own
// text specifically.
const w0with = async (page, n0, re) => pageMsgs(await since(page, n0)).filter((m) => m.sql && re.test(m.sql));

// A default mock handler: turn 1 profiles "sales", turn 2 proposes CLEAN.
function defaultHandler() {
  let n = 0;
  return async () => {
    n++;
    if (n === 1) return sse({ content: [toolUse("toolu_1", "profile_table", { table: "sales" })], stop_reason: "tool_use" });
    return sse({ content: [toolUse("toolu_2", "propose_dashboard", { sql: CLEAN, summary: "How regions and channels sell.", panels: [{ stmt: 2, title: "Overview", why: "the headline" }], open_issues: [] })], stop_reason: "tool_use" });
  };
}

// ---- never (N): the button/module/SDK must not appear at all ----
async function caseNever(label, setup, assertHidden = true) {
  staticLog.length = 0;
  currentCfg = null; // the default for N1; a case's own setup() may override it before its goto()
  const context = await newCtx();
  const mock = await installMock(context, async () => sse({ content: [text("n/a")] }));
  const page = await context.newPage();
  await setup(page, context);
  if (assertHidden) {
    const hidden = await page.evaluate(() => document.getElementById("assist-btn")?.hidden !== false);
    record(`N ${label}: #assist-btn hidden`, hidden);
  }
  const leaked = staticLog.filter((p) => /assist-ui\.(js|css)|vendor\/anthropic-sdk|assist\/guide\.md/.test(p));
  record(`N ${label}: no assist-ui.js/.css/vendor-sdk/guide.md requested`, leaked.length === 0, JSON.stringify(leaked));
  record(`N ${label}: no meta[data-anofox-assist]`, (await page.evaluate(() => !document.querySelector("meta[data-anofox-assist]"))) === true);
  record(`N ${label}: Ctrl+I does nothing`, true); // asserted implicitly: no panel ever appears below
  await page.keyboard.press("Control+KeyI").catch(() => {});
  await page.waitForTimeout(150);
  const paneShown = await page.evaluate(() => !!document.getElementById("assist-pane") && !document.getElementById("assist-pane").hidden);
  record(`N ${label}: Ctrl+I opens nothing`, !paneShown);
  record(`N ${label}: no api.anthropic.com request`, mock.requests.length === 0);
  await context.close();
}

if (run("never")) {
  await caseNever("1 cfg null", async (page) => page.goto(`${BASE}?mode=edit`).then(() => settled(page)));
  await caseNever("2 embed", async (page) => {
    currentCfg = cfg(true);
    await page.goto(`${BASE}?mode=edit&embed=1`);
    await settled(page);
  });
  await caseNever("3 served", async (page) => {
    currentCfg = cfg(true);
    await page.addInitScript(() => {
      window.__served = { id: "x", sql: "SELECT 1 AS n ::METRIC", title: "T" };
    });
    await page.goto(`${BASE}`);
    await settled(page);
  });
  await caseNever(
    "4 framed",
    async (page, context) => {
      currentCfg = cfg(true);
      await page.goto(`${BASE}__frame.html`);
      const frame = page.frames()[1];
      await frame.waitForFunction(() => /\d+ panels?/.test(document.getElementById("status")?.textContent || ""), null, { timeout: 60000 });
      const hidden = await frame.evaluate(() => document.getElementById("assist-btn")?.hidden !== false);
      record("N 4 framed: #assist-btn hidden in the frame", hidden);
    },
    false
  );
  await caseNever("5 url+comment can't enable it", async (page) => {
    currentCfg = null;
    await page.goto(`${BASE}?mode=edit&assistant=1&assistant.enabled=true#sql=${b64("-- @assistant enabled\nSELECT '{\"assistant\":{\"enabled\":true}}'::THEME;\nSELECT 1 AS n ::METRIC;")}`);
    await settled(page);
  });
  await caseNever("6 live backend", async (page) => {
    currentCfg = cfg(true);
    liveMode = true;
    await page.goto(`${BASE}?mode=edit`);
    await settled(page).catch(() => {});
    liveMode = false;
  });
  {
    // N7: view mode hidden; after switching to edit it's visible, but
    // assist-ui.js is still not requested until opened.
    currentCfg = cfg(true);
    staticLog.length = 0;
    const context = await newCtx();
    const mock = await installMock(context, async () => sse({ content: [text("n/a")] }));
    const page = await context.newPage();
    await page.goto(`${BASE}?mode=view#sql=${b64(SETUP_AND_METRIC)}`);
    await settled(page);
    record("N7 view mode: #assist-btn hidden", await page.evaluate(() => document.getElementById("assist-btn")?.hidden !== false));
    // Info-3 (sec-review-272, defence in depth): a scripted click on the
    // hidden button (bypassing the UI, not a real pointer event) must still
    // do nothing in View mode — assistCfg.enabled alone is not re-checked;
    // openAssistant() re-reads the live mode itself.
    await page.evaluate(() => document.getElementById("assist-btn")?.click());
    await page.waitForTimeout(300);
    record("N7 view mode: a scripted click on the hidden button opens nothing", await page.evaluate(() => document.getElementById("assist-pane")?.hidden !== false && !document.querySelector(".dp-dialog") && !document.querySelector("meta[data-anofox-assist]")));
    record("N7 view mode: scripted click still did not request assist-ui.js", !staticLog.some((p) => /assist-ui\.js/.test(p)));
    await page.click("#mode-edit");
    await page.waitForFunction(() => document.getElementById("assist-btn")?.hidden === false, null, { timeout: 15000 }).catch(() => {});
    record("N7 edit mode: #assist-btn visible", await page.evaluate(() => document.getElementById("assist-btn")?.hidden === false));
    record("N7 assist-ui.js still not requested", !staticLog.some((p) => /assist-ui\.js/.test(p)));
    await context.close();
  }
  {
    // N8: click ✦ — CSP meta present, key dialog open, SDK not requested;
    // Cancel keeps it that way; the SDK loads only once a key exists, at
    // the first Send (after consent), at the latest.
    currentCfg = cfg(true);
    staticLog.length = 0;
    const context = await newCtx();
    const mock = await installMock(context, defaultHandler());
    const page = await context.newPage();
    await gotoEdit(page, SETUP_AND_METRIC);
    await page.click("#assist-btn");
    await page.waitForSelector(".dp-dialog", { timeout: 15000 });
    record("N8: CSP meta present", await page.evaluate(() => !!document.querySelector("meta[data-anofox-assist]")));
    record("N8: assist-ui.js requested, SDK not yet", staticLog.some((p) => /assist-ui\.js/.test(p)) && !staticLog.some((p) => /vendor\/anthropic-sdk/.test(p)));
    await page.click('.dp-dialog button:has-text("Cancel")');
    await page.waitForTimeout(150);
    record("N8: after Cancel, the SDK is still not requested; the notice shows", !staticLog.some((p) => /vendor\/anthropic-sdk/.test(p)) && (await page.evaluate(() => !document.getElementById("ai-notice")?.hidden)));
    await context.close();
  }
}

// ---- main (M), cfg enabled, the recorder must stay empty throughout ----
let sharedPage = null,
  sharedContext = null,
  sharedMock = null;
async function mainCtx(handler) {
  currentCfg = cfg(true);
  const context = await newCtx();
  const mock = await installMock(context, handler);
  const page = await context.newPage();
  return { context, page, mock };
}
async function addKey(page, { remember = false } = {}) {
  await page.fill('.dp-dialog input[type="password"]', "sk-ant-e2e-0000000000000000000000000");
  if (remember) await page.check(".dp-dialog .ai-opt input[type=checkbox]").catch(() => {});
  await page.click('.dp-dialog button:has-text("Save key")');
  await page.waitForSelector(".dp-dialog", { state: "detached", timeout: 10000 }).catch(() => {});
}

if (run("main")) {
  // M0: the fixture is strict-clean.
  {
    const { context, page } = await mainCtx(defaultHandler());
    await gotoEdit(page, SETUP_AND_METRIC);
    await page.evaluate((sql) => {
      document.getElementById("sql").value = sql;
      document.getElementById("sql").dispatchEvent(new Event("input"));
    }, CLEAN + "\n" + SALES_SETUP.replace(/^/, "")); // placeholder, replaced below
    await page.fill("#sql", SALES_SETUP + "\n" + CLEAN);
    await page.check("#lint-strict");
    await page.click("#lint-run");
    await page.waitForFunction(() => !document.getElementById("lint-run").disabled, null, { timeout: 30000 });
    const okText = await page.evaluate(() => document.getElementById("lint-status")?.textContent || "");
    record("M0: setup+clean dashboard passes Check (strict)", /Passes/.test(okText), okText);
    await context.close();
  }
  // M1: new dashboard -> empty editor -> #assist-start visible.
  {
    const { context, page } = await mainCtx(defaultHandler());
    await gotoEdit(page, SETUP_AND_METRIC);
    await page.click("#side-new");
    await page.waitForTimeout(200);
    record("M1: editor empty after + New dashboard", (await page.inputValue("#sql")) === "");
    record("M1: #assist-start visible", await page.evaluate(() => document.getElementById("assist-start")?.hidden === false));
    await context.close();
  }
  // M2: type a request in the start card -> key dialog (no Remember, no opt-in).
  {
    const { context, page, mock } = await mainCtx(defaultHandler());
    await gotoEdit(page, SETUP_AND_METRIC);
    await page.click("#side-new");
    await page.fill("#assist-start-q", "Which regions sell the most?");
    await page.click('#assist-start button[type=submit]');
    await page.waitForSelector(".dp-dialog", { timeout: 15000 });
    record("M2: key dialog, no Remember checkbox (no opt-in)", (await page.locator(".dp-dialog .ai-opt").count()) === 0);
    await addKey(page);
    await page.waitForSelector(".dp-dialog", { timeout: 15000 }); // the "what will be sent" dialog
    record("M2: no request sent before consent", mock.requests.length === 0);
    const dlgText = await page.evaluate(() => document.querySelector(".dp-dialog")?.textContent || "");
    record('M2: "what will be sent" mentions the destination, sales and the request', /api\.anthropic\.com/.test(dlgText) && /sales/i.test(dlgText) && /Which regions sell the most/.test(dlgText));
    await context.close();
  }
  // M2b: with the opt-in, Remember is offered and stores the key in localStorage.
  {
    currentCfg = cfg(true, { remember: true });
    const context = await newCtx();
    const mock = await installMock(context, defaultHandler());
    const page = await context.newPage();
    await gotoEdit(page, SETUP_AND_METRIC);
    await page.click("#side-new");
    await page.fill("#assist-start-q", "x");
    await page.click('#assist-start button[type=submit]');
    await page.waitForSelector(".dp-dialog", { timeout: 15000 });
    record("M2b: Remember checkbox shown with the opt-in", (await page.locator(".dp-dialog .ai-opt").count()) === 1);
    await addKey(page, { remember: true });
    await page.waitForTimeout(200);
    const storage = await page.evaluate(() => ({ local: localStorage.getItem("dp_assist_key"), session: sessionStorage.getItem("dp_assist_key") }));
    record("M2b: remembered key -> localStorage, not sessionStorage", !!storage.local && !storage.session, JSON.stringify(storage));
    await context.close();
  }
  // M3: full propose flow.
  let M3 = null;
  {
    const { context, page, mock } = await mainCtx(defaultHandler());
    await gotoEdit(page, SETUP_AND_METRIC);
    await page.click("#side-new");
    await page.fill("#assist-start-q", "Which regions sell the most?");
    await page.click('#assist-start button[type=submit]');
    await page.waitForSelector(".dp-dialog", { timeout: 15000 });
    await addKey(page);
    await page.waitForSelector(".dp-dialog", { timeout: 15000 });
    await page.click('.dp-dialog button:has-text("Cancel")');
    record("M3: Cancel -> 0 requests, prompt kept", mock.requests.length === 0, (await page.inputValue("#ai-q")) || "");
    await page.click("#ai-send");
    await page.waitForSelector(".dp-dialog", { timeout: 15000 });
    await page.check(".dp-dialog .ai-opt input[type=checkbox]");
    await page.click('.dp-dialog button:has-text("Send")');
    if (process.env.ASSIST_E2E_DEBUG) {
      await page.waitForTimeout(15000);
      console.log("[debug] mock.requests.length:", mock.requests.length);
      console.log("[debug] ai-log:", await page.evaluate(() => document.getElementById("ai-log")?.textContent));
      console.log("[debug] ai-status:", await page.evaluate(() => document.getElementById("ai-status")?.textContent));
    }
    await page.waitForSelector("#ai-card:not([hidden])", { timeout: 30000 });
    record("M3: proposal card shown", await page.evaluate(() => !document.getElementById("ai-card").hidden));
    record("M3: focus on #ai-card-title", await page.evaluate(() => document.activeElement?.id === "ai-card-title"));
    record("M3: #preview-bar visible", await page.evaluate(() => document.getElementById("preview-bar")?.hidden === false));
    record("M3: the card's summary names regions and channels", /regions and channels/i.test(await page.evaluate(() => document.querySelector("#ai-card .ai-sum")?.textContent || "")));
    record("M3: editor still empty", (await page.inputValue("#sql")) === "");
    record("M3: engine sanity: 2 workers", (await dbSpy(page)).workers.length === 2, JSON.stringify((await dbSpy(page)).workers));
    M3 = { context, page, mock };
  }
  // M4: C-1 window — zero page-DB (worker 0) messages while inputs, drills
  // and pages change during the preview.
  {
    const { page } = M3;
    const n0 = await mark(page);
    await page.selectOption(".dash select[aria-label]", { index: 1 });
    await page.waitForTimeout(400);
    await page.click(".ai-pane .dash, .dash .dp-tab:has-text('Drill')").catch(() => {});
    await page.click("text=Drill").catch(() => {});
    await page.waitForTimeout(300);
    await page.evaluate(() => document.querySelector(".panel-svg > svg .dp-hit")?.dispatchEvent(new MouseEvent("click", { bubbles: true })));
    await page.waitForTimeout(400);
    await page.click("text=Detail").catch(() => {});
    await page.waitForTimeout(300);
    await page.click('button[title="next page"]').catch(() => {});
    await page.waitForTimeout(300);
    await page.click("#dark");
    await page.waitForTimeout(200);
    await page.click("#dark");
    await page.waitForTimeout(300);
    const msgsSince = await since(page, n0);
    const pageHits = pageMsgs(msgsSince);
    record("M4: zero messages to the page's own DuckDB worker (w===0) while previewing", pageHits.length === 0, JSON.stringify(pageHits.map((m) => m.sql).slice(0, 5)));
    record("M4: at least one message to the engine worker (w>=1)", msgsSince.some((m) => m.w >= 1));
    record("M4: recorder stays empty", recLog.length === 0, JSON.stringify(recLog));
    const url = page.url();
    record("M4: no v. or tab= params in the URL during preview", !/[?&](v\.|tab=)/.test(url), url);
  }
  // M5: Show changes.
  {
    const { page } = M3;
    await page.click("#preview-diff");
    await page.waitForSelector(".dp-dialog", { timeout: 10000 });
    const summary = await page.evaluate(() => document.querySelector(".dp-dialog p")?.textContent || "");
    record("M5: summary matches /panels? added/", /panels? added/.test(summary), summary);
    const insCount = await page.locator(".dp-dialog ins").count();
    record("M5: ins count > 0, each with sr-only 'added: '", insCount > 0 && (await page.locator(".dp-dialog ins .sr-only").count()) === insCount);
    await page.keyboard.press("Escape");
    await page.waitForTimeout(150);
    record("M5: Esc closes, focus back on #preview-diff", await page.evaluate(() => document.activeElement?.id === "preview-diff"));
  }
  // M5b (sec-review-272 L-1): "Preview again" must not leak an engine
  // connection — each re-preview should close the previous session's
  // connection first, so CONNECT minus DISCONNECT on the engine's own
  // Worker (index 1: "M3: engine sanity: 2 workers" above) nets to 1, not
  // 2/3/4 after repeated cycles. Typing in the editor exits the preview
  // without discarding the proposal (the existing #sql "input" listener),
  // so "Preview again" (#ai-preview) becomes clickable again each time.
  {
    const { page } = M3;
    for (let i = 0; i < 3; i++) {
      await page.fill("#sql", CLEAN + "\n-- edited " + i);
      await page.waitForTimeout(150);
      record(`M5b: #ai-preview is shown once the preview has exited (cycle ${i})`, await page.evaluate(() => document.getElementById("ai-preview")?.hidden === false));
      await page.click("#ai-preview");
      await page.waitForTimeout(300);
      record(`M5b: #ai-preview hides again once the preview is active (cycle ${i})`, await page.evaluate(() => document.getElementById("ai-preview")?.hidden === true));
    }
    const engineMsgs = (await dbSpy(page)).msgs.filter((m) => m.w === 1 && (m.type === "CONNECT" || m.type === "DISCONNECT"));
    const net = engineMsgs.filter((m) => m.type === "CONNECT").length - engineMsgs.filter((m) => m.type === "DISCONNECT").length;
    record("M5b: CONNECT - DISCONNECT on the engine Worker nets to 1 after 3 re-preview cycles", net === 1, `${JSON.stringify(engineMsgs.map((m) => m.type))} net=${net}`);
    // Restore exactly the state M5 left (previewing, empty editor) so M6's
    // own "editor === the proposal's SQL" / "no dialog" checks (which
    // compare against the empty editor Send started from) still hold.
    await page.fill("#sql", "");
    await page.waitForTimeout(150);
    await page.click("#ai-preview");
    await page.waitForTimeout(300);
  }
  // M6: Apply (no dialog expected: no readers, no class 3, not stale).
  {
    const { page } = M3;
    await page.click("#ai-apply");
    await page.waitForTimeout(400);
    const dlgOpen = await page.locator(".dp-dialog").count();
    record("M6: Apply with nothing to confirm shows no dialog", dlgOpen === 0);
    const sqlNow = await page.inputValue("#sql");
    record("M6: editor === the proposal's SQL", sqlNow.trim() === CLEAN.trim(), sqlNow.slice(0, 60));
    record("M6: preview bar hidden", await page.evaluate(() => document.getElementById("preview-bar")?.hidden !== false));
    record("M6: toast Undo button focused", await page.evaluate(() => document.activeElement?.className?.includes?.("side-toast-btn")));
  }
  // M7: Ctrl+Z / Ctrl+Shift+Z, then the toast's Undo.
  {
    const { page } = M3;
    await page.focus("#sql");
    await page.keyboard.press("Control+z");
    await page.waitForTimeout(150);
    record("M7: Ctrl+Z empties the editor", (await page.inputValue("#sql")) === "");
    await page.keyboard.press("Control+Shift+z");
    await page.waitForTimeout(150);
    record("M7: Ctrl+Shift+Z restores it", (await page.inputValue("#sql")).trim() === CLEAN.trim());
    // Re-apply through the toast for the undo path below (M8).
  }
  await M3.context.close();

  // M9: a hostile proposal, every round — refusals listed; the markdown
  // image/link to the recorder blocked; zero page-DB messages.
  {
    const { context, page, mock } = await mainCtx(() => sse({ content: [toolUse("toolu_h", "propose_dashboard", { sql: HOSTILE, summary: "Hostile.", panels: [{ stmt: 5, title: "Revenue by region", why: "the ask" }], open_issues: ["could not avoid the extra reads"] })], stop_reason: "tool_use" }));
    await gotoEdit(page, SETUP_AND_METRIC);
    await page.click("#assist-btn");
    await page.waitForSelector(".dp-dialog", { timeout: 15000 });
    await addKey(page);
    await page.fill("#ai-q", "x");
    await page.keyboard.press("Enter");
    await page.waitForSelector(".dp-dialog", { timeout: 15000 });
    await page.click('.dp-dialog button:has-text("Send")');
    await page.waitForSelector("#ai-card:not([hidden])", { timeout: 60000 });
    const cardTitle = await page.evaluate(() => document.getElementById("ai-card-title")?.textContent || "");
    record("M9: card shows open issues", /open issues/i.test(cardTitle), cardTitle);
    record("M9: recorder empty", recLog.length === 0, JSON.stringify(recLog));
    // The app's own ::MARKDOWN renderer has no image syntax at all (no
    // `<img>` is ever produced from `![alt](url)`) — a stronger property
    // than "blocked by CSP": there is no live resource to block in the
    // first place. The literal markdown text (including the image syntax)
    // must still only ever be text, never an actual <img src> to the
    // recorder.
    const liveImg = await page.locator(`.dash img[src*="127.0.0.1:${REC_PORT}"]`).count();
    record("M9: no live <img> to the recorder (the markdown renderer has no image syntax)", liveImg === 0);
    // the markdown link, if rendered, is blocked on click
    const linkCount = await page.locator(`.dash a[href*="127.0.0.1:${REC_PORT}"]`).count();
    if (linkCount) {
      const before = page.url();
      await page.click(`.dash a[href*="127.0.0.1:${REC_PORT}"]`).catch(() => {});
      await page.waitForTimeout(150);
      record("M9: clicking the markdown link shows the toast and does not navigate", (await page.locator("#side-toast").count()) > 0 && page.url() === before);
    }
    const n0 = await mark(page);
    await page.waitForTimeout(300);
    record("M9: zero page-DB messages since Send's preview start", pageMsgs(await since(page, n0)).length === 0);
    // H-2 (sec-review-272): a chart ::LINK with a *computed* (not literal)
    // URL must not open a window or navigate on click, during the preview
    // — before Apply, the gate never saw a URL literal to refuse. Checked
    // while the preview is still showing the hostile proposal's own panels
    // (the one place they're rendered at all).
    {
      let popup = null;
      const onPopup = (p) => (popup = p);
      context.on("page", onPopup);
      const beforeUrl = page.url();
      const nLink = await mark(page);
      await page.evaluate(() => document.querySelector(".panel-svg > svg .dp-hit")?.dispatchEvent(new MouseEvent("click", { bubbles: true })));
      await page.waitForTimeout(400);
      context.off("page", onPopup);
      if (popup) await popup.close().catch(() => {});
      record("M9: a chart ::LINK (computed URL) does not open a window or navigate during the preview", !popup && page.url() === beforeUrl);
      record("M9: recorder still empty after the chart ::LINK click", recLog.length === 0, JSON.stringify(recLog));
      record("M9: zero page-DB messages from the chart ::LINK click", pageMsgs(await since(page, nLink)).length === 0);
    }
    // H-2: a table ::LINK to a same-origin path — the click guard used to
    // let same-origin anchors through (only cross-origin was blocked),
    // so the browser's own default navigation moved the tab away,
    // dropping the preview and any unsaved work.
    {
      const beforeUrl = page.url();
      const nTable = await mark(page);
      await page.evaluate(() => document.querySelector(".dash .dp-link")?.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true })));
      await page.waitForTimeout(400);
      record("M9: a table ::LINK to a same-origin path does not navigate during the preview", page.url() === beforeUrl);
      record("M9: recorder still empty after the table ::LINK click", recLog.length === 0, JSON.stringify(recLog));
      record("M9: zero page-DB messages from the table ::LINK click", pageMsgs(await since(page, nTable)).length === 0);
    }
    await page.click("#ai-apply");
    await page.waitForSelector(".dp-dialog", { timeout: 10000 });
    const dlgText = await page.evaluate(() => document.querySelector(".dp-dialog")?.textContent || "");
    record("M9: Apply dialog lists the recorder URL, read_csv(), INSTALL, ATTACH", new RegExp(`127\\.0\\.0\\.1:${REC_PORT}`).test(dlgText) && /read_csv/.test(dlgText) && /INSTALL/.test(dlgText) && /ATTACH/.test(dlgText));
    const n1 = await mark(page);
    await page.click('.dp-dialog button:has-text("Apply, don\'t run")');
    await page.waitForTimeout(500);
    record("M9: editor === HOSTILE", (await page.inputValue("#sql")).trim() === HOSTILE.trim());
    record('M9: grid shows "Not run yet"', /Not run yet/.test(await page.evaluate(() => document.querySelector(".dash")?.textContent || "")));
    record("M9: zero page-DB messages from the Apply click", pageMsgs(await since(page, n1)).length === 0);
    record("M9: recorder still empty", recLog.length === 0);
    // H-1/L-2 (sec-review-272): "Apply, don't run" must actually hold — the
    // proposal's own ::RELOAD timer, the dark toggle, the refresh select
    // and Esc must none of them run the just-applied (unconfirmed) SQL on
    // the page's own DuckDB. Before the fix, the RELOAD timer alone did
    // this on its own within about 2s, with no further action at all.
    {
      const n2 = await mark(page);
      await page.waitForTimeout(3000); // >= 3 RELOAD ticks (the proposal's own SELECT 1::RELOAD) if the timer survived
      record("M9 H-1: 3s pass with the applied SQL held — zero page-DB messages (the RELOAD timer)", pageMsgs(await since(page, n2)).length === 0);
      const n3 = await mark(page);
      await page.click("#dark");
      await page.waitForTimeout(300);
      record("M9 H-1: the dark toggle does not run the held SQL", pageMsgs(await since(page, n3)).length === 0);
      const refreshDisabled = await page.evaluate(() => document.getElementById("refresh")?.disabled === true);
      record("M9 H-1: #refresh is disabled while the hold is up", refreshDisabled);
      const n4 = await mark(page);
      await page.keyboard.press("Escape");
      await page.waitForTimeout(300);
      record("M9 H-1: Esc does not run the held SQL", pageMsgs(await since(page, n4)).length === 0);
      record('M9 H-1: the grid still shows "Not run yet" (nothing silently ran)', /Not run yet/.test(await page.evaluate(() => document.querySelector(".dash")?.textContent || "")));
      record("M9 H-1: recorder still empty after 3s + dark + Esc", recLog.length === 0, JSON.stringify(recLog));
    }
    await context.close();
  }

  // M9b: H-1's hold lifts on an explicit Run — a *harmless* proposal (one
  // refused SET, no reader, no ATTACH/INSTALL/COPY) so actually running it
  // for real after "Apply, don't run" is safe, and proves the hold is not
  // permanent: it is meant to require confirmation, not to break Apply.
  {
    const { context, page, mock } = await mainCtx(() =>
      sse({ content: [toolUse("toolu_h9b", "propose_dashboard", { sql: HOLD_CLEAN, summary: "Hold test.", panels: [{ stmt: 3, title: "n", why: "y" }], open_issues: [] })], stop_reason: "tool_use" })
    );
    await gotoEdit(page, SETUP_AND_METRIC);
    await page.click("#assist-btn");
    await page.waitForSelector(".dp-dialog", { timeout: 15000 });
    await addKey(page);
    await page.fill("#ai-q", "x");
    await page.keyboard.press("Enter");
    await page.waitForSelector(".dp-dialog", { timeout: 15000 });
    await page.click('.dp-dialog button:has-text("Send")');
    await page.waitForSelector("#ai-card:not([hidden])", { timeout: 60000 });
    await page.click("#ai-apply");
    await page.waitForSelector(".dp-dialog", { timeout: 10000 });
    await page.click('.dp-dialog button:has-text("Apply, don\'t run")');
    await page.waitForTimeout(400);
    record('M9b: "Apply, don\'t run" shows "Not run yet"', /Not run yet/.test(await page.evaluate(() => document.querySelector(".dash")?.textContent || "")));
    const n5 = await mark(page);
    await page.click("#dark"); // a passive trigger must still be held
    await page.waitForTimeout(300);
    record("M9b: the dark toggle alone still does not run it", pageMsgs(await since(page, n5)).length === 0);
    const n6 = await mark(page);
    await page.click("#run"); // the explicit escape hatch
    await page.waitForTimeout(1500);
    record("M9b: an explicit Run click lifts the hold and actually runs it", pageMsgs(await since(page, n6)).length > 0);
    const refreshReenabled = await page.evaluate(() => document.getElementById("refresh")?.disabled === false);
    record("M9b: #refresh is re-enabled once the hold lifts", refreshReenabled);
    await context.close();
  }

  // M9c (sec-review-272-rereview H-1R): Discard and "Apply, don't run"
  // must never let a render that started under the (now-gone) preview
  // finish on the page database — a render the proposal's own ::RELOAD
  // timer queued while the slow range() count was still in flight. Three
  // trials each, matching the re-review's own probe.
  for (const mode of ["discard", "norun"]) {
    for (let trial = 0; trial < 3; trial++) {
      const { context, page } = await mainCtx(() =>
        sse({ content: [toolUse(`toolu_race_${mode}${trial}`, "propose_dashboard", { sql: RACE, summary: "Race.", panels: [{ stmt: 3, title: "n", why: "y" }], open_issues: [] })], stop_reason: "tool_use" })
      );
      await gotoEdit(page, SETUP_AND_METRIC);
      await page.click("#assist-btn");
      await page.waitForSelector(".dp-dialog", { timeout: 15000 });
      await addKey(page);
      await page.fill("#ai-q", "x");
      await page.keyboard.press("Enter");
      await page.waitForSelector(".dp-dialog", { timeout: 15000 });
      await page.click('.dp-dialog button:has-text("Send")');
      await page.waitForSelector("#ai-card:not([hidden])", { timeout: 60000 });
      // Let the slow range() count (and the ::RELOAD timer ticking on top
      // of it) get well under way before rejecting the proposal — this is
      // exactly the window H-1R's race needs.
      await page.waitForTimeout(2500);
      const n0 = await mark(page);
      if (mode === "discard") {
        await page.click("#ai-discard");
      } else {
        await page.click("#ai-apply");
        await page.waitForSelector(".dp-dialog", { timeout: 10000 });
        await page.click('.dp-dialog button:has-text("Apply, don\'t run")');
      }
      await page.waitForTimeout(2500); // the slow query's own worker round-trip
      const hits = await w0with(page, n0, /RACEMARK|race_attach|memory_limit\s*=\s*'3GB'/);
      record(`M9c ${mode} trial ${trial}: zero page-DB messages with the proposal's own text after exit`, hits.length === 0, JSON.stringify(hits.map((m) => String(m.sql).slice(0, 90))));
      await context.close();
    }
  }

  // M9d (sec-review-272-rereview M-2): "Check" must not run the held,
  // unconfirmed SQL on the page database either — same promise as Run.
  {
    const { context, page } = await mainCtx(() =>
      sse({ content: [toolUse("toolu_m2", "propose_dashboard", { sql: CHECK_HOLD, summary: "Check hold test.", panels: [{ stmt: 3, title: "m", why: "y" }], open_issues: [] })], stop_reason: "tool_use" })
    );
    await gotoEdit(page, SETUP_AND_METRIC);
    await page.click("#assist-btn");
    await page.waitForSelector(".dp-dialog", { timeout: 15000 });
    await addKey(page);
    await page.fill("#ai-q", "x");
    await page.keyboard.press("Enter");
    await page.waitForSelector(".dp-dialog", { timeout: 15000 });
    await page.click('.dp-dialog button:has-text("Send")');
    await page.waitForSelector("#ai-card:not([hidden])", { timeout: 60000 });
    await page.click("#ai-apply");
    await page.waitForSelector(".dp-dialog", { timeout: 10000 });
    await page.click('.dp-dialog button:has-text("Apply, don\'t run")');
    await page.waitForTimeout(400);
    const lintDisabled = await page.evaluate(() => document.getElementById("lint-run")?.disabled === true);
    record("M9d: #lint-run is disabled while the hold is up", lintDisabled);
    const n0 = await mark(page);
    await page.evaluate(() => {
      const d = document.querySelector("details:has(#lint)");
      if (d) d.open = true;
      document.getElementById("lint-run")?.click();
    });
    await page.waitForTimeout(800);
    const hits = await w0with(page, n0, /CHECKMARK|check_attach|memory_limit\s*=\s*'3GB'|read_csv/);
    record("M9d: zero page-DB messages from Check while held", hits.length === 0, JSON.stringify(hits.map((m) => String(m.sql).slice(0, 90))));
    record('M9d: grid still shows "Not run yet"', /Not run yet/.test(await page.evaluate(() => document.querySelector(".dash")?.textContent || "")));
    await context.close();
  }

  // M9e (sec-review-272-rereview L-3): a ::VIEW alternative's (model-
  // computed) label must not reach the address bar while previewing.
  {
    const { context, page } = await mainCtx(() =>
      sse({ content: [toolUse("toolu_l3", "propose_dashboard", { sql: VIEWP, summary: "View test.", panels: [{ stmt: 1, title: "rev", why: "y" }], open_issues: [] })], stop_reason: "tool_use" })
    );
    await gotoEdit(page, SETUP_AND_METRIC);
    await page.click("#assist-btn");
    await page.waitForSelector(".dp-dialog", { timeout: 15000 });
    await addKey(page);
    await page.fill("#ai-q", "x");
    await page.keyboard.press("Enter");
    await page.waitForSelector(".dp-dialog", { timeout: 15000 });
    await page.click('.dp-dialog button:has-text("Send")');
    await page.waitForSelector("#ai-card:not([hidden])", { timeout: 60000 });
    await page.waitForSelector(".dp-viewbar [role=tab]", { timeout: 15000 });
    const before = page.url();
    await page.evaluate(() => {
      const tabs = [...document.querySelectorAll(".dp-viewbar [role=tab]")];
      tabs[tabs.length - 1]?.click();
    });
    await page.waitForTimeout(300);
    record("M9e: a ::VIEW click during the preview does not change the URL", page.url() === before, page.url());
    await page.click("#ai-discard");
    await page.waitForTimeout(300);
    record("M9e: the URL is still unchanged after Discard", page.url() === before, page.url());
    await context.close();
  }

  // M9f (sec-review-272-rereview third pass, L-5): a stale `runPreviewToken`
  // left pointing at the dead preview after "Apply, don't run" broke the
  // page's own Explore query ("Cannot read properties of null (reading
  // 'run')", 3/3 trials) — it never reached any worker at all. Explore
  // must still work, on the page database, once the exit has finished
  // (no run in flight any more).
  {
    const { context, page } = await mainCtx(() =>
      sse({ content: [toolUse("toolu_l5", "propose_dashboard", { sql: HOLD_CLEAN, summary: "L-5 test.", panels: [{ stmt: 3, title: "n", why: "y" }], open_issues: [] })], stop_reason: "tool_use" })
    );
    const pageErrors = [];
    page.on("pageerror", (e) => pageErrors.push(String(e)));
    await gotoEdit(page, SETUP_AND_METRIC);
    await page.click("#assist-btn");
    await page.waitForSelector(".dp-dialog", { timeout: 15000 });
    await addKey(page);
    await page.fill("#ai-q", "x");
    await page.keyboard.press("Enter");
    await page.waitForSelector(".dp-dialog", { timeout: 15000 });
    await page.click('.dp-dialog button:has-text("Send")');
    await page.waitForSelector("#ai-card:not([hidden])", { timeout: 60000 });
    await page.click("#ai-apply");
    await page.waitForSelector(".dp-dialog", { timeout: 10000 });
    await page.click('.dp-dialog button:has-text("Apply, don\'t run")');
    await page.waitForTimeout(400);
    const n0 = await mark(page);
    await page.click("#side-explore");
    await page.waitForSelector("#xq-sql", { timeout: 15000 });
    await page.fill("#xq-sql", "SELECT 4242 AS explore_probe");
    await page.focus("#xq-sql");
    await page.keyboard.press(process.platform === "darwin" ? "Meta+Enter" : "Control+Enter");
    await page.waitForFunction(() => (document.getElementById("xq-info")?.textContent || "").length > 0, null, { timeout: 15000 }).catch(() => {});
    await page.waitForTimeout(300);
    record("M9f: no page error running an Explore query after \"Apply, don't run\"", pageErrors.length === 0, JSON.stringify(pageErrors));
    record("M9f: Explore's query reached the page worker (w0)", pageMsgs(await since(page, n0)).some((m) => /explore_probe/.test(m.sql || "")));
    record("M9f: no error shown in the Explore console", !/error/i.test(await page.evaluate(() => document.getElementById("xq-err")?.textContent || "")));
    record('M9f: #xq-info shows a row count, not stuck on "Running…"', !/^Running/.test(await page.evaluate(() => document.getElementById("xq-info")?.textContent || "")));
    await context.close();
  }

  // M9g (sec-review-272-rereview final check, H-1R2): an explicit run
  // (Ctrl/⌘+Enter) pressed ~80 ms into a preview exit (typing, or "Apply,
  // don't run") must not let the proposal's hidden-tab lazy jobs — idle-
  // prefetched while the preview sat there — run on the page database
  // once that lazy work finally executes. Adapted from the reviewer's own
  // probe8.mjs (`ltype`/`lnorun`). "Apply, don't run" also calls
  // host.editor.replace(p.sql) — AFTER previewExit() itself resolves —
  // so a trial whose 80 ms happens to outlast the whole exit (previewExit
  // already deferred-and-ran the pending explicit request, or finished
  // before Ctrl+Enter even fired) lands with the proposal now legitimately
  // IN the editor: pressing Ctrl+Enter then is an ordinary confirmed run,
  // not a window hit, exactly as the reviewer's own probe found ("landed
  // after the exit … a legitimate explicit run … not counted"). `ltype`
  // has no such ambiguity (typing never puts the proposal in the editor),
  // so it always asserts strictly.
  for (const mode of ["ltype", "lnorun"]) {
    for (let trial = 0; trial < 3; trial++) {
      const { context, page } = await mainCtx(() =>
        sse({ content: [toolUse(`toolu_lazy_${mode}${trial}`, "propose_dashboard", { sql: LAZY_PROP, summary: "Lazy.", panels: [{ stmt: 1, title: "m", why: "y" }], open_issues: [] })], stop_reason: "tool_use" })
      );
      await gotoEdit(page, SETUP_AND_METRIC);
      await page.click("#assist-btn");
      await page.waitForSelector(".dp-dialog", { timeout: 15000 });
      await addKey(page);
      await page.fill("#ai-q", "x");
      await page.keyboard.press("Enter");
      await page.waitForSelector(".dp-dialog", { timeout: 15000 });
      await page.click('.dp-dialog button:has-text("Send")');
      await page.waitForSelector("#ai-card:not([hidden])", { timeout: 60000 });
      await page.waitForTimeout(2500); // let the idle prefetch start the hidden tab's slow jobs
      const n0 = await mark(page);
      if (mode === "ltype") {
        page.evaluate(() => {
          const t = document.getElementById("sql");
          t.value += " ";
          t.dispatchEvent(new Event("input", { bubbles: true }));
        });
      } else {
        await page.click("#ai-apply");
        await page.waitForSelector(".dp-dialog", { timeout: 10000 });
        page.evaluate(() => [...document.querySelectorAll(".dp-dialog button")].find((b) => /Apply, don/.test(b.textContent))?.click());
      }
      await page.waitForTimeout(80); // the exit window H-1R2's race needs
      const st = await page.evaluate(() => {
        const previewing = document.body.classList.contains("previewing");
        document.getElementById("sql")?.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", ctrlKey: true, bubbles: true, cancelable: true }));
        return { previewing, editorIsProposal: /LAZYMARK/.test(document.getElementById("sql")?.value || "") };
      });
      await page.waitForTimeout(15000); // the hidden tab's slow panels get a chance to run
      const hits = await w0with(page, n0, LAZY_RE);
      if (mode === "lnorun" && st.editorIsProposal) {
        console.log(`M9g ${mode} trial ${trial}: not counted — the editor already held the applied proposal at Ctrl+Enter (a legitimate confirmed run), state=${JSON.stringify(st)}`);
      } else {
        record(`M9g ${mode} trial ${trial}: zero page-DB messages with the hidden tab's text after the exit`, hits.length === 0, `${JSON.stringify(hits.map((m) => String(m.sql).slice(0, 90)))} state=${JSON.stringify(st)}`);
      }
      await context.close();
    }
  }

  // M10: the key leak sweep (sk-ant- never appears anywhere).
  {
    const context = await newCtx();
    currentCfg = cfg(true);
    await context.grantPermissions(["clipboard-read", "clipboard-write"]);
    const consoleMsgs = [];
    const mock = await installMock(context, defaultHandler());
    const page = await context.newPage();
    page.on("console", (m) => consoleMsgs.push(m.text()));
    const allReq = [];
    page.on("request", (r) => allReq.push({ url: r.url(), body: r.postData() || "" }));
    await gotoEdit(page, SETUP_AND_METRIC);
    await page.click("#assist-btn");
    await page.waitForSelector(".dp-dialog", { timeout: 15000 });
    await addKey(page);
    await page.fill("#ai-q", "x");
    await page.keyboard.press("Enter");
    await page.waitForSelector(".dp-dialog", { timeout: 15000 });
    await page.click('.dp-dialog button:has-text("Send")');
    await page.waitForSelector("#ai-card:not([hidden])", { timeout: 30000 });
    await page.click("#ai-apply");
    await page.waitForTimeout(500);
    await page.fill("#dash-name", "key-leak-test");
    await page.click("#save");
    await page.waitForTimeout(300);

    const key = "sk-ant-e2e-0000000000000000000000000";
    const checks = [];
    // Share (view-only), then Shift+Share (editable).
    await page.click("#share");
    let clip = await page.evaluate(() => navigator.clipboard.readText());
    checks.push(["share link", clip]);
    await page.keyboard.down("Shift");
    await page.click("#share");
    await page.keyboard.up("Shift");
    clip = await page.evaluate(() => navigator.clipboard.readText());
    checks.push(["editable share link", clip]);
    // Download HTML.
    const [dl1] = await Promise.all([page.waitForEvent("download"), page.click("#dlhtml")]);
    const htmlPath = await dl1.path();
    checks.push(["downloaded html", htmlPath ? readFileSync(htmlPath, "utf8") : ""]);
    // Export all as .zip.
    await page.click("#side-my-more").catch(() => {});
    const exportBtn = page.locator('.side-menu button:has-text("Export all")');
    if (await exportBtn.count()) {
      const [dl2] = await Promise.all([page.waitForEvent("download"), exportBtn.click()]);
      const zipPath = await dl2.path();
      checks.push(["exported zip", zipPath ? readFileSync(zipPath, "latin1") : ""]);
    }
    checks.push(["localStorage", await page.evaluate(() => { const o = {}; for (let i = 0; i < localStorage.length; i++) { const k = localStorage.key(i); if (k !== "dp_assist_key") o[k] = localStorage.getItem(k); } return JSON.stringify(o); })]);
    checks.push(["page url", page.url()]);
    checks.push(["outerHTML", await page.evaluate(() => document.documentElement.outerHTML)]);
    checks.push(["console", consoleMsgs.join("\n")]);
    for (const r of allReq) {
      if (/api\.anthropic\.com/.test(r.url)) continue;
      checks.push(["request " + r.url, r.url + " " + r.body]);
    }
    const leaking = checks.filter(([, v]) => String(v).includes(key) || /sk-ant-/.test(String(v)));
    record("M10: sk-ant- appears nowhere (clipboard/export/storage/url/DOM/console/requests)", leaking.length === 0, JSON.stringify(leaking.map(([n]) => n)));
    // The key in the real Anthropic requests only, in x-api-key, never url/body.
    const anthReqs = sharedMock?.requests || mock.requests;
    let keyOk = true,
      hdrOk = true;
    for (const r of anthReqs) {
      if (JSON.stringify(r.body).includes(key)) keyOk = false;
      if (r.url.includes(key)) keyOk = false;
      if (r.headers["x-api-key"] !== key) hdrOk = false;
      if (r.headers["anthropic-dangerous-direct-browser-access"] !== "true") hdrOk = false;
      if (r.body.metadata) hdrOk = false;
    }
    record("M10: the key never travels in the request URL or body", keyOk);
    record("M10: x-api-key header set, no metadata", hdrOk);
    await context.close();
  }

  // M11: Forget key.
  {
    const context = await newCtx({ cfgObj: cfg(true) });
    const mock = await installMock(context, defaultHandler());
    const page = await context.newPage();
    await gotoEdit(page, SETUP_AND_METRIC);
    await page.click("#assist-btn");
    await page.waitForSelector(".dp-dialog", { timeout: 15000 });
    await addKey(page);
    await page.click("#ai-forget");
    await page.waitForTimeout(200);
    const storage = await page.evaluate(() => ({ local: localStorage.getItem("dp_assist_key"), session: sessionStorage.getItem("dp_assist_key") }));
    record("M11: key removed from both storages", !storage.local && !storage.session, JSON.stringify(storage));
    await page.reload();
    await settled(page);
    record("M11: no meta[data-anofox-assist] after reload", !(await page.evaluate(() => !!document.querySelector("meta[data-anofox-assist]"))));
    await context.close();
  }

  // M12: a 401 -> plain-words message, Change key button, no raw key text.
  {
    const context = await newCtx({ cfgObj: cfg(true) });
    const mock = await installMock(context, async () => ({ status: 401, json: { type: "error", error: { type: "authentication_error", message: "invalid x-api-key: sk-ant-should-not-leak" } } }));
    const page = await context.newPage();
    await gotoEdit(page, SETUP_AND_METRIC);
    await page.click("#assist-btn");
    await page.waitForSelector(".dp-dialog", { timeout: 15000 });
    await addKey(page);
    await page.fill("#ai-q", "x");
    await page.keyboard.press("Enter");
    await page.waitForSelector(".dp-dialog", { timeout: 15000 });
    await page.click('.dp-dialog button:has-text("Send")');
    await page.waitForFunction(() => /not accepted/i.test(document.getElementById("ai-log")?.textContent || ""), null, { timeout: 15000 });
    const logText = await page.evaluate(() => document.getElementById("ai-log")?.textContent || "");
    record("M12: 401 -> plain-words message, no sk-ant in the log", /not accepted/i.test(logText) && !/sk-ant-/.test(logText), logText.slice(-200));
    record("M12: a Change key button appears", (await page.locator("#ai-log button:has-text('Change key')").count()) > 0);
    await context.close();
  }

  // M13: the request delays; Cancel -> "Cancelled."; no card.
  {
    const context = await newCtx({ cfgObj: cfg(true) });
    const mock = await installMock(context, async () => ({ delayMs: 5000 }));
    const page = await context.newPage();
    await gotoEdit(page, SETUP_AND_METRIC);
    await page.click("#assist-btn");
    await page.waitForSelector(".dp-dialog", { timeout: 15000 });
    await addKey(page);
    await page.fill("#ai-q", "x");
    await page.keyboard.press("Enter");
    await page.waitForSelector(".dp-dialog", { timeout: 15000 });
    await page.click('.dp-dialog button:has-text("Send")');
    await page.waitForSelector("#ai-cancel:not([hidden])", { timeout: 15000 });
    await page.click("#ai-cancel");
    await page.waitForFunction(() => /Cancelled/.test(document.getElementById("ai-log")?.textContent || ""), null, { timeout: 15000 });
    record("M13: Cancelled, no card", await page.evaluate(() => document.getElementById("ai-card")?.hidden !== false));
    await context.close();
  }

  // M14: a reply-only hostile turn -> text only, no img/a descendants; a
  // follow-up carries history (D3).
  {
    let n = 0;
    const context = await newCtx({ cfgObj: cfg(true) });
    const mock = await installMock(context, async () => {
      n++;
      return sse({ content: [text(n === 1 ? REPLY_HOSTILE : "ok, noted")], stop_reason: "end_turn" });
    });
    let dialogFired = false;
    const page = await context.newPage();
    page.on("dialog", () => (dialogFired = true));
    await gotoEdit(page, SETUP_AND_METRIC);
    await page.click("#assist-btn");
    await page.waitForSelector(".dp-dialog", { timeout: 15000 });
    await addKey(page);
    await page.fill("#ai-q", "say something");
    await page.keyboard.press("Enter");
    await page.waitForSelector(".dp-dialog", { timeout: 15000 });
    await page.click('.dp-dialog button:has-text("Send")');
    await page.waitForFunction(() => document.querySelectorAll("#ai-log .ai-bot").length > 0, null, { timeout: 15000 });
    const botEntry = await page.locator("#ai-log .ai-bot").last();
    record("M14: the bot entry has 0 img/a descendants", (await botEntry.locator("img, a").count()) === 0);
    const txt = (await botEntry.textContent()) || "";
    record("M14: the literal strings are present as text", txt.includes("<img src=x onerror=alert(1)>") && txt.includes("[click](javascript:alert(1))"));
    record("M14: recorder empty, no alert() dialog", recLog.length === 0 && !dialogFired);
    const reqCountBefore = mock.requests.length;
    await page.fill("#ai-q", "thanks");
    await page.keyboard.press("Enter");
    // D3/M2-style: the consent dialog ("What will be sent") was never
    // dismissed with "Don't ask again" for the first Send, so it reappears
    // for this follow-up too — click it through, same as every other Send.
    await page.waitForSelector(".dp-dialog", { timeout: 15000 });
    await page.click('.dp-dialog button:has-text("Send")');
    for (let i = 0; i < 100 && mock.requests.length <= reqCountBefore; i++) await page.waitForTimeout(200);
    const lastReq = mock.requests[mock.requests.length - 1];
    record("M14: the follow-up carries history (>=3 messages)", (lastReq?.body?.messages || []).length >= 3, JSON.stringify((lastReq?.body?.messages || []).length));
    await context.close();
  }
}

// ---- csp (M15, C-2) ----
if (run("csp")) {
  async function cspTamper(label, seedKey) {
    const context = await newCtx({ cfgObj: cfg(true) });
    if (seedKey) {
      await context.addInitScript(() => {
        try {
          sessionStorage.setItem("dp_assist_key", "sk-ant-tamper-seed-0000000000000000");
        } catch (_) {}
      });
    }
    await context.addInitScript(() => {
      window.__metaTries = 0;
      const orig = HTMLHeadElement.prototype.appendChild;
      HTMLHeadElement.prototype.appendChild = function (node) {
        if (node?.tagName === "META" && node.dataset && node.getAttribute("http-equiv") === "Content-Security-Policy" && node.dataset.anofoxAssist === "1") {
          window.__metaTries++;
          return document.createElement("div").appendChild(node); // detached: never actually applied
        }
        return orig.call(this, node);
      };
    });
    const mock = await installMock(context, defaultHandler());
    const page = await context.newPage();
    await gotoEdit(page, SETUP_AND_METRIC);
    if (seedKey) {
      const stillThere = await page.evaluate(() => sessionStorage.getItem("dp_assist_key"));
      record(`M15 ${label}: the key is gone after boot's own failed probe`, !stillThere, stillThere || "");
    }
    await page.click("#assist-btn");
    await page.waitForTimeout(1200);
    const status = await page.evaluate(() => document.getElementById("status")?.textContent || "");
    record(`M15 ${label}: status says not available in this browser`, /not available in this browser/i.test(status), status);
    await page.waitForTimeout(200);
    const btnHidden = await page.evaluate(() => document.getElementById("assist-btn")?.hidden === true);
    record(`M15 ${label}: #assist-btn becomes hidden`, btnHidden);
    const metaTries = await page.evaluate(() => window.__metaTries);
    record(`M15 ${label}: the probe is attempted exactly once (the shared memoised promise)`, metaTries === 1, String(metaTries));
    record(`M15 ${label}: no SDK request, no assist-ui.js request`, !staticLog.some((p) => /vendor\/anthropic-sdk|assist-ui\.js/.test(p)));
    record(`M15 ${label}: no api.anthropic.com request`, mock.requests.length === 0);
    await context.close();
  }
  staticLog.length = 0;
  await cspTamper("a (seeded key)", true);
  staticLog.length = 0;
  await cspTamper("b (no key)", false);
}

// ---- phone (P), 390x844 ----
if (run("phone")) {
  for (const scheme of ["light", "dark"]) {
    const context = await newCtx({ viewport: { width: 390, height: 844 }, colorScheme: scheme, cfgObj: cfg(true) });
    const mock = await installMock(context, defaultHandler());
    const page = await context.newPage();
    await gotoEdit(page, SETUP_AND_METRIC);
    await page.click("#assist-btn");
    await page.waitForTimeout(300);
    record(`P1 ${scheme}: #assist-pane role=dialog aria-modal`, await page.evaluate(() => { const p = document.getElementById("assist-pane"); return p.getAttribute("role") === "dialog" && p.getAttribute("aria-modal") === "true"; }));
    record(`P1 ${scheme}: header/.dash are inert`, await page.evaluate(() => document.querySelector("header").inert === true && document.querySelector(".dash").inert === true));
    const box = await page.evaluate(() => { const r = document.getElementById("ai-form").getBoundingClientRect(); return { bottom: r.bottom, width: document.getElementById("assist-pane").getBoundingClientRect().width }; });
    record(`P1 ${scheme}: #ai-form near the bottom, pane 390 wide`, box.bottom >= 844 - 140 && box.bottom <= 844 && Math.round(box.width) === 390, JSON.stringify(box));
    if (SHOTS) await page.screenshot({ path: path.join(SHOTS, `phone-${scheme}-sheet.png`) });

    await addKey(page);
    await page.fill("#ai-q", "Which regions sell the most?");
    await page.keyboard.press("Enter");
    await page.waitForSelector(".dp-dialog", { timeout: 15000 });
    if (SHOTS) await page.screenshot({ path: path.join(SHOTS, `phone-${scheme}-sent-dialog.png`) });
    await page.click('.dp-dialog button:has-text("Send")');
    await page.waitForFunction(() => document.getElementById("preview-bar")?.hidden === false, null, { timeout: 30000 });
    record(`P2 ${scheme}: the sheet is hidden, preview-bar fixed+visible, focus on #preview-title`, await page.evaluate(() => {
      const pane = document.getElementById("assist-pane");
      const bar = document.getElementById("preview-bar");
      return pane.hidden === true && bar.hidden === false && getComputedStyle(bar).position === "fixed" && document.activeElement?.id === "preview-title";
    }));
    if (SHOTS) await page.screenshot({ path: path.join(SHOTS, `phone-${scheme}-preview.png`) });

    await page.click("#assist-btn");
    await page.click("#ai-diff");
    await page.waitForSelector(".dp-dialog", { timeout: 10000 });
    const dw = await page.evaluate(() => document.querySelector(".dp-dialog .ai-dlg-wide, .dp-dialog .ai-dlg-full")?.getBoundingClientRect().width);
    record(`P3 ${scheme}: the diff dialog is 390 wide on phone`, Math.round(dw || 0) === 390, String(dw));
    await page.keyboard.press("Escape");
    await page.click("#ai-apply");
    await page.waitForTimeout(500);
    record(`P3 ${scheme}: Apply -> toast with Undo`, (await page.locator("#side-toast").count()) > 0);
    await page.click("#side-toast .side-toast-btn");
    await page.waitForTimeout(200);
    record(`P3 ${scheme}: Undo restores the editor`, (await page.inputValue("#sql")).trim() === SETUP_AND_METRIC.trim());

    if (AXE && existsSync(AXE)) {
      // axe-core as an external, same-origin <script src>: once the key
      // dialog has been used, the assistant's own tab-wide CSP is active
      // (script-src has no 'unsafe-inline') — addScriptTag({path}) injects
      // an *inline* script and is blocked by exactly that policy; {url}
      // (served by this script's own static server, so it's same-origin)
      // is not.
      await page.addScriptTag({ url: `${BASE}__axe__/axe.min.js` });
      const res = await page.evaluate(async () => window.axe.run(['#assist-pane', '.dp-dialog', '#preview-bar'].filter((s) => document.querySelector(s)), { runOnly: { type: "tag", values: ["wcag2a", "wcag2aa"] } }));
      for (const v of res.violations) console.log(`axe ${scheme} ${v.impact}: ${v.id} (${v.nodes.length}) ${v.help} -- ${JSON.stringify(v.nodes.map(n => n.target))}`);
      const bad = res.violations.filter((v) => v.impact === "serious" || v.impact === "critical");
      record(`P4 ${scheme}: axe, no serious/critical violation`, bad.length === 0, bad.map((v) => v.id).join(","));
    }
    await context.close();
  }
}

// ---- a11y (desktop, light+dark), when run via --only a11y or a11y-e2e.mjs's --assist hook ----
if (run("a11y") && AXE && existsSync(AXE)) {
  for (const scheme of ["light", "dark"]) {
    const context = await newCtx({ colorScheme: scheme, cfgObj: cfg(true) });
    const mock = await installMock(context, defaultHandler());
    const page = await context.newPage();
    await gotoEdit(page, SETUP_AND_METRIC);
    await page.click("#assist-btn");
    await page.waitForTimeout(300);
    await addKey(page);
    await page.fill("#ai-q", "Which regions sell the most?");
    await page.keyboard.press("Enter");
    await page.waitForSelector(".dp-dialog", { timeout: 15000 });
    await page.addScriptTag({ url: `${BASE}__axe__/axe.min.js` }); // same-origin <script src>: the CSP (active once a key exists) has no 'unsafe-inline'
    const res = await page.evaluate(async () => window.axe.run(['#assist-pane', '.dp-dialog', '#preview-bar'].filter((s) => document.querySelector(s)), { runOnly: { type: "tag", values: ["wcag2a", "wcag2aa"] } }));
    for (const v of res.violations) console.log(`axe desktop ${scheme} ${v.impact}: ${v.id} (${v.nodes.length}) ${v.help} -- ${JSON.stringify(v.nodes.map(n => n.target))}`);
    const bad = res.violations.filter((v) => v.impact === "serious" || v.impact === "critical");
    record(`a11y desktop ${scheme}: no serious/critical violation`, bad.length === 0, bad.map((v) => v.id).join(","));
    // Known harness quirk, not a product bug: at this wide dialog size
    // (.ai-dlg-wide, 920px) headless Chromium's screenshot sometimes
    // captures a stale compositing layer for the backdrop-filter veil,
    // showing the card's *light* background even once body.dark is set and
    // axe's own color-contrast check (same computed styles) has already
    // passed above — verified directly: getComputedStyle(card).backgroundColor
    // is the correct dark --ui-card value at this point. The phone-sized
    // dialog (.ai-dlg-full) does not show it. Paste the PR's screenshots
    // with this noted rather than block on a screenshot-capture artifact.
    if (SHOTS) await page.screenshot({ path: path.join(SHOTS, `desktop-${scheme}-sent-dialog.png`) });
    await context.close();
  }
}

staticServer.close();
recServer.close();
await browser.close();

console.log("\n| case | ok | note |");
console.log("|---|---|---|");
for (const r of results) console.log(`| ${r.name} | ${r.ok ? "pass" : "FAIL"} | ${r.note ? String(r.note).slice(0, 200).replace(/\|/g, "/") : ""} |`);
console.log(`\nstatic request log (N cases): ${JSON.stringify(staticLog.slice(0, 50))}`);
console.log(`recorder log (must be empty outside M6/P2 apply-and-run): ${JSON.stringify(recLog)}`);
process.exit(process.exitCode || 0);
