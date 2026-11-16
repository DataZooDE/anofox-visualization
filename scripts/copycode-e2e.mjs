// #1253: a ::MARKDOWN code block's Copy button, in a real browser (the app,
// DuckDB-Wasm): the button sits in the block, Copy puts exactly the code on
// the clipboard (not the button's own label) and says "Copied"; a code span
// outside a block gets no button.
//
//   NODE_PATH=<dir with playwright>/node_modules node scripts/copycode-e2e.mjs [--url http://127.0.0.1:8947/]
import { createRequire } from "module";

const require = createRequire(import.meta.url);
const { chromium } = require("playwright");
const arg = (k, d) => {
  const i = process.argv.indexOf(k);
  return i > 0 ? process.argv[i + 1] : d;
};
const base = arg("--url", "http://127.0.0.1:8947/");
let bad = 0;
const check = (ok, m, got) => (ok ? console.log("ok   " + m) : (bad++, console.error("FAIL " + m + (got === undefined ? "" : " " + JSON.stringify(got)))));

const prompt = "Build a dashboard that answers:\nwhat share of returns is really defective?";
const sql = `SELECT 'Paste this into Claude Code, then run \`SELECT 1\`:

\`\`\`text
${prompt}
\`\`\`' ::MARKDOWN;`;

const browser = await chromium.launch();
const ctx = await browser.newContext({ viewport: { width: 1200, height: 800 } });
await ctx.grantPermissions(["clipboard-read", "clipboard-write"], { origin: new URL(base).origin });
const page = await ctx.newPage();
const errors = [];
page.on("pageerror", (e) => errors.push(String(e)));
await page.goto(`${base}?embed=1#sql=${Buffer.from(sql, "utf8").toString("base64")}`);
await page.waitForFunction(() => document.querySelector(".md-body pre .md-copy"), null, { timeout: 120000 }).catch(() => {});
const shape = await page.evaluate(() => ({
  buttons: document.querySelectorAll(".md-copy").length,
  inBlock: !!document.querySelector(".md-body pre > .md-copy"),
  label: document.querySelector(".md-copy")?.getAttribute("aria-label"),
}));
check(shape.buttons === 1 && shape.inBlock && shape.label === "Copy code", "one Copy button, in the code block (not on the inline code span)", shape);
await page.locator(".md-copy").click();
await page.waitForTimeout(200);
const clip = await page.evaluate(() => navigator.clipboard.readText());
check(clip === prompt, "Copy puts exactly the code on the clipboard", clip);
check((await page.locator(".md-copy").textContent()) === "Copied", "and says Copied");
check(!errors.length, "no page errors", errors);
await browser.close();
if (bad) {
  console.error(`copycode-e2e: ${bad} failed`);
  process.exit(1);
}
console.log("copycode-e2e: all checks passed");
