// #1236: an operator lists extra DuckDB-Wasm extensions in app-config.json
// (`"extensions": ["my_ext"]`); a dashboard's `LOAD my_ext;` then loads it
// from the app's own localext/<version>/<platform>/ — and only a listed name:
// - listed but missing: the page asks localext/ for it, and the error says
//   where the file was expected;
// - not listed: no localext/ request (DuckDB's own error, as before);
// - listed and there (anofox_statistics, from scripts/fetch-wasm-extensions.sh):
//   `LOAD anofox_statistics;` alone runs, and the next panel uses it.
//
//   scripts/build-wasm-pkg.sh && scripts/fetch-wasm-extensions.sh
//   NODE_PATH=<dir with playwright>/node_modules node scripts/localext-e2e.mjs [--pkg web/pkg] [--port 8796]
import { createRequire } from "module";
import { createServer } from "http";
import { readFileSync, existsSync, readdirSync } from "fs";
import path from "path";
import { fileURLToPath } from "url";

const require = createRequire(import.meta.url);
const { chromium } = require("playwright");
const arg = (k, d) => {
  const i = process.argv.indexOf(k);
  return i > 0 ? process.argv[i + 1] : d;
};
const PKG = path.resolve(arg("--pkg", "web/pkg"));
const PORT = Number(arg("--port", "8796"));
const here = path.dirname(fileURLToPath(import.meta.url));
const webDir = path.join(here, "..", "web");
let fails = 0;
const check = (ok, m) => (ok ? console.log("ok   " + m) : (fails++, console.error("FAIL " + m)));
const b64 = (s) => Buffer.from(s, "utf8").toString("base64");

let config = {};
const MIME = { ".html": "text/html", ".js": "text/javascript", ".mjs": "text/javascript", ".css": "text/css", ".json": "application/json", ".wasm": "application/wasm" };
const server = createServer((req, res) => {
  const u = new URL(req.url, "http://x");
  if (u.pathname === "/app-config.json") {
    res.writeHead(200, { "Content-Type": "application/json", "Cache-Control": "no-store" });
    res.end(JSON.stringify(config));
    return;
  }
  let p = u.pathname.startsWith("/pkg/") ? path.join(PKG, u.pathname.slice(5)) : path.join(webDir, decodeURIComponent(u.pathname));
  if (p.endsWith("/")) p = path.join(p, "index.html");
  try {
    const data = readFileSync(p);
    res.writeHead(200, { "Content-Type": MIME[path.extname(p)] || "application/octet-stream", "Cache-Control": "no-store" });
    res.end(data);
  } catch (_) {
    res.writeHead(404);
    res.end("not found");
  }
});
await new Promise((ok) => server.listen(PORT, "127.0.0.1", ok));
const BASE = `http://127.0.0.1:${PORT}/`;
const browser = await chromium.launch();
const errs = [];

async function run(cfg, sql) {
  config = cfg;
  const page = await browser.newPage({ viewport: { width: 1200, height: 800 } });
  page.on("pageerror", (e) => errs.push(String(e)));
  const asked = [];
  page.on("request", (r) => {
    if (r.url().includes("/localext/")) asked.push(new URL(r.url()).pathname);
  });
  await page.goto(`${BASE}?mode=view#sql=${b64(sql)}`);
  await page.waitForFunction(
    () => /\d+ panels?|error|failed/i.test(document.getElementById("status")?.textContent || "") && !document.body.classList.contains("loading"),
    null,
    { timeout: 120000 }
  );
  await page.waitForTimeout(800);
  // a setup statement's error card (its text, also where view mode keeps it small)
  const text = await page.evaluate(() => [...document.querySelectorAll(".dp-errcard")].map((e) => e.textContent).join("\n") || document.body.innerText);
  const metric = await page.evaluate(() => document.querySelector(".metric-value")?.textContent || "");
  await page.close();
  return { asked, text, metric };
}

{
  const r = await run({ extensions: ["my_ext"] }, "LOAD my_ext;\nSELECT 1 AS x ::METRIC, 'one' ::LABEL;\n");
  check(r.asked.some((p) => /\/localext\/v[\d.]+\/wasm_\w+\/my_ext\.duckdb_extension\.wasm$/.test(p)), `a listed extension is asked of localext/ (${r.asked.join(", ")})`);
  check(/my_ext\.duckdb_extension\.wasm/.test(r.text) && /localext\//.test(r.text) && /put the build/.test(r.text), "its error says where the file was expected");
}
{
  const r = await run({}, "LOAD my_ext;\nSELECT 1 AS x ::METRIC, 'one' ::LABEL;\n");
  check(!r.asked.some((p) => p.includes("my_ext")), `not listed: no localext/ request (${r.asked.join(", ")})`);
}
{
  const r = await run({ extensions: ["my_ext"] }, "LOAD other_ext;\nSELECT 1 AS x ::METRIC, 'one' ::LABEL;\n");
  check(!r.asked.some((p) => p.includes("other_ext")), "another name than the listed one: no localext/ request");
}
const hasStats = existsSync(path.join(webDir, "localext")) && readdirSync(path.join(webDir, "localext"), { recursive: true }).some((f) => String(f).endsWith("anofox_statistics.duckdb_extension.wasm"));
if (hasStats) {
  const r = await run({ extensions: ["anofox_statistics"] }, "LOAD anofox_statistics;\nSELECT 42 AS x ::METRIC, 'loaded' ::LABEL;\n");
  check(r.metric === "42" && !/Couldn't load/.test(r.text), `a listed extension that is there loads, its LOAD statement runs (${r.metric})`);
} else {
  console.log("skip a listed extension that is there (no web/localext — scripts/fetch-wasm-extensions.sh)");
}
check(!errs.length, "no page errors: " + errs.join(" | "));
await browser.close();
server.close();
console.log(`localext-e2e: ${fails} failed`);
process.exit(fails ? 1 : 0);
