// ::SAVE (#684 slice 2) on a real locked server (options.writeback, proxy
// auth): a writer's Save posts /api/save and the change log gets one
// fdatasync'd line per save — an edited graph replaces both declared
// tables, a ::FORM save upserts one typed row keyed by the clicked node; a
// table the server doesn't declare is refused, saying why; a retry with the
// same id doesn't save twice; a viewer who isn't a writer, and a dashboard
// the server doesn't save for, get the button disabled with the reason; the
// endpoint's CSRF header and declared-columns checks hold for a direct POST.
// Saves don't show in panels yet (slice 3 materialises them).
//
//   NODE_PATH=<dir with playwright>/node_modules node scripts/save-served-e2e.mjs \
//       --ext path/to/anofox_visualization.duckdb_extension [--port 8963] [--duckdb duckdb]
import { createRequire } from "module";
import { spawn } from "child_process";
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
const port = Number(arg("--port", "8963"));
const duckdb = arg("--duckdb", "duckdb");
if (!ext || !existsSync(ext)) {
  console.error("usage: save-served-e2e.mjs --ext <anofox_visualization.duckdb_extension> (build it with duckext/scripts/build-native.sh)");
  process.exit(2);
}
let bad = 0;
const check = (ok, m) => (ok ? console.log("ok   " + m) : (bad++, console.error("FAIL " + m)));
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const sqlStr = (s) => `'${s.replace(/'/g, "''")}'`;
const env = { ...process.env, DATAZOO_DISABLE_TELEMETRY: "1" };

// Scratch next to the extension, not under the system temp dir (the change
// log's path rules refuse a world-writable parent, like options.state's).
const W = mkdtempSync(path.join(path.dirname(path.resolve(ext)), "save-served-e2e-"));
const dashDir = path.join(W, "dash");
mkdirSync(dashDir, { recursive: true });
const PLAN = `-- @title Plan
SELECT id ::DIAGRAM, label ::LABEL, type ::CATEGORY, 'edit' AS graph ::DIAGRAM_MODE, 'Network' ::TITLE FROM plan_nodes;
SELECT 'Save plan' ::SAVE, 'graph' ::SAVE_GRAPH, 'plan_nodes, plan_edges' ::SAVE_TABLES;
SELECT field AS lt ::FORM, type, value FROM (VALUES ('lead_time', 'number', '5')) v(field, type, value);
SELECT 'Save parameters' ::SAVE, 'lt' ::SAVE_FORM, 'plan_params' ::SAVE_TABLES;
SELECT 'Bad' ::SAVE, 'graph' ::SAVE_GRAPH, 'x; DROP TABLE plan_nodes, y' ::SAVE_TABLES;
`;
writeFileSync(path.join(dashDir, "plan.sql"), PLAN);
writeFileSync(path.join(dashDir, "other.sql"), PLAN.replace("-- @title Plan", "-- @title Other"));
const sec = crypto.randomBytes(24).toString("hex");
const logPath = path.join(W, "changes.jsonl");
const opts = {
  auth: { mode: "proxy", shared_secret: sec, trusted_proxies: ["127.0.0.1"] },
  policy: "none",
  writeback: {
    path: logPath,
    dashboards: ["plan"],
    writers: { users: ["alice"] },
    // (d)'s refusals each cost a token (no free validation oracle); room for them here
    rate: { per_minute: 600, burst: 50 },
    tables: {
      plan_nodes: { key: ["id"], columns: { id: "VARCHAR", type: "VARCHAR", label: "VARCHAR", stage: "INTEGER" } },
      plan_edges: { key: ["from", "to"], columns: { from: "VARCHAR", to: "VARCHAR" } },
      plan_params: { key: ["id"], columns: { id: "VARCHAR", lead_time: "DOUBLE" } },
    },
  },
};
const SETUP = `CREATE TABLE plan_nodes AS SELECT * FROM (VALUES ('sup', 'supplier', 'Supplier', NULL::INTEGER), ('wh', 'stock', 'Warehouse', NULL)) v(id, type, label, stage);
CREATE TABLE plan_edges AS SELECT * FROM (VALUES ('sup', 'wh')) v("from", "to");
CREATE TABLE plan_params AS SELECT * FROM (VALUES ('sup', 7.0), ('wh', 3.0)) v(id, lead_time);`;

async function serve() {
  const p = spawn(duckdb, ["-unsigned", "-batch", ":memory:"], { stdio: ["pipe", "pipe", "pipe"], env });
  let log = "";
  p.stdout.on("data", (d) => (log += d));
  p.stderr.on("data", (d) => (log += d));
  p.stdin.write(`LOAD ${sqlStr(ext)};\n${SETUP}\nSELECT anofox_serve_dashboards(${sqlStr(dashDir)}, ${port}, ${sqlStr(JSON.stringify(opts))});\n`);
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
// The change log's records (the first line is its header).
const records = () =>
  readFileSync(logPath, "utf8")
    .split("\n")
    .filter(Boolean)
    .slice(1)
    .map((l) => JSON.parse(l));

let srv = null;
let browser = null;
try {
  srv = await serve();
  const header = JSON.parse(readFileSync(logPath, "utf8").split("\n")[0]);
  check(header.kind === "writeback", `the change log's header says what it is (${JSON.stringify(header)})`);
  browser = await chromium.launch();
  async function openAs(user, dash) {
    const ctx = await browser.newContext({ viewport: { width: 1300, height: 1000 }, reducedMotion: "reduce" });
    await ctx.setExtraHTTPHeaders({ "X-Anofox-Proxy-Secret": sec, "X-Forwarded-User": user });
    const page = await ctx.newPage();
    const errs = [];
    page.on("pageerror", (e) => errs.push(String(e)));
    await page.goto(`http://127.0.0.1:${port}/d/${dash}`);
    await page.waitForSelector(".save-panel .submit-btn");
    await page.waitForFunction(() => !document.body.classList.contains("loading") && !!document.querySelector(".dp-edit-bar"), null, { timeout: 15000 });
    return { ctx, page, errs };
  }
  const settle = async (page) => {
    await sleep(400);
    await page.waitForFunction(() => !document.body.classList.contains("loading"), null, { timeout: 15000 });
  };
  const btn = (page, label) => page.locator(`.save-panel .submit-btn:text-is("${label}")`);
  const panelText = (page, label) => page.locator(".save-panel", { has: page.locator(`.submit-btn:text-is("${label}")`) }).innerText();

  // (a) a writer saves an edited graph: one record replacing both tables.
  const { page, errs, ctx } = await openAs("alice", "plan");
  check(await btn(page, "Save plan").isEnabled(), "(a) a writer's Save is enabled");
  check(!(await page.locator(".save-panel .submit-preview-note").count()), "(a) no preview note on a server");
  await page.selectOption(".dp-edit-type", "stock");
  await page.fill(".dp-edit-name", "DC South");
  await page.click(".dp-edit-bar >> text=Add");
  await settle(page);
  await btn(page, "Save plan").click();
  await page.waitForFunction(() => /Saved at/.test(document.querySelector(".save-panel")?.textContent || ""), null, { timeout: 15000 });
  let r = records();
  const g = r[0]?.vars?.changes;
  check(
    r.length === 1 && r[0].user === "alice" && r[0].dashboard === "plan" && g?.[0]?.table === "plan_nodes" && g[0].op === "replace" && g[0].rows.length === 3 && g[1]?.table === "plan_edges",
    `(a) one change-log line: both tables replaced (${JSON.stringify(r[0])})`
  );
  check(g?.[0]?.rows.some((n) => n.label === "DC South" && n.type === "stock"), "(a) the added node is in the saved rows");

  // (b) a form save: one typed row keyed by the clicked node.
  await page.click('.dash rect[data-node="wh"]', { force: true });
  await settle(page);
  await page.fill('.dp-form-field[data-field="lead_time"] input', "9");
  await page.keyboard.press("Enter");
  await settle(page);
  await btn(page, "Save parameters").click();
  for (let k = 0; k < 50 && records().length < 2; k++) await sleep(100);
  r = records();
  const c1 = r[1]?.vars?.changes;
  check(
    r.length === 2 && c1.length === 1 && c1[0].table === "plan_params" && c1[0].op === "upsert" && c1[0].rows.length === 1 && c1[0].rows[0].id === "wh" && c1[0].rows[0].lead_time === 9,
    `(b) a form save upserts one typed row (${JSON.stringify(r[1]?.vars)})`
  );

  // (c) a table the server doesn't declare: refused, saying why; nothing written.
  await btn(page, "Bad").click();
  await page.waitForFunction(() => /not a table this server accepts saves into/.test(document.body.textContent), null, { timeout: 15000 }).catch(() => {});
  check(/not a table this server accepts saves into/.test(await panelText(page, "Bad")), `(c) an undeclared table is refused, saying why (${await panelText(page, "Bad")})`);
  check(records().length === 2, "(c) …and nothing was written");

  // (d) direct POSTs from the page (same origin): the gates hold.
  const post = (body, headers = { "X-Anofox-Submit": "1" }) =>
    page.evaluate(
      async ([b, h]) => {
        const res = await fetch("/api/save", { method: "POST", headers: { "Content-Type": "application/json", ...h }, body: JSON.stringify(b) });
        return { status: res.status, text: await res.text() };
      },
      [body, headers]
    );
  const formSave = { dashboard: "plan", panel: 3, vars: {}, form: { lead_time: 4 }, key: "sup" };
  let res = await post({ ...formSave, id: "aa01" }, {});
  check(res.status === 403 && /X-Anofox-Submit/.test(res.text), `(d) no X-Anofox-Submit header: 403 (${res.status} ${res.text})`);
  res = await post({ ...formSave, id: "aa02", form: { cost: 1 } });
  check(res.status === 400 && /'cost' is not a column of 'plan_params'/.test(res.text), `(d) an undeclared column: 400 (${res.status} ${res.text})`);
  res = await post({ ...formSave, id: "aa03", form: { lead_time: "fast" } });
  check(res.status === 400 && /expected a number/.test(res.text), `(d) a value of the wrong type: 400 (${res.status} ${res.text})`);
  res = await post({ ...formSave, id: "aa04", sql: "DROP TABLE plan_nodes" });
  check(res.status === 400 && /unknown request field 'sql'/.test(res.text), `(d) no SQL field: 400 (${res.status} ${res.text})`);
  res = await post({ ...formSave, id: "aa05", panel: 0 });
  check(res.status === 400 && /does not name a ::SAVE/.test(res.text), `(d) a panel that isn't a ::SAVE: 400 (${res.status} ${res.text})`);
  const first = await post({ ...formSave, id: "bb01" });
  const again = await post({ ...formSave, id: "bb01" });
  check(first.status === 201 && again.status === 201 && first.text === again.text && records().length === 3, `(d) a retry with the same id saves once (${first.text} / ${again.text}, ${records().length} records)`);
  check(errs.length === 0, `(a–d) no page errors${errs.length ? ": " + errs.join("; ") : ""}`);
  await ctx.close();

  // (e) a viewer who isn't a writer: disabled with the reason; a direct POST is 403.
  {
    const { page, ctx } = await openAs("bob", "plan");
    check(await btn(page, "Save plan").isDisabled(), "(e) a non-writer's Save is disabled");
    check(/Only this dashboard's writers can save/.test(await panelText(page, "Save plan")), "(e) …saying why");
    const res = await page.evaluate(async () => {
      const r = await fetch("/api/save", {
        method: "POST",
        headers: { "Content-Type": "application/json", "X-Anofox-Submit": "1" },
        body: JSON.stringify({ dashboard: "plan", panel: 3, id: "cc01", vars: {}, form: { lead_time: 1 }, key: "sup" }),
      });
      return { status: r.status, text: await r.text() };
    });
    check(res.status === 403 && /may not save/.test(res.text) && records().length === 3, `(e) a non-writer's POST: 403 (${res.status} ${res.text})`);
    await ctx.close();
  }
  // (f) a dashboard the server doesn't save for.
  {
    const { page, ctx } = await openAs("alice", "other");
    check(await btn(page, "Save plan").isDisabled(), "(f) a dashboard outside options.writeback: Save disabled");
    check(/Saving is not enabled on this server/.test(await panelText(page, "Save plan")), "(f) …saying why");
    await ctx.close();
  }
  const health = await (
    await fetch(`http://127.0.0.1:${port}/healthz`, { headers: { "X-Anofox-Proxy-Secret": sec, "X-Forwarded-User": "alice" } })
  ).json();
  check(health.writeback && health.writeback.ok === true && health.writeback.full === false, `/healthz reports the change log (${JSON.stringify(health.writeback)})`);
} finally {
  if (srv) stop(srv);
  if (browser) await browser.close();
  rmSync(W, { recursive: true, force: true });
}
console.log(`save-served-e2e: ${bad} failed`);
if (bad) process.exit(1);
