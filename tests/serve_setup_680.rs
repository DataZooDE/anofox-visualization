#![cfg(feature = "serve")]
//! #680: `serve --dashboards` runs a dashboard's setup TEMP VIEWs before its
//! panels (each render is a new read-only DuckDB session), skips any other
//! setup statement with a startup warning, and works without `--db`.
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
        let d = std::env::temp_dir().join(format!("anofox_680_{tag}_{}", std::process::id()));
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

fn http_get(port: u16, path: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let attempt = (|| -> std::io::Result<String> {
            let mut conn = TcpStream::connect(("127.0.0.1", port))?;
            conn.set_read_timeout(Some(Duration::from_secs(60)))?;
            write!(
                conn,
                "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"
            )?;
            let mut resp = String::new();
            conn.read_to_string(&mut resp)?;
            Ok(resp)
        })();
        match attempt {
            Ok(r) if !r.is_empty() => return r,
            Ok(_) | Err(_) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(100))
            }
            Ok(_) => panic!("serve on {port} answered nothing"),
            Err(e) => panic!("serve on {port}: {e}"),
        }
    }
}

/// Kills the server on drop (a failed assertion included).
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
fn setup_temp_views_reach_the_panels_without_a_db() {
    if !have_duckdb("setup_temp_views_reach_the_panels_without_a_db") {
        return;
    }
    let s = Scratch::new("tv");
    let dash = s.0.join("dash");
    std::fs::create_dir_all(&dash).unwrap();
    std::fs::write(
        dash.join("a.sql"),
        "-- @title A\nCREATE OR REPLACE TEMP VIEW v AS SELECT 680 AS x; -- trailing comment\n\
         SELECT sum(x)::METRIC, 'x'::LABEL FROM v;\n",
    )
    .unwrap();
    // a setup statement the read-only server can't run: skipped, warned
    std::fs::write(
        dash.join("b.sql"),
        "-- @title B\nCREATE TABLE t AS SELECT 1 AS x;\nSELECT 42::METRIC, 'b'::LABEL;\n",
    )
    .unwrap();
    // a claim is the server's to set: never from a dashboard's setup
    std::fs::write(
        dash.join("c.sql"),
        "-- @title C\nSET VARIABLE __claim_user = 'mallory';\n\
         SELECT coalesce(getvariable('__claim_user'), 'nobody') AS who ::TABLE;\n",
    )
    .unwrap();
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let server = Kill(Some(
        Command::new(env!("CARGO_BIN_EXE_serve"))
            .arg("--dashboards")
            .arg(&dash)
            .args(["--port", &port.to_string(), "--no-open"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap(),
    ));
    let a = http_get(port, "/d/a");
    assert!(a.starts_with("HTTP/1.1 200"), "{a}");
    assert!(a.contains(">680<"), "the TEMP VIEW's value: {a}");
    let b = http_get(port, "/d/b");
    assert!(b.starts_with("HTTP/1.1 200"), "{b}");
    assert!(b.contains(">42<"), "{b}");
    let c = http_get(port, "/d/c");
    assert!(!c.contains("mallory"), "{c}");
    let out = server.finish();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("dashboard 'b': setup statement 1 is skipped"),
        "{stderr}"
    );
    assert!(
        stderr
            .contains("dashboard 'c': setup statement 1 is skipped (it sets a reserved variable)"),
        "{stderr}"
    );
    assert!(!stderr.contains("failed:"), "{stderr}");
}

#[test]
fn a_missing_named_db_refuses_to_start() {
    let s = Scratch::new("nodb");
    let dash = s.0.join("dash");
    std::fs::create_dir_all(&dash).unwrap();
    std::fs::write(dash.join("a.sql"), "SELECT 1::METRIC;\n").unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_serve"))
        .arg("--dashboards")
        .arg(&dash)
        .arg("--db")
        .arg(s.0.join("missing.duckdb"))
        .args(["--port", "8681", "--no-open"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("no such database"), "{stderr}");
}
