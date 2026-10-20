//! #333: the `dashboard` binary defaults `DATAZOO_DISABLE_TELEMETRY` to "1"
//! before it ever spawns a `duckdb` child — so every anofox extension such a
//! child `LOAD`s inherits the default. This is a process-boundary check: it
//! proves the variable actually reaches the `duckdb` child's environment,
//! which the pure-function unit tests (`src/host/telemetry.rs`,
//! `src/bin/dashboard.rs`) cannot. No real `duckdb` or anofox extension is
//! involved — `duckdb` on `PATH` is replaced with a tiny script that reports
//! its own environment and exits, so this test can never reach the network
//! or send real telemetry, on CI or off it.
#![cfg(unix)]

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

/// A scratch directory under `std::env::temp_dir()` (TMPDIR), removed on drop.
struct Scratch(PathBuf);
impl Scratch {
    fn new(tag: &str) -> Self {
        let d = std::env::temp_dir().join(format!(
            "anofox_333_{tag}_{}_{}",
            std::process::id(),
            tag.len() // cheap disambiguator between the two tests below
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

/// Write a fake `duckdb` on `PATH` that, instead of running anything, dumps
/// `DATAZOO_DISABLE_TELEMETRY` (or the literal `<unset>`) to `probe` and exits
/// 0 — standing in for every anofox extension's own env check, without
/// actually loading or contacting one.
fn fake_duckdb(dir: &Path, probe: &Path) {
    let script = format!(
        "#!/bin/sh\nprintf '%s' \"${{DATAZOO_DISABLE_TELEMETRY:-<unset>}}\" > {}\nexit 0\n",
        shell_quote(probe)
    );
    let path = dir.join("duckdb");
    std::fs::File::create(&path)
        .unwrap()
        .write_all(script.as_bytes())
        .unwrap();
    let mut perm = std::fs::metadata(&path).unwrap().permissions();
    perm.set_mode(0o755);
    std::fs::set_permissions(&path, perm).unwrap();
}

fn shell_quote(p: &Path) -> String {
    format!("'{}'", p.to_string_lossy().replace('\'', "'\\''"))
}

fn run_describe(bin_dir: &Path, extra_env: Option<(&str, &str)>) -> String {
    let probe = bin_dir.join("probe.txt");
    fake_duckdb(bin_dir, &probe);
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_dashboard"));
    cmd.args(["--describe", "t"])
        .env_remove("DATAZOO_DISABLE_TELEMETRY")
        .env("PATH", bin_dir);
    if let Some((k, v)) = extra_env {
        cmd.env(k, v);
    }
    let out = cmd.output().expect("run dashboard --describe");
    assert!(
        out.status.success(),
        "dashboard --describe failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    std::fs::read_to_string(&probe).unwrap_or_else(|e| panic!("read probe file: {e}"))
}

#[test]
fn unset_env_reaches_the_duckdb_child_as_disabled() {
    let s = Scratch::new("unset");
    let seen = run_describe(&s.0, None);
    assert_eq!(
        seen, "1",
        "dashboard must default DATAZOO_DISABLE_TELEMETRY to \"1\" for its duckdb child when unset"
    );
}

#[test]
fn an_explicit_opt_in_reaches_the_duckdb_child_unchanged() {
    let s = Scratch::new("optin");
    let seen = run_describe(&s.0, Some(("DATAZOO_DISABLE_TELEMETRY", "0")));
    assert_eq!(
        seen, "0",
        "dashboard must leave an explicit DATAZOO_DISABLE_TELEMETRY alone (opt back into telemetry)"
    );
}

#[test]
fn an_empty_value_reaches_the_duckdb_child_as_disabled() {
    // #333 L1: a deployment template that renders the variable empty when
    // nobody actually set it (compose `VAR=${VAR}`, systemd
    // `Environment=VAR=`, an unset Helm value) is not an explicit opt-in.
    let s = Scratch::new("empty");
    let seen = run_describe(&s.0, Some(("DATAZOO_DISABLE_TELEMETRY", "")));
    assert_eq!(
        seen, "1",
        "dashboard must treat an empty DATAZOO_DISABLE_TELEMETRY as unset, not as an opt-in"
    );
}
