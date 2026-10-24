// #416: a dashboard's own `SET VARIABLE` that reads an input via
// getvariable() must see the viewer's CURRENT value, not the input's
// previous one — the exact repro from the issue, in a real browser
// (DuckDB-Wasm): load with `?v.extension=b` and pick `c` show the input's
// previous pick in the panel that reads the derived variable, because the
// render loop ran the dashboard's `SET VARIABLE sel = …` before applying
// the viewer's value (tests/web/setvar_order_test.mjs covers the pure
// decision logic; this proves the real run loop + DuckDB-Wasm end to end,
// a Share link included).
//
//   wasm-pack build --target web --out-dir web/pkg --profile wasm --no-default-features --features wasm
//   python3 serve_nocache.py 8947 &
//   NODE_PATH=<dir with playwright>/node_modules node scripts/setvar-order-e2e.mjs [--url http://127.0.0.1:8947/]
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
  page.waitForFunction(() => document.querySelector(".grid .panel") && !document.body.classList.contains("loading"), null, { timeout: 30000 });
const titleText = (page) => page.locator(".grid figcaption.panel-title, .grid .panel-title").first().textContent();

// The issue's own SQL, verbatim: a dropdown, then a `SET VARIABLE` deriving
// from it, then a panel reading the derived variable. This order already
// worked before #416 (the input's own SET VARIABLE ran first, in script
// order) — kept as a non-regression check alongside the broken order below.
const forward = `SELECT name AS extension ::DROPDOWN FROM (VALUES ('alpha'), ('bravo'), ('charlie')) t(name);
SET VARIABLE sel = coalesce(nullif(getvariable('extension'), ''), 'default');
SELECT 'Downloads of ' || getvariable('sel') AS t ::TITLE;`;

// The order that reproduced #416: the derived `SET VARIABLE` declared
// BEFORE the dropdown it reads — a dashboard is free to write it either
// way, and the locked server (duckext/src/serve.rs) handles both the same.
const reversed = `SET VARIABLE sel = coalesce(nullif(getvariable('extension'), ''), 'default');
SELECT name AS extension ::DROPDOWN FROM (VALUES ('alpha'), ('bravo'), ('charlie')) t(name);
SELECT 'Downloads of ' || getvariable('sel') AS t ::TITLE;`;

async function run(sql, label) {
  const page = await browser.newPage({ viewport: { width: 1000, height: 800 }, reducedMotion: "reduce" });
  const errs = [];
  page.on("pageerror", (e) => errs.push(String(e)));

  // 1. First load, as a Share link (`?embed=1#sql=…`) with a `?v.` seed —
  // the panel reading the derived variable must show the SEEDED pick, not
  // the dashboard's own fallback.
  await page.goto(`${base}?embed=1&v.extension=bravo#sql=${b64(sql)}`);
  await settled(page);
  check((await titleText(page)).includes("Downloads of bravo"), `${label}: first load (?v.extension=bravo) shows bravo, not the fallback`);

  // 2. Pick a different value: the panel must show THAT pick, never the
  // previous one (the exact "shows b, then picks c, still shows b" repro).
  await page.selectOption("select[aria-label=extension]", "charlie");
  await settled(page);
  check((await titleText(page)).includes("Downloads of charlie"), `${label}: picking charlie shows charlie, not bravo`);

  // 3. …and again, back to alpha — not stuck one pick behind.
  await page.selectOption("select[aria-label=extension]", "alpha");
  await settled(page);
  check((await titleText(page)).includes("Downloads of alpha"), `${label}: picking alpha shows alpha, not charlie`);

  check(errs.length === 0, `${label}: no page errors${errs.length ? ": " + errs.join("; ") : ""}`);
  await page.close();
}

try {
  await run(forward, "(a) derived SET VARIABLE after the input");
  await run(reversed, "(b) derived SET VARIABLE before the input (#416)");
} finally {
  await browser.close();
}
console.log(`setvar-order-e2e: ${bad} failed`);
if (bad) process.exit(1);
