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
const { imageSrc } = new Function(
  `${grabConst("IMAGE_DATA_TYPES")}\n${grabConst("MAX_IMAGE_SRC_CHARS")}\n` +
    `${grab("assetUrl")}\n${grab("imageSrc")}\nreturn { imageSrc };`
)();

const fx = JSON.parse(readFileSync(new URL("../fixtures/image_src_vectors.json", import.meta.url), "utf8"));
assert.ok(fx.vectors.length > 0);
for (const { src, kind } of fx.vectors) {
  const got = imageSrc(src);
  const gotKind = !got ? "bad" : src.trim().toLowerCase().startsWith("data:") ? "data" : /^https:/i.test(src) ? "https" : "relative";
  assert.equal(gotKind, kind, `imageSrc(${JSON.stringify(src)}) -> ${JSON.stringify(got)}`);
  // A non-rejected src round-trips unchanged (PR A: assetUrl() is the
  // identity; data:/https: are never rewritten).
  if (got) assert.equal(got, src, `imageSrc(${JSON.stringify(src)}) changed an accepted src`);
}

// A couple of direct checks beyond the shared vectors (JS-specific plumbing).
assert.equal(imageSrc(""), "");
assert.equal(imageSrc(null), "");
assert.equal(imageSrc(undefined), "");
assert.equal(imageSrc("data:image/png;base64,AAAA"), "data:image/png;base64,AAAA");
assert.equal(imageSrc("A".repeat(2_000_001)), "", "over the length cap");

console.log("image_src_test: ok");
