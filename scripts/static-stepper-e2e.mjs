// #1248: Step / Play on a static page. `dashboard <file.sql> out.html` (and
// `serve --dashboards`, the same page renderer) used to ship a chart's steps
// (`data-story`: a ::STORY chart, a ::DIAGRAM or a factor graph's message
// schedule) without the script that builds the controls. Renders a ::STORY
// chart to static HTML and checks, in a real browser, that the controls sit
// in the chart's panel, that Next walks the steps with their captions, and
// that a page without steps carries no stepper.
//
//   NODE_PATH=<dir with playwright>/node_modules node scripts/static-stepper-e2e.mjs --dashboard target/debug/dashboard
import { createRequire } from "module";
import { mkdtempSync, readFileSync, writeFileSync } from "fs";
import { execFileSync } from "child_process";
import path from "path";
import os from "os";

const require = createRequire(import.meta.url);
const { chromium } = require("playwright");
const arg = (k, d) => {
  const i = process.argv.indexOf(k);
  return i > 0 ? process.argv[i + 1] : d;
};
const bin = path.resolve(arg("--dashboard", "target/debug/dashboard"));
let bad = 0;
const check = (ok, m, got) => (ok ? console.log("ok   " + m) : (bad++, console.error("FAIL " + m + (got === undefined ? "" : " " + JSON.stringify(got)))));

const dir = mkdtempSync(path.join(os.tmpdir(), "static-stepper-"));
const render = (name, sql) => {
  writeFileSync(path.join(dir, name + ".sql"), sql);
  execFileSync(bin, [name + ".sql", name + ".html"], { cwd: dir, stdio: ["ignore", "ignore", "inherit"] });
  return path.join(dir, name + ".html");
};
const story = render(
  "story",
  `SELECT x ::XAXIS, y ::LINECHART, cap ::STORY, 'Orders' ::TITLE
FROM (VALUES (1, 10, 'Start'), (2, 20, NULL), (3, 15, 'A dip'), (4, 25, 'Recovery')) t(x, y, cap);`
);
const plain = render("plain", `SELECT x ::XAXIS, y ::LINECHART, 'Orders' ::TITLE FROM (VALUES (1, 10), (2, 20)) t(x, y);`);
check(!readFileSync(plain, "utf8").includes("animKit"), "a page without steps carries no stepper");

const browser = await chromium.launch();
const page = await browser.newPage({ viewport: { width: 1200, height: 800 } });
const errors = [];
page.on("pageerror", (e) => errors.push(String(e)));
page.on("console", (m) => m.type() === "error" && errors.push(m.text()));
await page.goto("file://" + story);
await page.waitForSelector(".dp-story", { timeout: 10000 }).catch(() => {});
const state = () =>
  page.evaluate(() => ({
    inPanel: !!document.querySelector(".panel .dp-story"),
    buttons: [...document.querySelectorAll(".dp-story button")].map((b) => b.textContent.trim()),
    pos: document.querySelector(".dp-story-pos")?.textContent || "",
    cap: (document.querySelector(".dp-story-cap")?.textContent || "").trim(),
  }));
const s0 = await state();
check(s0.inPanel && s0.buttons.includes("›") && s0.buttons.some((b) => /Play/.test(b)), "the controls sit in the chart's panel", s0);
check(/Overview/.test(s0.pos), "the stepper starts at its overview", s0.pos);
await page.locator(".dp-story-next").click();
await page.waitForTimeout(200);
const s1 = await state();
check(/Step 1 of 3/.test(s1.pos) && s1.cap === "Start", "Next shows step 1 with its caption", s1);
await page.locator(".dp-story-next").click();
await page.waitForTimeout(200);
const s2 = await state();
check(/Step 2 of 3/.test(s2.pos) && s2.cap === "A dip", "and step 2", s2);
check(!errors.length, "no page errors", errors);
await browser.close();
if (bad) {
  console.error(`static-stepper-e2e: ${bad} failed`);
  process.exit(1);
}
console.log("static-stepper-e2e: all checks passed");
