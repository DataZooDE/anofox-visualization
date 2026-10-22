// Node test for #368 (ggplot-rs 0.23.0 bump): a vertical legend's truncated
// label now carries a <title> (the untruncated text — ggplot-rs #159). It is
// a <text> element, and attachHover collects marks only from shape elements
// (web/app.js, `const marks = [...document.querySelectorAll(...)]`), so a
// legend label can never become a hover/select/drill mark. This test keeps
// it that way. The filter must NOT require `gg-mark`: the map writer
// (src/map) and the ::DIAGRAM writer (src/diagram.rs) draw their own marks,
// with a <title> but without ggplot-rs's `gg-mark` class.
import { readFileSync } from "fs";
import assert from "assert/strict";

const src = readFileSync(new URL("../../web/app.js", import.meta.url), "utf8");
const m = src.match(/const marks = \[\s*\.\.\.document\.querySelectorAll\(\s*"([^"]+)"\s*\),?\s*\]\.filter\(([^;]*?)\);/);
assert.ok(m, "attachHover's `marks` collection not found in web/app.js");
const [, selector, filter] = m;

const tags = selector.split(",").map((s) => s.trim().split(/\s+/).pop().replace(/[[:.].*$/, ""));
for (const t of tags)
  assert.ok(["rect", "circle", "polygon", "polyline", "path"].includes(t), `marks selector picks <${t}>`);
assert.ok(!tags.includes("text") && !tags.includes("tspan"), "legend/axis text must never be a mark");
assert.ok(/svg\[data-map\][^,]*path/.test(selector), "map paths stay hoverable");
assert.ok(!/gg-mark/.test(filter), "own map/diagram marks have no gg-mark class; don't require it");

console.log("legend_hover_test.mjs: ok");
