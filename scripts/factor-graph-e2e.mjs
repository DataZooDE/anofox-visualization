// #1152: a factor graph's message schedule steps in a real browser
// (DuckDB-Wasm): the example "How does the model reason from our
// inspections?" draws two factor graphs with `schedule :=`; Step lights the
// edge carrying the message (`.dp-msg`, `.dp-msg-back` against the edge),
// its two ends stay lit, the caption names the message, and stepping back to
// the overview clears it.
//
//   wasm-pack build --target web --out-dir web/pkg --profile wasm --no-default-features --features wasm
//   python3 serve_nocache.py 8947 &
//   NODE_PATH=<dir with playwright>/node_modules node scripts/factor-graph-e2e.mjs [--url http://127.0.0.1:8947/]
import { createRequire } from "module";

const require = createRequire(import.meta.url);
const { chromium } = require("playwright");
const arg = (k, d) => {
  const i = process.argv.indexOf(k);
  return i > 0 ? process.argv[i + 1] : d;
};
const base = arg("--url", "http://127.0.0.1:8947/");
let bad = 0;
const check = (ok, m, got) => (ok ? console.log("ok   " + m) : (bad++, console.error("FAIL " + m + (got === undefined ? "" : " " + JSON.stringify(got)))));

const browser = await chromium.launch();
const page = await browser.newPage({ viewport: { width: 1400, height: 1000 } });
const errors = [];
page.on("pageerror", (e) => errors.push(String(e)));
await page.goto(base + "?dashboard=how-does-the-model-reason-from-our-inspections");
await page.waitForFunction(() => document.querySelectorAll("svg[data-diagram]").length >= 2, null, { timeout: 120000 });
await page.waitForTimeout(500);

const state = () =>
  page.evaluate(() => {
    const p = [...document.querySelectorAll(".panel")].find((x) => x.querySelector("svg[data-diagram]"));
    const svg = p.querySelector("svg[data-diagram]");
    const msg = [...svg.querySelectorAll(".dp-msg")].map((e) => e.getAttribute("data-edge") + (e.classList.contains("dp-msg-back") ? " back" : ""));
    const lit = [...svg.querySelectorAll("rect[data-node]")].filter((e) => !e.style.opacity).map((e) => e.getAttribute("data-node"));
    return { pos: p.querySelector(".dp-story-pos")?.textContent || "", cap: p.querySelector(".dp-story-cap")?.textContent || "", msg, lit };
  });
const step = (cls) =>
  page.evaluate((c) => [...document.querySelectorAll(".panel")].find((x) => x.querySelector("svg[data-diagram]")).querySelector(c).click(), cls);

check((await page.locator(".dp-story").count()) === 2, "both factor graphs have a stepper");
let s = await state();
check(/Overview · 5 steps/.test(s.pos) && !s.msg.length, "starts at the overview, no message", s);
await step(".dp-story-next");
s = await state();
check(s.msg.join() === "f0>e4", "step 1 lights Beta's out edge", s);
check(s.lit.includes("f0") && s.lit.includes("e4") && !s.lit.includes("f1"), "its two ends stay lit, the rest dims", s);
check(/^Iteration 1 · message 1: Beta → θ \(out\); θ after iteration 1: /.test(s.cap), "the caption names the message and θ's marginal", s);
await step(".dp-story-next");
s = await state();
check(s.msg.join() === "e4>f1 back", "step 2: a message against the drawn edge", s);
await step(".dp-story-prev");
await step(".dp-story-prev");
s = await state();
check(!s.msg.length && /Overview/.test(s.pos), "back at the overview, no message lit", s);
check(!errors.length, "no page errors", errors);
await browser.close();
if (bad) {
  console.error(`factor-graph-e2e: ${bad} failed`);
  process.exit(1);
}
console.log("factor-graph-e2e: all checks passed");
