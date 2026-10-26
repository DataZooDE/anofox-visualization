//! #517/#538, review-530 round 2: `dashboard --check --options locked.json`
//! must derive a dashboard's id exactly the way the running server's own
//! directory loader does (`duckext`'s `load_dashboards`, via the shared
//! `anofox_visualization::host::serving::dashboard_id_from_path`), so a
//! `submit/not-enabled` / `state/not-enabled` diagnostic names the id
//! `options.state.dashboards` is actually matched against — the same id
//! the server registers this file under.
//!
//! Before the fix, folder mode derived an id by repeatedly trimming a
//! literal `".sql"` suffix off the bare filename (`str::trim_end_matches`),
//! which disagrees with `Path::file_stem()` (what the server uses) on a
//! filename with more than one trailing `.sql` — an admittedly odd
//! filename, but one `valid_dashboard_id` allows (`.` is a legal id
//! character) and the server would happily load. This test uses exactly
//! that shape (`survey.sql.sql`) to prove the check's id now matches.
use std::path::{Path, PathBuf};
use std::process::Command;

/// A scratch directory under `std::env::temp_dir()` (TMPDIR), removed on drop.
struct Scratch(PathBuf);
impl Scratch {
    fn new(tag: &str) -> Self {
        let d = std::env::temp_dir().join(format!("anofox_538_{tag}_{}", std::process::id()));
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

const SCRIPT: &str = "\
SELECT q1 ::BUTTONS FROM (VALUES ('A'), ('B')) t(q1);
SELECT 'Check my answer' AS quiz::SUBMIT, 'q1' AS r::SUBMIT_VARS, 'Saved.' AS d::SUBMIT_DONE;
";

fn run_check(dash_dir: &Path, options: &Path) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_dashboard"))
        .arg("--check")
        .arg(dash_dir)
        .arg("--json")
        .arg("--options")
        .arg(options)
        .output()
        .expect("run dashboard --check --options");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
    )
}

#[test]
fn a_filename_with_a_repeated_sql_suffix_is_checked_under_its_file_stem_id() {
    let s = Scratch::new("odd");
    let dash_dir = s.0.join("dash");
    std::fs::create_dir_all(&dash_dir).unwrap();
    // file_stem("survey.sql.sql") == "survey.sql" (only the final
    // extension is stripped); the old trim_end_matches(".sql") code
    // would have stripped both, landing on "survey".
    std::fs::write(dash_dir.join("survey.sql.sql"), SCRIPT).unwrap();

    let options = s.0.join("locked.json");
    std::fs::write(
        &options,
        format!(
            r#"{{"state": {{"path": {:?}, "dashboards": ["survey.sql"]}}}}"#,
            s.0.join("state.jsonl").to_string_lossy()
        ),
    )
    .unwrap();

    let (ok, stdout) = run_check(&dash_dir, &options);
    assert!(
        ok && !stdout.contains("submit/not-enabled"),
        "the dashboard's id (file_stem \"survey.sql\") IS in \
         options.state.dashboards, so --check must not report \
         submit/not-enabled; got: {stdout}"
    );

    // The same file checked against an options file that enables the
    // *wrong* (old, buggy) id must still flag it — proving the lack of a
    // diagnostic above is because the id now matches, not because the
    // check stopped looking.
    let wrong_options = s.0.join("locked-wrong.json");
    std::fs::write(
        &wrong_options,
        format!(
            r#"{{"state": {{"path": {:?}, "dashboards": ["survey"]}}}}"#,
            s.0.join("state.jsonl").to_string_lossy()
        ),
    )
    .unwrap();
    let (ok2, stdout2) = run_check(&dash_dir, &wrong_options);
    assert!(
        !ok2 && stdout2.contains("submit/not-enabled"),
        "enabling the old, wrongly-trimmed id (\"survey\") must not satisfy \
         the check for a file whose real id is \"survey.sql\"; got: {stdout2}"
    );
}

#[test]
fn single_file_mode_uses_the_same_id_derivation() {
    let s = Scratch::new("single");
    let file = s.0.join("report.v2.sql");
    std::fs::write(&file, SCRIPT).unwrap();

    let options = s.0.join("locked.json");
    std::fs::write(
        &options,
        format!(
            r#"{{"state": {{"path": {:?}, "dashboards": ["report.v2"]}}}}"#,
            s.0.join("state.jsonl").to_string_lossy()
        ),
    )
    .unwrap();

    let (ok, stdout) = run_check(&file, &options);
    assert!(
        ok && !stdout.contains("submit/not-enabled"),
        "single-file mode must derive the same file_stem id (\"report.v2\") \
         the folder loader (and the server) would; got: {stdout}"
    );
}
