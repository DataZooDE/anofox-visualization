#![cfg(feature = "serve")]
//! #340 finding A: on the script-free served page, a `-- @param`'s allowed
//! value containing a C0 control character must round-trip through the
//! no-JS `<select>`/`<option>` form — the browser submits exactly what the
//! author declared, and the server's allow-list check accepts it.
mod common;

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

fn have_duckdb(test: &str) -> bool {
    let ok = Command::new("duckdb").arg("-version").output().is_ok();
    if !ok {
        common::skip_without("duckdb", test);
    }
    ok
}

struct Scratch(PathBuf);
impl Scratch {
    fn new(tag: &str) -> Self {
        let d = std::env::temp_dir().join(format!("anofox_340a_{tag}_{}", std::process::id()));
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

fn http_body(resp: &str) -> String {
    resp.split_once("\r\n\r\n")
        .map(|(_, b)| b.to_string())
        .unwrap_or_default()
}

fn get(port: u16, path: &str) -> (u16, String) {
    let mut conn = TcpStream::connect(("127.0.0.1", port)).unwrap();
    conn.set_read_timeout(Some(Duration::from_secs(30)))
        .unwrap();
    write!(
        conn,
        "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut resp = String::new();
    conn.read_to_string(&mut resp).unwrap();
    let status: u16 = resp
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    (status, http_body(&resp))
}

/// Percent-encode exactly like a browser's `application/x-www-form-urlencoded`
/// GET submission would for the bytes of a control character.
fn percent_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[test]
fn an_allowed_value_with_a_control_character_round_trips() {
    if !have_duckdb("an_allowed_value_with_a_control_character_round_trips") {
        return;
    }
    let s = Scratch::new("rt");
    let dash = s.0.join("dash");
    std::fs::create_dir_all(&dash).unwrap();
    // The allowed value "a\x01b" (SOH, a C0 control character that is
    // neither tab, LF nor CR) — the sentinel the no-JS form must submit
    // back exactly for the request to pass the allow-list check.
    let special = "a\u{1}b";
    std::fs::write(
        dash.join("opt.sql"),
        format!("-- @param lvl [normal, {special}] = normal\nSELECT 1::METRIC;\n"),
    )
    .unwrap();
    let init = s.0.join("init.sql");
    std::fs::write(&init, "CREATE TABLE IF NOT EXISTS t AS SELECT 1 AS x;\n").unwrap();
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let _server = Kill(
        Command::new(env!("CARGO_BIN_EXE_serve"))
            .arg("--dashboards")
            .arg(&dash)
            .arg("--db")
            .arg(s.0.join("t.duckdb"))
            .arg("--init")
            .arg(&init)
            .args(["--port", &port.to_string(), "--no-open"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        match TcpStream::connect(("127.0.0.1", port)) {
            Ok(_) => break,
            Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(100)),
            Err(e) => panic!("serve did not start on {port}: {e}"),
        }
    }

    // The default page: the special option's `value` attribute carries the
    // control character as a numeric character reference (an HTML5 parser
    // decodes it back to the exact code point), not as the raw byte or a
    // lossy substitution — and it is not (yet) selected.
    let (st, body) = get(port, "/d/opt");
    assert_eq!(st, 200, "{body}");
    assert!(
        body.contains("<option value=\"a&#1;b\">"),
        "no round-trippable value attribute: {body}"
    );
    assert!(
        !body.contains("<option value=\"a&#1;b\" selected>"),
        "the special option should not be selected by default: {body}"
    );

    // Exactly what a browser submits after decoding that attribute and
    // percent-encoding it for a GET form: the request is accepted (the
    // allow-list check sees the author's own declared value, byte for
    // byte), not rejected with a 400.
    let qs = percent_encode(special);
    let (st, body) = get(port, &format!("/d/opt?lvl={qs}"));
    assert_eq!(st, 200, "{body}");
    assert!(
        body.contains("<option value=\"a&#1;b\" selected>"),
        "the special value should now be selected: {body}"
    );

    // The naive (pre-fix) submission — the escaped *text* content, as a
    // browser would read it back from an `<option>` with no `value`
    // attribute of its own — must NOT be treated as if it were allowed:
    // the allow-list still compares against the author's real value.
    let (st, body) = get(port, "/d/opt?lvl=a%EF%BF%BDb");
    assert_eq!(st, 400, "{body}");
}
