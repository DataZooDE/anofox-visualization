// #1152: a factor graph's message schedule steps in a real browser
// (DuckDB-Wasm): the example "How does the model reason from our
// inspections?" draws three factor graphs with `schedule :=`; Step lights the
// edge carrying the message (`.dp-msg`, `.dp-msg-back` against the edge),
// its two ends stay lit, the caption names the message, and stepping back to
// the overview clears it. A ::SELECT_AS before the macro names the variable
// a click on a node sets.
//
//   wasm-pack build --target web --out-dir web/pkg --profile wasm --no-default-features --features wasm
//   python3 serve_nocache.py 8947 &
//   NODE_PATH=<dir with playwright>/node_modules node scripts/factor-graph-e2e.mjs [--url http://127.0.0.1:8947/]
import { createRequire } from "module";

const require = createRequire(import.meta.url);
const { chromium } = require("playwright");
const arg = (k, d) => {
  const i = process.argv.indexOf(k);
  return i > 0 ? process.argv[i + 1] : d;
};
const base = arg("--url", "http://127.0.0.1:8947/");
let bad = 0;
const check = (ok, m, got) => (ok ? console.log("ok   " + m) : (bad++, console.error("FAIL " + m + (got === undefined ? "" : " " + JSON.stringify(got)))));

const browser = await chromium.launch();
const page = await browser.newPage({ viewport: { width: 1400, height: 1000 } });
const errors = [];
page.on("pageerror", (e) => errors.push(String(e)));
await page.goto(base + "?dashboard=how-does-the-model-reason-from-our-inspections");
await page.waitForFunction(() => document.querySelectorAll("svg[data-diagram]").length >= 3, null, { timeout: 120000 });
await page.waitForTimeout(500);

const state = () =>
  page.evaluate(() => {
    const p = [...document.querySelectorAll(".panel")].find((x) => x.querySelector("svg[data-diagram]"));
    const svg = p.querySelector("svg[data-diagram]");
    const msg = [...svg.querySelectorAll(".dp-msg")].map((e) => e.getAttribute("data-edge") + (e.classList.contains("dp-msg-back") ? " back" : ""));
    const lit = [...svg.querySelectorAll("rect[data-node]")].filter((e) => !e.style.opacity).map((e) => e.getAttribute("data-node"));
    return { pos: p.querySelector(".dp-story-pos")?.textContent || "", cap: p.querySelector(".dp-story-cap")?.textContent || "", msg, lit };
  });
const step = (cls) =>
  page.evaluate((c) => [...document.querySelectorAll(".panel")].find((x) => x.querySelector("svg[data-diagram]")).querySelector(c).click(), cls);

check((await page.locator(".dp-story").count()) === 3, "all three factor graphs have a stepper");
let s = await state();
check(/Overview · 5 steps/.test(s.pos) && !s.msg.length, "starts at the overview, no message", s);
await step(".dp-story-next");
s = await state();
check(s.msg.join() === "f0>e4", "step 1 lights Beta's out edge", s);
const dot = await page.evaluate(() => {
  const d = document.querySelector("svg[data-diagram] .dp-msg-dot");
  return d ? { motion: !!d.querySelector("animateMotion"), after: d.previousElementSibling?.getAttribute("data-edge") } : null;
});
check(dot && dot.motion && dot.after === "f0>e4", "a message dot runs along that edge (#1166)", dot);
check(s.lit.includes("f0") && s.lit.includes("e4") && !s.lit.includes("f1"), "its two ends stay lit, the rest dims", s);
check(/^Iteration 1 · message 1: Beta → θ \(out\), carrying Beta 0\.4 ± 0\.2; θ after iteration 1: /.test(s.cap), "the caption names the message, what it carries (#1172) and θ's marginal", s);
await step(".dp-story-next");
s = await state();
check(s.msg.join() === "e4>f1 back", "step 2: a message against the drawn edge", s);
await step(".dp-story-prev");
await step(".dp-story-prev");
s = await state();
check(!s.msg.length && /Overview/.test(s.pos), "back at the overview, no message lit", s);
check(!(await page.locator("svg[data-diagram] .dp-msg-dot").count()), "the overview has no message dot");

// A belief curve re-shapes to the step's iteration and back (#1166): the
// cycle-time graph (the second) has six iterations.
const curve = () =>
  page.evaluate(() => {
    const svg = document.querySelectorAll("svg[data-diagram]")[1];
    const p = svg && [...svg.querySelectorAll("polygon")].find((e) => e.classList.contains("dp-node-belief"));
    return p ? p.getAttribute("points") : null;
  });
const finalCurve = await curve();
await page.evaluate(() => document.querySelectorAll(".dp-story")[1].querySelector(".dp-story-next").click());
const firstCurve = await curve();
await page.evaluate(() => {
  const b = document.querySelectorAll(".dp-story")[1];
  b.querySelector(".dp-story-prev").click();
});
check(finalCurve && firstCurve && firstCurve !== finalCurve && (await curve()) === finalCurve, "a belief curve shows the step's iteration, the final one at the overview", { finalCurve: !!finalCurve });

// θ's prior sits behind its belief as a dashed outline (#1172).
const prior = await page.evaluate(() => {
  const svg = document.querySelector("svg[data-diagram]");
  const p = svg && svg.querySelector(".dp-node-prior");
  return p ? { dashed: !!p.getAttribute("stroke-dasharray"), of: p.closest("[data-of]")?.getAttribute("data-of") } : null;
});
check(prior && prior.dashed && prior.of === "e4", "θ's prior is a dashed outline behind its belief", prior);

// …and under the graph, what it learned in one sentence, also its label (#1172)
const lead = await page.evaluate(() => {
  const svg = document.querySelector("svg[data-diagram]");
  return { text: [...svg.querySelectorAll(":scope > text")].map((t) => t.textContent).join(" "), aria: svg.getAttribute("aria-label") || "" };
});
const said = "After 4 observations, θ is 0.56 ± 0.16 (prior 0.4 ± 0.2).";
check(lead.text.includes(said) && lead.aria.includes(said), "the graph says what the model learned", lead);

// A variable's tooltip draws its posterior mean over the iterations (#1152).
const v = page.locator("rect[data-node][data-spark]").first();
await v.scrollIntoViewIfNeeded();
await v.hover({ force: true });
await page.waitForTimeout(300);
const tip = await page.evaluate(() => {
  const t = [...document.querySelectorAll(".show")].find((e) => e.querySelector(".tip-spark"));
  return t ? { spark: !!t.querySelector(".tip-spark svg"), trend: t.querySelector(".tip-trend")?.textContent || "" } : null;
});
check(tip && tip.spark && / over 6 values$/.test(tip.trend), "a variable's tooltip draws its posterior over the iterations", tip);
// …in a node card: a kicker, a title and the sentence of what it does (#1166)
const card = await page.evaluate(() => {
  const t = document.querySelector(".dp-tip-card.show");
  return t ? { kicker: t.querySelector(".tip-kicker")?.textContent, title: t.querySelector(".tip-card-title")?.textContent, note: t.querySelector(".tip-note")?.textContent || "" } : null;
});
check(card && card.kicker === "Variable" && /^The variable /.test(card.title) && /^A hidden quantity/.test(card.note), "a variable's hover is a node card", card);
// …and says what the data did to the prior (#1172)
check(card && /6 observations moved τ from 1 ± 1 to 2\.85 ± 1\.43\. Most influential: y₃ \(without it 2\.45\)/.test(card.note), "the card says what the observations did to the prior, and which one moved it most (#1172)", card);

// #1172: one teal, labels as mathematics (italic), hover focus on the
// neighbourhood, and a card on an edge.
const look = await page.evaluate(() => {
  const svg = document.querySelector("svg[data-diagram]");
  return {
    italic: [...svg.querySelectorAll("text")].some((t) => t.textContent === "θ" && t.getAttribute("font-style") === "italic"),
    teal: [...svg.querySelectorAll("polyline[data-edge]")].every((e) => (e.getAttribute("stroke") || "").toUpperCase() === "#0D9488"),
  };
});
check(look.italic && look.teal, "a factor graph in one teal, its labels in the math italic", look);
const focus = await page.evaluate(async () => {
  const svg = document.querySelector("svg[data-diagram]");
  const theta = svg.querySelector('rect[data-node="e4"]');
  theta.dispatchEvent(new MouseEvent("mouseenter"));
  const op = (id) => svg.querySelector(`rect[data-node="${id}"]`).style.opacity;
  const r = { f1: op("f1"), v0: op("v0") };
  theta.dispatchEvent(new MouseEvent("mouseleave"));
  r.after = op("v0");
  return r;
});
check(focus.f1 === "" && Number(focus.v0) < 1 && focus.after === "", "hovering θ keeps its factors and fades the rest", focus);
const edgeCard = await page.evaluate(async () => {
  const e = document.querySelector('svg[data-diagram] polyline[data-edge="f1>v0"]');
  e.dispatchEvent(new MouseEvent("mouseenter"));
  await new Promise((r) => setTimeout(r, 50));
  const t = document.querySelector(".dp-tip-card.show");
  const out = t ? { kicker: t.querySelector(".tip-kicker")?.textContent, title: t.querySelector(".tip-card-title")?.textContent, note: t.querySelector(".tip-note")?.textContent } : null;
  e.dispatchEvent(new MouseEvent("mouseleave"));
  return out;
});
check(edgeCard && edgeCard.kicker === "Edge · out" && edgeCard.title === "y₀ — Bernoulli" && /^The observation y₀ enters the model here/.test(edgeCard.note), "an edge's hover is a card", edgeCard);

// Forney style (#1172): observations are terminal squares labelled beside
// them; the hidden-state graph's s₀ (on two factors, no marginals) is the
// edge between them, its card a variable's.
const forney = await page.evaluate(async () => {
  const svgs = document.querySelectorAll("svg[data-diagram]");
  const terminals = svgs[0].querySelectorAll("rect.dp-node-terminal").length;
  const e = svgs[2].querySelector('polyline[data-var="s₀"]');
  if (!e) return { terminals, edge: false };
  e.dispatchEvent(new MouseEvent("mouseenter"));
  await new Promise((r) => setTimeout(r, 50));
  const t = document.querySelector(".dp-tip-card.show");
  const card = t ? t.querySelector(".tip-card-title")?.textContent : null;
  e.dispatchEvent(new MouseEvent("mouseleave"));
  return { terminals, edge: true, node: !!svgs[2].querySelector('rect[data-node="v0"]'), card };
});
check(forney.terminals === 4 && forney.edge && !forney.node && forney.card === "The variable s₀", "Forney style: terminals, a variable as its edge", forney);

// #1172: hovering m widens each cycle's edge by its influence on m (y₄ the
// most, from the library's leave-one-out), and its observations stay in
// focus; leaving resets them.
const infl = await page.evaluate(() => {
  const svg = document.querySelectorAll("svg[data-diagram]")[1];
  const m = svg.querySelector('rect[data-node="e6"]');
  m.dispatchEvent(new MouseEvent("mouseenter"));
  const w = (to) => parseFloat(svg.querySelector(`polyline[data-to="${to}"]`).style.strokeWidth || "0");
  const r = { y4: w("v4"), y3: w("v3"), y4Opacity: svg.querySelector('rect[data-node="v4"]').style.opacity };
  m.dispatchEvent(new MouseEvent("mouseleave"));
  r.after = svg.querySelector('polyline[data-to="v4"]').style.strokeWidth;
  return r;
});
check(infl.y4 > infl.y3 && infl.y3 > 0 && infl.y4Opacity === "" && infl.after === "", "hovering m widens each cycle's edge by its influence", infl);
// …and with m selected (a click), they stay wide after the pointer leaves;
// a second click clears it (owner, 11-14)
const widths = () =>
  page.evaluate(() => {
    const svg = document.querySelectorAll("svg[data-diagram]")[1];
    const w = (to) => parseFloat(svg.querySelector(`polyline[data-to="${to}"]`)?.style.strokeWidth || "0");
    return { y4: w("v4"), y3: w("v3"), y4Opacity: svg.querySelector('rect[data-node="v4"]').style.opacity };
  });
await page.locator('svg[data-diagram] rect[data-node="e6"]').nth(0).click({ force: true });
await page.waitForFunction(() => /[?&]v\.\w+=e6/.test(location.search), null, { timeout: 20000 }).catch(() => {});
await page.waitForTimeout(500);
await page.mouse.move(2, 2);
const sel = await widths();
check(sel.y4 > sel.y3 && sel.y3 > 0 && sel.y4Opacity === "", "a selected variable keeps its observations' edges wide, and in focus", sel);
await page.locator('svg[data-diagram] rect[data-node="e6"]').nth(0).click({ force: true });
await page.waitForFunction(() => !/[?&]v\.\w+=e6/.test(location.search), null, { timeout: 20000 }).catch(() => {});
await page.waitForTimeout(500);
await page.mouse.move(2, 2);
const cleared = await widths();
check(cleared.y4 === 0 && cleared.y3 === 0, "unselected, the edges go back", cleared);

// A ::SELECT_AS before the macro names what a click on a node sets (#1152).
const sql = `
SELECT 'node'::SELECT_AS;
SELECT anofox_plot_factor_graph(n, edges := [{'variable': 'v0', 'factor': 'f1', 'interface': 'out'}, {'variable': 'v1', 'factor': 'f1', 'interface': 'p'}]) AS "Graph"
FROM (VALUES ('v0', 'data', 'y', 'obs'), ('v1', 'variable', 'theta', 'param'), ('f1', 'factor', 'Bernoulli', 'obs')) n(id, kind, label, layer);
SELECT 'picked: ' || coalesce(getvariable('node'), '-') AS m::MARKDOWN;
`;
await page.goto("about:blank");
await page.goto(`${base}?embed=1#sql=${Buffer.from(sql, "utf8").toString("base64")}`);
await page.waitForFunction(() => document.querySelector("svg[data-diagram]"), null, { timeout: 120000 });
await page.locator('svg[data-diagram] rect[data-node="f1"]').first().click({ force: true });
await page.waitForFunction(() => /picked: f1/.test(document.body.innerText), null, { timeout: 20000 }).catch(() => {});
const picked = await page.evaluate(() => (document.body.innerText.match(/picked: \S+/) || [""])[0]);
check(picked === "picked: f1" && /[?&]v\.node=f1/.test(page.url()), "a click on a node sets the ::SELECT_AS variable", { picked, url: page.url() });
// …and so does the keyboard: Tab to the graph, Home to a node, Enter (#1152)
await page.evaluate(() => document.querySelector("svg[data-diagram]").focus());
await page.keyboard.press("Home");
const keyed = await page.evaluate(() => document.activeElement?.getAttribute("data-node"));
await page.keyboard.press("Enter");
await page.waitForFunction((id) => document.body.innerText.includes("picked: " + id), keyed, { timeout: 20000 }).catch(() => {});
const kpicked = await page.evaluate(() => (document.body.innerText.match(/picked: \S+/) || [""])[0]);
check(keyed && keyed !== "f1" && kpicked === "picked: " + keyed, "Enter on a focused node selects it", { keyed, kpicked });
// A diagram shrunk to fit a narrow panel zooms and pans in place (#1152).
const narrow = await browser.newPage({ viewport: { width: 600, height: 900 } });
narrow.on("pageerror", (e) => errors.push(String(e)));
await narrow.goto(base + "?dashboard=how-does-the-model-reason-from-our-inspections");
await narrow.waitForFunction(() => document.querySelectorAll("svg[data-diagram]").length >= 3, null, { timeout: 120000 });
await narrow.waitForTimeout(500);
const zv = () =>
  narrow.evaluate(() => {
    const h = [...document.querySelectorAll(".panel-svg")].find((x) => x._dpDz);
    if (!h) return null;
    const r = h.getBoundingClientRect();
    return {
      t: h.querySelector("svg > g.dp-diagram").getAttribute("transform"),
      k: h._dpDz.k,
      fit: h._dpDz.fit,
      chip: !h.querySelector(".dp-diagram-reset").hidden,
      box: { x: r.left, y: r.top, w: r.width, h: r.height },
    };
  });
let z = await zv();
check(z && z.fit < 1 && z.k === 1 && !z.chip, "a factor graph shrunk to fit has zoom, at home", z);
const home = z.t;
await narrow.evaluate(() => [...document.querySelectorAll(".panel-svg")].find((x) => x._dpDz).scrollIntoView({ block: "center" }));
z = await zv();
await narrow.mouse.move(z.box.x + z.box.w / 2, z.box.y + z.box.h / 2);
await narrow.mouse.wheel(0, -200);
check((await zv()).t === home, "a plain wheel scrolls, it doesn't zoom");
await narrow.waitForTimeout(400);
z = await zv(); // the page scrolled under the pointer
await narrow.mouse.move(z.box.x + z.box.w / 2, z.box.y + z.box.h / 2);
await narrow.keyboard.down("Control");
await narrow.mouse.wheel(0, -200);
await narrow.keyboard.up("Control");
z = await zv();
check(z.k > 1 && z.chip && z.t !== home && z.t.endsWith(home), "Ctrl + wheel zooms the drawing, the reset chip shows", z);
const before = z.t;
const url0 = narrow.url();
await narrow.mouse.move(z.box.x + 8, z.box.y + z.box.h - 8);
await narrow.mouse.down();
await narrow.mouse.move(z.box.x + 60, z.box.y + z.box.h - 40, { steps: 5 });
await narrow.mouse.up();
z = await zv();
check(z.t !== before && narrow.url() === url0, "a drag on the background pans, and selects nothing", { t: z.t, url: narrow.url() });
// the keyboard: + zooms further, a node moved to off screen comes into view, 0 goes home
await narrow.evaluate(() => [...document.querySelectorAll(".panel-svg")].find((x) => x._dpDz).querySelector("svg").focus());
for (let i = 0; i < 4; i++) await narrow.keyboard.press("+");
const seen = await narrow.evaluate(() => {
  const h = [...document.querySelectorAll(".panel-svg")].find((x) => x._dpDz);
  const hr = h.getBoundingClientRect();
  const inside = (n) => {
    const r = n.getBoundingClientRect();
    return r.left >= hr.left - 1 && r.right <= hr.right + 1 && r.top >= hr.top - 1 && r.bottom <= hr.bottom + 1;
  };
  const off = [...h.querySelectorAll("[data-node][tabindex]")].find((n) => !inside(n));
  if (!off) return null;
  off.focus();
  return inside(off);
});
check(seen === true, "a node focused off screen pans into view", seen);
await narrow.keyboard.press("0");
z = await zv();
check(z.k === 1 && z.t === home && !z.chip, "0 goes back to the whole diagram", z);
await narrow.close();

// #1152 (owner 11-14, option A): a click on a collapsed model's ⋯ draws
// every slice (anofox_fg_slices = 'all', kept in the share link); the
// *Fewer slices* chip collapses it again.
const chain = `
SELECT anofox_plot_factor_graph(n, edges := (
  SELECT list(e) FROM (
    SELECT {'variable': 's' || (t - 1), 'factor': 'f' || t, 'interface': 'mean'} AS e FROM range(1, 10) r(t)
    UNION ALL SELECT {'variable': 's' || t, 'factor': 'f' || t, 'interface': 'out'} FROM range(1, 10) r(t)))) AS "Chain"
FROM (SELECT 's' || t AS id, 'variable' AS kind, 's_{' || t || '}' AS label, 'state' AS layer, t AS slice FROM range(10) r(t)
      UNION ALL SELECT 'f' || t, 'factor', 'Normal', 'state', t FROM range(1, 10) r(t)) n;
`;
const fg = await browser.newPage({ viewport: { width: 1400, height: 900 } });
fg.on("pageerror", (e) => errors.push(String(e)));
await fg.goto(`${base}?embed=1#sql=${Buffer.from(chain, "utf8").toString("base64")}`);
await fg.waitForFunction(() => document.querySelector('svg[data-slices="collapsed"] [data-node^="__more_"]'), null, { timeout: 120000 });
await fg.locator('svg[data-diagram] [data-node="__more_state"]').first().click({ force: true });
await fg.waitForFunction(() => document.querySelector('svg[data-slices="all"]'), null, { timeout: 30000 }).catch(() => {});
const open = await fg.evaluate(() => ({
  more: document.querySelectorAll('svg[data-diagram] [data-node^="__more_"]').length,
  f5: !!document.querySelector('svg[data-diagram] [data-node="f5"]'),
  chip: !!document.querySelector(".dp-fg-fewer"),
}));
check(!open.more && open.f5 && open.chip && /[?&]v\.anofox_fg_slices=all/.test(fg.url()), "a click on ⋯ draws every slice, with a Fewer chip and the state in the link", { ...open, url: fg.url() });
await fg.locator(".dp-fg-fewer").click();
await fg.waitForFunction(() => document.querySelector('svg[data-slices="collapsed"]'), null, { timeout: 30000 }).catch(() => {});
const shut = await fg.evaluate(() => ({ more: document.querySelectorAll('svg[data-diagram] [data-node^="__more_"]').length, chip: !!document.querySelector(".dp-fg-fewer") }));
check(shut.more > 0 && !shut.chip && !/anofox_fg_slices=all/.test(fg.url()), "Fewer slices collapses it again", { ...shut, url: fg.url() });
await fg.close();

check(!errors.length, "no page errors", errors);
await browser.close();
if (bad) {
  console.error(`factor-graph-e2e: ${bad} failed`);
  process.exit(1);
}
console.log("factor-graph-e2e: all checks passed");
