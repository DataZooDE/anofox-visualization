// Node test for #417 S2 (web/app.js validPathValueLiteral): the published
// site's own enforcement of the locked server's `path_vars` value rule
// (duckext/src/serve.rs valid_path_value / check_path_values) — a plain
// path segment only, so `getvariable('anofox_data') || '/' || getvariable(
// '<path_vars var>')`-style dashboard SQL can never leave the site's data/
// folder even though there is no server left to check requests.
//
// Pure function, no DOM: extracted the way nav_path_vars_test.mjs extracts
// pathVarValue/servedNavHref. The wiring this gates (runSqlInner dropping an
// invalid SET VARIABLE for a published site's path_vars name, with a status
// message) needs a real DuckDB-Wasm session; covered by scripts/site-e2e.mjs
// instead (a `../x` path value sent through the nav_picker/a URL seed must
// not change the dashboard's result).
import { readFileSync } from "fs";
import assert from "assert/strict";

const src = readFileSync(new URL("../../web/app.js", import.meta.url), "utf8");
const fn = (name) => {
  const m = src.match(new RegExp(`^function ${name}\\([\\s\\S]*?^}`, "m"));
  assert.ok(m, `function ${name} not found in app.js`);
  return m[0];
};
const validPathValueLiteral = new Function(`${fn("validPathValueLiteral")}\nreturn validPathValueLiteral;`)();

// NULL and plain non-negative integers: always fine (the server's own rule).
assert.equal(validPathValueLiteral("NULL"), true);
assert.equal(validPathValueLiteral("null"), true);
assert.equal(validPathValueLiteral("0"), true);
assert.equal(validPathValueLiteral("42"), true);
assert.equal(validPathValueLiteral("-1"), false); // a sign isn't a digit
assert.equal(validPathValueLiteral("3.5"), false);

// A quoted string: the unquoted text must be a plain name.
assert.equal(validPathValueLiteral("'east'"), true);
assert.equal(validPathValueLiteral("'north-west_2024.v1'"), true);
assert.equal(validPathValueLiteral("''"), false); // empty
assert.equal(validPathValueLiteral("'.'"), false);
assert.equal(validPathValueLiteral("'..'"), false);
assert.equal(validPathValueLiteral("'.hidden'"), false);
assert.equal(validPathValueLiteral("'../etc/passwd'"), false); // traversal — the exact attack #417 S2 closes
assert.equal(validPathValueLiteral("'a/b'"), false); // no '/' at all
assert.equal(validPathValueLiteral("'a b'"), false); // no spaces
assert.equal(validPathValueLiteral("'<script>'"), false);
assert.equal(validPathValueLiteral("'a'.repeat(200)"), false); // not even a literal
assert.equal(validPathValueLiteral("'" + "a".repeat(129) + "'"), false); // over the 128-char cap
assert.equal(validPathValueLiteral("'" + "a".repeat(128) + "'"), true); // exactly at the cap

// A `''`-doubled quote inside the literal unescapes to a plain `'`, which is
// itself not in [A-Za-z0-9_.-] — still refused.
assert.equal(validPathValueLiteral("'a''b'"), false);

// Not a recognised literal shape at all (a list, an unquoted bareword, a
// number expression) — refused, not merely ignored.
assert.equal(validPathValueLiteral("[1, 2]"), false);
assert.equal(validPathValueLiteral("bareword"), false);
assert.equal(validPathValueLiteral(""), false);

console.log("site_pathvalue_test: ok");
