// #258 (a follow-up to #200): scratchRunner's timeout against a real,
// uncooperative query in the real DuckDB-Wasm Worker — no mocked
// connection (tests/web/lint_test.mjs's mock proves the cleanup *logic*
// deterministically; this proves DuckDB-Wasm actually honours the cancel).
// A late query must really be interrupted — the call settles near its
// timeout, not after the query would finish on its own — and the scratch
// session must be usable right after: a fresh scratchRunner call against
// the same page database must answer promptly, not queued behind whatever
// was left of the cancelled one (web/lint.js's scratchRunner, #258).
//
//   wasm-pack build --target web --out-dir web/pkg --profile wasm --no-default-features --features wasm
//   python3 serve_nocache.py 8947 &          # serves web/
//   NODE_PATH=<dir with playwright>/node_modules node scripts/scratch-cancel-e2e.mjs \
//       [--url http://127.0.0.1:8947/]
//
// Fixture: web/__e2e__/{harness.html,csp-probe.js}, the same same-origin
// pair scripts/assist-sandbox-e2e.mjs uses (so csp-probe.js's relative
// import of ../lint.js resolves) — copied next to the already-served
// web/ for the run and removed afterwards.
import { createRequire } from "module";
import { mkdirSync, readFileSync, rmSync, writeFileSync } from "fs";
import path from "path";
import { fileURLToPath } from "url";

const require = createRequire(import.meta.url);
let chromium;
try {
  ({ chromium } = require("playwright"));
} catch (_) {
  console.error("Playwright is not installed (NODE_PATH=<dir with playwright>/node_modules node " + path.basename(import.meta.url));
  process.exit(1);
}
const arg = (k, d) => {
  const i = process.argv.indexOf(k);
  return i > 0 ? process.argv[i + 1] : d;
};
const base = arg("--url", "http://127.0.0.1:8947/");
const fail = (m) => {
  console.error("FAIL: " + m);
  process.exitCode = 1;
};
const check = (ok, m) => (ok ? console.log("ok   " + m) : fail(m));

const here = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(here, "..");
const e2eDir = path.join(root, "web", "__e2e__");
mkdirSync(e2eDir, { recursive: true });
for (const f of ["harness.html", "csp-probe.js"]) writeFileSync(path.join(e2eDir, f), readFileSync(path.join(here, "e2e-fixtures", f)));

try {
  const browser = await chromium.launch();
  try {
    const page = await browser.newPage();
    const pageErrors = [];
    page.on("pageerror", (e) => pageErrors.push(String(e)));

    await page.goto(new URL("__e2e__/harness.html", base).toString());
    await page.waitForFunction(() => !!window.__e2e, null, { timeout: 60000 });
    await page.evaluate(() => window.__e2e.makePageDb());

    const r = await page.evaluate(() => window.__e2e.scratchCancel());
    check(/took longer.*cancelled/.test(r.rejected || ""), `the late query is rejected by a real cancel (${r.rejected})`);
    check(r.cancelMs < 5000, `the call settled near the timeout (${Math.round(r.cancelMs)} ms), not after a ~2×10^11-row cross join would finish on its own`);
    check(JSON.stringify(r.after) === JSON.stringify([{ ok: 1 }]), `the page database answers a fresh query right after (${JSON.stringify(r.after)})`);
    check(r.afterMs < 3000, `…promptly: not queued behind the cancelled query (${Math.round(r.afterMs)} ms)`);
    check(!pageErrors.length, `no page errors${pageErrors.length ? ": " + pageErrors.join(" | ") : ""}`);
  } finally {
    await browser.close();
  }
} finally {
  rmSync(e2eDir, { recursive: true, force: true });
}
if (!process.exitCode) console.log("ok   scratch-cancel-e2e: a late query is really cancelled in the Worker, and the session works again right after");
