// Node test for #873: on a re-run (an input changed, a cross-filter click),
// a setup statement whose own variables did not change is still re-run when
// it reads a table an earlier setup statement of the same run rebuilt
// (forecast-callcentre-staffing: cc_plan reads plan_at → cc_erlang reads
// cc_plan only → cc_day reads cc_erlang).
import { readFileSync } from "fs";
import assert from "assert/strict";

const src = readFileSync(new URL("../../web/app.js", import.meta.url), "utf8");
const fn = src.match(/^function readsAnyOf\([\s\S]*?^}/m);
assert.ok(fn, "readsAnyOf not found in app.js");
// The run loop's skip rule consults it, and records what each setup builds.
assert.ok(
  src.includes("dpSetupRan.get(s.sql) === varSig(s.sql) && !readsAnyOf(s.sql, rebuilt)) continue;"),
  "the setup skip rule no longer consults readsAnyOf"
);
assert.ok(src.includes("if (made) rebuilt.add("), "the run loop no longer records rebuilt tables");

const sqlCode = (s) => s.replace(/--[^\n]*/g, "");
const readsAnyOf = new Function("sqlCode", `${fn[0]}; return readsAnyOf;`)(sqlCode);

const rebuilt = new Set(["cc_plan"]);
assert.equal(readsAnyOf("CREATE OR REPLACE TABLE cc_erlang AS SELECT * FROM cc_plan p JOIN x USING (q)", rebuilt), true);
assert.equal(readsAnyOf("CREATE TABLE y AS SELECT * FROM main.cc_plan", rebuilt), true, "schema-qualified");
assert.equal(readsAnyOf('CREATE TABLE y AS SELECT * FROM "CC_PLAN"', rebuilt), true, "quoted, any case");
assert.equal(readsAnyOf("CREATE TABLE y AS SELECT * FROM cc_plan_old", rebuilt), false, "a longer name");
assert.equal(readsAnyOf("CREATE TABLE y AS SELECT * FROM my_cc_plan", rebuilt), false, "a longer name");
assert.equal(readsAnyOf("CREATE TABLE fc AS SELECT * FROM ts_forecast_by('sales', id, ds, y)", rebuilt), false, "an unrelated forecast stays skipped");
assert.equal(readsAnyOf("-- from cc_plan\nCREATE TABLE y AS SELECT 1", rebuilt), false, "comments don't count");
assert.equal(readsAnyOf("SELECT * FROM cc_plan", new Set()), false, "nothing rebuilt");
console.log("setup_rerun_test: ok");
