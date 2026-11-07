// The Gallery section: one page per showcase/gallery/<page>.sql (inputs, KPIs,
// tables, …) — every `-- @tile Title | description` statement group drawn in
// the builder, with its SQL and an "Edit it in the builder" link — and one
// page per anofox extension from the plots its conformance run draws
// (gallery-plots.mjs).
//
//   node scripts/gen-gallery.mjs           write the pages (src/content/docs/gallery/, not committed)
//   node scripts/gen-gallery.mjs --check   every tile lints without an error (DASHBOARD, else
//                                          target/debug/dashboard) and has its pictures
//   node scripts/gen-gallery.mjs --shots [page,…]   screenshot the tiles in the builder (BUILDER,
//       default `npm run dev`'s …/next/app/) → src/assets/gallery/<page>/<n>-light|dark.webp
//   node scripts/gen-gallery.mjs --plots   copy the extension plots from a conformance run
//       (CONFORMANCE_OUT/<ext>/*.svg, default ../../scratch-showcase) → src/assets/plots/<ext>/
import { execFileSync } from "child_process";
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from "fs";
import { createRequire } from "module";
import os from "os";
import path from "path";
import { plots } from "./gallery-plots.mjs";

const site = path.resolve(import.meta.dirname, "..");
const root = path.resolve(site, "..");
const src = path.join(site, "showcase", "gallery");
const out = path.join(site, "src", "content", "docs", "gallery");
const pics = path.join(site, "src", "assets", "gallery");
const plotPics = path.join(site, "src", "assets", "plots");
const [mode, arg] = process.argv.slice(2);

// A page: its header lines, the setup before the first tile, and the tiles.
const pages = readdirSync(src).filter((f) => f.endsWith(".sql")).sort().map((f) => {
  const text = readFileSync(path.join(src, f), "utf8");
  const head = (k) => (new RegExp(`^-- @${k} (.+)$`, "m").exec(text) || [])[1]?.trim() || "";
  const parts = text.split(/^-- @tile /m);
  const setup = parts[0].split("\n").filter((l) => !/^-- @(page|lead|order) /.test(l)).join("\n").trim();
  const tiles = parts.slice(1).map((p, i) => {
    const [first, ...rest] = p.split("\n");
    // Title | description [| click: <selector> — clicked before the picture]
    const [title, desc = "", ...opts] = first.split(" | ");
    const click = (opts.find((o) => o.trim().startsWith("click:")) || "").replace(/^\s*click:\s*/, "") || null;
    return { n: i + 1, title: title.trim(), desc: desc.trim(), click, sql: rest.join("\n").trim() };
  });
  return { slug: f.replace(/\.sql$/, ""), title: head("page"), lead: head("lead"), order: Number(head("order") || 50), setup, tiles };
});
const full = (p, t) => `${p.setup}\n\n${t.sql}\n`;
const b64 = (s) => Buffer.from(s, "utf8").toString("base64");

if (mode === "--check") {
  const bin = process.env.DASHBOARD || path.join(root, "target", "debug", "dashboard");
  const tmp = mkdtempSync(path.join(os.tmpdir(), "gallery-"));
  const bad = [];
  for (const p of pages)
    for (const t of p.tiles) {
      const f = path.join(tmp, `${p.slug}-${t.n}.sql`);
      writeFileSync(f, full(p, t));
      let r;
      try { r = JSON.parse(execFileSync(bin, ["--check", f, "--json"], { encoding: "utf8" })); }
      catch (e) { r = JSON.parse(e.stdout || '{"diagnostics":[{"severity":"error","message":"no output"}]}'); }
      for (const d of r.diagnostics || []) if (d.severity === "error") bad.push(`${p.slug} › ${t.title}: ${d.code} — ${d.message.split("\n")[0]}`);
      for (const s of ["light", "dark"]) if (!existsSync(path.join(pics, p.slug, `${t.n}-${s}.webp`))) bad.push(`${p.slug} › ${t.title}: no ${s} picture (--shots)`);
    }
  for (const [ext, list] of Object.entries(plots))
    for (const x of list) if (!existsSync(path.join(plotPics, ext, x.file))) bad.push(`plots ${ext}: no ${x.file} (--plots)`);
  rmSync(tmp, { recursive: true, force: true });
  if (bad.length) { console.error(bad.join("\n")); process.exit(1); }
  console.log(`gallery: ${pages.reduce((n, p) => n + p.tiles.length, 0)} tiles ok`);
  process.exit(0);
}

if (mode === "--plots") {
  const from = process.env.CONFORMANCE_OUT || path.resolve(root, "..", "scratch-showcase");
  for (const [ext, list] of Object.entries(plots)) {
    mkdirSync(path.join(plotPics, ext), { recursive: true });
    for (const x of list) copyFileSync(path.join(from, ext, x.from), path.join(plotPics, ext, x.file));
  }
  console.log("plots copied");
  process.exit(0);
}

if (mode === "--shots") {
  const only = (arg || "").split(",").filter(Boolean);
  const { chromium } = createRequire(path.join(root, "tests", "web", "x.mjs"))("playwright");
  const builder = process.env.BUILDER || "http://localhost:4321/anofox-visualization/next/app/";
  const browser = await chromium.launch();
  for (const scheme of ["light", "dark"]) {
    const ctx = await browser.newContext({ viewport: { width: 1100, height: 800 }, deviceScaleFactor: 1.5, colorScheme: scheme, reducedMotion: "reduce" });
    for (const p of pages.filter((p) => !only.length || only.includes(p.slug))) {
      mkdirSync(path.join(pics, p.slug), { recursive: true });
      for (const t of p.tiles) {
        const page = await ctx.newPage();
        await page.goto(`${builder}?embed=1#sql=${b64(full(p, t))}`);
        await page.waitForSelector(".dash .panel", { timeout: 120000 });
        await page.waitForFunction(() => !document.body.classList.contains("loading") && !document.querySelector(".dash .panel.dp-skel, .dash .panel.placeholder"), null, { timeout: 120000 });
        if (t.click) {
          await page.locator(t.click).first().click();
          await page.waitForTimeout(2000);
          await page.waitForFunction(() => !document.querySelector(".dash .panel.dp-skel, .dash .panel.placeholder"), null, { timeout: 120000 });
        }
        await page.mouse.move(2, 2);
        await page.waitForTimeout(1200);
        // the picture: the panels and controls the tile drew, with a margin
        const box = await page.evaluate(() => {
          const els = [...document.querySelectorAll(".dash .panel, .dash .controls, .dash .dp-inputs, .dash .tabbar, .dash .kpi-strip, .dash .group, .dash .dp-viewbar, .dash .dp-step-head, .dash .dp-step-foot, .dash .dp-steppane")]
            .filter((e) => e.getBoundingClientRect().height > 0);
          const r = els.map((e) => e.getBoundingClientRect());
          const x0 = Math.min(...r.map((b) => b.left)), y0 = Math.min(...r.map((b) => b.top));
          const x1 = Math.max(...r.map((b) => b.right)), y1 = Math.max(...r.map((b) => b.bottom));
          return { x: Math.max(0, x0 - 12), y: Math.max(0, y0 - 12 + window.scrollY), width: x1 - x0 + 24, height: y1 - y0 + 24 };
        });
        const png = path.join(pics, p.slug, `${t.n}-${scheme}.png`);
        await page.screenshot({ path: png, clip: box, fullPage: true });
        execFileSync("magick", [png, "-resize", "1200x>", "-quality", "84", png.replace(/\.png$/, ".webp")]);
        rmSync(png);
        console.log(`${p.slug} ${t.n} ${t.title} (${scheme})`);
        await page.close();
      }
    }
    await ctx.close();
  }
  await browser.close();
  process.exit(0);
}

// ---- the pages (written in place: a running dev server keeps them) ----
mkdirSync(out, { recursive: true });
const written = new Set();
const put = (f, text) => {
  written.add(f);
  const p = path.join(out, f);
  if (!existsSync(p) || readFileSync(p, "utf8") !== text) writeFileSync(p, text);
};
const prose = (t) => t.split(/(`[^`]*`)/).map((s, i) => (i % 2 ? s : s.replace(/[{}]/g, "\\$&").replace(/</g, "&lt;"))).join("");
const app = "../../app/";

for (const p of pages) {
  const imp = [];
  const tiles = p.tiles.map((t) => {
    const has = existsSync(path.join(pics, p.slug, `${t.n}-light.webp`));
    if (has) imp.push(`import t${t.n}l from "../../../assets/gallery/${p.slug}/${t.n}-light.webp";`, `import t${t.n}d from "../../../assets/gallery/${p.slug}/${t.n}-dark.webp";`);
    return [
      `<div class="gal-tile">`,
      has ? `<div class="gal-pic"><img class="lp-light" src={t${t.n}l.src} alt="" loading="lazy" /><img class="lp-dark" src={t${t.n}d.src} alt="" loading="lazy" /></div>` : "",
      `<div class="gal-text">`,
      "",
      `### ${t.title}`,
      "",
      prose(t.desc),
      "",
      "```sql",
      t.sql,
      "```",
      "",
      `<a class="gal-edit" href="${app}?mode=edit#sql=${b64(full(p, t))}">Edit it in the builder ↗</a>`,
      "",
      `</div>`,
      `</div>`,
      "",
    ].join("\n");
  });
  put(`${p.slug}.mdx`, [
    "---",
    `title: ${JSON.stringify(p.title)}`,
    `description: ${JSON.stringify(p.lead.replace(/`/g, ""))}`,
    "tableOfContents: false",
    "sidebar:",
    `  order: ${p.order}`,
    "---",
    "",
    ...imp,
    "",
    prose(p.lead),
    "",
    `<details class="gal-data">`,
    `<summary>The sample data every tile on this page uses</summary>`,
    "",
    "```sql",
    p.setup,
    "```",
    "",
    "</details>",
    "",
    '<div class="gal">',
    "",
    ...tiles,
    "</div>",
    "",
  ].join("\n"));
}

// one page per extension: its plots
const extNames = { statistics: "anofox-statistics", forecast: "anofox-forecast", tabular: "anofox-tabular", optimize: "anofox-optimize", decide: "anofox-decide" };
for (const [ext, list] of Object.entries(plots)) {
  const imp = list.map((x, i) => `import p${i} from "../../../assets/plots/${ext}/${x.file}?url";`);
  const tiles = list.map((x, i) => [
    `<div class="gal-tile gal-plot">`,
    `<div class="gal-pic"><img src={p${i}} alt="" loading="lazy" /></div>`,
    `<div class="gal-text">`,
    "",
    `### ${x.title}`,
    "",
    prose(x.text),
    "",
    `<code class="gal-call">${x.call.replace(/</g, "&lt;").replace(/[{}]/g, (c) => `{"${c}"}`)}</code>`,
    "",
    `</div>`,
    `</div>`,
    "",
  ].join("\n"));
  put(`${ext}.mdx`, [
    "---",
    `title: ${JSON.stringify("What " + extNames[ext] + " draws")}`,
    `description: ${JSON.stringify(`The plots anofox-visualization draws from ${extNames[ext]}'s results — one anofox_plot call each.`)}`,
    "tableOfContents: false",
    "sidebar:",
    `  label: ${extNames[ext]}`,
    `  order: ${10 + Object.keys(plots).indexOf(ext)}`,
    "---",
    "",
    ...imp,
    "",
    `Each picture is one \`anofox_plot(...)\` call on a result table of ${extNames[ext]}, drawn in CI on every push from the extension's latest build. [Install and use it →](../../integrate/${ext}/)`,
    "",
    '<div class="gal gal-plots">',
    "",
    ...tiles,
    "</div>",
    "",
  ].join("\n"));
}
for (const f of readdirSync(out)) if (!written.has(f) && f !== "index.mdx") rmSync(path.join(out, f));
console.log(`gallery: ${pages.length} pages, ${Object.keys(plots).length} extension pages`);
