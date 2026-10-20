// The wasm bundle's size budget (#312): web/pkg/anofox_visualization_bg.wasm
// must not grow more than 30 KB past the committed baseline in
// web/wasm-size.txt. This is only meaningful when the wasm is built with
// `[profile.wasm]` (fat LTO, one codegen unit) — `wasm-pack build ... --profile
// wasm`, exactly as CI's web job and the release scripts do. Under the plain
// release profile (no LTO, 16 codegen units) a single PR's size delta is
// mostly codegen noise of ±20-40 K (how functions get split across codegen
// units reshuffles on unrelated changes), so that profile can't carry a tight
// budget — see ~/projects/duckdb/wasm-size-m2.md.
//
//   node scripts/check-wasm-size.mjs            write web/wasm-size.txt = the built size
//   node scripts/check-wasm-size.mjs --check    exit 1 if the built size exceeds budget
import { readFileSync, statSync, writeFileSync } from "fs";
import path from "path";
import { fileURLToPath } from "url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const WASM = path.join(root, "web/pkg/anofox_visualization_bg.wasm");
const BASELINE_FILE = path.join(root, "web/wasm-size.txt");
const BUDGET_BYTES = 30 * 1024; // 30 KB

const fmt = (n) => `${n} B (${(n / 1024).toFixed(1)} KiB)`;

let size;
try {
  size = statSync(WASM).size;
} catch (e) {
  console.error(`FAIL: ${WASM} not found — build it first: wasm-pack build --target web --out-dir web/pkg --no-default-features --features wasm --profile wasm`);
  process.exit(1);
}

if (process.argv.includes("--check")) {
  const baseline = parseInt(readFileSync(BASELINE_FILE, "utf8").trim(), 10);
  const limit = baseline + BUDGET_BYTES;
  console.log(`wasm size: ${fmt(size)}`);
  console.log(`baseline:  ${fmt(baseline)} (${BASELINE_FILE})`);
  console.log(`budget:    ${fmt(limit)} (baseline + 30 KiB)`);
  if (size > limit) {
    console.error(
      `FAIL: grew ${((size - baseline) / 1024).toFixed(1)} KiB past the baseline, over the 30 KB budget.\n` +
        `If the growth is real and intentional (not codegen noise — rebuild and re-check once to be sure),\n` +
        `update the baseline: node scripts/check-wasm-size.mjs`
    );
    process.exitCode = 1;
  } else {
    console.log(`ok   wasm size is within budget (${((limit - size) / 1024).toFixed(1)} KiB to spare)`);
  }
} else {
  writeFileSync(BASELINE_FILE, `${size}\n`);
  console.log(`wrote ${BASELINE_FILE} = ${size}`);
}
