// #195 M2-7: the Ask eval seed (tests/ask-eval/) is well-formed — 15 cases
// with unique ids and the plan's mix of kinds; every truth query runs on the
// example data (in DuckDB-Wasm, after the dashboard's own setup) and gives
// one row; every named panel is exactly one panel; the served copy turns Ask
// on with the inputs' variables. The free part of the eval: no API key, no
// network, no cost — the live run (scripts/ask-eval.mjs) is manual.
import { existsSync, readFileSync } from "fs";
import path from "path";
import assert from "assert/strict";

const here = path.dirname(new URL(import.meta.url).pathname);
const root = path.resolve(here, "../..");

if (!existsSync(path.join(root, "web/pkg/anofox_visualization_bg.wasm"))) {
  console.error("ask_eval_seed_test: FAILED — needs web/pkg (wasm-pack build)");
  process.exit(1);
}
const { loadCases, validateCases, loadWasm, runTruth, resolvePanels, servedCopy, missingTruth, numbersIn, parseSse, messageOf, targetsOf } = await import(path.join(root, "scripts/ask-eval.mjs"));
const wasm = await loadWasm(path.join(root, "web/pkg"));

const seed = loadCases(root);
validateCases(seed);
assert.equal(seed.cases.length, 30, `30 cases (plan §8 M3-4), found ${seed.cases.length}`);
const kinds = {};
for (const c of seed.cases) kinds[c.kind] = (kinds[c.kind] || 0) + 1;
for (const k of ["fact", "view", "explain", "meaning", "refusal"]) assert.ok(kinds[k] >= 2, `at least two ${k} cases: ${JSON.stringify(kinds)}`);
assert.ok(seed.cases.every((c) => targetsOf(c).length), "every case runs somewhere");
assert.ok(seed.cases.some((c) => c.canary && targetsOf(c).includes("server")), "a canary case on the server");

for (const c of seed.cases) {
  const script = readFileSync(path.join(root, c.dashboard), "utf8");
  const plan = JSON.parse(wasm.plan(script));
  if (c.panels) resolvePanels(plan, c.panels); // throws unless each names one panel
  if (c.truth) {
    const t = await runTruth(root, c, wasm);
    assert.ok(t.row.length && t.row.every((x) => x.value !== null && x.value !== undefined), `${c.id}: the truth has a value in every column: ${JSON.stringify(t.row)}`);
  }
  if (targetsOf(c).includes("server")) {
    const s = servedCopy(wasm, script).sql;
    assert.ok(s.startsWith("-- @ask on\n-- @vars"), `${c.id}: the served copy turns Ask on`);
    assert.ok(!/^\s*CREATE\s+(OR\s+REPLACE\s+)?(TABLE|VIEW|MACRO)\b/im.test(s), `${c.id}: the served copy's setup is TEMP`);
    assert.match(s.split("\n")[1], /^-- @vars( [a-z_]\w*(, [a-z_]\w*)*)?$/i, `${c.id}: the served copy's -- @vars`);
  }
}

// ---- the matchers ----
assert.deepEqual(missingTruth("The supplier's orders vary 24.7× as much.", [{ name: "r", value: 24.727901818484817 }]), []);
assert.deepEqual(missingTruth("about 25×", [{ name: "r", value: 24.727901818484817 }]), [], "25 is 24.73 at the precision shown");
assert.deepEqual(missingTruth("about 24.6×", [{ name: "r", value: 24.727901818484817 }]), ["r"]);
assert.deepEqual(missingTruth("€2.79M in the US", [{ name: "v", value: 2794943 }]), []);
assert.deepEqual(missingTruth("€2.77M", [{ name: "v", value: 2794943 }]), ["v"]);
assert.deepEqual(missingTruth("Store, with 1,152,823 €", [{ name: "c", value: "store" }, { name: "v", value: 1152823 }]), []);
assert.deepEqual(missingTruth("on 25 October 2026", [{ name: "d", value: "2026-10-25", date: true }]), []);
assert.deepEqual(missingTruth("by Oct 25", [{ name: "d", value: "2026-10-25", date: true }]), []);
assert.deepEqual(missingTruth("on 24 October", [{ name: "d", value: "2026-10-25", date: true }]), ["d"]);
assert.deepEqual(numbersIn("66.2 % and 8.2 thousand").map((n) => n.value), [66.2, 8200]);
const sse = 'event: message_start\ndata: {"type":"message_start","message":{"usage":{"input_tokens":7}}}\n\nevent: content_block_start\ndata: {"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"t","name":"answer","input":{}}}\n\nevent: content_block_delta\ndata: {"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\\"scope\\":"}}\n\nevent: content_block_delta\ndata: {"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"\\"partly\\"}"}}\n\nevent: content_block_stop\ndata: {"type":"content_block_stop","index":0}\n\nevent: message_delta\ndata: {"type":"message_delta","usage":{"output_tokens":3}}\n\n';
assert.equal(parseSse(sse).length, 6);
const m = messageOf(sse);
assert.deepEqual(m.content[0].input, { scope: "partly" });
assert.deepEqual(m.usage, { input_tokens: 7, output_tokens: 3 });

console.log("ask_eval_seed_test: ok");
