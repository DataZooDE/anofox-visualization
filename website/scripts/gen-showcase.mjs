// #1017: the landing page's pictures.
//   node scripts/gen-showcase.mjs [--only id,…]
// 1. showcase/*.sql (the "one line to an application" steps) → SVG through
//    the dashboard CLI (DASHBOARD, else target/debug/dashboard), cropped to
//    the cards → src/assets/showcase/<name>.svg
// 2. screenshots of the builder's examples, embed mode, light and dark →
//    src/assets/showcase/<id>-light.webp / -dark.webp. Needs the builder at
//    BUILDER (default: `npm run dev`'s http://localhost:4321/anofox-visualization/next/app/),
//    Playwright (tests/web) and ImageMagick (magick).
import { execFileSync } from "child_process";
import { createRequire } from "module";
import { existsSync, mkdirSync, readdirSync, readFileSync, rmSync, writeFileSync } from "fs";
import path from "path";
import { examples } from "./examples.mjs";

const site = path.resolve(import.meta.dirname, "..");
const root = path.resolve(site, "..");
const out = path.join(site, "src", "assets", "showcase");
mkdirSync(out, { recursive: true });
const only = (process.argv.find((a) => a.startsWith("--only=")) || "").slice(7).split(",").filter(Boolean);

// The examples shown on the landing page (ids as in the builder's sidebar),
// or `file`: a .sql of showcase/shots/ (the docs' own dashboards):
// `tab` opens that tab (the picture starts at the tab bar), `click` clicks an
// element first (a map region to drill into), `name` names the file (default: the id).
export const SHOTS = [
  { id: "where-does-our-revenue-come-from-from-country-down-to-district" },
  { id: "where-does-our-revenue-come-from-from-country-down-to-district", name: "map-drill-germany", click: '.dash svg path[data-label="Germany"]' },
  { id: "will-any-substation-exceed-its-capacity-next-week", tab: "Substation drill-down" },
  { id: "what-sets-the-price-of-a-home" },
  { id: "how-do-we-plan-a-busy-day-at-the-distribution-centre" },
  { id: "where-do-our-deliveries-and-customers-cluster-and-which-regions-are-underserved" },
  { id: "can-a-model-triage-the-support-inbox" },
  { id: "is-today-s-supplier-feed-safe-to-load" },
  { id: "which-chart-fits-which-question" },
  { id: "first-dashboard", file: "first-dashboard.sql" },
];

if (!only.length || only.includes("svg")) {
  const bin = process.env.DASHBOARD || path.join(root, "target", "debug", "dashboard");
  for (const f of readdirSync(path.join(site, "showcase")).filter((f) => f.endsWith(".sql")))
  for (const dark of [false, true]) {
    const svg = path.join(out, f.replace(/\.sql$/, dark ? "-dark.svg" : ".svg"));
    execFileSync(bin, ["--svg", path.join(site, "showcase", f), svg, "--width", "900", "--theme", path.join(site, "showcase", "site-theme.json"), ...(dark ? ["--dark"] : [])], { stdio: ["ignore", "ignore", "inherit"] });
    // drop the page background and crop to the cards (rx rects at the top level)
    let t = readFileSync(svg, "utf8").replace(/^(<svg [^>]*>)<rect width="[\d.]+" height="[\d.]+" fill="[^"]+"\/>/, "$1");
    const cards = [...t.matchAll(/<rect x="([\d.]+)" y="([\d.]+)" width="([\d.]+)" height="([\d.]+)" rx=/g)].map((m) => m.slice(1).map(Number));
    if (cards.length) {
      const x0 = Math.min(...cards.map((c) => c[0])), y0 = Math.min(...cards.map((c) => c[1]));
      const w = Math.max(...cards.map((c) => c[0] + c[2])) - x0, h = Math.max(...cards.map((c) => c[1] + c[3])) - y0;
      t = t.replace(/^<svg ([^>]*?)width="[\d.]+" height="[\d.]+" viewBox="[^"]+"/, `<svg $1width="${w + 2}" height="${h + 2}" viewBox="${x0 - 1} ${y0 - 1} ${w + 2} ${h + 2}"`);
    }
    writeFileSync(svg, t);
  }
}

const shots = SHOTS.filter((s) => !only.length || only.includes(s.name || s.id));
if (shots.length) {
  const { chromium } = createRequire(path.join(root, "tests", "web", "x.mjs"))("playwright");
  const builder = process.env.BUILDER || "http://localhost:4321/anofox-visualization/next/app/";
  const browser = await chromium.launch();
  for (const scheme of ["light", "dark"]) {
    const ctx = await browser.newContext({ viewport: { width: 1440, height: 900 }, deviceScaleFactor: 1.5, colorScheme: scheme, reducedMotion: "reduce" });
    for (const { id, tab, click, file, name = id } of shots) {
      const page = await ctx.newPage();
      // the example's own SQL in the site's theme (showcase/site-theme.json)
      const ex = file ? { sql: readFileSync(path.join(site, "showcase", "shots", file), "utf8") } : examples.find((e) => e.id === id);
      if (!ex) throw new Error(`no example ${id}`);
      const theme = JSON.stringify(JSON.parse(readFileSync(path.join(site, "showcase", "site-theme.json"), "utf8")));
      await page.goto(`${builder}?embed=1#sql=${Buffer.from(`SELECT '${theme}'::THEME;\n${ex.sql}`, "utf8").toString("base64")}`);
      await page.waitForSelector(".dash .panel", { timeout: 180000 });
      await page.waitForFunction(() => !document.body.classList.contains("loading") && !document.querySelector(".dash .panel.dp-skel, .dash .panel.placeholder"), null, { timeout: 180000 });
      if (tab) {
        await page.locator(".tabbar .tab-btn", { hasText: tab }).first().click();
        await page.waitForFunction(() => !document.querySelector(".dash .panel.dp-skel, .dash .panel.placeholder"), null, { timeout: 180000 });
        await page.evaluate(() => {
          const bar = document.querySelector(".tabbar");
          window.scrollTo(0, bar.getBoundingClientRect().top + window.scrollY - 16);
        });
      }
      if (click) {
        await page.locator(click).first().click();
        await page.waitForTimeout(2500);
        await page.waitForFunction(() => !document.querySelector(".dash .panel.dp-skel, .dash .panel.placeholder"), null, { timeout: 180000 });
      }
      await page.mouse.move(2, 2);
      await page.waitForTimeout(1500);
      const png = path.join(out, `${name}-${scheme}.png`);
      await page.screenshot({ path: png });
      execFileSync("magick", [png, "-resize", "1600x", "-quality", "82", png.replace(/\.png$/, ".webp")]);
      rmSync(png);
      console.log(`${name} (${scheme})`);
      await page.close();
    }
    await ctx.close();
  }
  await browser.close();
}
