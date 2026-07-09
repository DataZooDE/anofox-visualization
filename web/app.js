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
  $("sql").value = SAMPLES[Object.keys(SAMPLES)[0]];

  // layout: columns selector (CSS grid-template-columns)
  $("cols").onchange = () => {
    const v = $("cols").value;
    $("grid").style.gridTemplateColumns = v === "auto" ? "" : `repeat(${v}, minmax(0, 1fr))`;
  };

  $("run").disabled = false;
  $("run").onclick = run;
  status(backend === "live" ? "live DuckDB · ready" : "DuckDB-Wasm · ready");
  run();
}

const isDropdown = (s) => s.roles.some((r) => r[1] === "DROPDOWN");
const isHeading = (s) => s.roles.length === 1 && s.roles[0][1] === "LABEL";
let dpVars = {}; // DuckDB variable name -> selected value (persists across runs)

async function run() {
  const grid = $("grid");
  grid.innerHTML = "";
  status("running…");
  let stmts;
  try {
    stmts = JSON.parse(plan($("sql").value));
  } catch (e) {
    return showError(grid, String(e));
  }

  // Pass 1: setup + inputs — set DuckDB variables before any chart runs.
  const controls = [];
  for (const s of stmts) {
    try {
      if (s.setup) {
        await runSql(s.sql);
      } else if (isDropdown(s)) {
        const rows = JSON.parse(await runSql(s.sql));
        if (!rows.length) continue;
        const varname = Object.keys(rows[0])[0];
        const options = rows.map((r) => String(r[varname]));
        if (dpVars[varname] === undefined || !options.includes(dpVars[varname])) dpVars[varname] = options[0];
        await runSql(`SET VARIABLE ${varname} = '${dpVars[varname].replace(/'/g, "''")}'`);
        controls.push({ varname, options });
      }
    } catch (e) {
      showError(grid, `${s.sql}\n\n${e}`);
    }
  }
  if (controls.length) renderControls(grid, controls);

  // Pass 2: headings + charts.
  let panels = 0;
  for (const s of stmts) {
    if (s.setup || isDropdown(s)) continue;
    try {
      const rowsJson = await runSql(s.sql);
      if (isHeading(s)) {
        const rows = JSON.parse(rowsJson);
        const h = document.createElement("h2");
        h.className = "section";
        h.textContent = rows[0] ? Object.values(rows[0])[0] : "";
        grid.appendChild(h);
      } else {
        const fig = document.createElement("figure");
        fig.className = "panel";
        fig.innerHTML = render_panel(rowsJson, JSON.stringify(s.roles), 460, 300);
        grid.appendChild(fig);
        panels++;
      }
    } catch (e) {
      showError(grid, `${s.sql}\n\n${e}`);
    }
  }
  attachHover();
  status(`${panels} panel${panels === 1 ? "" : "s"}`);
}

// Build a filter bar of <select> controls; changing one re-runs the dashboard.
function renderControls(grid, controls) {
  const bar = document.createElement("div");
  bar.className = "controls";
  for (const c of controls) {
    const wrap = document.createElement("label");
    wrap.className = "control";
    wrap.textContent = c.varname + ":";
    const sel = document.createElement("select");
    for (const o of c.options) {
      const opt = document.createElement("option");
      opt.value = opt.textContent = o;
      if (o === dpVars[c.varname]) opt.selected = true;
      sel.appendChild(opt);
    }
    sel.onchange = () => {
      dpVars[c.varname] = sel.value;
      run();
    };
    wrap.appendChild(sel);
    bar.appendChild(wrap);
  }
  grid.appendChild(bar);
}

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
      dpSelected = dpSelected === series ? null : series;
      apply();
    });
  });
  apply();
}

// Click empty space to clear the linked selection.
document.addEventListener("click", () => {
  if (dpSelected !== null) {
    dpSelected = null;
    document.querySelectorAll(".dp-hit").forEach((el) => (el.style.opacity = ""));
  }
});

boot().catch((e) => status("boot failed: " + e));
