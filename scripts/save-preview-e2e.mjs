// ::SAVE (#684 slice 1) in the builder: a Save button writes the edited
// graph into the declared tables for this tab only (TEMP tables shadowing
// the base ones), so panels reading them recompute; a ::FORM save writes
// one row keyed by the clicked node, merged over the base table's other
// rows; a bad table name says why; nothing reaches a server.
//
//   scripts/build-wasm-pkg.sh
//   python3 serve_nocache.py 8947 &
//   NODE_PATH=<dir with playwright>/node_modules node scripts/save-preview-e2e.mjs [--url http://127.0.0.1:8947/]
import { createRequire } from "module";

const require = createRequire(import.meta.url);
const { chromium } = require("playwright");
const arg = (k, d) => {
  const i = process.argv.indexOf(k);
  return i > 0 ? process.argv[i + 1] : d;
};
const base = arg("--url", "http://127.0.0.1:8947/");
const check = (ok, m) => (ok ? console.log("ok   " + m) : (console.error("FAIL: " + m), (process.exitCode = 1)));
const b64 = (s) => Buffer.from(s, "utf8").toString("base64");

const SQL = `-- @title Save preview e2e (#684)
CREATE OR REPLACE TABLE plan_nodes AS SELECT * FROM (VALUES ('sup', 'supplier', 'Supplier', NULL), ('wh', 'stock', 'Warehouse', NULL)) v(id, type, label, stage);
CREATE OR REPLACE TABLE plan_edges AS SELECT * FROM (VALUES ('sup', 'wh')) v("from", "to");
CREATE OR REPLACE TABLE plan_params AS SELECT * FROM (VALUES ('sup', 7), ('wh', 3)) v(id, lead_time);
SELECT id ::DIAGRAM, label ::LABEL, type ::CATEGORY, 'edit' AS graph ::DIAGRAM_MODE, 'Network' ::TITLE FROM plan_nodes;
SELECT 'Save plan' ::SAVE, 'graph' ::SAVE_GRAPH, 'plan_nodes, plan_edges' ::SAVE_TABLES;
SELECT count(*) AS n ::METRIC, 'Saved nodes' ::LABEL FROM plan_nodes;
SELECT field AS lt ::FORM, type, value FROM (VALUES ('lead_time', 'number', '5')) v(field, type, value);
SELECT 'Save parameters' ::SAVE, 'lt' ::SAVE_FORM, 'plan_params' ::SAVE_TABLES;
SELECT string_agg(id || '=' || lead_time, ',' ORDER BY id) AS p ::TEXT_SMALL FROM plan_params;
SELECT 'Bad' ::SAVE, 'graph' ::SAVE_GRAPH, 'x; DROP TABLE plan_nodes, y' ::SAVE_TABLES;
`;

const browser = await chromium.launch();
const page = await browser.newPage({ viewport: { width: 1300, height: 1100 }, reducedMotion: "reduce" });
const errs = [];
page.on("pageerror", (e) => errs.push(String(e)));
const ready = () =>
  page.waitForFunction(() => /\d+ panels?/.test(document.getElementById("status")?.textContent || "") && !document.body.classList.contains("loading"), null, { timeout: 120000 });
const settle = async () => {
  await page.waitForTimeout(700);
  await ready();
};
await page.goto(`${base}?mode=view#sql=${b64(SQL)}`);
await ready();
const kpi = () => page.evaluate(() => document.querySelector(".metric-value")?.textContent);
const text = () => page.evaluate(() => [...document.querySelectorAll(".textcard .text-value")].map((e) => e.textContent).join("|"));
const btn = (label) => page.locator(`.save-panel .submit-btn:text-is("${label}")`);
check((await kpi()) === "2", `base rows before a save (${await kpi()})`);
check(await page.evaluate(() => /stays in this tab/.test(document.querySelector(".save-panel .submit-preview-note")?.textContent || "")), "the preview note says the save stays in the tab");

await page.selectOption(".dp-edit-type", "stock");
await page.fill(".dp-edit-name", "DC South");
await page.click('.dp-edit-bar >> text=Add');
await settle();
check((await kpi()) === "2", "an edit alone doesn't change the saved table");
await btn("Save plan").click();
await settle();
check((await kpi()) === "3", `Save writes the graph into plan_nodes (${await kpi()})`);
const said = await page.evaluate(() => [...document.querySelectorAll(".save-panel")].map((f) => f.textContent).join(" | "));
check(/Saved 3 nodes and 0 edges into plan_nodes, plan_edges/.test(said), `the button says what it saved (${said})`);

await page.click('.dash rect[data-node="wh"]', { force: true });
await settle();
await page.fill('.dp-form-field[data-field="lead_time"] input', "9");
await page.keyboard.press("Enter");
await settle();
await btn("Save parameters").click();
await settle();
check((await text()) === "sup=7,wh=9", `a form save replaces the clicked node's row, keeps the others (${await text()})`);

await btn("Bad").click();
await settle();
check(await page.evaluate(() => /is not a plain table name/.test(document.body.textContent)), "a bad table name is refused, saying why");
check((await kpi()) === "3", "…and nothing was written");
check(!errs.length, "no page errors: " + errs.join(" | "));
await browser.close();
console.log(`save-preview-e2e: ${process.exitCode ? "failed" : "0 failed"}`);
