//! Scheduled tasks (#767, docs/plans/plan-tasks-reports.md §1): `*.task.sql`
//! files in `<dashboards dir>/tasks/`, run by the locked server's task
//! runner. This module is the part without a database: reading a task file
//! (its `::SCHEDULE`, its statements, `::REFRESH`) and the statement guard.
//!
//! ```sql
//! SELECT INTERVAL '1 hour' ::SCHEDULE;      -- or a TIMESTAMP, or 'init'
//! CREATE OR REPLACE TABLE stage.downloads AS SELECT * FROM read_parquet('/data/inbox/*.parquet');
//! SELECT 1 ::REFRESH;                        -- rebuild the snapshot when done
//! ```

use crate::sql::lex::{first_keyword, strip_comments, tokenize, TokKind};

/// One task file.
#[derive(Debug, Clone, PartialEq)]
pub struct Task {
    /// The file name without `.task.sql`.
    pub name: String,
    /// The `::SCHEDULE` expression (SQL), evaluated by the runner.
    pub schedule: String,
    /// The statements to run, in order (the schedule and `::REFRESH` lines
    /// are not among them).
    pub statements: Vec<String>,
    /// A `::REFRESH` statement: rebuild the snapshot after a successful run.
    pub refresh: bool,
}

/// `<expr> ::NAME` (a trailing cast to a marker word) → `<expr>`.
fn marker(stmt: &str, name: &str) -> Option<String> {
    let s = stmt.trim().trim_end_matches(';').trim();
    let body = s
        .strip_prefix("SELECT")
        .or_else(|| s.strip_prefix("select"))?;
    let (expr, tail) = body.rsplit_once("::")?;
    tail.trim()
        .eq_ignore_ascii_case(name)
        .then(|| expr.trim().to_string())
        .filter(|e| !e.is_empty())
}

/// Read a task file. Errors name the problem (they are shown at start and by
/// the lint).
pub fn parse(name: &str, script: &str) -> Result<Task, String> {
    let stmts: Vec<String> = crate::sql::split_statements(script)
        .into_iter()
        .map(|s| strip_comments(&s).trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    let Some(first) = stmts.first() else {
        return Err(format!("task '{name}': empty"));
    };
    let schedule = marker(first, "SCHEDULE").ok_or_else(|| {
        format!("task '{name}': the first statement must be `SELECT <interval | timestamp | 'init'> ::SCHEDULE`")
    })?;
    let mut statements = Vec::new();
    let mut refresh = false;
    for s in &stmts[1..] {
        if marker(s, "REFRESH").is_some() {
            refresh = true;
            continue;
        }
        if marker(s, "SCHEDULE").is_some() {
            return Err(format!(
                "task '{name}': only the first statement may be a ::SCHEDULE"
            ));
        }
        guard(s).map_err(|e| format!("task '{name}': {e}"))?;
        statements.push(s.clone());
    }
    Ok(Task {
        name: name.to_string(),
        schedule,
        statements,
        refresh,
    })
}

/// Refuse a statement a task may not run: it would change the shared
/// connection or the server (settings, the default database, attachments,
/// extensions, temp objects), or write a file. Where it may write data is
/// enforced by DuckDB (the serving session's `READ_ONLY` attachments,
/// checked when the runner starts); this is the second line.
pub fn guard(stmt: &str) -> Result<(), String> {
    let kw = first_keyword(stmt).unwrap_or_default().to_ascii_uppercase();
    let words: Vec<String> = tokenize(stmt)
        .iter()
        .filter(|t| t.kind == TokKind::Word)
        .take(4)
        .map(|t| t.text(stmt).to_ascii_uppercase())
        .collect();
    let refused = match kw.as_str() {
        "SET" | "RESET" | "PRAGMA" | "USE" | "ATTACH" | "DETACH" | "INSTALL" | "LOAD" | "FORCE"
        | "EXPORT" | "IMPORT" | "CHECKPOINT" | "VACUUM" => Some(kw.as_str()),
        "CREATE" if words.iter().any(|w| w == "TEMP" || w == "TEMPORARY") => Some("CREATE TEMP"),
        "CREATE" if words.iter().any(|w| w == "SECRET") => Some("CREATE SECRET"),
        "COPY" if has_word(stmt, "TO") => Some("COPY … TO"),
        _ => None,
    };
    match refused {
        Some(what) => Err(format!(
            "`{what}` is not allowed in a task (it would change the server's own session, or write a file)"
        )),
        None => Ok(()),
    }
}

/// Does `stmt` have the keyword `w` outside strings and comments?
fn has_word(stmt: &str, w: &str) -> bool {
    tokenize(stmt)
        .iter()
        .any(|t| t.kind == TokKind::Word && t.text(stmt).eq_ignore_ascii_case(w))
}

/// The databases the runner must find `READ_ONLY` (`(name, readonly)` from
/// `duckdb_databases()`, non-internal): every one not in `write`. An error
/// names the first that is attached read-write.
pub fn check_attachments(dbs: &[(String, bool)], write: &[String]) -> Result<(), String> {
    for (name, readonly) in dbs {
        let writable = write.iter().any(|w| w.eq_ignore_ascii_case(name));
        if !writable && !readonly {
            return Err(format!(
                "options.tasks: database '{name}' is attached read-write but not in tasks.write — attach it READ_ONLY, or list it in tasks.write"
            ));
        }
    }
    for w in write {
        if !dbs.iter().any(|(n, _)| n.eq_ignore_ascii_case(w)) {
            return Err(format!(
                "options.tasks.write: no database '{w}' is attached"
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_task() {
        let t = parse(
            "load",
            "-- load the inbox\nSELECT INTERVAL '1 hour' ::SCHEDULE;\n\
             CREATE OR REPLACE TABLE stage.d AS SELECT 1 AS x;\n\
             DELETE FROM stage.d WHERE x > 1;\nSELECT 1 ::REFRESH;",
        )
        .unwrap();
        assert_eq!(t.schedule, "INTERVAL '1 hour'");
        assert_eq!(t.statements.len(), 2);
        assert!(t.refresh);
        let t = parse(
            "i",
            "select 'init'::schedule; INSERT INTO stage.t VALUES (1)",
        )
        .unwrap();
        assert_eq!((t.schedule.as_str(), t.refresh), ("'init'", false));
    }

    #[test]
    fn refuses_bad_tasks() {
        for (src, why) in [
            ("", "empty"),
            ("CREATE TABLE stage.t (x INT);", "first statement"),
            (
                "SELECT 'init'::SCHEDULE; SELECT 'init'::SCHEDULE;",
                "only the first",
            ),
            ("SELECT 'init'::SCHEDULE; SET threads = 1;", "`SET`"),
            ("SELECT 'init'::SCHEDULE; SET VARIABLE x = 1;", "`SET`"),
            ("SELECT 'init'::SCHEDULE; USE stage;", "`USE`"),
            ("SELECT 'init'::SCHEDULE; ATTACH 'x.db' AS x;", "`ATTACH`"),
            ("SELECT 'init'::SCHEDULE; INSTALL httpfs;", "`INSTALL`"),
            (
                "SELECT 'init'::SCHEDULE; CREATE TEMP TABLE t AS SELECT 1;",
                "CREATE TEMP",
            ),
            (
                "SELECT 'init'::SCHEDULE; CREATE OR REPLACE TEMPORARY VIEW v AS SELECT 1;",
                "CREATE TEMP",
            ),
            (
                "SELECT 'init'::SCHEDULE; CREATE SECRET s (TYPE s3);",
                "CREATE SECRET",
            ),
            (
                "SELECT 'init'::SCHEDULE; COPY stage.t TO '/tmp/x.csv';",
                "COPY … TO",
            ),
            (
                "SELECT 'init'::SCHEDULE; EXPORT DATABASE '/tmp/x';",
                "`EXPORT`",
            ),
            (
                "SELECT 'init'::SCHEDULE; PRAGMA enable_profiling;",
                "`PRAGMA`",
            ),
        ] {
            let e = parse("t", src).unwrap_err();
            assert!(e.contains(why), "{src}: {e}");
        }
        // reading a file, and COPY … FROM, are fine
        assert!(parse("t", "SELECT 'init'::SCHEDULE; COPY stage.t FROM '/data/x.csv'; CREATE TABLE stage.y AS SELECT 'TO' AS c;").is_ok());
    }

    #[test]
    fn attachments() {
        let dbs = vec![
            ("memory".to_string(), true),
            ("stage".to_string(), false),
            ("warehouse".to_string(), true),
        ];
        assert!(check_attachments(&dbs, &["stage".into()]).is_ok());
        let e = check_attachments(&dbs, &[]).unwrap_err();
        assert!(e.contains("'stage' is attached read-write"), "{e}");
        let e = check_attachments(&dbs, &["stage".into(), "nope".into()]).unwrap_err();
        assert!(e.contains("no database 'nope'"), "{e}");
    }
}
