// Searchable ::DROPDOWN (#411): the filter (ddMatches) and keyboard-wrap
// (ddWrapIndex) logic behind makeSearchableDropdown's combobox, extracted as
// standalone, DOM-free functions in web/app.js specifically so this can test
// them without a DOM (tests/web convention: no jsdom). The live widget
// itself — focus/open/close, aria-*, the hidden <select> fallback for the
// downloaded HTML snapshot — is scripts/dropdown-search-e2e.mjs.
import { readFileSync } from "fs";
import assert from "assert/strict";
const src = readFileSync(new URL("../../web/app.js", import.meta.url), "utf8");
const grab = (name) => {
  const m = src.match(new RegExp(`^function ${name}\\([\\s\\S]*?^}`, "m"));
  assert.ok(m, `function ${name} not found in app.js`);
  return m[0];
};
const grabConst = (name) => {
  const m = src.match(new RegExp(`^const ${name} = .*;$`, "m"));
  assert.ok(m, `const ${name} not found in app.js`);
  return m[0];
};
const { optionLabel, ddMatches, ddWrapIndex, dropdownIsSearchable, SEARCHABLE_OPTION_THRESHOLD } = new Function(`
  ${grabConst("SEARCHABLE_OPTION_THRESHOLD")}
  ${grab("optionLabel")}
  ${grab("ddMatches")}
  ${grab("ddWrapIndex")}
  ${grab("dropdownIsSearchable")}
  return { optionLabel, ddMatches, ddWrapIndex, dropdownIsSearchable, SEARCHABLE_OPTION_THRESHOLD };
`)();

// ---- ddMatches: value + ::OPTION_LABEL substring, case-insensitive -------
const meta = {
  options: ["ext_000", "ext_001", "ext_250", "ext_251"],
  labels: ["Zero", "One extension", "Two-fifty", null],
};
assert.deepEqual(ddMatches(meta, ""), [0, 1, 2, 3], "empty query: every option, in order");
assert.deepEqual(ddMatches(meta, "  "), [0, 1, 2, 3], "whitespace-only query: every option");
assert.deepEqual(ddMatches(meta, "25"), [2, 3], "filters by value substring");
assert.deepEqual(ddMatches(meta, "EXT_251"), [3], "case-insensitive");
assert.deepEqual(ddMatches(meta, "extension"), [1], "filters by ::OPTION_LABEL substring too");
assert.deepEqual(ddMatches(meta, "two-fifty"), [2], "label match is also case/substring insensitive");
assert.deepEqual(ddMatches(meta, "zzz"), [], "no matches: empty");
// A label that happens to equal another option's value must not cross-match
// the wrong row (the two fields are joined with a NUL, not concatenated raw).
const sep = { options: ["a", "ab"], labels: ["z", null] };
assert.deepEqual(ddMatches(sep, "az"), [], "a label + the next value never fuse into a false match");

// ---- ddWrapIndex: Arrow-key highlight wraps within [0, count) ------------
assert.equal(ddWrapIndex(0, 5), 0);
assert.equal(ddWrapIndex(-1, 5), 4, "ArrowUp from the first row wraps to the last");
assert.equal(ddWrapIndex(5, 5), 0, "ArrowDown past the last row wraps to the first");
assert.equal(ddWrapIndex(2, 5), 2);
assert.equal(ddWrapIndex(0, 0), -1, "no rows: nothing to highlight");
assert.equal(ddWrapIndex(-1, 0), -1);

// ---- dropdownIsSearchable: the threshold + ::DROPDOWN_STYLE override -----
const many = Array.from({ length: SEARCHABLE_OPTION_THRESHOLD + 1 }, (_, i) => String(i));
const few = Array.from({ length: SEARCHABLE_OPTION_THRESHOLD }, (_, i) => String(i));
assert.equal(dropdownIsSearchable({ options: many, dropdownStyle: null }), true, "above the threshold: searchable");
assert.equal(dropdownIsSearchable({ options: few, dropdownStyle: null }), false, "at the threshold: plain <select>");
assert.equal(dropdownIsSearchable({ options: many, dropdownStyle: "native" }), false, "'native'::DROPDOWN_STYLE overrides above the threshold");
assert.equal(dropdownIsSearchable({ options: few, dropdownStyle: "search" }), true, "'search'::DROPDOWN_STYLE overrides below the threshold");

process.stdout.write("dropdown_search_test: ok\n");
