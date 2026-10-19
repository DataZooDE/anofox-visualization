#![cfg(feature = "serve")]
//! The script-free page's panel errors (#279): a DuckDB error is never shown
//! to the viewer (it can quote file paths, SQL and values) — the viewer gets
//! a generic 400, and the server logs the detail to stderr.
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

/// A scratch directory under `std::env::temp_dir()` (TMPDIR), removed on drop.
struct Scratch(PathBuf);
impl Scratch {
    fn new(tag: &str) -> Self {
        let d = std::env::temp_dir().join(format!("anofox_279_{tag}_{}", std::process::id()));
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

/// Kills the child on drop (so an early panic never leaks the server); `finish`
/// kills it, then returns its full output (including the stderr piped above),
/// for the test's own assertions.
struct Kill(Option<std::process::Child>);
impl Kill {
    fn finish(mut self) -> std::process::Output {
        let mut child = self.0.take().expect("child present");
        let _ = child.kill();
        child.wait_with_output().unwrap()
    }
}
impl Drop for Kill {
    fn drop(&mut self) {
        if let Some(mut c) = self.0.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

#[test]
fn serve_page_hides_duckdb_errors() {
    if !have_duckdb("serve_page_hides_duckdb_errors") {
        return;
    }
    let s = Scratch::new("hide");
    let dash = s.0.join("dash");
    std::fs::create_dir_all(&dash).unwrap();
    std::fs::write(
        dash.join("broken.sql"),
        "SELECT x::XAXIS, y::BARCHART FROM secret_table_279;\n",
    )
    .unwrap();
    let init = s.0.join("init.sql");
    std::fs::write(&init, "CREATE TABLE IF NOT EXISTS t AS SELECT 1 AS one;\n").unwrap();
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let server = Kill(Some(
        Command::new(env!("CARGO_BIN_EXE_serve"))
            .arg("--dashboards")
            .arg(&dash)
            .arg("--db")
            .arg(s.0.join("t.duckdb"))
            .arg("--init")
            .arg(&init)
            .args(["--port", &port.to_string(), "--no-open"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap(),
    ));
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut conn = loop {
        match TcpStream::connect(("127.0.0.1", port)) {
            Ok(c) => break c,
            Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(100)),
            Err(e) => panic!("serve did not start on {port}: {e}"),
        }
    };
    conn.set_read_timeout(Some(Duration::from_secs(60)))
        .unwrap();
    write!(
        conn,
        "GET /d/broken HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut resp = String::new();
    conn.read_to_string(&mut resp).unwrap();
    assert!(resp.starts_with("HTTP/1.1 400"), "{resp}");
    let body = http_body(&resp);
    assert_eq!(
        body.trim(),
        "this dashboard could not be shown (the server log has the details)"
    );
    assert!(!body.contains("secret_table_279"), "{body}");

    let out = server.finish();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("[anofox-serve] dashboard 'broken' failed:"),
        "{stderr}"
    );
    assert!(stderr.contains("secret_table_279"), "{stderr}");
}
