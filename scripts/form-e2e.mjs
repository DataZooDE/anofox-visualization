// ::FORM (#683) in a real browser: a form generated from rows (one per
// field) renders as one card — dropdown, number, slider (with its value),
// toggle, unit, ⓘ help beside its label, provenance — and its values come
// back as ONE variable holding a JSON object (getvariable('lt')->>'mean');
// a field's show_if hides it while its condition is false; an edit updates
// the dependent panels and the URL (?v.lt={…}); a URL seed restores it and
// an out-of-range seed is ignored; a form filtered by a ::ROWSELECT click
// shows the picked node's own fields and values (edits reset per node); no
// overflow at 390 px; no page errors.
//
//   scripts/build-wasm-pkg.sh
//   python3 serve_nocache.py 8947 &
//   NODE_PATH=<dir with playwright>/node_modules node scripts/form-e2e.mjs \
//       [--url http://127.0.0.1:8947/] [--shots <dir>]
import { createRequire } from "module";

const require = createRequire(import.meta.url);
const { chromium } = require("playwright");
const arg = (k, d) => {
  const i = process.argv.indexOf(k);
  return i > 0 ? process.argv[i + 1] : d;
};
const base = arg("--url", "http://127.0.0.1:8947/");
const shots = arg("--shots", "");
const fail = (m) => {
  console.error("FAIL: " + m);
  process.exitCode = 1;
};
const check = (ok, m) => (ok ? console.log("ok   " + m) : fail(m));
const b64 = (s) => Buffer.from(s, "utf8").toString("base64");

const SQL = `-- @title Form e2e (#683)
CREATE OR REPLACE TEMP TABLE nodes AS SELECT * FROM (VALUES ('supA', 'Supplier A'), ('supB', 'Supplier B')) v(node_id, name);
CREATE OR REPLACE TEMP TABLE params AS SELECT * FROM (VALUES
 ('supA', 1, 'model', 'dropdown', 'normal', ['fixed', 'normal'], NULL, NULL, NULL, 'Lead-time model', NULL, 'How the lead time varies', NULL, 'fitted from 214 POs'),
 ('supA', 2, 'mean', 'number', '12', NULL, 0, 90, 1, 'Mean', 'days', NULL, NULL, 'fitted'),
 ('supA', 3, 'sd', 'slider', '3', NULL, 0, 20, 0.5, 'Std dev', 'days', NULL, 'model=normal', 'fitted'),
 ('supA', 4, 'disruption', 'toggle', 'false', NULL, NULL, NULL, NULL, 'Disruption risk', NULL, NULL, NULL, 'manual'),
 ('supB', 1, 'model', 'dropdown', 'fixed', ['fixed', 'normal'], NULL, NULL, NULL, 'Lead-time model', NULL, NULL, NULL, NULL),
 ('supB', 2, 'mean', 'number', '30', NULL, 0, 90, 1, 'Mean', 'days', NULL, NULL, NULL)
) v(node_id, ord, field, type, value, options, min, max, step, label, unit, help, show_if, source);
SELECT 'single'::ROWSELECT;
SELECT node_id ::TABLE, name FROM nodes;
SELECT field AS lt ::FORM, type, value, options, min, max, step, label, unit, help, show_if, source
FROM params WHERE node_id = coalesce(nullif(getvariable('node_id'), ''), 'supA') ORDER BY ord;
SELECT (getvariable('lt')->>'mean')::DOUBLE AS m ::METRIC, 'Mean lead time' ::LABEL;
SELECT coalesce(getvariable('lt')->>'model', '-') AS model ::TEXT_MEDIUM, 'Model' ::LABEL;
`;

const browser = await chromium.launch();
const errs = [];
const open = async (query, viewport = { width: 1440, height: 1000 }) => {
  const page = await browser.newPage({ viewport, reducedMotion: "reduce" });
  page.on("pageerror", (e) => errs.push(String(e)));
  await page.goto(`${base}?mode=view${query}#sql=${b64(SQL)}`);
  await ready(page);
  return page;
};
const ready = (page) =>
  page.waitForFunction(
    () => /\d+ panels?/.test(document.getElementById("status")?.textContent || "") && !document.body.classList.contains("loading"),
    null,
    { timeout: 120000 }
  );
const settle = async (page) => {
  await page.waitForTimeout(800);
  await ready(page);
};
const st = (page) =>
  page.evaluate(() => ({
    fields: [...document.querySelectorAll(".dp-form .dp-form-field")].map((f) => f.dataset.field),
    mean: document.querySelector('.dp-form-field[data-field="mean"] input')?.value,
    kpi: document.querySelector(".metric-value")?.textContent,
    model: [...document.querySelectorAll(".text-value")].map((e) => e.textContent).join(","),
    url: decodeURIComponent(location.search),
    helpInLabel: !!document.querySelector('.dp-form-field[data-field="model"] .dp-form-label .dp-info:not(.corner)'),
    sliderOut: document.querySelector('.dp-form-field[data-field="sd"] output')?.textContent,
    source: document.querySelector('.dp-form-field[data-field="model"] .dp-form-source')?.textContent,
    unit: document.querySelector('.dp-form-field[data-field="mean"] .dp-form-unit')?.textContent,
  }));

{
  const page = await open("");
  let s = await st(page);
  check(s.fields.join(",") === "model,mean,sd,disruption", `four fields in order (${s.fields})`);
  check(s.mean === "12" && s.kpi === "12", `the rows' value is the variable's (input ${s.mean}, KPI ${s.kpi})`);
  check(s.model === "normal", `a string field reads with ->> (${s.model})`);
  check(s.helpInLabel, "the ⓘ help sits beside its label");
  check(s.sliderOut === "3", `a slider shows its value (${s.sliderOut})`);
  check(s.source === "fitted from 214 POs" && s.unit === "days", `provenance and unit shown (${s.source} / ${s.unit})`);
  if (shots) await page.screenshot({ path: `${shots}/form-1440.png`, fullPage: true });

  await page.selectOption('.dp-form-field[data-field="model"] select', "fixed");
  await settle(page);
  s = await st(page);
  check(s.fields.join(",") === "model,mean,disruption", `show_if hides sd for 'fixed' (${s.fields})`);
  check(s.model === "fixed", `the dependent panel follows (${s.model})`);

  await page.fill('.dp-form-field[data-field="mean"] input', "15");
  await page.press('.dp-form-field[data-field="mean"] input', "Enter");
  await settle(page);
  s = await st(page);
  check(s.kpi === "15", `an edit updates the dependent KPI (${s.kpi})`);
  check(/v\.lt=\{"model":"fixed","mean":15,"sd":3,"disruption":false\}/.test(s.url), `the URL carries the form (${s.url})`);

  // Out of range: refused in the page, the old value stays.
  await page.fill('.dp-form-field[data-field="mean"] input', "500");
  await page.press('.dp-form-field[data-field="mean"] input', "Enter");
  await settle(page);
  s = await st(page);
  check(s.kpi === "15" && s.mean === "15", `a value above max is refused (input ${s.mean}, KPI ${s.kpi})`);

  // A ::ROWSELECT pick: the other node's own fields and values.
  await page.click(".dash td >> text=supB");
  await settle(page);
  s = await st(page);
  check(s.fields.join(",") === "model,mean" && s.mean === "30" && s.kpi === "30", `picking supB shows its own form (${s.fields}; ${s.mean}; ${s.kpi})`);
  await page.click(".dash td >> text=supA");
  await settle(page);
  s = await st(page);
  check(s.mean === "12" && s.kpi === "12", `back on supA: its own values again, the edit reset (${s.mean}; ${s.kpi})`);
  await page.close();
}

{
  // A URL seed restores the form; an out-of-range field in it is ignored.
  const seed = encodeURIComponent(JSON.stringify({ model: "normal", mean: 20, sd: 99 }));
  const page = await open(`&v.lt=${seed}`);
  const s = await st(page);
  check(s.mean === "20" && s.kpi === "20", `a ?v.lt= seed restores the mean (${s.mean}; ${s.kpi})`);
  check(s.sliderOut === "3", `an out-of-range seeded field keeps its row value (${s.sliderOut})`);
  await page.close();
}

{
  const page = await open("", { width: 390, height: 1200 });
  const overflow = await page.evaluate(() => document.documentElement.scrollWidth > document.documentElement.clientWidth + 1);
  check(!overflow, "no horizontal overflow at 390px");
  if (shots) await page.screenshot({ path: `${shots}/form-390.png`, fullPage: true });
  await page.close();
}

check(!errs.length, "no page errors: " + errs.join(" | "));
await browser.close();
console.log(`form-e2e: ${process.exitCode ? "failed" : "0 failed"}`);
