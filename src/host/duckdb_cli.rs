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

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::mpsc::Receiver;
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

/// The result of one [`OneProcessSession::round`].
pub enum RoundOutcome {
    /// The statement's own raw stdout text (DuckDB JSON; empty for a
    /// statement with no rows, such as plain DDL).
    Ok(String),
    /// The statement errored; the session is still alive and ready for the
    /// next round.
    Sql(String),
    /// The child process ended (crashed, was killed, or its stdin pipe
    /// broke) — not an ordinary SQL error. `String` is whatever of its
    /// stderr could still be collected.
    Ended(String),
}

/// A persistent `duckdb` CLI child, fed one statement at a time over stdin,
/// so a script's session state (`SET VARIABLE`, `LOAD`, TEMP views and
/// tables, `ATTACH`, plain DDL) carries forward exactly as a real connection
/// would (#435) — what the static renderers and `dashboard --check` use
/// instead of replaying every earlier setup statement before each later one
/// in a fresh process, which cost O(panels × setup).
///
/// Piping statements over stdin (rather than `-c "a; b; c;"`) matters: `-c`
/// aborts the whole script at the first error (confirmed empirically), but
/// feeding the same statements one at a time to a session reading from
/// stdin does not — the CLI prints the error and keeps reading the next
/// line, exactly the per-statement isolation every caller here already
/// relies on (a failing panel's error must not stop later panels from
/// rendering). Only the process itself ending unexpectedly — a crash, not a
/// SQL error — needs a caller-side fallback; see [`SessionRunner`].
///
/// Finding the end of one round's own stdout (which can itself span several
/// lines — a multi-row result prints one row per line) and of its own
/// stderr (which can ALSO span several lines — a Catalog Error's "Did you
/// mean" and `LINE 1: …` context) both use a fresh per-round random marker,
/// the same two-marker technique `serve.rs`'s `AuthoringSession` uses for
/// its own long-lived session, simplified for a single, already-trusted
/// script read once from disk by the operator running the CLI (no
/// watchdog, no staged-file `.read` defence against adversarial dot
/// commands — this is never network-facing, the same trust level
/// `run_query`'s `-c` argument already gave the script's text):
/// - a trailing `SELECT '<nonce>' AS __anofox_sep;`, whose one-line JSON
///   (`[{"__anofox_sep":"<nonce>"}]`, confirmed empirically to always be
///   exactly one line for a one-row, one-column result) marks the end of
///   this round's own stdout.
/// - a trailing `.read '<a path named after nonce, guaranteed missing>'`,
///   whose single-line `Error: cannot open "…"` (confirmed empirically to
///   be exactly one line, unlike a SQL error) marks the end of this
///   round's own stderr, drained on a background thread into a channel so
///   a child that only writes to stderr can't block the stdin writer or
///   the stdout reader.
pub struct OneProcessSession {
    child: Child,
    stdin: ChildStdin,
    out: BufReader<ChildStdout>,
    err: Receiver<String>,
}

impl OneProcessSession {
    /// Starts the session: `duckdb <db> -readonly -json` against a prebuilt
    /// database (as `--db` does for `--check`), or a fresh `duckdb :memory:
    /// -json` (as every static render already used).
    pub fn spawn(db: Option<&str>) -> Result<Self, String> {
        let mut cmd = Command::new("duckdb");
        match db {
            Some(f) => {
                cmd.arg("-readonly").arg(f);
            }
            None => {
                cmd.arg(":memory:");
            }
        }
        cmd.arg("-json");
        prepare("duckdb", &mut cmd);
        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("run duckdb: {e}"))?;
        let stdin = child.stdin.take().expect("piped stdin");
        let out = BufReader::new(child.stdout.take().expect("piped stdout"));
        let err_pipe = child.stderr.take().expect("piped stderr");
        let (tx, err) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(err_pipe).lines() {
                let Ok(l) = line else { break };
                if tx.send(l).is_err() {
                    break;
                }
            }
        });
        let mut s = OneProcessSession {
            child,
            stdin,
            out,
            err,
        };
        // Reset what a `~/.duckdbrc` could otherwise break every round with
        // (the same defence `AuthoringSession::spawn` applies in serve.rs):
        // `.bail on` would stop the session at the first error instead of
        // continuing, and a leftover `.output`/`.mode` would garble the
        // JSON this relies on.
        match s.round(".bail off\n.mode json\n") {
            RoundOutcome::Ended(tail) => Err(format!("duckdb did not start: {tail}")),
            RoundOutcome::Ok(_) | RoundOutcome::Sql(_) => Ok(s),
        }
    }

    /// Runs `sql` (one or more statements) in this session, after
    /// everything an earlier round sent — unlike `run_query`'s one-shot
    /// calls, `sql` carries no prelude of its own.
    pub fn round(&mut self, sql: &str) -> RoundOutcome {
        let nonce = super::serving::random_token();
        let missing = std::env::temp_dir().join(format!("anofox-sep-{nonce}"));
        let script = format!(
            "{sql}\nSELECT '{nonce}' AS __anofox_sep;\n.read '{}'\n",
            missing.display()
        );
        if self
            .stdin
            .write_all(script.as_bytes())
            .and_then(|_| self.stdin.flush())
            .is_err()
        {
            return RoundOutcome::Ended(self.drain_err());
        }
        let marker_line = format!("[{{\"__anofox_sep\":\"{nonce}\"}}]");
        let mut text = String::new();
        loop {
            let mut line = String::new();
            match self.out.read_line(&mut line) {
                Ok(0) | Err(_) => return RoundOutcome::Ended(self.drain_err()),
                Ok(_) => {
                    if line.trim_end_matches(['\r', '\n']) == marker_line {
                        break;
                    }
                    text.push_str(&line);
                }
            }
        }
        let mut err = String::new();
        loop {
            match self.err.recv() {
                Ok(l) if l.contains(&nonce) => break,
                Ok(l) => {
                    err.push_str(&l);
                    err.push('\n');
                }
                Err(_) => return RoundOutcome::Ended(err),
            }
        }
        if err.trim().is_empty() {
            RoundOutcome::Ok(text)
        } else {
            RoundOutcome::Sql(err.trim_end().to_string())
        }
    }

    /// stderr's remaining lines, once the CLI is gone (best-effort, short
    /// timeout: nothing more is coming once the pipe is closed, but a line
    /// already in flight gets a brief chance to land).
    fn drain_err(&mut self) -> String {
        let mut s = String::new();
        while let Ok(l) = self.err.recv_timeout(std::time::Duration::from_millis(200)) {
            s.push_str(&l);
            s.push('\n');
        }
        s
    }
}

impl Drop for OneProcessSession {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// [`OneProcessSession`], returning parsed rows and handling the rare case
/// of the child process ending unexpectedly (a crash — not an ordinary SQL
/// error, which the session already isolates per statement on its own):
/// one respawn, replaying everything this runner has ever been asked to
/// run (`sent`) into the fresh session before retrying the statement that
/// triggered the respawn. This is a best-effort recovery, not a guarantee —
/// if the crash was e.g. an out-of-memory kill, replaying the same setup
/// can reproduce it — but it is strictly better than the alternative of
/// giving up on every later statement over one transient failure, and it
/// can't loop forever (exactly one respawn attempt per call to [`Self::run`]).
pub struct SessionRunner {
    db: Option<String>,
    session: OneProcessSession,
    /// Everything ever handed to [`Self::run`] (successful or not — a
    /// statement that errored the first time errors the same way on
    /// replay, harmlessly), for the respawn-and-replay recovery above. Not
    /// used at all on the common, no-crash path.
    sent: String,
}

impl SessionRunner {
    pub fn spawn(db: Option<&str>) -> Result<Self, String> {
        Ok(SessionRunner {
            db: db.map(str::to_string),
            session: OneProcessSession::spawn(db)?,
            sent: String::new(),
        })
    }

    /// Runs `sql` in the session and returns its rows (parsed DuckDB JSON),
    /// or the DuckDB error text.
    pub fn run(
        &mut self,
        sql: &str,
    ) -> Result<Vec<serde_json::Map<String, serde_json::Value>>, String> {
        match self.session.round(sql) {
            RoundOutcome::Ok(text) => {
                self.sent.push_str(sql);
                self.sent.push('\n');
                crate::sql::parse_rows_json(&text)
            }
            RoundOutcome::Sql(e) => {
                self.sent.push_str(sql);
                self.sent.push('\n');
                Err(e)
            }
            RoundOutcome::Ended(tail) => self.recover_and_retry(sql, tail),
        }
    }

    fn recover_and_retry(
        &mut self,
        sql: &str,
        tail: String,
    ) -> Result<Vec<serde_json::Map<String, serde_json::Value>>, String> {
        eprintln!(
            "warning: the duckdb session ended unexpectedly{} — restarting it and replaying \
             what it had already run before continuing",
            if tail.trim().is_empty() {
                String::new()
            } else {
                format!(": {}", tail.trim())
            }
        );
        let mut fresh = OneProcessSession::spawn(self.db.as_deref()).map_err(|e| {
            format!("the duckdb session ended ({tail}) and a new one could not be started: {e}")
        })?;
        if !self.sent.is_empty() {
            // Best-effort: this is restoring session state the first pass
            // already reported on, not new work to diagnose, so its own
            // outcome (even another Ended) is not surfaced here — only
            // the retry below is.
            let _ = fresh.round(&self.sent);
        }
        self.session = fresh;
        match self.session.round(sql) {
            RoundOutcome::Ok(text) => {
                self.sent.push_str(sql);
                self.sent.push('\n');
                crate::sql::parse_rows_json(&text)
            }
            RoundOutcome::Sql(e) => {
                self.sent.push_str(sql);
                self.sent.push('\n');
                Err(e)
            }
            RoundOutcome::Ended(tail2) => Err(format!(
                "the duckdb session ended and could not be recovered: {tail2}"
            )),
        }
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
    ///
    /// Waits until the script can be exec'd (#361): a test on another thread
    /// that forks while `f` is open inherits the write fd until its child
    /// execs, and exec'ing a file open for writing fails with ETXTBSY, which
    /// `detect_major_version` would read as "no CLI".
    fn fake_duckdb(dir: &std::path::Path, version_line: &str) -> std::path::PathBuf {
        let path = dir.join("duckdb");
        {
            let mut f = std::fs::File::create(&path).unwrap();
            writeln!(f, "#!/bin/sh\necho '{version_line}'").unwrap();
            f.set_permissions(std::fs::Permissions::from_mode(0o755))
                .unwrap();
        }
        const ETXTBSY: i32 = 26;
        for _ in 0..200 {
            match Command::new(&path).arg("--version").output() {
                Err(e) if e.raw_os_error() == Some(ETXTBSY) => {
                    std::thread::sleep(std::time::Duration::from_millis(10))
                }
                _ => break,
            }
        }
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

    fn no_duckdb_on_path() -> bool {
        Command::new("duckdb").arg("-version").output().is_err()
    }

    /// #435: the whole point — a `CREATE TEMP TABLE` in one round is still
    /// there (not re-run, not forgotten) in a later round, exactly as a
    /// real connection would keep it, without any prelude text.
    #[test]
    fn session_keeps_state_across_rounds() {
        if no_duckdb_on_path() {
            eprintln!("skipped: no `duckdb` on PATH");
            return;
        }
        let mut s = OneProcessSession::spawn(None).unwrap();
        assert!(matches!(
            s.round("CREATE TEMP TABLE t AS SELECT 1 AS x;"),
            RoundOutcome::Ok(_)
        ));
        assert!(matches!(
            s.round("SET VARIABLE y = 41;"),
            RoundOutcome::Ok(_)
        ));
        match s.round("SELECT x, getvariable('y') + 1 AS y FROM t;") {
            RoundOutcome::Ok(text) => {
                let rows = crate::sql::parse_rows_json(&text).unwrap();
                assert_eq!(rows.len(), 1);
                assert_eq!(rows[0]["x"], 1);
                assert_eq!(rows[0]["y"], 42);
            }
            other => panic!("expected rows, got {}", outcome_label(&other)),
        }
    }

    /// #435: an ordinary SQL error on one round doesn't end the session —
    /// the very next round still succeeds, with everything before it still
    /// in scope. This is the property that makes a fallback unnecessary for
    /// per-statement errors (only a crashed process needs one).
    #[test]
    fn a_statement_error_does_not_end_the_session() {
        if no_duckdb_on_path() {
            eprintln!("skipped: no `duckdb` on PATH");
            return;
        }
        let mut s = OneProcessSession::spawn(None).unwrap();
        assert!(matches!(
            s.round("SET VARIABLE x = 1;"),
            RoundOutcome::Ok(_)
        ));
        match s.round("SELECT * FROM table_that_does_not_exist;") {
            RoundOutcome::Sql(e) => assert!(e.contains("table_that_does_not_exist"), "{e}"),
            other => panic!("expected a SQL error, got {}", outcome_label(&other)),
        }
        match s.round("SELECT getvariable('x') AS v;") {
            RoundOutcome::Ok(text) => {
                let rows = crate::sql::parse_rows_json(&text).unwrap();
                assert_eq!(rows[0]["v"], 1, "x is still set after the earlier error");
            }
            other => panic!("expected rows, got {}", outcome_label(&other)),
        }
    }

    /// A statement whose JSON result spans several lines (one row per
    /// line) is still read in full, and its boundary is found correctly —
    /// not cut short, not merged with the next round's own output.
    #[test]
    fn a_multi_row_result_does_not_confuse_the_round_boundary() {
        if no_duckdb_on_path() {
            eprintln!("skipped: no `duckdb` on PATH");
            return;
        }
        let mut s = OneProcessSession::spawn(None).unwrap();
        match s.round("SELECT * FROM range(50) t(x);") {
            RoundOutcome::Ok(text) => {
                let rows = crate::sql::parse_rows_json(&text).unwrap();
                assert_eq!(rows.len(), 50);
            }
            other => panic!("expected rows, got {}", outcome_label(&other)),
        }
        // the next round is still this round's own result, not leftovers.
        match s.round("SELECT 'next' AS v;") {
            RoundOutcome::Ok(text) => {
                let rows = crate::sql::parse_rows_json(&text).unwrap();
                assert_eq!(rows.len(), 1);
                assert_eq!(rows[0]["v"], "next");
            }
            other => panic!("expected rows, got {}", outcome_label(&other)),
        }
    }

    fn outcome_label(o: &RoundOutcome) -> &'static str {
        match o {
            RoundOutcome::Ok(_) => "Ok",
            RoundOutcome::Sql(_) => "Sql",
            RoundOutcome::Ended(_) => "Ended",
        }
    }

    /// #435: `SessionRunner::run` returns parsed rows (not raw text), and a
    /// statement error is just an `Err` — the recovery path is never
    /// exercised in ordinary use.
    #[test]
    fn session_runner_parses_rows_and_isolates_errors() {
        if no_duckdb_on_path() {
            eprintln!("skipped: no `duckdb` on PATH");
            return;
        }
        let mut r = SessionRunner::spawn(None).unwrap();
        r.run("CREATE TEMP TABLE t AS SELECT 1 AS x;").unwrap();
        let e = r.run("SELECT * FROM nope;").unwrap_err();
        assert!(e.contains("nope"), "{e}");
        let rows = r.run("SELECT x FROM t;").unwrap();
        assert_eq!(rows[0]["x"], 1, "t is still there after the error above");
    }

    /// #435: if the live process is killed outright, `SessionRunner`
    /// recovers by restarting and replaying what it had already run, then
    /// retries the statement that triggered the respawn — so the render
    /// keeps going instead of losing the whole rest of the script to one
    /// crash.
    #[test]
    fn session_runner_recovers_from_a_killed_process() {
        if no_duckdb_on_path() {
            eprintln!("skipped: no `duckdb` on PATH");
            return;
        }
        let mut r = SessionRunner::spawn(None).unwrap();
        r.run("CREATE TEMP TABLE t AS SELECT 7 AS x;").unwrap();
        r.session.child.kill().unwrap();
        // the in-flight round's write/read fails against the now-dead
        // process, triggering recovery — the table is back (replayed) and
        // this very statement's own rows come back too, from the new session.
        let rows = r.run("SELECT x FROM t;").unwrap();
        assert_eq!(rows[0]["x"], 7);
    }
}
