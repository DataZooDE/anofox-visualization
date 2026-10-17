// The builder's Check button (#145) in a real browser: in Edit mode it lints
// the editor's SQL over DuckDB-Wasm with the diagnostics of `dashboard
// --check` and lists them (severity, statement, code); a click or Enter on
// one selects that statement in the editor. Strict (`--strict`) and Design
// advice (`--no-design`) change the verdict and the list; an edit marks the
// list as old; View mode hides it. On a desktop and a phone viewport, light
// and dark, with axe (--axe) on the Check region.
//
//   python3 serve_nocache.py 8791 &
//   NODE_PATH=<dir with playwright>/node_modules node scripts/lint-e2e.mjs \
//       [--url http://127.0.0.1:8791/] [--axe axe.min.js] [--shots DIR]
import { createRequire } from "module";
import { existsSync, mkdirSync, readFileSync } from "fs";
import path from "path";

const require = createRequire(import.meta.url);
const { chromium } = require("playwright");
const arg = (k, d) => {
  const i = process.argv.indexOf(k);
  return i > 0 ? process.argv[i + 1] : d;
};
const base = arg("--url", "http://127.0.0.1:8791/");
const axe = arg("--axe", "");
const shots = arg("--shots", "");
if (shots) mkdirSync(shots, { recursive: true });
const root = path.resolve(path.dirname(new URL(import.meta.url).pathname), "..");
const fail = (m) => {
  console.error("FAIL: " + m);
  process.exitCode = 1;
};
const check = (ok, m) => (ok ? console.log("ok   " + m) : fail(m));

// The broken fixture and what `dashboard --check --json` reports for it, in
// the list's order (by statement, errors first).
const broken = readFileSync(path.join(root, "tests/fixtures/lint/broken-sales.sql"), "utf8");
const WANT = [
  [2, "error", "theme/unknown"],
  [4, "error", "silent-setup"],
  [5, "error", "sql-error"],
  [5, "warning", "unknown-cast"],
  [6, "error", "sql-error"],
  [7, "warning", "empty-panel"],
  [8, "warning", "design/untitled-chart"],
  [8, "warning", "design/pie-slices"],
  [9, "warning", "design/unsorted-bars"],
  [10, "warning", "design/raw-table"],
];
// Only a design advisory: passes, fails with Strict, clean without Design advice.
const advisory = `CREATE OR REPLACE TABLE t AS SELECT ['a','b','c','d','e','f','g','h'][1 + i % 8] AS k, i AS v FROM range(0, 80) t(i);
SELECT k ::CATEGORY, sum(v) ::PIE, 'Share by k'::TITLE FROM t GROUP BY k;`;
const b64 = (s) => Buffer.from(s, "utf8").toString("base64");

const browser = await chromium.launch();
for (const [label, viewport] of [
  ["desktop", { width: 1400, height: 1000 }],
  ["phone", { width: 390, height: 844 }],
]) {
  for (const scheme of ["light", "dark"]) {
    const tag = `${label} ${scheme}`;
    const page = await browser.newPage({ viewport, reducedMotion: "reduce", colorScheme: scheme });
    const errs = [];
    page.on("pageerror", (e) => errs.push(String(e)));
    let loads = 0; // (a new query each time: a hash change alone doesn't reload)
    const open = async (sql) => {
      await page.goto(`${base}?mode=edit&theme=${scheme}&n=${++loads}#sql=${b64(sql)}`);
      await page.waitForFunction(() => !document.getElementById("lint-run")?.disabled, null, { timeout: 120000 });
    };
    const runCheck = async () => {
      await page.click("#lint-run");
      await page.waitForFunction(() => /Passes|Fails|failed/.test(document.getElementById("lint-status").textContent), null, { timeout: 120000 });
      return page.$eval("#lint-status", (e) => e.textContent);
    };
    const items = () =>
      page.$$eval("#lint-list > li", (lis) =>
        lis.map((li) => ({
          sev: li.className.replace("lint-", ""),
          where: li.querySelector(".lint-where").textContent,
          code: li.querySelector(".lint-where code").textContent,
          button: !!li.querySelector("button.lint-go"),
        }))
      );

    await open(broken);
    check(await page.isVisible("#lint-run"), `${tag}: Check is shown in Edit mode`);
    const st = await runCheck();
    check(/^Fails · 4 errors, 6 warnings/.test(st), `${tag}: the verdict and counts (${st})`);
    const got = await items();
    const same =
      got.length === WANT.length &&
      got.every((g, i) => g.code === WANT[i][2] && g.sev === WANT[i][1] && g.where.startsWith(`Statement ${WANT[i][0]} `) && g.button);
    check(same, `${tag}: the list is dashboard --check's diagnostics, by statement`);
    if (!same) console.log(JSON.stringify(got));
    check(
      (await page.$eval("#lint-list li .lint-sev", (e) => e.textContent)) === "Error",
      `${tag}: the severity is a word, not a colour only`
    );
    check((await page.$eval("#lint-list", (e) => e.getAttribute("aria-label"))) === "Check results", `${tag}: the list is labelled`);
    check((await page.$eval("#lint-status", (e) => e.getAttribute("role"))) === "status", `${tag}: the verdict is a status`);

    if (shots) await page.screenshot({ path: path.join(shots, `lint-${label}-${scheme}.png`), fullPage: false });

    // Enter on the fourth item (statement 5, the mistyped role) selects it.
    await page.focus("#lint-list > li:nth-child(4) button");
    await page.keyboard.press("Enter");
    const sel = await page.evaluate(() => {
      const ta = document.getElementById("sql");
      return { focused: document.activeElement === ta, text: ta.value.slice(ta.selectionStart, ta.selectionEnd) };
    });
    check(sel.focused && /^SELECT day ::XAXIS, sum\(revenue\) ::LINECHRT FROM sales GROUP BY day ORDER BY day$/.test(sel.text), `${tag}: Enter selects statement 5 in the editor (${sel.text.slice(0, 60)})`);
    check(await page.$("#hl .lint-mark"), `${tag}: …and marks it`);
    // A click on the last one (the raw table) scrolls the editor to it.
    await page.click("#lint-list > li:last-child button");
    const last = await page.evaluate(() => {
      const ta = document.getElementById("sql");
      return { text: ta.value.slice(ta.selectionStart, ta.selectionEnd), top: ta.scrollTop, max: ta.scrollHeight - ta.clientHeight };
    });
    check(last.text === "SELECT * ::TABLE FROM sales", `${tag}: a click selects statement 10`);
    check(last.max <= 0 || last.top > 0, `${tag}: …and scrolls the editor to it`);
    const seen = await page.$eval("#hl .lint-mark", (m) => {
      const r = m.getBoundingClientRect();
      return [r.top >= 0 && r.bottom <= innerHeight, Math.round(r.top), Math.round(r.bottom), innerHeight];
    });
    check(seen[0], `${tag}: …into the window (${seen.slice(1)})`);

    if (axe && existsSync(axe)) {
      await page.addScriptTag({ path: axe });
      const res = await page.evaluate(async () => window.axe.run("#lint", { runOnly: { type: "tag", values: ["wcag2a", "wcag2aa"] } }));
      for (const v of res.violations) console.log(`axe ${tag} ${v.impact}: ${v.id} (${v.nodes.length}) ${v.help} — ${v.nodes[0]?.target}`);
      const bad = res.violations.filter((v) => v.impact === "serious" || v.impact === "critical");
      if (bad.length) fail(`axe ${tag}: ${bad.map((v) => v.id).join(", ")}`);
      else console.log(`ok   axe ${tag}: ${res.passes.length} rules pass, no serious violation`);
    }

    // An edit: the list is kept, marked as old; a jump then finds the
    // statement in the new text.
    await page.focus("#sql");
    await page.keyboard.press("Control+Home");
    await page.keyboard.type("-- edited\n");
    check(await page.$("#lint.stale"), `${tag}: an edit marks the list as old`);
    check(/Check again/.test(await page.$eval("#lint-status", (e) => e.textContent)), `${tag}: …and says so`);
    await page.click("#lint-list > li:nth-child(4) button");
    const moved = await page.evaluate(() => {
      const ta = document.getElementById("sql");
      return ta.value.slice(ta.selectionStart, ta.selectionEnd);
    });
    check(moved.startsWith("SELECT day ::XAXIS, sum(revenue) ::LINECHRT"), `${tag}: a jump after the edit still finds statement 5`);

    // Strict and Design advice on a script with one advisory only.
    await open(advisory);
    check(/^Passes · 1 warning/.test(await runCheck()), `${tag}: an advisory alone passes`);
    await page.check("#lint-strict");
    check(/^Fails \(strict\) · 1 warning/.test(await runCheck()), `${tag}: …and fails with Strict`);
    await page.uncheck("#lint-design");
    const clean = await runCheck();
    check(/^Passes \(strict\) · every panel/.test(clean) && (await page.$eval("#lint-list", (e) => e.hidden)), `${tag}: without Design advice it is clean (${clean})`);
    await page.uncheck("#lint-strict");
    await page.check("#lint-design");

    // View mode has no editor and no Check.
    await page.click("#mode-view");
    check(!(await page.isVisible("#lint-run")), `${tag}: View mode hides Check`);
    check(!errs.length, `${tag}: no page errors${errs.length ? ": " + errs.join(" | ") : ""}`);
    await page.close();
  }
}

// An embedded page has no editor, so no Check either.
const ep = await browser.newPage({ viewport: { width: 1200, height: 900 } });
await ep.goto(`${base}?embed=1#sql=${b64(advisory)}`);
await ep.waitForSelector(".dash .panel, .dash figure, .dash svg", { timeout: 120000 });
check(!(await ep.isVisible("#lint-run")), "embed: no Check");
await browser.close();
