// Node test for the per-panel state helpers in web/app.js (issue #35): the
// short error line / details of an error card, the missing-input class behind
// `-- @requires`, and the variables a panel reads (which panels re-query).
import { readFileSync } from "fs";
import assert from "assert/strict";

const src = readFileSync(new URL("../../web/app.js", import.meta.url), "utf8");
const grab = (name) => {
  const m = src.match(new RegExp(`^function ${name}\\([\\s\\S]*?^}`, "m"));
  assert.ok(m, `function ${name} not found in app.js`);
  return m[0];
};
const varsOfSrc = src.match(/^const varsOf = .*$/m);
assert.ok(varsOfSrc, "varsOf not found");
const reqTextSrc = src.match(/^const requiresText = .*$/m);
assert.ok(reqTextSrc, "requiresText not found");
const dpVarNow = {};
const { shortError, errorDetail, isMissingInputError, varsOf, requiresText, requiresName } = new Function(
  "dpVarNow",
  `${grab("shortError")}\n${grab("errorDetail")}\n${grab("isMissingInputError")}\n${varsOfSrc[0]}\n` +
    `${reqTextSrc[0]}\n${grab("requiresName")}\n` +
    `return { shortError, errorDetail, isMissingInputError, varsOf, requiresText, requiresName };`
)(dpVarNow);

// The short line: the cause, no "Error:" prefix, no LINE pointer, capped.
const duck = 'Error: Binder Error: Referenced column "revnue" not found in FROM clause!\nCandidate bindings: "revenue"\n\nLINE 1: SELECT revnue\n               ^';
assert.equal(shortError(duck), 'Binder Error: Referenced column "revnue" not found in FROM clause!');
assert.equal(shortError("x".repeat(300)).length, 158);
assert.equal(shortError(""), "Unknown error");
// Details keep the message but drop JS stack frames and DuckDB stack traces.
const withStack = "Error: IO Error: boom\n    at runSql (app.js:1:2)\n    at async runOnce (app.js:3:4)\n\nStack Trace:\n0 duckdb::X\n1 duckdb::Y";
assert.equal(errorDetail(withStack), "Error: IO Error: boom");
assert.equal(shortError(withStack), "IO Error: boom");

// Missing optional inputs (DuckDB native, DuckDB-Wasm fetch, a locked server).
for (const m of [
  'IO Error: No files found that match the pattern "data/run/x.parquet"',
  "couldn't fetch data/events/anomalies.parquet (HTTP 404)",
  "Catalog Error: Table with name anomalies does not exist!",
  "IO Error: Cannot open file \"x.csv\": No such file or directory",
])
  assert.ok(isMissingInputError(m), m);
for (const m of ['Binder Error: Referenced column "y" not found', "Parser Error: syntax error at or near"])
  assert.ok(!isMissingInputError(m), m);

// `-- @requires` requirements (alternatives within one) and the file a path names.
assert.equal(requiresText([["a.parquet", "b.parquet"], ["c.csv"]]), "a.parquet | b.parquet, c.csv");
assert.equal(requiresText(undefined), "");
Object.assign(dpVarNow, { run: "'r1'", n: "3", rd: "getvariable('anofox_data') || '/r1'" });
assert.equal(requiresName("${rd}/forecasts.parquet"), "forecasts.parquet");
assert.equal(requiresName("${rd}/${run}.parquet"), "r1.parquet");
assert.equal(requiresName("${rd}/part_${n}.csv"), "part_3.csv");
assert.equal(requiresName("${unset}"), "${unset}");
assert.equal(requiresName("${rd}"), "${rd}"); // an expression: not shown as a value

// A ::TAB / ::SUBTAB with `-- @requires` (#163): shown when each requirement
// has an existing alternative. Builder: `${name}` is the variable's value, a
// relative path resolves against anofox_data, glob() finds it; served: the
// server answers the tab's own statement.
{
  const m = src.match(/^async function tabShown\([\s\S]*?^}/m);
  assert.ok(m, "tabShown not found in app.js");
  const reqsOfSrc = src.match(/^const reqsOf = .*$/m)[0];
  const isOptSrc = src.match(/^const isOptionalMissing = .*$/m)[0];
  const mk = (served, files, vars, fail) => {
    const asked = [];
    const runSql = async (sql) => {
      asked.push(sql);
      if (fail) throw new Error(fail);
      const g = /FROM glob\('((?:[^']|'')*)'\)/.exec(sql);
      if (g) return JSON.stringify([{ n: String(files.filter((f) => f === g[1].replace(/''/g, "'")).length) }]);
      const row = {};
      [...sql.matchAll(/getvariable\('(\w+)'\) AS VARCHAR\) AS (v\d+)/g)].forEach(([, n, k]) => (row[k] = vars[n] ?? null));
      return JSON.stringify([row]);
    };
    const f = new Function("runSql", "servedMode", `${grab("isMissingInputError")}\n${reqsOfSrc}\n${isOptSrc}\n${m[0]}\nreturn tabShown;`)(runSql, served);
    return { tabShown: f, asked };
  };
  const tab = (requires) => ({ sql: "SELECT 'A' AS c0", requires, roles: [[0, "TAB"]] });
  let t = mk(false, ["/srv/runs/r1/fc.parquet"], { rd: "/srv/runs/r1", anofox_data: "/srv/runs" });
  assert.equal(await t.tabShown(tab([])), true);
  assert.equal(t.asked.length, 0, "no requires: no query");
  assert.equal(await t.tabShown(tab([["${rd}/x.parquet", "${rd}/fc.parquet"]])), true);
  assert.equal(await t.tabShown(tab([["${rd}/fc.parquet"], ["${rd}/x.parquet"]])), false, "every requirement");
  assert.equal(await t.tabShown(tab([["r1/fc.parquet"]])), true, "relative to anofox_data");
  assert.equal(await t.tabShown(tab([["${unset}/fc.parquet"]])), false, "an unset variable: missing");
  t = mk(false, ["a'b.csv"], {});
  assert.equal(await t.tabShown(tab([["a'b.csv"]])), true, "quotes are escaped");
  t = mk(false, [], {}, "IO Error: glob failed");
  assert.equal(await t.tabShown(tab([["x.csv"]])), false, "a failing glob: missing");
  // Served: the server's answer to the tab statement.
  t = mk(true, [], {}, "optional input missing: no files found for this panel's -- @requires");
  assert.equal(await t.tabShown(tab([["${rd}/fc.parquet"]])), false);
  assert.deepEqual(t.asked, ["SELECT 'A' AS c0"]);
  t = mk(true, [], {}, "Binder Error: something else");
  assert.equal(await t.tabShown(tab([["${rd}/fc.parquet"]])), true, "another error: shown (its panels report)");
  t = mk(true, [], {});
  assert.equal(await t.tabShown(tab([["${rd}/fc.parquet"]])), true);
}

// Which variables a panel reads (its re-query trigger).
assert.deepEqual(varsOf("SELECT 1 WHERE a = getvariable('region') AND list_contains(GETVARIABLE( 'channel' ), c)"), ["region", "channel"]);
assert.deepEqual(varsOf("SELECT 1"), []);
console.log("panel_states_test: ok");
