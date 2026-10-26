// #573: a ::HEIGHT pending before a ::VIEW group's first marker must apply
// to every alternative, not just the first — otherwise the card jumps in
// height when switching views (247px -> 526px in the issue's own repro). An
// alternative's own ::HEIGHT (set directly before its own marker) still
// wins over the group's. Mirrors scripts/view-col-width-e2e.mjs (#470),
// which already covers ::COL for a ::VIEW group the same way.
//
//   cargo build --lib --target wasm32-unknown-unknown --profile wasm --no-default-features --features wasm
//   wasm-bindgen --target web --out-dir web/pkg --out-name anofox_visualization <target>/wasm32-unknown-unknown/wasm/anofox_visualization.wasm
//   python3 serve_nocache.py 8798 &
//   NODE_PATH=<dir with playwright>/node_modules node scripts/view-height-e2e.mjs [--url http://127.0.0.1:8798/]
import { createRequire } from "module";

const require = createRequire(import.meta.url);
const { chromium } = require("playwright");
const arg = (k, d) => {
  const i = process.argv.indexOf(k);
  return i > 0 ? process.argv[i + 1] : d;
};
const base = arg("--url", "http://127.0.0.1:8798/");
const b64 = (s) => Buffer.from(s, "utf8").toString("base64");
let bad = 0;
const check = (ok, m) => (ok ? console.log("ok   " + m) : (bad++, console.error("FAIL " + m)));

// The issue's own repro (#573), verbatim.
const SQL = `SELECT 6::COL; SELECT 112::HEIGHT;
SELECT 'A' AS g ::VIEW;
SELECT i::XAXIS, i::LINECHART, 'A'::TITLE FROM range(5) t(i);
SELECT 'B' AS g ::VIEW;
SELECT i::XAXIS, i * 2 AS y ::LINECHART, 'B'::TITLE FROM range(5) t(i);
SELECT 1::ENDVIEW;
`;

// A second group: each alternative sets its OWN explicit ::HEIGHT, to
// different values — an alternative's own ::HEIGHT must still win over the
// group's, so these two must differ (not be forced equal by the #573 fix).
const SQL2 = `SELECT 112::HEIGHT;
SELECT 'A' AS h2 ::VIEW;
SELECT i::XAXIS, i::LINECHART, 'A'::TITLE FROM range(5) t(i);
SELECT 260::HEIGHT;
SELECT 'B' AS h2 ::VIEW;
SELECT i::XAXIS, i * 2 AS y ::LINECHART, 'B'::TITLE FROM range(5) t(i);
SELECT 1::ENDVIEW;
`;

const browser = await chromium.launch();
const settled = (page) =>
  page.waitForFunction(
    () => /\d+ panels?/.test(document.getElementById("status")?.textContent || "") && !document.body.classList.contains("loading"),
    null,
    { timeout: 120000 }
  );
// Only a VISIBLE pane's lazy jobs ever fire (see view-placement-e2e.mjs's
// own comment on paneReady) — ignore a .dp-lazy under any ancestor hidden
// with inline `display: none`.
const paneReady = (page) =>
  page
    .waitForFunction(
      () => {
        const hidden = (el) => {
          for (let n = el; n; n = n.parentElement) if (n.style && n.style.display === "none") return true;
          return false;
        };
        return ![...document.querySelectorAll(".dash .dp-lazy")].some((el) => !hidden(el));
      },
      null,
      { timeout: 120000 }
    )
    .then(() => page.waitForTimeout(300));

async function open(sql) {
  const page = await browser.newPage({ viewport: { width: 1920, height: 1080 }, reducedMotion: "reduce" });
  const errs = [];
  page.on("pageerror", (e) => errs.push(String(e)));
  await page.goto(`${base}#sql=${b64(sql)}`);
  await settled(page);
  await paneReady(page);
  return { page, errs };
}

// The visible pane's card: its own inline `min-height` (what ::HEIGHT /
// app.js's `nextHeight` actually sets — `panelBody`'s `fig.style.minHeight`)
// and its rendered box height, found via the switcher's data-view-id.
const cardHeight = (page, id) =>
  page.evaluate((id) => {
    const bar = document.querySelector(`.dp-viewbar[data-view-id="${id}"]`);
    const sec = bar?.closest(".dp-views");
    const pane = [...(sec?.querySelectorAll(".dp-viewpane") || [])].find((p) => p.style.display !== "none");
    const card = pane?.querySelector(".panel");
    return card
      ? { minHeight: parseFloat(card.style.minHeight) || null, rendered: card.getBoundingClientRect().height }
      : { minHeight: null, rendered: null };
  }, id);

const clickAlt = (page, id, label) => page.click(`.dp-viewbar[data-view-id="${id}"] [role="tab"]:has-text("${label}")`);

// (a) the issue's own repro: ::HEIGHT only set once, before the group's
// first marker — both alternatives must render at the same height.
{
  const { page, errs } = await open(SQL);
  const bar = await page.$(`.dp-viewbar[data-view-id="g"]`);
  check(!!bar, "(a) the group's switcher rendered");
  const hA = await cardHeight(page, "g");
  check(hA.minHeight === 112, `(a) alternative A gets the pending ::HEIGHT as its min-height (112px, got ${hA.minHeight})`);
  await clickAlt(page, "g", "B");
  await page.waitForTimeout(200);
  const hB = await cardHeight(page, "g");
  check(hB.minHeight === 112, `(a) alternative B ALSO gets the group's ::HEIGHT (112px, got ${hB.minHeight}) — before #573's fix this was null (the default)`);
  check(Math.abs(hB.rendered - hA.rendered) < 2, `(a) alternative B's rendered card height (${hB.rendered}) matches A's (${hA.rendered}) — no jump when switching`);
  check(!errs.length, `(a) no page errors: ${errs.join(" | ")}`);
  await page.close();
}

// (b) an alternative's OWN pending ::HEIGHT (set right before its own
// marker) still wins over the group's shared one.
{
  const { page, errs } = await open(SQL2);
  const hA = await cardHeight(page, "h2");
  check(hA.minHeight === 112, `(b) alternative A keeps the group's shared height (112px, got ${hA.minHeight})`);
  await clickAlt(page, "h2", "B");
  await page.waitForTimeout(200);
  const hB = await cardHeight(page, "h2");
  check(hB.minHeight === 260, `(b) alternative B's own ::HEIGHT (260px, got ${hB.minHeight}) overrides the group's`);
  check(!errs.length, `(b) no page errors: ${errs.join(" | ")}`);
  await page.close();
}

await browser.close();
console.log(bad ? `${bad} FAILED` : "all ok");
process.exit(bad ? 1 : 0);
