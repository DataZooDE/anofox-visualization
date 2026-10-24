// Node test for #428 (web/app.js's inputsToPreset, used by runOnce's render
// loop): an input's OPTIONS query that reads another input's variable via
// getvariable() must see that other input's CURRENT value even when it is
// declared AFTER it in the script — the pre-pass loop otherwise reaches an
// input's own `SET VARIABLE` in script order, one statement at a time, so
// an earlier-indexed input's options query only ever sees a LATER input's
// value from a previous run (locally) or never (on a locked server, which
// opens a fresh connection per request and never gets a second chance).
// inputsToPreset is the pure resolve half of the fix: given the statements
// and this run's already-known state (dpVars, dpUrlSeed), which `SET
// VARIABLE`s to apply before any OPTIONS query runs — the variable name
// comes straight from the plan (src/sql.rs's Panel::names via a role's 3rd
// array entry, tests/sql_edge_cases.rs's input_variable_name_is_recorded_
// statically), never from running the statement.
//
// No jsdom: inputsToPreset, inputVarName, inputCandidateLiteral, inputKind,
// role, toggleValue, validSeed and isIsoDate are pure, extracted from app.js
// like setvar_order_test.mjs does.
import { readFileSync } from "fs";
import assert from "assert/strict";

const src = readFileSync(new URL("../../web/app.js", import.meta.url), "utf8");
const fn = (name) => {
  const m = src.match(new RegExp(`^function ${name}\\([\\s\\S]*?^}`, "m"));
  assert.ok(m, `function ${name} not found in app.js`);
  return m[0];
};
const line = (name) => {
  const m = src.match(new RegExp(`^(?:let|const) ${name} = .*$`, "m"));
  assert.ok(m, `let/const ${name} not found in app.js`);
  return m[0];
};
const { inputsToPreset, inputCandidateLiteral } = new Function(
  `${line("INPUTS")}\n${line("role")}\n${line("inputKind")}\n${line("inputVarName")}\n${fn("toggleValue")}\n${fn("isIsoDate")}\n${fn("validSeed")}\n${fn("inputCandidateLiteral")}\n${fn("inputsToPreset")}\nreturn { inputsToPreset, inputCandidateLiteral };`
)();

// A plan statement: `setup`, `roles` (as the wasm plan() JSON gives them —
// [colIdx, "ROLE", name]), `sql` (unused by inputsToPreset, kept for shape).
const input = (kind, varname, sql = "") => ({ setup: false, sql, roles: [[0, kind, varname]] });
const setup = (sql = "") => ({ setup: true, sql, roles: [] });

// 1. The #428 repro: a dropdown declared BEFORE the ::TEXT box whose
// variable its OPTIONS query reads — a previous pick for `q` must be
// applied before the dropdown's own turn.
{
  const stmts = [input("DROPDOWN", "extension"), input("TEXT", "q")];
  const out = inputsToPreset(stmts, { q: "ano" }, {});
  assert.deepEqual(out, [{ varname: "q", sql: "SET VARIABLE q = 'ano'" }]);
}

// 2. A URL seed, not yet a dpVars pick (first load with ?v.q=... in the
// address bar) — peeked from dpUrlSeed, not a dpVars entry.
{
  const stmts = [input("DROPDOWN", "extension"), input("TEXT", "q")];
  const out = inputsToPreset(stmts, {}, { q: ["ano"] });
  assert.deepEqual(out, [{ varname: "q", sql: "SET VARIABLE q = 'ano'" }]);
}

// 3. Neither a pick nor a seed: nothing to preset (the input's own turn,
// unchanged, computes its default as it always did).
{
  const stmts = [input("DROPDOWN", "extension"), input("TEXT", "q")];
  assert.deepEqual(inputsToPreset(stmts, {}, {}), []);
}

// 4. Every listed/typed kind gets the right literal quoting.
{
  const stmts = [
    input("NUMBER", "n"),
    input("TOGGLE", "on_off"),
    input("MULTISELECT", "ms"),
    input("DATE", "d"),
    input("TEXT", "t"),
  ];
  const vars = { n: "42", on_off: "yes", ms: ["a", "b"], d: "2026-01-01", t: "it's fine" };
  const out = inputsToPreset(stmts, vars, {});
  assert.deepEqual(out, [
    { varname: "n", sql: "SET VARIABLE n = 42" },
    { varname: "on_off", sql: "SET VARIABLE on_off = TRUE" },
    { varname: "ms", sql: "SET VARIABLE ms = ['a','b']" },
    { varname: "d", sql: "SET VARIABLE d = '2026-01-01'" },
    { varname: "t", sql: "SET VARIABLE t = 'it''s fine'" },
  ]);
}

// 5. A MULTISELECT's URL seed is the whole list, not just its first value.
{
  const stmts = [input("MULTISELECT", "ms")];
  const out = inputsToPreset(stmts, {}, { ms: ["a", "b", "c"] });
  assert.deepEqual(out, [{ varname: "ms", sql: "SET VARIABLE ms = ['a','b','c']" }]);
}

// 6. PERIOD and DATERANGE are left to their own turn (neither narrows from
// an OPTIONS list, so there is nothing here for them to unblock).
{
  const stmts = [input("PERIOD", "p"), input("DATERANGE", "dr")];
  assert.deepEqual(inputsToPreset(stmts, { p: { preset: "30d" }, dr: "2026-01-01" }, {}), []);
}

// 7. setup statements and an input with no statically-known name (an
// unaliased, non-trivial expression — the plan's roles entry has no 3rd
// element) are skipped; that input is resolved from its own turn as before.
{
  const stmts = [setup("CREATE TABLE t AS SELECT 1"), { setup: false, sql: "", roles: [[0, "DROPDOWN"]] }];
  assert.deepEqual(inputsToPreset(stmts, { extension: "x" }, {}), []);
}

// 8. `?v.q=` (an explicit, empty value) decodes to `[""]` (readUrlSeed), not
// an empty array — a legitimate TEXT value, so it IS preset (unlike a name
// absent from dpUrlSeed entirely, case 3 above).
{
  const stmts = [input("TEXT", "q")];
  assert.deepEqual(inputsToPreset(stmts, {}, { q: [""] }), [{ varname: "q", sql: "SET VARIABLE q = ''" }]);
}

// 9. Security (PR #436 review): a NUMBER's URL seed is UNVALIDATED at this
// point (seedInputFromUrl's own validSeed() call hasn't run yet) — the one
// unquoted literal, so anything that isn't actually a number must never
// reach `SET VARIABLE n = <here>` as text, or it's arbitrary SQL. Neither
// payload produces a preset at all (validSeed rejects both, same as
// seedInputFromUrl already does on master); a genuinely valid number does.
{
  const stmts = [input("NUMBER", "n")];
  assert.deepEqual(inputsToPreset(stmts, {}, { n: ["1; SELECT 42"] }), []);
  assert.deepEqual(inputsToPreset(stmts, {}, { n: ["1) OR (1=1"] }), []);
  assert.deepEqual(inputsToPreset(stmts, {}, { n: ["12.5"] }), [{ varname: "n", sql: "SET VARIABLE n = 12.5" }]);
}

// 10. Security: inputCandidateLiteral itself (defense in depth) never emits
// a NUMBER's `v` unquoted without checking it first — a `dpVars` pick can
// reach it from somewhere other than validSeed, so the same injection
// payload as case 9, arriving as an already-"picked" value instead of a
// raw seed, still comes out as the inert NULL, never as the payload text.
{
  assert.equal(inputCandidateLiteral("NUMBER", "1; SELECT 42"), "NULL");
  assert.equal(inputCandidateLiteral("NUMBER", "1) OR (1=1"), "NULL");
  assert.equal(inputCandidateLiteral("NUMBER", "12.5"), "12.5");
  assert.equal(inputCandidateLiteral("NUMBER", "-3"), "-3");
}
{
  const stmts = [input("NUMBER", "n")];
  // Same payload, but already a `vars` (dpVars) pick, not a URL seed —
  // inputsToPreset doesn't re-validate that path (case 9 covers the seed
  // path); inputCandidateLiteral's own check (case 10) is what catches it.
  assert.deepEqual(inputsToPreset(stmts, { n: "1; SELECT 42" }, {}), [{ varname: "n", sql: "SET VARIABLE n = NULL" }]);
}

// 11. Security: a TEXT seed containing a quote is still escaped (the
// seed-validation path, case 9, doesn't change the existing quoting).
{
  const stmts = [input("TEXT", "t")];
  assert.deepEqual(inputsToPreset(stmts, {}, { t: ["a ' b"] }), [{ varname: "t", sql: "SET VARIABLE t = 'a '' b'" }]);
}

// 12. Security: a DROPDOWN/BUTTONS/MULTISELECT seed can't be checked
// against its own options yet (not fetched at this point) — it is still
// only ever quoted, escaped text, never anything that could be read back
// as SQL, whatever it contains.
{
  const stmts = [input("DROPDOWN", "extension")];
  assert.deepEqual(inputsToPreset(stmts, {}, { extension: ["a'); DROP TABLE t; --"] }), [
    { varname: "extension", sql: "SET VARIABLE extension = 'a''); DROP TABLE t; --'" },
  ]);
}

console.log("input_options_order_test: ok");
