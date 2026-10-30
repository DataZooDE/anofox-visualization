//! `-- @ask` marks and Ask's exclusion list (#745, plan-ai-assistant.md
//! §12.15 D1 / D3): the panels Ask's `get_panel` never sends to the model
//! provider, decided from the script alone, before `locked_panel` (I2).
//!
//! ```sql
//! -- @ask skip                (this panel stays out of Ask)
//! SELECT name::TABLE, salary FROM staff;
//!
//! -- @ask include             (opt a viewer_state panel in)
//! SELECT count(*)::METRIC FROM viewer_state_latest;
//! ```
//!
//! A statement is excluded when it
//! * carries `-- @ask skip`;
//! * is a write-back statement — `::SAVE`, `::SUBMIT` or `::FORM` (D3);
//! * reads `viewer_state` / `viewer_state_latest` (D1), unless it carries
//!   `-- @ask include`. "Reads" is conservative: the name anywhere in the
//!   statement (a string literal too, e.g. `query('…')`), or a view, table,
//!   macro or variable an earlier statement built from it (`CREATE`,
//!   `INSERT INTO`, `SET VARIABLE`). Any other setup statement that reads it
//!   and is not a plain query taints every later statement.
//!
//! Statements are numbered like [`super::plan`] (index into its panels).
//! The dashboard-level `-- @ask on` in the header is read by M2-3.

use super::lex::{tokenize, TokKind};
use super::Panel;
use crate::{InputKind, Role};

/// The marks a statement may carry; `on` / `off` only in the header (the
/// first statement), and `mask col,…` is §12.7's (read by M2-3).
pub const MARKS: &[&str] = &["skip", "include", "mask"];

/// Why a statement is on the exclusion list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// `-- @ask skip`.
    Skip,
    /// `::SAVE` / `::SUBMIT` / `::FORM`: not a panel to read (D3).
    WriteBack,
    /// Reads `viewer_state` (D1); `-- @ask include` opts it in.
    ViewerState,
}

impl Reason {
    pub fn as_str(self) -> &'static str {
        match self {
            Reason::Skip => "@ask skip",
            Reason::WriteBack => "write-back",
            Reason::ViewerState => "viewer_state",
        }
    }
}

/// The `-- @ask <word> [rest]` lines of one raw statement, in order, as
/// `(word lower-cased, rest trimmed)`.
pub fn marks(raw: &str) -> Vec<(String, String)> {
    raw.lines()
        .filter_map(|l| l.trim().strip_prefix("--"))
        .filter_map(|l| l.trim().strip_prefix("@ask"))
        .filter(|rest| rest.is_empty() || rest.starts_with(char::is_whitespace))
        .map(|rest| {
            let rest = rest.trim();
            let (word, tail) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
            (word.to_ascii_lowercase(), tail.trim().to_string())
        })
        .collect()
}

/// The raw text of each statement [`super::plan`] keeps (empty ones after
/// stripping comments dropped), so index `i` is `plan(script)[i]`.
pub fn raw_statements(script: &str) -> Vec<String> {
    super::split_statements(script)
        .into_iter()
        .filter(|raw| !super::strip_comments(raw).trim().is_empty())
        .collect()
}

/// Is this a write-back statement (D3)?
pub fn is_write_back(p: &Panel) -> bool {
    p.roles.iter().any(|(_, r)| {
        matches!(
            r,
            Role::Save | Role::SaveForm | Role::Submit | Role::Input(InputKind::Form)
        )
    })
}

/// Per statement of `plan`: does it read `viewer_state`, directly or
/// through an earlier statement (see the module docs)?
pub fn reads_viewer_state(plan: &[Panel]) -> Vec<bool> {
    let mut tainted_names: Vec<String> = Vec::new();
    let mut all = false;
    plan.iter()
        .map(|p| {
            let sql = &p.sql;
            let lower = sql.to_ascii_lowercase();
            let hit = all
                || lower.contains("viewer_state")
                || tokenize(sql).iter().any(|t| {
                    let text = t.text(sql);
                    let name = match t.kind {
                        TokKind::Word => text.to_ascii_lowercase(),
                        TokKind::QuotedIdent | TokKind::String => text
                            .trim_start_matches(['E', 'e'])
                            .trim_matches(['"', '\''])
                            .to_ascii_lowercase(),
                        _ => return false,
                    };
                    tainted_names.contains(&name)
                });
            if hit && p.setup {
                match created_name(sql) {
                    Some(n) => tainted_names.push(n),
                    None if only_reads(sql) => {}
                    None => all = true,
                }
            }
            hit
        })
        .collect()
}

/// The statement indexes (into [`super::plan`]) Ask must not read, with
/// why — panels only (setup statements are never a `get_panel` target).
pub fn exclusions(script: &str) -> Vec<(usize, Reason)> {
    let plan = super::plan(script);
    let raws = raw_statements(script);
    let state = reads_viewer_state(&plan);
    plan.iter()
        .enumerate()
        .filter(|(_, p)| !p.setup)
        .filter_map(|(i, p)| {
            let m = raws.get(i).map(|r| marks(r)).unwrap_or_default();
            let has = |w: &str| m.iter().any(|(word, _)| word == w);
            if has("skip") {
                Some((i, Reason::Skip))
            } else if is_write_back(p) {
                Some((i, Reason::WriteBack))
            } else if state[i] && !has("include") {
                Some((i, Reason::ViewerState))
            } else {
                None
            }
        })
        .collect()
}

/// Leading words of a statement, lower-cased, comments and whitespace
/// skipped.
fn words(sql: &str, n: usize) -> Vec<(TokKind, String)> {
    tokenize(sql)
        .into_iter()
        .filter(|t| !t.is_trivia())
        .take(n)
        .map(|t| (t.kind, t.text(sql).to_string()))
        .collect()
}

/// A statement that only reads (leaves nothing behind for a later one).
fn only_reads(sql: &str) -> bool {
    matches!(
        words(sql, 1)
            .first()
            .map(|(_, w)| w.to_ascii_lowercase())
            .as_deref(),
        Some("select" | "with" | "from" | "values" | "table" | "describe" | "summarize" | "show")
    )
}

/// The object a setup statement creates or writes: `CREATE [OR REPLACE]
/// [TEMP | TEMPORARY] {TABLE | VIEW | MACRO | FUNCTION} [IF NOT EXISTS]
/// name`, `INSERT [OR REPLACE] INTO name` or `SET VARIABLE name` —
/// lower-cased, without a schema.
fn created_name(sql: &str) -> Option<String> {
    let w = words(sql, 14);
    let mut it = w.into_iter().map(|(k, w)| (k, w.to_ascii_lowercase()));
    let ident = |(k, w): (TokKind, String)| match k {
        TokKind::Word if w != "as" => Some(w),
        TokKind::QuotedIdent => Some(w.trim_matches('"').to_string()),
        _ => None,
    };
    let mut name = match it.next()?.1.as_str() {
        "set" => {
            if it.next()?.1 != "variable" {
                return None;
            }
            return it.next().and_then(ident);
        }
        "insert" => {
            let mut w = it.next()?;
            if w.1 == "or" {
                it.next()?; // replace / ignore
                w = it.next()?;
            }
            if w.1 != "into" {
                return None;
            }
            it.next()?
        }
        "create" => {
            let mut w = it.next()?;
            if w.1 == "or" {
                it.next()?; // replace
                w = it.next()?;
            }
            if matches!(w.1.as_str(), "temp" | "temporary") {
                w = it.next()?;
            }
            if !matches!(w.1.as_str(), "table" | "view" | "macro" | "function") {
                return None;
            }
            let mut n = it.next()?;
            if n.1 == "if" {
                it.next()?; // not
                it.next()?; // exists
                n = it.next()?;
            }
            n
        }
        _ => return None,
    };
    // `schema.name` / `db.schema.name`: the last part.
    loop {
        let part = ident(name)?;
        match it.next() {
            Some((TokKind::Punct, d)) if d == "." => name = it.next()?,
            _ => return Some(part),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn excl(script: &str) -> Vec<(usize, Reason)> {
        exclusions(script)
    }

    #[test]
    fn parses_marks() {
        assert_eq!(
            marks("-- @ask skip\n--  @ask  mask name, email\nSELECT 1;"),
            [
                ("skip".to_string(), String::new()),
                ("mask".to_string(), "name, email".to_string())
            ]
        );
        assert!(marks("-- @asked skip\n-- ask skip\nSELECT 1;").is_empty());
        assert_eq!(marks("-- @ask\n"), [(String::new(), String::new())]);
    }

    #[test]
    fn skip_and_write_back() {
        let s = "SELECT 1::METRIC;\n-- @ask skip\nSELECT 2::METRIC;\n\
                 SELECT 'Save'::SAVE;\nSELECT 'Send'::SUBMIT;\nSELECT 3::METRIC;";
        assert_eq!(
            excl(s),
            [
                (1, Reason::Skip),
                (2, Reason::WriteBack),
                (3, Reason::WriteBack)
            ]
        );
    }

    #[test]
    fn viewer_state_direct_and_include() {
        let s = "SELECT count(*)::METRIC FROM viewer_state_latest;\n\
                 -- @ask include\nSELECT count(*)::METRIC FROM viewer_state;\n\
                 -- @ask include\n-- @ask skip\nSELECT 1::METRIC FROM VIEWER_STATE;\n\
                 SELECT n::METRIC FROM query('SELECT count(*) n FROM viewer_state');\n\
                 SELECT 1::METRIC;";
        assert_eq!(
            excl(s),
            [
                (0, Reason::ViewerState),
                (2, Reason::Skip),
                (3, Reason::ViewerState)
            ]
        );
    }

    #[test]
    fn viewer_state_through_setup() {
        let s = "CREATE OR REPLACE TEMP VIEW answers AS SELECT * FROM viewer_state;\n\
                 CREATE TEMP TABLE IF NOT EXISTS main.agg AS SELECT count(*) n FROM answers;\n\
                 SET VARIABLE last = (SELECT max(ts) FROM viewer_state);\n\
                 SELECT n::METRIC FROM agg;\n\
                 SELECT getvariable('last')::TEXT_SMALL;\n\
                 SELECT 1::METRIC FROM \"Answers\";\n\
                 SELECT 2::METRIC FROM sales;";
        assert_eq!(
            excl(s),
            [
                (3, Reason::ViewerState),
                (4, Reason::ViewerState),
                (5, Reason::ViewerState)
            ]
        );
    }

    #[test]
    fn unnamed_tainted_setup_taints_everything_after() {
        let s = "SELECT 1::METRIC FROM sales;\n\
                 COPY (SELECT * FROM viewer_state) TO 'x.csv';\n\
                 SELECT 2::METRIC FROM sales;";
        assert_eq!(excl(s), [(2, Reason::ViewerState)]);
        // a plain query setup statement creates nothing
        let s = "SELECT * FROM viewer_state;\nSELECT 2::METRIC FROM sales;";
        assert!(excl(s).is_empty());
    }

    #[test]
    fn numbering_skips_empty_statements() {
        let s = "-- @title X\n;\n-- only a comment\n;\nSELECT 1::METRIC FROM viewer_state;";
        assert_eq!(excl(s), [(0, Reason::ViewerState)]);
        assert_eq!(raw_statements(s).len(), super::super::plan(s).len());
    }

    #[test]
    fn created_names() {
        assert_eq!(
            created_name("create view v as select 1").as_deref(),
            Some("v")
        );
        assert_eq!(created_name("CREATE MACRO m(x) AS x").as_deref(), Some("m"));
        assert_eq!(
            created_name("CREATE TABLE s.t AS SELECT 1").as_deref(),
            Some("t")
        );
        assert_eq!(
            created_name("CREATE TABLE \"T x\" AS SELECT 1").as_deref(),
            Some("t x")
        );
        assert_eq!(created_name("SET VARIABLE v = 1").as_deref(), Some("v"));
        assert_eq!(
            created_name("INSERT INTO main.t SELECT 1").as_deref(),
            Some("t")
        );
        assert_eq!(created_name("CREATE VIEW AS SELECT 1"), None);
        assert_eq!(created_name("SET threads = 1"), None);
        assert_eq!(created_name("CREATE INDEX i ON t(x)"), None);
    }
}
