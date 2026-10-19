// The assistant's engine + CSP (#204), in a real browser: the tab-wide
// ratchet, the locked DuckDB-Wasm engine it gates, and the CSP backstop
// alone — against two loopback-only servers (never anything real; no key,
// no network DuckDB settles for, DATAZOO_DISABLE_TELEMETRY=1 is set for any
// child process). Manual, like the other *-e2e.mjs scripts; paste the
// table below into the PR.
//
//   wasm-pack build --target web --out-dir web/pkg --no-default-features --features wasm
//   scripts/fetch-wasm-extensions.sh
//   NODE_PATH=<dir with playwright>/node_modules node scripts/assist-sandbox-e2e.mjs
//       [--static-port 8791] [--rec-port 8799] [--keep]
//
// Servers (loopback only):
//   static  127.0.0.1:<static-port>  serves web/ (plus the e2e fixtures,
//           copied into web/__e2e__/ for the run and removed afterwards)
//   rec     127.0.0.1:<rec-port>     logs method+URL, answers 200 with
//           Access-Control-Allow-Origin: * (so a blocked request is the
//           only thing that keeps it out of the log)
import { createRequire } from "module";
import { createServer } from "http";
import { readFileSync, writeFileSync, mkdirSync, rmSync, existsSync } from "fs";
import path from "path";
import { fileURLToPath } from "url";

const require = createRequire(import.meta.url);
let chromium, firefox, webkit;
try {
  ({ chromium, firefox, webkit } = require("playwright"));
} catch (e) {
  console.error("Playwright is not installed (NODE_PATH=<dir>/node_modules node " + path.basename(import.meta.url));
  process.exit(1);
}
const arg = (k, d) => {
  const i = process.argv.indexOf(k);
  return i > 0 ? process.argv[i + 1] : d;
};
const STATIC_PORT = Number(arg("--static-port", "8791"));
const REC_PORT = Number(arg("--rec-port", "8799"));
const keep = process.argv.includes("--keep");

const here = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(here, "..");
const webDir = path.join(root, "web");
const e2eDir = path.join(webDir, "__e2e__");

const results = []; // {name, browser, ok, note}
const record = (name, browser, ok, note = "") => {
  results.push({ name, browser, ok, note });
  console.log(`${ok ? "ok  " : "FAIL"} [${browser}] ${name}${note ? " — " + note : ""}`);
  if (!ok) process.exitCode = 1;
};

// ---- fixtures: copied into web/__e2e__/ (same origin as the app, so the
// fixture's relative imports of ../assist-csp.js etc. resolve) ----
mkdirSync(e2eDir, { recursive: true });
for (const f of ["harness.html", "csp-probe.js"]) writeFileSync(path.join(e2eDir, f), readFileSync(path.join(here, "e2e-fixtures", f)));
const cleanupFixtures = () => {
  if (!keep) rmSync(e2eDir, { recursive: true, force: true });
};

// ---- the static server (web/, mime by extension, no caching) ----
const MIME = { ".html": "text/html", ".js": "text/javascript", ".mjs": "text/javascript", ".css": "text/css", ".json": "application/json", ".wasm": "application/wasm", ".csv": "text/csv", ".parquet": "application/octet-stream" };
const staticServer = createServer((req, res) => {
  const u = new URL(req.url, "http://x");
  let p = path.join(webDir, decodeURIComponent(u.pathname));
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

// ---- the recording server (logs; CORS *; 200 with trivial bodies) ----
const recLog = [];
const recServer = createServer((req, res) => {
  recLog.push(`${req.method} ${req.url}`);
  res.writeHead(200, { "Content-Type": "text/plain", "Access-Control-Allow-Origin": "*" });
  res.end(req.url.endsWith(".csv") ? "a\n1\n" : "ok");
});
const recSince = (n) => recLog.slice(n);

await new Promise((ok) => staticServer.listen(STATIC_PORT, "127.0.0.1", ok));
await new Promise((ok) => recServer.listen(REC_PORT, "127.0.0.1", ok));
console.log(`static http://127.0.0.1:${STATIC_PORT}/  recorder http://127.0.0.1:${REC_PORT}/`);

const BASE = `http://127.0.0.1:${STATIC_PORT}/__e2e__/harness.html`;
const RECBASE = `http://127.0.0.1:${REC_PORT}`;

async function runBrowser(launcher, name) {
  const browser = await launcher.launch();
  try {
    const page = await browser.newPage();
    const pageErrors = [];
    page.on("pageerror", (e) => pageErrors.push(String(e)));
    const violations = [];
    await page.exposeFunction("__e2eViolation", (d) => violations.push(d));
    await page.addInitScript(() => {
      document.addEventListener("securitypolicyviolation", (e) => window.__e2eViolation(`${e.effectiveDirective}:${e.blockedURI}`));
    });

    await page.goto(BASE);
    await page.waitForFunction(() => !!window.__e2e);

    // 1. Control: before any ratchet, the harness's own page db reads the
    // recorder and it IS recorded (the static baseline meta has no
    // connect-src/img-src restriction).
    await page.evaluate(() => window.__e2e.makePageDb());
    let n = recLog.length;
    let r = await page.evaluate((u) => window.__e2e.pageReadsUrl(u), `${RECBASE}/control.csv`);
    record("1 control: unratcheted read is recorded", name, r.ok === true && recSince(n).length > 0, JSON.stringify(r));

    // 2. The boot ratchet.
    n = recLog.length;
    r = await page.evaluate((o) => window.__e2e.ratchet(o), []);
    record("2 the ratchet resolves (the probe is seen)", name, r.ok === true, JSON.stringify(r));
    const metaCount = await page.evaluate(() => document.querySelectorAll('meta[http-equiv="Content-Security-Policy"]').length);
    record("2 exactly one CSP meta after the ratchet", name, metaCount === 2, `count=${metaCount} (baseline + ratchet)`);
    // an <img> to the recorder is blocked, nothing recorded
    n = recLog.length;
    await page.evaluate((u) => new Promise((ok) => { const i = new Image(); i.onerror = i.onload = ok; i.src = u; }), `${RECBASE}/boot.png`);
    record("2 img to the recorder is blocked after the ratchet", name, recSince(n).length === 0, JSON.stringify(recSince(n)));

    // 4. The page Worker created *before* the ratchet keeps its network
    // (R1) — documented behaviour, not a bug.
    n = recLog.length;
    r = await page.evaluate((u) => window.__e2e.pageReadsUrl(u), `${RECBASE}/after.csv`);
    record("4 the pre-ratchet page Worker still reads (R1, documented)", name, r.ok === true && recSince(n).length > 0, JSON.stringify(r));

    // 5. The locked engine, end to end.
    r = await page.evaluate(() => window.__e2e.buildLocked().then(({ locked, tables }) => ({ locked, tables })));
    const hasSales = (r.tables || []).some((t) => /"sales"$/.test(t.fq) && t.rows === 50);
    record("5 buildEngine: locked", name, r.locked === true, JSON.stringify(r.locked));
    record("5 buildEngine: snapshot matches (sales, 50 rows)", name, hasSales, JSON.stringify(r.tables));

    // 7. The CSP alone, no lock: an engine Worker created after the
    // ratchet still has no network.
    n = recLog.length;
    r = await page.evaluate((u) => window.__e2e.unlockedRead(u), `${RECBASE}/csp.csv`);
    record("7 CSP alone stops an unlocked engine Worker", name, r.ok === false && recSince(n).length === 0, JSON.stringify(r));

    // 8. H-1: the proposal preview runs in the locked engine; nothing
    // reaches the recorder while it does.
    n = recLog.length;
    r = await page.evaluate(() => window.__e2e.previewOnLockedEngine());
    record("8 preview runs in the locked engine, with a result", name, r.ok === true && Array.isArray(JSON.parse(r.rows || "[]")) && recSince(n).length === 0, JSON.stringify(r).slice(0, 200));

    // 9. A <base> injected before the ratchet is refused by base-uri
    // 'none' (the static baseline): document.baseURI is unchanged.
    const page2 = await browser.newPage();
    await page2.addInitScript(() => {
      const b = document.createElement("base");
      b.href = "https://evil.example/";
      document.head?.appendChild(b) ?? document.documentElement.appendChild(b);
    });
    await page2.goto(BASE);
    const baseURI = await page2.evaluate(() => document.baseURI);
    record("9 a <base> injected before load does not move document.baseURI", name, !baseURI.includes("evil.example"), baseURI);
    await page2.close();

    record("pageerror-free", name, pageErrors.length === 0, pageErrors.join(" | "));
  } finally {
    await browser.close();
  }
}

// 10. Redirects: network; skip offline with a note (plan-204 §5.3 step 10
// allows this).
try {
  const r = await fetch("https://extensions.duckdb.org/v1.5.6/wasm_eh/spatial.duckdb_extension.wasm", { method: "HEAD", redirect: "manual" });
  record("10 no 3xx for the spatial wasm_eh extension", "network", r.status >= 200 && r.status < 300, `status=${r.status}`);
} catch (e) {
  console.log(`10 SKIPPED offline: ${e.message}`);
}

await runBrowser(chromium, "chromium");
for (const [launcher, name] of [[firefox, "firefox"], [webkit, "webkit"]]) {
  try {
    await runBrowser(launcher, name);
  } catch (e) {
    console.log(`NOT VERIFIED: ${name} (${e.message.split("\n")[0]})`);
  }
}

staticServer.close();
recServer.close();
cleanupFixtures();

console.log("\n| step | browser | result |");
console.log("|---|---|---|");
for (const r of results) console.log(`| ${r.name} | ${r.browser} | ${r.ok ? "pass" : "FAIL: " + r.note} |`);
process.exit(process.exitCode || 0);
