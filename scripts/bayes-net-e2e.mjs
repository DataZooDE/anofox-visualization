// #1275 / #1276: Bayesian networks in a real browser (DuckDB-Wasm, the
// bayes-net plugin from plugins.json). The example "What does the evidence
// say about the cause?" draws the sprinkler and the chest clinic top to
// bottom, each node with a bar per state; hovering a node opens its card
// (belief against the prior, its conditional probability table), hovering an
// arc names its strength (#1275). Then it is queried by clicking (#1276): a
// click on a state observes it and the SQL recomputes the beliefs (each bar
// shows its change, the line under the graph says what the evidence did), a
// second click retracts it, the chip switches it to do() (the incoming arcs
// cut, a hand on the node), shift-click gives soft evidence; hovering a node
// lights its Markov blanket, and with a node anchored (a click on its name)
// the active paths to the hovered one.
//
//   scripts/build-wasm-pkg.sh && scripts/web-plugins.sh web
//   python3 serve_nocache.py 8947 &
//   NODE_PATH=<dir with playwright>/node_modules node scripts/bayes-net-e2e.mjs [--url http://127.0.0.1:8947/] [--shot out-prefix]
//   … --page http://127.0.0.1:8095/d/bayes-nets   (served: serve --dashboards <dir> --plugin …)
import { createRequire } from "module";

const require = createRequire(import.meta.url);
const { chromium } = require("playwright");
const arg = (k, d) => {
  const i = process.argv.indexOf(k);
  return i > 0 ? process.argv[i + 1] : d;
};
const base = arg("--url", "http://127.0.0.1:8947/");
const shot = arg("--shot", null);
// a served dashboard instead (serve --dashboards: no SQL crosses the wire)
const pageUrl = arg("--page", base + "?dashboard=what-does-the-evidence-say-about-the-cause");
let bad = 0;
const check = (ok, m, got) => (ok ? console.log("ok   " + m) : (bad++, console.error("FAIL " + m + (got === undefined ? "" : " " + JSON.stringify(got)))));

const browser = await chromium.launch();
const page = await browser.newPage({ viewport: { width: 1400, height: 1000 } });
const errors = [];
page.on("pageerror", (e) => errors.push(String(e)));
await page.goto(pageUrl);
await page.waitForFunction(() => document.querySelectorAll("svg[data-diagram]").length >= 2, null, { timeout: 120000 });
await page.waitForTimeout(500);

// the networks: 0 the sprinkler, 1 the chest clinic
const net = (k) => page.locator(".panel:has(svg[data-diagram])").nth(k);
// the centre of a node's state row (its name), or of its label
async function rowAt(k, node, state) {
  return page.evaluate(
    ([k, node, state]) => {
      const svg = document.querySelectorAll("svg[data-diagram]")[k];
      const t = state == null ? svg.querySelector(`[data-of="${node}"] text.dp-node-label`) : svg.querySelector(`[data-of="${node}"] text[data-state="${state}"]`);
      if (!t) return null;
      // on screen first (the chest clinic is below the fold)
      const b = t.getBoundingClientRect();
      if (b.top < 60 || b.bottom > innerHeight - 60) t.scrollIntoView({ block: "center" });
      const r = t.getBoundingClientRect();
      return { x: r.left + r.width / 2, y: r.top + r.height / 2 };
    },
    [k, node, state]
  );
}
// click, then wait for the re-run to draw the network again
async function clickAt(p, opts = {}) {
  const before = await page.evaluate(() => [...document.querySelectorAll("svg[data-diagram]")].map((s) => s.getAttribute("data-summary")).join("|"));
  if (opts.shift) await page.keyboard.down("Shift");
  if (opts.alt) await page.keyboard.down("Alt");
  await page.mouse.click(p.x, p.y);
  if (opts.shift) await page.keyboard.up("Shift");
  if (opts.alt) await page.keyboard.up("Alt");
  if (opts.rerun === false) return;
  await page
    .waitForFunction((b) => [...document.querySelectorAll("svg[data-diagram]")].map((s) => s.getAttribute("data-summary")).join("|") !== b, before, { timeout: 30000 })
    .catch(() => {});
  await page.waitForTimeout(300);
}
const state = (k) =>
  page.evaluate((k) => {
    const svg = document.querySelectorAll("svg[data-diagram]")[k];
    const panel = svg.closest(".panel");
    const vals = {};
    for (const t of svg.querySelectorAll("text.dp-node-state-value[data-state]")) {
      const of = t.closest("[data-of]")?.getAttribute("data-of");
      vals[of + "." + t.getAttribute("data-state")] = t.textContent;
    }
    return {
      summary: svg.getAttribute("data-summary") || "",
      vals,
      deltas: [...svg.querySelectorAll(".dp-node-state-delta text, text.dp-node-state-delta")].map((t) => t.closest("[data-of]")?.getAttribute("data-of") + ":" + t.textContent),
      chips: [...panel.querySelectorAll(".dp-bn-chip")].map((c) => c.firstChild.textContent),
      hint: !!panel.querySelector(".dp-bn-hint"),
      cut: [...svg.querySelectorAll('polyline[data-cut="1"]')].map((e) => e.getAttribute("data-edge")),
      cutMark: !!svg.querySelector(".dp-edge-cut"),
      hand: !!svg.querySelector(".dp-node-icon"),
      moved: Object.fromEntries([...svg.querySelectorAll("rect[data-moved]")].map((r) => [r.getAttribute("data-node"), +r.getAttribute("data-moved")])),
      fill: Object.fromEntries([...svg.querySelectorAll("rect[data-node]")].map((r) => [r.getAttribute("data-node"), r.getAttribute("fill")])),
    };
  }, k);

// ---- #1275: the drawing, the card, the arc's hover text -------------------
const drawn = await page.evaluate(() => {
  const [sp, asia] = document.querySelectorAll("svg[data-diagram]");
  return {
    dir: sp.getAttribute("data-diagram"),
    nodes: sp.querySelectorAll("rect[data-node]").length,
    bars: sp.querySelectorAll("rect.dp-node-state-bar").length,
    summary: sp.getAttribute("data-summary") || "",
    double: !!asia.querySelector('rect[data-node="either"].dp-node-double-box'),
    dashed: asia.querySelector('polyline[data-edge="asia>tub"]')?.getAttribute("stroke-dasharray") || "",
  };
});
check(drawn.dir === "TB" && drawn.nodes === 4, "the sprinkler: four nodes, top to bottom", drawn);
check(drawn.bars === 8, "a bar per state", drawn);
check(/^Bayesian network: 4 nodes, 4 arcs; most likely: /.test(drawn.summary), "no evidence yet: its summary", drawn.summary);
check(drawn.double, "TB or cancer has a double outline");
check(drawn.dashed !== "", "the weak arc from Visited Asia is dashed", drawn);
let s0 = await state(0);
check(s0.hint && !s0.chips.length, "no evidence: a hint, no chips", s0);

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
        hint: t.querySelector(".tip-bn-hint")?.textContent || "",
      }
    : null;
});
check(card && card.bn && card.kicker === "Chance node" && card.title === "Rain", "hovering Rain opens its card", card);
check(card && card.states[0] === "yes  50% 50%", "its belief against the prior", card?.states);
check(card && card.cpt[0] === "Cloudy yes no" && card.cpt[1] === "yes 0.8 0.2", "its table, by Cloudy", card?.cpt);
check(card && /Click a state to observe it/.test(card.hint), "the card says what a click does", card?.hint);

// #1276: the Markov blanket of Rain — Cloudy (parent), Wet grass (child),
// Sprinkler on (the child's other parent) — lit while hovered
const blanket = await page.evaluate(() =>
  [...document.querySelectorAll("svg[data-diagram]")[0].querySelectorAll("rect.dp-bn-blanket")].map((r) => r.getAttribute("data-node")).sort()
);
check(JSON.stringify(blanket) === JSON.stringify(["cloudy", "grass", "sprinkler"]), "hovering Rain lights its Markov blanket", blanket);

const arc = await page.evaluate(() => {
  const asia = document.querySelectorAll("svg[data-diagram]")[1];
  const e = asia.querySelector('polyline[data-edge="smoke>lung"]');
  return e?.querySelector("title")?.textContent || e?.getAttribute("data-tip") || "";
});
check(/Smoker → Lung cancer: strength 1/.test(arc), "an arc names its strength on hover", arc);
await page.mouse.move(5, 5);

// ---- #1276: observe, the change, retract ----------------------------------
await clickAt(await rowAt(0, "grass", "yes"));
let s1 = await state(0);
check(s1.chips.join("|") === "Wet grass = yes", "a click on Wet grass = yes observes it (a chip)", s1.chips);
check(s1.vals["rain.yes"] === "71%" && s1.vals["sprinkler.yes"] === "43%" && s1.vals["cloudy.yes"] === "58%", "the SQL recomputes the beliefs", s1.vals);
check(s1.deltas.includes("rain:▲21") && s1.deltas.includes("sprinkler:▲13"), "each bar shows its change", s1.deltas);
check(!s1.deltas.some((d) => d.startsWith("grass:")), "…not the observed node's own", s1.deltas);
check(s1.moved.rain > 0.1 && s1.fill.rain !== s0.fill.rain && s1.fill.grass !== s0.fill.grass, "Rain tinted by how far it moved, Wet grass shaded", { moved: s1.moved, fill: s1.fill });
check(/^Given Wet grass, Rain rose from 50% to 71%, Sprinkler on rose from 30% to 43% and Cloudy rose from 50% to 58%\./.test(s1.summary), "the summary says what the evidence did", s1.summary);
const under = await page.evaluate(() => [...document.querySelectorAll("svg[data-diagram]")[0].querySelectorAll(":scope > text")].map((t) => t.textContent).join(" "));
check(/^Given Wet grass, Rain rose from 50% to 71%/.test(under), "…also as a line under the graph", under);
check(/v\.sprinkler_ev=/.test(page.url()), "the evidence is in the address", page.url());
// #1311: only a network's states and nodes write the evidence. A click on an
// arc, or on a mark without data-bn (a plugin's own shape; here a node with
// its data-bn taken off), leaves it alone
const evUrl = page.url();
const offBn = async (sel, strip) =>
  page.evaluate(
    ([sel, strip]) => {
      const e = document.querySelectorAll("svg[data-diagram]")[0].querySelector(sel);
      if (!e) return null;
      if (strip) e.removeAttribute("data-bn");
      e.scrollIntoView({ block: "center" });
      const ctm = e.getScreenCTM();
      const p = e.tagName === "polyline" ? e.getPointAtLength(e.getTotalLength() / 2) : { x: +e.getAttribute("x") + 4, y: +e.getAttribute("y") + 4 };
      const q = new DOMPoint(p.x, p.y).matrixTransform(ctm);
      return { x: q.x, y: q.y };
    },
    [sel, strip]
  );
for (const [what, sel, strip] of [
  ["an arc", 'polyline[data-edge="sprinkler>grass"]', false],
  ["a mark without data-bn", 'rect[data-node="cloudy"]', true],
]) {
  const p = await offBn(sel, strip);
  check(!!p, what + " is drawn", p);
  if (p) await page.mouse.click(p.x, p.y);
  await page.waitForTimeout(1200);
  const sx = await state(0);
  check(sx.chips.join("|") === "Wet grass = yes" && sx.vals["rain.yes"] === "71%", "a click on " + what + " leaves the evidence alone", { chips: sx.chips, vals: sx.vals });
  check(page.url() === evUrl && !errors.length, "…the address keeps it, no error", { url: page.url(), errors });
}
const quiet = async (k) => {
  await net(k).evaluate((el) => window.scrollTo(0, el.getBoundingClientRect().top + window.scrollY - 120));
  const b = await net(k).boundingBox();
  await page.mouse.move(b.x + 6, b.y + b.height - 6);
  await page.waitForTimeout(300);
};
if (shot) await quiet(0), await net(0).screenshot({ path: shot + "-observe.png" });

// explaining away: rain observed too, the sprinkler falls back
await clickAt(await rowAt(0, "rain", "yes"));
let s2 = await state(0);
check(s2.vals["sprinkler.yes"] === "19%" && s2.deltas.includes("sprinkler:▼24"), "rain explains the wet grass away", { vals: s2.vals, deltas: s2.deltas });
check(/The last change moved Sprinkler on most: from 43% to 19%\./.test(s2.summary), "…and the summary names the last change", s2.summary);
// a second click retracts
await clickAt(await rowAt(0, "rain", "yes"));
let s3 = await state(0);
check(s3.chips.join("|") === "Wet grass = yes" && s3.vals["sprinkler.yes"] === "43%", "a second click retracts it", { chips: s3.chips, vals: s3.vals });

// ---- do(): seeing against doing --------------------------------------------
await net(0).locator(".dp-bn-chip").first().locator("button[aria-label^='Retract']").click();
await page.waitForTimeout(800);
await clickAt(await rowAt(0, "sprinkler", "yes"));
let s4 = await state(0);
check(s4.vals["cloudy.yes"] === "17%", "seeing the sprinkler on: a cloudy sky unlikely", s4.vals);
const before = s4.summary;
await net(0).locator(".dp-bn-chip .dp-bn-mode").click();
await page.waitForFunction((b) => document.querySelectorAll("svg[data-diagram]")[0].getAttribute("data-summary") !== b, before, { timeout: 30000 });
await page.waitForTimeout(300);
let s5 = await state(0);
check(s5.vals["cloudy.yes"] === "50%", "doing it: Cloudy back at 50%", s5.vals);
check(JSON.stringify(s5.cut) === JSON.stringify(["cloudy>sprinkler"]) && s5.cutMark, "its incoming arc is cut, with a break mark", s5);
check(s5.hand, "the set node shows a hand", s5);
check(/^Given Sprinkler on set to yes, /.test(s5.summary), "the summary says it was set", s5.summary);
const modeBtn = await net(0).locator(".dp-bn-chip .dp-bn-mode").getAttribute("aria-pressed");
check(modeBtn === "true", "the chip shows do", modeBtn);
if (shot) await quiet(0), await net(0).screenshot({ path: shot + "-do.png" });

// ---- soft evidence: shift-click, a likelihood per state ---------------------
await net(0).locator(".dp-bn-chip").first().locator("button[aria-label^='Retract']").click();
await page.waitForTimeout(800);
await clickAt(await rowAt(0, "grass", null), { shift: true, rerun: false });
await page.waitForSelector(".dp-bn-pop", { timeout: 5000 }).catch(() => {});
const pop = await page.evaluate(() => {
  const p = document.querySelector(".dp-bn-pop");
  return p ? { title: p.querySelector(".dp-bn-pop-title")?.textContent, sliders: p.querySelectorAll("input[type=range]").length } : null;
});
check(pop && pop.title === "Evidence on Wet grass" && pop.sliders === 2, "shift-click opens the evidence panel", pop);
if (pop) {
  await page.evaluate(() => {
    const [a, b] = document.querySelectorAll(".dp-bn-pop input[type=range]");
    a.value = "0.8";
    b.value = "0.2";
  });
  const was = await page.evaluate(() => document.querySelectorAll("svg[data-diagram]")[0].getAttribute("data-summary"));
  await page.click(".dp-bn-pop button[data-act=soft]");
  await page.waitForFunction((b) => document.querySelectorAll("svg[data-diagram]")[0].getAttribute("data-summary") !== b, was, { timeout: 30000 }).catch(() => {});
  await page.waitForTimeout(300);
  const s6 = await state(0);
  check(s6.chips.join("|") === "Wet grass: soft (yes 0.8, no 0.2)" && s6.vals["rain.yes"] === "64%", "soft evidence: a likelihood per state", { chips: s6.chips, vals: s6.vals });
}

// the keyboard: Enter on a focused node opens its evidence panel, Esc closes it
await page.locator('svg[data-diagram] rect[data-node="rain"]').first().focus();
await page.keyboard.press("Enter");
await page.waitForSelector(".dp-bn-pop", { timeout: 5000 }).catch(() => {});
const kbd = await page.evaluate(() => document.querySelector(".dp-bn-pop .dp-bn-pop-title")?.textContent || "");
await page.keyboard.press("Escape");
await page.waitForTimeout(200);
const closed = await page.evaluate(() => !document.querySelector(".dp-bn-pop"));
check(kbd === "Evidence on Rain" && closed, "Enter on a focused node opens its evidence panel, Esc closes it", { kbd, closed });
await page.evaluate(() => document.activeElement?.blur?.());

// ---- active paths in the chest clinic --------------------------------------
await clickAt(await rowAt(1, "asia", null), { rerun: false });
await page.waitForTimeout(200);
const anchored = await page.evaluate(() => !!document.querySelectorAll("svg[data-diagram]")[1].querySelector('rect[data-node="asia"].dp-bn-anchor'));
check(anchored, "a click on Visited Asia's name anchors it");
const hoverSmoke = async () => {
  await page.mouse.move(5, 5);
  const p = await rowAt(1, "smoke", null);
  await page.mouse.move(p.x, p.y);
  await page.waitForTimeout(250);
  return page.evaluate(() => ({
    path: [...document.querySelectorAll("svg[data-diagram]")[1].querySelectorAll("polyline.dp-bn-path")].map((e) => e.getAttribute("data-edge")).sort(),
    note: document.querySelector(".dp-tip.show .tip-bn-paths")?.textContent || "",
  }));
};
let h = await hoverSmoke();
check(!h.path.length && /^Independent of Visited Asia given the evidence/.test(h.note), "no evidence: Smoker independent of Visited Asia", h);
await page.mouse.move(5, 5);
await clickAt(await rowAt(1, "xray", "yes"));
h = await hoverSmoke();
check(
  JSON.stringify(h.path) === JSON.stringify(["asia>tub", "either>xray", "lung>either", "smoke>lung", "tub>either"].filter((e) => e !== "either>xray")) &&
    /^1 open path to Visited Asia/.test(h.note),
  "an abnormal X-ray opens the path through TB or cancer",
  h
);
await page.mouse.move(5, 5);
await page.waitForTimeout(200);
if (shot) {
  // each network on its own, light and dark
  for (const scheme of ["light", "dark"]) {
    await page.emulateMedia({ colorScheme: scheme });
    await page.waitForTimeout(600);
    for (const k of [0, 1]) {
      // the pointer in the panel's empty corner: no hover card
      // clear of the sticky app bar, no node focused (a focused node shows its card)
      await page.evaluate(() => document.activeElement?.blur?.());
      await net(k).evaluate((el) => window.scrollTo(0, el.getBoundingClientRect().top + window.scrollY - 120));
      const b = await net(k).boundingBox();
      await page.mouse.move(b.x + 6, b.y + b.height - 6);
      await page.waitForTimeout(300);
      await net(k).screenshot({ path: `${shot}-${scheme}-${k ? "asia" : "sprinkler"}.png` });
    }
  }
}

// ---- #1278: causal views ("Does the discount keep customers?") ------------
// The adjustment set on the DAG (X, Y, the ring, blocked back-door paths,
// the legend), no set (the paths open, red), seeing or doing by clicking,
// the twin network (three lanes, what the change did), the structure diff
// (the distance on the root, the legend; the slider lets the weak arc in).
const causalUrl = arg("--causal-page", base + "?dashboard=does-the-discount-keep-customers");
await page.goto(causalUrl);
await page.waitForFunction(() => document.querySelectorAll("svg[data-diagram]").length >= 2, null, { timeout: 120000 });
await page.waitForTimeout(500);
const causal = (k) =>
  page.evaluate((k) => {
    const svg = document.querySelectorAll("svg[data-diagram]")[k];
    return {
      ring: [...svg.querySelectorAll(".dp-node-ring")].map((r) => r.closest("[data-of]")?.getAttribute("data-of")),
      letters: [...svg.querySelectorAll("text.dp-node-icon")].map((t) => t.closest("[data-of]")?.getAttribute("data-of") + ":" + t.textContent),
      blocked: [...svg.querySelectorAll('polyline[data-blocked="1"]')].map((e) => e.getAttribute("data-edge")).sort(),
      keys: [...svg.querySelectorAll(".dp-edge-legend text")].map((t) => t.textContent),
      open: [...svg.querySelectorAll('polyline[data-key="back-door path, open"]')].map((e) => e.getAttribute("data-edge")).sort(),
      lanes: [...new Set([...svg.querySelectorAll("[data-lane]")].map((e) => e.getAttribute("data-lane")))],
      shd: svg.getAttribute("data-bn-shd"),
      summary: svg.getAttribute("data-summary") || "",
    };
  }, k);
let c = await causal(0);
check(JSON.stringify(c.ring) === '["engaged"]', "the minimal set: Engaged customer ringed", c);
check(c.letters.includes("discount:X") && c.letters.includes("stay:Y"), "X on the treatment, Y on the outcome", c);
check(JSON.stringify(c.blocked) === '["engaged>stay","engaged>usage"]', "the back-door paths blocked after Engaged", c);
check(c.keys.join("|") === "back-door path, blocked|causal path", "the legend", c.keys);
c = await causal(1);
check(c.letters.includes("usage:M") && c.open.length === 2, "engagement unmeasured: the front-door mediator M, the back-door path open", c);
await page.locator('select:has(option[value="none"])').first().selectOption("none");
await page.waitForFunction(() => !document.querySelectorAll("svg[data-diagram]")[0].querySelector(".dp-node-ring"), null, { timeout: 30000 }).catch(() => {});
c = await causal(0);
check(!c.ring.length && c.open.includes("engaged>stay"), "no adjustment: nothing ringed, the back-door path open", c);
await page.locator('svg[data-diagram] rect[data-node="discount"]').first().hover();
await page.waitForTimeout(200);
const kicker = await page.evaluate(() => document.querySelector(".dp-tip.show .tip-kicker")?.textContent || "");
check(/Treatment \(X\)/.test(kicker), "the card names the treatment", kicker);
await page.mouse.move(5, 5);
await clickAt(await rowAt(0, "discount", "yes"));
let seen = await state(0);
await clickAt(await rowAt(0, "discount", "yes"), { alt: true });
let done = await state(0);
check(seen.vals["stay.yes"] && done.vals["stay.yes"] && seen.vals["stay.yes"] !== done.vals["stay.yes"] && done.hand, "seeing then doing: renewal differs, a hand on the discount", [seen.vals, done.vals]);
await page.getByText("What if", { exact: true }).first().click();
await page.waitForFunction(() => [...document.querySelectorAll("svg[data-diagram]")].some((s) => s.querySelector('[data-lane="Counterfactual world"]')), null, { timeout: 60000 }).catch(() => {});
const twin = await page.evaluate(() => {
  const svg = [...document.querySelectorAll("svg[data-diagram]")].find((s) => s.querySelector('[data-lane="Counterfactual world"]'));
  return svg ? { summary: svg.getAttribute("data-summary") || "", copies: svg.querySelectorAll('rect[data-node$="*"]').length, hand: !!svg.querySelector('[data-of="discount*"] .dp-node-icon, [data-of="discount*"].dp-node-icon') } : null;
});
check(twin && twin.copies === 3 && /^If Got the discount had been set to no: /.test(twin.summary), "the twin: three counterfactual copies, what the change did", twin);
// the same page, after the dropdown changes above: the slider still re-runs
// the tab (#1304 — the diagram reads it through a TEMP VIEW)
await page.getByText("Did a learner find it", { exact: true }).first().click();
await page.waitForFunction(() => !!document.querySelector("svg[data-bn-shd]"), null, { timeout: 60000 }).catch(() => {});
const diff = () =>
  page.evaluate(() => {
    const svg = document.querySelector("svg[data-bn-shd]");
    return svg ? { shd: svg.getAttribute("data-bn-shd"), keys: [...svg.querySelectorAll(".dp-edge-legend text")].map((t) => t.textContent) } : null;
  });
let d = await diff();
check(d && d.shd === "3" && d.keys.join("|") === "matching (6)|reversed (1)|extra (1)|missing (1)|structural Hamming distance 3", "the structure diff at 0.5", d);
const slider = page.locator("#dp-form-cz_learn-threshold");
if (await slider.count()) {
  // a click at about 0.3 (snaps to a 0.05 step: 0.25 or 0.3)
  await slider.scrollIntoViewIfNeeded();
  await page.waitForTimeout(1500);
  const sb = await slider.boundingBox();
  await page.mouse.click(sb.x + sb.width * 0.3, sb.y + sb.height / 2);
  await page.waitForFunction(() => document.querySelector("svg[data-bn-shd]")?.textContent.includes("matching (7)"), null, { timeout: 30000 }).catch(() => {});
  d = await diff();
  check(d && d.keys[0] === "matching (7)" && d.keys.includes("extra (2)") && !d.keys.some((k) => k.startsWith("missing")), "lower, the weak arc matches and another extra one comes in", d);
}
if (shot) {
  for (const scheme of ["light", "dark"]) {
    await page.emulateMedia({ colorScheme: scheme });
    await page.waitForTimeout(600);
    for (const [tab, name] of [["Seeing or doing", "adjust"], ["What if", "twin"], ["Did a learner find it", "diff"]]) {
      await page.getByText(tab, { exact: true }).first().click();
      await page.waitForTimeout(2500);
      await page.mouse.move(5, 5);
      await page.screenshot({ path: `${shot}-${scheme}-causal-${name}.png`, fullPage: true });
    }
  }
}

check(!errors.length, "no page errors", errors);
await browser.close();
if (bad) {
  console.error(`bayes-net-e2e: ${bad} failed`);
  process.exit(1);
}
console.log("bayes-net-e2e: all checks passed");
