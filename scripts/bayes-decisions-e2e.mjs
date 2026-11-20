// #1279: decision views on a Bayesian network in a real browser
// (DuckDB-Wasm, the bayes-net plugin from plugins.json).
//
// "Should we drill, and is the seismic test worth paying for?": each
// decision's policy is a table in its box, information arcs are dotted, the
// payoff's expected value is in its diamond; a click on an option decides it,
// a click on a test result observes it, and the row of the policy that the
// evidence picks lights up.
// "The printer won't print: what do we try first?": the plan starts with the
// toner light; answering it (a click on "on") re-plans — the toner goes
// first — and a repair that leaves the printer "still broken" drops out of
// the plan. The second tab searches the answers that point to a fault,
// exhaustive and greedy.
//
//   scripts/build-wasm-pkg.sh && scripts/web-plugins.sh web
//   python3 serve_nocache.py 8947 &
//   NODE_PATH=<dir with playwright>/node_modules node scripts/bayes-decisions-e2e.mjs [--url http://127.0.0.1:8947/] [--shot out-prefix]
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

const svg = () => page.locator("svg[data-diagram]").first();
// the centre of a cell (a policy's option, or a state row's name)
async function cellAt(node, state, nth = 0) {
  return page.evaluate(
    ([node, state, nth]) => {
      const s = document.querySelector("svg[data-diagram]");
      const t = [...s.querySelectorAll(`[data-of="${node}"] text[data-state="${state}"]`)][nth];
      if (!t) return null;
      const b = t.getBoundingClientRect();
      if (b.top < 60 || b.bottom > innerHeight - 60) t.scrollIntoView({ block: "center" });
      const r = t.getBoundingClientRect();
      return { x: r.left + r.width / 2, y: r.top + r.height / 2 };
    },
    [node, state, nth]
  );
}
async function clickAt(p) {
  const before = await page.evaluate(() => document.querySelector("svg[data-diagram]")?.getAttribute("data-summary"));
  await page.mouse.click(p.x, p.y);
  await page
    .waitForFunction((b) => document.querySelector("svg[data-diagram]")?.getAttribute("data-summary") !== b, before, { timeout: 30000 })
    .catch(() => {});
  await page.waitForTimeout(400);
}
const drawing = () =>
  page.evaluate(() => {
    const s = document.querySelector("svg[data-diagram]");
    const texts = (sel) => [...s.querySelectorAll(sel)].map((t) => t.closest("[data-of]")?.getAttribute("data-of") + ":" + t.textContent);
    return {
      summary: s.getAttribute("data-summary") || "",
      chosen: texts("text.dp-node-table-chosen"),
      current: [...s.querySelectorAll("rect.dp-node-table-current")].map((r) => r.closest("[data-of]")?.getAttribute("data-of")),
      dotted: [...s.querySelectorAll("polyline[data-edge]")].filter((e) => /^0[ ,]/.test(e.getAttribute("stroke-dasharray") || "")).map((e) => e.getAttribute("data-edge")),
      values: Object.fromEntries([...s.querySelectorAll("text.dp-node-state-value[data-state]")].map((t) => [t.closest("[data-of]")?.getAttribute("data-of") + "." + t.getAttribute("data-state"), t.textContent])),
      diamonds: texts("text.dp-node-value"),
    };
  });
const kpis = () => page.evaluate(() => [...document.querySelectorAll(".metric-value")].map((k) => k.textContent.trim()));

// ---- the oil wildcatter ---------------------------------------------------
await page.goto(base + "?dashboard=should-we-drill-and-is-the-seismic-test-worth-paying-for");
await page.waitForFunction(() => document.querySelector("svg[data-diagram] .dp-node-table-chosen"), null, { timeout: 120000 });
await page.waitForTimeout(500);
let d = await drawing();
check(d.chosen.includes("test:22.5") && d.chosen.includes("drill:77.5"), "the policies: test (22.5), drill after a closed result (77.5)", d.chosen);
check(d.chosen.includes("drill:-10"), "walk away after a diffuse result", d.chosen);
check(!d.current.length, "nothing observed: no current row", d.current);
check(["test>drill", "seismic>drill"].every((e) => d.dotted.includes(e)) && !d.dotted.includes("oil>seismic"), "information arcs dotted, the others not", d.dotted);
check(d.diamonds.includes("payoff:32.5") && d.diamonds.includes("test_cost:-10"), "the payoffs' expected values in their diamonds", d.diamonds);
check(/decide Drill\?: /.test(d.summary) && /decide Test\?: test \(expected utility 22\.5\)/.test(d.summary), "the summary says what to decide", d.summary);
const k0 = await kpis();
check(k0.includes("22.5") && k0.includes("12.5"), "MEU and the test's worth as KPIs", k0);

// the decision's card: its policy
await page.locator('svg[data-diagram] rect[data-node="drill"]').first().hover();
await page.waitForTimeout(250);
const card = await page.evaluate(() => {
  const t = document.querySelector(".dp-tip.show");
  return t ? { policy: !!t.querySelector(".tip-bn-policy"), chosen: [...t.querySelectorAll(".tip-bn-chosen")].map((c) => c.textContent) } : null;
});
check(card && card.policy && card.chosen.includes("77.5"), "the decision's card shows the policy", card);
await page.mouse.move(5, 5);

// decide to test, then observe a diffuse result: that row lights up
await clickAt(await cellAt("test", "test"));
await clickAt(await cellAt("seismic", "diffuse"));
d = await drawing();
check(d.current.includes("drill"), "test + diffuse: the policy's row lights up", d.current);
check(d.values["oil.dry"] === "73%", "the oil is likely dry", d.values);
check(/given Test\? = test, Seismic = diffuse/.test(d.summary) && /decide Drill\?: walk away/.test(d.summary), "the policy walks away", d.summary);
check(d.diamonds.includes("payoff:0"), "walking away pays 0", d.diamonds);
// overrule it: decide to drill anyway (a click in the drill column of the current row)
await clickAt(await cellAt("drill", "drill", 3));
d = await drawing();
check(d.diamonds.some((x) => x.startsWith("payoff:-30.4")), "drilling anyway: the expected payoff falls", d.diamonds);
if (shot) {
  for (const scheme of ["light", "dark"]) {
    await page.emulateMedia({ colorScheme: scheme });
    await page.waitForTimeout(600);
    await page.evaluate(() => document.activeElement?.blur?.());
    await page.mouse.move(5, 5);
    await page.evaluate(() => window.scrollTo(0, 0));
    await page.waitForTimeout(800);
    await page.screenshot({ path: `${shot}-${scheme}-oil.png`, fullPage: true });
  }
  await page.emulateMedia({ colorScheme: "light" });
}

// ---- the printer ------------------------------------------------------------
await page.goto(base + "?dashboard=the-printer-won-t-print-what-do-we-try-first");
await page.waitForFunction(() => document.querySelector("svg[data-diagram]") && [...document.querySelectorAll("table")].some((t) => /Toner light/.test(t.textContent)), null, { timeout: 120000 });
await page.waitForTimeout(500);
const plan = () =>
  page.evaluate(() => {
    const t = [...document.querySelectorAll("table")].find((x) => /what to do/i.test(x.textContent));
    return t ? [...t.querySelectorAll("tbody tr")].map((r) => [...r.querySelectorAll("td")].map((c) => c.textContent.trim())) : [];
  });
let p = await plan();
check(p.length === 5 && p[0].includes("Toner light") && p[0].includes("test"), "the plan: look at the toner light first", p);
check(p.at(-1).some((c) => c === "26.49"), "its expected cost: 26.49", p.at(-1));
await page.locator(".panel", { hasText: "Toner light first or repair directly" }).last().scrollIntoViewIfNeeded();
const wf = await page
  .waitForFunction(() => [...document.querySelectorAll("svg")].some((s) => /Toner light first or repair directly/.test(s.closest(".panel")?.textContent || "") && s.querySelectorAll("rect[data-x]").length >= 4), null, { timeout: 15000 })
  .then(() => true, () => false);
check(wf, "the waterfall: repair directly, the test, each answer, test first");

// answer: the light is on — the toner goes first
await clickAt(await cellAt("light", "on"));
p = await plan();
check(p[0].includes("Replace the toner") && p[0].includes("repair"), "light on: replace the toner first", p[0]);
// the toner did not fix it: it drops out
await clickAt(await cellAt("after_toner", "still broken"));
p = await plan();
check(p.length && !p.some((r) => r.includes("Replace the toner")), "still broken after the toner: it drops out of the plan", p);
d = await drawing();
check(d.values["fault.toner"] === "0%", "the toner is ruled out", d.values);
if (shot) {
  for (const scheme of ["light", "dark"]) {
    await page.emulateMedia({ colorScheme: scheme });
    await page.waitForTimeout(600);
    await page.evaluate(() => document.activeElement?.blur?.());
    await page.mouse.move(5, 5);
    await page.evaluate(() => window.scrollTo(0, 0));
    await page.waitForTimeout(800);
    await page.screenshot({ path: `${shot}-${scheme}-printer.png`, fullPage: true });
  }
  await page.emulateMedia({ colorScheme: "light" });
}

// the evidence search
await page.locator("button.tab-btn", { hasText: "Which answers point to a fault?" }).first().click();
await page.waitForFunction(() => [...document.querySelectorAll("table")].some((t) => /greedy/.test(t.textContent)), null, { timeout: 60000 }).catch(() => {});
await page.waitForTimeout(800);
const search = await page.evaluate(() => {
  const t = [...document.querySelectorAll("table")].find((x) => /greedy/.test(x.textContent) && x.getBoundingClientRect().width > 0);
  return t ? [...t.querySelectorAll("tbody tr")].map((r) => [...r.querySelectorAll("td")].map((c) => c.textContent.trim())) : [];
});
check(search.length === 8 && search.filter((r) => r.includes("greedy")).length === 2 && search.filter((r) => r.includes("exhaustive")).length === 6, "the search: three exhaustive and one greedy per goal", search);
// given the light on and the toner still broken: the other PC and the test page printing point to the driver
check(search.some((r) => r.includes("Other PC prints, Test page prints") && r.includes("96.3%")), "other PC and test page print: 96 % the driver", search);
const ranked = (goal, method) => search.find((r) => r[0] === goal && r[1] === method)?.[3];
check(ranked("points to it", "greedy") === ranked("points to it", "exhaustive") && ranked("points away", "greedy") === ranked("points away", "exhaustive"), "greedy finds the exhaustive best here", search);
if (shot) {
  for (const scheme of ["light", "dark"]) {
    await page.emulateMedia({ colorScheme: scheme });
    await page.waitForTimeout(600);
    await page.mouse.move(5, 5);
    await page.screenshot({ path: `${shot}-${scheme}-search.png`, fullPage: true });
  }
}

check(!errors.length, "no page errors", errors);
await browser.close();
if (bad) {
  console.error(`bayes-decisions-e2e: ${bad} failed`);
  process.exit(1);
}
console.log("bayes-decisions-e2e: all checks passed");
