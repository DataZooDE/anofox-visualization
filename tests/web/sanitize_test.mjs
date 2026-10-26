// Node regression test for the browser's HTML sinks (run by tests/web_js.rs).
// Extracts escapeHtml / safeUrl / renderMarkdown from web/app.js (a browser
// module that can't be imported headless) and checks they are inert.
import { readFileSync } from "fs";
import assert from "assert/strict";

const src = readFileSync(new URL("../../web/app.js", import.meta.url), "utf8");
const grab = (name) => {
  const m = src.match(new RegExp(`^function ${name}\\([\\s\\S]*?^}`, "m"));
  assert.ok(m, `function ${name} not found in app.js`);
  return m[0];
};
// Grab the const declarations themselves too (#544 review), not a
// hand-copied duplicate.
const grabConst = (name) => {
  const m = src.match(new RegExp(`^const ${name} = [^\\n]*;$`, "m"));
  assert.ok(m, `const ${name} not found in app.js`);
  return m[0];
};
// #514 PR B: assetUrl() now reads servedMode/published/window.__served —
// stub them to the plain no-server builder's values so a markdown image in
// this test (imageSrc -> assetUrl) behaves exactly as PR A tested it
// (assetUrl is the identity).
const { escapeHtml, safeUrl, renderMarkdown } = new Function(
  `let servedMode = false; const published = false; const window = {};\n` +
    `${grabConst("IMAGE_DATA_TYPES")}\n${grabConst("MAX_IMAGE_SRC_CHARS")}\n` +
    `${grab("escapeHtml")}\n${grab("safeUrl")}\n${grab("assetUrl")}\n${grab("imageSrc")}\n${grab("mathSpans")}\n${grab("renderMarkdown")}\nreturn { escapeHtml, safeUrl, renderMarkdown };`
)();

// escapeHtml is attribute-safe.
assert.equal(escapeHtml(`<a href="x" onclick='y'>&`), "&lt;a href=&quot;x&quot; onclick=&#39;y&#39;&gt;&amp;");

// safeUrl allows http(s)/mailto/relative only.
for (const ok of ["https://a.b/c?d=1", "http://x", "mailto:a@b.c", "/rel/path", "page.html#x", "?q=1"])
  assert.equal(safeUrl(ok), ok, ok);
for (const bad of [
  "javascript:alert(1)",
  "JaVaScRiPt:alert(1)",
  " javascript:alert(1)",
  "java\tscript:alert(1)",
  "java\nscript:alert(1)",
  "vbscript:msgbox(1)",
  "data:text/html,<script>alert(1)</script>",
  "file:///etc/passwd",
])
  assert.equal(safeUrl(bad), "", JSON.stringify(bad));
assert.equal(safeUrl("data:image/png;base64,AAAA", { image: true }), "data:image/png;base64,AAAA");
assert.equal(safeUrl("data:image/png;base64,AAAA"), "");

// Markdown output never carries live markup from the source.
const cases = [
  `[x](javascript:alert(1))`,
  `[x](JAVASCRIPT:alert(1))`,
  `[x](" onmouseover="alert(1))`,
  `[x](https://ok.example/" onmouseover="alert(1))`,
  `<img src=x onerror=alert(1)>`,
  `**<script>alert(1)</script>**`,
  "```\n</code><script>alert(1)</script>\n```",
  `# "quoted" 'title' <b>`,
  `> [a](data:text/html,<script>x</script>)`,
  `![a](javascript:alert(1))`,
  `![a](data:text/html,<script>x</script>)`,
];
for (const md of cases) {
  const html = renderMarkdown(md);
  assert.ok(!/<script/i.test(html), `script in ${html}`);
  assert.ok(!/<img/i.test(html), `img in ${html}`);
  assert.ok(!/\son[a-z]+\s*=\s*["']/i.test(html), `event handler in ${html}`);
  for (const m of html.matchAll(/href="([^"]*)"/g)) {
    assert.ok(/^(https?:|mailto:|[^:]*$)/i.test(m[1]), `bad href ${m[1]}`);
    assert.ok(!m[1].includes('"'), `quote in href ${m[1]}`);
  }
}
// A good link still renders.
assert.match(renderMarkdown("[docs](https://example.com/a?b=1&c=2)"), /<a href="https:\/\/example\.com\/a\?b=1&amp;c=2"/);

// ---- markdown images (#514) ----
// A rejected src renders the alt text, not a broken <img>.
assert.doesNotMatch(renderMarkdown("![a](javascript:alert(1))"), /<img/i);
assert.ok(renderMarkdown("![a](javascript:alert(1))").startsWith("<p>a"));

// `![x [a](b)](c)`: the image rule runs before the link rule, so the `[a](b)`
// inside what looks like alt text never becomes a live <a> — the plan's
// named attribute/HTML-injection case. Exactly one <img>, no <a> anywhere,
// and (critically) no "<a" landing inside an attribute value.
{
  const html = renderMarkdown("![x [a](https://b)](https://c/i.png)");
  assert.equal((html.match(/<img/g) || []).length, 1, html);
  assert.ok(!/<a\b/i.test(html), `a link leaked: ${html}`);
  for (const m of html.matchAll(/="([^"]*)"/g)) assert.ok(!m[1].includes("<"), `tag inside an attribute: ${html}`);
}

// Alt text is escaped (never raw markup in the attribute).
assert.match(renderMarkdown("![<b>](https://c/i.png)"), /alt="&lt;b&gt;"/);

// A good data: image renders with that src.
assert.match(
  renderMarkdown("![a](data:image/svg+xml;base64,AAA)"),
  /<img class="md-img" src="data:image\/svg\+xml;base64,AAA" alt="a" loading="lazy" decoding="async">/
);

// No alt text: alt="".
assert.match(renderMarkdown("![](https://c/x.png)"), /alt=""/);

// Text and a formula survive around an image.
{
  const html = renderMarkdown("before ![a](https://c/i.png) $x^2$ after");
  assert.ok(html.startsWith("<p>before <img"), html);
  assert.ok(html.includes('class="md-math"'), html);
  assert.ok(html.endsWith(" after</p>"), html);
}

// A $…$ formula inside an image's alt bracket resolves to its own escaped
// TeX source in the attribute, not a leaked math-token/index (review of #544).
assert.match(renderMarkdown("![a $x$](https://c/i.png)"), /alt="a x"/);

// A literal private-use token sequence in the input can't forge or
// duplicate an already-tokenized <img> (review of #544): the real image
// still renders exactly once, whatever a pasted forged-looking sequence
// does.
{
  const html = renderMarkdown("![a](https://c/i.png) 0");
  assert.equal((html.match(/<img/g) || []).length, 1, html);
}
console.log("web sanitize tests: ok");
