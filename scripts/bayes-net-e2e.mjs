// #1275: Bayesian networks in a real browser (DuckDB-Wasm, the bayes-net
// plugin from plugins.json): the example "What does the evidence say about
// the cause?" draws the sprinkler and the chest clinic top to bottom, each
// node with a bar per state; the observed node is shaded; hovering a node
// opens its card (belief against the prior, its conditional probability
// table as a heat table), hovering an arc names its strength.
//
//   scripts/build-wasm-pkg.sh && scripts/web-plugins.sh web
//   python3 serve_nocache.py 8947 &
//   NODE_PATH=<dir with playwright>/node_modules node scripts/bayes-net-e2e.mjs [--url http://127.0.0.1:8947/]
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
const page = await browser.newPage({ viewport: { width: 1400, height: 1000 } });
const errors = [];
page.on("pageerror", (e) => errors.push(String(e)));
await page.goto(base + "?dashboard=what-does-the-evidence-say-about-the-cause");
await page.waitForFunction(() => document.querySelectorAll("svg[data-diagram]").length >= 2, null, { timeout: 120000 });
await page.waitForTimeout(500);

const drawn = await page.evaluate(() => {
  const [sp, asia] = document.querySelectorAll("svg[data-diagram]");
  const grass = sp.querySelector('rect[data-node="grass"]');
  return {
    dir: sp.getAttribute("data-diagram"),
    nodes: sp.querySelectorAll("rect[data-node]").length,
    bars: sp.querySelectorAll("rect.dp-node-state-bar").length,
    summary: sp.getAttribute("data-summary") || "",
    grassFill: grass?.getAttribute("fill"),
    cloudyFill: sp.querySelector('rect[data-node="cloudy"]')?.getAttribute("fill"),
    double: !!asia.querySelector('rect[data-node="either"].dp-node-double-box'),
    dashed: asia.querySelector('polyline[data-edge="asia>tub"]')?.getAttribute("stroke-dasharray") || "",
  };
});
check(drawn.dir === "TB" && drawn.nodes === 4, "the sprinkler: four nodes, top to bottom", drawn);
check(drawn.bars === 8, "a bar per state", drawn);
check(/^Bayesian network: 4 nodes, 4 arcs; evidence: Wet grass = yes; most likely: /.test(drawn.summary), "its summary names the evidence", drawn.summary);
check(drawn.grassFill && drawn.grassFill !== drawn.cloudyFill, "the observed node is shaded", drawn);
check(drawn.double, "TB or cancer has a double outline");
check(drawn.dashed !== "", "the weak arc from Visited Asia is dashed", drawn);

// a node's card: kind, belief vs prior, the table
await page.locator('svg[data-diagram] rect[data-node="rain"]').first().hover();
await page.waitForTimeout(200);
const card = await page.evaluate(() => {
  const t = document.querySelector(".dp-tip.show");
  return t
    ? {
        bn: t.classList.contains("dp-tip-bn"),
        kicker: t.querySelector(".tip-kicker")?.textContent,
        title: t.querySelector(".tip-card-title")?.textContent,
        states: [...t.querySelectorAll(".tip-bn-states tbody tr")].map((r) => [...r.children].map((c) => c.textContent.trim()).join(" ")),
        cpt: [...t.querySelectorAll(".tip-bn-cpt tr")].map((r) => [...r.children].map((c) => c.textContent.trim()).join(" ")),
      }
    : null;
});
check(card && card.bn && card.kicker === "Chance node" && card.title === "Rain", "hovering Rain opens its card", card);
check(card && card.states[0] === "yes  71% 50%", "its belief against the prior", card?.states);
check(card && card.cpt[0] === "Cloudy yes no" && card.cpt[1] === "yes 0.8 0.2", "its table, by Cloudy", card?.cpt);
if (shot) await page.screenshot({ path: shot });

await page.locator('svg[data-diagram] rect[data-node="grass"]').first().hover();
await page.waitForTimeout(200);
const ev = await page.evaluate(() => document.querySelector(".dp-tip.show .tip-bn-ev")?.textContent);
check(ev === "Observed: yes.", "the observed node's card says so", ev);

// an arc's hover text: its strength
const arc = await page.evaluate(() => {
  const asia = document.querySelectorAll("svg[data-diagram]")[1];
  const e = asia.querySelector('polyline[data-edge="smoke>lung"]');
  return e?.querySelector("title")?.textContent || e?.getAttribute("data-tip") || "";
});
check(/Smoker → Lung cancer: strength 1/.test(arc), "an arc names its strength on hover", arc);

check(!errors.length, "no page errors", errors);
await browser.close();
if (bad) {
  console.error(`bayes-net-e2e: ${bad} failed`);
  process.exit(1);
}
console.log("bayes-net-e2e: all checks passed");
