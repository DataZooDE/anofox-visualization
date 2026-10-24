//! #414: `dashboard file.sql out.html` (the static render) and `dashboard
//! --svg` must keep one DuckDB session's semantics across statements — a
//! `CREATE OR REPLACE TEMP VIEW` or a `SET VARIABLE` earlier in the script
//! stays in scope for every later panel, exactly as `dashboard --check` and
//! the locked server's cold path do — and must accept `--data <dir>` the
//! same way `--check` does. Needs the `duckdb` CLI (skips without it
//! locally).
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
        let d = std::env::temp_dir().join(format!("anofox_414_{tag}_{}", std::process::id()));
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

fn run_dashboard(args: &[&std::ffi::OsStr]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_dashboard"))
        .args(args)
        .output()
        .unwrap()
}

/// The issue's first repro: a `CREATE OR REPLACE TEMP VIEW` earlier in the
/// script must still be visible to a panel that queries it.
#[test]
fn render_html_keeps_a_temp_view_across_statements() {
    if !have_duckdb("render_html_keeps_a_temp_view_across_statements") {
        return;
    }
    let s = Scratch::new("temp-view");
    let (sql, out) = (s.0.join("w.sql"), s.0.join("w.html"));
    std::fs::write(
        &sql,
        "CREATE OR REPLACE TEMP VIEW w AS SELECT 1 AS a;\n\
         SELECT max(a)::METRIC, 'x'::TITLE FROM w;\n",
    )
    .unwrap();
    let run = run_dashboard(&[sql.as_os_str(), out.as_os_str()]);
    assert!(
        run.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    let html = std::fs::read_to_string(&out).unwrap();
    assert!(
        !html.contains("does not exist") && !html.contains("Catalog Error"),
        "{html}"
    );
    assert!(html.contains("1 panels"), "{html}");
}

/// The issue's second repro: a `SET VARIABLE x = (SELECT … FROM w)` (reading
/// a temp view from an earlier statement) must also see it.
#[test]
fn render_html_keeps_a_set_variable_reading_a_temp_view() {
    if !have_duckdb("render_html_keeps_a_set_variable_reading_a_temp_view") {
        return;
    }
    let s = Scratch::new("set-var-from-view");
    let (sql, out) = (s.0.join("w.sql"), s.0.join("w.html"));
    std::fs::write(
        &sql,
        "CREATE OR REPLACE TEMP VIEW w AS SELECT 1 AS a;\n\
         SET VARIABLE x = (SELECT max(a) FROM w);\n\
         SELECT getvariable('x')::METRIC, 'x'::TITLE;\n",
    )
    .unwrap();
    let run = run_dashboard(&[sql.as_os_str(), out.as_os_str()]);
    assert!(
        run.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    let html = std::fs::read_to_string(&out).unwrap();
    assert!(
        !html.contains("does not exist") && !html.contains("Catalog Error"),
        "{html}"
    );
    assert!(html.contains("1 panels"), "{html}");
}

/// Same repro, for `dashboard --svg`.
#[test]
fn render_svg_keeps_a_temp_view_across_statements() {
    if !have_duckdb("render_svg_keeps_a_temp_view_across_statements") {
        return;
    }
    let s = Scratch::new("temp-view-svg");
    let (sql, out) = (s.0.join("w.sql"), s.0.join("w.svg"));
    std::fs::write(
        &sql,
        "CREATE OR REPLACE TEMP VIEW w AS SELECT 1 AS a;\n\
         SELECT max(a)::METRIC, 'x'::TITLE FROM w;\n",
    )
    .unwrap();
    let run = run_dashboard(&[
        std::ffi::OsStr::new("--svg"),
        sql.as_os_str(),
        out.as_os_str(),
    ]);
    assert!(
        run.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    let svg = std::fs::read_to_string(&out).unwrap();
    assert!(svg.starts_with("<svg"), "{svg}");
}

/// `--data <dir>`: `getvariable('anofox_data')` resolves to the directory,
/// the same way `--check --data` and a locked server's `options.data` set
/// it — before the dashboard's own statements run.
#[test]
fn render_html_accepts_data_dir() {
    if !have_duckdb("render_html_accepts_data_dir") {
        return;
    }
    let s = Scratch::new("data-dir");
    let data_dir = s.0.join("data");
    std::fs::create_dir_all(&data_dir).unwrap();
    Command::new("duckdb")
        .arg(":memory:")
        .arg("-c")
        .arg(format!(
            "COPY (SELECT 7 AS n) TO '{}' (FORMAT PARQUET)",
            data_dir.join("x.parquet").display()
        ))
        .output()
        .unwrap();
    let (sql, out) = (s.0.join("d.sql"), s.0.join("d.html"));
    std::fs::write(
        &sql,
        "SELECT n::METRIC, 'n'::TITLE FROM read_parquet(getvariable('anofox_data') || '/x.parquet');\n",
    )
    .unwrap();
    let run = run_dashboard(&[
        sql.as_os_str(),
        out.as_os_str(),
        std::ffi::OsStr::new("--data"),
        data_dir.as_os_str(),
    ]);
    assert!(
        run.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    let html = std::fs::read_to_string(&out).unwrap();
    assert!(
        !html.contains("does not exist") && !html.contains("Catalog Error"),
        "{html}"
    );
    assert!(html.contains("1 panels"), "{html}");
}

/// An input's bound default (as the browser's input pre-pass picks on first
/// load) must feed a later `SET VARIABLE`/`getvariable()` panel, the same
/// as `--check` (#414 part 1): the input appears before the `SET VARIABLE`
/// in script order here, as the locked server's cold path requires (moving
/// it below would always hide a tab that reads it the same way).
#[test]
fn render_html_input_default_feeds_a_set_variable() {
    if !have_duckdb("render_html_input_default_feeds_a_set_variable") {
        return;
    }
    let s = Scratch::new("input-default");
    let (sql, out) = (s.0.join("i.sql"), s.0.join("i.html"));
    std::fs::write(
        &sql,
        "SELECT DISTINCT region::DROPDOWN FROM (VALUES ('east'), ('west')) v(region) ORDER BY region;\n\
         SET VARIABLE doubled = (SELECT length(getvariable('region')));\n\
         SELECT getvariable('doubled')::METRIC, 'len'::TITLE;\n",
    )
    .unwrap();
    let run = run_dashboard(&[sql.as_os_str(), out.as_os_str()]);
    assert!(
        run.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    let html = std::fs::read_to_string(&out).unwrap();
    assert!(
        !html.contains("does not exist") && !html.contains("Catalog Error"),
        "{html}"
    );
    // 'east' is the input's first (default) option, length 4.
    assert!(html.contains("1 panels"), "{html}");
}
