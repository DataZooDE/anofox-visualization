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

// Which variables a panel reads (its re-query trigger).
assert.deepEqual(varsOf("SELECT 1 WHERE a = getvariable('region') AND list_contains(GETVARIABLE( 'channel' ), c)"), ["region", "channel"]);
assert.deepEqual(varsOf("SELECT 1"), []);
console.log("panel_states_test: ok");
