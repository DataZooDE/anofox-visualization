// #1022: the website's chart catalogue (website/scripts/gen-charts.mjs) and Gallery (gen-gallery.mjs) —
// every chart kind of the role table has a data shape and a picture, every
// minimal panel lints without an error in the dashboard CLI (DASHBOARD, else
// target/debug/dashboard), the pages generate, and every `?dashboard=<id>` link
// of the site names one of the builder's examples (its id is its -- @title's
// slug, not its file name).
process.env.DATAZOO_DISABLE_TELEMETRY = "1";
import { execFileSync } from "child_process";
import { readdirSync, readFileSync } from "fs";
import path from "path";
import assert from "assert/strict";

const root = path.resolve(import.meta.dirname, "..", "..");
const gen = path.join(root, "website", "scripts", "gen-charts.mjs");
execFileSync(process.execPath, [gen, "--check"], { stdio: "inherit" });
execFileSync(process.execPath, [gen], { stdio: "inherit" });
// the Gallery: every tile lints and has its pictures, the pages generate
const gal = path.join(root, "website", "scripts", "gen-gallery.mjs");
execFileSync(process.execPath, [gal, "--check"], { stdio: "inherit" });
execFileSync(process.execPath, [gal], { stdio: "inherit" });
const pages = readdirSync(path.join(root, "website", "src", "content", "docs", "build", "charts"));
assert.ok(pages.includes("index.mdx") && pages.length > 40, `chart pages: ${pages.length}`);

const { examples } = await import(path.join(root, "website", "scripts", "examples.mjs"));
const ids = new Set(examples.map((e) => e.id));
const docs = path.join(root, "website", "src", "content", "docs");
const bad = [];
for (const f of readdirSync(docs, { recursive: true }).filter((f) => /\.mdx?$/.test(f)))
  for (const [, id] of readFileSync(path.join(docs, f), "utf8").matchAll(/[?&]dashboard=([a-z0-9-]+)/g)) if (!ids.has(id)) bad.push(`${f}: ${id}`);
assert.deepEqual(bad, [], "links to no example");

// The landing page's numbers (website/src/content/docs/index.mdx) follow the code.
const landing = readFileSync(path.join(docs, "index.mdx"), "utf8");
const doc = readFileSync(path.join(root, "docs", "DOCS.md"), "utf8");
const roleRows = doc.slice(doc.indexOf("<!-- roles-table:start -->"), doc.indexOf("<!-- roles-table:end -->")).split("\n").filter((l) => l.startsWith("| `::"));
const kinds = pages.filter((f) => f !== "index.mdx").length;
for (const [n, what] of [[kinds, "chart kinds"], [roleRows.length, "roles"], [examples.length, "live examples"]])
  assert.ok(landing.includes(`<strong>${n}</strong> ${what}`), `index.mdx: "<strong>${n}</strong> ${what}"`);

// The docs' own dashboards (showcase/shots/*.sql) are the SQL their pages show, and lint clean.
const shots = path.join(root, "website", "showcase", "shots");
const firstPage = readFileSync(path.join(docs, "start", "first-dashboard.mdx"), "utf8");
const firstSql = readFileSync(path.join(shots, "first-dashboard.sql"), "utf8");
assert.ok(firstPage.includes("```sql\n" + firstSql), "start/first-dashboard.mdx shows showcase/shots/first-dashboard.sql");
const cli = process.env.DASHBOARD || path.join(root, "target", "debug", "dashboard");
for (const f of readdirSync(shots).filter((f) => f.endsWith(".sql"))) {
  let r;
  try { r = JSON.parse(execFileSync(cli, ["--check", path.join(shots, f), "--json"], { encoding: "utf8" })); }
  catch (e) { r = JSON.parse(e.stdout); }
  assert.deepEqual((r.diagnostics || []).filter((d) => d.severity === "error"), [], f);
}
console.log("website charts: ok");
