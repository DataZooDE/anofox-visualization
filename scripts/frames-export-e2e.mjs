// Browser e2e for the ::FRAME export (M2f-B, #149): downloadHtml() embeds
// every frame of a time map (within the 3 MB/map budget, plan §M2f-B) as
// sibling <template data-frame="k"> elements next to its .panel-svg, and
// the exported file's own snapshotMotion() wires a swap-only player
// (anim.js's frames()/tween(), app.js's wireHits() re-run on the fresh
// marks) — so the export's time map still plays offline, not just a
// "N frames — Play in the live dashboard" note (M2f-A's own fallback,
// kept here for a map over budget).
//
//   cargo build --lib --target wasm32-unknown-unknown --profile wasm --no-default-features --features wasm
//   wasm-bindgen --target web --out-dir web/pkg <target>/wasm32-unknown-unknown/wasm/anofox_visualization.wasm
//   wasm-opt -O web/pkg/anofox_visualization_bg.wasm -o web/pkg/anofox_visualization_bg.wasm
//   python3 serve_nocache.py 8793 &
//   NODE_PATH=<dir with playwright>/node_modules node scripts/frames-export-e2e.mjs \
//       [--url http://127.0.0.1:8793/]
import { createRequire } from "module";
import { readFileSync } from "fs";
import path from "path";

const require = createRequire(import.meta.url);
const { chromium } = require("playwright");
const arg = (k, d) => {
  const i = process.argv.indexOf(k);
  return i > 0 ? process.argv[i + 1] : d;
};
const base = arg("--url", "http://127.0.0.1:8793/");
const fail = (m) => {
  console.error("FAIL: " + m);
  process.exitCode = 1;
};
const ok = (m) => console.log("ok: " + m);
const b64 = (s) => Buffer.from(s, "utf8").toString("base64");

const sqlPath = path.join(path.dirname(new URL(import.meta.url).pathname), "..", "web", "examples", "delivery-coverage.sql");
const SQL = readFileSync(sqlPath, "utf8");

async function openMap(page) {
  await page.locator(".tabbar button.tab-btn", { hasText: "On time" }).click();
  await page.waitForTimeout(150);
  await page.locator('.dp-viewbar [role="tab"]', { hasText: "Week by week" }).click();
  await page.waitForTimeout(150);
  await page.waitForFunction(() => !!document.querySelector(".dash .panel-svg > svg[data-frames]"), null, { timeout: 60000 });
}

async function run() {
  const browser = await chromium.launch();
  const page = await browser.newPage({ viewport: { width: 1200, height: 900 }, acceptDownloads: true });
  const liveErrs = [];
  page.on("pageerror", (e) => liveErrs.push(String(e)));
  await page.goto(`${base}?embed=1#sql=${b64(SQL)}`);
  await page.waitForFunction(() => !document.body.classList.contains("loading"), null, { timeout: 120000 });
  await openMap(page);
  check(!liveErrs.length, "no page errors building the live dashboard " + liveErrs.join(" | "));

  const subtitleOf = (p) =>
    p.evaluate(() => {
      const svg = document.querySelector(".dash svg[data-frames]");
      const name = svg?.getAttribute("data-frame-name") || "";
      return [...(svg?.querySelectorAll("text") || [])].find((t) => t.textContent.startsWith(name + ":"))?.textContent || "";
    });
  const liveLabel = await subtitleOf(page);
  ok(`live: subtitle reads "${liveLabel}" (last frame, D12)`);

  // Download the real, self-contained export (the actual downloadHtml() —
  // not a re-simulation). The toolbar (and so #dlhtml) is hidden by
  // ?embed=1 (needed above so the dashboard auto-runs from the #sql hash),
  // so this clicks it programmatically rather than requiring it visible.
  const [dl] = await Promise.all([page.waitForEvent("download"), page.evaluate(() => document.getElementById("dlhtml").click())]);
  const html = readFileSync(await dl.path(), "utf8");

  // 15 states × 12 weeks is well under the 3 MB budget: every frame must be
  // embedded as a <template>, no fallback note for this panel.
  const tplCount = (html.match(/<template data-frame="\d+">/g) || []).length;
  check(tplCount === 12, `export embeds 12 <template data-frame> frames (found ${tplCount})`);
  check(!/<p class="dp-frames-note">/.test(html), "export has no fallback note when the map is within budget");
  check(!/<div class="dp-frames"/.test(html), "the export's markup carries no live bar of its own (built offline, by snapshotMotion())");

  // Open the export cold, offline, with no server, no live app.js and no wasm.
  const off = await browser.newContext({ offline: true, viewport: { width: 1200, height: 900 } });
  const op = await off.newPage();
  const exportErrs = [];
  op.on("pageerror", (e) => exportErrs.push(String(e)));
  await op.route("**/*", (r) => (r.request().url().startsWith("http") ? (r.abort(), undefined) : r.continue()));
  await op.setContent(html, { waitUntil: "load" });

  const bar = await op.$(".dash .dp-frames");
  check(!!bar, "export: the offline player bar built itself (snapshotMotion()'s K.frames)");
  const pos0 = await op.locator(".dp-frames-pos").textContent();
  check(/12 of 12/.test(pos0 || ""), `export: starts on the last frame ("${pos0}")`);

  const fillBefore = await op.evaluate(() => document.querySelector(".dash svg[data-frames] path[data-x]")?.getAttribute("fill"));
  const titleBefore = await subtitleOf(op);
  await op.click(".dp-frames-prev");
  await op.waitForTimeout(400); // the step's own swap + tween
  const pos1 = await op.locator(".dp-frames-pos").textContent();
  check(/11 of 12/.test(pos1 || ""), `export: ‹ steps back offline (position "${pos1}")`);
  const titleAfter = await subtitleOf(op);
  check(titleBefore !== titleAfter, `export: the subtitle changed on step (before "${titleBefore}", after "${titleAfter}")`);
  const fillAfter = await op.evaluate(() => document.querySelector(".dash svg[data-frames] path[data-x]")?.getAttribute("fill"));
  check(fillBefore !== fillAfter || titleBefore !== titleAfter, "export: the swap actually replaced the map's own markup, not a no-op");

  // The legend's class boundaries (D21) must still be the shared ones
  // baked into every embedded frame — comparing week 12's (first loaded)
  // against week 11's (just swapped in) must show the same breaks.
  const classesOf = async () =>
    op.evaluate(() => [...document.querySelectorAll(".dash svg[data-frames] text")].map((t) => t.textContent).filter((t) => /–|—|-/.test(t) && /\(\d+\)/.test(t)).map((t) => t.replace(/\(\d+\)/, "")));
  const classesAfter = await classesOf();

  // Hover still works on the swapped-in marks (wireHits() re-run per step,
  // app.js) — a .dp-hit element on the fresh svg carries a working tooltip.
  const hoverWorks = await op.evaluate(() => {
    const mark = document.querySelector(".dash svg[data-frames] .dp-hit");
    if (!mark) return null;
    mark.dispatchEvent(new MouseEvent("mouseenter"));
    const shown = document.getElementById("dp-tip")?.classList.contains("show");
    mark.dispatchEvent(new MouseEvent("mouseleave"));
    return shown;
  });
  check(hoverWorks === true, `export: hover re-wires on the swapped-in marks (dp-tip shown: ${hoverWorks})`);

  // Slider to frame 0, Play/Pause, same shape as the live player (anim.js's
  // frames() itself — this just confirms it still runs with no wasm).
  const range = await op.$(".dp-frames input[type=range]");
  await range.fill("0");
  await range.dispatchEvent("input");
  await op.waitForTimeout(400);
  const pos2 = await op.locator(".dp-frames-pos").textContent();
  check(/1 of 12/.test(pos2 || ""), `export: slider → 0 (position "${pos2}")`);
  const classes0 = await classesOf();
  check(JSON.stringify(classes0) === JSON.stringify(classesAfter), "export: legend class boundaries unchanged between embedded frames (D21)");

  await op.click(".dp-frames-play");
  await op.waitForTimeout(2200);
  const posPlaying = await op.locator(".dp-frames-pos").textContent();
  check(posPlaying !== pos2, "export: Play advances offline too");
  await op.click(".dp-frames-play"); // Pause before closing

  check(!exportErrs.length, "export: no page errors stepping through the offline player " + exportErrs.join(" | "));
  await browser.close();
}

function check(c, m) {
  if (c) ok(m);
  else fail(m);
}

await run();
if (!process.exitCode) console.log("frames-export-e2e: ok");
