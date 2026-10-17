// Explore data in a real browser (issue #97): pick tables of the samples, read
// their column cards, chart a column, run queries (results, errors with their
// position, history, a click on a value adds a WHERE), add a file, build a
// starter dashboard and run it with no panel errors — in light and dark and
// at phone width (390 px). Also: picking a second table while the first still
// profiles supersedes it; profiling a 1 M-row table stays under a time bound;
// a locked (served) page never exposes Explore.
//
//   python3 serve_nocache.py 8940 &
//   NODE_PATH=<dir with playwright>/node_modules node scripts/explore-e2e.mjs \
//       [--url http://127.0.0.1:8940/] [--perf-ms 20000] [--shots DIR]
import { createRequire } from "module";
import { mkdirSync, writeFileSync } from "fs";
import os from "os";
import path from "path";

const require = createRequire(import.meta.url);
const { chromium } = require("playwright");
const arg = (k, d) => {
  const i = process.argv.indexOf(k);
  return i > 0 ? process.argv[i + 1] : d;
};
const base = arg("--url", "http://127.0.0.1:8940/");
const perfMs = Number(arg("--perf-ms", "20000"));
const shots = arg("--shots", "");
if (shots) mkdirSync(shots, { recursive: true });
let failed = 0;
const fail = (m) => {
  console.error("FAIL: " + m);
  failed++;
  process.exitCode = 1;
};
const ok = (m) => console.log("ok   " + m);

const browser = await chromium.launch();

// The samples whose tables are explored (each creates its tables when it runs).
const SAMPLES = ["Period & KPIs", "Overview", "Formatting & paging"];

// The first sample by URL, the others from the sidebar (same page: the
// in-browser DuckDB keeps every sample's tables).
const ran = () => document.querySelector(".dash .panel") && !document.body.classList.contains("loading");
async function open(page, dash, first) {
  if (first) await page.goto(`${base}?dashboard=${encodeURIComponent(dash)}`);
  else await page.evaluate((d) => [...document.querySelectorAll(".side-item")].find((b) => b.querySelector(".s-name")?.textContent === d).click(), dash);
  await page.waitForFunction((d) => document.getElementById("dash-name").value === d, dash, { timeout: 60000 });
  await page.waitForFunction(ran, null, { timeout: 120000 });
}
const settled = (page) => page.waitForFunction(() => document.querySelector(".xp-progress.done") && !document.querySelector("#xq-detail[aria-busy]"), null, { timeout: 120000 });
async function pick(page, table) {
  await page.click(`.cat-table[data-fq$='"${table}"']`);
  await settled(page);
}
const noPageErrors = (errs, what) => (errs.length ? fail(`${what}: page errors: ${errs.join(" | ")}`) : ok(`${what}: no page errors`));

async function exploreRun({ width, dark }) {
  const tag = `${width}px ${dark ? "dark" : "light"}`;
  const ctx = await browser.newContext({ viewport: { width, height: 900 }, colorScheme: dark ? "dark" : "light", reducedMotion: "reduce" });
  const page = await ctx.newPage();
  const errs = [];
  page.on("pageerror", (e) => errs.push(String(e)));
  // Run the samples so their tables exist in the in-browser DuckDB.
  for (const [i, d] of SAMPLES.entries()) await open(page, d, i === 0);
  if (width < 600 && (await page.isHidden("#side-explore"))) await page.click("#side-toggle");
  await page.click("#side-explore");
  if (!(await page.evaluate(() => document.body.classList.contains("mode-explore")))) return fail(`${tag}: Explore did not open`);
  // a phone's sidebar drawer closes once Explore is picked
  if (await page.evaluate(() => document.body.classList.contains("side-open"))) fail(`${tag}: the sidebar drawer stays open over Explore`);
  await page.waitForSelector(".cat-table");
  const tables = await page.$$eval(".cat-table", (b) => b.map((x) => x.dataset.fq));
  if (tables.length < 3) fail(`${tag}: only ${tables.length} tables in the catalog`);
  const meta = await page.$$eval(".cat-table .cat-meta", (m) => m.map((x) => x.textContent));
  if (!meta.some((m) => /col/.test(m))) fail(`${tag}: no row / column counts in the catalog`);

  // Every sample table: cards with a heading per column, mini charts with a summary.
  for (const t of ["shop", "sessions"]) {
    await pick(page, t);
    const cards = await page.$$eval(".xp-card", (cs) =>
      cs.map((c) => ({ h: !!c.querySelector("h4"), kind: c.dataset.kind, viz: c.querySelector("svg.xp-mini")?.getAttribute("aria-label") || "", pending: c.classList.contains("pending") }))
    );
    if (!cards.length || cards.some((c) => !c.h || c.pending)) fail(`${tag}: ${t}: cards missing or still pending`);
    else ok(`${tag}: ${t}: ${cards.length} column cards`);
    if (cards.some((c) => c.viz && !/\.$/.test(c.viz))) fail(`${tag}: ${t}: a mini chart without a sentence summary`);
    const head = await page.textContent(".xp-h");
    if (head !== t) fail(`${tag}: header shows ${head}, not ${t}`);
    const rows = await page.$$eval(".xp-rows .dp-table tbody tr", (r) => r.length);
    if (!rows) fail(`${tag}: ${t}: the paged rows are empty`);
  }
  // A newer pick supersedes a running one: the last table wins.
  await page.click(`.cat-table[data-fq$='"shop"']`);
  await page.click(`.cat-table[data-fq$='"sessions"']`);
  await settled(page);
  const won = await page.evaluate(() => [document.querySelector(".xp-h").textContent, [...document.querySelectorAll(".xp-card")].map((c) => c.dataset.col)]);
  if (won[0] !== "sessions" || won[1].includes("aov")) fail(`${tag}: a superseded pick painted its cards: ${JSON.stringify(won)}`);
  else ok(`${tag}: a second pick supersedes the first`);

  // Chart a column: a live preview with a summary, then "Add to dashboard".
  await pick(page, "shop");
  await page.click('.xp-card[data-col="orders"] .xp-chartbtn');
  await page.waitForSelector(".xp-chart-svg > svg[aria-label]", { timeout: 60000 });
  const label = await page.getAttribute(".xp-chart-svg > svg", "aria-label");
  if (!/^Histogram/.test(label)) fail(`${tag}: chart summary: ${label}`);
  for (const kind of await page.$$eval(".xp-chart select option", (o) => o.map((x) => x.value).filter((v) => !/^(day|channel|orders|aov)$/.test(v)))) {
    await page.selectOption(".xp-chart .xp-pick select >> nth=0", kind);
    await page.waitForFunction(() => document.querySelector(".xp-chart-msg")?.textContent !== "Drawing…", null, { timeout: 60000 });
    if (await page.$(".xp-chart .dp-err")) fail(`${tag}: chart kind ${kind} errors: ${await page.textContent(".xp-chart .dp-err")}`);
  }
  ok(`${tag}: chart this column (every kind it offers)`);
  if (shots) await page.screenshot({ path: path.join(shots, `explore-${width}-${dark ? "dark" : "light"}.png`), fullPage: false });
  const before = await page.$eval("#sql", (t) => t.value.length);
  await page.click(".xp-chart button:has-text('Add to dashboard')");
  const after = await page.$eval("#sql", (t) => t.value);
  if (after.length <= before || !/::(HISTOGRAM|BOXPLOT|BARCHART|SCATTER|LINECHART)/.test(after.slice(before))) fail(`${tag}: Add to dashboard did not append the chart`);
  else ok(`${tag}: Add to dashboard appends the statement`);

  // Queries: run, the row / time info, a click on a value adds a WHERE, history.
  await page.click('.xp-card[data-col="channel"] .xp-val >> nth=0');
  await page.waitForFunction(() => /rows? · /.test(document.getElementById("xq-info").textContent), null, { timeout: 60000 });
  const q = await page.$eval("#xq-sql", (t) => t.value);
  if (!/WHERE "channel" = '/.test(q)) fail(`${tag}: a value click wrote ${q}`);
  else ok(`${tag}: a click on a top value filters the query (${(await page.textContent("#xq-info")).trim()})`);
  // an error shows inline, with its position
  await page.fill("#xq-sql", "SELECT 1,\n  nosuchcolumn + 1 FROM shop");
  await page.focus("#xq-sql");
  await page.keyboard.press(process.platform === "darwin" ? "Meta+Enter" : "Control+Enter");
  await page.waitForSelector("#xq-err:not([hidden])", { timeout: 30000 });
  const err = await page.textContent("#xq-err");
  if (!/Line 2, column/.test(err)) fail(`${tag}: error without its position: ${err.slice(0, 200)}`);
  else ok(`${tag}: errors are inline with their position`);
  if (!(await page.$("#xq-hl .xq-errmark"))) fail(`${tag}: the error position is not marked in the editor`);
  const hist = await page.$$eval("#xq-hist option", (o) => o.length - 1);
  if (hist < 2) fail(`${tag}: history has ${hist} queries`);
  // ::ROLE completion
  await page.fill("#xq-sql", "SELECT 1::LINE");
  await page.focus("#xq-sql");
  await page.keyboard.press("End");
  await page.keyboard.type("C");
  await page.waitForSelector("#xq-ac:not([hidden])");
  await page.keyboard.press("Enter");
  const done = await page.$eval("#xq-sql", (t) => t.value);
  if (done !== "SELECT 1::LINECHART") fail(`${tag}: role completion wrote ${done}`);
  else ok(`${tag}: ::ROLE completion`);
  // results: downloads and "Chart this result"
  await page.fill("#xq-sql", "SELECT channel, sum(orders) AS orders FROM shop GROUP BY 1 ORDER BY 2 DESC");
  await page.click("#xq-run");
  await page.waitForSelector("#xq-tools button:has-text('CSV')", { timeout: 60000 });
  const [dl] = await Promise.all([page.waitForEvent("download"), page.click("#xq-tools button:has-text('CSV')")]);
  if (!/\.csv$/.test(dl.suggestedFilename())) fail(`${tag}: CSV download ${dl.suggestedFilename()}`);
  const [dlp] = await Promise.all([page.waitForEvent("download"), page.click("#xq-tools button:has-text('Parquet')")]);
  if (!/\.parquet$/.test(dlp.suggestedFilename())) fail(`${tag}: Parquet download ${dlp.suggestedFilename()}`);
  await page.click("#xq-tools button:has-text('Chart this result')");
  await page.waitForSelector("#xq-chart-slot .xp-chart-svg > svg[aria-label]", { timeout: 60000 });
  ok(`${tag}: result downloads and chart`);

  // A file: CSV dropped in → a view, profiled.
  const csv = path.join(os.tmpdir(), `explore-e2e-${process.pid}.csv`);
  writeFileSync(csv, "city,visits,day\nBerlin,12,2026-01-01\nHamburg,7,2026-01-02\nBerlin,3,2026-01-03\n");
  await page.setInputFiles("#cat-file-input", csv);
  await page.waitForFunction((v) => document.querySelector(".xp-h")?.textContent === v && document.querySelector(".xp-progress.done"), `explore_e2e_${process.pid}`, { timeout: 60000 });
  ok(`${tag}: an added CSV file is explored as a view`);

  // Perf: a 1 M-row table profiles within the bound (on a sample).
  if (width >= 600 && !dark) {
    await page.fill("#xq-sql", "CREATE OR REPLACE TABLE big AS SELECT i AS id, 'k' || (i % 50) AS cat, (i % 997) * 1.25 AS amount, DATE '2020-01-01' + (i % 2000)::INT AS day FROM range(1000000) t(i)");
    await page.click("#xq-run");
    await page.waitForSelector('.cat-table[data-fq$=\'"big"\']', { timeout: 120000 });
    const t0 = Date.now();
    await pick(page, "big");
    const ms = Date.now() - t0;
    const note = await page.textContent(".xp-progress");
    if (!/sample of 100,000 of 1\.0 M/.test(note)) fail(`perf: ${note}`);
    if (ms > perfMs) fail(`perf: profiling 1 M rows took ${ms} ms (bound ${perfMs} ms)`);
    else ok(`perf: profiled 1 M rows in ${ms} ms (${note})`);
  }

  // Likely joins: a key whose values match another table's.
  await page.fill("#xq-sql", "CREATE OR REPLACE TABLE customers AS SELECT i AS id, 'c' || i AS name FROM range(1, 51) t(i)");
  await page.click("#xq-run");
  await page.fill("#xq-sql", "CREATE OR REPLACE TABLE purchases AS SELECT i AS purchase_id, 1 + i % 50 AS customer_id, i * 2.5 AS total FROM range(500) t(i)");
  await page.click("#xq-run");
  await page.waitForSelector('.cat-table[data-fq$=\'"purchases"\']', { timeout: 60000 });
  await pick(page, "purchases");
  await page.waitForSelector(".xp-joins:not([hidden]) .xp-joinlist li", { timeout: 60000 });
  const join = await page.textContent(".xp-joinlist li .xp-join-txt");
  if (!/customer_id → customers\.id.*100%/.test(join)) fail(`${tag}: likely join: ${join}`);
  await page.click(".xp-joinlist li button");
  await page.waitForFunction(() => /^100 rows/.test(document.getElementById("xq-info").textContent), null, { timeout: 60000 });
  ok(`${tag}: likely joins (${join.replace(/\s+/g, " ").trim()})`);

  // Compare (#111): two tables by a key, then two time ranges of one.
  const sql = async (q) => {
    await page.fill("#xq-sql", q);
    await page.click("#xq-run");
    await page.waitForFunction(() => /rows? · /.test(document.getElementById("xq-info").textContent) || !document.getElementById("xq-err").hidden, null, { timeout: 60000 });
  };
  await sql("CREATE OR REPLACE TABLE cust_v1 AS SELECT i AS customer_id, 'n' || i AS name, ['DE','FR','IT'][1 + i % 3] AS country, i * 10 AS score FROM range(1, 301) t(i)");
  await sql("CREATE OR REPLACE TABLE cust_v2 AS SELECT i AS customer_id, CASE WHEN i % 25 = 0 THEN 'x' || i ELSE 'n' || i END AS name, ['DE','FR','IT'][1 + i % 3] AS country, (i * 10)::DOUBLE AS score, i % 2 = 0 AS active FROM range(20, 341) t(i)");
  await page.waitForSelector('.cat-table[data-fq$=\'"cust_v2"\']', { timeout: 60000 });
  await pick(page, "cust_v1");
  await page.click("button:has-text('Compare…')");
  await page.selectOption(".xp-cmp-form select", { label: "cust_v2" });
  await page.click(".xp-cmp-form button[type=submit]");
  await page.waitForSelector("table.xp-cmp", { timeout: 60000 });
  const sum = await page.textContent(".xp-cmp-sum");
  if (!/score retyped/.test(sum) || !/active added/.test(sum) || !/40 rows added, 19 removed, 12 changed/.test(sum)) fail(`${tag}: compare summary: ${sum}`);
  else ok(`${tag}: compare two tables (${sum.trim()})`);
  if (shots) await page.locator(".xp-cmp-panel").screenshot({ path: path.join(shots, `compare-${width}-${dark ? "dark" : "light"}.png`) });
  if (!(await page.$("table.xp-cmp caption")) || !(await page.$("table.xp-cmp svg.xp-ov[aria-label]"))) fail(`${tag}: compare table without a caption or overlay summaries`);
  await pick(page, "shop");
  await page.click("button:has-text('Compare…')");
  await page.check(".xp-cmp-mode input[value=range]");
  await page.click(".xp-cmp-form button[type=submit]");
  await page.waitForSelector("table.xp-cmp", { timeout: 60000 });
  ok(`${tag}: compare two time ranges (${(await page.textContent(".xp-cmp-panel .xp-progress")).trim()})`);
  await page.click("button:has-text('New dashboard from this comparison')");
  await page.waitForFunction(() => !document.body.classList.contains("mode-explore") && document.querySelector(".dash .panel") && !document.body.classList.contains("loading"), null, { timeout: 120000 });
  await page.waitForTimeout(500);
  const cbad = await page.$$eval(".dash .dp-errcard, .dash .dp-err", (e) => e.map((x) => x.textContent.slice(0, 160)));
  if (cbad.length) fail(`${tag}: comparison dashboard has panel errors: ${cbad.join(" | ")}`);
  else ok(`${tag}: comparison dashboard runs (${await page.$$eval(".dash .panel", (p) => p.length)} panels)`);
  await page.evaluate(() => document.getElementById("side-explore").click()); // (collapsed on a phone)
  await page.waitForSelector(".cat-table");

  // anofox_tabular (#111): with run_checks in the session (here a stand-in
  // macro with its contract), a hint runs its check and shows the result.
  await sql("CREATE OR REPLACE TABLE dupes AS SELECT i % 40 AS order_id, i AS v FROM range(100) t(i)");
  await sql(
    "CREATE OR REPLACE MACRO run_checks(t) AS TABLE SELECT check_name, check_type, 60.0::DOUBLE AS value, lower_threshold, upper_threshold, 'fail' AS status, 'stand-in' AS message FROM query_table(t)"
  );
  await page.waitForSelector('.cat-table[data-fq$=\'"dupes"\']', { timeout: 60000 });
  await pick(page, "dupes");
  await page.waitForSelector('.xp-card[data-col="order_id"] .xp-runcheck', { timeout: 60000 });
  await page.click('.xp-card[data-col="order_id"] .xp-runcheck');
  await page.waitForSelector('.xp-card[data-col="order_id"] .xp-hint-res', { timeout: 60000 });
  const res = await page.textContent('.xp-card[data-col="order_id"] .xp-hint-res');
  if (!/anofox_tabular duplicate_count: 60 — fail/.test(res)) fail(`${tag}: hint check: ${res}`);
  else ok(`${tag}: a hint runs its anofox_tabular check (${res})`);
  await sql("DROP MACRO TABLE run_checks");

  // A starter dashboard from the table runs with no panel errors.
  await pick(page, "shop");
  await page.click("button:has-text('New dashboard from this table')");
  await page.waitForFunction(() => !document.body.classList.contains("mode-explore") && document.querySelector(".dash .panel") && !document.body.classList.contains("loading"), null, { timeout: 120000 });
  await page.waitForTimeout(500);
  const bad = await page.$$eval(".dash .dp-errcard, .dash .dp-err", (e) => e.map((x) => x.textContent.slice(0, 160)));
  const panels = await page.$$eval(".dash .panel", (p) => p.length);
  if (bad.length) fail(`${tag}: starter dashboard has panel errors: ${bad.join(" | ")}`);
  else ok(`${tag}: starter dashboard runs (${panels} panels, no errors)`);
  if (!(await page.$(".dash .dp-period, .dash .controls"))) fail(`${tag}: starter dashboard has no filters`);
  if (shots) await page.screenshot({ path: path.join(shots, `starter-${width}-${dark ? "dark" : "light"}.png`), fullPage: false });
  noPageErrors(errs, tag);
  // The last picked table comes back (per viewer) in a new visit.
  const again = await ctx.newPage();
  await open(again, "Period & KPIs", true);
  await again.evaluate(() => document.getElementById("side-explore").click());
  await again.waitForFunction(() => document.querySelector(".xp-h")?.textContent === "shop", null, { timeout: 60000 }).then(
    () => ok(`${tag}: the last table is picked again`),
    () => fail(`${tag}: the last picked table is not remembered`)
  );
  await ctx.close();
}

for (const v of [{ width: 1400, dark: false }, { width: 1400, dark: true }, { width: 390, dark: false }]) await exploreRun(v);

// A locked (served) page never exposes Explore — not even when its button is
// clicked by script.
{
  const page = await browser.newPage({ viewport: { width: 1200, height: 800 } });
  await page.addInitScript(() => {
    window.__served = { id: "t", title: "Locked", sql: "SELECT 1::METRIC, 'One'::LABEL;", nav: [], locked: true };
  });
  await page.goto(base);
  await page.waitForFunction(() => document.body.classList.contains("embed"), null, { timeout: 60000 });
  await page.waitForTimeout(1000);
  const visible = await page.isVisible("#side-explore");
  await page.evaluate(() => document.getElementById("side-explore").click());
  await page.waitForTimeout(300);
  const st = await page.evaluate(() => ({ mode: document.body.classList.contains("mode-explore"), shown: getComputedStyle(document.getElementById("explore")).display !== "none" }));
  if (visible || st.mode || st.shown) fail(`locked page exposes Explore: ${JSON.stringify({ visible, ...st })}`);
  else ok("a locked page never exposes Explore");
  await page.close();
}

await browser.close();
console.log(failed ? `\n${failed} failure(s)` : "\nexplore e2e passed");
