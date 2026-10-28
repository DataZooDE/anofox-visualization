// ::FORM (#683) on a real locked server (anofox_serve_dashboards): the form's
// rows come from the server, its value travels as ONE string variable
// (a JSON object text, an ordinary typed literal — no new value type on the
// wire), and the panels reading it with getvariable('lt')->>'mean' follow
// an edit. A ?v.lt= seed restores it on a hard reload; a seed that isn't a
// JSON object, or carries quotes and SQL, is just text: the form falls back
// to its rows' values and nothing errors.
// Needs a native extension build with the real UI embedded (CI's `web-e2e`
// job builds it, #590).
//
//   node scripts/form-served-e2e.mjs \
//       --ext path/to/anofox_visualization.duckdb_extension [--port 8955] [--duckdb duckdb]
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
const port = Number(arg("--port", "8955"));
const duckdb = arg("--duckdb", "duckdb");
if (!ext || !existsSync(ext)) {
  console.error("usage: form-served-e2e.mjs --ext <anofox_visualization.duckdb_extension> (build it with duckext/scripts/build-native.sh)");
  process.exit(2);
}
let bad = 0;
const check = (ok, m) => (ok ? console.log("ok   " + m) : (bad++, console.error("FAIL " + m)));
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const sqlStr = (s) => `'${s.replace(/'/g, "''")}'`;
const env = { ...process.env, DATAZOO_DISABLE_TELEMETRY: "1" };

// 1. The dashboard: a form and two panels reading its variable.
const dir = mkdtempSync(path.join(tmpdir(), "dp-form-served-"));
const dashDir = path.join(dir, "dash");
mkdirSync(dashDir, { recursive: true });
writeFileSync(
  path.join(dashDir, "form.sql"),
  `SELECT field AS lt ::FORM, type, value, options, min, max, unit FROM (VALUES
  (1, 'model', 'dropdown', 'normal', ['fixed', 'normal'], NULL, NULL, NULL),
  (2, 'mean', 'number', '12', NULL, 0, 90, 'days')
) v(ord, field, type, value, options, min, max, unit) ORDER BY ord;

SELECT (getvariable('lt')->>'mean')::DOUBLE AS m ::METRIC, 'Mean lead time' ::LABEL;
SELECT 'Model: ' || coalesce(getvariable('lt')->>'model', '-') AS t ::TITLE;
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
  const settled = async () => {
    await sleep(300);
    await page.waitForFunction(
      () => !document.body.classList.contains("loading") && !!document.querySelector(".dp-form") && !!document.querySelector(".metric-value"),
      null,
      { timeout: 15000 }
    );
  };
  const kpi = () => page.locator(".metric-value").first().textContent();
  const grid = () => page.locator(".grid").first().textContent();

  await page.goto(`http://127.0.0.1:${port}/d/form`);
  await settled();
  check((await kpi()) === "12", "served: the form's row value reaches the panels");
  check((await grid()).includes("Model: normal"), "served: a string field reads with ->>");

  await page.fill('.dp-form-field[data-field="mean"] input', "25");
  await page.keyboard.press("Enter");
  await settled();
  await page.waitForFunction(() => document.querySelector(".metric-value")?.textContent === "25", null, { timeout: 15000 }).catch(() => {});
  check((await kpi()) === "25", "served: an edit travels to the server and back");

  await page.goto(`http://127.0.0.1:${port}/d/form?v.lt=${encodeURIComponent(JSON.stringify({ model: "fixed", mean: 40 }))}`);
  await settled();
  check((await kpi()) === "40" && (await grid()).includes("Model: fixed"), "served: a ?v.lt= seed restores the form on a hard reload");

  await page.goto(`http://127.0.0.1:${port}/d/form?v.lt=${encodeURIComponent("x'); DROP TABLE t; --")}`);
  await settled();
  check((await kpi()) === "12", "served: a seed that isn't a JSON object is ignored — the rows' values");

  check(errs.length === 0, `served: no page errors${errs.length ? ": " + errs.join("; ") : ""}`);
} finally {
  if (srv) stop(srv);
  if (browser) await browser.close();
  rmSync(dir, { recursive: true, force: true });
}
console.log(`form-served-e2e: ${bad} failed`);
if (bad) process.exit(1);
