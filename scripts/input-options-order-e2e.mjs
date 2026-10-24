// #428: an input's OPTIONS query that reads another input's variable must
// see that other input's CURRENT value, not the previous one — whether the
// first input is declared before or after the one it reads, in a real
// browser (DuckDB-Wasm). (tests/web/input_options_order_test.mjs covers the
// pure decision logic; scripts/input-options-order-served-e2e.mjs covers
// the locked server, where the bug is unconditional rather than one run
// stale — not run here, it needs a native extension build.)
//
//   wasm-pack build --target web --out-dir web/pkg --profile wasm --no-default-features --features wasm
//   python3 serve_nocache.py 8947 &
//   NODE_PATH=<dir with playwright>/node_modules node scripts/input-options-order-e2e.mjs [--url http://127.0.0.1:8947/]
import { createRequire } from "module";

const require = createRequire(import.meta.url);
const { chromium } = require("playwright");
const arg = (k, d) => {
  const i = process.argv.indexOf(k);
  return i > 0 ? process.argv[i + 1] : d;
};
const base = arg("--url", "http://127.0.0.1:8947/");
const b64 = (s) => Buffer.from(s, "utf8").toString("base64");
let bad = 0;
const check = (ok, m) => (ok ? console.log("ok   " + m) : (bad++, console.error("FAIL " + m)));

const browser = await chromium.launch();
const settled = (page) =>
  page.waitForFunction(
    () => !document.body.classList.contains("loading") && document.querySelectorAll("select[aria-label=extension] option").length > 0,
    null,
    { timeout: 30000 }
  );
const options = (page) => page.locator("select[aria-label=extension] option").allTextContents();

// The issue's own SQL, verbatim in spirit: a dropdown whose OPTIONS query
// filters by a search box's variable, with the dropdown declared BEFORE the
// search box — "a layout that's easy to end up with", per the issue.
const sql = `SELECT extension AS extension ::DROPDOWN
FROM (VALUES ('alpha'), ('anofox'), ('bravo'), ('charlie')) t(extension)
WHERE extension ILIKE '%' || COALESCE(getvariable('q'), '') || '%'
ORDER BY 1;

SELECT '' AS q ::TEXT, 'Search'::LABEL;
SELECT 'Picked: ' || getvariable('extension') AS t ::TITLE;`;

// The search box declared FIRST — already worked before #428 (the dropdown
// reaches its own turn after q's SET VARIABLE has run); kept as a
// non-regression check.
const sqlReversed = `SELECT '' AS q ::TEXT, 'Search'::LABEL;
SELECT extension AS extension ::DROPDOWN
FROM (VALUES ('alpha'), ('anofox'), ('bravo'), ('charlie')) t(extension)
WHERE extension ILIKE '%' || COALESCE(getvariable('q'), '') || '%'
ORDER BY 1;
SELECT 'Picked: ' || getvariable('extension') AS t ::TITLE;`;

async function run(label, sql) {
  const page = await browser.newPage({ viewport: { width: 900, height: 700 }, reducedMotion: "reduce" });
  const errs = [];
  page.on("pageerror", (e) => errs.push(String(e)));

  // 1. A hard reload with `?v.q=ano` already in the address bar: the
  // dropdown's OPTIONS must already be narrowed on this very first run.
  await page.goto(`${base}?embed=1&v.q=ano#sql=${b64(sql)}`);
  await settled(page);
  check(dequal(await options(page), ["anofox"]), `${label}: first load with ?v.q=ano narrows to just anofox`);

  // 2. Typing a different query: must show THAT query's matches, never the
  // previous one (the exact "shows all 4, lags one run behind" repro).
  await page.fill("label.control:has-text('Search') input", "bravo");
  await page.keyboard.press("Enter");
  await settled(page);
  check(dequal(await options(page), ["bravo"]), `${label}: typing bravo narrows to just bravo, not stale`);

  // 3. The current pick (bravo) falls out of a further narrowing: it must
  // fall back to the first remaining option (docs/DOCS.md), not error or
  // stay stuck on an option no longer offered.
  await page.fill("label.control:has-text('Search') input", "an");
  await page.keyboard.press("Enter");
  await settled(page);
  const opts3 = await options(page);
  check(dequal(opts3, ["anofox"]), `${label}: narrowing further to 'an' leaves just anofox`);
  const panel = await page.locator(".grid").first().textContent();
  check(panel.includes("Picked: anofox"), `${label}: the fallen-out pick falls back to the first remaining option (anofox)`);

  check(errs.length === 0, `${label}: no page errors${errs.length ? ": " + errs.join("; ") : ""}`);
  await page.close();
}
const dequal = (a, b) => JSON.stringify(a) === JSON.stringify(b);

try {
  await run("(a) dropdown before the search box (#428)", sql);
  await run("(b) search box before the dropdown", sqlReversed);
} finally {
  await browser.close();
}
console.log(`input-options-order-e2e: ${bad} failed`);
if (bad) process.exit(1);
