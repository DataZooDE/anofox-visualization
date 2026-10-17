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

// Reference rules (`data-rule`: ::XLINE "Today", ::REFLINE) keep their tooltip
// but are no data point: neither a series in the summary nor a keyboard stop.
const chartMarks = new Function(`${grab("chartMarks")}\nreturn chartMarks;`)();
const el = (attrs) => ({ getAttribute: (k) => attrs[k] ?? null, hasAttribute: (k) => k in attrs });
const point = el({ "data-tip": "Observed: 12", "data-series": "Observed" });
const today = el({ "data-tip": "2026-10-12", "data-x": "2026-10-12", "data-rule": "x" });
assert.deepEqual(chartMarks({ querySelectorAll: () => [point, today] }), [point]);
console.log("a11y_test: ok");
