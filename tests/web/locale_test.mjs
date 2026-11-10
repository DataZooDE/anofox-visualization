// The browser's number/date formatting in a dashboard locale (::LOCALE) must
// read exactly like the CLI / headless renderer: both use the shared vectors
// in tests/fixtures/locale_vectors.json (the Rust side: tests/theme.rs).
// Loads the wasm build (web/pkg, from `wasm-pack build --target web …`) and
// drives app.js's own fmtNum / fmtCell with it. Skips without web/pkg.
import { readFileSync, existsSync } from "fs";
import assert from "assert/strict";

const pkg = new URL("../../web/pkg/", import.meta.url);
if (!existsSync(new URL("anofox_visualization_bg.wasm", pkg))) {
  console.log("web/pkg not built — skipping locale_test");
  process.exit(0);
}
const wasm = await import(new URL("anofox_visualization.js", pkg));
wasm.initSync({ module: readFileSync(new URL("anofox_visualization_bg.wasm", pkg)) });

const src = readFileSync(new URL("../../web/app.js", import.meta.url), "utf8");
const grab = (name) => {
  const m = src.match(new RegExp(`^function ${name}\\([\\s\\S]*?^}`, "m"));
  assert.ok(m, `function ${name} not found in app.js`);
  return m[0];
};
let LOCALE = "";
const { fmtNum, fmtCell, panelLocale } = new Function(
  "format_number",
  "format_number_locale",
  "format_date_locale",
  "dashLocale",
  `const NUM_LOCALE = "en-US";\n${grab("fmtNum")}\n${grab("fmtCell")}\n${grab("panelLocale")}\nreturn { fmtNum, fmtCell, panelLocale };`
)(wasm.format_number, wasm.format_number_locale, wasm.format_date_locale, () => LOCALE);

const fx = JSON.parse(readFileSync(new URL("../fixtures/locale_vectors.json", import.meta.url), "utf8"));
for (const [n, fmt, tag, want] of fx.numbers) {
  LOCALE = tag === "en-US" ? "" : tag; // en-US = no ::LOCALE (the default path)
  assert.equal(fmtNum(n, fmt), want, `fmtNum(${n}, ${fmt}) in ${tag}`);
}
for (const [iso, tag, want] of fx.dates) {
  if (tag === "en-US") continue; // no ::LOCALE: cells are shown verbatim
  LOCALE = tag;
  assert.equal(fmtCell(iso), want, `fmtCell(${iso}) in ${tag}`);
}
LOCALE = "";
assert.equal(fmtCell("2026-10-15"), "2026-10-15", "no locale: verbatim");
assert.equal(fmtCell(1234.5), 1234.5, "no locale: verbatim number");
LOCALE = "de-DE";
assert.equal(fmtCell("1234.5"), "1.234,5");
// #1064: a panel's 'EUR'::CURRENCY_CODE rides on its locale tag (-u-cu-eur)
// to the wasm formatters — the same text as the headless tile
LOCALE = "";
const kpiRoles = [[0, "MONEY"], [1, "LABEL"], [2, "CURRENCY_CODE"]];
assert.equal(panelLocale(kpiRoles, { c0: 889560, c1: "Revenue", c2: "EUR" }), "en-US-u-cu-eur");
assert.equal(fmtNum(889560, "MONEY", panelLocale(kpiRoles, { c0: 1, c2: "EUR" })), "€889,560");
assert.equal(panelLocale([[0, "MONEY"]], { c0: 1 }), "", "no code: the dashboard's locale");
assert.equal(panelLocale(kpiRoles, { c0: 1, c2: "Euro" }), "", "not a code");
LOCALE = "de-DE";
assert.equal(fmtNum(1234, "MONEY", panelLocale(kpiRoles, { c2: "usd" })), "1.234 $");
assert.equal(fmtNum(1234, "MONEY"), "1.234 €", "a panel without a code keeps the locale's");
// a table row is keyed by column name
assert.equal(panelLocale([[0, "TABLE"], [2, "CURRENCY_CODE"]], { r: "a", v: 1, cur: "GBP" }), "de-DE-u-cu-gbp");
LOCALE = "";
// the theme resolver export
const t = JSON.parse(wasm.resolve_dashboard_theme("dark", JSON.stringify({ title: "T", locale: "de-DE" })));
assert.equal(t.ok, true);
assert.equal(t.mode, "dark");
assert.equal(t.dark.brand.title, "T");
assert.equal(t.dark.locale, "de-DE");
assert.ok(t.dark.css.startsWith(":root{--ax-bg:"));
assert.equal(JSON.parse(wasm.resolve_dashboard_theme("nope", "{}")).ok, false);
console.log("locale_test ok");
