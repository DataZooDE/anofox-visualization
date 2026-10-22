// Node test for #368 (ggplot-rs 0.23.0 bump): a vertical legend's truncated
// label now carries a <title> (the untruncated text, for a hover tooltip —
// ggplot-rs #159) but is drawn as plain legend text, not a `gg-mark`
// (`class`/`data-geom` are only set while an actual geom layer is drawing,
// never for legend swatches/labels). attachHover's mark-collection filter
// (web/app.js, ~line 13489) must require `gg-mark` in addition to a
// non-empty <title>, or a truncated legend label would be treated as a
// hoverable/selectable/drillable data point.
import { readFileSync } from "fs";
import assert from "assert/strict";

const src = readFileSync(new URL("../../web/app.js", import.meta.url), "utf8");

// Extract the exact filter predicate attachHover uses to collect `marks`,
// the same regex-grab style as svg_attrs_test.mjs / interaction_test.mjs use
// for functions, applied here to a specific statement since the predicate
// lives inline inside attachHover (a DOM-heavy function that can't run
// headless as a whole).
const m = src.match(/const marks = \[[\s\S]*?\]\.filter\(\s*([\s\S]*?)\n\s*\);/);
assert.ok(m, "attachHover's `marks` filter not found in web/app.js");
const predicateSrc = m[1];

// The guard must name `gg-mark` explicitly — not rely on tag name alone
// (a legend label is drawn as a <text>, which the selector list above
// already excludes today, but the filter itself must still be robust on
// its own, per #368/#159).
assert.ok(/gg-mark/.test(predicateSrc), "marks filter must require the gg-mark class");

const isMark = new Function("el", `return (${predicateSrc})(el);`);

const el = ({ ggMark = false, title = null } = {}) => ({
  classList: { contains: (c) => (c === "gg-mark" ? ggMark : false) },
  querySelector: (sel) => (sel === "title" && title != null ? { textContent: title } : null),
});

// A real data mark (e.g. a bar/point): gg-mark class + a non-empty title.
assert.ok(isMark(el({ ggMark: true, title: "web: 22" })), "a real gg-mark with a title must stay a mark");

// A truncated legend label (#159): a non-empty <title> (the untruncated
// label) but no gg-mark class (legend text, not a geom layer's mark) — must
// never be collected as a hover/select/drill mark.
assert.ok(
  !isMark(el({ ggMark: false, title: "A rather long series name that got truncated" })),
  "a legend label's <title> must not make it a mark"
);

// gg-mark with no title at all (e.g. a decorative mark, #161): still not
// collected — titles drive the hover tooltip text.
assert.ok(!isMark(el({ ggMark: true, title: null })), "a gg-mark with no title must not be collected");

// Neither gg-mark nor a title: never a mark.
assert.ok(!isMark(el({ ggMark: false, title: null })), "a plain element is never a mark");

// A title that's present but blank/whitespace-only must not count either
// (matches the existing `.textContent.trim()` check).
assert.ok(!isMark(el({ ggMark: true, title: "   " })), "a gg-mark with a blank title must not be collected");

console.log("legend_hover_test.mjs: ok");
