#![cfg(feature = "serve")]
//! The script-free page's panel errors (#279): a DuckDB error is never shown
//! to the viewer (it can quote file paths, SQL and values) — the viewer gets
//! a generic 400, and the server logs the detail to stderr.
mod common;

use anofox_visualization::host::auth::jwt::now_s;
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use serde_json::json;
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

/// GET `path` from the test server on `port` (#358, as #305 did for the same
/// flake in `static_title_pages.rs`): connect, send and read the whole
/// exchange, retried from scratch while the server is starting up or has
/// already closed — a refused or reset connection, or an empty answer — for
/// up to 20 s. A single `TcpStream::connect` that then succeeds once is not
/// enough: the OS can complete the handshake before the server's accept loop
/// is actually ready to read a request, and a connection made just as the
/// test's `Kill` tears the server down can read back nothing or a reset.
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
    let resp = http_get(port, "/d/broken");
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

/// #285: a panel's own renderer (not its SQL query) fails, quoting a role
/// option's value back verbatim (`::ABLINE '…' must be 'slope,intercept'`,
/// src/lib.rs). That value can carry data, so it must never reach the page —
/// only a generic `<pre>` and the server log, same redaction as #279 above.
#[test]
fn panel_render_error_is_redacted() {
    if !have_duckdb("panel_render_error_is_redacted") {
        return;
    }
    let s = Scratch::new("panel");
    let dash = s.0.join("dash");
    std::fs::create_dir_all(&dash).unwrap();
    std::fs::write(
        dash.join("sentinel.sql"),
        "SELECT x::XAXIS, y::LINECHART, 'hunter2_panel_sentinel_285'::ABLINE FROM t;\n\
         SELECT x::XAXIS, y::BARCHART FROM t;\n",
    )
    .unwrap();
    let init = s.0.join("init.sql");
    std::fs::write(
        &init,
        "CREATE TABLE IF NOT EXISTS t AS SELECT 1 AS x, 2 AS y;\n",
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
    let resp = http_get(port, "/d/sentinel");
    assert!(resp.starts_with("HTTP/1.1 200"), "{resp}");
    let body = http_body(&resp);
    assert!(!body.contains("hunter2_panel_sentinel_285"), "{body}");
    assert!(body.contains("this panel could not be drawn"), "{body}");

    let out = server.finish();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("[anofox-serve] dashboard 'sentinel' panel render failed:"),
        "{stderr}"
    );
    assert!(stderr.contains("hunter2_panel_sentinel_285"), "{stderr}");
}

/// #315 (sec-review-314 I1): the contract arm (`anofox_plot_*` macro panels)
/// is the chart arm's twin, redacted by the same three-line change, but only
/// the chart arm (above) had a test. A `tests` contract call whose row is
/// missing every column the schema needs fails with `RenderError::BadSpec`
/// naming the columns it *does* have — so a column named after a sentinel
/// reaches that error text exactly the way a data value reaches `::ABLINE`'s,
/// and must be redacted the same way.
#[test]
fn contract_panel_render_error_is_redacted() {
    if !have_duckdb("contract_panel_render_error_is_redacted") {
        return;
    }
    let s = Scratch::new("contract");
    let dash = s.0.join("dash");
    std::fs::create_dir_all(&dash).unwrap();
    std::fs::write(
        dash.join("sentinel.sql"),
        "SELECT anofox_plot_tests(t) FROM (SELECT 1 AS hunter2_contract_sentinel_315) t;\n\
         SELECT x::XAXIS, y::BARCHART FROM t;\n",
    )
    .unwrap();
    let init = s.0.join("init.sql");
    std::fs::write(
        &init,
        "CREATE TABLE IF NOT EXISTS t AS SELECT 1 AS x, 2 AS y;\n",
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
    let resp = http_get(port, "/d/sentinel");
    assert!(resp.starts_with("HTTP/1.1 200"), "{resp}");
    let body = http_body(&resp);
    assert!(!body.contains("hunter2_contract_sentinel_315"), "{body}");
    assert!(body.contains("this panel could not be drawn"), "{body}");

    let out = server.finish();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("[anofox-serve] dashboard 'sentinel' panel render failed:"),
        "{stderr}"
    );
    assert!(stderr.contains("hunter2_contract_sentinel_315"), "{stderr}");
}

/// #315 (sec-review-314 I2): `data-warnings` on a *successful* contract
/// render can name the panel's own data (`anofox_plot_compare`'s "not in
/// `<factor>`" warning) — the one place diagnostic text that isn't drawn
/// still reached the script-free page. No script there reads it, so it's
/// now stripped.
#[test]
fn contract_panel_data_warnings_are_stripped() {
    if !have_duckdb("contract_panel_data_warnings_are_stripped") {
        return;
    }
    let s = Scratch::new("warn");
    let dash = s.0.join("dash");
    std::fs::create_dir_all(&dash).unwrap();
    std::fs::write(
        dash.join("cmp.sql"),
        "SELECT anofox_plot_compare(r, value := 'y', factor := 'g', \
         tests := [{'group1': 'a', 'group2': 'hunter2_warn_sentinel_315', 'p_adj': 0.01}]) \
         FROM (VALUES ('a', 1.0), ('a', 1.4), ('b', 2.1), ('b', 2.6)) r(g, y);\n",
    )
    .unwrap();
    // `--db` is opened `-readonly`, which requires the file to already
    // exist; the dashboard above needs no table of its own, so `--init`
    // just creates the (otherwise unused) database file.
    let init = s.0.join("init.sql");
    std::fs::write(&init, "CREATE TABLE IF NOT EXISTS t AS SELECT 1 AS x;\n").unwrap();
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
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap(),
    ));
    let resp = http_get(port, "/d/cmp");
    assert!(resp.starts_with("HTTP/1.1 200"), "{resp}");
    let body = http_body(&resp);
    assert!(body.contains("<svg"), "{body}");
    assert!(!body.contains("data-warnings"), "{body}");
    assert!(!body.contains("hunter2_warn_sentinel_315"), "{body}");
    drop(server);
}

/// #515: a `::SHOW_IF` condition's own query failing is one of its fail-
/// closed cases (FALSE / NULL / no row / non-boolean / an error all hide the
/// target) — unlike an ordinary panel's query failure, it must never abort
/// the whole script-free page. The raw DuckDB error can echo a value from
/// the failing expression (here, a literal that fails a cast — the same
/// shape a viewer-tainted `getvariable()` comparison would take), so it must
/// not reach the viewer OR the server log: only a generic line does.
#[test]
fn show_if_condition_error_hides_the_panel_not_the_page() {
    if !have_duckdb("show_if_condition_error_hides_the_panel_not_the_page") {
        return;
    }
    let s = Scratch::new("showif");
    let dash = s.0.join("dash");
    std::fs::create_dir_all(&dash).unwrap();
    std::fs::write(
        dash.join("sentinel.sql"),
        "SELECT CAST('hunter2_showif_sentinel_515' AS INTEGER) ::SHOW_IF;\n\
         SELECT count(*) AS n ::METRIC, 'Hidden metric'::LABEL FROM t;\n\
         SELECT count(*) AS n ::METRIC, 'Still shown metric'::LABEL FROM t;\n",
    )
    .unwrap();
    let init = s.0.join("init.sql");
    std::fs::write(&init, "CREATE TABLE IF NOT EXISTS t AS SELECT 1 AS x;\n").unwrap();
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
    let resp = http_get(port, "/d/sentinel");
    assert!(resp.starts_with("HTTP/1.1 200"), "{resp}");
    let body = http_body(&resp);
    assert!(!body.contains("hunter2_showif_sentinel_515"), "{body}");
    assert!(!body.contains("Hidden metric"), "{body}");
    assert!(body.contains("Still shown metric"), "{body}");

    let out = server.finish();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("[anofox-serve] dashboard 'sentinel' ::SHOW_IF condition"),
        "{stderr}"
    );
    assert!(
        !stderr.contains("hunter2_showif_sentinel_515"),
        "the raw DuckDB error (which can echo a viewer-tainted value) must not reach the \
         log either: {stderr}"
    );
}

/// #540: an embed link locks `region`, but the dashboard's own `::DROPDOWN`
/// for `region` defaults to a different value. `render_dashboard_page`
/// `SET VARIABLE`s the locked value (from the link's `Scope`) right after
/// the resolved `@param`s, then — while querying each panel — rebinds an
/// `::INPUT`'s default for any name not already in `resolved`. `resolved`
/// never carries a locked name (it's stripped at the call site), so the
/// `::DROPDOWN`'s own default for `region` is appended *after* the locked
/// `SET VARIABLE`, winning for every panel that runs later in the same
/// render — an embed viewer sees the author's default, not the value the
/// link fixed.
#[test]
fn embed_locked_var_beats_an_input_default() {
    if !have_duckdb("embed_locked_var_beats_an_input_default") {
        return;
    }
    let s = Scratch::new("embedlock");
    let dash = s.0.join("dash");
    std::fs::create_dir_all(&dash).unwrap();
    // The ::DROPDOWN's first (alphabetical) option is 'EU' — its default.
    // The embed link below locks `region` to 'US'.
    std::fs::write(
        dash.join("sentinel.sql"),
        "SELECT region AS region ::DROPDOWN FROM (VALUES ('EU'), ('US')) v(region) \
         ORDER BY region;\n\
         SELECT 'region=' || getvariable('region')::VARCHAR ::SECTION;\n",
    )
    .unwrap();
    let init = s.0.join("init.sql");
    std::fs::write(&init, "CREATE TABLE IF NOT EXISTS t AS SELECT 1 AS x;\n").unwrap();

    const SECRET: &str = "embed-secret-540-0123456789abcdef0123456789";
    let auth = s.0.join("auth.json");
    std::fs::write(
        &auth,
        json!({"mode": "embed", "secret": SECRET}).to_string(),
    )
    .unwrap();
    let token = jsonwebtoken::encode(
        &Header::new(Algorithm::HS256),
        &json!({"dashboard": "sentinel", "vars": {"region": "US"}, "exp": now_s() + 120}),
        &EncodingKey::from_secret(SECRET.as_bytes()),
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
            .arg("--db")
            .arg(s.0.join("t.duckdb"))
            .arg("--init")
            .arg(&init)
            .arg("--auth")
            .arg(format!("@{}", auth.display()))
            .args(["--port", &port.to_string(), "--no-open"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap(),
    ));
    let resp = http_get(port, &format!("/d/sentinel?embed={token}"));
    assert!(resp.starts_with("HTTP/1.1 200"), "{resp}");
    let body = http_body(&resp);
    assert!(
        body.contains("region=US"),
        "the embed link locked region to 'US', but the page shows:\n{body}"
    );
    assert!(
        !body.contains("region=EU"),
        "the ::DROPDOWN's own default ('EU') overrode the embed link's locked \
         value — #540:\n{body}"
    );
    drop(server);
}
