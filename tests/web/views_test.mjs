// Node test for ::VIEW / ::ENDVIEW deep-linking (#214): extracts
// readViewSeed / setViewParam from web/app.js (same convention as
// tests/web/urlstate_test.mjs) and checks the seed validation rules and the
// "view." / "v." prefix non-collision.
import { readFileSync } from "fs";
import assert from "assert/strict";

const src = readFileSync(new URL("../../web/app.js", import.meta.url), "utf8");
const grab = (name) => {
  const m = src.match(new RegExp(`^function ${name}\\([\\s\\S]*?^}`, "m"));
  assert.ok(m, `function ${name} not found in app.js`);
  return m[0];
};
const re = src.match(/^const URL_VAR_RE = .*$/m)[0];
const { readViewSeed } = new Function(`${re}\n${grab("readViewSeed")}\nreturn { readViewSeed };`)();

// ---- readViewSeed: shape validation ----

// Only view.<id> keys, id = URL_VAR_RE or a 1-3 digit positional fallback.
assert.deepEqual(readViewSeed("?tab=X&view.yoy=Bars&view.2=Lines&v.region=EU&embed=1"), {
  yoy: "Bars",
  2: "Lines",
});
// A bad id (not URL_VAR_RE, not ^\d{1,3}$) is dropped.
assert.deepEqual(readViewSeed("?view.2x=Bars&view.1234=Lines&view.bad-name=X"), {});
// A positional id up to 3 digits is fine (leading zeros and all — it is
// matched as a string of digits, not parsed as a number); a value with
// special characters round-trips verbatim (decoded, not re-escaped).
assert.deepEqual(readViewSeed("?view.007=it%27s%20%3B--"), { "007": "it's ;--" });

// A value over 200 chars is dropped (its key never appears).
assert.deepEqual(readViewSeed("?view.a=" + "x".repeat(201)), {});
assert.deepEqual(readViewSeed("?view.a=" + "x".repeat(200)), { a: "x".repeat(200) });

// At most 32 distinct ids; the 33rd (a NEW id) is dropped, repeats of an
// already-admitted id still update it (shares its slot, doesn't count twice).
{
  const params = [];
  for (let i = 0; i < 40; i++) params.push(`view.v${i}=L${i}`);
  const seed = readViewSeed("?" + params.join("&"));
  assert.equal(Object.keys(seed).length, 32);
  assert.equal(seed.v0, "L0");
  assert.equal(seed.v31, "L31");
  assert.equal(seed.v32, undefined);
}
{
  // a repeated key for an id already in the (full) set still updates it
  const params = [];
  for (let i = 0; i < 32; i++) params.push(`view.v${i}=L${i}`);
  params.push("view.v0=L0b"); // repeat, already admitted
  params.push("view.v99=Lnew"); // a 33rd distinct id: dropped
  const seed = readViewSeed("?" + params.join("&"));
  assert.equal(Object.keys(seed).length, 32);
  assert.equal(seed.v0, "L0b");
  assert.equal(seed.v99, undefined);
}

// Labels that don't exactly match any of a group's CURRENT alternatives are
// simply ignored by the app (it falls back to the default, the first SHOWN
// alternative) — readViewSeed only validates the KEY's shape, not the value
// against a group's live alternatives (that match happens at render time,
// inline in web/app.js's runOnce, against a group built incrementally
// statement-by-statement; it has no standalone pure form to extract the way
// readViewSeed does, so it is not re-tested here — see the exact-match
// contract documented on setViewParam/readViewSeed above). What IS testable
// here is that readViewSeed treats such a label like any other opaque
// string: it round-trips verbatim, case and all (the match against
// alternatives is case-sensitive / exact, never normalised here).
assert.deepEqual(readViewSeed("?view.yoy=StaleLabel"), { yoy: "StaleLabel" });
assert.deepEqual(readViewSeed("?view.yoy=bars"), { yoy: "bars" }); // not "Bars": not normalised

// ---- "view." never collides with the plain "v." variable prefix ----
assert.deepEqual(readViewSeed("?v.region=EU"), {}, "readViewSeed must ignore v.* keys");
{
  const reV = src.match(/^const URL_VAR_RE = .*$/m)[0];
  const { readUrlSeed } = new Function(`${reV}\n${grab("readUrlSeed")}\nreturn { readUrlSeed };`)();
  assert.deepEqual(readUrlSeed("?view.yoy=Bars"), {}, "readUrlSeed must ignore view.* keys");
  assert.deepEqual(readUrlSeed("?view.yoy=Bars&v.region=EU"), { region: ["EU"] });
}

// ---- setViewParam: deletes the key when the label is the group's default ----
{
  globalThis.location = { href: "http://x.test/dash?tab=A&other=1" };
  const history = { replaceState: (_s, _t, u) => (globalThis.location.href = String(u)) };
  globalThis.history = history;
  const dpViewDefault = { yoy: "Lines" };
  const { setViewParam } = new Function("dpViewDefault", `${grab("setViewParam")}\nreturn { setViewParam };`)(dpViewDefault);
  setViewParam("yoy", "Bars"); // not the default: the key is set
  assert.ok(new URL(globalThis.location.href).searchParams.get("view.yoy") === "Bars");
  setViewParam("yoy", "Lines"); // the group's default: the key is removed
  assert.ok(new URL(globalThis.location.href).searchParams.get("view.yoy") === null);
  // ?tab=/other params are untouched either way (independent state, #214).
  assert.equal(new URL(globalThis.location.href).searchParams.get("tab"), "A");
  assert.equal(new URL(globalThis.location.href).searchParams.get("other"), "1");
  delete globalThis.location;
  delete globalThis.history;
}

// NOTE on coverage (per the plan): a patchSlots/shapeOf DOM-diff case for
// ".dp-viewbar [role=tab]" / ".dp-viewpane" exclusion, and the default-view
// fallback (no seed / an unmatched seed picks the first SHOWN alternative),
// are both exercised only inline in the live render loop (runOnce) and the
// DOM-manipulating shapeOf/patchSlots — neither has a standalone pure form
// this test file's convention (regex-extract + eval) can isolate without a
// much bigger DOM/harness than the rest of this suite uses, so — per the
// plan's explicit allowance for the patchSlots case — they are SKIPPED here
// rather than built as a fragile one-off harness. Both were instead verified
// by manual trace of every closer/hiding transition during implementation
// (new group / repeated alt / hidden alt / group-fully-hidden / closed via
// each of ::ENDVIEW, ::TAB, ::SUBTAB, ::SECTION, an outer ::ENDGROUP, and end
// of script) and are covered on the Rust side by src/sql/views.rs's and
// src/sql/requires.rs's own unit tests (`cargo test`).

console.log("views_test: ok");
