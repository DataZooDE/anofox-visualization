//! An OpenAI-compatible model endpoint (`/chat/completions`) behind the
//! Anthropic Messages shape (#843, plan-ai-assistant.md §2.1 transport (c)):
//! the authoring server's `/assist` proxy with `--assist-provider openai`, and
//! (M3-4) served Ask. The callers speak Messages only; this translates one
//! request there and the reply back, the same rules as `web/assist-openai.js`:
//! - system blocks → one `system` message; `tools` → `functions`;
//! - `tool_use` / `tool_result` blocks ⇄ `tool_calls` / `role: "tool"`;
//! - no caching, no thinking, no effort (dropped), no streaming upstream: a
//!   client that streams gets the whole reply as one Anthropic event stream.

use serde_json::{json, Map, Value};

/// The text of a `content` / `system` value: a string, or its text blocks.
fn text(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(blocks)) => blocks
            .iter()
            .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|b| b.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n\n"),
        _ => String::new(),
    }
}

fn blocks(m: &Value) -> Vec<Value> {
    match m.get("content") {
        Some(Value::String(s)) => vec![json!({"type": "text", "text": s})],
        Some(Value::Array(a)) => a.clone(),
        _ => Vec::new(),
    }
}

fn is(b: &Value, ty: &str) -> bool {
    b.get("type").and_then(Value::as_str) == Some(ty)
}

/// One Messages request → a chat completions body (`stream: false`).
pub fn to_chat(req: &Value) -> Value {
    let mut messages = Vec::new();
    let system = text(req.get("system"));
    if !system.is_empty() {
        messages.push(json!({"role": "system", "content": system}));
    }
    for m in req
        .get("messages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let bs = blocks(m);
        if m.get("role").and_then(Value::as_str) == Some("assistant") {
            let calls: Vec<Value> = bs
                .iter()
                .filter(|b| is(b, "tool_use"))
                .map(|b| {
                    json!({
                        "id": b.get("id").and_then(Value::as_str).unwrap_or(""),
                        "type": "function",
                        "function": {
                            "name": b.get("name").and_then(Value::as_str).unwrap_or(""),
                            "arguments": b.get("input").unwrap_or(&json!({})).to_string(),
                        },
                    })
                })
                .collect();
            let t = text(Some(&Value::Array(bs.clone())));
            let mut msg = json!({"role": "assistant", "content": if t.is_empty() { Value::Null } else { json!(t) }});
            if !calls.is_empty() {
                msg["tool_calls"] = Value::Array(calls);
            }
            messages.push(msg);
        } else {
            // tool results first (they answer the previous turn), then text
            for b in bs.iter().filter(|b| is(b, "tool_result")) {
                let c = text(b.get("content"));
                let err = b.get("is_error").and_then(Value::as_bool) == Some(true);
                messages.push(json!({
                    "role": "tool",
                    "tool_call_id": b.get("tool_use_id").and_then(Value::as_str).unwrap_or(""),
                    "content": if err { format!("Error: {c}") } else { c },
                }));
            }
            let t = text(Some(&Value::Array(bs)));
            if !t.is_empty() {
                messages.push(json!({"role": "user", "content": t}));
            }
        }
    }
    let mut body = Map::new();
    if let Some(m) = req.get("model") {
        body.insert("model".into(), m.clone());
    }
    body.insert("messages".into(), Value::Array(messages));
    if let Some(n) = req.get("max_tokens") {
        body.insert("max_tokens".into(), n.clone());
    }
    body.insert("stream".into(), json!(false));
    if let Some(tools) = req
        .get("tools")
        .and_then(Value::as_array)
        .filter(|t| !t.is_empty())
    {
        let fns: Vec<Value> = tools
            .iter()
            .map(|t| {
                json!({
                    "type": "function",
                    "function": {
                        "name": t.get("name").and_then(Value::as_str).unwrap_or(""),
                        "description": t.get("description").and_then(Value::as_str).unwrap_or(""),
                        "parameters": t.get("input_schema").cloned()
                            .unwrap_or_else(|| json!({"type": "object", "properties": {}})),
                    },
                })
            })
            .collect();
        body.insert("tools".into(), Value::Array(fns));
        let choice = req.get("tool_choice");
        match choice.and_then(|c| c.get("type")).and_then(Value::as_str) {
            Some("tool") => {
                if let Some(name) = choice.and_then(|c| c.get("name")).and_then(Value::as_str) {
                    body.insert(
                        "tool_choice".into(),
                        json!({"type": "function", "function": {"name": name}}),
                    );
                }
            }
            Some("any") => {
                body.insert("tool_choice".into(), json!("required"));
            }
            Some("none") => {
                body.insert("tool_choice".into(), json!("none"));
            }
            _ => {}
        }
    }
    Value::Object(body)
}

/// A chat completions reply → a Messages reply (`id`, `type`, `role`,
/// `model`, `content`, `stop_reason`, `usage`). A tool call whose arguments
/// aren't a JSON object becomes a `tool_use` with `{}` (the caller's input
/// check then says what is missing). `Err` when it isn't a chat completion.
pub fn from_chat(reply: &Value, model: &str) -> Result<Value, String> {
    let choices = reply
        .get("choices")
        .and_then(Value::as_array)
        .ok_or("the model endpoint's reply is not a chat completion")?;
    let choice = choices.first();
    let msg = choice.and_then(|c| c.get("message"));
    let mut content = Vec::new();
    if let Some(t) = msg
        .and_then(|m| m.get("content"))
        .and_then(Value::as_str)
        .filter(|t| !t.trim().is_empty())
    {
        content.push(json!({"type": "text", "text": t}));
    }
    for (i, c) in msg
        .and_then(|m| m.get("tool_calls"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
    {
        let f = c.get("function");
        let input = f
            .and_then(|f| f.get("arguments"))
            .and_then(Value::as_str)
            .and_then(|a| serde_json::from_str::<Value>(a).ok())
            .filter(Value::is_object)
            .unwrap_or_else(|| json!({}));
        let id = c
            .get("id")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| format!("call_{i}"));
        content.push(json!({
            "type": "tool_use",
            "id": id,
            "name": f.and_then(|f| f.get("name")).and_then(Value::as_str).unwrap_or(""),
            "input": input,
        }));
    }
    let has_tools = content.iter().any(|b| is(b, "tool_use"));
    let stop = match choice
        .and_then(|c| c.get("finish_reason"))
        .and_then(Value::as_str)
    {
        Some("stop") => "end_turn",
        Some("tool_calls" | "function_call") => "tool_use",
        Some("length") => "max_tokens",
        Some("content_filter") => "refusal",
        _ if has_tools => "tool_use",
        _ => "end_turn",
    };
    let u = reply.get("usage");
    let n = |k: &str| {
        u.and_then(|u| u.get(k))
            .and_then(Value::as_u64)
            .unwrap_or(0)
    };
    Ok(json!({
        "id": format!("msg_oai_{}", reply.get("id").and_then(Value::as_str).filter(|s| s.len() <= 64 && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')).unwrap_or("0")),
        "type": "message",
        "role": "assistant",
        "model": model,
        "content": content,
        "stop_reason": stop,
        "stop_sequence": null,
        "usage": {
            "input_tokens": n("prompt_tokens"),
            "output_tokens": n("completion_tokens"),
            "cache_creation_input_tokens": 0,
            "cache_read_input_tokens": 0,
        },
    }))
}

/// A whole Messages reply as the event stream the Messages API sends
/// (`message_start`, a start / delta / stop per block, `message_delta`,
/// `message_stop`): what a streaming client (the SDK's `messages.stream`)
/// reads, from a reply that came in one piece.
pub fn sse(msg: &Value) -> String {
    let mut out = String::new();
    let mut ev = |name: &str, data: Value| {
        out.push_str(&format!("event: {name}\ndata: {data}\n\n"));
    };
    let usage = msg.get("usage").cloned().unwrap_or_else(|| json!({}));
    let mut start = msg.clone();
    start["content"] = json!([]);
    start["stop_reason"] = Value::Null;
    start["usage"] = json!({
        "input_tokens": usage.get("input_tokens").cloned().unwrap_or(json!(0)),
        "output_tokens": 0,
        "cache_creation_input_tokens": 0,
        "cache_read_input_tokens": 0,
    });
    ev(
        "message_start",
        json!({"type": "message_start", "message": start}),
    );
    for (i, b) in msg
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
    {
        if is(b, "text") {
            ev(
                "content_block_start",
                json!({"type": "content_block_start", "index": i, "content_block": {"type": "text", "text": ""}}),
            );
            ev(
                "content_block_delta",
                json!({"type": "content_block_delta", "index": i, "delta": {"type": "text_delta", "text": b.get("text").cloned().unwrap_or(json!(""))}}),
            );
        } else {
            let mut cb = b.clone();
            let input = cb
                .as_object_mut()
                .and_then(|o| o.insert("input".into(), json!({})))
                .unwrap_or_else(|| json!({}));
            ev(
                "content_block_start",
                json!({"type": "content_block_start", "index": i, "content_block": cb}),
            );
            ev(
                "content_block_delta",
                json!({"type": "content_block_delta", "index": i, "delta": {"type": "input_json_delta", "partial_json": input.to_string()}}),
            );
        }
        ev(
            "content_block_stop",
            json!({"type": "content_block_stop", "index": i}),
        );
    }
    ev(
        "message_delta",
        json!({
            "type": "message_delta",
            "delta": {"stop_reason": msg.get("stop_reason").cloned().unwrap_or(Value::Null), "stop_sequence": null},
            "usage": {"output_tokens": usage.get("output_tokens").cloned().unwrap_or(json!(0))},
        }),
    );
    ev("message_stop", json!({"type": "message_stop"}));
    out
}

/// The Messages API's error type for an endpoint's HTTP status (the proxy
/// relays the type, never the endpoint's text).
pub fn error_type(status: u16) -> &'static str {
    match status {
        400 | 422 => "invalid_request_error",
        401 => "authentication_error",
        403 => "permission_error",
        404 => "not_found_error",
        413 => "request_too_large",
        429 => "rate_limit_error",
        529 | 503 => "overloaded_error",
        _ => "api_error",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_translates() {
        let req = json!({
            "model": "llama3.1:8b",
            "max_tokens": 4000,
            "system": [{"type": "text", "text": "You build dashboards."}, {"type": "text", "text": "# Guide", "cache_control": {"type": "ephemeral"}}],
            "tools": [{"name": "run_sql", "description": "Run a query", "input_schema": {"type": "object", "properties": {"sql": {"type": "string"}}}}],
            "thinking": {"type": "adaptive"},
            "output_config": {"effort": "high"},
            "cache_control": {"type": "ephemeral"},
            "messages": [
                {"role": "user", "content": [{"type": "text", "text": "Revenue by region"}]},
                {"role": "assistant", "content": [{"type": "thinking", "thinking": "hm", "signature": "s"}, {"type": "text", "text": "Let me look."}, {"type": "tool_use", "id": "t1", "name": "run_sql", "input": {"sql": "SELECT 1"}}]},
                {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "t1", "is_error": true, "content": [{"type": "text", "text": "refused"}]}, {"type": "text", "text": "and sort it"}]},
            ],
        });
        let b = to_chat(&req);
        assert_eq!(
            b["messages"],
            json!([
                {"role": "system", "content": "You build dashboards.\n\n# Guide"},
                {"role": "user", "content": "Revenue by region"},
                {"role": "assistant", "content": "Let me look.", "tool_calls": [{"id": "t1", "type": "function", "function": {"name": "run_sql", "arguments": "{\"sql\":\"SELECT 1\"}"}}]},
                {"role": "tool", "tool_call_id": "t1", "content": "Error: refused"},
                {"role": "user", "content": "and sort it"},
            ])
        );
        assert_eq!(
            b["tools"][0]["function"]["parameters"],
            req["tools"][0]["input_schema"]
        );
        assert_eq!(b["stream"], json!(false));
        let s = b.to_string();
        for k in ["thinking", "cache_control", "output_config", "signature"] {
            assert!(!s.contains(k), "{k} in {s}");
        }
        let mut forced = req.clone();
        forced["tool_choice"] = json!({"type": "tool", "name": "run_sql"});
        assert_eq!(
            to_chat(&forced)["tool_choice"],
            json!({"type": "function", "function": {"name": "run_sql"}})
        );
        forced["tool_choice"] = json!({"type": "any"});
        assert_eq!(to_chat(&forced)["tool_choice"], json!("required"));
        let only_calls = json!({"messages": [{"role": "assistant", "content": [{"type": "tool_use", "id": "x", "name": "n", "input": {}}]}]});
        assert_eq!(to_chat(&only_calls)["messages"][0]["content"], Value::Null);
    }

    #[test]
    fn reply_translates() {
        let m = from_chat(
            &json!({
                "id": "chatcmpl-1",
                "choices": [{"finish_reason": "tool_calls", "message": {"content": "Checking.", "tool_calls": [
                    {"id": "c1", "type": "function", "function": {"name": "run_sql", "arguments": "{\"sql\":\"SELECT 1\"}"}},
                    {"id": "c2", "type": "function", "function": {"name": "check", "arguments": "not json"}},
                    {"type": "function", "function": {"name": "x", "arguments": "[1]"}},
                ]}}],
                "usage": {"prompt_tokens": 120, "completion_tokens": 30},
            }),
            "llama3.1:8b",
        )
        .unwrap();
        assert_eq!(m["stop_reason"], "tool_use");
        assert_eq!(m["model"], "llama3.1:8b");
        assert_eq!(m["id"], "msg_oai_chatcmpl-1");
        assert_eq!(
            m["content"],
            json!([
                {"type": "text", "text": "Checking."},
                {"type": "tool_use", "id": "c1", "name": "run_sql", "input": {"sql": "SELECT 1"}},
                {"type": "tool_use", "id": "c2", "name": "check", "input": {}},
                {"type": "tool_use", "id": "call_2", "name": "x", "input": {}},
            ])
        );
        assert_eq!(m["usage"]["input_tokens"], 120);
        assert_eq!(m["usage"]["output_tokens"], 30);
        let stop = |r: &str| {
            from_chat(
                &json!({"choices": [{"finish_reason": r, "message": {"content": "x"}}]}),
                "m",
            )
            .unwrap()["stop_reason"]
                .clone()
        };
        assert_eq!(stop("stop"), "end_turn");
        assert_eq!(stop("length"), "max_tokens");
        assert_eq!(stop("content_filter"), "refusal");
        assert!(from_chat(&json!({"error": "x"}), "m").is_err());
        // a hostile id never reaches the message id
        let m = from_chat(&json!({"id": "a\"b<c>", "choices": []}), "m").unwrap();
        assert_eq!(m["id"], "msg_oai_0");
    }

    #[test]
    fn a_reply_as_an_event_stream() {
        let m = json!({
            "id": "msg_oai_1", "type": "message", "role": "assistant", "model": "m",
            "content": [{"type": "text", "text": "Hi"}, {"type": "tool_use", "id": "t", "name": "run_sql", "input": {"sql": "SELECT 1"}}],
            "stop_reason": "tool_use", "stop_sequence": null,
            "usage": {"input_tokens": 7, "output_tokens": 3, "cache_creation_input_tokens": 0, "cache_read_input_tokens": 0},
        });
        let s = sse(&m);
        let events: Vec<Value> = s
            .split("\n\n")
            .filter_map(|c| c.lines().find_map(|l| l.strip_prefix("data: ")))
            .map(|d| serde_json::from_str(d).unwrap())
            .collect();
        let types: Vec<&str> = events.iter().map(|e| e["type"].as_str().unwrap()).collect();
        assert_eq!(
            types,
            [
                "message_start",
                "content_block_start",
                "content_block_delta",
                "content_block_stop",
                "content_block_start",
                "content_block_delta",
                "content_block_stop",
                "message_delta",
                "message_stop"
            ]
        );
        assert_eq!(events[0]["message"]["usage"]["input_tokens"], 7);
        assert_eq!(events[0]["message"]["content"], json!([]));
        assert_eq!(events[4]["content_block"]["input"], json!({}));
        assert_eq!(events[5]["delta"]["partial_json"], "{\"sql\":\"SELECT 1\"}");
        assert_eq!(events[7]["delta"]["stop_reason"], "tool_use");
        assert_eq!(events[7]["usage"]["output_tokens"], 3);
    }
}
