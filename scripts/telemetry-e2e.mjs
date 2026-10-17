// No visitor's browser reports usage to a third party: open every example of
// the demo (each tab and sub-tab, so the anofox DuckDB-Wasm extensions load)
// and Explore in headless Chromium, record every request the browser makes —
// the page's and its workers' (Chromium's net log, so DuckDB-Wasm's worker
// and WebSocket traffic count too) — and fail on a request to a host outside
// ALLOWED, or to any telemetry host (PostHog & co.). Prints the hosts seen.
//
//   python3 serve_nocache.py 8947 &          # serves web/ (after wasm-pack build
//                                            # + scripts/fetch-wasm-extensions.sh)
//   NODE_PATH=<dir with playwright>/node_modules node scripts/telemetry-e2e.mjs \
//       [--url http://127.0.0.1:8947/] [--only <example name substring>] [--selftest]
//
// --selftest  the page also tries a request to PostHog, which must then fail
//             the run (a telemetry request is recorded and aborted, never sent)
import { createRequire } from "module";
import { mkdtempSync, readFileSync, rmSync } from "fs";
import os from "os";
import path from "path";

const require = createRequire(import.meta.url);
const { chromium } = require("playwright");
const arg = (k, d) => {
  const i = process.argv.indexOf(k);
  return i > 0 ? process.argv[i + 1] : d;
};
const base = arg("--url", "http://127.0.0.1:8947/");
const only = arg("--only", "");
const selftest = process.argv.includes("--selftest");
let failed = 0;
const fail = (m) => {
  console.error("FAIL: " + m);
  failed++;
  process.exitCode = 1;
};
const ok = (m) => console.log("ok   " + m);

// What the demo may reach besides its own origin: the DuckDB-Wasm bundle,
// DuckDB's core extensions, the web fonts, and one example's live data feed.
const ALLOWED = new Set([
  "cdn.jsdelivr.net",
  "extensions.duckdb.org",
  "fonts.googleapis.com",
  "fonts.gstatic.com",
  "earthquake.usgs.gov",
]);
// Not the app's or an extension's doing: the browser's Reporting API posting
// to endpoints the CDNs name in their response headers (jsDelivr is behind
// Cloudflare, `NEL: success_fraction 0.01`; Google Fonts' `Report-To`).
// Listed, not failed; self-hosting the bundle and fonts would end them.
const REPORTING = new Set(["a.nel.cloudflare.com", "csp.withgoogle.com"]);
const TELEMETRY = /posthog|telemetry|segment\.(io|com)|mixpanel|amplitude|sentry|datadog|google-analytics|googletagmanager|plausible|matomo|hotjar/i;
const local = (h) => h === "127.0.0.1" || h === "localhost" || h === "[::1]" || h === "::1";

const netlog = path.join(mkdtempSync(path.join(os.tmpdir(), "telemetry-e2e-")), "net.json");
const browser = await chromium.launch({ args: [`--log-net-log=${netlog}`, "--net-log-capture-mode=Default"] });
const ctx = await browser.newContext({ viewport: { width: 1440, height: 1000 }, reducedMotion: "reduce" });
const seen = new Map(); // host → [count, first url]
const note = (u) => {
  let h;
  try {
    h = new URL(u).hostname;
  } catch (_) {
    return;
  }
  if (!h) return;
  const s = seen.get(h) || [0, u];
  s[0]++;
  seen.set(h, s);
};
ctx.on("request", (r) => note(r.url()));
// recorded above, then aborted: a telemetry request never leaves this machine
await ctx.route((u) => TELEMETRY.test(u.hostname), (r) => r.abort());
const page = await ctx.newPage();
page.on("websocket", (w) => note(w.url()));
const errs = [];
page.on("pageerror", (e) => errs.push(String(e)));

const settled = () =>
  page.waitForFunction(
    () => /\d+ panels?/.test(document.getElementById("status")?.textContent || "") && !document.body.classList.contains("loading"),
    null,
    { timeout: 240000 }
  );
// A tab's panels run when it is first shown (lazy tabs).
const lazyDone = () =>
  page.waitForFunction(() => ![...document.querySelectorAll(".dash .dp-lazy")].some((e) => e.offsetParent !== null), null, { timeout: 240000 });

await page.goto(base);
await settled();
const names = (await page.$$eval(".side-item .s-name", (n) => n.map((x) => x.textContent))).filter((n) => n.includes(only));
if (!names.length) fail("no examples in the sidebar");
const extLoads = new Set();
ctx.on("request", (r) => {
  const m = /localext\/[^/]+\/[^/]+\/(\w+)\.duckdb_extension/.exec(r.url());
  if (m) extLoads.add(m[1]);
});
for (const name of names) {
  await page.evaluate((d) => [...document.querySelectorAll(".side-item")].find((b) => b.querySelector(".s-name")?.textContent === d).click(), name);
  try {
    await page.waitForFunction((d) => document.getElementById("dash-name").value === d, name, { timeout: 60000 });
    await settled();
    for (const t of await page.$$eval(".tabbar > .tab-btn", (bs) => bs.map((b) => b.textContent))) {
      await page.click(`.tabbar > .tab-btn:text-is("${t}")`);
      await lazyDone();
      for (const s of await page.$$eval(".tabpane:not([style*='none']) .subtab-btn", (bs) => bs.map((b) => b.textContent))) {
        await page.click(`.tabpane:not([style*='none']) .subtab-btn:text-is("${s}")`);
        await lazyDone();
      }
    }
    console.log(`     opened: ${name}`);
  } catch (e) {
    fail(`${name}: did not finish (${String(e).split("\n")[0]})`);
  }
}
// Explore probes anofox_tabular for its hints' checks (#111).
if (!only) {
  await page.click("#side-explore").catch(() => {});
  await page.waitForSelector(".cat-table", { timeout: 60000 }).catch(() => fail("Explore: no catalog"));
  await page.click(".cat-table").catch(() => {});
  await page.waitForFunction(() => document.querySelector(".xp-progress.done"), null, { timeout: 120000 }).catch(() => {});
}
if (selftest) await page.evaluate(() => fetch("https://eu.i.posthog.com/batch/", { method: "POST", body: "{}" }).catch(() => {}));
await page.waitForTimeout(3000); // a deferred beacon would go out now
await browser.close(); // flushes the net log

// The net log sees what page events may miss (workers, preconnects, DNS).
const log = readFileSync(netlog, "utf8");
for (const m of log.matchAll(/"(?:url|original_url)":"([a-z]+:\/\/[^"]+)"/g)) note(m[1]);
for (const m of log.matchAll(/"host":"([A-Za-z0-9.-]+)(?::\d+)?"/g)) note(`https://${m[1]}/`);
rmSync(path.dirname(netlog), { recursive: true, force: true });

console.log(`anofox extensions loaded in the browser: ${[...extLoads].sort().join(", ") || "none"}`);
for (const [h, [n, u]] of [...seen].sort()) console.log(`     ${h}: ${n} (${u.slice(0, 100)})`);
for (const [h, [, u]] of seen) {
  if (TELEMETRY.test(h) || TELEMETRY.test(u)) fail(`telemetry request: ${u}`);
  else if (REPORTING.has(h)) console.log(`note ${h}: a CDN's browser report (Reporting API), not from the app or an extension`);
  else if (!local(h) && !ALLOWED.has(h)) fail(`request to an unexpected host: ${u}`);
}
if (!["anofox_statistics", "anofox_forecast"].every((e) => extLoads.has(e)) && !only)
  fail(`the anofox wasm extensions did not all load (${[...extLoads].join(", ") || "none"}) — run scripts/fetch-wasm-extensions.sh`);
if (errs.length) console.log(`     page errors (not checked here): ${errs.slice(0, 3).join(" | ")}`);
if (!failed) ok(`${names.length} examples + Explore: no request to a telemetry or unexpected host`);
