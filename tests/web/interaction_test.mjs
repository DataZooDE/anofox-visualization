// Node test for the chart interaction helpers in web/app.js (issues #28, #29):
// the ::SELECT_AS value parser and the x-shape detection the brush relies on.
import { readFileSync } from "fs";
import assert from "assert/strict";

const src = readFileSync(new URL("../../web/app.js", import.meta.url), "utf8");
const grab = (name) => {
  const m = src.match(new RegExp(`^function ${name}\\([\\s\\S]*?^}`, "m"));
  assert.ok(m, `function ${name} not found in app.js`);
  return m[0];
};
const { parseSelectAs, xKindOf, parseDrill, fillLink } = new Function(
  `${grab("parseSelectAs")}\n${grab("xKindOf")}\n${grab("parseDrill")}\n${grab("fillLink")}\n` +
    `return { parseSelectAs, xKindOf, parseDrill, fillLink };`
)();

assert.deepEqual(parseSelectAs("region"), { name: "region", mode: "single" });
assert.deepEqual(parseSelectAs(" region multi "), { name: "region", mode: "multi" });
assert.deepEqual(parseSelectAs("region:MULTI"), { name: "region", mode: "multi" });
for (const bad of ["", "1x", "a b", "region many", "x;DROP", null]) assert.equal(parseSelectAs(bad), null, String(bad));

assert.equal(xKindOf(["1", "2.5", "-3"]), "num");
assert.equal(xKindOf(["2024-01-01", "2024-02-01 00:00:00"]), "date");
assert.equal(xKindOf(["EU", "US"]), null);
assert.equal(xKindOf([]), null);
assert.deepEqual(parseDrill("region > country > city"), { prefix: "drill", levels: ["region", "country", "city"] });
assert.deepEqual(parseDrill("geo: region>country"), { prefix: "geo", levels: ["region", "country"] });
for (const bad of ["region", "a > ", "a > b c", "x y: a > b", "a > b; DROP", ""]) assert.equal(parseDrill(bad), null, bad);

// Placeholders are URL-encoded; unknown ones are blank.
assert.equal(fillLink("#City detail?v.c={value}", { value: "New York" }), "#City detail?v.c=New%20York");
assert.equal(fillLink("/d/x?v.a={a}&v.b={b}", { a: "x&y=1" }), "/d/x?v.a=x%26y%3D1&v.b=");
assert.equal(fillLink("https://w.org/wiki/{value}", { value: "a/../b" }), "https://w.org/wiki/a%2F..%2Fb");
// ::MAP features (#map interactivity): the hover reads the renderer's
// data-series / data-value / data-label, not the "label: series" title.
const { mapMarkInfo, sharedZoom, zoomFromShared, chartLink } = new Function(
  "dpIa",
  "dpDrill",
  `const fmtTipValue = (v) => String(Math.round(Number(v) * 1000) / 1000);\n` +
    `${grab("fillLink")}\n${grab("mapMarkInfo")}\n${grab("sharedZoom")}\n${grab("zoomFromShared")}\n${grab("chartLink")}\n` +
    `return { mapMarkInfo, sharedZoom, zoomFromShared, chartLink };`
)({}, {});
const attrs = (o) => (k) => (k in o ? o[k] : null);
const stop = mapMarkInfo(attrs({ "data-row": "7", "data-series": "Van 2", "data-label": "Bakery\nVan 2, stop #3" }), "Bakery\nVan 2, stop #3: Van 2");
assert.deepEqual(stop, { series: "Van 2", value: "", x: "Bakery\nVan 2, stop #3", fx: true, own: false, detail: "" });
// a choropleth: the region is both label and series; the value is the measure
const region = mapMarkInfo(attrs({ "data-series": "Peru", "data-label": "Peru", "data-value": "12.34567" }), "Peru: 12.346");
assert.equal(region.own, true);
assert.equal(region.value, "12.346");
assert.equal(mapMarkInfo(attrs({ "data-series": "Peru", "data-value": "3", "data-fvalue": "3 quakes" }), "").value, "3 quakes");
// no metadata (an SVG from an older renderer): the title is the series
assert.equal(mapMarkInfo(attrs({}), "Van 1: Van 1").series, "Van 1: Van 1");

// ::SYNC_ZOOM: a shared view is a centre and a width; each map keeps its aspect.
assert.equal(sharedZoom(null), null);
const sv = sharedZoom({ x0: -10, x1: 10, y0: -5, y1: 5 });
assert.deepEqual(sv, { cx: 0, cy: 0, w: 20 });
assert.deepEqual(zoomFromShared(sv, 2), { x0: -10, x1: 10, y0: -5, y1: 5 });
assert.deepEqual(zoomFromShared({ cx: 1, cy: 2, w: 8 }, 0.5), { x0: -3, x1: 5, y0: -6, y1: 10 });
for (const bad of [null, { cx: 0, cy: 0, w: 0 }, { cx: 0, cy: 0, w: NaN }]) assert.equal(zoomFromShared(bad, 1), null);
assert.equal(zoomFromShared(sv, 0), null);

// A map feature follows the ::LINK of its own row (data-row), not the first
// row of its series.
const el = (a) => ({ getAttribute: (k) => (k in a ? a[k] : null) });
const link = {
  idx: 2,
  roles: [[0, "MAP"], [1, "CATEGORY"], [2, "LINK"]],
  rows: JSON.stringify([
    { c0: "POINT(0 0)", c1: "late stop", c2: "#Stop list?v.van=Van 1" },
    { c0: "POINT(1 1)", c1: "late stop", c2: "#Stop list?v.van=Van 3" },
  ]),
};
assert.equal(chartLink(link, el({ "data-row": "1", "data-series": "late stop" }), "late stop", "late stop"), "#Stop list?v.van=Van 3");
assert.equal(chartLink(link, el({ "data-series": "late stop" }), "late stop", "late stop"), "#Stop list?v.van=Van 1");
assert.equal(chartLink(link, el({ "data-row": "9", "data-series": "late stop" }), "late stop", "late stop"), "#Stop list?v.van=Van 1");
// #437: a ::HEATMAP's {<column>} placeholder resolves to that column's own
// value (by its role's name), not just {value}/{x}/{series} — and a
// ::YAXIS lane (no ::CATEGORY) still picks the clicked row, not the first
// one at that x.
const heatLink = {
  idx: 3,
  roles: [[0, "XAXIS"], [1, "YAXIS", "f"], [2, "HEATMAP", "v"], [3, "LINK"]],
  rows: JSON.stringify([
    { c0: "2026-01-01", c1: "alpha", c2: "0", c3: "?v.picked={f}" },
    { c0: "2026-01-01", c1: "beta", c2: "1", c3: "?v.picked={f}" },
  ]),
};
assert.equal(
  chartLink(heatLink, el({ "data-x": "2026-01-01", "data-series": "beta" }), "2026-01-01", "beta"),
  "?v.picked=beta"
);
assert.equal(
  chartLink(heatLink, el({ "data-x": "2026-01-01", "data-series": "alpha" }), "2026-01-01", "alpha"),
  "?v.picked=alpha"
);
// #437 review: `vars` is a null-prototype object — {constructor} (or any
// other Object.prototype name) must fill empty, not resolve to an inherited
// property, now that its keys come straight from data column names.
const protoLink = { ...heatLink, idx: 4, rows: JSON.stringify([{ c0: "2026-01-01", c1: "alpha", c2: "0", c4: "?v.x={constructor}" }]) };
assert.equal(chartLink(protoLink, el({ "data-x": "2026-01-01", "data-series": "alpha" }), "2026-01-01", "alpha"), "?v.x=");

// The legend toggle finds the legend above the panel rect, however many
// rows it wrapped onto (#175).
const { legendBand } = new Function(`${grab("legendBand")}\nreturn { legendBand };`)();
assert.equal(legendBand("62.05 65.50 572.45 294.45", 400), 65.5);
assert.equal(legendBand("62.05 119.50 572.45 240.45", 400), 119.5, "a five-row legend");
assert.equal(legendBand(null, 400), 64);
assert.equal(legendBand("0.00 0.00 640.00 400.00", 400), 64, "a panel at the top: no band");

// #213: hovering a mark focuses its series only where a series is a group of marks.
const { seriesFocusable } = new Function(`const HOVER_FOCUS_MAX = 2000;\n${grab("seriesFocusable")}\nreturn { seriesFocusable };`)();
assert.equal(seriesFocusable(["2024", "2024", "2025", "2025"]), true, "grouped / stacked bars");
assert.equal(seriesFocusable(["A", "A", "A"]), false, "one series");
assert.equal(seriesFocusable(["Jan", "Feb", "Mar"]), false, "a single-series bar: each bar its own series");
assert.equal(seriesFocusable([]), false);
const two = (n) => Array.from({ length: n }, (_, i) => (i % 2 ? "a" : "b"));
assert.equal(seriesFocusable(two(2000)), true, "at the cap");
assert.equal(seriesFocusable(two(2001)), false, "a dense chart keeps the plain hover");

// A decorative mark (e.g. a boxplot's mean dot: its own GeomPoint layer,
// marked with ggplot-rs 0.22.2's `GGPlot::decorative()` — #161 — so it
// carries `data-part="decorative"` but no title/data-* of its own) is
// excluded from select/drill/cross-filter and crosshair/hover-list
// groupings the same way a reference rule (geom_vline/geom_hline) already
// is. It has no tooltip of its own either way: the box's own tooltip
// already names the spot.
{
  const { isRuleMark } = new Function(`${grab("isRuleMark")}\nreturn { isRuleMark };`)();
  const elWith = (a) => ({
    getAttribute: (k) => (k in a ? a[k] : null),
    hasAttribute: (k) => k in a,
  });
  assert.equal(isRuleMark(elWith({ "data-geom": "vline" })), true);
  assert.equal(isRuleMark(elWith({ "data-geom": "hline" })), true);
  assert.equal(
    isRuleMark(elWith({ "data-geom": "point", "data-part": "decorative" })),
    true,
    "a decorative mean dot"
  );
  assert.equal(isRuleMark(elWith({ "data-geom": "point" })), false, "an ordinary point");
}

// #330: ::ROWSELECT must bind on a key column whose rendered text happens
// to look numeric (an id like "791", even cast to VARCHAR) — only the
// cell's right-alignment cares about that, never whether a click sets
// getvariable('<first column>'). Regression: a `.numeric` gate on this once
// silently dropped the click handler (and so the binding) with no warning.
{
  const { rowSelectable } = new Function(`${grab("rowSelectable")}\nreturn { rowSelectable };`)();
  assert.equal(rowSelectable("single", "picked"), true, "a plain text key column");
  assert.equal(rowSelectable("single", "picked"), true, "a numeric-looking key column is still selectable");
  assert.equal(rowSelectable("multi", "picked"), true);
  assert.equal(rowSelectable("none", "picked"), false, "a table that never opted in");
  assert.equal(rowSelectable("single", null), false, "no key column at all");
  assert.equal(rowSelectable("single", undefined), false);
}

console.log("interaction_test: ok");
