use super::*;
use std::sync::Mutex as StdMutex;

fn cfg() -> AssistConfig {
    AssistConfig::default()
}

fn req(extra: Value) -> Vec<u8> {
    let mut v = json!({
        "model": "claude-sonnet-5",
        "max_tokens": 1000,
        "system": [{"type": "text", "text": "You build dashboards.", "cache_control": {"type": "ephemeral"}}],
        "tools": [{"name": "run_sql", "description": "d", "input_schema": {"type": "object"}, "strict": true}],
        "messages": [{"role": "user", "content": "weekly revenue by region"}],
        "thinking": {"type": "adaptive"},
        "output_config": {"effort": "high"},
        "cache_control": {"type": "ephemeral"},
    });
    for (k, x) in extra.as_object().unwrap() {
        if x.is_null() {
            v.as_object_mut().unwrap().remove(k);
        } else {
            v[k] = x.clone();
        }
    }
    serde_json::to_vec(&v).unwrap()
}

fn refused(ep: Endpoint, body: &[u8]) -> String {
    match validate(ep, body, &cfg()) {
        Ok(_) => panic!("accepted: {}", String::from_utf8_lossy(body)),
        Err((code, msg)) => {
            assert!(code == 400 || code == 413, "{code}");
            msg
        }
    }
}

#[test]
fn accepts_the_assistants_own_request() {
    let c = validate(Endpoint::Messages, &req(json!({"stream": true})), &cfg()).unwrap();
    assert_eq!(c.model, "claude-sonnet-5");
    assert!(c.stream);
    // The body sent upstream is the parsed request, nothing added.
    let sent: Value = serde_json::from_slice(&c.body).unwrap();
    assert_eq!(sent["messages"][0]["content"], "weekly revenue by region");
    // A follow-up with the history of tool calls passes too.
    let history = json!({"messages": [
        {"role": "user", "content": "go"},
        {"role": "assistant", "content": [
            {"type": "thinking", "thinking": "…", "signature": "s"},
            {"type": "text", "text": "Let me look."},
            {"type": "tool_use", "id": "t1", "name": "list_tables", "input": {"filter": ""}}
        ]},
        {"role": "user", "content": [
            {"type": "tool_result", "tool_use_id": "t1", "content": [{"type": "text", "text": "sales"}]}
        ]}
    ]});
    validate(Endpoint::Messages, &req(history), &cfg()).unwrap();
    // count_tokens: no max_tokens, no stream.
    let count = req(json!({"max_tokens": null}));
    validate(Endpoint::CountTokens, &count, &cfg()).unwrap();
}

#[test]
fn refuses_anything_outside_the_allowlist() {
    let ep = Endpoint::Messages;
    assert!(refused(ep, &req(json!({"mcp_servers": []}))).contains("mcp_servers"));
    assert!(refused(ep, &req(json!({"temperature": 1}))).contains("temperature"));
    assert!(refused(ep, &req(json!({"model": "claude-haiku-4-5"})))
        .contains("not allowed on this server"));
    assert!(refused(ep, &req(json!({"max_tokens": 64000}))).contains("max_tokens"));
    assert!(refused(ep, &req(json!({"max_tokens": 0}))).contains("max_tokens"));
    assert!(refused(ep, &req(json!({"max_tokens": null}))).contains("max_tokens"));
    assert!(refused(ep, &req(json!({"stream": "yes"}))).contains("stream"));
    // Server tools carry a `type`; an unknown custom tool has a foreign name.
    let web = json!({"tools": [{"type": "web_fetch_20250910", "name": "web_fetch"}]});
    assert!(refused(ep, &req(web)).contains("tool field 'type'"));
    let foreign = json!({"tools": [{"name": "send_mail", "input_schema": {"type": "object"}}]});
    assert!(refused(ep, &req(foreign)).contains("send_mail"));
    let extra =
        json!({"tools": [{"name": "run_sql", "input_schema": {}, "allowed_callers": ["x"]}]});
    assert!(refused(ep, &req(extra)).contains("allowed_callers"));
    // No images or documents (a URL or a file id for the API to fetch).
    let img = json!({"messages": [{"role": "user", "content": [
        {"type": "image", "source": {"type": "url", "url": "https://example.com/x.png"}}]}]});
    assert!(refused(ep, &req(img)).contains("image"));
    let doc = json!({"messages": [{"role": "user", "content": [
        {"type": "tool_result", "tool_use_id": "t", "content": [{"type": "document", "source": {"type": "file", "file_id": "f"}}]}]}]});
    assert!(refused(ep, &req(doc)).contains("text blocks only"));
    let role = json!({"messages": [{"role": "system", "content": "x"}]});
    assert!(refused(ep, &req(role)).contains("role"));
    assert!(refused(ep, &req(json!({"messages": []}))).contains("non-empty"));
    // An identity in metadata.
    let who = json!({"metadata": {"user_id": "sm@example.com"}});
    assert!(refused(ep, &req(who)).contains("opaque"));
    validate(
        ep,
        &req(json!({"metadata": {"user_id": "b1e4-77"}})),
        &cfg(),
    )
    .unwrap();
    let tc = json!({"tool_choice": {"type": "tool", "name": "bash"}});
    assert!(refused(ep, &req(tc)).contains("tool_choice"));
    // count_tokens takes no max_tokens or stream.
    assert!(refused(Endpoint::CountTokens, &req(json!({}))).contains("max_tokens"));
    // Not JSON, too large.
    assert!(refused(ep, b"{").contains("not JSON"));
    let big = vec![b' '; MAX_REQUEST + 1];
    assert_eq!(validate(ep, &big, &cfg()).unwrap_err().0, 413);
}

#[test]
fn error_messages_never_echo_values() {
    let secret = "sk-ant-api03-SHOULD-NOT-APPEAR";
    let msg = refused(Endpoint::Messages, &req(json!({"model": secret})));
    // A model name is cut at 64 characters; the field names are ours.
    assert!(msg.len() < 300);
    let ctl = refused(Endpoint::Messages, &req(json!({"bad\nfield": 1})));
    assert!(!ctl.contains('\n'));
}

#[test]
fn base_url_rules() {
    for ok in [
        "https://api.anthropic.com",
        "https://gateway.example.com/anthropic/",
        "http://127.0.0.1:8080",
        "http://localhost:9000/x",
        "http://[::1]:9000",
    ] {
        check_base_url(ok).unwrap_or_else(|e| panic!("{ok}: {e}"));
    }
    for bad in [
        "http://api.anthropic.com",
        "http://10.0.0.1",
        "https://user:pw@host",
        "https://host/?q=1",
        "https://host/#x",
        "ftp://host",
        "https://",
        "https://ho st",
        "http://localhost.evil.com",
    ] {
        assert!(check_base_url(bad).is_err(), "{bad}");
    }
    let mut c = cfg();
    c.set_base_url("https://gw.example.com/a/").unwrap();
    assert_eq!(c.base_url, "https://gw.example.com/a");
}

#[test]
fn models_list() {
    let mut c = cfg();
    c.set_models("claude-opus-5-5, claude-sonnet-5").unwrap();
    assert_eq!(c.models, ["claude-opus-5-5", "claude-sonnet-5"]);
    assert!(c.set_models("").is_err());
    assert!(c.set_models("Claude Opus").is_err());
}

#[cfg(unix)]
#[test]
fn key_file_rules() {
    use std::os::unix::fs::PermissionsExt;
    let dir = std::env::temp_dir().join(format!("assist-key-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("key");
    std::fs::write(&p, "sk-test-123\n").unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
    let e = read_key_file(&p).unwrap_err();
    assert!(e.contains("chmod 600") && !e.contains("sk-test"), "{e}");
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(read_key_file(&p).unwrap(), "sk-test-123");
    assert_eq!(load_key(Some(&p)).unwrap().as_deref(), Some("sk-test-123"));
    let link = dir.join("link");
    let _ = std::fs::remove_file(&link);
    std::os::unix::fs::symlink(&p, &link).unwrap();
    assert!(read_key_file(&link).is_err());
    std::fs::write(&p, "two words").unwrap();
    let e = load_key(Some(&p)).unwrap_err();
    assert!(!e.contains("words"), "{e}");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn options_are_taken_out() {
    let mut o: Map<String, Value> = serde_json::from_str(
        r#"{"name": "Acme", "assist_models": ["claude-opus-5-5"], "assist_budget_tokens_per_hour": 5000,
            "assist_base_url": "http://127.0.0.1:9", "assist_log_content": true}"#,
    )
    .unwrap();
    let a = AssistOptions::take(&mut o).unwrap();
    assert_eq!(o.keys().collect::<Vec<_>>(), ["name"]);
    assert_eq!(a.cfg.models, ["claude-opus-5-5"]);
    assert_eq!(a.cfg.budget_tokens_per_hour, 5000);
    assert!(a.cfg.log_content && !a.off);
    let mut off: Map<String, Value> = serde_json::from_str(r#"{"assist": false}"#).unwrap();
    assert!(AssistOptions::take(&mut off).unwrap().off);
    let mut bad: Map<String, Value> =
        serde_json::from_str(r#"{"assist_key_env": "AWS_SECRET"}"#).unwrap();
    assert!(AssistOptions::take(&mut bad)
        .unwrap_err()
        .contains("unknown"));
    let mut http: Map<String, Value> =
        serde_json::from_str(r#"{"assist_base_url": "http://evil.example"}"#).unwrap();
    assert!(AssistOptions::take(&mut http).is_err());
    assert!(build(true, cfg(), None, None).unwrap().is_none());
}

// ---- the proxy over a fake upstream ----

/// What the fake saw: each call's path, body and `stream`.
type Seen = Arc<StdMutex<Vec<(String, Vec<u8>, bool)>>>;

struct Fake {
    status: u16,
    body: Vec<u8>,
    seen: Seen,
}

impl Upstream for Fake {
    fn post(&self, path: &str, body: &[u8], stream: bool) -> Result<UpResponse, String> {
        self.seen
            .lock()
            .unwrap()
            .push((path.into(), body.to_vec(), stream));
        Ok(UpResponse {
            status: self.status,
            request_id: Some("req_011".into()),
            body: Box::new(io::Cursor::new(self.body.clone())),
        })
    }
}

fn proxy(status: u16, body: &str, c: AssistConfig, log: Option<LogSink>) -> (Proxy, Seen) {
    let seen: Seen = Arc::default();
    let f = Fake {
        status,
        body: body.as_bytes().to_vec(),
        seen: seen.clone(),
    };
    (Proxy::new(c, Box::new(f), log), seen)
}

fn http(r: Reply) -> String {
    let mut out = Vec::new();
    write_http(&mut out, r).unwrap();
    String::from_utf8(out).unwrap()
}

const ANSWER: &str = r#"{"id":"msg_1","type":"message","role":"assistant","content":[{"type":"text","text":"hi"}],
 "usage":{"input_tokens":100,"output_tokens":20,"cache_creation_input_tokens":3000,"cache_read_input_tokens":0}}"#;

#[test]
fn relays_an_answer_and_counts_its_tokens() {
    let (p, seen) = proxy(200, ANSWER, cfg(), None);
    let r = p.handle(p.try_slot().unwrap(), Endpoint::Messages, &req(json!({})));
    assert_eq!(r.status, 200);
    let out = http(r);
    assert!(out.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(out.contains("request-id: req_011\r\n") && out.contains("Cache-Control: no-store"));
    assert!(out.ends_with(ANSWER));
    assert_eq!(p.used_last_hour(), 3120);
    let s = seen.lock().unwrap();
    assert_eq!(s[0].0, "/v1/messages");
    assert!(!s[0].2);
    // The slot went back to the pool.
    assert!(p.try_slot().is_some() && p.try_slot().is_some());
}

#[test]
fn budget_refuses_with_429_and_count_tokens_stays_free() {
    let mut c = cfg();
    c.budget_tokens_per_hour = 3000;
    let (p, seen) = proxy(200, ANSWER, c, None);
    p.handle(p.try_slot().unwrap(), Endpoint::Messages, &req(json!({})));
    let r = p.handle(p.try_slot().unwrap(), Endpoint::Messages, &req(json!({})));
    assert_eq!(r.status, 429);
    let out = http(r);
    assert!(out.contains("x-should-retry: false") && out.contains("rate_limit_error"));
    assert_eq!(
        seen.lock().unwrap().len(),
        1,
        "no upstream call over budget"
    );
    let (pc, _) = proxy(200, r#"{"input_tokens": 999}"#, cfg(), None);
    let r = pc.handle(
        pc.try_slot().unwrap(),
        Endpoint::CountTokens,
        &req(json!({"max_tokens": null})),
    );
    assert_eq!(r.status, 200);
    assert_eq!(pc.used_last_hour(), 0);
}

#[test]
fn upstream_error_bodies_are_not_relayed() {
    let body = r#"{"type":"error","error":{"type":"invalid_request_error","message":"messages.0: ECHO-OF-THE-PROMPT"}}"#;
    let (p, _) = proxy(400, body, cfg(), None);
    let r = p.handle(p.try_slot().unwrap(), Endpoint::Messages, &req(json!({})));
    assert_eq!(r.status, 400);
    let out = http(r);
    assert!(!out.contains("ECHO-OF-THE-PROMPT"), "{out}");
    assert!(out.contains("invalid_request_error") && out.contains("HTTP 400"));
    // An unknown error type becomes api_error; a non-JSON body too.
    let (p, _) = proxy(
        500,
        "<html>proxy page with the key? sk-…</html>",
        cfg(),
        None,
    );
    let out = http(p.handle(p.try_slot().unwrap(), Endpoint::Messages, &req(json!({}))));
    assert!(out.contains("api_error") && !out.contains("html"));
}

#[test]
fn refusals_never_reach_upstream() {
    let (p, seen) = proxy(200, ANSWER, cfg(), None);
    let r = p.handle(
        p.try_slot().unwrap(),
        Endpoint::Messages,
        &req(json!({"mcp_servers": []})),
    );
    assert_eq!(r.status, 400);
    assert!(http(r).contains("x-should-retry: false"));
    assert!(seen.lock().unwrap().is_empty());
    assert!(p.try_slot().is_some());
}

#[test]
fn the_pool_is_bounded() {
    let mut c = cfg();
    c.workers = 2;
    let (p, _) = proxy(200, ANSWER, c, None);
    let a = p.try_slot().unwrap();
    let _b = p.try_slot().unwrap();
    assert!(p.try_slot().is_none());
    assert_eq!(Proxy::busy().status, 503);
    drop(a);
    assert!(p.try_slot().is_some());
}

const SSE: &str = "event: message_start\n\
data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"usage\":{\"input_tokens\":50,\"cache_read_input_tokens\":2000,\"output_tokens\":1}}}\n\n\
event: content_block_delta\n\
data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hello\"}}\n\n\
event: message_delta\n\
data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":40}}\n\n\
event: message_stop\n\
data: {\"type\":\"message_stop\"}\n\n";

#[test]
fn streams_events_chunked_and_logs_usage_without_content() {
    let dir = std::env::temp_dir().join(format!("assist-log-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let logp = dir.join("assist.log");
    let _ = std::fs::remove_file(&logp);
    let mut c = cfg();
    c.workers = 1;
    let (p, seen) = proxy(200, SSE, c, Some(log_sink(logp.to_str().unwrap()).unwrap()));
    let r = p.handle(
        p.try_slot().unwrap(),
        Endpoint::Messages,
        &req(json!({"stream": true})),
    );
    assert_eq!(r.content_type, "text/event-stream");
    // The stream holds the slot until it is written out.
    assert!(p.try_slot().is_none());
    let out = http(r);
    assert!(out.contains("Transfer-Encoding: chunked\r\n\r\n"));
    assert!(out.contains("text_delta") && out.ends_with("\r\n0\r\n\r\n"));
    assert!(p.try_slot().is_some());
    assert!(seen.lock().unwrap()[0].2);
    assert_eq!(p.used_last_hour(), 2090);
    let log = std::fs::read_to_string(&logp).unwrap();
    let line: Value = serde_json::from_str(log.trim()).unwrap();
    assert_eq!(line["kind"], "assist");
    assert_eq!(line["input_tokens"], 50);
    assert_eq!(line["output_tokens"], 40);
    assert_eq!(line["cache_read"], 2000);
    assert_eq!(line["request_id"], "req_011");
    assert_eq!(line["complete"], true);
    assert!(!log.contains("weekly revenue"), "no prompt text: {log}");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn log_content_is_opt_in() {
    let mut c = cfg();
    c.log_content = true;
    let rec = Record {
        endpoint: Endpoint::Messages,
        model: "m".into(),
        status: 200,
        usage: Usage::default(),
        request_id: None,
        started: Instant::now(),
        complete: true,
        content: Some("weekly revenue".into()),
    };
    assert!(log_line(&rec, SystemTime::UNIX_EPOCH).contains("weekly revenue"));
}

#[test]
fn a_dropped_stream_still_counts() {
    let (p, _) = proxy(200, SSE, cfg(), None);
    let r = p.handle(
        p.try_slot().unwrap(),
        Endpoint::Messages,
        &req(json!({"stream": true})),
    );
    let Body::Stream(mut rd) = r.body else {
        panic!()
    };
    // The client reads the first bytes, then goes away.
    let mut first = [0u8; 300];
    rd.read_exact(&mut first).unwrap();
    drop(rd);
    assert_eq!(p.used_last_hour(), 2051);
}

#[test]
fn a_failed_client_write_stops_the_stream() {
    struct Broken;
    impl Write for Broken {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::ErrorKind::BrokenPipe.into())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let (p, _) = proxy(200, SSE, cfg(), None);
    let r = p.handle(
        p.try_slot().unwrap(),
        Endpoint::Messages,
        &req(json!({"stream": true})),
    );
    assert!(write_http(&mut Broken, r).is_err());
    assert!(p.try_slot().is_some() && p.try_slot().is_some());
}

// ---- the real client against a local fake API ----

#[test]
fn http_upstream_pins_headers_and_sends_the_key_only_upstream() {
    use std::io::{BufRead, BufReader};
    use std::net::TcpListener;
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let t = std::thread::spawn(move || {
        let (s, _) = l.accept().unwrap();
        let mut r = BufReader::new(s.try_clone().unwrap());
        let mut head = Vec::new();
        let mut len = 0usize;
        loop {
            let mut line = String::new();
            r.read_line(&mut line).unwrap();
            if line == "\r\n" {
                break;
            }
            if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                len = v.trim().parse().unwrap();
            }
            head.push(line);
        }
        let mut body = vec![0; len];
        r.read_exact(&mut body).unwrap();
        let mut w = s;
        let resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nrequest-id: req_x\r\nContent-Length: {}\r\n\r\n{ANSWER}",
            ANSWER.len()
        );
        w.write_all(resp.as_bytes()).unwrap();
        (head, body)
    });
    let up = HttpUpstream::new(&format!("http://127.0.0.1:{port}"), "sk-test-key".into()).unwrap();
    let p = Proxy::new(cfg(), Box::new(up), None);
    let r = p.handle(p.try_slot().unwrap(), Endpoint::Messages, &req(json!({})));
    assert_eq!(r.status, 200);
    let out = http(r);
    assert!(!out.contains("sk-test-key"));
    assert!(out.contains("request-id: req_x"));
    let (head, body) = t.join().unwrap();
    let head: Vec<String> = head.iter().map(|h| h.to_ascii_lowercase()).collect();
    assert!(head[0].starts_with("post /v1/messages "), "{head:?}");
    assert!(head.iter().any(|h| h == "x-api-key: sk-test-key\r\n"));
    assert!(head
        .iter()
        .any(|h| h == &format!("anthropic-version: {ANTHROPIC_VERSION}\r\n")));
    assert!(
        head.iter()
            .any(|h| h == "user-agent: anofox-visualization\r\n"),
        "{head:?}"
    );
    assert!(!head.iter().any(|h| h.starts_with("anthropic-beta")));
    let sent: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(sent["model"], "claude-sonnet-5");
}

#[test]
fn http_upstream_refuses_plain_http_off_loopback() {
    assert!(HttpUpstream::new("http://api.anthropic.com", "k".into()).is_err());
}
