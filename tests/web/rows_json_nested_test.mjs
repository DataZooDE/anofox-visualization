// #468: `rowsJson` (web/lint.js) converts a DuckDB-Wasm result to rows' JSON
// text for the renderer. DuckDB-Wasm's own `row.toJSON()` only resolves a
// row's *top-level* columns to plain values — a LIST/STRUCT/MAP's own
// elements stay live Arrow Vector/StructRow/MapRow objects, so a DECIMAL
// nested inside one reached the renderer as its raw, unscaled mantissa (a
// bare `0.1` in a DECIMAL(4,2) column's list became `"10"`) and a nested
// DATE/TIMESTAMP as a raw epoch number instead of an ISO string — found
// while building the #453/#425 ::SPARKBAR e2e, which routed around it with
// explicit ::DOUBLE casts. This proves the general fix (`fromArrow` walking
// a value alongside its own Arrow type, recursively over LIST/STRUCT/MAP)
// against real DuckDB-Wasm results, not a mock — no wasm-pack build or
// anofox extensions needed, `rowsJson` doesn't touch the Rust renderer.
//
//   npm install --prefix tests/web
//   node tests/web/rows_json_nested_test.mjs
process.env.DATAZOO_DISABLE_TELEMETRY = "1"; // no usage telemetry, whatever was inherited
import { createRequire } from "module";
import path from "path";
import assert from "assert/strict";
import { rowsJson } from "../../web/lint.js";

const here = path.dirname(new URL(import.meta.url).pathname);
const require = createRequire(path.join(here, "package.json"));
let duckdb, dist;
try {
  duckdb = require("@duckdb/duckdb-wasm/dist/duckdb-node-blocking.cjs");
  dist = path.dirname(require.resolve("@duckdb/duckdb-wasm/dist/duckdb-node-blocking.cjs"));
} catch (_) {
  console.log("rows_json_nested_test skipped: @duckdb/duckdb-wasm not installed (npm install --prefix tests/web)");
  console.log("rows_json_nested_test: ok");
  process.exit(0);
}

const db = await duckdb.createDuckDB(
  { mvp: { mainModule: path.join(dist, "duckdb-mvp.wasm"), mainWorker: path.join(dist, "duckdb-node-mvp.worker.cjs") } },
  new duckdb.VoidLogger(),
  duckdb.NODE_RUNTIME
);
await db.instantiate();
db.open({});
const c = db.connect();
const rows = (sql) => JSON.parse(rowsJson(c.query(sql)));

// ---- DECIMAL nested in a LIST, at several widths/scales (#468's ask) ----
assert.deepEqual(
  rows("SELECT [0.1, 0.25, 1.3]::DECIMAL(4,1)[] AS v")[0],
  { v: [0.1, 0.3, 1.3] }, // DECIMAL(4,1): 0.25 itself doesn't fit scale 1, rounds to 0.3
  "DECIMAL(4,1)[] unscales to the real values, not the raw mantissa"
);
assert.deepEqual(
  rows("SELECT [123456.789, -42.125]::DECIMAL(18,3)[] AS v")[0],
  { v: [123456.789, -42.125] },
  "DECIMAL(18,3)[]: wider precision, still unscaled correctly"
);
assert.deepEqual(
  rows("SELECT [1234.5678901234]::DECIMAL(38,10)[] AS v")[0],
  { v: [1234.5678901234] },
  "DECIMAL(38,10)[]: the widest DuckDB decimal, still unscaled correctly"
);

// ---- NULLs and negatives inside the list, and a NULL list itself ----
assert.deepEqual(
  rows("SELECT [NULL, -0.5, 0.25]::DECIMAL(4,2)[] AS v")[0],
  { v: [null, -0.5, 0.25] },
  "a NULL element and a negative value both unscale correctly"
);
assert.deepEqual(rows("SELECT NULL::DECIMAL(4,2)[] AS v")[0], { v: null }, "a NULL list itself stays NULL, not an empty/broken walk");

// ---- DECIMAL nested in a STRUCT, and a STRUCT inside a LIST (the
// #425/#453 ::SPARKBAR ref/cur shape this was found through) ----
assert.deepEqual(
  rows("SELECT {'ref': 0.1, 'cur': 0.25}::STRUCT(ref DECIMAL(4,2), cur DECIMAL(4,2)) AS v")[0],
  { v: { ref: 0.1, cur: 0.25 } },
  "DECIMAL nested in a STRUCT unscales correctly"
);
assert.deepEqual(
  rows("SELECT {'ref': NULL, 'cur': 0.5}::STRUCT(ref DECIMAL(4,2), cur DECIMAL(4,2)) AS v")[0],
  { v: { ref: null, cur: 0.5 } },
  "a NULL field inside a STRUCT stays NULL, its sibling still unscales"
);
assert.deepEqual(
  rows(
    "SELECT [{'ref': 0.1, 'cur': 0.3}, {'ref': 0.3, 'cur': 0.3}]::STRUCT(ref DECIMAL(4,2), cur DECIMAL(4,2))[] AS v"
  )[0],
  { v: [{ ref: 0.1, cur: 0.3 }, { ref: 0.3, cur: 0.3 }] },
  "LIST<STRUCT<DECIMAL>> (the ::SPARKBAR ref/cur shape) unscales every element"
);

// ---- DECIMAL nested in a MAP ----
assert.deepEqual(
  rows("SELECT MAP {'a': 0.1, 'b': 0.25}::MAP(VARCHAR, DECIMAL(4,2)) AS v")[0],
  { v: { a: 0.1, b: 0.25 } },
  "DECIMAL nested in a MAP's values unscales correctly"
);

// ---- DATE / TIMESTAMP nested in a LIST (same bug class, same fix) ----
assert.deepEqual(
  rows("SELECT [DATE '2026-01-01', NULL, DATE '2026-02-15']::DATE[] AS v")[0],
  { v: ["2026-01-01", null, "2026-02-15"] },
  "DATE nested in a LIST becomes an ISO string, not a raw epoch number"
);
assert.deepEqual(
  rows("SELECT [TIMESTAMP '2026-01-01 10:00:00']::TIMESTAMP[] AS v")[0],
  { v: ["2026-01-01 10:00:00"] },
  "TIMESTAMP nested in a LIST becomes an ISO string, not a raw epoch number"
);

// ---- a column with no DECIMAL/DATE/TIMESTAMP anywhere is untouched ----
assert.deepEqual(rows("SELECT ['a', 'b', 'c'] AS v")[0], { v: ["a", "b", "c"] }, "LIST<VARCHAR> is unaffected");
assert.deepEqual(rows("SELECT {'name': 'a', 'n': 5, 'ok': true} AS v")[0], { v: { name: "a", n: 5, ok: true } }, "a plain STRUCT is unaffected");
assert.deepEqual(rows("SELECT [[1, 2], [3, 4]] AS v")[0], { v: [[1, 2], [3, 4]] }, "LIST<LIST<INT>> is unaffected");
assert.deepEqual(rows("SELECT [1, 2, 3]::BIGINT[] AS v")[0], { v: [1, 2, 3] }, "LIST<BIGINT> still comes out as numbers, not bigints");

// ---- top-level columns: unchanged behaviour (the pre-#468 test case) ----
assert.deepEqual(
  rows("SELECT DATE '2026-01-02' AS d, TIMESTAMP '2026-01-02 03:04:05' AS ts, 12.50::DECIMAL(5,2) AS m, 3::BIGINT AS b")[0],
  { d: "2026-01-02", ts: "2026-01-02 03:04:05", m: 12.5, b: 3 },
  "top-level DATE/TIMESTAMP/DECIMAL/BIGINT columns still convert exactly as before"
);

console.log("rows_json_nested_test: ok");
process.exit(0);
