// Lazy-tab performance + behaviour check in a real browser: open a tabbed
// builder example from the sidebar and measure the time until the visible tab
// is fully rendered and the SQL statements run before the first paint (the
// dp:sql / dp:swap / dp:lazy performance entries web/app.js records), with
// lazy tabs and with ?lazy=0 (everything up front) for comparison. Then, with
// ?prefetch=0, check that no hidden tab ran before it was opened, that every
// tab renders when opened (no placeholder left, no error card, charts carry
// their screen-reader summary), and that a cross-filter re-run re-renders
// tabs opened afterwards from the result cache.
//
//   python3 serve_nocache.py 8791 &
//   NODE_PATH=<dir with playwright>/node_modules node scripts/perf-e2e.mjs \
//       [--url http://127.0.0.1:8791/] [--dashboard "Chart gallery"] [--rounds 3]
import { createRequire } from "module";

const require = createRequire(import.meta.url);
const { chromium } = require("playwright");
const arg = (k, d) => {
  const i = process.argv.indexOf(k);
  return i > 0 ? process.argv[i + 1] : d;
};
const base = arg("--url", "http://127.0.0.1:8791/");
const dash = arg("--dashboard", "Chart gallery");
const rounds = Math.max(1, Number(arg("--rounds", "3")) || 1);

const browser = await chromium.launch();
const bad = [];
const fail = (m) => {
  bad.push(m);
  console.log("FAIL: " + m);
};
const idle = (page) =>
  page.waitForFunction(
    () => /\d+ panels?/.test(document.getElementById("status")?.textContent || "") && !document.body.classList.contains("loading"),
    null,
    { timeout: 240000 }
  );
// The shown tab is done: no visible placeholder (lazy) or skeleton (first run).
const shownDone = (page) =>
  page.waitForFunction(() => ![...document.querySelectorAll(".dash .dp-lazy, .dash .dp-skel")].some((e) => e.offsetParent !== null), null, {
    timeout: 240000,
  });

async function open(query, retry = 1) {
  const page = await browser.newPage({ viewport: { width: 1440, height: 1000 }, reducedMotion: "reduce" });
  const errs = [];
  page.on("pageerror", (e) => errs.push(String(e)));
  // (the builder probes for a live /query backend: a static server answers 501)
  page.on("console", (m) => m.type() === "error" && !/status of 501/.test(m.text()) && errs.push(m.text()));
  // Boot on a small example first, so the timing covers opening `dash` only.
  await page.goto(`${base}?dashboard=Overview${query}`);
  try {
    await page.waitForFunction(() => /\d+ panels?/.test(document.getElementById("status")?.textContent || ""), null, { timeout: 60000 });
  } catch (e) {
    // a single-threaded dev server can drop the wasm download: boot once more
    await page.close();
    if (!retry) throw e;
    return open(query, retry - 1);
  }
  await idle(page);
  return { page, errs };
}

// Click the example in the sidebar; time until the shown tab is complete.
async function measure(page) {
  await page.evaluate(() => {
    performance.clearMarks();
    performance.clearMeasures();
    window.__t0 = performance.now();
  });
  await page.click(`.side-item[data-name="${dash}"]`);
  await idle(page);
  await shownDone(page);
  return page.evaluate(() => {
    const t0 = window.__t0;
    const sql = performance.getEntriesByName("dp:sql");
    const swap = performance.getEntriesByName("dp:swap")[0];
    return {
      visibleMs: performance.now() - t0,
      paintMs: swap ? swap.startTime - t0 : NaN,
      sqlBeforePaint: sql.filter((e) => swap && e.startTime < swap.startTime).length,
      lazyBeforePaint: performance.getEntriesByName("dp:lazy").filter((e) => swap && e.startTime < swap.startTime).length,
      renders: performance.getEntriesByName("dp:render").filter((e) => swap && e.startTime < swap.startTime).length,
    };
  });
}

const median = (xs) => [...xs].sort((a, b) => a - b)[Math.floor(xs.length / 2)];
const results = {};
for (const [label, q] of [["lazy", ""], ["eager (?lazy=0)", "&lazy=0"]]) {
  const { page, errs } = await open(q);
  const runs = [];
  for (let r = 0; r < rounds; r++) {
    runs.push(await measure(page));
    await page.click('.side-item[data-name="Overview"]');
    await idle(page);
  }
  const m = {
    visibleMs: median(runs.map((x) => x.visibleMs)),
    firstOpenMs: runs[0].visibleMs,
    paintMs: median(runs.map((x) => x.paintMs)),
    sqlBeforePaint: median(runs.map((x) => x.sqlBeforePaint)),
    rendersBeforePaint: median(runs.map((x) => x.renders)),
  };
  results[label] = m;
  console.log(
    `${label.padEnd(16)} first open ${m.firstOpenMs.toFixed(0)} ms, median: shown tab complete ${m.visibleMs.toFixed(0)} ms, ` +
      `first paint ${m.paintMs.toFixed(0)} ms, ${m.sqlBeforePaint} SQL statements and ${m.rendersBeforePaint} chart renders before it`
  );
  if (errs.length) fail(`${label}: page errors: ${errs.slice(0, 3).join(" | ")}`);
  await page.close();
}
const L = results["lazy"];
const E = results["eager (?lazy=0)"];
if (!(L.sqlBeforePaint * 2 <= E.sqlBeforePaint)) fail(`lazy runs ${L.sqlBeforePaint} statements before the first paint (eager ${E.sqlBeforePaint}): hidden tabs ran up front?`);
if (!(L.rendersBeforePaint < E.rendersBeforePaint)) fail(`lazy renders ${L.rendersBeforePaint} charts before the first paint (eager ${E.rendersBeforePaint})`);

// Behaviour, without the idle prefetch: hidden tabs wait until opened.
{
  const { page, errs } = await open("&prefetch=0");
  await page.click(`.side-item[data-name="${dash}"]`);
  await idle(page);
  await shownDone(page);
  await page.waitForTimeout(1500); // nothing may start on its own meanwhile
  const st = await page.evaluate(() => ({
    hidden: [...document.querySelectorAll(".dash .dp-lazy")].filter((e) => e.offsetParent === null).length,
    jobs: performance.getEntriesByName("dp:lazy").filter((e) => e.startTime > performance.getEntriesByName("dp:swap").at(-1).startTime).length,
  }));
  if (!st.hidden) fail("no hidden tab panel was left for later (placeholders)");
  if (st.jobs) fail(`${st.jobs} hidden-tab panel(s) ran before their tab was opened`);
  const tabs = await page.$$eval(".tabbar > .tab-btn", (bs) => bs.map((b) => b.textContent));
  const check = async (name) => {
    await shownDone(page);
    // lazily rendered panels are wired (hover, a11y summary…) on the next frame
    await page.evaluate(() => new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r))));
    const r = await page.evaluate(() => {
      const vis = (e) => e.offsetParent !== null;
      const panels = [...document.querySelectorAll(".dash .panel")].filter(vis);
      return {
        panels: panels.length,
        errors: [...document.querySelectorAll(".dash .err, .dash .dp-errcard")].filter(vis).map((e) => e.textContent.slice(0, 120)),
        unsummarised: [...document.querySelectorAll(".dash .panel-svg > svg")].filter((s) => vis(s) && !s.dataset.a11y).length,
        busy: [...document.querySelectorAll(".tabpane[aria-busy], .subtabpane[aria-busy]")].filter(vis).length,
      };
    });
    if (!r.panels) fail(`${name}: no panel rendered`);
    for (const e of r.errors) fail(`${name}: ${e}`);
    if (r.unsummarised) fail(`${name}: ${r.unsummarised} chart(s) without the screen-reader summary`);
    if (r.busy) fail(`${name}: pane still aria-busy`);
    return r.panels;
  };
  let n = 0;
  for (const t of tabs) {
    await page.click(`.tabbar > .tab-btn:text-is("${t}")`);
    const subs = await page.$$eval(".tabpane:not([style*='none']) .subtab-btn", (bs) => bs.map((b) => b.textContent));
    if (!subs.length) n += await check(t);
    for (const s of subs) {
      await page.click(`.tabpane:not([style*='none']) .subtab-btn:text-is("${s}")`);
      n += await check(`${t} / ${s}`);
    }
  }
  console.log(`opened ${tabs.length} tabs on demand: ${n} panels rendered`);
  if (await page.$(".dash .dp-lazy")) fail("placeholders left after opening every tab");

  // A cross-filter re-run (a chart mark click) on the first tab, then a tab
  // opened again: its unaffected panels come from the result cache.
  if (tabs.length > 1) {
    await page.click(`.tabbar > .tab-btn:text-is("${tabs[0]}")`);
    await shownDone(page);
    const runs = await page.evaluate(() => performance.getEntriesByName("dp:run").length);
    await page.evaluate(() => {
      performance.clearMarks("dp:cache-hit");
      document.querySelector(".tabpane:not([style*='none']) .panel-svg > svg .dp-hit:not([data-rule])")?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    await page.waitForFunction((n) => performance.getEntriesByName("dp:run").length > n, runs, { timeout: 60000 });
    await page.click(`.tabbar > .tab-btn:text-is("${tabs[1]}")`);
    await check(`${tabs[1]} after a re-run`);
    const hits = await page.evaluate(() => performance.getEntriesByName("dp:cache-hit").length);
    console.log(`re-run + reopen: ${hits} cached result(s) reused`);
    if (!hits) fail("re-opening a tab after a cross-filter re-run reused no cached result");
  }
  if (errs.length) fail(`page errors: ${errs.slice(0, 3).join(" | ")}`);
  await page.close();
}
await browser.close();
if (bad.length) {
  console.log(`\n${bad.length} problem(s)`);
  process.exit(1);
}
console.log("perf-e2e ok");
