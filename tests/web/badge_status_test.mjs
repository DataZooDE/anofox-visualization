// badgeHtml (::BADGE table pills, web/app.js): classifies a cell's text into
// good/warn/bad/neutral by keyword — the same classification
// src/diagram.rs's badge_status gives a diagram node/edge (its own doc
// comment cross-references this function by name). #553: "late" moves from
// "bad" (sharing failed's red) to "warn" (amber, alongside "delayed") in
// both places, so a "late" status reads the same colour everywhere it
// appears — a diagram node, an edge, or a table pill. Standalone (DOM-free),
// extracted the same way dropdown_search_test.mjs pulls functions out of
// app.js to test them without a DOM.
import { readFileSync } from "fs";
import assert from "assert/strict";

const src = readFileSync(new URL("../../web/app.js", import.meta.url), "utf8");
const grab = (name) => {
  const m = src.match(new RegExp(`^function ${name}\\([\\s\\S]*?^}`, "m"));
  assert.ok(m, `function ${name} not found in app.js`);
  return m[0];
};
const escapeHtml = (s) => String(s).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);
const { badgeHtml } = new Function("escapeHtml", `${grab("badgeHtml")}\nreturn { badgeHtml };`)(escapeHtml);

const cls = (text) => {
  const m = badgeHtml(text).match(/badge-(\w+)/);
  return m && m[1];
};

// #553: "late" is its own warning, not sharing "failed"'s red.
assert.equal(cls("late"), "warn");
assert.equal(cls("running late"), "warn");
assert.equal(cls("late, but earlier"), "warn");
assert.equal(cls("delayed"), "warn"); // same bucket as "late" — both timing problems
assert.equal(cls("failed"), "bad");
assert.notEqual(cls("late"), cls("failed"));

// A handful of the other words, unaffected by the move.
assert.equal(cls("OK"), "good");
assert.equal(cls("at-risk"), "warn");
assert.equal(cls("alarm"), "bad");
assert.equal(cls("some other text"), "neutral");

console.log("badge_status_test: ok");
