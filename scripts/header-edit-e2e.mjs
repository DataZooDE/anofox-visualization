// The builder's header editing (#60) in a real browser: in Edit mode the
// dashboard name is edited in place (Enter / blur saves, Esc cancels, empty
// removes) and the logo is set from a URL or an uploaded file, each as the
// `::DASHBOARD_TITLE` / `::LOGO` statement in the SQL; View mode and the HTML
// export show neither control.
//
//   python3 serve_nocache.py 8791 &
//   NODE_PATH=<dir with playwright>/node_modules node scripts/header-edit-e2e.mjs \
//       [--url http://127.0.0.1:8791/] [--dashboard "A sales cockpit in your brand"] [--theme dark|light]
import { createRequire } from "module";
import { readFileSync } from "fs";

const require = createRequire(import.meta.url);
const { chromium } = require("playwright");
const arg = (k, d) => {
  const i = process.argv.indexOf(k);
  return i > 0 ? process.argv[i + 1] : d;
};
const base = arg("--url", "http://127.0.0.1:8791/");
const dash = arg("--dashboard", "A sales cockpit in your brand");
const theme = arg("--theme", "");
const fail = (m) => {
  console.error("FAIL: " + m);
  process.exitCode = 1;
};
const check = (ok, m) => (ok ? console.log("ok   " + m) : fail(m));

const browser = await chromium.launch();
const page = await browser.newPage({ viewport: { width: 1400, height: 1000 }, reducedMotion: "reduce", acceptDownloads: true });
const errs = [];
page.on("pageerror", (e) => errs.push(String(e)));
await page.goto(`${base}?mode=edit&dashboard=${encodeURIComponent(dash)}${theme ? "&theme=" + theme : ""}`);
await page.waitForSelector(".dash .dp-head", { timeout: 120000 });
const sql = () => page.$eval("#sql", (t) => t.value);
const focused = () => page.evaluate(() => document.activeElement?.className || "");
// A re-run replaces the header: wait for the name to read `text`.
const titleIs = (text) =>
  page.waitForFunction((t) => document.querySelector(".dash .dp-title")?.dataset.title === t, text, { timeout: 60000 });
const sql0 = await sql();
const title0 = await page.$eval(".dash .dp-title", (h) => h.dataset.title);

// Name: focus + Enter edits, Enter saves into the SQL, the focus comes back.
await page.focus(".dash .dp-title-btn");
await page.keyboard.press("Enter");
check(await page.$(".dash .dp-title-input"), "Enter opens the name field");
check((await page.$eval(".dash .dp-title-input", (i) => i.getAttribute("aria-label"))) === "Dashboard name", "the field is labelled");
await page.fill(".dash .dp-title-input", "Q3's cockpit");
await page.keyboard.press("Enter");
await titleIs("Q3's cockpit");
check((await sql()).includes("SELECT 'Q3''s cockpit'::DASHBOARD_TITLE;"), "Enter writes ::DASHBOARD_TITLE");
check(!(await sql()).includes(`'${title0.replace(/'/g, "''")}'::DASHBOARD_TITLE`), "…replacing the old statement");
await page.waitForFunction(() => document.activeElement?.classList.contains("dp-title-btn"));
check(true, "the focus is back on the name");

// Esc cancels.
await page.keyboard.press("Enter");
await page.fill(".dash .dp-title-input", "Not this");
await page.keyboard.press("Escape");
check((await page.$eval(".dash .dp-title", (h) => h.textContent)) === "Q3's cockpit", "Esc keeps the name");
check((await focused()).includes("dp-title-btn"), "Esc returns the focus to the name");

// Click, clear, blur: the statement is removed; "Add a dashboard name" remains.
await page.click(".dash .dp-title-btn");
await page.fill(".dash .dp-title-input", "");
await page.$eval(".dash .dp-title-input", (i) => i.blur());
await titleIs("");
check(!/::DASHBOARD_TITLE;/.test(await sql()), "an empty name removes ::DASHBOARD_TITLE");
check((await page.$eval(".dash .dp-title-btn", (b) => b.textContent)) === "Add a dashboard name", "the empty name offers to add one");

// Logo: a bad URL is refused (no SQL change), an upload is embedded as data:image/.
const logoBtn = ".dash .dp-logo-edit > .dp-hbtn";
await page.click(logoBtn);
check((await page.$eval(logoBtn, (b) => b.getAttribute("aria-expanded"))) === "true", "the logo form opens (aria-expanded)");
const before = await sql();
await page.fill(".dash .dp-logo-form input[type=text]", "javascript:alert(1)");
await page.click(".dash .dp-logo-form button[type=submit]");
check(/allowed image URL/.test(await page.$eval(".dash .dp-logo-err", (e) => e.textContent)), "a javascript: URL is refused");
check((await sql()) === before, "…and the SQL is unchanged");
await page.fill(".dash .dp-logo-form input[type=text]", "");
const big = Buffer.alloc(300 * 1024, 1);
await page.setInputFiles(".dash .dp-logo-form input[type=file]", { name: "big.png", mimeType: "image/png", buffer: big });
await page.click(".dash .dp-logo-form button[type=submit]");
check(/KB/.test(await page.$eval(".dash .dp-logo-err", (e) => e.textContent)), "an image over the size cap is refused");
const png = Buffer.from(
  "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==",
  "base64"
);
await page.setInputFiles(".dash .dp-logo-form input[type=file]", { name: "dot.png", mimeType: "image/png", buffer: png });
await page.click(".dash .dp-logo-form button[type=submit]");
await page.waitForFunction(() => /^data:image\/png;base64,/.test(document.querySelector(".dash .dp-logo")?.getAttribute("src") || ""), null, { timeout: 60000 });
check(/SELECT 'data:image\/png;base64,[^']+'::LOGO;/.test(await sql()), "an upload writes ::LOGO as a data:image/png URL");
check((await sql()).match(/::LOGO;/g).length === 1, "…replacing the old ::LOGO");
await page.waitForFunction((s) => document.activeElement?.matches(s), logoBtn);
check(true, "the focus is back on the logo button");

// The HTML export carries the dashboard, not the editing controls.
const [dl] = await Promise.all([page.waitForEvent("download"), page.click("#dlhtml")]);
const html = readFileSync(await dl.path(), "utf8");
check(!/dp-hedit|dp-title-btn|dp-logo-form/.test(html), "the HTML export has no editing controls");
check(/class="dp-logo"/.test(html), "…but the logo");

// Remove the logo; then View mode shows no controls.
await page.click(logoBtn);
await page.click(".dash .dp-logo-form button:text('Remove logo')");
await page.waitForFunction(() => !document.querySelector(".dash .dp-logo"), null, { timeout: 60000 });
check(!/::LOGO;/.test(await sql()), "Remove logo deletes ::LOGO");
check((await page.$eval(logoBtn, (b) => b.textContent)) === "Add logo", "…and offers to add one");

// Restore the original name and look at View mode.
await page.click(".dash .dp-title-btn");
await page.fill(".dash .dp-title-input", title0);
await page.keyboard.press("Enter");
await titleIs(title0);
await page.click("#mode-view");
check(!(await page.$(".dash .dp-title-btn")), "View mode: the name is plain text");
check((await page.$eval(".dash .dp-title", (h) => h.textContent)) === title0, "…and reads the name");
check(!(await page.$$eval(".dash .dp-hedit", (es) => es.some((e) => e.getClientRects().length))), "View mode: no logo control");
check(!(await page.$eval("body", (b) => b.innerText)).includes("Powered by DataZoo GmbH"), "the DataZoo credit is gone");
// (the name was removed and added again, so it may sit elsewhere in the Look section)
const lines = (t) => t.split("\n").filter((l) => l.trim() && !/::LOGO;$/.test(l)).sort().join("\n");
check(lines(await sql()) === lines(sql0), "the SQL is back to the example (but its logo)");
check(!errs.length, "no page errors " + errs.join(" | "));
await browser.close();
