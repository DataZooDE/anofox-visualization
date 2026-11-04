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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The name the built database is attached under in the snapshot (the
/// dashboard's bare canonical names resolve there).
pub const ATTACH_AS: &str = "canonical";

fn lit(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

fn qi(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

/// What a build gives the snapshot: `(name, file, notes)` like a source
/// build, and the agent's questions this mapping can't answer (#982).
type Built = ((String, PathBuf, Vec<String>), Vec<String>);

/// Build the canonical tables into `dir/canonical.duckdb`, or say why the
/// mapping is refused. `script` (the agent's dashboard): its questions are
/// bound against the views ([`canonical::questions`]); one whose panel
/// reads a column the mapping leaves out is off for this customer.
pub fn build(
    m: &MappingOptions,
    dir: &Path,
    stop: &Arc<AtomicBool>,
    script: Option<&str>,
) -> Result<Built, String> {
    let read = |p: &str| std::fs::read_to_string(p).map_err(|e| format!("agent.mapping: {p}: {e}"));
    let contract = canonical::parse_contract(&read(&m.contract)?)
        .map_err(|e| format!("agent.mapping.contract: {e}"))?;
    let mapping = canonical::parse_mapping(&read(&m.views)?, &contract)
        .map_err(|e| format!("agent.mapping.views: {}", e.join("; ")))?;
    let source =
        std::fs::canonicalize(&m.db).map_err(|e| format!("agent.mapping.db {}: {e}", m.db))?;
    let out = dir.join("canonical.duckdb");

    let db = Database::open(
        ":memory:",
        &[
            ("autoinstall_known_extensions", "false"),
            ("autoload_known_extensions", "false"),
            ("allow_community_extensions", "false"),
        ],
    )?;
    let conn = db.connect()?;
    let intr = conn.interrupter();
    let finished = AtomicBool::new(false);
    let deadline = Instant::now() + Duration::from_secs(m.timeout_s);
    let timed_out = AtomicBool::new(false);
    let result = std::thread::scope(|scope| {
        let (finished, timed_out) = (&finished, &timed_out);
        let stop = stop.clone();
        scope.spawn(move || {
            while !finished.load(Ordering::SeqCst) {
                if Instant::now() >= deadline || stop.load(Ordering::SeqCst) {
                    timed_out.store(true, Ordering::SeqCst);
                    intr.interrupt();
                    return;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        });
        let r = (|| -> Result<(Vec<String>, Vec<String>), String> {
            let mut duck = |sql: &str| -> Result<Vec<Vec<Option<String>>>, String> {
                conn.query(sql)?.string_rows(1_000_000)
            };
            // the source and the output first, then the bounds and the lock
            let setup = canonical::instance_setup(&source.to_string_lossy(), &m.memory_limit, 4);
            let (attach_source, rest) = setup.split_first().ok_or("no setup")?;
            duck(attach_source)?;
            duck(&format!(
                "ATTACH {} AS {}",
                lit(&out.to_string_lossy()),
                qi(ATTACH_AS)
            ))?;
            for s in rest {
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
                duck(&format!(
                    "CREATE TABLE {}.main.{} AS SELECT * FROM memory.main.{}",
                    qi(ATTACH_AS),
                    qi(&t.name),
                    qi(&t.name)
                ))?;
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
    match result {
        Ok((notes, off)) => Ok(((ATTACH_AS.to_string(), out, notes), off)),
        Err(_) if timed_out.load(Ordering::SeqCst) => Err(format!(
            "agent.mapping: the build took longer than {} s (timeout_s) or the server stopped",
            m.timeout_s
        )),
        Err(e) => Err(e),
    }
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
