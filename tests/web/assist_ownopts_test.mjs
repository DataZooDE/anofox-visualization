// sec-review-256 (final check, hardened at "last check"): systematic
// prototype-pollution safety — every *exported* function (a `function`
// declaration or an `export const f = (…) => …` arrow) in web/assist.js,
// web/assist-engine.js, web/assist-csp.js, web/lint.js and web/explore.js
// must never destructure an object parameter directly in its signature
// (`function f({a, b} = {})` reads *inherited* Object.prototype properties
// too), and any parameter named like an options bag (opts/options/config/
// cfg/o, any position) must be passed, by that exact name, to `ownOpts(`
// somewhere in the function's body — comments stripped first, so a stray
// mention of "ownOpts(" in a comment, or a call on the wrong argument,
// doesn't satisfy the check.
//
// "Final check" only looked at bare parameter *names*, so a parameter
// destructured right in the signature — which has no bare name at all —
// passed unseen (`resultProfile`'s `values`/`names` being the one with a
// real security effect: sec-review-256 "Last check"). This version also
// flags that pattern outright, and extends the file list with explore.js,
// which "final check"'s FILES never covered.
import { readFileSync } from "fs";
import path from "path";
import assert from "assert/strict";

const here = path.dirname(new URL(import.meta.url).pathname);
const root = path.resolve(here, "../..");
const FILES = ["web/assist.js", "web/assist-engine.js", "web/assist-csp.js", "web/lint.js", "web/explore.js"];
const OPTS_NAMES = new Set(["opts", "options", "config", "cfg", "o"]);

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

// Every top-level exported function in `src`: a `function` declaration or
// an `export const NAME = (...) => ...` arrow — [{name, params, body, kind}].
// `body` is the brace-matched block for a `{ … }` body, or the raw
// expression text for an arrow with an implicit return.
function exportedFunctions(src) {
  const out = [];
  const declRe = /export\s+(?:async\s+)?function\s+(\w+)\s*\(/g;
  let m;
  while ((m = declRe.exec(src))) {
    const name = m[1];
    const parenStart = declRe.lastIndex - 1;
    let depth = 0,
      i = parenStart,
      paramEnd = -1;
    for (; i < src.length; i++) {
      if (src[i] === "(") depth++;
      else if (src[i] === ")") {
        depth--;
        if (depth === 0) {
          paramEnd = i;
          break;
        }
      }
    }
    const params = topLevelSplit(src.slice(parenStart + 1, paramEnd));
    const bodyStart = src.indexOf("{", paramEnd);
    const body = braceBlock(src, bodyStart);
    out.push({ name, params, body, kind: "function" });
    declRe.lastIndex = bodyStart + body.length;
  }
  const arrowRe = /export\s+const\s+(\w+)\s*=\s*(?:async\s*)?\(/g;
  while ((m = arrowRe.exec(src))) {
    const name = m[1];
    const parenStart = arrowRe.lastIndex - 1;
    let depth = 0,
      i = parenStart,
      paramEnd = -1;
    for (; i < src.length; i++) {
      if (src[i] === "(") depth++;
      else if (src[i] === ")") {
        depth--;
        if (depth === 0) {
          paramEnd = i;
          break;
        }
      }
    }
    if (paramEnd < 0) continue;
    const params = topLevelSplit(src.slice(parenStart + 1, paramEnd));
    // Only a genuine arrow function: "=>" (skipping whitespace/newlines)
    // right after the parameter list, not e.g. a plain function *call*
    // assigned to a const (which this regex could otherwise also match).
    const afterParen = src.slice(paramEnd + 1).match(/^\s*=>\s*/);
    if (!afterParen) continue;
    const bodyAt = paramEnd + 1 + afterParen[0].length;
    let body;
    if (src[bodyAt] === "{") body = braceBlock(src, bodyAt);
    else {
      // An implicit-return arrow: take the rest of the statement, up to
      // the matching top-level ";" or newline-at-depth-0 — good enough for
      // this test (it only ever needs to see whether "ownOpts(" is there).
      let depth2 = 0,
        j = bodyAt;
      for (; j < src.length; j++) {
        const c = src[j];
        if ("([{".includes(c)) depth2++;
        else if (")]}".includes(c)) depth2--;
        else if (c === ";" && depth2 <= 0) break;
        else if (c === "\n" && depth2 <= 0) break;
      }
      body = src.slice(bodyAt, j);
    }
    out.push({ name, params, body, kind: "arrow" });
  }
  return out;
}

let checked = 0;
let destructuredSeen = 0;
for (const rel of FILES) {
  const src = readFileSync(path.join(root, rel), "utf8");
  for (const fn of exportedFunctions(src)) {
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
      if (!/^[A-Za-z_$][\w$]*$/.test(base) || !OPTS_NAMES.has(base)) continue;
      checked++;
      const callRe = new RegExp(`\\bownOpts\\(\\s*${base}\\s*\\)`);
      assert.ok(callRe.test(cleanBody), `${rel}: ${fn.name}(${fn.params.join(", ")}) takes an options parameter "${base}" but never calls ownOpts(${base}) in its body`);
    }
  }
}
// A sanity floor: if this drops to 0, the regex/brace-matching above broke
// silently (e.g. a reformat changed "export function" spacing) and the
// whole test would pass vacuously.
assert.ok(checked >= 8, `expected to find at least 8 qualifying functions, found ${checked}`);
assert.equal(destructuredSeen, 0, "expected zero signature-destructured parameters left (unreachable: assert.fail above throws first)");

console.log(`assist_ownopts_test: ok (${checked} functions checked, 0 signature-destructured parameters)`);
