// Reference-rule sweep in a real browser: open every tab of a dashboard and
// hover each reference rule on screen (::XLINE, ::REFLINE, thresholds —
// `data-rule`) with the real pointer. A chart with the axis crosshair must
// list it as a "Reference line" row, any other chart show the rule's own
// tooltip; a rule is never a keyboard stop. a11y-e2e only sees the first tab.
//
//   python3 serve_nocache.py 8791 &
//   NODE_PATH=<dir with playwright>/node_modules node scripts/rules-e2e.mjs \
//       [--url http://127.0.0.1:8791/] [--dashboard "Chart gallery" | --names names.txt] [--jobs 4]
//
// --names f   one dashboard name per line (empty lines skipped)
import { createRequire } from "module";
import { readFileSync } from "fs";

const require = createRequire(import.meta.url);
const { chromium } = require("playwright");
const arg = (k, d) => {
  const i = process.argv.indexOf(k);
  return i > 0 ? process.argv[i + 1] : d;
};
const base = arg("--url", "http://127.0.0.1:8791/");
const namesFile = arg("--names", "");
const names = namesFile
  ? readFileSync(namesFile, "utf8").split("\n").map((s) => s.trim()).filter(Boolean)
  : [arg("--dashboard", "Chart gallery")];
const jobs = Math.max(1, Number(arg("--jobs", "4")) || 1);

const browser = await chromium.launch();
let total = 0;
const bad = [];

async function sweep(dash) {
  const page = await browser.newPage({ viewport: { width: 1400, height: 1000 }, reducedMotion: "reduce" });
  const errs = [];
  page.on("pageerror", (e) => errs.push(String(e)));
  await page.goto(`${base}?embed=1&dashboard=${encodeURIComponent(dash)}`);
  await page.waitForFunction(
    () =>
      document.querySelector(".dash .panel-svg > svg[aria-label]") ||
      (document.querySelector(".dash .panel") && !document.querySelector(".dash .dp-skel")),
    null,
    { timeout: 120000 }
  );
  await page.waitForTimeout(800);
  const tabs = await page.locator(".tab-btn").count();
  const lines = [];
  for (let t = 0; t < Math.max(1, tabs); t++) {
    if (tabs) {
      await page.locator(".tab-btn").nth(t).evaluate((e) => e.click());
      // lazy tabs: the tab's panels render once it is shown
      await page.waitForFunction(() => ![...document.querySelectorAll(".dash .dp-lazy")].some((e) => e.offsetParent !== null), null, { timeout: 120000 });
      await page.waitForTimeout(1500);
    }
    const rules = page.locator(".dash .panel-svg > svg [data-rule]");
    for (let i = 0, n = await rules.count(); i < n; i++) {
      // Rules of the tabs not shown have no box: skipped.
      const g = await rules.nth(i).evaluate((e) => {
        const r0 = e.getBoundingClientRect();
        if (!r0.width && !r0.height) return null;
        e.closest(".panel").scrollIntoView({ block: "center" });
        const r = e.getBoundingClientRect();
        const cross = !!e.closest(".has-axis-pointer");
        return {
          x: r.left + r.width / 2,
          y: r.top + r.height / 2,
          want: cross ? "Reference line" : e.getAttribute("data-tip"),
          focusable: e.hasAttribute("tabindex"),
          title: e.closest(".panel").querySelector(".panel-title")?.textContent?.trim().slice(0, 50) || "",
        };
      });
      if (!g) continue;
      await page.mouse.move(2, 2);
      await page.waitForTimeout(80);
      await page.mouse.move(g.x, g.y, { steps: 3 });
      await page.waitForTimeout(250);
      const tip = await page.evaluate(() => {
        const t = document.getElementById("dp-tip");
        return t?.classList.contains("show") ? t.textContent : "";
      });
      total++;
      const why = !tip.includes(g.want) ? `no "${g.want}" in the tooltip "${tip.slice(0, 80)}"` : g.focusable ? "a keyboard stop" : "";
      if (why) bad.push(`${dash} / ${g.title}: ${why}`);
      lines.push(`${why ? "FAIL" : "ok  "} ${g.title}: ${tip.slice(0, 80)}`);
    }
  }
  console.log(`${dash} (${tabs} tabs, ${lines.length} rules)` + lines.map((l) => "\n    " + l).join(""));
  if (errs.length) bad.push(`${dash}: page errors: ${errs.join(" | ")}`);
  await page.close();
}

const queue = [...names];
await Promise.all(
  Array.from({ length: Math.min(jobs, queue.length) }, async () => {
    while (queue.length) {
      const dash = queue.shift();
      await sweep(dash).catch((e) => bad.push(`${dash}: ${String(e.message || e).split("\n")[0]}`));
    }
  })
);
await browser.close();
for (const b of bad) console.error("FAIL: " + b);
console.log(`rules-e2e: ${total} rules on ${names.length} dashboards, ${bad.length} failed`);
if (bad.length) process.exitCode = 1;
