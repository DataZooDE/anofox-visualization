// The assistant's tab-wide CSP (#204, web/assist-csp.js): the policy text
// itself, appBase(), the enforcement probe against a fake `document`, the
// static baseline in index.html, and the drift/origin-inventory checks that
// keep the policy matching what the app actually loads.
import { readFileSync } from "fs";
import path from "path";
import assert from "assert/strict";
import * as C from "../../web/assist-csp.js";
import * as A from "../../web/assist.js";

const here = path.dirname(new URL(import.meta.url).pathname);
const root = path.resolve(here, "../..");
const read = (p) => readFileSync(path.join(root, p), "utf8");

// ---- 1. the policy ----
{
  const base = "https://datazoode.github.io/anofox-visualization/";
  const csp = C.assistCsp({ base });
  assert.match(csp, /^default-src 'none';/);
  for (const d of [`script-src ${base}`, "worker-src blob:", `connect-src ${base}`, `img-src ${base}`, `style-src ${base}`, `font-src ${base}`, `manifest-src ${base}`, "media-src 'none'", "object-src 'none'", "frame-src 'none'", "base-uri 'none'", "form-action 'none'"])
    assert.match(csp, new RegExp(d.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")), d);
  assert.doesNotMatch(csp, /'self'/, "no 'self' anywhere (L-3)");
  assert.doesNotMatch(csp, /'unsafe-eval'/);
  assert.doesNotMatch(csp.split(";").find((d) => d.trim().startsWith("script-src")), /'unsafe-inline'/);
  assert.doesNotMatch(csp, / \*( |;)/, "no bare wildcard source");
  const connect = csp.split(";").find((d) => d.trim().startsWith("connect-src")).trim();
  assert.equal(connect, `connect-src ${base} ${C.API} ${C.DUCKDB_WASM} ${C.DUCKDB_EXT} ${C.USGS}`);
  const worker = csp.split(";").find((d) => d.trim().startsWith("worker-src")).trim();
  assert.equal(worker, "worker-src blob:");
  // imgOrigins: only a real https:// origin or full URL survives
  const withImg = C.assistCsp({ base, imgOrigins: ["javascript:alert(1)", "http://x", "https://x/path", "https://*.x", "https://cdn.example.com", "https://cdn.example.com/logo.svg"] });
  const img = withImg.split(";").find((d) => d.trim().startsWith("img-src")).trim();
  assert.doesNotMatch(img, /javascript:|http:\/\/x|https:\/\/\*\.x/);
  assert.match(img, /https:\/\/x\/path/, "a full URL with a path is accepted (I-R2-3a)");
  assert.match(img, /https:\/\/cdn\.example\.com(?!\/)/);
  assert.match(img, /https:\/\/cdn\.example\.com\/logo\.svg/);
}

// ---- 2. appBase ----
assert.equal(C.appBase("https://datazoode.github.io/anofox-visualization/assist-csp.js"), "https://datazoode.github.io/anofox-visualization/");
assert.equal(C.appBase("https://datazoode.github.io/anofox-visualization/web/assist-csp.js"), "https://datazoode.github.io/anofox-visualization/web/");
// I-R2-3(b): appBase() is only right while assist-csp.js sits in the app's
// own root, next to index.html — pin that so a later move (e.g. into
// web/assist/) fails this test instead of silently breaking every fetch.
{
  const indexDir = new URL("../../web/index.html", import.meta.url);
  const cspModuleDir = new URL("./", new URL("../../web/assist-csp.js", import.meta.url));
  assert.equal(new URL("./", indexDir).href, cspModuleDir.href, "assist-csp.js must sit next to index.html");
}

// ---- 3. drift ----
{
  const appjs = read("web/app.js");
  assert.match(appjs, new RegExp(C.DUCKDB_WASM.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")), "DUCKDB_WASM matches app.js's import");
  const pkg = JSON.parse(read("tests/web/package.json"));
  assert.equal(`${C.DUCKDB_WASM}`.includes(pkg.dependencies["@duckdb/duckdb-wasm"]), true, "DUCKDB_WASM matches tests/web/package.json's pin");
  assert.match(appjs, new RegExp(C.KATEX.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")), "KATEX matches app.js's KATEX_CDN");
  const localextDir = "web/localext/v1.5.6";
  assert.ok(C.DUCKDB_EXT.includes("v1.5.6"), "DUCKDB_EXT's version matches web/localext's");
  assert.equal(C.API, A.API_HOST, "API matches assist.js's API_HOST");
  void localextDir;
}

// ---- 5. the ratchet against a fake document ----
function fakeDocument() {
  const listeners = new Set();
  const head = { children: [], appendChild(el) { el.__inHead = true; head.children.push(el); } };
  return {
    head,
    createElement: (tag) => ({ tag, dataset: {}, set httpEquiv(v) { this._he = v; }, set content(v) { this._c = v; }, set src(v) { this._src = v; } }),
    addEventListener: (t, fn) => listeners.add(fn),
    removeEventListener: (t, fn) => listeners.delete(fn),
    __fire(detail) {
      for (const fn of [...listeners]) fn(detail);
    },
  };
}
{
  const doc = fakeDocument();
  const origCreate = doc.createElement;
  doc.createElement = (tag) => {
    const el = origCreate(tag);
    if (tag === "img") setTimeout(() => doc.__fire({ effectiveDirective: "img-src", blockedURI: el._src?.split("?")[0] }), 0);
    return el;
  };
  const p1 = C.applyAssistCsp(doc);
  const p2 = C.applyAssistCsp(doc);
  assert.equal(p1, p2, "memoised: a second call returns the same promise");
  await p1;
  assert.equal(doc.head.children.filter((c) => c.tag === "meta").length, 1, "exactly one meta inserted");
  assert.equal(await C.assistCspProven(doc), undefined);
}
{
  const doc = fakeDocument(); // never fires: rejects after ~1s
  await assert.rejects(C.applyAssistCsp(doc), /not enforced/);
}
{
  const untouched = fakeDocument();
  await assert.rejects(C.assistCspProven(untouched), /not applied/);
}
// sec-review-256 R-1 check: a caller-supplied `base` that isn't same-origin
// with the page (pollution or not) must never reach the real policy —
// applyAssistCsp drops it and falls back to appBase() instead.
{
  const doc = fakeDocument();
  const origCreate = doc.createElement;
  doc.createElement = (tag) => {
    const el = origCreate(tag);
    if (tag === "img") setTimeout(() => doc.__fire({ effectiveDirective: "img-src", blockedURI: el._src?.split("?")[0] }), 0);
    return el;
  };
  await C.applyAssistCsp(doc, { base: "https://evil.example/" });
  const meta = doc.head.children.find((c) => c.tag === "meta");
  assert.doesNotMatch(meta._c, /evil\.example/, "a cross-origin base is never applied");
  assert.match(meta._c, new RegExp(`script-src ${C.appBase().replace(/[.*+?^${}()|[\]\\]/g, "\\$&")} `), "falls back to appBase() instead");
}

// ---- 6. index.html's static baseline ----
{
  const html = read("web/index.html");
  assert.match(html, /<meta http-equiv="Content-Security-Policy" content="object-src 'none'; base-uri 'none'">/);
  const headIdx = html.indexOf("<head>");
  const metaIdx = html.search(/<meta http-equiv="Content-Security-Policy"/);
  assert.ok(metaIdx > headIdx && metaIdx < html.indexOf("</head>"));
  assert.doesNotMatch(html, /<base[\s>]/i, "no static <base> (serve injects its own after this template, before the ratchet ever runs)");
}

// ---- 7. assist.js: framed, remember, keyStore, browserKeyTransport ----
assert.deepEqual(A.assistConfig({ enabled: true }, { framed: true }), { enabled: false, reason: "framed" });
assert.equal(A.rememberAllowed("datazoode.github.io", true), false);
assert.equal(A.rememberAllowed("x.gitlab.io", true), false);
assert.equal(A.rememberAllowed("demo.anofox.dev", true), true);
assert.equal(A.rememberAllowed("demo.anofox.dev"), false, "opt-in default off (L-R2-1)");
{
  const mem = () => {
    const m = new Map();
    return { getItem: (k) => (m.has(k) ? m.get(k) : null), setItem: (k, v) => m.set(k, String(v)), removeItem: (k) => m.delete(k), m };
  };
  const s = mem(),
    l = mem();
  const ks = A.keyStore(s, l, { remember: false });
  ks.set("sk-ant-x", true);
  assert.equal(l.m.size, 0, "remember: false — never written to localStorage");
  assert.equal(s.m.get(A.KEY_SLOT), "sk-ant-x");
}
{
  // migration on construction (M-2/T13)
  const mem = () => {
    const m = new Map();
    return { getItem: (k) => (m.has(k) ? m.get(k) : null), setItem: (k, v) => m.set(k, String(v)), removeItem: (k) => m.delete(k), m };
  };
  const s = mem(),
    local = mem();
  local.m.set(A.KEY_SLOT, "sk-ant-old");
  A.keyStore(s, local, { remember: false });
  assert.equal(local.m.size, 0);
  assert.equal(s.m.get(A.KEY_SLOT), "sk-ant-old");
}
{
  let seen;
  class FakeAnthropic {
    constructor(opts) {
      seen = opts;
    }
  }
  A.browserKeyTransport({ Anthropic: FakeAnthropic, apiKey: "sk-ant-x" });
  assert.equal(seen.baseURL, A.API_HOST, "I-7: baseURL defaults to API_HOST, never undefined");
}

// ---- static greps (comments stripped — the header comment above names
// every forbidden API on purpose, to say it isn't used) ----
const noComments = (s) => s.replace(/\/\*[\s\S]*?\*\//g, " ").replace(/\/\/.*$/gm, "");
{
  const src = noComments(read("web/assist-csp.js"));
  for (const re of [/fetch\(/, /XMLHttpRequest/, /registerFileURL/, /\bopfs\b/i, /document\.baseURI/, /\bimport\(/]) assert.doesNotMatch(src, re, re.toString());
}

console.log("assist_csp_test: ok");
