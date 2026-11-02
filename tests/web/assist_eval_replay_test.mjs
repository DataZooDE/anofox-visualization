// #845 (M3-3): the assistant eval's free CI checks (plan §6), over the stored
// results in tests/assist-eval/results/latest/ (once a live run put them
// there) and over a hand-written recording (tests/assist-eval/replay-fixture/)
// that proves the checks themselves:
//   1. recheck — each stored final SQL is linted again with today's rules on
//      its dataset: a role or lint change that breaks what the assistant
//      produced fails here;
//   2. replay — the recorded model turns are played back through the real
//      loop and the real tools: the same outcome and the same final SQL;
//   3. freshness — results made with another prefix (instructions, guide,
//      roles) get a non-blocking warning: run the live eval again.
import { existsSync, readFileSync, readdirSync } from "fs";
import path from "path";
import assert from "assert/strict";

const here = path.dirname(new URL(import.meta.url).pathname);
const root = path.resolve(here, "../..");
const E = await import(path.join(root, "scripts/assist-eval.mjs"));
const { prefixHash } = await import(path.join(root, "web/assist.js"));
const wasm = await import(path.join(root, "web/pkg/anofox_visualization.js"));
wasm.initSync({ module: readFileSync(path.join(root, "web/pkg/anofox_visualization_bg.wasm")) });

const seed = E.loadSeed(root);
const byId = new Map(seed.prompts.map((p) => [p.id, p]));
const current = await prefixHash(E.evalPrefix(wasm));
const read = (f) => JSON.parse(readFileSync(f, "utf8"));

// replayTransport itself: assistant turns only, in order; text to onText
{
  const t = E.replayTransport([
    { role: "user", content: "x" },
    { role: "assistant", content: [{ type: "text", text: "a" }] },
    { role: "assistant", content: [{ type: "tool_use", id: "t", name: "n", input: {} }] },
  ]);
  const seen = [];
  assert.equal((await t.send({}, { onText: (x) => seen.push(x) })).stop_reason, "end_turn");
  assert.equal((await t.send({})).stop_reason, "tool_use");
  assert.deepEqual(seen, ["a"]);
  await assert.rejects(() => t.send({}), /no more model turns/);
}

// the recheck catches what no longer runs
{
  const r = await E.recheck(byId.get("sales-top-regions"), "SELECT nope ::XAXIS, sum(units) ::BARCHART FROM sales GROUP BY 1;", wasm);
  assert.equal(r.ok, false);
  assert.match(r.issues.join(" "), /nope/);
}

const sets = [{ dir: path.join(root, "tests/assist-eval/replay-fixture"), fixture: true }];
const latest = path.join(root, "tests/assist-eval/results/latest");
if (existsSync(path.join(latest, "summary.json"))) sets.push({ dir: latest, fixture: false });
else console.log("assist_eval_replay_test: no stored results yet (tests/assist-eval/results/latest/) — the fixture only");

let cases = 0;
for (const { dir, fixture } of sets) {
  const summary = read(path.join(dir, "summary.json"));
  if (!fixture && summary.prefix_hash !== current) {
    console.log("::warning title=assistant eval is stale::tests/assist-eval/results/latest/ was made with another prefix (the instructions, the guide or the roles changed): run the live eval (scripts/assist-eval.mjs) and store its results in latest/");
  }
  for (const id of readdirSync(dir).filter((x) => existsSync(path.join(dir, x, "transcript.json"))).sort()) {
    const p = byId.get(id);
    assert.ok(p, `${dir}: ${id} is not in prompts.jsonl`);
    const recorded = read(path.join(dir, id, "result.json"));
    const final = readFileSync(path.join(dir, id, "final.sql"), "utf8");
    // 1. recheck
    if (final.trim()) {
      const r = await E.recheck(p, final, wasm);
      if (recorded.runs) assert.ok(r.ok, `${id}: the stored SQL ran clean when it was made and no longer does: ${r.issues.join("; ")}`);
    }
    // 2. replay
    const { messages } = read(path.join(dir, id, "transcript.json"));
    const run = await E.runCase(p, { model: "replay", wasm, makeTransport: () => E.replayTransport(messages) });
    assert.equal(run.result.kind, recorded.kind, `${id}: the replay ends as a ${run.result.kind}, the recording as a ${recorded.kind}`);
    if (recorded.kind === "proposal") assert.equal(run.final.trim(), final.trim(), `${id}: the replay proposes other SQL`);
    if (fixture) assert.equal(run.result.runs, true, `${id}: the fixture's proposal runs clean`);
    cases++;
  }
}
console.log(`assist_eval_replay_test: ok (${cases} recorded case${cases === 1 ? "" : "s"} rechecked and replayed)`);
