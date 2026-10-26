// #428 on a real locked server (anofox_serve_dashboards): an input's
// OPTIONS query that reads another input's variable sees that other
// input's CURRENT value. Worse than the browser's own version of this bug
// (one run stale, see scripts/input-options-order-e2e.mjs): a locked
// server opens a fresh DuckDB connection per request and only ever applies
// the variables the client's request sends, so a dropdown's OPTIONS
// request made before the client knows the search box's value never sees
// it, full stop — not just the previous value, every single time,
// including a hard reload with `?v.q=...` already in the address bar.
// Needs a native extension build with the real UI embedded (not the
// SKIP_WASM=1 one the `rust` job builds) — run in CI's `web-e2e` job
// (#590), which already downloads web/pkg and builds that extension from
// it. scripts/varscope-e2e.mjs still isn't run in CI (it's a before/after
// measurement, see its own header).
//
//   node scripts/input-options-order-served-e2e.mjs \
//       --ext path/to/anofox_visualization.duckdb_extension [--port 8954] [--duckdb duckdb]
import { createRequire } from "module";
import { spawn } from "child_process";
import { mkdtempSync, mkdirSync, writeFileSync, rmSync, existsSync } from "fs";
import { tmpdir } from "os";
import path from "path";

const require = createRequire(import.meta.url);
const { chromium } = require("playwright");
const arg = (k, d) => {
  const i = process.argv.indexOf(k);
  return i > 0 ? process.argv[i + 1] : d;
};
const ext = arg("--ext", "");
const port = Number(arg("--port", "8954"));
const duckdb = arg("--duckdb", "duckdb");
if (!ext || !existsSync(ext)) {
  console.error("usage: input-options-order-served-e2e.mjs --ext <anofox_visualization.duckdb_extension> (build it with duckext/scripts/build-native.sh)");
  process.exit(2);
}
let bad = 0;
const check = (ok, m) => (ok ? console.log("ok   " + m) : (bad++, console.error("FAIL " + m)));
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const sqlStr = (s) => `'${s.replace(/'/g, "''")}'`;
const env = { ...process.env, DATAZOO_DISABLE_TELEMETRY: "1" };

// 1. The dashboard: the issue's own repro — a dropdown whose OPTIONS query
// filters by a search box's variable, dropdown declared BEFORE the box.
const dir = mkdtempSync(path.join(tmpdir(), "dp-input-options-order-"));
const dashDir = path.join(dir, "dash");
mkdirSync(dashDir, { recursive: true });
writeFileSync(
  path.join(dashDir, "repro.sql"),
  `SELECT extension AS extension ::DROPDOWN
FROM (VALUES ('alpha'), ('anofox'), ('bravo'), ('charlie')) t(extension)
WHERE extension ILIKE '%' || COALESCE(getvariable('q'), '') || '%'
ORDER BY 1;

SELECT '' AS q ::TEXT, 'Search'::LABEL;
SELECT 'Picked: ' || getvariable('extension') AS t ::TITLE;
`
);

// 2. Start the locked server (a DuckDB process we own; stopped by PID).
async function serve() {
  const p = spawn(duckdb, ["-unsigned", "-batch"], { stdio: ["pipe", "pipe", "pipe"], env });
  let log = "";
  p.stdout.on("data", (d) => (log += d));
  p.stderr.on("data", (d) => (log += d));
  p.stdin.write(`LOAD ${sqlStr(ext)};\nSELECT anofox_serve_dashboards(${sqlStr(dashDir)}, ${port});\n`);
  for (let i = 0; i < 100; i++) {
    try {
      if ((await fetch(`http://127.0.0.1:${port}/healthz`)).ok) return { proc: p, log: () => log };
    } catch (_) {}
    if (p.exitCode != null) break;
    await sleep(100);
  }
  p.kill();
  throw new Error(`the server did not start:\n${log}`);
}
const stop = (s) => {
  s.proc.stdin.end();
  s.proc.kill(); // our own child (pid known), never by name
};

let srv = null;
let browser = null;
try {
  srv = await serve();
  browser = await chromium.launch();
  const page = await browser.newPage({ viewport: { width: 900, height: 700 }, reducedMotion: "reduce" });
  const errs = [];
  page.on("pageerror", (e) => errs.push(String(e)));

  const settled = () =>
    page.waitForFunction(
      () => !document.body.classList.contains("loading") && document.querySelectorAll("select[aria-label=extension] option").length > 0,
      null,
      { timeout: 15000 }
    );
  const options = () => page.locator("select[aria-label=extension] option").allTextContents();
  const dequal = (a, b) => JSON.stringify(a) === JSON.stringify(b);

  // 3. A hard reload with `?v.q=ano` already in the address bar.
  await page.goto(`http://127.0.0.1:${port}/d/repro?v.q=ano`);
  await settled();
  check(dequal(await options(), ["anofox"]), "served: first load with ?v.q=ano narrows to just anofox");

  // 4. Typing a different query.
  await page.fill("label.control:has-text('Search') input", "bravo");
  await page.keyboard.press("Enter");
  await settled();
  check(dequal(await options(), ["bravo"]), "served: typing bravo narrows to just bravo");

  // 5. The current pick falls out of a further narrowing: falls back to
  // the first remaining option.
  await page.fill("label.control:has-text('Search') input", "an");
  await page.keyboard.press("Enter");
  await settled();
  check(dequal(await options(), ["anofox"]), "served: narrowing further to 'an' leaves just anofox");
  const panel = await page.locator(".grid").first().textContent();
  check(panel.includes("Picked: anofox"), "served: the fallen-out pick falls back to the first remaining option");

  check(errs.length === 0, `served: no page errors${errs.length ? ": " + errs.join("; ") : ""}`);
} finally {
  if (srv) stop(srv);
  if (browser) await browser.close();
  rmSync(dir, { recursive: true, force: true });
}
console.log(`input-options-order-served-e2e: ${bad} failed`);
if (bad) process.exit(1);
