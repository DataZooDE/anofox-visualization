// sec-review-256 (final check, hardened at "last check"): systematic
// prototype-pollution safety — every function (a `function` declaration, a
// `const f = (…) => …` arrow, or a method-shorthand `f(…) { … }` inside an
// object literal or class) in web/assist.js, web/assist-engine.js,
// web/assist-csp.js, web/lint.js, web/explore.js and web/assist-ui.js must
// never destructure an object parameter directly in its signature
// (`function f({a, b} = {})` reads *inherited* Object.prototype properties
// too), and any parameter named like an options bag must be passed, by
// that exact name, to `ownOpts(` somewhere in the function's body —
// comments stripped first, so a stray mention of "ownOpts(" in a comment,
// or a call on the wrong argument, doesn't satisfy the check.
//
// "Final check" only looked at bare parameter *names*, so a parameter
// destructured right in the signature — which has no bare name at all —
// passed unseen (`resultProfile`'s `values`/`names` being the one with a
// real security effect: sec-review-256 "Last check"). This version also
// flags that pattern outright, and extends the file list with explore.js,
// which "final check"'s FILES never covered.
//
// #244: web/assist-ui.js joins the scan too — the panel builds its own
// options objects (applyModel, diffScripts, buildCtx-shaped calls, …) and
// must follow the same rule as every other assistant module.
//
// sec-review-272 Info-1: the above only ever looked at *exported* functions
// — assist-ui.js's own inner ones (dialog, sentDialog, open, close: built
// by mountAssistant(), never exported, called only from inside the module)
// read their options bags directly and went unchecked. web/assist-ui.js
// was widened to every function — exported or not, at any nesting depth.
//
// #268 (follow-up from sec-review-256's own "Verdict"): that widening
// stopped at assist-ui.js. Three more gaps, none exploitable (no pollution
// path was found for any of them — this is completeness of the STATIC
// CHECK, not a fix to a live hole), but all three are now closed:
//  1. Every file is scanned for every function, exported or not — the
//     same breadth assist-ui.js already had. A `function`/arrow declared
//     but never exported (a private helper in assist-engine.js, say) could
//     otherwise destructure an options bag with nobody ever checking.
//  2. Method shorthand (`name(params) { … }` inside an object literal or
//     class body — never a `function` keyword, never a `const … =>`) is
//     now found too: the transport's `send(request, {signal, …})` and the
//     tool handlers (`list_tables({filter})`, `profile_table({table})`,
//     `run_sql({sql})`, `check(sql, {signal})` in assist.js's
//     `wasmTools()`) all destructure directly and went completely unseen
//     by the old function-or-arrow-only scan.
//  3. OPTS_NAMES grows by two real parameter names this codebase actually
//     uses for an options-shaped bag: `limits` (assist-engine.js's
//     snapshotTables) and `u` (assist.js's spent — a response's `usage`
//     object, read key-by-key with defaults exactly like every other
//     options bag here).
import { readFileSync } from "fs";
import path from "path";
import assert from "assert/strict";

const here = path.dirname(new URL(import.meta.url).pathname);
const root = path.resolve(here, "../..");
const FILES = ["web/assist.js", "web/assist-engine.js", "web/assist-csp.js", "web/lint.js", "web/explore.js", "web/assist-ui.js"];
const OPTS_NAMES = new Set(["opts", "options", "config", "cfg", "o", "limits"]);
// #268: a couple of functions use an even less descriptive name for an
// options-shaped bag (assist.js's `spent(u = {})` is why `u` is here) —
// these only count as options-like when the signature's own default is
// the empty object literal `{}`, so an unrelated, same-named parameter
// that ISN'T an options bag (assist-csp.js's `appBase(u = import.meta.url)`,
// a URL string — "u" is short for "URL" there, not "usage") is never
// mistaken for one.
const OPTS_NAMES_IF_EMPTY_DEFAULT = new Set(["u"]);

// Splits a parameter list (or any comma-separated text) on top-level
// commas only — commas inside (), [] or {} don't count.
function topLevelSplit(s) {
  const parts = [];
  let depth = 0,
    cur = "";
  for (const ch of s) {
    if ("([{".includes(ch)) depth++;
    else if (")]}".includes(ch)) depth--;
    if (ch === "," && depth === 0) {
      parts.push(cur);
      cur = "";
    } else cur += ch;
  }
  if (cur.trim()) parts.push(cur);
  return parts.map((p) => p.trim());
}

// The source text of the {...} block starting at `openBraceIdx` (which
// must point at a '{'), matching braces (a crude but sufficient scan: the
// five files have no template-literal `${}` nesting deep enough, nor
// stray braces in strings, to confuse a plain counter at this grain).
function braceBlock(src, openBraceIdx) {
  let depth = 0,
    i = openBraceIdx;
  for (; i < src.length; i++) {
    if (src[i] === "{") depth++;
    else if (src[i] === "}") {
      depth--;
      if (depth === 0) return src.slice(openBraceIdx, i + 1);
    }
  }
  return src.slice(openBraceIdx);
}

// Strips // and /* */ comments from JS source, tracking string/template
// literal state (', ", `) so a "//" or "/*" inside a quoted string (e.g.
// "https://…") is never mistaken for the start of a comment. Each stripped
// comment is replaced by a single space (so token boundaries survive, and
// line numbers roughly do too — not that this test needs them).
function stripComments(src) {
  let out = "";
  let i = 0;
  const n = src.length;
  while (i < n) {
    const ch = src[i];
    const two = src.slice(i, i + 2);
    if (ch === "'" || ch === '"' || ch === "`") {
      const quote = ch;
      let j = i + 1;
      while (j < n) {
        if (src[j] === "\\") {
          j += 2;
          continue;
        }
        if (src[j] === quote) {
          j++;
          break;
        }
        j++;
      }
      out += src.slice(i, j);
      i = j;
      continue;
    }
    if (two === "//") {
      let j = src.indexOf("\n", i);
      if (j < 0) j = n;
      out += " ";
      i = j;
      continue;
    }
    if (two === "/*") {
      const j = src.indexOf("*/", i + 2);
      out += " ";
      i = j < 0 ? n : j + 2;
      continue;
    }
    out += ch;
    i++;
  }
  return out;
}

// Every `function NAME(...)` and `const NAME = (...) =>` in `cleanSrc`
// (already comment-stripped, so "function" inside a comment can't be
// mistaken for a declaration), exported or not, at any nesting depth —
// [{name, params, body, kind}]. `body` is the brace-matched block for a
// `{ … }` body, or the raw expression text for an arrow with an implicit
// return.
function allFunctions(cleanSrc) {
  const out = [];
  const declRe = /\bfunction\s+(\w+)\s*\(/g;
  let m;
  while ((m = declRe.exec(cleanSrc))) {
    const name = m[1];
    const parenStart = declRe.lastIndex - 1;
    let depth = 0,
      i = parenStart,
      paramEnd = -1;
    for (; i < cleanSrc.length; i++) {
      if (cleanSrc[i] === "(") depth++;
      else if (cleanSrc[i] === ")") {
        depth--;
        if (depth === 0) {
          paramEnd = i;
          break;
        }
      }
    }
    const params = topLevelSplit(cleanSrc.slice(parenStart + 1, paramEnd));
    const bodyStart = cleanSrc.indexOf("{", paramEnd);
    const body = braceBlock(cleanSrc, bodyStart);
    out.push({ name, params, body, kind: "function" });
    // Unlike exportedFunctions() above, lastIndex is NOT skipped past the
    // whole body: a nested function (dialog, sentDialog, open, close, …,
    // all declared inside mountAssistant's one big body) must still be
    // found by the next iteration, not stepped over.
  }
  const arrowRe = /\bconst\s+(\w+)\s*=\s*(?:async\s*)?\(/g;
  while ((m = arrowRe.exec(cleanSrc))) {
    const name = m[1];
    const parenStart = arrowRe.lastIndex - 1;
    let depth = 0,
      i = parenStart,
      paramEnd = -1;
    for (; i < cleanSrc.length; i++) {
      if (cleanSrc[i] === "(") depth++;
      else if (cleanSrc[i] === ")") {
        depth--;
        if (depth === 0) {
          paramEnd = i;
          break;
        }
      }
    }
    if (paramEnd < 0) continue;
    const params = topLevelSplit(cleanSrc.slice(parenStart + 1, paramEnd));
    // Only a genuine arrow function (see exportedFunctions() above).
    const afterParen = cleanSrc.slice(paramEnd + 1).match(/^\s*=>\s*/);
    if (!afterParen) continue;
    const bodyAt = paramEnd + 1 + afterParen[0].length;
    let body;
    if (cleanSrc[bodyAt] === "{") body = braceBlock(cleanSrc, bodyAt);
    else {
      let depth2 = 0,
        j = bodyAt;
      for (; j < cleanSrc.length; j++) {
        const c = cleanSrc[j];
        if ("([{".includes(c)) depth2++;
        else if (")]}".includes(c)) depth2--;
        else if (c === ";" && depth2 <= 0) break;
        else if (c === "\n" && depth2 <= 0) break;
      }
      body = cleanSrc.slice(bodyAt, j);
    }
    out.push({ name, params, body, kind: "arrow" });
  }
  return out;
}

// #268: keywords that can precede a `(…) {` shape exactly like a method
// definition does (`if (x) {`, `catch (e) {`, …) — every one of these is a
// control-flow construct, never a function, and must not be mistaken for
// one by methodFunctions() below.
const METHOD_RESERVED = new Set([
  "if", "for", "while", "switch", "catch", "do", "else", "function", "return", "typeof", "new", "yield", "await",
  "get", "set", "static", "class", "try", "finally", "with", "in", "of", "instanceof", "void", "delete", "throw",
  "case", "default", "export", "import", "const", "let", "var", "async",
]);
// Every method-shorthand function (`name(params) { … }`, inside an object
// literal or a class body — never behind the `function` keyword, never a
// `const … =>` arrow) in `cleanSrc`, at any nesting depth —
// [{name, params, body, kind: "method"}]. #268: the old scan only ever
// matched `function NAME(` or `const NAME = (…) =>`, so a method such as
// assist.js's `send(request, {signal, …}) {` (the transport) or its
// `list_tables({filter}) {` / `profile_table({table}) {` / `run_sql({sql})
// {` / `check(sql, {signal}) {` tool handlers (all inside `wasmTools()`'s
// returned object) went completely unseen.
//
// A method name is only recognised right after a structural boundary — a
// newline, `;`, `{`, `}`, `(` or `,` — with nothing but whitespace (never
// another keyword) in between: this is exactly what rules out `function
// NAME(` (the word "function" sits in that gap) and an arrow's `NAME = (`
// (there is no `(` immediately after NAME at all, only `=`). What's left
// that fits "identifier, then `(…)`, then `{`" is either a genuine method,
// or a control-flow keyword — hence METHOD_RESERVED.
function methodFunctions(cleanSrc) {
  const out = [];
  const methodRe = /(?:^|[\n;{}(,])[ \t]*(?:async\s+)?([A-Za-z_$][\w$]*)\s*\(/gm;
  let m;
  while ((m = methodRe.exec(cleanSrc))) {
    const name = m[1];
    if (METHOD_RESERVED.has(name)) continue;
    const parenStart = methodRe.lastIndex - 1;
    let depth = 0,
      i = parenStart,
      paramEnd = -1;
    for (; i < cleanSrc.length; i++) {
      if (cleanSrc[i] === "(") depth++;
      else if (cleanSrc[i] === ")") {
        depth--;
        if (depth === 0) {
          paramEnd = i;
          break;
        }
      }
    }
    if (paramEnd < 0) continue;
    // A method definition's `)` is followed (modulo whitespace/newlines) by
    // its `{` body, directly — never `=>` (an arrow), never anything else
    // (a plain call, a condition used some other way). This is what tells
    // a real method apart from, say, a call like `foo(x)` that happens to
    // sit right after a `{`/`;`/`,` boundary.
    const afterParen = cleanSrc.slice(paramEnd + 1).match(/^[ \t\n]*/);
    const bodyAt = paramEnd + 1 + afterParen[0].length;
    if (cleanSrc[bodyAt] !== "{") continue;
    const params = topLevelSplit(cleanSrc.slice(parenStart + 1, paramEnd));
    const body = braceBlock(cleanSrc, bodyAt);
    out.push({ name, params, body, kind: "method" });
  }
  return out;
}

let checked = 0;
let destructuredSeen = 0;
for (const rel of FILES) {
  const src = readFileSync(path.join(root, rel), "utf8");
  // #268: every file is scanned for every function AND every method,
  // exported or not, at any nesting depth — the same breadth assist-ui.js
  // (Info-1) already had, now applied everywhere (methodFunctions is new
  // for every file, assist-ui.js included: mountAssistant() may well build
  // object literals with method shorthand too).
  const clean = stripComments(src);
  const fns = [...allFunctions(clean), ...methodFunctions(clean)];
  for (const fn of fns) {
    if (fn.name === "ownOpts") continue; // the helper itself: nothing to protect it from
    const cleanBody = stripComments(fn.body);
    for (const raw of fn.params) {
      const base = raw.split("=")[0].trim();
      // 1. A parameter destructured directly in the signature
      // (`{ a, b }` or `{ a, b } = {}`) has no bare name at all, so there
      // is nothing to pass to ownOpts(): the signature itself must not do
      // this (sec-review-256 "last check" — resultProfile's `values`/
      // `names` were exactly this, unseen by the older bare-name-only
      // check below).
      if (base.startsWith("{")) {
        destructuredSeen++;
        assert.fail(`${rel}: ${fn.name}(${fn.params.join(", ")}) destructures an object parameter directly in its signature — rewrite as (opts = {}) and destructure ownOpts(opts) in the body instead`);
      }
      // 2. A bare options-like parameter name must reach ownOpts(), by
      // that exact name, somewhere in the (comment-stripped) body — not
      // merely contain the text "ownOpts(" anywhere (a comment, or a call
      // on some other value, must not satisfy this).
      if (!/^[A-Za-z_$][\w$]*$/.test(base)) continue;
      const defaultPart = raw.includes("=") ? raw.slice(raw.indexOf("=") + 1).trim() : null;
      const isOpts = OPTS_NAMES.has(base) || (OPTS_NAMES_IF_EMPTY_DEFAULT.has(base) && defaultPart === "{}");
      if (!isOpts) continue;
      checked++;
      const callRe = new RegExp(`\\bownOpts\\(\\s*${base}\\s*\\)`);
      assert.ok(callRe.test(cleanBody), `${rel}: ${fn.name}(${fn.params.join(", ")}) takes an options parameter "${base}" but never calls ownOpts(${base}) in its body`);
    }
  }
}
// A sanity floor: if this drops too low, the regex/brace-matching above
// broke silently (e.g. a reformat changed "export function" spacing, or
// methodFunctions() stopped finding methods) and the whole test would
// pass vacuously. #268: the widened scan (every file, every function AND
// method, exported or not) finds over 40 today — comfortably above the
// pre-#268 floor of 8, which only ever covered exported functions/arrows.
assert.ok(checked >= 30, `expected to find at least 30 qualifying functions, found ${checked}`);
assert.equal(destructuredSeen, 0, "expected zero signature-destructured parameters left (unreachable: assert.fail above throws first)");

console.log(`assist_ownopts_test: ok (${checked} functions checked, 0 signature-destructured parameters)`);
