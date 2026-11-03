// Saved views on a locked server (#913, options.views, proxy auth): a
// signed-in viewer saves a view from the served page's header (Views →
// Save this view…); the server keeps it in its own file (kind "views"), so
// the same login sees it in a second browser and opening it restores the
// input; another viewer never sees it. The endpoint keeps only page state
// (tab, v.*, frame.*, view.*), caps views per dashboard, deletes by id,
// holds the cross-site gate on writes, refuses a shared credential, and
// reads the file back after a restart; options.views without a login
// refuses to start.
//
//   NODE_PATH=<dir with playwright>/node_modules node scripts/views-served-e2e.mjs \
//       --ext path/to/anofox_visualization.duckdb_extension [--port 8967] [--duckdb duckdb]
import { createRequire } from "module";
import { spawn } from "child_process";
import { mkdtempSync, mkdirSync, writeFileSync, readFileSync, rmSync, existsSync } from "fs";
import path from "path";
import crypto from "crypto";

const require = createRequire(import.meta.url);
const { chromium } = require("playwright");
const arg = (k, d) => {
  const i = process.argv.indexOf(k);
  return i > 0 ? process.argv[i + 1] : d;
};
const ext = arg("--ext", "");
const port = Number(arg("--port", "8967"));
const duckdb = arg("--duckdb", "duckdb");
if (!ext || !existsSync(ext)) {
  console.error("usage: views-served-e2e.mjs --ext <anofox_visualization.duckdb_extension> (build it with duckext/scripts/build-native.sh)");
  process.exit(2);
}
let bad = 0;
const check = (ok, m) => (ok ? console.log("ok   " + m) : (bad++, console.error("FAIL " + m)));
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const sqlStr = (s) => `'${s.replace(/'/g, "''")}'`;
const env = { ...process.env, DATAZOO_DISABLE_TELEMETRY: "1" };

// Scratch next to the extension (the file's path rules refuse a
// world-writable parent, like options.state's).
const W = mkdtempSync(path.join(path.dirname(path.resolve(ext)), "views-served-e2e-"));
const dashDir = path.join(W, "dash");
mkdirSync(dashDir, { recursive: true });
const SALES = `-- @title Sales
SELECT DISTINCT region ::DROPDOWN FROM sales ORDER BY region;
SELECT sum(units) AS units ::METRIC, 'Units' ::LABEL FROM sales WHERE region = coalesce(getvariable('region'), region);
`;
writeFileSync(path.join(dashDir, "sales.sql"), SALES);
writeFileSync(path.join(dashDir, "other.sql"), SALES.replace("-- @title Sales", "-- @title Other"));
const SETUP = `CREATE TABLE sales AS SELECT * FROM (VALUES ('east', 10), ('west', 32), ('north', 7)) v(region, units);`;
const sec = crypto.randomBytes(24).toString("hex");
const viewsPath = path.join(W, "views.jsonl");
const auth = { mode: "proxy", shared_secret: sec, trusted_proxies: ["127.0.0.1"] };
const opts = { auth, policy: "none", views: { path: viewsPath, max_per_dashboard: 3 } };

function spawnServer(o) {
  const p = spawn(duckdb, ["-unsigned", "-batch", ":memory:"], { stdio: ["pipe", "pipe", "pipe"], env });
  let log = "";
  p.stdout.on("data", (d) => (log += d));
  p.stderr.on("data", (d) => (log += d));
  p.stdin.write(`LOAD ${sqlStr(ext)};\n${SETUP}\nSELECT anofox_serve_dashboards(${sqlStr(dashDir)}, ${port}, ${sqlStr(JSON.stringify(o))});\n`);
  return { proc: p, log: () => log };
}
async function serve(o = opts) {
  const s = spawnServer(o);
  for (let i = 0; i < 100; i++) {
    try {
      if ((await fetch(`http://127.0.0.1:${port}/healthz`)).ok) return s;
    } catch (_) {}
    if (s.proc.exitCode != null) break;
    await sleep(100);
  }
  s.proc.kill();
  throw new Error(`the server did not start:\n${s.log()}`);
}
const stop = async (s) => {
  s.proc.stdin.end();
  s.proc.kill(); // our own child (pid known), never by name
  for (let i = 0; i < 50 && s.proc.exitCode == null && s.proc.signalCode == null; i++) await sleep(100);
};
const BASE = `http://127.0.0.1:${port}`;
// A direct request as `user` through the trusted proxy.
async function api(user, method, p, body, extra = {}) {
  const headers = { "X-Anofox-Proxy-Secret": sec, "X-Forwarded-User": user };
  if (body !== undefined) Object.assign(headers, { "Content-Type": "application/json", Origin: BASE });
  Object.assign(headers, extra);
  for (const [k, v] of Object.entries(headers)) if (v === null) delete headers[k];
  const r = await fetch(BASE + p, { method, headers, body: body === undefined ? undefined : JSON.stringify(body) });
  const text = await r.text();
  let json = null;
  try {
    json = JSON.parse(text);
  } catch (_) {}
  return { status: r.status, text, json };
}
const W1 = { "X-Anofox-Views": "1" };
const list = async (user, dash = "sales") => (await api(user, "GET", `/api/views?dashboard=${dash}`)).json?.views ?? null;
const records = () =>
  readFileSync(viewsPath, "utf8")
    .split("\n")
    .filter(Boolean)
    .map((l) => JSON.parse(l));

let srv = null;
let browser = null;
try {
  // ── a login is required ──
  {
    const s = spawnServer({ policy: "none", views: { path: viewsPath } });
    for (let i = 0; i < 100 && !/options\.views needs options\.auth/.test(s.log()); i++) await sleep(100);
    check(/options\.views needs options\.auth/.test(s.log()), "options.views without a login refuses to start");
    await stop(s);
  }

  srv = await serve();
  check(records()[0]?.kind === "views", `the file's header says what it is (${JSON.stringify(records()[0])})`);

  // ── the page: save from the header, see it in another browser ──
  browser = await chromium.launch();
  async function openAs(user, url) {
    const ctx = await browser.newContext({ viewport: { width: 1200, height: 900 }, reducedMotion: "reduce" });
    await ctx.setExtraHTTPHeaders({ "X-Anofox-Proxy-Secret": sec, "X-Forwarded-User": user });
    const page = await ctx.newPage();
    const errs = [];
    page.on("pageerror", (e) => errs.push(String(e)));
    await page.goto(BASE + url);
    await page.waitForFunction(() => !document.body.classList.contains("loading") && !!document.querySelector(".metric-value"), null, { timeout: 20000 });
    return { ctx, page, errs };
  }
  const kpi = (page) => page.evaluate(() => document.querySelector(".metric-value")?.textContent?.trim());
  const menuItems = (page) => page.evaluate(() => [...document.querySelectorAll(".side-menu .side-mi")].map((b) => b.textContent));

  const a1 = await openAs("alice", "/d/sales/?v.region=west");
  check((await kpi(a1.page)) === "32", `the seeded input filters (${await kpi(a1.page)})`);
  check(await a1.page.locator(".dp-head .dp-views-btn").isVisible(), "a served page shows Views in its header");
  await a1.page.click(".dp-head .dp-views-btn");
  await a1.page.waitForSelector(".side-menu");
  check(JSON.stringify(await menuItems(a1.page)) === JSON.stringify(["Save this view…"]), `no views yet (${await menuItems(a1.page)})`);
  await a1.page.click('.side-menu .side-mi:text-is("Save this view…")');
  await a1.page.fill(".dp-dialog input", "West only");
  await a1.page.click('.dp-dialog button:text-is("Save")');
  await a1.page.waitForFunction(() => /saved/.test(document.getElementById("side-toast")?.textContent || ""), null, { timeout: 10000 });
  const saved = records().slice(1);
  check(saved.length === 1 && saved[0].user === "alice" && saved[0].submit === "view" && saved[0].vars.name === "West only", `one line in the file (${JSON.stringify(saved)})`);
  check(saved[0]?.vars.query === "v.region=west", `it keeps the page state only (${saved[0]?.vars.query})`);
  check(a1.errs.length === 0, `no page error (${a1.errs})`);

  const a2 = await openAs("alice", "/d/sales/");
  check((await kpi(a2.page)) === "10", `a second browser starts on the default (${await kpi(a2.page)})`);
  await a2.page.click(".dp-head .dp-views-btn");
  await a2.page.waitForSelector(".side-menu");
  check((await menuItems(a2.page)).includes("West only"), `the second browser lists alice's view (${await menuItems(a2.page)})`);
  await a2.page.click('.side-menu .side-mi:text-is("West only")');
  await a2.page.waitForURL(/v\.region=west/, { timeout: 10000 });
  await a2.page.waitForFunction(() => document.querySelector(".metric-value")?.textContent?.trim() === "32", null, { timeout: 20000 });
  check(new URL(a2.page.url()).pathname === "/d/sales/", `opening it stays on this dashboard (${a2.page.url()})`);
  check((await kpi(a2.page)) === "32", "opening it restores the input");

  const b1 = await openAs("bob", "/d/sales/");
  await b1.page.click(".dp-head .dp-views-btn");
  await b1.page.waitForSelector(".side-menu");
  check(!(await menuItems(b1.page)).includes("West only"), "another viewer never sees alice's view");
  check(JSON.stringify(await list("bob")) === "[]", "bob's list is empty");
  check((await list("alice", "other")).length === 0, "views are per dashboard");

  // ── the endpoint ──
  let r = await api("alice", "POST", "/api/views", { dashboard: "sales", name: "x", query: "v.region=west" });
  check(r.status === 403 && /X-Anofox-Views/.test(r.text), `a write without the custom header is refused (${r.status} ${r.text})`);
  r = await api("alice", "POST", "/api/views", { dashboard: "sales", name: "x", query: "" }, { ...W1, Origin: "https://evil.example" });
  check(r.status === 403, `a cross-site Origin is refused (${r.status} ${r.text})`);
  r = await api("alice", "POST", "/api/views", { dashboard: "sales", name: "x", query: "" }, { ...W1, Origin: null });
  check(r.status === 403, `a write without an Origin is refused (${r.status})`);
  r = await api("alice", "POST", "/api/views", { dashboard: "sales", name: "x", query: "" }, { ...W1, "Content-Type": "text/plain" });
  check(r.status === 403, `a non-JSON write is refused (${r.status})`);
  r = await api("alice", "GET", "/api/views?dashboard=sales", undefined, { "Sec-Fetch-Site": "cross-site" });
  check(r.status === 403, `a cross-site read is refused (${r.status})`);
  r = await api("alice", "POST", "/api/views", { dashboard: "sales", name: "Evil", query: "next=https://evil.example" }, W1);
  check(r.status === 400 && /keeps only/.test(r.text), `a query beyond page state is refused (${r.status} ${r.text})`);
  r = await api("alice", "POST", "/api/views", { dashboard: "sales", name: "x".repeat(81), query: "" }, W1);
  check(r.status === 400, `an over-long name is refused (${r.status})`);
  r = await api("alice", "POST", "/api/views", { dashboard: "nope", name: "x", query: "" }, W1);
  check(r.status === 404, `an unknown dashboard is 404 (${r.status})`);
  r = await api("share:x", "GET", "/api/views?dashboard=sales");
  check(r.status === 403 || r.status === 401, `a reserved identity is refused (${r.status})`);

  r = await api("alice", "POST", "/api/views", { dashboard: "sales", name: "West", query: "?v.region=west&tab=Main" }, W1);
  check(r.status === 201 && r.json?.view?.query === "v.region=west&tab=Main", `a direct save answers with the view (${r.status} ${r.text})`);
  r = await api("alice", "POST", "/api/views", { dashboard: "sales", name: "North", query: "v.region=north" }, W1);
  check(r.status === 201, "a third view fits");
  r = await api("alice", "POST", "/api/views", { dashboard: "sales", name: "Fourth", query: "" }, W1);
  check(r.status === 409 && /at most 3/.test(r.text), `max_per_dashboard holds (${r.status} ${r.text})`);
  r = await api("alice", "POST", "/api/views", { dashboard: "sales", name: "West", query: "v.region=west" }, W1);
  check(r.status === 201, "the same name replaces, also at the cap");
  let l = await list("alice");
  check(JSON.stringify(l.map((v) => v.name)) === JSON.stringify(["West", "North", "West only"]), `newest first, West once (${l.map((v) => v.name)})`);
  check(l[0].query === "v.region=west", "the replaced view has the new state");

  const north = l.find((v) => v.name === "North").id;
  r = await api("bob", "POST", "/api/views/delete", { dashboard: "sales", id: north }, W1);
  check(r.status === 404, `bob can't delete alice's view (${r.status})`);
  r = await api("alice", "POST", "/api/views/delete", { dashboard: "sales", id: north }, W1);
  check(r.status === 200, `alice deletes hers (${r.status} ${r.text})`);
  r = await api("alice", "POST", "/api/views/delete", { dashboard: "sales", id: north }, W1);
  check(r.status === 404, "a second delete finds nothing");
  r = await api("alice", "POST", "/api/views/delete", { dashboard: "sales", id: "../x" }, W1);
  check(r.status === 400, "a malformed id is refused");
  l = await list("alice");
  check(JSON.stringify(l.map((v) => v.name)) === JSON.stringify(["West", "West only"]), `the list after the delete (${l.map((v) => v.name)})`);

  // ── a restart reads the file back ──
  await stop(srv);
  srv = await serve();
  l = await list("alice");
  check(JSON.stringify(l.map((v) => v.name)) === JSON.stringify(["West", "West only"]), `after a restart (${l.map((v) => v.name)})`);
} catch (e) {
  bad++;
  console.error("FAIL", e);
  if (srv) console.error(srv.log());
} finally {
  if (browser) await browser.close();
  if (srv) await stop(srv);
  rmSync(W, { recursive: true, force: true });
}
if (bad) {
  console.error(`${bad} check(s) failed`);
  process.exit(1);
}
console.log("all checks passed");
