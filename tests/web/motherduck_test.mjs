// Node test for #380: the ☁ MotherDuck connect button and its dialog are
// gone from the builder's top bar (web/index.html), along with the handler,
// the auto-connect-from-stored-token code and the "connect MotherDuck" hint
// (web/app.js). A previously stored token is cleared once on boot so it
// doesn't linger in a viewer's browser; the storage key itself is kept only
// for that cleanup.
import { readFileSync } from "fs";
import assert from "assert/strict";

const html = readFileSync(new URL("../../web/index.html", import.meta.url), "utf8");
const src = readFileSync(new URL("../../web/app.js", import.meta.url), "utf8");

// The button, its dialog and every element inside it are gone.
for (const id of ["md", "md-dialog", "md-token", "md-db", "md-status", "md-connect", "md-cancel", "md-disconnect"])
  assert.ok(!html.includes(`id="${id}"`), `index.html still has id="${id}"`);
assert.ok(!/MotherDuck/.test(html), "MotherDuck text lingers in index.html");
assert.ok(!/#md-connect\{|#md\.connected\{|\.md-status\b/.test(html), "MotherDuck-only CSS lingers in index.html");

// The shared dialog chrome (.md-dialog/.md-card/.md-note/.md-actions) stays:
// dpDialog() in app.js still builds on it.
for (const cls of [".md-dialog{", ".md-card{", ".md-note{", ".md-actions{"]) assert.ok(html.includes(cls), `${cls} missing — dpDialog() needs it`);

// The connect machinery, its wiring and the empty-catalog hint are gone from app.js.
for (const fn of ["mdOpen", "mdConnect", "mdDoConnect", "mdDisconnect", "mdMark", "mdSaved"])
  assert.ok(!new RegExp(`function ${fn}\\b`).test(src), `${fn} is still defined in app.js`);
assert.ok(!/\$\("md"\)/.test(src), 'a $("md") lookup remains in app.js');
assert.ok(!/\$\("md-/.test(src), 'a $("md-...") lookup remains in app.js');
assert.ok(!/connect MotherDuck/.test(src), 'the "connect MotherDuck" catalog hint remains in app.js');
assert.ok(!/"md"/.test(src.match(/^const PREVIEW_TOOLBAR_IDS = .*$/m)?.[0] ?? ""), '"md" still listed in PREVIEW_TOOLBAR_IDS');

// The old storage key is kept only so a stale token gets cleared once on boot.
const keyDecl = src.match(/^const MD_KEY = "dp_md";$/m);
assert.ok(keyDecl, "MD_KEY constant missing from app.js");
const cleanup = src.match(/try \{\s*\n\s*localStorage\.removeItem\(MD_KEY\);\s*\n\s*\} catch \(_\) \{\}/);
assert.ok(cleanup, "boot() no longer clears a stored MotherDuck token");

// Exercise that exact snippet against a stand-in localStorage holding a
// stale token under the old key.
const m = new Map([["dp_md", JSON.stringify({ token: "stale-token", db: "x" })]]);
const localStorage = {
  getItem: (k) => (m.has(k) ? m.get(k) : null),
  setItem: (k, v) => m.set(k, String(v)),
  removeItem: (k) => m.delete(k),
};
new Function("localStorage", "MD_KEY", cleanup[0])(localStorage, "dp_md");
assert.ok(!m.has("dp_md"), "the stale MotherDuck token was not cleared");

console.log("motherduck_test: ok");
