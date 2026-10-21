//! #352: when the `duckdb` CLI on PATH is a different major version than
//! this build's pin (`.duckdb-version`), `dashboard`/`serve` must warn
//! clearly on stderr -- DuckDB's storage format is one-way across major
//! versions (a 2.0 file can't be opened by a 1.x CLI, and vice versa), so a
//! silent mismatch can leave a file unreadable by the other side.
//!
//! Exercised through `dashboard --describe`, the simplest subcommand that
//! spawns a `duckdb` child: a fake script on PATH stands in for the CLI, so
//! the test doesn't depend on which DuckDB happens to be installed locally.

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

struct Scratch(PathBuf);
impl Scratch {
    fn new(tag: &str) -> Self {
        let d = std::env::temp_dir().join(format!(
            "anofox_352_version_guard_{tag}_{}",
            std::process::id()
        ));
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

/// Writes an executable `duckdb` shim into `dir` that ignores its arguments
/// and always prints `version_line` to stdout, exit 0 -- enough to answer
/// both the `--version` probe and the real `-c SUMMARIZE ...` call that
/// follows it.
fn fake_duckdb(dir: &Path, version_line: &str) -> PathBuf {
    let path = dir.join("duckdb");
    let mut f = std::fs::File::create(&path).unwrap();
    writeln!(f, "#!/bin/sh\necho '{version_line}'").unwrap();
    f.set_permissions(std::fs::Permissions::from_mode(0o755))
        .unwrap();
    path
}

fn run_describe_with_fake_cli(version_line: &str, tag: &str) -> std::process::Output {
    let scratch = Scratch::new(tag);
    fake_duckdb(&scratch.0, version_line);
    let path = format!(
        "{}:{}",
        scratch.0.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    Command::new(env!("CARGO_BIN_EXE_dashboard"))
        .args(["--describe", "range(3)"])
        .env("PATH", path)
        .output()
        .expect("run dashboard --describe")
}

#[test]
fn warns_on_a_duckdb_cli_major_version_mismatch() {
    // This build is pinned to v1.5.6 (.duckdb-version); a 2.0 CLI on PATH
    // must trigger the warning.
    let out = run_describe_with_fake_cli("v2.0.0 (Cyanoptera) d54cbbe3df", "mismatch");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("different") || stderr.contains("major version"),
        "expected a version-mismatch warning on stderr, got: {stderr}"
    );
    assert!(
        stderr.contains("one-way")
            || stderr.contains("cannot be opened")
            || stderr.contains("unusable"),
        "expected the warning to explain the one-way storage risk, got: {stderr}"
    );
}

#[test]
fn no_warning_when_the_duckdb_cli_matches_the_pin() {
    let out = run_describe_with_fake_cli("v1.5.6 d8cdaa33fd", "match");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("major version"),
        "unexpected version-mismatch warning on a matching CLI: {stderr}"
    );
}
