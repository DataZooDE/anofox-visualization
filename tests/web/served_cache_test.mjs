// Node test for the served page's per-panel cache (#210): servedRequest
// reuses an answer while the variables its statement depends on
// (window.__served.deps, the server's own analysis) keep their values, skips
// setup statements entirely, and turns the narrowing off — clearing the
// cache — when the server's X-Anofox-Deps no longer matches
// window.__served.depsTag (a refresh changed the analysis).
import { readFileSync } from "fs";
import assert from "assert/strict";

const src = readFileSync(new URL("../../web/app.js", import.meta.url), "utf8");
const depsOf = src.match(/^function servedDepsOf\([\s\S]*?^}/m);
const varSig = src.match(/^function servedVarSig\([\s\S]*?^}/m);
const request = src.match(/^async function servedRequest\([\s\S]*?^}/m);
assert.ok(depsOf, "servedDepsOf not found in app.js");
assert.ok(varSig, "servedVarSig not found in app.js");
assert.ok(request, "servedRequest not found in app.js");
// The harness builds its own state (below): keep it from drifting from the
// real file's declarations.
assert.ok(src.includes("const servedCache = new Map();"), "servedCache not found in app.js");
assert.ok(src.includes("const SERVED_CACHE_MAX = 400;"), "SERVED_CACHE_MAX not found in app.js");
assert.ok(src.includes("let servedDepsOff = false;"), "servedDepsOff not found in app.js");

const PLAN = [
  { setup: true, sql: "SET VARIABLE sel = x" },
  { setup: false, sql: "A" },
  { setup: false, sql: "B" },
  { setup: false, sql: "C" },
];

function makeHarness() {
  let tag = "t1";
  const fetches = [];
  const fetchStub = async (url, opts) => {
    const b = JSON.parse(opts.body);
    fetches.push(b);
    return {
      status: 200,
      ok: true,
      headers: { get: (h) => (h === "X-Anofox-Deps" ? tag : null) },
      text: async () => JSON.stringify({ panel: b.panel, vars: b.vars, page: b.page }),
    };
  };
  const windowStub = { __served: { id: "d", sql: "", deps: [[], ["sel", "series"], [], null], depsTag: "t1" } };
  const planStub = () => JSON.stringify(PLAN);
  const prelude = `
let servedIndex = null, servedPlan = null;
const servedVars = {}, servedAborts = new Map(), servedCache = new Map();
const SERVED_CACHE_MAX = 400; let servedDepsOff = false;
const servedViewer = "0".repeat(32);`;
  const make = new Function(
    "window",
    "plan",
    "fetch",
    `${prelude}\n${depsOf[0]}\n${varSig[0]}\n${request[0]}\nreturn { servedRequest, servedVarSig, servedDepsOf, servedVars, servedCache, off: () => servedDepsOff };`
  );
  const h = make(windowStub, planStub, fetchStub);
  return { ...h, fetches, setTag: (t) => (tag = t), window: windowStub };
}

// 1. Three panels, three fetches; the setup statement never sends a request.
{
  const h = makeHarness();
  h.servedVars.series = "a";
  const a = await h.servedRequest("A", null, null, 1);
  const b = await h.servedRequest("B", null, null, 2);
  const c = await h.servedRequest("C", null, null, 3);
  assert.equal(h.fetches.length, 3, "A, B, C each sent a request");
  assert.equal(await h.servedRequest("SET VARIABLE sel = x", null, null, 0), "[]", "setup answers [] on its own");
  assert.equal(h.fetches.length, 3, "setup sent no request");
  assert.ok(a && b && c, "A/B/C got answers");
  console.log("ok   three statements, three fetches; setup is free");
}

// 2. The same three again: every one a cache hit.
{
  const h = makeHarness();
  h.servedVars.series = "a";
  await h.servedRequest("A", null, null, 1);
  await h.servedRequest("B", null, null, 2);
  await h.servedRequest("C", null, null, 3);
  const before = h.fetches.length;
  await h.servedRequest("A", null, null, 1);
  await h.servedRequest("B", null, null, 2);
  await h.servedRequest("C", null, null, 3);
  assert.equal(h.fetches.length, before, "repeating the same three sent no new request");
  console.log("ok   repeating the same requests hits the cache");
}

// 3. A variable the panel doesn't depend on (region): A and B hit, C (deps
// null = every variable) is fetched again.
{
  const h = makeHarness();
  h.servedVars.series = "a";
  await h.servedRequest("A", null, null, 1);
  await h.servedRequest("B", null, null, 2);
  await h.servedRequest("C", null, null, 3);
  const before = h.fetches.length;
  h.servedVars.region = "x";
  await h.servedRequest("A", null, null, 1);
  await h.servedRequest("B", null, null, 2);
  await h.servedRequest("C", null, null, 3);
  assert.equal(h.fetches.length, before + 1, "only C (deps: null) re-fetched");
  console.log("ok   an unrelated variable only re-requests a panel with deps: null");
}

// 4. series changes: A (deps ["sel","series"]) re-fetches and its answer
// carries the new value — never stale; C re-fetches (deps null); B (deps [])
// stays a hit.
{
  const h = makeHarness();
  h.servedVars.series = "a";
  await h.servedRequest("A", null, null, 1);
  await h.servedRequest("B", null, null, 2);
  await h.servedRequest("C", null, null, 3);
  const beforeB = h.fetches.filter((f) => f.panel === 2).length;
  h.servedVars.series = "b";
  const aAnswer = JSON.parse(await h.servedRequest("A", null, null, 1));
  await h.servedRequest("B", null, null, 2);
  await h.servedRequest("C", null, null, 3);
  const afterB = h.fetches.filter((f) => f.panel === 2).length;
  assert.equal(afterB, beforeB, "B (deps []) is still a hit");
  assert.equal(aAnswer.vars.series, "b", "A's answer carries the new series — no stale answer");
  console.log("ok   series change re-fetches its readers, B stays a hit");
}

// 5. Page requests: two different pages are two fetches; repeating the first
// is a hit.
{
  const h = makeHarness();
  h.servedVars.series = "a";
  const r1 = await h.servedRequest(null, { base: "A", limit: 10 }, null, 1);
  const r2 = await h.servedRequest(null, { base: "A", limit: 10, offset: 10 }, null, 1);
  assert.equal(h.fetches.length, 2, "two distinct pages are two fetches");
  const before = h.fetches.length;
  const r1b = await h.servedRequest(null, { base: "A", limit: 10 }, null, 1);
  assert.equal(h.fetches.length, before, "repeating the first page is a hit");
  assert.equal(r1b, r1, "the hit returns the same body");
  assert.notEqual(r1, r2, "the two pages differ");
  console.log("ok   page requests cache by page, not just panel");
}

// 6. servedVarSig: case-insensitive names; unset differs from NULL.
{
  const h = makeHarness();
  assert.equal(h.servedVarSig(["series"], { Series: "b" }), h.servedVarSig(["series"], { series: "b" }), "case-insensitive names");
  assert.notEqual(h.servedVarSig(["x"], {}), h.servedVarSig(["x"], { x: null }), "unset differs from NULL");
  console.log("ok   servedVarSig: case-insensitive, unset != null");
}

// 7. servedDepsOf: a length mismatch or missing __served means "all".
{
  const h = makeHarness();
  assert.equal(h.servedDepsOf({ deps: [[]] }, 4, 0), null, "plan length mismatch -> all");
  assert.equal(h.servedDepsOf(undefined, 4, 1), null, "no __served -> all");
  console.log("ok   servedDepsOf falls back to all (null)");
}

// 8. A changed X-Anofox-Deps turns the narrowing off and clears the cache: a
// new region value then re-fetches A (deps [] no longer narrows it).
{
  const h = makeHarness();
  h.servedVars.series = "a";
  await h.servedRequest("A", null, null, 1);
  h.setTag("t2");
  await h.servedRequest("B", { base: "B" }, null, 2);
  // (the B fetch above observed the new tag and turned narrowing off)
  await h.servedRequest("B", { base: "B" }, null, 2);
  assert.equal(h.off(), true, "the stale analysis turned the narrowing off");
  const before = h.fetches.length;
  h.servedVars.region = "z";
  await h.servedRequest("A", null, null, 1);
  assert.equal(h.fetches.length, before + 1, "with narrowing off, a new region value re-fetches A too");
  console.log("ok   a changed X-Anofox-Deps turns narrowing off and clears the cache");
}

// 9. A body snapshot: mutating servedVars after starting a request (before
// awaiting it) must not change what was already sent.
{
  const h = makeHarness();
  h.servedVars.series = "a";
  const p = h.servedRequest("A", null, null, 1);
  h.servedVars.series = "mutated-after-start";
  await p;
  assert.equal(h.fetches[0].vars.series, "a", "the recorded body has the old value");
  console.log("ok   servedRequest snapshots vars at call time");
}

console.log("served_cache_test: ok");
