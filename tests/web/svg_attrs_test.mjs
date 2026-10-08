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
console.log("web svg attribute tests: ok");
