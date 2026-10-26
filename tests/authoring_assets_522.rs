#![cfg(feature = "serve")]
//! #522: the authoring builder (`serve [db]`, loopback-only) has no
//! dashboards folder, so it never served `assets/…` even after #514 PR B —
//! `--assets <dir>` is a new, explicit, opt-in root for `GET`/`HEAD
//! /assets/<rest>`, off by default. These are the containment/auth
//! negative tests from the #522 brief, against the real `serve` binary
//! over a raw TCP connection (no `duckdb` CLI needed: none of this reaches
//! `/query`).
//!
//! Review of #604 S2: every test asks the binary itself for a port
//! (`--port 0`, resolved/printed by the server, never guessed up front —
//! see `src/bin/serve.rs::authoring_mode`) rather than binding-then-
//! releasing one and racing to hand it to a spawned child. That race was
//! real: `cargo nextest` runs each test in its own *process*, so an
//! in-file `Mutex` (this file's first cut) doesn't serialize anything
//! across them, and CI hit `ConnectionReset` from exactly this. `--port 0`
//! has no such gap — the only bind is the real server's own, and it
//! reports back whatever the OS gave it.

use std::io::{BufRead, Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

/// A per-test throwaway directory (removed on drop), with an `assets/`
/// subfolder holding one real SVG and one non-image file, and a sibling
/// `outside/` directory (never under `assets/`) holding another real SVG
/// — review of #604 S3: a symlink *to* that file, reached *through*
/// `assets/`, is what a real containment test needs; a target with no
/// image extension (the first cut's `/etc`) is already refused at the
/// extension check, before containment is even reached.
struct Scratch(PathBuf);
impl Scratch {
    fn new(tag: &str) -> Self {
        let d = std::env::temp_dir().join(format!("anofox_522_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let assets = d.join("assets");
        std::fs::create_dir_all(&assets).unwrap();
        std::fs::write(assets.join("pic.svg"), SVG).unwrap();
        std::fs::write(assets.join("notes.txt"), b"not an image").unwrap();
        let outside = d.join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("secret.svg"), SVG).unwrap();
        Scratch(d)
    }
    fn assets_dir(&self) -> PathBuf {
        self.0.join("assets")
    }
    fn outside_dir(&self) -> PathBuf {
        self.0.join("outside")
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

const SVG: &[u8] =
    br#"<svg xmlns="http://www.w3.org/2000/svg" width="4" height="4"><rect width="4" height="4"/></svg>"#;

struct Kill(std::process::Child);
impl Drop for Kill {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Spawn `serve --db <scratch>/a.duckdb [extra] --port 0 --no-open`, parse
/// the port and token the server itself resolved and printed on its first
/// stdout line (`anofox-visualization serving http://127.0.0.1:<port>/
/// ?token=<tok>`), and wait until it accepts connections on that exact
/// port. Panics (failing the test) if the process exits early — use
/// [`spawn_expect_refusal`] instead when a bad argument should make it
/// refuse to start.
fn spawn_authoring(scratch: &Scratch, extra: &[&str]) -> (Kill, u16, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_serve"))
        .arg(scratch.0.join("a.duckdb"))
        .args(["--port", "0", "--no-open"])
        .args(extra)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut out = std::io::BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    out.read_line(&mut line).unwrap();
    let port: u16 = line
        .split("127.0.0.1:")
        .nth(1)
        .and_then(|s| s.split(['/', '?']).next())
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| panic!("no port in first stdout line: {line:?}"));
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
    (Kill(child), port, tok)
}

/// `serve --db <scratch>/a.duckdb --assets <bad> --port 0` is expected to
/// refuse to start (exit code 2, a message on stderr) — before it ever
/// gets as far as resolving/binding a port.
fn spawn_expect_refusal(scratch: &Scratch, assets: &str) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_serve"))
        .arg(scratch.0.join("a.duckdb"))
        .args(["--assets", assets, "--port", "0", "--no-open"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// One raw HTTP/1.1 request (no cookie dance: the token goes straight in
/// `X-Anofox-Token`, the same header `/query`'s own tests use) over a
/// fresh connection: `(status, lowercased head, body)`.
fn req(port: u16, method: &str, path: &str, tok: Option<&str>) -> (u16, String, String) {
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
    (status, head.to_ascii_lowercase(), body.to_string())
}

#[test]
fn assets_route_serves_a_valid_image_with_the_locked_servers_own_headers() {
    let s = Scratch::new("ok");
    let (_child, port, tok) = spawn_authoring(&s, &["--assets", s.assets_dir().to_str().unwrap()]);

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
    let s = Scratch::new("neg");
    let (_child, port, tok) = spawn_authoring(&s, &["--assets", s.assets_dir().to_str().unwrap()]);

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

/// Review of #604 S3: the first cut of this test sent `/assets/escape/
/// hostname`, which `open_validated_asset` already refuses at the
/// extension check (`hostname` has none) — a would-be-containment test
/// that would still pass with containment itself removed. These reach a
/// *real* image on the other side of the symlink, through both shapes
/// (a symlinked subdirectory, and a symlinked leaf file), so a regression
/// in `canonicalize(..).starts_with(root)` itself would actually show up
/// as an unexpected 200.
#[cfg(unix)]
#[test]
fn assets_route_refuses_a_symlink_escaping_the_root() {
    let s = Scratch::new("symlink-escape");
    std::os::unix::fs::symlink(s.outside_dir(), s.assets_dir().join("sublink")).unwrap();
    std::os::unix::fs::symlink(
        s.outside_dir().join("secret.svg"),
        s.assets_dir().join("leaf.svg"),
    )
    .unwrap();
    let (_child, port, tok) = spawn_authoring(&s, &["--assets", s.assets_dir().to_str().unwrap()]);
    for path in ["/assets/sublink/secret.svg", "/assets/leaf.svg"] {
        let (status, head, _) = req(port, "GET", path, Some(&tok));
        assert_eq!(status, 404, "{path}: {head}");
    }
}

#[test]
fn assets_route_needs_the_same_token_as_every_other_authoring_route() {
    let s = Scratch::new("auth");
    let (_child, port, _tok) = spawn_authoring(&s, &["--assets", s.assets_dir().to_str().unwrap()]);
    let (status, head, _) = req(port, "GET", "/assets/pic.svg", None);
    assert_eq!(status, 401, "{head}");
}

#[test]
fn assets_route_is_get_head_only() {
    let s = Scratch::new("method");
    let (_child, port, tok) = spawn_authoring(&s, &["--assets", s.assets_dir().to_str().unwrap()]);
    let (status, head, _) = req(port, "POST", "/assets/pic.svg", Some(&tok));
    assert_eq!(status, 405, "{head}");
}

#[test]
fn assets_option_off_is_a_plain_404_not_a_different_error() {
    let s = Scratch::new("off");
    // No --assets at all: today's unchanged behaviour.
    let (_child, port, tok) = spawn_authoring(&s, &[]);
    let (status, head, _) = req(port, "GET", "/assets/pic.svg", Some(&tok));
    assert_eq!(status, 404, "{head}");
}

#[test]
fn assets_option_refuses_a_missing_directory_at_start() {
    let s = Scratch::new("bad-missing");
    let err = spawn_expect_refusal(&s, s.0.join("no-such-dir").to_str().unwrap());
    assert!(err.contains("--assets"), "{err}");
}

#[test]
fn assets_option_refuses_a_non_directory_at_start() {
    let s = Scratch::new("bad-file");
    let err = spawn_expect_refusal(&s, s.assets_dir().join("pic.svg").to_str().unwrap());
    assert!(err.contains("not a directory"), "{err}");
}

#[cfg(unix)]
#[test]
fn assets_option_refuses_a_symlinked_root_at_start() {
    let s = Scratch::new("bad-symlink");
    let link = s.0.join("assets-link");
    std::os::unix::fs::symlink(s.assets_dir(), &link).unwrap();
    let err = spawn_expect_refusal(&s, link.to_str().unwrap());
    assert!(err.contains("symlink"), "{err}");
}

/// Review of #604 S5: a trailing slash (or `/.`) on a symlinked root used
/// to reach `std::fs::symlink_metadata` by way of a path that *follows*
/// the link instead of stat-ing it, so the refusal above didn't fire —
/// `--assets rootlink/` (the shape a shell's own tab-completion produces)
/// started right up and served the link's target. Same root as the test
/// above, named with a trailing slash.
#[cfg(unix)]
#[test]
fn assets_option_refuses_a_symlinked_root_at_start_even_with_a_trailing_slash() {
    let s = Scratch::new("bad-symlink-trailing-slash");
    let link = s.0.join("assets-link");
    std::os::unix::fs::symlink(s.assets_dir(), &link).unwrap();
    let with_slash = format!("{}/", link.to_str().unwrap());
    let err = spawn_expect_refusal(&s, &with_slash);
    assert!(err.contains("symlink"), "{err}");
}
