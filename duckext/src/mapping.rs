//! A customer mapping built at an agent's start (#194 A3b, plan §4, H1):
//! `options.agent.mapping` names a canonical contract, the customer's
//! mapping onto it and the customer's source database. Every snapshot build
//! runs it in its own throwaway staging instance — never the serving one:
//! the source attached `READ_ONLY`, the output file attached, then
//! `memory_limit` / `threads`, external access off and the configuration
//! locked, before any of the customer's SQL runs. The checks are
//! `dashboard --conform`'s own ([`canonical::conform`]); a mapping that
//! doesn't conform refuses the start (or the rebuild, which keeps the old
//! snapshot). Then each contract table is copied into `canonical.duckdb` in
//! the snapshot directory, which the serving instance attaches `READ_ONLY`
//! like any other snapshot database. A watchdog interrupts the build at
//! `timeout_s` or when the server stops.

use crate::db::Database;
use anofox_visualization::canonical;
use anofox_visualization::host::agent::MappingOptions;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The name the built database is attached under in the snapshot (the
/// dashboard's bare canonical names resolve there).
pub const ATTACH_AS: &str = "canonical";

/// Mapping builds running at once in this process, at most (#987: every
/// locked agent server in a process may build at the same time, each up to
/// its `memory_limit`); a build past the cap waits for a slot until its
/// own deadline.
const MAX_CONCURRENT_BUILDS: usize = 2;
static BUILDS: AtomicUsize = AtomicUsize::new(0);

/// A build slot, released on drop (a panic too).
struct Slot;
impl Drop for Slot {
    fn drop(&mut self) {
        BUILDS.fetch_sub(1, Ordering::SeqCst);
    }
}

fn take_slot(deadline: Instant, stop: &AtomicBool) -> Result<Slot, String> {
    loop {
        if BUILDS.fetch_add(1, Ordering::SeqCst) < MAX_CONCURRENT_BUILDS {
            return Ok(Slot);
        }
        BUILDS.fetch_sub(1, Ordering::SeqCst);
        if Instant::now() >= deadline || stop.load(Ordering::SeqCst) {
            return Err(format!(
                "agent.mapping: {MAX_CONCURRENT_BUILDS} mapping builds were running in this process until timeout_s passed"
            ));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn lit(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

fn qi(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

/// What a build gives the snapshot: `(name, file, notes)` like a source
/// build, and the agent's questions this mapping can't answer (#982).
type Built = ((String, PathBuf, Vec<String>), Vec<String>);

/// Why the watchdog stopped a build.
const RUNNING: u8 = 0;
const TIMED_OUT: u8 = 1;
const TOO_BIG: u8 = 2;

/// Bytes on disk of `out` (and its WAL) and of the files in `tmp`.
fn bytes_used(out: &Path, tmp: &Path) -> u64 {
    let len = |p: &Path| std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
    let mut wal = out.as_os_str().to_owned();
    wal.push(".wal");
    let spill: u64 = std::fs::read_dir(tmp)
        .map(|d| d.flatten().map(|e| len(&e.path())).sum())
        .unwrap_or(0);
    len(out) + len(Path::new(&wal)) + spill
}

/// Build the canonical tables into `dir/canonical.duckdb`, or say why the
/// mapping is refused. `script` (the agent's dashboard): its questions are
/// bound against the views ([`canonical::questions`]); one whose panel
/// reads a column the mapping leaves out is off for this customer.
///
/// Bounded (#987): `memory_limit` and 4 threads; spills only into the
/// snapshot directory, capped with the output at `max_bytes` (a watchdog
/// checks the files, and each copied table); `max_rows` per contract table;
/// `timeout_s` for the whole build, the server's stop flag too — the
/// watchdog interrupts on every tick once it fired, every statement after
/// that is refused, and a fired build never counts as built.
pub fn build(
    m: &MappingOptions,
    dir: &Path,
    stop: &Arc<AtomicBool>,
    script: Option<&str>,
) -> Result<Built, String> {
    let read = |p: &str| std::fs::read_to_string(p).map_err(|e| format!("agent.mapping: {p}: {e}"));
    let (contract_text, views_text) = (read(&m.contract)?, read(&m.views)?);
    // #194 A4 (C4.1): the files the bundle was checked against at the start
    if let Some((views, contract)) = &m.pinned {
        if *views != views_text || *contract != contract_text {
            return Err("agent.mapping: the views or contract file changed since the start \
                        (they are pinned by agent.digest) — publish, pin the new digest and restart"
                .into());
        }
    }
    let contract = canonical::parse_contract(&contract_text)
        .map_err(|e| format!("agent.mapping.contract: {e}"))?;
    // #988: a scope the contract doesn't declare is a typo, not "unchecked"
    if let Some(k) = canonical::unknown_scopes(&contract, &m.scope).first() {
        return Err(format!(
            "agent.mapping.scope.{k}: the contract declares no such scope"
        ));
    }
    let mapping = canonical::parse_mapping(&views_text, &contract)
        .map_err(|e| format!("agent.mapping.views: {}", e.join("; ")))?;
    let source =
        std::fs::canonicalize(&m.db).map_err(|e| format!("agent.mapping.db {}: {e}", m.db))?;
    let deadline = Instant::now() + Duration::from_secs(m.timeout_s);
    let _slot = take_slot(deadline, stop)?;
    let out = dir.join("canonical.duckdb");
    let tmp = dir.join("mapping-tmp");
    std::fs::create_dir_all(&tmp).map_err(|e| format!("agent.mapping: {}: {e}", tmp.display()))?;
    let (tmp_s, max_temp) = (
        tmp.to_string_lossy().into_owned(),
        format!("{}B", m.max_bytes),
    );
    let db = Database::open(
        ":memory:",
        &[
            ("autoinstall_known_extensions", "false"),
            ("autoload_known_extensions", "false"),
            ("allow_community_extensions", "false"),
            // #987: spills stay in the snapshot directory, capped
            ("temp_directory", &tmp_s),
            ("max_temp_directory_size", &max_temp),
        ],
    )?;
    let conn = db.connect()?;
    let intr = conn.interrupter();
    let finished = AtomicBool::new(false);
    let fired = AtomicU8::new(RUNNING);
    let result = std::thread::scope(|scope| {
        let (finished, fired, out, tmp) = (&finished, &fired, &out, &tmp);
        let stop = stop.clone();
        let max_bytes = m.max_bytes;
        scope.spawn(move || {
            while !finished.load(Ordering::SeqCst) {
                let why = if Instant::now() >= deadline || stop.load(Ordering::SeqCst) {
                    TIMED_OUT
                } else if bytes_used(out, tmp) > max_bytes {
                    TOO_BIG
                } else {
                    RUNNING
                };
                if why != RUNNING {
                    let _ =
                        fired.compare_exchange(RUNNING, why, Ordering::SeqCst, Ordering::SeqCst);
                }
                // #987: once fired, on every tick — an interrupt that lands
                // between two statements would otherwise be lost
                if fired.load(Ordering::SeqCst) != RUNNING {
                    intr.interrupt();
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        });
        let r = (|| -> Result<(Vec<String>, Vec<String>), String> {
            let mut duck = |sql: &str| -> Result<Vec<Vec<Option<String>>>, String> {
                if fired.load(Ordering::SeqCst) != RUNNING {
                    return Err("the build was stopped".into());
                }
                conn.query(sql)?.string_rows(1_000_000)
            };
            // #988: the classifier and the scope check parse with
            // json_serialize_sql — loaded while that is still allowed
            duck("LOAD json").map_err(|e| {
                format!("agent.mapping needs DuckDB's json extension (INSTALL json): {e}")
            })?;
            let setup = canonical::instance_setup(&source.to_string_lossy(), &m.memory_limit, 4);
            // the output is attached right after the source (#989: both
            // after autoloading is off, before external access and the lock)
            let at = setup
                .iter()
                .position(|s| s.starts_with("ATTACH"))
                .ok_or("the build instance's setup attaches no source")?;
            for s in &setup[..=at] {
                duck(s)?;
            }
            duck(&format!(
                "ATTACH {} AS {} (TYPE duckdb)",
                lit(&out.to_string_lossy()),
                qi(ATTACH_AS)
            ))?;
            for s in &setup[at + 1..] {
                duck(s)?;
            }
            let opts = canonical::Options {
                scope_values: m.scope.clone(),
            };
            let report = canonical::conform(&mut duck, &contract, &mapping, &opts)?;
            if !report.ok() {
                return Err(format!(
                    "agent.mapping: the customer mapping does not conform (dashboard --conform shows the same):\n{}",
                    report.to_text()
                ));
            }
            let mut notes: Vec<String> = report
                .findings
                .iter()
                .filter(|f| !f.error)
                .map(|f| format!("agent.mapping: {}: {}", f.table, f.message))
                .collect();
            for t in &contract.tables {
                let target = format!("{}.main.{}", qi(ATTACH_AS), qi(&t.name));
                // #987: one row past the cap is enough to refuse
                duck(&format!(
                    "CREATE TABLE {target} AS SELECT * FROM memory.main.{} LIMIT {}",
                    qi(&t.name),
                    m.max_rows.saturating_add(1)
                ))?;
                let n: u64 = duck(&format!("SELECT count(*)::VARCHAR FROM {target}"))?
                    .first()
                    .and_then(|r| r.first().cloned().flatten())
                    .and_then(|n| n.parse().ok())
                    .unwrap_or(u64::MAX);
                if n > m.max_rows {
                    return Err(format!(
                        "agent.mapping: {} has more than max_rows ({}) rows",
                        t.name, m.max_rows
                    ));
                }
                if bytes_used(out, tmp) > m.max_bytes {
                    return Err(too_big(m.max_bytes));
                }
            }
            // the output is done: detached before any dashboard SQL runs here
            duck(&format!("DETACH {}", qi(ATTACH_AS)))?;
            // #982: the dashboard's questions bound against the views. Only a
            // column the mapping leaves out switches one off; any other bind
            // error (a function this instance doesn't load, a setup statement
            // that failed here) keeps it listed — the panel path answers it,
            // or fails it, as before.
            let mut off = Vec::new();
            for q in script
                .map(|s| canonical::questions(&mut duck, s))
                .unwrap_or_default()
            {
                let Some(why) = q.off else { continue };
                if is_missing_column(&why) {
                    notes.push(format!(
                        "agent.mapping: question '{}' is off for this mapping: {why}",
                        q.id
                    ));
                    off.push(q.id);
                }
            }
            Ok((notes, off))
        })();
        finished.store(true, Ordering::SeqCst);
        r
    });
    drop(conn);
    drop(db);
    let _ = std::fs::remove_dir_all(&tmp);
    // #987: a build the watchdog stopped is refused, whatever it returned
    match fired.load(Ordering::SeqCst) {
        TIMED_OUT => Err(format!(
            "agent.mapping: the build took longer than {} s (timeout_s) or the server stopped",
            m.timeout_s
        )),
        TOO_BIG => Err(too_big(m.max_bytes)),
        _ => result.map(|(notes, off)| ((ATTACH_AS.to_string(), out, notes), off)),
    }
}

fn too_big(max: u64) -> String {
    format!("agent.mapping: the built tables and the build's spill passed max_bytes ({max} bytes)")
}

/// A bind error that says a column isn't there (and nothing failed before).
fn is_missing_column(why: &str) -> bool {
    !why.contains("a setup statement failed first")
        && (why.contains("Referenced column") || why.contains("does not have a column named"))
}

#[cfg(test)]
mod tests {
    #[test]
    fn only_a_missing_column_switches_a_question_off() {
        use super::is_missing_column;
        assert!(is_missing_column(
            r#"Binder Error: Referenced column "rating" not found in FROM clause!"#
        ));
        assert!(is_missing_column(
            r#"Binder Error: Table "s" does not have a column named "rating""#
        ));
        assert!(!is_missing_column(
            "Catalog Error: Scalar Function with name anofox_x does not exist!"
        ));
        assert!(!is_missing_column(
            r#"Binder Error: Referenced column "x" not found (a setup statement failed first: …)"#
        ));
    }
}

// Tested end to end through a loaded extension (the C API exists only
// there): scripts/agent-served-e2e.mjs — a conforming mapping serves its
// canonical tables and a refresh rebuilds them; a refused one stops the
// start, or keeps the old snapshot on a refresh.
