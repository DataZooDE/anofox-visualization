// Ask in the builder (#195 M2-5), in a real browser: View mode's ✦ Ask sheet
// and Edit mode's Ask tab, against a mocked Anthropic Messages API (never a
// real call — api.anthropic.com is routed to a local server,
// scripts/e2e-fixtures/assist-mock.mjs). Checks: the notice before a
// conversation's first question; a personal value never reaches the model and
// comes back unmasked; a verified citation chip goes to its panel; a
// "what if" read with another input value runs on the page and leaves the
// page's own value and panels as they were; an @ask skip panel never leaves.
//
//   NODE_PATH=<dir with playwright>/node_modules TMPDIR=<scratch>/tmp \
//     node scripts/ask-builder-e2e.mjs --pkg web/pkg [--port 8795] [--shots DIR]
import { createRequire } from "module";
import { createServer } from "http";
import { readFileSync, existsSync, mkdirSync } from "fs";
import path from "path";
import { fileURLToPath } from "url";
import { installMock, sse, toolUse, cfg } from "./e2e-fixtures/assist-mock.mjs";

const require = createRequire(import.meta.url);
let chromium;
try {
  ({ chromium } = require("playwright"));
} catch (e) {
  console.error("Playwright is not installed (NODE_PATH=<dir with playwright>/node_modules node scripts/ask-builder-e2e.mjs)");
  process.exit(1);
}
const arg = (k, d) => {
  const i = process.argv.indexOf(k);
  return i > 0 ? process.argv[i + 1] : d;
};
const PKG = path.resolve(arg("--pkg", "web/pkg"));
const PORT = Number(arg("--port", "8795"));
const SHOTS = arg("--shots", "");
if (!existsSync(path.join(PKG, "anofox_visualization_bg.wasm"))) {
  console.error("usage: ask-builder-e2e.mjs --pkg <wasm pkg dir>");
  process.exit(2);
}
if (SHOTS) mkdirSync(SHOTS, { recursive: true });
const here = path.dirname(fileURLToPath(import.meta.url));
const webDir = path.join(here, "..", "web");

const DASH = `CREATE OR REPLACE TABLE stock AS SELECT * FROM (VALUES ('EU', 'SKU-1', 'ann@example.com', 4), ('EU', 'SKU-2', 'bob@example.com', 9), ('US', 'SKU-9', 'cy@example.com', 1)) t(region, sku, contact, cover);
SELECT DISTINCT region ::DROPDOWN FROM stock ORDER BY 1;
SELECT sku AS "SKU" ::TABLE, contact AS "Contact", cover AS "Cover" FROM stock WHERE region = getvariable('region') ORDER BY cover;
-- @ask skip
SELECT 'CANARY-SKIP' AS secret ::TABLE;
`;
// plan indices: 0 setup, 1 input, 2 table, 3 skipped
const KEY = "sk-ant-e2e-0000000000000000000000000";

const results = [];
const record = (name, ok, note = "") => {
  results.push({ name, ok });
  console.log(`${ok ? "ok  " : "FAIL"} ${name}${note ? " — " + note : ""}`);
  if (!ok) process.exitCode = 1;
};
const b64 = (s) => Buffer.from(s, "utf8").toString("base64");

const MIME = { ".html": "text/html", ".js": "text/javascript", ".mjs": "text/javascript", ".css": "text/css", ".json": "application/json", ".wasm": "application/wasm", ".md": "text/markdown" };
const server = createServer((req, res) => {
  const u = new URL(req.url, "http://x");
  if (u.pathname === "/app-config.json") {
    res.writeHead(200, { "Content-Type": "application/json", "Cache-Control": "no-store" });
    res.end(JSON.stringify(cfg(true)));
    return;
  }
  let p = u.pathname.startsWith("/pkg/") ? path.join(PKG, u.pathname.slice(5)) : path.join(webDir, decodeURIComponent(u.pathname));
  if (p.endsWith("/")) p = path.join(p, "index.html");
  try {
    const data = readFileSync(p);
    res.writeHead(200, { "Content-Type": MIME[path.extname(p)] || "application/octet-stream", "Cache-Control": "no-store" });
    res.end(data);
  } catch (_) {
    res.writeHead(404);
    res.end("not found");
  }
});
await new Promise((ok) => server.listen(PORT, "127.0.0.1", ok));
const BASE = `http://127.0.0.1:${PORT}/`;
const browser = await chromium.launch();

// The model: read the table, read it again for the US, answer citing both.
function handler() {
  let token = null;
  return async (_, n) => {
    // the mock records a request's body as it arrives: wait for this one
    for (let i = 0; i < 100 && mockRef.requests.length < n; i++) await new Promise((r) => setTimeout(r, 10));
    const req = mockRef.requests[n - 1]?.body;
    if (n === 1 || n === 4) return sse({ content: [toolUse(`t${n}a`, "get_panel", { panel: 2 })], stop_reason: "tool_use" });
    if (n === 2) {
      const last = JSON.stringify(req?.messages?.at(-1) ?? "");
      token = (/⟦E\d+⟧/.exec(last) || [""])[0];
      return sse({ content: [toolUse("t2", "get_panel", { panel: 2, vars: [{ name: "region", value: "US" }] })], stop_reason: "tool_use" });
    }
    if (n === 3)
      return sse({
        content: [
          toolUse("t3", "answer", {
            text: `**SKU-1** has the least cover in the EU (contact ${token}); in the US it is SKU-9.`,
            scope: "answered",
            citations: [
              { panel: 2, label: "SKU-1", column: "Cover", value: "4", row: [{ name: "SKU", value: "SKU-1" }] },
              { panel: 2, label: "SKU-9", column: "Cover", value: "1", row: [{ name: "SKU", value: "SKU-9" }] },
            ],
          }),
        ],
        stop_reason: "tool_use",
      });
    return sse({ content: [toolUse(`t${n}`, "answer", { text: "Edit-mode answer.", scope: "answered", citations: [{ panel: 2, label: "SKU-2", column: "Cover", value: "9", row: [{ name: "SKU", value: "SKU-2" }] }] })], stop_reason: "tool_use" });
  };
}
let mockRef = null;

const settled = (page) =>
  page.waitForFunction(() => /\d+ panels?/.test(document.getElementById("status")?.textContent || "") && !document.body.classList.contains("loading"), null, { timeout: 120000 });

const context = await browser.newContext({ viewport: { width: 1400, height: 1000 }, reducedMotion: "reduce", ignoreHTTPSErrors: true });
await context.addInitScript((k) => sessionStorage.setItem("dp_assist_key", k), KEY);
mockRef = await installMock(context, handler());
const page = await context.newPage();
const errors = [];
page.on("pageerror", (e) => errors.push(String(e)));

// ---- View mode ----
await page.goto(`${BASE}?mode=view#sql=${b64(DASH)}`);
await settled(page);
await page.waitForSelector(".ax-ask-open:not([hidden])", { timeout: 30000 });
record("V1 the ✦ Ask button shows in View mode", true);
await page.click(".ax-ask-open");
record("V2 the notice comes first", await page.isVisible(".ax-ask-notice"));
record("V2 the notice names where it goes", ((await page.textContent(".ax-ask-notice")) || "").includes("Anthropic (Claude), with your API key"));
await page.click(".ax-ask-continue");
await page.fill(".ax-ask-input", "Which SKU has the least cover?");
await page.click(".ax-ask-send");
await page.waitForSelector(".ax-ask-bot .ax-ask-answer", { timeout: 60000 });
const answer = await page.textContent(".ax-ask-bot .ax-ask-answer");
record("V3 the answer shows, unmasked", answer.includes("ann@example.com") && answer.includes("SKU-9"), answer.slice(0, 160));
const bodies = mockRef.requests.map((r) => JSON.stringify(r.body));
record("V4 no e-mail reached the model", bodies.every((b) => !/@example\.com/.test(b)));
record("V4 no @ask skip panel reached the model", bodies.every((b) => !b.includes("CANARY-SKIP")));
record("V4 the view went along", bodies[0].includes("Inputs: region = EU"));
record("V4 the US read reached the model", bodies[2].includes("SKU-9"));
const chips = await page.$$eval(".ax-ask-bot .ax-ask-chip", (b) => b.map((x) => x.textContent));
record("V5 two verified chips", chips.length === 2, JSON.stringify(chips));
record("V5 the US chip says its vars", chips.some((c) => c.includes("region = US")), JSON.stringify(chips));
// the page is as it was: its own value, its own rows
const tableText = await page.textContent('.dash .panel[data-stmt="2"]');
record("V6 the page's table still shows the EU", tableText.includes("SKU-1") && !tableText.includes("SKU-9"));
const dd = await page.evaluate(() => [...document.querySelectorAll(".dash select")].map((x) => x.value).join(",") || document.querySelector(".dash")?.textContent?.slice(0, 200));
record("V6 the page's input is still EU", String(dd).includes("EU"), String(dd));
await page.click(".ax-ask-bot .ax-ask-chip");
await page.waitForSelector('.dash .panel[data-stmt="2"].dp-ask-hit', { timeout: 5000 }).then(
  () => record("V7 a chip highlights its panel", true),
  () => record("V7 a chip highlights its panel", false)
);
if (SHOTS) await page.screenshot({ path: path.join(SHOTS, "ask-builder-view.png") });
// the page re-runs with the same value after the what-if read (no stale var)
await page.evaluate(() => document.querySelector(".ax-ask-close")?.click());

// ---- Edit mode: the assistant panel's Ask tab ----
await page.goto(`${BASE}?mode=edit#sql=${b64(DASH)}`);
await settled(page);
await page.click("#assist-btn");
await page.waitForSelector("#ai-mode-ask", { timeout: 30000 });
await page.click("#ai-mode-ask");
await page.waitForSelector("#ai-askbox .ax-ask-notice:not([hidden])", { timeout: 10000 });
record("E1 the Ask tab shows its notice", true);
await page.click("#ai-askbox .ax-ask-continue");
await page.fill("#ai-askbox .ax-ask-input", "Which has the most cover?");
await page.click("#ai-askbox .ax-ask-send");
await page.waitForSelector("#ai-askbox .ax-ask-answer", { timeout: 60000 });
record("E2 the Ask tab answers", ((await page.textContent("#ai-askbox .ax-ask-answer")) || "").includes("Edit-mode answer."));
record("E2 its chip is verified", (await page.$$("#ai-askbox .ax-ask-chip")).length === 1);
await page.click("#ai-askbox .ax-ask-what summary");
await page.waitForSelector("#ai-askbox .ax-ask-what li", { timeout: 5000 }).catch(() => {});
const what = (await page.textContent("#ai-askbox .ax-ask-what")) || "";
record("E3 What will be sent lists the panels, not the skipped one", what.includes("SKU") && !what.includes("CANARY"), what.slice(0, 200));
await page.click("#ai-mode-build");
record("E4 Build comes back", await page.isVisible("#ai-q"));
if (SHOTS) await page.screenshot({ path: path.join(SHOTS, "ask-builder-edit.png") });
record("no page errors", errors.length === 0, errors.slice(0, 3).join(" | "));

await browser.close();
server.close();
const failed = results.filter((r) => !r.ok).length;
console.log(failed ? `ask-builder-e2e: ${failed} FAILED` : `ask-builder-e2e: ok (${results.length} checks)`);
process.exit(failed ? 1 : 0);
