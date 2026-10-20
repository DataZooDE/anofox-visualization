// Node test for the ::ASOF freshness chip (issue #34): extracts asofParse /
// asofStale / asofRelative from web/app.js and checks they follow the same
// rules as src/freshness.rs (text without an offset is UTC). Also covers the
// date-only chip (#282): asofDay / asofText read a DATE as a calendar day,
// with no time and no time-zone shift.
process.env.TZ = "Europe/Berlin";
import { readFileSync } from "fs";
import assert from "assert/strict";

const src = readFileSync(new URL("../../web/app.js", import.meta.url), "utf8");
const grab = (name) => {
  const m = src.match(new RegExp(`^function ${name}\\([\\s\\S]*?^}`, "m"));
  assert.ok(m, `function ${name} not found in app.js`);
  return m[0];
};
const { asofParse, asofStale, asofRelative, asofDay, asofText } = new Function(
  `${grab("asofParse")}\n${grab("asofStale")}\n${grab("asofRelative")}\n${grab("asofDay")}\n${grab("asofText")}\nreturn { asofParse, asofStale, asofRelative, asofDay, asofText };`
)();

const at = 1792159320000; // 2026-10-16 14:02:00 UTC
assert.equal(asofParse("2026-10-16 14:02:00"), at);
assert.equal(asofParse("2026-10-16T14:02"), at);
assert.equal(asofParse("2026-10-16 14:02:00.123456Z"), at + 123);
assert.equal(asofParse("2026-10-16 16:02:00+02"), at);
assert.equal(asofParse("2026-10-16 12:32:00-0130"), at);
assert.equal(asofParse("1970-01-01"), 0);
assert.equal(asofParse(at), at); // DuckDB-Wasm hands timestamps over as epoch ms
for (const bad of ["", null, "yesterday", "2026-10-16 14:02 PST"]) assert.equal(asofParse(bad), null, String(bad));

assert.equal(asofStale("stale=24h"), 86400);
assert.equal(asofStale("stale = 90m"), 5400);
assert.equal(asofStale("1w"), 604800);
for (const bad of [undefined, "stale=", "stale=24", "soon"]) assert.equal(asofStale(bad), 0, String(bad));

assert.equal(asofRelative(-5), "just now");
assert.equal(asofRelative(600), "10 min ago");
assert.equal(asofRelative(2 * 3600 + 5), "2 h ago");
assert.equal(asofRelative(3 * 86400), "3 d ago");

assert.equal(asofDay("2026-01-01"), "2026-01-01");
assert.equal(asofDay("2026-01-01 00:00:00"), null);
assert.equal(
  asofText({ at: asofParse("2026-01-01"), day: "2026-01-01", stale: 0 }, Date.UTC(2026, 9, 19, 12), "de-DE").text,
  "Data as of 1.1.2026 (291 d ago)"
);
assert.ok(
  asofText({ at: asofParse("2026-01-01"), day: "2026-01-01", stale: 0 }, Date.UTC(2026, 0, 1, 10), "de-DE").text.endsWith(
    "(today)"
  )
);
assert.ok(asofText({ at, day: null, stale: 0 }, at + 1000, "de-DE").text.includes(":"));
console.log("asof_test: ok");
