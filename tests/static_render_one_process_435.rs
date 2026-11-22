//! #435: `dashboard file.sql out.html`, `dashboard --svg` and `dashboard
//! --check` run a whole script in ONE `duckdb` process instead of replaying
//! every earlier setup statement before each later one, in a fresh process
//! (#414's fix for the session semantics, which cost O(panels × setup)).
//!
//! This measures the fix directly: a 20-panel dashboard whose setup has a
//! deliberately slow statement must run that statement once — with the old
//! replay-per-statement approach, the total time would scale with the panel
//! count (≈ 20 × the slow statement's own time); with one session, it does
//! not. The threshold is relative: the same command on a 1-panel script,
//! timed on the same machine just before, sets it (`within_budget`), so a
//! slow or busy CI runner slows both runs alike (#1342: an absolute 5 s
//! budget failed at 5.3 s on one), while "ran ~20 times" still fails loudly.
//! Needs the `duckdb` CLI (skips without it locally, fails on CI — see
//! `common::skip_without`).
mod common;

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

/// Runs `make(script)` on an `n`-panel script and returns how long it took
/// (and its stdout); `make` builds the command, writing any output next to
/// the script.
fn timed(tag: &str, n: usize, make: &dyn Fn(&Path) -> Command) -> (Duration, Vec<u8>) {
    let s = Scratch::new(&format!("{tag}-{n}"));
    let sql = s.0.join("w.sql");
    std::fs::write(&sql, script_with_n_panels(n)).unwrap();
    let start = Instant::now();
    let run = make(&sql).output().unwrap();
    let elapsed = start.elapsed();
    assert!(
        run.status.success(),
        "{tag} ({n} panels) stderr: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    let out = sql.with_extension("out");
    let body = std::fs::read(&out).unwrap_or(run.stdout);
    (elapsed, body)
}

/// 20 panels must cost far less than 20 × one panel: at most 4 × the
/// 1-panel run plus 1 s of fixed overhead. Running the slow setup once per
/// panel would take ~20 × as long, on any machine.
fn within_budget(tag: &str, make: &dyn Fn(&Path) -> Command) -> Vec<u8> {
    let (one, _) = timed(tag, 1, make);
    let (twenty, body) = timed(tag, 20, make);
    let budget = one * 4 + Duration::from_secs(1);
    assert!(
        twenty < budget,
        "{tag}: 20 panels took {twenty:?}, 1 panel {one:?} (budget {budget:?}) — the slow setup \
         statement looks like it ran more than once"
    );
    body
}

fn dashboard() -> Command {
    Command::new(env!("CARGO_BIN_EXE_dashboard"))
}

#[test]
fn render_html_runs_slow_setup_once_for_20_panels() {
    if !have_duckdb("render_html_runs_slow_setup_once_for_20_panels") {
        return;
    }
    let html = within_budget("render-once", &|sql| {
        let mut c = dashboard();
        c.arg(sql).arg(sql.with_extension("out"));
        c
    });
    let html = String::from_utf8_lossy(&html);
    assert!(
        !html.contains("does not exist") && !html.contains("Catalog Error"),
        "{html}"
    );
    assert!(html.contains("20 panels"), "{html}");
}

#[test]
fn svg_runs_slow_setup_once_for_20_panels() {
    if !have_duckdb("svg_runs_slow_setup_once_for_20_panels") {
        return;
    }
    let svg = within_budget("svg-once", &|sql| {
        let mut c = dashboard();
        c.arg("--svg").arg(sql).arg(sql.with_extension("out"));
        c
    });
    let svg = String::from_utf8_lossy(&svg);
    assert!(svg.starts_with("<svg"), "{svg}");
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
    within_budget("check-once", &|sql| {
        let mut c = dashboard();
        c.arg("--check").arg(sql);
        c
    });
}

/// The same check against a prebuilt `--db` (read-only), the other session
/// shape `--check` supports — a `CREATE TEMP TABLE` setup statement still
/// runs (TEMP objects are never part of the read-only, on-disk catalog
/// `-readonly` protects).
#[test]
fn check_with_db_runs_slow_setup_once_for_20_panels() {
    if !have_duckdb("check_with_db_runs_slow_setup_once_for_20_panels") {
        return;
    }
    let s = Scratch::new("check-db");
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
    within_budget("check-db-once", &|sql| {
        let mut c = dashboard();
        c.arg("--check").arg(sql).arg("--db").arg(&db);
        c
    });
}
