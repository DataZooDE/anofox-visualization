// Node test for TeX math in Markdown (issue #61): extracts mathSpans /
// renderMarkdown / markdownWithValues from web/app.js and checks the $…$
// tokenizer (amounts and \$ stay text), the inert .md-math markup and values
// inside a formula. Typesetting itself (KaTeX) is checked in the browser.
import { readFileSync } from "fs";
import assert from "assert/strict";

const src = readFileSync(new URL("../../web/app.js", import.meta.url), "utf8");
const grab = (name) => {
  const m = src.match(new RegExp(`^function ${name}\\([\\s\\S]*?^}`, "m"));
  assert.ok(m, `function ${name} not found in app.js`);
  return m[0];
};
const { mathSpans, renderMarkdown, markdownWithValues } = new Function(
  ["escapeHtml", "safeUrl", "mathSpans", "renderMarkdown", "markdownColumn", "markdownWithValues", "texText"].map(grab).join("\n") +
    "\nreturn { mathSpans, renderMarkdown, markdownWithValues };"
)();
const math = (s) => mathSpans(s).filter((g) => g.tex != null);

// Delimiters (the same cases as src/math.rs).
assert.deepEqual(math("a $x^2$ b"), [{ tex: "x^2", display: false }]);
assert.deepEqual(math("$$ \\hat{y}_t $$"), [{ tex: "\\hat{y}_t", display: true }]);
assert.deepEqual(math("costs $5 and $10 today"), []);
assert.deepEqual(math("$5 and $10, then $x$ and $$y$$"), [
  { tex: "x", display: false },
  { tex: "y", display: true },
]);
assert.deepEqual(math("a $ x$ and $x $ b"), []);
assert.deepEqual(math("`$x$` and \\$y\\$"), []);
assert.deepEqual(mathSpans("pay \\$5"), [{ text: "pay $5" }]);
assert.deepEqual(math("$a$1"), []);
assert.deepEqual(math("$\\$5$"), [{ tex: "\\$5", display: false }]);
assert.deepEqual(math("for $5 and $10:\n$$ \\sum_i x_i $$"), [{ tex: "\\sum_i x_i", display: true }]);

// Markup: an inert span with the TeX source as text (until KaTeX), escaped;
// Markdown around a formula still works, Markdown inside it doesn't.
const h = renderMarkdown("**Model** $a*b*c < d$ and $5 and $10, \\$3.");
assert.ok(h.includes("<strong>Model</strong>"), h);
assert.ok(h.includes('<span class="md-math" data-tex="a*b*c &lt; d"><code class="md-tex">a*b*c &lt; d</code></span>'), h);
assert.ok(h.includes("and $5 and $10, $3."), h);
assert.ok(!h.includes("<em>"), h);
const d = renderMarkdown("Text\n\n$$\n\\frac{a}{b}\n$$");
assert.ok(d.includes('<span class="md-math md-math-display" data-tex="\\frac{a}{b}">'), d);
// TeX can't smuggle markup: quotes / tags are escaped in the attribute and text.
const x = renderMarkdown('$"><img src=x onerror=alert(1)>$');
assert.ok(!/<img/i.test(x) && x.includes("&quot;&gt;&lt;img"), x);
// A formula inside a link URL never becomes markup.
const l = renderMarkdown("[go](https://a.b/$x$)");
assert.ok(!l.includes("md-math"), l);

// {{col:fmt}} inside a formula: TeX text, special characters escaped; outside
// it unchanged.
const fmt = (v, spec) => (spec ? `${v}|${spec}` : String(v));
const v = markdownWithValues("Grew $g = {{growth:pct}}$ to {{growth}}", { growth: "12.5%_$#{}\\" }, fmt);
assert.ok(v.includes('data-tex="g = \\text{12.5\\%\\_\\$\\#\\{\\}\\textbackslash{}|pct}"'), v);
assert.ok(v.includes('<code class="md-tex">g = 12.5%_$#{}\\|pct</code>'), v);
assert.ok(v.includes("to 12.5%_$#{}\\</p>"), v);
const evil = markdownWithValues("$x={{e}}$", { e: '"><script>' }, fmt);
assert.ok(!/<script/i.test(evil) && evil.includes('data-tex="x=\\text{&quot;&gt;&lt;script&gt;}"'), evil);
console.log("math_test: ok");
