// The builder assistant's `/assist` proxy (#740) in the DuckDB extension:
// `anofox_serve(port, options)` relays a Messages request to a local fake of
// the model API with the key from ANTHROPIC_API_KEY, behind the authoring
// guard; the key never comes back. A locked server (`anofox_serve_dashboards`)
// answers 404 on /assist. A session with enable_external_access = false gets
// no proxy: assist_* options are refused, a key in the environment alone
// leaves it off with a note.
//
//   node scripts/assist-served-e2e.mjs \
//       --ext path/to/anofox_visualization.duckdb_extension [--port 8975] [--duckdb duckdb]
import { spawn } from "child_process";
import http from "http";
import { mkdtempSync, mkdirSync, writeFileSync, rmSync, existsSync, chmodSync } from "fs";
import path from "path";

const arg = (k, d) => {
  const i = process.argv.indexOf(k);
  return i > 0 ? process.argv[i + 1] : d;
};
const ext = arg("--ext", "");
const port = Number(arg("--port", "8975"));
const duckdb = arg("--duckdb", "duckdb");
if (!ext || !existsSync(ext)) {
  console.error("usage: assist-served-e2e.mjs --ext <anofox_visualization.duckdb_extension> (build it with duckext/scripts/build-native.sh)");
  process.exit(2);
}
let bad = 0;
const check = (ok, m) => (ok ? console.log("ok   " + m) : (bad++, console.error("FAIL " + m)));
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const sqlStr = (s) => `'${s.replace(/'/g, "''")}'`;
const KEY = "sk-ant-e2e-740-never-in-a-reply";

const W = mkdtempSync(path.join(path.dirname(path.resolve(ext)), "assist-served-e2e-"));
// anofox_serve opens a browser: a stub `xdg-open` first on PATH keeps it shut.
const bin = path.join(W, "bin");
mkdirSync(bin);
writeFileSync(path.join(bin, "xdg-open"), "#!/bin/sh\nexit 0\n");
chmodSync(path.join(bin, "xdg-open"), 0o755);
const dashDir = path.join(W, "dash");
mkdirSync(dashDir);
writeFileSync(path.join(dashDir, "one.sql"), "-- @title One\nSELECT 1 AS n ::KPI;\n");

// The fake model API: records each request, answers with a fixed message.
const seen = [];
const api = http.createServer((req, res) => {
  let body = "";
  req.on("data", (d) => (body += d));
  req.on("end", () => {
    seen.push({ url: req.url, headers: req.headers, body });
    res.writeHead(200, { "Content-Type": "application/json", "request-id": "req_e2e" });
    res.end(JSON.stringify({ id: "msg_e2e", type: "message", role: "assistant", content: [{ type: "text", text: "hi" }], usage: { input_tokens: 3, output_tokens: 2 } }));
  });
});
await new Promise((r) => api.listen(0, "127.0.0.1", r));
const apiBase = `http://127.0.0.1:${api.address().port}`;

const env = { ...process.env, DATAZOO_DISABLE_TELEMETRY: "1", ANTHROPIC_API_KEY: KEY, PATH: `${bin}:${process.env.PATH}` };
const procs = [];
// One duckdb session running `sql`; resolves with its output so far once
// `until` matches it (or the session ends).
function session(sql, until) {
  const p = spawn(duckdb, ["-unsigned", "-batch", ":memory:"], { stdio: ["pipe", "pipe", "pipe"], env });
  procs.push(p);
  let out = "";
  p.stdout.on("data", (d) => (out += d));
  p.stderr.on("data", (d) => (out += d));
  p.stdin.write(`LOAD ${sqlStr(ext)};\n${sql}\n`);
  return new Promise(async (resolve) => {
    for (let i = 0; i < 100; i++) {
      if (until.test(out) || p.exitCode != null) break;
      await sleep(100);
    }
    resolve(out);
  });
}
const request = JSON.stringify({ model: "claude-sonnet-5", max_tokens: 100, messages: [{ role: "user", content: "PROMPT-TEXT" }] });
const post = (p, pathname, headers = {}, body = request) =>
  fetch(`http://127.0.0.1:${p}${pathname}`, { method: "POST", headers: { "Content-Type": "application/json", ...headers }, body });

try {
  const opts = { assist_base_url: apiBase, assist_models: ["claude-sonnet-5"] };
  const out = await session(`SELECT anofox_serve(${port}, ${sqlStr(JSON.stringify(opts))}) AS r;`, /token=[0-9a-f]+/);
  const tok = (out.match(/token=([0-9a-f]+)/) || [])[1];
  check(!!tok, "anofox_serve started with the proxy");
  check(/assistant proxy on at \/assist \(key from ANTHROPIC_API_KEY/.test(out) && !out.includes(KEY), "its result says so, without the key");

  check((await post(port, "/assist/v1/messages")).status === 401, "no token → 401");
  check((await post(port, "/assist/v1/messages", { "X-Anofox-Token": tok, Origin: "http://evil.example" })).status === 403, "a foreign Origin → 403");
  check(seen.length === 0, "…nothing went upstream");

  const r = await post(port, "/assist/v1/messages", { "X-Anofox-Token": tok, "anthropic-beta": "x", "x-api-key": "sk-browser" });
  const text = await r.text();
  check(r.status === 200 && text.includes("msg_e2e"), `a request is relayed (${r.status})`);
  check(!text.includes(KEY) && ![...r.headers.values()].some((v) => v.includes(KEY)), "the key never comes back");
  const up = seen[0] || { headers: {} };
  check(up.url === "/v1/messages" && up.headers["x-api-key"] === KEY, "upstream got the server's key");
  check(up.headers["anthropic-version"] === "2023-06-01" && !up.headers["anthropic-beta"] && !up.headers["x-anofox-token"] && !up.headers.origin, "…and no client header");

  const haiku = request.replace("claude-sonnet-5", "claude-opus-5-5");
  check((await post(port, "/assist/v1/messages", { "X-Anofox-Token": tok }, haiku)).status === 400, "a model off assist_models → 400");
  check(seen.length === 1, "…not sent upstream");

  // A locked server never has /assist.
  const lport = port + 1;
  await session(`SELECT anofox_serve_dashboards(${sqlStr(dashDir)}, ${lport});`, /serving|dashboards/i);
  let up2 = false;
  for (let i = 0; i < 50 && !up2; i++) {
    try {
      up2 = (await fetch(`http://127.0.0.1:${lport}/healthz`)).ok;
    } catch (_) {}
    if (!up2) await sleep(100);
  }
  check(up2, "the locked server started");
  check((await post(lport, "/assist/v1/messages")).status === 404, "locked: POST /assist → 404");
  check((await fetch(`http://127.0.0.1:${lport}/assist/v1/messages`)).status === 404, "locked: GET /assist → 404");

  // enable_external_access = false: no proxy.
  const off = await session(`SET enable_external_access = false;\nSELECT anofox_serve(${port + 2}, ${sqlStr(JSON.stringify({ assist_base_url: apiBase }))});`, /enable_external_access/);
  check(/assist_\* needs enable_external_access/.test(off), "locked-down session: assist_* options are refused");
  const note = await session(`SET enable_external_access = false;\nSELECT anofox_serve(${port + 3}) AS r;`, /token=[0-9a-f]+/);
  check(/assistant proxy off: enable_external_access is false/.test(note), "…a key in the environment alone leaves it off, with a note");
  const tok3 = (note.match(/token=([0-9a-f]+)/) || [])[1];
  if (tok3) check((await post(port + 3, "/assist/v1/messages", { "X-Anofox-Token": tok3 })).status === 404, "…and /assist is a 404 there");
} catch (e) {
  bad++;
  console.error("FAIL " + (e.stack || e));
} finally {
  for (const p of procs) p.kill();
  api.close();
  rmSync(W, { recursive: true, force: true });
}
console.log(bad ? `${bad} check(s) failed` : "all checks passed");
process.exit(bad ? 1 : 0);
