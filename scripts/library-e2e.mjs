// The sidebar's search, tag chips, Recently opened, "Use as a template",
// Download .sql, Export all (.zip) and Import (.sql / .zip, name clashes) in
// a real browser (sidebar plan, PR 3). The downloaded .zip is checked with
// Python's zipfile; the import reads zips Python writes (stored) and refuses
// a deflated one. --width 390 runs it in the phone drawer (Esc in the search
// clears it before it closes the drawer). --axe checks the sidebar and a
// dialog; --shots DIR saves screenshots.
//
//   python3 serve_nocache.py 8791 &
//   NODE_PATH=<dir with playwright>/node_modules node scripts/library-e2e.mjs \
//       [--url http://127.0.0.1:8791/] [--width 390] [--theme dark|light] [--shots DIR] [--axe axe.min.js]
import { createRequire } from "module";
import { mkdirSync, mkdtempSync, readFileSync, writeFileSync, existsSync, rmSync } from "fs";
import { execFileSync } from "child_process";
import { tmpdir } from "os";
import path from "path";

const require = createRequire(import.meta.url);
const { chromium } = require("playwright");
const arg = (k, d) => {
  const i = process.argv.indexOf(k);
  return i > 0 ? process.argv[i + 1] : d;
};
const base = arg("--url", "http://127.0.0.1:8791/");
const width = Number(arg("--width", "1400"));
const theme = arg("--theme", "");
const shots = arg("--shots", "");
const axe = arg("--axe", "");
if (shots) mkdirSync(shots, { recursive: true });
const phone = width <= 820;
const tag = `${phone ? "phone" : "desktop"}${theme ? "-" + theme : ""}`;
const fail = (m) => {
  console.error("FAIL: " + m);
  process.exitCode = 1;
};
const check = (ok, m) => (ok ? console.log("ok   " + m) : fail(m));
const tmp = mkdtempSync(path.join(tmpdir(), "dp-lib-"));
const py = (code, ...args) => execFileSync("python3", ["-c", code, ...args], { encoding: "utf8" });

const browser = await chromium.launch();
const ctx = await browser.newContext({ viewport: { width, height: phone ? 844 : 1000 }, reducedMotion: "reduce", colorScheme: theme === "dark" ? "dark" : "light", acceptDownloads: true });
const page = await ctx.newPage();
const errs = [];
page.on("pageerror", (e) => errs.push(String(e)));
page.on("dialog", (d) => {
  errs.push(`a native ${d.type()}() dialog: ${d.message()}`);
  d.dismiss();
});
const q = theme ? `&theme=${theme}` : "";
const ran = () => page.waitForFunction(() => document.querySelector(".dash .panel") && !document.body.classList.contains("loading"), null, { timeout: 120000 });
const store = () => page.evaluate(() => JSON.parse(localStorage.getItem("dp_dashboards") || "null"));
const mine = () => page.$$eval('#side-nav .side-row[data-key^="my:"] .s-name', (l) => l.map((e) => e.textContent));
const shownExamples = () => page.$$eval('#side-nav .side-row[data-key^="ex:"]', (l) => l.filter((r) => r.offsetParent).length);
const item = async (n) => {
  const s = await store();
  const id = Object.keys(s.items).find((k) => s.items[k].name === n);
  return id ? { id, ...s.items[id] } : null;
};
const focusId = () => page.evaluate(() => document.activeElement?.id || "");
const shot = async (name) => shots && page.screenshot({ path: `${shots}/${tag}-${name}.png` });
const drawer = async (open) => {
  if (!phone) return;
  const isOpen = await page.evaluate(() => document.body.classList.contains("side-open"));
  if (isOpen === open) return;
  if (open) await page.click("#side-toggle");
  else await page.mouse.click(width - 20, 420);
  await page.waitForTimeout(250);
};
async function menuPick(moreSel, label) {
  await drawer(true);
  await page.focus(moreSel);
  await page.keyboard.press("Enter");
  await page.waitForSelector(".side-menu");
  await page.click(`.side-menu .side-mi:text-is("${label}")`);
}
async function answer(button, { check: tick = false } = {}) {
  const dlg = await page.waitForSelector(".dp-dialog [role=dialog]", { timeout: 10000 });
  const title = await dlg.$eval("h3", (h) => h.textContent);
  const text = await dlg.$eval(".md-note", (p) => p.textContent).catch(() => "");
  if (tick) await page.check(".dp-dialog input[type=checkbox]");
  await page.click(`.dp-dialog button:text-is("${button}")`);
  await page.waitForSelector(".dp-dialog", { state: "detached" });
  return { title, text };
}
async function save(name, sql) {
  await drawer(true);
  await page.click("#side-new");
  await drawer(false);
  await page.fill("#sql", sql);
  await page.fill("#dash-name", name);
  await page.click("#save");
  await page.waitForFunction((n) => Object.values(JSON.parse(localStorage.getItem("dp_dashboards")).items).some((d) => d.name === n), name);
}

// ── 1. Two dashboards of yours, tagged ──
await page.goto(`${base}?mode=edit${q}`);
await ran();
await save("Weekly sales", "-- @group Sales\n-- @tags weekly, kpi\n-- @owner Sales controlling\n-- @certified 2026-10-15\nSELECT 41 + 1 AS n ::METRIC, 'Orders'::LABEL;");
await save("Margin bridge", "-- @tags margin, finance\nSELECT 0.31 AS m ::PERCENT, 'Margin'::LABEL;");
check(JSON.stringify(await mine()) === JSON.stringify(["Margin bridge", "Weekly sales"]), "two saved (ungrouped first)");

// ── 2. Search: every section at once ──
await drawer(true);
const allEx = await page.$$eval('#side-nav .side-row[data-key^="ex:"]', (l) => l.length);
await page.fill("#side-search", "weekly");
check(JSON.stringify(await mine()) === JSON.stringify(["Weekly sales"]) && (await shownExamples()) === 0, "search filters My dashboards (by its tag 'weekly')");
await page.fill("#side-search", "sales");
const exSales = await shownExamples();
check(JSON.stringify(await mine()) === JSON.stringify(["Weekly sales"]) && exSales > 0 && exSales < allEx, `…and the examples, by name / group (${exSales} of ${allEx} shown, areas opened)`);
check(await page.$eval("#side-ex-toggle", (b) => b.disabled && b.getAttribute("aria-expanded") === "true"), "while searching the library is open");
await page.fill("#side-search", "stock forecast");
const n2 = await shownExamples();
check(n2 > 0 && (await mine()).length === 0, `two words narrow further (${n2} examples, none of yours) — "How it works" included`);
await page.fill("#side-search", "zzzz-nothing");
check(!!(await page.$(".side-nomatch")), "nothing matches: says so, with a Clear button");
await page.focus("#side-search");
await page.keyboard.press("Escape");
check((await page.inputValue("#side-search")) === "" && (await mine()).length === 2, "Esc clears the search");
if (phone) check(await page.evaluate(() => document.body.classList.contains("side-open")), "…and keeps the drawer open");
await page.fill("#side-search", "sales");
await shot("search");

// ── 2b. Owner and certification ──
await page.fill("#side-search", "");
check((await page.textContent("#side-my-weekly-sales .s-owner")) === "Sales controlling", "the owner is a small line under the name");
check((await page.textContent("#side-my-weekly-sales .s-cert")).includes("certified"), "✓ marks a certified dashboard (with text for screen readers)");
check(/Certified on 2026-10-15/.test(await page.getAttribute("#side-my-weekly-sales", "title")), "…and the tooltip says when");
await page.click("#side-chip-certified");
check((await page.getAttribute("#side-chip-certified", "aria-pressed")) === "true" && JSON.stringify(await mine()) === JSON.stringify(["Weekly sales"]) && (await shownExamples()) === 0, "the ✓ certified chip keeps only certified dashboards");
await page.click("#side-chip-clear");
await page.fill("#side-search", "controlling");
check(JSON.stringify(await mine()) === JSON.stringify(["Weekly sales"]), "the search finds the owner");
await page.fill("#side-search", "");

// ── 3. Tag chips ──
await page.fill("#side-search", "");
const chips = await page.$$eval("#side-chips .side-chip:not(.side-chip-clear):not(.side-chip-cert)", (l) => l.map((b) => b.textContent));
check(chips.length > 0 && chips.length <= 8, `tag chips: ${chips.join(", ")}`);
await page.fill("#side-search", "finance");
await page.click("#side-chip-margin");
check((await page.getAttribute("#side-chip-margin", "aria-pressed")) === "true", "a chip is a toggle (aria-pressed)");
check(JSON.stringify(await mine()) === JSON.stringify(["Margin bridge"]), "the chip filters by its tag");
await page.click("#side-chip-clear");
check((await page.inputValue("#side-search")) === "" && (await focusId()) === "side-search", "× clears search and chips, focus to the search");
await page.fill("#side-search", "forecast");
await page.click("#side-chip-forecast");
const fc = await shownExamples();
check(fc > 0, `chip 'forecast': ${fc} examples`);
await shot("chips");
await page.click("#side-chip-clear");

// ── 4. Recently opened ──
await drawer(true);
check((await page.getAttribute("#side-ex-toggle", "aria-expanded")) === "false", "with dashboards of your own the library starts closed");
await page.click("#side-ex-toggle");
for (const id of ["why-do-customers-cancel", "who-should-sales-call-first"]) {
  await drawer(true);
  await page.click(`#side-nav .side-row[data-key="ex:${id}"] .side-item`);
  await ran();
}
await drawer(true);
check((await page.getAttribute("#side-recent-toggle", "aria-expanded")) === "false", "Recently opened starts closed");
await page.click("#side-recent-toggle");
const rec = await page.$$eval("#side-recent .side-item", (l) => l.map((b) => b.dataset.key));
check(rec[0] === "ex:who-should-sales-call-first" && rec[1] === "ex:why-do-customers-cancel" && rec.length <= 5, `most recent first: ${rec.join(", ")}`);
await page.reload();
await ran();
await drawer(true);
check((await page.$$eval("#side-recent .side-item", (l) => l.length)) === rec.length, "recents and their open state survive a reload");

// ── 5. Use as a template ──
await menuPick("#side-ex-more-why-do-customers-cancel", "Use as a template");
await ran();
const tpl = await item("Why do customers cancel?");
check(tpl && !/^-- @group/m.test(tpl.sql) && /^-- @tags/m.test(tpl.sql), "Use as a template: a copy of yours, ungrouped, tags kept");
check((await page.inputValue("#dash-name")) === "Why do customers cancel?", "…and it opens");

// ── 6. Download .sql ──
const weekly = await item("Weekly sales");
let dl = page.waitForEvent("download");
await menuPick(`#side-more-${weekly.id}`, "Download .sql");
dl = await dl;
const sqlPath = path.join(tmp, dl.suggestedFilename());
await dl.saveAs(sqlPath);
check(dl.suggestedFilename() === "weekly-sales.sql", "Download .sql: <id>.sql");
check(readFileSync(sqlPath, "utf8").startsWith("-- @title Weekly sales\n-- @group Sales\n-- @tags weekly, kpi\n-- @owner Sales controlling\n-- @certified 2026-10-15\n"), "…with its @title, @group, @tags, @owner and @certified");

// ── 7. Export all ──
dl = page.waitForEvent("download");
await menuPick("#side-my-more", "Export all (3) as .zip");
dl = await dl;
const zipPath = path.join(tmp, "export.zip");
await dl.saveAs(zipPath);
const listing = py("import sys,zipfile; z=zipfile.ZipFile(sys.argv[1]); assert z.testzip() is None; print('|'.join(sorted(z.namelist()))); print(z.read('weekly-sales.sql').decode().splitlines()[0])", zipPath).trim().split("\n");
check(listing[0] === "margin-bridge.sql|weekly-sales.sql|why-do-customers-cancel.sql", `Export all: a valid zip of <id>.sql (${listing[0]})`);
check(listing[1] === "-- @title Weekly sales", "…each with its @title");

// ── 8. Import ──
const one = path.join(tmp, "one.sql");
writeFileSync(one, "-- @title Imported one\n-- @tags imported\nSELECT 7 AS n ::METRIC;");
const mkzip = (out, method, entries) =>
  py(`import sys,json,zipfile; z=zipfile.ZipFile(sys.argv[1],'w',${method}); [z.writestr(n,t) for n,t in json.loads(sys.argv[2])]; z.close()`, out, JSON.stringify(entries));
const team = path.join(tmp, "team.zip");
mkzip(team, "zipfile.ZIP_STORED", [
  ["team/weekly.sql", "-- @title Weekly sales\n-- @group Team\nSELECT 1 AS n ::METRIC;"],
  ["team/fresh.sql", "SELECT 2 AS n ::METRIC;"],
  ["team/notes.txt", "ignored"],
]);
await drawer(true);
await page.setInputFiles("#side-import", [one, team]);
const clash = await answer("Keep both");
check(/“Weekly sales” already exists/.test(clash.title), "a name clash asks: Keep both / Replace / Skip");
await page.waitForFunction(() => /imported/.test(document.getElementById("status")?.textContent || ""));
let names = await mine();
check(["Imported one", "fresh", "Weekly sales (2)"].every((n) => names.includes(n)), `imported: ${names.join(", ")}`);
check((await store()).groups.includes("Team"), "an imported @group becomes one of your groups");
// the same zip again: two clashes, one answer for both
await page.setInputFiles("#side-import", [team]);
const second = await answer("Replace", { check: true });
check(/Weekly sales/.test(second.title), "one dialog for both clashes (Do the same…)");
await page.waitForFunction(() => /replaced/.test(document.getElementById("status")?.textContent || ""));
check((await item("Weekly sales")).sql.includes("@group Team") && (await mine()).filter((n) => n === "fresh").length === 1, "Replace overwrote both");
// a deflated zip is refused, with a reason
const defl = path.join(tmp, "deflated.zip");
mkzip(defl, "zipfile.ZIP_DEFLATED", [["a.sql", "SELECT 1;"]]);
await page.setInputFiles("#side-import", [defl]);
const refused = await answer("OK");
check(/Nothing was imported/.test(refused.title) && /compressed/.test(refused.text), `a deflated zip is refused: ${refused.text}`);
await shot("imported");

// ── 9. Dialog a11y and the sidebar with axe ──
if (axe && existsSync(axe)) {
  await page.addScriptTag({ path: axe });
  await drawer(true);
  await page.fill("#side-search", "sales");
  const res = await page.evaluate(async () => window.axe.run("#sidebar", { runOnly: { type: "tag", values: ["wcag2a", "wcag2aa"] } }));
  for (const v of res.violations) console.log(`axe sidebar ${v.impact}: ${v.id} (${v.nodes.length}) ${v.help} — ${v.nodes[0]?.target}`);
  check(!res.violations.some((v) => v.impact === "serious" || v.impact === "critical"), `axe sidebar while searching: no serious violation (${res.passes.length} pass)`);
  await page.fill("#side-search", "");
  await page.click("#side-add-group");
  await page.waitForSelector(".dp-dialog");
  const d = await page.evaluate(async () => window.axe.run(".dp-dialog", { runOnly: { type: "tag", values: ["wcag2a", "wcag2aa"] } }));
  for (const v of d.violations) console.log(`axe dialog ${v.impact}: ${v.id} (${v.nodes.length}) ${v.help} — ${v.nodes[0]?.target}`);
  check(!d.violations.some((v) => v.impact === "serious" || v.impact === "critical"), `axe dialog: no serious violation (${d.passes.length} pass)`);
  await page.keyboard.press("Tab");
  await page.keyboard.press("Tab");
  await page.keyboard.press("Tab");
  check(await page.evaluate(() => !!document.activeElement.closest(".dp-dialog")), "Tab stays in the dialog");
  await shot("dialog");
  await page.keyboard.press("Escape");
  check((await focusId()) === "side-add-group", "Esc: focus back on + group");
  if (phone) check(await page.evaluate(() => document.body.classList.contains("side-open")), "…and the drawer stays open");
}

check(!errs.length, `no page errors ${errs.join(" | ")}`);
await browser.close();
rmSync(tmp, { recursive: true, force: true });
if (!process.exitCode) console.log(`library-e2e (${tag}): ok`);
