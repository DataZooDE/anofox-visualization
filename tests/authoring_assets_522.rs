#![cfg(feature = "serve")]
//! #522: the authoring builder (`serve [db]`, loopback-only) has no
//! dashboards folder, so it never served `assets/…` even after #514 PR B —
//! `--assets <dir>` is a new, explicit, opt-in root for `GET`/`HEAD
//! /assets/<rest>`, off by default. These are the containment/auth
//! negative tests from the #522 brief, against the real `serve` binary
//! over a raw TCP connection (no `duckdb` CLI needed: none of this reaches
//! `/query`).

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::Command;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Every test here grabs an OS-assigned "free" port ([`free_port`]) and
/// then spawns its own `serve` child bound to it — but bind-then-release
/// only reserves the number for a moment, and under this file's own test
/// parallelism (several of these tests at once, same process) two of them
/// can race for the same just-released port before either child actually
/// binds it. One serialization point for the whole file turns that into a
/// non-issue (no two of these tests ever run their socket-bound bodies at
/// the same time) rather than chasing the resulting flakiness per test;
/// other test *binaries* (a separate process each) are unaffected.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

/// A per-test throwaway directory (removed on drop), with an `assets/`
/// subfolder holding one real SVG and one non-image file.
struct Scratch(PathBuf);
impl Scratch {
    fn new(tag: &str) -> Self {
        let d = std::env::temp_dir().join(format!("anofox_522_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let assets = d.join("assets");
        std::fs::create_dir_all(&assets).unwrap();
        std::fs::write(
            assets.join("pic.svg"),
            br#"<svg xmlns="http://www.w3.org/2000/svg" width="4" height="4"><rect width="4" height="4"/></svg>"#,
        )
        .unwrap();
        std::fs::write(assets.join("notes.txt"), b"not an image").unwrap();
        Scratch(d)
    }
    fn assets_dir(&self) -> PathBuf {
        self.0.join("assets")
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

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// Spawn `serve --db <scratch>/a.duckdb [extra] --port <port> --no-open`,
/// read its token off the first stdout line, and wait until it accepts
/// connections. Panics (failing the test) if the process exits early —
/// use [`spawn_expect_refusal`] instead when a bad argument should make it
/// refuse to start.
fn spawn_authoring(scratch: &Scratch, port: u16, extra: &[&str]) -> (Kill, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_serve"))
        .arg(scratch.0.join("a.duckdb"))
        .args(["--port", &port.to_string(), "--no-open"])
        .args(extra)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut out = std::io::BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    use std::io::BufRead;
    out.read_line(&mut line).unwrap();
    let tok = line
        .split("token=")
        .nth(1)
        .unwrap_or_else(|| panic!("no token in first stdout line: {line:?}"))
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
    (Kill(child), tok)
}

/// `serve --db <scratch>/a.duckdb --assets <bad>` is expected to refuse to
/// start (exit code 2, a message on stderr) — never bind the port.
fn spawn_expect_refusal(scratch: &Scratch, assets: &str) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_serve"))
        .arg(scratch.0.join("a.duckdb"))
        .args([
            "--assets",
            assets,
            "--port",
            &free_port().to_string(),
            "--no-open",
        ])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// One raw HTTP/1.1 request (no cookie dance: the token goes straight in
/// `X-Anofox-Token`, the same header `/query`'s own tests use) over a
/// fresh connection: `(status, lowercased head, body)`. This test binary's
/// several tests each grab their own ephemeral port (`free_port`) and
/// start their own `serve` child, but under heavy parallel load on a
/// shared, busy box a just-released port can occasionally be re-handed
/// out before the owning child's listener is actually ready to answer,
/// producing one empty response — retried a few times (status `0`, never
/// a real HTTP status, is the tell) rather than call that a failure of
/// the route itself.
fn req(port: u16, method: &str, path: &str, tok: Option<&str>) -> (u16, String, String) {
    for attempt in 0..5 {
        let mut conn = TcpStream::connect(("127.0.0.1", port)).unwrap();
        conn.set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let tok_header = tok
            .map(|t| format!("X-Anofox-Token: {t}\r\n"))
            .unwrap_or_default();
        write!(
            conn,
            "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n{tok_header}Connection: close\r\n\r\n"
        )
        .unwrap();
        let mut resp = String::new();
        conn.read_to_string(&mut resp).unwrap();
        let (head, body) = resp.split_once("\r\n\r\n").unwrap_or((resp.as_str(), ""));
        let status: u16 = head
            .lines()
            .next()
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        if status != 0 || attempt == 4 {
            return (status, head.to_ascii_lowercase(), body.to_string());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    unreachable!()
}

#[test]
fn assets_route_serves_a_valid_image_with_the_locked_servers_own_headers() {
    let _guard = ONE_AT_A_TIME.lock().unwrap_or_else(|p| p.into_inner());
    let s = Scratch::new("ok");
    let port = free_port();
    let (_child, tok) = spawn_authoring(&s, port, &["--assets", s.assets_dir().to_str().unwrap()]);

    let (status, head, body) = req(port, "GET", "/assets/pic.svg", Some(&tok));
    assert_eq!(status, 200, "{head}");
    assert!(head.contains("content-type: image/svg+xml"), "{head}");
    assert!(head.contains("x-content-type-options: nosniff"), "{head}");
    assert!(
        head.contains(
            "content-security-policy: default-src 'none'; style-src 'unsafe-inline'; sandbox"
        ),
        "{head}"
    );
    assert!(
        head.contains("cache-control: private, max-age=60"),
        "{head}"
    );
    assert!(
        head.contains("cross-origin-resource-policy: same-origin"),
        "{head}"
    );
    assert!(body.contains("<svg"), "{body}");

    // HEAD: same headers, no body, Content-Length agrees with GET's.
    let (status, head2, body2) = req(port, "HEAD", "/assets/pic.svg", Some(&tok));
    assert_eq!(status, 200, "{head2}");
    assert!(body2.is_empty(), "{body2:?}");
    let len = |h: &str| {
        h.lines()
            .find_map(|l| l.strip_prefix("content-length: "))
            .map(|v| v.trim())
            .unwrap()
            .to_string()
    };
    assert_eq!(len(&head), len(&head2), "{head}\n---\n{head2}");
}

#[test]
fn assets_route_refuses_traversal_bad_extension_and_missing_files() {
    let _guard = ONE_AT_A_TIME.lock().unwrap_or_else(|p| p.into_inner());
    let s = Scratch::new("neg");
    let port = free_port();
    let (_child, tok) = spawn_authoring(&s, port, &["--assets", s.assets_dir().to_str().unwrap()]);

    for (path, why) in [
        ("/assets/../a.duckdb", "literal .. segment"),
        ("/assets/%2e%2e/a.duckdb", "percent-encoded .."),
        ("/assets/..%2fa.duckdb", "percent-encoded slash"),
        ("/assets//etc/passwd", "absolute-looking path"),
        ("/assets/notes.txt", "extension not on the allow-list"),
        ("/assets/nope.svg", "file does not exist"),
        ("/assets/.hidden.svg", "dot-leading segment"),
    ] {
        let (status, head, _) = req(port, "GET", path, Some(&tok));
        assert_eq!(status, 404, "{path} ({why}): {head}");
    }
}

#[cfg(unix)]
#[test]
fn assets_route_refuses_a_symlink_escaping_the_root() {
    let _guard = ONE_AT_A_TIME.lock().unwrap_or_else(|p| p.into_inner());
    let s = Scratch::new("symlink-escape");
    std::os::unix::fs::symlink("/etc", s.assets_dir().join("escape")).unwrap();
    let port = free_port();
    let (_child, tok) = spawn_authoring(&s, port, &["--assets", s.assets_dir().to_str().unwrap()]);
    let (status, head, _) = req(port, "GET", "/assets/escape/hostname", Some(&tok));
    assert_eq!(status, 404, "{head}");
}

#[test]
fn assets_route_needs_the_same_token_as_every_other_authoring_route() {
    let _guard = ONE_AT_A_TIME.lock().unwrap_or_else(|p| p.into_inner());
    let s = Scratch::new("auth");
    let port = free_port();
    let (_child, _tok) = spawn_authoring(&s, port, &["--assets", s.assets_dir().to_str().unwrap()]);
    let (status, head, _) = req(port, "GET", "/assets/pic.svg", None);
    assert_eq!(status, 401, "{head}");
}

#[test]
fn assets_route_is_get_head_only() {
    let _guard = ONE_AT_A_TIME.lock().unwrap_or_else(|p| p.into_inner());
    let s = Scratch::new("method");
    let port = free_port();
    let (_child, tok) = spawn_authoring(&s, port, &["--assets", s.assets_dir().to_str().unwrap()]);
    let (status, head, _) = req(port, "POST", "/assets/pic.svg", Some(&tok));
    assert_eq!(status, 405, "{head}");
}

#[test]
fn assets_option_off_is_a_plain_404_not_a_different_error() {
    let _guard = ONE_AT_A_TIME.lock().unwrap_or_else(|p| p.into_inner());
    let s = Scratch::new("off");
    let port = free_port();
    // No --assets at all: today's unchanged behaviour.
    let (_child, tok) = spawn_authoring(&s, port, &[]);
    let (status, head, _) = req(port, "GET", "/assets/pic.svg", Some(&tok));
    assert_eq!(status, 404, "{head}");
}

#[test]
fn assets_option_refuses_a_missing_directory_at_start() {
    let _guard = ONE_AT_A_TIME.lock().unwrap_or_else(|p| p.into_inner());
    let s = Scratch::new("bad-missing");
    let err = spawn_expect_refusal(&s, s.0.join("no-such-dir").to_str().unwrap());
    assert!(err.contains("--assets"), "{err}");
}

#[test]
fn assets_option_refuses_a_non_directory_at_start() {
    let _guard = ONE_AT_A_TIME.lock().unwrap_or_else(|p| p.into_inner());
    let s = Scratch::new("bad-file");
    let err = spawn_expect_refusal(&s, s.assets_dir().join("pic.svg").to_str().unwrap());
    assert!(err.contains("not a directory"), "{err}");
}

#[cfg(unix)]
#[test]
fn assets_option_refuses_a_symlinked_root_at_start() {
    let _guard = ONE_AT_A_TIME.lock().unwrap_or_else(|p| p.into_inner());
    let s = Scratch::new("bad-symlink");
    let link = s.0.join("assets-link");
    std::os::unix::fs::symlink(s.assets_dir(), &link).unwrap();
    let err = spawn_expect_refusal(&s, link.to_str().unwrap());
    assert!(err.contains("symlink"), "{err}");
}
