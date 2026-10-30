//! `::FIGURE` … `::ENDFIGURE` (#661): several charts composed into one
//! picture (ggplot-rs `PlotGrid`) in every static output — `dashboard --svg`,
//! the CLI's HTML (`dashboard file.sql out.html`) and the script-free page of
//! `serve --dashboards` — and its `--check` diagnostics. Needs the `duckdb`
//! CLI (skips without it locally).
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

/// A figure of three charts (two stacked, one beside them, a shared x for
/// the stacked pair, one legend, tags) with a hostile title, then a plain
/// chart after it.
const SCRIPT: &str = "\
CREATE OR REPLACE TEMP TABLE pts AS SELECT i, CASE WHEN i % 2 = 0 THEN 'reference' ELSE 'now' END AS period, (i % 50) / 5.0 AS x, 3 + 0.8 * ((i % 50) / 5.0) AS y FROM range(200) t(i);
SELECT '</figure><script>alert(1)</script> drift' ::FIGURE, '(hist / scatter) | rule' ::LAYOUT, '1,4' ::HEIGHTS, 'x' ::AXES_COLLECT, true ::COLLECT_LEGENDS, 'A' ::TAG_LEVELS;
-- @name hist
SELECT x ::HISTOGRAM, period ::CATEGORY FROM pts;
-- @name scatter
SELECT x ::XAXIS, y ::SCATTER, period ::CATEGORY FROM pts;
-- @name rule
SELECT x ::XAXIS, avg(y) AS y ::LINECHART FROM pts GROUP BY ALL ORDER BY 1;
SELECT 1 ::ENDFIGURE;
SELECT x ::XAXIS, y ::LINECHART, 'After' ::TITLE FROM (VALUES (1, 2), (2, 3)) v(x, y);
";

/// A scratch directory under `std::env::temp_dir()` (TMPDIR), removed on drop.
struct Scratch(PathBuf);
impl Scratch {
    fn new(tag: &str) -> Self {
        let d = std::env::temp_dir().join(format!("anofox_661_{tag}_{}", std::process::id()));
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

fn dashboard(args: &[&std::ffi::OsStr]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_dashboard"))
        .args(args)
        .output()
        .unwrap()
}

/// One figure card with its three tagged cells and the escaped title, then
/// the plain chart's own card.
fn check_page(html: &str) {
    assert!(!html.contains("<script>alert"), "{html}");
    assert_eq!(
        html.matches("<figure class=\"panel dp-figure\">").count(),
        1,
        "{html}"
    );
    assert_eq!(
        html.matches("<figure class=\"panel\">").count(),
        1,
        "{html}"
    );
    for tag in ["A", "B", "C"] {
        assert!(
            html.contains(&format!("data-panel=\"{tag}\"")),
            "{tag}: {html}"
        );
    }
    assert!(html.contains("&lt;/figure&gt;&lt;script&gt;alert(1)&lt;/script&gt; drift</text>"));
    assert!(html.contains(">After</text>"), "{html}");
}

#[test]
fn dashboard_cli_html_draws_one_figure() {
    if !have_duckdb("dashboard_cli_html_draws_one_figure") {
        return;
    }
    let s = Scratch::new("cli");
    let (sql, out) = (s.0.join("fig.sql"), s.0.join("fig.html"));
    std::fs::write(&sql, SCRIPT).unwrap();
    let run = dashboard(&[sql.as_os_str(), out.as_os_str()]);
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    check_page(&std::fs::read_to_string(&out).unwrap());
}

#[test]
fn dashboard_svg_draws_one_figure() {
    if !have_duckdb("dashboard_svg_draws_one_figure") {
        return;
    }
    let s = Scratch::new("svg");
    let (sql, out) = (s.0.join("fig.sql"), s.0.join("fig.svg"));
    std::fs::write(&sql, SCRIPT).unwrap();
    let run = dashboard(&["--svg".as_ref(), sql.as_os_str(), out.as_os_str()]);
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let svg = std::fs::read_to_string(&out).unwrap();
    common::assert_safe_svg(&svg, "figure --svg");
    for tag in ["A", "B", "C"] {
        assert_eq!(
            svg.matches(&format!("data-panel=\"{tag}\"")).count(),
            1,
            "{tag}"
        );
    }
    // One legend for the three cells (collected): each series once.
    assert_eq!(svg.matches(">reference</text>").count(), 1, "{svg}");
    // #695: each cell says which statement it is and its -- @name.
    for (i, name) in ["hist", "scatter", "rule"].iter().enumerate() {
        assert!(
            svg.contains(&format!("data-cell=\"{i}\" data-dp-name=\"{name}\"")),
            "{name}"
        );
    }
}

#[test]
fn check_passes_a_good_figure_and_flags_a_bad_one() {
    if !have_duckdb("check_passes_a_good_figure_and_flags_a_bad_one") {
        return;
    }
    let s = Scratch::new("check");
    let good = s.0.join("good.sql");
    std::fs::write(&good, SCRIPT).unwrap();
    let run = dashboard(&["--check".as_ref(), "--strict".as_ref(), good.as_os_str()]);
    assert!(
        run.status.success(),
        "{}{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    let bad = s.0.join("bad.sql");
    std::fs::write(
        &bad,
        "SELECT 'F' ::FIGURE, 'a | nope' ::LAYOUT, '1,2,3' ::WIDTHS;\n\
         -- @name a\n\
         SELECT i ::XAXIS, i AS y ::LINECHART FROM range(3) t(i);\n\
         SELECT i AS v ::PAGED FROM range(3) t(i);\n",
    )
    .unwrap();
    let run = dashboard(&["--check".as_ref(), "--json".as_ref(), bad.as_os_str()]);
    assert!(!run.status.success());
    let out = String::from_utf8_lossy(&run.stdout);
    for code in [
        "figure/unclosed",
        "figure/unknown-cell",
        "figure/unplaced-cell",
        "figure/bad-weights",
        "figure/not-a-chart",
    ] {
        assert!(out.contains(code), "{code}: {out}");
    }
}

#[cfg(feature = "serve")]
#[test]
fn serve_static_page_draws_one_figure() {
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::time::{Duration, Instant};

    if !have_duckdb("serve_static_page_draws_one_figure") {
        return;
    }
    let s = Scratch::new("serve");
    let dash = s.0.join("dash");
    std::fs::create_dir_all(&dash).unwrap();
    std::fs::write(dash.join("fig.sql"), SCRIPT).unwrap();
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
    // HTTP/1.0: no chunked transfer encoding to undo.
    let deadline = Instant::now() + Duration::from_secs(20);
    let resp = loop {
        let attempt = (|| -> std::io::Result<String> {
            let mut conn = TcpStream::connect(("127.0.0.1", port))?;
            conn.set_read_timeout(Some(Duration::from_secs(60)))?;
            write!(conn, "GET /d/fig HTTP/1.0\r\nHost: 127.0.0.1\r\n\r\n")?;
            let mut resp = String::new();
            conn.read_to_string(&mut resp)?;
            Ok(resp)
        })();
        match attempt {
            Ok(r) if !r.is_empty() => break r,
            _ if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(100)),
            other => panic!("serve on {port}: {other:?}"),
        }
    };
    assert!(
        resp.starts_with("HTTP/1.1 200") || resp.starts_with("HTTP/1.0 200"),
        "{resp}"
    );
    check_page(resp.split_once("\r\n\r\n").map_or("", |(_, b)| b));
}
