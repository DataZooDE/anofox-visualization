//! #435: `dashboard file.sql out.html`, `dashboard --svg` and `dashboard
//! --check` run a whole script in ONE `duckdb` process instead of replaying
//! every earlier setup statement before each later one, in a fresh process
//! (#414's fix for the session semantics, which cost O(panels × setup)).
//!
//! This measures the fix directly: a 20-panel dashboard whose setup has a
//! deliberately slow statement must run that statement once — with the old
//! replay-per-statement approach, the total time would scale with the panel
//! count (≈ 20 × the slow statement's own time); with one session, it does
//! not. A generous threshold (`BUDGET`) keeps this robust on a slow CI
//! runner while still clearly separating "ran once" from "ran ~20 times".
//! Needs the `duckdb` CLI (skips without it locally, fails on CI — see
//! `common::skip_without`).
mod common;

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

struct Scratch(PathBuf);
impl Scratch {
    fn new(tag: &str) -> Self {
        let d = std::env::temp_dir().join(format!("anofox_435_{tag}_{}", std::process::id()));
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

/// A `CREATE TEMP TABLE` setup statement that reliably takes a few hundred
/// ms (measured empirically on this crate's dev box: ~0.6–0.8s), with no
/// file I/O and no dependency on the surrounding environment — a plain
/// cross join aggregate. If the session replayed this before every one of
/// the 20 panels below (the old behaviour), the whole render would take on
/// the order of 20 × this statement's own time.
const SLOW_SETUP: &str =
    "CREATE TEMP TABLE slow_setup AS SELECT sum(a.i * b.i) AS n FROM range(15000) a(i), range(15000) b(i);";

/// A generous ceiling: comfortably above one run of `SLOW_SETUP` plus 20
/// trivial round trips (observed total: well under 1.5s), but far below
/// what 20 replays of it would take (~14s+) — any regression back to
/// per-panel replay fails this loudly rather than just "being slower".
const BUDGET: Duration = Duration::from_secs(5);

/// `N` panels, each a trivial KPI reading `slow_setup` — cheap on their own,
/// so the render's total time is dominated by how many times `SLOW_SETUP`
/// itself actually ran.
fn script_with_n_panels(n: usize) -> String {
    let mut s = String::from(SLOW_SETUP);
    s.push('\n');
    for i in 0..n {
        s.push_str(&format!(
            "SELECT (n + {i})::METRIC, 'panel {i}'::TITLE FROM slow_setup;\n"
        ));
    }
    s
}

#[test]
fn render_html_runs_slow_setup_once_for_20_panels() {
    if !have_duckdb("render_html_runs_slow_setup_once_for_20_panels") {
        return;
    }
    let s = Scratch::new("render-once");
    let (sql, out) = (s.0.join("w.sql"), s.0.join("w.html"));
    std::fs::write(&sql, script_with_n_panels(20)).unwrap();
    let start = Instant::now();
    let run = Command::new(env!("CARGO_BIN_EXE_dashboard"))
        .arg(&sql)
        .arg(&out)
        .output()
        .unwrap();
    let elapsed = start.elapsed();
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
    assert!(html.contains("20 panels"), "{html}");
    assert!(
        elapsed < BUDGET,
        "render took {elapsed:?} (budget {BUDGET:?}) — the slow setup statement looks like it \
         ran more than once (20 panels × its own time would take far longer than this budget)"
    );
}

#[test]
fn svg_runs_slow_setup_once_for_20_panels() {
    if !have_duckdb("svg_runs_slow_setup_once_for_20_panels") {
        return;
    }
    let s = Scratch::new("svg-once");
    let (sql, out) = (s.0.join("w.sql"), s.0.join("w.svg"));
    std::fs::write(&sql, script_with_n_panels(20)).unwrap();
    let start = Instant::now();
    let run = Command::new(env!("CARGO_BIN_EXE_dashboard"))
        .arg("--svg")
        .arg(&sql)
        .arg(&out)
        .output()
        .unwrap();
    let elapsed = start.elapsed();
    assert!(
        run.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    let svg = std::fs::read_to_string(&out).unwrap();
    assert!(svg.starts_with("<svg"), "{svg}");
    assert!(
        elapsed < BUDGET,
        "render took {elapsed:?} (budget {BUDGET:?}) — the slow setup statement looks like it \
         ran more than once"
    );
}

/// `dashboard --check` replays the identical growing prelude `check_opts`
/// built internally before #435 — the same cost shape as the renderers, now
/// fixed the same way (one session per file, via dashboard.rs's own
/// `SessionRunner`-backed closure; `lint::check_opts` itself only ever
/// expected "the next statement, in session" from its `run_query`, not a
/// whole-script replay).
#[test]
fn check_runs_slow_setup_once_for_20_panels() {
    if !have_duckdb("check_runs_slow_setup_once_for_20_panels") {
        return;
    }
    let s = Scratch::new("check-once");
    let sql = s.0.join("w.sql");
    std::fs::write(&sql, script_with_n_panels(20)).unwrap();
    let start = Instant::now();
    let run = Command::new(env!("CARGO_BIN_EXE_dashboard"))
        .arg("--check")
        .arg(&sql)
        .output()
        .unwrap();
    let elapsed = start.elapsed();
    assert!(
        run.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert!(
        elapsed < BUDGET,
        "check took {elapsed:?} (budget {BUDGET:?}) — the slow setup statement looks like it \
         ran more than once"
    );
}

/// The same budget check against a prebuilt `--db` (read-only), the other
/// session shape `--check` supports — a `CREATE TEMP TABLE` setup statement
/// still runs (TEMP objects are never part of the read-only, on-disk
/// catalog `-readonly` protects).
#[test]
fn check_with_db_runs_slow_setup_once_for_20_panels() {
    if !have_duckdb("check_with_db_runs_slow_setup_once_for_20_panels") {
        return;
    }
    let s = Scratch::new("check-db-once");
    let db = s.0.join("w.duckdb");
    let out = Command::new("duckdb")
        .arg(&db)
        .arg("-c")
        .arg("CREATE TABLE t AS SELECT 1 AS x;")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let sql = s.0.join("w.sql");
    std::fs::write(&sql, script_with_n_panels(20)).unwrap();
    let start = Instant::now();
    let run = Command::new(env!("CARGO_BIN_EXE_dashboard"))
        .arg("--check")
        .arg(&sql)
        .arg("--db")
        .arg(&db)
        .output()
        .unwrap();
    let elapsed = start.elapsed();
    assert!(
        run.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert!(
        elapsed < BUDGET,
        "check --db took {elapsed:?} (budget {BUDGET:?}) — the slow setup statement looks like \
         it ran more than once"
    );
}
