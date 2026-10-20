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

// A real local HTTPS server, started once per process (lazily) and reused
// by every installMock() call: route.fulfill() delivers an SSE body as a
// single atomic chunk, and the vendored SDK's stream parser drops every
// content_block_* event when the whole response arrives in one read() —
// verified directly against the SDK (message_start/message_delta/
// message_stop come through; the text/tool_use content does not). A real
// server, actually streamed with small, separately-flushed writes (exactly
// how tests/web/assist_test.mjs's own fake server behaves, and how the
// real API behaves), round-trips correctly — so https://api.anthropic.com/**
// is redirected (route.continue({url})) to this server rather than
// answered in place. The server needs TLS (route.continue keeps the
// request's scheme), so a throwaway self-signed cert is generated once per
// process (openssl, into the OS temp dir under TMPDIR — never the repo);
// callers need `ignoreHTTPSErrors: true` on their browser context.
import { execFileSync } from "child_process";
import { mkdtempSync, readFileSync as rfsSync, writeFileSync } from "fs";
import { createServer } from "https";
import os from "os";
import path from "path";

let mockServerP = null;
// The currently active per-test mock state; tests run sequentially (one
// context at a time) in scripts/assist-e2e.mjs, so a single shared slot,
// swapped by each installMock() call, never crosses two tests in practice —
// each context is closed before the next test's installMock() runs.
let active = null;

function ensureMockServer() {
  if (mockServerP) return mockServerP;
  mockServerP = (async () => {
    const dir = mkdtempSync(path.join(process.env.TMPDIR || os.tmpdir(), "assist-mock-"));
    const key = path.join(dir, "key.pem");
    const cert = path.join(dir, "cert.pem");
    execFileSync("openssl", ["req", "-x509", "-newkey", "rsa:2048", "-keyout", key, "-out", cert, "-days", "2", "-nodes", "-subj", "/CN=127.0.0.1"], { stdio: ["ignore", "ignore", "ignore"] });
    const server = createServer({ key: rfsSync(key), cert: rfsSync(cert) }, async (req, res) => {
      const origin = req.headers.origin || "*";
      if (req.method === "OPTIONS") {
        res.writeHead(204, { "access-control-allow-origin": origin, "access-control-allow-headers": "*", "access-control-allow-methods": "POST, OPTIONS" });
        res.end();
        return;
      }
      // sec note (harness-only, never production code): the request body is
      // read in the background, never awaited before the response starts —
      // awaiting req's 'end' event first (reading the POST body before
      // responding) was tried and measurably breaks the vendored SDK's SSE
      // parsing: content_block_*/message_delta events silently vanish
      // (finalMessage() resolves with an empty `content` and the wrong
      // `stop_reason`, confirmed by a dedicated isolated repro against the
      // SDK directly — unrelated to this file's own event framing, which is
      // provably correct independently). Responding immediately and
      // collecting the body purely for `mock.requests`/the propose_dashboard
      // diagnostic (read only after the test's own await, never gating the
      // response) avoids it.
      let raw = "";
      req.on("data", (d) => (raw += d));
      const bodyP = new Promise((resolve) => req.on("end", () => resolve(raw)));
      const a = active;
      const n = a ? ++a.n : 0;
      if (process.env.ASSIST_E2E_DEBUG) console.log("[mock]", req.method, req.url, "n=", n);
      if (a) {
        bodyP.then((raw) => {
          let body = {};
          try {
            body = JSON.parse(raw || "{}");
          } catch (_) {}
          a.mock.requests.push({ url: `https://api.anthropic.com${req.url}`, headers: req.headers, body });
          const lastUser = [...(body.messages || [])].reverse().find((m) => m.role === "user");
          const errResult = Array.isArray(lastUser?.content) ? lastUser.content.find((c) => c.type === "tool_result" && c.is_error && /propose_dashboard/i.test(JSON.stringify(c.content || ""))) : null;
          if (errResult && a.handler.expectPass) throw new Error(`fixture failed the check: ${JSON.stringify(errResult.content).slice(0, 500)}`);
        });
      }
      const out = a ? await a.handler({}, n) : { content: [] };
      if (out?.delayMs) {
        await new Promise((r) => setTimeout(r, out.delayMs));
        res.destroy(); // never actually answers — the page aborts it (Cancel)
        return;
      }
      if (out?.status) {
        res.writeHead(out.status, { "access-control-allow-origin": origin, "content-type": "application/json" });
        res.end(JSON.stringify(out.json || {}));
        return;
      }
      res.writeHead(200, { "access-control-allow-origin": origin, "content-type": "text/event-stream" });
      // `out` is either a turn description ({content, stop_reason, usage}),
      // built into an SSE body here, or already an SSE string (a handler
      // that called sse() itself, as scripts/assist-e2e.mjs's handlers do)
      // — both call shapes are accepted so a handler is free to pre-build
      // it. (Named `sseBody`, not `text`, so it never shadows this file's
      // own exported `text()` helper.)
      const sseBody = typeof out === "string" ? out : sse(out);
      let i = 0;
      const chunk = () => {
        if (i >= sseBody.length) {
          res.end();
          return;
        }
        res.write(sseBody.slice(i, i + 64));
        i += 64;
        setTimeout(chunk, 4);
      };
      chunk();
    });
    const port = await new Promise((resolve) => server.listen(0, "127.0.0.1", () => resolve(server.address().port)));
    return { server, port };
  })();
  return mockServerP;
}

// Routes https://api.anthropic.com/** inside `context` to `handler(body, n)`
// (n: the 1-based call count), which returns either a turn
// ({content, stop_reason, usage}), {status, json} (e.g. a 401), or
// {delayMs} (a turn that never answers, for the Cancel case). `mock.requests`
// records every POST {url, headers, body}. The context needs
// `ignoreHTTPSErrors: true` (browser.newContext(...)).
export async function installMock(context, handler) {
  const { port } = await ensureMockServer();
  const mock = { requests: [] };
  active = { mock, handler, n: 0 };
  await context.route("https://api.anthropic.com/**", async (route) => {
    await route.continue({ url: `https://127.0.0.1:${port}${new URL(route.request().url()).pathname}` });
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
