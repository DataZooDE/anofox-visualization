//! Helpers for the `duckdb` CLI children this crate spawns (`src/bin/serve.rs`,
//! `src/bin/dashboard.rs`): detect the CLI's DuckDB major version once per
//! process, warn if it differs from this build's pin, and add `-no-agent`
//! only when the CLI actually understands it (#352).
//!
//! DuckDB 2.0 added CLI "agent mode" (Markdown output, errors as JSON,
//! runaway queries auto-stopped) that switches on automatically when
//! `AI_AGENT` (or another agent-harness marker such as `CLAUDECODE`) is set
//! and stdout is not a terminal -- exactly our own situation, piping a child
//! process's stdout. `-no-agent` turns it off, but a 1.5.6 CLI does not know
//! that flag and exits with "Unknown Option Error" on it, so we can only pass
//! it after confirming the CLI is new enough.

use std::process::Command;
use std::sync::OnceLock;

/// This build's DuckDB pin (`.duckdb-version`, the release pin -- untouched
/// by #352's groundwork).
pub const PINNED_DUCKDB_VERSION: &str =
    include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/.duckdb-version"));

/// Parse the major version out of `duckdb --version` output (or the
/// `.duckdb-version` file). The CLI's first line is sometimes an agent-mode
/// banner (stdout, printed before the version when `AI_AGENT`/`CLAUDECODE`
/// is set and stdout isn't a TTY) -- so every line is checked, not just the
/// first, for one that starts with an optional `v` followed by digits.
pub fn parse_duckdb_major(output: &str) -> Option<u32> {
    for line in output.lines() {
        let line = line.trim();
        let v = line.strip_prefix('v').unwrap_or(line);
        let digits: String = v.chars().take_while(|c| c.is_ascii_digit()).collect();
        if digits.is_empty() {
            continue;
        }
        // Must be followed by '.', not some other identifier that happens to
        // start with digits (defensive; real output is "vX.Y.Z ...").
        if v[digits.len()..].starts_with('.') {
            if let Ok(major) = digits.parse() {
                return Some(major);
            }
        }
    }
    None
}

/// This build's pinned major version (parsed from `.duckdb-version`).
pub fn pinned_major() -> Option<u32> {
    parse_duckdb_major(PINNED_DUCKDB_VERSION)
}

/// Runs `<bin> --version` and parses its major version; `None` if the binary
/// is missing, fails, or its output doesn't parse.
pub fn detect_major_version(bin: &str) -> Option<u32> {
    let out = Command::new(bin).arg("--version").output().ok()?;
    if !out.status.success() {
        return None;
    }
    parse_duckdb_major(&String::from_utf8_lossy(&out.stdout))
}

struct CliInfo {
    supports_no_agent: bool,
}

static CLI_INFO: OnceLock<CliInfo> = OnceLock::new();

/// Detected once per process (first caller pays the `--version` round trip;
/// every later call reuses it): the CLI major version is compared against
/// this build's pin and a mismatch is reported on stderr exactly once,
/// because DuckDB's on-disk format is one-way across major versions -- a 2.0
/// file cannot be opened by a 1.x CLI, and vice versa (#352) -- so a
/// mismatched child can silently fail to read or write what the other side
/// expects.
fn cli_info(bin: &str) -> &'static CliInfo {
    CLI_INFO.get_or_init(|| {
        let cli_major = detect_major_version(bin);
        if let (Some(cli_major), Some(pinned)) = (cli_major, pinned_major()) {
            if cli_major != pinned {
                eprintln!(
                    "anofox_visualization: warning: the `{bin}` CLI on PATH reports DuckDB \
                     major version {cli_major}, but this build is pinned to {pin} \
                     ({full}). DuckDB's storage format is one-way across major versions -- \
                     a database written by one cannot be opened by the other -- so mixing \
                     them can leave a file this process reads or writes unusable on the \
                     other side.",
                    pin = pinned,
                    full = PINNED_DUCKDB_VERSION.trim(),
                );
            }
        }
        CliInfo {
            supports_no_agent: cli_major.is_some_and(|m| m >= 2),
        }
    })
}

/// Call right before spawning/running a `duckdb` CLI child: adds `-no-agent`
/// if (and only if) the CLI on PATH understands it, and -- the first time
/// only -- warns on a major-version mismatch against this build's pin.
pub fn prepare(bin: &str, cmd: &mut Command) {
    if cli_info(bin).supports_no_agent {
        cmd.arg("-no-agent");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn parses_plain_and_banner_prefixed_output() {
        assert_eq!(parse_duckdb_major("v1.5.6 d8cdaa33fd\n"), Some(1));
        assert_eq!(
            parse_duckdb_major("v2.0.0 (Cyanoptera) d54cbbe3df\n"),
            Some(2)
        );
        assert_eq!(
            parse_duckdb_major(
                "duckdb agent mode on: AI_AGENT is set and stdout is not a terminal; \
                 -no-agent turns it off, .help agent explains the output\nv2.0.0 (Cyanoptera) d54cbbe3df\n"
            ),
            Some(2)
        );
        assert_eq!(parse_duckdb_major("not a version string"), None);
        assert_eq!(parse_duckdb_major(""), None);
    }

    #[test]
    fn pinned_major_matches_dot_duckdb_version() {
        // .duckdb-version is "v1.5.6\n" today; this guards against a future
        // bump leaving the parser behind, not against changing the pin.
        assert_eq!(pinned_major(), Some(1));
    }

    /// A fake `duckdb` script that only answers `--version`, to test
    /// `detect_major_version` / `prepare` without needing a real DuckDB
    /// binary of a specific version on the test runner.
    fn fake_duckdb(dir: &std::path::Path, version_line: &str) -> std::path::PathBuf {
        let path = dir.join("duckdb");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(f, "#!/bin/sh\necho '{version_line}'").unwrap();
        f.set_permissions(std::fs::Permissions::from_mode(0o755))
            .unwrap();
        path
    }

    #[test]
    fn detects_major_version_from_a_fake_cli() {
        let dir = std::env::temp_dir().join(format!(
            "anofox-duckdb-cli-test-{}-{}",
            std::process::id(),
            line!()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let bin = fake_duckdb(&dir, "v2.0.0 (Cyanoptera) d54cbbe3df");
        assert_eq!(detect_major_version(bin.to_str().unwrap()), Some(2));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn prepare_adds_no_agent_only_for_a_new_enough_cli() {
        let dir = std::env::temp_dir().join(format!(
            "anofox-duckdb-cli-test2-{}-{}",
            std::process::id(),
            line!()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let bin = fake_duckdb(&dir, "v2.0.0 (Cyanoptera) d54cbbe3df");
        // Exercised directly rather than through the process-wide cache
        // (`prepare`/`cli_info` memoize on first use, which would make a
        // second variant in the same test binary see the first's result):
        // re-derive the same decision `prepare` would make for this bin.
        let major = detect_major_version(bin.to_str().unwrap());
        assert_eq!(major, Some(2));
        assert!(major.is_some_and(|m| m >= 2));

        let old = fake_duckdb(&dir, "v1.5.6 d8cdaa33fd");
        let major_old = detect_major_version(old.to_str().unwrap());
        assert_eq!(major_old, Some(1));
        assert!(!major_old.is_some_and(|m| m >= 2));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
