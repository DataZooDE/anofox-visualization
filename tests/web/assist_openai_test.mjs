// #843 (M3-1): transport (c), an OpenAI-compatible endpoint — the
// translation both ways (tools, tool calls, tool results, errors), the
// transport itself against a fake fetch, the configuration and the CSP's
// connect-src, the separate key slot, and three of the loop's scripted
// conversations replayed through (c) end to end.
import { readFileSync } from "fs";
import path from "path";
import assert from "assert/strict";

const here = path.dirname(new URL(import.meta.url).pathname);
const root = path.resolve(here, "../..");
const O = await import(path.join(root, "web/assist-openai.js"));
const A = await import(path.join(root, "web/assist.js"));
const C = await import(path.join(root, "web/assist-csp.js"));
const wasm = await import(path.join(root, "web/pkg/anofox_visualization.js"));
wasm.initSync({ module: readFileSync(path.join(root, "web/pkg/anofox_visualization_bg.wasm")) });

// ---- the endpoint and model rules ----
for (const ok of ["http://localhost:11434/v1", "http://127.0.0.1:8000/v1", "http://[::1]:1234/v1", "https://llm.example.com/v1", "https://gw.example.com:8443/openai/v1"]) assert.ok(O.endpointOk(ok), ok);
for (const bad of ["http://llm.example.com/v1", "https://user@x.example/v1", "https://x.example/v1?k=1", "https://x.example/v1#a", "ftp://x.example", "javascript:alert(1)", "https://x .example/v1", "", null, "http://localhost.evil.example/v1"]) assert.ok(!O.endpointOk(bad), String(bad));
assert.equal(O.endpointOrigin("http://localhost:11434/v1"), "http://localhost:11434");
assert.equal(O.endpointOrigin("http://evil.example/v1"), null);
for (const ok of ["llama3.1:8b", "qwen2.5-coder:32b", "mistralai/Mistral-7B-Instruct-v0.3", "gpt-4o"]) assert.ok(O.modelIdOk(ok), ok);
for (const bad of ["", " x", "a b", "x;y", "-x", "a".repeat(129)]) assert.ok(!O.modelIdOk(bad), bad);

// ---- request: Messages → chat completions ----
{
  const req = {
    model: "llama3.1:8b",
    max_tokens: 4000,
    system: [{ type: "text", text: "You build dashboards." }, { type: "text", text: "# Guide", cache_control: { type: "ephemeral" } }],
    tools: [{ name: "run_sql", description: "Run a query", input_schema: { type: "object", properties: { sql: { type: "string" } }, required: ["sql"] } }],
    thinking: { type: "adaptive" },
    output_config: { effort: "high" },
    cache_control: { type: "ephemeral" },
    messages: [
      { role: "user", content: [{ type: "text", text: "Revenue by region" }] },
      { role: "assistant", content: [{ type: "thinking", thinking: "hm", signature: "s" }, { type: "text", text: "Let me look." }, { type: "tool_use", id: "t1", name: "run_sql", input: { sql: "SELECT 1" } }, { type: "tool_use", id: "t2", name: "run_sql", input: { sql: "SELECT 2" } }] },
      { role: "user", content: [{ type: "tool_result", tool_use_id: "t1", content: '{"rows":1}' }, { type: "tool_result", tool_use_id: "t2", is_error: true, content: [{ type: "text", text: "refused" }] }, { type: "text", text: "and sort it" }] },
    ],
  };
  const b = O.toChat(req);
  assert.deepEqual(Object.keys(b).sort(), ["max_tokens", "messages", "model", "stream", "tools"]);
  assert.equal(b.stream, false);
  assert.deepEqual(b.messages[0], { role: "system", content: "You build dashboards.\n\n# Guide" });
  assert.deepEqual(b.messages[1], { role: "user", content: "Revenue by region" });
  assert.deepEqual(b.messages[2], {
    role: "assistant",
    content: "Let me look.",
    tool_calls: [
      { id: "t1", type: "function", function: { name: "run_sql", arguments: '{"sql":"SELECT 1"}' } },
      { id: "t2", type: "function", function: { name: "run_sql", arguments: '{"sql":"SELECT 2"}' } },
    ],
  });
  assert.deepEqual(b.messages.slice(3), [
    { role: "tool", tool_call_id: "t1", content: '{"rows":1}' },
    { role: "tool", tool_call_id: "t2", content: "Error: refused" },
    { role: "user", content: "and sort it" },
  ]);
  assert.deepEqual(b.tools[0], { type: "function", function: { name: "run_sql", description: "Run a query", parameters: req.tools[0].input_schema } });
  assert.doesNotMatch(JSON.stringify(b), /thinking|cache_control|output_config|signature/);
  assert.deepEqual(O.toChat({ ...req, tool_choice: { type: "tool", name: "run_sql" } }).tool_choice, { type: "function", function: { name: "run_sql" } });
  assert.equal(O.toChat({ ...req, tool_choice: { type: "any" } }).tool_choice, "required");
  assert.equal(O.toChat({ ...req, tool_choice: { type: "auto" } }).tool_choice, undefined);
  // an assistant turn of tool calls only: content null, as the API wants it
  assert.equal(O.toChat({ messages: [{ role: "assistant", content: [{ type: "tool_use", id: "x", name: "n", input: {} }] }] }).messages[0].content, null);
  assert.equal(O.toChat({ messages: [{ role: "user", content: "plain" }] }).messages[0].content, "plain");
}

// ---- reply: chat completions → Messages ----
{
  const m = O.fromChat({
    choices: [{ finish_reason: "tool_calls", message: { role: "assistant", content: "Checking.", tool_calls: [{ id: "c1", type: "function", function: { name: "run_sql", arguments: '{"sql":"SELECT 1"}' } }, { id: "c2", type: "function", function: { name: "check", arguments: "not json" } }, { type: "function", function: { name: "x", arguments: "[1]" } }] } }],
    usage: { prompt_tokens: 120, completion_tokens: 30 },
  });
  assert.equal(m.stop_reason, "tool_use");
  assert.deepEqual(m.content, [
    { type: "text", text: "Checking." },
    { type: "tool_use", id: "c1", name: "run_sql", input: { sql: "SELECT 1" } },
    { type: "tool_use", id: "c2", name: "check", input: {} },
    { type: "tool_use", id: "call_2", name: "x", input: {} },
  ]);
  assert.deepEqual(m.usage, { input_tokens: 120, output_tokens: 30, cache_read_input_tokens: 0, cache_creation_input_tokens: 0 });
  assert.equal(O.fromChat({ choices: [{ finish_reason: "stop", message: { content: "Hi" } }] }).stop_reason, "end_turn");
  assert.equal(O.fromChat({ choices: [{ finish_reason: "length", message: { content: "…" } }] }).stop_reason, "max_tokens");
  assert.equal(O.fromChat({ choices: [{ finish_reason: "content_filter", message: { content: "" } }] }).stop_reason, "refusal");
  // some servers say "stop" with tool calls
  assert.equal(O.fromChat({ choices: [{ finish_reason: "stop", message: { tool_calls: [{ id: "a", function: { name: "n", arguments: "{}" } }] } }] }).stop_reason, "end_turn");
  assert.equal(O.fromChat({ choices: [{ finish_reason: null, message: { tool_calls: [{ id: "a", function: { name: "n", arguments: "{}" } }] } }] }).stop_reason, "tool_use");
  assert.deepEqual(O.fromChat({}).content, []);
}

// ---- the transport ----
{
  const calls = [];
  const fake = (status, body) => async (url, init) => {
    calls.push({ url, init });
    return { ok: status < 300, status, json: async () => body };
  };
  const t = O.openaiTransport({ endpoint: "http://localhost:11434/v1/", fetchImpl: fake(200, { choices: [{ finish_reason: "stop", message: { content: "Hello" } }] }) });
  assert.equal(t.name, "openai");
  assert.equal(t.destination, "http://localhost:11434/v1/chat/completions");
  const m = await t.send({ model: "m", max_tokens: 10, messages: [{ role: "user", content: "hi" }] });
  assert.equal(m.content[0].text, "Hello");
  assert.equal(calls[0].url, "http://localhost:11434/v1/chat/completions");
  assert.equal(calls[0].init.credentials, "omit");
  assert.equal(calls[0].init.headers.authorization, undefined, "no key, no Authorization");
  await O.openaiTransport({ endpoint: "https://gw.example.com/v1", apiKey: "tok-12345678", fetchImpl: fake(200, { choices: [] }) }).send({ messages: [] });
  assert.equal(calls[1].init.headers.authorization, "Bearer tok-12345678");
  await assert.rejects(() => O.openaiTransport({ endpoint: "http://localhost:1/v1", fetchImpl: fake(401, { error: { message: "missing key" } }) }).send({ messages: [] }), (e) => e.status === 401 && e.error.error.message === "missing key" && /missing key/.test(A.describeError(e, "openai")) && /add one/.test(A.describeError(e, "openai")));
  await assert.rejects(() => O.openaiTransport({ endpoint: "http://localhost:1/v1", fetchImpl: fake(200, { nope: 1 }) }).send({ messages: [] }), /not a chat completion/);
  assert.throws(() => O.openaiTransport({ endpoint: "http://evil.example/v1" }), /must be https/);
  // #941: a published site's own endpoint — a same-origin path, the
  // viewer's cookies along, never a key; and only with sameOrigin: true
  const n = calls.length;
  await O.openaiTransport({ endpoint: "/api/llm/", sameOrigin: true, apiKey: "tok-12345678", fetchImpl: fake(200, { choices: [] }) }).send({ messages: [] });
  assert.equal(calls[n].url, "/api/llm/chat/completions");
  assert.equal(calls[n].init.credentials, "same-origin");
  assert.equal(calls[n].init.headers.authorization, undefined, "no key to the site's endpoint");
  assert.throws(() => O.openaiTransport({ endpoint: "/api/llm" }), /must be https/, "a path needs sameOrigin");
  for (const bad of ["https://gw.example.com/v1", "//evil.example/v1", "/api/../x", "/api?x=1", "/", "/a b", "api/llm"]) {
    assert.equal(O.sitePathOk(bad), false, bad);
    assert.throws(() => O.openaiTransport({ endpoint: bad, sameOrigin: true }), /a path like/, bad);
  }
  for (const ok of ["/api/llm", "/api/llm/", "/v1", "/x/y-z_1.2"]) assert.equal(O.sitePathOk(ok), true, ok);
  assert.match(A.describeError(new TypeError("Failed to fetch"), "openai"), /could not reach the model endpoint/);
}

// ---- configuration ----
{
  const cfg = (block, opts = {}) => A.assistConfig({ enabled: true, ...block }, opts);
  const ok = cfg({ transport: "openai", endpoint: "http://localhost:11434/v1/", models: { default: "llama3.1:8b" } });
  assert.equal(ok.enabled, true);
  assert.equal(ok.endpoint, "http://localhost:11434/v1");
  assert.deepEqual(ok.models, { default: "llama3.1:8b", harder: "llama3.1:8b" });
  assert.deepEqual(cfg({ transport: "openai", endpoint: "https://gw.example.com/v1", models: { default: "a", harder: "b-large" } }).models, { default: "a", harder: "b-large" });
  assert.equal(cfg({ transport: "openai", models: { default: "m" } }).reason, "openai-needs-an-endpoint");
  assert.equal(cfg({ transport: "openai", endpoint: "http://llm.example.com/v1", models: { default: "m" } }).reason, "openai-needs-an-endpoint");
  assert.equal(cfg({ transport: "openai", endpoint: "http://localhost:1/v1" }).reason, "openai-needs-a-model");
  assert.equal(cfg({ transport: "openai", endpoint: "http://localhost:1/v1", models: { default: "m" } }, { live: true }).reason, "live-needs-serve", "an authoring server uses its proxy");
  assert.equal(cfg({ transport: "openai", endpoint: "http://localhost:1/v1", models: { default: "m" } }, { served: true }).enabled, false);
  // #843 part 2: the server's proxy may name an endpoint's own models
  assert.deepEqual(cfg({ transport: "serve", models: { default: "llama3.1:8b", harder: "x y" } }, { live: true }).models, { default: "llama3.1:8b", harder: "claude-opus-5-5" });
  // a Claude transport keeps its own model rule and has no endpoint
  const bk = cfg({ models: { default: "llama3.1:8b" } });
  assert.equal(bk.models.default, "claude-sonnet-5");
  assert.equal(bk.endpoint, undefined);
  // an inherited endpoint never counts (own properties only)
  Object.prototype.endpoint = "http://localhost:9/v1";
  try {
    assert.equal(cfg({ transport: "openai", models: { default: "m" } }).reason, "openai-needs-an-endpoint");
  } finally {
    delete Object.prototype.endpoint;
  }
}

// ---- the CSP's connect-src ----
{
  const csp = C.assistCsp({ connect: ["http://localhost:11434", "https://gw.example.com", "http://evil.example", "https://x.example/path", "javascript:alert(1)", "*", "http://localhost:1 https://evil.example"] });
  const connect = csp.split("; ").find((d) => d.startsWith("connect-src "));
  assert.ok(connect.includes(" http://localhost:11434") && connect.includes(" https://gw.example.com"), connect);
  for (const bad of ["evil.example", "/path", "javascript", " * "]) assert.ok(!connect.includes(bad), `${bad} in ${connect}`);
  assert.equal(C.assistCsp({}).split("; ").find((d) => d.startsWith("connect-src ")).split(" ").length, 6, "nothing extra without an endpoint");
}

// ---- separate key slots ----
{
  const mem = () => {
    const m = new Map();
    return { getItem: (k) => m.get(k) ?? null, setItem: (k, v) => m.set(k, v), removeItem: (k) => m.delete(k), m };
  };
  const s = mem(),
    l = mem();
  A.keyStore(s, l).set("sk-ant-api03-" + "a".repeat(30));
  const oai = A.keyStore(s, l, { slot: A.OPENAI_KEY_SLOT });
  assert.equal(oai.get(), "", "an Anthropic key is never the endpoint's");
  oai.set("gateway-token-1");
  assert.equal(A.keyStore(s, l).get().startsWith("sk-ant-"), true);
  assert.equal(s.m.get(A.OPENAI_KEY_SLOT), "gateway-token-1");
  oai.clear();
  assert.ok(A.keyStore(s, l).get(), "clearing one slot leaves the other");
  assert.equal(A.keySlotFor("openai"), A.OPENAI_KEY_SLOT);
  assert.equal(A.keySlotFor("browser-key"), A.KEY_SLOT);
  assert.throws(() => A.keyStore(s, l, { slot: "other" }), /no such key slot/);
}

// ---- three scripted conversations through (c), end to end ----
// The fake endpoint answers each chat request with the next scripted turn
// (written as Messages, sent back as a chat completion) and checks every
// request: valid roles, every tool message answers a tool call before it.
const prefix = A.buildPrefix({ instructions: "You build dashboards.", guide: "Guide.", rolesJson: wasm.roles_json(), version: "test" });
let ids = 0;
const use = (name, input) => ({ type: "tool_use", id: `toolu_${++ids}`, name, input });
const text = (t) => ({ type: "text", text: t });
const asChat = (content, stop) => ({
  choices: [
    {
      finish_reason: stop === "tool_use" ? "tool_calls" : "stop",
      message: {
        role: "assistant",
        content: content.filter((b) => b.type === "text").map((b) => b.text).join("") || null,
        tool_calls: content.filter((b) => b.type === "tool_use").map((b) => ({ id: b.id, type: "function", function: { name: b.name, arguments: JSON.stringify(b.input) } })),
      },
    },
  ],
  usage: { prompt_tokens: 500, completion_tokens: 40 },
});
function endpoint(turns) {
  const sent = [];
  const fetchImpl = async (_url, init) => {
    const body = JSON.parse(init.body);
    sent.push(body);
    const open = new Set();
    for (const m of body.messages) {
      assert.ok(["system", "user", "assistant", "tool"].includes(m.role), m.role);
      if (m.role === "assistant") for (const c of m.tool_calls || []) open.add(c.id);
      if (m.role === "tool") assert.ok(open.has(m.tool_call_id), `tool result ${m.tool_call_id} answers a call`);
    }
    assert.equal(body.model, "llama3.1:8b");
    assert.ok(body.tools.every((t) => t.type === "function" && t.function.parameters?.type === "object"));
    const t = turns.shift();
    if (!t) throw new Error("no more turns");
    return { ok: true, status: 200, json: async () => asChat(t[0], t[1] || "tool_use") };
  };
  return { sent, transport: O.openaiTransport({ endpoint: "http://localhost:11434/v1", fetchImpl }) };
}
const GOOD = "SELECT region ::XAXIS, sum(x) ::BARCHART FROM sales GROUP BY ALL ORDER BY 2 DESC";
const BAD = "SELECT region ::XAXIS, sum(nope) ::BARCHART FROM sales GROUP BY ALL";
function fakeTools() {
  return {
    async list_tables() {
      return '[{"name":"sales","rows":3}]';
    },
    async profile_table() {
      return '{"table":"sales","columns":[]}';
    },
    async run_sql() {
      return '{"rows":3}';
    },
    async check(sql) {
      const bad = /nope/.test(sql);
      const issues = bad ? [{ stmt: 1, severity: "error", code: "sql-error", message: 'Binder Error: Referenced column "nope" not found' }] : [];
      return { ok: !bad, issues, text: JSON.stringify({ ok: !bad, diagnostics: issues }) };
    },
    async gate(sql) {
      return { allowed: true, statements: A.classifyScript(sql).danger.map((d) => ({ sql: d.sql, reason: d.reason })) };
    },
  };
}
const propose = (sql) => use("propose_dashboard", { sql, summary: "Revenue by region", panels: [{ stmt: 1, title: "Revenue by region", why: "the ranking" }], open_issues: [] });
const ctx = A.buildContext({ mode: "new", tables: [{ name: "sales", rows: 3 }] });
const run = (transport) => A.runAssistant({ transport, prefix, ctx, request: "Revenue by region", tools: fakeTools(), model: "llama3.1:8b" });
{
  // 1. self-correction: two tools, a failing proposal, the fix
  const e = endpoint([[[text("Let me look."), use("profile_table", { table: "sales" }), use("list_tables", { filter: "" })]], [[propose(BAD)]], [[text("Fixed."), propose(GOOD)]]]);
  const r = await run(e.transport);
  assert.equal(r.kind, "proposal");
  assert.equal(r.proposal.passed, true);
  assert.equal(r.proposal.sql, GOOD);
  assert.equal(r.rounds, 1);
  assert.equal(e.sent.length, 3);
  const back = e.sent[2].messages.filter((m) => m.role === "tool");
  assert.match(back.at(-1).content, /^Error: [\s\S]*nope[\s\S]*Fix these/);
  assert.equal(r.usage.input_tokens, 1500);
}
{
  // 2. a plain answer, no tool calls
  const e = endpoint([[[text("Which table do you mean?")], "end_turn"]]);
  const r = await run(e.transport);
  assert.equal(r.text, "Which table do you mean?");
  assert.notEqual(r.kind, "proposal");
}
{
  // 3. a tool call with arguments the model got wrong, then a good proposal
  const e = endpoint([[[use("run_sql", { query: "SELECT 1" })]], [[propose(GOOD)]]]);
  const r = await run(e.transport);
  assert.equal(r.kind, "proposal");
  assert.equal(r.proposal.passed, true);
  const bad = e.sent[1].messages.find((m) => m.role === "tool");
  assert.match(bad.content, /^Error: .*INVALID_INPUT/);
}

console.log("assist_openai_test: ok");
