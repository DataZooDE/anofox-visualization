// Browser dashboard builder — 100% client-side.
//   DuckDB-Wasm runs the SQL, duckplot (wasm) plans the ::ROLE annotations and
//   renders each panel to SVG. No server, no DuckDB extension.
import init, { plan, render_panel } from "./pkg/duckplot.js";

const SAMPLES = {
  "Weekly sessions": `-- Annotate result columns with ::ROLE casts. Statements without a role
-- (this CREATE) are setup; annotated SELECTs become panels.
CREATE OR REPLACE TABLE sessions AS SELECT * FROM (VALUES
  ('W1','app',30),('W1','web',22),('W1','api',12),
  ('W2','app',41),('W2','web',28),('W2','api',15),
  ('W3','app',26),('W3','web',33),('W3','api', 9),
  ('W4','app',48),('W4','web',30),('W4','api',18)
) t(week, channel, n);

SELECT 'Weekly sessions'::LABEL;

SELECT week::XAXIS, channel::CATEGORY, sum(n)::BARCHART_STACKED
FROM sessions GROUP BY ALL ORDER BY week, channel;

-- one line per channel — colours match the bars; click a series to highlight it
SELECT week::XAXIS, channel::CATEGORY, sum(n)::LINECHART
FROM sessions GROUP BY ALL ORDER BY week, channel;

SELECT channel::XAXIS, sum(n)::BARCHART
FROM sessions GROUP BY ALL ORDER BY sum(n) DESC;`,

  "Generated series": `SELECT 'Signal explorer'::LABEL;

SELECT i::XAXIS, sin(i/6.0)*40 + 50::LINECHART
FROM range(0, 40) t(i);

SELECT (i % 5)::XAXIS, count(*)::BARCHART
FROM range(0, 137) t(i) GROUP BY ALL ORDER BY 1;`,

  "Dropdown filter": `CREATE OR REPLACE TABLE sessions AS SELECT * FROM (VALUES
  ('W1','app',30),('W1','web',22),('W1','api',12),
  ('W2','app',41),('W2','web',28),('W2','api',15),
  ('W3','app',26),('W3','web',33),('W3','api', 9),
  ('W4','app',48),('W4','web',30),('W4','api',18)
) t(week, channel, n);

-- A dropdown: the column's values become options and its name ('channel')
-- becomes a DuckDB variable. Referenced below via getvariable('channel').
SELECT DISTINCT channel::DROPDOWN FROM sessions ORDER BY channel;

SELECT 'Sessions for the selected channel'::LABEL;

SELECT week::XAXIS, sum(n)::BARCHART
FROM sessions WHERE channel = getvariable('channel')
GROUP BY ALL ORDER BY week;`,

  "Layout & filters": `CREATE OR REPLACE TABLE sessions AS SELECT * FROM (VALUES
  ('W1','app','EU',30),('W1','web','EU',22),('W1','app','US',18),('W1','web','US',12),
  ('W2','app','EU',41),('W2','web','EU',28),('W2','app','US',20),('W2','web','US',15),
  ('W3','app','EU',26),('W3','web','EU',33),('W3','app','US',14),('W3','web','US',19),
  ('W4','app','EU',48),('W4','web','EU',30),('W4','app','US',22),('W4','web','US',17)
) t(week, channel, region, n);

SELECT 2::COLUMNS;                       -- a 2-column grid

SELECT 'Filters'::GROUP;                 -- put both dropdowns in one box
SELECT DISTINCT region::DROPDOWN  FROM sessions ORDER BY region;
SELECT DISTINCT channel::DROPDOWN FROM sessions ORDER BY channel;
SELECT 1::ENDGROUP;

SELECT 'Weekly sessions (filtered)'::LABEL;

-- widths are bootstrap-style, out of 12 columns:
SELECT 12::COL;                          -- full width
SELECT week::XAXIS, channel::CATEGORY, sum(n)::BARCHART_STACKED
FROM sessions WHERE region = getvariable('region')
GROUP BY ALL ORDER BY week, channel;

SELECT 8::COL;                           -- 8 of 12 (two-thirds)
SELECT week::XAXIS, channel::CATEGORY, sum(n)::LINECHART
FROM sessions WHERE region = getvariable('region')
GROUP BY ALL ORDER BY week, channel;

SELECT 4::COL;                           -- 4 of 12 (one-third), sits beside it
SELECT week::XAXIS, sum(n)::BARCHART
FROM sessions WHERE region = getvariable('region') AND channel = getvariable('channel')
GROUP BY ALL ORDER BY week;`,

  "Cross-filter": `CREATE OR REPLACE TABLE sessions AS SELECT * FROM (VALUES
  ('W1','app',30),('W1','web',22),('W1','api',12),
  ('W2','app',41),('W2','web',28),('W2','api',15),
  ('W3','app',26),('W3','web',33),('W3','api', 9),
  ('W4','app',48),('W4','web',30),('W4','api',18)
) t(week, channel, n);

SELECT 'Click a channel to filter the charts below (click empty space to clear)'::LABEL;

-- source: click a segment -> sets the cross-filter getvariable('selected')
SELECT 12::COL;
SELECT week::XAXIS, channel::CATEGORY, sum(n)::BARCHART_STACKED
FROM sessions GROUP BY ALL ORDER BY week, channel;

-- these opt in: show ONLY the clicked channel (all channels when nothing clicked)
SELECT 6::COL;
SELECT week::XAXIS, sum(n)::LINECHART
FROM sessions WHERE getvariable('selected') IN ('', channel)
GROUP BY ALL ORDER BY week;

SELECT 6::COL;
SELECT channel::XAXIS, sum(n)::BARCHART
FROM sessions WHERE getvariable('selected') IN ('', channel)
GROUP BY ALL ORDER BY channel;`,

  "KPIs, pie & table": `CREATE OR REPLACE TABLE sessions AS SELECT * FROM (VALUES
  ('W1','app',30),('W1','web',22),('W1','api',12),
  ('W2','app',41),('W2','web',28),('W2','api',15),
  ('W3','app',26),('W3','web',33),('W3','api', 9),
  ('W4','app',48),('W4','web',30),('W4','api',18)
) t(week, channel, n);

SELECT 'Overview'::LABEL;

-- KPI cards (big numbers), 4 columns each
SELECT 4::COL; SELECT sum(n)::METRIC,   'Total sessions'::LABEL FROM sessions;
SELECT 4::COL; SELECT count(DISTINCT channel)::METRIC, 'Channels'::LABEL FROM sessions;
SELECT 4::COL; SELECT round(avg(n),1)::METRIC, 'Avg / cell'::LABEL FROM sessions;

-- a pie by channel + a data table, side by side
SELECT 6::COL;
SELECT channel::CATEGORY, sum(n)::PIE FROM sessions GROUP BY ALL;

SELECT 6::COL;
SELECT week, channel, sum(n) AS sessions ::TABLE
FROM sessions GROUP BY ALL ORDER BY week, channel;`,

  "More charts & inputs": `CREATE OR REPLACE TABLE m AS
SELECT i AS id,
       (i * 7 % 13) + 5              AS value,
       ['app','web','api'][(i % 3) + 1] AS channel,
       'W' || ((i % 4) + 1)          AS week
FROM range(0, 120) t(i);

-- inputs: a number box + a dropdown, together
SELECT 'Controls'::GROUP;
SELECT 5 AS min_value ::NUMBER;
SELECT DISTINCT channel::DROPDOWN FROM m ORDER BY channel;
SELECT 1::ENDGROUP;

SELECT 'Distributions (value ≥ the number input)'::LABEL;

-- histogram of a numeric column
SELECT 6::COL;
SELECT value::HISTOGRAM FROM m WHERE value >= getvariable('min_value');

-- box plot of value by channel
SELECT 6::COL;
SELECT channel::XAXIS, value::BOXPLOT
FROM m WHERE value >= getvariable('min_value');

-- heatmap: week × channel coloured by average value
SELECT 12::COL;
SELECT week::XAXIS, channel::YAXIS, round(avg(value),1)::HEATMAP
FROM m GROUP BY ALL ORDER BY week, channel;`,

  "Tabs & formats": `CREATE OR REPLACE TABLE sales AS SELECT * FROM (VALUES
  ('W1','app',30,1200.0),('W1','web',22,900.0),('W1','api',12,400.0),
  ('W2','app',41,1600.0),('W2','web',28,1100.0),('W2','api',15,520.0),
  ('W3','app',26,980.0),('W3','web',33,1300.0),('W3','api', 9,330.0),
  ('W4','app',48,2000.0),('W4','web',30,1250.0),('W4','api',18,640.0)
) t(week, channel, n, revenue);

-- header KPIs (above the tabs) with value formats:
SELECT 4::COL; SELECT sum(revenue)::MONEY,  'Revenue'::LABEL  FROM sales;
SELECT 4::COL; SELECT sum(n)::COMPACT,      'Sessions'::LABEL FROM sales;
SELECT 4::COL; SELECT round(100.0*sum(n) FILTER (WHERE channel='app')/sum(n),0)::PERCENT,
                      'App share'::LABEL FROM sales;

SELECT 'Revenue'::TAB;
SELECT 12::COL;
SELECT week::XAXIS, channel::CATEGORY, sum(revenue)::BARCHART_STACKED
FROM sales GROUP BY ALL ORDER BY week, channel;

SELECT 'Sessions'::TAB;
SELECT 6::COL;
SELECT week::XAXIS, channel::CATEGORY, sum(n)::LINECHART
FROM sales GROUP BY ALL ORDER BY week, channel;
SELECT 6::COL;
SELECT channel::CATEGORY, sum(n)::PIE FROM sales GROUP BY ALL;

SELECT 'Data'::TAB;
SELECT 12::COL;
SELECT week, channel, n, revenue ::TABLE FROM sales ORDER BY week, channel;`,
};

const $ = (id) => document.getElementById(id);
const status = (t) => ($("status").textContent = t);

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
  const rows = res.toArray().map((row) => row.toJSON());
  return JSON.stringify(rows, (_, v) => (typeof v === "bigint" ? Number(v) : v));
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

  // samples dropdown
  const sel = $("samples");
  for (const name of Object.keys(SAMPLES)) {
    const o = document.createElement("option");
    o.value = o.textContent = name;
    sel.appendChild(o);
  }
  sel.onchange = () => ($("sql").value = SAMPLES[sel.value]);
  $("sql").value = decodeHashSql() || SAMPLES[Object.keys(SAMPLES)[0]];
  $("share").onclick = shareLink;
  $("dlhtml").onclick = downloadHtml;

  // layout: default columns-per-row (12-col bootstrap grid; panels span 12/cols)
  $("cols").onchange = () => {
    const v = $("cols").value;
    dpCols = v === "auto" ? 2 : parseInt(v);
    run();
  };

  $("run").disabled = false;
  $("run").onclick = run;
  status(backend === "live" ? "live DuckDB · ready" : "DuckDB-Wasm · ready");
  run();
}

const role = (s, name) => s.roles.some((r) => r[1] === name);
const INPUTS = ["DROPDOWN", "NUMBER", "DATE", "TEXT"];
const inputKind = (s) => INPUTS.find((k) => role(s, k));
const isInput = (s) => !!inputKind(s);
const METRICS = ["METRIC", "MONEY", "PERCENT", "COMPACT"];
const metricRole = (s) => s.roles.find((r) => METRICS.includes(r[1]));
const isHeading = (s) => s.roles.length === 1 && s.roles[0][1] === "LABEL";
const directive = (s) => ["COLUMNS", "GROUP", "ENDGROUP", "SPAN", "TAB"].find((d) => role(s, d));
let dpVars = {}; // DuckDB variable name -> selected value (persists across runs)
let dpCols = 2; // default panels-per-row on the 12-column grid
let dpFilter = ""; // cross-filter: the clicked value, exposed as getvariable('selected')

async function run() {
  const grid = $("grid");
  grid.innerHTML = "";
  document.querySelector(".dash").querySelectorAll(".tabbar,.tabwrap").forEach((e) => e.remove());
  status("running…");
  let stmts;
  try {
    stmts = JSON.parse(plan($("sql").value));
  } catch (e) {
    return showError(grid, String(e));
  }

  // Cross-filter value (from the last click) — available to every query as
  // getvariable('selected'); e.g. `WHERE getvariable('selected') IN ('', channel)`.
  try {
    await runSql(`SET VARIABLE selected = '${dpFilter.replace(/'/g, "''")}'`);
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
        const varname = Object.keys(rows[0])[0];
        if (kind === "DROPDOWN") {
          const options = rows.map((r) => String(r[varname]));
          if (dpVars[varname] === undefined || !options.includes(dpVars[varname])) dpVars[varname] = options[0];
          dd[i] = { kind, varname, options };
        } else {
          // number / date / text: the query's value is the default
          if (dpVars[varname] === undefined) dpVars[varname] = String(rows[0][varname] ?? "");
          dd[i] = { kind, varname };
        }
        // numbers unquoted (so getvariable() is numeric); others quoted
        const v = String(dpVars[varname]);
        const lit = kind === "NUMBER" ? v || "0" : `'${v.replace(/'/g, "''")}'`;
        await runSql(`SET VARIABLE ${varname} = ${lit}`);
      }
    } catch (e) {
      showError(grid, `${s.sql}\n\n${e}`);
    }
  }

  // Render pass: place controls / headings / charts in document order into the
  // current container (the grid, or an open ::GROUP box). ::COLUMNS sets the
  // grid columns; ::SPAN widens the next panel.
  let container = grid;
  let curGrid = grid; // the active surface (the main grid, or the current tab pane)
  let tabBar = null;
  let tabWrap = null;
  let nextSpan = 0;
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
        $("cols").value = n > 0 && n <= 3 ? String(n) : "auto";
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
      } else if (d === "TAB") {
        const name = String((await firstValue(s)) ?? "Tab");
        if (!tabBar) {
          const dash = document.querySelector(".dash");
          tabBar = document.createElement("div");
          tabBar.className = "tabbar";
          tabWrap = document.createElement("div");
          tabWrap.className = "tabwrap";
          dash.appendChild(tabBar);
          dash.appendChild(tabWrap);
        }
        const pane = document.createElement("div");
        pane.className = "grid tabpane";
        pane.style.display = "none";
        tabWrap.appendChild(pane);
        const btn = document.createElement("button");
        btn.className = "tab-btn";
        btn.textContent = name;
        btn.onclick = () => {
          tabWrap.querySelectorAll(".tabpane").forEach((p) => (p.style.display = "none"));
          tabBar.querySelectorAll(".tab-btn").forEach((b) => b.classList.remove("active"));
          pane.style.display = "";
          btn.classList.add("active");
        };
        tabBar.appendChild(btn);
        if (tabBar.children.length === 1) {
          pane.style.display = "";
          btn.classList.add("active");
        }
        curGrid = pane;
        container = pane;
      } else if (isInput(s)) {
        if (dd[i]) container.appendChild(makeControl(dd[i], container === curGrid));
      } else {
        const rowsJson = await runSql(s.sql);
        const span = Math.min(12, nextSpan || defaultSpan);
        const mkPanel = () => {
          const fig = document.createElement("figure");
          fig.className = "panel";
          if (container === curGrid) fig.style.gridColumn = `span ${span}`;
          return fig;
        };
        if (isHeading(s)) {
          const rows = JSON.parse(rowsJson);
          const h = document.createElement("h2");
          h.className = "section";
          h.textContent = rows[0] ? Object.values(rows[0])[0] : "";
          container.appendChild(h);
        } else if (role(s, "TABLE")) {
          const fig = mkPanel();
          fig.appendChild(renderTable(JSON.parse(rowsJson)));
          container.appendChild(fig);
          panels++;
        } else if (metricRole(s)) {
          const r0 = JSON.parse(rowsJson)[0] || {};
          const mr = metricRole(s);
          const lr = s.roles.find((r) => r[1] === "LABEL");
          const fig = mkPanel();
          fig.classList.add("metric");
          fig.innerHTML =
            `<div class="metric-value">${fmtNum(r0["c" + mr[0]], mr[1])}</div>` +
            `<div class="metric-cap">${escapeHtml(lr ? r0["c" + lr[0]] : "")}</div>`;
          container.appendChild(fig);
          panels++;
        } else {
          const fig = mkPanel();
          fig.innerHTML = render_panel(rowsJson, JSON.stringify(s.roles), 460, 300);
          container.appendChild(fig);
          panels++;
        }
        nextSpan = 0;
      }
    } catch (e) {
      showError(container, `${s.sql}\n\n${e}`);
    }
  }
  attachHover();
  addExportButtons();
  status(`${panels} panel${panels === 1 ? "" : "s"}`);
}

// A labelled <select>; changing it re-runs the dashboard. `bar` wraps a
// stand-alone control in its own spanning row (grouped ones sit inline).
function makeControl(meta, bar) {
  const wrap = document.createElement("label");
  wrap.className = "control";
  wrap.textContent = meta.varname + ":";
  let input;
  if (meta.kind === "DROPDOWN") {
    input = document.createElement("select");
    for (const o of meta.options) {
      const opt = document.createElement("option");
      opt.value = opt.textContent = o;
      if (o === dpVars[meta.varname]) opt.selected = true;
      input.appendChild(opt);
    }
  } else {
    input = document.createElement("input");
    input.type = meta.kind === "NUMBER" ? "number" : meta.kind === "DATE" ? "date" : "text";
    input.value = dpVars[meta.varname] ?? "";
  }
  input.onchange = () => {
    dpVars[meta.varname] = input.value;
    run();
  };
  wrap.appendChild(input);
  if (!bar) return wrap;
  const box = document.createElement("div");
  box.className = "controls";
  box.appendChild(wrap);
  return box;
}

// A ::TABLE result → an HTML table (original column names, first 500 rows).
function renderTable(rows) {
  const t = document.createElement("table");
  t.className = "dp-table";
  if (!rows.length) {
    t.textContent = "(no rows)";
    return t;
  }
  const cols = Object.keys(rows[0]);
  const hr = t.createTHead().insertRow();
  for (const c of cols) {
    const th = document.createElement("th");
    th.textContent = c;
    hr.appendChild(th);
  }
  const tb = t.createTBody();
  for (const r of rows.slice(0, 500)) {
    const tr = tb.insertRow();
    for (const c of cols) {
      let v = r[c];
      // DuckDB-Wasm serialises HUGEINT/DECIMAL as a quote-wrapped string.
      if (typeof v === "string" && /^"-?[\d.]+"$/.test(v)) v = v.slice(1, -1);
      tr.insertCell().textContent = v == null ? "" : v;
    }
  }
  return t;
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
  return [...table.rows]
    .map((r) => [...r.cells].map((c) => `"${c.textContent.replace(/"/g, '""')}"`).join(","))
    .join("\n");
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
    `<!doctype html><html><head><meta charset="utf-8"><title>duckplot dashboard</title>` +
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
    for (const el of marks) {
      const s = el.getAttribute("data-series");
      el.style.opacity = !dpSelected || s === dpSelected ? "" : "0.15";
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

// Click empty dashboard space to clear the cross-filter / selection.
document.querySelector(".dash").addEventListener("click", () => {
  if (dpFilter || dpSelected !== null) {
    dpFilter = "";
    dpSelected = null;
    run();
  }
});

boot().catch((e) => status("boot failed: " + e));
