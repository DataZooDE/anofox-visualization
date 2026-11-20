// #1282: "clarify the graph" in a real browser (DuckDB-Wasm, the bayes-net
// plugin from plugins.json). The example "Which cause-and-effect questions
// must our experts still settle?" draws the learned network with its open
// arc (no arrowheads) and the expert-set one (✓ badge, who / when / why on
// hover), ranks the questions, and takes an answer through a ::FORM
// (buttons, rationale, name) and ::SAVE into the `answers` table: the arc
// becomes the expert's, the disagreement is settled, the effect's range
// collapses and the decision reads "pays off".
//
//   scripts/build-wasm-pkg.sh && scripts/web-plugins.sh web
//   python3 serve_nocache.py 8947 &
//   NODE_PATH=<dir with playwright>/node_modules node scripts/bayes-clarify-e2e.mjs [--url http://127.0.0.1:8947/] [--shot out-prefix]
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
  const page = await browser.newPage({ viewport: { width: 1400, height: 1100 }, colorScheme: scheme, reducedMotion: "reduce" });
  page.on("pageerror", (e) => errors.push(String(e)));
  await page.goto(base + "?dashboard=which-cause-and-effect-questions-must-our-experts-still-settle");
  await page.waitForFunction(() => document.querySelector("svg[data-diagram]") && document.querySelector(".dp-form"), null, { timeout: 120000 });
  await page.waitForTimeout(500);
  return page;
}
const page = await open("light");
const look = () =>
  page.evaluate(() => {
    const svg = document.querySelector("svg[data-diagram]");
    const edge = (id) => svg.querySelector(`polyline[data-edge="${id}"]`);
    const tables = [...document.querySelectorAll(".panel")].map((p) => ({
      title: p.querySelector(".panel-title")?.textContent || "",
      rows: [...p.querySelectorAll("tbody tr")].map((r) => [...r.children].map((c) => c.textContent.trim())),
    }));
    return {
      summary: svg.getAttribute("data-summary") || "",
      open: [...svg.querySelectorAll('polyline[data-undirected="1"]')].map((e) => e.getAttribute("data-edge")),
      arrows: svg.querySelectorAll('[data-part="arrow"]').length,
      edges: svg.querySelectorAll("polyline[data-edge]").length,
      labels: [...svg.querySelectorAll("text")].map((t) => t.textContent).filter((t) => t.startsWith("✓")),
      texts: [...svg.querySelectorAll("text")].map((t) => t.textContent),
      colours: Object.fromEntries([...svg.querySelectorAll("polyline[data-edge]")].map((e) => [e.getAttribute("data-edge"), e.getAttribute("stroke")])),
      lungTip: edge("smoke>lung")?.getAttribute("data-tip") || edge("smoke>lung")?.querySelector("title")?.textContent || "",
      broncTip: (edge("smoke>bronc") || edge("bronc>smoke"))?.getAttribute("data-tip") || "",
      verdict: [...document.querySelectorAll(".textcard .text-value")].map((e) => e.textContent.trim())[0] || "",
      buttons: [...document.querySelectorAll('.dp-form-field[data-field="answer"] button')].map((b) => b.textContent),
      decides: !!document.querySelector('.dp-form-field[data-field="decides"]'),
      formTitle: document.querySelector(".dp-form .panel-title")?.textContent || "",
      tables,
      xline: !!document.querySelector(".panel svg [data-xline], .panel svg .dp-xline") || /Decided/.test([...document.querySelectorAll(".panel svg text")].map((t) => t.textContent).join("|")),
    };
  });
const rowsOf = (s, re) => (s.tables.find((t) => t.rows.length && t.rows.some((r) => re.test(r.join(" ")))) || { rows: [] }).rows;

let s = await look();
check(JSON.stringify(s.open) === JSON.stringify(["smoke>bronc"]), "the open arc Smoker – Bronchitis is undirected", s.open);
check(s.arrows === s.edges - 1, "it has no arrowhead, every other arc one", { arrows: s.arrows, edges: s.edges });
check(/direction open: Smoker – Bronchitis/.test(s.summary), "the screen-reader text names it", s.summary);
check(/open: which way\? The data lean 52% Smoker → Bronchitis/.test(s.broncTip), "hover says how the data lean", s.broncTip);
check(s.labels.includes("✓ dr-a"), "the expert-set arc carries dr-a's badge", s.labels);
check(
  s.colours["smoke>bronc"] !== s.colours["either>dysp"] && s.colours["smoke>lung"] !== s.colours["either>dysp"] && s.colours["smoke>bronc"] !== s.colours["smoke>lung"],
  "the open arc and the expert arc have colours of their own",
  s.colours
);
check(s.texts.includes("Open: the data can't tell") && s.texts.includes("Settled by an expert"), "the legend under the graph names them", s.texts.slice(-4));
check(/set by dr-a on 2026-11-12: Cohort studies/.test(s.lungTip), "…and who, when and why on hover", s.lungTip);
check(s.verdict === "open", "the decision is still open", s.verdict);
const qs = rowsOf(s, /lead to bronchitis/).filter((r) => /^\d+$/.test(r[0]));
check(qs.length === 2 && /bronchitis/.test(qs[0][1]) && /decides/.test(qs[0].join(" ")), "the questions, the bronchitis one first (it decides)", qs);
check(JSON.stringify(s.buttons) === JSON.stringify(["Smoker → Bronchitis", "Bronchitis → Smoker", "No direct link", "Not sure"]), "four answer buttons, in the question's words", s.buttons);
check(s.decides && /bronchitis/.test(s.formTitle), "a disputed question: the card offers to settle it", { decides: s.decides, title: s.formTitle });
check(rowsOf(s, /dr-c/).filter((r) => /dr-b|dr-c/.test(r.join(" "))).length >= 2, "the disagreement shows both experts", rowsOf(s, /dr-c/));
check(!s.xline, "no decision marker yet");

// the question picker takes the card to another question, and back
const pick = async (q, re) => {
  await page.selectOption('.control[data-varname="q"] select', q);
  await page.waitForFunction((re) => new RegExp(re).test(document.querySelector(".dp-form .panel-title")?.textContent || ""), re, { timeout: 30000 }).catch(() => {});
  return page.evaluate(() => document.querySelector(".dp-form .panel-title")?.textContent || "");
};
check(/lung cancer/.test(await pick("smoke|lung", "lung cancer")), "the picker takes the card to the other question");
check(/bronchitis/.test(await pick("smoke|bronc", "bronchitis")), "…and back");

if (shot) await page.screenshot({ path: shot + "-before-light.png", fullPage: true });

// ---- answer as the approver: Smoker → Bronchitis, settles the disagreement ----
const settle = async () => {
  await page.waitForTimeout(700);
  await page.waitForFunction(() => !document.body.classList.contains("loading"), null, { timeout: 60000 });
};
await page.locator('.dp-form-field[data-field="answer"] button', { hasText: "Smoker → Bronchitis" }).click();
await settle();
check((await page.locator('.dp-form-field[data-field="answer"] button[aria-checked="true"]').textContent()) === "Smoker → Bronchitis", "the pressed button is the answer");
await page.fill('.dp-form-field[data-field="rationale"] input', "Smoke damages the airway lining first; the cough follows.");
await page.keyboard.press("Tab");
await settle();
await page.fill('.dp-form-field[data-field="who"] input', "dr-d");
await page.keyboard.press("Tab");
await settle();
await page.locator('.dp-form-field[data-field="decides"] input').check();
await settle();
await page.locator('.save-panel .submit-btn:text-is("Submit answer")').click();
await page.waitForFunction(() => /pays off/.test(document.querySelector(".textcard .text-value")?.textContent || ""), null, { timeout: 30000 }).catch(() => {});
await settle();
s = await look();
check(s.verdict === "pays off", "the answer settles it: the programme pays off", s.verdict);
check(s.open.length === 0 && s.arrows === s.edges, "no open arc left, every arc has its head", { open: s.open, arrows: s.arrows });
check(s.labels.includes("✓ dr-d"), "the bronchitis arc now carries dr-d's badge", s.labels);
check(s.xline, "the progress chart marks where it was decided");
const hist = rowsOf(s, /dr-d/);
check(hist.some((r) => r[0] === "dr-d" && r.includes("decision")), "the history has dr-d's decision", hist);
check(!rowsOf(s, /dr-b/).some((r) => r.length === 5 && /dr-b/.test(r[1] || "")), "the disagreement table is empty");
check(await page.evaluate(() => /Saved 'smoke\|bronc' into answers/.test(document.body.textContent)), "the save said where it went");
if (shot) await page.screenshot({ path: shot + "-after-light.png", fullPage: true });

if (shot) {
  const dark = await open("dark");
  await dark.screenshot({ path: shot + "-dark.png", fullPage: true });
}
check(!errors.length, "no page errors", errors);
await browser.close();
console.log(bad ? `bayes-clarify-e2e: ${bad} failed` : "bayes-clarify-e2e: all checks passed");
process.exit(bad ? 1 : 0);
