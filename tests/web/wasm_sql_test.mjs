// #1025: wasmSql (web/lint.js) spells `current_date` as today() for the
// in-browser engine — where `current_date` in an expression traps once the
// database is opened with db.open(…) — and leaves literals, quoted
// identifiers, comments and longer names alone.
import assert from "assert/strict";
import { wasmSql } from "../../web/lint.js";

const cases = [
  ["SELECT day(current_date) ::METRIC", "SELECT day(today()) ::METRIC"],
  ["SELECT CURRENT_DATE - INTERVAL 7 DAY", "SELECT today() - INTERVAL 7 DAY"],
  ["SELECT current_date() AS d", "SELECT today() AS d"],
  ["SELECT current_date ( ) AS d", "SELECT today() AS d"],
  ["WHERE d < current_date AND e > current_date", "WHERE d < today() AND e > today()"],
  ["SELECT 'current_date' AS s", "SELECT 'current_date' AS s"],
  ["SELECT 'it''s current_date' AS s", "SELECT 'it''s current_date' AS s"],
  ['SELECT "current_date" FROM t', 'SELECT "current_date" FROM t'],
  ["SELECT 1 -- current_date\n, current_date", "SELECT 1 -- current_date\n, today()"],
  ["SELECT /* current_date */ 1", "SELECT /* current_date */ 1"],
  ["SELECT my_current_date, current_dates, t.current_date FROM t", "SELECT my_current_date, current_dates, t.current_date FROM t"],
  ["SELECT 1 ::METRIC", "SELECT 1 ::METRIC"],
];
for (const [inp, want] of cases) assert.equal(wasmSql(inp), want, inp);
console.log(`wasm sql: ${cases.length} cases ok`);
