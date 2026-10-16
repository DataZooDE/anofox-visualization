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
const { readUrlSeed, validSeed } = new Function(
  `${re}\n${grab("readUrlSeed")}\n${grab("isIsoDate")}\n${grab("validSeed")}\nreturn { readUrlSeed, validSeed };`
)();

// Only v.<identifier> keys; lists repeat the key; other params are ignored.
assert.deepEqual(readUrlSeed("?tab=X&v.region=EU&v.ch=a&v.ch=b&v.bad-name=1&v.x%3BDROP=1&embed=1"), {
  region: ["EU"],
  ch: ["a", "b"],
});
assert.deepEqual(readUrlSeed("?v.note=it%27s%20%3B--"), { note: ["it's ;--"] });

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
console.log("urlstate_test: ok");
