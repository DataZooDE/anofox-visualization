// #1331: "Which lever lowers scrap?" in a real browser (DuckDB-Wasm, the
// bayes-net plugin from plugins.json). The network's beliefs are looked up
// from data/bn-production/beliefs.parquet by the clicks' key: a click on a
// lever's state observes it (high scrap 17 % → 13 % for a senior operator),
// Alt-click sets it with do() (16 %), and a click on a node that is not a
// lever (`settable` false) leaves the evidence alone.
//
//   scripts/build-wasm-pkg.sh && scripts/web-plugins.sh web
//   (cd web && python3 ../serve_nocache.py 8947 &)
//   NODE_PATH=<dir with playwright>/node_modules node scripts/bayes-production-e2e.mjs [--url http://127.0.0.1:8947/] [--shot out-prefix]
import { createRequire } from "module";

const require = createRequire(import.meta.url);
const { chromium } = require("playwright");
const arg = (k, d) => {
  const i = process.argv.indexOf(k);
  return i > 0 ? process.argv[i + 1] : d;
};
const base = arg("--url", "http://127.0.0.1:8947/");
const shot = arg("--shot", null);
let bad = 0;
const check = (ok, m, got) => (ok ? console.log("ok   " + m) : (bad++, console.error("FAIL " + m + (got === undefined ? "" : " " + JSON.stringify(got)))));

const browser = await chromium.launch();
const errors = [];
async function open(scheme) {
  const page = await browser.newPage({ viewport: { width: 1400, height: 1000 }, colorScheme: scheme });
  page.on("pageerror", (e) => errors.push(String(e)));
  await page.goto(base + "?dashboard=which-lever-lowers-scrap");
  await page.waitForFunction(() => document.querySelector("svg[data-diagram] rect[data-node]"), null, { timeout: 120000 });
  await page.waitForTimeout(800);
  return page;
}
const page = await open("light");

const rowAt = (node, state) =>
  page.evaluate(
    ([node, state]) => {
      const svg = document.querySelector("svg[data-diagram]");
      const t = svg.querySelector(`[data-of="${node}"] text[data-state="${state}"]`);
      if (!t) return null;
      t.scrollIntoView({ block: "center" });
      const r = t.getBoundingClientRect();
      return { x: r.left + r.width / 2, y: r.top + r.height / 2 };
    },
    [node, state]
  );
const summary = () => page.evaluate(() => document.querySelector("svg[data-diagram]")?.getAttribute("data-summary") || "");
async function clickAt(p, opts = {}) {
  const before = await summary();
  if (opts.alt) await page.keyboard.down("Alt");
  await page.mouse.click(p.x, p.y);
  if (opts.alt) await page.keyboard.up("Alt");
  if (opts.rerun === false) return page.waitForTimeout(1500);
  await page.waitForFunction((b) => (document.querySelector("svg[data-diagram]")?.getAttribute("data-summary") || "") !== b, before, { timeout: 30000 }).catch(() => {});
  await page.waitForTimeout(400);
}
const state = () =>
  page.evaluate(() => {
    const svg = document.querySelector("svg[data-diagram]");
    const vals = {};
    for (const t of svg.querySelectorAll("text.dp-node-state-value[data-state]")) vals[t.closest("[data-of]")?.getAttribute("data-of") + "." + t.getAttribute("data-state")] = t.textContent;
    const panel = svg.closest(".panel");
    return {
      nodes: svg.querySelectorAll("rect[data-node]").length,
      fixed: svg.querySelectorAll(`rect[data-bn*='"settable":false']`).length,
      vals,
      chips: [...panel.querySelectorAll(".dp-bn-chip")].map((c) => c.firstChild.textContent),
      cut: [...svg.querySelectorAll('polyline[data-cut="1"]')].map((e) => e.getAttribute("data-edge")),
    };
  });
const pct = (v) => parseFloat(String(v).replace(/[^\d.]/g, ""));

let s = await state();
check(s.nodes === 13, "thirteen nodes", s.nodes);
check(s.fixed === 7, "seven nodes are not levers (settable false)", s.fixed);
check(Math.round(pct(s.vals["scrap_rate.high"])) === 17, "no evidence: high scrap 17 %", s.vals["scrap_rate.high"]);
if (shot) await page.screenshot({ path: `${shot}-light.png`, fullPage: true });

await clickAt(await rowAt("operator", "senior"));
s = await state();
check(Math.round(pct(s.vals["scrap_rate.high"])) === 13, "seen a senior operator: high scrap 13 %", s.vals["scrap_rate.high"]);
check(Math.round(pct(s.vals["shift.day"])) === 80, "seen a senior operator: a day lot, 80 %", s.vals["shift.day"]);
check(s.chips.length === 1, "one chip", s.chips);

await clickAt(await rowAt("operator", "senior"), { alt: true });
s = await state();
check(Math.round(pct(s.vals["scrap_rate.high"])) === 16, "do(senior operator): high scrap 16 %", s.vals["scrap_rate.high"]);
check(Math.round(pct(s.vals["shift.day"])) === 50, "do(senior operator): the shift stays 50/50", s.vals["shift.day"]);
check(s.cut.some((e) => e === "shift>operator"), "do(): the arc from Shift is cut", s.cut);
if (shot) await page.screenshot({ path: `${shot}-do-light.png`, fullPage: true });

const chips = s.chips;
await clickAt(await rowAt("scrap_rate", "high"), { rerun: false });
s = await state();
check(JSON.stringify(s.chips) === JSON.stringify(chips), "a click on Scrap (not a lever) changes no evidence", s.chips);

await clickAt(await rowAt("maintenance", "on time"), { alt: true });
s = await state();
check(Math.round(pct(s.vals["scrap_rate.high"])) === 11, "do(senior) and do(maintenance on time): high scrap 11 %", s.vals["scrap_rate.high"]);

if (shot) {
  const dark = await open("dark");
  await dark.screenshot({ path: `${shot}-dark.png`, fullPage: true });
}
check(!errors.length, "no page errors", errors);
await browser.close();
process.exit(bad ? 1 : 0);
