// The builder assistant on an authoring server (#740), in a real browser:
// `anofox_serve(port, options)` with ANTHROPIC_API_KEY and a local fake of
// the model API as `assist_base_url`. The page gets no key dialog and stores
// no key; the model calls go to the server's /assist proxy (which adds the
// key) and never to api.anthropic.com; the assistant's tools run in the
// locked engine on a snapshot of the live session's tables (owner
// 2026-10-30), so the model sees the live `sales` table.
//
//   NODE_PATH=tests/web/node_modules node scripts/assist-live-e2e.mjs \
//       --ext path/to/anofox_visualization.duckdb_extension [--port 8985] [--duckdb duckdb]
import { createRequire } from "module";
import { spawn } from "child_process";
import http from "http";
import { mkdtempSync, mkdirSync, writeFileSync, rmSync, existsSync, chmodSync } from "fs";
import path from "path";
import { sse, text, toolUse } from "./e2e-fixtures/assist-mock.mjs";

const require = createRequire(import.meta.url);
let chromium;
try {
  ({ chromium } = require("playwright"));
} catch (e) {
  console.error("Playwright is not installed (NODE_PATH=<dir with playwright>/node_modules)");
  process.exit(1);
}
const arg = (k, d) => {
  const i = process.argv.indexOf(k);
  return i > 0 ? process.argv[i + 1] : d;
};
const ext = arg("--ext", "");
const port = Number(arg("--port", "8985"));
const duckdb = arg("--duckdb", "duckdb");
if (!ext || !existsSync(ext)) {
  console.error("usage: assist-live-e2e.mjs --ext <anofox_visualization.duckdb_extension>");
  process.exit(2);
}
let bad = 0;
const check = (ok, m) => (ok ? console.log("ok   " + m) : (bad++, console.error("FAIL " + m)));
const sqlStr = (s) => `'${s.replace(/'/g, "''")}'`;
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const KEY = "sk-ant-live-e2e-740-server-only";
const REPLY = "LIVE-REPLY-OK: the sales table is there.";

const W = mkdtempSync(path.join(path.dirname(path.resolve(ext)), "assist-live-e2e-"));
const bin = path.join(W, "bin");
mkdirSync(bin);
writeFileSync(path.join(bin, "xdg-open"), "#!/bin/sh\nexit 0\n"); // anofox_serve opens a browser
chmodSync(path.join(bin, "xdg-open"), 0o755);

// The fake model API: turn 1 lists the tables, turn 2 answers in text.
const seen = [];
const api = http.createServer((req, res) => {
  let body = "";
  req.on("data", (d) => (body += d));
  req.on("end", () => {
    seen.push({ url: req.url, headers: req.headers, body });
    res.writeHead(200, { "Content-Type": "text/event-stream" });
    res.end(seen.length === 1 ? sse({ content: [toolUse("toolu_1", "list_tables", { filter: "" })], stop_reason: "tool_use" }) : sse({ content: [text(REPLY)] }));
  });
});
await new Promise((r) => api.listen(0, "127.0.0.1", r));
const apiBase = `http://127.0.0.1:${api.address().port}`;

const env = { ...process.env, DATAZOO_DISABLE_TELEMETRY: "1", ANTHROPIC_API_KEY: KEY, PATH: `${bin}:${process.env.PATH}` };
const proc = spawn(duckdb, ["-unsigned", "-batch", ":memory:"], { stdio: ["pipe", "pipe", "pipe"], env });
let log = "";
proc.stdout.on("data", (d) => (log += d));
proc.stderr.on("data", (d) => (log += d));
const opts = { assist: true, assist_base_url: apiBase };
proc.stdin.write(
  `LOAD ${sqlStr(ext)};\nCREATE TABLE sales AS SELECT i AS id, ['EU','US','APAC'][1 + i % 3] AS region, (i * 1.5)::DOUBLE AS amount FROM range(0, 600) t(i);\n` +
    // #742: a view the (empty) editor doesn't name: listed, never copied, never run.
    `CREATE VIEW v_unnamed AS SELECT region, count(*) AS n FROM sales GROUP BY region;\n` +
    `SELECT anofox_serve(${port}, ${sqlStr(JSON.stringify(opts))}) AS r;\n`
);
let browser = null;
try {
  for (let i = 0; i < 100 && !/token=[0-9a-f]+/.test(log); i++) await sleep(100);
  const tok = (log.match(/token=([0-9a-f]+)/) || [])[1];
  if (!tok) throw new Error(`anofox_serve did not start:\n${log}`);
  check(/assistant proxy on/.test(log), "the server's proxy is on");

  browser = await chromium.launch();
  const context = await browser.newContext();
  const page = await context.newPage();
  const browserRequests = [];
  page.on("request", (r) => browserRequests.push(r.url()));
  const errs = [];
  page.on("pageerror", (e) => errs.push(String(e)));
  const base = `http://127.0.0.1:${port}`;
  await page.goto(`${base}/?token=${tok}`); // → the cookie, then /
  const sql = Buffer.from("SELECT count(*) AS n ::METRIC FROM sales;\n", "utf8").toString("base64");
  await page.goto(`${base}/?mode=edit#sql=${sql}`);
  await page.waitForFunction(() => document.getElementById("assist-btn")?.hidden === false, null, { timeout: 60000 }).catch(async (e) => {
    console.error("debug:", JSON.stringify(await page.evaluate(() => ({ app: window.__anofoxApp, status: document.getElementById("status")?.textContent, duck: performance.getEntriesByType("resource").map((e) => e.name).filter((n) => /duckdb|assist/.test(n)), btn: !!document.getElementById("assist-btn"), mode: document.body.className }))), errs);
    console.error("cfg:", JSON.stringify(await page.evaluate(async () => (await import("/assist.js")).assistConfig(window.__anofoxApp.assistant, { live: true, host: location.hostname }))));
    throw e;
  });
  check(true, "the assistant button shows on the authoring server");
  const block = await page.evaluate(() => window.__anofoxApp?.assistant);
  check(block?.transport === "serve" && block?.enabled === true, `the page was told the proxy is on (${JSON.stringify(block)})`);

  await page.click("#side-new");
  await page.fill("#assist-start-q", "What tables are there?");
  await page.click("#assist-start button[type=submit]");
  await page.waitForSelector(".dp-dialog", { timeout: 60000 });
  const dlg = await page.evaluate(() => document.querySelector(".dp-dialog")?.textContent || "");
  check(!/API key/i.test(dlg) && !(await page.locator('.dp-dialog input[type="password"]').count()), "no key dialog");
  check(dlg.includes(`${base}/assist`) && /this server/.test(dlg), "\"what will be sent\" names this server's proxy");
  check(/sales/.test(dlg), "…and the live sales table is in scope (the engine's snapshot)");
  check(await page.evaluate(() => !!document.querySelector("meta[data-anofox-assist]")), "the tab's CSP is in place");
  await page.click('.dp-dialog button:has-text("Send")');
  await page.waitForFunction((t) => document.getElementById("ai-log")?.textContent.includes(t), REPLY, { timeout: 60000 }).catch(async (e) => {
    console.error("debug:", seen.length, JSON.stringify(await page.evaluate(() => ({ log: document.getElementById("ai-log")?.textContent, status: document.getElementById("ai-status")?.textContent }))), errs);
    throw e;
  });
  check(true, "the model's reply arrives through the proxy");

  check(seen.length === 2 && seen.every((r) => r.url === "/v1/messages" && r.headers["x-api-key"] === KEY), `the proxy called upstream with the server's key (${seen.length} calls)`);
  const second = seen[1] ? JSON.parse(seen[1].body) : {};
  const result = JSON.stringify(second.messages?.at(-1)?.content || "");
  check(/sales/.test(result) && /600/.test(result), "list_tables ran in the engine on the live table (600 rows)");
  check(/not copied[^}]*v_unnamed/.test(result), "a view the dashboard doesn't name is listed as not copied (#742)");
  check(!seen.some((r) => r.body.includes(KEY)), "the key is never in a request body");
  check(!browserRequests.some((u) => /api\.anthropic\.com/.test(u)), "the browser never called api.anthropic.com");
  check(browserRequests.some((u) => u === `${base}/assist/v1/messages`), "the browser called the server's /assist");
  const stored = await page.evaluate(() => [sessionStorage.getItem("dp_assist_key"), localStorage.getItem("dp_assist_key")]);
  check(!stored[0] && !stored[1], "no key in the browser's storage");
  check(await page.evaluate(() => document.getElementById("ai-forget")?.hidden === true), "no Forget key button");
  check(!(await page.content()).includes(KEY), "the key is nowhere in the page");
  check(errs.length === 0, `no page errors (${errs.join(" | ")})`);
} catch (e) {
  bad++;
  console.error("FAIL " + (e.stack || e));
} finally {
  await browser?.close();
  proc.kill();
  api.close();
  rmSync(W, { recursive: true, force: true });
}
console.log(bad ? `${bad} check(s) failed` : "all checks passed");
process.exit(bad ? 1 : 0);
