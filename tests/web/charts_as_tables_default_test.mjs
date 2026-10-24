// #403: the installation default for a viewer's initial "Charts as tables"
// state (web/app.js's `dpAsTables`). Pure precedence logic, no DOM — the
// three functions this extracts and grabs out of app.js:
//
//   ownAppBool(src, path)                         — own-property, later-wins
//                                                    read (shared with
//                                                    assistantRememberOptIn)
//   tablesExplicitChoice(urlParam, stored)         — "never set" vs
//                                                    "explicitly off", from
//                                                    ?tables= / the
//                                                    remembered toggle
//   chartsAsTablesShouldApplyDefault(explicit,
//     installDefault, current)                    — may the install default
//                                                    flip dpAsTables on?
//
// Owner decisions (#403, following #401's own-property pattern for
// assistantRememberOptIn): the default only ever seeds a viewer who never
// made an explicit choice; it never overrides an explicit off; it never
// forces the in-page `-- @astables on` bar.
import { readFileSync } from "fs";
import assert from "assert/strict";

const src = readFileSync(new URL("../../web/app.js", import.meta.url), "utf8");
const grab = (name) => {
  const m = src.match(new RegExp(`^function ${name}\\([\\s\\S]*?^}`, "m"));
  assert.ok(m, `function ${name} not found in app.js`);
  return m[0];
};
const { ownAppBool, tablesExplicitChoice, chartsAsTablesShouldApplyDefault } = new Function(
  `${grab("ownAppBool")}\n${grab("tablesExplicitChoice")}\n${grab("chartsAsTablesShouldApplyDefault")}\n` +
    `return { ownAppBool, tablesExplicitChoice, chartsAsTablesShouldApplyDefault };`
)();

// ---- ownAppBool: own-property, later-wins, prototype-pollution safe ----

// absent at every level → undefined, not false (callers distinguish "says
// nothing" from "says false" with ??)
assert.equal(ownAppBool(null, ["charts_as_tables"]), undefined);
assert.equal(ownAppBool({}, ["charts_as_tables"]), undefined);
assert.equal(ownAppBool({ charts_as_tables: true }, ["charts_as_tables"]), true);
assert.equal(ownAppBool({ charts_as_tables: false }, ["charts_as_tables"]), false);
// a non-boolean value is returned as-is — callers require `=== true`
assert.equal(ownAppBool({ charts_as_tables: "true" }, ["charts_as_tables"]), "true");
// nested path (assistantRememberOptIn's own shape), missing at either level
assert.equal(ownAppBool({ assistant: {} }, ["assistant", "remember"]), undefined);
assert.equal(ownAppBool({ assistant: { remember: true } }, ["assistant", "remember"]), true);
assert.equal(ownAppBool({ assistant: null }, ["assistant", "remember"]), undefined);
// Object.hasOwn, not a `?.` chain: a polluted Object.prototype must not be read
Object.prototype.charts_as_tables = true;
try {
  assert.equal(ownAppBool({}, ["charts_as_tables"]), undefined, "a polluted prototype must not be read as an own property");
} finally {
  delete Object.prototype.charts_as_tables;
}
// the "later wins, but only when it says something" combine pattern used by
// appBrandLoad(): window.__anofoxApp ?? file
const combine = (win, file) => (ownAppBool(win, ["charts_as_tables"]) ?? ownAppBool(file, ["charts_as_tables"])) === true;
assert.equal(combine({ charts_as_tables: true }, { charts_as_tables: false }), true, "the serving process wins when it says something");
assert.equal(combine({}, { charts_as_tables: true }), true, "app-config.json applies when window.__anofoxApp says nothing");
assert.equal(combine({ charts_as_tables: false }, { charts_as_tables: true }), false, "an explicit false from the serving process is not overridden by the file");
assert.equal(combine({}, {}), false, "neither source set it");
assert.equal(combine(null, null), false);

// ---- tablesExplicitChoice: "never set" vs "explicitly off" ----

assert.equal(tablesExplicitChoice(null, null), null, "neither ?tables= nor the remembered toggle was ever set");
assert.equal(tablesExplicitChoice(undefined, undefined), null);
assert.equal(tablesExplicitChoice("maybe", "nope"), null, "an unrecognized value on either side counts as never set");
assert.equal(tablesExplicitChoice("1", null), true, "?tables=1, nothing remembered yet");
assert.equal(tablesExplicitChoice("0", null), false, "?tables=0: explicitly off, not 'never set'");
assert.equal(tablesExplicitChoice(null, "1"), true, "the remembered toggle alone");
assert.equal(tablesExplicitChoice(null, "0"), false, "remembered off is still an explicit choice");
assert.equal(tablesExplicitChoice("1", "0"), true, "the URL wins over the remembered toggle");
assert.equal(tablesExplicitChoice("0", "1"), false, "…either way");

// ---- chartsAsTablesShouldApplyDefault: the actual #403 decision ----

// the headline case: a viewer who never chose, on an installation that
// defaults it on, and isn't already on — the default applies.
assert.equal(chartsAsTablesShouldApplyDefault(null, true, false), true);
// an explicit choice (on OR off) always wins, no matter what the
// installation says — this is decision #1 from the issue.
assert.equal(chartsAsTablesShouldApplyDefault(false, true, false), false, "an explicit off beats the install default");
assert.equal(chartsAsTablesShouldApplyDefault(true, true, false), false, "an explicit on needs no applying anyway");
assert.equal(chartsAsTablesShouldApplyDefault(false, true, true), false);
// no installation default set — never flips anything on
assert.equal(chartsAsTablesShouldApplyDefault(null, false, false), false);
// already on (e.g. a dashboard's own `-- @astables on` bar was clicked, or a
// second dashboard already applied the default) — nothing to re-render
assert.equal(chartsAsTablesShouldApplyDefault(null, true, true), false);

console.log("charts_as_tables_default_test: ok");
