// #1213 step 3: runtime plugins in the browser builder (DuckDB-Wasm, no
// server). A copy of web/ lists the example plugin in plugins.json; the page
// compiles it with no imports, registers it in the core, and
//   (a) draws a ::HELLO panel through it,
//   (b) draws its contract macro, anofox_plot_hello(t, greeting := …),
//   (c) refuses a plugin that imports anything (the page still works),
//   (d) without the plugin, a dashboard that ::REQUIRES it says so.
//
//   NODE_PATH=<dir with playwright>/node_modules node scripts/plugins-browser-e2e.mjs \
//       --plugin hello_plugin.wasm [--web web] [--vendor web/vendor] [--port 8971]
import { createRequire } from "module";
import { createServer } from "http";
import { cpSync, mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync, existsSync } from "fs";
import path from "path";
import os from "os";

const require = createRequire(import.meta.url);
const { chromium } = require("playwright");
const arg = (k, d) => {
  const i = process.argv.indexOf(k);
  return i > 0 ? process.argv[i + 1] : d;
};
const plugin = arg("--plugin", "");
const web = path.resolve(arg("--web", "web"));
const vendor = path.resolve(arg("--vendor", path.join(web, "vendor")));
const port = Number(arg("--port", "8971"));
if (!plugin || !existsSync(plugin) || !existsSync(path.join(web, "pkg"))) {
  console.error("usage: plugins-browser-e2e.mjs --plugin <hello_plugin.wasm> [--web web] (web/pkg built)");
  process.exit(2);
}
let bad = 0;
const check = (ok, m) => (ok ? console.log("ok   " + m) : (bad++, console.error("FAIL " + m)));
const b64 = (s) => Buffer.from(s, "utf8").toString("base64");

// a copy of web/ with the plugin, and one without it
const W = mkdtempSync(path.join(os.tmpdir(), "plugins-browser-e2e-"));
const site = (name, list) => {
  const d = path.join(W, name);
  cpSync(web, d, { recursive: true });
  // only the plugins this check lists (not web/'s bundled ones, #1218)
  rmSync(path.join(d, "plugins.json"), { force: true });
  rmSync(path.join(d, "modules"), { recursive: true, force: true });
  // a local DuckDB-Wasm, when there is one (else the page loads its own)
  const dw = path.join(vendor, "duckdb-wasm");
  if (!existsSync(path.join(d, "vendor", "duckdb-wasm")) && existsSync(dw)) cpSync(dw, path.join(d, "vendor", "duckdb-wasm"), { recursive: true });
  if (list) writeFileSync(path.join(d, "plugins.json"), JSON.stringify(list));
  return d;
};
const withPlugin = site("with", ["modules/hello/plugin.wasm", "modules/bad/plugin.wasm"]);
mkdirSync(path.join(withPlugin, "modules", "hello"), { recursive: true });
writeFileSync(path.join(withPlugin, "modules", "hello", "plugin.wasm"), readFileSync(plugin));
// (c) a module that imports a function: refused before it runs
mkdirSync(path.join(withPlugin, "modules", "bad"), { recursive: true });
writeFileSync(
  path.join(withPlugin, "modules", "bad", "plugin.wasm"),
  // (module (import "env" "fetch" (func)))
  Buffer.from([0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, 0x01, 0x04, 0x01, 0x60, 0x00, 0x00, 0x02, 0x0d, 0x01, 0x03, 0x65, 0x6e, 0x76, 0x05, 0x66, 0x65, 0x74, 0x63, 0x68, 0x00, 0x00])
);
const without = site("without", null);

const types = { ".js": "text/javascript", ".mjs": "text/javascript", ".wasm": "application/wasm", ".json": "application/json", ".html": "text/html", ".css": "text/css", ".svg": "image/svg+xml" };
const serve = (root, p) =>
  new Promise((ok) => {
    const s = createServer((req, res) => {
      let f = decodeURIComponent(new URL(req.url, "http://x").pathname);
      if (f.endsWith("/")) f += "index.html";
      const file = path.join(root, f);
      if (!file.startsWith(root) || !existsSync(file)) {
        res.writeHead(404);
        return res.end("not found");
      }
      res.writeHead(200, { "Content-Type": types[path.extname(file)] || "application/octet-stream", "Cache-Control": "no-store" });
      res.end(readFileSync(file));
    });
    s.listen(p, "127.0.0.1", () => ok(s));
  });
const s1 = await serve(withPlugin, port);
const s2 = await serve(without, port + 1);

const browser = await chromium.launch();
async function open(base, sql, panels = true) {
  const page = await browser.newPage({ viewport: { width: 1200, height: 900 } });
  const errs = [];
  const warns = [];
  page.on("pageerror", (e) => errs.push(String(e)));
  page.on("console", (m) => {
    if (m.type() === "warning") warns.push(m.text());
    if (process.env.E2E_DEBUG) console.log("console:", m.type(), m.text().slice(0, 300));
  });
  if (process.env.E2E_DEBUG) page.on("pageerror", (e) => console.log("pageerror:", String(e).slice(0, 300)));
  if (process.env.E2E_DEBUG) page.on("response", (r) => r.status() >= 400 && console.log("HTTP", r.status(), r.url()));
  await page.goto(`${base}?embed=1#sql=${b64(sql)}`);
  if (!panels) return { page, errs, warns };
  await page.waitForSelector(".dash .panel", { timeout: 120000 });
  await page.waitForFunction(() => !document.body.classList.contains("loading") && !document.querySelector(".dash .panel.dp-skel, .dash .panel.placeholder"), null, { timeout: 120000 });
  return { page, errs, warns };
}
try {
  const base = `http://127.0.0.1:${port}/`;
  // (a) + (b)
  let { page, errs, warns } = await open(
    base,
    "SELECT 'hello'::REQUIRES;\nSELECT 'Ada' AS who ::HELLO, 'Greeting'::TITLE;\n" +
      "SELECT anofox_plot_hello(t, greeting := 'Servus') AS \"Contract\" FROM (VALUES ('Max')) t(name);\n"
  );
  const text = await page.evaluate(() => document.querySelector(".dash").textContent);
  check(/Hello, Ada!/.test(text), `(a) a ::HELLO panel is drawn by the plugin in the browser (${text.slice(0, 120)})`);
  check(/Servus, Max!/.test(text), "(b) its contract macro is drawn by the plugin");
  check(warns.some((w) => /modules\/bad\/plugin\.wasm: imports env::fetch/.test(w)), `(c) a plugin with an import is refused (${warns.join(" | ").slice(0, 200)})`);
  check(errs.length === 0, `no page errors (${errs.join(" | ")})`);
  await page.close();
  // (d) no plugin: ::REQUIRES says so
  ({ page, errs } = await open(`http://127.0.0.1:${port + 1}/`, "SELECT 'hello'::REQUIRES;\nSELECT 'Ada' AS who ::HELLO;\n", false));
  const said = await page
    .waitForFunction(() => /needs the module 'hello'/.test(document.body.textContent), null, { timeout: 120000 })
    .then(() => true, () => false);
  check(said, "(d) without the plugin the dashboard says it needs the module");
  await page.close();
} finally {
  await browser.close();
  s1.close();
  s2.close();
  rmSync(W, { recursive: true, force: true });
}
console.log(`plugins-browser-e2e: ${bad} failed`);
if (bad) process.exit(1);
