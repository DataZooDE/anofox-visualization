// `-- @materialize` (#727) on a real locked server: a heavy setup statement
// runs once per input and every panel reads that one result. With no warm
// connections (`warm: 0`) every panel request is cold, so without it each
// panel runs the statement itself — a `random()` table then differs from
// panel to panel; with it, panels fired at once all read the same rows and
// the server log shows one build. A variable the statement reads builds a
// new result; one only the panels read reuses it. The result is shared
// between logins (the setup reads no claim), and a save (options.writeback)
// builds it once more, reading the saved row.
//
//   node scripts/materialize-served-e2e.mjs \
//       --ext path/to/anofox_visualization.duckdb_extension [--port 8965] [--duckdb duckdb]
import { spawn } from "child_process";
import { mkdtempSync, mkdirSync, writeFileSync, rmSync, existsSync } from "fs";
import path from "path";
import crypto from "crypto";

const arg = (k, d) => {
  const i = process.argv.indexOf(k);
  return i > 0 ? process.argv[i + 1] : d;
};
const ext = arg("--ext", "");
const port = Number(arg("--port", "8965"));
const duckdb = arg("--duckdb", "duckdb");
if (!ext || !existsSync(ext)) {
  console.error("usage: materialize-served-e2e.mjs --ext <anofox_visualization.duckdb_extension> (build it with duckext/scripts/build-native.sh)");
  process.exit(2);
}
let bad = 0;
const check = (ok, m) => (ok ? console.log("ok   " + m) : (bad++, console.error("FAIL " + m)));
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const sqlStr = (s) => `'${s.replace(/'/g, "''")}'`;
const env = { ...process.env, DATAZOO_DISABLE_TELEMETRY: "1" };

const W = mkdtempSync(path.join(path.dirname(path.resolve(ext)), "materialize-served-e2e-"));
const dashDir = path.join(W, "dash");
mkdirSync(dashDir, { recursive: true });
const PANELS = 5;
const dash = (title, mark) => `-- @title ${title}
-- @vars n, m
SET VARIABLE n = coalesce(getvariable('n'), 3);
${mark}
CREATE OR REPLACE TEMP TABLE sim AS SELECT i, random() AS r, (SELECT sum(lead_time) FROM plan_params) AS lt FROM range(getvariable('n')) t(i);
SELECT field AS lt ::FORM, type, value FROM (VALUES ('lead_time', 'number', '5')) v(field, type, value);
SELECT 'Save parameters' ::SAVE, 'lt' ::SAVE_FORM, 'plan_params' ::SAVE_TABLES;
${Array.from({ length: PANELS }, () => "SELECT string_agg(r::VARCHAR || '@' || lt, ',' ORDER BY i) AS s ::TEXT_SMALL FROM sim WHERE coalesce(getvariable('m'), 0) >= 0;").join("\n")}
`;
const FIRST = 4; // the panels after the setup, the form and the save
writeFileSync(path.join(dashDir, "mat.sql"), dash("Materialized", "-- @materialize"));
writeFileSync(path.join(dashDir, "plain.sql"), dash("Plain", "-- not materialized"));
const sec = crypto.randomBytes(24).toString("hex");
const opts = {
  warm: 0,
  auth: { mode: "proxy", shared_secret: sec, trusted_proxies: ["127.0.0.1"] },
  policy: "none",
  writeback: {
    path: path.join(W, "changes.jsonl"),
    dashboards: ["mat"],
    writers: { users: ["alice"] },
    tables: { plan_params: { key: ["id"], columns: { id: "VARCHAR", lead_time: "DOUBLE" } } },
  },
};
const SETUP = "CREATE TABLE plan_params AS SELECT * FROM (VALUES ('sup', 7.0), ('wh', 3.0)) v(id, lead_time);";

let proc = null;
let log = "";
try {
  proc = spawn(duckdb, ["-unsigned", "-batch", ":memory:"], { stdio: ["pipe", "pipe", "pipe"], env });
  proc.stdout.on("data", (d) => (log += d));
  proc.stderr.on("data", (d) => (log += d));
  proc.stdin.write(`LOAD ${sqlStr(ext)};\n${SETUP}\nSELECT anofox_serve_dashboards(${sqlStr(dashDir)}, ${port}, ${sqlStr(JSON.stringify(opts))});\n`);
  let up = false;
  for (let i = 0; i < 100 && !up; i++) {
    try {
      up = (await fetch(`http://127.0.0.1:${port}/healthz`)).ok;
    } catch (_) {}
    if (proc.exitCode != null) break;
    if (!up) await sleep(100);
  }
  if (!up) throw new Error(`the server did not start:\n${log}`);

  const as = (user) => ({ "X-Anofox-Proxy-Secret": sec, "X-Forwarded-User": user });
  const panel = async (d, p, vars = {}, user = "alice") => {
    const r = await fetch(`http://127.0.0.1:${port}/api/panel`, {
      method: "POST",
      headers: { "Content-Type": "application/json", ...as(user) },
      body: JSON.stringify({ dashboard: d, panel: p, vars }),
    });
    const t = await r.text();
    if (!r.ok) throw new Error(`/api/panel ${d}#${p}: ${r.status} ${t}`);
    return t;
  };
  // The panels after the two setup statements, all at once.
  const page = (d, vars, user) => Promise.all(Array.from({ length: PANELS }, (_, k) => panel(d, FIRST + k, vars, user)));
  const builds = () => (log.match(/materialized statement 1 of 'mat'/g) || []).length;

  const plain = await page("plain", {});
  check(new Set(plain).size > 1, `without @materialize, cold panels each run the statement (${new Set(plain).size} different results)`);

  const a = await page("mat", { n: 3, m: 1 });
  check(new Set(a).size === 1, `with @materialize, ${PANELS} panels at once read one result (${a[0]})`);
  await sleep(200);
  check(builds() === 1, `…built once (${builds()} builds)`);

  const b = await page("mat", { n: 5, m: 1 });
  check(new Set(b).size === 1 && b[0] !== a[0], "a variable the statement reads builds a new result");
  check(JSON.parse(b[0])[0].c0.split(",").length === 5, "…for its value (n = 5 rows)");
  await sleep(200);
  check(builds() === 2, `…once (${builds()} builds)`);

  // A variable only the panels read: new panel results, the same build.
  const a2 = await page("mat", { n: 3, m: 2 });
  check(a2[0] === a[0] && new Set(a2).size === 1, "a variable the statement doesn't read reuses its result");
  await sleep(200);
  check(builds() === 2, `…without a build (${builds()} builds)`);

  // Another login, the same values: the same result (the setup reads no claim).
  const bob = await page("mat", { n: 3, m: 3 }, "bob");
  check(bob[0] === a[0], "another login with the same values reads the same result");
  await sleep(200);
  check(builds() === 2, `…without a build (${builds()} builds)`);

  // A save: the next reads build once more, with the saved value.
  const res = await fetch(`http://127.0.0.1:${port}/api/save`, {
    method: "POST",
    headers: { "Content-Type": "application/json", "X-Anofox-Submit": "1", Origin: `http://127.0.0.1:${port}`, ...as("alice") },
    body: JSON.stringify({ dashboard: "mat", panel: 3, id: "5a01", vars: {}, form: { lead_time: 4 }, key: "sup", base: 0 }),
  });
  check(res.status === 201 || res.status === 200, `alice saves lead_time = 4 (${res.status} ${await res.text()})`);
  // (the save request itself runs the setup too, with its own variables)
  await sleep(200);
  const before = builds();
  const c = await page("mat", { n: 3, m: 1 });
  check(new Set(c).size === 1 && c[0] !== a[0] && JSON.parse(c[0])[0].c0.endsWith("@7.0"), `after the save, one new result with the saved row (${c[0]})`);
  await sleep(200);
  check(builds() === before + 1, `…built once (${builds() - before} builds)`);
  check(!/materializing statement .* failed/.test(log), "no build failed");
} catch (e) {
  bad++;
  console.error("FAIL " + (e.stack || e));
  console.error(log);
} finally {
  if (proc) {
    proc.stdin.end();
    proc.kill(); // our own child (pid known), never by name
  }
  rmSync(W, { recursive: true, force: true });
}
console.log(`materialize-served-e2e: ${bad} failed`);
process.exit(bad ? 1 : 0);
