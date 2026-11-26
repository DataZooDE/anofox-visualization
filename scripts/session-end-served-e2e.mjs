// #1391 (3): a session that ended reads nothing more, not even an answer the
// server already holds (result cache, warm connections), and the next viewer
// on the same connections gets only their own rows. Embed page sessions with
// an 8 s lifetime, under a row policy (a logged-out OIDC cookie replayed is
// scripts/auth-e2e/p3.mjs):
//
//   - alice's page renders; her exact panel request, replayed with her session
//     while it lives → 200 with her rows (an answer the server now holds);
//   - after the session's end the same replay → 401, an input change on her
//     open page draws no new rows and no other viewer's;
//   - bob, straight after on the same warm server → his rows only, and alice's
//     replay with bob's panel body → still 401;
//   - a session the server signed for another dashboard or tampered → 401/403.
//
//   node scripts/session-end-served-e2e.mjs \
//       --ext path/to/anofox_visualization.duckdb_extension [--port 8973] [--duckdb duckdb]
import { createRequire } from "module";
import { spawn } from "child_process";
import { mkdtempSync, mkdirSync, writeFileSync, rmSync, existsSync } from "fs";
import path from "path";
import crypto from "crypto";

const require = createRequire(import.meta.url);
const { chromium, request } = require("playwright");
const arg = (k, d) => {
  const i = process.argv.indexOf(k);
  return i > 0 ? process.argv[i + 1] : d;
};
const ext = arg("--ext", "");
const port = Number(arg("--port", "8973"));
const duckdb = arg("--duckdb", "duckdb");
if (!ext || !existsSync(ext)) {
  console.error("usage: session-end-served-e2e.mjs --ext <anofox_visualization.duckdb_extension>");
  process.exit(2);
}
let bad = 0;
const check = (ok, m) => (ok ? console.log("ok   " + m) : (bad++, console.error("FAIL " + m)));
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const sqlStr = (s) => `'${s.replace(/'/g, "''")}'`;
const TTL = 8;
const EMBED_SECRET = crypto.randomBytes(32).toString("hex");
const SESSION_SECRET = crypto.randomBytes(32).toString("hex");
const env = { ...process.env, DATAZOO_DISABLE_TELEMETRY: "1", EMBED_SECRET, SESSION_SECRET };
const b64u = (b) => Buffer.from(b).toString("base64url");
const hs256 = (payload) => {
  const h = b64u(JSON.stringify({ alg: "HS256", typ: "JWT" })),
    p = b64u(JSON.stringify(payload));
  return `${h}.${p}.${crypto.createHmac("sha256", EMBED_SECRET).update(`${h}.${p}`).digest("base64url")}`;
};
const link = (user, dashboard = "rev") => hs256({ dashboard, user, exp: Math.floor(Date.now() / 1000) + 60 });

const W = mkdtempSync(path.join(path.dirname(path.resolve(ext)), "session-end-e2e-"));
const dashDir = path.join(W, "dash");
mkdirSync(dashDir, { recursive: true });
writeFileSync(
  path.join(dashDir, "rev.sql"),
  `-- @title Revenue
-- @vars channel
SELECT channel ::DROPDOWN, 'Channel'::LABEL FROM (SELECT DISTINCT channel FROM orders) ORDER BY channel;
SELECT region ::TABLE, channel, revenue FROM orders WHERE getvariable('channel') IS NULL OR channel = getvariable('channel') ORDER BY region, channel;
`
);
writeFileSync(path.join(dashDir, "other.sql"), `-- @title Other\nSELECT 'other dashboard'::MARKDOWN;\n`);
const SETUP = `CREATE TABLE orders AS SELECT * FROM (VALUES
  ('Avalon','web',1101), ('Avalon','app',1102),
  ('Brigadoon','web',1103), ('Brigadoon','app',1104)) t(region, channel, revenue);
CREATE TABLE entitlements AS SELECT * FROM (VALUES ('alice','Avalon'), ('bob','Brigadoon')) t(viewer, region);`;
const policyFile = path.join(W, "policy.json");
writeFileSync(
  policyFile,
  JSON.stringify({
    tables: {
      orders: { rows: "region IN (SELECT region FROM entitlements WHERE viewer = getvariable('__claim_user'))" },
      entitlements: { rows: "viewer = getvariable('__claim_user')" },
    },
  })
);
const opts = {
  auth: { mode: "embed", secret_env: "EMBED_SECRET", session_secret_env: "SESSION_SECRET", session_ttl_s: TTL },
  policy: policyFile,
};
const BASE = `http://127.0.0.1:${port}`;
// what a viewer's page must show (the first channel is the default), and
// every value of theirs the other viewer must never get
const SEE = { alice: ["Avalon"], bob: ["Brigadoon"] };
const OWN = { alice: ["Avalon", "1101", "1102"], bob: ["Brigadoon", "1103", "1104"] };

let proc = null;
let log = "";
let browser = null;
try {
  proc = spawn(duckdb, ["-unsigned", "-batch", ":memory:"], { stdio: ["pipe", "pipe", "pipe"], env });
  proc.stdout.on("data", (d) => (log += d));
  proc.stderr.on("data", (d) => (log += d));
  proc.stdin.write(`LOAD ${sqlStr(ext)};\n${SETUP}\nSELECT anofox_serve_dashboards(${sqlStr(dashDir)}, ${port}, ${sqlStr(JSON.stringify(opts))});\n`);
  let up = false;
  for (let i = 0; i < 100 && !up; i++) {
    try {
      up = (await fetch(`${BASE}/healthz`)).ok;
    } catch (_) {}
    if (proc.exitCode != null) break;
    if (!up) await sleep(100);
  }
  if (!up) throw new Error(`the server did not start:\n${log}`);

  browser = await chromium.launch();
  const api = await request.newContext();
  const replay = (body, session) =>
    api.post(`${BASE}/api/panel`, { headers: { "Content-Type": "application/json", "X-Anofox-Session": session }, data: body });
  // Open the dashboard as `user`; returns the page, its session and the body
  // of the panel request that drew the table.
  const open = async (user) => {
    const ctx = await browser.newContext({ viewport: { width: 1200, height: 800 } });
    const page = await ctx.newPage();
    const errs = [];
    page.on("pageerror", (e) => errs.push(String(e)));
    const bodies = [];
    page.on("request", (r) => r.url().endsWith("/api/panel") && bodies.push(r.postDataJSON()));
    await page.goto(`${BASE}/d/rev?embed=${link(user)}`);
    await page.waitForFunction(() => /Avalon|Brigadoon/.test(document.querySelector(".dash")?.innerText || ""), null, { timeout: 30000 });
    await page.waitForTimeout(500);
    const session = await page.evaluate(() => window.__served.session);
    const born = Date.now();
    const text = () => page.evaluate(() => document.querySelector(".dash").innerText);
    return { ctx, page, errs, bodies, session, born, text };
  };
  // a value as text: plain, or with a thousands separator (a table prints 1,101)
  const hasVal = (t, v) => [v, v.replace(/^(\d)(\d{3})$/, "$1,$2")].some((x) => t.includes(x));
  // the table's request: the dashboard's last panel
  const tableOf = (bodies) => bodies.filter((b) => b && b.dashboard === "rev").reduce((x, y) => (y.panel > x.panel ? y : x));
  const has = (t, words) => words.every((w) => hasVal(t, w));
  const none = (t, words) => words.every((w) => !hasVal(t, w));

  // 1. alice, live session
  const a = await open("alice");
  check(!!a.session, "alice: the embed link became a page session");
  const at = await a.text();
  check(has(at, SEE.alice) && none(at, OWN.bob), "alice: her rows only");
  const tableBody = tableOf(a.bodies);
  let r = await replay(tableBody, a.session);
  const live = await r.text();
  check(r.status() === 200 && has(live, SEE.alice) && none(live, OWN.bob), `alice: her panel request replayed while the session lives → 200, her rows (${r.status()})`);
  r = await replay({ ...tableBody, dashboard: "other" }, a.session);
  check(r.status() === 403, `alice: her session on another dashboard → 403 (${r.status()})`);
  const parts = a.session.split(".");
  const forged = [parts[0], b64u(JSON.stringify({ ...JSON.parse(Buffer.from(parts[1], "base64url")), u: "bob" })), parts[2]].join(".");
  r = await replay(tableBody, forged);
  check(r.status() === 401, `a session with its user changed → 401 (${r.status()})`);

  // 2. the session ends
  await sleep(Math.max(0, a.born + (TTL + 1.5) * 1000 - Date.now()));
  r = await replay(tableBody, a.session);
  check(r.status() === 401, `alice: the same request after the session's end → 401, not the cached answer (${r.status()})`);
  check(none(await r.text(), [...OWN.alice, ...OWN.bob]), "alice: the refusal carries no rows");
  const before = await a.text();
  const panelStatus = [];
  a.page.on("response", (res) => res.url().endsWith("/api/panel") && panelStatus.push(res.status()));
  const sel = a.page.locator(".dash select").first();
  const other = await sel.evaluate((s) => [...s.options].map((o) => o.value).find((v) => v && v !== s.value));
  await sel.selectOption(other);
  await a.page.waitForTimeout(2500);
  const after = await a.text();
  check(panelStatus.length > 0 && panelStatus.every((s) => s === 401), `alice: her open page's next panel requests are refused (${panelStatus})`);
  check(none(after, OWN.bob), "alice: her expired page shows no other viewer's rows");

  // 3. bob, right after, on the same (warm) server
  const b = await open("bob");
  const bt = await b.text();
  check(has(bt, SEE.bob) && none(bt, OWN.alice), "bob: straight after alice, on the warm server → his rows only");
  const bobBody = tableOf(b.bodies);
  r = await replay(bobBody, a.session);
  check(r.status() === 401, `alice's ended session with bob's request → 401 (${r.status()})`);
  r = await replay(tableBody, b.session);
  const bobReplay = await r.text();
  check(r.status() === 200 && has(bobReplay, SEE.bob) && none(bobReplay, OWN.alice), `bob's session with alice's request → his rows, not her cached answer (${r.status()})`);
  check(!a.errs.length && !b.errs.length, "no page errors: " + [...a.errs, ...b.errs].join(" | "));
  await a.ctx.close();
  await b.ctx.close();
} catch (e) {
  bad++;
  console.error("FAIL " + (e.stack || e));
  console.error(log);
} finally {
  if (browser) await browser.close();
  if (proc) {
    proc.stdin.end();
    proc.kill(); // our own child (pid known), never by name
  }
  rmSync(W, { recursive: true, force: true });
}
console.log(`session-end-served-e2e: ${bad} failed`);
process.exit(bad ? 1 : 0);
