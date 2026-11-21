// The clarify-the-graph answers (#1282) on a real locked server (#1317):
// options.writeback with `"stamp": {"who": "user", "when": "ts"}` and an
// approver-only column (`"approver_only": ["decides"]`,
// options.writeback.approvers), proxy logins.
//
// - two experts answer one question through the page: each keeps a row of
//   their own (the stamped login is part of the key), `who` is the login
//   and `when` the server's clock;
// - a forged who / when in the request is overwritten by the server;
// - a writer who isn't an approver sees "settle the disagreement" disabled,
//   saying why, and a direct POST setting it is refused (403), nothing
//   written; leaving it unticked saves;
// - the stored row decides: a non-approver can't unsettle a settled row
//   (false / null / blank over a stored true: 403, it stays true), may
//   resend the stored value or leave the column out;
// - an approver ticks it and saves; the row says so;
// - an approver who isn't a writer can't save at all.
//
//   NODE_PATH=<dir with playwright>/node_modules node scripts/clarify-served-e2e.mjs \
//       --ext path/to/anofox_visualization.duckdb_extension [--port 8979] [--duckdb duckdb] [--shots dir]
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
const port = Number(arg("--port", "8979"));
const duckdb = arg("--duckdb", "duckdb");
const shots = arg("--shots", "");
if (!ext || !existsSync(ext)) {
  console.error("usage: clarify-served-e2e.mjs --ext <anofox_visualization.duckdb_extension> (build it with duckext/scripts/build-native.sh)");
  process.exit(2);
}
let bad = 0;
const check = (ok, m) => (ok ? console.log("ok   " + m) : (bad++, console.error("FAIL " + m)));
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const sqlStr = (s) => `'${s.replace(/'/g, "''")}'`;
const env = { ...process.env, DATAZOO_DISABLE_TELEMETRY: "1" };

// Scratch next to the extension (the change log's path rules refuse a
// world-writable parent).
const W = mkdtempSync(path.join(path.dirname(path.resolve(ext)), "clarify-served-e2e-"));
const dashDir = path.join(W, "dash");
mkdirSync(dashDir, { recursive: true });
// The example's answer card, cut down: the question picker, the card, its
// Submit (statement 2) and the answers.
writeFileSync(
  path.join(dashDir, "clarify.sql"),
  `-- @title Clarify
SELECT q ::DROPDOWN, text ::OPTION_LABEL, 'Question' ::LABEL FROM qs ORDER BY q;
SELECT field AS ans ::FORM, type, value, options, labels, label, help, 'Does smoking lead to lung cancer?' ::TITLE
FROM (
  SELECT 1 AS ord, 'answer' AS field, 'buttons' AS type, NULL AS value, ['a_to_b', 'b_to_a'] AS options,
         ['Smoker → Lung cancer', 'Lung cancer → Smoker'] AS labels, 'Your answer' AS label, NULL AS help
  UNION ALL SELECT 2, 'rationale', 'text', NULL, NULL, NULL, 'Why', NULL
  UNION ALL SELECT 3, 'decides', 'toggle', 'false', NULL, NULL, 'Settle the disagreement', 'As the approver: this answer decides the question'
) ORDER BY ord;
SELECT 'Submit answer' ::SAVE, 'ans' ::SAVE_FORM, 'answers' ::SAVE_TABLES, 'q' ::SAVE_KEY;
SELECT string_agg(id || ':' || who || '=' || answer || CASE WHEN decides THEN '!' ELSE '' END, ',' ORDER BY id, who) AS a ::TEXT_SMALL FROM answers;
`
);
const sec = crypto.randomBytes(24).toString("hex");
const logPath = path.join(W, "changes.jsonl");
const opts = {
  auth: { mode: "proxy", shared_secret: sec, trusted_proxies: ["127.0.0.1"] },
  policy: "none",
  writeback: {
    path: logPath,
    dashboards: ["clarify"],
    writers: { users: ["dr-a", "dr-b"] },
    approvers: { users: ["dr-b", "carol"] },
    rate: { per_minute: 600, burst: 50 },
    tables: {
      answers: {
        key: ["id", "who"],
        columns: { id: "VARCHAR", answer: "VARCHAR", rationale: "VARCHAR", who: "VARCHAR", when: "VARCHAR", decides: "BOOLEAN" },
        stamp: { who: "user", when: "ts" },
        approver_only: ["decides"],
      },
    },
  },
};
const SETUP = `CREATE TABLE qs AS SELECT * FROM (VALUES ('smoke|lung', 'Does smoking lead to lung cancer?')) v(q, text);
CREATE TABLE answers (id VARCHAR, answer VARCHAR, rationale VARCHAR, who VARCHAR, "when" VARCHAR, decides BOOLEAN);
-- dr-a's earlier answer, settled back when dr-a was an approver
INSERT INTO answers VALUES ('smoke|bronc', 'b_to_a', 'Our clinic', 'dr-a', '2026-11-01T09:00:00Z', true);`;

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
const records = () =>
  readFileSync(logPath, "utf8")
    .split("\n")
    .filter(Boolean)
    .slice(1)
    .map((l) => JSON.parse(l));
const rowOf = (rec) => rec?.vars?.changes?.[0]?.rows?.[0];

let srv = null;
let browser = null;
try {
  srv = await serve();
  browser = await chromium.launch();
  async function openAs(user, scheme = "light") {
    const ctx = await browser.newContext({ viewport: { width: 1100, height: 900 }, reducedMotion: "reduce", colorScheme: scheme });
    await ctx.setExtraHTTPHeaders({ "X-Anofox-Proxy-Secret": sec, "X-Forwarded-User": user });
    const page = await ctx.newPage();
    const errs = [];
    page.on("pageerror", (e) => errs.push(String(e)));
    await page.goto(`http://127.0.0.1:${port}/d/clarify`);
    await page.waitForSelector(".save-panel .submit-btn");
    await page.waitForSelector('.dp-form-field[data-field="decides"]');
    await page.waitForFunction(() => !document.body.classList.contains("loading"), null, { timeout: 15000 });
    return { ctx, page, errs };
  }
  const settle = async (page) => {
    await sleep(400);
    await page.waitForFunction(() => !document.body.classList.contains("loading"), null, { timeout: 15000 });
  };
  const toggle = (page) => page.locator('.dp-form-field[data-field="decides"] input');
  const shown = (page) => page.evaluate(() => document.querySelector(".textcard .text-value")?.textContent || "");
  const until = async (page, want) => {
    for (let k = 0; k < 50 && !want.test(await shown(page)); k++) await sleep(100);
    return shown(page);
  };
  const post = (page, body) =>
    page.evaluate(async (b) => {
      const res = await fetch("/api/save", { method: "POST", headers: { "Content-Type": "application/json", "X-Anofox-Submit": "1" }, body: JSON.stringify(b) });
      return { status: res.status, text: await res.text() };
    }, body);
  const save = async (page) => {
    await page.click('.save-panel .submit-btn:text-is("Submit answer")');
    await page.waitForFunction(() => /Saved at|forbidden|failed|Couldn/.test(document.querySelector(".save-panel")?.textContent || ""), null, { timeout: 15000 });
    await settle(page);
    return page.locator(".save-panel").innerText();
  };
  const answer = async (page, option, why) => {
    await page.click(`.dp-form-field[data-field="answer"] button[data-option="${option}"]`);
    await settle(page);
    await page.fill('.dp-form-field[data-field="rationale"] input', why);
    await page.keyboard.press("Enter");
    await settle(page);
  };
  const t0 = Date.now();
  const serverTime = (r) => {
    const t = Date.parse(r?.when);
    return Number.isFinite(t) && t >= t0 - 60000 && t <= Date.now() + 60000;
  };

  // (a) dr-a, a writer but no approver: the settle toggle is disabled, saying why.
  const a = await openAs("dr-a");
  check(await toggle(a.page).isDisabled(), "(a) a non-approver's 'settle' toggle is disabled");
  check(
    /Only an approver can set this/.test(await a.page.locator('.dp-form-field[data-field="decides"]').innerText()),
    "(a) …saying why"
  );
  check(await a.page.locator('.dp-form-field[data-field="answer"] button').first().isEnabled(), "(a) the answer buttons stay enabled");
  await answer(a.page, "a_to_b", "Cohort studies");
  let t = await save(a.page);
  let r = records();
  check(/Saved at/.test(t) && r.length === 1, `(a) dr-a answers without settling: saved (${t})`);
  check(
    rowOf(r[0])?.who === "dr-a" && rowOf(r[0])?.id === "smoke|lung" && rowOf(r[0])?.answer === "a_to_b" && rowOf(r[0])?.decides === false && serverTime(rowOf(r[0])),
    `(a) the row: who is the login, when the server's clock (${JSON.stringify(rowOf(r[0]))})`
  );
  check(/smoke\|lung:dr-a=a_to_b/.test(await until(a.page, /smoke\|lung:dr-a=a_to_b/)), `(a) the page shows the answer (${await shown(a.page)})`);

  // (b) the stamp can't be forged: a direct POST naming someone else, long ago.
  const base = records().length + 10;
  const body = { dashboard: "clarify", panel: 2, vars: { q: "smoke|lung" }, key: "smoke|lung", base };
  let res = await post(a.page, { ...body, id: "b001", form: { answer: "b_to_a", who: "dr-b", when: "1999-01-01T00:00:00Z" } });
  r = records();
  check(res.status === 201 && r.length === 2, `(b) a save with a forged who/when is accepted (${res.status} ${res.text})`);
  check(rowOf(r[1])?.who === "dr-a" && serverTime(rowOf(r[1])) && rowOf(r[1])?.when !== "1999-01-01T00:00:00Z", `(b) …but who/when are the server's (${JSON.stringify(rowOf(r[1]))})`);

  // (c) dr-a can't settle: a direct POST with decides is 403, nothing written.
  for (const [id, v] of [["c001", true], ["c002", "true"]]) {
    res = await post(a.page, { ...body, id, form: { answer: "b_to_a", decides: v } });
    check(
      res.status === 403 && /only approvers may change answers\.decides/.test(res.text) && records().length === 2,
      `(c) a non-approver's settle (${JSON.stringify(v)}) is refused, nothing written (${res.status} ${res.text})`
    );
  }
  res = await post(a.page, { ...body, id: "c003", form: { answer: "b_to_a", decides: false } });
  check(res.status === 201 && records().length === 3, `(c) …unticked it saves (${res.status} ${res.text})`);
  // (c2) the stored row decides, not the request: dr-a can't unsettle the
  // settled base row, can resend its value, and can save without it.
  const bronc = { ...body, key: "smoke|bronc", vars: { q: "smoke|bronc" } };
  for (const [id, v] of [["c101", false], ["c102", null], ["c103", "false"]]) {
    res = await post(a.page, { ...bronc, id, form: { answer: "a_to_b", decides: v } });
    check(
      res.status === 403 && /only approvers may change answers\.decides/.test(res.text) && records().length === 3,
      `(c2) a non-approver's ${JSON.stringify(v)} over a settled true is refused, nothing written (${res.status} ${res.text})`
    );
  }
  res = await post(a.page, { ...bronc, id: "c104", form: { answer: "b_to_a", decides: true } });
  check(res.status === 201 && records().length === 4, `(c2) …resending the stored true saves (${res.status} ${res.text})`);
  res = await post(a.page, { ...bronc, id: "c105", form: { rationale: "Still our clinic" } });
  check(res.status === 201 && records().length === 5, `(c2) …and a save without the column saves (${res.status} ${res.text})`);
  check(/smoke\|bronc:dr-a=b_to_a!/.test(await until(a.page, /smoke\|bronc:dr-a=b_to_a!/)), `(c2) the stored value stays true (${await shown(a.page)})`);
  check(a.errs.length === 0, `(a–c) no page errors${a.errs.length ? ": " + a.errs.join("; ") : ""}`);

  // (d) dr-b, a writer and an approver: settles from the page.
  const b = await openAs("dr-b");
  check(await toggle(b.page).isEnabled(), "(d) an approver's 'settle' toggle is enabled");
  check(!(await b.page.locator(".dp-form-approver-only").count()), "(d) …with no 'only an approver' note");
  await answer(b.page, "b_to_a", "Our clinic");
  await toggle(b.page).check();
  await settle(b.page);
  t = await save(b.page);
  r = records();
  check(/Saved at/.test(t) && r.length === 6, `(d) dr-b settles: saved (${t})`);
  check(
    rowOf(r[5])?.who === "dr-b" && rowOf(r[5])?.decides === true && rowOf(r[5])?.answer === "b_to_a" && serverTime(rowOf(r[5])),
    `(d) the row: dr-b, settled, the server's time (${JSON.stringify(rowOf(r[5]))})`
  );
  const both = await until(b.page, /smoke\|lung:dr-a=b_to_a,smoke\|lung:dr-b=b_to_a!/);
  check(/smoke\|lung:dr-a=b_to_a,smoke\|lung:dr-b=b_to_a!$/.test(both), `(d) two experts, two rows of one question (${both})`);
  check(b.errs.length === 0, `(d) no page errors${b.errs.length ? ": " + b.errs.join("; ") : ""}`);
  if (shots) {
    mkdirSync(shots, { recursive: true });
    await a.page.reload();
    await a.page.waitForSelector('.dp-form-field[data-field="decides"]');
    await settle(a.page);
    await a.page.screenshot({ path: path.join(shots, "clarify-served-non-approver-light.png"), fullPage: true });
    await b.page.screenshot({ path: path.join(shots, "clarify-served-approver-light.png"), fullPage: true });
    const d = await openAs("dr-a", "dark");
    await d.page.screenshot({ path: path.join(shots, "clarify-served-non-approver-dark.png"), fullPage: true });
    await d.ctx.close();
  }
  await a.ctx.close();
  await b.ctx.close();

  // (e) carol approves but doesn't write: no saving at all.
  {
    const c = await openAs("carol");
    check(await c.page.locator('.save-panel .submit-btn:text-is("Submit answer")').isDisabled(), "(e) an approver who isn't a writer can't save");
    res = await post(c.page, { ...body, id: "e001", form: { answer: "a_to_b", decides: true } });
    check(res.status === 403 && /may not save/.test(res.text) && records().length === 6, `(e) …a direct POST is 403 (${res.status} ${res.text})`);
    await c.ctx.close();
  }
} finally {
  if (srv) {
    srv.proc.stdin.end();
    srv.proc.kill(); // our own child (pid known), never by name
  }
  if (browser) await browser.close();
  rmSync(W, { recursive: true, force: true });
}
console.log(`clarify-served-e2e: ${bad} failed`);
if (bad) process.exit(1);
