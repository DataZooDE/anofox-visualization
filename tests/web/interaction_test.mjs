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
const { parseSelectAs, xKindOf } = new Function(
  `${grab("parseSelectAs")}\n${grab("xKindOf")}\nreturn { parseSelectAs, xKindOf };`
)();

assert.deepEqual(parseSelectAs("region"), { name: "region", mode: "single" });
assert.deepEqual(parseSelectAs(" region multi "), { name: "region", mode: "multi" });
assert.deepEqual(parseSelectAs("region:MULTI"), { name: "region", mode: "multi" });
for (const bad of ["", "1x", "a b", "region many", "x;DROP", null]) assert.equal(parseSelectAs(bad), null, String(bad));

assert.equal(xKindOf(["1", "2.5", "-3"]), "num");
assert.equal(xKindOf(["2024-01-01", "2024-02-01 00:00:00"]), "date");
assert.equal(xKindOf(["EU", "US"]), null);
assert.equal(xKindOf([]), null);
console.log("interaction_test: ok");
