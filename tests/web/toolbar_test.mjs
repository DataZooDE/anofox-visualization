// ::TOOLBAR in the browser (#354): app.js's parseToolbar — an allow-list of
// toolbar parts to keep ('none' = none) for a small ::TABLE / ::PAGED panel
// that never needs the full chrome (search, filters, totals, stats, columns,
// density, export). Pure logic, no wasm needed.
import { readFileSync } from "fs";
import assert from "assert/strict";

const src = readFileSync(new URL("../../web/app.js", import.meta.url), "utf8");
const grab = (name) => {
  const m = src.match(new RegExp(`^function ${name}\\([\\s\\S]*?^}`, "m"));
  assert.ok(m, `function ${name} not found in app.js`);
  return m[0];
};
const parts = src.match(/^const TOOLBAR_PARTS = .*;$/m);
assert.ok(parts, "const TOOLBAR_PARTS not found in app.js");
const { parseToolbar, TOOLBAR_PARTS } = new Function(
  `${parts[0]}\n${grab("parseToolbar")}\nreturn { parseToolbar, TOOLBAR_PARTS };`
)();

// unset (no ::TOOLBAR directive at all): show everything — the pre-#354 default.
assert.equal(parseToolbar(undefined), null);
assert.equal(parseToolbar(null), null);
assert.equal(parseToolbar(""), null);
assert.equal(parseToolbar("   "), null);

// 'none' (any case / surrounding whitespace) hides the whole toolbar — the
// same as an empty allow-list.
for (const spec of ["none", "None", " NONE "]) assert.deepEqual(parseToolbar(spec), new Set());

// an allow-list keeps only the named parts, case-insensitive, ',' or ';' separated.
assert.deepEqual(parseToolbar("search,export"), new Set(["search", "export"]));
assert.deepEqual(parseToolbar("Search; Export"), new Set(["search", "export"]));
assert.deepEqual(parseToolbar(" density "), new Set(["density"]));

// every documented part name round-trips.
assert.deepEqual(parseToolbar(TOOLBAR_PARTS.join(",")), new Set(TOOLBAR_PARTS));
assert.deepEqual(
  TOOLBAR_PARTS.sort(),
  ["columns", "density", "export", "filters", "search", "stats", "totals"].sort()
);

// unknown part names are dropped, not errors (like an ::TOTALS entry naming
// no column of the table — ignored, never `dashboard --check` noise).
assert.deepEqual(parseToolbar("search,bogus"), new Set(["search"]));
assert.deepEqual(parseToolbar("bogus"), new Set());

console.log("toolbar_test ok");
