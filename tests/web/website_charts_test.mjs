// #1022: the website's chart catalogue (website/scripts/gen-charts.mjs) —
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
const pages = readdirSync(path.join(root, "website", "src", "content", "docs", "build", "charts"));
assert.ok(pages.includes("index.mdx") && pages.length > 40, `chart pages: ${pages.length}`);

const { examples } = await import(path.join(root, "website", "scripts", "examples.mjs"));
const ids = new Set(examples.map((e) => e.id));
const docs = path.join(root, "website", "src", "content", "docs");
const bad = [];
for (const f of readdirSync(docs, { recursive: true }).filter((f) => /\.mdx?$/.test(f)))
  for (const [, id] of readFileSync(path.join(docs, f), "utf8").matchAll(/[?&]dashboard=([a-z0-9-]+)/g)) if (!ids.has(id)) bad.push(`${f}: ${id}`);
assert.deepEqual(bad, [], "links to no example");
console.log("website charts: ok");
