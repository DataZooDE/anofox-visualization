// ::SAVE (#684 slice 2) on a real locked server (options.writeback, proxy
// auth): a writer's Save posts /api/save and the change log gets one
// fdatasync'd line per save — an edited graph replaces both declared
// tables, a ::FORM save upserts one typed row keyed by the clicked node; a
// table the server doesn't declare is refused, saying why; a retry with the
// same id doesn't save twice; a viewer who isn't a writer, and a dashboard
// the server doesn't save for, get the button disabled with the reason; the
// endpoint's CSRF header and declared-columns checks hold for a direct POST.
// Slice 3: every panel reading a saved table reads base ⊕ saves — the
// writer's own page right after the save, another viewer's, and another
// dashboard's (one change log for the server); a restart reads the change
// log back; a base column the declaration doesn't name refuses the start.
// #726: under an access policy on another table the saved rows still show.
// #736: a policy naming a saved table filters its saved rows per viewer, a
// writer may only save rows (and columns) the policy lets them see.
// Slice 4, two writers: a save made on an older generation over someone
// else's newer save of the same row (or a replaced graph) is refused,
// naming who saved what; the panels then show theirs, the edits stay, and
// saving again keeps this one; a different row doesn't clash; the audit
// log names each save's tables, ops and keys.
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
SELECT count(*) AS n ::METRIC, 'Nodes' ::LABEL FROM plan_nodes;
SELECT string_agg(id || '=' || lead_time, ',' ORDER BY id) AS p ::TEXT_SMALL FROM plan_params;
`;
writeFileSync(path.join(dashDir, "plan.sql"), PLAN);
writeFileSync(path.join(dashDir, "other.sql"), PLAN.replace("-- @title Plan", "-- @title Other"));
const sec = crypto.randomBytes(24).toString("hex");
const logPath = path.join(W, "changes.jsonl");
const opts = {
  auth: { mode: "proxy", shared_secret: sec, trusted_proxies: ["127.0.0.1"] },
  policy: "none",
  audit: path.join(W, "audit.jsonl"),
  writeback: {
    path: logPath,
    dashboards: ["plan"],
    writers: { users: ["alice", "carol"] },
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

async function serve(setup = SETUP, o = opts) {
  const p = spawn(duckdb, ["-unsigned", "-batch", ":memory:"], { stdio: ["pipe", "pipe", "pipe"], env });
  let log = "";
  p.stdout.on("data", (d) => (log += d));
  p.stderr.on("data", (d) => (log += d));
  p.stdin.write(`LOAD ${sqlStr(ext)};\n${setup}\nSELECT anofox_serve_dashboards(${sqlStr(dashDir)}, ${port}, ${sqlStr(JSON.stringify(o))});\n`);
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
  const kpi = (page) => page.evaluate(() => document.querySelector(".metric-value")?.textContent);
  const params = (page) => page.evaluate(() => [...document.querySelectorAll(".textcard .text-value")].map((e) => e.textContent).join("|"));
  const until = async (page, f, want) => {
    for (let k = 0; k < 50 && (await f(page)) !== want; k++) await sleep(100);
    return f(page);
  };
  const panelText = (page, label) => page.locator(".save-panel", { has: page.locator(`.submit-btn:text-is("${label}")`) }).innerText();

  // (a) a writer saves an edited graph: one record replacing both tables.
  const { page, errs, ctx } = await openAs("alice", "plan");
  check((await kpi(page)) === "2" && (await params(page)) === "sup=7.0,wh=3.0", `(a) before any save, panels read the base tables (${await kpi(page)}, ${await params(page)})`);
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
  check((await until(page, kpi, "3")) === "3", `(a) the page re-runs on the saved plan_nodes (${await kpi(page)})`);

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
  check((await until(page, params, "sup=7.0,wh=9.0")) === "sup=7.0,wh=9.0", `(b) the saved row is merged over the base by key (${await params(page)})`);

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
  const formSave = { dashboard: "plan", panel: 3, vars: {}, form: { lead_time: 4 }, key: "sup", base: 0 };
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
    check((await kpi(page)) === "3" && (await params(page)) === "sup=4.0,wh=9.0", `(e) another viewer reads the saves (${await kpi(page)}, ${await params(page)})`);
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
    check((await kpi(page)) === "3", `(f) another dashboard reading plan_nodes reads the saves too (${await kpi(page)})`);
    check(await btn(page, "Save plan").isDisabled(), "(f) a dashboard outside options.writeback: Save disabled");
    check(/Saving is not enabled on this server/.test(await panelText(page, "Save plan")), "(f) …saying why");
    await ctx.close();
  }
  // (i) two writers. Each page loads at the current generation.
  {
    const a = await openAs("alice", "plan");
    const c = await openAs("carol", "plan");
    const saveLt = async (page, node, v) => {
      await page.click(`.dash rect[data-node="${node}"]`, { force: true });
      await settle(page);
      await page.fill('.dp-form-field[data-field="lead_time"] input', String(v));
      await page.keyboard.press("Enter");
      await settle(page);
      await btn(page, "Save parameters").click();
      await page.waitForFunction(
        () => /Saved at|Changed meanwhile|failed|Couldn/.test([...document.querySelectorAll(".save-panel")].map((f) => f.textContent).join(" ")),
        null,
        { timeout: 15000 }
      );
      await settle(page);
      return panelText(page, "Save parameters");
    };
    const n0 = records().length;
    let t = await saveLt(a.page, "wh", 11);
    check(/Saved at/.test(t) && records().length === n0 + 1, `(i) alice saves wh (${t})`);
    t = await saveLt(c.page, "wh", 12);
    check(/Changed meanwhile: alice saved plan_params \(wh\)/.test(t) && records().length === n0 + 1, `(i) carol's stale save of wh is refused, naming alice's (${t})`);
    check((await until(c.page, params, "sup=4.0,wh=11.0")) === "sup=4.0,wh=11.0", `(i) …and carol's panels now show alice's value (${await params(c.page)})`);
    await btn(c.page, "Save parameters").click();
    await c.page.waitForFunction(() => /Saved at/.test(document.querySelectorAll(".save-panel")[1]?.textContent || ""), null, { timeout: 15000 }).catch(() => {});
    check(records().length === n0 + 2 && (await until(c.page, params, "sup=4.0,wh=12.0")) === "sup=4.0,wh=12.0", `(i) saving again keeps carol's (${await params(c.page)}; ${records().length - n0} new; ${await panelText(c.page, "Save parameters")}; last ${JSON.stringify(records().at(-1)?.vars)})`);
    // A different row on a stale page: no clash.
    const c2 = await openAs("carol", "plan");
    const a2 = await openAs("alice", "plan");
    t = await saveLt(a2.page, "wh", 13);
    check(/Saved at/.test(t), `(i) alice, on a fresh page, saves wh again (${t})`);
    await a2.ctx.close();
    t = await saveLt(c2.page, "sup", 6);
    check(/Saved at/.test(t), `(i) a stale page saving another row doesn't clash (${t})`);
    // A graph: replaced whole, so any newer change to its tables clashes.
    const g1 = await openAs("alice", "plan");
    const g2 = await openAs("carol", "plan");
    for (const p of [g1.page, g2.page]) {
      await p.selectOption(".dp-edit-type", "stock");
      await p.fill(".dp-edit-name", "DC " + (p === g1.page ? "East" : "West"));
      await p.click(".dp-edit-bar >> text=Add");
      await settle(p);
    }
    await btn(g1.page, "Save plan").click();
    await g1.page.waitForFunction(() => /Saved at/.test(document.querySelector(".save-panel")?.textContent || ""), null, { timeout: 15000 });
    await btn(g2.page, "Save plan").click();
    await g2.page.waitForFunction(() => /Changed meanwhile/.test(document.querySelector(".save-panel")?.textContent || ""), null, { timeout: 15000 }).catch(() => {});
    t = await panelText(g2.page, "Save plan");
    check(/Changed meanwhile: alice saved plan_nodes/.test(t), `(i) a stale graph save is refused (${t})`);
    for (const x of [a, c, c2, g1, g2]) await x.ctx.close();
    const audit = readFileSync(path.join(W, "audit.jsonl"), "utf8").split("\n").filter(Boolean).map((l) => JSON.parse(l)).filter((l) => l.audit === "save");
    check(audit.some((l) => l.outcome === "ok" && l.user === "carol" && /plan_params:upsert\(wh\)/.test(l.detail || "")), `(i) the audit log names carol's save of plan_params (wh)`);
    check(audit.some((l) => l.outcome === "deny" && l.reason === "conflict" && l.user === "carol"), "(i) …and the refused one, as a conflict");
  }

  const health = await (
    await fetch(`http://127.0.0.1:${port}/healthz`, { headers: { "X-Anofox-Proxy-Secret": sec, "X-Forwarded-User": "alice" } })
  ).json();
  check(health.writeback && health.writeback.ok === true && health.writeback.full === false, `/healthz reports the change log (${JSON.stringify(health.writeback)})`);

  // (g) a restart reads the change log back: the saves are still there.
  stop(srv);
  srv = null;
  await sleep(300);
  srv = await serve();
  {
    const { page, ctx } = await openAs("bob", "plan");
    check((await params(page)) === "sup=6.0,wh=13.0", `(g) after a restart the saves still show (${await params(page)})`);
    await ctx.close();
  }
  stop(srv);
  srv = null;
  await sleep(300);
  // (h) a base column the declaration doesn't name: refused at start.
  let refused = "";
  try {
    srv = await serve(SETUP.replace("SELECT * FROM (VALUES ('sup', 7.0), ('wh', 3.0)) v(id, lead_time)", "SELECT *, 1 AS cost FROM (VALUES ('sup', 7.0), ('wh', 3.0)) v(id, lead_time)"));
  } catch (e) {
    refused = String(e.message);
  }
  check(/plan_params: the table has columns the declaration doesn't name \(cost\)/.test(refused), `(h) an undeclared base column refuses the start (${refused.split("\n").find((l) => /writeback/.test(l)) || refused.slice(0, 200)})`);

  // (k) #726: with an access policy that doesn't name the saved tables, the
  // server starts and every viewer of the dashboards reads them in full —
  // the start line says so.
  const policyFile = (tables) => {
    const f = path.join(W, `policy-${Object.keys(tables).join("-")}.json`);
    writeFileSync(f, JSON.stringify({ tables }));
    return f;
  };
  const ORDERS = SETUP + "\nCREATE TABLE orders AS SELECT * FROM (VALUES ('alice', 1), ('bob', 2)) v(owner, n);";
  srv = await serve(ORDERS, { ...opts, policy: policyFile({ orders: { rows: "owner = getvariable('__claim_user')" } }) });
  {
    check(/saved table\(s\) plan_edges, plan_nodes, plan_params are not under the policy/.test(srv.log()), "(k) the start line says the saved tables are outside the policy");
    const { page, ctx } = await openAs("bob", "plan");
    check((await params(page)) === "sup=6.0,wh=13.0", `(k) under a policy on another table the saved rows still show (${await params(page)})`);
    await ctx.close();
    // a writer's save works under the policy too, and the panels read it
    const w = await openAs("alice", "plan");
    const gen = await w.page.evaluate(() => window.__served?.writeback?.gen);
    const r = await w.page.evaluate(async (base) => {
      const res = await fetch("/api/save", {
        method: "POST",
        headers: { "Content-Type": "application/json", "X-Anofox-Submit": "1" },
        body: JSON.stringify({ dashboard: "plan", panel: 3, id: "dd01", vars: {}, form: { lead_time: 2 }, key: "sup", base }),
      });
      return { status: res.status, text: await res.text() };
    }, gen ?? 0);
    check(r.status === 201, `(k) a writer's save under the policy is accepted (${r.status} ${r.text})`);
    const b2 = await openAs("bob", "plan");
    check((await until(b2.page, params, "sup=2.0,wh=13.0")) === "sup=2.0,wh=13.0", `(k) …and another viewer reads it (${await params(b2.page)})`);
    await w.ctx.close();
    await b2.ctx.close();
  }
  stop(srv);
  srv = null;
  await sleep(300);
  // (l) #736: a policy naming a saved table applies to its saved rows — a
  // viewer sees only the rows its rule lets through, and a writer may only
  // save rows they can see (before and after the save).
  const saveAs = async (user, form, key, id) => {
    const w = await openAs(user, "plan");
    const gen = await w.page.evaluate(() => window.__served?.writeback?.gen);
    const r = await w.page.evaluate(
      async ([b]) => {
        const res = await fetch("/api/save", { method: "POST", headers: { "Content-Type": "application/json", "X-Anofox-Submit": "1" }, body: JSON.stringify(b) });
        return { status: res.status, text: await res.text() };
      },
      [{ dashboard: "plan", panel: 3, id, vars: {}, form, key, base: gen ?? 0 }]
    );
    await w.ctx.close();
    return r;
  };
  const seen = async (user) => {
    const v = await openAs(user, "plan");
    const p = await params(v.page);
    await v.ctx.close();
    return p;
  };
  srv = await serve(ORDERS, { ...opts, policy: policyFile({ plan_params: { rows: "id = 'sup' OR getvariable('__claim_user') = 'carol'" } }) });
  {
    check(/saved table\(s\) plan_edges, plan_nodes are not under the policy/.test(srv.log()), "(l) the start line no longer lists the policy's own saved table");
    check((await seen("bob")) === "sup=2.0", `(l) bob sees only the rows the rule lets through (${await seen("bob")})`);
    check((await seen("carol")) === "sup=2.0,wh=13.0", `(l) carol sees every row (${await seen("carol")})`);
    let r = await saveAs("alice", { lead_time: 3 }, "sup", "ee01");
    check(r.status === 201, `(l) alice saves a row she can see (${r.status} ${r.text})`);
    check((await seen("bob")) === "sup=3.0", `(l) …and bob reads it through the policy (${await seen("bob")})`);
    r = await saveAs("alice", { lead_time: 1 }, "wh", "ee02");
    check(r.status === 403 && /rows of 'plan_params' you can see/.test(r.text), `(l) alice can't save a row the policy hides from her (${r.status} ${r.text})`);
    r = await saveAs("alice", { lead_time: 1 }, "zz", "ee03");
    check(r.status === 403 && /rows of 'plan_params' you can see/.test(r.text), `(l) …nor make one she couldn't see (${r.status} ${r.text})`);
    r = await saveAs("carol", { lead_time: 12 }, "wh", "ee04");
    check(r.status === 201, `(l) carol may save it (${r.status} ${r.text})`);
    check((await seen("carol")) === "sup=3.0,wh=12.0", `(l) …and sees it (${await seen("carol")})`);
  }
  stop(srv);
  srv = null;
  await sleep(300);
  // (m) a column the policy restricts for the writer may not be saved.
  srv = await serve(ORDERS, { ...opts, policy: policyFile({ plan_params: { rows: true, columns: { lead_time: { allow_users: ["carol"], else: "null" } } } }) });
  {
    const r = await saveAs("alice", { lead_time: 5 }, "sup", "ff01");
    check(r.status === 403 && /'plan_params.lead_time'/.test(r.text), `(m) alice can't save a column the policy restricts for her (${r.status} ${r.text})`);
    const r2 = await saveAs("carol", { lead_time: 5 }, "sup", "ff02");
    check(r2.status === 201, `(m) carol, allowed the column, can (${r2.status} ${r2.text})`);
    check(/sup=5\.0/.test(await seen("carol")) && !/5\.0/.test(await seen("bob")), `(m) the column rule applies to saved values too: carol sees 5.0, bob doesn't (${await seen("carol")} / ${await seen("bob")})`);
  }
  stop(srv);
  srv = null;
  await sleep(300);
  // (n) a policy naming the saved table in another schema: ambiguous, refused.
  refused = "";
  try {
    srv = await serve(ORDERS + "\nCREATE SCHEMA other; CREATE TABLE other.plan_params AS SELECT * FROM plan_params;", { ...opts, policy: policyFile({ "other.plan_params": { rows: "true" } }) });
  } catch (e) {
    refused = String(e.message);
  }
  check(/names 'plan_params' in schema 'other'/.test(refused), `(n) a policy on the saved table's name in another schema refuses the start (${refused.split("\n").find((l) => /writeback/.test(l)) || refused.slice(0, 200)})`);
} finally {
  if (srv) stop(srv);
  if (browser) await browser.close();
  rmSync(W, { recursive: true, force: true });
}
console.log(`save-served-e2e: ${bad} failed`);
if (bad) process.exit(1);
