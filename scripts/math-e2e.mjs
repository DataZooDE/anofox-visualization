// TeX math in ::MARKDOWN in a real browser (issue #61): KaTeX is loaded only
// when a dashboard has a formula, typesets each $…$ / $$…$$ with MathML for
// screen readers in the theme's text colour, and the "⤓ HTML" export draws
// the formulas with the network blocked (embedded CSS + fonts).
//
//   python3 serve_nocache.py 8791 &
//   NODE_PATH=<dir with playwright>/node_modules node scripts/math-e2e.mjs \
//       [--url http://127.0.0.1:8791/] [--dashboard "Will any substation exceed its capacity next week?"] \
//       [--tab Backtest] [--theme dark|light]
import { createRequire } from "module";
import { readFileSync } from "fs";

const require = createRequire(import.meta.url);
const { chromium } = require("playwright");
const arg = (k, d) => {
  const i = process.argv.indexOf(k);
  return i > 0 ? process.argv[i + 1] : d;
};
const base = arg("--url", "http://127.0.0.1:8791/");
const dash = arg("--dashboard", "Will any substation exceed its capacity next week?");
const tab = arg("--tab", "Backtest");
const theme = arg("--theme", "");
const fail = (m) => {
  console.error("FAIL: " + m);
  process.exitCode = 1;
};
const ready = (page) =>
  page.waitForFunction(
    () =>
      /\d+ panels?/.test(document.getElementById("status")?.textContent || "") &&
      !document.body.classList.contains("loading"),
    null,
    { timeout: 240000 }
  );

const browser = await chromium.launch();
const ctx = await browser.newContext({ acceptDownloads: true, viewport: { width: 1400, height: 1000 }, reducedMotion: "reduce" });
const page = await ctx.newPage();
const katexReqs = [];
page.on("request", (r) => /katex/.test(r.url()) && katexReqs.push(r.url()));
page.on("pageerror", (e) => fail("page error: " + e.message));

// A dashboard without formulas never fetches KaTeX.
await page.goto(`${base}?dashboard=Overview`);
await ready(page);
if (katexReqs.length) fail("KaTeX fetched for a dashboard without math: " + katexReqs.join(", "));

const q = new URLSearchParams({ dashboard: dash, tab });
if (theme) q.set("theme", theme);
await page.goto(`${base}?${q}`);
await ready(page);
await page.waitForSelector(".md-math-done .katex", { timeout: 30000 }).catch(() => fail("no typeset formula"));
const info = await page.evaluate(() => {
  const els = [...document.querySelectorAll(".md-math")];
  const k = document.querySelector(".md-math-done .katex-html");
  const body = k?.closest(".md-body");
  return {
    total: els.length,
    done: els.filter((e) => e.classList.contains("md-math-done")).length,
    err: els.filter((e) => e.classList.contains("md-math-err")).map((e) => e.dataset.tex + ": " + e.title),
    mathml: document.querySelectorAll(".md-math-done math").length,
    hidden: k?.getAttribute("aria-hidden"),
    display: document.querySelectorAll(".md-math-display .katex-display").length,
    color: k && getComputedStyle(k).color,
    ink: body && getComputedStyle(body).color,
  };
});
console.log(JSON.stringify(info));
if (!info.total || info.done !== info.total) fail(`typeset ${info.done}/${info.total}: ${info.err.join("; ")}`);
if (info.mathml !== info.done) fail("a formula without MathML");
if (info.hidden !== "true") fail("the visual KaTeX HTML is not aria-hidden");
if (!info.display) fail("no display formula");
if (info.color !== info.ink) fail(`formula colour ${info.color} ≠ text colour ${info.ink}`);

// The HTML export draws the formulas offline.
const [dl] = await Promise.all([page.waitForEvent("download"), page.click("#dlhtml")]);
const html = readFileSync(await dl.path(), "utf8");
if (!/@font-face\{[^}]*KaTeX_Main[^}]*data:font\/woff2;base64,/.test(html)) fail("export lacks the embedded KaTeX fonts");
if (/cdn\.jsdelivr\.net\/npm\/katex/.test(html)) fail("export still references the KaTeX CDN");
const off = await browser.newContext({ offline: true, viewport: { width: 1400, height: 1000 } });
const op = await off.newPage();
const blocked = [];
await op.route("**/*", (r) => (r.request().url().startsWith("http") ? (blocked.push(r.request().url()), r.abort()) : r.continue()));
await op.setContent(html, { waitUntil: "load" });
await op.evaluate(() => document.fonts.ready);
const exp = await op.evaluate(async () => {
  const pane = document.querySelector(".md-math-done")?.closest(".tabpane");
  if (pane) pane.style.display = "";
  await document.fonts.load('1em "KaTeX_Main"');
  return {
    katex: document.querySelectorAll(".md-math-done .katex-html").length,
    font: document.fonts.check('1em "KaTeX_Main"') && [...document.fonts].some((f) => f.family.includes("KaTeX_Main") && f.status === "loaded"),
  };
});
console.log("export:", JSON.stringify(exp), blocked.length ? "blocked: " + blocked.slice(0, 3).join(", ") : "");
if (!exp.katex) fail("export has no typeset formula");
if (!exp.font) fail("export's KaTeX font did not load offline");
if (blocked.some((u) => /katex/.test(u))) fail("export fetched KaTeX");

await browser.close();
if (!process.exitCode) console.log("math-e2e: ok");
