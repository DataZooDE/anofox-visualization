// The browser table reads a column exactly like the Rust table module (the
// headless SVG table): the shared vectors in tests/fixtures/table_vectors.json
// (the Rust side: tests/table.rs). Loads the wasm build (web/pkg) for
// table_profile / format_number_locale and drives app.js's own tableNumText.
// Skips without web/pkg.
import { readFileSync, existsSync } from "fs";
import assert from "assert/strict";

const pkg = new URL("../../web/pkg/", import.meta.url);
if (!existsSync(new URL("anofox_visualization_bg.wasm", pkg))) {
  console.log("web/pkg not built — skipping table_test");
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
const { tableNumText, humanHeader } = new Function(
  "format_number",
  "format_number_locale",
  "human_header",
  "dashLocale",
  `const NUM_LOCALE = "en-US"; const METRICS = ["METRIC", "MONEY", "PERCENT", "COMPACT"];\n` +
    `${grab("fmtNum")}\n${grab("tableNumText")}\n${grab("humanHeader")}\nreturn { tableNumText, humanHeader };`
)(wasm.format_number, wasm.format_number_locale, wasm.human_header, () => LOCALE);

const norm = (s) => String(s).replace(/[  ]/g, " ");
const vectors = JSON.parse(readFileSync(new URL("../fixtures/table_vectors.json", import.meta.url), "utf8"));
let n = 0;
for (const c of vectors.cases) {
  LOCALE = c.locale;
  const profiles = JSON.parse(
    wasm.table_profile(
      JSON.stringify({ locale: c.locale, many: true, cols: c.cols.map((col, k) => ({ name: col.name, fmt: col.fmt || "", first: k === 0, values: col.values })) })
    )
  );
  c.cols.forEach((col, k) => {
    if (!col.head) return;
    const p = profiles[k];
    const at = `${c.locale} ${col.name}`;
    assert.equal(norm(p.head), norm(col.head), `head of ${at}`);
    const got = col.values.map((v) => norm(tableNumText(typeof v === "number" ? v : parseFloat(v), p, col.fmt, c.locale, "minus")));
    assert.deepEqual(got, col.cells.map(norm), `cells of ${at}`);
    n++;
  });
}
assert.equal(humanHeader("load_pct"), "Load %");
console.log(`table_test: ${n} columns ok`);
