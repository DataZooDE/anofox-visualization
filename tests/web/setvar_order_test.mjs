// Node test for #416 (web/app.js's setVarsToReplay, used by runOnce's
// render loop): a dashboard's own `SET VARIABLE` that reads an input via
// getvariable() must see the viewer's CURRENT value even when it is
// declared BEFORE that input in the script — the same order the locked
// server uses (it sets every viewer value first, then replays the
// dashboard's own SET VARIABLEs; see duckext/src/serve.rs's
// request_sets_named + run_panel, and the comment near `servedVars` in
// web/app.js). setVarsToReplay is the pure decision half of that fix: given
// the statements and this run's known viewer variable names, which of the
// dashboard's own SET VARIABLE setup statements need a corrective re-run,
// in script order.
//
// No jsdom: setVarsToReplay, sqlCode and varsOf are pure (array/string in,
// array/string out), extracted from app.js like served_nav_test.mjs does.
import { readFileSync } from "fs";
import assert from "assert/strict";

const src = readFileSync(new URL("../../web/app.js", import.meta.url), "utf8");
const fn = (name) => {
  const m = src.match(new RegExp(`^function ${name}\\([\\s\\S]*?^}`, "m"));
  assert.ok(m, `function ${name} not found in app.js`);
  return m[0];
};
const constLine = (name) => {
  const m = src.match(new RegExp(`^const ${name} = .*$`, "m"));
  assert.ok(m, `const ${name} not found in app.js`);
  return m[0];
};
const { setVarsToReplay } = new Function(
  `${constLine("varsOf")}\n${constLine("SETVAR_RE")}\n${fn("sqlCode")}\n${fn("setVarsToReplay")}\nreturn { setVarsToReplay };`
)();

// A plan index (`setup`/`sql`) is all setVarsToReplay reads.
const stmt = (sql, setup = true) => ({ setup, sql });

// 1. The exact #416 repro: the derived SET VARIABLE declared BEFORE the
// dropdown it reads via getvariable('extension').
{
  const stmts = [
    stmt("SET VARIABLE sel = coalesce(nullif(getvariable('extension'), ''), 'default')"),
    stmt("SELECT 'alpha' AS extension", false), // the dropdown's own SELECT: not setup
    stmt("SELECT 'Downloads of ' || getvariable('sel') AS t", false),
  ];
  const out = setVarsToReplay(stmts, new Set(["extension"]));
  assert.equal(out.length, 1, "the derived SET VARIABLE is replayed");
  assert.equal(out[0], stmts[0]);
}

// 2. Declared AFTER the input: the pre-pass loop already got it right, but
// replaying it again is a pure function of the (unchanged) viewer value, so
// it is harmless — setVarsToReplay doesn't need to know the statement's own
// position to decide this, only that it reads a known viewer variable.
{
  const stmts = [
    stmt("SELECT 'alpha' AS extension", false),
    stmt("SET VARIABLE sel = coalesce(nullif(getvariable('extension'), ''), 'default')"),
  ];
  const out = setVarsToReplay(stmts, new Set(["extension"]));
  assert.equal(out.length, 1);
  assert.equal(out[0], stmts[1]);
}

// 3. A statement that reads no viewer variable — a running total, reading
// only its own prior value — is never replayed (it would double-apply).
{
  const stmts = [stmt("SET VARIABLE counter = coalesce(getvariable('counter'), 0) + 1")];
  assert.deepEqual(setVarsToReplay(stmts, new Set(["extension"])), []);
  // Even once 'counter' is (wrongly) offered as a "viewer" name, it must
  // still never replay a statement whose only dependency is itself unless
  // something else actually changed it — this guards the *caller*'s
  // contract (dpViewerVars only ever holds names a real viewer input set),
  // not a case setVarsToReplay must detect on its own.
}

// 4. A chain: sel2 reads sel, sel reads the (later-declared) input. One
// forward pass resolves both, in script order.
{
  const stmts = [
    stmt("SET VARIABLE sel = coalesce(nullif(getvariable('extension'), ''), 'default')"),
    stmt("SET VARIABLE sel2 = upper(getvariable('sel'))"),
    stmt("SELECT 'alpha' AS extension", false),
  ];
  const out = setVarsToReplay(stmts, new Set(["extension"]));
  assert.equal(out.length, 2, "both links of the chain are replayed");
  assert.equal(out[0], stmts[0]);
  assert.equal(out[1], stmts[1]);
}

// 5. The reverse chain order (sel2 declared before sel) is not resolved in
// one forward pass — same limitation as the server's single stmts[..panel]
// replay, so this just pins the known boundary rather than claiming it works.
{
  const stmts = [
    stmt("SET VARIABLE sel2 = upper(getvariable('sel'))"),
    stmt("SET VARIABLE sel = coalesce(nullif(getvariable('extension'), ''), 'default')"),
    stmt("SELECT 'alpha' AS extension", false),
  ];
  const out = setVarsToReplay(stmts, new Set(["extension"]));
  assert.equal(out.length, 1, "only the directly-dependent link is replayed");
  assert.equal(out[0], stmts[1]);
}

// 6. Case-insensitive names, like DuckDB's own variables.
{
  const stmts = [stmt("SET VARIABLE Sel = coalesce(getvariable('Extension'), 'default')")];
  assert.equal(setVarsToReplay(stmts, new Set(["extension"])).length, 1);
}

console.log("setvar_order_test: ok");
