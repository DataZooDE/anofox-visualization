//! Lint an annotated dashboard script — the feedback an author (human or AI)
//! needs to self-correct.
//!
//! Dashboards fail *silently*: role casts inside a `CREATE … AS SELECT` or a
//! subquery are ignored and the statement is quietly treated as setup (no
//! panel, no error); a filter that returns no rows draws a blank card. The linter runs
//! each statement and reports these, so a generate → validate → repair loop can
//! fix them instead of shipping a broken dashboard.
//!
//! It is transport-agnostic: pass a `run_query` closure that executes SQL
//! against a **stateful** connection (setup persists for later panels) and
//! returns the rows, or a DuckDB error string.

use crate::{render_with_warnings, roles, sql, InputKind, Kind, RenderOptions, Role};
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
    /// `render-error` | `render-warning` (the plotting engine dropped data,
    /// e.g. non-finite rows or an empty stat layer) | `empty-panel` |
    /// `unknown-cast` | `theme/unknown` | `theme/multiple` | `locale/unknown`
    /// | `chrome/bad-url`. Design advisories
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
    let mut themes = 0usize; // ::THEME directives seen (theme/multiple)
                             // Variables set by inputs / ::SELECT_AS / ::DRILL: name -> (stmt, kind),
                             // so two different directives writing one variable are reported.
    let mut owners: std::collections::HashMap<String, (usize, &'static str)> =
        std::collections::HashMap::new();
    let mut claim = |diags: &mut Vec<Diagnostic>, names: &[String], stmt, kind, sql: &str| {
        for n in names {
            match owners.get(n) {
                Some(&(other, k)) if other != stmt && (k != kind || kind == "::DRILL") => diags
                    .push(Diagnostic {
                        severity: Severity::Warning,
                        stmt,
                        code: "interaction/name-clash",
                        message: format!(
                            "variable `{n}` is set by this {kind} and by statement {other} ({k}) — \
                             each overwrites the other; rename one"
                        ),
                        sql: sql.to_string(),
                    }),
                Some(_) => {}
                None => {
                    owners.insert(n.clone(), (stmt, kind));
                }
            }
        }
    };
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
                              recognised, so it renders nothing. Role casts are read only from \
                              the main (outermost) SELECT list of a query statement — put them \
                              on the outer SELECT, not inside a subquery or CTE body. (A \
                              leading WITH is fine.)"
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
                    let names: Vec<String> = rows
                        .first()
                        .map(|r| {
                            let n = if kind == InputKind::DateRange { 2 } else { 1 };
                            let at = input_index(&p.roles).min(r.len().saturating_sub(1));
                            r.keys().skip(at).take(n).cloned().collect()
                        })
                        .unwrap_or_default();
                    claim(&mut diags, &names, stmt, "input", &short);
                    if kind == InputKind::Buttons && rows.len() > 7 {
                        diags.push(Diagnostic {
                            severity: Severity::Warning,
                            stmt,
                            code: "input/many-buttons",
                            message: format!(
                                "::BUTTONS has {} options — a button group reads well up to ~7; use ::DROPDOWN",
                                rows.len()
                            ),
                            sql: short.clone(),
                        });
                    }
                    for setv in input_defaults(kind, input_index(&p.roles), &rows) {
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

        // Chrome directives: a bad theme / locale / logo URL is reported here
        // (the browser would fall back to the default look).
        if let Some(r) = p
            .roles
            .iter()
            .map(|(_, r)| *r)
            .find(|r| roles::is_chrome_directive(*r))
        {
            let v = rows
                .first()
                .and_then(|row| row.values().next())
                .map(|v| match v {
                    Value::String(s) => s.clone(),
                    v => v.to_string(),
                })
                .unwrap_or_default();
            let mut bad = |code: &'static str, message: String, severity| {
                diags.push(Diagnostic {
                    severity,
                    stmt,
                    code,
                    message,
                    sql: short.clone(),
                })
            };
            match r {
                Role::Theme => {
                    themes += 1;
                    if themes == 2 {
                        bad(
                            "theme/multiple",
                            "more than one ::THEME — the last one wins".into(),
                            Severity::Warning,
                        );
                    }
                    if let Err(e) = crate::theme::resolve(&v) {
                        bad("theme/unknown", e, Severity::Error);
                    }
                }
                Role::Locale if crate::format::Locale::from_tag(&v).is_none() => bad(
                    "locale/unknown",
                    format!(
                        "unknown locale `{v}` (known: {})",
                        crate::format::Locale::TAGS.join(", ")
                    ),
                    Severity::Error,
                ),
                Role::AsOf => {
                    if crate::freshness::parse_timestamp(&v).is_none() {
                        bad(
                            "asof/unparseable",
                            format!("::ASOF value `{v}` is not a date or timestamp — the freshness chip is hidden"),
                            Severity::Warning,
                        );
                    }
                    let opt = rows.first().and_then(|row| {
                        row.values().skip(1).find_map(|c| {
                            c.as_str().filter(|c| crate::freshness::is_stale_option(c))
                        })
                    });
                    if let Some(o) = opt.filter(|o| crate::freshness::parse_stale(o).is_none()) {
                        bad(
                            "asof/bad-stale",
                            format!("`{o}` is not a stale threshold — write 'stale=24h' (s, m, h, d or w)"),
                            Severity::Warning,
                        );
                    }
                }
                Role::Logo | Role::Favicon if !crate::theme::safe_url(&v) => bad(
                    "chrome/bad-url",
                    format!("`{v}` is not an allowed image URL (relative, https: or data:image/)"),
                    Severity::Warning,
                ),
                _ => {}
            }
            continue;
        }

        // Interaction directives: prime their variables the way the browser does
        // ("nothing selected"), so panels filtering by a selection are checked
        // with data; a malformed value is reported.
        if let Some((_, r)) = p
            .roles
            .iter()
            .find(|(_, r)| matches!(r, Role::SelectAs | Role::Drill))
        {
            let v = first_text(&rows);
            let sets = if *r == Role::Drill {
                drill_prelude(&v)
            } else {
                select_as_prelude(&v)
            };
            if let Ok(sets) = &sets {
                // the variable names the directive sets (`SET VARIABLE <name> = …`)
                let names: Vec<String> = sets
                    .lines()
                    .filter_map(|l| l.strip_prefix("SET VARIABLE "))
                    .filter_map(|l| l.split(' ').next())
                    .map(str::to_string)
                    .collect();
                let kind = if *r == Role::Drill {
                    "::DRILL"
                } else {
                    "::SELECT_AS"
                };
                claim(&mut diags, &names, stmt, kind, &short);
            }
            match sets {
                Ok(sets) => prelude.push_str(&sets),
                Err(message) => diags.push(Diagnostic {
                    severity: Severity::Warning,
                    stmt,
                    code: "interaction/bad-value",
                    message,
                    sql: short.clone(),
                }),
            }
            continue;
        }

        // A ::LINK target must be a tab, a query, a path or an http(s) URL.
        if let Some((idx, _)) = p.roles.iter().find(|(_, r)| *r == Role::Link) {
            let cell = rows.first().and_then(|row| {
                row.get(&format!("c{idx}"))
                    .or_else(|| row.values().nth(*idx))
                    .and_then(|v| v.as_str())
            });
            if let Some(t) = cell.filter(|t| !link_target_ok(t)) {
                diags.push(Diagnostic {
                    severity: Severity::Warning,
                    stmt,
                    code: "link/bad-url",
                    message: format!(
                        "::LINK `{}` is not followed — use '#Tab', '?v.x={{value}}', \
                         '/d/<id>?v.x={{value}}' or an http(s) URL",
                        one_line(t)
                    ),
                    sql: short.clone(),
                });
            }
        }

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

        // A contract plot call: render every result row (one plot per group).
        if let Some(cp) = &p.contract {
            if rows.is_empty() {
                diags.push(Diagnostic {
                    severity: Severity::Warning,
                    stmt,
                    code: "empty-panel",
                    message: format!(
                        "{}() returned no rows — this panel will be blank",
                        cp.macro_name
                    ),
                    sql: short.clone(),
                });
            }
            for (label, r) in sql::contract_call::render_rows(cp, &rows, 460, 300, "") {
                if let Err(e) = r {
                    let at = if label.is_empty() {
                        String::new()
                    } else {
                        format!(" [{label}]")
                    };
                    diags.push(Diagnostic {
                        severity: Severity::Error,
                        stmt,
                        code: "render-error",
                        message: one_line(&format!("{}{at}: {e}", cp.macro_name)),
                        sql: short.clone(),
                    });
                }
            }
            if opts.design {
                rendered_panels += 1;
                flush_kpi_run(&mut diags, &mut kpi_run, kpi_run_stmt);
            }
            continue;
        }

        // Layout directives don't render a chart — nothing to check.
        if roles::is_directive_panel(&p.roles) {
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
        match render_with_warnings(&cols, 460, 300, &RenderOptions::default()) {
            Err(e) => diags.push(Diagnostic {
                severity: Severity::Error,
                stmt,
                code: "render-error",
                message: one_line(&e.to_string()),
                sql: short.clone(),
            }),
            // The plotting engine's build warnings: the panel renders, but some
            // data was dropped (non-finite positions, a layer with no data).
            Ok(r) => diags.extend(r.warnings.iter().map(|w| Diagnostic {
                severity: Severity::Warning,
                stmt,
                code: "render-warning",
                message: one_line(w),
                sql: short.clone(),
            })),
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

/// Every `::TOKEN` cast in a statement (outside literals and comments), in order.
fn cast_tokens(sql: &str) -> Vec<String> {
    use sql::lex::{tokenize, TokKind};
    let toks: Vec<_> = tokenize(sql)
        .into_iter()
        .filter(|t| !t.is_trivia())
        .collect();
    toks.windows(3)
        .filter(|w| {
            w[0].is_punct(sql, ':')
                && w[1].is_punct(sql, ':')
                && w[0].end == w[1].start
                && w[2].kind == TokKind::Word
        })
        .map(|w| w[2].text(sql).to_ascii_uppercase())
        .collect()
}

/// Does the statement carry a `::ROLE` cast (a recognised role token, not a SQL
/// type cast like `::INT`)? Distinguishes a lost-roles panel from real setup.
fn has_role_cast(sql: &str) -> bool {
    cast_tokens(sql)
        .iter()
        .any(|t| crate::parse_role(t).is_some() && !roles::is_sql_type_token(t))
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

/// `::TOKEN` casts that are neither a recognised role nor a known SQL type —
/// i.e. probable typo'd roles. Literals and comments are skipped.
fn unknown_casts(sql: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for up in cast_tokens(sql) {
        if crate::parse_role(&up).is_none()
            && !KNOWN_TYPES.contains(&up.as_str())
            && !out.contains(&up)
        {
            out.push(up);
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

/// Output position of a panel's input column.
fn input_index(roles: &[(usize, Role)]) -> usize {
    roles
        .iter()
        .find(|(_, r)| matches!(r, Role::Input(_)))
        .map(|(i, _)| *i)
        .unwrap_or(0)
}

/// Quote a DuckDB identifier (`"a""b"`).
fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// `SET VARIABLE` statement(s) that give an input its default value — the first
/// option for a single input, all options for a multiselect, both ends for a
/// date range — so downstream `getvariable()` panels have something to read.
/// `idx` is the input column's output position (rows keep SELECT column order,
/// via serde_json's `preserve_order`); the variable name is quoted.
fn input_defaults(kind: InputKind, idx: usize, rows: &[Map<String, Value>]) -> Vec<String> {
    let Some(first) = rows.first() else {
        return Vec::new();
    };
    let keys: Vec<&String> = first.keys().collect();
    let Some(k) = keys.get(idx).or(keys.first()).copied() else {
        return Vec::new();
    };
    match kind {
        InputKind::DateRange => keys
            .iter()
            .skip(idx.min(keys.len().saturating_sub(1)))
            .take(2)
            .filter_map(|k| {
                first
                    .get(*k)
                    .map(|v| format!("SET VARIABLE {} = {}", quote_ident(k), lit(v)))
            })
            .collect(),
        InputKind::Multiselect => {
            let vals: Vec<String> = rows.iter().filter_map(|r| r.get(k)).map(lit).collect();
            vec![format!(
                "SET VARIABLE {} = [{}]",
                quote_ident(k),
                vals.join(", ")
            )]
        }
        // A toggle is a BOOLEAN whatever its default was written as.
        InputKind::Toggle => vec![format!(
            "SET VARIABLE {} = {}",
            quote_ident(k),
            if first.get(k).is_some_and(truthy) {
                "TRUE"
            } else {
                "FALSE"
            }
        )],
        _ => vec![format!(
            "SET VARIABLE {} = {}",
            quote_ident(k),
            first.get(k).map(lit).unwrap_or_else(|| "NULL".into())
        )],
    }
}

/// A `::TOGGLE` default as a boolean: `true`, a non-zero number, or the text
/// `true` / `1` / `yes` / `on` (as the browser reads it).
fn truthy(v: &Value) -> bool {
    match v {
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => matches!(
            s.trim().to_ascii_lowercase().as_str(),
            "true" | "t" | "1" | "yes" | "on"
        ),
        _ => false,
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

/// The first cell of a result as text (`""` when there is none).
fn first_text(rows: &[Map<String, Value>]) -> String {
    rows.first()
        .and_then(|row| row.values().next())
        .map(|v| match v {
            Value::String(s) => s.clone(),
            Value::Null => String::new(),
            v => v.to_string(),
        })
        .unwrap_or_default()
}

fn is_ident(s: &str) -> bool {
    let mut c = s.chars();
    matches!(c.next(), Some(ch) if ch.is_ascii_alphabetic() || ch == '_')
        && c.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
        && s.len() <= 60
}

/// `'name'` / `'name multi'` of a `::SELECT_AS` → the browser's initial
/// variables: the selection (`''`, or `[]` in multi mode) and an unset brushed
/// range (`<name>_from` / `<name>_to` = NULL).
fn select_as_prelude(v: &str) -> Result<String, String> {
    let mut parts = v
        .split(|c: char| c.is_whitespace() || c == ':' || c == ',')
        .filter(|p| !p.is_empty());
    let name = parts.next().unwrap_or("");
    let mode = parts.next().unwrap_or("single").to_ascii_lowercase();
    if !is_ident(name) || !matches!(mode.as_str(), "single" | "multi") || parts.next().is_some() {
        return Err(format!(
            "::SELECT_AS `{v}` — expected a variable name, optionally followed by \
             'single' or 'multi' (e.g. 'region' or 'region multi'); the chart keeps \
             the shared `selected` click"
        ));
    }
    let sel = if mode == "multi" {
        "[]::VARCHAR[]"
    } else {
        "''"
    };
    Ok(format!(
        "SET VARIABLE {n} = {sel};\nSET VARIABLE {n}_from = NULL;\nSET VARIABLE {n}_to = NULL;\n",
        n = name
    ))
}

/// `'[prefix:] a > b > c'` of a `::DRILL` → the browser's initial variables:
/// the top level, depth 0, an empty path and every level variable `''`.
fn drill_prelude(v: &str) -> Result<String, String> {
    let (prefix, path) = match v.split_once(':') {
        Some((p, rest)) => (p.trim(), rest),
        None => ("drill", v),
    };
    let levels: Vec<&str> = path.split('>').map(str::trim).collect();
    if !is_ident(prefix) || levels.len() < 2 || !levels.iter().all(|l| is_ident(l)) {
        return Err(format!(
            "::DRILL `{v}` — expected two or more column names joined by '>' \
             (e.g. 'region > country > city', optionally 'geo: region > …'); \
             the chart does not drill"
        ));
    }
    let mut out = format!(
        "SET VARIABLE {prefix}_level = '{}';\nSET VARIABLE {prefix}_depth = 0;\n\
         SET VARIABLE {prefix}_path = []::VARCHAR[];\n",
        levels[0]
    );
    for l in &levels {
        out.push_str(&format!("SET VARIABLE {l} = '';\n"));
    }
    Ok(out)
}

/// A `::LINK` template the browser follows: `#tab`, `?query`, a path, or an
/// http(s) / mailto URL — never another scheme.
fn link_target_ok(t: &str) -> bool {
    let low = t.trim().to_ascii_lowercase();
    match low.find(':') {
        Some(i) if !low[..i].contains(['/', '?', '#', '{']) => {
            ["http", "https", "mailto"].contains(&&low[..i])
        }
        _ => !low.is_empty(),
    }
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
            | Kind::Bubble
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
    fn select_as_primes_its_variables() {
        let mut seen = Vec::new();
        let d = check(
            "SELECT 'week multi'::SELECT_AS;\nSELECT 1::METRIC FROM t WHERE list_contains(getvariable('week'), x);",
            |q| {
                seen.push(q.to_string());
                Ok(vec![json!({ "c0": "week multi" }).as_object().unwrap().clone()])
            },
        );
        assert!(!has(&d, "interaction/bad-value"), "{d:?}");
        let last = seen.last().unwrap();
        assert!(
            last.contains("SET VARIABLE week = []::VARCHAR[];"),
            "{last}"
        );
        assert!(last.contains("SET VARIABLE week_from = NULL;"), "{last}");
        assert_eq!(
            select_as_prelude("region").unwrap(),
            "SET VARIABLE region = '';\nSET VARIABLE region_from = NULL;\nSET VARIABLE region_to = NULL;\n"
        );
        for bad in ["", "1x", "a b c", "region many", "x; DROP"] {
            assert!(select_as_prelude(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn drill_primes_its_variables_and_links_are_checked() {
        assert_eq!(
            drill_prelude("geo: region > country").unwrap(),
            "SET VARIABLE geo_level = 'region';\nSET VARIABLE geo_depth = 0;\n\
             SET VARIABLE geo_path = []::VARCHAR[];\nSET VARIABLE region = '';\n\
             SET VARIABLE country = '';\n"
        );
        assert!(drill_prelude("region > country > city")
            .unwrap()
            .starts_with("SET VARIABLE drill_level = 'region';"));
        for bad in ["region", "a > ", "a > b c", "x y: a > b", "a > b; DROP"] {
            assert!(drill_prelude(bad).is_err(), "{bad}");
        }
        for ok in [
            "#Detail",
            "#Detail?v.r={value}",
            "?v.r={value}",
            "/d/sales?v.r={value}",
            "https://x.y/{value}",
            "detail.html",
        ] {
            assert!(link_target_ok(ok), "{ok}");
        }
        for bad in [
            "javascript:alert(1)",
            " JavaScript:x",
            "data:text/html,x",
            "",
        ] {
            assert!(!link_target_ok(bad), "{bad}");
        }
        let d = lint_one(
            "SELECT r::XAXIS, n::BARCHART, 'javascript:alert(1)'::LINK, 't'::TITLE FROM t;",
            vec![
                json!({ "c0": "a", "c1": 1.0, "c2": "javascript:alert(1)", "c3": "t" })
                    .as_object()
                    .unwrap()
                    .clone(),
            ],
        );
        assert!(has(&d, "link/bad-url"), "{d:?}");
    }

    #[test]
    fn a_selection_named_like_a_drill_level_clashes() {
        let d = check(
            "SELECT 'region multi'::SELECT_AS;\nSELECT 'region > country'::DRILL;",
            |q| {
                let v = if q.trim_end().ends_with("::DRILL;") || q.contains("> country' AS") {
                    "region > country"
                } else {
                    "region multi"
                };
                Ok(vec![json!({ "c0": v }).as_object().unwrap().clone()])
            },
        );
        assert!(has(&d, "interaction/name-clash"), "{d:?}");
        let ok = check(
            "SELECT 'week'::SELECT_AS;\nSELECT 'week'::SELECT_AS;",
            |_| Ok(vec![json!({ "c0": "week" }).as_object().unwrap().clone()]),
        );
        assert!(!has(&ok, "interaction/name-clash"), "{ok:?}");
    }

    #[test]
    fn select_as_with_a_bad_value_warns() {
        let d = lint_one(
            "SELECT 'not a name'::SELECT_AS;",
            vec![json!({ "c0": "not a name" }).as_object().unwrap().clone()],
        );
        assert!(has(&d, "interaction/bad-value"), "{d:?}");
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
    fn engine_warnings_become_render_warnings() {
        // A density per category where every group has one value: the stat
        // can't estimate anything, so ggplot-rs skips the layer with a warning.
        let data = rows(2, |i| {
            let g = ["a", "b"][i];
            json!({ "c0": g, "c1": 1.0 })
        });
        let d = check_opts(
            "SELECT g::CATEGORY, v::DENSITY FROM t;",
            |_q| Ok(data.clone()),
            LintOptions { design: false },
        );
        let w: Vec<_> = d.iter().filter(|x| x.code == "render-warning").collect();
        assert_eq!(w.len(), 1, "{d:?}");
        assert_eq!(w[0].severity, Severity::Warning);
        assert!(w[0].message.contains("stat_density"), "{}", w[0].message);
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

    #[test]
    fn input_priming_uses_the_input_column_and_quotes_it() {
        // Output order: the input column `zone` first, then the HINT (c1). A
        // sorted map would pick "c1" — the wrong variable.
        let opts = rows(2, |i| {
            let z = ["EU", "US"][i];
            json!({ "zone": z, "c1": 3 })
        });
        let mut seen = Vec::new();
        let script = "SELECT DISTINCT zone::DROPDOWN, count(*)::HINT FROM t GROUP BY 1;\n\
                      SELECT x::XAXIS, y::BARCHART, 'T'::TITLE FROM t WHERE z = getvariable('zone');";
        check(script, |q| {
            seen.push(q.to_string());
            Ok(opts.clone())
        });
        let last = seen.last().unwrap();
        assert!(
            last.contains("SET VARIABLE \"zone\" = 'EU';"),
            "variable not primed: {last}"
        );
        // A weird column name is quoted, not injected.
        let weird = rows(1, |_| json!({ "a b\"; DROP TABLE t; --": "v" }));
        let d = input_defaults(InputKind::Dropdown, 0, &weird);
        assert_eq!(
            d,
            vec!["SET VARIABLE \"a b\"\"; DROP TABLE t; --\" = 'v'".to_string()]
        );
        // Multiselect: all options as a list.
        let d = input_defaults(InputKind::Multiselect, 0, &opts);
        assert_eq!(d, vec!["SET VARIABLE \"zone\" = ['EU', 'US']".to_string()]);
        // Buttons: the first option, like a dropdown.
        let d = input_defaults(InputKind::Buttons, 0, &opts);
        assert_eq!(d, vec!["SET VARIABLE \"zone\" = 'EU'".to_string()]);
        // Toggle: always a BOOLEAN, whatever the default was written as.
        for (v, want) in [
            (json!(true), "TRUE"),
            (json!("on"), "TRUE"),
            (json!(0), "FALSE"),
            (json!("false"), "FALSE"),
        ] {
            let r = rows(1, |_| json!({ "net": v.clone() }));
            let d = input_defaults(InputKind::Toggle, 0, &r);
            assert_eq!(d, vec![format!("SET VARIABLE \"net\" = {want}")]);
        }
    }

    #[test]
    fn asof_value_and_stale_option_are_checked() {
        let d = check("SELECT 'yesterday'::ASOF, 'stale=soon';", |_q| {
            Ok(rows(1, |_| json!({ "c0": "yesterday", "o": "stale=soon" })))
        });
        let codes: Vec<_> = d.iter().map(|x| x.code).collect();
        assert!(codes.contains(&"asof/unparseable"), "{d:?}");
        assert!(codes.contains(&"asof/bad-stale"), "{d:?}");
        let d = check("SELECT max(t)::ASOF, 'stale=24h' FROM x;", |_q| {
            Ok(rows(
                1,
                |_| json!({ "c0": "2026-10-16 09:24:16.87+02", "o": "stale=24h" }),
            ))
        });
        assert!(!d.iter().any(|x| x.code.starts_with("asof/")), "{d:?}");
    }

    #[test]
    fn cte_panels_lint_clean() {
        let data = rows(
            3,
            |i| json!({ "c0": format!("c{i}"), "c1": (3 - i) as f64 }),
        );
        let d = check(
            "WITH s AS (SELECT 1) SELECT x::XAXIS, y::BARCHART, 'T'::TITLE FROM s;",
            |_q| Ok(data.clone()),
        );
        assert!(!has(&d, "silent-setup"), "{d:?}");
    }
}
