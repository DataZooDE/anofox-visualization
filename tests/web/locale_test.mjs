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
const { fmtNum, fmtCell } = new Function(
  "format_number",
  "format_number_locale",
  "format_date_locale",
  "dashLocale",
  `const NUM_LOCALE = "en-US";\n${grab("fmtNum")}\n${grab("fmtCell")}\nreturn { fmtNum, fmtCell };`
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
// the theme resolver export
const t = JSON.parse(wasm.resolve_dashboard_theme("dark", JSON.stringify({ title: "T", locale: "de-DE" })));
assert.equal(t.ok, true);
assert.equal(t.mode, "dark");
assert.equal(t.dark.brand.title, "T");
assert.equal(t.dark.locale, "de-DE");
assert.ok(t.dark.css.startsWith(":root{--ax-bg:"));
assert.equal(JSON.parse(wasm.resolve_dashboard_theme("nope", "{}")).ok, false);
console.log("locale_test ok");
