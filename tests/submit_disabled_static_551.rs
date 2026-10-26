//! #551: a `::SUBMIT` panel can never actually POST `/api/submit` on a page
//! with no script to send it — the static `dashboard` CLI's HTML export,
//! and the script-free page of the plain `serve` binary (`src/bin/serve.rs`,
//! which has no `/api/submit` route at all). Both now render a disabled
//! button naming why, instead of silently dropping the panel (the plan §6
//! "never a silent no-op" rule `web/app.js`'s `mkSubmit` already follows in
//! the browser). Needs the `duckdb` CLI (skips without it locally, fails on
//! CI — see `common::skip_without`).
mod common;

use std::path::PathBuf;
use std::process::Command;

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
        let d = std::env::temp_dir().join(format!("anofox_551_{tag}_{}", std::process::id()));
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

/// One chart panel, then a `::SUBMIT` panel whose label carries hostile
/// markup (never rendered as HTML) and whose `::SUBMIT_VARS` names an
/// input this script also declares.
const SCRIPT: &str = "\
SELECT q1 ::BUTTONS FROM (VALUES ('A'), ('B')) t(q1);
SELECT x::XAXIS, y::BARCHART, 'Weekly'::TITLE FROM (VALUES ('a', 1), ('b', 2)) v(x, y) ORDER BY x;
SELECT 'Check <b>my</b> answer' AS quiz_1 ::SUBMIT, 'q1' AS r ::SUBMIT_VARS, 'Saved!' AS d ::SUBMIT_DONE;
";

/// Shared assertions once the disabled card is in hand: a `<figure
/// class="panel submit-panel">` with a disabled, `aria-disabled` button
/// carrying the escaped label, `aria-describedby` pointing at a visible,
/// matching reason line — and never the raw `<b>` the label's own text
/// carried.
fn check_disabled_submit(html: &str, reason: &str) {
    assert!(
        html.contains("class=\"panel submit-panel\""),
        "no submit-panel card: {html}"
    );
    assert!(
        html.contains("class=\"submit-btn\" disabled aria-disabled=\"true\""),
        "button isn't disabled: {html}"
    );
    assert!(
        html.contains("Check &lt;b&gt;my&lt;/b&gt; answer"),
        "label missing/unescaped: {html}"
    );
    assert!(
        !html.contains("<b>my</b>"),
        "label rendered as HTML: {html}"
    );
    let rid_marker = "aria-describedby=\"";
    let i = html.find(rid_marker).expect("aria-describedby") + rid_marker.len();
    let rid = &html[i..i + html[i..].find('"').unwrap()];
    let anchor = format!("id=\"{rid}\"");
    assert!(html.contains(&anchor), "no element with id={rid}: {html}");
    let j = html.find(&anchor).unwrap();
    assert!(
        html[j..].contains(reason),
        "reason {reason:?} not next to id={rid}: {html}"
    );
}

#[test]
fn dashboard_cli_html_shows_a_disabled_submit_button() {
    if !have_duckdb("dashboard_cli_html_shows_a_disabled_submit_button") {
        return;
    }
    let s = Scratch::new("cli");
    let (sql, out) = (s.0.join("quiz.sql"), s.0.join("quiz.html"));
    std::fs::write(&sql, SCRIPT).unwrap();
    let run = Command::new(env!("CARGO_BIN_EXE_dashboard"))
        .arg(&sql)
        .arg(&out)
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let html = std::fs::read_to_string(&out).unwrap();
    check_disabled_submit(&html, "Submitting needs the dashboard server");
}

/// `GET /d/<id>` on a plain `serve --dashboards` instance over `script`, as
/// a raw HTTP/1.1 response (status line + headers + body).
#[cfg(feature = "serve")]
fn serve_get(tag: &str, id: &str, script: &str) -> String {
    use std::net::TcpListener;
    let s = Scratch::new(tag);
    let dash = s.0.join("dash");
    std::fs::create_dir_all(&dash).unwrap();
    std::fs::write(dash.join(format!("{id}.sql")), script).unwrap();
    let init = s.0.join("init.sql");
    std::fs::write(&init, "CREATE TABLE IF NOT EXISTS t AS SELECT 1 AS one;\n").unwrap();
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    struct Kill(std::process::Child);
    impl Drop for Kill {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
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
    http_get(port, &format!("/d/{id}"))
}

/// GET `path` from the test server on `port`, retried while the server is
/// starting (a refused/reset connection or an empty answer) for up to 20s.
#[cfg(feature = "serve")]
fn http_get(port: u16, path: &str) -> String {
    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::time::{Duration, Instant};
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

/// Dechunk a raw HTTP/1.1 response body (`serve` always chunks a response
/// it doesn't know the length of up front, this one included) — reading
/// the socket straight into a string leaves hex chunk-size markers
/// embedded and corrupts the body wherever a chunk boundary lands inside
/// the HTML.
#[cfg(feature = "serve")]
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

#[cfg(feature = "serve")]
#[test]
fn serve_static_page_shows_a_disabled_submit_button() {
    if !have_duckdb("serve_static_page_shows_a_disabled_submit_button") {
        return;
    }
    let resp = serve_get("serve", "quiz", SCRIPT);
    assert!(resp.starts_with("HTTP/1.1 200"), "{resp}");
    let html = http_body(&resp);
    check_disabled_submit(&html, "Submitting is not enabled here");
}

/// #274-style gating: a `::SUBMIT` behind an unmet `-- @requires` is left
/// out entirely on the script-free page (as a gated `::INPUT` already is),
/// not shown disabled — the missing path never even reaches a query.
#[cfg(feature = "serve")]
#[test]
fn serve_static_page_leaves_out_a_gated_submit() {
    if !have_duckdb("serve_static_page_leaves_out_a_gated_submit") {
        return;
    }
    let script = "\
SELECT x::XAXIS, y::BARCHART, 'Always'::TITLE FROM (VALUES ('a', 1), ('b', 2)) v(x, y) ORDER BY x;
-- @requires nowhere/gate.parquet
SELECT 'Check my answer' AS quiz_1 ::SUBMIT, 'q1' AS r ::SUBMIT_VARS FROM read_parquet('nowhere/gate.parquet');
";
    let resp = serve_get("serve-gated", "gated", script);
    assert!(resp.starts_with("HTTP/1.1 200"), "{resp}");
    let html = http_body(&resp);
    // Not the embedded stylesheet's own `.submit-panel{...}` rule (always
    // present) — the actual card, which a gated panel must never get.
    assert!(
        !html.contains("<figure class=\"panel submit-panel\">"),
        "{html}"
    );
    assert!(!html.contains("Check my answer"), "{html}");
    assert!(!html.contains("nowhere/"), "{html}");
}
