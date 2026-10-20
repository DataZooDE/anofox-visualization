// Browser e2e for hovering a mark focusing its series (#213): grouped /
// stacked bars, lines, scatter and a faceted chart dim every other series on
// mouseenter (and on keyboard focus) and clear on mouseleave/blur/Escape;
// opted out (single-series bars, 'none'::HOVER_FOCUS, a chart above the
// 2,000-mark cap) stays plain hover; a sticky click selection wins over a
// later hover; a touch tap is a click, never a hover; the change is instant
// under reduced motion, and a 1,800-point chart still focuses in well under
// the frame budget.
//
//   wasm-pack build --target web --out-dir web/pkg --profile wasm --no-default-features --features wasm
//   python3 serve_nocache.py 8813 &
//   NODE_PATH=<playwright>/node_modules node scripts/hover-focus-e2e.mjs [--url http://127.0.0.1:8813/]
import { createRequire } from "module";

const require = createRequire(import.meta.url);
const { chromium } = require("playwright");
const arg = (k, d) => {
  const i = process.argv.indexOf(k);
  return i > 0 ? process.argv[i + 1] : d;
};
const base = arg("--url", "http://127.0.0.1:8813/");
const fail = (m) => {
  console.error("FAIL: " + m);
  process.exitCode = 1;
};
const ok = (m) => console.log("ok: " + m);
const b64 = (s) => Buffer.from(s, "utf8").toString("base64");

const SQL = `
SELECT 'Hover focus e2e'::DASHBOARD_TITLE;
SELECT m::XAXIS, y::CATEGORY, v::BARCHART, 'Grouped'::TITLE FROM (VALUES ('Jan','2024',3),('Feb','2024',4),('Mar','2024',2),('Jan','2025',5),('Feb','2025',6),('Mar','2025',3)) t(m, y, v) ORDER BY y, m;
SELECT m::XAXIS, y::CATEGORY, v::BARCHART_STACKED, 'Stacked'::TITLE FROM (VALUES ('Jan','2024',3),('Feb','2024',4),('Mar','2024',2),('Jan','2025',5),('Feb','2025',6),('Mar','2025',3)) t(m, y, v) ORDER BY y, m;
SELECT d::XAXIS, s::CATEGORY, v::LINECHART, 'Lines'::TITLE FROM (SELECT i AS d, s, (i * CASE s WHEN 'A' THEN 3 ELSE 5 END) % 11 AS v FROM range(1, 9) r(i), (VALUES ('A'), ('B')) c(s)) ORDER BY s, d;
SELECT x::XAXIS, g::CATEGORY, y::SCATTER, 'Scatter'::TITLE FROM (SELECT i AS x, CASE WHEN i % 2 = 0 THEN 'even' ELSE 'odd' END AS g, (i * 7) % 11 AS y FROM range(1, 21) t(i));
SELECT m::XAXIS, y::CATEGORY, v::BARCHART, r::FACET, 'Faceted'::TITLE FROM (VALUES ('Jan','2024',3,'EU'),('Feb','2024',4,'EU'),('Jan','2025',5,'EU'),('Feb','2025',6,'EU'),('Jan','2024',2,'US'),('Feb','2024',1,'US'),('Jan','2025',4,'US'),('Feb','2025',3,'US')) t(m, y, v, r) ORDER BY r, y, m;
SELECT m::XAXIS, v::BARCHART, 'Single'::TITLE FROM (VALUES ('a',1),('b',2),('c',3)) t(m, v);
SELECT m::XAXIS, y::CATEGORY, v::BARCHART, 'none'::HOVER_FOCUS, 'OptOut'::TITLE FROM (VALUES ('Jan','2024',3),('Feb','2024',4),('Jan','2025',5),('Feb','2025',6)) t(m, y, v);
SELECT x::XAXIS, g::CATEGORY, y::SCATTER, 'Dense'::TITLE FROM (SELECT i AS x, CASE WHEN i % 2 = 0 THEN 'a' ELSE 'b' END AS g, (i * 13) % 97 AS y FROM range(0, 2500) t(i));
SELECT x::XAXIS, g::CATEGORY, y::SCATTER, 'Perf'::TITLE FROM (SELECT i AS x, CASE WHEN i % 2 = 0 THEN 'a' ELSE 'b' END AS g, (i * 13) % 97 AS y FROM range(0, 1800) t(i));
`;

const browser = await chromium.launch();
const allErrs = [];

async function open(ctxOpts) {
  const page = await browser.newPage({ viewport: { width: 1400, height: 3200 }, ...ctxOpts });
  const errs = [];
  page.on("pageerror", (e) => errs.push(String(e)));
  allErrs.push(errs);
  await page.goto(`${base}?embed=1#sql=${b64(SQL)}`);
  await page.waitForFunction(
    () =>
      document.querySelectorAll(".dash .panel-svg > svg[aria-label]").length >= 9 &&
      !document.body.classList.contains("loading"),
    null,
    { timeout: 120000 }
  );
  await page.waitForTimeout(300);
  return page;
}

const idle = (page) => page.waitForFunction(() => !document.body.classList.contains("loading"), null, { timeout: 120000 });
const frames = (page) => page.evaluate(() => new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r))));

// Among a panel's data marks, the first whose centre really hits it (nothing
// (e.g. a legend, a sibling panel) covers it after scrollIntoView).
const markAt = (page, title, notSeries = null) =>
  page.evaluate(
    ({ title, notSeries }) => {
      const panel = [...document.querySelectorAll(".dash .panel")].find(
        (p) => p.querySelector(".panel-title")?.textContent.trim() === title
      );
      const svg = panel?.querySelector(".panel-svg > svg");
      if (!svg) return null;
      const cands = [...svg.querySelectorAll("circle.dp-hit, rect.dp-hit")].filter(
        (el) =>
          el.hasAttribute("data-tip") &&
          !el.hasAttribute("data-rule") &&
          (notSeries == null || el.getAttribute("data-series") !== notSeries)
      );
      for (const m of cands) {
        m.scrollIntoView({ block: "center" });
        const r = m.getBoundingClientRect();
        const x = r.left + r.width / 2;
        const y = r.top + r.height / 2;
        if (document.elementFromPoint(x, y)?.closest(".dp-hit") === m) {
          return { x, y, series: m.getAttribute("data-series") };
        }
      }
      return null;
    },
    { title, notSeries }
  );

// The emphasis state of the panel titled `title`: the shared layers it holds
// (`select`, `filter`, `story`, `hoverfocus`, …), `data-hover-focus`, and
// every data mark's `[series, inline opacity]`.
const emphOf = (page, title) =>
  page.evaluate((title) => {
    const panel = [...document.querySelectorAll(".dash .panel")].find(
      (p) => p.querySelector(".panel-title")?.textContent.trim() === title
    );
    const svg = panel.querySelector(".panel-svg > svg");
    const marks = [...svg.querySelectorAll(".dp-hit, polyline[data-series], polygon[data-series]")]
      .filter((el) => !el.hasAttribute("data-rule"))
      .map((el) => [el.getAttribute("data-series"), el.style.opacity]);
    return {
      layers: [...(svg._dpEmph?.keys() ?? [])],
      focusAttr: svg.getAttribute("data-hover-focus"),
      marks,
    };
  }, title);

const focused = (marks, s) => marks.length > 0 && marks.every(([k, op]) => (k === s ? op === "" : op === "0.15"));
const clear = (marks) => marks.every(([, op]) => op === "");

async function checkFocuses(page, label) {
  for (const title of ["Grouped", "Stacked", "Lines", "Scatter", "Faceted"]) {
    await page.mouse.move(1, 1);
    await frames(page);
    const m = await markAt(page, title);
    if (!m) {
      fail(`${label}/${title}: no hoverable mark found`);
      continue;
    }
    await page.mouse.move(m.x, m.y);
    await frames(page);
    let { layers, marks } = await emphOf(page, title);
    if (layers.includes("hoverfocus") && focused(marks, m.series)) ok(`${label}/${title}: hover focuses ${m.series}`);
    else fail(`${label}/${title}: hover did not focus series ${m.series} (layers=${layers})`);

    if (title === "Grouped") {
      const noTransition = await page.evaluate((title) => {
        const panel = [...document.querySelectorAll(".dash .panel")].find(
          (p) => p.querySelector(".panel-title")?.textContent.trim() === title
        );
        const bar = panel.querySelector(".dp-hit");
        return !/opacity|all/.test(getComputedStyle(bar).transitionProperty);
      }, title);
      if (noTransition) ok(`${label}/Grouped: no CSS transition on the dimmed opacity (instant)`);
      else fail(`${label}/Grouped: a CSS transition rides the hover-focus opacity`);
    }

    await page.mouse.move(1, 1);
    await frames(page);
    ({ layers, marks } = await emphOf(page, title));
    if (!layers.includes("hoverfocus") && clear(marks)) ok(`${label}/${title}: clears on mouseleave`);
    else fail(`${label}/${title}: did not clear on mouseleave (layers=${layers})`);
  }
}

async function checkOptedOut(page, label) {
  for (const title of ["Single", "OptOut", "Dense"]) {
    await page.mouse.move(1, 1);
    await frames(page);
    const m = await markAt(page, title);
    if (!m) {
      fail(`${label}/${title}: no mark found`);
      continue;
    }
    await page.mouse.move(m.x, m.y);
    await frames(page);
    const { layers, marks, focusAttr } = await emphOf(page, title);
    if (!layers.includes("hoverfocus") && clear(marks)) ok(`${label}/${title}: stays plain hover`);
    else fail(`${label}/${title}: hover unexpectedly focused a series (layers=${layers})`);
    if (title === "OptOut") {
      if (focusAttr === "none") ok(`${label}/OptOut: data-hover-focus="none"`);
      else fail(`${label}/OptOut: data-hover-focus is "${focusAttr}", expected "none"`);
    }
    await page.mouse.move(1, 1);
    await frames(page);
  }
}

async function checkKeyboard(page, label) {
  await page.mouse.move(1, 1);
  await page.evaluate((title) => {
    const panel = [...document.querySelectorAll(".dash .panel")].find(
      (p) => p.querySelector(".panel-title")?.textContent.trim() === title
    );
    panel.querySelector(".panel-svg > svg").focus();
  }, "Grouped");
  await page.keyboard.press("ArrowRight");
  await frames(page);
  const k = await page.evaluate(() => document.activeElement.getAttribute("data-series"));
  let { marks } = await emphOf(page, "Grouped");
  if (focused(marks, k)) ok(`${label}/keyboard: ArrowRight focuses ${k}`);
  else fail(`${label}/keyboard: ArrowRight on ${k} did not dim other series`);

  await page.keyboard.press("ArrowDown");
  await frames(page);
  const k2 = await page.evaluate(() => document.activeElement.getAttribute("data-series"));
  ({ marks } = await emphOf(page, "Grouped"));
  if (k2 === k) fail(`${label}/keyboard: ArrowDown did not move to a different series (still ${k})`);
  else if (focused(marks, k2)) ok(`${label}/keyboard: ArrowDown focuses ${k2}`);
  else fail(`${label}/keyboard: ArrowDown on ${k2} did not dim other series`);

  await page.keyboard.press("Escape");
  await frames(page);
  ({ marks } = await emphOf(page, "Grouped"));
  if (clear(marks)) ok(`${label}/keyboard: Escape clears`);
  else fail(`${label}/keyboard: Escape did not clear`);
}

async function checkSticky(page, label) {
  const a = await markAt(page, "Grouped");
  if (!a) {
    fail(`${label}/sticky: no mark found`);
    return;
  }
  await page.mouse.click(a.x, a.y);
  await idle(page);
  await page.waitForTimeout(300);
  await frames(page);
  let { layers } = await emphOf(page, "Grouped");
  if (!layers.includes("select")) {
    fail(`${label}/sticky: click did not set a select layer (layers=${layers})`);
    return;
  }
  ok(`${label}/sticky: click sets the select layer`);

  const b = await markAt(page, "Grouped", a.series);
  if (!b) {
    fail(`${label}/sticky: no second mark of a different series found`);
  } else {
    await page.mouse.move(b.x, b.y);
    await frames(page);
    const { layers: l2, marks } = await emphOf(page, "Grouped");
    if (!l2.includes("hoverfocus") && focused(marks, a.series)) ok(`${label}/sticky: hover does not override the selection`);
    else fail(`${label}/sticky: hover changed the emphasis (layers=${l2})`);
  }

  await page.mouse.move(1, 1);
  await page.keyboard.press("Escape");
  await idle(page);
}

async function checkTouch(page, label) {
  await page.evaluate((title) => {
    const panel = [...document.querySelectorAll(".dash .panel")].find(
      (p) => p.querySelector(".panel-title")?.textContent.trim() === title
    );
    const svg = panel.querySelector(".panel-svg > svg");
    svg._dpEmph = svg._dpEmph || new Map();
    const m = svg._dpEmph;
    const origSet = m.set.bind(m);
    m.set = (k, v) => {
      if (k === "hoverfocus") window.__hf = (window.__hf || 0) + 1;
      return origSet(k, v);
    };
    window.__hf = 0;
  }, "Grouped");
  const m = await markAt(page, "Grouped");
  if (!m) {
    fail(`${label}/touch: no mark found`);
    return;
  }
  await page.touchscreen.tap(m.x, m.y);
  await page.waitForTimeout(400);
  const hf = await page.evaluate(() => window.__hf);
  if (hf === 0) ok(`${label}/touch: a tap never set the hoverfocus layer`);
  else fail(`${label}/touch: a tap set the hoverfocus layer ${hf} time(s)`);
}

async function checkPerf(page, label) {
  const { ms, on } = await page.evaluate(async () => {
    const panel = [...document.querySelectorAll(".dash .panel")].find(
      (p) => p.querySelector(".panel-title")?.textContent.trim() === "Perf"
    );
    const svg = panel.querySelector(".panel-svg > svg");
    const mark = svg.querySelector("circle.dp-hit");
    const raf2 = () => new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r)));
    const t0 = performance.now();
    mark.dispatchEvent(new MouseEvent("mouseenter"));
    await raf2();
    const ms = performance.now() - t0;
    const on = (svg._dpEmph?.keys ? [...svg._dpEmph.keys()] : []).includes("hoverfocus");
    mark.dispatchEvent(new MouseEvent("mouseleave"));
    await raf2();
    return { ms, on };
  });
  console.log(`perf: 1800 marks, focus in ${ms.toFixed(1)} ms`);
  if (on && ms < 250) ok(`${label}/perf: focused within budget`);
  else fail(`${label}/perf: focus was not on (${on}) or too slow (${ms.toFixed(1)} ms)`);
}

for (const [label, ctxOpts] of [
  ["reduce", { reducedMotion: "reduce" }],
  ["no-preference", { reducedMotion: "no-preference" }],
]) {
  const page = await open(ctxOpts);
  await checkFocuses(page, label);
  await checkOptedOut(page, label);
  if (label === "reduce") await checkKeyboard(page, label);
  await checkSticky(page, label);
  await page.close();
}

{
  const page = await open({ hasTouch: true, reducedMotion: "reduce" });
  await checkTouch(page, "touch");
  await page.close();
}

{
  const page = await open({ reducedMotion: "reduce" });
  await checkPerf(page, "perf");
  await page.close();
}

for (const errs of allErrs) {
  for (const e of errs) fail(`page error: ${e}`);
}

await browser.close();
console.log(process.exitCode ? "FAIL" : "ok");
