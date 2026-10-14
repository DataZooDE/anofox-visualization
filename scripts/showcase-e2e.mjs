// Headless check of a builder example in a real browser: load it, wait for
// every panel, fail on any error panel / error SVG, and screenshot each tab.
//
//   python3 serve_nocache.py 8791 &          # serves web/ (after wasm-pack build
//                                            # + scripts/fetch-wasm-extensions.sh)
//   NODE_PATH=<dir with playwright>/node_modules node scripts/showcase-e2e.mjs \
//       [--url http://127.0.0.1:8791/] [--dashboard "Statistics showcase: property market (live anofox-statistics)"] \
//       [--out ~/.cache/stats-showcase]
import { createRequire } from "module";
import { mkdirSync } from "fs";
import path from "path";
import os from "os";

const require = createRequire(import.meta.url);
const { chromium } = require("playwright");
const arg = (k, d) => {
  const i = process.argv.indexOf(k);
  return i > 0 ? process.argv[i + 1] : d;
};
const base = arg("--url", "http://127.0.0.1:8791/");
const dash = arg("--dashboard", "Statistics showcase: property market (live anofox-statistics)");
const out = arg("--out", path.join(os.homedir(), ".cache/stats-showcase"));
mkdirSync(out, { recursive: true });

const browser = await chromium.launch();
const page = await browser.newPage({ viewport: { width: 1440, height: 1000 }, reducedMotion: "reduce" });
const logs = [];
page.on("console", (m) => logs.push(`[${m.type()}] ${m.text()}`));
page.on("pageerror", (e) => logs.push(`[pageerror] ${e.message}`));
const t0 = Date.now();
await page.goto(`${base}?dashboard=${encodeURIComponent(dash)}`);
// Done when the status shows the panel count and the page is no longer loading.
try {
  await page.waitForFunction(
    () =>
      /\d+ panels?/.test(document.getElementById("status")?.textContent || "") &&
      !document.body.classList.contains("loading"),
    null,
    { timeout: Number(arg("--timeout", "240000")) }
  );
} catch (e) {
  console.log(`timed out; status: ${await page.textContent("#status")}`);
  console.log(logs.slice(-30).join("\n"));
  await page.screenshot({ path: path.join(out, "timeout.png"), fullPage: true });
  await browser.close();
  process.exit(1);
}
console.log(`loaded in ${((Date.now() - t0) / 1000).toFixed(1)}s: ${await page.textContent("#status")}`);
await page.click("#mode-view").catch(() => {});
await page.evaluate(() => document.body.classList.add("side-collapsed"));
const slug = (s) => s.replace(/[^A-Za-z0-9]+/g, "-").replace(/^-|-$/g, "").toLowerCase();
const problems = [];
const tabs = await page.$$eval(".tabbar > .tab-btn", (bs) => bs.map((b) => b.textContent));
const shoot = async (name) => {
  // errors: error boxes, error SVGs (red text) and "ships with the next release" notes
  const found = await page.evaluate(() => {
    const vis = (e) => e.offsetParent !== null;
    const errs = [...document.querySelectorAll(".dash .err, .dash pre")].filter(vis).map((e) => e.textContent.slice(0, 300));
    const svgErr = [...document.querySelectorAll(".dash .panel svg text[fill='#b42318']")]
      .filter((e) => vis(e.closest(".panel")))
      .map((e) => e.textContent.slice(0, 300));
    const notes = [...document.querySelectorAll(".dash .panel svg text")]
      .filter((e) => vis(e.closest(".panel")) && /next anofox-visualization release/.test(e.textContent))
      .map((e) => e.textContent);
    const panels = [...document.querySelectorAll(".dash .panel")].filter(vis).length;
    return { errs, svgErr, notes, panels };
  });
  for (const e of [...found.errs, ...found.svgErr]) problems.push(`${name}: ${e}`);
  for (const n of found.notes) console.log(`  note (${name}): ${n}`);
  const file = path.join(out, `${slug(name)}.png`);
  await page.screenshot({ path: file, fullPage: true });
  console.log(`${name}: ${found.panels} panels → ${file}`);
};
if (!tabs.length) await shoot("dashboard");
for (const t of tabs) {
  await page.click(`.tabbar > .tab-btn:text-is("${t}")`);
  await page.waitForTimeout(400);
  const subs = await page.$$eval(".tabpane:not([style*='none']) .subtab-btn", (bs) => bs.map((b) => b.textContent));
  if (!subs.length) await shoot(t);
  for (const s of subs) {
    await page.click(`.tabpane:not([style*='none']) .subtab-btn:text-is("${s}")`);
    await page.waitForTimeout(300);
    await shoot(`${t} - ${s}`);
  }
}
await browser.close();
const bad = logs.filter((l) => /\[(error|pageerror)\]/.test(l));
if (bad.length) console.log("console errors:\n  " + bad.join("\n  "));
if (problems.length) {
  console.log("PANEL ERRORS:\n  " + problems.join("\n  "));
  process.exit(1);
}
console.log("no panel errors");
