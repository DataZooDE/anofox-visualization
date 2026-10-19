//! The static pages draw a panel's `::TITLE` bar (#232): the dashboard CLI's
//! HTML (`dashboard file.sql out.html`) and the script-free page of
//! `serve --dashboards`. Needs the `duckdb` CLI (skips without it locally).
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

/// Four panels: a titled chart, a first-NULL title, a KPI tile with a title,
/// and a hostile title.
const SCRIPT: &str = "\
SELECT x::XAXIS, y::BARCHART, 'Bins used'::TITLE FROM (VALUES ('a', 1), ('b', 2)) v(x, y) ORDER BY x;
SELECT x::XAXIS, y::BARCHART, t::TITLE FROM (VALUES ('a', 1, NULL), ('b', 2, 'Second')) v(x, y, t) ORDER BY x;
SELECT 42 AS n ::METRIC, 'Orders'::TITLE;
SELECT x::XAXIS, y::BARCHART, '</figure><script>alert(1)</script> & \"q\"'::TITLE FROM (VALUES ('a', 1), ('b', 2)) v(x, y) ORDER BY x;
";

const BAR_OPEN: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="460" height="26" viewBox="0 0 460 26"><text x="14" y="20" "#;

/// A scratch directory under `std::env::temp_dir()` (TMPDIR), removed on drop.
struct Scratch(PathBuf);
impl Scratch {
    fn new(tag: &str) -> Self {
        let d = std::env::temp_dir().join(format!("anofox_232_{tag}_{}", std::process::id()));
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
/// dechunking the body first when `Transfer-Encoding: chunked` is present
/// (tiny_http, which `serve` uses, chunks any response it doesn't know the
/// length of up front — including this one). Reading the raw socket bytes
/// straight into a string, as the naive version of this test did, leaves the
/// hex chunk-size markers embedded in the body and corrupts it wherever a
/// chunk boundary lands inside the HTML.
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

/// The page's `<figure class="panel">` contents, in order.
fn figures(html: &str) -> Vec<&str> {
    html.split("<figure class=\"panel\">")
        .skip(1)
        .map(|f| &f[..f.find("</figure>").expect("closed figure")])
        .collect()
}

fn check_page(html: &str) {
    // The hostile title closes nothing and opens no script.
    assert!(!html.contains("<script>alert"), "{html}");
    assert_eq!(html.matches("</figure>").count(), 4, "{html}");
    let f = figures(html);
    assert_eq!(f.len(), 4, "{html}");
    assert!(f[0].starts_with(BAR_OPEN), "{}", f[0]);
    assert!(f[0].contains(">Bins used</text></svg><svg"), "{}", f[0]);
    assert!(f[0].contains(" height=\"274\""), "{}", f[0]);
    assert!(f[1].starts_with(BAR_OPEN), "{}", f[1]);
    assert!(f[1].contains(">Second</text></svg><svg"), "{}", f[1]);
    // A KPI tile: no title bar (its caption is ::LABEL).
    assert!(
        !f[2].contains("height=\"26\" viewBox=\"0 0 460 26\""),
        "{}",
        f[2]
    );
    assert!(f[3].starts_with(BAR_OPEN), "{}", f[3]);
    assert!(
        f[3].contains(">&lt;/figure&gt;&lt;script&gt;alert(1)&lt;/script&gt; &amp; &quot;q&quot;</text></svg>"),
        "{}",
        f[3]
    );
}

#[test]
fn dashboard_cli_html_draws_title_bars() {
    if !have_duckdb("dashboard_cli_html_draws_title_bars") {
        return;
    }
    let s = Scratch::new("cli");
    let (sql, out) = (s.0.join("titles.sql"), s.0.join("titles.html"));
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
    check_page(&std::fs::read_to_string(&out).unwrap());
}

#[cfg(feature = "serve")]
#[test]
fn serve_static_page_draws_title_bars() {
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::time::{Duration, Instant};

    if !have_duckdb("serve_static_page_draws_title_bars") {
        return;
    }
    let s = Scratch::new("serve");
    let dash = s.0.join("dash");
    std::fs::create_dir_all(&dash).unwrap();
    std::fs::write(dash.join("titles.sql"), SCRIPT).unwrap();
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
        "GET /d/titles HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut resp = String::new();
    conn.read_to_string(&mut resp).unwrap();
    assert!(resp.starts_with("HTTP/1.1 200"), "{resp}");
    check_page(&http_body(&resp));
}

/// #214: a `::VIEW` group's two alternatives, each with its own title —
/// only the default (first) one should ever reach the static page.
const VIEW_SCRIPT: &str = "\
SELECT 'Lines' AS yoy ::VIEW;
SELECT x::XAXIS, y::BARCHART, 'Weekly'::TITLE FROM (VALUES ('a', 1), ('b', 2)) v(x, y) ORDER BY x;
SELECT 'Bars' AS yoy2 ::VIEW;
SELECT x::XAXIS, y::LINECHART, 'Other'::TITLE FROM (VALUES ('a', 1), ('b', 2)) v(x, y) ORDER BY x;
";

fn check_view_page(html: &str) {
    assert_eq!(html.matches("</figure>").count(), 1, "{html}");
    let f = figures(html);
    assert!(f[0].contains(">Weekly</text></svg><svg"), "{}", f[0]);
    assert!(!html.contains("Other"), "{html}");
}

#[test]
fn dashboard_cli_html_draws_only_the_default_view() {
    if !have_duckdb("dashboard_cli_html_draws_only_the_default_view") {
        return;
    }
    let s = Scratch::new("cli-view");
    let (sql, out) = (s.0.join("views.sql"), s.0.join("views.html"));
    std::fs::write(&sql, VIEW_SCRIPT).unwrap();
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
    check_view_page(&std::fs::read_to_string(&out).unwrap());
}

#[cfg(feature = "serve")]
#[test]
fn serve_static_page_draws_only_the_default_view() {
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::time::{Duration, Instant};

    if !have_duckdb("serve_static_page_draws_only_the_default_view") {
        return;
    }
    let s = Scratch::new("serve-view");
    let dash = s.0.join("dash");
    std::fs::create_dir_all(&dash).unwrap();
    std::fs::write(dash.join("views.sql"), VIEW_SCRIPT).unwrap();
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
        "GET /d/views HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut resp = String::new();
    conn.read_to_string(&mut resp).unwrap();
    assert!(resp.starts_with("HTTP/1.1 200"), "{resp}");
    check_view_page(&http_body(&resp));
}
