// #1022: the website's chart catalogue (website/scripts/gen-charts.mjs) —
// every chart kind of the role table has a data shape and a picture, every
// minimal panel lints without an error in the dashboard CLI (DASHBOARD, else
// target/debug/dashboard), and the pages generate.
process.env.DATAZOO_DISABLE_TELEMETRY = "1";
import { execFileSync } from "child_process";
import { readdirSync } from "fs";
import path from "path";
import assert from "assert/strict";

const root = path.resolve(import.meta.dirname, "..", "..");
const gen = path.join(root, "website", "scripts", "gen-charts.mjs");
execFileSync(process.execPath, [gen, "--check"], { stdio: "inherit" });
execFileSync(process.execPath, [gen], { stdio: "inherit" });
const pages = readdirSync(path.join(root, "website", "src", "content", "docs", "build", "charts"));
assert.ok(pages.includes("index.mdx") && pages.length > 40, `chart pages: ${pages.length}`);
console.log("website charts: ok");
