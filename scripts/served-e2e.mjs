// A served folder on the locked server (anofox_serve_dashboards), in a real
// browser (sidebar plan, PR 4): the `/` landing page shows the dashboards as
// cards grouped by `-- @group` (counts; title, description, tags, owner,
// "✓ certified", updated), with options.app as its header and, above 8
// dashboards, a search field, tag chips and "certified only"; options.home
// redirects `/`. The nav on a dashboard page groups its links, and above 8
// dashboards becomes one button with a searchable, grouped list (keyboard:
// Enter opens, the search takes the focus, Esc closes back onto the button).
// A ::VIEW switcher (#214) on its own locked server: axe on the view bar,
// keyboard (ArrowRight moves + activates, focus stays on the tab), and at
// --width 390 no horizontal overflow, the view bar visible and tappable, the
// panel full width.
// #407 parts 2–3 on its own locked server (options.data + path_vars +
// nav_picker): window.__served.pathVars and .navPicker.options (a non-plain
// value dropped); the picker starts on the URL's ?v.model=, its own choice
// updates the URL and a sibling nav link's href (a real href, checked
// without clicking), and clicking that link opens the sibling already on
// the carried-over value — axe on the nav bar + picker, no overflow at
// --width 390.
// Runs five servers (4 and 10 dashboards, one with options.home, one with a
// ::VIEW switcher, one with path_vars + nav_picker), each started and
// stopped by this script.
//
//   NODE_PATH=<dir with playwright>/node_modules node scripts/served-e2e.mjs \
//       --ext path/to/anofox_visualization.duckdb_extension [--port 8930] \
//       [--duckdb duckdb] [--theme dark|light] [--width 390] [--axe axe.min.js] [--shots DIR]
import { createRequire } from "module";
import { spawn } from "child_process";
import { mkdirSync, mkdtempSync, writeFileSync, rmSync, existsSync } from "fs";
import { tmpdir } from "os";
import path from "path";

const require = createRequire(import.meta.url);
const { chromium } = require("playwright");
const arg = (k, d) => {
  const i = process.argv.indexOf(k);
  return i > 0 ? process.argv[i + 1] : d;
};
const ext = arg("--ext", "");
const port0 = Number(arg("--port", "8930"));
const duckdb = arg("--duckdb", "duckdb");
const theme = arg("--theme", "");
const width = Number(arg("--width", "1280"));
const axe = arg("--axe", "");
const shots = arg("--shots", "");
if (!ext || !existsSync(ext)) {
  console.error("usage: served-e2e.mjs --ext <anofox_visualization.duckdb_extension> (build it with duckext/scripts/build-native.sh)");
  process.exit(2);
}
if (!/^anofox_visualization(\..+)?\.duckdb_extension$/.test(path.basename(ext))) {
  console.error(`--ext ${ext}: LOAD takes the entry point from the file name, so it must be named anofox_visualization.duckdb_extension (or anofox_visualization.<x>.duckdb_extension). Rename it, or: ln -s '${ext}' /tmp/anofox_visualization.duckdb_extension`);
  process.exit(2);
}
if (shots) mkdirSync(shots, { recursive: true });
const tag = `${width <= 820 ? "phone" : "desktop"}${theme ? "-" + theme : ""}`;
const fail = (m) => {
  console.error("FAIL: " + m);
  process.exitCode = 1;
};
const check = (ok, m) => (ok ? console.log("ok   " + m) : fail(m));
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const sqlStr = (s) => `'${s.replace(/'/g, "''")}'`;

// A folder of small dashboards: [id, header lines].
function folder(entries) {
  const dir = mkdtempSync(path.join(tmpdir(), "dp-served-"));
  for (const [id, head] of entries) writeFileSync(path.join(dir, `${id}.sql`), `${head}\nSELECT 42 AS n ::METRIC, 'Orders'::LABEL;\n`);
  return dir;
}
// Start the locked server on `dir` (a DuckDB process we own; stopped by PID).
async function serve(dir, port, options = null) {
  const p = spawn(duckdb, ["-unsigned", "-batch", ":memory:"], { stdio: ["pipe", "pipe", "pipe"] });
  let log = "";
  p.stdout.on("data", (d) => (log += d));
  p.stderr.on("data", (d) => (log += d));
  const opts = options ? `, ${sqlStr(JSON.stringify(options))}` : "";
  p.stdin.write(`LOAD ${sqlStr(ext)};\nSELECT anofox_serve_dashboards(${sqlStr(dir)}, ${port}${opts});\n`);
  for (let i = 0; i < 100; i++) {
    try {
      if ((await fetch(`http://127.0.0.1:${port}/healthz`)).ok) return { proc: p, log: () => log };
    } catch (_) {}
    if (p.exitCode != null) break;
    await sleep(100);
  }
  p.kill();
  throw new Error(`the server did not start:\n${log}`);
}
const stop = (s) => {
  s.proc.stdin.end();
  s.proc.kill(); // our own child (pid known), never by name
};

const browser = await chromium.launch();
const errs = [];
// #244: a served (locked) page never has the assistant — it has no SQL
// endpoint for the panel to drive, and app-config.json/window.__anofoxApp's
// `assistant` block never reaches it. Every request on every page this
// script opens is logged here; the assertion is at the end.
const allRequests = [];
async function page(url) {
  const ctx = await browser.newContext({ viewport: { width, height: 900 }, reducedMotion: "reduce", colorScheme: theme === "dark" ? "dark" : "light" });
  const p = await ctx.newPage();
  p.on("pageerror", (e) => errs.push(String(e)));
  p.on("request", (r) => allRequests.push(r.url()));
  // #407 §5 is the first caller whose own `url` already carries a query
  // string (?v.model=…): a literal "?" there instead of "&" would merge
  // into one malformed param (URLSearchParams has no second "?" separator),
  // silently corrupting it — pick the right separator instead.
  await p.goto(url + (theme ? `${url.includes("?") ? "&" : "?"}theme=${theme}` : ""));
  return p;
}
async function axeCheck(p, sel, what) {
  if (!axe || !existsSync(axe)) return;
  await p.addScriptTag({ path: axe });
  const r = await p.evaluate(async (s) => window.axe.run(s, { runOnly: { type: "tag", values: ["wcag2a", "wcag2aa"] } }), sel);
  for (const v of r.violations) console.log(`axe ${what} ${v.impact}: ${v.id} (${v.nodes.length}) ${v.help} — ${v.nodes.map((n) => n.target).join(" ")}`);
  check(!r.violations.some((v) => v.impact === "serious" || v.impact === "critical"), `axe ${what}: no serious violation (${r.passes.length} pass)`);
}

// ── 1. Four dashboards: a grouped nav bar ──
const small = folder([
  ["weekly", "-- @title Weekly sales\n-- @group Sales\n-- @tags kpi, weekly\n-- @owner Sales controlling\n-- @certified 2026-10-15\n-- @description Orders and revenue per store, week by week."],
  ["margin", "-- @title Margin bridge\n-- @group Sales\n-- @owner Finance"],
  ["stock", "-- @title Stock outlook\n-- @group Ops\n-- @certified 2026-02-30"],
  ["start", "-- @title Start here"],
]);
let srv = await serve(small, port0);
try {
  const list = await (await fetch(`http://127.0.0.1:${port0}/`)).text();
  const at = (s) => list.indexOf(s);
  check(at(">Start here<") > 0 && at(">Start here<") < at(">Ops <") && at(">Ops <") < at(">Sales <"), "landing: ungrouped first, then the groups by name");
  check(at(">Sales <") < at(">Weekly sales<") && at(">Sales <") < at(">Margin bridge<"), "landing: cards under their group");
  check(/>Sales <span class="lp-n"[^>]*>2</.test(list), "landing: a count per group");
  check((list.match(/✓ certified/g) || []).length === 1, "landing: one ✓ certified (a bad date is no certification)");
  check(list.includes(">Sales controlling<") && list.includes("<li>kpi</li>") && list.includes(">Orders and revenue per store, week by week.<"), "landing: owner, tags and description");
  check(/Updated <time datetime="\d{4}-\d\d-\d\d">/.test(list), "landing: updated (the file's date)");
  check(list.includes("<h1 class=\"lp-title\">Dashboards</h1>") && list.includes("4 dashboards in 2 groups"), "landing: a header without options.app");
  check(!list.includes('id="lp-q"'), "landing: no filters for 4 dashboards");

  const p = await page(`http://127.0.0.1:${port0}/d/weekly`);
  await p.waitForSelector(".served-nav a", { timeout: 60000 });
  const bar = await p.$$eval(".served-nav a, .served-nav .served-nav-group", (l) => l.map((e) => (e.tagName === "A" ? "a:" + e.textContent.trim() : "g:" + e.textContent)));
  check(JSON.stringify(bar) === JSON.stringify(["a:Start here", "g:Ops", "a:Stock outlook", "g:Sales", "a:Margin bridge", "a:Weekly sales ✓"]), `nav bar grouped: ${bar.join(" | ")}`);
  check((await p.getAttribute(".served-nav a.active", "aria-current")) === "page", "the open dashboard is aria-current");
  check(/Owner: Sales controlling/.test(await p.getAttribute(".served-nav a.active", "title")), "owner in the link's tooltip");
  await p.waitForSelector(".dash .panel", { timeout: 60000 });
  await axeCheck(p, ".served-nav", "nav bar");
  if (shots) await p.screenshot({ path: `${shots}/${tag}-served-bar.png` });
  const lp = await page(`http://127.0.0.1:${port0}/`);
  await axeCheck(lp, "body", "landing (4)");
  if (shots) await lp.screenshot({ path: `${shots}/${tag}-landing-small.png`, fullPage: true });
} finally {
  stop(srv);
}

// ── 2. Ten dashboards: one button, a grouped searchable list ──
const big = folder(
  Array.from({ length: 10 }, (_, i) => [
    `d${i}`,
    `-- @title Report ${i}${i % 3 === 0 ? "\n-- @group Finance" : i % 3 === 1 ? "\n-- @group Ops / Plants" : ""}${i === 4 ? "\n-- @owner Plant team\n-- @certified 2026-10-01" : ""}\n-- @tags ${i % 2 ? "weekly" : "monthly"}\n-- @description What report ${i} answers, in one line.`,
  ])
);
const port1 = port0 + 1;
srv = await serve(big, port1, { app: { name: "Bakery <BI>", tagline: "Sales & stock" } });
try {
  const list = await (await fetch(`http://127.0.0.1:${port1}/`)).text();
  check(list.includes('id="lp-q"') && list.includes(">Plants</h3>") && list.includes('id="lp-cert"'), "landing: filters above 8, `A / B` as a sub-heading");
  check(list.includes("<h1 class=\"lp-title\">Bakery &lt;BI&gt;</h1>") && list.includes("Sales &amp; stock · 10 dashboards"), "landing: options.app is the header (escaped)");
  // the filters, in the browser
  const lp = await page(`http://127.0.0.1:${port1}/`);
  await lp.waitForSelector("#lp-q");
  const shownCards = () => lp.$$eval(".lp-card", (l) => l.filter((c) => !c.hidden).map((c) => c.querySelector(".lp-link").textContent));
  await lp.fill("#lp-q", "report 4");
  check(JSON.stringify(await shownCards()) === JSON.stringify(["Report 4"]), "landing search");
  check(/1 of 10/.test(await lp.textContent("#lp-status")), "…announced (role=status)");
  await lp.click("#lp-clear");
  check((await shownCards()).length === 10 && (await lp.evaluate(() => document.activeElement.id)) === "lp-q", "Clear shows all, focus to the search");
  await lp.click('.lp-chip[data-tag="weekly"]');
  check((await shownCards()).length === 5 && (await lp.getAttribute('.lp-chip[data-tag="weekly"]', "aria-pressed")) === "true", "a tag chip filters (aria-pressed)");
  await lp.click('.lp-chip[data-tag="weekly"]');
  await lp.click("#lp-cert");
  check(JSON.stringify(await shownCards()) === JSON.stringify(["Report 4"]), "certified only");
  check((await lp.$$eval(".lp-group", (l) => l.filter((g) => !g.hidden).length)) === 1, "groups without a match hide");
  await lp.click("#lp-clear");
  await lp.focus("#lp-q");
  await lp.keyboard.press("Tab");
  check(await lp.evaluate(() => document.activeElement.classList.contains("lp-chip")), "Tab reaches the chips");
  await axeCheck(lp, "body", "landing (10)");
  if (shots) await lp.screenshot({ path: `${shots}/${tag}-landing.png`, fullPage: true });
  const p = await page(`http://127.0.0.1:${port1}/d/d4`);
  await p.waitForSelector("#served-nav-pick", { timeout: 60000 });
  check((await p.$$(".served-nav > a")).length === 0, "no row of 10 links: one button");
  check((await p.textContent("#served-nav-pick")).startsWith("Report 4"), "the button names the open dashboard");
  await p.focus("#served-nav-pick");
  await p.keyboard.press("Enter");
  check((await p.getAttribute("#served-nav-pick", "aria-expanded")) === "true", "Enter opens the list");
  check(await p.evaluate(() => document.activeElement.classList.contains("served-nav-q")), "the search takes the focus");
  const heads = await p.$$eval("#served-nav-menu h3", (l) => l.map((h) => h.textContent));
  check(JSON.stringify(heads) === JSON.stringify(["Finance", "Ops / Plants"]), `groups: ${heads.join(", ")}`);
  await p.keyboard.type("plant");
  const shown = await p.$$eval("#served-nav-menu li", (l) => l.filter((li) => !li.hidden).map((li) => li.querySelector("a").textContent.trim()));
  check(shown.length === 3 && shown.every((t) => /Report (1|4|7)/.test(t)), `search "plant": ${shown.join(", ")}`);
  await axeCheck(p, ".served-nav", "nav menu");
  if (shots) await p.screenshot({ path: `${shots}/${tag}-served-menu.png` });
  await p.keyboard.press("Escape");
  check((await p.getAttribute("#served-nav-pick", "aria-expanded")) === "false" && (await p.evaluate(() => document.activeElement.id)) === "served-nav-pick", "Esc closes, focus back on the button");
  await p.click("#served-nav-pick");
  await p.click('#served-nav-menu a[href="/d/d7"]');
  await p.waitForURL(/\/d\/d7$/);
  check(true, "a link in the list opens that dashboard");
} finally {
  stop(srv);
}

// ── 3. options.home: / goes straight to that dashboard ──
const port2 = port0 + 2;
srv = await serve(small, port2, { home: "weekly" });
try {
  const r = await fetch(`http://127.0.0.1:${port2}/`, { redirect: "manual" });
  check(r.status === 302 && r.headers.get("location") === "/d/weekly", `options.home: / → ${r.status} ${r.headers.get("location")}`);
} finally {
  stop(srv);
}

// ── 4. A ::VIEW switcher (#214): axe on the view bar, keyboard (ArrowRight
// moves + activates, aria-selected updates, focus stays on the tab — so the
// document-level arrow-key chart-paging handler never also fires), and (at
// --width 390) no horizontal overflow, the view bar visible and tappable,
// the panel full width ──
const viewsDir = folder([
  [
    "views",
    "-- @title Views\nSELECT 'Bars' AS yoy ::VIEW;\nSELECT i::XAXIS, i::BARCHART FROM range(5) t(i);\nSELECT 'Lines'::VIEW;\nSELECT i::XAXIS, i::LINECHART FROM range(5) t(i);\nSELECT 1::ENDVIEW;",
  ],
]);
const port3 = port0 + 3;
srv = await serve(viewsDir, port3);
try {
  const p = await page(`http://127.0.0.1:${port3}/d/views`);
  await p.waitForSelector(".dp-viewbar [role='tab']", { timeout: 60000 });
  await axeCheck(p, ".dp-views", "::VIEW switcher");
  const tabs = () => p.$$eval(".dp-viewbar [role='tab']", (l) => l.map((b) => b.textContent));
  check(JSON.stringify(await tabs()) === '["Bars","Lines"]', `::VIEW switcher: two alternatives ${JSON.stringify(await tabs())}`);
  await p.focus(".dp-viewbar [role='tab']:first-child");
  await p.keyboard.press("ArrowRight");
  // activateView's refocus() lands on the next animation frame.
  await p.waitForFunction(() => document.activeElement === document.querySelectorAll(".dp-viewbar [role='tab']")[1], null, { timeout: 5000 });
  const st = await p.evaluate(() => {
    const btns = [...document.querySelectorAll(".dp-viewbar [role='tab']")];
    const pane = document.getElementById(btns[1].getAttribute("aria-controls"));
    return { sel: btns.map((b) => b.getAttribute("aria-selected")), focused: document.activeElement === btns[1], shown: pane?.style.display !== "none" };
  });
  check(st.sel[1] === "true" && st.focused && st.shown, `::VIEW switcher: ArrowRight moves + activates, focus stays on the tab: ${JSON.stringify(st)}`);
  if (shots) await p.screenshot({ path: `${shots}/${tag}-view-switcher.png` });
  if (width <= 820) {
    const r = await p.evaluate(() => {
      const bar = document.querySelector(".dp-viewbar");
      const btn = bar?.querySelector('[role="tab"][aria-selected="true"]');
      const pane = btn && document.getElementById(btn.getAttribute("aria-controls"));
      const br = btn?.getBoundingClientRect();
      const pr = pane?.getBoundingClientRect();
      return {
        overflow: document.documentElement.scrollWidth - document.documentElement.clientWidth,
        barVisible: !!bar && getComputedStyle(bar).display !== "none" && bar.getBoundingClientRect().width > 0,
        tapSize: br ? Math.min(br.width, br.height) : 0,
        paneWidth: pr?.width ?? 0,
      };
    });
    console.log(`::VIEW switcher at ${width}px:`, JSON.stringify(r));
    check(r.overflow <= 1, `::VIEW switcher: no horizontal overflow at ${width}px (${r.overflow}px)`);
    check(r.barVisible, `::VIEW switcher: the view bar is visible at ${width}px`);
    check(r.tapSize >= 24, `::VIEW switcher: the tab is tappable at ${width}px (${r.tapSize}px)`);
    check(r.paneWidth >= width * 0.75, `::VIEW switcher: the panel is full width at ${width}px (${r.paneWidth}px)`);
  }
} finally {
  stop(srv);
}

// ── 5. #407 parts 2–3: path_vars carried across the served nav, and a
// nav_picker dropdown that sets one. Two dashboards reading `model` (no
// ::DROPDOWN of their own — the whole point of nav_picker); its SQL is a
// literal VALUES list, one row of which isn't a plain path_vars value (the
// server must drop it, never send it to the page) ──
const navData = mkdtempSync(path.join(tmpdir(), "dp-served-navdata-"));
const navDir = folder([
  ["m1", "-- @title Model one\nSELECT coalesce(getvariable('model'), 'none')::VARCHAR AS m ::TEXT_SMALL;"],
  ["m2", "-- @title Model two\nSELECT coalesce(getvariable('model'), 'none')::VARCHAR AS m ::TEXT_SMALL;"],
]);
const pickerSql = "SELECT * FROM (VALUES ('car-price','Car price'), ('../etc','Bad — dropped'), ('van','Van')) t(value, label)";
const port4 = port0 + 4;
srv = await serve(navDir, port4, { data: navData, path_vars: ["model"], nav_picker: { var: "model", label: "Model", sql: pickerSql } });
try {
  const html = await (await fetch(`http://127.0.0.1:${port4}/d/m1`)).text();
  const served = JSON.parse(html.match(/window\.__served=(\{.*?\});<\/script>/s)[1]);
  check(JSON.stringify(served.pathVars) === '["model"]', `window.__served.pathVars: ${JSON.stringify(served.pathVars)}`);
  // #556: the locked server now sends `navPickers` (an array) — the
  // singular `nav_picker` config option still works, as a one-element list.
  check(
    JSON.stringify(served.navPickers[0].options) === JSON.stringify([{ value: "car-price", label: "Car price" }, { value: "van", label: "Van" }]),
    `window.__served.navPickers[0].options drops the non-plain-name row: ${JSON.stringify(served.navPickers?.[0]?.options)}`
  );

  // Opening with a value already on the URL: the picker starts on it, and
  // the m2 link already carries it (no interaction needed — a real href,
  // not just a click handler, so a middle-click / open-in-new-tab works).
  let p = await page(`http://127.0.0.1:${port4}/d/m1?v.model=car-price`);
  await p.waitForSelector(".served-nav-picker select", { timeout: 60000 });
  // The nav bar (and the picker's initial value) is built before the first
  // run() — wait for that run to finish (dpVars populated from the URL,
  // syncUrlState → refreshServedNavHrefs applied) before reading hrefs.
  await p.waitForFunction(() => document.querySelector(".panel")?.textContent.includes("car-price"), null, { timeout: 60000 });
  check((await p.$eval(".served-nav-picker select", (s) => s.value)) === "car-price", "nav_picker: starts on the URL's ?v.model=");
  check((await p.getAttribute('.served-nav a[data-nav-id="m2"]', "href")) === "/d/m2?v.model=car-price", "nav link already carries ?v.model= on load");
  await axeCheck(p, ".served-nav", "nav bar + picker");
  if (shots) await p.screenshot({ path: `${shots}/${tag}-nav-picker.png` });
  if (width <= 820) {
    const overflow = await p.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth);
    check(overflow <= 1, `nav_picker: no horizontal overflow at ${width}px (${overflow}px)`);
  }

  // Picking a new value: the metric panel re-runs (no ::DROPDOWN on this
  // dashboard — nav_picker alone drives it), the URL and the m2 link follow.
  await p.selectOption(".served-nav-picker select", "van");
  await p.waitForFunction(() => document.querySelector(".panel")?.textContent.includes("van"), null, { timeout: 60000 });
  // [?&] : syncUrlState appends v.* params after any other param already on
  // the URL (e.g. this script's own ?theme=), so v.model isn't necessarily
  // the first one.
  check(/[?&]v\.model=van(&|$)/.test(await p.evaluate(() => location.href)), "picking a value updates ?v.model= on the URL");
  check((await p.getAttribute('.served-nav a[data-nav-id="m2"]', "href")) === "/d/m2?v.model=van", "nav link follows the picker's new value");

  // Clicking the m2 link carries the value over (part 2, a plain navigation).
  await p.click('.served-nav a[data-nav-id="m2"]');
  await p.waitForURL(/\/d\/m2\?v\.model=van$/);
  await p.waitForFunction(() => document.querySelector(".panel")?.textContent.includes("van"), null, { timeout: 60000 });
  check(true, "m2 opens already on the carried-over model");
} finally {
  stop(srv);
}

check(!errs.length, `no page errors ${errs.join(" | ")}`);
// #244: never requested on a served (locked) page.
const assistLeak = allRequests.filter((u) => /assist-ui\.(js|css)|vendor\/anthropic-sdk|assist\/(guide|instructions)\.md/.test(u));
check(assistLeak.length === 0, `no assistant request on a served page (${JSON.stringify(assistLeak)})`);
await browser.close();
for (const d of [small, big, viewsDir, navDir, navData]) rmSync(d, { recursive: true, force: true });
if (!process.exitCode) console.log(`served-e2e (${tag}): ok`);
