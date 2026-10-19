// Explore data (issue #97): the pure parts of the Explore page — the profiling
// SQL, the column-card model, chart suggestions, the starter dashboard, joins
// between tables, data-quality hints and the small SQL edits the page makes.
//
// No DOM, no app.js globals, no engine: app.js runs the SQL these functions
// write and hands the rows back, so tests/web/explore_test.mjs drives the same
// code under Node (and DuckDB-Wasm), and the starter dashboards are linted with
// `dashboard --check --strict` (tests/explore_starter.rs).
//
// Every numeric statistic reads a value the way the ::PAGED table does — the
// statistics and distributions come from its page_sql() builder, passed in as
// `pageSql`, which counts only finite values (#74) — and the extra statistics
// written here use the same isfinite() guard.
//
// sec-review-256 last check: every exported function here that takes an
// options object (destructured in its signature, or read key by key off a
// parameter named `opts`) runs it through ownOpts first — a null-prototype,
// shallow copy of its own enumerable properties — so a missing key is
// always `undefined`, never read off a polluted Object.prototype.
import { ownOpts } from "./lint.js";

// Above this many rows a table is profiled on a sample of SAMPLE_ROWS rows.
export const SAMPLE_OVER = 200000;
export const SAMPLE_ROWS = 100000;
// Columns per statistics / distribution query (a wide table takes a few).
export const STAT_BATCH = 32;

export const qid = (s) => `"${String(s).replace(/"/g, '""')}"`;
export const sqlStr = (s) => `'${String(s).replace(/'/g, "''")}'`;

// A DuckDB type → the card kind: num | date | time | bool | text | other.
export function colKind(type) {
  const t = String(type || "").toUpperCase().trim();
  if (/\[\]$|^(STRUCT|MAP|UNION|LIST)\b|^(BLOB|BYTEA|BIT|INTERVAL|TIME|TIME WITH TIME ZONE|TIMETZ|GEOMETRY)$/.test(t)) return "other";
  if (/^(TINYINT|SMALLINT|INTEGER|INT|BIGINT|HUGEINT|UTINYINT|USMALLINT|UINTEGER|UBIGINT|UHUGEINT|FLOAT|REAL|DOUBLE|DECIMAL|NUMERIC)\b/.test(t)) return "num";
  if (t === "DATE") return "date";
  if (/^(TIMESTAMP|DATETIME)/.test(t)) return "time";
  if (t === "BOOLEAN" || t === "BOOL") return "bool";
  if (/^(VARCHAR|TEXT|STRING|CHAR|BPCHAR|ENUM|UUID|JSON)\b/.test(t)) return "text";
  return "other";
}

// A column name that reads as an identifier / key rather than a measure.
export const idLike = (name) => /(^|_)(id|key|uuid|guid|code|zip|postcode|sku)$|^id_|Id$/i.test(String(name));
// … a rate / level that is averaged, not summed.
const avgLike = (name) => /(rate|ratio|pct|percent|share|score|price|avg|average|mean|median|aov|temp|age|lat|lon|lng|latitude|longitude|margin|prob|level)/i.test(String(name));

// A column name as words for a title or label: "order_total" → "Order total".
export function words(name) {
  const s = String(name).replace(/[_]+/g, " ").replace(/([a-z])([A-Z])/g, "$1 $2").replace(/\s+/g, " ").trim();
  return s ? s[0].toUpperCase() + s.slice(1) : String(name);
}
const lower = (name) => {
  const w = words(name);
  return /^[A-Z][a-z]/.test(w) ? w[0].toLowerCase() + w.slice(1) : w;
};

// The relation a profile reads: the table itself, or a sample of a big one.
export function profileBase(fq, total) {
  const sampled = Number(total) > SAMPLE_OVER;
  return {
    base: sampled ? `SELECT * FROM ${fq} USING SAMPLE reservoir(${SAMPLE_ROWS} ROWS) REPEATABLE (97)` : `SELECT * FROM ${fq}`,
    sampled,
    rows: sampled ? SAMPLE_ROWS : Number(total) || 0,
  };
}

// The catalog: every table / view with its (estimated) row count and its
// column count, then every column (for the joins).
export const CATALOG_SQL =
  "SELECT database_name, schema_name, table_name, 'table' AS kind, estimated_size AS n, column_count AS cols " +
  "FROM duckdb_tables() WHERE NOT internal " +
  "UNION ALL SELECT database_name, schema_name, view_name, 'view', NULL, column_count FROM duckdb_views() " +
  "WHERE NOT internal ORDER BY 1, 2, 3";
export const COLUMNS_SQL =
  "SELECT database_name, schema_name, table_name, column_name, data_type FROM duckdb_columns() WHERE NOT internal";

export const countSql = (fq) => `SELECT count(*) AS n FROM ${fq}`;
export const describeSql = (fq) => `DESCRIBE SELECT * FROM ${fq}`;

// A value read as a finite DOUBLE (NULL otherwise) — the ::PAGED rule.
const finite = (c) => `(CASE WHEN isfinite(TRY_CAST(${c} AS DOUBLE)) THEN TRY_CAST(${c} AS DOUBLE) END)`;

// One row of what the ::PAGED statistics don't have: the 5th / 95th
// percentiles of numbers, the first / last date (as ISO text), exact distinct
// counts of key-like columns (duplicate keys) and how many text values read as
// numbers (mixed types). Columns: q05_i, q95_i, t0_i, t1_i, dx_i, tn_i.
export function extrasSql(base, cols) {
  const sel = ["count(*) AS n"];
  cols.forEach((c, i) => {
    const q = qid(c.name);
    if (c.kind === "num")
      sel.push(`quantile_cont(${finite(q)}, 0.05) AS q05_${i}, quantile_cont(${finite(q)}, 0.95) AS q95_${i}`);
    else if (c.kind === "date" || c.kind === "time")
      sel.push(`CAST(min(${q}) AS VARCHAR) AS t0_${i}, CAST(max(${q}) AS VARCHAR) AS t1_${i}`);
    else if (c.kind === "text") sel.push(`count(TRY_CAST(${q} AS DOUBLE)) AS tn_${i}`);
    if (idLike(c.name) && c.kind !== "other") sel.push(`count(DISTINCT ${q}) AS dx_${i}`);
  });
  return `SELECT ${sel.join(", ")} FROM (${base}) _x`;
}

// The grain of a time column's sparkline: day up to ~3 months, week up to
// ~2 years, month up to ~30 years, else year.
export function timeGrain(t0, t1) {
  const a = Date.parse(String(t0 || "").slice(0, 10));
  const b = Date.parse(String(t1 || "").slice(0, 10));
  if (!isFinite(a) || !isFinite(b)) return null;
  const days = (b - a) / 864e5;
  return days <= 92 ? "day" : days <= 730 ? "week" : days <= 365 * 30 ? "month" : "year";
}

// Rows per bucket of each time column (`cols`: [{i, name, grain}]): rows
// (c, b, n) — the column index, the bucket start as text, the count.
export function seriesSql(base, cols) {
  if (!cols.length) return "";
  const parts = cols.map(
    (c) =>
      `SELECT ${c.i} AS c, CAST(CAST(date_trunc('${c.grain}', ${qid(c.name)}) AS DATE) AS VARCHAR) AS b, count(*) AS n ` +
      `FROM _f WHERE ${qid(c.name)} IS NOT NULL GROUP BY 2`
  );
  return `WITH _f AS (${base}) ${parts.join(" UNION ALL ")} ORDER BY c, b`;
}

const num = (v) => (v == null || v === "" ? null : Number.isFinite(Number(v)) ? Number(v) : null);

// The column cards from the query results: `cols` = [{name, type}] (DESCRIBE),
// `stats` = the ::PAGED statistics row (n, n_i, d_i, s_i, m_i, md_i, lo_i,
// hi_i), `prof` = its distribution rows (c, t, k, v, n), `extras` = the
// extrasSql row, `series` = the seriesSql rows. Indices are per batch: pass
// `at` = the batch's first column index to merge batches.
// sec-review-256 last check: ownOpts first (a signature-destructured
// parameter reads inherited properties too).
export function buildCards(cols, opts = {}) {
  const { stats = {}, prof = [], extras = {}, series = [] } = ownOpts(opts);
  return cols.map((c, i) => {
    const kind = c.kind || colKind(c.type);
    const n = num(stats.n ?? extras.n) ?? 0;
    const nonnull = num(stats["n" + i]) ?? 0;
    const card = {
      name: c.name,
      type: c.type,
      kind,
      n,
      nonnull,
      nulls: Math.max(0, n - nonnull),
      nullPct: n ? (100 * (n - nonnull)) / n : 0,
      distinct: num(stats["d" + i]),
    };
    const dx = num(extras["dx_" + i]);
    if (dx != null) card.distinct = dx; // exact for keys
    if (kind === "num") {
      Object.assign(card, {
        min: num(stats["lo" + i]),
        max: num(stats["hi" + i]),
        mean: num(stats["m" + i]),
        median: num(stats["md" + i]),
        sum: num(stats["s" + i]),
        p05: num(extras["q05_" + i]),
        p95: num(extras["q95_" + i]),
      });
      const hist = Array(10).fill(0);
      for (const r of prof) if (Number(r.c) === i && r.t === "h" && r.k != null) hist[Math.min(9, Math.max(0, Number(r.k)))] += Number(r.n) || 0;
      card.hist = hist;
    }
    if (kind === "date" || kind === "time") {
      card.tmin = extras["t0_" + i] ?? null;
      card.tmax = extras["t1_" + i] ?? null;
      card.grain = timeGrain(card.tmin, card.tmax);
      card.series = series.filter((r) => Number(r.c) === i).map((r) => ({ b: String(r.b), n: Number(r.n) || 0 }));
    }
    if (kind === "text" || kind === "bool" || kind === "num") {
      card.top = prof.filter((r) => Number(r.c) === i && r.t === "t").map((r) => ({ v: String(r.v), n: Number(r.n) || 0 }));
    }
    if (kind === "text") card.numericText = num(extras["tn_" + i]) ?? 0;
    card.hints = qualityHints(card);
    return card;
  });
}

// Data-quality hints of one card: [{code, text}].
export function qualityHints(c) {
  const out = [];
  if (!c.n) return out;
  if (c.nonnull === 0) out.push({ code: "all-null", text: "Every value is empty" });
  else if (c.nullPct >= 50) out.push({ code: "mostly-null", text: `${Math.round(c.nullPct)}% of the values are empty` });
  if (c.nonnull > 1 && c.distinct === 1) out.push({ code: "constant", text: "One value in every row" });
  if (c.kind === "text" && c.numericText > 0 && c.numericText < c.nonnull) {
    const pct = (100 * c.numericText) / c.nonnull;
    if (pct >= 5 && pct <= 95) out.push({ code: "mixed-types", text: `${Math.round(pct)}% of the values are numbers, the rest text` });
  }
  if (c.kind === "num" && c.p05 != null && c.p95 != null && c.min != null && c.max != null) {
    const w = c.p95 - c.p05;
    if (w > 0 && (c.max > c.p95 + 3 * w || c.min < c.p05 - 3 * w)) out.push({ code: "outliers", text: "Extreme values far outside the usual range" });
  }
  if (idLike(c.name) && c.distinct != null && c.nonnull > 1 && c.distinct < c.nonnull)
    out.push({ code: "duplicate-keys", text: `${(c.nonnull - c.distinct).toLocaleString("en-US")} repeated values in a key-like column` });
  return out;
}

// A short statistic as text (counts, measures, percentages).
export function fmtStat(v, digits = 3) {
  if (v == null || !Number.isFinite(Number(v))) return "–";
  const x = Number(v);
  const a = Math.abs(x);
  if (a !== 0 && (a >= 1e15 || a < 1e-4)) return x.toExponential(2);
  if (a >= 1e6) return (x / 1e6).toLocaleString("en-US", { maximumFractionDigits: 2 }) + " M";
  if (Number.isInteger(x)) return x.toLocaleString("en-US");
  return x.toLocaleString("en-US", { maximumSignificantDigits: Math.max(digits, Math.ceil(Math.log10(a + 1)) + 2) });
}

// "48 M" style row counts for the catalog.
export function fmtCount(n) {
  const x = Number(n);
  if (!Number.isFinite(x)) return "";
  if (x >= 1e9) return (x / 1e9).toFixed(x >= 1e10 ? 0 : 1) + " B";
  if (x >= 1e6) return (x / 1e6).toFixed(x >= 1e7 ? 0 : 1) + " M";
  if (x >= 1e4) return (x / 1e3).toFixed(x >= 1e5 ? 0 : 1) + " k";
  return x.toLocaleString("en-US");
}

// One sentence per card for screen readers (and the mini chart's label).
export function cardSummary(c) {
  const parts = [`${c.name}: ${c.type}`];
  if (c.n) parts.push(`${fmtStat(c.nullPct, 2)}% empty`);
  if (c.distinct != null) parts.push(`about ${fmtStat(c.distinct)} distinct values`);
  if (c.kind === "num" && c.min != null) parts.push(`from ${fmtStat(c.min)} to ${fmtStat(c.max)}, median ${fmtStat(c.median)}`);
  if ((c.kind === "date" || c.kind === "time") && c.tmin) parts.push(`from ${c.tmin} to ${c.tmax}`);
  if (c.top && c.top.length && c.kind !== "num") parts.push(`most common ${c.top.map((t) => `${t.v} (${t.n.toLocaleString("en-US")})`).join(", ")}`);
  return parts.join("; ") + ".";
}

// ---- charts ----

// The chart kinds the Explore page offers for a column, best first:
// [{id, label}] — a histogram / box plot of a measure, bars of the top values,
// a line over time, a scatter against another measure.
export function chartChoices(card, cards = []) {
  const others = cards.filter((c) => c.kind === "num" && c.name !== card.name && !idLike(c.name));
  if (card.kind === "num") {
    const out = [{ id: "hist", label: "Histogram" }, { id: "box", label: "Box plot" }];
    if (card.distinct != null && card.distinct <= 30) out.splice(1, 0, { id: "bars", label: "Bar of top values" });
    if (others.length) out.push({ id: "scatter", label: "Scatter" });
    if (cards.some((c) => c.kind === "date" || c.kind === "time")) out.push({ id: "line", label: "Line over time" });
    return out;
  }
  if (card.kind === "date" || card.kind === "time") return [{ id: "line", label: "Rows over time" }, { id: "bars", label: "Bar of top values" }];
  if (card.kind === "text" || card.kind === "bool") {
    const out = [{ id: "bars", label: "Bar of top values" }];
    if (cards.some((c) => c.kind === "date" || c.kind === "time") && card.distinct != null && card.distinct <= 7) out.push({ id: "line", label: "Line over time" });
    return out;
  }
  return [];
}

const firstTime = (cards) => cards.find((c) => (c.kind === "date" || c.kind === "time") && c.nonnull > 0);
const grainOf = (c) => (c && c.grain && c.grain !== "year" ? c.grain : c && c.grain === "year" ? "year" : "month");

// The annotated SQL of a column chart (one dashboard statement, `;`-ended).
// `opts.vs` = the other measure of a scatter.
// sec-review-256 last check: ownOpts first (same reason as profileTable).
export function columnChartSql(fq, card, kind, cards = [], opts = {}) {
  const o = ownOpts(opts);
  const q = qid(card.name);
  const nm = String(card.name);
  switch (kind) {
    case "hist":
      return `SELECT ${q}::HISTOGRAM, ${sqlStr("Distribution of " + nm)}::TITLE\nFROM (SELECT ${q} FROM ${fq} WHERE ${q} IS NOT NULL) _s USING SAMPLE reservoir(5000 ROWS) REPEATABLE (97);`;
    case "box":
      return `SELECT ${sqlStr(nm)}::XAXIS, ${q}::BOXPLOT, ${sqlStr("Spread of " + nm)}::TITLE\nFROM (SELECT ${q} FROM ${fq} WHERE ${q} IS NOT NULL) _s USING SAMPLE reservoir(5000 ROWS) REPEATABLE (97);`;
    case "bars":
      return `SELECT coalesce(CAST(${q} AS VARCHAR), '(empty)')::XAXIS, count(*)::BARCHART, ${sqlStr("Most common " + nm)}::TITLE\nFROM ${fq} GROUP BY 1 ORDER BY 2 DESC, 1 LIMIT 15;`;
    case "scatter": {
      const vs = o.vs || (cards.find((c) => c.kind === "num" && c.name !== card.name && !idLike(c.name)) || {}).name;
      if (!vs) return "";
      return `SELECT ${qid(vs)}::XAXIS, ${q}::SCATTER, ${sqlStr(nm + " against " + vs)}::TITLE\nFROM (SELECT ${qid(vs)}, ${q} FROM ${fq} WHERE ${q} IS NOT NULL AND ${qid(vs)} IS NOT NULL) _s USING SAMPLE reservoir(2000 ROWS) REPEATABLE (97);`;
    }
    case "line": {
      const t = card.kind === "date" || card.kind === "time" ? card : firstTime(cards);
      if (!t) return "";
      const g = grainOf(t);
      const x = `CAST(date_trunc('${g}', ${qid(t.name)}) AS DATE)`;
      if (t === card)
        return `SELECT ${x}::XAXIS, count(*)::LINECHART, ${sqlStr(`Rows per ${g}`)}::TITLE\nFROM ${fq} WHERE ${q} IS NOT NULL GROUP BY 1 ORDER BY 1;`;
      if (card.kind === "num")
        return `SELECT ${x}::XAXIS, sum(${q})::LINECHART, ${sqlStr(`${nm} per ${g}`)}::TITLE\nFROM ${fq} WHERE ${qid(t.name)} IS NOT NULL GROUP BY 1 ORDER BY 1;`;
      return `SELECT ${x}::XAXIS, coalesce(CAST(${q} AS VARCHAR), '(empty)')::CATEGORY, count(*)::LINECHART, ${sqlStr(`Rows per ${g} by ${nm}`)}::TITLE\nFROM ${fq} WHERE ${qid(t.name)} IS NOT NULL GROUP BY 1, 2 ORDER BY 1, 2;`;
    }
  }
  return "";
}

// The chart kinds for a query result: [{id, label}].
export const RESULT_CHARTS = [
  { id: "bar", label: "Bar", role: "BARCHART" },
  { id: "line", label: "Line", role: "LINECHART" },
  { id: "area", label: "Area", role: "AREACHART" },
  { id: "scatter", label: "Scatter", role: "SCATTER" },
  { id: "hist", label: "Histogram", role: "HISTOGRAM" },
];

// Pick an x and a y column of a result: x = the first non-numeric (else the
// first) column, y = the first numeric column that isn't x.
export function resultAxes(cols, rows) {
  const isNum = (c) => rows.slice(0, 50).some((r) => r[c] != null) && rows.slice(0, 50).every((r) => r[c] == null || (typeof r[c] === "number" && Number.isFinite(r[c])) || /^-?\d+(\.\d+)?([eE][+-]?\d+)?$/.test(String(r[c])));
  const nums = cols.filter(isNum);
  const x = cols.find((c) => !nums.includes(c)) ?? cols[0];
  const y = nums.find((c) => c !== x) ?? nums[0] ?? cols[1] ?? cols[0];
  return { x, y, nums };
}

// The annotated SQL that charts a query result (the query becomes a subquery).
export function resultChartSql(sql, kind, x, y) {
  const inner = stripSemi(sql);
  const k = RESULT_CHARTS.find((c) => c.id === kind) || RESULT_CHARTS[0];
  const title = sqlStr(kind === "hist" ? `Distribution of ${y}` : `${y} by ${x}`);
  if (kind === "hist") return `SELECT ${qid(y)}::HISTOGRAM, ${title}::TITLE\nFROM (\n${inner}\n) _q;`;
  return `SELECT ${qid(x)}::XAXIS, ${qid(y)}::${k.role}, ${title}::TITLE\nFROM (\n${inner}\n) _q;`;
}

// ---- the starter dashboard ----

const MEASURE_RE = /(revenue|sales|amount|total|value|price|cost|profit|qty|quantity|units|volume|spend|count|orders|n)$/i;
const TIME_RE = /(date|day|time|ts|timestamp|created|updated|at|month|period)$/i;

// A variable name derived from a column: f_<snake case>.
export function varName(col, taken = new Set()) {
  let v = "f_" + String(col).toLowerCase().replace(/[^a-z0-9_]+/g, "_").replace(/^_+|_+$/g, "").slice(0, 40);
  if (v === "f_") v = "f_col";
  let k = v,
    i = 2;
  while (taken.has(k)) k = `${v}_${i++}`;
  taken.add(k);
  return k;
}

// What a starter dashboard is built from: the date to filter on, the measures,
// the categories (chart and filter).
export function starterPlan(cards) {
  const usable = (c) => c.nonnull > 0 && !(c.hints || []).some((h) => h.code === "constant");
  const dates = cards.filter((c) => (c.kind === "date" || c.kind === "time") && usable(c) && c.tmin && c.tmin !== c.tmax);
  dates.sort((a, b) => TIME_RE.test(b.name) - TIME_RE.test(a.name));
  const measures = cards.filter((c) => c.kind === "num" && usable(c) && !idLike(c.name) && (c.distinct ?? 2) > 1);
  measures.sort((a, b) => MEASURE_RE.test(b.name) - MEASURE_RE.test(a.name) || avgLike(a.name) - avgLike(b.name));
  const cats = cards.filter(
    (c) => (c.kind === "text" || c.kind === "bool") && usable(c) && !idLike(c.name) && c.distinct != null && c.distinct >= 2 && c.distinct <= 50 && c.nullPct < 50
  );
  cats.sort((a, b) => a.distinct - b.distinct);
  const filter = cats.find((c) => c.distinct <= 60) || null;
  return { date: dates[0] || null, measures: measures.slice(0, 2), cats, filter };
}

// A starter dashboard for a table: KPI tiles (rows, key sums), the data's
// latest date (::ASOF), filters (::PERIOD for the date, ::BUTTONS / ::DROPDOWN
// for a category with few values), a line over time, top-N bars for the main
// categories, the main measure's distribution, and the paged table. `cards` =
// the table's column cards (buildCards). Returns the dashboard SQL.
// sec-review-256 last check: ownOpts first (same reason as buildCards).
export function starterDashboard(opts) {
  const { name, fq, cards } = ownOpts(opts);
  const p = starterPlan(cards);
  const title = String(name || "Table");
  const out = [
    `-- A starter dashboard for ${title} (${fq}), written by Explore data from the`,
    "-- table's profile. Every panel is a query on the table: edit, add, remove.",
    "",
  ];
  // The filters, as conditions every panel repeats (no macro or view: a
  // client that runs each statement on its own sees them too). An unset
  // variable filters nothing.
  const conds = [];
  const vars = new Set(["period"]);
  if (p.date) {
    const d = qid(p.date.name);
    out.push(`SELECT max(${d})::ASOF FROM ${fq};`);
    const day = `strftime(CAST(max(${d}) AS DATE), '%Y-%m-%d')`;
    out.push(
      `SELECT 'presets=30d,90d,12m,ytd; compare=prev; from=' || strftime(CAST(min(${d}) AS DATE), '%Y-%m-%d') ||\n` +
        `       '; to=' || ${day} || '; today=' || ${day} AS period ::PERIOD, 'Period'::LABEL\nFROM ${fq} WHERE ${d} IS NOT NULL;`
    );
    conds.push(
      `CAST(${d} AS DATE) BETWEEN coalesce(getvariable('period_from')::DATE, DATE '0001-01-01') AND coalesce(getvariable('period_to')::DATE, DATE '9999-12-31')`
    );
  }
  if (p.filter) {
    const f = qid(p.filter.name);
    const fvar = varName(p.filter.name, vars);
    const kind = p.filter.distinct <= 5 ? "BUTTONS" : "DROPDOWN";
    out.push(
      `SELECT v AS ${fvar} ::${kind}, ${sqlStr(words(p.filter.name))}::LABEL FROM (\n` +
        `  SELECT 'All' AS v, 0 AS o UNION ALL\n` +
        `  SELECT DISTINCT CAST(${f} AS VARCHAR), 1 FROM ${fq} WHERE ${f} IS NOT NULL\n) ORDER BY o, v;`
    );
    conds.push(`coalesce(getvariable('${fvar}'), 'All') IN ('All', CAST(${f} AS VARCHAR))`);
  }
  const where = (...more) => {
    const all = [...conds, ...more];
    return all.length ? `\nWHERE ${all.join("\n  AND ")}` : "";
  };
  const from = `FROM ${fq}${where()}`;
  // KPI tiles
  out.push("", "SELECT 'At a glance'::GROUP;", `SELECT count(*)::COMPACT, 'Rows'::LABEL\n${from};`);
  for (const m of p.measures) {
    const avg = avgLike(m.name);
    out.push(`SELECT ${avg ? "avg" : "sum"}(${qid(m.name)})::COMPACT, ${sqlStr((avg ? "Average " : "Total ") + lower(m.name))}::LABEL\n${from};`);
  }
  out.push("SELECT 1::ENDGROUP;", "");
  const main = p.measures[0] || null;
  const valueOf = main && !avgLike(main.name) ? `sum(${qid(main.name)})` : "count(*)";
  const valueName = main && !avgLike(main.name) ? words(main.name) : "Rows";
  if (p.date) {
    const d = qid(p.date.name);
    const grain = "coalesce(getvariable('period_grain'), 'month')";
    out.push(
      `SELECT CAST(date_trunc(${grain}, ${d}) AS DATE)::XAXIS, ${valueOf}::LINECHART,\n` +
        `       ${sqlStr(valueName + " per ")} || ${grain} ::TITLE\n${from}\nGROUP BY 1 ORDER BY 1;`
    );
  }
  const barCats = p.cats.filter((c) => c !== p.filter);
  for (const c of (barCats.length ? barCats : p.cats).slice(0, 2)) {
    const q = qid(c.name);
    out.push(
      `SELECT coalesce(CAST(${q} AS VARCHAR), '(empty)')::XAXIS, ${valueOf}::BARCHART, ${sqlStr(`${valueName} by ${lower(c.name)}`)}::TITLE\n` +
        `${from}\nGROUP BY 1 ORDER BY 2 DESC, 1 LIMIT 10;`
    );
  }
  if (main) {
    const q = qid(main.name);
    out.push(
      `SELECT ${q}::HISTOGRAM, ${sqlStr("Distribution of " + lower(main.name))}::TITLE\n` +
        `FROM (SELECT ${q} FROM ${fq}${where(`${q} IS NOT NULL`)}) _s USING SAMPLE reservoir(5000 ROWS) REPEATABLE (97);`
    );
  }
  // The paged table: the columns a table can show (no lists / structs).
  const shown = cards.filter((c) => c.kind !== "other").slice(0, 24);
  if (shown.length) {
    const items = shown.map((c, i) => (i === shown.length - 1 ? `${qid(c.name)} ::PAGED` : qid(c.name)));
    out.push("", "SELECT 12::SPAN;", `SELECT ${items.join(", ")}\n${from};`);
  }
  return out.join("\n") + "\n";
}

// ---- joins between tables ----

// Likely join keys of table `t` (`cols`: [{name, kind}]) with the other tables
// (`others`: [{fq, name, cols}]): the same column name and kind, or `<x>_id`
// against an `id` column of a table named like x. Up to `max`, keys first.
export function joinCandidates(t, others, max = 6) {
  const out = [];
  const stem = (s) => String(s).toLowerCase().replace(/(ies)$/, "y").replace(/s$/, "");
  for (const o of others) {
    if (o.fq === t.fq) continue;
    for (const a of t.cols) {
      if (a.kind === "other") continue;
      for (const b of o.cols) {
        if (b.kind === "other") continue;
        const same = a.name.toLowerCase() === b.name.toLowerCase() && a.kind === b.kind;
        const fk =
          (/_id$/i.test(a.name) && b.name.toLowerCase() === "id" && stem(a.name.slice(0, -3)) === stem(o.name)) ||
          (/_id$/i.test(b.name) && a.name.toLowerCase() === "id" && stem(b.name.slice(0, -3)) === stem(t.name));
        if ((same && (idLike(a.name) || a.kind === "text" || a.kind === "num")) || fk)
          out.push({ other: o, a: a.name, b: b.name, key: idLike(a.name) || idLike(b.name) || fk });
      }
    }
  }
  out.sort((x, y) => y.key - x.key);
  return out.slice(0, max);
}

// The share of `a`'s distinct key values found in `b` (rows: n, hit).
export function overlapSql(fqA, colA, fqB, colB) {
  return (
    `SELECT count(*) AS n, count(_b.k) AS hit FROM ` +
    `(SELECT DISTINCT CAST(${qid(colA)} AS VARCHAR) AS k FROM ${fqA} WHERE ${qid(colA)} IS NOT NULL LIMIT 20000) _a ` +
    `LEFT JOIN (SELECT DISTINCT CAST(${qid(colB)} AS VARCHAR) AS k FROM ${fqB}) _b ON _a.k = _b.k`
  );
}

export function joinSql(fqA, colA, fqB, colB) {
  return `SELECT *\nFROM ${fqA} a\nJOIN ${fqB} b ON a.${qid(colA)} = b.${qid(colB)}\nLIMIT 100`;
}

// ---- SQL edits ----

export const stripSemi = (sql) => String(sql).trim().replace(/;\s*$/, "").trim();

// The top-level keywords of a query (outside brackets, strings, comments,
// quoted names): [{kw, at}] in order.
function topLevel(sql) {
  const out = [];
  let depth = 0;
  for (let i = 0; i < sql.length; ) {
    const ch = sql[i];
    if (ch === "'" || ch === '"') {
      const j = sql.indexOf(ch, i + 1);
      i = j < 0 ? sql.length : j + 1;
      continue;
    }
    if (ch === "-" && sql[i + 1] === "-") {
      const j = sql.indexOf("\n", i);
      i = j < 0 ? sql.length : j + 1;
      continue;
    }
    if (ch === "(") depth++;
    else if (ch === ")") depth--;
    else if (depth === 0 && /[A-Za-z]/.test(ch) && !/[A-Za-z0-9_]/.test(sql[i - 1] || "")) {
      const m = /^(where|group\s+by|having|qualify|order\s+by|limit|offset|union|except|intersect|window)\b/i.exec(sql.slice(i));
      if (m) {
        out.push({ kw: m[1].toLowerCase().replace(/\s+/g, " "), at: i });
        i += m[0].length;
        continue;
      }
    }
    i++;
  }
  return out;
}

// Add a filter to a query: ANDed into its WHERE, or a new WHERE before its
// GROUP BY / ORDER BY / LIMIT. A query with a set operation is wrapped.
export function addWhere(sql, cond) {
  const s = stripSemi(sql);
  if (!s) return "";
  const kws = topLevel(s);
  if (kws.some((k) => ["union", "except", "intersect"].includes(k.kw))) return `SELECT * FROM (\n${s}\n) _q\nWHERE ${cond}`;
  const w = kws.find((k) => k.kw === "where");
  const tail = kws.find((k) => (!w || k.at > w.at) && k.kw !== "where");
  if (w) {
    const end = tail ? tail.at : s.length;
    const body = s.slice(w.at + 5, end).trim();
    return `${s.slice(0, w.at)}WHERE (${body}) AND ${cond}${tail ? "\n" + s.slice(tail.at) : ""}`;
  }
  if (tail) return `${s.slice(0, tail.at).trimEnd()}\nWHERE ${cond}\n${s.slice(tail.at)}`;
  return `${s}\nWHERE ${cond}`;
}

// The condition "column = value" for a value shown on a card (values come back
// as text): a number compares as a number, a date as a date, else as text.
export function valueCond(card, v) {
  const q = qid(card.name);
  if (v === "(empty)") return `${q} IS NULL`;
  if (card.kind === "num" && Number.isFinite(Number(v))) return `${q} = ${Number(v)}`;
  if (card.kind === "bool" && /^(true|false)$/i.test(v)) return `${q} = ${v.toLowerCase()}`;
  if (card.kind === "text") return `${q} = ${sqlStr(v)}`;
  return `CAST(${q} AS VARCHAR) = ${sqlStr(v)}`;
}

// Where a DuckDB error points: "LINE 3: …" plus a caret line → {line, col}
// (1-based), else null.
export function errorLocation(msg) {
  const m = /LINE (\d+):( ?)(.*)\n(\s*)\^/.exec(String(msg));
  if (!m) return null;
  const prefix = `LINE ${m[1]}:${m[2]}`.length;
  return { line: Number(m[1]), col: Math.max(1, m[4].length - prefix + 1) };
}

// The text offset of a 1-based line / column.
export function offsetOf(sql, line, col) {
  const lines = String(sql).split("\n");
  let at = 0;
  for (let i = 0; i < Math.min(line - 1, lines.length); i++) at += lines[i].length + 1;
  return Math.min(String(sql).length, at + Math.max(0, col - 1));
}

// The recent-queries list: newest first, no duplicates, at most `max`.
export function pushHistory(list, sql, max = 20) {
  const s = String(sql || "").trim();
  if (!s) return list.slice(0, max);
  return [s, ...list.filter((x) => x !== s)].slice(0, max);
}

// A file name → a view name and the DuckDB reader for it.
export function fileView(fileName) {
  const base = String(fileName).split(/[\\/]/).pop();
  const ext = (/\.([a-z0-9]+)(\.gz)?$/i.exec(base) || [])[1]?.toLowerCase() || "";
  const reader = { csv: "read_csv_auto", tsv: "read_csv_auto", txt: "read_csv_auto", parquet: "read_parquet", json: "read_json_auto", jsonl: "read_json_auto", ndjson: "read_json_auto" }[ext];
  let view = base.replace(/\.[^.]*(\.gz)?$/i, "").toLowerCase().replace(/[^a-z0-9_]+/g, "_").replace(/^_+|_+$/g, "");
  if (!view || /^\d/.test(view)) view = "file_" + view;
  return { view, reader: reader || null, ext };
}

// The view over a registered file or an https URL.
export function fileViewSql(view, reader, path) {
  return `CREATE OR REPLACE VIEW ${qid(view)} AS SELECT * FROM ${reader}(${sqlStr(path)})`;
}

// ---- the profile, end to end ----

// Thrown when a newer request supersedes a profile (the caller ignores it).
export class Superseded extends Error {
  constructor() {
    super("superseded");
    this.name = "Superseded";
  }
}

// Profile table `fq`: `run(sql)` → rows (a Promise), `pageSql(base, pageJson)`
// = the ::PAGED builder (wasm page_sql). `opts.alive()` false → stop (throws
// Superseded) before the next query; `opts.onStage(stage, state)` after each
// step — "columns" (names and types known), "stats", "dist", "extras",
// "series" — so the page fills in progressively. Resolves to
// {total, sampled, rows, cards}.
// sec-review-256 last check: ownOpts first — `opts.alive`/`opts.total`/
// `opts.onStage` were read straight off the parameter, which reads
// inherited properties too (the same issue as a destructured signature).
export async function profileTable(run, pageSql, fq, opts = {}) {
  const o = ownOpts(opts);
  const alive = o.alive || (() => true);
  const step = async (sql) => {
    if (!alive()) throw new Superseded();
    const rows = await run(sql);
    if (!alive()) throw new Superseded();
    return rows;
  };
  const paged = (base, page) => {
    const sql = pageSql(base, JSON.stringify(page));
    if (sql.startsWith("{")) throw new Error(JSON.parse(sql).error);
    return sql;
  };
  const total = o.total ?? Number((await step(countSql(fq)))[0]?.n ?? 0);
  const cols = (await step(describeSql(fq))).map((r) => ({ name: String(r.column_name), type: String(r.column_type), kind: colKind(r.column_type) }));
  const { base, sampled, rows } = profileBase(fq, total);
  const state = { total, sampled, rows, cards: cols.map((c) => ({ name: c.name, type: c.type, kind: c.kind, pending: true })) };
  o.onStage?.("columns", state);
  if (!total) {
    state.cards = buildCards(cols, { stats: { n: 0 } });
    o.onStage?.("done", state);
    return state;
  }
  const parts = Array.from({ length: Math.ceil(cols.length / STAT_BATCH) }, (_, b) => cols.slice(b * STAT_BATCH, (b + 1) * STAT_BATCH));
  const res = parts.map(() => ({ stats: {}, prof: [], extras: {}, series: [] }));
  const merge = () => (state.cards = parts.flatMap((p, b) => buildCards(p, res[b])));
  // 1. counts, distinct values and the ::PAGED statistics of numbers
  for (const [b, p] of parts.entries()) res[b].stats = (await step(paged(base, { stats: p.map((c) => c.name) })))[0] || {};
  merge();
  o.onStage?.("stats", state);
  // 2. histograms and top values (the ::PAGED distributions)
  for (const [b, p] of parts.entries()) {
    const idx = p.map((c, i) => (c.kind === "other" || c.kind === "date" || c.kind === "time" ? -1 : i)).filter((i) => i >= 0);
    if (!idx.length) continue;
    const rows = await step(paged(base, { profile: idx.map((i) => p[i].name) }));
    res[b].prof = rows.map((r) => ({ ...r, c: idx[Number(r.c)] }));
  }
  merge();
  o.onStage?.("dist", state);
  // 3. percentiles, time ranges, exact key counts, mixed types
  for (const [b, p] of parts.entries()) res[b].extras = (await step(extrasSql(base, p)))[0] || {};
  merge();
  o.onStage?.("extras", state);
  // 4. rows per day / week / month of each time column
  for (const [b, p] of parts.entries()) {
    const ts = p
      .map((c, i) => ({ i, name: c.name, grain: (c.kind === "date" || c.kind === "time") && timeGrain(res[b].extras["t0_" + i], res[b].extras["t1_" + i]) }))
      .filter((c) => c.grain);
    if (ts.length) res[b].series = await step(seriesSql(base, ts));
  }
  merge();
  o.onStage?.("done", state);
  return state;
}

// ---- anofox_tabular: a hint's check (#111) ----

// Is anofox_tabular's run_checks there (loaded in this session)?
export const TABULAR_PROBE = "SELECT count(*) AS n FROM duckdb_functions() WHERE function_name = 'run_checks'";

// The anofox_tabular check behind a hint of column `card` of table `fq`:
// {type, table, column, params, lower, upper} (run_checks' contract), or null.
export function hintCheck(fq, card, hint) {
  const q = qid(card.name);
  const c = { table: fq, column: card.name, params: null, lower: null, upper: null };
  switch (hint.code) {
    case "all-null":
    case "mostly-null":
      return { ...c, type: "null_rate", upper: 0.5 };
    case "constant":
      return { ...c, type: "distinct_count", lower: 2 };
    case "duplicate-keys":
      return { ...c, type: "duplicate_count", upper: 0 };
    case "mixed-types": // the share of values that are not numbers
      return { ...c, column: null, type: "compliance", params: { expression: `${q} IS NULL OR TRY_CAST(${q} AS DOUBLE) IS NULL` }, lower: 1 };
    case "outliers": {
      const w = card.p95 - card.p05;
      if (!(w > 0)) return null;
      const lo = card.p05 - 3 * w,
        hi = card.p95 + 3 * w;
      return { ...c, column: null, type: "compliance", params: { expression: `${q} IS NULL OR ${finite(q)} BETWEEN ${lo} AND ${hi}` }, lower: 1 };
    }
  }
  return null;
}

// Run checks with anofox_tabular: its checks table lives in an attached
// in-memory database (run_checks needs a regular table; the data's own
// database may be read-only). One script, so a client that runs each request
// in a fresh process sees it whole; `load` prepends LOAD anofox_tabular.
// The last statement's rows: check_name, check_type, value, thresholds,
// status, message.
export function tabularChecksSql(checks, load = false) {
  const s = (v) => (v == null ? "CAST(NULL AS VARCHAR)" : sqlStr(v));
  const d = (v) => (v == null || !Number.isFinite(Number(v)) ? "CAST(NULL AS DOUBLE)" : `CAST(${Number(v)} AS DOUBLE)`);
  const rows = checks.map(
    (c, i) =>
      `(${sqlStr(`${i + 1} ${c.type}${c.column ? " " + c.column : ""}`)}, ${sqlStr(c.type)}, ${sqlStr(c.table)}, ${s(c.column)}, ${s(c.params && JSON.stringify(c.params))}, ` +
      `${d(c.lower)}, ${d(c.upper)}, false, CAST(NULL AS VARCHAR), CAST(NULL AS VARCHAR))`
  );
  return [
    load && "LOAD anofox_tabular",
    "ATTACH IF NOT EXISTS ':memory:' AS xp_checks (READ_WRITE)",
    "CREATE OR REPLACE TABLE xp_checks.main.checks AS SELECT * FROM (VALUES " +
      rows.join(", ") +
      ") t(check_name, check_type, table_name, column_name, params, lower_threshold, upper_threshold, monitor_only, identifier_column, filter_expr)",
    "SELECT check_name, check_type, value, lower_threshold, upper_threshold, status, message FROM run_checks('xp_checks.main.checks') ORDER BY check_name",
  ]
    .filter(Boolean)
    .join(";\n");
}

// ---- compare two tables, or two time ranges of one (#111) ----

// One side of a time-range comparison: the table's rows in [from, to].
export const rangeSide = (fq, col, from, to) =>
  `(SELECT * FROM ${fq} WHERE CAST(${qid(col)} AS DATE) BETWEEN DATE ${sqlStr(from)} AND DATE ${sqlStr(to)}) _r`;

// Two halves of a date range (ISO text): [[from, to], [from, to]].
export function halves(t0, t1) {
  const a = Date.parse(String(t0).slice(0, 10)),
    b = Date.parse(String(t1).slice(0, 10));
  if (!isFinite(a) || !isFinite(b) || b <= a) return null;
  const day = (ms) => new Date(ms).toISOString().slice(0, 10);
  const mid = a + Math.floor((b - a) / 864e5 / 2) * 864e5;
  return [
    [day(a), day(mid)],
    [day(mid + 864e5), day(b)],
  ];
}

// The column-by-column comparison of two profiles: rows {name, change:
// same | added | removed | retyped, a, b (the cards), shifts: [text]}.
export function compareCards(A, B) {
  const byB = new Map(B.map((c) => [c.name, c]));
  const out = [];
  const rel = (x, y) => (x == null || y == null ? null : x === y ? 0 : (y - x) / (Math.abs(x) || 1));
  for (const a of A) {
    const b = byB.get(a.name);
    if (!b) {
      out.push({ name: a.name, change: "removed", a, b: null, shifts: [] });
      continue;
    }
    byB.delete(a.name);
    const shifts = [];
    if (Math.abs((b.nullPct || 0) - (a.nullPct || 0)) >= 5) shifts.push(`empty ${fmtStat(a.nullPct, 2)}% → ${fmtStat(b.nullPct, 2)}%`);
    const dd = rel(a.distinct, b.distinct);
    if (dd != null && Math.abs(dd) >= 0.2 && Math.abs(b.distinct - a.distinct) >= 3) shifts.push(`distinct ${fmtStat(a.distinct)} → ${fmtStat(b.distinct)}`);
    for (const k of ["mean", "median"]) {
      const r = rel(a[k], b[k]);
      if (r != null && Math.abs(r) >= 0.1) shifts.push(`${k} ${fmtStat(a[k])} → ${fmtStat(b[k])} (${r > 0 ? "+" : ""}${Math.round(100 * r)}%)`);
    }
    out.push({ name: a.name, change: a.type === b.type ? "same" : "retyped", a, b, shifts });
  }
  for (const b of byB.values()) out.push({ name: b.name, change: "added", a: null, b, shifts: [] });
  return out;
}

// A key both sides share: a column unique (and never empty) on both, named
// like a key first. Its name, or null.
export function compareKey(A, B) {
  const byB = new Map(B.map((c) => [c.name, c]));
  const unique = (c) => c && c.kind !== "other" && c.n > 0 && c.nonnull === c.n && c.distinct === c.n;
  const keys = A.filter((a) => unique(a) && unique(byB.get(a.name)));
  keys.sort((x, y) => idLike(y.name) - idLike(x.name));
  return keys.length && (idLike(keys[0].name) || keys[0].kind !== "num" || keys.length === 1) ? keys[0].name : null;
}

// Rows added / removed / changed between two sides by `key`, over the
// common columns (the anofox_tabular diff_joindiff semantics, in plain SQL).
// `common`: column names, or {name, num} (num: compare as numbers, so an
// INTEGER retyped to DOUBLE is no change).
export function keyDiffSql(srcA, srcB, key, common) {
  const k = qid(key);
  const others = common.map((c) => (typeof c === "string" ? { name: c } : c)).filter((c) => c.name !== key);
  const val = (c) => (c.num ? `CAST(${qid(c.name)} AS DOUBLE)` : qid(c.name));
  const h = others.length ? `md5(CAST(row(${others.map(val).join(", ")}) AS VARCHAR))` : "''";
  const side = (src) => `SELECT CAST(${k} AS VARCHAR) AS k, ${h} AS h FROM ${src}`;
  return (
    `SELECT count(*) FILTER (WHERE _a.k IS NULL) AS added, count(*) FILTER (WHERE _b.k IS NULL) AS removed, ` +
    `count(*) FILTER (WHERE _a.k IS NOT NULL AND _b.k IS NOT NULL AND _a.h <> _b.h) AS changed, ` +
    `count(*) FILTER (WHERE _a.h = _b.h) AS same FROM (${side(srcA)}) _a FULL OUTER JOIN (${side(srcB)}) _b ON _a.k = _b.k`
  );
}

// 10-bin histograms of both sides on one shared range per numeric column
// (`cols`: [{i, name, lo, hi}]): rows (c, s = 'a' | 'b', k, n).
export function overlaySql(baseA, baseB, cols) {
  const part = (base, s, c) => {
    const x = finite(qid(c.name));
    const w = c.hi - c.lo;
    return (
      `SELECT ${c.i} AS c, '${s}' AS s, ${w > 0 ? `least(9, floor((_x - ${c.lo}) / ${w} * 10))::INTEGER` : "0"} AS k, count(*) AS n ` +
      `FROM (SELECT ${x} AS _x FROM (${base}) _o) _v WHERE _x IS NOT NULL GROUP BY 3`
    );
  };
  return cols.flatMap((c) => [part(baseA, "a", c), part(baseB, "b", c)]).join(" UNION ALL ") + " ORDER BY 1, 2, 3";
}

// Profile both sides (`a`, `b`: {src, label, total?}) and compare them:
// {a, b (profiles), cols (compareCards), key, diff, overlay: Map col -> {a, b}}.
// Same `alive` / `onStage(stage, state)` contract as profileTable.
// sec-review-256 last check: ownOpts first (same reason as profileTable).
export async function compareTables(run, pageSql, a, b, opts = {}) {
  const o = ownOpts(opts);
  const alive = o.alive || (() => true);
  const step = async (sql) => {
    if (!alive()) throw new Superseded();
    const r = await run(sql);
    if (!alive()) throw new Superseded();
    return r;
  };
  const state = { a: null, b: null, cols: [], key: null, diff: null, overlay: new Map() };
  state.a = await profileTable(run, pageSql, a.src, { alive, total: a.total });
  o.onStage?.("a", state);
  state.b = await profileTable(run, pageSql, b.src, { alive, total: b.total });
  state.cols = compareCards(state.a.cards, state.b.cards);
  o.onStage?.("b", state);
  const nums = state.cols
    .map((r, i) => ({ r, i }))
    .filter(({ r }) => r.a && r.b && r.a.kind === "num" && r.b.kind === "num" && r.a.min != null && r.b.min != null)
    .slice(0, 24)
    .map(({ r, i }) => ({ i, name: r.name, lo: Math.min(r.a.min, r.b.min), hi: Math.max(r.a.max, r.b.max) }));
  if (nums.length) {
    const rows = await step(overlaySql(profileBase(a.src, state.a.total).base, profileBase(b.src, state.b.total).base, nums));
    for (const c of nums) {
      const h = { a: Array(10).fill(0), b: Array(10).fill(0), lo: c.lo, hi: c.hi };
      for (const r of rows) if (Number(r.c) === c.i) h[r.s][Math.min(9, Math.max(0, Number(r.k)))] += Number(r.n) || 0;
      state.overlay.set(c.name, h);
    }
  }
  o.onStage?.("overlay", state);
  state.key = compareKey(state.a.cards, state.b.cards);
  if (state.key) {
    const common = state.cols.filter((r) => r.a && r.b && r.a.kind !== "other").map((r) => ({ name: r.name, num: r.a.kind === "num" && r.b.kind === "num" }));
    state.diff = (await step(keyDiffSql(a.src, b.src, state.key, common)))[0] || null;
  }
  o.onStage?.("done", state);
  return state;
}

// A starter dashboard for a comparison: row counts (and the main measure)
// with B against A as a ::DELTA, key changes, the schema differences, rows by
// the main category per side, the main measure's spread per side and, for
// two tables with a date, both over time. Passes dashboard --check --strict.
// sec-review-256 last check: ownOpts first (same reason as buildCards).
export function compareDashboard(opts) {
  const { a, b, cmp } = ownOpts(opts);
  const A = a.src,
    B = b.src;
  const la = String(a.label),
    lb = String(b.label);
  const both = cmp.cols.filter((r) => r.a && r.b);
  const plan = starterPlan(both.map((r) => r.b));
  const out = [
    `-- Compare ${la} (A) with ${lb} (B), written by Explore data from both profiles.`,
    "-- Every panel is a query on both sides: edit, add, remove.",
    "",
    "SELECT 'B against A'::GROUP;",
    `SELECT (SELECT count(*) FROM ${B})::COMPACT, (SELECT count(*) FROM ${A})::DELTA, ${sqlStr("vs " + la)}::DELTA_LABEL, ${sqlStr("Rows in " + lb)}::LABEL;`,
  ];
  const m = plan.measures[0];
  if (m) {
    const f = avgLike(m.name) ? "avg" : "sum";
    out.push(
      `SELECT (SELECT ${f}(${qid(m.name)}) FROM ${B})::COMPACT, (SELECT ${f}(${qid(m.name)}) FROM ${A})::DELTA, ${sqlStr("vs " + la)}::DELTA_LABEL, ` +
        `${sqlStr((f === "avg" ? "Average " : "Total ") + lower(m.name))}::LABEL;`
    );
  }
  if (cmp.key) {
    const common = both.filter((r) => r.a.kind !== "other").map((r) => ({ name: r.name, num: r.a.kind === "num" && r.b.kind === "num" }));
    const d = `(${keyDiffSql(A, B, cmp.key, common)}) _d`;
    for (const [col, word] of [["added", "added"], ["removed", "removed"], ["changed", "changed"]])
      out.push(`SELECT ${col}::COMPACT, ${sqlStr(`Rows ${word} (by ${cmp.key})`)}::LABEL FROM ${d};`);
  }
  out.push("SELECT 1::ENDGROUP;", "");
  if (cmp.cols.some((r) => r.change !== "same"))
    out.push(
      `SELECT coalesce(_a.column_name, _b.column_name) AS "Column", _a.column_type AS "In A", _b.column_type AS "In B",\n` +
        `       CASE WHEN _a.column_name IS NULL THEN 'added' WHEN _b.column_name IS NULL THEN 'removed' ELSE 'retyped' END AS "Change" ::TABLE,\n` +
        `       'Schema differences'::TITLE\n` +
        `FROM (DESCRIBE SELECT * FROM ${A}) _a FULL OUTER JOIN (DESCRIBE SELECT * FROM ${B}) _b ON _a.column_name = _b.column_name\n` +
        `WHERE _a.column_type IS DISTINCT FROM _b.column_type ORDER BY 1;`
    );
  const sides = (cols) => `SELECT ${sqlStr(la)} AS side, ${cols} FROM ${A} UNION ALL SELECT ${sqlStr(lb)}, ${cols} FROM ${B}`;
  const cat = plan.cats[0];
  if (cat) {
    const q = qid(cat.name);
    out.push(
      `SELECT coalesce(CAST(${q} AS VARCHAR), '(empty)')::XAXIS, side::CATEGORY, count(*)::BARCHART, ${sqlStr(`Rows by ${lower(cat.name)}`)}::TITLE\n` +
        `FROM (${sides(q)}) _s\nWHERE coalesce(CAST(${q} AS VARCHAR), '(empty)') IN (SELECT coalesce(CAST(${q} AS VARCHAR), '(empty)') FROM (${sides(q)}) _t GROUP BY 1 ORDER BY count(*) DESC, 1 LIMIT 10)\n` +
        `GROUP BY 1, 2 ORDER BY 1, 2;`
    );
  }
  if (m) {
    const q = qid(m.name);
    const smp = (src) => `(SELECT ${q} FROM ${src} WHERE ${q} IS NOT NULL) _x USING SAMPLE reservoir(2500 ROWS) REPEATABLE (97)`;
    out.push(
      `SELECT side::XAXIS, ${q}::BOXPLOT, ${sqlStr("Spread of " + lower(m.name))}::TITLE\n` +
        `FROM (SELECT ${sqlStr(la)} AS side, ${q} FROM ${smp(A)} UNION ALL SELECT ${sqlStr(lb)}, ${q} FROM ${smp(B)}) _s;`
    );
  }
  if (plan.date && !a.range) {
    const d = qid(plan.date.name);
    out.push(
      `SELECT CAST(date_trunc('month', ${d}) AS DATE)::XAXIS, side::CATEGORY, count(*)::LINECHART, 'Rows per month'::TITLE\n` +
        `FROM (${sides(d)}) _s WHERE ${d} IS NOT NULL GROUP BY 1, 2 ORDER BY 1, 2;`
    );
  }
  return out.join("\n") + "\n";
}
