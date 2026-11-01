// #771 in a real browser: <anofox-dashboard> (sdk/anofox-embed.js) on a host
// page of another origin. The host's "backend" (this script) mints a token at
// POST /api/auth/token with an API key that has the embed scope; the element
// loads the dashboard in an iframe, sizes it to the dashboard's height, gets
// "anofox-state" with the variables after every run, sets a variable with
// el.setVars() (the panel follows), gets "anofox-link" for a link that leaves
// the dashboard (the frame doesn't navigate), and the token's claim is the
// key's. A token for a dashboard outside the key's scope, or another claim,
// is refused; a frame on an origin outside frame_ancestors gets no bridge.
//
//   NODE_PATH=<dir with playwright>/node_modules node scripts/embed-e2e.mjs \
//       --ext path/to/anofox_visualization.duckdb_extension [--port 8936] [--duckdb duckdb]
import { createRequire } from "module";
import { spawn } from "child_process";
import { createHash, randomBytes } from "crypto";
import { mkdtempSync, writeFileSync, chmodSync, rmSync, existsSync } from "fs";
import { createServer } from "http";
import { tmpdir } from "os";
import path from "path";

const require = createRequire(import.meta.url);
const { chromium } = require("playwright");
const arg = (k, d) => {
  const i = process.argv.indexOf(k);
  return i > 0 ? process.argv[i + 1] : d;
};
const ext = arg("--ext", "");
const port = Number(arg("--port", "8936"));
const hostPort = port + 1;
const otherPort = port + 2;
const duckdb = arg("--duckdb", "duckdb");
if (!ext || !existsSync(ext)) {
  console.error("usage: embed-e2e.mjs --ext <anofox_visualization.duckdb_extension>");
  process.exit(2);
}
const fail = (m) => {
  console.error("FAIL: " + m);
  process.exitCode = 1;
};
const check = (ok, m) => (ok ? console.log("ok   " + m) : fail(m));
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const sqlStr = (s) => `'${s.replace(/'/g, "''")}'`;
const dash = `http://127.0.0.1:${port}`;

const dir = mkdtempSync(path.join(tmpdir(), "dp-embed-"));
const dashDir = path.join(dir, "dash");
require("fs").mkdirSync(dashDir);
writeFileSync(
  path.join(dashDir, "sales.sql"),
  `-- @title Sales
SELECT r AS region ::DROPDOWN FROM (VALUES ('EU'), ('US')) t(r);
SELECT 'region=' || coalesce(getvariable('region'), '?') AS shown ::TABLE, 'tenant=' || coalesce(getvariable('__claim_tenant'), '?') AS who, 'Where'::TITLE;
SELECT 'other' AS name ::TABLE, '/d/other' AS go ::LINK, 'Links'::TITLE;
`
);
writeFileSync(path.join(dashDir, "other.sql"), "SELECT 1 AS n ::METRIC;\n");
writeFileSync(path.join(dashDir, "payroll.sql"), "SELECT 2 AS n ::METRIC;\n");
// The host's API key: embed scope, the sales and other dashboards, tenant=acme.
const id = randomBytes(6).toString("hex");
const key = `afx_${id}_${randomBytes(32).toString("hex")}`;
const keys = path.join(dir, "keys.json");
writeFileSync(
  keys,
  JSON.stringify({
    keys: [
      {
        id,
        sha256: createHash("sha256").update(key).digest("hex"),
        user: "svc-host",
        claims: { tenant: "acme" },
        dashboards: ["sales", "other"],
        scopes: ["embed"],
      },
    ],
  })
);
chmodSync(keys, 0o600);
const options = {
  auth: [
    { mode: "embed", secret_env: "AFX_EMBED_SECRET", claims: ["tenant"], max_link_ttl_s: 120 },
    { mode: "api_keys", file: keys },
  ],
  policy: "none",
  frame_ancestors: [`http://127.0.0.1:${hostPort}`],
};
const env = { ...process.env, AFX_EMBED_SECRET: randomBytes(24).toString("hex") };
const proc = spawn(duckdb, ["-unsigned", "-batch", ":memory:"], { stdio: ["pipe", "pipe", "pipe"], env });
let log = "";
proc.stdout.on("data", (d) => (log += d));
proc.stderr.on("data", (d) => (log += d));
proc.stdin.write(`LOAD ${sqlStr(ext)};\nSELECT anofox_serve_dashboards(${sqlStr(dashDir)}, ${port}, ${sqlStr(JSON.stringify(options))});\n`);
let up = false;
for (let i = 0; i < 100 && !up; i++) {
  try {
    up = (await fetch(`${dash}/healthz`)).ok;
  } catch (_) {}
  if (!up) await sleep(200);
}
if (!up) {
  console.error(log);
  proc.kill();
  process.exit(1);
}

const mint = async (body) => {
  const r = await fetch(`${dash}/api/auth/token`, {
    method: "POST",
    headers: { Authorization: `Bearer ${key}`, "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });
  const text = await r.text();
  let parsed;
  try {
    parsed = JSON.parse(text);
  } catch (_) {
    parsed = { text };
  }
  return { status: r.status, body: parsed };
};
// The token endpoint caps a token by its key.
{
  const ok = await mint({ dashboard: "sales" });
  check(ok.status === 200 && typeof ok.body.token === "string", `token minted (${ok.status})`);
  check((await mint({ dashboard: "payroll" })).status === 400, "no token for a dashboard outside the key's scope");
  check((await mint({ dashboard: "sales", claims: { tenant: "globex" } })).status === 400, "no token with another claim");
  const anon = await fetch(`${dash}/api/auth/token`, { method: "POST", body: "{}" });
  check(anon.status === 401, `the token endpoint needs a key (${anon.status})`);
}

// The host page (another origin), and one more origin outside frame_ancestors.
const hostHtml = `<!doctype html><meta charset="utf-8"><title>Host</title>
<script type="module" src="${dash}/sdk/anofox-embed.js"></script>
<h1>Host page</h1>
<anofox-dashboard id="d" src="${dash}/d/sales" token-callback="getToken"></anofox-dashboard>
<script>
  window.events = [];
  async function getToken(dashboard) { return await window.mintToken(dashboard); }
  const d = document.getElementById("d");
  for (const t of ["anofox-ready", "anofox-state", "anofox-link", "anofox-error"])
    d.addEventListener(t, (e) => window.events.push({ type: t, detail: e.detail }));
</script>`;
const servers = [hostPort, otherPort].map((p) =>
  createServer((req, res) => {
    res.writeHead(200, { "Content-Type": "text/html" });
    res.end(hostHtml);
  }).listen(p, "127.0.0.1")
);

const browser = await chromium.launch();
try {
  const page = await browser.newPage({ viewport: { width: 1200, height: 900 } });
  const errs = [];
  page.on("pageerror", (e) => errs.push(String(e)));
  await page.exposeFunction("mintToken", async (dashboard) => (await mint({ dashboard })).body.token);
  await page.goto(`http://127.0.0.1:${hostPort}/`);
  const ready = await page
    .waitForFunction(() => window.events.some((e) => e.type === "anofox-state"), null, { timeout: 60000 })
    .then(() => true, () => false);
  check(ready, "the element got anofox-ready / anofox-state from the dashboard");
  const frame = page.frames().find((f) => f.url().startsWith(dash + "/d/sales"));
  check(!!frame, "the dashboard is in an iframe on its own origin");
  if (frame) {
    await frame.waitForFunction(() => /region=/.test(document.body.innerText), null, { timeout: 30000 });
    const shown = await frame.evaluate(() => document.body.innerText);
    check(/tenant=acme/.test(shown), "the token carries the key's claim (tenant=acme)");
    await sleep(500);
    const h = await page.evaluate(() => parseInt(document.getElementById("d").shadowRoot.querySelector("iframe").style.height, 10));
    const content = await frame.evaluate(() => Math.ceil(document.querySelector(".dash").getBoundingClientRect().bottom + 16));
    check(Number.isFinite(h) && Math.abs(h - content) <= 4, `the frame is sized to the dashboard (${h}px for ${content}px of content)`);
    // setVars → the dashboard re-runs with it, and reports it back
    await page.evaluate(() => document.getElementById("d").setVars({ region: "US" }));
    const set = await frame
      .waitForFunction(() => /region=US/.test(document.body.innerText), null, { timeout: 30000 })
      .then(() => true, () => false);
    check(set, "el.setVars({region: 'US'}) sets the dashboard's variable");
    const state = await page
      .waitForFunction(() => window.events.some((e) => e.type === "anofox-state" && e.detail.vars && e.detail.vars.region === "US"), null, { timeout: 15000 })
      .then(() => true, () => false);
    check(state, "anofox-state reports region=US");
    // a link out of the dashboard: an event for the host, the frame stays
    const before = frame.url();
    await frame.evaluate(() => document.querySelector(".dp-link")?.click());
    const linked = await page
      .waitForFunction(() => window.events.some((e) => e.type === "anofox-link" && /\/d\/other$/.test(e.detail.href)), null, { timeout: 10000 })
      .then(() => true, () => false);
    check(linked, "anofox-link carries the link's href");
    await sleep(300);
    check(frame.url() === before, "the frame did not navigate (that would drop its session)");
  }
  check(errs.length === 0, `no page errors on the host ${errs.join("; ")}`);
  // An origin outside frame_ancestors: the browser refuses the frame (CSP),
  // so no state ever reaches that host.
  const other = await browser.newPage();
  await other.exposeFunction("mintToken", async (dashboard) => (await mint({ dashboard })).body.token);
  await other.goto(`http://127.0.0.1:${otherPort}/`);
  await sleep(4000);
  const leaked = await other.evaluate(() => window.events.filter((e) => e.type === "anofox-state").length);
  check(leaked === 0, `a host outside frame_ancestors gets no state (${leaked})`);
} finally {
  await browser.close();
  servers.forEach((s) => s.close());
  proc.stdin.write(`SELECT anofox_serve_stop(${port});\n`);
  proc.stdin.end();
  await sleep(300);
  proc.kill();
  rmSync(dir, { recursive: true, force: true });
}
