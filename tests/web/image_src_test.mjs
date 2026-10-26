// Parity guard (#514): the browser's imageSrc() must classify every
// ::IMAGE / ::IMAGE_DARK / markdown-image `src` exactly like the Rust side
// (src/image.rs::src_ok, tests/image.rs), via the shared vectors in
// tests/fixtures/image_src_vectors.json.
import { readFileSync } from "fs";
import assert from "assert/strict";

const src = readFileSync(new URL("../../web/app.js", import.meta.url), "utf8");
const grab = (name) => {
  const m = src.match(new RegExp(`^function ${name}\\([\\s\\S]*?^}`, "m"));
  assert.ok(m, `function ${name} not found in app.js`);
  return m[0];
};
// Grab the const declarations themselves too (#544 review), not a
// hand-copied duplicate — so a drift in the allowed type list or the
// length cap fails this test instead of silently going unparitied.
const grabConst = (name) => {
  const m = src.match(new RegExp(`^const ${name} = [^\\n]*;$`, "m"));
  assert.ok(m, `const ${name} not found in app.js`);
  return m[0];
};
// #514 PR B: assetUrl() now reads `servedMode`/`published`/`window.__served`/
// `window.__anofoxSite` (app.js's real top-level state), not just `p`. For
// this parity test — which is about imageSrc()'s src_ok-equivalent
// classification, not assetUrl()'s URL rewriting — stub them to the plain
// no-server browser builder's values (assetUrl() then stays the identity,
// exactly as PR A tested it); the served/published rewriting gets its own
// checks below.
const plainGlobals = `let servedMode = false; const published = false; const window = {};\n`;
const { imageSrc } = new Function(
  plainGlobals +
    `${grabConst("IMAGE_DATA_TYPES")}\n${grabConst("MAX_IMAGE_SRC_CHARS")}\n` +
    `${grab("assetUrl")}\n${grab("imageSrc")}\nreturn { imageSrc };`
)();

const fx = JSON.parse(readFileSync(new URL("../fixtures/image_src_vectors.json", import.meta.url), "utf8"));
assert.ok(fx.vectors.length > 0);
for (const { src, kind } of fx.vectors) {
  const got = imageSrc(src);
  const gotKind = !got ? "bad" : src.trim().toLowerCase().startsWith("data:") ? "data" : /^https:/i.test(src) ? "https" : "relative";
  assert.equal(gotKind, kind, `imageSrc(${JSON.stringify(src)}) -> ${JSON.stringify(got)}`);
  // A non-rejected src round-trips unchanged in the plain (no-server)
  // builder (PR A: assetUrl() is the identity there; data:/https: are
  // never rewritten in any mode).
  if (got) assert.equal(got, src, `imageSrc(${JSON.stringify(src)}) changed an accepted src`);
}

// A couple of direct checks beyond the shared vectors (JS-specific plumbing).
assert.equal(imageSrc(""), "");
assert.equal(imageSrc(null), "");
assert.equal(imageSrc(undefined), "");
assert.equal(imageSrc("data:image/png;base64,AAAA"), "data:image/png;base64,AAAA");
assert.equal(imageSrc("A".repeat(2_000_001)), "", "over the length cap");

// #514 PR B: assetUrl() itself, in served and published mode (plan-514
// decision 5) — a relative path under `assets/` is rewritten; one that
// isn't gives "" (the alt box), and a locked server's own URL encodes each
// path segment.
{
  const { assetUrl } = new Function(
    `let servedMode = true; const published = false;\n` +
      `const window = { __served: { id: "sales q1" } };\n` +
      `${grab("assetUrl")}\nreturn { assetUrl };`
  )();
  assert.equal(assetUrl("assets/concept.svg"), "/d/sales%20q1/assets/concept.svg");
  assert.equal(assetUrl("assets/sub/b.png"), "/d/sales%20q1/assets/sub/b.png");
  assert.equal(assetUrl("other/a.svg"), "", "not under assets/: the alt box, not a request");
}
{
  const { assetUrl } = new Function(
    `let servedMode = false; const published = true;\n` +
      `const window = { __anofoxSite: { base: new URL("https://example.com/site/") } };\n` +
      `${grab("assetUrl")}\nreturn { assetUrl };`
  )();
  assert.equal(assetUrl("assets/concept.svg"), "https://example.com/site/assets/concept.svg");
  assert.equal(assetUrl("other/a.svg"), "");
}
{
  // Neither served nor published (the plain browser builder, PR A):
  // unchanged.
  const { assetUrl } = new Function(
    `let servedMode = false; const published = false; const window = {};\n` +
      `${grab("assetUrl")}\nreturn { assetUrl };`
  )();
  assert.equal(assetUrl("assets/concept.svg"), "assets/concept.svg");
  assert.equal(assetUrl("other/a.svg"), "other/a.svg");
}

console.log("image_src_test: ok");
