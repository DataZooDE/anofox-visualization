// The tile basemap's endpoint on a locked server (#760, options.basemap,
// proxy auth): GET /api/tiles/{z}/{x}/{y}?dashboard=<id> serves a tile of
// the installation's source (here a local directory) for a dashboard that
// draws one (::MAPTILES), with nosniff and a private cache header. It needs
// a login, refuses a cross-site request, a dashboard without ::MAPTILES, a
// tile outside the grid or past max_zoom, anything but three plain numbers,
// a file that isn't a PNG/JPEG, and a link; a source with a bad template or
// a cache inside the dashboards folder refuses to start.
//
//   node scripts/tiles-served-e2e.mjs \
//       --ext path/to/anofox_visualization.duckdb_extension [--port 8971] [--duckdb duckdb]
import { spawn } from "child_process";
import { mkdtempSync, mkdirSync, writeFileSync, rmSync, existsSync, symlinkSync } from "fs";
import path from "path";
import crypto from "crypto";

const arg = (k, d) => {
  const i = process.argv.indexOf(k);
  return i > 0 ? process.argv[i + 1] : d;
};
const ext = arg("--ext", "");
const port = Number(arg("--port", "8971"));
const duckdb = arg("--duckdb", "duckdb");
if (!ext || !existsSync(ext)) {
  console.error("usage: tiles-served-e2e.mjs --ext <anofox_visualization.duckdb_extension> (build it with duckext/scripts/build-native.sh)");
  process.exit(2);
}
let bad = 0;
const check = (ok, m) => (ok ? console.log("ok   " + m) : (bad++, console.error("FAIL " + m)));
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const sqlStr = (s) => `'${s.replace(/'/g, "''")}'`;
const env = { ...process.env, DATAZOO_DISABLE_TELEMETRY: "1" };

const W = mkdtempSync(path.join(path.dirname(path.resolve(ext)), "tiles-served-e2e-"));
const dashDir = path.join(W, "dash");
mkdirSync(dashDir, { recursive: true });
writeFileSync(
  path.join(dashDir, "depots.sql"),
  `-- @title Depots
SELECT 'POINT(10 50)' AS g ::MAP, 1 AS v ::BARCHART, 'mercator' ::PROJECTION, 'on' ::MAPTILES;
`,
);
writeFileSync(path.join(dashDir, "plain.sql"), "-- @title Plain\nSELECT 'POINT(10 50)' AS g ::MAP;\n");
// The tile source: a directory template.
const PNG = Buffer.from("89504e470d0a1a0a0000000d49484452000000010000000108060000001f15c4890000000d4944415478da6300010000050001" + "0d0a2db40000000049454e44ae426082", "hex");
const src = path.join(W, "tiles");
mkdirSync(path.join(src, "3", "4"), { recursive: true });
writeFileSync(path.join(src, "3", "4", "2.png"), PNG);
writeFileSync(path.join(src, "3", "4", "3.png"), "<html>not a tile</html>");
symlinkSync(path.join(src, "3", "4", "2.png"), path.join(src, "3", "4", "5.png"));
const sec = crypto.randomBytes(24).toString("hex");
const auth = { mode: "proxy", shared_secret: sec, trusted_proxies: ["127.0.0.1"] };
const basemap = { url: `${src}/{z}/{x}/{y}.png`, attribution: "© Test tiles", max_zoom: 5 };
const opts = { auth, policy: "none", basemap };

function spawnServer(o) {
  const p = spawn(duckdb, ["-unsigned", "-batch", ":memory:"], { stdio: ["pipe", "pipe", "pipe"], env });
  let log = "";
  p.stdout.on("data", (d) => (log += d));
  p.stderr.on("data", (d) => (log += d));
  p.stdin.write(`LOAD ${sqlStr(ext)};\nSELECT anofox_serve_dashboards(${sqlStr(dashDir)}, ${port}, ${sqlStr(JSON.stringify(o))});\n`);
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
// A refused start: the server prints the error and exits (or keeps no port).
async function refused(o, want) {
  const s = spawnServer(o);
  for (let i = 0; i < 60 && !s.log().includes(want); i++) await sleep(100);
  const ok = s.log().includes(want);
  await stop(s);
  return ok;
}
const BASE = `http://127.0.0.1:${port}`;
async function get(p, extra = { "X-Anofox-Proxy-Secret": sec, "X-Forwarded-User": "alice" }) {
  const r = await fetch(BASE + p, { headers: extra });
  const body = Buffer.from(await r.arrayBuffer());
  return { status: r.status, headers: r.headers, body };
}

try {
  const s = await serve();
  const t = await get("/api/tiles/3/4/2?dashboard=depots");
  check(t.status === 200 && t.body.equals(PNG), `a tile is served (${t.status}, ${t.body.length} B)`);
  check(t.headers.get("content-type") === "image/png", `…as image/png (${t.headers.get("content-type")})`);
  check(t.headers.get("x-content-type-options") === "nosniff", "…with nosniff");
  check(/^private/.test(t.headers.get("cache-control") || ""), `…and a private cache header (${t.headers.get("cache-control")})`);
  check((await get("/api/tiles/3/4/2?dashboard=depots", {})).status === 401, "without a login: 401");
  const cross = await get("/api/tiles/3/4/2?dashboard=depots", { "X-Anofox-Proxy-Secret": sec, "X-Forwarded-User": "alice", "Sec-Fetch-Site": "cross-site" });
  check(cross.status === 403, `a cross-site request: 403 (${cross.status})`);
  check((await get("/api/tiles/3/4/2?dashboard=plain")).status === 404, "a dashboard without ::MAPTILES: 404");
  check((await get("/api/tiles/3/4/2?dashboard=nope")).status === 404, "an unknown dashboard: 404");
  check((await get("/api/tiles/3/4/2")).status === 400, "no dashboard: 400");
  check((await get("/api/tiles/3/8/2?dashboard=depots")).status === 404, "outside the grid: 404");
  check((await get("/api/tiles/6/0/0?dashboard=depots")).status === 404, "past max_zoom: 404");
  check((await get("/api/tiles/3/4/1?dashboard=depots")).status === 404, "a tile the source lacks: 404");
  for (const p of ["/api/tiles/3/4/2.png", "/api/tiles/3/4", "/api/tiles/3/4/2/1", "/api/tiles/3/4/%2e%2e", "/api/tiles/3/-4/2"]) {
    const r = await get(p + "?dashboard=depots");
    check(r.status === 400, `${p}: 400 (${r.status})`);
  }
  const html = await get("/api/tiles/3/4/3?dashboard=depots");
  check(html.status === 502 && !html.body.includes("html"), `a file that isn't a tile: 502, not passed on (${html.status})`);
  check((await get("/api/tiles/3/4/5?dashboard=depots")).status === 502, "a link in the source: 502");
  // (DuckDB echoes the test's own call, options and all, as a column header.)
  const tileLines = s.log().split("\n").filter((l) => l.startsWith("anofox_serve: tile"));
  check(tileLines.length === 2 && tileLines.every((l) => !l.includes(src)), `the server logs why a tile failed, without the source path (${tileLines.join(" | ")})`);
  await stop(s);

  check(await refused({ ...opts, basemap: { ...basemap, url: "http://tiles.example/{z}/{x}/{y}.png" } }, "https://"), "an http:// source refuses the start");
  check(await refused({ ...opts, basemap: { ...basemap, url: "https://t.example/{z}/{x}/{y}.png?key={k}" } }, "only {z}, {x} and {y}"), "a template with another placeholder refuses the start");
  check(
    await refused({ ...opts, basemap: { url: "https://t.example/{z}/{x}/{y}.png", attribution: "a", cache_dir: path.join(dashDir, "cache") } }, "outside the dashboards folder"),
    "a cache inside the dashboards folder refuses the start",
  );
  check(await refused({ ...opts, basemap: { url: basemap.url } }, "attribution"), "a source without an attribution refuses the start");
} finally {
  rmSync(W, { recursive: true, force: true });
}
console.log(`tiles-served-e2e: ${bad} failed`);
process.exit(bad ? 1 : 0);
