// #195 M2-5: Ask in the builder — web/assist.js runAsk over the wasm
// AskSession (src/host/ask/session.rs), with a scripted fake model that
// records every request and a fake page reader. The plan §12.9 cases that
// apply to the builder: masked values never reach the model and come back
// unmasked, citations are verified (right value, wrong row: not verified),
// @ask skip panels are neither described nor read, forged vars are refused
// before anything runs, a model that stops without `answer` gives the fixed
// message, Cancel stops. Plus static checks over web/app.js: vars are bound
// as typed SET VARIABLE literals (never spliced into the panel's SQL) and
// put back after the read.
import { existsSync, readFileSync } from "fs";
import path from "path";
import assert from "assert/strict";
import { askTrim, runAsk } from "../../web/assist.js";

const here = path.dirname(new URL(import.meta.url).pathname);
const root = path.resolve(here, "../..");
const read = (p) => readFileSync(path.join(root, p), "utf8");
const wasmPkg = path.join(root, "web/pkg");
if (!existsSync(path.join(wasmPkg, "anofox_visualization_bg.wasm"))) {
  console.error("ask_builder_test: FAILED — needs web/pkg (wasm-pack build)");
  process.exit(1);
}
const wasm = await import(path.join(wasmPkg, "anofox_visualization.js"));
wasm.initSync({ module: readFileSync(path.join(wasmPkg, "anofox_visualization_bg.wasm")) });

const DASH = `-- @title Stock
SELECT region ::DROPDOWN FROM (VALUES ('EU'), ('US')) t(region);
SELECT sku AS "SKU" ::TABLE, contact AS "Contact", cover AS "Cover" FROM stock WHERE region = getvariable('region');
-- @ask skip
SELECT 'CANARY-SKIP' AS s ::TABLE, salary FROM staff;
`;
const ROWS = {
  EU: [
    { c0: "SKU-1", Contact: "ann@example.com", Cover: 4 },
    { c0: "SKU-2", Contact: "bob@example.com", Cover: 9 },
  ],
  US: [{ c0: "SKU-9", Contact: "cy@example.com", Cover: 1 }],
};

// The fake model: each reply is a function of the request (or a message).
function fakeModel(replies) {
  const seen = [];
  return {
    seen,
    name: "browser-key",
    async send(request, { signal } = {}) {
      seen.push(JSON.stringify(request));
      if (signal?.aborted) throw Object.assign(new Error("aborted"), { name: "AbortError" });
      const r = replies.shift();
      if (!r) throw new Error("no reply scripted");
      return typeof r === "function" ? r(request) : r;
    },
  };
}
const use = (id, name, input) => ({ type: "tool_use", id, name, input });
const msg = (...content) => ({ content, stop_reason: "tool_use", usage: { input_tokens: 100, output_tokens: 20 } });

// The page: records every read.
function fakePage() {
  const reads = [];
  return {
    reads,
    readPanel: async (req) => {
      reads.push(JSON.parse(JSON.stringify(req)));
      if (req.panel === 0) return JSON.stringify([{ c0: "EU" }, { c0: "US" }]);
      if (req.panel === 1) return JSON.stringify(ROWS[req.vars?.region ?? "EU"]);
      throw new Error("unexpected panel");
    },
  };
}
const view = { tab: null, vars: { region: "EU" }, hidden: [] };

// 1. read, mask, answer, verify
{
  const session = new wasm.AskSession(DASH);
  const page = fakePage();
  let token = null;
  const model = fakeModel([
    msg(use("t1", "get_panel", { panel: 1 })),
    (req) => {
      const last = req.messages[req.messages.length - 1].content[0].content;
      token = /⟦E\d+⟧/.exec(last)[0];
      return msg(
        use("t2", "answer", {
          text: `SKU-1 has the least cover; its contact is ${token}. See https://evil.example/x`,
          scope: "answered",
          citations: [
            { panel: 1, label: "SKU-1", column: "Cover", value: "4", row: [{ name: "SKU", value: "SKU-1" }] },
            { panel: 1, label: "wrong row", column: "Cover", value: "4", row: [{ name: "SKU", value: "SKU-2" }] },
          ],
        })
      );
    },
  ]);
  const r = await runAsk({ transport: model, session, question: "Which SKU is lowest?", view, readPanel: page.readPanel });
  assert.equal(r.kind, "answer", JSON.stringify(r));
  for (const body of model.seen) {
    assert.ok(!body.includes("ann@example.com") && !body.includes("bob@example.com"), "an e-mail reached the model");
    assert.ok(!body.includes("CANARY-SKIP"), "an @ask skip panel reached the model");
  }
  assert.ok(model.seen[0].includes("SKU") && model.seen[0].includes("Inputs: region = EU"));
  assert.ok(r.answer.text.includes("ann@example.com"), "the token comes back as the real value");
  assert.equal(r.answer.citations[0].verified, true);
  assert.equal(r.answer.citations[1].verified, false, "the right value in the wrong row is not verified");
  assert.deepEqual(r.answer.panels, [1]);
  assert.deepEqual(page.reads, [{ panel: 1, vars: { region: "EU" } }]);
  // the turn: user, assistant, user (tool results), assistant, user
  assert.equal(r.turn.length, 5);

  // 2. a follow-up carries the conversation (and its pseudonyms)
  const model2 = fakeModel([
    (req) => {
      assert.ok(JSON.stringify(req.messages).includes(token), "the earlier turn is in the history");
      assert.equal(req.messages[0].role, "user");
      return msg(use("t3", "answer", { text: `Still ${token}.`, scope: "answered", citations: [] }));
    },
  ]);
  const r2 = await runAsk({ transport: model2, session, question: "And who to call?", view, readPanel: page.readPanel, turns: [r.turn] });
  assert.equal(r2.answer.text, "Still ann@example.com.");
  session.free();
}

// 3. an option input's value: the options are read first, then the panel with it
{
  const session = new wasm.AskSession(DASH);
  const page = fakePage();
  const model = fakeModel([
    msg(use("t1", "get_panel", { panel: 1, vars: [{ name: "region", value: "US" }] })),
    msg(use("t2", "get_panel", { panel: 1, vars: [{ name: "region", value: "MARS" }] })),
    (req) => {
      const res = req.messages[req.messages.length - 1].content[0];
      assert.equal(res.is_error, true);
      assert.match(res.content, /not one of the options/);
      return msg(use("t3", "answer", { text: "SKU-9.", scope: "answered", citations: [{ panel: 1, label: "SKU-9", column: "Cover", value: "1", row: [{ name: "SKU", value: "SKU-9" }] }] }));
    },
  ]);
  const r = await runAsk({ transport: model, session, question: "And in the US?", view, readPanel: page.readPanel });
  assert.equal(r.kind, "answer");
  assert.deepEqual(page.reads.slice(0, 2), [
    { panel: 0, vars: null },
    { panel: 1, vars: { region: "US" } },
  ]);
  assert.ok(!page.reads.some((x) => x.vars?.region === "MARS"), "a value outside the options never runs");
  assert.equal(r.answer.citations[0].verified, true);
  assert.deepEqual(r.answer.citations[0].vars, { region: "US" });
  session.free();
}

// 4. forged vars and a skipped panel: refused before anything runs
{
  const session = new wasm.AskSession(DASH);
  const page = fakePage();
  const results = [];
  const model = fakeModel([
    msg(use("t1", "get_panel", { panel: 1, vars: [{ name: "__claim_user", value: "x" }] }), use("t2", "get_panel", { panel: 2 }), use("t3", "get_panel", { panel: 1, vars: [{ name: "anofox_data", value: "/etc" }] })),
    (req) => {
      results.push(...req.messages[req.messages.length - 1].content);
      return msg(use("t4", "answer", { text: "No.", scope: "not_on_dashboard", citations: [] }));
    },
  ]);
  const r = await runAsk({ transport: model, session, question: "Show me salaries", view, readPanel: page.readPanel });
  assert.equal(r.answer.scope, "not_on_dashboard");
  assert.equal(page.reads.length, 0, "nothing ran");
  assert.ok(results.every((x) => x.is_error), JSON.stringify(results));
  // the view's own reserved names are refused too
  const bad = await runAsk({ transport: fakeModel([]), session, question: "q", view: { vars: { __claim_user: "x" } }, readPanel: page.readPanel });
  assert.equal(bad.kind, "error");
  session.free();
}

// 5. no answer tool: the fixed message, never the model's text; Cancel
{
  const session = new wasm.AskSession(DASH);
  const r = await runAsk({
    transport: fakeModel([{ content: [{ type: "text", text: "<img src=x onerror=alert(1)>" }], stop_reason: "end_turn", usage: {} }]),
    session,
    question: "q",
    view,
    readPanel: fakePage().readPanel,
  });
  assert.equal(r.kind, "error");
  assert.ok(r.text.startsWith("I couldn't answer that"), r.text);
  const ctl = new AbortController();
  ctl.abort();
  const c = await runAsk({ transport: fakeModel([msg()]), session, question: "q", view, readPanel: fakePage().readPanel, signal: ctl.signal });
  assert.equal(c.kind, "cancelled");
  // a page read that fails is a tool error, not a crash
  const failing = await runAsk({
    transport: fakeModel([msg(use("t1", "get_panel", { panel: 1 })), (req) => {
      const res = req.messages[req.messages.length - 1].content[0];
      assert.equal(res.is_error, true);
      assert.match(res.content, /^Panel 1 failed: the dashboard re-ran/);
      return msg(use("t2", "answer", { text: "Try again.", scope: "answered", citations: [] }));
    }]),
    session,
    question: "q",
    view,
    readPanel: async () => {
      throw new Error("the dashboard re-ran: ask again");
    },
  });
  assert.equal(failing.kind, "answer");
  session.free();
}

// 6. the conversation's caps: whole questions, the oldest first
{
  const t = (n) => [{ role: "user", content: [{ type: "text", text: "x".repeat(n) }] }];
  assert.equal(askTrim(Array.from({ length: 14 }, () => t(10))).length, 10);
  assert.equal(askTrim([t(200000), t(100000)]).length, 1);
}

// 7. static: the page binds vars as typed SET VARIABLE literals and puts
// them back; the panel's own SQL is run as the dashboard has it
{
  const app = read("web/app.js");
  const fn = app.slice(app.indexOf("async function askReadNow("), app.indexOf("// Where the builder's questions go"));
  assert.ok(fn.includes("await q(`SET VARIABLE ${k} = ${askLiteral(v)}`);"), "vars through SET VARIABLE");
  assert.ok(fn.includes("RESET VARIABLE ${k}") && fn.includes("await q(`SET VARIABLE ${k} = ${lit}`)"), "vars put back");
  assert.ok(/classifyStatement\(st\.sql, \{ known: new Set\(pathsOf\(st\.sql\)\) \}\)\.cls !== 1/.test(fn), "read-only statements only");
  assert.ok(!/st\.sql\.replace|sql\.replace\(/.test(fn), "no textual substitution into the panel's SQL");
  assert.ok(fn.includes("/^[A-Za-z_]\\w*$/.test(k)"), "variable names are identifiers");
  const lit = app.slice(app.indexOf("function askLiteral("), app.indexOf("// One panel read for Ask"));
  const askLiteral = new Function(`${lit}; return askLiteral;`)();
  assert.equal(askLiteral("it's"), "'it''s'");
  assert.equal(askLiteral(["a'b", "c"]), "['a''b','c']::VARCHAR[]");
  assert.equal(askLiteral(3), "3");
  assert.equal(askLiteral(Infinity), "NULL");
  assert.equal(askLiteral(true), "TRUE");
  assert.equal(askLiteral(null), "NULL");
}

console.log("ask_builder_test: ok");
