// ::FORM (#683) in a real browser: a form generated from rows (one per
// field) renders as one card — dropdown, number, slider (with its value),
// toggle, unit, ⓘ help beside its label, provenance — and its values come
// back as ONE variable holding a JSON object (getvariable('lt')->>'mean');
// a field's show_if hides it while its condition is false; an edit updates
// the dependent panels and the URL (?v.lt={…}); a URL seed restores it and
// an out-of-range seed is ignored; a form filtered by a ::ROWSELECT click
// shows the picked node's own fields and values (edits reset per node); no
// overflow at 390 px; no page errors.
// #1233, long forms: ::TITLE / ::SUBTITLE on the card; `section` as
// headings, tabs or an accordion (::FORM_LAYOUT), the open tab / sections /
// "More" parts kept in the URL (?form.<var>=) and restored from it;
// `advanced` fields under "More"; `width` 'half' two to a row, every field
// one height; show_if with & and |; a preset fills several fields (and
// shows "Custom" once one is edited), round-tripped through ?v.<var>=;
// numbers in the dashboard's ::LOCALE (de-DE: "3,2", typing "3,5").
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

// ---- #1233: long forms --------------------------------------------------
const LONG = (layout, locale = "") => `-- @title Long form e2e (#1233)
${locale ? `SELECT '${locale}'::LOCALE;\n` : ""}SELECT field AS cmp ::FORM, section, type, value, options, min, max, step, label, unit, show_if, advanced, width,
       'Scenario inputs' ::TITLE, 'Pick a preset' ::SUBTITLE, '${layout}' ::FORM_LAYOUT
FROM (VALUES
 (1, 'Household', 'size', 'preset', NULL, '{"Flat": {"use_kwh": 2000, "people": 1}, "House": {"use_kwh": 5000, "people": 4}}', NULL, NULL, NULL, 'Preset', NULL, NULL, false, 'full'),
 (2, 'Household', 'people', 'number', '2', NULL, 1, 12, 1, 'People', NULL, NULL, false, 'half'),
 (3, 'Household', 'use_kwh', 'number', '3000', NULL, 500, 20000, 100, 'Use', 'kWh/yr', NULL, false, 'half'),
 (4, 'Household', 'use_sd', 'number', '10', NULL, 0, 50, 1, 'Use, ±', '%', NULL, true, 'half'),
 (5, 'Systems', 'capex', 'toggle', 'false', NULL, NULL, NULL, NULL, 'Count the investment', NULL, NULL, false, 'half'),
 (6, 'Systems', 'heating', 'dropdown', 'gas', 'gas, heat pump', NULL, NULL, NULL, 'Heating', NULL, NULL, false, 'half'),
 (7, 'Systems', 'cop', 'number', '3.2', NULL, 2, 5, 0.1, 'COP', NULL, 'heating=heat pump', false, 'half'),
 (8, 'Money', 'fin', 'dropdown', 'cash', 'cash, loan', NULL, NULL, NULL, 'Paid by', NULL, NULL, false, 'half'),
 (9, 'Money', 'loan_pct', 'number', '5', NULL, 0, 15, 0.1, 'Loan interest', '%/yr', 'capex=true & fin=loan', false, 'half'),
 (10, 'Money', 'note', 'text', 'x', NULL, NULL, NULL, NULL, 'Note', NULL, 'fin=loan | heating=heat pump', false, 'full')
) t(ord, section, field, type, value, options, min, max, step, label, unit, show_if, advanced, width) ORDER BY ord;
SELECT (getvariable('cmp')->>'use_kwh')::DOUBLE AS u ::METRIC, 'Use' ::LABEL;
`;
const openLong = async (sql, query = "", viewport = { width: 1200, height: 1000 }) => {
  const page = await browser.newPage({ viewport, reducedMotion: "reduce" });
  page.on("pageerror", (e) => errs.push(String(e)));
  await page.goto(`${base}?mode=view${query}#sql=${b64(sql)}`);
  await ready(page);
  return page;
};
const lst = (page) =>
  page.evaluate(() => {
    const form = document.querySelector(".dp-form");
    // (a closed <details> keeps its content laid out in Chrome)
    const shown = (el) => !!el && el.getClientRects().length > 0 && !el.closest("details:not([open]) > :not(summary)");
    const field = (k) => form.querySelector(`.dp-form-field[data-field="${k}"]`);
    return {
      title: form.querySelector(".panel-title")?.textContent,
      subtitle: form.querySelector(".panel-subtitle")?.textContent,
      layout: form.dataset.layout,
      legends: [...form.querySelectorAll("fieldset.dp-form-sec > legend")].map((e) => e.textContent),
      tabs: [...form.querySelectorAll(".dp-form-tab")].map((e) => e.textContent + (e.classList.contains("active") ? "*" : "")),
      acc: [...form.querySelectorAll("details.dp-form-sec")].map((d) => d.dataset.section + (d.open ? "*" : "")),
      visible: [...form.querySelectorAll(".dp-form-field")].filter(shown).map((f) => f.dataset.field),
      present: [...form.querySelectorAll(".dp-form-field")].map((f) => f.dataset.field),
      moreOpen: [...form.querySelectorAll(".dp-form-more")].filter((d) => d.open).map((d) => d.dataset.section),
      preset: field("size")?.querySelector("select")?.value,
      people: field("people")?.querySelector("input")?.value,
      use: field("use_kwh")?.querySelector("input")?.value,
      cop: field("cop")?.querySelector("input")?.value,
      halfSideBySide: (() => {
        const a = field("people")?.getBoundingClientRect(), b = field("use_kwh")?.getBoundingClientRect();
        return !!a && !!b && Math.abs(a.top - b.top) < 2 && b.left > a.right - 1;
      })(),
      heights: [...form.querySelectorAll(".dp-form-field:not(.half) select.dp-form-input, .dp-form-field .dp-form-box, .dp-form-field input.dp-form-input:not([type=checkbox]):not([type=range])")]
        .filter(shown).map((e) => Math.round(e.closest(".dp-form-box") ? e.closest(".dp-form-box").getBoundingClientRect().height : e.getBoundingClientRect().height)),
      kpi: document.querySelector(".metric-value")?.textContent,
      url: decodeURIComponent(location.search),
    };
  });

{
  // list: headings, title, subtitle, half fields side by side, one height
  const page = await openLong(LONG("list"));
  let s = await lst(page);
  check(s.title === "Scenario inputs" && s.subtitle === "Pick a preset", `::TITLE / ::SUBTITLE on the form card (${s.title} / ${s.subtitle})`);
  check(s.layout === "list" && s.legends.join() === "Household,Systems,Money", `sections as headings (${s.legends})`);
  check(s.halfSideBySide, "two 'half' fields share a row");
  check(s.heights.length > 3 && new Set(s.heights).size === 1, `every field one height (${s.heights})`);
  check(!s.visible.includes("use_sd") && s.present.includes("people"), `an advanced field waits under "More" (${s.visible})`);
  check(!s.present.includes("cop") && !s.present.includes("loan_pct") && !s.present.includes("note"), `show_if hides cop, loan_pct, note (${s.present})`);
  // & : loan_pct needs capex AND a loan; | : note needs a loan OR a heat pump
  await page.selectOption('.dp-form-field[data-field="fin"] select', "loan");
  await settle(page);
  s = await lst(page);
  check(!s.present.includes("loan_pct") && s.present.includes("note"), `fin=loan alone: no loan_pct (&), note shows (|) (${s.present})`);
  await page.click('.dp-form-field[data-field="capex"] input');
  await settle(page);
  s = await lst(page);
  check(s.present.includes("loan_pct"), `capex AND loan: loan_pct shows (${s.present})`);
  // "More" opens and goes into the URL
  await page.click('.dp-form-more[data-section="Household"] > summary');
  await page.waitForTimeout(300);
  s = await lst(page);
  check(s.visible.includes("use_sd") && /form\.cmp=\{"more":\["Household"\]\}/.test(s.url), `"More" opens, kept in the URL (${s.url})`);
  // a preset fills several fields; an edit afterwards shows "Custom"
  await page.selectOption('.dp-form-field[data-field="size"] select', "House");
  await settle(page);
  s = await lst(page);
  check(s.people === "4" && s.use === "5000" && s.kpi === "5,000" && s.preset === "House", `the preset fills people and use (${s.people}, ${s.use}, KPI ${s.kpi}, ${s.preset})`);
  check(/v\.cmp=\{[^}]*"use_kwh":5000/.test(s.url) && !/"size"/.test(s.url), `the preset's values in ?v.cmp=, the preset itself no key (${s.url})`);
  const url = s.url;
  await page.fill('.dp-form-field[data-field="people"] input', "3");
  await page.press('.dp-form-field[data-field="people"] input', "Enter");
  await settle(page);
  s = await lst(page);
  check(s.people === "3" && s.use === "5000" && s.preset === "", `an edit keeps the rest, the preset shows Custom (${s.people}, ${s.use}, "${s.preset}")`);
  await page.close();
  // the URL round trip: values, the preset recognised, "More" open
  const p2 = await openLong(LONG("list"), "&" + url.replace(/^\?/, "").split("&").filter((x) => /^(v|form)\./.test(x)).map((x) => x.replace(/=(.*)$/, (_, v) => "=" + encodeURIComponent(v))).join("&"));
  s = await lst(p2);
  check(s.people === "4" && s.use === "5000" && s.preset === "House" && s.moreOpen.join() === "Household", `a link restores the values, the preset and "More" (${s.people}, ${s.use}, ${s.preset}, ${s.moreOpen})`);
  if (shots) await p2.screenshot({ path: `${shots}/form-long-list.png`, fullPage: true });
  await p2.close();
}

{
  // tabs: the first active; a click switches, kept in and restored from the URL
  const page = await openLong(LONG("tabs"));
  let s = await lst(page);
  check(s.tabs.join() === "Household*,Systems,Money" && s.visible.includes("people") && !s.visible.includes("heating"), `tabs, the first open (${s.tabs}; ${s.visible})`);
  await page.click('.dp-form-tab >> text=Money');
  await page.waitForTimeout(300);
  s = await lst(page);
  check(s.tabs.join() === "Household,Systems,Money*" && s.visible.includes("fin") && !s.visible.includes("people") && /form\.cmp=\{"tab":"Money"\}/.test(s.url), `a tab click switches, kept in the URL (${s.tabs}; ${s.url})`);
  if (shots) await page.screenshot({ path: `${shots}/form-long-tabs.png`, fullPage: true });
  await page.close();
  const p2 = await openLong(LONG("tabs"), "&form.cmp=" + encodeURIComponent('{"tab":"Systems"}'));
  s = await lst(p2);
  check(s.tabs.join() === "Household,Systems*,Money", `?form.cmp= opens that tab (${s.tabs})`);
  await p2.close();
}

{
  // accordion: the first section open; opening another is kept
  const page = await openLong(LONG("accordion"));
  let s = await lst(page);
  check(s.acc.join() === "Household*,Systems,Money", `accordion, the first open (${s.acc})`);
  await page.click('details.dp-form-sec[data-section="Money"] > summary');
  await page.waitForTimeout(300);
  s = await lst(page);
  check(s.acc.join() === "Household*,Systems,Money*" && /form\.cmp=\{"open":\["Household","Money"\]\}/.test(s.url), `another section opens, kept in the URL (${s.acc}; ${s.url})`);
  await page.close();
}

{
  // numbers in the dashboard's ::LOCALE
  const page = await openLong(LONG("list", "de-DE"));
  await page.selectOption('.dp-form-field[data-field="heating"] select', "heat pump");
  await settle(page);
  let s = await lst(page);
  check(s.cop === "3,2", `de-DE shows the decimal comma (${s.cop})`);
  await page.fill('.dp-form-field[data-field="cop"] input', "3,5");
  await page.press('.dp-form-field[data-field="cop"] input', "Enter");
  await settle(page);
  s = await lst(page);
  check(s.cop === "3,5" && /"cop":3\.5/.test(s.url), `"3,5" reads as 3.5 (${s.cop}; ${s.url})`);
  await page.fill('.dp-form-field[data-field="cop"] input', "9");
  await page.press('.dp-form-field[data-field="cop"] input', "Enter");
  await settle(page);
  s = await lst(page);
  check(s.cop === "3,5", `out of range: refused, the value stays (${s.cop})`);
  await page.close();
}

{
  const page = await openLong(LONG("tabs"), "", { width: 390, height: 1200 });
  const overflow = await page.evaluate(() => document.documentElement.scrollWidth > document.documentElement.clientWidth + 1);
  check(!overflow, "a long form: no horizontal overflow at 390px");
  await page.close();
}

check(!errs.length, "no page errors: " + errs.join(" | "));
await browser.close();
console.log(`form-e2e: ${process.exitCode ? "failed" : "0 failed"}`);
