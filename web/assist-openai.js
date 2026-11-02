// Transport (c) of the builder's assistant (#843, plan-ai-assistant.md §2.1):
// an OpenAI-compatible `/chat/completions` endpoint — a local model (Ollama,
// vLLM, LM Studio) or a company gateway. The loop speaks the Anthropic
// Messages shape only; this module translates one request there and the
// reply back:
//   * system blocks → one `system` message; `tools` → `functions`;
//   * `tool_use` / `tool_result` blocks ⇄ `tool_calls` / `role: "tool"`;
//   * no caching, no thinking, no effort (dropped: the endpoint doesn't know
//     them), no streaming (one reply per call).
// A pure ES module (no DOM, no SDK): tests/web/assist_openai_test.mjs runs it
// under Node against a fake endpoint.
import { ownOpts } from "./lint.js";

// https://host[:port][/path], or http:// to a loopback host (a local model);
// no userinfo, query or fragment. The same rule as the server's
// `check_base_url` (src/host/assist.rs).
export function endpointOk(url) {
  if (typeof url !== "string" || url.length > 512 || !/^[\x21-\x7e]+$/.test(url) || /[?#@\\]/.test(url)) return false;
  const m = /^(https?):\/\/([^/]+)(\/.*)?$/i.exec(url);
  if (!m) return false;
  if (m[1].toLowerCase() === "https") return /^[a-z0-9.-]+(:\d{1,5})?$/i.test(m[2]);
  return /^(127\.\d{1,3}\.\d{1,3}\.\d{1,3}|localhost|\[::1\])(:\d{1,5})?$/i.test(m[2]);
}
// The endpoint's origin, for the assistant's CSP connect-src.
export const endpointOrigin = (url) => (endpointOk(url) ? new URL(url).origin : null);
// A model id as a local server names it: `llama3.1:8b`, `qwen2.5-coder:32b`,
// `org/model-name`.
export const modelIdOk = (m) => typeof m === "string" && /^[A-Za-z0-9][A-Za-z0-9._:/-]{0,127}$/.test(m);

const text = (content) =>
  typeof content === "string"
    ? content
    : (Array.isArray(content) ? content : [])
        .filter((b) => b?.type === "text")
        .map((b) => String(b.text ?? ""))
        .join("\n\n");

// One Messages request → a chat completions body.
export function toChat(request) {
  const r = ownOpts(request);
  const messages = [];
  const system = text(r.system);
  if (system) messages.push({ role: "system", content: system });
  for (const m of Array.isArray(r.messages) ? r.messages : []) {
    const blocks = typeof m?.content === "string" ? [{ type: "text", text: m.content }] : Array.isArray(m?.content) ? m.content : [];
    if (m?.role === "assistant") {
      const calls = blocks
        .filter((b) => b?.type === "tool_use")
        .map((b) => ({ id: String(b.id), type: "function", function: { name: String(b.name), arguments: JSON.stringify(b.input ?? {}) } }));
      const t = text(blocks);
      messages.push({ role: "assistant", content: t || null, ...(calls.length ? { tool_calls: calls } : {}) });
    } else {
      // tool results first (they answer the previous assistant turn), then the user's text
      for (const b of blocks.filter((x) => x?.type === "tool_result")) {
        const c = text(b.content);
        messages.push({ role: "tool", tool_call_id: String(b.tool_use_id), content: b.is_error ? `Error: ${c}` : c });
      }
      const t = text(blocks);
      if (t) messages.push({ role: "user", content: t });
    }
  }
  const body = { model: r.model, messages, max_tokens: r.max_tokens, stream: false };
  if (Array.isArray(r.tools) && r.tools.length) {
    body.tools = r.tools.map((t) => ({ type: "function", function: { name: t.name, description: t.description || "", parameters: t.input_schema || { type: "object", properties: {} } } }));
    const c = ownOpts(r.tool_choice);
    if (c.type === "tool" && c.name) body.tool_choice = { type: "function", function: { name: c.name } };
    else if (c.type === "any") body.tool_choice = "required";
    else if (c.type === "none") body.tool_choice = "none";
  }
  return body;
}

const STOP = { stop: "end_turn", tool_calls: "tool_use", function_call: "tool_use", length: "max_tokens", content_filter: "refusal" };

// A chat completions reply → a Messages reply (`content`, `stop_reason`,
// `usage`). A tool call whose arguments aren't a JSON object becomes a
// tool_use with `{}` (the loop's input check then says what is missing).
export function fromChat(reply) {
  const choice = Array.isArray(reply?.choices) ? reply.choices[0] : null;
  const msg = ownOpts(choice?.message);
  const content = [];
  if (typeof msg.content === "string" && msg.content.trim()) content.push({ type: "text", text: msg.content });
  for (const [i, c] of (Array.isArray(msg.tool_calls) ? msg.tool_calls : []).entries()) {
    let input = {};
    try {
      const v = JSON.parse(c?.function?.arguments || "{}");
      if (v && typeof v === "object" && !Array.isArray(v)) input = v;
    } catch (_) {}
    content.push({ type: "tool_use", id: String(c?.id || `call_${i}`), name: String(c?.function?.name || ""), input });
  }
  const stop = STOP[choice?.finish_reason] || (content.some((b) => b.type === "tool_use") ? "tool_use" : "end_turn");
  const u = ownOpts(reply?.usage);
  return {
    role: "assistant",
    content,
    stop_reason: stop,
    usage: { input_tokens: Number(u.prompt_tokens) || 0, output_tokens: Number(u.completion_tokens) || 0, cache_read_input_tokens: 0, cache_creation_input_tokens: 0 },
  };
}

// (c) The transport. `endpoint`: the base URL (`…/v1`); `apiKey`: optional
// (a gateway's bearer token; a local model needs none). Errors carry
// `status` and the endpoint's own message as `error.error.message`, the
// shape describeError() reads.
export function openaiTransport(opts) {
  const { endpoint, apiKey = "", fetchImpl = globalThis.fetch } = ownOpts(opts);
  if (!endpointOk(endpoint)) throw new Error("the model endpoint must be https://host[/path] (http:// only to this computer)");
  const url = endpoint.replace(/\/+$/, "") + "/chat/completions";
  return {
    name: "openai",
    destination: url,
    async send(request, o = {}) {
      const { signal, onText } = ownOpts(o);
      const headers = { "content-type": "application/json" };
      if (apiKey) headers.authorization = `Bearer ${apiKey}`;
      const r = await fetchImpl(url, { method: "POST", headers, body: JSON.stringify(toChat(request)), signal, credentials: "omit", referrerPolicy: "no-referrer" });
      let body = null;
      try {
        body = await r.json();
      } catch (_) {}
      if (!r.ok) {
        const e = new Error(`the model endpoint answered HTTP ${r.status}`);
        e.status = r.status;
        e.error = { error: { message: String(body?.error?.message ?? body?.error ?? "").slice(0, 300) } };
        throw e;
      }
      if (!body || !Array.isArray(body.choices)) throw new Error("the model endpoint's reply is not a chat completion");
      const m = fromChat(body);
      // the loop collects the model's text through onText (a streaming
      // transport calls it as text arrives; this one, once, per block)
      if (typeof onText === "function") for (const b of m.content) if (b.type === "text") onText(b.text);
      return m;
    },
  };
}
