// Editing the dashboard header in the builder (#60): setChromeSql writes the
// `'…'::DASHBOARD_TITLE` / `'…'::LOGO` statement into the SQL text (update,
// insert in the "Look" section, remove), chromeUrlOk applies the ::LOGO URL
// rules of src/theme.rs safe_url (lint chrome/bad-url).
import { readFileSync, readdirSync } from "fs";
import assert from "assert/strict";

const src = readFileSync(new URL("../../web/app.js", import.meta.url), "utf8");
const grab = (name) => {
  const m = src.match(new RegExp(`^function ${name}\\([\\s\\S]*?^}`, "m"));
  assert.ok(m, `function ${name} not found in app.js`);
  return m[0];
};
const { setChromeSql, chromeUrlOk } = new Function(
  `${grab("setChromeSql")}\n${grab("chromeUrlOk")}\nreturn { setChromeSql, chromeUrlOk };`
)();

// Update in place: the literal changes, quotes are doubled, the rest is untouched.
const look = [
  "-- A dashboard.",
  "",
  "-- ── Look ──────────",
  "SELECT 'ACME'::BRAND;",
  "SELECT 'Sales cockpit'::DASHBOARD_TITLE;",
  "",
  "SELECT 1 AS n;",
].join("\n");
assert.equal(
  setChromeSql(look, "DASHBOARD_TITLE", "Q3's numbers"),
  look.replace("'Sales cockpit'", "'Q3''s numbers'")
);
// An empty name removes the statement and its line.
assert.equal(setChromeSql(look, "DASHBOARD_TITLE", ""), look.replace("SELECT 'Sales cockpit'::DASHBOARD_TITLE;\n", ""));
// No ::LOGO yet: added at the end of the Look section.
assert.equal(
  setChromeSql(look, "LOGO", "logo.svg"),
  look.replace("::DASHBOARD_TITLE;\n", "::DASHBOARD_TITLE;\nSELECT 'logo.svg'::LOGO;\n")
);
// Removing a statement that isn't there changes nothing.
assert.equal(setChromeSql(look, "LOGO", ""), look);

// The role inside a string (a Markdown note about ::DASHBOARD_TITLE) or a
// comment isn't a statement; a data: URL's ';' doesn't end the statement.
const tricky = [
  "-- SELECT 'old'::DASHBOARD_TITLE;",
  "SELECT 'Set `::DASHBOARD_TITLE`; then ::LOGO.'::MARKDOWN;",
  "SELECT 'data:image/svg+xml;base64,AAA='::LOGO;",
  "select 'lower'  ::  dashboard_title",
].join("\n");
const t2 = setChromeSql(tricky, "DASHBOARD_TITLE", "New");
assert.equal(t2, tricky.replace("select 'lower'  ::  dashboard_title", "SELECT 'New'::DASHBOARD_TITLE;"));
assert.equal(
  setChromeSql(tricky, "LOGO", "https://x.test/l.png"),
  tricky.replace("'data:image/svg+xml;base64,AAA='", "'https://x.test/l.png'")
);
assert.equal(setChromeSql(tricky, "LOGO", ""), tricky.replace("SELECT 'data:image/svg+xml;base64,AAA='::LOGO;\n", ""));
// A computed title is replaced by the literal the author typed.
assert.equal(
  setChromeSql("SELECT name::DASHBOARD_TITLE FROM shop;\nSELECT 1;", "DASHBOARD_TITLE", "Shop"),
  "SELECT 'Shop'::DASHBOARD_TITLE;\nSELECT 1;"
);

// No Look section: after the leading comments, blank-line separated.
assert.equal(
  setChromeSql("-- About.\n\nSELECT 1 AS n;\n", "DASHBOARD_TITLE", "T"),
  "-- About.\n\nSELECT 'T'::DASHBOARD_TITLE;\n\nSELECT 1 AS n;\n"
);
assert.equal(setChromeSql("SELECT 1 AS n;", "LOGO", "l.png"), "SELECT 'l.png'::LOGO;\n\nSELECT 1 AS n;");
assert.equal(setChromeSql("", "DASHBOARD_TITLE", "T"), "SELECT 'T'::DASHBOARD_TITLE;\n");

// Every bundled example keeps its other statements: set, then remove again.
for (const f of readdirSync(new URL("../../web/examples/", import.meta.url)).filter((f) => f.endsWith(".sql"))) {
  const sql = readFileSync(new URL(`../../web/examples/${f}`, import.meta.url), "utf8");
  const set = setChromeSql(sql, "DASHBOARD_TITLE", "Renamed");
  const others = (t) => t.split("\n").filter((l) => l.trim() && !/^SELECT '.*'::(DASHBOARD_TITLE|LOGO);$/.test(l));
  assert.deepEqual(others(set), others(sql), f);
  assert.deepEqual(others(setChromeSql(sql, "LOGO", "l.png")), others(sql), f);
  assert.equal((set.match(/'Renamed'::DASHBOARD_TITLE;/g) || []).length, 1, f);
  assert.equal(setChromeSql(setChromeSql(set, "DASHBOARD_TITLE", "Back"), "DASHBOARD_TITLE", "Renamed"), set, f);
  const gone = setChromeSql(set, "DASHBOARD_TITLE", "");
  assert.ok(!/'Renamed'::DASHBOARD_TITLE/.test(gone), f);
  assert.equal(gone.length, set.length - "SELECT 'Renamed'::DASHBOARD_TITLE;\n".length, f);
}

// The ::LOGO URL rules.
for (const ok of ["logo.svg", "/img/l.png", "https://x.test/l.png", "http://x.test/l.png", "data:image/png;base64,AAAA", "a/b:c.png"])
  assert.ok(chromeUrlOk(ok), ok);
for (const bad of ["", "javascript:alert(1)", "data:text/html,<b>", "ftp://x/l.png", "l'.png", "url(x)", "a\\b", "a\u0007b", "x".repeat(512001)])
  assert.ok(!chromeUrlOk(bad), bad.slice(0, 40));

console.log("chrome_edit_test ok");
