// #941: Ask on a published static site (`dashboard --site` with
// options.site_ask). The page asks the site's own same-origin endpoint (here a
// fake OpenAI-compatible model on this script's static server): only on a
// dashboard whose header says `-- @ask on`, with the notice naming that
// endpoint, the viewer's cookies sent along (an auth proxy needs them) and no
// key in the request or in the published files; nothing leaves the site.
//
//   cargo build --bin dashboard
//   NODE_PATH=<dir with playwright>/node_modules node scripts/site-ask-e2e.mjs \
//       [--dashboard target/debug/dashboard] [--app web] [--port 8935]
process.env.DATAZOO_DISABLE_TELEMETRY = "1"; // no usage telemetry from anofox extensions
import { createRequire } from "module";
import { execFileSync } from "child_process";
import { createServer } from "http";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from "fs";
import { tmpdir } from "os";
import path from "path";
import { fileURLToPath } from "url";

const require = createRequire(import.meta.url);
const { chromium } = require("playwright");
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const arg = (k, d) => {
  const i = process.argv.indexOf(k);
  return i > 0 ? process.argv[i + 1] : d;
};
const DASH = arg("--dashboard", path.join(root, "target/debug/dashboard"));
const APP = arg("--app", path.join(root, "web"));
const PORT = Number(arg("--port", "8935"));
let bad = 0;
const check = (ok, m) => (ok ? console.log("ok   " + m) : (bad++, console.error("FAIL " + m)));

const W = mkdtempSync(path.join(tmpdir(), "site-ask-e2e-"));
const dash = path.join(W, "dash");
mkdirSync(dash);
writeFileSync(
  path.join(dash, "sales.sql"),
  `-- @title Sales
-- @ask on
-- @vars
SELECT region ::XAXIS, sales ::BARCHART, 'Sales by region' ::TITLE FROM (VALUES ('EU', 10), ('US', 20)) t(region, sales);
`,
);
writeFileSync(path.join(dash, "plain.sql"), "-- @title Plain\nSELECT 1 AS n ::METRIC, 'One' ::LABEL;\n");
writeFileSync(path.join(W, "options.json"), JSON.stringify({ site_ask: { endpoint: "/api/llm", model: "test-model" } }));
const out = path.join(W, "out");
execFileSync(DASH, ["--site", dash, out, "--app", APP, "--options", path.join(W, "options.json")], { stdio: ["ignore", "pipe", "pipe"] });
const siteJson = readFileSync(path.join(out, "site.json"), "utf8");
const site = JSON.parse(siteJson);
check(site.ask?.endpoint === "/api/llm" && site.ask?.model === "test-model" && JSON.stringify(site.ask?.dashboards) === '["sales"]', `site.json: the endpoint, the model, only the @ask on dashboard (${JSON.stringify(site.ask)})`);
check(!/key|token|secret/i.test(JSON.stringify(site.ask)), "site.json: no key field");

// The fake model: read the panel, then answer.
const calls = [];
const toolCall = (id, name, args) => ({ id, type: "function", function: { name, arguments: JSON.stringify(args) } });
const reply = (msg) => ({ id: "c", object: "chat.completion", choices: [{ index: 0, message: msg, finish_reason: msg.tool_calls ? "tool_calls" : "stop" }], usage: { prompt_tokens: 10, completion_tokens: 5 } });
function model(body) {
  const read = body.messages.some((m) => m.role === "tool");
  if (!read) return reply({ role: "assistant", content: null, tool_calls: [toolCall("t1", "get_panel", { panel: 0 })] });
  return reply({ role: "assistant", content: null, tool_calls: [toolCall("t2", "answer", { text: "The US sells the most (20).", scope: "answered", citations: [] })] });
}

const MIME = { ".html": "text/html", ".js": "text/javascript", ".mjs": "text/javascript", ".css": "text/css", ".json": "application/json", ".wasm": "application/wasm", ".sql": "text/plain" };
const server = createServer((req, res) => {
  const u = new URL(req.url, "http://x");
  if (u.pathname === "/api/llm/chat/completions" && req.method === "POST") {
    let b = "";
    req.on("data", (d) => (b += d));
    req.on("end", () => {
      const body = JSON.parse(b);
      calls.push({ headers: req.headers, body });
      res.writeHead(200, { "Content-Type": "application/json" });
      res.end(JSON.stringify(model(body)));
    });
    return;
  }
  let rel = decodeURIComponent(u.pathname).replace(/^\//, "");
  if (rel === "" || rel.endsWith("/")) rel += "index.html";
  const p = path.join(out, rel);
  if (!p.startsWith(out)) {
    res.writeHead(403);
    return res.end();
  }
  try {
    const data = readFileSync(p);
    res.writeHead(200, { "Content-Type": MIME[path.extname(p)] || "application/octet-stream", "Cache-Control": "no-store" });
    res.end(data);
  } catch (_) {
    res.writeHead(404);
    res.end("not found");
  }
});
await new Promise((r) => server.listen(PORT, "127.0.0.1", r));
const BASE = `http://127.0.0.1:${PORT}/`;

const browser = await chromium.launch();
try {
  const ctx = await browser.newContext({ viewport: { width: 1300, height: 900 }, reducedMotion: "reduce" });
  // what an auth proxy in front of the site would have set
  await ctx.addCookies([{ name: "CF_Authorization", value: "viewer-session", domain: "127.0.0.1", path: "/" }]);
  const seen = [];
  ctx.on("request", (r) => seen.push(r.url()));
  const page = await ctx.newPage();
  const errs = [];
  page.on("pageerror", (e) => errs.push(String(e)));
  const settled = () => page.waitForFunction(() => /[1-9]\d* panels?/.test(document.getElementById("status")?.textContent || "") && !document.body.classList.contains("loading"), null, { timeout: 120000 });

  await page.goto(BASE + "d/sales/");
  await settled();
  const shown = await page
    .waitForSelector(".ax-ask-open:not([hidden])", { timeout: 30000 })
    .then(() => true, () => false);
  check(shown, "the ✦ Ask button shows on the @ask on dashboard");
  if (shown) {
    await page.click(".ax-ask-open");
    const notice = (await page.textContent(".ax-ask-notice")) || "";
    check(notice.includes("/api/llm"), `the notice names the site's endpoint (${notice.slice(0, 160)})`);
    await page.click(".ax-ask-continue");
    await page.fill(".ax-ask-input", "Which region sells the most?");
    await page.click(".ax-ask-send");
    const answered = await page
      .waitForSelector(".ax-ask-bot .ax-ask-answer", { timeout: 60000 })
      .then(() => true, () => false);
    check(answered && ((await page.textContent(".ax-ask-bot .ax-ask-answer")) || "").includes("US sells the most"), "the answer shows");
    check(calls.length === 2, `two model calls: read the panel, then answer (${calls.length})`);
    check(calls.length > 0 && calls.every((c) => c.body.model === "test-model"), "…with the site's model");
    check(calls.length > 0 && calls.every((c) => /CF_Authorization=viewer-session/.test(c.headers.cookie || "")), "…with the viewer's cookie (an auth proxy sees who asks)");
    check(calls.length > 0 && calls.every((c) => !c.headers.authorization), "…and no key from the page");
    check(calls.length > 1 && JSON.stringify(calls[1].body).includes("US"), "the panel's rows went to the site's endpoint");
  }
  const CDN = new Set(["cdn.jsdelivr.net", "fonts.googleapis.com", "fonts.gstatic.com", "extensions.duckdb.org"]);
  const away = seen.filter((u) => {
    const h = new URL(u).hostname;
    return !["127.0.0.1", "localhost"].includes(h) && !CDN.has(h) && !u.startsWith("data:") && !u.startsWith("blob:");
  });
  check(away.length === 0, `no request leaves the site but the page's own CDN files (${away.join(", ")})`);
  check(errs.length === 0, `no page errors (${errs.join(" | ")})`);

  // switching in place (the site's nav, no reload) to a dashboard without
  // `-- @ask on` hides Ask; a fresh load of it never shows it
  await page.evaluate(() => document.querySelector(".ax-ask-close")?.click());
  const link = await page.$('a[href*="d/plain"]');
  if (link) {
    await page.evaluate(() => (window.__marker = 1));
    await link.click();
    await page.waitForFunction(() => window.__served?.id === "plain", null, { timeout: 30000 });
    await settled();
    check((await page.evaluate(() => window.__marker)) === 1, "the nav switched in place (no reload)");
    check(await page.$$eval(".ax-ask-open", (b) => b.every((x) => x.hidden)), "switching to a dashboard without -- @ask on hides Ask");
  } else check(false, "the site's nav has a link to the other dashboard");
  await page.goto(BASE + "d/plain/");
  await settled();
  check(await page.$$eval(".ax-ask-open", (b) => b.every((x) => x.hidden)), "no Ask on a dashboard without -- @ask on");

  // a site built without site_ask: no Ask anywhere
  const plainOut = path.join(W, "plain-out");
  execFileSync(DASH, ["--site", dash, plainOut, "--app", APP], { stdio: ["ignore", "pipe", "pipe"] });
  check(!JSON.parse(readFileSync(path.join(plainOut, "site.json"), "utf8")).ask, "without site_ask: no ask in site.json");
} finally {
  await browser.close();
  server.close();
  rmSync(W, { recursive: true, force: true });
}
console.log(`site-ask-e2e: ${bad} failed`);
process.exit(bad ? 1 : 0);
