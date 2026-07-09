// Browser dashboard builder — 100% client-side.
//   DuckDB-Wasm runs the SQL, duckplot (wasm) plans the ::ROLE annotations and
//   renders each panel to SVG. No server, no DuckDB extension.
import init, { plan, render_panel } from "./pkg/duckplot.js";

// Examples, grouped for the sidebar. Each entry is a full dashboard script.
const SESSIONS = `CREATE OR REPLACE TABLE sessions AS SELECT * FROM (VALUES
  ('W1','app',30),('W1','web',22),('W1','api',12),
  ('W2','app',41),('W2','web',28),('W2','api',15),
  ('W3','app',26),('W3','web',33),('W3','api', 9),
  ('W4','app',48),('W4','web',30),('W4','api',18)
) t(week, channel, n);`;
const SALES = `CREATE OR REPLACE TABLE sales AS SELECT * FROM (VALUES
  ('W1','app',30,1200.0),('W1','web',22,900.0),('W1','api',12,400.0),
  ('W2','app',41,1600.0),('W2','web',28,1100.0),('W2','api',15,520.0),
  ('W3','app',26,980.0),('W3','web',33,1300.0),('W3','api', 9,330.0),
  ('W4','app',48,2000.0),('W4','web',30,1250.0),('W4','api',18,640.0)
) t(week, channel, n, revenue);`;

const SAMPLE_GROUPS = [
  {
    group: "Start here",
    items: {
      Overview: `-- Annotate result columns with ::ROLE casts. Un-annotated statements
-- (this CREATE) are setup; annotated SELECTs become panels.
${SESSIONS}

SELECT 'Overview — click a pie slice or a table row to filter the KPIs'::LABEL;

-- KPIs in a ::GROUP render as a compact strip. They opt into the cross-filter
-- (getvariable('selected')), so clicking a channel re-computes them.
SELECT 'Key metrics'::GROUP;
SELECT sum(n)::COMPACT, 'Sessions'::LABEL FROM sessions WHERE getvariable('selected') IN ('', channel);
SELECT count(DISTINCT channel)::METRIC, 'Channels'::LABEL FROM sessions WHERE getvariable('selected') IN ('', channel);
SELECT round(avg(n),1)::METRIC, 'Avg / cell'::LABEL FROM sessions WHERE getvariable('selected') IN ('', channel);
SELECT 1::ENDGROUP;

SELECT 6::COL;
SELECT week::XAXIS, channel::CATEGORY, sum(n)::BARCHART_STACKED, 'Sessions by channel'::TITLE
FROM sessions GROUP BY ALL ORDER BY week, channel;

SELECT 6::COL;
SELECT week::XAXIS, channel::CATEGORY, sum(n)::LINECHART, 'Weekly trend'::TITLE
FROM sessions GROUP BY ALL ORDER BY week, channel;

SELECT 6::COL;
SELECT channel::CATEGORY, sum(n)::PIE, 'Share'::TITLE FROM sessions GROUP BY ALL;

SELECT 6::COL;
SELECT 'Detail'::TITLE, channel, week, sum(n) AS n ::TABLE
FROM sessions GROUP BY ALL ORDER BY channel, week;`,

      "Signal explorer": `-- Everything below is generated on the fly with range() — no table needed.
SELECT 'Signal explorer — generated with range()'::LABEL;

SELECT 'Signal stats'::GROUP;
SELECT round(avg(sin(i/6.0)*40+50),1)::METRIC, 'Mean'::LABEL FROM range(0,120) t(i);
SELECT round(max(sin(i/6.0)*40+50),1)::METRIC, 'Peak'::LABEL FROM range(0,120) t(i);
SELECT round(stddev(sin(i/6.0)*40+50),1)::METRIC, 'Std dev'::LABEL FROM range(0,120) t(i);
SELECT 1::ENDGROUP;

SELECT 8::COL;
SELECT i::XAXIS,
       (50 + i*0.4 + sin(i/6.0)*22)::LINECHART,
       (50 + i*0.4 + sin(i/6.0)*22 - 10)::BAND_LOWER,
       (50 + i*0.4 + sin(i/6.0)*22 + 10)::BAND_UPPER,
       'Trend + seasonality (± band)'::TITLE
FROM range(0, 80) t(i);

SELECT 4::COL;
SELECT round(100.0*count(*) FILTER (WHERE sin(i/6.0) > 0)/count(*),0)::GAUGE, '0,100'::RANGE,
       '#e03131,#efc94c,#0ca678'::COLORS, 'Time above mean'::TITLE
FROM range(0,120) t(i);

SELECT 6::COL;
SELECT i::XAXIS, (sin(i/5.0)*25 + 50 + ((i*13) % 20))::SCATTER, 'Noisy samples'::TITLE
FROM range(0, 120) t(i);

SELECT 6::COL;
SELECT ((i*7) % 100)::HISTOGRAM, 'Value distribution'::TITLE FROM range(0, 500) t(i);

SELECT 12::COL;
SELECT (i % 12)::XAXIS, floor(i/12.0)::YAXIS,
       round(avg(sin(i/6.0)*cos(floor(i/12.0)/3.0)*40 + 50),0)::HEATMAP, 'sin · cos surface'::TITLE
FROM range(0, 96) t(i) GROUP BY 1,2 ORDER BY 1,2;`,
    },
  },

  {
    group: "Charts",
    items: {
      "Chart gallery": `${SALES}
CREATE OR REPLACE TABLE m AS
SELECT i AS id, (i * 7 % 13) + 5 AS value, ['app','web','api'][(i % 3) + 1] AS channel, 'W' || ((i % 4) + 1) AS week
FROM range(0, 120) t(i);

SELECT 'Chart gallery — every chart kind, in tabs'::LABEL;

SELECT 'Bar & line'::TAB;
SELECT 6::COL; SELECT week::XAXIS, channel::CATEGORY, sum(revenue)::BARCHART_STACKED, 'Revenue'::TITLE FROM sales GROUP BY ALL ORDER BY week, channel;
SELECT 6::COL; SELECT week::XAXIS, channel::CATEGORY, sum(n)::LINECHART, 'Sessions'::TITLE FROM sales GROUP BY ALL ORDER BY week, channel;

SELECT 'Pie / donut / gauge'::TAB;
SELECT 4::COL; SELECT channel::CATEGORY, sum(n)::PIE, 'Pie'::TITLE FROM sales GROUP BY ALL;
SELECT 4::COL; SELECT channel::CATEGORY, sum(revenue)::DONUTCHART, 'Donut'::TITLE FROM sales GROUP BY ALL;
SELECT 4::COL; SELECT sum(n) FILTER (WHERE week='W4')::GAUGE, '0,120'::RANGE, '#e03131,#efc94c,#0ca678'::COLORS, 'Gauge'::TITLE FROM sales;

SELECT 'Distributions'::TAB;
SELECT 6::COL; SELECT value::HISTOGRAM, 'Histogram'::TITLE FROM m;
SELECT 6::COL; SELECT channel::XAXIS, value::BOXPLOT, 'Box plot'::TITLE FROM m;
SELECT 12::COL; SELECT week::XAXIS, channel::YAXIS, round(avg(value),1)::HEATMAP, 'Heatmap'::TITLE FROM m GROUP BY ALL ORDER BY week, channel;

SELECT 'Combo & sparkline'::TAB;
SELECT 8::COL; SELECT week::XAXIS, sum(n)::BARCHART, sum(revenue)/50::LINECHART, 35::REFLINE, 'Sessions vs revenue'::TITLE FROM sales GROUP BY ALL ORDER BY week;
SELECT 4::COL; SELECT sum(revenue)::SPARKLINE, 'Revenue trend'::TITLE FROM sales GROUP BY week ORDER BY week;

SELECT 'Map'::TAB;
SELECT 12::COL;
SELECT geom::MAP, value::BARCHART, name::LABEL, 'Regions by value (WKT choropleth)'::TITLE FROM (VALUES
  ('North','POLYGON((0 2, 4 2, 4 4, 0 4, 0 2))', 40),
  ('South-west','POLYGON((0 0, 2 0, 2 2, 0 2, 0 0))', 75),
  ('South-east','POLYGON((2 0, 4 0, 4 2, 2 2, 2 0))', 20)
) r(name, geom, value);`,
    },
  },

  {
    group: "Interactivity",
    items: {
      "Filters & inputs": `CREATE OR REPLACE TABLE events AS SELECT * FROM (VALUES
  (DATE '2024-01-05','app','EU','launch',30),(DATE '2024-01-12','web','EU','promo',22),(DATE '2024-01-20','api','US','launch',12),
  (DATE '2024-02-03','app','US','promo',41),(DATE '2024-02-14','web','EU','launch',28),(DATE '2024-02-22','api','EU','promo',15),
  (DATE '2024-03-02','app','EU','promo',26),(DATE '2024-03-11','web','US','launch',33),(DATE '2024-03-19','api','US','promo', 9),
  (DATE '2024-03-28','app','US','launch',48)
) t(day, channel, region, note, n);

SELECT 'Filters & inputs — every input type'::LABEL;

-- the output COLUMN NAME becomes the DuckDB variable (getvariable('name'))
SELECT 'Filters'::GROUP;
SELECT DISTINCT region::DROPDOWN FROM events ORDER BY region;      -- single-select
SELECT DISTINCT channel::MULTISELECT FROM events ORDER BY channel; -- multi-select (a list)
SELECT 5 AS min_n ::NUMBER;                                        -- number
SELECT '' AS note ::TEXT;                                          -- free text (try 'promo')
SELECT DATE '2024-01-01' AS since ::DATE;                          -- single date
SELECT 1::ENDGROUP;

SELECT 'Date range'::GROUP;
SELECT min(day) AS from_day, max(day) AS to_day ::DATERANGE FROM events;  -- from → to
SELECT 1::ENDGROUP;

SELECT 4::COL;
SELECT sum(n)::METRIC, 'Sessions (filtered)'::LABEL FROM events
WHERE region = getvariable('region')
  AND list_contains(getvariable('channel'), channel)
  AND n >= getvariable('min_n')
  AND day >= getvariable('since')::DATE
  AND day BETWEEN getvariable('from_day')::DATE AND getvariable('to_day')::DATE
  AND (getvariable('note') = '' OR note ILIKE '%' || getvariable('note') || '%');

SELECT 8::COL;
SELECT day::XAXIS, channel::CATEGORY, sum(n)::BARCHART_STACKED, 'Sessions (all filters applied)'::TITLE
FROM events
WHERE region = getvariable('region')
  AND list_contains(getvariable('channel'), channel)
  AND n >= getvariable('min_n')
  AND day >= getvariable('since')::DATE
  AND day BETWEEN getvariable('from_day')::DATE AND getvariable('to_day')::DATE
  AND (getvariable('note') = '' OR note ILIKE '%' || getvariable('note') || '%')
GROUP BY ALL ORDER BY day, channel;`,

      "Cross-filter & drill-down": `-- Two tables, two INDEPENDENT named cross-filters. Each table emits a variable
-- named after its first column (sku / region); the panels filter by both.
CREATE OR REPLACE TABLE sales2 AS
SELECT sku, region, month, (abs(hash(sku || region || month)) % 80 + 40) AS amount
FROM (VALUES ('SKU-A'),('SKU-B'),('SKU-C')) a(sku),
     (VALUES ('EU'),('US')) b(region),
     (VALUES ('2024-01'),('2024-02'),('2024-03'),('2024-04'),('2024-05'),('2024-06')) c(month);

SELECT 'Click a SKU and a region — the KPI and chart filter by both'::LABEL;

SELECT 4::COL; SELECT sku, sum(amount) AS total ::TABLE FROM sales2 GROUP BY sku ORDER BY total DESC;
SELECT 4::COL; SELECT region, sum(amount) AS total ::TABLE FROM sales2 GROUP BY region ORDER BY total DESC;

SELECT 4::COL;
SELECT sum(amount)::METRIC, 'Total (filtered)'::LABEL FROM sales2
WHERE (COALESCE(getvariable('sku'),'') = '' OR sku = getvariable('sku'))
  AND (COALESCE(getvariable('region'),'') = '' OR region = getvariable('region'));

SELECT 12::COL;
SELECT month::XAXIS, sum(amount)::LINECHART, 'Monthly (by SKU & region)'::TITLE FROM sales2
WHERE (COALESCE(getvariable('sku'),'') = '' OR sku = getvariable('sku'))
  AND (COALESCE(getvariable('region'),'') = '' OR region = getvariable('region'))
GROUP BY month ORDER BY month;`,

      "Date range": `CREATE OR REPLACE TABLE events AS SELECT * FROM (VALUES
  (DATE '2024-01-03','app',30),(DATE '2024-01-10','web',22),(DATE '2024-01-18','app',41),
  (DATE '2024-01-27','api',28),(DATE '2024-02-04','web',26),(DATE '2024-02-13','app',33),
  (DATE '2024-02-21','api',48),(DATE '2024-02-28','web',30),(DATE '2024-03-07','app',37)
) t(day, channel, n);

SELECT 'Date range'::GROUP;
SELECT min(day) AS from_day, max(day) AS to_day ::DATERANGE FROM events;
SELECT 1::ENDGROUP;

SELECT 'Sessions in the selected range'::LABEL;
SELECT 4::COL;
SELECT sum(n)::METRIC, 'Total sessions'::LABEL FROM events
WHERE day BETWEEN getvariable('from_day')::DATE AND getvariable('to_day')::DATE;
SELECT 8::COL;
SELECT day::XAXIS, channel::CATEGORY, sum(n)::BARCHART_STACKED, 'By day'::TITLE FROM events
WHERE day BETWEEN getvariable('from_day')::DATE AND getvariable('to_day')::DATE
GROUP BY ALL ORDER BY day, channel;`,
    },
  },

  {
    group: "Tables",
    items: {
      "Formatted table": `CREATE OR REPLACE TABLE fc AS SELECT * FROM (VALUES
  ('SKU-A',12400, 4.2,'on track',  8.5),
  ('SKU-B', 7300,11.8,'at risk',  -3.1),
  ('SKU-C',21850, 6.5,'on track',  2.7),
  ('SKU-D', 4200,23.4,'breach',  -12.0)
) t(sku, forecast, mape, status, growth);
CREATE OR REPLACE TABLE hist AS SELECT * FROM (VALUES
  ('SKU-A',1,100),('SKU-A',2,108),('SKU-A',3,104),('SKU-A',4,120),('SKU-A',5,126),
  ('SKU-B',1, 90),('SKU-B',2, 85),('SKU-B',3, 70),('SKU-B',4, 72),('SKU-B',5, 66),
  ('SKU-C',1,200),('SKU-C',2,205),('SKU-C',3,210),('SKU-C',4,208),('SKU-C',5,215),
  ('SKU-D',1, 60),('SKU-D',2, 52),('SKU-D',3, 48),('SKU-D',4, 40),('SKU-D',5, 35)
) t(sku, m, sales);

SELECT 'Per-column formatting: MONEY / COLORSCALE / BADGE / TREND / SPARKLINE'::LABEL;
SELECT 12::COL;
SELECT sku::TABLE,
       forecast::MONEY,
       mape AS "MAPE %" ::COLORSCALE,
       status::BADGE,
       growth AS "growth %" ::TREND,
       (SELECT list(sales ORDER BY m) FROM hist WHERE hist.sku = fc.sku) AS trend ::SPARKLINE
FROM fc ORDER BY forecast DESC;`,

      "Large tables": `SELECT 'Large tables — client pagination vs SQL-paged (in tabs)'::LABEL;

SELECT '1,000 rows (client)'::TAB;
SELECT 12::COL;
SELECT 'ID-' || lpad(i::VARCHAR, 4, '0') AS id, ['app','web','api','cli'][(i % 4) + 1] AS channel,
       ((i * 37) % 100) AS score, ((i * 7) % 500) AS events ::TABLE
FROM range(1, 1001) t(i) ORDER BY i;

SELECT '100,000 rows (::PAGED)'::TAB;
SELECT 12::COL;
-- ::PAGED runs LIMIT/OFFSET + COUNT in DuckDB — one page at a time, so it scales
-- to huge / remote tables (parquet in S3, MotherDuck). Sorting is server-side.
SELECT 'ID-' || lpad(i::VARCHAR, 6, '0') AS id, ['app','web','api','cli'][(i % 4) + 1] AS channel,
       ((i * 37) % 1000) AS score, ((i * 91) % 100) AS load_pct ::PAGED
FROM range(1, 100001) t(i);`,
    },
  },

  {
    group: "Layout",
    items: {
      "Groups, tabs & height": `${SALES}

-- KPIs in a ::GROUP box (compact strip)
SELECT 'Key metrics'::GROUP;
SELECT sum(revenue)::MONEY, 'Revenue'::LABEL FROM sales;
SELECT sum(n)::COMPACT, 'Sessions'::LABEL FROM sales;
SELECT count(DISTINCT week)::METRIC, 'Weeks'::LABEL FROM sales;
SELECT 1::ENDGROUP;

SELECT 1::COLUMNS;

-- top-level ::TAB, each with nested ::SUBTAB; ::HEIGHT sets a taller box
SELECT 'Revenue'::TAB;
SELECT 'By week'::SUBTAB;
SELECT 420::HEIGHT;
SELECT week::XAXIS, channel::CATEGORY, sum(revenue)::BARCHART_STACKED, 'Revenue by week (tall)'::TITLE
FROM sales GROUP BY ALL ORDER BY week, channel;
SELECT 'By region'::SUBTAB;
SELECT channel::XAXIS, sum(revenue)::BARCHART, 'Revenue by channel'::TITLE FROM sales GROUP BY ALL ORDER BY channel;

SELECT 'Sessions'::TAB;
SELECT 'Trend'::SUBTAB;
SELECT week::XAXIS, channel::CATEGORY, sum(n)::LINECHART, 'Sessions trend'::TITLE FROM sales GROUP BY ALL ORDER BY week, channel;
SELECT 'Share'::SUBTAB;
SELECT channel::CATEGORY, sum(n)::PIE, 'Session share'::TITLE FROM sales GROUP BY ALL;`,
    },
  },
];

const SAMPLES = Object.fromEntries(SAMPLE_GROUPS.flatMap((g) => Object.entries(g.items)));

const $ = (id) => document.getElementById(id);
const status = (t) => ($("status").textContent = t);

// SQL syntax highlighting for the editor overlay (no dependency).
const SQL_KW =
  /^(SELECT|FROM|WHERE|GROUP|ORDER|BY|HAVING|LIMIT|OFFSET|AS|AND|OR|NOT|IN|IS|NULL|LIKE|ILIKE|BETWEEN|CASE|WHEN|THEN|ELSE|END|JOIN|LEFT|RIGHT|INNER|OUTER|FULL|CROSS|ON|USING|UNION|EXCEPT|INTERSECT|ALL|DISTINCT|CREATE|REPLACE|TEMP|TEMPORARY|TABLE|VIEW|IF|EXISTS|INSERT|INTO|VALUES|UPDATE|DELETE|SET|VARIABLE|WITH|DESC|ASC|OVER|PARTITION|FILTER|CAST|COALESCE|NULLIF|COUNT|SUM|AVG|MIN|MAX|ROUND|ABS|FLOOR|CEIL|LENGTH|LOWER|UPPER|SUBSTR|LPAD|LIST|RANGE|GETVARIABLE|LIST_CONTAINS|INSTALL|LOAD|ATTACH|PRAGMA|SUMMARIZE|DESCRIBE|SHOW|CALL)$/i;
function highlightSQL(code) {
  const esc = (s) => s.replace(/[&<>]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;" })[c]);
  const token = /--[^\n]*|'(?:[^']|'')*'|::\w+|\b\d+(?:\.\d+)?\b|[A-Za-z_]\w*|\s+|[^\sA-Za-z0-9_']+/g;
  let out = "",
    m;
  while ((m = token.exec(code))) {
    const t = m[0];
    if (t.startsWith("--")) out += `<span class="com">${esc(t)}</span>`;
    else if (t[0] === "'") out += `<span class="str">${esc(t)}</span>`;
    else if (t.startsWith("::")) out += `<span class="role">${esc(t)}</span>`;
    else if (/^\d/.test(t)) out += `<span class="num">${esc(t)}</span>`;
    else if (SQL_KW.test(t)) out += `<span class="kw">${esc(t)}</span>`;
    else out += esc(t);
  }
  return out + "\n";
}
function syncHL() {
  const hl = $("hl");
  if (!hl) return;
  hl.innerHTML = highlightSQL($("sql").value);
  hl.scrollTop = $("sql").scrollTop;
  hl.scrollLeft = $("sql").scrollLeft;
}

let backend = "wasm"; // "live" (HTTP /query) or "wasm" (DuckDB-Wasm)
let conn = null;

// Run one SQL statement and return its rows as a JSON string ([{c0,…}, …]).
async function runSql(sql) {
  if (backend === "live") {
    const r = await fetch("/query", { method: "POST", body: sql });
    if (!r.ok) throw new Error(await r.text());
    return (await r.text()) || "[]";
  }
  const res = await conn.query(sql);
  // DuckDB-Wasm returns DATE/TIMESTAMP as epoch numbers; convert those columns
  // back to ISO strings so date variables and date axes read as YYYY-MM-DD.
  const dateCols = new Map(); // name -> "date" | "time"
  const decCols = new Map(); // name -> scale (DECIMAL comes back as the unscaled mantissa)
  try {
    for (const f of res.schema.fields) {
      const t = String(f.type);
      if (/date/i.test(t)) dateCols.set(f.name, "date");
      else if (/timestamp/i.test(t)) dateCols.set(f.name, "time");
      else if (/decimal/i.test(t)) decCols.set(f.name, Number(f.type.scale) || 0);
    }
  } catch (_) {}
  const rows = res.toArray().map((row) => {
    const o = row.toJSON();
    for (const [c, kind] of dateCols) {
      if (o[c] != null) o[c] = toIso(o[c], kind === "date");
    }
    for (const [c, scale] of decCols) {
      if (o[c] != null && scale > 0) o[c] = Number(o[c]) / 10 ** scale;
    }
    return o;
  });
  return JSON.stringify(rows, (_, v) => (typeof v === "bigint" ? Number(v) : v));
}

// Normalise a DuckDB-Wasm date/time value (Date, or epoch as days/ms/µs) to an
// ISO string — YYYY-MM-DD for dates, "YYYY-MM-DD HH:MM:SS" for timestamps.
function toIso(v, dateOnly) {
  let d;
  if (v instanceof Date) d = v;
  else {
    let n = Number(v);
    if (!isFinite(n)) return String(v);
    if (Math.abs(n) < 1e11) n *= 86400000; // days -> ms
    else if (Math.abs(n) > 1e14) n = Math.round(n / 1000); // µs -> ms
    d = new Date(n);
  }
  if (isNaN(d.getTime())) return String(v);
  const iso = d.toISOString();
  return dateOnly ? iso.slice(0, 10) : iso.replace("T", " ").slice(0, 19);
}

async function boot() {
  await init(); // duckplot wasm (plan + render_panel — used in both modes)

  // Prefer a live DuckDB bridge (served by `duckplot serve`); else DuckDB-Wasm.
  try {
    const r = await fetch("/query", { method: "POST", body: "SELECT 1 AS ok" });
    if (r.ok) backend = "live";
  } catch (_) {}

  if (backend !== "live") {
    const duckdb = await import("https://cdn.jsdelivr.net/npm/@duckdb/duckdb-wasm@1.29.0/+esm");
    const bundle = await duckdb.selectBundle(duckdb.getJsDelivrBundles());
    const workerUrl = URL.createObjectURL(
      new Blob([`importScripts("${bundle.mainWorker}");`], { type: "text/javascript" })
    );
    const db = new duckdb.AsyncDuckDB(new duckdb.ConsoleLogger(), new Worker(workerUrl));
    await db.instantiate(bundle.mainModule, bundle.pthreadWorker);
    conn = await db.connect();
  }

  // Sidebar (dashboard list) + app-shell controls
  renderSidebar();
  $("side-new").onclick = () => loadDash("", "");
  $("save").onclick = saveDash;
  $("side-toggle").onclick = () => document.body.classList.toggle("side-collapsed");
  $("mode-edit").onclick = () => setMode("edit");
  $("mode-view").onclick = () => setMode("view");
  $("side-explore").onclick = () => setMode(bodyMode() === "explore" ? "edit" : "explore");
  $("cat-refresh").onclick = loadCatalog;
  $("cat-search").oninput = () => filterCatalog($("cat-search").value);
  $("xq-run").onclick = runExplore;
  $("xq-sql").addEventListener("keydown", (e) => {
    if ((e.metaKey || e.ctrlKey) && e.key === "Enter") {
      e.preventDefault();
      runExplore();
    }
  });
  // URL params — for embedding / theming:
  //   ?embed=1        hide the header + sidebar + toolbar (dashboard only)
  //   ?primary=RRGGBB brand accent colour (UI + chart primary)
  //   ?dashboard=Name load a saved/example dashboard by name
  //   #sql=<base64>   inline SQL (from Share)
  const params = new URLSearchParams(location.search);
  const primary = params.get("primary");
  if (primary && /^#?[0-9a-fA-F]{6}$/.test(primary)) {
    const hex = primary.replace(/^#/, "");
    document.documentElement.style.setProperty("--accent", "#" + hex);
    document.documentElement.style.setProperty("--accent2", "#" + hex);
    dpPrimary = hex; // chart primary colour (passed to the wasm renderer)
  }
  if (params.get("embed") === "1" || params.has("embed")) {
    document.body.classList.add("embed");
  }
  const hashSql = decodeHashSql();
  const wantDash = params.get("dashboard");
  const savedItems = dashStore().items;
  if (hashSql) {
    $("sql").value = hashSql;
    $("dash-name").value = "Shared";
  } else if (wantDash && (SAMPLES[wantDash] || savedItems[wantDash])) {
    curDash = wantDash;
    $("sql").value = SAMPLES[wantDash] || savedItems[wantDash].sql;
    $("dash-name").value = wantDash;
    markActive();
  } else {
    const first = Object.keys(SAMPLES)[0];
    curDash = first;
    $("sql").value = SAMPLES[first];
    $("dash-name").value = first;
    markActive();
  }
  $("share").onclick = shareLink;
  $("dlhtml").onclick = downloadHtml;
  $("dark").onclick = () => document.body.classList.toggle("dark");
  $("refresh").onchange = () => {
    clearInterval(dpTimer);
    const s = parseInt($("refresh").value);
    if (s > 0) dpTimer = setInterval(run, s * 1000);
  };


  // MotherDuck connect dialog + auto-connect from a stored token.
  $("md").onclick = mdOpen;
  $("md-cancel").onclick = () => ($("md-dialog").hidden = true);
  $("md-connect").onclick = mdDoConnect;
  $("md-disconnect").onclick = mdDisconnect;
  const saved = mdSaved();
  if (saved && saved.token) {
    try {
      await mdConnect(saved.token, saved.db);
      mdMark(true, saved.db);
    } catch (e) {
      mdMark(false);
    }
  }

  $("run").disabled = false;
  $("run").onclick = () => run();

  // SQL editor: highlight overlay + scroll sync
  $("sql").addEventListener("input", syncHL);
  $("sql").addEventListener("scroll", () => {
    $("hl").scrollTop = $("sql").scrollTop;
    $("hl").scrollLeft = $("sql").scrollLeft;
  });
  syncHL();
  // Movable editor / dashboard boundary (persisted)
  let ew = parseInt(localStorage.getItem("dp_editor_w") || "400");
  document.documentElement.style.setProperty("--editor-w", ew + "px");
  $("splitter").addEventListener("mousedown", (e) => {
    e.preventDefault();
    $("splitter").classList.add("dragging");
    const startX = e.clientX,
      startW = ew;
    const move = (ev) => {
      ew = Math.max(260, Math.min(920, startW + ev.clientX - startX));
      document.documentElement.style.setProperty("--editor-w", ew + "px");
    };
    const up = () => {
      $("splitter").classList.remove("dragging");
      localStorage.setItem("dp_editor_w", ew);
      document.removeEventListener("mousemove", move);
      document.removeEventListener("mouseup", up);
    };
    document.addEventListener("mousemove", move);
    document.addEventListener("mouseup", up);
  });

  status(backend === "live" ? "live DuckDB · ready" : "DuckDB-Wasm · ready");
  run();
}

// ---------- app shell: sidebar, dashboards, modes, data exploration ----------
const DASH_KEY = "dp_dashboards";
let curDash = null;
// Store: { items: { name: {sql, group} }, groups: [names], collapsed: {group:bool} }.
function dashStore() {
  let raw;
  try {
    raw = JSON.parse(localStorage.getItem(DASH_KEY) || "null");
  } catch (_) {
    raw = null;
  }
  if (!raw) return { items: {}, groups: [], collapsed: {} };
  if (!raw.items) {
    // migrate the old flat { name: sql } shape
    const items = {};
    for (const [n, sql] of Object.entries(raw)) if (typeof sql === "string") items[n] = { sql, group: "" };
    return { items, groups: [], collapsed: {} };
  }
  raw.items ||= {};
  raw.groups ||= [];
  raw.collapsed ||= {};
  return raw;
}
const dashSaveStore = (s) => localStorage.setItem(DASH_KEY, JSON.stringify(s));

function renderSidebar() {
  const nav = $("side-nav");
  nav.innerHTML = "";
  const store = dashStore();
  const names = Object.keys(store.items);

  // "My dashboards" header with a + group button (drop here to ungroup)
  const hdr = document.createElement("div");
  hdr.className = "side-section side-section-row";
  const lbl = document.createElement("span");
  lbl.textContent = "My dashboards";
  const add = document.createElement("button");
  add.className = "side-mini";
  add.textContent = "+ group";
  add.title = "new group";
  add.onclick = createGroup;
  hdr.append(lbl, add);
  hdr.ondragover = (e) => {
    e.preventDefault();
    hdr.classList.add("drop");
  };
  hdr.ondragleave = () => hdr.classList.remove("drop");
  hdr.ondrop = (e) => {
    e.preventDefault();
    hdr.classList.remove("drop");
    moveDash(e.dataTransfer.getData("text/plain"), "");
  };
  nav.appendChild(hdr);

  // bucket dashboards by group
  const byGroup = {};
  for (const g of store.groups) byGroup[g] = [];
  const ungrouped = [];
  for (const n of names) {
    const g = store.items[n].group;
    if (g && byGroup[g]) byGroup[g].push(n);
    else ungrouped.push(n);
  }
  for (const g of store.groups) {
    nav.appendChild(groupHeader(g, store));
    if (!store.collapsed[g])
      for (const n of byGroup[g]) {
        const it = sideItem(n, store.items[n].sql, true);
        it.classList.add("in-group");
        nav.appendChild(it);
      }
  }
  for (const n of ungrouped) nav.appendChild(sideItem(n, store.items[n].sql, true));

  // Examples — visually separated from your dashboards, collectively collapsible.
  const exc = exCollapsed();
  const master = document.createElement("div");
  master.className = "side-section side-section-toggle side-master";
  const mc = document.createElement("span");
  mc.className = "ex-caret";
  mc.textContent = exc.__all__ ? "▸" : "▾";
  const ml = document.createElement("span");
  ml.textContent = "Examples";
  master.append(mc, ml);
  master.onclick = () => toggleEx("__all__");
  nav.appendChild(master);
  if (!exc.__all__) {
    for (const g of SAMPLE_GROUPS) {
      const hdr = document.createElement("div");
      hdr.className = "side-section side-section-toggle side-sub";
      const caret = document.createElement("span");
      caret.className = "ex-caret";
      caret.textContent = exc[g.group] ? "▸" : "▾";
      const lbl = document.createElement("span");
      lbl.textContent = g.group;
      hdr.append(caret, lbl);
      hdr.onclick = () => toggleEx(g.group);
      nav.appendChild(hdr);
      if (!exc[g.group]) for (const [n, sql] of Object.entries(g.items)) nav.appendChild(sideItem(n, sql, false));
    }
  }
  markActive();
}
const EXC_KEY = "dp_ex_collapsed";
function exCollapsed() {
  try {
    return JSON.parse(localStorage.getItem(EXC_KEY) || "{}");
  } catch (_) {
    return {};
  }
}
function toggleEx(group) {
  const c = exCollapsed();
  c[group] = !c[group];
  localStorage.setItem(EXC_KEY, JSON.stringify(c));
  renderSidebar();
}
function sideSection(t) {
  const d = document.createElement("div");
  d.className = "side-section";
  d.textContent = t;
  return d;
}
function groupHeader(g, store) {
  const h = document.createElement("div");
  h.className = "side-group" + (store.collapsed[g] ? " collapsed" : "");
  const caret = document.createElement("span");
  caret.className = "g-caret";
  caret.textContent = store.collapsed[g] ? "▸" : "▾";
  const nm = document.createElement("span");
  nm.className = "g-name";
  nm.textContent = g;
  const ren = document.createElement("button");
  ren.className = "g-btn";
  ren.textContent = "✎";
  ren.title = "rename group";
  const del = document.createElement("button");
  del.className = "g-btn";
  del.textContent = "✕";
  del.title = "delete group";
  h.append(caret, nm, ren, del);
  h.onclick = (e) => {
    if (e.target === ren || e.target === del) return;
    const s = dashStore();
    s.collapsed[g] = !s.collapsed[g];
    dashSaveStore(s);
    renderSidebar();
  };
  ren.onclick = (e) => {
    e.stopPropagation();
    renameGroup(g);
  };
  del.onclick = (e) => {
    e.stopPropagation();
    deleteGroup(g);
  };
  h.ondragover = (e) => {
    e.preventDefault();
    h.classList.add("drop");
  };
  h.ondragleave = () => h.classList.remove("drop");
  h.ondrop = (e) => {
    e.preventDefault();
    h.classList.remove("drop");
    moveDash(e.dataTransfer.getData("text/plain"), g);
  };
  return h;
}
function sideItem(name, sql, deletable) {
  const b = document.createElement("button");
  b.className = "side-item";
  b.dataset.name = name;
  if (deletable) {
    b.draggable = true;
    b.ondragstart = (e) => {
      e.dataTransfer.setData("text/plain", name);
      e.dataTransfer.effectAllowed = "move";
    };
  }
  const s = document.createElement("span");
  s.className = "s-name";
  s.textContent = name;
  b.appendChild(s);
  if (deletable) {
    const del = document.createElement("span");
    del.className = "s-del";
    del.textContent = "✕";
    del.title = "delete";
    del.onclick = (e) => {
      e.stopPropagation();
      delDash(name);
    };
    b.appendChild(del);
  }
  b.onclick = () => loadDash(name, sql);
  return b;
}
function markActive() {
  document.querySelectorAll(".side-item").forEach((el) => el.classList.toggle("active", el.dataset.name === curDash));
}
function createGroup() {
  const g = (prompt("New group name") || "").trim();
  if (!g) return;
  const s = dashStore();
  if (!s.groups.includes(g)) s.groups.push(g);
  dashSaveStore(s);
  renderSidebar();
}
function renameGroup(old) {
  const g = (prompt("Rename group", old) || "").trim();
  if (!g || g === old) return;
  const s = dashStore();
  s.groups = s.groups.map((x) => (x === old ? g : x));
  for (const n in s.items) if (s.items[n].group === old) s.items[n].group = g;
  if (s.collapsed[old] !== undefined) {
    s.collapsed[g] = s.collapsed[old];
    delete s.collapsed[old];
  }
  dashSaveStore(s);
  renderSidebar();
}
function deleteGroup(g) {
  const s = dashStore();
  s.groups = s.groups.filter((x) => x !== g);
  for (const n in s.items) if (s.items[n].group === g) s.items[n].group = "";
  delete s.collapsed[g];
  dashSaveStore(s);
  renderSidebar();
}
function moveDash(name, group) {
  const s = dashStore();
  if (s.items[name]) {
    s.items[name].group = group;
    dashSaveStore(s);
    renderSidebar();
  }
}
function loadDash(name, sql) {
  curDash = name || null;
  // A different dashboard starts clean — don't carry tab/filter/page state over.
  dpTab = null;
  dpSubTab = {};
  dpFilter = "";
  dpSelected = null;
  dpXf = {};
  dpPage = {};
  dpSort = {};
  $("sql").value = sql;
  $("dash-name").value = name || "";
  syncHL();
  if (bodyMode() === "explore") setMode("edit");
  markActive();
  run();
}
function saveDash() {
  const name = ($("dash-name").value || "").trim();
  if (!name) {
    $("dash-name").focus();
    return status("name it first");
  }
  const s = dashStore();
  const group = s.items[name] ? s.items[name].group : "";
  s.items[name] = { sql: $("sql").value, group };
  dashSaveStore(s);
  curDash = name;
  renderSidebar();
  status("saved ✓");
}
function delDash(name) {
  const s = dashStore();
  delete s.items[name];
  dashSaveStore(s);
  if (curDash === name) curDash = null;
  renderSidebar();
}

// ---- Edit / View / Explore modes ----
function bodyMode() {
  const c = document.body.classList;
  return c.contains("mode-explore") ? "explore" : c.contains("mode-view") ? "view" : "edit";
}
function setMode(m) {
  document.body.classList.remove("mode-view", "mode-explore");
  if (m === "view") document.body.classList.add("mode-view");
  else if (m === "explore") document.body.classList.add("mode-explore");
  $("mode-edit").classList.toggle("active", m === "edit");
  $("mode-view").classList.toggle("active", m === "view");
  $("side-explore").classList.toggle("active", m === "explore");
  if (m === "explore") loadCatalog();
}

// ---- Data exploration: catalog browser + table preview + column stats ----
const qid = (s) => `"${String(s).replace(/"/g, '""')}"`;
const fqn = (db, sc, t) => `${qid(db)}.${qid(sc)}.${qid(t)}`;
async function loadCatalog() {
  const tree = $("cat-tree");
  tree.innerHTML = '<div class="cat-empty">Loading…</div>';
  let rows;
  try {
    rows = JSON.parse(
      await runSql(
        "SELECT database_name, schema_name, table_name FROM duckdb_tables() " +
          "UNION ALL SELECT database_name, schema_name, view_name FROM duckdb_views() " +
          "WHERE NOT internal ORDER BY 1,2,3"
      )
    );
  } catch (_) {
    try {
      rows = JSON.parse(
        await runSql(
          "SELECT table_catalog AS database_name, table_schema AS schema_name, table_name FROM information_schema.tables ORDER BY 1,2,3"
        )
      );
    } catch (e) {
      tree.innerHTML = `<div class="cat-empty">${escapeHtml(String(e))}</div>`;
      return;
    }
  }
  if (!rows.length) {
    tree.innerHTML = '<div class="cat-empty">No tables yet. Create one in Edit mode, or connect MotherDuck.</div>';
    return;
  }
  const groups = {};
  for (const r of rows) {
    const db = r.database_name,
      sc = r.schema_name,
      t = r.table_name;
    (groups[db] ??= {})[sc] ??= [];
    groups[db][sc].push(t);
  }
  tree.innerHTML = "";
  for (const db of Object.keys(groups)) {
    const dn = document.createElement("div");
    dn.className = "cat-node cat-db";
    dn.textContent = "🗄 " + db;
    tree.appendChild(dn);
    for (const sc of Object.keys(groups[db])) {
      const sn = document.createElement("div");
      sn.className = "cat-node cat-schema";
      sn.textContent = sc;
      tree.appendChild(sn);
      for (const t of groups[db][sc]) {
        const tn = document.createElement("div");
        tn.className = "cat-node cat-table";
        tn.textContent = t;
        tn.dataset.fq = fqn(db, sc, t);
        tn.onclick = () => previewTable(db, sc, t, tn);
        tree.appendChild(tn);
      }
    }
  }
}
function filterCatalog(q) {
  q = q.trim().toLowerCase();
  const tree = $("cat-tree");
  tree.querySelectorAll(".cat-table").forEach((tn) => {
    tn.style.display = !q || tn.textContent.toLowerCase().includes(q) ? "" : "none";
  });
  tree.querySelectorAll(".cat-schema").forEach((sn) => {
    let any = false;
    for (let el = sn.nextElementSibling; el && el.classList.contains("cat-table"); el = el.nextElementSibling)
      if (el.style.display !== "none") any = true;
    sn.style.display = any ? "" : "none";
  });
  tree.querySelectorAll(".cat-db").forEach((dn) => {
    let any = false;
    for (let el = dn.nextElementSibling; el && !el.classList.contains("cat-db"); el = el.nextElementSibling)
      if (el.classList.contains("cat-table") && el.style.display !== "none") any = true;
    dn.style.display = any ? "" : "none";
  });
}
// Run whatever is in the explore SQL editor and show the result table.
async function runExplore() {
  $("xq-detail").innerHTML = ""; // a manual query clears the table-stats card
  await renderExploreResult($("xq-sql").value.trim());
}
async function renderExploreResult(sql) {
  const info = $("xq-info"),
    res = $("xq-results");
  if (!sql) {
    res.innerHTML = '<div class="explore-empty">Pick a table on the left, or write a query above and Run.</div>';
    info.textContent = "";
    return;
  }
  info.textContent = "Running…";
  res.innerHTML = "";
  try {
    const rows = JSON.parse(await runSql(sql));
    info.textContent = `${rows.length.toLocaleString()} row${rows.length === 1 ? "" : "s"}`;
    res.appendChild(renderTable(rows));
  } catch (e) {
    info.textContent = "";
    res.innerHTML = `<div class="err">${escapeHtml(String(e))}</div>`;
  }
}
async function previewTable(db, sc, t, node) {
  document.querySelectorAll(".cat-table").forEach((el) => el.classList.toggle("active", el === node));
  const fq = fqn(db, sc, t);
  const path = `${escapeHtml(db)}.${escapeHtml(sc)}`;
  $("xq-sql").value = `SELECT * FROM ${fq} LIMIT 100`;
  const detail = $("xq-detail");
  detail.innerHTML = '<div class="explore-empty">Loading…</div>';
  renderExploreResult(`SELECT * FROM ${fq} LIMIT 100`);
  try {
    const stats = JSON.parse(await runSql(`SUMMARIZE FROM ${fq}`));
    let total = null;
    try {
      total = Number(JSON.parse(await runSql(`SELECT count(*) AS n FROM ${fq}`))[0].n);
    } catch (_) {}
    detail.innerHTML = "";
    const h = document.createElement("div");
    h.className = "explore-head";
    const meta = `${total != null ? total.toLocaleString() + " rows" : ""} · ${stats.length} columns`;
    h.innerHTML = `<div><h3>${escapeHtml(t)}</h3><div class="explore-sub">${path} — ${meta}</div></div>`;
    const open = document.createElement("button");
    open.className = "btn2";
    open.textContent = "＋ New dashboard from this table";
    open.onclick = () =>
      openTableAsDashboard(
        t,
        fq,
        stats.map((r) => r.column_name)
      );
    h.appendChild(open);
    detail.appendChild(h);
    const sec = document.createElement("div");
    sec.className = "explore-sec";
    sec.textContent = `Columns (${stats.length})`;
    detail.appendChild(sec);
    detail.appendChild(renderTable(stats));
  } catch (e) {
    detail.innerHTML = `<div class="err">${escapeHtml(String(e))}</div>`;
  }
}
// Explore → build: prewrite a paged dashboard querying the picked table.
function openTableAsDashboard(name, fq, colNames) {
  const cols = (colNames || []).filter(Boolean);
  let sql;
  if (cols.length) {
    const items = cols.map((c, i) => (i === cols.length - 1 ? `${qid(c)} ::PAGED` : qid(c))).join(", ");
    sql = `-- Paged view of ${fq}\nSELECT ${items} FROM ${fq};`;
  } else {
    sql = `SELECT * FROM ${fq} LIMIT 100 ::TABLE;`;
  }
  setMode("edit");
  loadDash(name, sql);
  $("dash-name").value = name;
}

// ---------- MotherDuck ----------
const MD_KEY = "dp_md";
function mdSaved() {
  try {
    return JSON.parse(localStorage.getItem(MD_KEY) || "null");
  } catch (_) {
    return null;
  }
}
// Attach an md: database. Token/db are substituted here (never in the dashboard
// SQL), so they stay out of the share link and exported HTML.
async function mdConnect(token, db) {
  const q = (s) => `'${String(s).replace(/'/g, "''")}'`;
  for (const stmt of ["INSTALL motherduck", "LOAD motherduck"]) {
    try {
      await runSql(stmt);
    } catch (_) {
      /* autoloaded on some builds */
    }
  }
  await runSql(`SET motherduck_token=${q(token)}`);
  try {
    await runSql(db ? `ATTACH ${q("md:" + db)}` : "ATTACH 'md:'");
  } catch (e) {
    if (!/already attached|already exists/i.test(String(e))) throw e;
  }
}
function mdMark(ok, db) {
  const btn = $("md");
  btn.classList.toggle("connected", ok);
  btn.textContent = ok ? `☁ ${db || "MotherDuck"} ✓` : "☁ MotherDuck";
}
function mdOpen() {
  const s = mdSaved() || {};
  $("md-token").value = s.token || "";
  $("md-db").value = s.db || "";
  const st = $("md-status");
  st.textContent = "";
  st.className = "md-status";
  $("md-dialog").hidden = false;
  $("md-token").focus();
}
async function mdDoConnect() {
  const token = $("md-token").value.trim();
  const db = $("md-db").value.trim();
  const st = $("md-status");
  if (!token) {
    st.textContent = "Please paste a token.";
    st.className = "md-status err";
    return;
  }
  st.textContent = "Connecting…";
  st.className = "md-status";
  try {
    await mdConnect(token, db);
    localStorage.setItem(MD_KEY, JSON.stringify({ token, db }));
    mdMark(true, db);
    st.textContent = "Connected ✓";
    st.className = "md-status ok";
    setTimeout(() => ($("md-dialog").hidden = true), 700);
    run();
  } catch (e) {
    st.textContent = "Failed: " + String(e).replace(/^Error:\s*/, "");
    st.className = "md-status err";
  }
}
async function mdDisconnect() {
  const s = mdSaved();
  localStorage.removeItem(MD_KEY);
  if (s && s.db) {
    try {
      await runSql(`DETACH ${s.db.replace(/[^A-Za-z0-9_]/g, "")}`); // detach by name
    } catch (_) {}
  }
  mdMark(false);
  const st = $("md-status");
  st.textContent = "Disconnected — reload the page for a fully clean session.";
  st.className = "md-status";
}

const role = (s, name) => s.roles.some((r) => r[1] === name);
const INPUTS = ["DROPDOWN", "NUMBER", "DATE", "TEXT", "MULTISELECT", "DATERANGE"];
const inputKind = (s) => INPUTS.find((k) => role(s, k));
const isInput = (s) => !!inputKind(s);
const METRICS = ["METRIC", "MONEY", "PERCENT", "COMPACT"];
const metricRole = (s) => s.roles.find((r) => METRICS.includes(r[1]));
const isHeading = (s) => s.roles.length === 1 && s.roles[0][1] === "LABEL";
const directive = (s) =>
  ["COLUMNS", "GROUP", "ENDGROUP", "SPAN", "HEIGHT", "TAB", "SUBTAB", "PLACEHOLDER"].find((d) => role(s, d));
let dpVars = {}; // DuckDB variable name -> selected value (persists across runs)
let dpCols = 2; // default panels-per-row on the 12-column grid
let dpFilter = ""; // generic cross-filter: last clicked value, as getvariable('selected')
let dpXf = {}; // named cross-filters: column-name -> value, each getvariable('<name>')
let dpPage = {}; // ::PAGED tables: statement index -> current page (server-side)
let dpSort = {}; // ::PAGED tables: statement index -> {col, dir} (server-side sort)
let dpTab = null; // the active tab name (preserved across re-runs)
let dpSubTab = {}; // top-tab name -> active sub-tab name (nested tabs)
let dpTimer = null; // auto-refresh interval handle
let dpPrimary = null; // optional brand colour (hex, no #) for chart primary

async function run() {
  const grid = $("grid");
  document.body.classList.add("loading");
  status("running…");
  // Double-buffer: build the whole dashboard off-screen, then swap it in at the
  // end. The old dashboard stays visible during the (async) rebuild, so a
  // cross-filter re-run updates in place instead of blinking empty.
  const newGrid = document.createElement("div");
  newGrid.className = "grid";
  let stmts;
  try {
    stmts = JSON.parse(plan($("sql").value));
  } catch (e) {
    grid.replaceChildren();
    return showError(grid, String(e));
  }

  // Cross-filter values — the generic `selected` (last click) plus any NAMED
  // cross-filters (each table emits getvariable('<its first column>') so two
  // tables give two independent live selections). Unset named vars read as NULL,
  // so targets guard with COALESCE(getvariable('name'),'').
  try {
    await runSql(`SET VARIABLE selected = '${dpFilter.replace(/'/g, "''")}'`);
    for (const [k, v] of Object.entries(dpXf)) {
      if (/^[A-Za-z_][A-Za-z0-9_]*$/.test(k)) {
        await runSql(`SET VARIABLE ${k} = '${String(v).replace(/'/g, "''")}'`);
      }
    }
  } catch (e) {
    /* ignore */
  }

  // Pre-pass: run setup + set each dropdown's DuckDB variable (before charts),
  // caching options for the render pass by statement index.
  const dd = {};
  for (let i = 0; i < stmts.length; i++) {
    const s = stmts[i];
    try {
      if (s.setup) {
        await runSql(s.sql);
      } else if (isInput(s)) {
        const kind = inputKind(s);
        const rows = JSON.parse(await runSql(s.sql));
        if (!rows.length) continue;
        if (kind === "DATERANGE") {
          const keys = Object.keys(rows[0]);
          const fk = keys[0];
          const tk = keys[1] || keys[0];
          if (dpVars[fk] === undefined) dpVars[fk] = String(rows[0][fk] ?? "");
          if (dpVars[tk] === undefined) dpVars[tk] = String(rows[0][tk] ?? "");
          dd[i] = { kind, varnames: [fk, tk] };
          await runSql(`SET VARIABLE ${fk} = '${String(dpVars[fk]).replace(/'/g, "''")}'`);
          await runSql(`SET VARIABLE ${tk} = '${String(dpVars[tk]).replace(/'/g, "''")}'`);
          continue;
        }
        const varname = Object.keys(rows[0])[0];
        let lit;
        if (kind === "DROPDOWN") {
          const options = rows.map((r) => String(r[varname]));
          if (dpVars[varname] === undefined || !options.includes(dpVars[varname])) dpVars[varname] = options[0];
          // Optional ::HINT column → a hint shown next to each option.
          const hr = s.roles.find((r) => r[1] === "HINT");
          const hints = hr ? rows.map((r) => String(r["c" + hr[0]] ?? "")) : null;
          dd[i] = { kind, varname, options, hints };
          lit = `'${String(dpVars[varname]).replace(/'/g, "''")}'`;
        } else if (kind === "MULTISELECT") {
          const options = rows.map((r) => String(r[varname]));
          if (!Array.isArray(dpVars[varname])) dpVars[varname] = options.slice(); // default: all
          dpVars[varname] = dpVars[varname].filter((v) => options.includes(v));
          dd[i] = { kind, varname, options };
          lit = "[" + dpVars[varname].map((v) => `'${String(v).replace(/'/g, "''")}'`).join(",") + "]";
        } else {
          // number / date / text: the query's value is the default
          if (dpVars[varname] === undefined) dpVars[varname] = String(rows[0][varname] ?? "");
          dd[i] = { kind, varname };
          const v = String(dpVars[varname]);
          lit = kind === "NUMBER" ? v || "0" : `'${v.replace(/'/g, "''")}'`; // number unquoted
        }
        await runSql(`SET VARIABLE ${varname} = ${lit}`);
      }
    } catch (e) {
      showError(newGrid, `${s.sql}\n\n${e}`);
    }
  }

  // Render pass: place controls / headings / charts in document order into the
  // current container (the grid, or an open ::GROUP box). ::COLUMNS sets the
  // grid columns; ::SPAN widens the next panel.
  let container = newGrid;
  let curGrid = newGrid; // the active surface (main grid / tab pane / sub-tab pane)
  let tabBar = null;
  let tabWrap = null;
  let curPane = null; // the current top-level ::TAB pane (nesting host for ::SUBTAB)
  let curTopName = null;
  let subBar = null;
  let subWrap = null;
  let nextSpan = 0;
  let nextHeight = 0; // ::HEIGHT → the next panel's height in px (optional)
  let defaultSpan = Math.max(1, Math.round(12 / dpCols)); // 12-col bootstrap default
  let panels = 0;
  const firstValue = async (s) => {
    const rows = JSON.parse(await runSql(s.sql));
    return rows[0] ? Object.values(rows[0])[0] : null;
  };
  for (let i = 0; i < stmts.length; i++) {
    const s = stmts[i];
    if (s.setup) continue;
    const d = directive(s);
    try {
      if (d === "COLUMNS") {
        const n = parseInt(await firstValue(s));
        if (n > 0) {
          dpCols = n;
          defaultSpan = Math.max(1, Math.round(12 / n));
        }
      } else if (d === "GROUP") {
        const title = await firstValue(s);
        const box = document.createElement("section");
        box.className = "group";
        if (title) {
          const t = document.createElement("div");
          t.className = "group-title";
          t.textContent = title;
          box.appendChild(t);
        }
        const body = document.createElement("div");
        body.className = "group-body";
        box.appendChild(body);
        curGrid.appendChild(box);
        container = body;
      } else if (d === "ENDGROUP") {
        container = curGrid;
      } else if (d === "SPAN") {
        nextSpan = parseInt(await firstValue(s)) || 0;
      } else if (d === "HEIGHT") {
        nextHeight = parseInt(await firstValue(s)) || 0;
      } else if (d === "TAB") {
        const name = String((await firstValue(s)) ?? "Tab");
        if (!tabBar) {
          tabBar = document.createElement("div");
          tabBar.className = "tabbar";
          tabWrap = document.createElement("div");
          tabWrap.className = "tabwrap";
        }
        const pane = document.createElement("div");
        pane.className = "grid tabpane";
        pane.style.display = "none";
        tabWrap.appendChild(pane);
        const btn = document.createElement("button");
        btn.className = "tab-btn";
        btn.textContent = name;
        btn.onclick = (e) => {
          e.stopPropagation();
          dpTab = name; // remember, so a cross-filter re-run keeps this tab
          tabWrap.querySelectorAll(".tabpane").forEach((p) => (p.style.display = "none"));
          tabBar.querySelectorAll(".tab-btn").forEach((b) => b.classList.remove("active"));
          pane.style.display = "";
          btn.classList.add("active");
        };
        tabBar.appendChild(btn);
        if (dpTab === name || (dpTab === null && tabBar.children.length === 1)) {
          pane.style.display = "";
          btn.classList.add("active");
        }
        curGrid = pane;
        container = pane;
        curPane = pane; // nesting host for ::SUBTAB
        curTopName = name;
        subBar = null; // start fresh sub-tabs for this top tab
        subWrap = null;
      } else if (d === "SUBTAB") {
        const name = String((await firstValue(s)) ?? "Tab");
        const host = curPane || curGrid; // nest inside the current top-level tab
        if (!subBar) {
          subBar = document.createElement("div");
          subBar.className = "subtabbar";
          subWrap = document.createElement("div");
          subWrap.className = "subtabwrap";
          host.append(subBar, subWrap);
        }
        const spane = document.createElement("div");
        spane.className = "grid subtabpane";
        spane.style.display = "none";
        subWrap.appendChild(spane);
        const sbtn = document.createElement("button");
        sbtn.className = "tab-btn subtab-btn";
        sbtn.textContent = name;
        const topName = curTopName || "";
        // Capture THIS sub-tab's bar/wrap — `subBar`/`subWrap` are shared loop
        // variables that get reassigned for later top tabs, so the closure must
        // not read them directly (that made a sub-tab operate on the last tab's
        // bar: siblings stayed active + other tabs went blank).
        const myBar = subBar;
        const myWrap = subWrap;
        sbtn.onclick = (e) => {
          e.stopPropagation();
          dpSubTab[topName] = name;
          myWrap.querySelectorAll(".subtabpane").forEach((p) => (p.style.display = "none"));
          myBar.querySelectorAll(".subtab-btn").forEach((b) => b.classList.remove("active"));
          spane.style.display = "";
          sbtn.classList.add("active");
        };
        subBar.appendChild(sbtn);
        const activeSub = dpSubTab[topName];
        if (activeSub === name || (activeSub == null && subBar.children.length === 1)) {
          spane.style.display = "";
          sbtn.classList.add("active");
        }
        curGrid = spane;
        container = spane;
      } else if (d === "PLACEHOLDER") {
        const span = Math.min(12, nextSpan || defaultSpan);
        const ph = document.createElement("div");
        ph.className = "panel placeholder";
        if (container === curGrid) ph.style.gridColumn = `span ${span}`;
        container.appendChild(ph);
        nextSpan = 0;
        nextHeight = 0;
      } else if (isInput(s)) {
        if (dd[i]) container.appendChild(makeControl(dd[i], container === curGrid));
      } else if (role(s, "PAGED")) {
        // SQL-driven pagination: only ONE page (+ a COUNT) is fetched, so the
        // browser never holds the whole table. LIMIT/OFFSET + ORDER BY run in
        // DuckDB — the same over a huge parquet in S3 or MotherDuck.
        const span = Math.min(12, nextSpan || defaultSpan);
        const fig = document.createElement("figure");
        fig.className = "panel";
        if (container === curGrid) fig.style.gridColumn = `span ${span}`;
        const TFMT = ["MONEY", "PERCENT", "COMPACT", "METRIC", "TREND", "COLORSCALE", "BADGE", "SPARKLINE", "PLAIN"];
        const fmtByIdx = {};
        for (const [ix, r] of s.roles) if (TFMT.includes(r)) fmtByIdx[ix] = r;
        const titleRole = s.roles.find((r) => r[1] === "TITLE");
        const titleIdx = titleRole ? titleRole[0] : -1;
        const base = s.sql;
        const pageSize = 50;
        const idx = i;
        const titleHolder = document.createElement("div");
        const holder = document.createElement("div");
        fig.append(titleHolder, holder);
        const qident = (c) => `"${String(c).replace(/"/g, '""')}"`;
        let cachedTotal = null;
        const load = async () => {
          const page = dpPage[idx] || 0;
          const sort = dpSort[idx];
          if (cachedTotal == null) {
            try {
              const c = JSON.parse(await runSql(`SELECT count(*) AS n FROM (${base}) _dp`));
              cachedTotal = Number(c[0] && c[0].n) || 0;
            } catch (_) {
              cachedTotal = 0;
            }
          }
          const order = sort && sort.col ? ` ORDER BY ${qident(sort.col)} ${sort.dir > 0 ? "ASC" : "DESC"}` : "";
          let rows = [];
          try {
            rows = JSON.parse(await runSql(`SELECT * FROM (${base}) _dp${order} LIMIT ${pageSize} OFFSET ${page * pageSize}`));
          } catch (e) {
            holder.innerHTML = "";
            showError(holder, String(e));
            return;
          }
          titleHolder.innerHTML = "";
          holder.innerHTML = "";
          let sk = -1;
          if (titleIdx >= 0 && rows.length) {
            const tv = Object.values(rows[0])[titleIdx];
            if (tv != null) titleHolder.appendChild(mkTitle(String(tv).replace(/^"|"$/g, "")));
            sk = titleIdx;
          }
          const server = {
            total: cachedTotal,
            page,
            pageSize,
            sortCol: sort ? sort.col : null,
            sortDir: sort ? sort.dir : 1,
            onPage: (p) => {
              dpPage[idx] = Math.max(0, p);
              load();
            },
            onSort: (col) => {
              const cur = dpSort[idx];
              dpSort[idx] = { col, dir: cur && cur.col === col ? -cur.dir : 1 };
              dpPage[idx] = 0;
              load();
            },
          };
          holder.appendChild(renderTable(rows, sk, fmtByIdx, server));
        };
        await load();
        container.appendChild(fig);
        panels++;
        nextSpan = 0;
        nextHeight = 0;
      } else {
        const rowsJson = await runSql(s.sql);
        const span = Math.min(12, nextSpan || defaultSpan);
        const boxH = nextHeight;
        const mkPanel = () => {
          const fig = document.createElement("figure");
          fig.className = "panel";
          if (container === curGrid) fig.style.gridColumn = `span ${span}`;
          if (boxH) fig.style.minHeight = boxH + "px";
          return fig;
        };
        const firstCell = () => {
          const r = JSON.parse(rowsJson)[0];
          return r ? String(Object.values(r)[0] ?? "").replace(/^"|"$/g, "") : "";
        };
        if (isHeading(s)) {
          const rows = JSON.parse(rowsJson);
          const h = document.createElement("h2");
          h.className = "section";
          h.textContent = rows[0] ? Object.values(rows[0])[0] : "";
          container.appendChild(h);
        } else if (role(s, "RELOAD")) {
          // Auto-refresh every N seconds, driven from SQL.
          const secs = parseFloat(firstCell()) || 0;
          clearInterval(dpTimer);
          if (secs > 0) dpTimer = setInterval(run, secs * 1000);
          if ($("refresh")) $("refresh").value = [0, 5, 15, 30, 60].includes(secs) ? String(secs) : "0";
        } else if (role(s, "HEADER_IMAGE")) {
          const img = document.createElement("img");
          img.className = "header-image";
          img.src = firstCell();
          container.appendChild(img);
        } else if (role(s, "FOOTER_LINK")) {
          const rows = JSON.parse(rowsJson);
          const vals = rows[0] ? Object.values(rows[0]).map((v) => String(v ?? "")) : [""];
          const a = document.createElement("a");
          a.className = "footer-link";
          a.href = vals[0];
          a.textContent = (vals[1] || vals[0]).replace(/^"|"$/g, "");
          a.target = "_blank";
          a.rel = "noopener";
          container.appendChild(a);
        } else if (role(s, "DOWNLOAD_CSV") || role(s, "DOWNLOAD_XLSX") || role(s, "DOWNLOAD_PDF")) {
          const rows = JSON.parse(rowsJson);
          container.appendChild(mkDownload(s, rows));
        } else if (textSizeOf(s)) {
          const fig = mkPanel();
          fig.classList.add("textcard", "text-" + textSizeOf(s));
          const lr = s.roles.find((r) => r[1] === "LABEL");
          const r0 = JSON.parse(rowsJson)[0] || {};
          fig.innerHTML =
            `<div class="text-value">${escapeHtml(firstCell())}</div>` +
            (lr ? `<div class="metric-cap">${escapeHtml(r0["c" + lr[0]])}</div>` : "");
          container.appendChild(fig);
          panels++;
        } else if (role(s, "TABLE")) {
          const rows = JSON.parse(rowsJson);
          const fig = mkPanel();
          const tr = s.roles.find((r) => r[1] === "TITLE");
          let skip = -1;
          if (tr && rows.length) {
            skip = tr[0];
            const tv = Object.values(rows[0])[skip];
            if (tv != null) fig.appendChild(mkTitle(String(tv).replace(/^"|"$/g, "")));
          }
          // Per-column formatting (::MONEY/::PERCENT/::COMPACT/::METRIC number
          // formats, ::TREND arrows, ::COLORSCALE heatmap cells, ::BADGE pills,
          // ::SPARKLINE mini charts), keyed by output column index.
          const TFMT = ["MONEY", "PERCENT", "COMPACT", "METRIC", "TREND", "COLORSCALE", "BADGE", "SPARKLINE", "PLAIN"];
          const fmtByIdx = {};
          for (const [idx, r] of s.roles) if (TFMT.includes(r)) fmtByIdx[idx] = r;
          fig.appendChild(renderTable(rows, skip, fmtByIdx));
          container.appendChild(fig);
          panels++;
        } else if (metricRole(s)) {
          const r0 = JSON.parse(rowsJson)[0] || {};
          const mr = metricRole(s);
          const lr = s.roles.find((r) => r[1] === "LABEL");
          const dr = s.roles.find((r) => r[1] === "DELTA");
          const fig = mkPanel();
          fig.classList.add("metric");
          let deltaHtml = "";
          if (dr) {
            const cur = parseFloat(r0["c" + mr[0]]);
            const prev = parseFloat(r0["c" + dr[0]]);
            if (!isNaN(cur) && !isNaN(prev) && prev !== 0) {
              const pct = ((cur - prev) / Math.abs(prev)) * 100;
              const up = pct >= 0;
              deltaHtml =
                `<div class="metric-delta ${up ? "up" : "down"}">${up ? "▲" : "▼"} ` +
                `${Math.abs(pct).toLocaleString(undefined, { maximumFractionDigits: 1 })}%</div>`;
            }
          }
          fig.innerHTML =
            `<div class="metric-value">${fmtNum(r0["c" + mr[0]], mr[1])}</div>` +
            deltaHtml +
            `<div class="metric-cap">${escapeHtml(lr ? r0["c" + lr[0]] : "")}</div>`;
          container.appendChild(fig);
          panels++;
        } else {
          const fig = mkPanel();
          const t = titleOf(s, rowsJson);
          if (t) fig.appendChild(mkTitle(t));
          // A selection that filters everything out renders a clean note, not a
          // broken/empty chart box.
          if (!JSON.parse(rowsJson).length) {
            fig.appendChild(mkNoData());
          } else {
            const ph = boxH || (role(s, "SPARKLINE") ? 90 : 300); // ::HEIGHT, else default
            fig.insertAdjacentHTML(
              "beforeend",
              render_panel(rowsJson, JSON.stringify(s.roles), 460, ph, dpPrimary || "")
            );
          }
          container.appendChild(fig);
          panels++;
        }
        nextSpan = 0;
        nextHeight = 0;
      }
    } catch (e) {
      showError(container, `${s.sql}\n\n${e}`);
    }
  }
  // If the remembered tab no longer exists, fall back to the first.
  if (tabBar && !tabBar.querySelector(".tab-btn.active")) {
    tabWrap.querySelector(".tabpane").style.display = "";
    tabBar.querySelector(".tab-btn").classList.add("active");
  }
  // Same fallback for nested sub-tabs: a remembered sub-tab that no longer
  // exists (edited SQL, or a stale name carried from another dashboard) would
  // otherwise leave the sub-pane blank. Activate the first sub-tab in that case.
  (tabWrap || newGrid).querySelectorAll(".subtabbar").forEach((bar) => {
    if (bar.querySelector(".subtab-btn.active")) return;
    const firstBtn = bar.querySelector(".subtab-btn");
    const wrap = bar.nextElementSibling; // subtabwrap follows subtabbar
    const firstPane = wrap && wrap.querySelector(".subtabpane");
    if (firstBtn) firstBtn.classList.add("active");
    if (firstPane) firstPane.style.display = "";
  });
  // Atomic swap: replace the visible content in one synchronous step (no flash).
  grid.replaceChildren(...newGrid.childNodes);
  const dash = document.querySelector(".dash");
  dash.querySelectorAll(".tabbar,.tabwrap").forEach((e) => e.remove());
  if (tabBar) dash.append(tabBar, tabWrap);
  attachHover();
  addExportButtons();
  status(`${panels} panel${panels === 1 ? "" : "s"}`);
  document.body.classList.remove("loading");
}

// A labelled <select>; changing it re-runs the dashboard. `bar` wraps a
// stand-alone control in its own spanning row (grouped ones sit inline).
function finalizeControl(wrap, bar) {
  if (!bar) return wrap;
  const box = document.createElement("div");
  box.className = "controls";
  box.appendChild(wrap);
  return box;
}

function makeControl(meta, bar) {
  const wrap = document.createElement("label");
  wrap.className = "control";
  wrap.textContent = (meta.varname || "date") + ":";
  if (meta.kind === "DATERANGE") {
    const mk = (k) => {
      const inp = document.createElement("input");
      inp.type = "date";
      inp.value = dpVars[k] || "";
      inp.onchange = () => {
        dpVars[k] = inp.value;
        run();
      };
      return inp;
    };
    wrap.appendChild(mk(meta.varnames[0]));
    const arrow = document.createElement("span");
    arrow.textContent = "→";
    arrow.className = "daterange-arrow";
    wrap.appendChild(arrow);
    wrap.appendChild(mk(meta.varnames[1]));
    return finalizeControl(wrap, bar);
  }
  let input;
  if (meta.kind === "DROPDOWN") {
    input = document.createElement("select");
    meta.options.forEach((o, k) => {
      const opt = document.createElement("option");
      opt.value = o;
      const hint = meta.hints && meta.hints[k];
      opt.textContent = hint ? `${o} — ${hint}` : o;
      if (o === dpVars[meta.varname]) opt.selected = true;
      input.appendChild(opt);
    });
    input.onchange = () => {
      dpVars[meta.varname] = input.value;
      run();
    };
  } else if (meta.kind === "MULTISELECT") {
    input = document.createElement("select");
    input.multiple = true;
    input.size = Math.min(4, Math.max(2, meta.options.length));
    const sel = new Set(dpVars[meta.varname] || []);
    for (const o of meta.options) {
      const opt = document.createElement("option");
      opt.value = opt.textContent = o;
      if (sel.has(o)) opt.selected = true;
      input.appendChild(opt);
    }
    input.onchange = () => {
      dpVars[meta.varname] = [...input.selectedOptions].map((o) => o.value);
      run();
    };
  } else {
    input = document.createElement("input");
    input.type = meta.kind === "NUMBER" ? "number" : meta.kind === "DATE" ? "date" : "text";
    input.value = dpVars[meta.varname] ?? "";
    input.onchange = () => {
      dpVars[meta.varname] = input.value;
      run();
    };
  }
  wrap.appendChild(input);
  return finalizeControl(wrap, bar);
}

// A per-panel title bar from a ::TITLE column (constant across the result).
function titleOf(s, rowsJson) {
  const tr = s.roles.find((r) => r[1] === "TITLE");
  if (!tr) return null;
  let rows;
  try {
    rows = JSON.parse(rowsJson);
  } catch (_) {
    return null;
  }
  const v = rows[0] ? rows[0]["c" + tr[0]] : null;
  return v == null ? null : String(v).replace(/^"|"$/g, "");
}

function mkTitle(text) {
  const c = document.createElement("figcaption");
  c.className = "panel-title";
  c.textContent = text;
  return c;
}

function mkNoData() {
  const d = document.createElement("div");
  d.className = "nodata";
  d.textContent = "No data for this selection";
  return d;
}

const cleanNum = (v) => {
  if (v == null) return null;
  if (typeof v === "number") return v;
  const n = parseFloat(String(v).replace(/^"|"$/g, ""));
  return isNaN(n) ? null : n;
};

// A ::TABLE result → a sortable HTML table with in-cell bars + per-column
// formatting (fmtByIdx maps an output column index to a format role). Pass
// `server` = {total, page, pageSize, onPage, onSort, sortCol, sortDir} for
// SQL-driven pagination/sorting (::PAGED); otherwise it paginates client-side.
function renderTable(rows, skip = -1, fmtByIdx = {}, server = null) {
  const t = document.createElement("table");
  t.className = "dp-table";
  if (!rows.length) {
    t.textContent = "(no rows)";
    return t;
  }
  const allKeys = Object.keys(rows[0]);
  const colFmt = {}; // column key -> format role
  for (const [idx, f] of Object.entries(fmtByIdx)) colFmt[allKeys[idx]] = f;
  const cols = allKeys.filter((_, i) => i !== skip);
  const numeric = {};
  const maxAbs = {};
  const colMin = {};
  const colMax = {};
  for (const c of cols) {
    const nums = rows.map((r) => cleanNum(r[c]));
    const numFmt = ["MONEY", "PERCENT", "COMPACT", "METRIC", "COLORSCALE", "TREND", "PLAIN"].includes(colFmt[c]);
    numeric[c] = numFmt || (nums.some((v) => v != null) && nums.every((v) => v == null || !isNaN(v)));
    maxAbs[c] = Math.max(1, ...nums.map((v) => Math.abs(v) || 0));
    const fin = nums.filter((v) => v != null);
    colMin[c] = fin.length ? Math.min(...fin) : 0;
    colMax[c] = fin.length ? Math.max(...fin) : 1;
  }
  let sortCol = server ? server.sortCol : null;
  let dir = server ? server.sortDir || 1 : 1;
  const hr = t.createTHead().insertRow();
  cols.forEach((c) => {
    const th = document.createElement("th");
    th.style.cursor = "pointer";
    th.onclick = (e) => {
      if (server) {
        e.stopPropagation();
        server.onSort(c); // server re-queries with ORDER BY + reloads this panel
        return;
      }
      dir = sortCol === c ? -dir : 1;
      sortCol = c;
      head();
      body();
    };
    hr.appendChild(th);
  });
  const tb = t.createTBody();
  let page = 0;
  const pageSize = server ? server.pageSize : 50;
  const sortedRows = () => {
    if (server) return rows; // already sorted + paged server-side
    const data = rows.slice();
    if (sortCol) {
      const num = numeric[sortCol];
      data.sort((a, b) =>
        num
          ? ((cleanNum(a[sortCol]) || 0) - (cleanNum(b[sortCol]) || 0)) * dir
          : String(a[sortCol]).localeCompare(String(b[sortCol])) * dir
      );
    }
    return data;
  };
  const head = () => cols.forEach((c, i) => (hr.cells[i].textContent = c + (c === sortCol ? (dir > 0 ? " ▲" : " ▼") : "")));
  const body = () => {
    tb.innerHTML = "";
    const data = sortedRows();
    if (server) {
      updateFoot(server.total, Math.max(1, Math.ceil(server.total / pageSize)));
    } else {
      const pages = Math.max(1, Math.ceil(data.length / pageSize));
      page = Math.min(page, pages - 1);
      updateFoot(data.length, pages);
    }
    const pageRows = server ? data : data.slice(page * pageSize, (page + 1) * pageSize);
    for (const r of pageRows) {
      const tr = tb.insertRow();
      // A categorical first column makes the row a cross-filter source. Clicking
      // sets BOTH the generic getvariable('selected') AND a NAMED cross-filter
      // getvariable('<first column name>') — so two tables with different first
      // columns drive two independent live selections. Click again / the
      // background to clear. Each table highlights its OWN named selection.
      const key = cols[0];
      if (!numeric[key]) {
        const keyVal = String(r[key] ?? "").replace(/^"|"$/g, "");
        tr.style.cursor = "pointer";
        const own = dpXf[key] !== undefined ? dpXf[key] : dpSelected;
        if (own && keyVal === own) tr.classList.add("row-sel");
        tr.onclick = (e) => {
          if (document.body.classList.contains("mode-explore")) return; // preview tables aren't cross-filters
          e.stopPropagation();
          const on = (dpXf[key] ?? "") !== keyVal;
          dpXf[key] = on ? keyVal : "";
          dpFilter = on ? keyVal : "";
          dpSelected = dpFilter || null;
          run();
        };
      }
      for (const c of cols) {
        const td = tr.insertCell();
        let v = r[c];
        if (typeof v === "string" && /^"-?[\d.]+"$/.test(v)) v = v.slice(1, -1);
        const f = colFmt[c];
        if (f === "SPARKLINE") {
          td.className = "spark-cell";
          td.innerHTML = cellSpark(v);
          continue;
        }
        if (f === "BADGE") {
          td.innerHTML = v == null ? "" : badgeHtml(unq(v));
          continue;
        }
        if (f === "TREND") {
          const n = cleanNum(v);
          td.style.textAlign = "right";
          if (n != null) {
            td.innerHTML =
              `<span class="trend ${n >= 0 ? "up" : "down"}">${n >= 0 ? "▲" : "▼"} ` +
              `${Math.abs(n).toLocaleString(undefined, { maximumFractionDigits: 1 })}</span>`;
          }
          continue;
        }
        if (f === "PLAIN") {
          // a numeric column with NO in-cell bar (::PLAIN / ::NOBAR)
          td.textContent = v == null ? "" : v;
          td.style.textAlign = "right";
          td.style.fontVariantNumeric = "tabular-nums";
          continue;
        }
        if (["MONEY", "PERCENT", "COMPACT", "METRIC", "COLORSCALE"].includes(f)) {
          const n = cleanNum(v);
          td.style.textAlign = "right";
          td.style.fontVariantNumeric = "tabular-nums";
          td.textContent =
            n == null ? (v == null ? "" : v) : f === "COLORSCALE" ? n.toLocaleString(undefined, { maximumFractionDigits: 2 }) : fmtNum(n, f);
          if (f === "COLORSCALE" && n != null) {
            td.style.background = heatColor((n - colMin[c]) / (colMax[c] - colMin[c] || 1));
            td.style.fontWeight = "600";
          }
          continue;
        }
        td.textContent = v == null ? "" : v;
        if (numeric[c]) {
          const n = cleanNum(v);
          td.style.textAlign = "right";
          td.style.fontVariantNumeric = "tabular-nums";
          if (n != null) {
            const pct = (Math.abs(n) / maxAbs[c]) * 100;
            td.style.background = `linear-gradient(90deg, rgba(42,157,143,.16) 0, rgba(31,140,166,.14) ${pct}%, transparent ${pct}%)`;
          }
        }
      }
    }
  };
  // Client-side pagination (50/page) so thousands of rows stay responsive; the
  // full result is kept for sorting and CSV export.
  const wrap = document.createElement("div");
  wrap.className = "table-wrap";
  wrap.appendChild(t);
  t._rows = rows;
  t._cols = cols;
  const foot = document.createElement("div");
  foot.className = "table-foot";
  wrap.appendChild(foot);
  function updateFoot(total, pages) {
    if (total <= pageSize) {
      foot.style.display = "none";
      return;
    }
    foot.style.display = "";
    foot.innerHTML = "";
    const cur = server ? server.page : page;
    const from = cur * pageSize + 1;
    const to = Math.min(total, (cur + 1) * pageSize);
    const mk = (label, disabled, fn) => {
      const btn = document.createElement("button");
      btn.className = "page-btn";
      btn.textContent = label;
      btn.disabled = disabled;
      btn.onclick = (e) => {
        e.stopPropagation();
        if (server) {
          fn(); // server.onPage handles the re-query + re-render
        } else {
          fn();
          body();
        }
      };
      return btn;
    };
    const info = document.createElement("span");
    info.className = "table-info";
    info.textContent = `${from.toLocaleString()}–${to.toLocaleString()} of ${total.toLocaleString()}`;
    foot.append(
      mk("◀", cur === 0, () => (server ? server.onPage(cur - 1) : (page = Math.max(0, page - 1)))),
      info,
      mk("▶", cur >= pages - 1, () => (server ? server.onPage(cur + 1) : (page = Math.min(pages - 1, page + 1))))
    );
  }
  head();
  body();
  return wrap;
}

// ::COLORSCALE cell background — a diverging green→amber→red scale by the
// normalized value t∈[0,1] (low = green, high = red). Soft tones keep the cell
// text readable.
function heatColor(t) {
  t = Math.max(0, Math.min(1, t));
  const stops = [
    [0x63, 0xc9, 0x7f], // green
    [0xff, 0xe0, 0x8a], // amber
    [0xff, 0x8a, 0x8a], // red
  ];
  const seg = t < 0.5 ? 0 : 1;
  const u = t < 0.5 ? t / 0.5 : (t - 0.5) / 0.5;
  const a = stops[seg];
  const b = stops[seg + 1];
  const m = (i) => Math.round(a[i] + (b[i] - a[i]) * u);
  return `rgb(${m(0)},${m(1)},${m(2)})`;
}

// ::BADGE — a coloured status pill; colour inferred from common status words.
function badgeHtml(text) {
  const t = text.toLowerCase();
  let cls = "badge-neutral";
  if (/\b(ok|good|on.?track|pass(ed)?|active|done|up|healthy|nominal|green|low)\b/.test(t)) cls = "badge-good";
  else if (/\b(warn(ing)?|risk|at.?risk|pending|review|amber|medium|hold|watch)\b/.test(t)) cls = "badge-warn";
  else if (/\b(bad|fail(ed)?|late|error|down|critical|red|overdue|stale|high|breach)\b/.test(t)) cls = "badge-bad";
  return `<span class="badge ${cls}">${escapeHtml(text)}</span>`;
}

// ::SPARKLINE cell — a tiny inline trend line from a numeric array (DuckDB list()).
function cellSpark(v) {
  let arr = Array.isArray(v) ? v : null;
  if (!arr && typeof v === "string") {
    try {
      const p = JSON.parse(v);
      if (Array.isArray(p)) arr = p;
    } catch (_) {}
  }
  const nums = (arr || []).map(Number).filter((x) => isFinite(x));
  if (nums.length < 2) return "";
  const w = 84;
  const h = 22;
  const pad = 2;
  const mn = Math.min(...nums);
  const rng = Math.max(...nums) - mn || 1;
  const xs = (i) => pad + (i * (w - 2 * pad)) / (nums.length - 1);
  const ys = (y) => h - pad - ((y - mn) / rng) * (h - 2 * pad);
  const pts = nums.map((y, i) => `${xs(i).toFixed(1)},${ys(y).toFixed(1)}`).join(" ");
  const lx = xs(nums.length - 1).toFixed(1);
  const ly = ys(nums[nums.length - 1]).toFixed(1);
  return (
    `<svg class="spark" width="${w}" height="${h}" viewBox="0 0 ${w} ${h}">` +
    `<polyline points="${pts}" fill="none" stroke="#456481" stroke-width="0.9" stroke-linejoin="round" stroke-linecap="round"/>` +
    `<circle cx="${lx}" cy="${ly}" r="1.8" fill="#E8335D"/></svg>`
  );
}

// Format a KPI value. fmt: METRIC (plain), MONEY, PERCENT, COMPACT.
function fmtNum(v, fmt) {
  const n = typeof v === "number" ? v : parseFloat(v);
  if (v == null) return "–";
  if (Number.isNaN(n)) return String(v);
  if (fmt === "MONEY")
    return n.toLocaleString(undefined, { style: "currency", currency: "USD", maximumFractionDigits: 0 });
  if (fmt === "PERCENT") return n.toLocaleString(undefined, { maximumFractionDigits: 1 }) + "%";
  if (fmt === "COMPACT")
    return new Intl.NumberFormat(undefined, { notation: "compact", maximumFractionDigits: 1 }).format(n);
  return n.toLocaleString(undefined, { maximumFractionDigits: 2 });
}

function escapeHtml(s) {
  return String(s ?? "").replace(/[&<>]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;" })[c]);
}

// ---------- export & share ----------
function download(blob, name) {
  const a = document.createElement("a");
  a.href = URL.createObjectURL(blob);
  a.download = name;
  a.click();
  setTimeout(() => URL.revokeObjectURL(a.href), 1000);
}

function csvOf(table) {
  // Export the FULL result (all pages), not just the rendered page.
  if (table._rows && table._cols) return csvOfRows(table._rows, table._cols);
  return [...table.rows]
    .map((r) => [...r.cells].map((c) => `"${c.textContent.replace(/"/g, '""')}"`).join(","))
    .join("\n");
}

// ::TEXT_SMALL/_MEDIUM/_LARGE → "small" | "medium" | "large" (or null).
function textSizeOf(s) {
  const t = s.roles.find((r) => ["TEXT_SMALL", "TEXT_MEDIUM", "TEXT_LARGE"].includes(r[1]));
  return t ? t[1].split("_")[1].toLowerCase() : null;
}

const unq = (v) => String(v ?? "").replace(/^"|"$/g, "");

// CSV / .xls (HTML-table Excel) directly from JSON result rows.
function csvOfRows(rows, cols) {
  const esc = (v) => `"${unq(v).replace(/"/g, '""')}"`;
  return [cols.map(esc).join(","), ...rows.map((r) => cols.map((c) => esc(r[c])).join(","))].join("\n");
}
function xlsOfRows(rows, cols) {
  const cell = (v) => escapeHtml(unq(v));
  const head = "<tr>" + cols.map((c) => `<th>${cell(c)}</th>`).join("") + "</tr>";
  const body = rows.map((r) => "<tr>" + cols.map((c) => `<td>${cell(r[c])}</td>`).join("") + "</tr>").join("");
  return `<html><head><meta charset="utf-8"></head><body><table>${head}${body}</table></body></html>`;
}

// A ::DOWNLOAD_CSV/_XLSX/_PDF button. CSV/XLSX export the query rows; PDF prints.
function mkDownload(s, rows) {
  const fmt = role(s, "DOWNLOAD_XLSX") ? "xlsx" : role(s, "DOWNLOAD_PDF") ? "pdf" : "csv";
  const label = { csv: "Download CSV", xlsx: "Download Excel", pdf: "Download PDF" }[fmt];
  const wrap = document.createElement("div");
  wrap.className = "controls download-row";
  const btn = document.createElement("button");
  btn.className = "dl-btn";
  btn.textContent = "⤓ " + label;
  btn.onclick = () => {
    if (fmt === "pdf") return window.print();
    const cols = rows.length ? Object.keys(rows[0]) : [];
    if (fmt === "xlsx") download(new Blob([xlsOfRows(rows, cols)], { type: "application/vnd.ms-excel" }), "data.xls");
    else download(new Blob([csvOfRows(rows, cols)], { type: "text/csv" }), "data.csv");
  };
  wrap.appendChild(btn);
  return wrap;
}

// Rasterise a chart's SVG to a PNG (white background, 2× for crispness).
function svgToPng(svg, name) {
  const vb = svg.viewBox.baseVal;
  const w = vb.width || svg.clientWidth || 460;
  const h = vb.height || svg.clientHeight || 300;
  const xml = new XMLSerializer().serializeToString(svg);
  const img = new Image();
  img.onload = () => {
    const c = document.createElement("canvas");
    c.width = w * 2;
    c.height = h * 2;
    const ctx = c.getContext("2d");
    ctx.scale(2, 2);
    ctx.fillStyle = "#fff";
    ctx.fillRect(0, 0, w, h);
    ctx.drawImage(img, 0, 0, w, h);
    c.toBlob((b) => download(b, name + ".png"));
  };
  img.src = "data:image/svg+xml;base64," + btoa(unescape(encodeURIComponent(xml)));
}

// A hover download button on every chart (PNG) / table (CSV) panel.
function addExportButtons() {
  document.querySelectorAll(".panel").forEach((fig) => {
    if (fig.querySelector(".dl")) return;
    const svg = fig.querySelector("svg");
    const table = fig.querySelector(".dp-table");
    if (!svg && !table) return;
    fig.style.position = "relative";
    const btn = document.createElement("button");
    btn.className = "dl";
    btn.textContent = "⤓";
    btn.title = table ? "download CSV" : "download PNG";
    btn.onclick = (e) => {
      e.stopPropagation();
      if (table) download(new Blob([csvOf(table)], { type: "text/csv" }), "data.csv");
      else svgToPng(svg, "chart");
    };
    fig.appendChild(btn);
  });
}

// Share: encode the SQL in the URL hash (no server needed).
const encodeSql = (sql) => btoa(unescape(encodeURIComponent(sql)));
function decodeHashSql() {
  const m = location.hash.match(/sql=([^&]+)/);
  if (!m) return null;
  try {
    return decodeURIComponent(escape(atob(m[1])));
  } catch (e) {
    return null;
  }
}
function shareLink() {
  location.hash = "sql=" + encodeSql($("sql").value);
  navigator.clipboard.writeText(location.href).then(
    () => status("link copied ✓"),
    () => status("URL updated — copy it from the address bar")
  );
}

// Download the current dashboard as a standalone, self-contained HTML file.
function downloadHtml() {
  const style = document.querySelector("style").textContent;
  const content = document.querySelector(".dash").innerHTML;
  const html =
    `<!doctype html><html><head><meta charset="utf-8"><title>anofox-visualization dashboard</title>` +
    `<style>${style}</style></head><body style="background:#f4f6f9;padding:1.5rem">` +
    `<div class="dash">${content}</div><div id="dp-tip" class="dp-tip"></div>` +
    `<script>${SNAPSHOT_JS}<\/script></body></html>`;
  download(new Blob([html], { type: "text/html" }), "dashboard.html");
}

// Self-contained hover + tab switching for the exported snapshot.
const SNAPSHOT_JS = `(function(){
  var tip=document.getElementById('dp-tip');
  document.querySelectorAll('.dp-hit').forEach(function(el){
    var t=el.getAttribute('data-tip'); if(!t)return;
    el.addEventListener('mouseenter',function(){tip.textContent=t;tip.classList.add('show');});
    el.addEventListener('mousemove',function(e){tip.style.left=(e.clientX+14)+'px';tip.style.top=(e.clientY+14)+'px';});
    el.addEventListener('mouseleave',function(){tip.classList.remove('show');});
  });
  var wrap=document.querySelector('.tabwrap');
  document.querySelectorAll('.tab-btn').forEach(function(btn,i){
    btn.addEventListener('click',function(){
      wrap.querySelectorAll('.tabpane').forEach(function(p){p.style.display='none';});
      document.querySelectorAll('.tab-btn').forEach(function(b){b.classList.remove('active');});
      wrap.querySelectorAll('.tabpane')[i].style.display=''; btn.classList.add('active');
    });
  });
})();`;

function showError(grid, msg) {
  const d = document.createElement("div");
  d.className = "err";
  d.textContent = msg;
  grid.appendChild(d);
}

// Styled hover tooltips + click-to-highlight LINKING across all panels.
// Every mark carrying a `<title>` ("series: value") becomes hoverable; its series
// (the part before ": ") is stored on the element. Clicking a mark highlights
// that series everywhere and dims the rest; click again (or the background) clears.
let dpSelected = null;

function attachHover() {
  const tip = $("dp-tip");
  const marks = [
    ...document.querySelectorAll(".panel svg rect,.panel svg circle,.panel svg polygon,.panel svg polyline"),
  ].filter((el) => el.querySelector("title") && el.querySelector("title").textContent.trim());

  const apply = () => {
    if (!dpSelected) {
      for (const el of marks) el.style.opacity = "";
      return;
    }
    // Dim non-selected marks, but only inside a panel that actually contains the
    // selected series. A panel keyed by a different dimension (e.g. a line
    // filtered to one channel, keyed by week) has no matching mark, so it stays
    // fully visible instead of every point dimming away.
    const byPanel = new Map();
    for (const el of marks) {
      const fig = el.closest(".panel") || document.body;
      (byPanel.get(fig) || byPanel.set(fig, []).get(fig)).push(el);
    }
    for (const els of byPanel.values()) {
      const hasSel = els.some((el) => el.getAttribute("data-series") === dpSelected);
      for (const el of els) {
        el.style.opacity = !hasSel || el.getAttribute("data-series") === dpSelected ? "" : "0.15";
      }
    }
  };

  marks.forEach((el) => {
    const t = el.querySelector("title");
    const txt = t.textContent;
    const series = txt.includes(": ") ? txt.slice(0, txt.lastIndexOf(": ")) : txt;
    el.removeChild(t);
    el.setAttribute("data-series", series);
    el.setAttribute("data-tip", txt);
    el.classList.add("dp-hit");
    el.style.cursor = "pointer";
    el.addEventListener("mouseenter", () => {
      tip.textContent = txt;
      tip.classList.add("show");
    });
    el.addEventListener("mousemove", (e) => {
      tip.style.left = e.clientX + 14 + "px";
      tip.style.top = e.clientY + 14 + "px";
    });
    el.addEventListener("mouseleave", () => tip.classList.remove("show"));
    el.addEventListener("click", (e) => {
      e.stopPropagation();
      // Cross-filter: toggle `selected` to this value and re-query. Queries that
      // opt in (getvariable('selected')) filter; the rest just highlight it.
      dpFilter = dpFilter === series ? "" : series;
      dpSelected = dpFilter || null;
      run();
    });
  });
  apply();
}

// Click empty dashboard space to clear all cross-filters / selections.
document.querySelector(".dash").addEventListener("click", () => {
  const anyNamed = Object.values(dpXf).some((v) => v);
  if (dpFilter || dpSelected !== null || anyNamed) {
    dpFilter = "";
    dpSelected = null;
    for (const k in dpXf) dpXf[k] = "";
    run();
  }
});

boot().catch((e) => status("boot failed: " + e));
