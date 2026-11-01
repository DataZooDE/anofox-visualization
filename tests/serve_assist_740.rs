#![cfg(feature = "serve")]
//! #740: the authoring server's `/assist` proxy, end to end — the real
//! `serve` binary, a local fake of the model API, raw HTTP. No `duckdb` CLI
//! needed (nothing here reaches `/query`).
//!
//! The guard is applied (no token → 401, a foreign Origin → 403), the key
//! goes upstream and never back, no client header is forwarded, a stream
//! arrives as it is written and doesn't hold the authoring worker, a refused
//! request never reaches upstream, the log has token counts and no prompt
//! text, and a server without a key — or a locked one — answers 404.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const KEY: &str = "sk-ant-test-740-never-in-a-reply";

struct Scratch(PathBuf);
impl Scratch {
    fn new(tag: &str) -> Self {
        let d = std::env::temp_dir().join(format!("anofox_740_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        Scratch(d)
    }
    fn key_file(&self) -> PathBuf {
        let p = self.0.join("key");
        std::fs::write(&p, format!("{KEY}\n")).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        p
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Kill(std::process::Child);
impl Drop for Kill {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// What the fake API saw: each request's lower-cased head lines and body.
type Seen = Arc<Mutex<Vec<(Vec<String>, String)>>>;

const ANSWER: &str = r#"{"id":"msg_1","type":"message","role":"assistant","content":[{"type":"text","text":"hi"}],"usage":{"input_tokens":10,"output_tokens":5}}"#;
const SSE_FIRST: &str = "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_2\",\"usage\":{\"input_tokens\":7,\"output_tokens\":1}}}\n\n";
const SSE_REST: &str = "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":9}}\n\nevent: message_stop\ndata: {\"type\":\"message_stop\"}\n\n";
/// How long the fake waits between the first event and the rest.
const GAP: Duration = Duration::from_millis(1500);

/// A fake model API on a free loopback port.
fn fake_api() -> (u16, Seen) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let seen: Seen = Arc::default();
    let s2 = seen.clone();
    std::thread::spawn(move || {
        for conn in l.incoming() {
            let Ok(conn) = conn else { continue };
            let seen = s2.clone();
            std::thread::spawn(move || answer(conn, &seen));
        }
    });
    (port, seen)
}

fn answer(conn: TcpStream, seen: &Seen) {
    let mut r = BufReader::new(conn.try_clone().unwrap());
    let mut head = Vec::new();
    let mut len = 0usize;
    loop {
        let mut line = String::new();
        if r.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
            break;
        }
        let l = line.to_ascii_lowercase();
        if let Some(v) = l.strip_prefix("content-length:") {
            len = v.trim().parse().unwrap_or(0);
        }
        head.push(l);
    }
    let mut body = vec![0; len];
    let _ = r.read_exact(&mut body);
    let body = String::from_utf8_lossy(&body).into_owned();
    let stream = body.contains("\"stream\":true");
    seen.lock().unwrap().push((head, body));
    let mut w = conn;
    if stream {
        let _ = write!(
            w,
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nrequest-id: req_s\r\nConnection: close\r\n\r\n{SSE_FIRST}"
        );
        let _ = w.flush();
        std::thread::sleep(GAP);
        let _ = w.write_all(SSE_REST.as_bytes());
    } else {
        let _ = write!(
            w,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nrequest-id: req_j\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{ANSWER}",
            ANSWER.len()
        );
    }
}

/// `serve <scratch>/a.duckdb --port 0 --no-open [extra]` without
/// `ANTHROPIC_API_KEY` in its environment.
fn spawn_authoring(scratch: &Scratch, extra: &[&str]) -> (Kill, u16, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_serve"))
        .arg(scratch.0.join("a.duckdb"))
        .args(["--port", "0", "--no-open"])
        .args(extra)
        .env_remove("ANTHROPIC_API_KEY")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let mut out = BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    out.read_line(&mut line).unwrap();
    std::thread::spawn(move || std::io::copy(&mut out, &mut std::io::sink()));
    let port: u16 = line
        .split("127.0.0.1:")
        .nth(1)
        .and_then(|s| s.split(['/', '?']).next())
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| panic!("no port in {line:?}"));
    let tok = line
        .split("token=")
        .nth(1)
        .and_then(|s| s.split_whitespace().next())
        .unwrap_or_else(|| panic!("no token in {line:?}"))
        .to_string();
    let deadline = Instant::now() + Duration::from_secs(20);
    while send(port, "GET", "/", &[], "").0 == 0 {
        assert!(Instant::now() < deadline, "serve not ready");
        std::thread::sleep(Duration::from_millis(50));
    }
    (Kill(child), port, tok)
}

/// One raw request: `(status, lower-cased head, body)`; status 0 on a
/// connection error.
fn send(
    port: u16,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &str,
) -> (u16, String, String) {
    let Ok(mut c) = TcpStream::connect(("127.0.0.1", port)) else {
        return (0, String::new(), String::new());
    };
    c.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    let mut h = String::new();
    for (k, v) in headers {
        h.push_str(&format!("{k}: {v}\r\n"));
    }
    let _ = write!(
        c,
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n{h}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let mut resp = String::new();
    let _ = c.read_to_string(&mut resp);
    let (head, rest) = resp.split_once("\r\n\r\n").unwrap_or((resp.as_str(), ""));
    let status = head
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    (status, head.to_ascii_lowercase(), rest.to_string())
}

fn request(stream: bool) -> String {
    format!(
        r#"{{"model":"claude-sonnet-5","max_tokens":500,"stream":{stream},"messages":[{{"role":"user","content":"PROMPT-TEXT weekly revenue"}}],"tools":[{{"name":"run_sql","description":"d","input_schema":{{"type":"object"}}}}]}}"#
    )
}

#[test]
fn proxy_relays_with_the_servers_key_behind_the_authoring_guard() {
    let s = Scratch::new("on");
    let (api, seen) = fake_api();
    let key = s.key_file();
    let log = s.0.join("assist.log");
    let base = format!("http://127.0.0.1:{api}");
    let (_child, port, tok) = spawn_authoring(
        &s,
        &[
            "--assist",
            "--assist-key-file",
            key.to_str().unwrap(),
            "--assist-base-url",
            &base,
            "--assist-log",
            log.to_str().unwrap(),
        ],
    );
    let path = "/assist/v1/messages";
    let body = request(false);
    let auth = ("X-Anofox-Token", tok.as_str());

    // The guard: no token, a foreign Origin.
    assert_eq!(send(port, "POST", path, &[], &body).0, 401);
    let evil = ("Origin", "http://evil.example");
    assert_eq!(send(port, "POST", path, &[auth, evil], &body).0, 403);
    assert!(seen.lock().unwrap().is_empty());

    // A plain answer, with client headers that must not go along.
    let beta = ("anthropic-beta", "files-api-2025-04-14");
    let version = ("anthropic-version", "2099-01-01");
    let browser_key = ("x-api-key", "sk-from-the-browser");
    let (status, head, out) = send(
        port,
        "POST",
        path,
        &[auth, beta, version, browser_key],
        &body,
    );
    assert_eq!(status, 200, "{head}\n{out}");
    assert!(out.contains("msg_1"), "{out}");
    assert!(head.contains("request-id: req_j") && head.contains("cache-control: no-store"));
    assert!(!head.contains(KEY) && !out.contains(KEY));
    {
        let s = seen.lock().unwrap();
        let (h, b) = &s[0];
        assert!(h[0].starts_with("post /v1/messages "), "{h:?}");
        assert!(
            h.iter().any(|l| l.trim() == format!("x-api-key: {KEY}")),
            "{h:?}"
        );
        assert!(
            h.iter()
                .any(|l| l.trim() == "anthropic-version: 2023-06-01"),
            "{h:?}"
        );
        for l in h {
            assert!(!l.starts_with("anthropic-beta"), "{l}");
            assert!(
                !l.starts_with("x-anofox-token") && !l.starts_with("cookie"),
                "{l}"
            );
            assert!(
                !l.contains("sk-from-the-browser") && !l.starts_with("origin"),
                "{l}"
            );
        }
        assert!(b.contains("PROMPT-TEXT"));
    }

    // A stream: the first event arrives before the fake sends the rest,
    // and the builder (the one authoring worker) answers meanwhile.
    let t0 = Instant::now();
    let mut c = TcpStream::connect(("127.0.0.1", port)).unwrap();
    c.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    let body = request(true);
    write!(
        c,
        "POST {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nX-Anofox-Token: {tok}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    let mut r = BufReader::new(c);
    let mut got = String::new();
    while !got.contains("message_start") {
        let mut line = String::new();
        assert!(
            r.read_line(&mut line).unwrap() > 0,
            "stream ended early: {got}"
        );
        got.push_str(&line);
    }
    let first = t0.elapsed();
    assert!(
        first < GAP,
        "first event after {first:?}: not flushed as it came"
    );
    assert!(
        got.to_ascii_lowercase()
            .contains("transfer-encoding: chunked"),
        "{got}"
    );
    let (status, _, page) = send(port, "GET", "/", &[auth], "");
    assert_eq!(status, 200, "the builder must answer during a stream");
    assert!(page.contains("<html") || page.contains("<!doctype") || page.contains("<!DOCTYPE"));
    // Up to the chunked terminator (the reply says `Connection: close`,
    // but this client never closes its side, so no EOF to wait for).
    let mut rest = String::new();
    while !rest.ends_with("0\r\n\r\n") {
        let mut line = String::new();
        assert!(
            r.read_line(&mut line).unwrap() > 0,
            "stream ended early: {rest}"
        );
        rest.push_str(&line);
    }
    assert!(
        rest.contains("message_stop") && rest.ends_with("0\r\n\r\n"),
        "{rest}"
    );

    // A refused request never reaches upstream.
    let calls = seen.lock().unwrap().len();
    let haiku = request(false).replace("claude-sonnet-5", "claude-haiku-4-5");
    let (status, head, _) = send(port, "POST", path, &[auth], &haiku);
    assert_eq!(status, 400);
    assert!(head.contains("x-should-retry: false"));
    let mcp = request(false).replacen('{', r#"{"mcp_servers":[],"#, 1);
    assert_eq!(send(port, "POST", path, &[auth], &mcp).0, 400);
    assert_eq!(seen.lock().unwrap().len(), calls);
    assert_eq!(send(port, "GET", path, &[auth], "").0, 405);
    assert_eq!(send(port, "POST", "/assist/v1/files", &[auth], "{}").0, 404);
    // The builder is told the proxy is on, and its CSP lets DuckDB-Wasm's
    // `+esm` modules load (the assistant's engine).
    let (status, head, page) = send(port, "GET", "/", &[auth], "");
    assert_eq!(status, 200);
    assert!(
        page.contains(r#""transport":"serve""#),
        "no assistant block in the page"
    );
    assert!(!page.contains(KEY));
    let csp = head
        .lines()
        .find(|l| l.starts_with("content-security-policy:"))
        .unwrap_or("");
    assert!(
        csp.contains("apache-arrow@") && csp.contains("flatbuffers@"),
        "{csp}"
    );
    // `/assist/<file>` is the builder's own code, not the proxy's.
    let (status, _, js) = send(port, "GET", "/assist/functions.js", &[auth], "");
    assert_eq!(status, 200);
    assert!(js.contains("export"), "{js}");

    // The log: one line per call, counts, no prompt text, no key.
    let text = std::fs::read_to_string(&log).unwrap();
    assert!(
        !text.contains("PROMPT-TEXT") && !text.contains(KEY),
        "{text}"
    );
    let lines: Vec<serde_json::Value> = text
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(lines[0]["status"], 200);
    assert_eq!(lines[0]["output_tokens"], 5);
    assert_eq!(lines[0]["request_id"], "req_j");
    assert!(
        lines
            .iter()
            .any(|l| l["output_tokens"] == 9 && l["complete"] == true),
        "{text}"
    );
}

#[test]
fn no_key_no_proxy() {
    let s = Scratch::new("off");
    let (_child, port, tok) = spawn_authoring(&s, &[]);
    let auth = ("X-Anofox-Token", tok.as_str());
    assert_eq!(
        send(
            port,
            "POST",
            "/assist/v1/messages",
            &[auth],
            &request(false)
        )
        .0,
        404
    );
    let (_, head, page) = send(port, "GET", "/", &[auth], "");
    assert!(!page.contains(r#""transport":"serve""#) && !head.contains("apache-arrow@"));
    // --no-assist wins over a key.
    let key = s.key_file();
    let (_c2, port, tok) = spawn_authoring(
        &s,
        &["--assist-key-file", key.to_str().unwrap(), "--no-assist"],
    );
    let auth = ("X-Anofox-Token", tok.as_str());
    assert_eq!(
        send(
            port,
            "POST",
            "/assist/v1/messages",
            &[auth],
            &request(false)
        )
        .0,
        404
    );
}

#[test]
fn bad_assist_settings_refuse_to_start() {
    let s = Scratch::new("bad");
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_serve"))
            .args(args)
            .env_remove("ANTHROPIC_API_KEY")
            .output()
            .unwrap()
    };
    let db = s.0.join("a.duckdb");
    let db = db.to_str().unwrap();
    let out = run(&[
        db,
        "--no-open",
        "--port",
        "0",
        "--assist-base-url",
        "http://api.example.com",
    ]);
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    // A key file others can read.
    let key = s.0.join("open-key");
    std::fs::write(&key, KEY).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o644)).unwrap();
        let out = run(&[
            db,
            "--no-open",
            "--port",
            "0",
            "--assist",
            "--assist-key-file",
            key.to_str().unwrap(),
        ]);
        assert_eq!(out.status.code(), Some(2), "{out:?}");
        assert!(!String::from_utf8_lossy(&out.stderr).contains(KEY));
    }
    // Opt-in: settings without --assist are refused, and --assist without a
    // key fails the start.
    let good = s.key_file();
    let out = run(&[
        db,
        "--no-open",
        "--port",
        "0",
        "--assist-key-file",
        good.to_str().unwrap(),
    ]);
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("opt-in"),
        "{out:?}"
    );
    {
        let out = run(&[db, "--no-open", "--port", "0", "--assist"]);
        assert_ne!(out.status.code(), Some(0), "{out:?}");
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("has no key"),
            "{out:?}"
        );
    }
    // A locked server has no /assist: the flags are refused there.
    let dir = s.0.join("dash");
    std::fs::create_dir_all(&dir).unwrap();
    let out = run(&[
        "--dashboards",
        dir.to_str().unwrap(),
        "--assist-models",
        "claude-sonnet-5",
    ]);
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    assert!(String::from_utf8_lossy(&out.stderr).contains("authoring mode only"));
}
