// Node test for the resizable sidebar (issue #59): extracts SIDE_W / clampSideW
// from web/app.js and checks the bounds and the fallback for a bad stored
// width, plus the separator markup and the phone-width override in index.html.
import { readFileSync } from "fs";
import assert from "assert/strict";

const src = readFileSync(new URL("../../web/app.js", import.meta.url), "utf8");
const html = readFileSync(new URL("../../web/index.html", import.meta.url), "utf8");
const conf = src.match(/^const SIDE_W = .*$/m);
const fn = src.match(/^function clampSideW\([\s\S]*?^}/m);
assert.ok(conf && fn, "SIDE_W / clampSideW not found in app.js");
const { SIDE_W, clampSideW } = new Function(`${conf[0]}\n${fn[0]}\nreturn { SIDE_W, clampSideW };`)();

assert.deepEqual([SIDE_W.def, SIDE_W.min, SIDE_W.max], [238, 180, 480]);
assert.equal(clampSideW(300), 300);
assert.equal(clampSideW("320"), 320);
assert.equal(clampSideW(300.6), 301);
assert.equal(clampSideW(10), 180);
assert.equal(clampSideW(9999), 480);
for (const bad of [null, "", "abc", NaN, -5, 0, Infinity]) assert.equal(clampSideW(bad), 238, String(bad));

// The handle is a focusable separator with its bounds; the sidebar takes its
// width from --side-w, and phone widths make it a fixed-width drawer (closed
// by default, opened by ☰) and hide the handle.
const handle = html.match(/<div class="side-resizer"[^>]*>/);
assert.ok(handle, "side-resizer missing in index.html");
for (const a of ['role="separator"', 'tabindex="0"', 'aria-valuemin="180"', 'aria-valuemax="480"', 'aria-valuenow="238"', 'aria-controls="sidebar"'])
  assert.ok(handle[0].includes(a), `side-resizer lacks ${a}`);
assert.match(html, /\.sidebar\{width:var\(--side-w,238px\)/);
// Other sections keep their own ≤820px blocks (Explore has one), so pick the
// one-line block that opens with the drawer rule rather than the first match.
const phone = html.match(/@media\(max-width:820px\)\{(\.sidebar,.*)\n/);
assert.ok(phone, "phone-width rule missing in index.html");
assert.match(phone[1], /^\.sidebar,body\.side-collapsed \.sidebar\{[^}]*position:fixed;[^}]*width:min\(85vw,320px\);[^}]*visibility:hidden/);
assert.match(phone[1], /body\.side-open \.sidebar\{transform:none;visibility:visible/);
assert.match(phone[1], /\.side-resizer\{display:none\}/);
assert.match(html, /<button id="side-toggle"[^>]*aria-controls="sidebar" aria-expanded=/);

console.log("sidebar_test: ok");
