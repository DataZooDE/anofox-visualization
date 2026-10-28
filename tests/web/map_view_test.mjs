// Node test for the map zoom maths (M3d, #643): extracts mapViewOf /
// viewToProj / bboxToProj / clampView / boxView from web/app.js (the
// tests/web/views_test.mjs convention).
import { readFileSync } from "fs";
import assert from "assert/strict";

const src = readFileSync(new URL("../../web/app.js", import.meta.url), "utf8");
const grab = (name) => {
  const m = src.match(new RegExp(`^function ${name}\\([\\s\\S]*?^}`, "m"));
  assert.ok(m, `function ${name} not found in app.js`);
  return m[0];
};
const fns = ["mapViewOf", "viewToProj", "bboxToProj", "clampView", "boxView"];
const { mapViewOf, viewToProj, bboxToProj, clampView, boxView } = new Function(`${fns.map(grab).join("\n")}\nreturn { ${fns.join(", ")} };`)();
const near = (a, b, eps = 1e-9) => assert.ok(Math.abs(a - b) < eps, `${a} vs ${b}`);

// parse
const mv = mapViewOf("10 20 400 200", "0 100 -50 0");
assert.deepEqual(mv, { vp: { x: 10, y: 20, w: 400, h: 200 }, d: { x0: 0, x1: 100, y0: -50, y1: 0 } });
assert.equal(mapViewOf(null, "0 1 0 1"), null);
assert.equal(mapViewOf("0 0 0 10", "0 1 0 1"), null); // zero width
assert.equal(mapViewOf("0 0 10 10", "0 1 x 1"), null);

// viewBox → map units: corners and the centre (y grows up)
assert.deepEqual(viewToProj(mv, 10, 20), [0, 0]);
assert.deepEqual(viewToProj(mv, 410, 220), [100, -50]);
assert.deepEqual(viewToProj(mv, 210, 120), [50, -25]);

// a bbox → a projected box
assert.deepEqual(bboxToProj(mv, { x: 10, y: 20, width: 200, height: 100 }), { x0: 0, x1: 50, y0: -25, y1: 0 });

// clamp: within limits unchanged; too small grows to home/64, too big shrinks to 2×home, about the centre
const home = { x0: 0, x1: 640, y0: 0, y1: 320 };
const v = { x0: 100, x1: 200, y0: 0, y1: 50 };
assert.equal(clampView(v, home), v);
const tiny = clampView({ x0: 10, x1: 11, y0: 10, y1: 10.5 }, home);
near(tiny.x1 - tiny.x0, 10);
near((tiny.x0 + tiny.x1) / 2, 10.5);
near((tiny.y1 - tiny.y0) / (tiny.x1 - tiny.x0), 0.5); // aspect kept
const huge = clampView({ x0: -1000, x1: 3000, y0: 0, y1: 100 }, home);
near(huge.x1 - huge.x0, 1280);
assert.equal(clampView(v, null), v);

// boxView: 15% padding per side, the viewport aspect, at least home/8 wide
const b = boxView({ x0: 100, x1: 200, y0: 100, y1: 150 }, home, 2);
near(b.x1 - b.x0, 130);
near((b.x1 - b.x0) / (b.y1 - b.y0), 2);
near((b.x0 + b.x1) / 2, 150);
near((b.y0 + b.y1) / 2, 125);
const pt = boxView({ x0: 5, x1: 5, y0: 5, y1: 5 }, home, 2); // a point
near(pt.x1 - pt.x0, 80);
near((pt.x0 + pt.x1) / 2, 5);
const tall = boxView({ x0: 0, x1: 10, y0: 0, y1: 100 }, home, 2); // height-bound
near(tall.y1 - tall.y0, 130);
near(tall.x1 - tall.x0, 260);
console.log("map_view_test: ok");
