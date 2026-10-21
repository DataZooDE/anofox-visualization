#![cfg(feature = "serve")]
//! One long-lived `duckdb` CLI session behind the builder's `/query` (#275):
//! what a statement leaves (ATTACH, TEMP tables/views/macros, SET VARIABLE)
//! is there for the next request, broken SQL doesn't desync the markers, and
//! CLI dot-commands are still refused.
mod common;

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
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

    // 10. #304: a read of the CLI's own stdin — the pipe this session's
    // staged queries are written through — is refused before it ever
    // reaches the CLI, instead of hanging the only authoring worker until
    // the process is killed and restarted. Checked case-insensitively and
    // for the `/dev/fd/0` alias too; the session still answers afterwards.
    for sql in [
        "SELECT * FROM read_csv('/dev/stdin')",
        "SELECT * FROM READ_CSV('/DEV/STDIN')",
        "COPY t275 FROM '/dev/fd/0'",
    ] {
        let (st, body) = q(port, &tok, sql);
        assert_eq!(st, 400, "{sql}: {body}");
        assert!(body.contains("/dev/stdin"), "{sql}: {body}");
    }
    let (st, body) = q(port, &tok, "SELECT 5 AS five");
    assert_eq!(st, 200, "{body}");
    assert!(body.contains("\"five\":5"), "{body}");
}

/// sec-review-342 L1: `reads_stdin` is a cheap, best-effort fast path — it
/// cannot see a path built at query time. The structural backstop is the
/// per-round timeout (`--query-timeout`): a round that doesn't finish in
/// time gets its `duckdb` child killed, the client gets a clear error
/// within that timeout (not a hang until someone kills the process), and
/// the next query starts a fresh session that works normally.
#[test]
fn authoring_session_recovers_from_an_unblocked_stdin_read_via_the_round_timeout() {
    if !have_duckdb("authoring_session_recovers_from_an_unblocked_stdin_read_via_the_round_timeout")
    {
        return;
    }
    let s = Scratch::new("timeout");
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let mut child = Kill(
        Command::new(env!("CARGO_BIN_EXE_serve"))
            .arg(s.0.join("a.duckdb"))
            .args([
                "--port",
                &port.to_string(),
                "--no-open",
                "--query-timeout",
                "2",
            ])
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

    // A path built at query time (string concatenation): `reads_stdin`
    // cannot see this is `/dev/stdin`, so it reaches the CLI and blocks —
    // confirmed in sec-review-342 as a real bypass of the fast path.
    let t0 = Instant::now();
    let (st, body) = q(port, &tok, "SELECT * FROM read_csv('/dev/' || 'stdin')");
    let waited = t0.elapsed();
    assert_eq!(st, 400, "{body}");
    assert!(
        body.contains("exceeded") && body.contains("stopped"),
        "{body}"
    );
    // It took roughly the configured timeout, not instantly (it really
    // went through the CLI and hung) and not "until someone kills -9 the
    // process" (it recovered on its own).
    assert!(
        waited >= Duration::from_secs(2) && waited < Duration::from_secs(15),
        "{waited:?}"
    );

    // The session works normally afterwards: a fresh child, fresh state.
    let (st, body) = q(port, &tok, "SELECT 1 AS ok");
    assert_eq!(st, 200, "{body}");
    assert!(body.contains("\"ok\":1"), "{body}");
}

/// #304: the authoring session's private staging directory
/// (`anofox-session-*`, holding each round's `q.sql`) is removed even when
/// the process is killed with SIGTERM instead of exiting normally — before
/// this, `Drop for AuthoringSession` simply never ran on a kill, and every
/// run leaked one such directory into the temp dir.
#[cfg(unix)]
#[test]
fn authoring_session_removes_private_dir_on_sigterm() {
    if !have_duckdb("authoring_session_removes_private_dir_on_sigterm") {
        return;
    }
    fn session_dirs(tmp: &Path) -> Vec<PathBuf> {
        std::fs::read_dir(tmp)
            .map(|it| {
                it.flatten()
                    .map(|e| e.path())
                    .filter(|p| {
                        p.file_name()
                            .and_then(|n| n.to_str())
                            .is_some_and(|n| n.starts_with("anofox-session-"))
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    let s = Scratch::new("sigterm");
    // Its own TMPDIR (not the shared machine-wide one): other tests in this
    // binary create their own session dirs concurrently, and `private_dir`
    // reads `TMPDIR`, so this keeps the two from colliding.
    let tmp = s.0.join("tmp");
    std::fs::create_dir(&tmp).unwrap();
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let mut child = Kill(
        Command::new(env!("CARGO_BIN_EXE_serve"))
            .arg(s.0.join("a.duckdb"))
            .args(["--port", &port.to_string(), "--no-open"])
            .env("TMPDIR", &tmp)
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
    // The session's directory is created eagerly at startup (#275), but make
    // sure the session (and the private dir holding this round's q.sql) is
    // fully up before we kill it.
    let (st, _) = q(port, &tok, "SELECT 1");
    assert_eq!(st, 200);

    let dirs = session_dirs(&tmp);
    assert_eq!(dirs.len(), 1, "{dirs:?}");
    let dir = dirs[0].clone();
    assert!(dir.is_dir(), "{}", dir.display());

    let pid = child.0.id();
    let rc = unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
    assert_eq!(rc, 0, "kill(SIGTERM) on {pid}");

    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match child.0.try_wait().unwrap() {
            Some(_) => break,
            None if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            None => panic!("serve did not exit within 10s of SIGTERM"),
        }
    }
    assert!(
        !dir.exists(),
        "{} should have been removed by the SIGTERM handler",
        dir.display()
    );
}

/// The process's thread count, from `/proc/<pid>/status`'s `Threads:` line.
#[cfg(target_os = "linux")]
fn thread_count(pid: u32) -> usize {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).unwrap();
    status
        .lines()
        .find_map(|l| l.strip_prefix("Threads:"))
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or_else(|| panic!("no Threads: line in /proc/{pid}/status:\n{status}"))
}

/// sec-review-342 L3: the old watchdog spawned a thread that slept the
/// *full* round timeout (default 120 s) no matter how fast the round
/// actually finished, so a client sending queries in a loop built up one
/// parked thread per round. The fixed watchdog blocks on
/// `recv_timeout(timeout)` and is `join`ed before the round returns, so a
/// fast round's watchdog wakes (and the thread exits) almost immediately
/// instead of outliving the round.
#[cfg(target_os = "linux")]
#[test]
fn authoring_session_fast_rounds_do_not_leak_watchdog_threads() {
    if !have_duckdb("authoring_session_fast_rounds_do_not_leak_watchdog_threads") {
        return;
    }
    let s = Scratch::new("threads");
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let mut child = Kill(
        Command::new(env!("CARGO_BIN_EXE_serve"))
            .arg(s.0.join("a.duckdb"))
            .args([
                "--port",
                &port.to_string(),
                "--no-open",
                "--query-timeout",
                "2",
            ])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap(),
    );
    let pid = child.0.id();
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
    // One round so the session's `duckdb` child (and its stderr-reader
    // thread) is already up before the baseline is taken.
    let (st, _) = q(port, &tok, "SELECT 1");
    assert_eq!(st, 200);
    let before = thread_count(pid);

    for i in 0..500 {
        let (st, body) = q(port, &tok, &format!("SELECT {i} AS n"));
        assert_eq!(st, 200, "round {i}: {body}");
    }

    let after = thread_count(pid);
    assert!(
        after <= before + 5,
        "thread count grew from {before} to {after} over 500 fast rounds \
         (one parked watchdog per round would add 500)"
    );

    // The watchdog still works: a round that actually hangs still times
    // out (same mechanism as authoring_session_recovers_from_an_unblocked_
    // stdin_read_via_the_round_timeout, just asserted here too since this
    // test replaces the session's child and could in principle leave the
    // watchdog unable to find it).
    let (st, body) = q(port, &tok, "SELECT * FROM read_csv('/dev/' || 'stdin')");
    assert_eq!(st, 400, "{body}");
    assert!(
        body.contains("exceeded") && body.contains("stopped"),
        "{body}"
    );
    let (st, body) = q(port, &tok, "SELECT 1 AS ok");
    assert_eq!(st, 200, "{body}");
    assert!(body.contains("\"ok\":1"), "{body}");
}
