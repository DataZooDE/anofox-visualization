// Node test for #759: GFM tables in renderMarkdown (::MARKDOWN and ::INFO),
// the `breaks` option ::INFO uses, and that nothing from data becomes HTML.
import { readFileSync } from "fs";
import assert from "assert/strict";

const src = readFileSync(new URL("../../web/app.js", import.meta.url), "utf8");
const grab = (name) => {
  const m = src.match(new RegExp(`^function ${name}\\([\\s\\S]*?^}`, "m"));
  assert.ok(m, `function ${name} not found in app.js`);
  return m[0];
};
const { renderMarkdown } = new Function(
  ["escapeHtml", "safeUrl", "mathSpans", "renderMarkdown"].map(grab).join("\n") + "\nreturn { renderMarkdown };"
)();

// A table: header, alignment, inline Markdown in cells, an escaped pipe.
const t = renderMarkdown(
  "Columns:\n\n| Column | What it means | n |\n|:---|---|--:|\n| **Rank** | position by `places` | 1 |\n| Trend % | a \\| b | 2 |\n\nAfter."
);
assert.ok(t.includes('<table class="md-table"><thead><tr><th style="text-align:left">Column</th><th>What it means</th><th style="text-align:right">n</th></tr></thead>'), t);
assert.ok(t.includes("<td style=\"text-align:left\"><strong>Rank</strong></td><td>position by <code>places</code></td>"), t);
assert.ok(t.includes("<td>a | b</td>"), t);
assert.ok(t.includes("<p>Columns:</p>") && t.includes("<p>After.</p>"), t);
// rows: missing cells are empty, extra ones dropped
const r = renderMarkdown("a | b\n--|--\n1 |\n2 | 3 | 4\nnot a row");
assert.ok(r.includes("<tr><td>1</td><td></td></tr><tr><td>2</td><td>3</td></tr></tbody></table><p>not a row</p>"), r);
// a table right after a paragraph line (no blank line) still starts a table
assert.ok(renderMarkdown("Intro\n| a | b |\n|---|---|\n| 1 | 2 |").includes("<p>Intro</p><table"));
// no table: a rule, or pipes without a separator row
assert.ok(!renderMarkdown("a | b\nc | d").includes("<table"));
assert.ok(renderMarkdown("text\n\n---").includes("<hr>"));
// nothing from data becomes HTML, in a header or a cell
const x = renderMarkdown('| <b>h</b> | x |\n|---|---|\n| <script>alert(1)</script> | <img src=x onerror=alert(1)> |');
assert.ok(!/<script|<img|<b>/.test(x), x);
assert.ok(x.includes("&lt;script&gt;alert(1)&lt;/script&gt;"), x);
// a link in a cell: only a safe URL
const l = renderMarkdown("| a |\n|---|\n| [x](javascript:alert(1)) [y](https://example.com) |");
assert.ok(!l.includes("javascript:") && l.includes('<a href="https://example.com"'), l);

// breaks: a newline is a line break (::INFO), a space otherwise (::MARKDOWN)
assert.equal(renderMarkdown("Rank: position\nPlaces: count"), "<p>Rank: position Places: count</p>");
assert.equal(renderMarkdown("Rank: position\nPlaces: count", { breaks: true }), "<p>Rank: position<br>Places: count</p>");
assert.equal(renderMarkdown("<i>a</i>\nb", { breaks: true }), "<p>&lt;i&gt;a&lt;/i&gt;<br>b</p>");
// a list in an ::INFO
const li = renderMarkdown("Columns:\n- **Rank**: position\n- *Trend*: change", { breaks: true });
assert.ok(li.includes("<ul><li><strong>Rank</strong>: position</li><li><em>Trend</em>: change</li></ul>"), li);

console.log("md_table_test: ok");
