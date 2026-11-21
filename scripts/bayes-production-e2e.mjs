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
  await page.goto(base + "?dashboard=which-lever-lowers-scrap&tab=Ask%20the%20network");
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

// #1334: Full size (⤢) enlarges the live panel, so a click there is evidence too
await clickAt(await rowAt("maintenance", "on time"), { alt: true }); // retract it again
const net = page.locator(".panel:has(svg[data-diagram])").first();
await net.hover();
await net.locator('button.dp-tool[title="Full size"]').click();
await page.waitForTimeout(800);
const big = await page.evaluate(() => {
  const p = document.querySelector(".panel.dp-panel-full");
  const svg = p?.querySelector("svg[data-diagram]");
  return p && svg ? { w: svg.getBoundingClientRect().width, vw: innerWidth } : null;
});
check(big && big.w > big.vw * 0.8, "Full size: the panel itself, as wide as the window", big);
check(!(await page.locator(".dp-modal-full").count()), "Full size: no picture copy");
await clickAt(await rowAt("line_speed", "normal"), { alt: true });
s = await state();
check(Math.round(pct(s.vals["scrap_rate.high"])) === 7, "Full size: do(normal line speed) by a click there (with do(senior): 7 %)", s.vals["scrap_rate.high"]);
check(await page.locator(".panel.dp-panel-full svg[data-diagram]").count() === 1, "Full size: still full size after the re-run");
if (shot) await page.screenshot({ path: `${shot}-full-light.png` });
await page.keyboard.press("Escape");
await page.waitForTimeout(600);
check(!(await page.locator(".dp-panel-full, .dp-panel-full-back").count()), "Esc: back in the dashboard");

// #1336: what to check first, recomputed after a click; the active paths
const voiTitle = () => page.evaluate(() => [...document.querySelectorAll(".panel-title")].map((t) => t.textContent).find((t) => /What to check first/.test(t)) || "");
await clickAt(await rowAt("operator", "senior"), { alt: true }); // retract do(senior)
await clickAt(await rowAt("line_speed", "normal"), { alt: true }); // and do(normal)
await page.waitForTimeout(800);
check(/now: Line speed/.test(await voiTitle()), "no evidence: check the line speed first", await voiTitle());
await clickAt(await rowAt("line_speed", "high"));
await page.waitForTimeout(1500);
check(/now: (?!Line speed)/.test(await voiTitle()), "line speed seen: something else is next", await voiTitle());
check(await page.evaluate(() => (JSON.parse(document.querySelector("svg[data-bn-paths]")?.getAttribute("data-bn-paths") || "[]")).length > 0), "the active paths ride on the drawing");

const tabText = async (tab) => {
  await page.goto(base + "?dashboard=which-lever-lowers-scrap&tab=" + encodeURIComponent(tab));
  await page.waitForTimeout(5000);
  return page.evaluate(() => document.querySelector(".dash")?.innerText || "");
};
// seeing or doing: the operator's seen and done effects, then the material's
let t = await tabText("Seeing or doing");
check(t.includes("-3.9") && t.includes("-0.9"), "seeing or doing: the operator seen -3.9, done -0.9");
check(await page.evaluate(() => !!document.querySelector("svg[data-diagram] .dp-bn-legend, svg[data-diagram]")), "seeing or doing: the causal drawing");
await page.locator(".dash button", { hasText: /^Material$/ }).click();
await page.waitForTimeout(3000);
t = await page.evaluate(() => document.querySelector(".dash")?.innerText || "");
check(/-3\.3[\s\S]*-3\.3/.test(t), "seeing or doing: the material, seen = done", t.slice(0, 200));
// best plan: the cheapest plan under 10 %, then with a dearer slow line
t = await tabText("Best plan");
check(/normal line speed\s*THE PLAN/i.test(t), "best plan: a normal line speed", t.slice(0, 400));
const speed = page.locator(".dash input:visible").nth(1);
await speed.fill("30");
await speed.press("Enter");
await page.waitForTimeout(4000);
t = await page.evaluate(() => document.querySelector(".dash")?.innerText || "");
check(/maintenance on time \+ inspect incoming material\s*THE PLAN/i.test(t), "best plan: with a dearer slow line, maintenance and inspection", t.slice(0, 400));
// how the graph was found: one arc off the plant's graph
await tabText("How the graph was found");
check((await page.evaluate(() => document.querySelector("svg[data-bn-shd]")?.getAttribute("data-bn-shd"))) === "1", "the learned graph: structural Hamming distance 1");

if (shot) {
  const dark = await open("dark");
  await dark.screenshot({ path: `${shot}-dark.png`, fullPage: true });
}
check(!errors.length, "no page errors", errors);
await browser.close();
process.exit(bad ? 1 : 0);
