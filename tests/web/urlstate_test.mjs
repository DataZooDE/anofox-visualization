// Node test for the viewer state in the URL (`?v.<name>=<value>`, issue #25):
// extracts readUrlSeed / isIsoDate / validSeed from web/app.js and checks that a
// seed only becomes a variable when it passes the input's own validation.
import { readFileSync } from "fs";
import assert from "assert/strict";

const src = readFileSync(new URL("../../web/app.js", import.meta.url), "utf8");
const grab = (name) => {
  const m = src.match(new RegExp(`^function ${name}\\([\\s\\S]*?^}`, "m"));
  assert.ok(m, `function ${name} not found in app.js`);
  return m[0];
};
const re = src.match(/^const URL_VAR_RE = .*$/m)[0];
const { readUrlSeed, validSeed, toggleValue } = new Function(
  `${re}\n${grab("readUrlSeed")}\n${grab("isIsoDate")}\n${grab("toggleValue")}\n${grab("validSeed")}\nreturn { readUrlSeed, validSeed, toggleValue };`
)();

// Only v.<identifier> keys; lists repeat the key; other params are ignored.
assert.deepEqual(readUrlSeed("?tab=X&v.region=EU&v.ch=a&v.ch=b&v.bad-name=1&v.x%3BDROP=1&embed=1"), {
  region: ["EU"],
  ch: ["a", "b"],
});
assert.deepEqual(readUrlSeed("?v.note=it%27s%20%3B--"), { note: ["it's ;--"] });
// "view." never collides with "v." (#214: ::VIEW's own ?view.<id>=<label>
// deep-link prefix, tests/web/views_test.mjs) — readUrlSeed must not pick up
// a view.* key just because it starts with "v".
assert.deepEqual(readUrlSeed("?view.yoy=Bars"), {});
assert.deepEqual(readUrlSeed("?view.yoy=Bars&v.region=EU"), { region: ["EU"] });

const opts = ["EU", "US"];
assert.equal(validSeed("DROPDOWN", ["US"], opts), "US");
assert.equal(validSeed("DROPDOWN", ["XX"], opts), undefined);
assert.equal(validSeed("DROPDOWN", ["EU", "US"], opts), undefined);
assert.deepEqual(validSeed("MULTISELECT", ["US", "XX"], opts), ["US"]);
assert.deepEqual(validSeed("MULTISELECT", [""], opts), []);
assert.equal(validSeed("MULTISELECT", ["XX"], opts), undefined);
assert.equal(validSeed("NUMBER", ["20"]), "20");
assert.equal(validSeed("NUMBER", ["-1.5e3"]), "-1.5e3");
for (const bad of ["abc", "1; DROP", "", "1e400", "0x10"]) assert.equal(validSeed("NUMBER", [bad]), undefined, bad);
assert.equal(validSeed("DATE", ["2024-02-29"]), "2024-02-29");
for (const bad of ["2024-02-30", "2023-02-29", "24-01-01", "2024-01-01'"]) assert.equal(validSeed("DATE", [bad]), undefined, bad);
assert.equal(validSeed("DATERANGE", ["2024-01-31"]), "2024-01-31");
assert.equal(validSeed("TEXT", ["promo"]), "promo");
assert.equal(validSeed("TEXT", ["x".repeat(501)]), undefined);
// ::BUTTONS validates like a dropdown; ::TOGGLE normalises to "true"/"false".
assert.equal(validSeed("BUTTONS", ["US"], opts), "US");
assert.equal(validSeed("BUTTONS", ["XX"], opts), undefined);
assert.equal(validSeed("TOGGLE", ["true"]), "true");
assert.equal(validSeed("TOGGLE", ["0"]), "false");
assert.equal(validSeed("TOGGLE", ["ON"]), "true");
for (const bad of ["maybe", "", "true;--"]) assert.equal(validSeed("TOGGLE", [bad]), undefined, bad);
assert.equal(validSeed("TOGGLE", ["true", "false"]), undefined);
assert.equal(toggleValue(true), "true");
assert.equal(toggleValue(false), "false");
assert.equal(toggleValue(null), "false");
// Locked server: a toggle's SET VARIABLE literal travels as a JSON boolean (the
// server binds it as a typed TRUE / FALSE), a button's as a string.
const sqlLiteralToJson = new Function(`${grab("sqlLiteralToJson")}\nreturn sqlLiteralToJson;`)();
assert.equal(sqlLiteralToJson("TRUE"), true);
assert.equal(sqlLiteralToJson("FALSE"), false);
assert.equal(sqlLiteralToJson("'Peak day'"), "Peak day");
console.log("urlstate_test: ok");
