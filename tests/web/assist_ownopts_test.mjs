// sec-review-256 (final check): systematic prototype-pollution safety —
// every *exported* function in web/assist.js, web/assist-engine.js,
// web/assist-csp.js and web/lint.js whose options/config parameter is
// named `opts`, `options`, `config`, `cfg` or `o` (any position, not just
// the first or second — a strict superset of what the review asked for)
// must call `ownOpts(` somewhere in its body. A function that destructures
// such a parameter directly (or reads it with a plain `.`/`?.` chain)
// reads *inherited* Object.prototype properties too; `ownOpts` is this
// PR's one fix for that, so every qualifying function must actually use
// it — a static, source-level guarantee that a future function doesn't
// quietly reintroduce the bug this review found three times running.
import { readFileSync } from "fs";
import path from "path";
import assert from "assert/strict";

const here = path.dirname(new URL(import.meta.url).pathname);
const root = path.resolve(here, "../..");
const FILES = ["web/assist.js", "web/assist-engine.js", "web/assist-csp.js", "web/lint.js"];
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
// four files have no template-literal `${}` nesting deep enough, nor
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

// Every `export [async] function NAME(...) { ... }` in `src`:
// [{name, params: [raw param text, …], body}]
function exportedFunctions(src) {
  const out = [];
  const re = /export\s+(?:async\s+)?function\s+(\w+)\s*\(/g;
  let m;
  while ((m = re.exec(src))) {
    const name = m[1];
    const parenStart = re.lastIndex - 1;
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
    out.push({ name, params, body });
    re.lastIndex = bodyStart + body.length;
  }
  return out;
}

let checked = 0;
for (const rel of FILES) {
  const src = readFileSync(path.join(root, rel), "utf8");
  for (const fn of exportedFunctions(src)) {
    if (fn.name === "ownOpts") continue; // the helper itself: nothing to protect it from
    const paramNames = fn.params.map((p) => p.split("=")[0].trim());
    const needsOwnOpts = paramNames.some((p) => OPTS_NAMES.has(p));
    if (!needsOwnOpts) continue;
    checked++;
    assert.ok(fn.body.includes("ownOpts("), `${rel}: ${fn.name}(${fn.params.join(", ")}) takes an options parameter but never calls ownOpts()`);
  }
}
// A sanity floor: if this drops to 0, the regex/brace-matching above broke
// silently (e.g. a reformat changed "export function" spacing) and the
// whole test would pass vacuously.
assert.ok(checked >= 8, `expected to find at least 8 qualifying functions, found ${checked}`);

console.log(`assist_ownopts_test: ok (${checked} functions checked)`);
