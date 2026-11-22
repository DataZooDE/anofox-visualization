// Parity guard (#1345): the browser's renderMarkdown() and the static HTML
// export's Rust port (src/markdown_html.rs) give the same HTML for every
// case in tests/fixtures/markdown_html_vectors.json — image sources as the
// plain no-server builder resolves them.
import { readFileSync } from "fs";
import assert from "assert/strict";

const src = readFileSync(new URL("../../web/app.js", import.meta.url), "utf8");
const grab = (name) => {
  const m = src.match(new RegExp(`^function ${name}\\([\\s\\S]*?^}`, "m"));
  assert.ok(m, `function ${name} not found in app.js`);
  return m[0];
};
const grabConst = (name) => {
  const m = src.match(new RegExp(`^const ${name} = [^\\n]*;$`, "m"));
  assert.ok(m, `const ${name} not found in app.js`);
  return m[0];
};
const { renderMarkdown } = new Function(
  `let servedMode = false; const published = false; const window = {};\n` +
    `${grabConst("IMAGE_DATA_TYPES")}\n${grabConst("MAX_IMAGE_SRC_CHARS")}\n` +
    ["escapeHtml", "safeUrl", "mathSpans", "assetUrl", "imageSrc", "renderMarkdown"].map(grab).join("\n") +
    "\nreturn { renderMarkdown };"
)();

const fx = JSON.parse(readFileSync(new URL("../fixtures/markdown_html_vectors.json", import.meta.url), "utf8"));
assert.ok(fx.vectors.length > 20);
for (const { md, html } of fx.vectors) assert.equal(renderMarkdown(md), html, `markdown: ${JSON.stringify(md)}`);
console.log(`markdown_html: ${fx.vectors.length} vectors match`);
