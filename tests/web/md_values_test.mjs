// Node test for formatted values in Markdown (issue #38): extracts
// markdownWithValues (+ the renderer / sanitiser it builds on) from web/app.js
// and checks placeholders fill from the row, stay escaped and never reach a URL.
import { readFileSync } from "fs";
import assert from "assert/strict";

const src = readFileSync(new URL("../../web/app.js", import.meta.url), "utf8");
const grab = (name) => {
  const m = src.match(new RegExp(`^function ${name}\\([\\s\\S]*?^}`, "m"));
  assert.ok(m, `function ${name} not found in app.js`);
  return m[0];
};
const { markdownWithValues } = new Function(
  `${grab("escapeHtml")}\n${grab("safeUrl")}\n${grab("renderMarkdown")}\n${grab("markdownWithValues")}\nreturn { markdownWithValues };`
)();
// a stub formatter: shows which spec was asked for
const fmt = (v, spec) => (spec ? `${v}|${spec}` : String(v));
const row = { growth: 12.5, revenue: 1234.5, Region: "EU", c0: "ignored", evil: "<img src=x onerror=alert(1)>", url: "javascript:alert(1)", stars: "**bold**" };

const h = markdownWithValues("Revenue grew {{growth:pct}} to {{ revenue : money }} in {{region}}.", row, fmt);
assert.ok(h.includes("Revenue grew 12.5|pct to 1234.5|money in EU."), h);
// Unknown columns stay as written; a row without placeholders renders as before.
assert.ok(markdownWithValues("{{nope}} and {{growth}}", row, fmt).includes("{{nope}} and 12.5"));
assert.equal(markdownWithValues("**a**", row, fmt), markdownWithValues("**a**", {}, fmt));
// Values are text: escaped, never markup, never Markdown.
const e = markdownWithValues("x {{evil}} {{stars}}", row, fmt);
assert.ok(!/<img/i.test(e), e);
assert.ok(e.includes("&lt;img src=x onerror=alert(1)&gt;"), e);
assert.ok(e.includes("**bold**") && !e.includes("<strong>bold"), e);
// A placeholder inside a link URL is dropped (a value never becomes a URL).
const l = markdownWithValues("[go]({{url}}) and [ok](https://a.b/{{region}})", row, fmt);
assert.ok(!/javascript/i.test(l), l);
assert.ok(!l.includes("EU"), l);
// Placeholders keep working inside emphasis and lists.
assert.ok(markdownWithValues("- **{{region}}** _{{growth}}_", row, fmt).includes("<strong>EU</strong>"));
console.log("md_values_test: ok");
