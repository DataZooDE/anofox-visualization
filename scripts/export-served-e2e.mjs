// #1391 (1): exports from a locked server hold only what that viewer may see.
// Two viewers (proxy auth) under a row policy. A served page offers no ⤓ HTML
// (the browser's export); one forced from the hidden button is built from
// the viewer's own answers. That file and every `/api/export` (#764: the
// dashboard as SVG, a panel as SVG / CSV / JSON) have the viewer's rows and
// none of the other's — also not in the pre-rendered input variants (#1368)
// — no SQL (statement text, comments, table names) and no secret (the proxy
// secret, a session token, server paths).
//
//   node scripts/export-served-e2e.mjs \
//       --ext path/to/anofox_visualization.duckdb_extension [--port 8972] [--duckdb duckdb]
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
const port = Number(arg("--port", "8972"));
const duckdb = arg("--duckdb", "duckdb");
if (!ext || !existsSync(ext)) {
  console.error("usage: export-served-e2e.mjs --ext <anofox_visualization.duckdb_extension>");
  process.exit(2);
}
let bad = 0;
const check = (ok, m) => (ok ? console.log("ok   " + m) : (bad++, console.error("FAIL " + m)));
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const sqlStr = (s) => `'${s.replace(/'/g, "''")}'`;
const env = { ...process.env, DATAZOO_DISABLE_TELEMETRY: "1" };

const W = mkdtempSync(path.join(path.dirname(path.resolve(ext)), "export-served-e2e-"));
const dashDir = path.join(W, "dash");
mkdirSync(dashDir, { recursive: true });
// Every region and every revenue figure is a distinct marker, so a leak of
// another viewer's rows shows up as plain text in the file. The SQL carries
// markers of its own (a comment, a column alias, the table names).
writeFileSync(
  path.join(dashDir, "rev.sql"),
  `-- @title Revenue
-- @vars channel
-- sqlmark-comment-4711: this comment must not reach a viewer
SELECT channel ::DROPDOWN, 'Channel'::LABEL FROM (SELECT DISTINCT channel FROM orders) ORDER BY channel;
SELECT region::XAXIS, sum(revenue)::BARCHART AS sqlmark_alias_rev, 'Revenue by region'::TITLE
FROM orders WHERE getvariable('channel') IS NULL OR channel = getvariable('channel')
GROUP BY region ORDER BY region;
-- @name orders
SELECT region ::TABLE, channel, revenue FROM orders ORDER BY region, channel;
`
);
// Alice: Avalon only; Bob: Brigadoon and Camelot. Revenues 1101…1106 are
// unique, so each one names its row.
const SETUP = `CREATE TABLE orders AS SELECT * FROM (VALUES
  ('Avalon','web',1101), ('Avalon','app',1102),
  ('Brigadoon','web',1103), ('Brigadoon','app',1104),
  ('Camelot','web',1105), ('Camelot','app',1106)) t(region, channel, revenue);
CREATE TABLE entitlements AS SELECT * FROM (VALUES ('alice','Avalon'), ('bob','Brigadoon'), ('bob','Camelot')) t(viewer, region);`;
const policyFile = path.join(W, "policy.json");
writeFileSync(
  policyFile,
  JSON.stringify({
    tables: {
      orders: { rows: "region IN (SELECT region FROM entitlements WHERE viewer = getvariable('__claim_user'))" },
      entitlements: { rows: "viewer = getvariable('__claim_user')" },
    },
  })
);
const sec = crypto.randomBytes(24).toString("hex");
const opts = { auth: { mode: "proxy", shared_secret: sec, trusted_proxies: ["127.0.0.1"] }, policy: policyFile };
const BASE = `http://127.0.0.1:${port}`;
const VIEWERS = {
  alice: { regions: ["Avalon"], revenues: [1101, 1102] },
  bob: { regions: ["Brigadoon", "Camelot"], revenues: [1103, 1104, 1105, 1106] },
};
// a value as text: plain, or with a thousands separator (an SVG table prints 1,101)
const hasVal = (text, v) => [String(v), String(v).replace(/^(\d)(\d{3})$/, "$1,$2"), String(v).replace(/^(\d)(\d{3})$/, "$1\u202f$2")].some((t) => text.includes(t));
const SQL_MARKS = ["sqlmark-comment-4711", "sqlmark_alias_rev", "getvariable", "entitlements", "FROM orders", "GROUP BY"];

let proc = null;
let log = "";
let browser = null;
try {
  proc = spawn(duckdb, ["-unsigned", "-batch", ":memory:"], { stdio: ["pipe", "pipe", "pipe"], env });
  proc.stdout.on("data", (d) => (log += d));
  proc.stderr.on("data", (d) => (log += d));
  proc.stdin.write(`LOAD ${sqlStr(ext)};\n${SETUP}\nSELECT anofox_serve_dashboards(${sqlStr(dashDir)}, ${port}, ${sqlStr(JSON.stringify(opts))});\n`);
  let up = false;
  for (let i = 0; i < 100 && !up; i++) {
    try {
      up = (await fetch(`${BASE}/healthz`)).ok;
    } catch (_) {}
    if (proc.exitCode != null) break;
    if (!up) await sleep(100);
  }
  if (!up) throw new Error(`the server did not start:\n${log}`);

  browser = await chromium.launch();
  const exportAs = async (user) => {
    const ctx = await browser.newContext({ viewport: { width: 1300, height: 800 }, acceptDownloads: true });
    await ctx.setExtraHTTPHeaders({ "X-Anofox-Proxy-Secret": sec, "X-Forwarded-User": user });
    const page = await ctx.newPage();
    const errs = [];
    page.on("pageerror", (e) => errs.push(String(e)));
    await page.goto(`${BASE}/d/rev/`);
    await page.waitForFunction(() => !document.body.classList.contains("loading") && document.querySelectorAll(".dash .panel-svg svg").length >= 1, null, { timeout: 30000 });
    await page.waitForTimeout(800);
    const live = await page.evaluate(() => document.querySelector(".dash").innerText);
    const btn = page.locator("#dlhtml");
    check(!(await btn.isVisible()), `${user}: the served page offers no ⤓ HTML`);
    // a viewer who un-hides it (devtools): the file is built in their browser
    const [dl] = await Promise.all([
      page.waitForEvent("download", { timeout: 60000 }),
      page.evaluate(() => {
        const b = document.getElementById("dlhtml");
        b.disabled = false;
        b.click();
      }),
    ]);
    const file = path.join(W, `${user}.html`);
    await dl.saveAs(file);
    const html = readFileSync(file, "utf8");
    // the session token(s) this page holds, if any
    const tokens = await page.evaluate(() => {
      const out = [];
      for (const s of [sessionStorage, localStorage]) for (let i = 0; i < s.length; i++) out.push(s.getItem(s.key(i)));
      return out.filter((v) => v && v.length >= 20);
    });
    const cookies = (await ctx.cookies()).map((c) => c.value).filter((v) => v.length >= 20);
    const exports = {};
    for (const u of ["rev.svg", "rev/orders.svg", "rev/orders.csv", "rev/orders.json", "rev.svg?v.channel=app"]) {
      const r = await ctx.request.get(`${BASE}/api/export/${u}`);
      exports[u] = { status: r.status(), body: await r.text() };
    }
    await ctx.close();
    return { html, live, errs, exports, secrets: [sec, ...tokens, ...cookies] };
  };

  for (const [user, own] of Object.entries(VIEWERS)) {
    const other = VIEWERS[user === "alice" ? "bob" : "alice"];
    const { html, live, errs, exports, secrets } = await exportAs(user);
    check(own.regions.every((r) => live.includes(r)) && other.regions.every((r) => !live.includes(r)), `${user}: the live page shows only their regions`);
    check(html.length > 5000 && /<svg/.test(html), `${user}: the export is a dashboard (${html.length} bytes)`);
    check(own.regions.every((r) => html.includes(r)), `${user}: the export has their regions (${own.regions})`);
    check(own.revenues.every((n) => hasVal(html, n)), `${user}: the export has their rows (${own.revenues})`);
    const leakedRegions = other.regions.filter((r) => html.includes(r));
    const leakedRows = other.revenues.filter((n) => hasVal(html, n));
    check(!leakedRegions.length && !leakedRows.length, `${user}: no other viewer's rows, also not in the input variants (${leakedRegions} ${leakedRows})`);
    check(/<template/.test(html), `${user}: the export carries pre-rendered input variants (#1368)`);
    const sql = SQL_MARKS.filter((m) => html.includes(m));
    check(!sql.length, `${user}: no SQL in the export (${sql})`);
    const leakedSecrets = secrets.filter((s) => html.includes(s));
    check(!leakedSecrets.length, `${user}: no secret in the export (proxy secret, session tokens: ${secrets.length} checked)`);
    check(!html.includes(policyFile) && !html.includes(dashDir), `${user}: no server paths in the export`);
    check(!errs.length, `${user}: no page errors: ` + errs.join(" | "));
    for (const [u, { status, body }] of Object.entries(exports)) {
      const leaks = [...other.regions, ...other.revenues].filter((x) => hasVal(body, x));
      const sqlIn = SQL_MARKS.filter((m) => body.includes(m));
      const secIn = secrets.filter((x) => body.includes(x));
      const mine = u.includes("channel=app") ? own.regions : [...own.regions, ...own.revenues];
      check(
        status === 200 && mine.every((x) => hasVal(body, x)) && !leaks.length && !sqlIn.length && !secIn.length && !body.includes(W),
        `${user}: /api/export/${u} → their rows only, no SQL, no secret (${status}${mine.filter((x) => !hasVal(body, x)).map((x) => " missing " + x).join("")}${leaks.length ? " leaks " + leaks : ""}${sqlIn.length ? " sql " + sqlIn : ""}${secIn.length ? " secret" : ""})`
      );
    }
  }
} catch (e) {
  bad++;
  console.error("FAIL " + (e.stack || e));
  console.error(log);
} finally {
  if (browser) await browser.close();
  if (proc) {
    proc.stdin.end();
    proc.kill(); // our own child (pid known), never by name
  }
  rmSync(W, { recursive: true, force: true });
}
console.log(`export-served-e2e: ${bad} failed`);
process.exit(bad ? 1 : 0);
