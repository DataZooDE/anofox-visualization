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
const { escapeHtml, safeUrl, renderMarkdown } = new Function(
  `${grab("escapeHtml")}\n${grab("safeUrl")}\n${grab("renderMarkdown")}\nreturn { escapeHtml, safeUrl, renderMarkdown };`
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
console.log("web sanitize tests: ok");
