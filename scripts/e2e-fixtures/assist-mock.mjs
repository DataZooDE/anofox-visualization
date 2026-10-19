// #244: a scripted Anthropic Messages API, for scripts/assist-e2e.mjs.
// Never a real call — api.anthropic.com is intercepted at the browser
// network layer (installMock, via Playwright's context.route) and answered
// from here. The event sequence matches tests/web/assist_test.mjs's own
// fake server (text/event-stream, the same event names and shapes the
// real API sends), so the same vendored SDK (web/vendor/anthropic-sdk-*.mjs)
// that production code uses parses it identically.

// A text block becomes text_delta events, chunked.
function textEvents(blockIndex, t, chunk = 20) {
  const out = [];
  for (let i = 0; i < t.length || i === 0; i += chunk) out.push({ index: blockIndex, delta: { type: "text_delta", text: t.slice(i, i + chunk) } });
  return out;
}
// A tool_use block becomes input_json_delta events of its JSON, chunked —
// eager_input_streaming's own shape (propose_dashboard streams as it goes).
function toolUseEvents(blockIndex, input, chunk = 40) {
  const json = JSON.stringify(input);
  const out = [];
  for (let i = 0; i < json.length || i === 0; i += chunk) out.push({ index: blockIndex, delta: { type: "input_json_delta", partial_json: json.slice(i, i + chunk) } });
  return out;
}

// One assistant turn's content, in the shape `sse()` below renders:
//   text("reply text")
//   toolUse(id, name, input)
export const text = (t) => ({ kind: "text", text: t });
export const toolUse = (id, name, input) => ({ kind: "tool_use", id, name, input });

// The full SSE body (a string) for one turn: {content: [text(...)|toolUse(...), …],
// stop_reason, usage}. Mirrors tests/web/assist_test.mjs:640-654's event
// sequence and shapes exactly (message_start -> per-block start/delta/stop
// -> message_delta -> message_stop).
export function sse({ content = [], stop_reason = "end_turn", usage = {} } = {}) {
  const lines = [];
  const ev = (type, data) => lines.push(`event: ${type}\ndata: ${JSON.stringify({ type, ...data })}\n\n`);
  ev("message_start", {
    message: {
      id: "msg_mock_" + Math.random().toString(36).slice(2),
      type: "message",
      role: "assistant",
      model: "claude-sonnet-5",
      content: [],
      stop_reason: null,
      usage: { input_tokens: 100, output_tokens: 10, cache_read_input_tokens: 1000, cache_creation_input_tokens: 0, ...usage },
    },
  });
  content.forEach((block, i) => {
    if (block.kind === "text") {
      ev("content_block_start", { index: i, content_block: { type: "text", text: "" } });
      for (const d of textEvents(i, block.text)) ev("content_block_delta", d);
      ev("content_block_stop", { index: i });
    } else {
      ev("content_block_start", { index: i, content_block: { type: "tool_use", id: block.id, name: block.name, input: {} } });
      for (const d of toolUseEvents(i, block.input)) ev("content_block_delta", d);
      ev("content_block_stop", { index: i });
    }
  });
  ev("message_delta", { delta: { stop_reason, stop_sequence: null }, usage: { output_tokens: usage.output_tokens ?? 20 } });
  ev("message_stop", {});
  return lines.join("");
}

// Routes https://api.anthropic.com/** inside `context` to `handler(body, n)`
// (n: the 1-based call count), which returns either a turn
// ({content, stop_reason, usage}), {status, json} (e.g. a 401), or
// {delayMs} (a turn that never answers, for the Cancel case). `mock.requests`
// records every POST {url, headers, body}.
export function installMock(context, handler) {
  const mock = { requests: [] };
  let n = 0;
  context.route("https://api.anthropic.com/**", async (route) => {
    const req = route.request();
    if (req.method() === "OPTIONS") {
      await route.fulfill({
        status: 204,
        headers: {
          "access-control-allow-origin": req.headers().origin || "*",
          "access-control-allow-headers": "*",
          "access-control-allow-methods": "POST, OPTIONS",
        },
      });
      return;
    }
    let body = {};
    try {
      body = JSON.parse(req.postData() || "{}");
    } catch (_) {}
    n++;
    mock.requests.push({ url: req.url(), headers: req.headers(), body });
    // Diagnostic: a propose_dashboard tool_result the harness marked
    // is_error while the test expected a clean pass means the fixture
    // itself (not the code under test) is broken — fail loudly rather
    // than silently producing a confusing downstream assertion failure.
    const lastUser = [...body.messages || []].reverse().find((m) => m.role === "user");
    const errResult = Array.isArray(lastUser?.content) ? lastUser.content.find((c) => c.type === "tool_result" && c.is_error && /propose_dashboard/i.test(JSON.stringify(c.content || ""))) : null;
    if (errResult && handler.expectPass) throw new Error(`fixture failed the check: ${JSON.stringify(errResult.content).slice(0, 500)}`);
    const out = await handler(body, n);
    if (out?.delayMs) {
      await new Promise((r) => setTimeout(r, out.delayMs));
      // never actually answers within the test's own timeout — the request
      // stays pending until the page aborts it (Cancel).
      return;
    }
    if (out?.status) {
      await route.fulfill({ status: out.status, headers: { "access-control-allow-origin": req.headers().origin || "*" }, contentType: "application/json", body: JSON.stringify(out.json || {}) });
      return;
    }
    await route.fulfill({
      status: 200,
      headers: { "access-control-allow-origin": req.headers().origin || "*" },
      contentType: "text/event-stream",
      body: sse(out),
    });
  });
  return mock;
}

// addInitScript: wraps every Worker so `window.__dbSpy = {workers: [url, …],
// msgs: [{w, type, sql, t}, …]}` records every postMessage. Worker 0 is the
// page's own DuckDB (created first, at boot); a later worker is an
// assistant engine. This is the C-1/R9 instrument: the e2e asserts zero
// `w===0` messages while a preview is active.
export function workerSpyInit() {
  const W = window.Worker;
  const L = (window.__dbSpy = { workers: [], msgs: [] });
  window.Worker = class extends W {
    constructor(u, o) {
      super(u, o);
      const id = L.workers.push(String(u)) - 1;
      const pm = this.postMessage.bind(this);
      this.postMessage = (m, t) => {
        const d = m && m.data;
        L.msgs.push({
          w: id,
          type: m && m.type,
          sql: typeof d === "string" ? d : Array.isArray(d) ? d.find((x) => typeof x === "string") : undefined,
          t: performance.now(),
        });
        return t ? pm(m, t) : pm(m);
      };
    }
  };
}

// addInitScript: records every securitypolicyviolation into window.__csp.
export function violationInit() {
  window.__csp = [];
  document.addEventListener("securitypolicyviolation", (e) => {
    window.__csp.push({ effectiveDirective: e.effectiveDirective, blockedURI: e.blockedURI });
  });
}

// The static server's in-memory app-config.json body.
export const cfg = (enabled, extra = {}) => ({ assistant: { enabled, ...extra } });
