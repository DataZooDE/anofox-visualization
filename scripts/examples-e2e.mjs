// Every sidebar example in a real browser (#891): open it, visit every tab
// and sub-tab, and fail on what a viewer would see broken — an error card, a
// page error, a chart with no data marks, or a chart whose data marks are
// all uninked (#872: a ::SMOOTH lost its points above ~100 rows; #874: a
// gallery table failed to bind), or an empty result on the default view
// (#995: the forecast backtests). The optional-data card of an example that
// demonstrates `-- @requires` ("Not available") is fine.
//
//   python3 serve_nocache.py 8947 &
//   NODE_PATH=<playwright>/node_modules node scripts/examples-e2e.mjs \
//       [--url http://127.0.0.1:8947/] [--only id,id] [--shard i/n]
import { createRequire } from "module";

const require = createRequire(import.meta.url);
const { chromium } = require("playwright");
const arg = (k, d) => {
  const i = process.argv.indexOf(k);
  return i > 0 ? process.argv[i + 1] : d;
};
const base = arg("--url", "http://127.0.0.1:8947/");
const only = arg("--only", "");
const [shard, shards] = arg("--shard", "0/1").split("/").map(Number);
let bad = 0;
const fail = (m) => {
  console.error("FAIL: " + m);
  bad++;
};

const browser = await chromium.launch();
const ctx = await browser.newContext({ viewport: { width: 1400, height: 1000 }, reducedMotion: "reduce" });
const idle = (page) =>
  page
    .waitForFunction(
      () => !document.body.classList.contains("loading") && !document.querySelector(".dash .panel.dp-skel, .dash .panel.placeholder"),
      null,
      { timeout: 180000 }
    )
    .then(() => page.waitForTimeout(300));

// What's broken on the visible part of the page.
const problems = (page) =>
  page.evaluate(() => {
    const out = [];
    const vis = (el) => el.offsetParent !== null;
    const title = (p) => p.querySelector(".panel-title")?.textContent.trim().slice(0, 60) || "(untitled)";
    for (const p of [...document.querySelectorAll(".dash .panel")].filter(vis)) {
      if (p.classList.contains("dp-errcard")) out.push(`error card: ${title(p)}: ${p.textContent.trim().slice(0, 200)}`);
      // #995: an example's default view shows data — an empty result there
      // (a backtest join that matched nothing) is broken, not "no data"
      const empty = /\bNo rows\b|No data for this selection|\b0 rows\b/.exec(p.innerText);
      if (empty) out.push(`empty panel ("${empty[0]}"): ${title(p)}`);
      const svg = p.querySelector(".panel-svg > svg");
      if (!svg || svg.hasAttribute("data-diagram")) continue;
      const marks = [...svg.querySelectorAll("circle[data-geom='point'], rect.gg-mark, path.gg-mark, polyline.gg-mark, polygon.gg-mark, .dp-hit")];
      if (!marks.length) continue; // a gauge, an image, a note …
      const inked = marks.some((m) => {
        const cs = getComputedStyle(m);
        const fo = parseFloat(m.getAttribute("fill-opacity") ?? cs.fillOpacity ?? "1");
        const so = parseFloat(m.getAttribute("stroke-opacity") ?? cs.strokeOpacity ?? "1");
        const fill = m.getAttribute("fill") ?? cs.fill;
        const stroke = m.getAttribute("stroke") ?? cs.stroke;
        return (fill && fill !== "none" && fo > 0) || (stroke && stroke !== "none" && so > 0);
      });
      if (!inked) out.push(`chart with every data mark uninked: ${title(p)}`);
      // Uninked point marks are only right as a dense line's markers (ggplot-rs
      // PointVisibility::AUTO): with no line-type layer, data went missing.
      const geoms = new Set([...svg.querySelectorAll("[data-geom]")].map((m) => m.getAttribute("data-geom")));
      const lineLike = ["line", "path", "step", "area", "ribbon", "density"].some((g) => geoms.has(g));
      const hiddenPts = [...svg.querySelectorAll("circle[data-geom='point']")].filter((c) => parseFloat(c.getAttribute("fill-opacity") ?? "1") === 0);
      if (hiddenPts.length && !lineLike) out.push(`${hiddenPts.length} data points uninked with no line to carry them: ${title(p)}`);
    }
    return out;
  });

const page0 = await ctx.newPage();
await page0.goto(base);
await page0.waitForSelector(".side-item[data-key^='ex:']", { timeout: 120000 });
let ids = await page0.$$eval(".side-item[data-key^='ex:']", (b) => [...new Set(b.map((x) => x.dataset.key.slice(3)))]);
await page0.close();
if (only) ids = ids.filter((i) => only.split(",").includes(i));
ids = ids.filter((_, i) => i % shards === shard);
if (!ids.length) fail("no examples found in the sidebar");

for (const id of ids) {
  const t0 = Date.now();
  const before = bad;
  const page = await ctx.newPage();
  const errs = [];
  page.on("pageerror", (e) => errs.push(String(e).slice(0, 300)));
  let views = 0;
  try {
    await page.goto(`${base}?dashboard=${encodeURIComponent(id)}`);
    await page.waitForSelector(".dash .panel", { timeout: 180000 });
    await idle(page);
    const check = async (label) => {
      views++;
      for (const p of await problems(page)) fail(`${id} › ${label}: ${p}`);
    };
    const top = page.locator(".tabbar .tab-btn:not(.subtab-btn)");
    const n = await top.count();
    for (let i = 0; i < Math.max(n, 1); i++) {
      let label = "(page)";
      if (n) {
        await top.nth(i).click();
        await idle(page);
        label = (await top.nth(i).textContent()).trim();
      }
      await check(label);
      const subs = page.locator(".subtab-btn:visible");
      const m = await subs.count();
      for (let k = 1; k < m; k++) {
        await subs.nth(k).click();
        await idle(page);
        await check(`${label} › ${(await subs.nth(k).textContent()).trim()}`);
      }
    }
  } catch (e) {
    fail(`${id}: ${String(e).split("\n")[0]}`);
  }
  for (const e of errs) fail(`${id}: page error: ${e}`);
  console.log(`${bad > before ? "FAIL" : "ok  "} ${id} (${views} view${views === 1 ? "" : "s"}, ${((Date.now() - t0) / 1000).toFixed(0)} s)`);
  await page.close();
}
await browser.close();
if (bad) {
  console.error(`\n${bad} problem(s)`);
  process.exit(1);
}
console.log(`\nexamples-e2e: ok (${ids.length} examples)`);
