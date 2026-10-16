// Load test for a locked server (anofox_serve_dashboards, issue #23). Drives
// virtual viewers against the reference dashboard examples/load-test/dash/ops.sql
// (KPIs, two charts, a 2M-row ::PAGED table) with the requests the browser
// client really sends (recorded with Playwright): every panel one after the
// other, the table's first page and its column statistics, page / sort / filter
// requests that supersede each other (X-Anofox-Supersede).
//
//   # build: duckext/scripts/build-native.sh --release   (→ OUT=…extension)
//   node scripts/load-test.mjs --ext /tmp/anofox_visualization.duckdb_extension \
//       [--duckdb duckdb] [--port 8845] [--rows 2000000] [--server-options '{"threads":16}'] \
//       [--scenario all|burst|cold|hot|scroll|abuse|smoke] [--viewers N] [--duration 30] \
//       [--think 1000] [--scroll-ms 25] [--refresh-s 5] [--timeout 60] [--cpus 0-3] [--json out.json]
//   node scripts/load-test.mjs --url http://127.0.0.1:8845 [--pid <server pid>] …
//
// With --ext the script starts a fresh server per scenario (DuckDB CLI with
// `-unsigned`, data in a temporary database file, the dashboard dir served
// locked) and reads its log, so it also reports executions, cache hits and
// interrupted queries; --cpus pins it to those CPUs (taskset, Linux). With
// --url it targets a running server that serves
// examples/load-test/dash (latency, status counts; RSS too with --pid).
//
// Scenarios (docs/plans/plan-duckdb2-server.md §5):
//   cold    every viewer picks its own period (the cache can't help)
//   hot     five filter combinations shared by all viewers
//   scroll  viewers only page the table: 10 page requests --scroll-ms apart,
//           each superseding the last (a fast drag in the Scroll view)
//   refresh the "hot" viewers while the server rebuilds its snapshot every
//           --refresh-s seconds (refresh_s): no errors, no latency cliff
//   abuse   20 abusive clients (huge pages, wide filters, every column's
//           stats + distributions, bad requests) next to 10 ordinary viewers
//   burst   viewers arriving at once on a fresh server (one dashboard load each)
//   smoke   all six for a few seconds on 200k rows; exits 1 on any 5xx,
//           network error or timeout, missing 409s or a cold cache (for CI)
import { spawn } from "child_process";
import { readFileSync, writeFileSync, mkdtempSync, rmSync } from "fs";
import { tmpdir, loadavg, cpus } from "os";
import { dirname, join, resolve } from "path";
import { fileURLToPath } from "url";
import { randomBytes } from "crypto";
import http from "http";
import https from "https";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const arg = (k, d) => {
  const i = process.argv.indexOf(k);
  return i > 0 ? process.argv[i + 1] : d;
};
const scenarioArg = arg("--scenario", "all");
const smoke = scenarioArg === "smoke";
const ext = arg("--ext", "");
const duckdb = arg("--duckdb", "duckdb");
const port = Number(arg("--port", "8845"));
const rows = Number(arg("--rows", smoke ? "200000" : "2000000"));
const serverOptions = arg("--server-options", '{"threads": 16}');
const refreshS = Number(arg("--refresh-s", smoke ? "1" : "5"));
const duration = Number(arg("--duration", smoke ? "4" : "30")) * 1000;
const think = Number(arg("--think", smoke ? "200" : "1000"));
const scrollMs = Number(arg("--scroll-ms", smoke ? "5" : "25"));
const cpusArg = arg("--cpus", "");
const timeoutMs = Number(arg("--timeout", smoke ? "15" : "60")) * 1000;
const viewersArg = arg("--viewers", "");
const jsonOut = arg("--json", "");
const target = arg("--url", ext ? `http://127.0.0.1:${port}` : "");
if (!target) {
  console.error("usage: node scripts/load-test.mjs --ext <extension> | --url <server> (see the header)");
  process.exit(2);
}

// ---------------------------------------------------------------- the model --
// Plan indices of examples/load-test/dash/ops.sql in the order the client
// requests them (a repeated `SELECT 4::COL` is requested under its first index).
const PANELS = [1, 2, 0, 3, 4, 3, 6, 3, 8, 9, 10, 3, 12, 13];
const PAGED = 14;
const COLS = ["Order", "Day", "Region", "Channel", "Category", "Customer", "Qty", "Price", "Revenue"];
const DAY = 864e5;
const iso = (t) => new Date(t).toISOString().slice(0, 10);
// The variables a ::PERIOD sets (p_from … p_prev_label) for [from, to].
function period(from, to, grain = "week") {
  const f = Date.parse(from), t = Date.parse(to);
  const pt = f - DAY, pf = pt - (t - f);
  return {
    p_from: iso(f), p_to: iso(t), p_prev_from: iso(pf), p_prev_to: iso(pt),
    p_grain: grain, p_compare: "prev", p_label: `${iso(f)} – ${iso(t)}`, p_prev_label: `vs ${iso(pf)} – ${iso(pt)}`,
  };
}
const vars = (region, p) => ({ selected: "", ...p, region });
// L2: five combinations every viewer shares (presets of the dashboard).
const HOT = [
  vars("All", period("2025-10-03", "2025-12-31")),
  vars("EU", period("2025-10-03", "2025-12-31")),
  vars("Americas", period("2025-01-01", "2025-12-31", "month")),
  vars("APAC", period("2025-12-02", "2025-12-31", "day")),
  vars("MEA", period("2024-01-01", "2024-12-31", "month")),
];
const rnd = (n) => Math.floor(Math.random() * n);
const pick = (a) => a[rnd(a.length)];
// L1: a period nobody else asks for.
function coldVars() {
  const from = Date.parse("2024-01-01") + rnd(600) * DAY;
  return vars(pick(["All", "EU", "Americas", "APAC", "MEA"]), period(iso(from), iso(from + (7 + rnd(120)) * DAY)));
}

// ------------------------------------------------------------- the client --
class Stats {
  constructor() {
    this.lat = { panel: [], page: [], stats: [], load: [], probe: [] };
    this.status = {};
    this.requests = 0;
  }
  count(s) {
    this.status[s] = (this.status[s] || 0) + 1;
  }
}
let S = new Stats();
let stopAt = 0;
const live = () => Date.now() < stopAt;
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// node:http, keep-alive connections per viewer like a browser's.
const proto = target.startsWith("https:") ? https : http;
const newAgent = () => new proto.Agent({ keepAlive: true });
const sharedAgent = newAgent();
function request(path, { method = "GET", headers = {}, body, agent = sharedAgent } = {}) {
  return new Promise((resolve, reject) => {
    const data = body === undefined ? undefined : Buffer.from(body);
    if (data) headers["Content-Length"] = data.length;
    const r = proto.request(new URL(path, target), { method, headers, agent }, (res) => {
      const chunks = [];
      res.on("data", (c) => chunks.push(c));
      res.on("end", () => resolve({ status: res.statusCode, text: Buffer.concat(chunks).toString() }));
      res.on("error", reject);
    });
    r.on("error", reject);
    r.setTimeout(timeoutMs, () => r.destroy(Object.assign(new Error("timeout"), { code: "TIMEOUT" })));
    r.end(data);
  });
}

async function post(body, { supersede, kind = "panel", stats = S, agent } = {}) {
  const headers = { "Content-Type": "application/json" };
  if (supersede) headers["X-Anofox-Supersede"] = supersede;
  const t0 = performance.now();
  stats.requests++;
  try {
    const r = await request("/api/panel", { method: "POST", headers, body: JSON.stringify(body), agent });
    const text = r.text;
    const ms = performance.now() - t0;
    stats.count(r.status);
    if (r.status === 200) stats.lat[kind].push(ms);
    else if (r.status >= 500) stats.count(`${r.status}: ${text.slice(0, 120)}`);
    return { status: r.status, text, ms };
  } catch (e) {
    stats.count(`network: ${e.code || e.message}`);
    return { status: 0, ms: performance.now() - t0 };
  }
}

class Viewer {
  constructor(stats = S) {
    this.id = randomBytes(16).toString("hex");
    this.table = 0;
    this.stats = stats;
    this.agent = newAgent();
  }
  req(panel, v, page, kind) {
    const body = { dashboard: "ops", panel, vars: v };
    if (page) body.page = page;
    return post(body, { supersede: page && `${this.id}/${PAGED}/load${this.table}`, kind, stats: this.stats, agent: this.agent });
  }
  // A dashboard (re)load: every panel in turn, then the table's first page and
  // its column statistics — like the client.
  async load(v, first = false) {
    const t0 = performance.now();
    this.table++;
    for (const [i, p] of PANELS.entries()) {
      const pv = first && i === 0 ? { selected: "" } : v;
      if ((await this.req(p, pv)).status !== 200) return;
    }
    if ((await this.req(PAGED, v, { limit: 10, offset: 0 }, "page")).status !== 200) return;
    if ((await this.req(PAGED, v, { stats: COLS }, "stats")).status !== 200) return;
    this.stats.lat.load.push(performance.now() - t0);
  }
  // Fast paging: n page requests `every` ms apart, each superseding the last
  // (the client aborts its own previous fetch; here they all run to the 409).
  async scroll(v, { n = 10, every = 200, offset = 0, sort, desc = false } = {}) {
    const all = [];
    for (let i = 0; i < n; i++) {
      const page = { limit: 10, offset: offset + 10 * i };
      if (sort) Object.assign(page, { sort, desc });
      all.push(this.req(PAGED, v, page, "page"));
      await sleep(every);
    }
    return Promise.all(all);
  }
}

// One ordinary viewer session, looped: open, think, change a filter, think,
// page through the table, sort, filter.
async function browse(pickVars, coldPages) {
  const v = new Viewer();
  let first = true;
  while (live()) {
    let cur = pickVars();
    await v.load(cur, first);
    first = false;
    await sleep(think);
    if (!live()) break;
    cur = pickVars();
    await v.load(cur);
    await sleep(think);
    if (!live()) break;
    await v.scroll(cur, { offset: coldPages ? 10 * rnd(5000) : 0 });
    await v.req(PAGED, cur, { limit: 10, offset: 0, sort: "Revenue", desc: true }, "page");
    await v.req(PAGED, cur, { limit: 10, offset: 0, sort: "Revenue", desc: true, filter: "Toys" }, "page");
    await sleep(think);
  }
}

// L3: only fast paging, from a random place, sorted by a random column.
async function scroller() {
  const v = new Viewer();
  while (live()) {
    const cur = pick(HOT);
    v.table++;
    await v.scroll(cur, { n: 10, every: scrollMs, offset: 10 * rnd(20000), sort: pick(COLS), desc: Math.random() < 0.5 });
    await sleep(think);
  }
}

// Many viewers arriving at once on a fresh server: each opens its own
// keep-alive connection and opens the dashboard. Every connection must be
// served (a connection left waiting for a server thread shows up as a timeout).
async function arrive() {
  await new Viewer().load(coldVars(), true);
}

// L5: an abusive client.
const abuse = [
  (v) => ({ page: { limit: 1_000_000, offset: 0 } }), // over the page limit → 400
  (v) => ({ page: { limit: 10_000, offset: 0, sort: "Customer", desc: true } }), // the largest page
  (v) => ({ page: { limit: 10, offset: 0, filter: "e" } }),
  (v) => ({ page: { stats: COLS } }),
  (v) => ({ page: { profile: COLS } }),
  (v) => ({ page: { distinct: "Customer" } }),
  (v) => ({ vars: { ...v, region: "x".repeat(100_000) } }), // over max_body_bytes → 413
  (v) => ({ panel: 999 }), // 404
];
async function abuser() {
  const stats = new Stats(); // its own numbers: errors here are expected
  abuserStats.push(stats);
  const agent = newAgent();
  while (live()) {
    const v = coldVars();
    const a = pick(abuse)(v);
    await post({ dashboard: "ops", panel: a.panel ?? PAGED, vars: a.vars ?? v, ...(a.page ? { page: a.page } : {}) }, { stats, kind: "page", agent });
  }
}
let abuserStats = [];

// ------------------------------------------------------------- the server --
let server = null;
const log = { ran: 0, superseded: 0, execMs: [] };
async function startServer(cpus = cpusArg, options = serverOptions) {
  if (await request("/healthz").then(() => true, () => false)) throw new Error(`${target} is already in use`);
  const dir = mkdtempSync(join(tmpdir(), "anofox-load-"));
  const setup = readFileSync(join(ROOT, "examples/load-test/setup.sql"), "utf8").replace(/range\(\d+\)/, `range(${rows})`);
  writeFileSync(join(dir, "setup.sql"), setup);
  const q = (s) => `'${s.replace(/'/g, "''")}'`;
  writeFileSync(join(dir, "serve.sql"),
    `LOAD ${q(resolve(ext))};\n.read ${join(dir, "setup.sql")}\n` +
    `SELECT anofox_serve_dashboards(${q(join(ROOT, "examples/load-test/dash"))}, ${port}, ${q(options)});\n`);
  const t0 = Date.now();
  const cmd = [duckdb, "-unsigned", "-init", join(dir, "serve.sql"), join(dir, "data.duckdb")];
  if (cpus) cmd.unshift("taskset", "-c", cpus);
  const p = spawn(cmd[0], cmd.slice(1), { stdio: ["pipe", "pipe", "pipe"] });
  let out = "";
  p.stdout.on("data", (d) => (out += d));
  let buf = "";
  p.stderr.on("data", (d) => {
    buf += d;
    let i;
    while ((i = buf.indexOf("\n")) >= 0) {
      const line = buf.slice(0, i);
      buf = buf.slice(i + 1);
      const m = /\] ran \S+ in (\d+) ms( \(superseded\))?/.exec(line);
      if (m) {
        log.ran++;
        log.execMs.push(+m[1]);
        if (m[2]) log.superseded++;
      } else if (!/\] (GET|POST) /.test(line)) out += line + "\n";
    }
  });
  for (let i = 0; i < 600; i++) {
    if (p.exitCode !== null) break;
    try {
      if (/ serving \d+ locked/.test(out) && (await request("/healthz")).status === 200) {
        server = { p, dir, startMs: Date.now() - t0 };
        return;
      }
    } catch {}
    await sleep(100);
  }
  p.kill();
  rmSync(dir, { recursive: true, force: true });
  throw new Error("server did not start:\n" + out);
}
async function stopServer() {
  if (!server) return;
  const { p, dir } = server;
  server = null;
  if (p.exitCode === null) {
    const exited = new Promise((r) => p.once("exit", r));
    p.kill();
    await Promise.race([exited, sleep(5000)]);
  }
  rmSync(dir, { recursive: true, force: true });
}
const pidArg = arg("--pid", "");
const pid = () => server?.p.pid ?? (pidArg ? Number(pidArg) : 0);
function rssMb() {
  try {
    const m = /VmRSS:\s+(\d+) kB/.exec(readFileSync(`/proc/${pid()}/status`, "utf8"));
    return m ? Math.round(+m[1] / 1024) : null;
  } catch {
    return null;
  }
}

// ---------------------------------------------------------------- running --
const pct = (a, p) => {
  if (!a.length) return null;
  const s = [...a].sort((x, y) => x - y);
  return Math.round(s[Math.min(s.length - 1, Math.floor((p / 100) * s.length))] * 10) / 10;
};
const summary = (a) => ({ n: a.length, p50: pct(a, 50), p95: pct(a, 95), p99: pct(a, 99), max: pct(a, 100) });

async function run(name, viewers, body) {
  // On one CPU a burst of new connections reliably finds a server that leaves
  // some of them waiting for a thread (the regression check of the smoke run).
  const options = name === "refresh" ? JSON.stringify({ ...JSON.parse(serverOptions), refresh_s: refreshS }) : serverOptions;
  if (ext) await startServer(name === "burst" && smoke && !cpusArg && process.platform === "linux" ? "0" : cpusArg, options);
  // Warm-up check: the model still matches the dashboard.
  const probe = await new Viewer(new Stats()).req(PAGED, HOT[0], { limit: 10, offset: 0 }, "page");
  if (probe.status !== 200 || JSON.parse(probe.text).length !== 10)
    throw new Error(`reference dashboard did not answer a page: ${probe.status} ${probe.text?.slice(0, 200)}`);
  S = new Stats();
  abuserStats = [];
  Object.assign(log, { ran: 0, superseded: 0, execMs: [] });
  const rss0 = rssMb();
  let rssPeak = rss0;
  const sampler = setInterval(() => {
    const r = rssMb();
    if (r !== null && r > rssPeak) rssPeak = r;
  }, 200);
  const t0 = Date.now();
  stopAt = t0 + duration;
  await body(viewers);
  const secs = (Date.now() - t0) / 1000;
  clearInterval(sampler);
  await sleep(300); // let the server finish logging
  const ok = S.status[200] || 0;
  const res = {
    scenario: name, viewers, seconds: Math.round(secs * 10) / 10, requests: S.requests,
    throughput: Math.round((ok / secs) * 10) / 10,
    status: S.status,
    latency: Object.fromEntries(Object.entries(S.lat).filter(([, a]) => a.length).map(([k, a]) => [k, summary(a)])),
    rssMb: { start: rss0, peak: rssPeak, end: rssMb() },
    loadavg: loadavg().map((x) => Math.round(x * 10) / 10),
  };
  if (name === "refresh") {
    const h = await request("/healthz").then((r) => JSON.parse(r.text), () => ({}));
    res.refreshes = h.epoch ? h.epoch - 1 : null; // epoch: 1 at start, +1 per refresh
  }
  if (server) {
    res.server = {
      startMs: server.startMs, executions: log.ran, interrupted: log.superseded,
      cacheHitRate: ok && name !== "abuse" ? Math.round((1 - Math.min(ok, log.ran - log.superseded) / ok) * 1000) / 10 : null,
      exec: summary(log.execMs),
    };
    if (name === "scroll") {
      // Superseded requests whose query still ran to the end (nobody wanted it).
      const pageReqs = S.requests;
      res.server.wastedPct = Math.round((Math.max(0, log.ran - log.superseded - ok) / pageReqs) * 1000) / 10;
    }
  }
  if (abuserStats.length) {
    const st = {};
    for (const a of abuserStats) for (const [k, n] of Object.entries(a.status)) st[k] = (st[k] || 0) + n;
    res.abusers = { requests: abuserStats.reduce((s, a) => s + a.requests, 0), status: st };
  }
  await stopServer();
  return res;
}

const scenarios = {
  burst: [viewersArg || (smoke ? 32 : 100), (n) => Promise.all(Array.from({ length: n }, () => arrive()))],
  cold: [viewersArg || (smoke ? 4 : 20), (n) => Promise.all(Array.from({ length: n }, () => browse(coldVars, true)))],
  hot: [viewersArg || (smoke ? 8 : 200), (n) => Promise.all(Array.from({ length: n }, () => browse(() => pick(HOT), false)))],
  refresh: [viewersArg || (smoke ? 8 : 100), (n) => Promise.all(Array.from({ length: n }, () => browse(() => pick(HOT), false)))],
  scroll: [viewersArg || (smoke ? 4 : 100), (n) => Promise.all(Array.from({ length: n }, () => scroller()))],
  abuse: [viewersArg || (smoke ? 4 : 20), (n) => Promise.all([
    ...Array.from({ length: n }, () => abuser()),
    ...Array.from({ length: smoke ? 2 : 10 }, () => browse(() => pick(HOT), false)),
  ])],
};
const which = scenarioArg === "all" || smoke ? Object.keys(scenarios) : scenarioArg.split(",");
const results = [];
for (const sig of ["SIGINT", "SIGTERM"]) process.on(sig, () => stopServer().then(() => process.exit(130)));
try {
  for (const name of which) {
    if (!scenarios[name]) throw new Error(`unknown scenario '${name}'`);
    const [n, body] = scenarios[name];
    process.stderr.write(`${name}: ${n} viewers for ${duration / 1000}s …\n`);
    const r = await run(name, Number(n), body);
    results.push(r);
    console.log(JSON.stringify(r));
  }
} finally {
  await stopServer();
}

// Table.
const f = (x) => (x === null || x === undefined ? "–" : x);
console.log("\n| scenario | viewers | req/s (200) | panel p50 / p95 / p99 ms | page p50 / p95 / p99 ms | dashboard load p50 / p95 ms | 409 | errors | exec | cache hits | RSS start / peak MB |");
console.log("|---|---|---|---|---|---|---|---|---|---|---|");
for (const r of results) {
  const L = r.latency, p = L.panel || {}, g = L.page || {}, d = L.load || {};
  const errs = Object.entries(r.status).filter(([k]) => k !== "200" && k !== "409").reduce((s, [, n]) => s + n, 0);
  console.log(`| ${r.scenario} | ${r.viewers} | ${r.throughput} | ${f(p.p50)} / ${f(p.p95)} / ${f(p.p99)} | ${f(g.p50)} / ${f(g.p95)} / ${f(g.p99)} | ${f(d.p50)} / ${f(d.p95)} | ${r.status[409] || 0} | ${errs} | ${f(r.server?.executions)} | ${r.server?.cacheHitRate != null ? r.server.cacheHitRate + " %" : "–"} | ${f(r.rssMb.start)} / ${f(r.rssMb.peak)} |`);
}
if (jsonOut) writeFileSync(jsonOut, JSON.stringify({ cpu: cpus()[0]?.model, cores: cpus().length, rows, serverOptions, duration, think, results }, null, 2));

// Smoke assertions: ordinary viewers never see a 5xx or a network error, the
// table's superseded requests get 409, the shared filters hit the cache.
if (smoke) {
  const bad = [];
  for (const r of results) {
    const errs = Object.keys(r.status).filter((k) => k !== "200" && k !== "409");
    if (errs.length) bad.push(`${r.scenario}: ${errs.join(", ")}`);
    if (!(r.status[200] > 0)) bad.push(`${r.scenario}: no successful request`);
  }
  const by = Object.fromEntries(results.map((r) => [r.scenario, r]));
  if (!(by.scroll.status[409] > 0)) bad.push("scroll: no superseded request (409)");
  if (by.hot.server && !(by.hot.server.cacheHitRate >= 50)) bad.push(`hot: cache hit rate ${by.hot.server.cacheHitRate} %`);
  if (!(by.refresh.refreshes >= 1)) bad.push(`refresh: ${by.refresh.refreshes} snapshot refreshes`);
  if (by.abuse.abusers && Object.keys(by.abuse.abusers.status).some((k) => /^network/.test(k))) bad.push("abuse: abusers lost connections");
  if (bad.length) {
    console.error("load-test smoke FAILED:\n  " + bad.join("\n  "));
    process.exit(1);
  }
  console.error("load-test smoke ok");
}
