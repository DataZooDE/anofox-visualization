// Ask (#195, M2-4) on a real locked server with a fake model: the page's
// Ask button and panel (web/ask-ui.js) — the notice before the first
// question, the streamed answer rendered inert (an <img>, a link and a
// script scheme in the model's text stay text), a verified citation chip that
// opens the other tab and highlights its panel, an unverified one shown as
// such, Esc back to the opener, the phone layout; no button where Ask is off.
// Needs a native extension build with the real UI embedded (CI's `web-e2e`).
//
//   node scripts/ask-served-e2e.mjs --ext path/to/anofox_visualization.duckdb_extension [--port 8957]
import { createRequire } from "module";
import { spawn } from "child_process";
import { createServer } from "http";
import { mkdtempSync, mkdirSync, writeFileSync, rmSync, existsSync } from "fs";
import { tmpdir } from "os";
import path from "path";

const require = createRequire(import.meta.url);
const { chromium } = require("playwright");
const arg = (k, d) => {
  const i = process.argv.indexOf(k);
  return i > 0 ? process.argv[i + 1] : d;
};
const ext = arg("--ext", "");
const port = Number(arg("--port", "8957"));
const duckdb = arg("--duckdb", "duckdb");
if (!ext || !existsSync(ext)) {
  console.error("usage: ask-served-e2e.mjs --ext <anofox_visualization.duckdb_extension>");
  process.exit(2);
}
let bad = 0;
const check = (ok, m) => (ok ? console.log("ok   " + m) : (bad++, console.error("FAIL " + m)));
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const sqlStr = (s) => `'${s.replace(/'/g, "''")}'`;
const SECRET = require("crypto").randomBytes(24).toString("hex");
const KEY = "sk-ant-e2e-" + Math.random().toString(16).slice(2);
const env = { ...process.env, DATAZOO_DISABLE_TELEMETRY: "1", SEC: SECRET, ANTHROPIC_API_KEY: KEY };

// The fake model: turn 1 reads panel 3 (the "Details" tab), turn 2 answers
// with hostile text and two citations.
const model = createServer((req, res) => {
  let body = "";
  req.on("data", (d) => (body += d));
  req.on("end", () => {
    const r = JSON.parse(body);
    const last = r.messages[r.messages.length - 1];
    const results = (last.content || []).filter((b) => b.type === "tool_result");
    const blocks = results.length
      ? [
          {
            type: "tool_use",
            id: "a",
            name: "answer",
            input: {
              text: "**EU** leads.<img src=x onerror=\"window.__pwned=1\"> See [here](https://evil.example/x) or javascript:alert(1)\n- one\n- two",
              citations: [
                { panel: 3, label: "EU", column: "Revenue", value: "10", row: [{ name: "Region", value: "EU" }] },
                { panel: 3, label: "Made up", column: "Revenue", value: "777", row: [{ name: "Region", value: "EU" }] },
              ],
              scope: "answered",
            },
          },
        ]
      : [{ type: "tool_use", id: "t", name: "get_panel", input: { panel: 3 } }];
    const out = JSON.stringify({ content: blocks, stop_reason: "tool_use", usage: { input_tokens: 100, output_tokens: 10 } });
    res.writeHead(200, { "content-type": "application/json", "request-id": "req_e2e" });
    res.end(out);
  });
});
await new Promise((r) => model.listen(0, "127.0.0.1", r));
const mport = model.address().port;

const dir = mkdtempSync(path.join(tmpdir(), "dp-ask-served-"));
const dashDir = path.join(dir, "dash");
mkdirSync(dashDir, { recursive: true });
writeFileSync(
  path.join(dashDir, "sales.sql"),
  `-- @title Sales
-- @ask on
-- @vars region
SELECT 'Overview'::TAB;
SELECT 42 AS n ::METRIC, 'Orders' ::LABEL;
SELECT 'Details'::TAB;
SELECT region AS "Region" ::XAXIS, v AS "Revenue" ::BARCHART, 'Revenue by region' ::TITLE
FROM (VALUES ('EU', 10), ('US', 7)) t(region, v);
`
);
writeFileSync(path.join(dashDir, "plain.sql"), "SELECT 1 AS n ::METRIC, 'One' ::LABEL;\n");

const opts = JSON.stringify({
  auth: { mode: "proxy", shared_secret_env: "SEC", trusted_proxies: ["127.0.0.1"] },
  policy: "none",
  ask: { enabled: true, base_url: `http://127.0.0.1:${mport}`, notice: "Ask IT if unsure." },
});
async function serve() {
  const p = spawn(duckdb, ["-unsigned", "-batch"], { stdio: ["pipe", "pipe", "pipe"], env });
  let log = "";
  p.stdout.on("data", (d) => (log += d));
  p.stderr.on("data", (d) => (log += d));
  p.stdin.write(`LOAD ${sqlStr(ext)};\nSELECT anofox_serve_dashboards(${sqlStr(dashDir)}, ${port}, ${sqlStr(opts)});\n`);
  for (let i = 0; i < 100; i++) {
    try {
      if ((await fetch(`http://127.0.0.1:${port}/healthz`)).ok) return { proc: p, log: () => log };
    } catch (_) {}
    if (p.exitCode != null) break;
    await sleep(100);
  }
  p.kill();
  throw new Error(`the server did not start:\n${log}`);
}

let srv = null;
let browser = null;
try {
  srv = await serve();
  browser = await chromium.launch();
  const ctx = await browser.newContext({
    viewport: { width: 1100, height: 760 },
    reducedMotion: "reduce",
    extraHTTPHeaders: { "X-Anofox-Proxy-Secret": SECRET, "X-Forwarded-User": "alice" },
  });
  const page = await ctx.newPage();
  const errs = [];
  page.on("pageerror", (e) => errs.push(String(e)));
  await page.goto(`http://127.0.0.1:${port}/d/sales`);
  await page.waitForSelector(".ax-ask-open", { timeout: 15000 });
  await page.waitForFunction(() => !!document.querySelector(".metric-value"), null, { timeout: 15000 });
  const open = page.locator(".ax-ask-open");
  check((await open.getAttribute("aria-expanded")) === "false", "the Ask button is there, closed");

  await open.click();
  check(await page.locator(".ax-ask-notice").isVisible(), "the notice comes first");
  const notice = await page.locator(".ax-ask-notice p").textContent();
  check(notice.includes("sent to Anthropic (Claude)") && notice.includes("Ask IT if unsure."), "the notice names the provider and the operator's text");
  check(!(await page.locator(".ax-ask-form").isVisible()), "no question box before Continue");
  await page.click(".ax-ask-continue");
  await page.fill(".ax-ask-input", "Which region leads?");
  await page.keyboard.press("Enter");
  await page.waitForSelector(".ax-ask-answer, .ax-ask-error", { timeout: 20000 });
  const err = await page.locator(".ax-ask-error").allTextContents();
  check(err.length === 0, `the question is answered${err.length ? ": " + err.join("; ") : ""}`);

  const ans = page.locator(".ax-ask-answer").first();
  check((await ans.locator("strong").textContent()) === "EU", "bold renders");
  check((await ans.locator("li").count()) === 2, "a list renders");
  check((await page.locator(".ax-ask img, .ax-ask a, .ax-ask script").count()) === 0, "no image, link or script from the answer");
  check((await ans.textContent()).includes('<img src=x onerror="window.__pwned=1">'), "the <img> is shown as text");
  check(!(await page.evaluate(() => window.__pwned)), "nothing ran");
  const inert = await ans.locator(".ax-ask-inert").allTextContents();
  check(inert.includes("https://evil.example/x") && inert.includes("javascript:alert(1)"), "a URL and a script scheme are inert text");
  const chip = ans.locator("button.ax-ask-chip").first();
  const chipText = (await chip.textContent()) || "";
  check(chipText.startsWith("Revenue by region · EU · Revenue: 10"), `the chip is built from the page's title and the verified cell (${chipText})`);
  check((await ans.locator(".ax-ask-unverified").textContent()).includes("not verified"), "the made-up value is not verified");
  check((await ans.locator(".ax-ask-label").textContent()).includes("AI-generated"), "labelled AI-generated");

  await chip.click();
  await page.waitForFunction(() => document.querySelector(".dash > .tabbar > .tab-btn.active")?.textContent === "Details", null, { timeout: 5000 }).catch(() => {});
  check((await page.locator(".dash > .tabbar > .tab-btn.active").textContent()) === "Details", "the chip opens the panel's tab");
  check((await page.locator(".dash .panel.dp-ask-hit").count()) === 1, "and highlights the panel");
  check(await page.evaluate(() => !!document.activeElement?.closest(".panel")), "the focus moves to the panel");
  check(await page.locator(".ax-ask-back").isVisible(), "with a way back to the answer");

  await page.focus(".ax-ask-input");
  await page.keyboard.press("Escape");
  check(await page.locator(".ax-ask").isHidden(), "Esc closes the panel");

  // the phone layout
  await page.setViewportSize({ width: 390, height: 800 });
  await open.click();
  const w = await page.locator(".ax-ask").evaluate((n) => n.getBoundingClientRect().width);
  check(Math.abs(w - 390) < 2, `full width on a phone (${w})`);
  check((await page.locator(".ax-ask").getAttribute("role")) === "region" && (await page.locator(".ax-ask-log").getAttribute("role")) === "log", "a labelled region with a log");

  // no Ask where the dashboard doesn't turn it on
  await page.goto(`http://127.0.0.1:${port}/d/plain`);
  await page.waitForFunction(() => !!document.querySelector(".metric-value"), null, { timeout: 15000 });
  check((await page.locator(".ax-ask-open").count()) === 0, "no Ask button without -- @ask on");
  check(!(await page.evaluate(() => "ask" in (window.__served || {}))), "and no ask settings in the page");
  check(errs.length === 0, `no page errors${errs.length ? ": " + errs.join("; ") : ""}`);
} catch (e) {
  bad++;
  console.error("FAIL " + (e && e.stack ? e.stack : e));
  if (srv) console.error(srv.log().slice(-3000));
} finally {
  if (srv) {
    srv.proc.stdin.end();
    srv.proc.kill(); // our own child, by its handle
  }
  if (browser) await browser.close();
  model.close();
  rmSync(dir, { recursive: true, force: true });
}
console.log(`ask-served-e2e: ${bad} failed`);
process.exit(bad ? 1 : 0);
