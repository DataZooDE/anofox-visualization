#![cfg(feature = "serve")]
//! One long-lived `duckdb` CLI session behind the builder's `/query` (#275):
//! what a statement leaves (ATTACH, TEMP tables/views/macros, SET VARIABLE)
//! is there for the next request, broken SQL doesn't desync the markers, and
//! CLI dot-commands are still refused.
mod common;

use std::io::{BufRead, BufReader, Read, Write};
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

/// A scratch directory under `std::env::temp_dir()` (TMPDIR), removed on drop.
struct Scratch(PathBuf);
impl Scratch {
    fn new(tag: &str) -> Self {
        let d = std::env::temp_dir().join(format!("anofox_275_{tag}_{}", std::process::id()));
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

/// Split a raw HTTP/1.1 response into (status-line-and-headers, body),
/// dechunking the body first when `Transfer-Encoding: chunked` is present.
fn http_body(resp: &str) -> String {
    let (head, body) = resp.split_once("\r\n\r\n").expect("header/body split");
    let chunked = head.lines().any(|l| {
        l.to_ascii_lowercase()
            .starts_with("transfer-encoding: chunked")
    });
    if !chunked {
        return body.to_string();
    }
    let mut out = String::new();
    let mut rest = body;
    loop {
        let (size_line, after_size) = rest.split_once("\r\n").expect("chunk size line");
        let size = usize::from_str_radix(size_line.trim(), 16).expect("hex chunk size");
        if size == 0 {
            break;
        }
        out.push_str(&after_size[..size]);
        rest = after_size[size..]
            .strip_prefix("\r\n")
            .expect("chunk trailing CRLF");
    }
    out
}

struct Kill(std::process::Child);
impl Drop for Kill {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// `POST /query` with the session's token, over a fresh connection: `(status,
/// body)`.
fn q(port: u16, tok: &str, sql: &str) -> (u16, String) {
    let mut conn = TcpStream::connect(("127.0.0.1", port)).unwrap();
    conn.set_read_timeout(Some(Duration::from_secs(30)))
        .unwrap();
    write!(
        conn,
        "POST /query HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nX-Anofox-Token: {tok}\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{sql}",
        sql.len()
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

#[test]
fn authoring_session_persists_state() {
    if !have_duckdb("authoring_session_persists_state") {
        return;
    }
    let s = Scratch::new("auth");
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let mut child = Kill(
        Command::new(env!("CARGO_BIN_EXE_serve"))
            .arg(s.0.join("a.duckdb"))
            .args(["--port", &port.to_string(), "--no-open"])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap(),
    );
    let mut out = BufReader::new(child.0.stdout.take().unwrap());
    let mut line = String::new();
    out.read_line(&mut line).unwrap();
    let tok = line
        .split("token=")
        .nth(1)
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .to_string();

    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        match TcpStream::connect(("127.0.0.1", port)) {
            Ok(_) => break,
            Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            Err(e) => panic!("serve did not start on {port}: {e}"),
        }
    }

    // 1. SET VARIABLE persists to the next request.
    let (st, _) = q(port, &tok, "SET VARIABLE x = 42");
    assert_eq!(st, 200);
    let (st, body) = q(port, &tok, "SELECT getvariable('x') AS v");
    assert_eq!(st, 200, "{body}");
    assert!(body.contains("\"v\":42"), "{body}");

    // 2. A TEMP TABLE persists.
    let (st, _) = q(port, &tok, "CREATE TEMP TABLE t275 AS SELECT 7 AS n");
    assert_eq!(st, 200);
    let (st, body) = q(port, &tok, "SELECT n FROM t275");
    assert_eq!(st, 200, "{body}");
    assert!(body.contains("\"n\":7"), "{body}");

    // 3. A TEMP MACRO persists.
    let (st, _) = q(port, &tok, "CREATE TEMP MACRO twice(a) AS a * 2");
    assert_eq!(st, 200);
    let (st, body) = q(port, &tok, "SELECT twice(21) AS m");
    assert_eq!(st, 200, "{body}");
    assert!(body.contains("\"m\":42"), "{body}");

    // 4. ATTACH persists, and a table created on the attached database is
    // visible in the same session.
    let (st, _) = q(port, &tok, "ATTACH ':memory:' AS side");
    assert_eq!(st, 200);
    let (st, _) = q(port, &tok, "CREATE TABLE side.s AS SELECT 1 AS k");
    assert_eq!(st, 200);
    let (st, body) = q(port, &tok, "SELECT k FROM side.s");
    assert_eq!(st, 200, "{body}");
    assert!(body.contains("\"k\":1"), "{body}");

    // 5. Broken SQL (an unterminated string) is an error, but doesn't desync
    // the markers: the next query still answers normally.
    let (st, body) = q(port, &tok, "SELECT 'abc");
    assert_eq!(st, 400, "{body}");
    assert!(body.to_ascii_lowercase().contains("unterminated"), "{body}");
    let (st, body) = q(port, &tok, "SELECT 1 AS ok");
    assert_eq!(st, 200, "{body}");
    assert!(body.contains("\"ok\":1"), "{body}");

    // 6. A statement-level error leaves no stale stderr for the next query.
    let (st, body) = q(port, &tok, "SELECT error('boom')");
    assert_eq!(st, 400, "{body}");
    assert!(body.contains("boom"), "{body}");
    let (st, body) = q(port, &tok, "SELECT 2 AS two");
    assert_eq!(st, 200, "{body}");

    // 7. An aborted transaction answers every statement until ROLLBACK.
    let (st, _) = q(port, &tok, "BEGIN");
    assert_eq!(st, 200);
    let (st, _) = q(port, &tok, "SELECT error('x')");
    assert_eq!(st, 400);
    let (st, body) = q(port, &tok, "SELECT 3 AS three");
    assert_eq!(st, 400, "{body}");
    assert!(body.to_ascii_lowercase().contains("aborted"), "{body}");
    let (st, _) = q(port, &tok, "ROLLBACK");
    assert_eq!(st, 200);
    let (st, body) = q(port, &tok, "SELECT 3 AS three");
    assert_eq!(st, 200, "{body}");
    assert!(body.contains("\"three\":3"), "{body}");

    // 8. A CLI dot-command is still refused before it ever reaches the CLI.
    let (st, body) = q(port, &tok, ".shell echo hi");
    assert_eq!(st, 400, "{body}");
    assert!(body.contains("dot-commands"), "{body}");

    // 9. #275 L1: output with no trailing newline (a `\r` line terminator)
    // doesn't merge with the `.print` end marker and hang the server — the
    // request still answers, and the next one isn't desynced.
    let (st, body) = q(
        port,
        &tok,
        "COPY (SELECT 1) TO '/dev/stdout' (FORMAT csv, NEW_LINE '\\r')",
    );
    assert_eq!(st, 200, "{body}");
    let (st, body) = q(port, &tok, "SELECT 4 AS four");
    assert_eq!(st, 200, "{body}");
    assert!(body.contains("\"four\":4"), "{body}");
}
