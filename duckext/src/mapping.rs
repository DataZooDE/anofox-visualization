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

/// Build the canonical tables into `dir/canonical.duckdb`: `(name, file,
/// notes)` for the snapshot, or why the mapping is refused.
pub fn build(
    m: &MappingOptions,
    dir: &Path,
    stop: &Arc<AtomicBool>,
) -> Result<(String, PathBuf, Vec<String>), String> {
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
        let r = (|| -> Result<Vec<String>, String> {
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
            for t in &contract.tables {
                duck(&format!(
                    "CREATE TABLE {}.main.{} AS SELECT * FROM memory.main.{}",
                    qi(ATTACH_AS),
                    qi(&t.name),
                    qi(&t.name)
                ))?;
            }
            duck(&format!("DETACH {}", qi(ATTACH_AS)))?;
            Ok(report
                .findings
                .iter()
                .filter(|f| !f.error)
                .map(|f| format!("agent.mapping: {}: {}", f.table, f.message))
                .collect())
        })();
        finished.store(true, Ordering::SeqCst);
        r
    });
    drop(conn);
    drop(db);
    match result {
        Ok(notes) => Ok((ATTACH_AS.to_string(), out, notes)),
        Err(_) if timed_out.load(Ordering::SeqCst) => Err(format!(
            "agent.mapping: the build took longer than {} s (timeout_s) or the server stopped",
            m.timeout_s
        )),
        Err(e) => Err(e),
    }
}

// Tested end to end through a loaded extension (the C API exists only
// there): scripts/agent-served-e2e.mjs — a conforming mapping serves its
// canonical tables and a refresh rebuilds them; a refused one stops the
// start, or keeps the old snapshot on a refresh.
