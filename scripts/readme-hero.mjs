// The README's screenshot (docs/img/readme-hero-{light,dark}.png): the
// period-KPI example (web/examples/period-kpis.sql) without its explanation
// box, over the last 12 months by month against the year before, in the
// default theme — the dashboard alone (no app around it), at 2× so it stays
// sharp at GitHub's column width.
//
//   python3 serve_nocache.py 8799 &   (from web/)
//   NODE_PATH=<dir with playwright>/node_modules node scripts/readme-hero.mjs [--url http://127.0.0.1:8799/]
import { createRequire } from "module";
import { readFileSync } from "fs";

const require = createRequire(import.meta.url);
const { chromium } = require("playwright");
const arg = (k, d) => {
  const i = process.argv.indexOf(k);
  return i > 0 ? process.argv[i + 1] : d;
};
const base = arg("--url", "http://127.0.0.1:8799/");
const out = arg("--out", "docs/img");
const WIDTH = 1200; // CSS px; the PNG is twice as wide

// the example as it ships, less its "What this shows" box (and the ::COL before it)
const sql = readFileSync("web/examples/period-kpis.sql", "utf8")
  .split("\n")
  .filter((l, i, all) => !/^SELECT 'One \*\*::PERIOD\*\* input/.test(l) && !(l === "SELECT 12::COL;" && /^SELECT 'One \*\*::PERIOD/.test(all[i + 1] || "")))
  .join("\n")
  .replace("'Period & KPIs — this period vs the previous one'::LABEL", "'Sales this year against last'::LABEL");
const b64 = Buffer.from(sql, "utf8").toString("base64");

const browser = await chromium.launch();
for (const scheme of ["light", "dark"]) {
  const ctx = await browser.newContext({ viewport: { width: WIDTH, height: 1000 }, deviceScaleFactor: 2, colorScheme: scheme, reducedMotion: "reduce" });
  const page = await ctx.newPage();
  await page.goto(`${base}?mode=view&v.p=12m&v.p_grain=month&v.p_compare=yoy${scheme === "dark" ? "&theme=dark" : ""}#sql=${b64}`);
  await page.waitForFunction(() => /\d+ panels?/.test(document.getElementById("status")?.textContent || "") && !document.body.classList.contains("loading"), null, { timeout: 180000 });
  await page.waitForTimeout(2500);
  // the dashboard alone: no app bar, toolbar or sidebar
  await page.addStyleTag({ content: "header,#side,.side,aside,.toolbar,#toolbar,.dp-a11y-bar{display:none!important}" });
  await page.waitForTimeout(1500);
  // cropped to what the dashboard draws, with a margin of its own background
  const r = await page.evaluate(() => {
    const dash = document.querySelector(".dash");
    const d = dash.getBoundingClientRect();
    const parts = [...dash.querySelectorAll(".panel, .controls, .dp-autobar, .dp-label, h1, h2")].map((e) => e.getBoundingClientRect()).filter((b) => b.width && b.top < d.top + 900);
    const pad = 22;
    const x0 = Math.min(...parts.map((b) => b.left)) - pad, x1 = Math.max(...parts.map((b) => b.right)) + pad;
    const y0 = d.top, y1 = Math.max(...parts.map((b) => b.bottom)) + pad;
    return { x: Math.max(d.left, x0), y: y0 + scrollY, width: Math.min(d.right, x1) - Math.max(d.left, x0), height: y1 - y0 };
  });
  const file = `${out}/readme-hero-${scheme}.png`;
  await page.screenshot({ path: file, clip: r, fullPage: true });
  console.log("wrote", file);
  await ctx.close();
}
await browser.close();
