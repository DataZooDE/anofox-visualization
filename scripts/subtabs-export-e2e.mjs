// Downloaded HTML (⤓ HTML, downloadHtml() in app.js): every ::TAB / ::SUBTAB
// page must show the same content as the live dashboard (#381). The export's
// self-contained tab script (SNAPSHOT_JS) used to wire every `.tab-btn` in
// the document — top-level AND sub-tab buttons mixed together — against a
// single top-level `.tabwrap`'s panes by a global index, so clicking any
// sub-tab button in the exported file hid every top-level pane and then threw
// (index out of range), leaving the whole dashboard blank.
//
// Builds a small dashboard with two top ::TAB sections, each with its own
// nested ::SUBTAB pair/trio (reusing sub-tab names across the two top tabs,
// since dpSubTab is keyed per top tab) plus a plain ::TAB with no sub-tabs,
// downloads it, and — in a fresh, offline page — clicks through every tab
// and sub-tab, checking each one's own panel content shows and nothing else
// bleeds through.
//
//   python3 serve_nocache.py 8947 &
//   NODE_PATH=<dir with playwright>/node_modules node scripts/subtabs-export-e2e.mjs \
//       [--url http://127.0.0.1:8947/]
import { createRequire } from "module";
import { readFileSync } from "fs";

const require = createRequire(import.meta.url);
const { chromium } = require("playwright");
const arg = (k, d) => {
  const i = process.argv.indexOf(k);
  return i > 0 ? process.argv[i + 1] : d;
};
const base = arg("--url", "http://127.0.0.1:8947/");
const fail = (m) => {
  console.error("FAIL: " + m);
  process.exitCode = 1;
};
const check = (ok, m) => (ok ? console.log("ok   " + m) : fail(m));
const b64 = (s) => Buffer.from(s, "utf8").toString("base64");

// Two top tabs, each with its own sub-tabs (same sub-tab names reused across
// top tabs — a per-top-tab dpSubTab regression check), plus one plain tab.
const SQL = `SELECT 'Alpha'::TAB;
SELECT 'One'::SUBTAB;
SELECT 'alpha-one' AS cell ::TABLE;
SELECT 'Two'::SUBTAB;
SELECT 'alpha-two' AS cell ::TABLE;
SELECT 'Beta'::TAB;
SELECT 'One'::SUBTAB;
SELECT 'beta-one' AS cell ::TABLE;
SELECT 'Two'::SUBTAB;
SELECT 'beta-two' AS cell ::TABLE;
SELECT 'Three'::SUBTAB;
SELECT 'beta-three' AS cell ::TABLE;
SELECT 'Gamma'::TAB;
SELECT 'gamma-plain' AS cell ::TABLE;
`;

const ready = (page) =>
  page.waitForFunction(
    () => /\d+ panels?/.test(document.getElementById("status")?.textContent || "") && !document.body.classList.contains("loading"),
    null,
    { timeout: 120000 }
  );

const browser = await chromium.launch();
const page = await browser.newPage({ viewport: { width: 1280, height: 900 }, reducedMotion: "reduce", acceptDownloads: true });
const liveErrs = [];
page.on("pageerror", (e) => liveErrs.push(String(e)));
await page.goto(`${base}?mode=edit#sql=${b64(SQL)}`);
await ready(page);
check(!liveErrs.length, "no page errors building the live dashboard " + liveErrs.join(" | "));

// The live dashboard itself: sanity-check every tab/sub-tab resolves (not
// what #381 is about, but a real repro should start from a working source).
// Ends back on Alpha/One and Beta's own One sub-tab, so the export below
// starts from the same, known default state (a visit to Beta/Three along
// the way, then back to One, also checks that a sub-tab click is undone by
// a later one on the same bar — not just that the first click works).
const visibleText = (t) => page.getByText(t, { exact: true }).isVisible();
const liveClick = (name) => page.getByRole("button", { name, exact: true }).click();
check(await visibleText("alpha-one"), "live: Alpha/One shows alpha-one");
await liveClick("Beta");
check(await visibleText("beta-one"), "live: Beta defaults to its own One sub-tab");
await liveClick("Three");
check(await visibleText("beta-three"), "live: Beta/Three shows beta-three");
await liveClick("One");
check(await visibleText("beta-one"), "live: Beta/One shows beta-one again");
await liveClick("Gamma");
check(await visibleText("gamma-plain"), "live: a plain tab with no sub-tabs still works");
await liveClick("Alpha");
check(await visibleText("alpha-one"), "live: back on Alpha, its own One sub-tab (unaffected by Beta)");

// Download the exported, self-contained HTML.
const [dl] = await Promise.all([page.waitForEvent("download"), page.click("#dlhtml")]);
const html = readFileSync(await dl.path(), "utf8");
for (const needle of ["alpha-one", "alpha-two", "beta-one", "beta-two", "beta-three", "gamma-plain"])
  check(html.includes(needle), `export carries ${needle} (every panel rendered before the snapshot, not just the shown pane)`);

// Open the export cold, offline, with no server and no live app.js.
const off = await browser.newContext({ offline: true, viewport: { width: 1280, height: 900 } });
const op = await off.newPage();
const exportErrs = [];
op.on("pageerror", (e) => exportErrs.push(String(e)));
await op.route("**/*", (r) => (r.request().url().startsWith("http") ? (r.abort(), undefined) : r.continue()));
await op.setContent(html, { waitUntil: "load" });

const shows = (t) => op.getByText(t, { exact: true }).isVisible();
const click = (name) => op.getByRole("button", { name, exact: true }).click();

check(await shows("alpha-one"), "export: opens on Alpha/One (alpha-one)");
check(!(await shows("beta-one").catch(() => false)), "export: Beta's content is not also showing");

await click("Beta");
check(await shows("beta-one"), "export: Beta tab shows its own One sub-tab (beta-one)");
check(!(await shows("alpha-one").catch(() => false)), "export: Alpha's pane is hidden once Beta is shown");

await click("Three");
check(await shows("beta-three"), "export: Beta's Three sub-tab shows beta-three — the exact #381 repro");
check(!(await shows("beta-one").catch(() => false)), "export: Beta's One sub-tab is hidden once Three is shown");

await click("Gamma");
check(await shows("gamma-plain"), "export: a plain tab (no sub-tabs) still switches correctly");
check(!(await shows("beta-three").catch(() => false)), "export: Beta's pane is hidden once Gamma is shown");

await click("Alpha");
check(await shows("alpha-two") === false && (await shows("alpha-one")), "export: back on Alpha, still its own One sub-tab (never touched)");

await click("Two");
check(await shows("alpha-two"), "export: Alpha's Two sub-tab shows alpha-two");

await click("Beta");
check(await shows("beta-three"), "export: back on Beta, its Three sub-tab is still the one shown (per-tab sub-tab state, not reset)");

check(!exportErrs.length, "export: no page errors clicking through every tab/sub-tab " + exportErrs.join(" | "));

await browser.close();
if (!process.exitCode) console.log("subtabs-export-e2e: ok");
