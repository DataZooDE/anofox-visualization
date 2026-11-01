#![cfg(feature = "serve")]
//! #195 M2-6: Ask on the static render server (`serve --dashboards`, "B"),
//! end to end — the real binary, the `duckdb` CLI, a scripted fake of the
//! model API, raw HTTP. Plan §12.9 for B:
//! - the start refuses Ask without `--auth`, and `ask.anonymous` here;
//! - the 404 / 403 matrix: no form and 404 on a dashboard without
//!   `-- @ask on`; a post without the page's token, with another viewer's
//!   token, cross-origin or cross-site is refused;
//! - `get_panel` sets `@param`s only, to their declared values;
//! - the canary: a row only another viewer's claims show, an e-mail address
//!   (masked) and an `@ask skip` panel never reach the model;
//! - the answer page: escaped text (no markup of the model's), verified
//!   citations as links into the page (`#panel-<n>`, with the params a
//!   citation was read with), `no-store, private`.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

const SECRET: &str = "proxy-secret-m2-6-0123456789abcdef";
const KEY: &str = "sk-ant-test-m2-6";

fn duckdb_available() -> bool {
    Command::new("duckdb").arg("-version").output().is_ok()
}

struct Scratch(PathBuf);
impl Scratch {
    fn new(tag: &str) -> Self {
        let d = std::env::temp_dir().join(format!("anofox_m26_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        Scratch(d)
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

/// The fake model: scripted replies, every request body recorded.
#[derive(Clone, Default)]
struct Model {
    replies: Arc<Mutex<VecDeque<Value>>>,
    seen: Arc<Mutex<Vec<String>>>,
}

fn fake_model(m: Model) -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for conn in l.incoming() {
            let Ok(conn) = conn else { continue };
            let m = m.clone();
            std::thread::spawn(move || answer(conn, &m));
        }
    });
    port
}

fn answer(conn: TcpStream, m: &Model) {
    let mut r = BufReader::new(conn.try_clone().unwrap());
    let mut len = 0usize;
    loop {
        let mut line = String::new();
        if r.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
            break;
        }
        if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
            len = v.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0u8; len];
    let _ = r.read_exact(&mut body);
    m.seen
        .lock()
        .unwrap()
        .push(String::from_utf8_lossy(&body).to_string());
    let reply = m.replies.lock().unwrap().pop_front().unwrap_or_else(|| {
        json!({"content": [{"type": "text", "text": "no more replies"}], "stop_reason": "end_turn", "usage": {}})
    });
    let s = reply.to_string();
    let mut w = conn;
    let _ = write!(
        w,
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nrequest-id: req_m26\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{s}",
        s.len()
    );
}

fn tool(id: &str, name: &str, input: Value) -> Value {
    json!({"content": [{"type": "tool_use", "id": id, "name": name, "input": input}],
           "stop_reason": "tool_use", "usage": {"input_tokens": 100, "output_tokens": 20}})
}

const DASH: &str = "-- @title Sales\n-- @ask on\n-- @param region [EU, US] = EU\n\
SELECT product AS \"Product\" ::TABLE, units AS \"Units\", contact AS \"Contact\" FROM sales \
WHERE region = getvariable('region') AND (getvariable('__claim_user') = 'alice' OR product <> 'CANARY') ORDER BY product;\n\
-- @ask skip\nSELECT 'SKIPCANARY' AS secret ::TABLE;\n";
const PLAIN: &str = "-- @title Plain\nSELECT 'x' AS a ::TABLE;\n";
const INIT: &str = "CREATE TABLE IF NOT EXISTS sales AS SELECT * FROM (VALUES \
('EU', 'Widget', 5, 'ann@example.com'), ('EU', 'CANARY', 99, 'zed@example.com'), \
('US', 'Gadget', 7, 'bo@example.com')) t(region, product, units, contact);\n";

fn setup(s: &Scratch, model_port: u16, extra_ask: Value) -> (Vec<String>, PathBuf) {
    let dash = s.0.join("dash");
    std::fs::create_dir_all(&dash).unwrap();
    std::fs::write(dash.join("sales.sql"), DASH).unwrap();
    std::fs::write(dash.join("plain.sql"), PLAIN).unwrap();
    std::fs::write(s.0.join("init.sql"), INIT).unwrap();
    std::fs::write(
        s.0.join("auth.json"),
        json!({"mode": "proxy", "shared_secret": SECRET}).to_string(),
    )
    .unwrap();
    let mut ask = json!({"enabled": true, "model": "claude-sonnet-5",
        "base_url": format!("http://127.0.0.1:{model_port}"), "key_env": "ANOFOX_M26_KEY"});
    for (k, v) in extra_ask.as_object().unwrap() {
        ask[k] = v.clone();
    }
    std::fs::write(s.0.join("ask.json"), ask.to_string()).unwrap();
    let args = vec![
        "--dashboards".into(),
        dash.display().to_string(),
        "--db".into(),
        s.0.join("t.duckdb").display().to_string(),
        "--init".into(),
        s.0.join("init.sql").display().to_string(),
        "--ask".into(),
        format!("@{}", s.0.join("ask.json").display()),
        "--no-open".into(),
    ];
    (args, s.0.join("auth.json"))
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn start(args: &[String], auth: Option<&PathBuf>) -> (Kill, u16) {
    let port = free_port();
    let mut c = Command::new(env!("CARGO_BIN_EXE_serve"));
    c.args(args)
        .args(["--port", &port.to_string()])
        .env("ANOFOX_M26_KEY", KEY);
    if let Some(a) = auth {
        c.arg("--auth").arg(format!("@{}", a.display()));
    }
    let k = Kill(
        c.stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let t0 = Instant::now();
    while TcpStream::connect(("127.0.0.1", port)).is_err() {
        assert!(
            t0.elapsed() < Duration::from_secs(30),
            "server did not start"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    (k, port)
}

/// One request; `user` signs in through the proxy headers.
fn http(
    port: u16,
    method: &str,
    path: &str,
    user: &str,
    extra: &[(&str, &str)],
    body: &str,
) -> String {
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    let mut h = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nX-Forwarded-User: {user}\r\nX-Anofox-Proxy-Secret: {SECRET}\r\nConnection: close\r\n"
    );
    for (k, v) in extra {
        h.push_str(&format!("{k}: {v}\r\n"));
    }
    if method == "POST" {
        h.push_str(&format!(
            "Content-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\n",
            body.len()
        ));
    }
    h.push_str("\r\n");
    h.push_str(body);
    s.write_all(h.as_bytes()).unwrap();
    let mut out = String::new();
    let _ = s.read_to_string(&mut out);
    out
}

fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            b => format!("%{b:02X}"),
        })
        .collect()
}

fn token_of(page: &str) -> String {
    let at = page
        .find("name=\"ask_token\" value=\"")
        .expect("an Ask form")
        + 24;
    page[at..at + page[at..].find('"').unwrap()].to_string()
}

#[test]
fn start_needs_a_login_and_no_anonymous() {
    if !duckdb_available() {
        eprintln!("skip: no duckdb CLI");
        return;
    }
    let s = Scratch::new("start");
    let (args, auth) = setup(&s, 9, json!({}));
    let run = |args: &[String], auth: Option<&PathBuf>| {
        let mut c = Command::new(env!("CARGO_BIN_EXE_serve"));
        c.args(args)
            .args(["--port", &free_port().to_string()])
            .env("ANOFOX_M26_KEY", KEY);
        if let Some(a) = auth {
            c.arg("--auth").arg(format!("@{}", a.display()));
        }
        let mut child = c
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let t0 = Instant::now();
        loop {
            if let Some(st) = child.try_wait().unwrap() {
                let mut e = String::new();
                child.stderr.take().unwrap().read_to_string(&mut e).unwrap();
                return (st.success(), e);
            }
            if t0.elapsed() > Duration::from_secs(10) {
                let _ = child.kill();
                return (true, "still running".into());
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    };
    let (ok, err) = run(&args, None);
    assert!(!ok, "Ask without --auth must not start: {err}");
    let s2 = Scratch::new("anon");
    let (args2, auth2) = setup(&s2, 9, json!({"anonymous": true}));
    let (ok, err) = run(&args2, Some(&auth2));
    assert!(!ok && err.contains("anonymous"), "{err}");
    let _ = auth;
}

#[test]
fn ask_on_the_static_render_server() {
    if !duckdb_available() {
        eprintln!("skip: no duckdb CLI");
        return;
    }
    let model = Model::default();
    let model_port = fake_model(model.clone());
    let s = Scratch::new("main");
    let (args, auth) = setup(&s, model_port, json!({}));
    let (_server, port) = start(&args, Some(&auth));

    // the form under an @ask on dashboard; none on another one
    let page = http(port, "GET", "/d/sales", "bob", &[], "");
    assert!(page.starts_with("HTTP/1.1 200"), "{page}");
    assert!(page.contains("action=\"/d/sales/ask\""), "{page}");
    assert!(page.contains("id=\"panel-0\""), "{page}");
    assert!(page.contains("sent to Anthropic (Claude)"), "{page}");
    let token = token_of(&page);
    let plain = http(port, "GET", "/d/plain", "bob", &[], "");
    assert!(!plain.contains("/ask\""), "{plain}");
    let origin = format!("http://127.0.0.1:{port}");
    let same = [("Origin", origin.as_str())];
    let form =
        |token: &str, q: &str| format!("ask_token={}&region=EU&question={}", enc(token), enc(q));
    let p = http(
        port,
        "POST",
        "/d/plain/ask",
        "bob",
        &same,
        &form(&token, "q"),
    );
    assert!(p.starts_with("HTTP/1.1 404"), "{p}");

    // refusals before the model is called
    let p = http(
        port,
        "POST",
        "/d/sales/ask",
        "bob",
        &same,
        "region=EU&question=q",
    );
    assert!(p.starts_with("HTTP/1.1 403"), "no token: {p}");
    let p = http(
        port,
        "POST",
        "/d/sales/ask",
        "alice",
        &same,
        &form(&token, "q"),
    );
    assert!(p.starts_with("HTTP/1.1 403"), "bob's token for alice: {p}");
    let p = http(
        port,
        "POST",
        "/d/sales/ask",
        "bob",
        &[("Origin", "https://evil.example")],
        &form(&token, "q"),
    );
    assert!(p.starts_with("HTTP/1.1 403"), "cross-origin: {p}");
    let p = http(
        port,
        "POST",
        "/d/sales/ask",
        "bob",
        &[
            ("Origin", origin.as_str()),
            ("Sec-Fetch-Site", "cross-site"),
        ],
        &form(&token, "q"),
    );
    assert!(p.starts_with("HTTP/1.1 403"), "cross-site: {p}");
    let p = http(
        port,
        "POST",
        "/d/sales/ask",
        "bob",
        &same,
        &format!("ask_token={}&region=MARS&question=q", enc(&token)),
    );
    assert!(
        p.starts_with("HTTP/1.1 400"),
        "a value outside the param's list: {p}"
    );
    assert!(
        model.seen.lock().unwrap().is_empty(),
        "nothing reached the model"
    );

    // the question: EU, US, a param value outside the list, the skipped
    // panel, then an answer with markup, a URL and a pseudonym
    {
        let mut r = model.replies.lock().unwrap();
        r.push_back(tool("t1", "get_panel", json!({"panel": 0})));
        r.push_back(tool(
            "t2",
            "get_panel",
            json!({"panel": 0, "vars": [{"name": "region", "value": "US"}]}),
        ));
        r.push_back(tool(
            "t3",
            "get_panel",
            json!({"panel": 0, "vars": [{"name": "region", "value": "MARS"}]}),
        ));
        r.push_back(tool("t4", "get_panel", json!({"panel": 1})));
        r.push_back(tool(
            "t5",
            "get_panel",
            json!({"panel": 0, "vars": [{"name": "__claim_user", "value": "alice"}]}),
        ));
    }
    // the answer quotes the first e-mail's pseudonym (known once the first
    // read went upstream): scripted after reading the model's view of it
    let p = {
        let model2 = model.clone();
        let waiter = std::thread::spawn(move || {
            let t0 = Instant::now();
            loop {
                let seen = model2.seen.lock().unwrap().clone();
                if let Some(tok) = seen.iter().find_map(|b| {
                    let i = b.find("⟦E")?;
                    let j = b[i..].find('⟧')? + i + '⟧'.len_utf8();
                    Some(b[i..j].to_string())
                }) {
                    model2.replies.lock().unwrap().push_back(tool(
                        "t6",
                        "answer",
                        json!({"text": format!("**Gadget** leads the US <img src=x onerror=alert(1)>; ask {tok}, see https://evil.example/x"),
                               "scope": "answered",
                               "citations": [
                                   {"panel": 0, "label": "Gadget", "column": "Units", "value": "7", "row": [{"name": "Product", "value": "Gadget"}]},
                                   {"panel": 0, "label": "<b>fake</b>", "column": "Units", "value": "7", "row": [{"name": "Product", "value": "Widget"}]}
                               ]}),
                    ));
                    return;
                }
                if t0.elapsed() > Duration::from_secs(30) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        });
        let p = http(
            port,
            "POST",
            "/d/sales/ask",
            "bob",
            &same,
            &form(&token, "Which product leads?"),
        );
        waiter.join().unwrap();
        p
    };
    assert!(p.starts_with("HTTP/1.1 200"), "{p}");
    assert!(
        p.to_ascii_lowercase()
            .contains("cache-control: no-store, private"),
        "{p}"
    );
    let seen = model.seen.lock().unwrap().clone();
    for b in &seen {
        assert!(
            !b.contains("CANARY") && !b.contains("zed@example.com"),
            "another viewer's row reached the model"
        );
        assert!(
            !b.contains("SKIPCANARY"),
            "an @ask skip panel reached the model"
        );
        assert!(
            !b.contains("ann@example.com") && !b.contains("bo@example.com"),
            "an e-mail address reached the model"
        );
    }
    let all = seen.join("\n");
    assert!(all.contains("Gadget"), "the US read went up");
    assert!(
        all.contains("not one of the options"),
        "a value outside the param's list is refused"
    );
    assert!(
        all.contains("No parameter of this dashboard sets") || all.contains("can't be set"),
        "a claim can't be set: {all}"
    );
    // the answer page
    assert!(
        p.contains("&lt;img src=x onerror=alert(1)&gt;"),
        "the model's markup is escaped"
    );
    assert!(!p.contains("<img src=x"), "no markup of the model's");
    assert!(
        !p.contains("href=\"https://evil.example"),
        "a URL stays text"
    );
    assert!(
        p.contains("ann@example.com"),
        "the pseudonym comes back as the real value"
    );
    assert!(
        p.contains("href=\"/d/sales?region=US#panel-0\""),
        "a verified citation links into the page with its params: {p}"
    );
    assert!(
        p.contains("not verified: &lt;b&gt;fake&lt;/b&gt;"),
        "the right value in the wrong row is not verified: {p}"
    );
    assert!(p.contains("name=\"conversation\""), "a follow-up form");
    assert!(p.contains("id=\"panel-0\""), "the dashboard is on the page");
}
