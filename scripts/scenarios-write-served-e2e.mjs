// Scenario writes on a real locked server (#1243 S2): a writer adds a
// scenario from the page (`::SAVE_ACTION 'create'`), it becomes the picked
// one, a ::FORM's row saves into it (an update keeps the columns the form
// doesn't carry; a new key inserts), main stays as it was, a scenario made
// from another one starts from its changes; a name that exists, a save
// while main is picked and a viewer who isn't a writer are refused with the
// reason; the endpoint's checks hold for a direct POST; the log has one
// line per operation. Needs a native extension build with the real UI
// embedded (CI's `web-e2e` job builds it) and anofox_scenario from the
// community repository (skipped, exit 0, when it can't be installed).
//
//   node scripts/scenarios-write-served-e2e.mjs \
//       --ext path/to/anofox_visualization.duckdb_extension [--port 8957] [--duckdb duckdb]
import { createRequire } from "module";
import { spawn, spawnSync } from "child_process";
import { mkdtempSync, mkdirSync, writeFileSync, readFileSync, rmSync, existsSync } from "fs";
import path from "path";
import crypto from "crypto";

const require = createRequire(import.meta.url);
const { chromium } = require("playwright");
const arg = (k, d) => {
  const i = process.argv.indexOf(k);
  return i > 0 ? process.argv[i + 1] : d;
};
const ext = arg("--ext", "");
const port = Number(arg("--port", "8957"));
const duckdb = arg("--duckdb", "duckdb");
if (!ext || !existsSync(ext)) {
  console.error("usage: scenarios-write-served-e2e.mjs --ext <anofox_visualization.duckdb_extension> (build it with duckext/scripts/build-native.sh)");
  process.exit(2);
}
let bad = 0;
const check = (ok, m) => (ok ? console.log("ok   " + m) : (bad++, console.error("FAIL " + m)));
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const sqlStr = (s) => `'${s.replace(/'/g, "''")}'`;
const env = { ...process.env, DATAZOO_DISABLE_TELEMETRY: "1" };

// Scratch next to the extension (the log's path rules refuse a
// world-writable parent, like options.state's).
const dir = mkdtempSync(path.join(path.dirname(path.resolve(ext)), "scenarios-write-e2e-"));
const db = path.join(dir, "plan.duckdb");
// 1. A database: capacity with a primary key (and a column the form never
// writes), plan without one (its key comes from the declaration).
const made = spawnSync(duckdb, [db, "-c", `INSTALL anofox_scenario FROM community; LOAD anofox_scenario;
  CREATE TABLE capacity (line VARCHAR PRIMARY KEY, per_shift DOUBLE, note VARCHAR);
  INSERT INTO capacity VALUES ('L1', 100, 'keep'), ('L2', 80, 'keep');
  CREATE TABLE plan (sku VARCHAR, week DATE, qty DOUBLE);
  INSERT INTO plan VALUES ('A', '2026-01-05', 10);`], { env, encoding: "utf8" });
if (made.status !== 0) {
  console.log(`NOTE: anofox_scenario could not be installed or set up; skipped (${(made.stderr || "").split("\n")[0]})`);
  rmSync(dir, { recursive: true, force: true });
  process.exit(0);
}
const dashDir = path.join(dir, "dash");
mkdirSync(dashDir, { recursive: true });
writeFileSync(
  path.join(dashDir, "sop.sql"),
  `SELECT name AS scenario ::SCENARIO FROM anofox_scenarios();
SELECT 'second_shift' AS new_name ::TEXT;
SELECT 'Add scenario' ::SAVE, 'create' ::SAVE_ACTION, 'new_name' ::SAVE_KEY, 'scenario' ::SAVE_SCENARIO;
SELECT field AS cap ::FORM, type, value, options FROM (VALUES
  (1, 'line', 'dropdown', 'L1', 'L1, L2, L3'),
  (2, 'per_shift', 'number', '100', NULL)
) v(o, field, type, value, options) ORDER BY o;
SELECT 'Save capacity' ::SAVE, 'cap' ::SAVE_FORM, 'capacity' ::SAVE_TABLES, 'scenario' ::SAVE_SCENARIO;
SELECT sum(per_shift)::BIGINT AS c ::METRIC, 'Capacity' ::LABEL FROM capacity;
SELECT string_agg(line || '=' || per_shift::BIGINT || ':' || coalesce(note, '-'), ' ' ORDER BY line) AS t ::TEXT_SMALL FROM capacity;
SELECT 'Freeze' ::SAVE, 'freeze' ::SAVE_ACTION, 'scenario' ::SAVE_SCENARIO;
SELECT 'Unfreeze' ::SAVE, 'unfreeze' ::SAVE_ACTION, 'scenario' ::SAVE_SCENARIO;
SELECT 'Make official' ::SAVE, 'merge' ::SAVE_ACTION, 'scenario' ::SAVE_SCENARIO;
SELECT 'Drop' ::SAVE, 'drop' ::SAVE_ACTION, 'scenario' ::SAVE_SCENARIO;
`
);
const sec = crypto.randomBytes(24).toString("hex");
const logPath = path.join(dir, "scenarios.jsonl");
const opts = {
  auth: { mode: "proxy", shared_secret: sec, trusted_proxies: ["127.0.0.1"] },
  policy: "none",
  search_path: "warehouse",
  scenarios: {
    database: "warehouse",
    path: logPath,
    dashboards: ["sop"],
    writers: { users: ["alice"] },
    approvers: { users: ["carol"] },
    rate: { per_minute: 600, burst: 50 },
    tables: {
      capacity: { key: ["line"], columns: { line: "VARCHAR", per_shift: "DOUBLE" } },
      plan: { key: ["sku", "week"], columns: { sku: "VARCHAR", week: "DATE", qty: "DOUBLE" } },
    },
  },
};

// 2. The locked server (a DuckDB process we own; stopped by PID).
async function serve() {
  const p = spawn(duckdb, ["-unsigned", "-batch"], { stdio: ["pipe", "pipe", "pipe"], env });
  let log = "";
  p.stdout.on("data", (d) => (log += d));
  p.stderr.on("data", (d) => (log += d));
  p.stdin.write(`LOAD ${sqlStr(ext)};\nLOAD anofox_scenario;\nATTACH ${sqlStr(db)} AS warehouse;\nSELECT anofox_serve_dashboards(${sqlStr(dashDir)}, ${port}, ${sqlStr(JSON.stringify(opts))});\n`);
  for (let i = 0; i < 100; i++) {
    try {
      if ((await fetch(`http://127.0.0.1:${port}/healthz`)).status < 500) return { proc: p, log: () => log };
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

let srv = null;
let browser = null;
try {
  srv = await serve();
  browser = await chromium.launch();
  const as = async (user) => {
    const ctx = await browser.newContext({ viewport: { width: 1100, height: 900 }, reducedMotion: "reduce" });
    await ctx.setExtraHTTPHeaders({ "X-Anofox-Proxy-Secret": sec, "X-Forwarded-User": user });
    const page = await ctx.newPage();
    const errs = [];
    page.on("pageerror", (e) => errs.push(String(e)));
    return { page, errs };
  };
  const { page, errs } = await as("alice");
  const settled = async () => {
    await sleep(300);
    await page.waitForFunction(
      () => !document.body.classList.contains("loading") && !!document.querySelector(".metric-value"),
      null,
      { timeout: 15000 }
    );
  };
  const kpi = () => page.locator(".metric-value").first().textContent();
  const kpiIs = async (v) => {
    await page.waitForFunction((v) => document.querySelector(".metric-value")?.textContent === v, v, { timeout: 15000 }).catch(() => {});
    return (await kpi()) === v;
  };
  const rows = () => page.locator(".text-small, .text-card").first().textContent().catch(() => "");
  const picked = () => page.locator(".dash select[aria-label=scenario]").inputValue();
  const panel = (p, label) => p.locator(".save-panel", { has: p.locator(`.submit-btn:text-matches("^${label}")`) });
  const btn = (p, label) => panel(p, label).locator(".submit-btn");

  await page.goto(`http://127.0.0.1:${port}/d/sop`);
  await settled();
  check((await kpi()) === "180", `main by default (${await kpi()})`);
  check(await btn(page, "Save capacity").isDisabled(), "a save is disabled while main is picked");
  check(/Pick or create a scenario first/.test(await panel(page, "Save capacity").innerText()), "… saying why");

  // (a) create: the new scenario becomes the picked one, a copy of main
  await btn(page, "Add scenario").click();
  await page.waitForFunction(() => document.querySelector(".dash select[aria-label=scenario]")?.value === "second_shift", null, { timeout: 15000 }).catch(() => {});
  check((await picked()) === "second_shift", `the new scenario is picked (${await picked()})`);
  await settled();
  check((await kpi()) === "180", `it starts as main (${await kpi()})`);
  check(/Created second_shift/.test(await panel(page, "Add scenario").innerText()), "the button says it was created");
  check(/→ second_shift/.test(await btn(page, "Save capacity").innerText()), `the save names the scenario (${await btn(page, "Save capacity").innerText()})`);

  // (b) write: an update keeps the column the form doesn't carry, a new key inserts
  await page.fill('.dp-form-field[data-field="per_shift"] input', "150");
  await page.keyboard.press("Enter");
  await settled();
  await btn(page, "Save capacity").click();
  check(await kpiIs("230"), `the save reads back in the scenario (${await kpi()})`);
  check(/L1=150:keep/.test(await rows()), `an update keeps the other columns (${await rows()})`);
  await page.selectOption('.dp-form-field[data-field="line"] select', "L3");
  await page.fill('.dp-form-field[data-field="per_shift"] input', "20");
  await page.keyboard.press("Enter");
  await settled();
  await btn(page, "Save capacity").click();
  check(await kpiIs("250"), `a new key inserts a row (${await kpi()})`);
  check(/L3=20:-/.test(await rows()), `… with only its declared columns (${await rows()})`);

  // (c) main is unchanged
  await page.selectOption(".dash select[aria-label=scenario]", "main");
  check(await kpiIs("180"), `main is unchanged (${await kpi()})`);

  // (d) a scenario made from another one starts from its changes; a name that exists is refused
  await page.selectOption(".dash select[aria-label=scenario]", "second_shift");
  await settled();
  await btn(page, "Add scenario").click();
  await page.waitForFunction(() => /exists already/.test(document.body.innerText), null, { timeout: 15000 }).catch(() => {});
  check(/exists already/.test(await panel(page, "Add scenario").innerText()), "a name that exists is refused, saying why");
  const nameInput = page.locator(".dash input[type=text]:not(.dp-form-input)").first();
  await nameInput.fill("third");
  await nameInput.press("Enter");
  await settled();
  await btn(page, "Add scenario").click();
  await page.waitForFunction(() => document.querySelector(".dash select[aria-label=scenario]")?.value === "third", null, { timeout: 15000 }).catch(() => {});
  check((await picked()) === "third", `the child is picked (${await picked()})`);
  check(await kpiIs("250"), `it starts from its parent's changes (${await kpi()})`);

  // (e) a viewer who isn't a writer: the buttons disabled with the reason
  const bob = await as("bob");
  await bob.page.goto(`http://127.0.0.1:${port}/d/sop?v.scenario=second_shift`);
  await bob.page.waitForFunction(() => !!document.querySelector(".metric-value"), null, { timeout: 15000 });
  await sleep(500);
  check(await btn(bob.page, "Add scenario").isDisabled(), "bob can't add a scenario");
  check(/Only this dashboard's scenario writers/.test(await panel(bob.page, "Add scenario").innerText()), "… saying why");
  check((await bob.page.locator(".metric-value").first().textContent()) === "250", "bob reads the scenario alice wrote");

  // (f) the endpoint's own checks, for a direct POST
  const post = (body, headers = { "X-Anofox-Submit": "1" }) =>
    page.evaluate(
      async ([b, h]) => {
        const res = await fetch("/api/save", { method: "POST", headers: { "Content-Type": "application/json", ...h }, body: JSON.stringify(b) });
        return [res.status, await res.text()];
      },
      [body, headers]
    );
  const id = () => crypto.randomBytes(8).toString("hex");
  let [st, msg] = await post({ dashboard: "sop", panel: 4, id: id(), vars: { scenario: "main" }, form: { line: "L1", per_shift: 1 } });
  check(st === 400 && /never writes main/.test(msg), `a write into main is refused (${st} ${msg})`);
  [st, msg] = await post({ dashboard: "sop", panel: 4, id: id(), vars: { scenario: "second_shift" }, form: { line: "L1", note: "x" } });
  check(st === 400 && /'note' is not a column/.test(msg), `an undeclared column is refused (${st} ${msg})`);
  [st, msg] = await post({ dashboard: "sop", panel: 2, id: id(), vars: { scenario: "main", new_name: "x'; DROP TABLE capacity; --" } });
  check(st === 400 && /letters, digits and _/.test(msg), `a hostile name is refused (${st} ${msg})`);
  [st, msg] = await post({ dashboard: "sop", panel: 4, id: id(), vars: { scenario: "ghost" }, form: { line: "L1", per_shift: 1 } });
  check(st === 409 && /no scenario 'ghost'/.test(msg), `an unknown scenario is refused (${st} ${msg})`);
  [st] = await post({ dashboard: "sop", panel: 4, id: id(), vars: { scenario: "second_shift" }, form: { line: "L1", per_shift: 1 } }, {});
  check(st === 403, `the CSRF header is required (${st})`);

  // (g) the log: one line per operation, with the writer
  const lines = readFileSync(logPath, "utf8").trim().split("\n").slice(1).map((l) => JSON.parse(l));
  const acts = lines.map((l) => `${l.vars.action}:${l.vars.scenario}`).join(" ");
  check(acts === "create:second_shift write:second_shift write:second_shift create:third", `the log (${acts})`);
  check(lines.every((l) => l.user === "alice") && lines[3].vars.parent === "second_shift", "… each by alice, the child with its parent");

  // S3, approvers: (h) a writer can't approve; an approver can't write
  check(await btn(page, "Freeze").isDisabled(), "alice (a writer) can't freeze");
  check(/Only approvers/.test(await panel(page, "Freeze").innerText()), "… saying why");
  const carol = await as("carol");
  const cp = carol.page;
  const cSettled = async () => {
    await sleep(300);
    await cp.waitForFunction(() => !document.body.classList.contains("loading") && !!document.querySelector(".metric-value"), null, { timeout: 15000 });
  };
  const cKpiIs = async (v) => {
    await cp.waitForFunction((v) => document.querySelector(".metric-value")?.textContent === v, v, { timeout: 15000 }).catch(() => {});
    return (await cp.locator(".metric-value").first().textContent()) === v;
  };
  const cPick = async (v) => {
    await cp.selectOption(".dash select[aria-label=scenario]", v);
    await cSettled();
  };
  const cSays = async (label, re) => {
    await cp.waitForFunction(([l, r]) => [...document.querySelectorAll(".save-panel")].some((p) => p.innerText.startsWith(l) && new RegExp(r).test(p.innerText)), [label, re.source], { timeout: 15000 }).catch(() => {});
    return re.test(await panel(cp, label).innerText());
  };
  await cp.goto(`http://127.0.0.1:${port}/d/sop?v.scenario=second_shift`);
  await cSettled();
  check(await btn(cp, "Save capacity").isDisabled(), "carol (an approver) can't save into a scenario");

  // (i) a scenario others branch from can't be merged; drop the child
  await btn(cp, "Make official").click();
  check(await cSays("Make official", /branch from 'second_shift' \(third\)/), "a merge waits for the scenarios branching from it");
  await cPick("third");
  await btn(cp, "Drop").click();
  await cp.waitForFunction(() => document.querySelector(".dash select[aria-label=scenario]")?.value === "main", null, { timeout: 15000 }).catch(() => {});
  const cOpts = await cp.locator(".dash select[aria-label=scenario] option").allTextContents();
  check(!cOpts.includes("third") && (await cp.locator(".dash select[aria-label=scenario]").inputValue()) === "main", `a dropped scenario is gone, the page is back on main (${cOpts})`);

  // (j) freeze stops writes, unfreeze allows them again
  await cPick("second_shift");
  await btn(cp, "Freeze").click();
  check(await cSays("Freeze", /Froze second_shift/), "carol froze second_shift");
  [st, msg] = await post({ dashboard: "sop", panel: 4, id: id(), vars: { scenario: "second_shift" }, form: { line: "L1", per_shift: 1 } });
  check(st === 409 && /frozen/.test(msg), `a write into a frozen scenario is refused (${st} ${msg})`);
  await btn(cp, "Unfreeze").click();
  check(await cSays("Unfreeze", /Unfroze second_shift/), "carol unfroze it");

  // (k) a sibling that changes the same row; merge previews, then merges
  [st] = await post({ dashboard: "sop", panel: 2, id: id(), vars: { scenario: "main", new_name: "night_shift" } });
  check(st === 201, `alice adds night_shift (${st})`);
  [st] = await post({ dashboard: "sop", panel: 4, id: id(), vars: { scenario: "night_shift" }, form: { line: "L1", per_shift: 170 } });
  check(st === 201, `… and changes L1 in it (${st})`);
  await btn(cp, "Make official").click();
  check(await cSays("Make official", /changes 2 in capacity\. Click again to confirm/), "a merge says what it changes first");
  await btn(cp, "Make official").click();
  check(await cSays("Make official", /Made official: second_shift/), "… and merges on the second click");
  await cPick("main");
  check(await cKpiIs("250"), `main has the merged changes (${await cp.locator(".metric-value").first().textContent()})`);
  await cp.reload();
  await cSettled();
  await cPick("night_shift");
  await btn(cp, "Make official").click();
  check(await cSays("Make official", /main changed meanwhile \(capacity L1\)/), "a conflicting merge is refused, naming the row");
  [st, msg] = await post({ dashboard: "sop", panel: 8, id: id(), vars: { scenario: "second_shift" } });
  check(st === 403, `alice may not unfreeze (${st})`);
  const cPost = (body) =>
    cp.evaluate(async (b) => {
      const res = await fetch("/api/save", { method: "POST", headers: { "Content-Type": "application/json", "X-Anofox-Submit": "1" }, body: JSON.stringify(b) });
      return [res.status, await res.text()];
    }, body);
  [st, msg] = await cPost({ dashboard: "sop", panel: 8, id: id(), vars: { scenario: "second_shift" } });
  check(st === 409 && /official/.test(msg), `a merged scenario stays frozen (${st} ${msg})`);
  [st, msg] = await cPost({ dashboard: "sop", panel: 9, id: id(), vars: { scenario: "night_shift" } });
  check(st === 409 && /"conflicts"/.test(msg), `a direct merge with conflicts is refused (${st})`);
  const all = readFileSync(logPath, "utf8").trim().split("\n").slice(1).map((l) => JSON.parse(l));
  const later = all.slice(4).map((l) => `${l.user}:${l.vars.action}:${l.vars.scenario}`).join(" ");
  check(
    later === "carol:drop:third carol:freeze:second_shift carol:unfreeze:second_shift alice:create:night_shift alice:write:night_shift carol:merge:second_shift",
    `the approvals are logged (${later})`
  );

  check(errs.length === 0 && bob.errs.length === 0 && carol.errs.length === 0, `no page errors${errs.length ? ": " + errs.join("; ") : ""}`);
} catch (e) {
  bad++;
  console.error("FAIL " + (e && e.stack ? e.stack : e));
  if (srv) console.error(srv.log().split("\n").slice(-30).join("\n"));
} finally {
  if (srv) stop(srv);
  if (browser) await browser.close();
  rmSync(dir, { recursive: true, force: true });
}
console.log(`scenarios-write-served-e2e: ${bad} failed`);
if (bad) process.exit(1);
