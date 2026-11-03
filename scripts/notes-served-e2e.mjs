// Notes on charts on a locked server (#889, options.notes, proxy auth): a
// writer points at a value, picks ✎ Add a note in the chart's tools and
// types the text; every viewer of the dashboard then sees it on the chart (a
// rule with the text, author and day as its tooltip, escaped), a viewer who
// isn't a writer gets no ✎ and can't post one; the author or a moderator
// deletes it, nobody else; the endpoint holds the cross-site gate, checks the
// panel and the text, refuses dashboards without notes, and reads the file
// back after a restart; options.notes without a login refuses to start.
//
//   NODE_PATH=<dir with playwright>/node_modules node scripts/notes-served-e2e.mjs \
//       --ext path/to/anofox_visualization.duckdb_extension [--port 8969] [--duckdb duckdb]
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
const port = Number(arg("--port", "8969"));
const duckdb = arg("--duckdb", "duckdb");
if (!ext || !existsSync(ext)) {
  console.error("usage: notes-served-e2e.mjs --ext <anofox_visualization.duckdb_extension> (build it with duckext/scripts/build-native.sh)");
  process.exit(2);
}
let bad = 0;
const check = (ok, m) => (ok ? console.log("ok   " + m) : (bad++, console.error("FAIL " + m)));
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const sqlStr = (s) => `'${s.replace(/'/g, "''")}'`;
const env = { ...process.env, DATAZOO_DISABLE_TELEMETRY: "1" };

const W = mkdtempSync(path.join(path.dirname(path.resolve(ext)), "notes-served-e2e-"));
const dashDir = path.join(W, "dash");
mkdirSync(dashDir, { recursive: true });
const SALES = `-- @title Sales
SELECT d ::XAXIS, units ::LINECHART, 'Units' ::TITLE FROM sales ORDER BY d;
`;
writeFileSync(path.join(dashDir, "sales.sql"), SALES);
writeFileSync(path.join(dashDir, "other.sql"), SALES.replace("-- @title Sales", "-- @title Other"));
const SETUP = `CREATE TABLE sales AS SELECT DATE '2026-02-01' + i::INTEGER AS d, (3 + i % 4)::DOUBLE AS units FROM range(7) t(i);`;
const sec = crypto.randomBytes(24).toString("hex");
const notesPath = path.join(W, "notes.jsonl");
const auth = { mode: "proxy", shared_secret: sec, trusted_proxies: ["127.0.0.1"] };
const opts = {
  auth,
  policy: "none",
  notes: { path: notesPath, dashboards: ["sales"], writers: { users: ["alice", "carol"] }, moderators: { users: ["carol"] } },
};

function spawnServer(o) {
  const p = spawn(duckdb, ["-unsigned", "-batch", ":memory:"], { stdio: ["pipe", "pipe", "pipe"], env });
  let log = "";
  p.stdout.on("data", (d) => (log += d));
  p.stderr.on("data", (d) => (log += d));
  p.stdin.write(`LOAD ${sqlStr(ext)};\n${SETUP}\nSELECT anofox_serve_dashboards(${sqlStr(dashDir)}, ${port}, ${sqlStr(JSON.stringify(o))});\n`);
  return { proc: p, log: () => log };
}
async function serve(o = opts) {
  const s = spawnServer(o);
  for (let i = 0; i < 100; i++) {
    try {
      if ((await fetch(`http://127.0.0.1:${port}/healthz`)).ok) return s;
    } catch (_) {}
    if (s.proc.exitCode != null) break;
    await sleep(100);
  }
  s.proc.kill();
  throw new Error(`the server did not start:\n${s.log()}`);
}
const stop = async (s) => {
  s.proc.stdin.end();
  s.proc.kill(); // our own child (pid known), never by name
  for (let i = 0; i < 50 && s.proc.exitCode == null && s.proc.signalCode == null; i++) await sleep(100);
};
async function refuses(o, re, what) {
  const s = spawnServer(o);
  for (let i = 0; i < 100 && !re.test(s.log()) && s.proc.exitCode == null; i++) await sleep(100);
  await sleep(200);
  check(re.test(s.log()), what);
  await stop(s);
}
const BASE = `http://127.0.0.1:${port}`;
async function api(user, method, p, body, extra = {}) {
  const headers = { "X-Anofox-Proxy-Secret": sec, "X-Forwarded-User": user };
  if (body !== undefined) Object.assign(headers, { "Content-Type": "application/json", Origin: BASE, "X-Anofox-Notes": "1" });
  Object.assign(headers, extra);
  for (const [k, v] of Object.entries(headers)) if (v === null) delete headers[k];
  const r = await fetch(BASE + p, { method, headers, body: body === undefined ? undefined : JSON.stringify(body) });
  const text = await r.text();
  let json = null;
  try {
    json = JSON.parse(text);
  } catch (_) {}
  return { status: r.status, text, json };
}
const notes = async (user, dash = "sales") => (await api(user, "GET", `/api/notes?dashboard=${dash}`)).json?.notes ?? null;

let srv = null;
let browser = null;
try {
  await refuses({ policy: "none", notes: opts.notes }, /options\.notes needs options\.auth/, "options.notes without a login refuses to start");
  await refuses({ ...opts, notes: { ...opts.notes, dashboards: ["nope"] } }, /names dashboard 'nope'/, "a dashboard the server doesn't serve refuses to start");

  srv = await serve();
  check(JSON.parse(readFileSync(notesPath, "utf8").split("\n")[0]).kind === "notes", "the file's header says what it is");

  browser = await chromium.launch();
  async function openAs(user) {
    const ctx = await browser.newContext({ viewport: { width: 1200, height: 900 }, reducedMotion: "reduce" });
    await ctx.setExtraHTTPHeaders({ "X-Anofox-Proxy-Secret": sec, "X-Forwarded-User": user });
    const page = await ctx.newPage();
    const errs = [];
    page.on("pageerror", (e) => errs.push(String(e)));
    await page.goto(BASE + "/d/sales/");
    await page.waitForFunction(() => !document.body.classList.contains("loading") && !!document.querySelector(".panel svg [data-x]"), null, { timeout: 20000 });
    return { ctx, page, errs };
  }
  const tool = (page) => page.evaluate(() => !!document.querySelector('.dp-tool[title="Add a note"]'));
  const noteTips = (page) =>
    page.evaluate(() => [...document.querySelectorAll(".panel svg [data-note]")].map((m) => m.getAttribute("data-tip") || m.querySelector("title")?.textContent || ""));

  // ── a writer adds one from the page ──
  const a = await openAs("alice");
  check(await tool(a.page), "a writer gets ✎ Add a note in the chart tools");
  await a.page.hover('.panel svg [data-x="2026-02-04"]');
  await a.page.evaluate(() => document.querySelector('.dp-tool[title="Add a note"]').click());
  await a.page.waitForSelector(".dp-dialog input");
  check(/At 2026-02-04/.test(await a.page.textContent(".dp-dialog")), "the dialog names the value pointed at");
  await a.page.fill(".dp-dialog input", "Promotion <b>week</b>");
  await a.page.click('.dp-dialog button:text-is("Add")');
  await a.page.waitForFunction(() => !!document.querySelector(".panel svg [data-note]"), null, { timeout: 10000 });
  let tips = await noteTips(a.page);
  check(tips.length === 1 && tips[0].startsWith("Promotion <b>week</b> — alice, 20"), `the chart draws it with its text, author and day (${tips})`);
  check(!(await a.page.evaluate(() => !!document.querySelector(".panel svg b"))), "the text stays text (no markup)");
  check(a.errs.length === 0, `no page error (${a.errs})`);
  const lines = readFileSync(notesPath, "utf8").split("\n").filter(Boolean).slice(1).map((l) => JSON.parse(l));
  check(lines.length === 1 && lines[0].submit === "note" && lines[0].vars.x === "2026-02-04" && lines[0].vars.panel === 0, `one line in the file (${JSON.stringify(lines)})`);

  // ── another viewer sees it, can't add or delete ──
  const b = await openAs("bob");
  await b.page.waitForFunction(() => !!document.querySelector(".panel svg [data-note]"), null, { timeout: 10000 });
  tips = await noteTips(b.page);
  check(tips.length === 1 && tips[0].startsWith("Promotion"), "another viewer sees the note");
  check(!(await tool(b.page)), "a viewer who isn't a writer gets no ✎");
  await b.page.evaluate(() => document.querySelector(".panel svg [data-note]").dispatchEvent(new MouseEvent("click", { bubbles: true })));
  await b.page.waitForSelector(".dp-dialog");
  const btns = await b.page.$$eval(".dp-dialog button", (bs) => bs.map((x) => x.textContent));
  check(!btns.includes("Delete"), `bob's note dialog has no Delete (${btns})`);

  // ── the endpoint ──
  let r = await api("bob", "POST", "/api/notes", { dashboard: "sales", panel: 0, x: "2026-02-02", text: "x" });
  check(r.status === 403 && /writers/.test(r.text), `a non-writer can't add (${r.status} ${r.text})`);
  r = await api("alice", "POST", "/api/notes", { dashboard: "sales", panel: 0, x: "2026-02-02", text: "x" }, { "X-Anofox-Notes": null });
  check(r.status === 403, `a write without the custom header is refused (${r.status})`);
  r = await api("alice", "POST", "/api/notes", { dashboard: "sales", panel: 0, x: "2026-02-02", text: "x" }, { Origin: "https://evil.example" });
  check(r.status === 403, `a cross-site Origin is refused (${r.status})`);
  r = await api("alice", "POST", "/api/notes", { dashboard: "sales", panel: 7, x: "2026-02-02", text: "x" });
  check(r.status === 400, `a panel the dashboard doesn't have is refused (${r.status})`);
  r = await api("alice", "POST", "/api/notes", { dashboard: "sales", panel: 0, x: "", text: "x" });
  check(r.status === 400, `a note needs an x (${r.status})`);
  r = await api("alice", "POST", "/api/notes", { dashboard: "sales", panel: 0, x: "2026-02-02", text: "x".repeat(281) });
  check(r.status === 400, `text_max_chars holds (${r.status})`);
  r = await api("alice", "POST", "/api/notes", { dashboard: "other", panel: 0, x: "2026-02-02", text: "x" });
  check(r.status === 404, `a dashboard without notes is 404 (${r.status})`);
  r = await api("share:x", "GET", "/api/notes?dashboard=sales");
  check(r.status === 401 || r.status === 403, `a reserved identity is refused (${r.status})`);
  r = await api("carol", "POST", "/api/notes", { dashboard: "sales", panel: 0, x: "2026-02-02", x2: "2026-02-03", text: "Outage" });
  check(r.status === 201 && r.json?.note?.x2 === "2026-02-03", `a range note (${r.status} ${r.text})`);
  let n = await notes("bob");
  check(n.length === 2 && !n.some((x) => x.mine), `every viewer reads both, none theirs (${n.map((x) => x.text)})`);
  const promo = n.find((x) => x.text.startsWith("Promotion")).id;
  const outage = n.find((x) => x.text === "Outage").id;
  r = await api("bob", "POST", "/api/notes/delete", { dashboard: "sales", id: promo });
  check(r.status === 403, `bob can't delete alice's note (${r.status})`);
  r = await api("alice", "POST", "/api/notes/delete", { dashboard: "sales", id: outage });
  check(r.status === 403, `an author can't delete another's (${r.status})`);
  r = await api("carol", "POST", "/api/notes/delete", { dashboard: "sales", id: promo });
  check(r.status === 200, `a moderator deletes any note (${r.status} ${r.text})`);
  r = await api("carol", "POST", "/api/notes/delete", { dashboard: "sales", id: outage });
  check(r.status === 200, "the author deletes theirs");
  r = await api("carol", "POST", "/api/notes", { dashboard: "sales", panel: 0, x: "2026-02-05", text: "Kept" });
  check(r.status === 201, "a third note");

  // ── a restart reads the file back ──
  await stop(srv);
  srv = await serve();
  n = await notes("alice");
  check(JSON.stringify(n.map((x) => x.text)) === JSON.stringify(["Kept"]), `after a restart (${n.map((x) => x.text)})`);
} catch (e) {
  bad++;
  console.error("FAIL", e);
  if (srv) console.error(srv.log());
} finally {
  if (browser) await browser.close();
  if (srv) await stop(srv);
  rmSync(W, { recursive: true, force: true });
}
if (bad) {
  console.error(`${bad} check(s) failed`);
  process.exit(1);
}
console.log("all checks passed");
