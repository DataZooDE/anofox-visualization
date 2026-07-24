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

use crate::{render, sql, InputKind, Kind, Role};
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
    /// Stable machine code. Correctness: `sql-error` | `silent-setup` |
    /// `render-error` | `empty-panel` | `unknown-cast`. Design advisories
    /// (prefix `design/`): `pie-slices` | `unsorted-bars` | `untitled-chart` |
    /// `many-series` | `too-many-panels` | `ungrouped-kpis` | `raw-table`.
    pub code: &'static str,
    pub message: String,
    /// The offending statement, trimmed to one line.
    pub sql: String,
}

/// Options for [`check_opts`].
#[derive(Debug, Clone, Copy)]
pub struct LintOptions {
    /// Also emit `design/*` advisory warnings (layout / chart-choice quality).
    /// On by default; the CLI's `--no-design` turns it off.
    pub design: bool,
}

impl Default for LintOptions {
    fn default() -> Self {
        LintOptions { design: true }
    }
}

/// Lint `script`. `run_query` runs a whole SQL script in a **fresh** session and
/// returns the last statement's rows (empty on a non-SELECT), or a DuckDB error.
///
/// The linter replays an accumulated `prelude` (setup + input `SET`s — none of
/// which emit rows) before each statement, so session state (temp views, bound
/// variables) is faithfully in scope without needing a long-lived connection.
pub fn check<F>(script: &str, run_query: F) -> Vec<Diagnostic>
where
    F: FnMut(&str) -> Result<Vec<Map<String, Value>>, String>,
{
    check_opts(script, run_query, LintOptions::default())
}

/// Like [`check`], but with [`LintOptions`] (e.g. to disable the `design/*`
/// advisory pass).
pub fn check_opts<F>(script: &str, mut run_query: F, opts: LintOptions) -> Vec<Diagnostic>
where
    F: FnMut(&str) -> Result<Vec<Map<String, Value>>, String>,
{
    let mut diags = Vec::new();
    let mut prelude = String::new();
    // Cross-panel design counters (only used when opts.design).
    let mut rendered_panels = 0usize; // visible panels (charts/tables/kpis/text/markdown)
    let mut has_tabs = false;
    let mut group_depth = 0i32;
    let mut kpi_run = 0usize; // consecutive top-level KPI tiles
    let mut kpi_run_stmt = 0usize; // stmt index where the current run started
                                   // The original statements (with casts intact) — plan() rewrites panels, so we
                                   // scan these for typo'd roles. Same non-empty order as plan(), so indices align.
    let raws: Vec<String> = sql::split_statements(&sql::strip_line_comments(script))
        .into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    for (i, p) in sql::plan(script).into_iter().enumerate() {
        let stmt = i + 1;
        let short = one_line(p.sql.trim());

        // A mistyped role token (::BARCHRT) silently drops that column's role.
        if let Some(raw) = raws.get(i) {
            for tok in unknown_casts(raw) {
                diags.push(Diagnostic {
                    severity: Severity::Warning,
                    stmt,
                    code: "unknown-cast",
                    message: format!(
                        "`::{tok}` is not a role or a known type — likely a mistyped ::ROLE, \
                         so that column won't be mapped. See `dashboard --roles`."
                    ),
                    sql: short.clone(),
                });
            }
        }

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
                    sql: short.clone(),
                });
            } else if let Err(e) = run_query(&format!("{prelude}{};", p.sql)) {
                diags.push(Diagnostic {
                    severity: Severity::Error,
                    stmt,
                    code: "sql-error",
                    message: one_line(&e),
                    sql: short.clone(),
                });
            } else {
                // Genuine setup succeeded — keep it in scope for later panels.
                prelude.push_str(&p.sql);
                prelude.push_str(";\n");
            }
            continue;
        }

        // Input panel: run its option query and prime the variable(s) to a
        // default (into the prelude), so downstream getvariable() panels aren't
        // spuriously empty — mirrors the browser's input pre-pass.
        if let Some(kind) = input_kind(&p.roles) {
            match run_query(&format!("{prelude}{};", p.sql)) {
                Ok(rows) => {
                    for setv in input_defaults(kind, &rows) {
                        prelude.push_str(&setv);
                        prelude.push_str(";\n");
                    }
                }
                Err(e) => diags.push(Diagnostic {
                    severity: Severity::Error,
                    stmt,
                    code: "sql-error",
                    message: one_line(&e),
                    sql: short.clone(),
                }),
            }
            continue;
        }

        // Panel: run it (with the prelude in scope), then check rows and render.
        let rows = match run_query(&format!("{prelude}{};", p.sql)) {
            Ok(rows) => rows,
            Err(e) => {
                diags.push(Diagnostic {
                    severity: Severity::Error,
                    stmt,
                    code: "sql-error",
                    message: one_line(&e),
                    sql: short.clone(),
                });
                continue;
            }
        };

        // ---- design: cross-panel layout counters. Run for every planned panel,
        // including the layout directives handled just below. ----
        if opts.design {
            for (_, r) in &p.roles {
                match r {
                    Role::GroupStart => group_depth += 1,
                    Role::GroupEnd => group_depth = (group_depth - 1).max(0),
                    Role::Tab | Role::SubTab => has_tabs = true,
                    _ => {}
                }
            }
        }

        // Layout directives don't render a chart — nothing to check.
        if p.roles.iter().any(|(_, r)| is_non_render(r)) {
            continue;
        }
        // A lone ::LABEL is a section heading, not a card. It also ends a run of
        // KPI tiles (they should be grouped within, not across, a heading).
        if p.roles.len() == 1 && matches!(p.roles[0].1, Role::Label) {
            if opts.design {
                flush_kpi_run(&mut diags, &mut kpi_run, kpi_run_stmt);
            }
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
                sql: short.clone(),
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
                sql: short.clone(),
            });
        }

        // ---- design: per-panel advisory checks (see docs/dashboard-design.md) ----
        if opts.design {
            // Count content panels (charts/tables/markdown) toward the
            // scan-budget, but NOT individual KPI tiles — a strip of KPIs is one
            // band to scan, not N panels.
            if is_kpi(&p.roles) {
                if group_depth == 0 {
                    if kpi_run == 0 {
                        kpi_run_stmt = stmt;
                    }
                    kpi_run += 1;
                }
            } else {
                rendered_panels += 1;
                // A non-KPI content panel ends a run of top-level KPI tiles.
                flush_kpi_run(&mut diags, &mut kpi_run, kpi_run_stmt);
            }

            if let Some(k) = chart_kind(&p.roles) {
                let titled = p.roles.iter().any(|(_, r)| matches!(r, Role::Title));
                if is_big_chart(k) && !titled {
                    diags.push(Diagnostic {
                        severity: Severity::Warning,
                        stmt,
                        code: "design/untitled-chart",
                        message: "chart has no ::TITLE — add a title that states the takeaway \
                                  ('Revenue up 12% YoY', not just 'Revenue'). See \
                                  docs/dashboard-design.md."
                            .into(),
                        sql: short.clone(),
                    });
                }
                if matches!(k, Kind::Pie | Kind::Donut) && rows.len() > 6 {
                    diags.push(Diagnostic {
                        severity: Severity::Warning,
                        stmt,
                        code: "design/pie-slices",
                        message: format!(
                            "pie/donut with {} slices — angles past ~6 are hard to compare; \
                             use a sorted ::BARCHART instead.",
                            rows.len()
                        ),
                        sql: short.clone(),
                    });
                }
                let has_cat = p.roles.iter().any(|(_, r)| matches!(r, Role::Category));
                if matches!(k, Kind::Bar) && !has_cat && rows.len() >= 3 {
                    if let (Some(xi), Some(mi)) =
                        (role_index(&p.roles, Role::X), value_index(&p.roles))
                    {
                        if col_is_discrete(&rows, xi)
                            && !col_is_temporal(&rows, xi)
                            && !col_is_monotonic(&rows, mi)
                        {
                            diags.push(Diagnostic {
                                severity: Severity::Warning,
                                stmt,
                                code: "design/unsorted-bars",
                                message: "bars aren't sorted by value — order the query by the \
                                          measure (ORDER BY 2 DESC) so the ranking reads at a \
                                          glance. See docs/dashboard-design.md."
                                    .into(),
                                sql: short.clone(),
                            });
                        }
                    }
                }
                if matches!(
                    k,
                    Kind::Bar | Kind::BarStacked | Kind::Line | Kind::Area | Kind::AreaStacked
                ) {
                    if let Some(ci) = role_index(&p.roles, Role::Category) {
                        let n = distinct_count(&rows, ci);
                        if n > 7 {
                            diags.push(Diagnostic {
                                severity: Severity::Warning,
                                stmt,
                                code: "design/many-series",
                                message: format!(
                                    "{n} colour series — past ~7 the legend is unreadable; keep \
                                     the top few and group the rest as 'Other', or use small \
                                     multiples."
                                ),
                                sql: short.clone(),
                            });
                        }
                    }
                }
            }

            // ::TABLE paginates the *view* client-side but loads every row into
            // the browser — a few hundred is a heavy payload and usually a raw
            // dump. (Only flag genuinely large results.)
            let is_plain_table = p.roles.iter().any(|(_, r)| matches!(r, Role::Table));
            if is_plain_table && rows.len() > 200 {
                diags.push(Diagnostic {
                    severity: Severity::Warning,
                    stmt,
                    code: "design/raw-table",
                    message: format!(
                        "::TABLE loads all {} rows into the browser — cut it to a top-N \
                         (ORDER BY … LIMIT) or aggregate. Use ::PAGED only for very large / \
                         remote sources.",
                        rows.len()
                    ),
                    sql: short.clone(),
                });
            }
        }
    }

    // ---- design: end-of-script checks ----
    if opts.design {
        flush_kpi_run(&mut diags, &mut kpi_run, kpi_run_stmt);
        if rendered_panels > 8 && !has_tabs {
            diags.push(Diagnostic {
                severity: Severity::Warning,
                stmt: 0,
                code: "design/too-many-panels",
                message: format!(
                    "{rendered_panels} panels on one page with no ::TAB — a reader can't scan it. \
                     Split topics across ::TAB pages (aim for ≤ ~7 per view)."
                ),
                sql: String::new(),
            });
        }
    }

    diags
}

/// A one-line summary: `N error(s), M warning(s)`; empty script-clean message.
pub fn summary(diags: &[Diagnostic]) -> String {
    let e = diags
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .count();
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

/// DuckDB scalar types that are *not* role tokens — so a `::TYPE` cast to one of
/// these is legitimate SQL, not a mistyped role. (Types that are also roles —
/// TEXT, DATE, NUMBER, MAP — parse as roles first and never reach here.)
const KNOWN_TYPES: &[&str] = &[
    "INT",
    "INTEGER",
    "INT1",
    "INT2",
    "INT4",
    "INT8",
    "TINYINT",
    "SMALLINT",
    "BIGINT",
    "HUGEINT",
    "UTINYINT",
    "USMALLINT",
    "UINTEGER",
    "UBIGINT",
    "UHUGEINT",
    "FLOAT",
    "FLOAT4",
    "FLOAT8",
    "REAL",
    "DOUBLE",
    "DECIMAL",
    "NUMERIC",
    "VARCHAR",
    "CHAR",
    "BPCHAR",
    "BLOB",
    "BYTEA",
    "BOOL",
    "BOOLEAN",
    "BIT",
    "TIMESTAMP",
    "TIMESTAMPTZ",
    "TIMESTAMP_S",
    "TIMESTAMP_MS",
    "TIMESTAMP_NS",
    "TIME",
    "TIMETZ",
    "INTERVAL",
    "UUID",
    "JSON",
    "STRUCT",
    "LIST",
    "ARRAY",
    "ENUM",
    "NULL",
];

/// Trailing `::TOKEN` casts that are neither a recognised role nor a known SQL
/// type — i.e. probable typo'd roles. Skips single-quoted string literals.
fn unknown_casts(sql: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let b = sql.as_bytes();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'\'' => {
                // skip a single-quoted string (with '' escape)
                i += 1;
                while i < b.len() {
                    if b[i] == b'\'' {
                        if i + 1 < b.len() && b[i + 1] == b'\'' {
                            i += 2;
                            continue;
                        }
                        i += 1;
                        break;
                    }
                    i += 1;
                }
            }
            b':' if i + 1 < b.len() && b[i + 1] == b':' => {
                let mut j = i + 2;
                while j < b.len() && b[j] == b' ' {
                    j += 1;
                }
                let start = j;
                while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'_') {
                    j += 1;
                }
                if j > start {
                    let up = sql[start..j].to_ascii_uppercase();
                    if crate::parse_role(&up).is_none()
                        && !KNOWN_TYPES.contains(&up.as_str())
                        && !out.contains(&up)
                    {
                        out.push(up);
                    }
                }
                i = j.max(i + 2);
            }
            _ => i += 1,
        }
    }
    out
}

fn input_kind(roles: &[(usize, Role)]) -> Option<InputKind> {
    roles.iter().find_map(|(_, r)| match r {
        Role::Input(k) => Some(*k),
        _ => None,
    })
}

/// `SET VARIABLE` statement(s) that give an input its default value — the first
/// option for a single input, all options for a multiselect, both ends for a
/// date range — so downstream `getvariable()` panels have something to read.
fn input_defaults(kind: InputKind, rows: &[Map<String, Value>]) -> Vec<String> {
    let Some(first) = rows.first() else {
        return Vec::new();
    };
    let keys: Vec<&String> = first.keys().collect();
    if keys.is_empty() {
        return Vec::new();
    }
    match kind {
        InputKind::DateRange => keys
            .iter()
            .take(2)
            .filter_map(|k| {
                first
                    .get(*k)
                    .map(|v| format!("SET VARIABLE {k} = {}", lit(v)))
            })
            .collect(),
        InputKind::Multiselect => {
            let k = keys[0];
            let vals: Vec<String> = rows.iter().filter_map(|r| r.get(k)).map(lit).collect();
            vec![format!("SET VARIABLE {k} = [{}]", vals.join(", "))]
        }
        _ => {
            let k = keys[0];
            vec![format!("SET VARIABLE {k} = {}", lit(&first[k]))]
        }
    }
}

fn lit(v: &Value) -> String {
    match v {
        Value::String(s) => format!("'{}'", s.replace('\'', "''")),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => {
            if *b {
                "TRUE".into()
            } else {
                "FALSE".into()
            }
        }
        _ => "NULL".into(),
    }
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

// ---- design-lint helpers -------------------------------------------------

/// A KPI tile — a big-number card (`::METRIC`/`::MONEY`/`::PERCENT`/`::COMPACT`).
fn is_kpi(roles: &[(usize, Role)]) -> bool {
    roles.iter().any(|(_, r)| matches!(r, Role::Metric(_)))
}

/// The panel's chart kind, if it's a chart (`count()::BARCHART` → `Kind::Bar`).
fn chart_kind(roles: &[(usize, Role)]) -> Option<Kind> {
    roles.iter().find_map(|(_, r)| match r {
        Role::Value(k) => Some(*k),
        _ => None,
    })
}

/// A "real" chart that ought to carry a `::TITLE` (excludes sparklines, gauges,
/// calendars, candlesticks and QQ, which are self-explanatory or captioned).
fn is_big_chart(k: Kind) -> bool {
    matches!(
        k,
        Kind::Bar
            | Kind::BarStacked
            | Kind::BarPercent
            | Kind::BarStackedPercent
            | Kind::Line
            | Kind::LinePercent
            | Kind::Step
            | Kind::Smooth
            | Kind::Area
            | Kind::AreaStacked
            | Kind::Point
            | Kind::Pie
            | Kind::Donut
            | Kind::Histogram
            | Kind::Boxplot
            | Kind::Violin
            | Kind::Density
            | Kind::Heatmap
            | Kind::Radar
    )
}

/// Output-column index carrying an exact (unit) role, e.g. `Role::X`.
fn role_index(roles: &[(usize, Role)], want: Role) -> Option<usize> {
    roles.iter().find(|(_, r)| *r == want).map(|(i, _)| *i)
}

/// Output-column index of the first measure column (`Role::Value(_)`).
fn value_index(roles: &[(usize, Role)]) -> Option<usize> {
    roles
        .iter()
        .find(|(_, r)| matches!(r, Role::Value(_)))
        .map(|(i, _)| *i)
}

/// First non-null value of output column `c{col}` is a string → a discrete axis.
fn col_is_discrete(rows: &[Map<String, Value>], col: usize) -> bool {
    let key = format!("c{col}");
    rows.iter()
        .find_map(|r| r.get(&key))
        .is_some_and(|v| v.is_string())
}

/// A string x that looks like an ISO date/timestamp (`YYYY-MM-…`) is a time axis
/// — bars along time are legitimately in chronological (not value) order.
fn col_is_temporal(rows: &[Map<String, Value>], col: usize) -> bool {
    let key = format!("c{col}");
    rows.iter()
        .filter_map(|r| r.get(&key))
        .take(4)
        .any(|v| matches!(v, Value::String(s) if looks_like_date(s)))
}

fn looks_like_date(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() >= 8 && b[0..4].iter().all(u8::is_ascii_digit) && b[4] == b'-'
}

/// Are output column `c{col}`'s numeric values sorted (either direction)?
fn col_is_monotonic(rows: &[Map<String, Value>], col: usize) -> bool {
    let key = format!("c{col}");
    let vals: Vec<f64> = rows
        .iter()
        .filter_map(|r| r.get(&key))
        .filter_map(|v| match v {
            Value::Number(n) => n.as_f64(),
            Value::String(s) => s.parse::<f64>().ok(),
            _ => None,
        })
        .collect();
    if vals.len() < 2 {
        return true;
    }
    let non_dec = vals.windows(2).all(|w| w[1] >= w[0]);
    let non_inc = vals.windows(2).all(|w| w[1] <= w[0]);
    non_dec || non_inc
}

/// Distinct values in output column `c{col}` (for the series-count check).
fn distinct_count(rows: &[Map<String, Value>], col: usize) -> usize {
    let key = format!("c{col}");
    let mut seen = std::collections::HashSet::new();
    for r in rows {
        if let Some(v) = r.get(&key) {
            seen.insert(v.to_string());
        }
    }
    seen.len()
}

/// Emit `design/ungrouped-kpis` if a run of ≥3 top-level KPI tiles closed, and
/// reset the run counter.
fn flush_kpi_run(diags: &mut Vec<Diagnostic>, run: &mut usize, start_stmt: usize) {
    if *run >= 3 {
        diags.push(Diagnostic {
            severity: Severity::Warning,
            stmt: start_stmt,
            code: "design/ungrouped-kpis",
            message: format!(
                "{} KPI tiles in a row at top level — wrap them in a ::GROUP … ::ENDGROUP box so \
                 they read as one compact strip (the static renderer won't auto-group them).",
                *run
            ),
            sql: String::new(),
        });
    }
    *run = 0;
}

#[cfg(test)]
mod design_tests {
    use super::*;
    use serde_json::json;

    /// Build `n` rows from a closure producing a JSON object per index.
    fn rows(n: usize, f: impl Fn(usize) -> serde_json::Value) -> Vec<Map<String, Value>> {
        (0..n).map(|i| f(i).as_object().unwrap().clone()).collect()
    }

    /// Lint a one-panel script whose single query returns `data`.
    fn lint_one(sql: &str, data: Vec<Map<String, Value>>) -> Vec<Diagnostic> {
        check(sql, |_q| Ok(data.clone()))
    }

    fn has(diags: &[Diagnostic], code: &str) -> bool {
        diags.iter().any(|d| d.code == code)
    }

    #[test]
    fn pie_with_many_slices_warns() {
        let data = rows(11, |i| json!({ "c0": format!("s{i}"), "c1": i as f64 }));
        let d = lint_one(
            "SELECT cat::CATEGORY, sum(n)::PIE, 'Share'::TITLE FROM t GROUP BY 1;",
            data,
        );
        assert!(has(&d, "design/pie-slices"), "{d:?}");
    }

    #[test]
    fn small_pie_is_clean() {
        let data = rows(4, |i| json!({ "c0": format!("s{i}"), "c1": i as f64 }));
        let d = lint_one(
            "SELECT cat::CATEGORY, sum(n)::PIE, 'Share'::TITLE FROM t GROUP BY 1;",
            data,
        );
        assert!(!has(&d, "design/pie-slices"), "{d:?}");
    }

    #[test]
    fn unsorted_bars_warn_but_sorted_are_clean() {
        let unsorted = rows(3, |i| {
            let v = [3.0, 1.0, 2.0][i];
            json!({ "c0": format!("cat{i}"), "c1": v })
        });
        let d = lint_one(
            "SELECT sector::XAXIS, cnt::BARCHART, 'By sector'::TITLE FROM t;",
            unsorted,
        );
        assert!(has(&d, "design/unsorted-bars"), "{d:?}");

        let sorted = rows(3, |i| {
            let v = [3.0, 2.0, 1.0][i];
            json!({ "c0": format!("cat{i}"), "c1": v })
        });
        let d2 = lint_one(
            "SELECT sector::XAXIS, cnt::BARCHART, 'By sector'::TITLE FROM t;",
            sorted,
        );
        assert!(!has(&d2, "design/unsorted-bars"), "{d2:?}");
    }

    #[test]
    fn temporal_bars_are_not_flagged_unsorted() {
        // Bars along a date axis are legitimately chronological, not value-sorted.
        let data = rows(3, |i| {
            let v = [3.0, 1.0, 2.0][i];
            json!({ "c0": format!("2021-0{}-01", i + 1), "c1": v })
        });
        let d = lint_one(
            "SELECT day::XAXIS, cnt::BARCHART, 'Daily'::TITLE FROM t;",
            data,
        );
        assert!(!has(&d, "design/unsorted-bars"), "{d:?}");
    }

    #[test]
    fn untitled_chart_warns() {
        let data = rows(
            3,
            |i| json!({ "c0": format!("c{i}"), "c1": (3 - i) as f64 }),
        );
        let d = lint_one("SELECT sector::XAXIS, cnt::BARCHART FROM t;", data.clone());
        assert!(has(&d, "design/untitled-chart"), "{d:?}");

        let d2 = lint_one(
            "SELECT sector::XAXIS, cnt::BARCHART, 'T'::TITLE FROM t;",
            data,
        );
        assert!(!has(&d2, "design/untitled-chart"), "{d2:?}");
    }

    #[test]
    fn many_series_warns() {
        let data = rows(
            8,
            |i| json!({ "c0": "w", "c1": format!("series{i}"), "c2": i as f64 }),
        );
        let d = lint_one(
            "SELECT wk::XAXIS, ch::CATEGORY, n::BARCHART, 'T'::TITLE FROM t GROUP BY ALL;",
            data,
        );
        assert!(has(&d, "design/many-series"), "{d:?}");
    }

    #[test]
    fn raw_table_dump_warns() {
        let big = rows(250, |i| json!({ "A": format!("row{i}"), "B": i as f64 }));
        let d = lint_one("SELECT a AS \"A\" ::TABLE, b AS \"B\" FROM t;", big);
        assert!(has(&d, "design/raw-table"), "{d:?}");

        // A modest table paginates fine client-side — no warning.
        let small = rows(75, |i| json!({ "A": format!("row{i}"), "B": i as f64 }));
        let d2 = lint_one("SELECT a AS \"A\" ::TABLE, b AS \"B\" FROM t;", small);
        assert!(!has(&d2, "design/raw-table"), "{d2:?}");
    }

    #[test]
    fn ungrouped_kpis_warn_but_grouped_are_clean() {
        let one = rows(1, |_| json!({ "c0": 42.0 }));
        let script = "SELECT sum(a)::METRIC, 'A'::LABEL FROM t;\n\
                      SELECT sum(b)::METRIC, 'B'::LABEL FROM t;\n\
                      SELECT sum(c)::METRIC, 'C'::LABEL FROM t;";
        let d = check(script, |_q| Ok(one.clone()));
        assert!(has(&d, "design/ungrouped-kpis"), "{d:?}");

        let grouped = "SELECT 'KPIs'::GROUP;\n\
                       SELECT sum(a)::METRIC, 'A'::LABEL FROM t;\n\
                       SELECT sum(b)::METRIC, 'B'::LABEL FROM t;\n\
                       SELECT sum(c)::METRIC, 'C'::LABEL FROM t;\n\
                       SELECT 1::ENDGROUP;";
        let d2 = check(grouped, |_q| Ok(one.clone()));
        assert!(!has(&d2, "design/ungrouped-kpis"), "{d2:?}");
    }

    #[test]
    fn too_many_panels_warn_but_tabs_are_clean() {
        let data = rows(
            3,
            |i| json!({ "c0": format!("c{i}"), "c1": (3 - i) as f64 }),
        );
        let panel = "SELECT x::XAXIS, y::BARCHART, 'T'::TITLE FROM t;\n";
        let flat = panel.repeat(9);
        let d = check(&flat, |_q| Ok(data.clone()));
        assert!(has(&d, "design/too-many-panels"), "{d:?}");

        let tabbed = format!("SELECT 'Page'::TAB;\n{flat}");
        let d2 = check(&tabbed, |_q| Ok(data.clone()));
        assert!(!has(&d2, "design/too-many-panels"), "{d2:?}");
    }

    #[test]
    fn design_pass_can_be_disabled() {
        let data = rows(
            3,
            |i| json!({ "c0": format!("c{i}"), "c1": (3 - i) as f64 }),
        );
        let flat = "SELECT x::XAXIS, y::BARCHART FROM t;\n".repeat(9);
        let d = check_opts(&flat, |_q| Ok(data.clone()), LintOptions { design: false });
        assert!(!d.iter().any(|x| x.code.starts_with("design/")), "{d:?}");
    }
}
