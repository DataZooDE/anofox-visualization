// #1357: a chrome directive (::LOCALE, ::THEME, …) that reads a variable is
// evaluated again once the viewer's values are final — a URL / setVars seed
// that no input claims is only applied after the pre-pass. chromeReadingVars
// picks exactly those statements.
import { readFileSync } from "fs";
import assert from "assert/strict";

const src = readFileSync(new URL("../../web/app.js", import.meta.url), "utf8");
const grab = (name) => {
  const m = src.match(new RegExp(`^function ${name}\\([\\s\\S]*?^}`, "m"));
  assert.ok(m, `function ${name} not found in app.js`);
  return m[0];
};
const line = (re) => {
  const m = src.match(re);
  assert.ok(m, `${re} not found in app.js`);
  return m[0];
};
const { chromeReadingVars } = new Function(
  [
    line(/^const role = .*$/m),
    line(/^let CHROME = .*$/m),
    line(/^const chromeRole = .*$/m),
    grab("sqlCode"),
    grab("chromeReadingVars"),
    "return { chromeReadingVars };",
  ].join("\n")
)();

const st = (sql, roles, setup = false) => ({ sql, roles, setup });
const stmts = [
  st("SELECT coalesce(nullif(getvariable('locale'), ''), 'de-DE')::LOCALE", [[0, "LOCALE"]]),
  st("SELECT 'fr-FR'::LOCALE", [[0, "LOCALE"]]),
  st("SELECT 1234567.5::METRIC", [[0, "METRIC"]]),
  st("SELECT GETVARIABLE ('brand') ::THEME", [[0, "THEME"]]),
  st("SELECT 'getvariable(x)'::DASHBOARD_TITLE", [[0, "DASHBOARD_TITLE"]]),
  st("SET VARIABLE locale = getvariable('x')", [], true),
  st("SELECT getvariable('region') AS r ::METRIC", [[0, "METRIC"]]),
];
// only the chrome directives whose code (not a string literal) reads a variable
assert.deepEqual(chromeReadingVars(stmts), [0, 3]);
assert.deepEqual(chromeReadingVars([]), []);
console.log("chrome_vars_1357 ok");
