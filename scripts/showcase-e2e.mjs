// Headless check of a builder example in a real browser: load it, wait for
// every panel, fail on any error panel / error SVG, and screenshot each tab.
//
//   python3 serve_nocache.py 8791 &          # serves web/ (after wasm-pack build
//                                            # + scripts/fetch-wasm-extensions.sh)
//   NODE_PATH=<dir with playwright>/node_modules node scripts/showcase-e2e.mjs \
//       [--url http://127.0.0.1:8791/] [--dashboard "What sets the price of a home?"] \
//       [--out ~/.cache/stats-showcase] [--theme dark|light] [--sql file.sql] [--query "&k=v"]
//       [--width 390]
//
// --theme dark   view with the viewer's dark preference (?theme=dark) and fail
//                if a panel (card or chart SVG) still has a white background
// --sql f.sql    load a dashboard from a file instead of a built-in example
//                (passed in the #sql hash, like a Share link)
// --width 390   view the builder at that viewport width (default 1440, where
//                the sidebar is collapsed) and fail if a chart's toolbox
//                (shown or not) covers its title; on phones (≤820 px) also
//                if the page scrolls sideways or the sidebar squeezes the
//                dashboard (it must be a closed drawer), and (≤760 px, where
//                charts are drawn at their shown width) if a chart's largest
//                text is below 8 px (ggplot-rs draws axis and legend text at
//                8.8 px; less means the chart was rendered wider than shown),
//                or if an x tick label is clipped by its chart's edge or
//                overlaps its neighbour (measured with getBBox)
// Every run also fails when the "Powered by DataZoo GmbH" credit is missing.
import { createRequire } from "module";
import { mkdirSync, readFileSync } from "fs";
import path from "path";
import os from "os";

const require = createRequire(import.meta.url);
const { chromium } = require("playwright");
const arg = (k, d) => {
  const i = process.argv.indexOf(k);
  return i > 0 ? process.argv[i + 1] : d;
};
const base = arg("--url", "http://127.0.0.1:8791/");
const dash = arg("--dashboard", "What sets the price of a home?");
const out = arg("--out", path.join(os.homedir(), ".cache/stats-showcase"));
const theme = arg("--theme", "");
const sqlFile = arg("--sql", "");
const extra = arg("--query", "");
const width = Number(arg("--width", "0"));
mkdirSync(out, { recursive: true });

const browser = await chromium.launch();
const page = await browser.newPage({ viewport: { width: width || 1440, height: 1000 }, reducedMotion: "reduce" });
const logs = [];
page.on("console", (m) => logs.push(`[${m.type()}] ${m.text()}`));
page.on("pageerror", (e) => logs.push(`[pageerror] ${e.message}`));
const t0 = Date.now();
const q = [theme ? `theme=${theme}` : "", extra.replace(/^&/, "")].filter(Boolean).join("&");
if (sqlFile) {
  const b64 = Buffer.from(readFileSync(sqlFile, "utf8"), "utf8").toString("base64");
  await page.goto(`${base}${q ? "?" + q : ""}#sql=${b64}`);
} else {
  await page.goto(`${base}?dashboard=${encodeURIComponent(dash)}${q ? "&" + q : ""}`);
}
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
const problems = [];
if (width && width <= 820) {
  await page.waitForTimeout(400); // charts re-fit to the view mode's panel width
  const shell = await page.evaluate(() => ({
    scroll: document.documentElement.scrollWidth,
    dash: document.querySelector(".dash").getBoundingClientRect().width,
    view: innerWidth,
  }));
  if (shell.scroll > shell.view + 1) problems.push(`builder: the page scrolls sideways (${shell.scroll} px wide)`);
  if (shell.dash < shell.view * 0.9) problems.push(`builder: the dashboard gets ${Math.round(shell.dash)} of ${shell.view} px`);
} else if (!width) await page.evaluate(() => document.body.classList.add("side-collapsed"));
const slug = (s) => s.replace(/[^A-Za-z0-9]+/g, "-").replace(/^-|-$/g, "").toLowerCase();
const tabs = await page.$$eval(".tabbar > .tab-btn", (bs) => bs.map((b) => b.textContent));
// A tab's panels run and render when it is first shown (lazy tabs): wait until
// no placeholder is left on screen and the fresh panels are wired.
const lazyDone = async () => {
  await page.waitForFunction(() => ![...document.querySelectorAll(".dash .dp-lazy")].some((e) => e.offsetParent !== null), null, {
    timeout: Number(arg("--timeout", "240000")),
  });
  await page.evaluate(() => new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r))));
};
const shoot = async (name) => {
  // errors: error boxes, error SVGs (red text) and "ships with the next release" notes
  const found = await page.evaluate((width) => {
    const vis = (e) => e.offsetParent !== null;
    const errs = [...document.querySelectorAll(".dash .err, .dash pre")].filter(vis).map((e) => e.textContent.slice(0, 300));
    const svgErr = [...document.querySelectorAll(".dash .panel svg text[fill='#b42318']")]
      .filter((e) => vis(e.closest(".panel")))
      .map((e) => e.textContent.slice(0, 300));
    const notes = [...document.querySelectorAll(".dash .panel svg text")]
      .filter((e) => vis(e.closest(".panel")) && /next anofox-visualization release/.test(e.textContent))
      .map((e) => e.textContent);
    const panels = [...document.querySelectorAll(".dash .panel")].filter(vis).length;
    // dark: no visible card or chart may keep a white background
    const white = (c) => /^(rgb\(255, 255, 255\)|#fff(fff)?|white)$/i.test(String(c).trim());
    const whiteCards = [...document.querySelectorAll(".dash .panel, .dash .group")]
      .filter(vis)
      .filter((e) => white(getComputedStyle(e).backgroundColor)).length;
    const whiteSvg = [...document.querySelectorAll(".dash .panel svg")]
      .filter((e) => vis(e.closest(".panel")))
      .filter((svg) => {
        const vb = svg.viewBox && svg.viewBox.baseVal;
        return [...svg.querySelectorAll("rect")].some((r) => {
          const f = r.getAttribute("fill") || "";
          const w = parseFloat(r.getAttribute("width")) || 0;
          const h = parseFloat(r.getAttribute("height")) || 0;
          return white(f) && vb && w >= vb.width * 0.9 && h >= vb.height * 0.9;
        });
      }).length;
    const credit = document.querySelector(".dash .dp-credit");
    const creditOk = !!credit && /Powered by\s*DataZoo GmbH/.test(credit.textContent) &&
      !!credit.querySelector('a[href^="https://data-zoo.de"]');
    // --width: no displayed chart tool may sit on its panel's title text, and
    // chart text must be drawn at a readable size (font size × shown / drawn
    // width; the largest, as heatmap cell and radar grid labels are drawn
    // smaller on purpose)
    const covered = [],
      tiny = [],
      clipped = [];
    if (width)
      for (const panel of document.querySelectorAll(".dash .panel")) {
        const svg = panel.querySelector(":scope > .panel-svg > svg");
        if (!vis(panel) || !svg) continue;
        const title = panel.querySelector(".panel-title");
        const name = (title?.textContent || "untitled").slice(0, 60);
        // hidden or not, a tool on the title would cover it once shown
        if (title) {
          const range = document.createRange();
          range.selectNodeContents(title);
          const text = [...range.getClientRects()];
          const hit = [...panel.querySelectorAll(".dp-toolbox .dp-tool")].some((b) =>
            [...b.getClientRects()].some((r) =>
              text.some((t) => r.left < t.right && t.left < r.right && r.top < t.bottom && t.top < r.bottom)
            )
          );
          if (hit) covered.push(name);
        }
        if (width > 760) continue; // desktop: drawn at span × 100, as before
        const scale = svg.getBoundingClientRect().width / (svg.viewBox.baseVal.width || 1);
        const px = [...svg.querySelectorAll("text")]
          .filter((t) => t.textContent.trim() && t.getClientRects().length)
          .map((t) => parseFloat(getComputedStyle(t).fontSize) * scale);
        if (px.length && Math.max(...px) < 8) tiny.push(`${name} (${Math.max(...px).toFixed(1)} px)`);
        // x tick labels (#108): each chart's runs of same-style texts on one
        // baseline below its panel; their drawn boxes must lie inside the
        // chart and not overlap
        for (const chart of [svg, ...svg.querySelectorAll("svg")]) {
          const plot = (chart.getAttribute("data-plot") || "").split(" ").map(Number);
          const vb = chart.viewBox && chart.viewBox.baseVal;
          if (plot.length < 4 || !vb || !vb.width) continue;
          const bottom = plot[1] + plot[3];
          const toChart = chart.getScreenCTM().inverse();
          const box = (t) => {
            const b = t.getBBox();
            const m = toChart.multiply(t.getScreenCTM());
            const ps = [
              [b.x, b.y],
              [b.x + b.width, b.y],
              [b.x, b.y + b.height],
              [b.x + b.width, b.y + b.height],
            ].map(([x, y]) => new DOMPoint(x, y).matrixTransform(m));
            const xs = ps.map((p) => p.x),
              ys = ps.map((p) => p.y);
            return [Math.min(...xs), Math.min(...ys), Math.max(...xs), Math.max(...ys)];
          };
          const angle = (t) => +((t.getAttribute("transform") || "").match(/rotate\(([-\d.]+)/) || [0, 0])[1];
          const key = (t) =>
            ["y", "font-size", "text-anchor", "fill"].map((k) => t.getAttribute(k)).join(" ") + " " + angle(t);
          const rows = [];
          let run = [];
          const flush = () => {
            if (run.length >= 2) rows.push(run);
            run = [];
          };
          for (const t of chart.querySelectorAll(":scope > text")) {
            const prev = run[run.length - 1];
            if (!prev || key(prev) !== key(t) || +prev.getAttribute("x") >= +t.getAttribute("x")) flush();
            if (+t.getAttribute("y") > bottom) run.push(t);
          }
          flush();
          for (const row of rows) {
            const bs = row.map(box);
            row.forEach((t, i) => {
              const [x0, y0, x1, y1] = bs[i];
              if (x0 < -0.5 || y0 < -0.5 || x1 > vb.width + 0.5 || y1 > vb.height + 0.5)
                clipped.push(`${name}: "${t.textContent}" [${bs[i].map((v) => v.toFixed(1))}] outside ${vb.width}×${vb.height}`);
            });
            for (let i = 1; i < row.length; i++) {
              const a = angle(row[i]);
              const hit = Math.abs(a) < 1
                ? bs[i - 1][2] > bs[i][0] + 0.5
                : Math.abs(+row[i].getAttribute("x") - +row[i - 1].getAttribute("x")) * Math.abs(Math.sin((a * Math.PI) / 180)) <
                  +row[i].getAttribute("font-size");
              if (hit) clipped.push(`${name}: "${row[i - 1].textContent}" overlaps "${row[i].textContent}"`);
            }
          }
        }
      }
    return { errs, svgErr, notes, panels, whiteCards, whiteSvg, creditOk, covered, tiny, clipped };
  }, width);
  for (const e of [...found.errs, ...found.svgErr]) problems.push(`${name}: ${e}`);
  for (const n of found.notes) console.log(`  note (${name}): ${n}`);
  if (theme === "dark" && found.whiteCards) problems.push(`${name}: ${found.whiteCards} white card(s) in dark mode`);
  if (theme === "dark" && found.whiteSvg) problems.push(`${name}: ${found.whiteSvg} chart(s) with a white background in dark mode`);
  for (const t of found.covered) problems.push(`${name}: the chart toolbox covers the title of "${t}"`);
  for (const t of found.tiny) problems.push(`${name}: chart text too small in "${t}"`);
  for (const t of found.clipped) problems.push(`${name}: x tick label ${t}`);
  if (!found.creditOk) problems.push(`${name}: the "Powered by DataZoo GmbH" credit is missing`);
  const file = path.join(out, `${slug(name)}.png`);
  await page.screenshot({ path: file, fullPage: true });
  console.log(`${name}: ${found.panels} panels → ${file}`);
};
if (!tabs.length) await shoot("dashboard");
for (const t of tabs) {
  await page.click(`.tabbar > .tab-btn:text-is("${t}")`);
  await page.waitForTimeout(400);
  await lazyDone();
  const subs = await page.$$eval(".tabpane:not([style*='none']) .subtab-btn", (bs) => bs.map((b) => b.textContent));
  if (!subs.length) await shoot(t);
  for (const s of subs) {
    await page.click(`.tabpane:not([style*='none']) .subtab-btn:text-is("${s}")`);
    await page.waitForTimeout(300);
    await lazyDone();
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
