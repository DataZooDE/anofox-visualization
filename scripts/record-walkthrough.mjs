// Records the locked-server walkthrough on the website (#1017 P4,
// website/src/content/docs/deploy/walkthrough.mdx): a local
// anofox_serve_dashboards with an auth proxy login, a row policy and shared
// write-back, driven by Playwright with recordVideo. Synthetic data only.
//
// Story: the dashboard list → a dashboard with an input → a second viewer
// sees other rows (row policy) → a target saved with ::SAVE, seen by the
// other viewer → the same dashboard as an SVG export.
//
// Re-record:
//   OUT=$PWD/scratch/anofox_visualization.duckdb_extension duckext/scripts/build-native.sh
//   NODE_PATH=tests/web/node_modules node scripts/record-walkthrough.mjs \
//       --ext scratch/anofox_visualization.duckdb_extension [--port 8971] [--duckdb duckdb] \
//       [--out website/src/assets/walkthrough]
// It writes <out>/locked-server.webm (re-encoded with ffmpeg, VP9, no
// audio) and <out>/locked-server-poster.webp, and prints the length, the
// size and the caption timestamps for the page's list. Scratch files go in
// a temporary folder next to the extension (the write-back log refuses a
// world-writable parent such as /tmp).
import { createRequire } from "module";
import { spawn, execFileSync } from "child_process";
import { mkdtempSync, mkdirSync, writeFileSync, readdirSync, rmSync, statSync, existsSync } from "fs";
import path from "path";
import crypto from "crypto";

const require = createRequire(import.meta.url);
const { chromium } = require("playwright");
const arg = (k, d) => {
  const i = process.argv.indexOf(k);
  return i > 0 ? process.argv[i + 1] : d;
};
const ext = arg("--ext", "");
const port = Number(arg("--port", "8971"));
const duckdb = arg("--duckdb", "duckdb");
const out = path.resolve(arg("--out", "website/src/assets/walkthrough"));
if (!ext || !existsSync(ext)) {
  console.error("usage: record-walkthrough.mjs --ext <anofox_visualization.duckdb_extension> [--out dir]");
  process.exit(2);
}
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const sqlStr = (s) => `'${s.replace(/'/g, "''")}'`;
const env = { ...process.env, DATAZOO_DISABLE_TELEMETRY: "1" };

const W = mkdtempSync(path.join(path.dirname(path.resolve(ext)), "walkthrough-"));
const dashDir = path.join(W, "dash");
mkdirSync(dashDir, { recursive: true });

// Two dashboards. Every name and number below is made up.
writeFileSync(
  path.join(dashDir, "sales.sql"),
  `-- @title How are my regions selling?
-- @description Revenue per month for the regions you may see, by product.
SELECT p AS product ::DROPDOWN, 'Product'::LABEL FROM (SELECT DISTINCT product AS p FROM sales) ORDER BY p;
SELECT round(sum(revenue)) AS revenue ::METRIC, 'Revenue (k€)'::LABEL FROM sales WHERE product = getvariable('product');
SELECT count(DISTINCT region) AS regions ::METRIC, 'Regions you can see'::LABEL FROM sales;
SELECT strftime(m, '%b %Y') AS month ::XAXIS, region ::CATEGORY, rev ::BARCHART, 'Revenue per month (k€)'::TITLE
FROM (SELECT month AS m, region, sum(revenue) AS rev FROM sales WHERE product = getvariable('product') GROUP BY ALL)
ORDER BY m, region;
-- @name by_region
SELECT region AS "Region" ::TABLE, round(sum(revenue)) AS "Revenue (k€)", 'By region'::TITLE
FROM sales WHERE product = getvariable('product') GROUP BY ALL ORDER BY 1;
`
);
writeFileSync(
  path.join(dashDir, "targets.sql"),
  `-- @title What targets do we plan per region?
-- @description Set next quarter's target for one of your regions; everyone sees it.
SELECT r AS region ::DROPDOWN, 'Region'::LABEL FROM (SELECT DISTINCT region AS r FROM sales) ORDER BY r;
SELECT field AS plan ::FORM, type, value, label, unit, 'Target for next quarter'::TITLE
FROM (SELECT 'target' AS field, 'number' AS type,
             coalesce((SELECT target FROM targets WHERE id = getvariable('region')), 0)::VARCHAR AS value,
             'Target' AS label, 'k€' AS unit);
SELECT 'Save target' ::SAVE, 'plan' ::SAVE_FORM, 'region' ::SAVE_KEY, 'targets' ::SAVE_TABLES;
SELECT t.id AS "Region" ::TABLE, t.target AS "Target (k€)", round(sum(s.revenue)) AS "Last quarter (k€)",
       coalesce(t.updated_by, '—') AS "Saved by", 'Targets you can see'::TITLE
FROM targets t JOIN sales s ON s.region = t.id GROUP BY ALL ORDER BY 1;
`
);

const SETUP = `CREATE TABLE sales AS
  SELECT region, product, DATE '2026-07-01' + to_months(m) AS month,
         round(40 + 25 * (hash(region || product || m) % 60) / 60.0 + 10 * m + CASE product WHEN 'Bikes' THEN 60 ELSE 0 END, 1) AS revenue
  FROM (VALUES ('North'), ('South'), ('East'), ('West')) r(region),
       (VALUES ('Bikes'), ('Helmets')) p(product), range(3) t(m);
CREATE TABLE targets AS
  SELECT * FROM (VALUES ('North', 450.0), ('South', 380.0), ('East', 300.0), ('West', 410.0)) v(id, target),
  LATERAL (SELECT NULL::VARCHAR AS updated_by, NULL::VARCHAR AS updated_at);
CREATE TABLE access AS
  SELECT * FROM (VALUES ('alice', 'North'), ('alice', 'East'), ('bob', 'South')) v(viewer, region);`;

const rule = "region IN (SELECT region FROM access WHERE viewer = getvariable('__claim_user'))";
writeFileSync(
  path.join(W, "policy.json"),
  JSON.stringify({
    tables: {
      sales: { rows: rule },
      targets: { rows: rule.replace("region IN", "id IN") },
    },
  })
);
const sec = crypto.randomBytes(24).toString("hex");
const opts = {
  auth: { mode: "proxy", shared_secret: sec, trusted_proxies: ["127.0.0.1"] },
  policy: path.join(W, "policy.json"),
  writeback: {
    path: path.join(W, "changes.jsonl"),
    dashboards: ["targets"],
    writers: { users: ["alice", "bob"] },
    tables: {
      targets: {
        key: ["id"],
        columns: { id: "VARCHAR", target: "DOUBLE", updated_by: "VARCHAR", updated_at: "VARCHAR" },
        stamp: { updated_by: "user", updated_at: "ts" },
      },
    },
  },
};

async function serve() {
  const p = spawn(duckdb, ["-unsigned", "-batch", ":memory:"], { stdio: ["pipe", "pipe", "pipe"], env });
  let log = "";
  p.stdout.on("data", (d) => (log += d));
  p.stderr.on("data", (d) => (log += d));
  p.stdin.write(`LOAD ${sqlStr(path.resolve(ext))};\n${SETUP}\nSELECT anofox_serve_dashboards(${sqlStr(dashDir)}, ${port}, ${sqlStr(JSON.stringify(opts))});\n`);
  for (let i = 0; i < 150; i++) {
    try {
      if ((await fetch(`http://127.0.0.1:${port}/healthz`)).ok) return p;
    } catch (_) {}
    if (p.exitCode != null) break;
    await sleep(100);
  }
  p.kill(); // our own child, by its handle
  throw new Error(`the server did not start:\n${log}`);
}

// A caption bar and a "signed in as" badge, drawn into the page itself.
let T0 = Date.now();
const marks = [];
async function caption(page, text, who) {
  if (text) marks.push({ t: (Date.now() - T0) / 1000, text });
  await page.evaluate(
    ([text, who]) => {
      let c = document.getElementById("wt-caption");
      if (!c) {
        c = document.createElement("div");
        c.id = "wt-caption";
        c.style.cssText =
          "position:fixed;left:50%;bottom:100px;transform:translateX(-50%);max-width:1100px;z-index:99999;" +
          "background:rgba(17,24,39,.92);color:#fff;font:600 22px/1.35 system-ui,sans-serif;padding:14px 26px;" +
          "border-radius:12px;box-shadow:0 6px 24px rgba(0,0,0,.25);text-align:center;pointer-events:none";
        document.body.appendChild(c);
      }
      let b = document.getElementById("wt-who");
      if (!b) {
        b = document.createElement("div");
        b.id = "wt-who";
        b.style.cssText =
          "position:fixed;right:18px;top:14px;z-index:99999;background:#2563eb;color:#fff;" +
          "font:600 16px/1 system-ui,sans-serif;padding:9px 14px;border-radius:999px;pointer-events:none";
        document.body.appendChild(b);
      }
      if (text != null) c.textContent = text;
      c.style.display = c.textContent ? "" : "none";
      if (who != null) b.textContent = "Signed in as " + who;
    },
    [text, who]
  );
}
const settle = async (page) => {
  await sleep(500);
  await page.waitForFunction(() => !document.body.classList.contains("loading"), null, { timeout: 20000 });
};

const vidDir = path.join(W, "video");
let srv = null;
let browser = null;
try {
  srv = await serve();
  browser = await chromium.launch();
  const ctx = await browser.newContext({
    viewport: { width: 1280, height: 800 },
    recordVideo: { dir: vidDir, size: { width: 1280, height: 800 } },
    colorScheme: "light",
  });
  // The auth proxy's part: the shared secret and the user header. Switching
  // the header switches the viewer.
  const as = (user) => ctx.setExtraHTTPHeaders({ "X-Anofox-Proxy-Secret": sec, "X-Forwarded-User": user });
  const page = await ctx.newPage();
  T0 = Date.now();
  const base = `http://127.0.0.1:${port}`;
  let who = "alice";
  const go = async (url, text) => {
    await page.goto(base + url);
    await settle(page);
    await caption(page, text, who);
  };

  // a. the list of dashboards
  await as(who);
  await go("/", "A locked server: DuckDB serves a folder of SQL dashboards. Nobody can run their own SQL.");
  await sleep(4500);
  await caption(page, "Every viewer signs in. Here an auth proxy in front says who: alice.");
  await sleep(4000);

  // b. a dashboard with an input
  await go("/d/sales", "alice opens a dashboard. The row policy shows her only her regions: North and East.");
  await sleep(5000);
  await caption(page, "Inputs work as usual: pick another product, and the panels re-run on the server.");
  await sleep(2000);
  const sel = page.locator('select[aria-label="Product"]');
  await sel.selectOption("Helmets");
  await settle(page);
  await sleep(4000);

  // c. a second viewer: other rows
  who = "bob";
  await as(who);
  await go("/d/sales", "Same dashboard, signed in as bob. The policy rewrites every query: he sees only South.");
  await sleep(6000);

  // d. write-back
  who = "alice";
  await as(who);
  await go("/d/targets", "Write-back: alice plans next quarter's targets with a ::FORM and a ::SAVE button.");
  await sleep(4500);
  await caption(page, "She picks North and sets a new target …");
  await page.locator('select[aria-label="Region"]').selectOption("North");
  await settle(page);
  await sleep(1500);
  const field = page.locator('.dp-form-field[data-field="target"] input');
  await field.click();
  await field.fill("");
  await sleep(300);
  await field.pressSequentially("520", { delay: 220 });
  await page.keyboard.press("Enter");
  await settle(page);
  await sleep(1500);
  await caption(page, "… and saves. The server checks the value and her access, then logs the change.");
  await page.locator('.save-panel .submit-btn:text-is("Save target")').click();
  await page.waitForFunction(() => /Saved at/.test(document.querySelector(".save-panel")?.textContent || ""), null, { timeout: 15000 });
  await settle(page);
  await sleep(5000);
  await caption(page, "Every panel that reads the targets table shows the save, stamped with who saved it.");
  await page.locator(".table-card, table").last().scrollIntoViewIfNeeded().catch(() => {});
  await sleep(5000);

  // e. the export link
  await caption(page, "The same dashboards export as SVG, PNG, PDF or CSV through /api/export, under the same login and policy.");
  await page.evaluate(async () => {
    const res = await fetch("/api/export/sales.svg?v.product=Bikes&width=1100");
    const url = URL.createObjectURL(await res.blob());
    const o = document.createElement("div");
    o.style.cssText =
      "position:fixed;inset:0;z-index:99990;background:rgba(15,23,42,.55);display:flex;align-items:flex-start;justify-content:center;padding-top:60px";
    const img = document.createElement("img");
    img.src = url;
    img.style.cssText = "max-width:1100px;max-height:600px;background:#fff;border-radius:8px;box-shadow:0 10px 40px rgba(0,0,0,.4)";
    o.appendChild(img);
    document.body.appendChild(o);
    await new Promise((r) => (img.complete ? r() : (img.onload = r)));
  });
  await sleep(6000);
  await caption(page, "Recorded on a local server with made-up data. Set it up: Deploy → The locked server.");
  await sleep(4000);

  const video = page.video();
  await ctx.close();
  const raw = await video.path();
  mkdirSync(out, { recursive: true });
  const webm = path.join(out, "locked-server.webm");
  // VP9 at a constant quality: screen content compresses well.
  execFileSync("ffmpeg", ["-y", "-loglevel", "error", "-i", raw, "-an", "-c:v", "libvpx-vp9", "-b:v", "0", "-crf", "40", "-row-mt", "1", "-deadline", "good", "-cpu-used", "2", webm]);
  execFileSync("ffmpeg", ["-y", "-loglevel", "error", "-ss", "11", "-i", webm, "-frames:v", "1", "-c:v", "libwebp", "-quality", "80", path.join(out, "locked-server-poster.webp")]);
  const dur = execFileSync("ffprobe", ["-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0", webm]).toString().trim();
  console.log(`video: ${webm} ${(statSync(webm).size / 1e6).toFixed(2)} MB, ${Number(dur).toFixed(1)} s`);
  for (const m of marks) console.log(`${String(Math.floor(m.t / 60)).padStart(1, "0")}:${String(Math.floor(m.t % 60)).padStart(2, "0")}  ${m.text}`);
} finally {
  if (browser) await browser.close();
  if (srv) {
    srv.stdin.end();
    srv.kill(); // our own child, by its handle
  }
  if (!process.env.KEEP) rmSync(W, { recursive: true, force: true });
  else console.log("kept " + W + " " + readdirSync(W).join(" "));
}
