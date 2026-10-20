// Node test for the accessible-chart helpers in web/app.js (issue #37): the
// one-sentence screen-reader summary and the chart kind behind it.
import { readFileSync } from "fs";
import assert from "assert/strict";

const src = readFileSync(new URL("../../web/app.js", import.meta.url), "utf8");
const grab = (name) => {
  const m = src.match(new RegExp(`^function ${name}\\([\\s\\S]*?^}`, "m"));
  assert.ok(m, `function ${name} not found in app.js`);
  return m[0];
};
const kinds = src.match(/^const CHART_KINDS = \[[\s\S]*?^\];/m);
assert.ok(kinds, "CHART_KINDS not found");
const { chartSummary, chartKind } = new Function(
  `${kinds[0]}\n${grab("chartKind")}\n${grab("chartSummary")}\nreturn { chartSummary, chartKind };`
)();

assert.equal(chartKind([[0, "XAXIS", ""], [1, "BARCHART_STACKED", ""]]), "Bar chart");
assert.equal(chartKind([[0, "XAXIS", ""], [1, "LINECHART", "sales"]]), "Line chart");
assert.equal(chartKind([]), "Chart");
// A box plot from statistics carries no ::BOXPLOT: its quartile roles name it.
assert.equal(chartKind([[0, "XAXIS", ""], [1, "Q1", ""], [2, "MEDIAN", ""], [3, "Q3", ""]]), "Box plot");
assert.equal(chartKind([[0, "XAXIS", ""], [1, "BOXPLOT", "err"]]), "Box plot");

const fmt = (v) => String(v);
// Multi-series: kind, title, series, x range, extremes with where they are.
const items = [
  { series: "app", x: "W1", value: 30 },
  { series: "app", x: "W2", value: 48 },
  { series: "web", x: "W1", value: 22 },
  { series: "web", x: "W2", value: 9 },
];
assert.equal(
  chartSummary("Bar chart", "Sessions by week", items, fmt),
  "Bar chart: Sessions by week; 2 series (app, web); 2 points from W1 to W2; highest 48 (app, W2), lowest 9 (web, W2)."
);
// Single series whose "series" is its x (a plain bar): no series clause.
assert.equal(
  chartSummary("Bar chart", "", [
    { series: "EU", x: "EU", value: 5 },
    { series: "US", x: "US", value: 7 },
  ], fmt),
  "Bar chart; 2 points from EU to US; highest 7 (US), lowest 5 (EU)."
);
// Many series are capped; no marks → kind + title only.
const many = "abcdef".split("").map((s, i) => ({ series: s, x: "1", value: i }));
assert.match(chartSummary("Line chart", "T", many, fmt), /6 series \(a, b, c, d, and 2 more\)/);
assert.equal(chartSummary("Gauge", "Uptime", [], fmt), "Gauge: Uptime.");

// Reference rules (geom_vline/geom_hline: ::XLINE "Today", ::REFLINE) keep
// their tooltip but are no data point: neither a series in the summary nor a
// keyboard stop.
const chartMarks = new Function(`${grab("isRuleMark")}\n${grab("chartMarks")}\nreturn chartMarks;`)();
const el = (attrs) => ({ getAttribute: (k) => attrs[k] ?? null, hasAttribute: (k) => k in attrs });
const point = el({ "data-tip": "Observed: 12", "data-series": "Observed" });
const today = el({ "data-tip": "2026-10-12", "data-x": "2026-10-12", "data-geom": "vline" });
assert.deepEqual(chartMarks({ querySelectorAll: () => [point, today] }), [point]);

// "Charts as tables" for a ::MAP: one row per feature (a multipoint's dots
// once), named by its label, sorted by value, with its class — no WKT.
{
  const escapeHtml = (s) => String(s).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);
  const fmtTipValue = (v) => String(Math.round(Number(v) * 1000) / 1000);
  const mapTableHtml = new Function("escapeHtml", "fmtTipValue", `${grab("mapTableHtml")}\nreturn mapTableHtml;`)(
    escapeHtml,
    fmtTipValue
  );
  const f = (a) => el(a);
  const marks = [
    f({ "data-row": "0", "data-series": "Nord", "data-label": "Nord", "data-value": "12", "data-class": "1" }),
    f({ "data-row": "1", "data-series": "Ost", "data-label": "Ost\nmore", "data-value": "980", "data-fvalue": "$980", "data-class": "2" }),
    f({ "data-row": "1", "data-series": "Ost", "data-label": "Ost\nmore", "data-value": "980", "data-class": "2" }),
    f({ "data-row": "2", "data-series": "Süd", "data-label": "Süd" }),
  ];
  const svg = { getAttribute: (k) => (k === "data-classes" ? '["0 – 100","100 – 1K"]' : null), querySelectorAll: () => marks };
  const html = mapTableHtml(svg, 200, "Sales & more");
  assert.ok(html.startsWith('<table class="dp-dv" aria-label="Sales &amp; more"><thead><tr><th scope="col">name</th><th scope="col">value</th><th scope="col">class</th></tr>'), html);
  const rows = [...html.matchAll(/<tr><td>(.*?)<\/td>/g)].map((m) => m[1]);
  assert.deepEqual(rows, ["Ost", "Nord", "Süd"]);
  assert.ok(html.includes("<td>$980</td><td>2 of 2: 100 – 1K</td>"), html);
  assert.ok(!/POLYGON|POINT/.test(html));
}

// M2a: graduated symbols add a "size" column (data-fsize), sorted by
// data-size when the colour measure is the size itself (no colour measure).
{
  const escapeHtml = (s) => String(s).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);
  const fmtTipValue = (v) => String(Math.round(Number(v) * 1000) / 1000);
  const mapTableHtml = new Function("escapeHtml", "fmtTipValue", `${grab("mapTableHtml")}\nreturn mapTableHtml;`)(
    escapeHtml,
    fmtTipValue
  );
  const f = (a) => el(a);
  const marks = [
    f({ "data-row": "0", "data-label": "Dortmund", "data-value": "104", "data-size": "104", "data-fsize": "104" }),
    f({ "data-row": "1", "data-label": "Hamburg", "data-value": "76", "data-size": "76", "data-fsize": "76" }),
  ];
  const svg = { getAttribute: () => null, querySelectorAll: () => marks };
  const html = mapTableHtml(svg, 200, "Customers");
  assert.ok(html.includes('<th scope="col">size</th>'), html);
  const rows = [...html.matchAll(/<tr><td>(.*?)<\/td>/g)].map((m) => m[1]);
  assert.deepEqual(rows, ["Dortmund", "Hamburg"]);
}

// M2b: a flow map's as-table has from/to columns (data-orig/data-dest),
// sorted by data-size descending.
{
  const escapeHtml = (s) => String(s).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);
  const fmtTipValue = (v) => String(Math.round(Number(v) * 1000) / 1000);
  const mapTableHtml = new Function("escapeHtml", "fmtTipValue", `${grab("mapTableHtml")}\nreturn mapTableHtml;`)(
    escapeHtml,
    fmtTipValue
  );
  const f = (a) => el(a);
  const marks = [
    f({ "data-row": "0", "data-orig": "Dortmund DC", "data-dest": "DE-NW", "data-value": "100", "data-size": "100", "data-fsize": "100" }),
    f({ "data-row": "1", "data-orig": "Hamburg DC", "data-dest": "DE-SH", "data-value": "40", "data-size": "40", "data-fsize": "40" }),
  ];
  const svg = { getAttribute: () => null, querySelectorAll: () => marks };
  const html = mapTableHtml(svg, 200, "Flows");
  assert.ok(html.includes('<th scope="col">from</th>'), html);
  assert.ok(html.includes('<th scope="col">to</th>'), html);
  const rows = [...html.matchAll(/<tr><td>(.*?)<\/td>/g)].map((m) => m[1]);
  assert.deepEqual(rows, ["Dortmund DC", "Hamburg DC"]);
}
console.log("a11y_test: ok");
