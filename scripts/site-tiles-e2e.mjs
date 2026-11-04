// #946: `dashboard --site --basemap` writes the tiles a ::MAPTILES map draws
// into the site (tiles/<z>/<x>/<y>) and site.json's `basemap`; the published
// page draws them from there — same origin, nothing from the tile source,
// no /api/tiles (there is no server). A site built without --basemap has no
// tiles and asks for none.
//
//   cargo build --bin dashboard --features auth
//   NODE_PATH=<dir with playwright>/node_modules node scripts/site-tiles-e2e.mjs \
//       [--dashboard target/debug/dashboard] [--app web] [--port 8933]
process.env.DATAZOO_DISABLE_TELEMETRY = "1"; // no usage telemetry from anofox extensions
import { createRequire } from "module";
import { execFileSync } from "child_process";
import { createServer } from "http";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from "fs";
import { tmpdir } from "os";
import path from "path";
import { fileURLToPath } from "url";

const require = createRequire(import.meta.url);
const { chromium } = require("playwright");
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const arg = (k, d) => {
  const i = process.argv.indexOf(k);
  return i > 0 ? process.argv[i + 1] : d;
};
const DASH = arg("--dashboard", path.join(root, "target/debug/dashboard"));
const APP = arg("--app", path.join(root, "web"));
const PORT = Number(arg("--port", "8933"));
if (!existsSync(DASH) || !existsSync(APP)) {
  console.error(`site-tiles-e2e: missing ${!existsSync(DASH) ? DASH : APP}`);
  process.exit(2);
}
let bad = 0;
const check = (ok, m) => (ok ? console.log("ok   " + m) : (bad++, console.error("FAIL " + m)));

const W = mkdtempSync(path.join(tmpdir(), "site-tiles-e2e-"));
const dash = path.join(W, "dash");
mkdirSync(dash);
writeFileSync(
  path.join(dash, "depots.sql"),
  `-- @title Depots
SELECT g ::MAP, v ::BARCHART, 'mercator' ::PROJECTION, 'on' ::MAPTILES
FROM (VALUES ('POINT(10 50)', 1), ('POINT(-70 40)', 2), ('POINT(30 -20)', 3)) t(g, v);
`,
);
writeFileSync(path.join(dash, "plain.sql"), "-- @title Plain\nSELECT 'POINT(10 50)' AS g ::MAP;\n");
// the tile source: every tile up to z 2, in a local directory
const PNG = Buffer.from("89504e470d0a1a0a0000000d49484452000000010000000108060000001f15c4890000000d4944415478da6300010000050001" + "0d0a2db40000000049454e44ae426082", "hex");
const src = path.join(W, "src");
for (let z = 0; z <= 2; z++)
  for (let x = 0; x < 1 << z; x++) {
    mkdirSync(path.join(src, String(z), String(x)), { recursive: true });
    for (let y = 0; y < 1 << z; y++) writeFileSync(path.join(src, String(z), String(x), `${y}.png`), PNG);
  }
const basemap = JSON.stringify({ url: `${src}/{z}/{x}/{y}.png`, attribution: "© Test tiles", max_zoom: 2 });

const build = (out, extra) => execFileSync(DASH, ["--site", dash, out, "--app", APP, ...extra], { stdio: ["ignore", "pipe", "pipe"], env: process.env }).toString();
const out = path.join(W, "out");
build(out, ["--basemap", basemap]);
const site = JSON.parse(readFileSync(path.join(out, "site.json"), "utf8"));
check(site.basemap?.attribution === "© Test tiles" && site.basemap?.max_zoom === 2, `site.json carries the basemap (${JSON.stringify(site.basemap)})`);
const written = existsSync(path.join(out, "tiles")) ? readdirSync(path.join(out, "tiles"), { recursive: true }).filter((f) => /^\d+\/\d+\/\d+$/.test(f)) : [];
check(written.length > 0, `the site holds the map's tiles (${written.length})`);
check(!JSON.stringify(site).includes(src), "site.json doesn't name the tile source");

const plainOut = path.join(W, "plain");
build(plainOut, []);
check(!existsSync(path.join(plainOut, "tiles")) && !JSON.parse(readFileSync(path.join(plainOut, "site.json"), "utf8")).basemap, "without --basemap: no tiles, no basemap");

const MIME = { ".html": "text/html", ".js": "text/javascript", ".mjs": "text/javascript", ".css": "text/css", ".json": "application/json", ".wasm": "application/wasm", ".sql": "text/plain" };
let dir = out;
const server = createServer((req, res) => {
  const u = new URL(req.url, "http://x");
  let rel = decodeURIComponent(u.pathname).replace(/^\//, "");
  if (rel === "" || rel.endsWith("/")) rel += "index.html";
  const p = path.join(dir, rel);
  if (!p.startsWith(dir)) {
    res.writeHead(403);
    return res.end();
  }
  try {
    const data = readFileSync(p);
    res.writeHead(200, { "Content-Type": MIME[path.extname(p)] || "application/octet-stream", "Cache-Control": "no-store" });
    res.end(data);
  } catch (_) {
    res.writeHead(404);
    res.end("not found");
  }
});
await new Promise((r) => server.listen(PORT, "127.0.0.1", r));
const BASE = `http://127.0.0.1:${PORT}/`;

const browser = await chromium.launch();
try {
  const open = async (url) => {
    const ctx = await browser.newContext({ viewport: { width: 1200, height: 900 }, reducedMotion: "reduce" });
    const seen = [];
    const missed = [];
    ctx.on("request", (r) => seen.push(r.url()));
    ctx.on("response", (r) => r.url().includes("/tiles/") && r.status() !== 200 && missed.push(new URL(r.url()).pathname));
    const page = await ctx.newPage();
    const errs = [];
    page.on("pageerror", (e) => errs.push(String(e)));
    await page.goto(url);
    return { page, seen, errs, missed };
  };
  const { page, seen, errs, missed } = await open(BASE + "d/depots/");
  const drawn = await page
    .waitForFunction(() => !!document.querySelector(".panel svg [data-tile]"), null, { timeout: 60000 })
    .then(() => true, () => false);
  check(drawn, "the published page draws the tiles under the map");
  check(await page.evaluate(() => document.querySelector(".panel svg")?.textContent.includes("© Test tiles")), "…with the attribution");
  const tileReqs = seen.filter((u) => new URL(u).pathname.startsWith("/tiles/"));
  check(tileReqs.length > 0, `…fetched from the site's tiles/ (${tileReqs.length})`);
  check(!seen.some((u) => u.includes("/api/tiles")), "…never from /api/tiles");
  await page.waitForLoadState("networkidle");
  check(missed.length === 0, `every tile the page asks for is in the site (missing: ${missed.join(", ")})`);
  const CDN = new Set(["cdn.jsdelivr.net", "fonts.googleapis.com", "fonts.gstatic.com", "extensions.duckdb.org"]);
  const away = seen.filter((u) => {
    const h = new URL(u).hostname;
    return !["127.0.0.1", "localhost"].includes(h) && !CDN.has(h) && !u.startsWith("data:") && !u.startsWith("blob:");
  });
  check(away.length === 0, `no request leaves the site but the page's own CDN files (${away.join(", ")})`);
  check(errs.length === 0, `no page errors (${errs.join(" | ")})`);

  // a site built without --basemap asks for no tiles
  dir = plainOut;
  const p2 = await open(BASE + "d/depots/");
  await p2.page.waitForFunction(() => !!document.querySelector(".panel svg"), null, { timeout: 60000 }).catch(() => {});
  check(!p2.seen.some((u) => u.includes("/tiles/")), "no --basemap: the page asks for no tiles");
} finally {
  await browser.close();
  server.close();
  rmSync(W, { recursive: true, force: true });
}
console.log(`site-tiles-e2e: ${bad} failed`);
process.exit(bad ? 1 : 0);
