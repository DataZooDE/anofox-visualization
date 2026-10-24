// Browser e2e for #459 (slice 1 of #420: plain choropleths via ggplot-rs
// geom_sf instead of our own map SVG writer) — proves the interaction
// contract `src/map/plot.rs`'s new `GGPlot` output still satisfies, per the
// `web/app.js` audit in scratch-420/plan.md:
//   - hover a map feature: its tooltip shows (data-row/-series/-label/-value
//     already come from ggplot-rs's own `Aes::data_attr`/`svg_formatted_values`,
//     not hand-written) and mapHoverEmph highlights its series on every map
//     that has it (svg[data-map] + the per-feature data-series contract).
//   - click a map feature: cross-filters like every other chart (`dpSelected`/
//     `getvariable('selected')`), so a dependent panel re-runs and narrows.
//   - scroll-to-zoom: attachMapZoom's wheel handler re-renders through the
//     same wasm `render` call every map already used (CoordSf's xlim/ylim
//     from `Frame::window`, unchanged by this port) — the panel's data-domain
//     narrows.
//
//   cargo build --lib --target wasm32-unknown-unknown --profile wasm --no-default-features --features wasm
//   wasm-bindgen --target web --out-dir web/pkg <target>/wasm32-unknown-unknown/wasm/anofox_visualization.wasm
//   wasm-opt -O --strip-debug -o web/pkg/anofox_visualization_bg.wasm web/pkg/anofox_visualization_bg.wasm
//   python3 serve_nocache.py 8959 &
//   NODE_PATH=<playwright>/node_modules node scripts/map-geom-sf-e2e.mjs [--url http://127.0.0.1:8959/]
import { createRequire } from "module";

const require = createRequire(import.meta.url);
const { chromium } = require("playwright");
const arg = (k, d) => {
  const i = process.argv.indexOf(k);
  return i > 0 ? process.argv[i + 1] : d;
};
const base = arg("--url", "http://127.0.0.1:8959/");
const fail = (m) => {
  console.error("FAIL: " + m);
  process.exitCode = 1;
};
const ok = (m) => console.log("ok: " + m);
const b64 = (s) => Buffer.from(s, "utf8").toString("base64");

// Two polygons (::CATEGORY, no classes/flows/hex/symbols — slice 1's
// `plot::eligible()`), plus a dependent ::TABLE that narrows to the clicked
// region via `getvariable('selected')`, the same generic cross-filter every
// chart uses.
const SQL = `
SELECT 'Map geom_sf e2e'::DASHBOARD_TITLE;
SELECT 6::COL;
SELECT wkt ::MAP, region ::CATEGORY, region ::LABEL, 'Regions'::TITLE
FROM (VALUES
    ('POLYGON((0 0, 0 10, 10 10, 10 0, 0 0))', 'North'),
    ('POLYGON((10 0, 10 10, 20 10, 20 0, 10 0))', 'South')
) t(wkt, region);
SELECT 6::COL;
SELECT region AS "Region" ::TABLE, 'Filtered by map click'::TITLE
FROM (VALUES ('North'), ('South')) t(region)
WHERE coalesce(getvariable('selected'), '') IN ('', region);
`;

const browser = await chromium.launch();
const page = await browser.newPage({ viewport: { width: 1400, height: 1200 } });
const errs = [];
page.on("pageerror", (e) => errs.push(String(e)));

await page.goto(`${base}?embed=1#sql=${b64(SQL)}`);
// Only the map panel has an <svg> (the dependent panel is a ::TABLE).
await page.waitForFunction(
  () => document.querySelectorAll(".dash .panel-svg > svg[aria-label]").length >= 1 && document.querySelectorAll(".dash table").length >= 1 && !document.body.classList.contains("loading"),
  null,
  { timeout: 120000 }
);
await page.waitForTimeout(300);

const frames = (p) => p.evaluate(() => new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r))));
const idle = (p) => p.waitForFunction(() => !document.body.classList.contains("loading"), null, { timeout: 120000 });

const panel = (title) =>
  page.evaluateHandle(
    (t) => [...document.querySelectorAll(".dash .panel")].find((p) => p.querySelector(".panel-title")?.textContent.trim() === t),
    title
  );

// ---- data-* contract: every map mark carries data-row/-series/-label, the
// root svg carries data-map (#459's new GGPlot path writes these natively
// via Aes::data_attr, same names the old hand-written writer used). ----
const contract = await page.evaluate(() => {
  const panel = [...document.querySelectorAll(".dash .panel")].find((p) => p.querySelector(".panel-title")?.textContent.trim() === "Regions");
  const svg = panel.querySelector(".panel-svg > svg");
  const marks = [...svg.querySelectorAll("[data-row]")];
  return {
    hasDataMap: svg.hasAttribute("data-map"),
    markCount: marks.length,
    everyMarkHasSeriesAndLabel: marks.every((m) => m.hasAttribute("data-series") && m.hasAttribute("data-label")),
    // attachHover()/wireCharts() already ran by now: it reads each mark's
    // <title> once (the universal "this becomes .dp-hit (hoverable/
    // clickable)" gate), moves its text into data-tip, and removes the
    // <title> node — so post-wiring, data-tip + the dp-hit class is the
    // proof the gate fired, not a surviving <title>.
    everyMarkIsWiredHoverable: marks.every((m) => m.classList.contains("dp-hit") && (m.getAttribute("data-tip") || "").length > 0),
  };
});
if (contract.hasDataMap) ok("root svg carries data-map");
else fail("root svg is missing data-map");
if (contract.markCount === 2) ok("two per-feature marks, one per data row");
else fail(`expected 2 marks, found ${contract.markCount}`);
if (contract.everyMarkHasSeriesAndLabel) ok("every mark carries data-series + data-label");
else fail("a mark is missing data-series or data-label");
if (contract.everyMarkIsWiredHoverable) ok("every mark's <title> gated it into .dp-hit with a data-tip (the hoverability contract)");
else fail("a mark was not wired hoverable — its <title> didn't gate it into .dp-hit/data-tip");

// ---- hover: tooltip shows, text names the hovered region ----
const markAt = async (title, region) =>
  page.evaluate(
    ({ title, region }) => {
      const panel = [...document.querySelectorAll(".dash .panel")].find((p) => p.querySelector(".panel-title")?.textContent.trim() === title);
      const svg = panel.querySelector(".panel-svg > svg");
      const m = [...svg.querySelectorAll("[data-row]")].find((el) => el.getAttribute("data-series") === region);
      if (!m) return null;
      m.scrollIntoView({ block: "center" });
      const r = m.getBoundingClientRect();
      return { x: r.left + r.width / 2, y: r.top + r.height / 2 };
    },
    { title, region }
  );

const north = await markAt("Regions", "North");
if (!north) fail("could not find the 'North' map mark");
else {
  await page.mouse.move(north.x, north.y);
  await frames(page);
  const tipText = await page.evaluate(() => document.getElementById("dp-tip")?.textContent || "");
  const tipShown = await page.evaluate(() => document.getElementById("dp-tip")?.classList.contains("show"));
  if (tipShown && /North/.test(tipText)) ok(`hover shows a tooltip naming the region (${JSON.stringify(tipText)})`);
  else fail(`hover tooltip missing or wrong: shown=${tipShown} text=${JSON.stringify(tipText)}`);
  await page.mouse.move(1, 1);
  await frames(page);
}

// ---- click-to-filter: a dependent ::TABLE panel narrows to the clicked region ----
const tableRows = async () =>
  page.evaluate(() => {
    const panel = [...document.querySelectorAll(".dash .panel")].find((p) => p.querySelector(".panel-title")?.textContent.trim() === "Filtered by map click");
    return [...panel.querySelectorAll("tbody tr")].map((tr) => tr.textContent.trim());
  });

let rows = await tableRows();
if (rows.length === 2) ok("dependent table starts unfiltered (2 rows)");
else fail(`dependent table should start with 2 rows, has ${rows.length}: ${JSON.stringify(rows)}`);

if (north) {
  await page.mouse.click(north.x, north.y);
  await idle(page);
  await page.waitForTimeout(200);
  rows = await tableRows();
  if (rows.length === 1 && rows[0].includes("North")) ok("click on the 'North' map mark cross-filters the dependent table to North");
  else fail(`click did not narrow the dependent table as expected: ${JSON.stringify(rows)}`);

  // undo the selection (click the same mark again toggles it off)
  await page.mouse.click(north.x, north.y);
  await idle(page);
  await page.waitForTimeout(200);
  rows = await tableRows();
  if (rows.length === 2) ok("clicking the same mark again clears the filter");
  else fail(`clicking again did not clear the filter: ${JSON.stringify(rows)}`);
}

// ---- zoom: scroll-to-zoom over the map re-renders with a narrower domain ----
const domainBefore = await page.evaluate(() => {
  const panel = [...document.querySelectorAll(".dash .panel")].find((p) => p.querySelector(".panel-title")?.textContent.trim() === "Regions");
  return panel.querySelector(".panel-svg > svg").getAttribute("data-domain");
});
const holderBox = await page.evaluate(() => {
  const panel = [...document.querySelectorAll(".dash .panel")].find((p) => p.querySelector(".panel-title")?.textContent.trim() === "Regions");
  const r = panel.querySelector(".panel-svg").getBoundingClientRect();
  return { x: r.left + r.width / 2, y: r.top + r.height / 2 };
});
await page.mouse.move(holderBox.x, holderBox.y);
await page.mouse.wheel(0, -400); // scroll up = zoom in (attachMapZoom's wheel handler)
await page.waitForTimeout(400);
await frames(page);
const domainAfter = await page.evaluate(() => {
  const panel = [...document.querySelectorAll(".dash .panel")].find((p) => p.querySelector(".panel-title")?.textContent.trim() === "Regions");
  return panel.querySelector(".panel-svg > svg").getAttribute("data-domain");
});
if (domainAfter && domainAfter !== domainBefore) ok(`scroll-to-zoom narrows the map's data-domain (${domainBefore} -> ${domainAfter})`);
else fail(`scroll-to-zoom did not change data-domain (before=${domainBefore} after=${domainAfter})`);

for (const e of errs) fail(`page error: ${e}`);

await browser.close();
console.log(process.exitCode ? "FAIL" : "ok");
