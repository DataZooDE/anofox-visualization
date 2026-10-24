// Searchable ::DROPDOWN (#411) in a real browser: above ~50 options (or
// forced by `'search'`/`'native'::DROPDOWN_STYLE`) a ::DROPDOWN renders an
// accessible combobox instead of a plain <select> — makeSearchableDropdown()
// in web/app.js, which no unit test can exercise (it's all live DOM + real
// keyboard events).
//
//   wasm-pack build --target web --out-dir web/pkg --profile wasm --no-default-features --features wasm
//   python3 serve_nocache.py 8797 &
//   NODE_PATH=<dir with playwright>/node_modules node scripts/dropdown-search-e2e.mjs \
//       [--url http://127.0.0.1:8797/] [--shots <dir>] [--axe axe.min.js]
//
// (a) <= 50 options: a plain <select>, no combobox.
// (b) > 50 options (400): a combobox — role=combobox/aria-expanded/
//     aria-controls/aria-activedescendant on the input, role=listbox/option
//     + aria-selected in the popover; typing filters by value AND
//     ::OPTION_LABEL (case-insensitive substring); Arrow keys move the
//     highlighted option, Enter picks it (same code path as the native
//     <select>: dpVars + the URL's ?v.<name>= + a re-run); Esc closes and
//     restores the shown label without picking; a click outside closes too.
// (c) 'native'::DROPDOWN_STYLE keeps the plain <select> above the threshold;
//     'search'::DROPDOWN_STYLE forces the combobox below it.
// (d) 2,000 options: filtering renders at most a window of matches (not
//     2,000 DOM rows) and stays responsive.
// (e) the downloaded HTML snapshot has no JS to drive the combobox, so it
//     keeps the hidden native <select> fallback instead (downloadHtml()'s
//     swap) — the offline file still shows a working dropdown, as a plain
//     ::DROPDOWN's <select> already does.
// --shots <dir> also writes light/dark (1280x900) and phone (390x844)
// screenshots of the open combobox.
import { createRequire } from "module";
import { readFileSync, existsSync } from "fs";

const require = createRequire(import.meta.url);
const { chromium } = require("playwright");
const arg = (k, d) => {
  const i = process.argv.indexOf(k);
  return i > 0 ? process.argv[i + 1] : d;
};
const base = arg("--url", "http://127.0.0.1:8797/");
const shots = arg("--shots", "");
const axe = arg("--axe", "");
const b64 = (s) => Buffer.from(s, "utf8").toString("base64");
let bad = 0;
const check = (ok, m) => (ok ? console.log("ok   " + m) : (bad++, console.error("FAIL " + m)));

const browser = await chromium.launch();

const ready = (page) =>
  page.waitForFunction(
    () => /\d+ panels?/.test(document.getElementById("status")?.textContent || "") && !document.body.classList.contains("loading"),
    null,
    { timeout: 120000 }
  );
async function open(sql, opts = {}) {
  const page = await browser.newPage({
    viewport: opts.viewport || { width: 1280, height: 900 },
    reducedMotion: "reduce",
    colorScheme: opts.theme || "light",
    acceptDownloads: true,
  });
  const errs = [];
  page.on("pageerror", (e) => errs.push(String(e)));
  await page.goto(`${base}#sql=${b64(sql)}`);
  await ready(page);
  return { page, errs };
}
// 400 distinct options ('ext_000' .. 'ext_399') with a count ::HINT — the
// issue's own "long option list" shape (388 extensions).
const MANY = `SELECT 'ext_' || lpad(i::VARCHAR, 3, '0') AS ext ::DROPDOWN, i ::HINT FROM range(400) t(i);
SELECT getvariable('ext') AS picked ::TABLE;`;
const FEW = `SELECT v AS ext ::DROPDOWN FROM (VALUES ('a'), ('b'), ('c')) t(v);`;

// (a) ---------------------------------------------------------------------
{
  const { page, errs } = await open(FEW);
  const info = await page.evaluate(() => ({
    select: !!document.querySelector(".control select"),
    combo: !!document.querySelector(".dp-dd"),
  }));
  check(info.select && !info.combo, "(a) <= 50 options: a plain <select>, no combobox");
  check(!errs.length, "(a) no page errors: " + errs.join(" | "));
  await page.close();
}

// (b) ---------------------------------------------------------------------
{
  const { page, errs } = await open(MANY);
  const combo = page.locator(".dp-dd-input");
  const info0 = await page.evaluate(() => {
    const inp = document.querySelector(".dp-dd-input");
    return {
      select: !!document.querySelector(".control > select"),
      role: inp?.getAttribute("role"),
      expanded: inp?.getAttribute("aria-expanded"),
      controls: inp?.getAttribute("aria-controls"),
      value: inp?.value,
    };
  });
  check(!info0.select, "(b) > 50 options: no plain <select> as the visible control");
  check(info0.role === "combobox", "(b) role=combobox on the input");
  check(info0.expanded === "false", "(b) aria-expanded=false while closed");
  check(info0.value === "ext_000", "(b) shows the current pick (ext_000) while closed, got " + info0.value);

  await combo.click();
  const afterOpen = await page.evaluate(() => {
    const inp = document.querySelector(".dp-dd-input");
    const list = document.getElementById(inp.getAttribute("aria-controls"));
    return {
      expanded: inp.getAttribute("aria-expanded"),
      activedescendant: inp.getAttribute("aria-activedescendant"),
      listRole: list?.getAttribute("role"),
      rows: list.querySelectorAll('[role="option"]').length,
      firstSelected: list.querySelector('[role="option"]')?.getAttribute("aria-selected"),
    };
  });
  check(afterOpen.expanded === "true", "(b) a click opens it (aria-expanded=true)");
  check(afterOpen.listRole === "listbox", "(b) the popover is role=listbox");
  check(afterOpen.rows === 200, "(b) renders a window of the 400 options, got " + afterOpen.rows);
  check(afterOpen.firstSelected === "true", "(b) the current pick is aria-selected in the list");
  check(!!afterOpen.activedescendant, "(b) aria-activedescendant points at the highlighted option");

  // Type a filter that matches one option by value.
  await combo.fill("ext_250");
  const filtered = await page.evaluate(() => {
    const list = document.querySelector(".dp-dd-list");
    return [...list.querySelectorAll('[role="option"]')].map((o) => o.textContent);
  });
  check(filtered.length === 1 && filtered[0].includes("ext_250"), "(b) typing filters to the matching option, got " + JSON.stringify(filtered));

  await page.keyboard.press("Enter");
  // Picking re-runs (dpVars + the URL + getvariable() downstream) — async,
  // same as a native <select>'s onchange: wait for the table to catch up.
  await page.waitForFunction(() => document.querySelector(".dp-table td")?.textContent === "ext_250", null, { timeout: 10000 });
  const picked = await page.evaluate(() => ({
    value: document.querySelector(".dp-dd-input").value,
    expanded: document.querySelector(".dp-dd-input").getAttribute("aria-expanded"),
    cell: document.querySelector(".dp-table td")?.textContent,
    url: location.href,
  }));
  check(picked.value.includes("ext_250"), "(b) Enter picks the filtered option, input shows " + picked.value);
  check(picked.expanded === "false", "(b) Enter closes the popover");
  check(picked.cell === "ext_250", "(b) the picked value reached getvariable('ext') (same code path as a native <select>)");
  check(/[?&]v\.ext=ext_250(&|#|$)/.test(picked.url), "(b) the URL carries ?v.ext=ext_250, got " + picked.url);

  // Arrow keys move the highlighted option without a filter.
  const combo2 = page.locator(".dp-dd-input");
  await combo2.click();
  await combo2.fill("");
  await page.keyboard.press("ArrowDown");
  await page.keyboard.press("ArrowDown");
  const afterArrows = await page.evaluate(() => {
    const inp = document.querySelector(".dp-dd-input");
    const list = document.getElementById(inp.getAttribute("aria-controls"));
    const active = list.querySelector(".dp-dd-active");
    return { activeId: active?.id, adId: inp.getAttribute("aria-activedescendant"), idx: [...list.children].indexOf(active) };
  });
  check(afterArrows.idx === 2, "(b) two ArrowDown presses move the highlight to the 3rd row, got index " + afterArrows.idx);
  check(afterArrows.activeId === afterArrows.adId, "(b) aria-activedescendant follows the arrow-key highlight");

  // Esc closes without picking and restores the shown label.
  await combo2.fill("zzz-no-match");
  await page.keyboard.press("Escape");
  const afterEsc = await page.evaluate(() => ({
    value: document.querySelector(".dp-dd-input").value,
    expanded: document.querySelector(".dp-dd-input").getAttribute("aria-expanded"),
    url: location.href,
  }));
  check(afterEsc.value.includes("ext_250"), "(b) Esc restores the current pick's label, got " + afterEsc.value);
  check(afterEsc.expanded === "false", "(b) Esc closes the popover");
  check(/v\.ext=ext_250/.test(afterEsc.url), "(b) Esc does not change the picked variable");

  // A click outside closes it too.
  await combo2.click();
  check((await page.evaluate(() => document.querySelector(".dp-dd").getAttribute("data-open"))) === "true", "(b) setup: reopened before the outside click");
  await page.mouse.click(640, 890); // the bottom of the viewport: clear of any header/sidebar chrome
  await page.waitForTimeout(50);
  check(
    (await page.evaluate(() => document.querySelector(".dp-dd").getAttribute("data-open"))) === null,
    "(b) a click outside the combobox closes it"
  );

  if (axe && existsSync(axe)) {
    await combo2.click();
    await page.addScriptTag({ path: axe });
    const res = await page.evaluate(async () => window.axe.run(".dp-dd", { runOnly: { type: "tag", values: ["wcag2a", "wcag2aa"] } }));
    for (const v of res.violations) console.log(`axe ::DROPDOWN combobox ${v.impact}: ${v.id} (${v.nodes.length}) ${v.help}`);
    const axeBad = res.violations.filter((v) => v.impact === "serious" || v.impact === "critical");
    if (axeBad.length) check(false, "axe ::DROPDOWN combobox: " + axeBad.map((v) => v.id).join(", "));
    else console.log(`axe ::DROPDOWN combobox: ${res.passes.length} rules pass, no serious violation`);
  }

  check(!errs.length, "(b) no page errors: " + errs.join(" | "));
  if (shots) {
    await combo2.click();
    await combo2.fill("ext_1");
    await page.screenshot({ path: `${shots}/dropdown-search-light-1280.png` });
  }
  await page.close();
}

// (c) ::DROPDOWN_STYLE overrides the threshold either way ------------------
{
  const { page, errs } = await open(`SELECT 'ext_' || lpad(i::VARCHAR, 3, '0') AS ext ::DROPDOWN, 'native' ::DROPDOWN_STYLE FROM range(400) t(i);`);
  const native = await page.evaluate(() => ({ select: !!document.querySelector(".control select"), combo: !!document.querySelector(".dp-dd") }));
  check(native.select && !native.combo, "(c) 'native'::DROPDOWN_STYLE keeps the plain <select> above the threshold");
  check(!errs.length, "(c, native) no page errors: " + errs.join(" | "));
  await page.close();
}
{
  const { page, errs } = await open(`SELECT v AS ext ::DROPDOWN, 'search' ::DROPDOWN_STYLE FROM (VALUES ('a'), ('b'), ('c')) t(v);`);
  const forced = await page.evaluate(() => ({ select: !!document.querySelector(".control > select"), combo: !!document.querySelector(".dp-dd") }));
  check(forced.combo && !forced.select, "(c) 'search'::DROPDOWN_STYLE forces the combobox below the threshold");
  check(!errs.length, "(c, search) no page errors: " + errs.join(" | "));
  await page.close();
}

// (d) 2,000 options: a window of matches, not 2,000 DOM rows ---------------
{
  const { page, errs } = await open(`SELECT 'opt_' || lpad(i::VARCHAR, 4, '0') AS opt ::DROPDOWN FROM range(2000) t(i);`);
  const combo = page.locator(".dp-dd-input");
  const t0 = Date.now();
  await combo.click();
  await combo.fill("opt_1"); // matches opt_1000..opt_1999 (1000 of them)
  const elapsed = Date.now() - t0;
  const info = await page.evaluate(() => ({
    rows: document.querySelectorAll(".dp-dd-opt").length,
    more: document.querySelector(".dp-dd-more")?.textContent || "",
  }));
  check(info.rows > 0 && info.rows <= 200, "(d) at most a window of rows is rendered for 1,000 matches, got " + info.rows);
  check(/more/.test(info.more), "(d) a \"+N more\" note shows the rest were narrowed, not rendered: " + info.more);
  check(elapsed < 3000, "(d) filtering 2,000 options stayed responsive (" + elapsed + "ms)");
  check(!errs.length, "(d) no page errors: " + errs.join(" | "));
  await page.close();
}

// (e) the downloaded HTML snapshot keeps a working native <select> --------
{
  const { page, errs } = await open(MANY);
  await page.locator(".dp-dd-input").click();
  await page.locator(".dp-dd-input").fill("ext_250");
  await page.keyboard.press("Enter");
  await page.waitForFunction(() => document.querySelector(".dp-table td")?.textContent === "ext_250", null, { timeout: 10000 });
  check(/v\.ext=ext_250/.test(page.url()), "(e) setup: ext_250 is picked before exporting");

  const [dl] = await Promise.all([page.waitForEvent("download"), page.click("#dlhtml")]);
  const html = readFileSync(await dl.path(), "utf8");
  check(!/class="dp-dd"/.test(html), "(e) the exported HTML drops the live combobox markup");
  check(/<select[^>]*class="dp-dd-fallback/.test(html), "(e) …and keeps its native <select> fallback, promoted");

  const off = await browser.newContext({ offline: true });
  const op = await off.newPage();
  const exportErrs = [];
  op.on("pageerror", (e) => exportErrs.push(String(e)));
  await op.route("**/*", (r) => (r.request().url().startsWith("http") ? (r.abort(), undefined) : r.continue()));
  await op.setContent(html, { waitUntil: "load" });
  const snap = await op.evaluate(() => {
    const sel = document.querySelector("select");
    return { exists: !!sel, hidden: sel?.hidden, options: sel?.options.length, value: sel?.value };
  });
  check(snap.exists && !snap.hidden, "(e) the snapshot shows a plain, visible <select>");
  check(snap.options === 400, "(e) …with all 400 options, got " + snap.options);
  // A plain ::DROPDOWN's own <select> has the same limitation already (no
  // JS there to carry a picked value's "selected" into the serialized
  // markup — innerHTML never did, before #411): same parity, not a
  // regression, so this only checks it is a usable, browsable dropdown.
  check(!!snap.value, "(e) …with a valid option selected (some value, not blank), got " + JSON.stringify(snap.value));
  check(!exportErrs.length, "(e) no errors opening the export offline: " + exportErrs.join(" | "));
  await off.close();
  check(!errs.length, "(e) no page errors on the live page: " + errs.join(" | "));
  await page.close();
}

// dark + phone screenshots -------------------------------------------------
if (shots) {
  for (const theme of ["light", "dark"]) {
    const { page } = await open(MANY, { theme });
    await page.locator(".dp-dd-input").click();
    await page.screenshot({ path: `${shots}/dropdown-search-${theme}-1280.png` });
    await page.close();
  }
  {
    const { page } = await open(MANY, { viewport: { width: 390, height: 844 } });
    await page.locator(".dp-dd-input").click();
    const overflow = await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth);
    check(overflow <= 1, "(phone) no horizontal overflow at 390px with the combobox open, got " + overflow);
    await page.screenshot({ path: `${shots}/dropdown-search-phone-390.png` });
    await page.close();
  }
}

await browser.close();
console.log(`dropdown-search-e2e: ${bad} failed`);
if (bad) process.exitCode = 1;
