// captionColon (#539): every input control's caption/label gets a trailing
// ":" appended — makeControl, makeSearchableDropdown, mkButtons,
// makePeriodControl, the ::MULTISELECT caption and the served nav picker's
// label all go through it (web/app.js) — except when the caption already
// ends with its own terminal punctuation (? ! : … .) once trailing
// whitespace is trimmed, so a question like "Which check sees concept
// drift?" doesn't become "Which check sees concept drift?:" (seen in
// #537/#515's onboarding screenshots). Standalone (DOM-free), so it's
// tested here without a DOM — see dropdown_search_test.mjs for the same
// extraction convention.
import { readFileSync } from "fs";
import assert from "assert/strict";

const src = readFileSync(new URL("../../web/app.js", import.meta.url), "utf8");
const grab = (name) => {
  const m = src.match(new RegExp(`^function ${name}\\([\\s\\S]*?^}`, "m"));
  assert.ok(m, `function ${name} not found in app.js`);
  return m[0];
};
const { captionColon } = new Function(`${grab("captionColon")}\nreturn { captionColon };`)();

// Plain caption: gets a colon, as before.
assert.equal(captionColon("Your answer"), "Your answer:");
assert.equal(captionColon("See it as a"), "See it as a:");
assert.equal(captionColon("region"), "region:"); // a varname fallback

// Already ends with its own terminal punctuation: no colon added.
assert.equal(captionColon("Which check sees concept drift?"), "Which check sees concept drift?");
assert.equal(captionColon("Great job!"), "Great job!");
assert.equal(captionColon("Pick one:"), "Pick one:"); // already a colon — not doubled
assert.equal(captionColon("Loading…"), "Loading…");
assert.equal(captionColon("Done."), "Done.");

// Trailing whitespace is trimmed before the check and not kept in the result.
assert.equal(captionColon("Which check sees concept drift?  "), "Which check sees concept drift?");
assert.equal(captionColon("Your answer   "), "Your answer:");

// Falsy/empty input: no dangling bare colon.
assert.equal(captionColon(""), "");
assert.equal(captionColon(null), "");
assert.equal(captionColon(undefined), "");
assert.equal(captionColon("   "), "");

console.log("ok   caption_colon_test.mjs");
