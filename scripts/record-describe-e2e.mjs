// #774 in a real browser: ::RECORD shows one record as key/value pairs (a
// STRUCT's, MAP's or JSON object's fields, else the first row's columns, with
// a note when there were more rows; values are text, never HTML); a bare
// DESCRIBE / SUMMARIZE / SHOW TABLES is a table panel. Light and dark, at
// 1440 and 390 px.
//
//   (web/pkg built as for the other e2e scripts)
//   python3 serve_nocache.py 8947 &
//   NODE_PATH=<dir with playwright>/node_modules node scripts/record-describe-e2e.mjs [--url http://127.0.0.1:8947/]
import { createRequire } from "module";

const require = createRequire(import.meta.url);
const { chromium } = require("playwright");
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
const b64 = (s) => Buffer.from(s, "utf8").toString("base64");

const SQL = `-- @title Record e2e (#774)
CREATE OR REPLACE TEMP TABLE rec_t AS SELECT 1 AS a, 'x' AS b;
SELECT {'Name': 'Ada', 'City': 'Berlin', 'Tags': [1, 2]}::RECORD, 'Struct record'::TITLE;
SELECT name ::RECORD, city, '<b>x</b>' AS html, 'Rows record'::TITLE FROM (VALUES ('Ada', 'Berlin'), ('Bob', 'Paris')) t(name, city);
SELECT MAP {'k1': 1, 'k2': 2}::RECORD, 'Map record'::TITLE;
SELECT '{"a": 1, "b": null}'::JSON ::RECORD, 'Json record'::TITLE;
DESCRIBE rec_t;
SUMMARIZE rec_t;
SHOW TABLES;
`;

const ready = (page) =>
  page.waitForFunction(
    () => /\d+ panels?/.test(document.getElementById("status")?.textContent || "") && !document.body.classList.contains("loading"),
    null,
    { timeout: 120000 }
  );

const browser = await chromium.launch();
for (const [w, h] of [[1440, 900], [390, 800]]) {
  for (const dark of [false, true]) {
    const tag = `${w}px${dark ? " dark" : ""}`;
    const page = await browser.newPage({ viewport: { width: w, height: h }, colorScheme: dark ? "dark" : "light", reducedMotion: "reduce" });
    const errs = [];
    page.on("pageerror", (e) => errs.push(String(e)));
    await page.goto(`${base}?mode=view#sql=${b64(SQL)}`);
    await ready(page);
    const recs = await page.evaluate(() =>
      Object.fromEntries(
        [...document.querySelectorAll(".dp-record")].map((dl) => {
          const fig = dl.closest(".panel");
          const t = fig.querySelector(".panel-title")?.textContent || "";
          const pairs = [...dl.querySelectorAll("dt")].map((dt) => [dt.textContent, dt.nextElementSibling.textContent]);
          return [t, { pairs, note: fig.querySelector(".dp-record-note")?.textContent || null, html: dl.querySelectorAll("b").length, overflow: dl.scrollWidth > dl.clientWidth + 1 }];
        })
      )
    );
    const eq = (a, b) => JSON.stringify(a) === JSON.stringify(b);
    check(eq(recs["Struct record"]?.pairs, [["Name", "Ada"], ["City", "Berlin"], ["Tags", "[1,2]"]]), `${tag}: a STRUCT's fields (${JSON.stringify(recs["Struct record"])})`);
    check(eq(recs["Rows record"]?.pairs, [["name", "Ada"], ["city", "Berlin"], ["html", "<b>x</b>"]]) && recs["Rows record"].html === 0, `${tag}: the first row's columns, HTML as text (${JSON.stringify(recs["Rows record"])})`);
    check(recs["Rows record"]?.note === "The first of 2 rows.", `${tag}: a note for more rows`);
    check(eq(recs["Map record"]?.pairs, [["k1", "1"], ["k2", "2"]]), `${tag}: a MAP's entries (${JSON.stringify(recs["Map record"])})`);
    check(eq(recs["Json record"]?.pairs, [["a", "1"], ["b", ""]]), `${tag}: a JSON object's fields (${JSON.stringify(recs["Json record"])})`);
    check(Object.values(recs).every((r) => !r.overflow), `${tag}: no record overflows its panel`);
    const tables = await page.evaluate(() =>
      [...document.querySelectorAll(".panel table")].map((t) => [...t.querySelectorAll("thead th")].map((th) => th.textContent.replace(/[↕↑↓]/g, "").trim().toLowerCase().replace(/ /g, "_")))
    );
    const has = (cols) => tables.some((hs) => cols.every((c) => hs.some((h) => h.startsWith(c))));
    check(has(["column_name", "column_type"]), `${tag}: DESCRIBE is a table (${JSON.stringify(tables)})`);
    check(has(["column_name", "min", "max"]), `${tag}: SUMMARIZE is a table`);
    check(has(["name"]) && tables.length >= 3, `${tag}: SHOW TABLES is a table`);
    check(errs.length === 0, `${tag}: no page errors ${errs.join("; ")}`);
    await page.close();
  }
}
await browser.close();
