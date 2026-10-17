// Smoke test of the anofox DuckDB-Wasm extensions the browser demo serves from
// web/localext/ (run by tests/web_js.rs): load each with DuckDB-Wasm's
// node-blocking bundle — the same engine version as web/app.js — and run one
// producer each. Skips (exit 0, with a note) unless
//   * the extensions were fetched: scripts/fetch-wasm-extensions.sh, and
//   * DuckDB-Wasm is installed: npm install --prefix tests/web.
process.env.DATAZOO_DISABLE_TELEMETRY = "1"; // no usage telemetry from anofox extensions, whatever was inherited (child processes inherit it)
import { existsSync, readFileSync } from "fs";
import { spawn } from "child_process";
import { createRequire } from "module";
import path from "path";
import assert from "assert/strict";

const here = path.dirname(new URL(import.meta.url).pathname);
const root = path.resolve(here, "../..");
const VER = "v1.5.6";
const repoDir = path.join(root, "web/localext");
const skip = (why) => {
  console.log(`wasm extension test skipped: ${why}`);
  process.exit(0);
};
for (const p of ["wasm_eh", "wasm_mvp"])
  for (const e of ["anofox_statistics", "anofox_forecast"])
    if (!existsSync(path.join(repoDir, VER, p, `${e}.duckdb_extension.wasm`)))
      skip(`web/localext/${VER}/${p}/${e} missing (run scripts/fetch-wasm-extensions.sh)`);
// The browser builds carry no usage telemetry (upstream compiles the PostHog
// client out under Emscripten): no PostHog strings, no OpenSSL imports. Were
// it ever compiled in, web/app.js would have to switch it off after LOAD.
for (const p of ["wasm_eh", "wasm_mvp"])
  for (const e of ["anofox_statistics", "anofox_forecast"]) {
    const bin = readFileSync(path.join(repoDir, VER, p, `${e}.duckdb_extension.wasm`)).toString("latin1");
    for (const s of ["posthog", "anofox_telemetry_enabled", "SSL_CTX_new", "TLS_client_method"])
      assert.ok(!bin.toLowerCase().includes(s.toLowerCase()), `${p}/${e}: the wasm build contains "${s}" — telemetry is compiled in`);
  }
const require = createRequire(path.join(here, "package.json"));
let duckdb, dist;
try {
  duckdb = require("@duckdb/duckdb-wasm/dist/duckdb-node-blocking.cjs");
  dist = path.dirname(require.resolve("@duckdb/duckdb-wasm/dist/duckdb-node-blocking.cjs"));
} catch (_) {
  skip("@duckdb/duckdb-wasm not installed (npm install --prefix tests/web)");
}

// The blocking bundle fetches extensions synchronously, so the repository must
// be served by another process (python3 is needed by the fetch script anyway).
const port = 18000 + (process.pid % 1000);
const server = spawn("python3", ["-u", "-m", "http.server", String(port), "--bind", "127.0.0.1"], {
  cwd: repoDir,
  stdio: ["ignore", "pipe", "ignore"],
});
await new Promise((ok) => server.stdout.once("data", ok));

try {
  for (const bundle of (process.env.BUNDLES || "eh,mvp").split(",")) {
    const db = await duckdb.createDuckDB(
      {
        [bundle]: {
          mainModule: path.join(dist, `duckdb-${bundle}.wasm`),
          mainWorker: path.join(dist, `duckdb-node-${bundle}.worker.cjs`),
        },
      },
      new duckdb.VoidLogger(),
      duckdb.NODE_RUNTIME
    );
    await db.instantiate();
    db.open({ allowUnsignedExtensions: true });
    const c = db.connect();
    const q = (s) => {
      if (process.env.DEBUG) console.log(`[${bundle}] ${s.slice(0, 60)}`);
      let res;
      try {
        res = c.query(s);
      } catch (e) {
        throw new Error(`[${bundle}] ${s.slice(0, 60)}: ${String(e.message || e).slice(0, 400)}`);
      }
      return res
        .toArray()
        .map((r) => JSON.parse(JSON.stringify(r.toJSON(), (_, v) => (typeof v === "bigint" ? Number(v) : v))));
    };
    assert.equal(q("PRAGMA version")[0].library_version, VER);
    q(`SET custom_extension_repository='http://127.0.0.1:${port}'`);
    q(`CREATE TABLE d AS SELECT 'a' AS g, i::DOUBLE AS x, DATE '2024-01-01' + INTERVAL (i) DAY AS ds,
         1.0 + 0.5 * i + sin(i) AS y FROM range(1, 41) t(i)`);
    // FORCE: DuckDB-Wasm in Node caches installs under ~/.duckdb/extensions.
    // anofox_statistics: a coefficient table.
    q("FORCE INSTALL anofox_statistics");
    q("LOAD anofox_statistics");
    const noTelemetry = () =>
      assert.equal(q("SELECT count(*) AS n FROM duckdb_settings() WHERE name = 'anofox_telemetry_enabled'")[0].n, 0, "a wasm build registers anofox_telemetry_enabled");
    noTelemetry();
    const tidy = q("SELECT term, estimate FROM ols_tidy_by('d', g, y, [x]) ORDER BY term");
    assert.equal(tidy.length, 2, JSON.stringify(tidy));
    assert.ok(Math.abs(tidy.find((r) => r.term !== "(Intercept)").estimate - 0.5) < 0.1, JSON.stringify(tidy));
    // anofox_forecast: a 7-day forecast. Its wasm_mvp build throws on LOAD
    // ("_setThrew is not defined": the MVP runtime has no wasm exceptions);
    // browsers with wasm exception handling (all current ones) get wasm_eh.
    if (bundle === "eh") {
      q("FORCE INSTALL anofox_forecast");
      q("LOAD anofox_forecast");
      const fc = q("SELECT count(*) AS n FROM ts_forecast_by('d', g, ds, y, 'AutoETS', 7, '1d')");
      assert.equal(fc[0].n, 7);
      noTelemetry();
    }
    console.log(`wasm extensions (${bundle}): ok`);
  }
} finally {
  server.kill();
}
