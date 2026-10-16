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
console.log("interaction_test: ok");
