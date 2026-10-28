// Node test for a named time map's frame in the URL (#647): extracts
// readFrameSeed from web/app.js (the tests/web/views_test.mjs convention) and
// checks its validation rules and the "frame." / "v." / "view." prefixes.
import { readFileSync } from "fs";
import assert from "assert/strict";

const src = readFileSync(new URL("../../web/app.js", import.meta.url), "utf8");
const m = src.match(/^function readFrameSeed\([\s\S]*?^}/m);
assert.ok(m, "readFrameSeed not found in app.js");
const raw = new Function(`${m[0]}\nreturn readFrameSeed;`)();
const seed = (s) => ({ ...raw(s) });

// Only frame.<name> keys; v.* and view.* belong to others.
assert.deepEqual(seed("?frame.weekly=Week%205&v.frame=x&view.weekly=Map&tab=On"), { weekly: "Week 5" });
// A `-- @name` may hold hyphens and underscores, not dots or a leading digit.
assert.deepEqual(seed("?frame.on-time_map=3&frame.1bad=x&frame.a.b=y&frame.=z"), { "on-time_map": "3" });
// Values over 200 chars are dropped; 200 is fine.
assert.deepEqual(seed("?frame.a=" + "x".repeat(201)), {});
assert.deepEqual(seed("?frame.a=" + "x".repeat(200)), { a: "x".repeat(200) });
// At most 32 names; a repeated name keeps the last value.
const many = Array.from({ length: 40 }, (_, i) => `frame.n${i}=${i}`).join("&");
assert.equal(Object.keys(seed("?" + many)).length, 32);
assert.deepEqual(seed("?frame.a=1&frame.a=2"), { a: "2" });
// Inherited Object.prototype names are ordinary own keys (null prototype).
const r = raw("?frame.constructor=1&frame.__proto__=2");
assert.equal(Object.getPrototypeOf(r), null);
assert.equal(r.constructor, "1");
assert.equal(r["__proto__"], "2");
// Garbage never throws.
assert.deepEqual(seed("%%%"), {});
console.log("frame_url_test: ok");
