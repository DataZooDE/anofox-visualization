// #1240: uppercased labels keep their Greek letters (μ, not M). In a real
// browser, `innerText` is the text as drawn (CSS text-transform applied):
//   - the app (DuckDB-Wasm, the :8947 server): a KPI label and a ::FORM
//     section title;
//   - a static page (`dashboard <file.sql> out.html`): the theme's brand name.
//
//   NODE_PATH=<dir with playwright>/node_modules node scripts/nocase-e2e.mjs \
//     [--url http://127.0.0.1:8947/] [--dashboard target/debug/dashboard]
// Runs the part each flag names (CI runs the app part while its :8947 server
// is up, the static part once the DuckDB CLI is installed).
import { createRequire } from "module";
import { mkdtempSync, writeFileSync } from "fs";
import { execFileSync } from "child_process";
import path from "path";
import os from "os";

const require = createRequire(import.meta.url);
const { chromium } = require("playwright");
const arg = (k, d) => {
  const i = process.argv.indexOf(k);
  return i > 0 ? process.argv[i + 1] : d;
};
const base = arg("--url", null);
const bin = arg("--dashboard", null) && path.resolve(arg("--dashboard"));
if (!base && !bin) {
  console.error("nocase-e2e: pass --url and/or --dashboard");
  process.exit(2);
}
let bad = 0;
const check = (ok, m, got) => (ok ? console.log("ok   " + m) : (bad++, console.error("FAIL " + m + (got === undefined ? "" : " " + JSON.stringify(got)))));

const sql = `SELECT 11.98 ::METRIC, 'Average cycle μ, min' ::LABEL;
SELECT field AS ct ::FORM, section, type, value, label
FROM (VALUES ('How much a job varies (τ)', 'sd', 'number', '1', 'Spread')) t(section, field, type, value, label);`;

const browser = await chromium.launch();
const errors = [];
const open = async (url) => {
  const p = await browser.newPage({ viewport: { width: 1200, height: 800 } });
  p.on("pageerror", (e) => errors.push(String(e)));
  await p.goto(url);
  return p;
};
const drawn = (p, sel) => p.evaluate((sel) => [...document.querySelectorAll(sel)].map((e) => e.innerText.trim()), sel);

// the app
if (base) {
  const p = await open(`${base}?embed=1#sql=${Buffer.from(sql, "utf8").toString("base64")}`);
  await p.waitForFunction(() => document.querySelector(".metric-cap") && document.querySelector(".dp-form-sec-title"), null, { timeout: 120000 });
  await p.waitForTimeout(300);
  const cap = await drawn(p, ".metric-cap");
  check(cap.includes("AVERAGE CYCLE μ, MIN"), "app: the KPI label reads AVERAGE CYCLE μ, MIN", cap);
  const sec = await drawn(p, ".dp-form-sec-title");
  check(sec.includes("HOW MUCH A JOB VARIES (τ)"), "app: the form section reads … (τ)", sec);
  await p.close();
}

// a static page
if (bin) {
  const dir = mkdtempSync(path.join(os.tmpdir(), "nocase-"));
  writeFileSync(path.join(dir, "k.sql"), "SELECT 11.98 ::METRIC, 'Average, min' ::LABEL;");
  writeFileSync(path.join(dir, "t.json"), JSON.stringify({ name: "t", extends: "light", brand: { name: "Team μ" } }));
  execFileSync(bin, ["k.sql", "k.html", "--theme", "t.json"], { cwd: dir, stdio: ["ignore", "ignore", "inherit"] });
  const p = await open("file://" + path.join(dir, "k.html"));
  await p.waitForTimeout(300);
  const h = await drawn(p, ".dp-brand");
  check(h.includes("TEAM μ"), "static page: the brand name reads TEAM μ", h);
  await p.close();
}

check(!errors.length, "no page errors", errors);
await browser.close();
if (bad) {
  console.error(`nocase-e2e: ${bad} failed`);
  process.exit(1);
}
console.log("nocase-e2e: all checks passed");
