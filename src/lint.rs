//! Lint an annotated dashboard script — the feedback an author (human or AI)
//! needs to self-correct.
//!
//! Dashboards fail *silently*: a panel that starts with `WITH` loses its
//! `::ROLE` casts and is quietly treated as a setup statement (no panel, no
//! error); a filter that returns no rows draws a blank card. The linter runs
//! each statement and reports these, so a generate → validate → repair loop can
//! fix them instead of shipping a broken dashboard.
//!
//! It is transport-agnostic: pass a `run_query` closure that executes SQL
//! against a **stateful** connection (setup persists for later panels) and
//! returns the rows, or a DuckDB error string.

use crate::{render, sql, Role};
use serde_json::{Map, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

impl Severity {
    pub fn label(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub severity: Severity,
    /// 1-based statement index in the script.
    pub stmt: usize,
    /// Stable machine code: `sql-error` | `silent-setup` | `render-error` | `empty-panel`.
    pub code: &'static str,
    pub message: String,
    /// The offending statement, trimmed to one line.
    pub sql: String,
}

/// Lint `script`. `run_query` runs a statement against a stateful connection
/// and returns its rows (empty on a successful non-SELECT), or a DuckDB error.
pub fn check<F>(script: &str, mut run_query: F) -> Vec<Diagnostic>
where
    F: FnMut(&str) -> Result<Vec<Map<String, Value>>, String>,
{
    let mut diags = Vec::new();
    for (i, p) in sql::plan(script).into_iter().enumerate() {
        let stmt = i + 1;
        let short = one_line(p.sql.trim());

        // Setup statements: a query-shaped one that still carries ::ROLE casts
        // is a panel that lost its roles (the #1 silent failure) — flag it, and
        // don't run it (the casts would just raise a confusing type error).
        // Genuine setup (DDL / SET / …) is run so later panels see its effects.
        if p.setup {
            if looks_like_query(&p.sql) && has_role_cast(&p.sql) {
                diags.push(Diagnostic {
                    severity: Severity::Error,
                    stmt,
                    code: "silent-setup",
                    message: "this looks like a chart panel but its ::ROLE casts weren't \
                              recognised, so it renders nothing. A panel must NOT start with \
                              WITH (the detector keys off the first SELECT) — move the CTE \
                              into a FROM (SELECT …) subquery, or put the ::ROLE casts on the \
                              outer SELECT list."
                        .into(),
                    sql: short,
                });
            } else if let Err(e) = run_query(&p.sql) {
                diags.push(Diagnostic {
                    severity: Severity::Error,
                    stmt,
                    code: "sql-error",
                    message: one_line(&e),
                    sql: short,
                });
            }
            continue;
        }

        // Panel: run it, then check the rows and the render.
        let rows = match run_query(&p.sql) {
            Ok(rows) => rows,
            Err(e) => {
                diags.push(Diagnostic {
                    severity: Severity::Error,
                    stmt,
                    code: "sql-error",
                    message: one_line(&e),
                    sql: short,
                });
                continue;
            }
        };

        // Inputs and layout directives don't render a chart — nothing to check.
        if p.roles.iter().any(|(_, r)| is_non_render(r)) {
            continue;
        }
        // A lone ::LABEL is a section heading, not a card.
        if p.roles.len() == 1 && matches!(p.roles[0].1, Role::Label) {
            continue;
        }

        if rows.is_empty() {
            diags.push(Diagnostic {
                severity: Severity::Warning,
                stmt,
                code: "empty-panel",
                message: "query returned 0 rows — this panel will be blank. Check WHERE \
                          filters and getvariable() defaults (a multiselect with nothing \
                          selected filters everything out unless you guard with len()=0)."
                    .into(),
                sql: short,
            });
            continue;
        }

        // Let the render engine validate the role/column combination (this
        // catches missing required aesthetics, e.g. a bar chart with no x).
        let cols = sql::columns_from_rows(&rows, &p.roles);
        if let Err(e) = render(&cols, 460, 300) {
            diags.push(Diagnostic {
                severity: Severity::Error,
                stmt,
                code: "render-error",
                message: one_line(&e),
                sql: short,
            });
        }
    }
    diags
}

/// A one-line summary: `N error(s), M warning(s)`; empty script-clean message.
pub fn summary(diags: &[Diagnostic]) -> String {
    let e = diags.iter().filter(|d| d.severity == Severity::Error).count();
    let w = diags.len() - e;
    if diags.is_empty() {
        "clean — every panel plans, runs, and renders".into()
    } else {
        format!("{e} error(s), {w} warning(s)")
    }
}

fn looks_like_query(sql: &str) -> bool {
    let u = sql.trim_start().to_ascii_uppercase();
    u.starts_with("SELECT") || u.starts_with("WITH") || u.starts_with('(') || u.starts_with("FROM ")
}

/// Does the statement carry a `::ROLE` cast (a recognised role token, not a SQL
/// type cast like `::INT`)? Distinguishes a lost-roles panel from real setup.
fn has_role_cast(sql: &str) -> bool {
    let b = sql.as_bytes();
    let mut i = 0;
    while i + 1 < b.len() {
        if b[i] == b':' && b[i + 1] == b':' {
            let mut j = i + 2;
            while j < b.len() && b[j] == b' ' {
                j += 1;
            }
            let start = j;
            while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'_') {
                j += 1;
            }
            if j > start && crate::parse_role(&sql[start..j].to_ascii_uppercase()).is_some() {
                return true;
            }
            i = j.max(i + 2);
        } else {
            i += 1;
        }
    }
    false
}

fn is_non_render(r: &Role) -> bool {
    matches!(
        r,
        Role::Input(_)
            | Role::Columns
            | Role::GroupStart
            | Role::GroupEnd
            | Role::Span
            | Role::Height
            | Role::Tab
            | Role::SubTab
            | Role::Placeholder
    )
}

fn one_line(s: &str) -> String {
    let s: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if s.chars().count() > 140 {
        format!("{}…", s.chars().take(139).collect::<String>())
    } else {
        s
    }
}
