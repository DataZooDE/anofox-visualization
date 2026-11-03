// #843 (M3-1): the builder's assistant on an OpenAI-compatible endpoint
// (transport (c)), in a real browser, against a fake local model server — a
// real HTTP server on 127.0.0.1, not a route mock, so the assistant's CSP
// must let it through. Checks: no key dialog for a local model; the "what
// will be sent" dialog names the endpoint; the request is a chat completion
// (tools as functions, no Authorization); a tool call and a proposal round
// trip to the proposal card; the CSP still blocks any other origin; a 401
// offers a key, which then goes along as a Bearer token. #844: sample rows —
// off by default, shown in "What will be sent" once turned on, sent only
// then, off again in a new conversation, absent where the installation
// forbids them.
//
//   NODE_PATH=<dir with playwright>/node_modules node scripts/assist-openai-e2e.mjs --pkg web/pkg [--port 8791]
import { createRequire } from "module";
import { createServer } from "http";
import { readFileSync, existsSync } from "fs";
import path from "path";
import { fileURLToPath } from "url";

const require = createRequire(import.meta.url);
let chromium;
try {
  ({ chromium } = require("playwright"));
} catch (e) {
  console.error("Playwright is not installed (NODE_PATH=<dir with playwright>/node_modules node scripts/assist-openai-e2e.mjs)");
  process.exit(1);
}
const arg = (k, d) => {
  const i = process.argv.indexOf(k);
  return i > 0 ? process.argv[i + 1] : d;
};
const PKG = path.resolve(arg("--pkg", "web/pkg"));
const PORT = Number(arg("--port", "8791"));
if (!existsSync(path.join(PKG, "anofox_visualization_bg.wasm"))) {
  console.error("usage: assist-openai-e2e.mjs --pkg <wasm pkg dir>");
  process.exit(2);
}
const here = path.dirname(fileURLToPath(import.meta.url));
const webDir = path.join(here, "..", "web");
const results = [];
const record = (name, ok, note = "") => {
  results.push({ name, ok });
  console.log(`${ok ? "ok  " : "FAIL"} ${name}${note ? " — " + note : ""}`);
  if (!ok) process.exitCode = 1;
};
const b64 = (s) => Buffer.from(s, "utf8").toString("base64");
const listen = (srv) => new Promise((ok) => srv.listen(0, "127.0.0.1", () => ok(srv.address().port)));

// The fake model server: records every request; `reply(n, body)` answers.
const model = { requests: [], reply: null };
const modelServer = createServer((req, res) => {
  const cors = { "access-control-allow-origin": "*", "access-control-allow-headers": "content-type, authorization", "access-control-allow-methods": "POST" };
  if (req.method === "OPTIONS") {
    res.writeHead(204, cors);
    res.end();
    return;
  }
  let b = "";
  req.on("data", (d) => (b += d));
  req.on("end", () => {
    const body = JSON.parse(b || "{}");
    model.requests.push({ path: req.url, auth: req.headers.authorization || null, body });
    const [status, out] = model.reply(model.requests.length, body);
    res.writeHead(status, { "content-type": "application/json", ...cors });
    res.end(JSON.stringify(out));
  });
});
const MPORT = await listen(modelServer);
// Another loopback origin the page must not reach.
const other = { hits: 0 };
const otherServer = createServer((req, res) => {
  other.hits++;
  res.writeHead(200, { "access-control-allow-origin": "*" });
  res.end("x");
});
const OPORT = await listen(otherServer);

const ENDPOINT = `http://127.0.0.1:${MPORT}/v1`;
let CFG = { assistant: { enabled: true, transport: "openai", endpoint: ENDPOINT, models: { default: "llama3.1:8b" } } };
const MIME = { ".html": "text/html", ".js": "text/javascript", ".mjs": "text/javascript", ".css": "text/css", ".json": "application/json", ".wasm": "application/wasm", ".md": "text/markdown" };
const server = createServer((req, res) => {
  const u = new URL(req.url, "http://x");
  if (u.pathname === "/app-config.json") {
    res.writeHead(200, { "Content-Type": "application/json", "Cache-Control": "no-store" });
    res.end(JSON.stringify(CFG));
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

const DASH = `CREATE OR REPLACE TABLE sales AS SELECT * FROM (VALUES ('EU', 10), ('US', 7), ('APAC', 4)) t(region, x);
SELECT count(*) AS n ::METRIC, 'Rows' ::LABEL FROM sales;
`;
const PROPOSAL = "SELECT region ::XAXIS, sum(x) ::BARCHART, 'Sales by region' ::TITLE FROM sales GROUP BY ALL ORDER BY 2 DESC;";
const call = (id, name, args) => ({ id, type: "function", function: { name, arguments: JSON.stringify(args) } });
const chat = (message, finish = "tool_calls") => ({ id: "c", object: "chat.completion", choices: [{ index: 0, finish_reason: finish, message: { role: "assistant", ...message } }], usage: { prompt_tokens: 900, completion_tokens: 60 } });
// turn 1: profile the table; turn 2: propose
const happy = (n) => (n % 2 === 1 ? [200, chat({ content: "Let me look.", tool_calls: [call(`p${n}`, "profile_table", { table: "sales" })] })] : [200, chat({ content: null, tool_calls: [call(`d${n}`, "propose_dashboard", { sql: PROPOSAL, summary: "Sales by region.", panels: [{ stmt: 1, title: "Sales by region", why: "the ranking" }], open_issues: [] })] })]);

const settled = (page) => page.waitForFunction(() => /[1-9]\d* panels?/.test(document.getElementById("status")?.textContent || "") && !document.body.classList.contains("loading"), null, { timeout: 120000 });
const browser = await chromium.launch();
const errors = [];
async function start(page, q) {
  // #937: on a timeout, say what the page shows (CI-only failures)
  const failed = [];
  const consoleMsgs = [];
  page.on("requestfailed", (r) => failed.push(`${r.url()} ${r.failure()?.errorText}`));
  page.on("console", (m) => m.type() === "error" && consoleMsgs.push(m.text()));
  await page.goto(`${BASE}?mode=edit#sql=${b64(DASH)}`);
  await settled(page);
  await page.click("#side-new");
  await page.fill("#assist-start-q", q);
  await page.click("#assist-start button[type=submit]");
  await page.waitForSelector(".dp-dialog", { timeout: 120000 }).catch(async (e) => {
    const state = await page.evaluate(() => ({
      status: document.getElementById("status")?.textContent,
      body: document.body.className,
      start: !!document.querySelector("#assist-start") && getComputedStyle(document.querySelector("#assist-start")).display,
      aiLog: (document.getElementById("ai-log")?.textContent || "").slice(-300),
      toast: document.getElementById("side-toast")?.textContent,
      dialogs: document.querySelectorAll(".dp-dialog, .md-dialog").length,
    }));
    console.error("debug #937:", JSON.stringify({ state, failed, console: consoleMsgs, pageErrors: errors }));
    throw e;
  });
}

// ---- a local model: no key, a full round trip ----
{
  model.reply = happy;
  const context = await browser.newContext({ viewport: { width: 1400, height: 1000 }, reducedMotion: "reduce" });
  const page = await context.newPage();
  page.on("pageerror", (e) => errors.push(String(e)));
  await start(page, "Which region sells the most?");
  const dlg = (await page.textContent(".dp-dialog")) || "";
  record("O1 no key dialog: the first dialog is what will be sent", !(await page.locator('.dp-dialog input[type="password"]').count()) && dlg.includes(`${ENDPOINT}/chat/completions`), dlg.slice(0, 120));
  record("O1 nothing sent before consent", model.requests.length === 0);
  await page.click('.dp-dialog button:has-text("Send")');
  await page.waitForSelector("#ai-card:not([hidden])", { timeout: 60000 }).catch(() => {});
  record("O2 the proposal card shows", await page.evaluate(() => !document.getElementById("ai-card")?.hidden), (await page.textContent("#ai-log").catch(() => "")) || "");
  const [r1, r2] = model.requests;
  record("O3 a chat completion to /v1/chat/completions", r1?.path === "/v1/chat/completions" && r1.body.model === "llama3.1:8b" && r1.body.stream === false && r1.body.messages[0].role === "system");
  record("O3 tools as functions, no Claude-only fields", r1?.body.tools?.every((t) => t.type === "function" && t.function.parameters) && !/cache_control|"thinking"|output_config/.test(JSON.stringify(r1.body)));
  record("O3 no Authorization without a key", r1?.auth === null && r2?.auth === null);
  const tool = r2?.body.messages.find((m) => m.role === "tool");
  record("O4 the tool result answers its call", tool?.tool_call_id === "p1" && /sales/.test(tool.content || ""), JSON.stringify(tool || {}).slice(0, 160));
  record("O4 the model's text reaches the log", ((await page.textContent("#ai-log")) || "").includes("Let me look."));
  record("O4 two model calls: the proposal passed its check", model.requests.length === 2, String(model.requests.length));
  const blocked = await page.evaluate(async (u) => {
    try {
      await fetch(u);
      return "reached";
    } catch (_) {
      return "blocked";
    }
  }, `http://127.0.0.1:${OPORT}/x`);
  record("O5 the CSP blocks any other origin", blocked === "blocked" && other.hits === 0, blocked);
  const csp = await page.evaluate(() => document.querySelector('meta[http-equiv="Content-Security-Policy"][data-anofox-assist]')?.content || "");
  const connect = (csp.match(/connect-src [^;]*/)?.[0] || "").split(" ");
  record("O5 connect-src names the endpoint's origin only", connect.includes(`http://127.0.0.1:${MPORT}`) && !connect.includes(ENDPOINT) && !connect.some((x) => x.includes(`:${OPORT}`)), connect.join(" "));
  await context.close();
}

// ---- a gateway that wants a key: 401, then a Bearer token ----
{
  model.requests = [];
  let refused = true;
  model.reply = (n, body) => {
    if (refused) {
      refused = false;
      return [401, { error: { message: "missing bearer token" } }];
    }
    return happy(model.requests.length - 1);
  };
  const context = await browser.newContext({ viewport: { width: 1400, height: 1000 }, reducedMotion: "reduce" });
  const page = await context.newPage();
  page.on("pageerror", (e) => errors.push(String(e)));
  await start(page, "x");
  await page.click('.dp-dialog button:has-text("Send")');
  await page.waitForFunction(() => /did not accept/.test(document.getElementById("ai-log")?.textContent || ""), null, { timeout: 30000 }).catch(() => {});
  const log = (await page.textContent("#ai-log")) || "";
  record("K1 a 401 says so, with the endpoint's reason", /did not accept the request: missing bearer token/.test(log), log.slice(-200));
  const add = page.locator('#ai-log button:has-text("Add key")');
  record("K1 and offers a key", (await add.count()) === 1);
  await add.click();
  await page.waitForSelector('.dp-dialog input[type="password"]', { timeout: 10000 });
  const title = (await page.textContent(".dp-dialog")) || "";
  record("K2 the key dialog names the endpoint, not Anthropic", title.includes(`http://127.0.0.1:${MPORT}`) && !/Anthropic/.test(title), title.slice(0, 160));
  await page.fill('.dp-dialog input[type="password"]', "gw-token-e2e-1234");
  await page.click('.dp-dialog button:has-text("Save key")');
  const slots = await page.evaluate(() => ({ openai: sessionStorage.getItem("dp_assist_key_openai"), anthropic: sessionStorage.getItem("dp_assist_key") }));
  record("K2 the key is in its own slot", slots.openai === "gw-token-e2e-1234" && !slots.anthropic, JSON.stringify(slots));
  await page.fill("#ai-q", "Which region sells the most?");
  await page.click("#ai-send");
  await page.waitForSelector(".dp-dialog", { timeout: 15000 }).catch(() => {});
  if (await page.locator('.dp-dialog button:has-text("Send")').count()) await page.click('.dp-dialog button:has-text("Send")');
  await page.waitForSelector("#ai-card:not([hidden])", { timeout: 60000 }).catch(() => {});
  record("K3 the next requests carry the Bearer token", model.requests.slice(1).every((r) => r.auth === "Bearer gw-token-e2e-1234") && model.requests.length === 3, JSON.stringify(model.requests.map((r) => r.auth)));
  record("K3 and the proposal shows", await page.evaluate(() => !document.getElementById("ai-card")?.hidden));
  await context.close();
}

// ---- #844: sample rows, an opt-in per conversation ----
{
  model.requests = [];
  model.reply = happy;
  const context = await browser.newContext({ viewport: { width: 1400, height: 1000 }, reducedMotion: "reduce" });
  const page = await context.newPage();
  page.on("pageerror", (e) => errors.push(String(e)));
  await page.goto(`${BASE}?mode=edit#sql=${b64(DASH)}`);
  await settled(page);
  await page.click("#assist-btn");
  await page.waitForSelector("#ai-q", { timeout: 30000 });
  record("S1 sample rows are off by default", (await page.isVisible("#ai-sample")) && !(await page.isChecked("#ai-sample")));
  await page.check("#ai-sample");
  await page.fill("#ai-q", "Which region sells the most?");
  await page.click("#ai-send");
  await page.waitForSelector(".dp-dialog", { timeout: 120000 });
  const dlg = (await page.textContent(".dp-dialog")) || "";
  record("S2 What will be sent shows the rows before they go", dlg.includes('"sample_rows":{"columns":["region","x"],"rows":[["EU",10]') && /first 20 rows/.test(dlg) && model.requests.length === 0, dlg.slice(0, 200));
  await page.click('.dp-dialog button:has-text("Send")');
  await page.waitForSelector("#ai-card:not([hidden])", { timeout: 60000 }).catch(() => {});
  const first = JSON.stringify(model.requests[0]?.body || {});
  record("S3 the rows reach the model with the opt-in", first.includes('\\"sample_rows\\":{\\"columns\\":[\\"region\\",\\"x\\"]'), first.slice(0, 120));
  record("S3 the chips say so", /sample rows ✓/.test((await page.textContent("#ai-chips")) || ""));
  await page.click("#ai-new");
  record("S4 a new conversation turns them off", !(await page.isChecked("#ai-sample")));
  model.requests = [];
  await page.fill("#ai-q", "Again, please.");
  await page.click("#ai-send");
  await page.waitForSelector(".dp-dialog", { timeout: 120000 });
  await page.click('.dp-dialog button:has-text("Send")');
  await page.waitForFunction(() => document.querySelectorAll("#ai-log .ai-note, #ai-card:not([hidden])").length > 0, null, { timeout: 60000 }).catch(() => {});
  const again = JSON.stringify(model.requests[0]?.body || {});
  record("S4 and nothing of the rows goes", model.requests.length > 0 && !again.includes("[\\\"EU\\\",10]") && again.includes('\\"sample_rows\\":false'), again.slice(0, 120));
  await context.close();
  // an installation that forbids them: no box
  CFG = { assistant: { ...CFG.assistant, sample_rows_allowed: false } };
  const c2 = await browser.newContext({ viewport: { width: 1400, height: 1000 }, reducedMotion: "reduce" });
  const p2 = await c2.newPage();
  await p2.goto(`${BASE}?mode=edit#sql=${b64(DASH)}`);
  await settled(p2);
  await p2.click("#assist-btn");
  await p2.waitForSelector("#ai-q", { timeout: 30000 });
  record("S5 sample_rows_allowed: false hides the box", !(await p2.isVisible("#ai-sample")));
  await c2.close();
}

record("no page errors", errors.length === 0, errors.slice(0, 3).join(" | "));
await browser.close();
server.close();
modelServer.close();
otherServer.close();
const failed = results.filter((r) => !r.ok).length;
console.log(failed ? `assist-openai-e2e: ${failed} FAILED` : `assist-openai-e2e: ok (${results.length} checks)`);
process.exit(failed ? 1 : 0);
