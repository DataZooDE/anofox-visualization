// End-to-end check of the served example: starts the server on fresh data
// (its own work folder), drives it over HTTP like the browser client does,
// and stops it. Node 18+, no dependencies.
//
//   node examples/served/smoke.mjs [--port 8912] [--auth] [--browser]
//
// --auth     also run the login flow through auth-proxy.mjs (needs a server
//            with options.auth, PR #78) and the onboarding quiz's per-viewer
//            state (::SUBMIT, viewer_state, trainer scoping; options.state,
//            #517, only configured in this mode since it needs auth)
// --browser  also open each dashboard in Chromium and fail on an error panel
//            (needs Playwright: NODE_PATH=<dir>/node_modules)
// Uses DUCKDB and ANOFOX_EXT like serve.sh.
import { spawn, execFileSync } from "node:child_process";
import { existsSync, readFileSync, rmSync } from "node:fs";
import { createRequire } from "node:module";
import path from "node:path";
import { fileURLToPath } from "node:url";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const arg = (k, d) => {
  const i = process.argv.indexOf(k);
  return i > 0 ? process.argv[i + 1] : d;
};
const PORT = Number(arg("--port", 8912));
const WITH_AUTH = process.argv.includes("--auth");
const WITH_BROWSER = process.argv.includes("--browser");
const WORK = path.join(HERE, "work", "smoke");
const env = { ...process.env, WORK };

let failures = 0;
const ok = (cond, what, detail = "") => {
  console.log(`${cond ? "  ok  " : "  FAIL"} ${what}${detail ? ` — ${detail}` : ""}`);
  if (!cond) failures++;
};
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const sh = (...args) => execFileSync(path.join(HERE, "serve.sh"), args, { env, encoding: "utf8" });
const gen = (...args) => execFileSync(path.join(HERE, "generate.sh"), args, { env, encoding: "utf8" });

// ── the server ────────────────────────────────────────────────────────────
let server = null;
let log = "";
async function start(...extra) {
  log = "";
  server = spawn(path.join(HERE, "serve.sh"), ["start", "--port", String(PORT), "--refresh-s", "0", ...extra], {
    env,
    stdio: ["ignore", "pipe", "pipe"],
  });
  server.stdout.on("data", (d) => (log += d));
  server.stderr.on("data", (d) => (log += d));
  const exited = new Promise((r) => server.once("exit", r));
  for (let i = 0; i < 600; i++) {
    if (/Ctrl-C \(or/.test(log)) return true;
    if (await Promise.race([exited.then(() => true), sleep(250).then(() => false)])) return false;
  }
  return false;
}
async function stop() {
  if (!server || server.exitCode !== null) return;
  const exited = new Promise((r) => server.once("exit", r));
  try {
    sh("stop");
  } catch {
    server.kill("SIGTERM");
  }
  await Promise.race([exited, sleep(10000)]);
}
const ranCount = (dash, panel) =>
  (log.match(new RegExp(`\\[anofox-serve\\] ran ${dash}#${panel} `, "g")) || []).length;

// ── the client side: what the browser sends ──────────────────────────────
const BASE = `http://127.0.0.1:${PORT}`;
async function panel(base, dashboard, n, vars = {}, { page, supersede, headers = {} } = {}) {
  const h = { "Content-Type": "application/json", ...headers };
  if (supersede) h["X-Anofox-Supersede"] = supersede;
  const body = { dashboard, panel: n, vars };
  if (page) body.page = page;
  const r = await fetch(`${base}/api/panel`, { method: "POST", headers: h, body: JSON.stringify(body) });
  const text = await r.text();
  let rows = null;
  try {
    rows = JSON.parse(text);
  } catch {}
  return { status: r.status, etag: r.headers.get("etag"), rows, text };
}
// Every statement of a dashboard, in order, until the server says 404.
async function allPanels(base, dashboard, vars, opts) {
  const out = [];
  for (let n = 0; n < 100; n++) {
    const r = await panel(base, dashboard, n, vars, opts);
    if (r.status === 404) break;
    out.push(r);
  }
  return out;
}
const findPanel = (panels, pred) => panels.findIndex((p) => Array.isArray(p.rows) && p.rows.length && pred(p.rows));

const SALES = "how-did-our-bakeries-sell";
const FORECAST = "what-will-each-store-sell";
const MINE = "how-are-my-stores-doing";
const TUTORIAL = "tutorial";
const PROGRESS = "progress";
// tutorial.sql's fixed statement order: 0 title, 1 locale, 2 intro, 3 group,
// 4 the ::BUTTONS input, 5 the ::SUBMIT button, 6 endgroup, 7 the ::SHOW_IF
// gate, 8 group, 9 input, 10 ::SUBMIT, 11 endgroup, 12 ::COL, 13 "My progress".
const STEP1_SUBMIT = 5;
const STEP1_GATE = 7;
const STEP2_SUBMIT = 10;
const MY_PROGRESS = 13;

async function submit(base, dashboard, n, id, vars, { headers = {} } = {}) {
  const h = { "Content-Type": "application/json", "X-Anofox-Submit": "1", Origin: base, ...headers };
  const r = await fetch(`${base}/api/submit`, {
    method: "POST",
    headers: h,
    body: JSON.stringify({ dashboard, panel: n, id, vars }),
  });
  const text = await r.text();
  let body = null;
  try {
    body = JSON.parse(text);
  } catch {}
  return { status: r.status, body, text };
}
const newId = () => [...crypto.getRandomValues(new Uint8Array(16))].map((b) => b.toString(16).padStart(2, "0")).join("");

async function main() {
  rmSync(WORK, { recursive: true, force: true });

  console.log("== start (fresh data, cache on, no interval refresh)");
  ok(await start(), "the server starts", `port ${PORT}`);
  if (server.exitCode !== null) throw new Error(log);

  const health = await (await fetch(`${BASE}/healthz`)).json();
  ok(health.epoch === 1 && health.dashboards === 2, "/healthz: epoch 1, two dashboards", JSON.stringify(health));
  const index = await (await fetch(`${BASE}/`)).text();
  ok(index.includes(`/d/${SALES}`) && index.includes(`/d/${FORECAST}`), "/ lists both dashboards");
  ok(!index.includes(MINE), "the per-user dashboard is not served without --auth");
  for (const d of [SALES, FORECAST]) {
    const r = await fetch(`${BASE}/d/${d}`);
    ok(r.status === 200, `GET /d/${d}`, String(r.status));
  }
  const q = await fetch(`${BASE}/query`, { method: "POST", body: "SELECT 42" });
  ok(q.status === 410, "there is no SQL endpoint (/query)", String(q.status));

  console.log("== every panel answers");
  const all = { region: "All regions" };
  const sales = await allPanels(BASE, SALES, all);
  const pagedIdx = sales.findIndex((p) => p.status !== 200);
  ok(sales.length > 10, `${SALES}: ${sales.length} statements`);
  // The ::PAGED table needs a page object; everything else answers 200.
  const pagedProbe = await panel(BASE, SALES, sales.length - 1, all, { page: { limit: 10, offset: 0, count: true } });
  ok(pagedProbe.status === 200, "the ::PAGED table answers a page request", pagedProbe.text.slice(0, 120));
  ok(sales.slice(0, -1).every((p) => p.status === 200), "every other panel is 200",
    pagedIdx >= 0 ? `${pagedIdx}: ${sales[pagedIdx].status} ${sales[pagedIdx].text.slice(0, 120)}` : "");

  const runsIdx = 4; // [title, locale, asof, group, run dropdown, …]
  const runs0 = await panel(BASE, FORECAST, runsIdx);
  ok(runs0.status === 200 && runs0.rows.length === 2 && "run" in runs0.rows[0], "the run dropdown lists 2 runs",
    runs0.text.slice(0, 160));
  const run = runs0.rows[0].run;
  const stores = await panel(BASE, FORECAST, runsIdx + 1);
  const store = stores.rows[0].store;
  const fvars = { run, store };
  const forecast = await allPanels(BASE, FORECAST, fvars);
  ok(forecast.every((p) => p.status === 200), `${FORECAST}: all ${forecast.length} statements are 200`,
    forecast.filter((p) => p.status !== 200).map((p) => p.text.slice(0, 120)).join(" | "));
  const chartIdx = findPanel(forecast, (rows) => rows.some((r) => r.c1 === "Forecast"));
  ok(chartIdx > 0, "the forecast chart has forecast rows from the run's parquet file");

  console.log("== result cache and single-flight (#73)");
  const kpiIdx = findPanel(sales, (rows) => typeof rows[0].c0 === "number" && Array.isArray(rows[0].c3));
  const north = { region: "North" };
  const before = ranCount(SALES, kpiIdx);
  const burst = await Promise.all(Array.from({ length: 8 }, () => panel(BASE, SALES, kpiIdx, north)));
  await sleep(100);
  ok(burst.every((r) => r.status === 200), "8 identical concurrent requests: all 200");
  ok(ranCount(SALES, kpiIdx) - before === 1, "… ran the query once", `ran ${ranCount(SALES, kpiIdx) - before}×`);
  const again = await panel(BASE, SALES, kpiIdx, north);
  ok(ranCount(SALES, kpiIdx) - before === 1 && again.etag === burst[0].etag, "a repeat is a cache hit (same ETag)",
    again.etag || "");
  const other = await panel(BASE, SALES, kpiIdx, { region: "South" });
  ok(ranCount(SALES, kpiIdx) - before === 2 && other.etag !== again.etag, "another value is another entry");

  console.log("== superseded requests are cancelled (#73)");
  const viewer = [...crypto.getRandomValues(new Uint8Array(16))].map((b) => b.toString(16).padStart(2, "0")).join("");
  const paged = sales.length - 1;
  const ticket = `${viewer}/${paged}/rows`;
  const scroll = await Promise.all(
    Array.from({ length: 16 }, async (_, i) => {
      await sleep(i * 2);
      return panel(BASE, SALES, paged, all, {
        page: { limit: 50, offset: 50 * i, sort: "Revenue", desc: i % 2 === 0, filter: "o" },
        supersede: ticket,
      });
    }),
  );
  const codes = scroll.map((r) => r.status);
  ok(codes.at(-1) === 200, "fast paging: the newest request is answered", codes.join(" "));
  ok(codes.slice(0, -1).every((c) => c === 200 || c === 409) && codes.includes(409),
    "… older ones are cancelled (409)", `${codes.filter((c) => c === 409).length} of ${codes.length - 1}`);
  const bad = await panel(BASE, SALES, paged, all, { page: { limit: 10 }, supersede: "not-a-ticket" });
  ok(bad.status === 400, "a malformed supersede header is 400", String(bad.status));

  console.log("== a live data folder, run picked by a variable (#79)");
  const out = gen("run", "trend");
  const newRun = out.match(/run (\S+) →/)[1];
  const fresh = await panel(BASE, FORECAST, chartIdx, { run: newRun, store });
  ok(fresh.status === 200 && fresh.rows.some((r) => r.c1 === "Forecast"), "a new run is readable at once",
    `${newRun}: ${fresh.status}`);
  const runs1 = await panel(BASE, FORECAST, runsIdx);
  ok(runs1.rows.length === 2, "… while the cached run list still shows 2 (until a refresh)");
  for (const [v, why] of [["../incoming", "a path"], [".hidden", "a hidden name"], ["a/b", "a slash"]]) {
    const r = await panel(BASE, FORECAST, chartIdx, { run: v, store });
    ok(r.status === 400, `run = '${v}' (${why}) is refused before any SQL`, String(r.status));
  }
  const dataVar = await panel(BASE, FORECAST, chartIdx, { run, store, anofox_data: "/" });
  ok(dataVar.status === 400, "a viewer can't set anofox_data", String(dataVar.status));
  const missing = await panel(BASE, FORECAST, chartIdx, { run: "no-such-run", store });
  ok(missing.status >= 400, "a missing run is an error",
    `${missing.status} ${missing.text.split("\n")[0].slice(0, 100)}`);

  console.log("== refresh without a restart (#79)");
  const kpi0 = await panel(BASE, SALES, kpiIdx, all);
  const weeks0 = kpi0.rows[0].c3.length;
  const t0 = Date.now();
  sh("week");
  const h2 = await (await fetch(`${BASE}/healthz`)).json();
  ok(h2.epoch === 2, "serve.sh week: the snapshot is rebuilt (epoch 2)", `${Date.now() - t0} ms`);
  const kpi1 = await panel(BASE, SALES, kpiIdx, all);
  ok(kpi1.rows[0].c3.length === weeks0 + 1 && kpi1.etag !== kpi0.etag, "… the new week is in the KPI's sparkline",
    `${weeks0} → ${kpi1.rows[0].c3.length} weeks`);
  const runs2 = await panel(BASE, FORECAST, runsIdx);
  ok(runs2.rows.length === 3 && runs2.rows.some((r) => r.run === newRun), "… and the run list shows the new run");

  if (WITH_BROWSER)
    await browser(BASE, [
      [SALES, "How did our bakeries sell last week?"],
      [`${FORECAST}?v.run=${newRun}`, "What will each store sell in the next four weeks?"],
    ]);

  console.log("== stop");
  await stop();
  const gone = await fetch(`${BASE}/healthz`).then(() => false, () => true);
  ok(gone && !existsSync(path.join(WORK, "run")), "serve.sh stop: port closed, run folder removed");

  if (WITH_AUTH) await auth();
}

// ── login through the proxy (#78) ─────────────────────────────────────────
async function auth() {
  console.log("== login through an auth proxy, rows per user (#78)");
  const PROXY = `http://127.0.0.1:${PORT + 1}`;
  if (!(await start("--auth"))) {
    ok(false, "the server starts with --auth", log.match(/.*auth.*/i)?.[0] || log.slice(-300));
    return;
  }
  // The proxy needs a moment after the server.
  for (let i = 0; i < 40 && !(await fetch(`${PROXY}/_login`).then(() => true, () => false)); i++) await sleep(250);

  const direct = await panel(BASE, MINE, 0, {}, { headers: { "X-Forwarded-User": "carol", "X-Forwarded-Groups": "admins" } });
  ok(direct.status === 401, "straight to the server with forged identity headers: 401", String(direct.status));
  const anon = await panel(PROXY, MINE, 0);
  ok(anon.status === 401, "through the proxy without signing in: 401", String(anon.status));

  const login = async (user) => {
    const r = await fetch(`${PROXY}/_login`, {
      method: "POST",
      redirect: "manual",
      headers: { "Content-Type": "application/x-www-form-urlencoded", Origin: PROXY },
      body: `user=${user}`,
    });
    return (r.headers.get("set-cookie") || "").split(";")[0];
  };
  const regionsOf = async (cookie, extra = {}) => {
    const panels = await allPanels(PROXY, MINE, {}, { headers: { Cookie: cookie, ...extra } });
    const table = panels.find((p) => Array.isArray(p.rows) && p.rows.length && "Region" in p.rows[0]);
    return { panels, regions: [...new Set((table?.rows || []).map((r) => r.Region))].sort().join(",") };
  };
  const want = { alice: "North", bob: "Midlands,South", carol: "Midlands,North,South" };
  const cookies = {};
  for (const user of Object.keys(want)) {
    cookies[user] = await login(user);
    const { panels, regions } = await regionsOf(cookies[user]);
    ok(panels.every((p) => p.status === 200) && regions === want[user], `${user} sees ${want[user]}`, regions);
  }
  const forged = await regionsOf(cookies.alice, { "X-Forwarded-User": "carol", "X-Forwarded-Groups": "admins" });
  ok(forged.regions === want.alice, "alice sending forged identity headers through the proxy still sees North",
    forged.regions);
  const claim = await panel(PROXY, MINE, 0, { __claim_user: "carol" }, { headers: { Cookie: cookies.alice } });
  ok(claim.status === 400, "a viewer can't set __claim_user", String(claim.status));
  const other = await panel(PROXY, SALES, 0, {}, { headers: { Cookie: cookies.bob } });
  ok(other.status === 200, "the other dashboards work through the proxy", String(other.status));
  const reqLog = path.join(WORK, "run", "requests.jsonl");
  const lines = existsSync(reqLog) ? readFileSync(reqLog, "utf8").trim().split("\n").map((l) => JSON.parse(l)) : [];
  ok(lines.some((l) => l.user === "alice") && lines.some((l) => l.deny),
    "the request log names the user, and the refusals", `${lines.length} lines`);
  ok(!/[0-9a-f]{64}/.test(JSON.stringify(lines)), "… and never the secret");

  await submitState(PROXY, cookies);
  await stop();
}

// ── per-viewer state: ::SUBMIT, viewer_state, trainer scoping (#517) ───────
async function submitState(PROXY_BASE, cookies) {
  console.log("== per-viewer state: ::SUBMIT, viewer_state, trainers (#517)");
  const before = await panel(PROXY_BASE, TUTORIAL, MY_PROGRESS, {}, { headers: { Cookie: cookies.alice } });
  ok(Array.isArray(before.rows) && before.rows.length === 0, "alice's progress starts empty", before.text.slice(0, 120));

  const gate0 = await panel(PROXY_BASE, TUTORIAL, STEP1_GATE, {}, { headers: { Cookie: cookies.alice } });
  ok(gate0.rows?.[0]?.c0 === false, "step 2 is hidden before step 1 is answered", JSON.stringify(gate0.rows));

  const id1 = newId();
  const r1 = await submit(PROXY_BASE, TUTORIAL, STEP1_SUBMIT, id1, { day: "Monday" }, { headers: { Cookie: cookies.alice } });
  ok(r1.status === 201 && r1.body?.seq === 1, "alice submits step 1", JSON.stringify(r1.body || r1.text));
  const retry1 = await submit(PROXY_BASE, TUTORIAL, STEP1_SUBMIT, id1, { day: "Monday" }, { headers: { Cookie: cookies.alice } });
  ok(retry1.status === 201 && retry1.body?.seq === r1.body?.seq, "the same id retried is idempotent, not a second row",
    JSON.stringify(retry1.body));

  const gate1 = await panel(PROXY_BASE, TUTORIAL, STEP1_GATE, {}, { headers: { Cookie: cookies.alice } });
  ok(gate1.rows?.[0]?.c0 === true, "step 2 unlocks once step 1 is recorded", JSON.stringify(gate1.rows));

  const bad = await submit(PROXY_BASE, TUTORIAL, STEP1_SUBMIT, newId(), { day: "Christmas" }, { headers: { Cookie: cookies.alice } });
  ok(bad.status === 400, "an answer outside the real options is refused", String(bad.status));

  const mine = await panel(PROXY_BASE, TUTORIAL, MY_PROGRESS, {}, { headers: { Cookie: cookies.alice } });
  ok(mine.rows?.length === 1 && mine.rows[0]["Your answer"] === "Monday", "alice reads her own answer back",
    JSON.stringify(mine.rows));

  const noAuth = await submit(PROXY_BASE, TUTORIAL, STEP1_SUBMIT, newId(), { day: "Monday" });
  ok(noAuth.status === 401, "submitting without signing in is 401", String(noAuth.status));
  const bypass = await submit(BASE, TUTORIAL, STEP1_SUBMIT, newId(), { day: "Monday" }, {
    headers: { "X-Forwarded-User": "alice", "X-Forwarded-Groups": "" },
  });
  ok(bypass.status === 401, "/api/submit straight to the server, bypassing the proxy, is 401 (forged headers, no shared secret)",
    String(bypass.status));

  const idB = newId();
  const rb = await submit(PROXY_BASE, TUTORIAL, STEP1_SUBMIT, idB, { day: "Thursday" }, { headers: { Cookie: cookies.bob } });
  ok(rb.status === 201, "bob submits his own step 1 answer", JSON.stringify(rb.body || rb.text));

  // progress.sql's own SQL never filters by who is asking (see the file) —
  // this is entirely the server deciding what reaches each connection.
  const bobView = await panel(PROXY_BASE, PROGRESS, 7, {}, { headers: { Cookie: cookies.bob } });
  const bobNames = (bobView.rows || []).map((r) => r["Staff member"]).sort();
  ok(JSON.stringify(bobNames) === JSON.stringify(["bob"]), "bob (not a trainer) sees only his own row on progress.sql",
    JSON.stringify(bobNames));

  const carolView = await panel(PROXY_BASE, PROGRESS, 7, {}, { headers: { Cookie: cookies.carol } });
  const carolNames = (carolView.rows || []).map((r) => r["Staff member"]).sort();
  ok(JSON.stringify(carolNames) === JSON.stringify(["alice", "bob"]),
    "carol (the admins group — the configured trainer) sees every staff member", JSON.stringify(carolNames));

  const auditLog = path.join(WORK, "run", "audit.jsonl");
  const auditLines = existsSync(auditLog)
    ? readFileSync(auditLog, "utf8").trim().split("\n").map((l) => JSON.parse(l))
    : [];
  const submitEvents = auditLines.filter((l) => l.audit === "submit");
  const okEvents = submitEvents.filter((l) => l.outcome === "ok");
  ok(okEvents.length === 2 && okEvents.every((l) => l.user && l.detail),
    "the audit log has an 'ok' submit event per successful submission (never a replayed retry)",
    `${okEvents.length} ok of ${submitEvents.length} total`);
  ok(submitEvents.some((l) => l.outcome !== "ok"), "… and a deny event for the refused 'Christmas' answer",
    JSON.stringify(submitEvents.filter((l) => l.outcome !== "ok")));
  ok(!submitEvents.some((l) => /Monday|Thursday/.test(JSON.stringify(l))), "… and never the answer value",
    JSON.stringify(submitEvents).slice(0, 200));

  const stateFile = path.join(WORK, "run", "state.jsonl");
  ok(existsSync(stateFile), "the state file exists under run/, not inside the data folder or the dashboard folder");
}

// ── optional: the real client in a browser ────────────────────────────────
async function browser(base, pages) {
  console.log("== in a browser");
  let chromium;
  try {
    ({ chromium } = createRequire(import.meta.url)("playwright"));
  } catch {
    ok(false, "Playwright is available (set NODE_PATH)");
    return;
  }
  const b = await chromium.launch();
  const p = await b.newPage({ viewport: { width: 1280, height: 900 } });
  for (const [d, title] of pages) {
    await p.goto(`${base}/d/${d}`);
    await p.getByText(title).first().waitFor({ timeout: 30000 }).catch(() => {});
    await p.waitForLoadState("networkidle");
    const errors = await p.$$eval(".panel-error, .err", (els) => els.map((e) => e.textContent.slice(0, 160)));
    const charts = await p.$$eval("svg", (els) => els.length);
    const shown = await p.getByText(title).count();
    ok(shown > 0 && charts > 0 && errors.length === 0, `${d}: renders`,
      `${charts} charts, ${errors.length} errors ${errors.join(" | ")}`);
  }
  await b.close();
}

try {
  await main();
} catch (e) {
  console.error(e);
  console.error(log.slice(-2000));
  failures++;
} finally {
  await stop();
}
console.log(failures ? `\n${failures} check(s) failed` : "\nall checks passed");
process.exit(failures ? 1 : 0);
