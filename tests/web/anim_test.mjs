// Node test for web/anim.js's ::STORY key matching (run by tests/web_js.rs):
// a step's key lights the marks whose data-x is the same x, however each
// side spells it.
import assert from "assert/strict";
import { animKit } from "../../web/anim.js";

const { sameX } = animKit({ document: {} });

assert.ok(sameX("W43", "W43"));
assert.ok(!sameX("W43", "W44"));
assert.ok(!sameX(null, "W43"));
// numbers by value
assert.ok(sameX("2", "2.0"));
assert.ok(!sameX("2", "3"));
// dates: with or without a midnight time
assert.ok(sameX("2026-10-12", "2026-10-12 00:00:00"));
assert.ok(!sameX("2026-10-12", "2026-10-13"));
// an older SVG's epoch-seconds key against the marks' ISO date (time), in UTC
assert.ok(sameX("1788220800", "2026-09-01"));
assert.ok(sameX("2026-09-01", "1788220800"));
assert.ok(sameX("1788255000", "2026-09-01 09:30:00"));
assert.ok(!sameX("1788220800", "2026-09-02"));
// only a 9–11 digit number reads as epoch seconds
assert.ok(!sameX("123", "1970-01-01 00:02:03"));

console.log("anim_test: ok");
