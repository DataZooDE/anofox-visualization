// Node test for the browser's readers of ggplot-rs SVG metadata (run by
// tests/web_js.rs). Extracts the helpers from web/app.js (a browser module
// that can't be imported headless) and checks them against stub elements.
import { readFileSync } from "fs";
import assert from "assert/strict";

const src = readFileSync(new URL("../../web/app.js", import.meta.url), "utf8");
const grab = (name) => {
  const m = src.match(new RegExp(`^function ${name}\\([\\s\\S]*?^}`, "m"));
  assert.ok(m, `function ${name} not found in app.js`);
  return m[0];
};
const el = (attrs) => ({ getAttribute: (k) => (k in attrs ? attrs[k] : null) });

// svgDomain: the drawn x/y domain from the root <svg data-domain>.
const { svgDomain } = new Function(`${grab("svgDomain")}\nreturn { svgDomain };`)();
assert.deepEqual(svgDomain(el({ "data-domain": "0 10 -1.5 2.5" })), { x0: 0, x1: 10, y0: -1.5, y1: 2.5 });
assert.deepEqual(svgDomain(el({ "data-domain": "1709125920 1711882080 98.6 105.6" })).x1, 1711882080);
for (const bad of [null, "", "0 10 1", "0 0 1 2", "a b c d", "0 10 NaN 1"])
  assert.equal(svgDomain(el(bad === null ? {} : { "data-domain": bad })), null, String(bad));
assert.equal(svgDomain(null), null);

// markInfo: series/value/x from ggplot's data-* attributes, title fallback.
const { markInfo, fmtTipValue } = new Function(
  `${grab("fmtTipValue")}\n${grab("markInfo")}\nreturn { markInfo, fmtTipValue };`
)();
const info = (attrs, tip) => markInfo(el(attrs).getAttribute, tip);
assert.equal(fmtTipValue("0.30000000000000004"), "0.3");
assert.equal(fmtTipValue("12"), "12");
assert.equal(fmtTipValue("up"), "up");
// Attributes win over the title text (series names may contain ": ").
assert.deepEqual(info({ "data-series": "a: b", "data-value": "22", "data-x": "W1" }, "a: b: 22"), {
  series: "a: b",
  value: "22",
  x: "W1",
  fx: false,
  own: false,
  detail: "",
});
// A stacked segment: the raw segment value, rounded.
assert.equal(info({ "data-series": "web", "data-value": "21.99999" }, "web: 22").value, "22");
// No attributes (a map feature) → the "series: value" title.
assert.deepEqual(info({}, "Germany: 42"), { series: "Germany", value: "42", x: "", fx: false, own: false, detail: "" });
// Line/polygon titles are just the series.
assert.equal(info({ "data-series": "Model A" }, "Model A").detail, "");
// A richer tooltip is kept as detail (box-plot stats, OHLC).
const box = info({ "data-series": "web", "data-value": "5", "data-x": "web" }, "web: median 5 (2–7)");
assert.equal(box.value, "5");
assert.equal(box.detail, "web: median 5 (2–7)");
const ohlc = info({ "data-series": "up", "data-value": "101.487" }, "2024-03-02 — O 100.7 H 102.583 L 99.604 C 101.487");
assert.equal(ohlc.series, "up");
assert.ok(ohlc.detail.includes("O 100.7"));
// Heatmap tile "x, y: v" with data-series = y.
assert.deepEqual(info({ "data-series": "pm", "data-value": "7", "data-x": "Mon" }, "Mon, pm: 7").detail, "");
// The renderer's formatted value / x win for display (::YFORMAT, ::LOCALE);
// the raw value still decides whether the title says more.
const money = info(
  { "data-series": "api", "data-value": "1234.5", "data-fvalue": "$1,234.50", "data-x": "W1" },
  "api: 1234.5"
);
assert.equal(money.value, "$1,234.50");
assert.equal(money.detail, "");
const de = info(
  { "data-series": "web", "data-value": "12.5", "data-fvalue": "12,50 €", "data-x": "2024-03-01", "data-fx": "01.03.2024" },
  "web: 12.5"
);
assert.deepEqual([de.value, de.x, de.fx, de.detail], ["12,50 €", "01.03.2024", true, ""]);
// A ::BARCHART_STACKED_PERCENT segment (#218): the renderer writes its share,
// so the tooltip reads the share with no extra detail line.
const share = info({ "data-series": "a", "data-value": "0.375", "data-fvalue": "37.5%", "data-x": "D1" }, "a: 0.375");
assert.deepEqual([share.value, share.detail], ["37.5%", ""]);
// A single-series bar (series = x): the tooltip row drops the repeated name.
const one = info({ "data-series": "2024-03-01", "data-value": "3", "data-x": "2024-03-01", "data-fx": "01.03.2024" }, "2024-03-01: 3");
assert.equal(one.own, true);
// A heatmap tile whose title carries the formatted value.
assert.equal(
  info({ "data-series": "pm", "data-value": "7.5", "data-fvalue": "$7.50", "data-x": "Mon" }, "Mon, pm: $7.50").detail,
  ""
);
// ruleRow: the crosshair's "Reference line" row. A date ::XLINE (geom_vline)
// shows its formatted date, not the raw epoch seconds in data-value (#365);
// a ::REFLINE (geom_hline) still shows its value.
const { ruleRow } = new Function(
  `${grab("escapeHtml")}\n${grab("fmtTipValue")}\n${grab("markInfo")}\n${grab("ruleRow")}\nreturn { ruleRow };`
)();
const vline = ruleRow(el({ "data-geom": "vline", "data-value": "1781481600", "data-fx": "2026-06-15", "data-tip": "2026-06-15" }));
assert.ok(vline.includes("<b>2026-06-15</b>"), vline);
const hline = ruleRow(el({ "data-geom": "hline", "data-value": "100", "data-fvalue": "$100.00", "data-tip": "100" }));
assert.ok(hline.includes("<b>$100.00</b>"), hline);
console.log("web svg attribute tests: ok");
