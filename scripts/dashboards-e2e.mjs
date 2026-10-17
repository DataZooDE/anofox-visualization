// "My dashboards" in a real browser (sidebar plan, PR 2): save → move to a
// group → rename → duplicate → delete / undo → reload, through the ⋯ menus by
// mouse and keyboard; Save asks before replacing another dashboard; the
// sidebar name and the page title (::DASHBOARD_TITLE) stay one name; groups
// collapse, reorder (Alt+↑/↓) and rename (the members' `-- @group` lines are
// rewritten); sort by name; a v1 store is migrated with a backup. With
// --width 390 the same flow runs in the phone drawer (Esc in a menu closes
// the menu, not the drawer). --shots DIR saves screenshots.
//
//   python3 serve_nocache.py 8791 &
//   NODE_PATH=<dir with playwright>/node_modules node scripts/dashboards-e2e.mjs \
//       [--url http://127.0.0.1:8791/] [--width 390] [--theme dark|light] [--shots DIR]
//       [--axe axe.min.js]   (axe on the sidebar and an open ⋯ menu: no serious violation)
import { createRequire } from "module";
import { mkdirSync, existsSync } from "fs";

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

const browser = await chromium.launch();
const ctx = await browser.newContext({ viewport: { width, height: phone ? 844 : 1000 }, reducedMotion: "reduce", colorScheme: theme === "dark" ? "dark" : "light" });
const page = await ctx.newPage();
const errs = [];
page.on("pageerror", (e) => errs.push(String(e)));
// confirm(): answer with the next queued reply (default: accept), and record it
const dialogs = [];
const replies = [];
page.on("dialog", async (d) => {
  dialogs.push(d.message());
  const yes = replies.length ? replies.shift() : true;
  if (d.type() === "prompt") await (yes === false ? d.dismiss() : d.accept(typeof yes === "string" ? yes : ""));
  else await (yes ? d.accept() : d.dismiss());
});
const q = theme ? `&theme=${theme}` : "";
const ran = () => page.waitForFunction(() => document.querySelector(".dash .panel") && !document.body.classList.contains("loading"), null, { timeout: 120000 });
const store = () => page.evaluate(() => JSON.parse(localStorage.getItem("dp_dashboards") || "null"));
const names = () => page.$$eval("#side-nav .side-row .s-name", (l) => l.map((e) => e.textContent));
const item = async (n) => {
  const s = await store();
  const id = Object.keys(s.items).find((k) => s.items[k].name === n);
  return id ? { id, ...s.items[id] } : null;
};
const focusId = () => page.evaluate(() => document.activeElement?.id || document.activeElement?.className || "");
const drawer = async (open) => {
  if (!phone) return;
  const isOpen = await page.evaluate(() => document.body.classList.contains("side-open"));
  if (isOpen !== open) {
    // ☰ opens it; a tap outside (the scrim) closes it
    if (open) await page.click("#side-toggle");
    else await page.mouse.click(width - 20, 420);
    await page.waitForTimeout(250);
  }
};
const shot = async (name) => shots && page.screenshot({ path: `${shots}/${tag}-${name}.png` });
// open the ⋯ menu of dashboard `n` and pick `label` (keyboard on phones)
async function menu(n, label) {
  await drawer(true);
  const d = await item(n);
  const more = `#side-more-${d.id}`;
  if (phone) {
    await page.focus(more);
    await page.keyboard.press("Enter");
  } else {
    await page.hover(`#side-my-${d.id}`);
    await page.click(more);
  }
  await page.waitForSelector(".side-menu");
  if (phone) {
    const labels = await page.$$eval(".side-menu .side-mi", (l) => l.map((b) => b.textContent));
    const at = labels.indexOf(label);
    const from = await page.evaluate(() => [...document.querySelectorAll(".side-menu .side-mi")].indexOf(document.activeElement));
    for (let i = from; i < at; i++) await page.keyboard.press("ArrowDown");
    await page.keyboard.press("Enter");
  } else await page.click(`.side-menu .side-mi:text-is("${label}")`);
}
async function pickInMenu(label) {
  await page.waitForSelector(".side-menu");
  await page.click(`.side-menu .side-mi:text-is("${label}")`);
}

// ── 1. Save an example under a new name: the page title follows ──
await page.goto(`${base}?mode=edit&dashboard=who-should-sales-call-first${q}`);
await ran();
await page.fill("#dash-name", "Leads this week");
await page.press("#dash-name", "Tab"); // change → ::DASHBOARD_TITLE follows
await page.waitForFunction(() => document.querySelector(".dash .dp-title")?.dataset.title === "Leads this week", null, { timeout: 60000 });
check(true, "renaming in the toolbar renames the page title");
await page.click("#save");
let leads = await item("Leads this week");
check(leads && /'Leads this week'::DASHBOARD_TITLE/.test(leads.sql), "saved, with the page title in its SQL");
check(/^-- @group Sales & customers$/m.test(leads?.sql || ""), "an example's @group comes along");
check((await store()).v === 2, "the store is v2");

// ── 2. A second one, from scratch ──
await drawer(true);
await page.click("#side-new");
await drawer(false);
await page.fill("#sql", "SELECT 41 + 1 AS n ::METRIC, 'Answer'::LABEL;");
await page.fill("#dash-name", "Scratch");
await page.click("#save");
check((await names()).includes("Scratch"), "Scratch saved (ungrouped)");
await shot("saved");

// ── 3. Move to a group through ⋯ ──
await menu("Scratch", "Move to group…");
await pickInMenu("Sales & customers");
let scratch = await item("Scratch");
check(/^-- @group Sales & customers$/m.test(scratch.sql), "Move to group writes the -- @group line");
check(/^-- @group Sales & customers$/m.test(await page.$eval("#sql", (t) => t.value)), "…in the open editor too");

// ── 4. Rename in place ──
await menu("Scratch", "Rename");
await page.waitForSelector(".side-rename");
check((await focusId()) === "side-rename", "the rename field takes the focus");
await page.fill(".side-rename", "Scratch pad");
await page.keyboard.press("Enter");
scratch = await item("Scratch pad");
check(scratch && scratch.id === "scratch", "renamed; the id stays");
check((await page.inputValue("#dash-name")) === "Scratch pad", "the open dashboard's name follows");
check((await focusId()) === `side-my-${scratch.id}`, "focus back on the renamed entry");
// Esc cancels
await menu("Scratch pad", "Rename");
await page.fill(".side-rename", "Nope");
await page.keyboard.press("Escape");
check(!(await item("Nope")) && !!(await item("Scratch pad")), "Esc cancels a rename");
if (phone) check(await page.evaluate(() => document.body.classList.contains("side-open")), "Esc in the rename field keeps the drawer open");

// ── 5. Duplicate ──
await menu("Leads this week", "Duplicate");
const copy = await item("Leads this week copy");
check(copy && /'Leads this week copy'::DASHBOARD_TITLE/.test(copy.sql), "duplicated, with its own page title");
const order = (await store()).order;
check(order.indexOf(copy.id) === order.indexOf(leads.id) + 1, "the copy sits below the original");
await shot("menu-done");

// ── 6. Save asks before replacing another dashboard ──
await drawer(true);
await page.click(`#side-my-${copy.id}`);
await drawer(false);
await ran();
await page.fill("#dash-name", "Leads this week");
replies.push(false);
const nDialogs = dialogs.length;
await page.click("#save");
check(dialogs.length === nDialogs + 1 && /Replace "Leads this week"\?/.test(dialogs.at(-1)), "Save asks: Replace …?");
check(!!(await item("Leads this week copy")) && (await names()).filter((n) => n === "Leads this week").length === 1, "declined: nothing overwritten");
await page.fill("#dash-name", "Leads this week copy");

// ── 7. Delete with Undo ──
const before = (await store()).order;
await menu("Scratch pad", "Delete");
await page.waitForSelector("#side-toast");
check(!(await item("Scratch pad")), "deleted");
check((await focusId()) === "side-toast-btn", "Undo takes the focus");
await shot("undo-toast");
await page.keyboard.press("Enter");
check(JSON.stringify((await store()).order) === JSON.stringify(before), "Undo puts it back in its place");
check(!(await page.$("#side-toast")), "the toast is gone");
// delete again and let the toast expire (≈8 s once the focus leaves it)
await menu("Leads this week copy", "Delete");
await page.waitForSelector("#side-toast");
await page.waitForTimeout(9000);
check(!!(await page.$("#side-toast")), "the toast waits while its Undo has the focus");
await page.evaluate(() => document.activeElement.blur());
await page.waitForSelector("#side-toast", { state: "detached", timeout: 15000 });
check(!(await item("Leads this week copy")), "deleted for good after the toast");

// ── 8. Groups: a new one, collapse, keyboard reorder, rename rewrites @group ──
replies.push("Ops");
await drawer(true);
await page.click("#side-add-group");
check((await store()).groups.includes("Ops"), "an empty group is kept");
const groupsNow = async () => page.$$eval("#side-nav .side-group", (l) => l.map((g) => g.dataset.group));
check(JSON.stringify(await groupsNow()) === JSON.stringify(["Sales & customers", "Ops"]), "groups in their order");
await page.focus("#side-group-ops");
await page.keyboard.press("Alt+ArrowUp");
check(JSON.stringify(await groupsNow()) === JSON.stringify(["Ops", "Sales & customers"]), "Alt+↑ moves a group up");
check((await focusId()) === "side-group-ops", "…and keeps the focus");
await page.focus("#side-group-sales-and-customers");
await page.keyboard.press("Enter");
check((await page.getAttribute("#side-group-sales-and-customers", "aria-expanded")) === "false", "a group collapses");
check(!(await page.$(`#side-my-${leads.id}`)), "…hiding its dashboards");
await page.keyboard.press("Enter");
replies.push("Sales");
await page.click('.side-group[data-group="Sales & customers"] .g-btn[aria-label^="Rename"]');
leads = await item("Leads this week");
check(/^-- @group Sales$/m.test(leads.sql) && /^-- @group Sales$/m.test((await item("Scratch pad")).sql), "renaming a group rewrites its members' -- @group");
check(!(await store()).groups.includes("Sales & customers"), "the old name is gone");

// ── 9. Sort, reorder by keyboard ──
await page.evaluate(() => document.getElementById("side-my-more").click());
await pickInMenu("Sort: name");
check((await store()).sort === "name", "sort by name is kept");
await page.evaluate(() => document.getElementById("side-my-more").click());
await pickInMenu("Sort: manual (drag)");
const sales = async () => page.$$eval('#side-nav .side-row.in-group .s-name', (l) => l.map((e) => e.textContent));
const s0 = await sales();
await page.focus(`#side-my-${(await item(s0[1])).id}`);
await page.keyboard.press("Alt+ArrowUp");
check(JSON.stringify(await sales()) === JSON.stringify([s0[1], s0[0]]), "Alt+↑ reorders dashboards (manual sort)");
await shot("groups");

// ── 10. The header name edit renames the toolbar name too ──
await drawer(false);
await page.click(`#mode-edit`);
await drawer(true);
await page.click(`#side-my-${leads.id}`);
await drawer(false);
await ran();
await page.click(".dash .dp-title-btn");
await page.fill(".dp-title-input", "Leads, this week");
await page.keyboard.press("Enter");
await page.waitForFunction(() => document.getElementById("dash-name").value === "Leads, this week", null, { timeout: 30000 });
check(true, "the header name edit renames the toolbar name");
await page.click("#save");
check(!!(await item("Leads, this week")) && !(await item("Leads this week")), "Save renames the open dashboard (no copy)");

// ── 11. Reload: everything persisted ──
const want = await names();
await page.goto(`${base}?mode=edit${q}`); // no ?dashboard=: the last one opens
await ran();
check(JSON.stringify(await names()) === JSON.stringify(want), "the list survives a reload");
check((await page.inputValue("#dash-name")) === "Leads, this week", "the last dashboard reopens");

// ── 12. Phone: Esc in a menu closes the menu, not the drawer ──
if (phone) {
  await drawer(true);
  const d = await item("Scratch pad");
  await page.focus(`#side-more-${d.id}`);
  await page.keyboard.press("Enter");
  await page.waitForSelector(".side-menu");
  await page.keyboard.press("Escape");
  check(!(await page.$(".side-menu")) && (await page.evaluate(() => document.body.classList.contains("side-open"))), "Esc closes the menu and keeps the drawer");
  check((await focusId()) === `side-more-${d.id}`, "focus back on ⋯");
  await shot("drawer");
}

if (axe && existsSync(axe)) {
  await drawer(true);
  await page.addScriptTag({ path: axe });
  const d = await item("Scratch pad");
  await page.focus(`#side-more-${d.id}`);
  await page.keyboard.press("Enter");
  await page.waitForSelector(".side-menu");
  for (const sel of ["#sidebar", ".side-menu"]) {
    const res = await page.evaluate(async (s) => window.axe.run(s, { runOnly: { type: "tag", values: ["wcag2a", "wcag2aa"] } }), sel);
    for (const v of res.violations) console.log(`axe ${sel} ${v.impact}: ${v.id} (${v.nodes.length}) ${v.help} — ${v.nodes[0]?.target}`);
    check(!res.violations.some((v) => v.impact === "serious" || v.impact === "critical"), `axe ${sel}: no serious violation (${res.passes.length} rules pass)`);
  }
  await page.keyboard.press("Escape");
}

// ── 13. A v1 store is migrated, with a backup ──
const p2 = await ctx.browser().newPage({ viewport: { width, height: 900 } });
p2.on("pageerror", (e) => errs.push(String(e)));
const v1 = { items: { "Old one": { sql: "SELECT 1 AS n ::METRIC;", group: "Team" }, Loose: { sql: "SELECT 2 AS n ::METRIC;", group: "" } }, groups: ["Team"], collapsed: {} };
await p2.addInitScript((s) => localStorage.getItem("dp_dashboards_v1") || localStorage.setItem("dp_dashboards", s), JSON.stringify(v1));
await p2.goto(base);
await p2.waitForSelector("#side-nav .side-row", { state: "attached", timeout: 60000 });
const migrated = await p2.evaluate(() => [JSON.parse(localStorage.getItem("dp_dashboards")), localStorage.getItem("dp_dashboards_v1")]);
check(migrated[0].v === 2 && /^-- @group Team$/m.test(migrated[0].items["old-one"].sql), "v1 → v2: the group is in the SQL");
check(JSON.stringify(JSON.parse(migrated[1])) === JSON.stringify(v1), "the v1 store is kept as dp_dashboards_v1");
check((await p2.$$eval("#side-nav .side-group", (l) => l.map((g) => g.dataset.group))).join() === "Team", "the migrated group shows");
await p2.close();

check(!errs.length, `no page errors ${errs.join(" | ")}`);
await browser.close();
if (!process.exitCode) console.log(`dashboards-e2e (${tag}): ok`);
