// #470: a chart inside a ::VIEW group that sits in an n::COL slot must lay
// out at the slot's width, like a panel outside one — not at full width,
// scaled down afterwards (half the height, half-size text). The group's
// own ::COL (read off the view ::DIV `<section class="dp-views">`'s
// `data-outer-span`, set in app.js where the group is built) now scales a
// chart's desktop render width (`rwUnit`, app.js) by the slot's own share
// of the page, instead of assuming the view's full-width nested grid is the
// whole page.
//
//   cargo build --lib --target wasm32-unknown-unknown --profile wasm --no-default-features --features wasm
//   wasm-bindgen --target web --out-dir web/pkg --out-name anofox_visualization <target>/wasm32-unknown-unknown/wasm/anofox_visualization.wasm
//   python3 serve_nocache.py 8798 &
//   NODE_PATH=<dir with playwright>/node_modules node scripts/view-col-width-e2e.mjs [--url http://127.0.0.1:8798/] [--shots <dir>]
import { createRequire } from "module";

const require = createRequire(import.meta.url);
const { chromium } = require("playwright");
const arg = (k, d) => {
  const i = process.argv.indexOf(k);
  return i > 0 ? process.argv[i + 1] : d;
};
const base = arg("--url", "http://127.0.0.1:8798/");
const shots = arg("--shots", "");
const b64 = (s) => Buffer.from(s, "utf8").toString("base64");
let bad = 0;
const check = (ok, m) => (ok ? console.log("ok   " + m) : (bad++, console.error("FAIL " + m)));

// The issue's own repro (#470).
const SQL = `-- @title View width repro
SELECT 6::COL; SELECT 95::HEIGHT;
SELECT 'Left: plain 6-col chart'::TITLE, i::XAXIS, i::LINECHART FROM range(10) t(i);
SELECT 6::COL;
SELECT 'A' AS sw ::VIEW;
SELECT 95::HEIGHT;
SELECT 'Right: chart in a ::VIEW in a 6-col slot'::TITLE, i::XAXIS, (i * i)::LINECHART FROM range(10) t(i);
SELECT 'B' AS sw ::VIEW;
SELECT 95::HEIGHT;
SELECT 'Right, view B'::TITLE, i::XAXIS, (10 - i)::LINECHART FROM range(10) t(i);
SELECT 1::ENDVIEW;
`;

const browser = await chromium.launch();
const settled = (page) =>
  page.waitForFunction(
    () => /\d+ panels?/.test(document.getElementById("status")?.textContent || "") && !document.body.classList.contains("loading"),
    null,
    { timeout: 120000 }
  );
async function open(viewport, sql = SQL) {
  const page = await browser.newPage({ viewport, reducedMotion: "reduce" });
  const errs = [];
  page.on("pageerror", (e) => errs.push(String(e)));
  await page.goto(`${base}#sql=${b64(sql)}`);
  await settled(page);
  await page.waitForTimeout(300);
  return { page, errs };
}

// Every chart panel's title, its SVG's viewBox (the chart's own internal
// layout size) and its rendered box (actual on-screen pixels).
// The chart's own svg (not the whole .panel box — a view's panel sits below
// its group's tab bar, which shares the top-level grid row and so stretches
// the two panels' boxes to different heights by ordinary CSS grid row
// alignment; that's unrelated to #470, which is about the chart's own
// internal layout size, not the panel chrome around it).
const measure = (page) =>
  page.evaluate(() => {
    return [...document.querySelectorAll(".dash .panel, .dash .dp-viewpane .panel")].map((p) => {
      const title = p.querySelector(".panel-title, figcaption")?.textContent || "";
      const svg = p.querySelector(".panel-svg > svg");
      const r = svg?.getBoundingClientRect();
      return {
        title,
        vb: svg ? { w: svg.viewBox.baseVal.width, h: svg.viewBox.baseVal.height } : null,
        box: r ? { w: r.width, h: r.height } : null,
      };
    });
  });

{
  const { page, errs } = await open({ width: 1920, height: 1080 });
  const panels = await measure(page);
  const left = panels.find((p) => p.title.includes("Left: plain"));
  const right = panels.find((p) => p.title.includes("chart in a ::VIEW"));
  check(!!left?.vb && !!right?.vb, `both panels have a chart svg (left ${!!left?.vb}, right ${!!right?.vb})`);
  // The core bug: the view's chart used to lay out at full width (viewBox
  // width == roughly 2x the neighbour's) and was then shrunk by the browser
  // into its actual, narrower box.
  check(
    left.vb && right.vb && left.vb.w === right.vb.w,
    `the view's chart's viewBox width equals its 6-col neighbour's (left ${left.vb?.w}, right ${right.vb?.w})`
  );
  // The chart's rendered (on-screen) svg size lines up too — no residual
  // squash (the pre-#470-fix bug: half the height, half-size text).
  check(
    Math.abs(left.box.w - right.box.w) <= 1 && Math.abs(left.box.h - right.box.h) <= 1,
    `left/right rendered chart svgs match (left ${left.box.w.toFixed(1)}x${left.box.h.toFixed(1)}, right ${right.box.w.toFixed(1)}x${right.box.h.toFixed(1)})`
  );
  check(!errs.length, `no page errors: ${errs.join(" | ")}`);
  if (shots) {
    await page.screenshot({ path: `${shots}/view-col-width-1920.png` });
    console.log(`shot ${shots}/view-col-width-1920.png`);
  }
  await page.close();
}

await browser.close();
console.log(`view-col-width-e2e: ${bad} failed`);
if (bad) process.exitCode = 1;
