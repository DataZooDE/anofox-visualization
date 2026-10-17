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

use crate::inputs::{input_defaults, input_index, input_kind};
use crate::{render_with_warnings, roles, sql, InputKind, Kind, RenderOptions, Role};
use serde_json::{Map, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
    /// Informational (an optional `-- @requires` input that is missing).
    Note,
}

impl Severity {
    pub fn label(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Note => "note",
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
    /// | `chrome/bad-url` | `period/bad-spec` (a `::PERIOD` spec with an
    /// unknown key / preset / grain / compare or a bad date) |
    /// `kpi/bad-delta-format` | `kpi/no-delta` (warning) |
    /// `kpi/sparkline-not-list` (warning) | `animate/unknown` |
    /// `animate/reveal-no-xline` | `linestyle/unknown` | `story/bad-by` |
    /// `transition/unknown` (warnings) | `diagram/null-id` |
    /// `diagram/duplicate-node` | `diagram/unknown-node` | `diagram/bad-edges` |
    /// `diagram/bad-style` | `diagram/bad-direction` | `diagram/too-large`
    /// (warnings) | `box/missing` (error: a box from statistics without
    /// ::Q1 / ::MEDIAN / ::Q3, unpaired whiskers, no ::XAXIS, or beside a chart
    /// measure) | `box/unordered` | `box/null` (warnings) |
    /// `requires/unknown-variable` | `requires/empty-alternative` |
    /// `requires/bad-variable` | `requires/on-setup` |
    /// `requires/tab-always-hidden` (warnings) | `optional/missing` (a note: a
    /// `-- @requires` input is missing, so the panel shows "Not available" —
    /// a ::TAB / ::SUBTAB is hidden with its panels).
    /// Access policy ([`check_policy`]): `policy/invalid` | `policy/refused`
    /// (errors) | `policy/denied-column` | `policy/duckdb-version` (warnings:
    /// the CLI isn't the pinned DuckDB) | `policy/masked-column` (a note).
    /// Header comments ([`header_diags`], warnings): `header/empty-group` |
    /// `header/long-group` | `header/too-many-tags` | `header/empty-owner` |
    /// `header/bad-certified` | `header/long-description`. A served folder ([`folder_diags`], warnings):
    /// `folder/duplicate-title` | `folder/bad-id`.
    /// Design advisories
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
    let mut diags = header_diags(script);
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
    // A ::EMPTY message is waiting for the next panel (its 0 rows are expected).
    let mut empty_msg = false;
    // What a ::TRACK here would sit under: `Some(true)` a chart with a numeric
    // / date x, `Some(false)` one with a discrete x, `None` no chart.
    let mut track_host: Option<bool> = None;
    // Cross-panel design counters (only used when opts.design).
    let mut rendered_panels = 0usize; // visible panels (charts/tables/kpis/text/markdown)
    let mut has_tabs = false;
    let mut sections = sql::requires::Sections::default(); // hidden tabs
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

        // Malformed `-- @requires` lines (an empty alternative, a bad `${…}`).
        for (code, message) in &p.requires_issues {
            diags.push(Diagnostic {
                severity: Severity::Warning,
                stmt,
                code,
                message: message.clone(),
                sql: short.clone(),
            });
        }
        // `-- @requires` on a setup statement that reads no file: a viewer who
        // picks a run without that file skips it (and keeps their own value
        // of what it sets).
        if p.setup && !p.requires.is_empty() && !reads_a_file(&p.sql) {
            diags.push(Diagnostic {
                severity: Severity::Warning,
                stmt,
                code: "requires/on-setup",
                message: "`-- @requires` on a setup statement that reads no file: a viewer can \
                          skip it by choosing a run without that file, so nothing it sets may \
                          narrow what a viewer sees — move the `@requires` to the statement \
                          that reads the file"
                    .into(),
                sql: short.clone(),
            });
        }
        // A ::TAB / ::SUBTAB whose optional input is missing is hidden: its
        // statements are left out (setup still runs, as in the browser).
        let tab = p.tab_kind();
        let mut tab_unknown = Vec::new();
        let mut decided_here = false; // (not a sub-tab of a hidden tab)
        if let Some(path) = sections.step(&p, |reqs| {
            let u = unmet_requirement(&mut run_query, &prelude, reqs);
            tab_unknown = u.unknown;
            decided_here = u.missing.is_some();
            u.missing
        }) {
            has_tabs |= tab.is_some();
            if tab.is_some() && !tab_unknown.is_empty() {
                diags.push(tab_always_hidden(
                    stmt,
                    &tab_unknown,
                    raws.get(i + 1..).unwrap_or_default(),
                    &short,
                ));
            }
            if decided_here {
                diags.push(Diagnostic {
                    severity: Severity::Note,
                    stmt,
                    code: "optional/missing",
                    message: format!(
                        "optional input `{path}` is missing — this {} is hidden, with its panels \
                         (`-- @requires`)",
                        if tab == Some(true) {
                            "::SUBTAB"
                        } else {
                            "::TAB"
                        }
                    ),
                    sql: short.clone(),
                });
            }
            continue;
        }
        // An optional input that is missing: the statement is skipped (setup
        // never reaches the prelude) and its panel reads "Not available".
        let unmet = if tab.is_some() {
            // (checked above; a met tab can still name an unset variable)
            sql::requires::Unmet {
                missing: None,
                unknown: tab_unknown,
            }
        } else {
            unmet_requirement(&mut run_query, &prelude, &p.requires)
        };
        for v in &unmet.unknown {
            diags.push(Diagnostic {
                severity: Severity::Warning,
                stmt,
                code: "requires/unknown-variable",
                message: format!(
                    "`${{{v}}}` in `-- @requires` names no variable set here (a SET VARIABLE \
                     above, an input, or `anofox_data`, which `dashboard --check --data <dir>` \
                     and the locked server set) — that path counts as missing"
                ),
                sql: short.clone(),
            });
        }
        if let Some(path) = unmet.missing {
            diags.push(Diagnostic {
                severity: Severity::Note,
                stmt,
                code: "optional/missing",
                message: format!(
                    "optional input `{path}` is missing — {} (`-- @requires`)",
                    if p.setup {
                        "this statement is skipped"
                    } else if crate::track::is_track(&p.roles) {
                        "this strip is left out of its chart"
                    } else {
                        "this panel shows \"Not available\""
                    }
                ),
                sql: short.clone(),
            });
            // Its "Not available" card still takes the panel's place: a KPI
            // tile stays in its run, any other panel ends it — with or
            // without the file, the same verdict.
            if opts.design && !p.setup && is_content_panel(&p.roles) {
                count_design_panel(
                    &p.roles,
                    group_depth == 0,
                    stmt,
                    &mut kpi_run,
                    &mut kpi_run_stmt,
                    &mut rendered_panels,
                    &mut diags,
                );
            }
            continue;
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
                // In a hidden tab, a setup statement that reads its missing
                // input fails as expected.
                let reqs = if sections.hiding() {
                    [p.requires.clone(), p.tab_requires.clone()].concat()
                } else {
                    p.requires.clone()
                };
                diags.push(sql_error(stmt, &reqs, &e, &short));
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
                            let cols: Vec<String> = r.keys().skip(at).take(n).cloned().collect();
                            if kind == InputKind::Period {
                                let prefix = crate::period::var_prefix(
                                    cols.first().map(String::as_str).unwrap_or(""),
                                );
                                crate::period::SUFFIXES
                                    .iter()
                                    .map(|sfx| format!("{prefix}_{sfx}"))
                                    .collect()
                            } else {
                                cols
                            }
                        })
                        .unwrap_or_default();
                    claim(&mut diags, &names, stmt, "input", &short);
                    if kind == InputKind::Period {
                        let spec = rows
                            .first()
                            .and_then(|r| r.values().nth(input_index(&p.roles)))
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        for e in crate::period::parse_spec(spec).1 {
                            diags.push(Diagnostic {
                                severity: Severity::Error,
                                stmt,
                                code: "period/bad-spec",
                                message: format!(
                                    "::PERIOD spec: {e} — the control falls back to its defaults"
                                ),
                                sql: short.clone(),
                            });
                        }
                    }
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
                Err(e) => diags.push(sql_error(stmt, &p.requires, &e, &short)),
            }
            continue;
        }

        // Panel: run it (with the prelude in scope), then check rows and render.
        let rows = match run_query(&format!("{prelude}{};", p.sql)) {
            Ok(rows) => rows,
            Err(e) => {
                diags.push(sql_error(stmt, &p.requires, &e, &short));
                // (its "Not available" / error card takes the panel's place)
                if opts.design && is_content_panel(&p.roles) {
                    count_design_panel(
                        &p.roles,
                        group_depth == 0,
                        stmt,
                        &mut kpi_run,
                        &mut kpi_run_stmt,
                        &mut rendered_panels,
                        &mut diags,
                    );
                }
                continue;
            }
        };

        // A ::TRACK goes right after a chart with a numeric / date x (a
        // ::HEIGHT may sit between; further tracks follow it).
        let is_track = crate::track::is_track(&p.roles);
        if is_track {
            let message = match track_host {
                Some(true) => None,
                Some(false) => Some(
                    "a ::TRACK shares the x axis of the chart above it, which needs a numeric \
                     or date ::XAXIS (not categories)",
                ),
                None => Some(
                    "a ::TRACK goes right after the chart it sits under (only a ::HEIGHT may \
                     come between) — here it is drawn as a panel of its own",
                ),
            };
            if let Some(m) = message {
                diags.push(Diagnostic {
                    severity: Severity::Warning,
                    stmt,
                    code: "track/no-chart",
                    message: m.into(),
                    sql: short.clone(),
                });
            }
        } else if !p.roles.iter().any(|(_, r)| *r == Role::Height) {
            track_host = track_host_of(&p.roles, &rows);
        }

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

        // ::CALLOUT styles a ::MARKDOWN box: an unknown kind falls back to 'note',
        // a callout without Markdown text shows nothing.
        if let Some((i, _)) = p.roles.iter().find(|(_, r)| *r == Role::Callout) {
            let kind = rows
                .first()
                .and_then(|row| row.values().nth(*i))
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase();
            let mut warn = |code: &'static str, message: String| {
                diags.push(Diagnostic {
                    severity: Severity::Warning,
                    stmt,
                    code,
                    message,
                    sql: short.clone(),
                })
            };
            if !CALLOUT_KINDS.contains(&kind.as_str()) {
                warn(
                    "callout/unknown",
                    format!(
                        "::CALLOUT `{kind}` — use 'note', 'warning' or 'success' (drawn as a note)"
                    ),
                );
            }
            if !p.roles.iter().any(|(_, r)| *r == Role::Markdown) {
                warn(
                    "callout/no-text",
                    "::CALLOUT needs a ::MARKDOWN column in the same SELECT (the callout's text)"
                        .into(),
                );
            }
        }

        // {{col}} / {{col:fmt}} placeholders in a ::MARKDOWN text must name a
        // column of the same SELECT and a known format.
        if let (Some((i, _)), Some(row)) = (
            p.roles.iter().find(|(_, r)| *r == Role::Markdown),
            rows.first(),
        ) {
            let md = row.values().nth(*i).and_then(Value::as_str).unwrap_or("");
            for (name, spec) in md_placeholders(md) {
                let mut warn = |code: &'static str, message: String| {
                    diags.push(Diagnostic {
                        severity: Severity::Warning,
                        stmt,
                        code,
                        message,
                        sql: short.clone(),
                    })
                };
                if md_column(row.keys(), &name).is_none() {
                    let message = if is_positional_key(&name) {
                        format!("{{{{{name}}}}} names no column of this SELECT — `{name}` is also an internal column name: alias the value `… AS {name}` in this SELECT, not in a subquery or `*` (it is shown as written)")
                    } else {
                        format!("{{{{{name}}}}} names no column of this SELECT — add `… AS {name}` (it is shown as written)")
                    };
                    warn("markdown/unknown-placeholder", message);
                }
                if let Some(sp) =
                    spec.filter(|sp| !MD_FORMATS.contains(&sp.to_ascii_lowercase().as_str()))
                {
                    warn(
                        "markdown/unknown-format",
                        format!("{{{{{name}:{sp}}}}}: unknown format — use money, percent, compact, metric, date or text"),
                    );
                }
            }
            // $…$ / $$…$$ formulas KaTeX can't typeset (drawn as red source)
            for seg in crate::math::spans(md) {
                if let crate::math::Seg::Math { tex, .. } = seg {
                    if let Err(e) = crate::math::check(&tex) {
                        diags.push(Diagnostic {
                            severity: Severity::Warning,
                            stmt,
                            code: "markdown/bad-math",
                            message: format!("formula `{}`: {e}", one_line(&tex)),
                            sql: short.clone(),
                        });
                    }
                }
            }
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

        // Annotation roles that need a partner on the same row (#31).
        let has = |r: Role| p.roles.iter().any(|(_, x)| *x == r);
        for (part, needs) in [
            (Role::VLineLabel, Role::VLine),
            (Role::RefLineLabel, Role::RefLine),
            (Role::XBandFrom, Role::XBandTo),
            (Role::XBandTo, Role::XBandFrom),
            (Role::XBandLabel, Role::XBandFrom),
        ] {
            if has(part) && !has(needs) {
                diags.push(Diagnostic {
                    severity: Severity::Warning,
                    stmt,
                    code: "annotation/unpaired",
                    message: format!(
                        "::{} draws nothing without a ::{} column in the same SELECT",
                        part.token(),
                        needs.token()
                    ),
                    sql: short.clone(),
                });
            }
        }

        // Bands come in ::BAND_LOWER + ::BAND_UPPER pairs (several = a fan).
        let count = |r: Role| p.roles.iter().filter(|(_, x)| *x == r).count();
        let (lo, hi) = (count(Role::BandLower), count(Role::BandUpper));
        if lo != hi {
            diags.push(Diagnostic {
                severity: Severity::Warning,
                stmt,
                code: "annotation/unpaired",
                message: format!(
                    "{lo} ::BAND_LOWER but {hi} ::BAND_UPPER — bands pair up in column order \
                     (lower 1 with upper 1, …); the unpaired edge draws nothing"
                ),
                sql: short.clone(),
            });
        }

        // ::THRESHOLDS rules must parse; a table directive names its columns.
        if let Some((idx, _)) = p.roles.iter().find(|(_, r)| *r == Role::Thresholds) {
            let cell = rows.first().and_then(|row| {
                row.get(&format!("c{idx}"))
                    .or_else(|| row.values().nth(*idx))
                    .and_then(|v| v.as_str())
            });
            let mut warn = |severity, code, message: String| {
                diags.push(Diagnostic {
                    severity,
                    stmt,
                    code,
                    message,
                    sql: short.clone(),
                })
            };
            if let Some(spec) = cell {
                if roles::is_table_thresholds(&p.roles) {
                    match crate::thresholds::parse_table(spec) {
                        Err(e) => warn(Severity::Error, "thresholds/bad-rule", e),
                        Ok(g) if g.iter().any(|(c, _)| c.is_none()) => warn(
                            Severity::Warning,
                            "thresholds/no-column",
                            "a lone ::THRESHOLDS colours the next table's cells by column — \
                             name them: 'Margin %: <10:bad; good | accuracy: <0.85:bad'"
                                .into(),
                        ),
                        Ok(_) => {}
                    }
                } else if let Err(e) = crate::thresholds::Rules::parse(spec) {
                    warn(Severity::Error, "thresholds/bad-rule", e);
                }
            }
        }
        // A ::COLORS level map ('Critical:bad; …') must parse and needs a
        // ::CATEGORY to colour.
        if let Some((idx, _)) = p.roles.iter().find(|(_, r)| *r == Role::GaugeColors) {
            let spec = rows.first().and_then(|row| {
                row.get(&format!("c{idx}"))
                    .or_else(|| row.values().nth(*idx))
                    .and_then(|v| v.as_str())
            });
            if let Some(spec) = spec.filter(|s| crate::thresholds::is_level_map(s)) {
                let mut warn = |severity, code, message: String| {
                    diags.push(Diagnostic {
                        severity,
                        stmt,
                        code,
                        message,
                        sql: short.clone(),
                    })
                };
                if let Err(e) = crate::thresholds::parse_level_colors(spec) {
                    warn(Severity::Error, "colors/bad-map", e);
                } else if !p.roles.iter().any(|(_, r)| *r == Role::Category) {
                    warn(
                        Severity::Warning,
                        "colors/no-category",
                        "a ::COLORS level map colours the ::CATEGORY levels it names — \
                         add the column: status::CATEGORY, 'Critical:bad; OK:good'::COLORS"
                            .into(),
                    );
                }
            }
        }
        if p.roles
            .iter()
            .any(|(_, r)| matches!(r, Role::Table | Role::PagedTable))
            && cast_tokens(raws.get(i).map_or("", String::as_str))
                .iter()
                .any(|t| roles::lookup(t).is_some_and(|s| s.role == Role::Thresholds))
        {
            diags.push(Diagnostic {
                severity: Severity::Warning,
                stmt,
                code: "thresholds/in-table",
                message:
                    "::THRESHOLDS inside a ::TABLE is shown as a column — put it in its own \
                          statement before the table: SELECT 'Margin %: <10:bad; good'::THRESHOLDS;"
                        .into(),
                sql: short.clone(),
            });
        }

        diags.extend(
            motion_diags(&p.roles, &rows)
                .into_iter()
                .map(|(code, message)| Diagnostic {
                    severity: Severity::Warning,
                    stmt,
                    code,
                    message,
                    sql: short.clone(),
                }),
        );

        diags.extend(
            map_diags(&p.roles, &rows)
                .into_iter()
                .map(|(severity, code, message)| Diagnostic {
                    severity,
                    stmt,
                    code,
                    message,
                    sql: short.clone(),
                }),
        );

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
            empty_msg |= p.roles.iter().any(|(_, r)| *r == Role::Empty);
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

        let expect_empty = std::mem::take(&mut empty_msg);
        if rows.is_empty() && (expect_empty || is_track) {
            // the author wrote a message for this case (::EMPTY); a track
            // without events still shows its lanes
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
        // A box plot from statistics: its roles (else it cannot render),
        // then its numbers.
        let box_roles = crate::boxstats::prepare(cols.clone());
        match &box_roles {
            Err(e) => diags.push(Diagnostic {
                severity: Severity::Error,
                stmt,
                code: "box/missing",
                message: e.clone(),
                sql: short.clone(),
            }),
            Ok(_) => diags.extend(crate::boxstats::issues(&cols).into_iter().map(
                |(code, message)| Diagnostic {
                    severity: Severity::Warning,
                    stmt,
                    code,
                    message,
                    sql: short.clone(),
                },
            )),
        }
        match render_with_warnings(&cols, 460, 300, &RenderOptions::default()) {
            Err(_) if box_roles.is_err() => {}
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

        // A diagram's ids, edges, direction, size and loops.
        if let Some(d) =
            crate::diagram::Diagram::from_columns(&cols, &crate::format::Locale::default())
        {
            diags.extend(d.problems.into_iter().map(|pr| Diagnostic {
                severity: Severity::Warning,
                stmt,
                code: pr.code,
                message: pr.message,
                sql: short.clone(),
            }));
        }

        // A KPI's change line and trend line.
        if is_kpi(&p.roles) {
            diags.extend(kpi_checks(&p.roles, &rows).into_iter().map(
                |(severity, code, message)| Diagnostic {
                    severity,
                    stmt,
                    code,
                    message,
                    sql: short.clone(),
                },
            ));
        }

        // Waterfall / bullet / funnel rows are named by their ::XAXIS (step,
        // category, stage); without it the panel only shows a note.
        if let Some(k @ (Kind::Waterfall | Kind::Bullet | Kind::Funnel)) = chart_kind(&p.roles) {
            if role_index(&p.roles, Role::X).is_none() {
                diags.push(Diagnostic {
                    severity: Severity::Error,
                    stmt,
                    code: "chart/needs-xaxis",
                    message: format!(
                        "::{} needs an ::XAXIS column naming each {}.",
                        Role::Value(k).token(),
                        match k {
                            Kind::Waterfall => "step",
                            Kind::Bullet => "row",
                            _ => "stage",
                        }
                    ),
                    sql: short.clone(),
                });
            }
        }

        // ---- design: per-panel advisory checks (see docs/dashboard-design.md) ----
        // (A track is part of the chart above it: no title, no panel of its own.)
        if opts.design && !is_track {
            // Count content panels (charts/tables/markdown) toward the
            // scan-budget, but NOT individual KPI tiles — a strip of KPIs is one
            // band to scan, not N panels. A non-KPI content panel ends a run of
            // top-level KPI tiles.
            count_design_panel(
                &p.roles,
                group_depth == 0,
                stmt,
                &mut kpi_run,
                &mut kpi_run_stmt,
                &mut rendered_panels,
                &mut diags,
            );

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

/// Access-policy lint (issue #49), for `dashboard --check --policy`: the
/// statements the policy refuses (the locked server would not start), and the
/// ones that use a column some viewers can't see — `policy/denied-column`
/// (the statement fails for them; an error when nobody may see it) and
/// `policy/masked-column` (they get NULL, `****` or a hash).
pub fn check_policy(
    script: &str,
    enf: &crate::host::policy::Enforcer,
    duck: &mut dyn crate::host::policy::Duck,
) -> Vec<Diagnostic> {
    use crate::host::policy::Mask;
    let plan = sql::plan(script);
    let stmts: Vec<(bool, String)> = plan.iter().map(|p| (p.setup, p.sql.clone())).collect();
    let mut diags = Vec::new();
    for (i, r) in enf.rewrite_each(duck, &stmts).into_iter().enumerate() {
        let stmt = i + 1;
        let short = one_line(stmts[i].1.trim());
        let uses = match r {
            Err(e) => {
                diags.push(Diagnostic {
                    severity: Severity::Error,
                    stmt,
                    code: "policy/refused",
                    message: format!("the access policy refuses this statement: {e}"),
                    sql: short,
                });
                continue;
            }
            Ok((_, tables, cols)) => (tables, cols),
        };
        for (t, c) in enf.restricted_columns(&uses) {
            let who = c.audience();
            let (severity, code, message) = match c.otherwise {
                Mask::Deny if c.groups.is_empty() && c.users.is_empty() => (
                    Severity::Error,
                    "policy/denied-column",
                    format!(
                        "uses `{}` of `{}`, which the access policy denies to every viewer: this statement always fails",
                        c.column,
                        t.name()
                    ),
                ),
                Mask::Deny => (
                    Severity::Warning,
                    "policy/denied-column",
                    format!(
                        "uses `{}` of `{}`, which only {who} may see: this statement fails for every other viewer",
                        c.column,
                        t.name()
                    ),
                ),
                m => (
                    Severity::Note,
                    "policy/masked-column",
                    format!(
                        "uses `{}` of `{}`: viewers other than {who} see {}",
                        c.column,
                        t.name(),
                        match m {
                            Mask::Null => "NULL",
                            Mask::Mask => "'****'",
                            _ => "a hash",
                        }
                    ),
                ),
            };
            diags.push(Diagnostic {
                severity,
                stmt,
                code,
                message,
                sql: short.clone(),
            });
        }
    }
    diags
}

/// The longest `-- @description` (characters, its lines joined) the linter
/// takes without a warning.
pub const MAX_DESCRIPTION_CHARS: usize = 160;

/// The header comments that place and describe a dashboard (`-- @group`,
/// `-- @tags`, `-- @description`, `-- @owner`, `-- @certified`): an empty or
/// over-long group, more tags than are kept, a description over
/// [`MAX_DESCRIPTION_CHARS`], an empty owner, a certification date that
/// isn't `YYYY-MM-DD`. Reported as statement 0 (the header comes before the first
/// statement).
pub fn header_diags(script: &str) -> Vec<Diagnostic> {
    use crate::host::serving::{
        header_value, split_tags, valid_iso_date, MAX_GROUP_CHARS, MAX_TAGS,
    };
    let mut diags = Vec::new();
    let mut warn = |code, message: String, line: &str| {
        diags.push(Diagnostic {
            severity: Severity::Warning,
            stmt: 0,
            code,
            message,
            sql: one_line(line.trim()),
        })
    };
    let mut tags: Vec<String> = Vec::new();
    let mut tags_line = "";
    let (mut desc, mut desc_warned) = (0usize, false);
    for line in script.lines() {
        if let Some(g) = header_value(line, "group") {
            if g.split('/').all(|p| p.trim().is_empty()) {
                warn(
                    "header/empty-group",
                    "`-- @group` has no name — the dashboard is shown ungrouped".into(),
                    line,
                );
            } else if g.chars().count() > MAX_GROUP_CHARS {
                warn(
                    "header/long-group",
                    format!(
                        "`-- @group` is {} characters long — keep it to {MAX_GROUP_CHARS} or fewer \
                         (it is a sidebar heading)",
                        g.chars().count()
                    ),
                    line,
                );
            }
        } else if let Some(d) = header_value(line, "description") {
            desc += d.chars().count() + usize::from(desc > 0);
            if desc > MAX_DESCRIPTION_CHARS && !desc_warned {
                desc_warned = true;
                warn(
                    "header/long-description",
                    format!(
                        "`-- @description` is over {MAX_DESCRIPTION_CHARS} characters — keep it to \
                         one line (the landing page shows two lines of it)"
                    ),
                    line,
                );
            }
        } else if header_value(line, "owner") == Some("") {
            warn(
                "header/empty-owner",
                "`-- @owner` names nobody — write who looks after this dashboard, or drop the line"
                    .into(),
                line,
            );
        } else if let Some(d) = header_value(line, "certified").filter(|d| !valid_iso_date(d)) {
            warn(
                "header/bad-certified",
                format!(
                    "`-- @certified {d}` is not a date written YYYY-MM-DD (e.g. 2026-10-17) — \
                     the dashboard is shown as not certified"
                ),
                line,
            );
        } else if let Some(t) = header_value(line, "tags") {
            for t in split_tags(t) {
                if !tags.contains(&t) {
                    tags.push(t);
                }
            }
            tags_line = line;
        }
    }
    if tags.len() > MAX_TAGS {
        warn(
            "header/too-many-tags",
            format!(
                "{} tags in `-- @tags` — only the first {MAX_TAGS} are kept ({} dropped)",
                tags.len(),
                tags[MAX_TAGS..].join(", ")
            ),
            tags_line,
        );
    }
    diags
}

/// The files of a served folder, linted together: each `(file name, script)`
/// gets the diagnostics only the folder shows — a `-- @title` another file
/// has too (the nav and the `/` list would show the same name twice), and a
/// file name the servers skip (its id must be `[A-Za-z0-9_.-]`, at most 128,
/// not starting with `.`). Returned with the file each belongs to.
pub fn folder_diags(files: &[(String, String)]) -> Vec<(String, Diagnostic)> {
    use crate::host::serving::{parse_dashboard_meta, valid_dashboard_id};
    let mut out = Vec::new();
    let id_of = |f: &str| f.strip_suffix(".sql").unwrap_or(f).to_string();
    let mut titles: Vec<(String, String)> = Vec::new(); // (title, file)
    for (file, script) in files {
        let id = id_of(file);
        if !valid_dashboard_id(&id) {
            out.push((
                file.clone(),
                Diagnostic {
                    severity: Severity::Warning,
                    stmt: 0,
                    code: "folder/bad-id",
                    message: format!(
                        "`{file}` is not served: a dashboard id (the file name) may only use \
                         letters, digits, `_`, `-` and `.`"
                    ),
                    sql: String::new(),
                },
            ));
            continue;
        }
        let title = parse_dashboard_meta(&id, script).title;
        if let Some((_, first)) = titles.iter().find(|(t, _)| t.eq_ignore_ascii_case(&title)) {
            out.push((
                file.clone(),
                Diagnostic {
                    severity: Severity::Warning,
                    stmt: 0,
                    code: "folder/duplicate-title",
                    message: format!(
                        "`{title}` is also the title of `{first}` — viewers can't tell the two \
                         apart in the nav; give each its own `-- @title`"
                    ),
                    sql: String::new(),
                },
            ));
        } else {
            titles.push((title, file.clone()));
        }
    }
    out
}

/// A one-line summary: `N error(s), M warning(s)` (`, K note(s)`); empty script-clean message.
pub fn summary(diags: &[Diagnostic]) -> String {
    let e = diags
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .count();
    let n = diags
        .iter()
        .filter(|d| d.severity == Severity::Note)
        .count();
    let w = diags.len() - e - n;
    if diags.is_empty() {
        "clean — every panel plans, runs, and renders".into()
    } else if n > 0 {
        format!("{e} error(s), {w} warning(s), {n} note(s)")
    } else {
        format!("{e} error(s), {w} warning(s)")
    }
}

/// Whether a lint passes, as `dashboard --check` decides its exit code: no
/// error, and with `strict` also no `design/*` advisory and no `requires/*`
/// warning (for CI gating).
pub fn passes(diags: &[Diagnostic], strict: bool) -> bool {
    !diags.iter().any(|d| {
        d.severity == Severity::Error
            || (strict && (d.code.starts_with("design/") || d.code.starts_with("requires/")))
    })
}

impl Diagnostic {
    /// `{severity, stmt, code, message, sql}`, as `dashboard --check --json`.
    pub fn to_json(&self) -> Value {
        serde_json::json!({
            "severity": self.severity.label(),
            "stmt": self.stmt,
            "code": self.code,
            "message": self.message,
            "sql": self.sql,
        })
    }
}

/// The answers [`check_step`] replays: a query's text → its rows, or its
/// DuckDB error.
pub type Known = std::collections::HashMap<String, Result<Vec<Map<String, Value>>, String>>;

/// One step of [`check_opts`] for a caller whose queries are asynchronous
/// (the browser: DuckDB-Wasm can't answer a synchronous `run_query`).
#[derive(Debug, Clone)]
pub struct Step {
    /// Every query the lint ran was answered from `known`: the diagnostics
    /// are final.
    pub done: bool,
    /// The queries `known` had no answer for, in order (empty when done).
    pub need: Vec<String>,
    /// The diagnostics (empty until done).
    pub diagnostics: Vec<Diagnostic>,
}

/// Run [`check_opts`] answering each query from `known`. A query that isn't
/// there is recorded in [`Step::need`] and answered with no rows (as a setup
/// statement that succeeds), so the lint goes on and asks for the later
/// queries too — up to the first unknown query of an input or a
/// `::SELECT_AS` / `::DRILL`: the variables its rows set are in the prelude
/// of every later query, so those are asked for in the next step. The caller
/// runs the needed queries (each in a fresh session, as for [`check`]), adds
/// the answers to `known` and calls again, until `done`. The queries depend
/// only on the script and the earlier answers, so this ends, after about one
/// step per input or selection.
pub fn check_step(script: &str, known: &Known, opts: LintOptions) -> Step {
    // The statements whose rows set variables for the rest of the script: an
    // input's default, a ::SELECT_AS / ::DRILL's initial selection.
    let binders: Vec<String> = sql::plan(script)
        .iter()
        .filter(|p| {
            !p.setup
                && (input_kind(&p.roles).is_some()
                    || p.roles
                        .iter()
                        .any(|(_, r)| matches!(r, Role::SelectAs | Role::Drill)))
        })
        .map(|p| format!("{};", p.sql))
        .collect();
    let mut need: Vec<String> = Vec::new();
    let mut settled = true; // no unknown binder query yet: the prelude is right
    let diags = check_opts(
        script,
        |q| match known.get(q) {
            Some(answer) => answer.clone(),
            None => {
                if settled && !need.iter().any(|n| n == q) {
                    need.push(q.to_string());
                    settled = !binders.iter().any(|b| q.ends_with(b.as_str()));
                }
                Ok(Vec::new())
            }
        },
        opts,
    );
    let done = need.is_empty();
    Step {
        done,
        need,
        diagnostics: if done { diags } else { Vec::new() },
    }
}

/// [`check_step`] with JSON in and out (the wasm `lint_step`).
/// `known_json` = `{"<query>": {"rows": "<the rows as JSON text>"} |
/// {"error": "<DuckDB error>"}}` (`rows` is the same text `render_panel`
/// takes; bare `NaN` / `Infinity` are fine); `opts_json` = `{"design": bool,
/// "strict": bool}` (`--no-design`, `--strict`; both optional). Returns
/// `{"done", "need": […], "diagnostics": [{severity, stmt, code, message,
/// sql}], "ok", "summary", "spans": [[start, end], …]}`: `ok` and `summary`
/// as `dashboard --check` (its exit code and last line), meaningful once
/// `done`; `spans[n - 1]` is where statement `n` is in `script`, in UTF-16
/// code units (JavaScript string offsets), for jumping to it. A bad argument
/// comes back as `{"error": "…"}`.
pub fn step_json(script: &str, known_json: &str, opts_json: &str) -> String {
    let parse = |s: &str, what: &str| -> Result<Map<String, Value>, String> {
        if s.trim().is_empty() {
            return Ok(Map::new());
        }
        match serde_json::from_str(s) {
            Ok(Value::Object(m)) => Ok(m),
            Ok(_) => Err(format!("{what}: not a JSON object")),
            Err(e) => Err(format!("{what}: {e}")),
        }
    };
    let (known, opts) = match (parse(known_json, "known"), parse(opts_json, "opts")) {
        (Ok(k), Ok(o)) => (k, o),
        (Err(e), _) | (_, Err(e)) => return serde_json::json!({ "error": e }).to_string(),
    };
    let flag = |k: &str, d: bool| opts.get(k).and_then(Value::as_bool).unwrap_or(d);
    let mut answers = Known::new();
    for (q, a) in known {
        let answer = match (a.get("rows").and_then(Value::as_str), a.get("error")) {
            (Some(rows), _) => sql::parse_rows_json(rows),
            (None, Some(e)) => Err(e.as_str().map_or_else(|| e.to_string(), str::to_string)),
            (None, None) => Err("lint_step: an answer needs `rows` or `error`".into()),
        };
        answers.insert(q, answer);
    }
    let step = check_step(
        script,
        &answers,
        LintOptions {
            design: flag("design", true),
        },
    );
    serde_json::json!({
        "done": step.done,
        "need": step.need,
        "diagnostics": step.diagnostics.iter().map(Diagnostic::to_json).collect::<Vec<_>>(),
        "ok": passes(&step.diagnostics, flag("strict", false)),
        "summary": summary(&step.diagnostics),
        "spans": sql::lex::statement_spans(script).iter().map(|&(a, b)| {
            let utf16 = |i: usize| script[..i].encode_utf16().count();
            [utf16(a), utf16(b)]
        }).collect::<Vec<_>>(),
    })
    .to_string()
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

/// `{{col:fmt}}` formats the browser knows (`web/app.js` `MD_FMT` + date / text).
const MD_FORMATS: &[&str] = &[
    "money", "currency", "usd", "eur", "dollar", "percent", "pct", "compact", "metric", "number",
    "num", "date", "text",
];

/// `c0`, `c1`, …: the rewrite's positional output names (`AS c{i}`).
fn is_positional_key(k: &str) -> bool {
    k.len() > 1
        && k.as_bytes()[0].eq_ignore_ascii_case(&b'c')
        && k[1..].bytes().all(|b| b.is_ascii_digit())
}

/// The row key a `{{name}}` placeholder reads, as `markdownWithValues` in
/// `web/app.js` resolves it: the statement's own `"md:name"` column, else a
/// `*`-expanded column of that name — never a positional `c{i}` key (#138).
fn md_column<'a>(keys: impl Iterator<Item = &'a String> + Clone, name: &str) -> Option<&'a String> {
    let pre = crate::sql::MD_PREFIX;
    keys.clone()
        .find(|k| {
            k.len() > pre.len() && k.starts_with(pre) && k[pre.len()..].eq_ignore_ascii_case(name)
        })
        .or_else(|| {
            keys.into_iter().find(|k| {
                !k.starts_with(pre) && !is_positional_key(k) && k.eq_ignore_ascii_case(name)
            })
        })
}

/// The `{{name}}` / `{{name:fmt}}` placeholders of a Markdown text.
fn md_placeholders(md: &str) -> Vec<(String, Option<String>)> {
    let mut out = Vec::new();
    let mut rest = md;
    while let Some(a) = rest.find("{{") {
        let Some(b) = rest[a + 2..].find("}}") else {
            break;
        };
        let inner = &rest[a + 2..a + 2 + b];
        rest = &rest[a + 2 + b + 2..];
        let (name, spec) = match inner.split_once(':') {
            Some((n, f)) => (n.trim(), Some(f.trim().to_string())),
            None => (inner.trim(), None),
        };
        let ident = name.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
            && name
                .chars()
                .all(|c| c.is_alphanumeric() || c == '_' || c == ' ');
        if ident {
            out.push((name.to_string(), spec));
        }
    }
    out
}

/// `::CALLOUT` kinds the browser draws (`info` = note, `tip` = success,
/// `caution` / `danger` = warning).
const CALLOUT_KINDS: &[&str] = &[
    "note", "info", "warning", "caution", "danger", "success", "tip",
];

/// The non-empty text cells of result column `idx` (`c{idx}`, else by position).
fn column_texts(rows: &[Map<String, Value>], idx: usize) -> Vec<String> {
    rows.iter()
        .filter_map(|row| {
            row.get(&format!("c{idx}"))
                .or_else(|| row.values().nth(idx))
        })
        .filter_map(|v| match v {
            Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
            _ => None,
        })
        .collect()
}

/// Motion directives the browser would ignore: an unknown `::ANIMATE` mode, a
/// `'reveal'` with no `::XLINE` to start from, an unknown `::LINESTYLE`, a
/// `::STORY_BY` that is not `'x'` / `'series'` (or `'series'` with no
/// `::CATEGORY`), an unknown `::TRANSITION`.
fn motion_diags(
    roles: &[(usize, Role)],
    rows: &[Map<String, Value>],
) -> Vec<(&'static str, String)> {
    let mut out = Vec::new();
    let has = |want: Role| roles.iter().any(|(_, r)| *r == want);
    for (idx, r) in roles {
        let texts = column_texts(rows, *idx);
        let Some(v) = texts.first().map(|t| t.to_ascii_lowercase()) else {
            continue;
        };
        // a ::DIAGRAM's edge styles are checked as diagram/bad-style
        if *r == Role::LineStyle && !has(Role::Diagram) {
            if let Some(bad) = texts
                .iter()
                .map(|t| t.to_ascii_lowercase())
                .find(|t| !crate::LINE_STYLES.contains(&t.as_str()))
            {
                out.push((
                    "linestyle/unknown",
                    format!(
                        "::LINESTYLE `{bad}` is not a style ({}) — that series stays solid",
                        crate::LINE_STYLES.join(", ")
                    ),
                ));
            }
        }
        if *r == Role::Transition && v != "tween" && v != "none" {
            out.push((
                "transition/unknown",
                format!("::TRANSITION `{v}` is not 'tween' or 'none' — the chart is swapped"),
            ));
        }
        if *r == Role::StoryBy {
            if v != "x" && v != "series" {
                out.push((
                    "story/bad-by",
                    format!("::STORY_BY `{v}` is not 'x' or 'series' — steps light up their x"),
                ));
            } else if v == "series" && !has(Role::Category) {
                out.push((
                    "story/bad-by",
                    "::STORY_BY 'series' needs a ::CATEGORY — steps light up their x".to_string(),
                ));
            }
        }
        if *r == Role::Animate {
            if !crate::ANIMATE_MODES.contains(&v.as_str()) {
                out.push((
                    "animate/unknown",
                    format!(
                        "::ANIMATE `{v}` is not a mode ({}) — the chart plays 'auto'",
                        crate::ANIMATE_MODES.join(", ")
                    ),
                ));
            } else if v == "reveal" && !has(Role::VLine) && !has(Role::Diagram) {
                out.push((
                    "animate/reveal-no-xline",
                    "::ANIMATE 'reveal' grows forward from the panel's ::XLINE (e.g. Today); \
                     without one it wipes in from the left"
                        .to_string(),
                ));
            }
        }
    }
    out
}

/// Map modifiers the renderer would ignore or that mislead: an unknown
/// `::PROJECTION`, a bad `::CLASSES` spec, a Mercator choropleth (areas
/// grow towards the poles) and more point marks than a map draws well.
fn map_diags(
    roles: &[(usize, Role)],
    rows: &[Map<String, Value>],
) -> Vec<(Severity, &'static str, String)> {
    let mut out = Vec::new();
    let Some(geom) = roles
        .iter()
        .find(|(i, r)| {
            *r == Role::Geometry
                && column_texts(rows, *i)
                    .iter()
                    .any(|t| t.trim().parse::<f64>().is_err())
        })
        .map(|(i, _)| *i)
    else {
        return out;
    };
    let first = |want: Role| {
        roles
            .iter()
            .find(|(_, r)| *r == want)
            .and_then(|(i, _)| column_texts(rows, *i).into_iter().next())
    };
    let projection = first(Role::Projection);
    if let Some(p) = &projection {
        if crate::map::project::Projection::parse(p).is_none() {
            out.push((
                Severity::Warning,
                "map/unknown-projection",
                format!(
                    "::PROJECTION '{p}' is not one of {} — the map is drawn with 'auto'",
                    crate::map::project::Projection::NAMES.join(", ")
                ),
            ));
        }
    }
    if let Some(c) = first(Role::Classes) {
        if let Err(e) = crate::map::classes::Method::parse(&c) {
            out.push((
                Severity::Warning,
                "map/bad-classes",
                format!("{e} — the map is drawn with a continuous ramp"),
            ));
        }
    }
    let wkt = column_texts(rows, geom);
    let filled = roles.iter().any(|(i, r)| {
        matches!(r, Role::Value(_))
            || (*r == Role::Geometry && *i != geom && !column_texts(rows, *i).is_empty())
    });
    let areas = wkt
        .iter()
        .any(|t| t.trim_start().to_ascii_uppercase().contains("POLYGON"));
    if filled
        && areas
        && projection
            .as_deref()
            .and_then(crate::map::project::Projection::parse)
            == Some(crate::map::project::Projection::Mercator)
    {
        out.push((
            Severity::Note,
            "map/mercator-choropleth",
            "a choropleth in 'mercator' inflates areas towards the poles — 'auto' or \
             'equal-earth' keeps them comparable"
                .to_string(),
        ));
    }
    let points: usize = wkt
        .iter()
        .map(|t| match ggplot_rs::spatial::parse_wkt(t) {
            Some(ggplot_rs::spatial::Geometry::Point(_)) => 1,
            Some(ggplot_rs::spatial::Geometry::MultiPoint(p)) => p.len(),
            _ => 0,
        })
        .sum();
    if points > MAP_POINTS_WARN {
        out.push((
            Severity::Warning,
            "map/too-many-marks",
            format!(
                "{points} points on one map (over {MAP_POINTS_WARN}) — aggregate in SQL, e.g. to H3 \
                 cells (the h3 extension) or to regions, to keep the map light"
            ),
        ));
    }
    out
}

/// Point marks above which a map is linted as too dense.
const MAP_POINTS_WARN: usize = 20_000;

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
/// In a `::TABLE` / `::PAGED` statement those are column formats, not tiles.
fn is_kpi(roles: &[(usize, Role)]) -> bool {
    roles.iter().any(|(_, r)| matches!(r, Role::Metric(_)))
        && !roles
            .iter()
            .any(|(_, r)| matches!(r, Role::Diagram | Role::Table | Role::PagedTable))
}

/// A statement that shows a content panel or KPI tile: not an input, a layout
/// directive, a section heading (a lone ::LABEL) or a ::TRACK.
fn is_content_panel(roles: &[(usize, Role)]) -> bool {
    input_kind(roles).is_none()
        && !roles::is_directive_panel(roles)
        && !(roles.len() == 1 && matches!(roles[0].1, Role::Label))
        && !crate::track::is_track(roles)
}

/// The design counters for one panel: a top-level KPI tile extends the run of
/// tiles; any other panel counts toward the scan budget and ends the run.
fn count_design_panel(
    roles: &[(usize, Role)],
    top_level: bool,
    stmt: usize,
    kpi_run: &mut usize,
    kpi_run_stmt: &mut usize,
    rendered_panels: &mut usize,
    diags: &mut Vec<Diagnostic>,
) {
    if is_kpi(roles) {
        if top_level {
            if *kpi_run == 0 {
                *kpi_run_stmt = stmt;
            }
            *kpi_run += 1;
        }
    } else {
        *rendered_panels += 1;
        flush_kpi_run(diags, kpi_run, *kpi_run_stmt);
    }
}

/// A KPI statement's `::DELTA_FORMAT` / `::DELTA_LABEL` / `::SPARKLINE`
/// columns: `kpi/bad-delta-format` (an unknown format — 'pct' is used),
/// `kpi/no-delta` (a format or label but no `::DELTA`), `kpi/sparkline-not-list`
/// (the trend column is not a `list()`).
fn kpi_checks(
    roles: &[(usize, Role)],
    rows: &[Map<String, Value>],
) -> Vec<(Severity, &'static str, String)> {
    let mut out = Vec::new();
    let cell = |want: &dyn Fn(Role) -> bool| {
        roles
            .iter()
            .find(|(_, r)| want(*r))
            .map(|(i, _)| rows.first().and_then(|r| r.get(&format!("c{i}"))))
    };
    if let Some(Some(Value::String(f))) = cell(&|r| r == Role::DeltaFormat) {
        if crate::kpi::DeltaFormat::parse(f).is_none() {
            out.push((
                Severity::Error,
                "kpi/bad-delta-format",
                format!(
                    "::DELTA_FORMAT `{f}` is not one of {} — the change is shown as a %",
                    crate::kpi::DeltaFormat::NAMES.join(", ")
                ),
            ));
        }
    }
    let has = |want: &dyn Fn(Role) -> bool| roles.iter().any(|(_, r)| want(*r));
    if has(&|r| matches!(r, Role::DeltaFormat | Role::DeltaLabel))
        && !has(&|r| matches!(r, Role::Delta | Role::DeltaLowerGood))
    {
        out.push((
            Severity::Warning,
            "kpi/no-delta",
            "::DELTA_FORMAT / ::DELTA_LABEL without a ::DELTA comparison column — nothing              to format or label"
                .into(),
        ));
    }
    if let Some(Some(v)) = cell(&|r| r == Role::Value(Kind::Sparkline)) {
        let list = match v {
            Value::Array(_) => true,
            Value::String(s) => serde_json::from_str::<Vec<Value>>(s).is_ok(),
            _ => false,
        };
        if !list && !v.is_null() {
            out.push((
                Severity::Warning,
                "kpi/sparkline-not-list",
                "a KPI's ::SPARKLINE must be a list of numbers — e.g.                  list(v ORDER BY d)::SPARKLINE"
                    .into(),
            ));
        }
    }
    out
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
            | Kind::Waterfall
            | Kind::Bullet
            | Kind::Funnel
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
/// What a `::TRACK` after this panel would sit under (see `track_host`).
fn track_host_of(roles: &[(usize, Role)], rows: &[Map<String, Value>]) -> Option<bool> {
    let has = |f: &dyn Fn(&Role) -> bool| roles.iter().any(|(_, r)| f(r));
    if !has(&|r| matches!(r, Role::Value(_)))
        || has(&|r| matches!(r, Role::Table | Role::PagedTable | Role::Markdown))
        || is_kpi(roles)
    {
        return None;
    }
    let x = role_index(roles, Role::X)?;
    Some(!col_is_discrete(rows, x) || col_is_temporal(rows, x))
}

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

/// A failed statement: an error, or — for a `-- @requires` statement whose
/// input is missing — a note (the panel reads "Not available").
fn sql_error(
    stmt: usize,
    requires: &[sql::requires::Requirement],
    e: &str,
    short: &str,
) -> Diagnostic {
    let optional = !requires.is_empty() && sql::is_missing_input_error(e);
    Diagnostic {
        severity: if optional {
            Severity::Note
        } else {
            Severity::Error
        },
        stmt,
        code: if optional {
            "optional/missing"
        } else {
            "sql-error"
        },
        message: if optional {
            format!(
                "optional input missing — the panel shows \"Not available\": {}",
                one_line(e)
            )
        } else {
            one_line(e)
        },
        sql: short.to_string(),
    }
}

/// Does a statement read a file: a `read_…` / `glob` / `parquet_…` table
/// function, or a quoted path with a data-file extension (`FROM 'x.parquet'`)?
/// (`requires/on-setup`; a `SET VARIABLE` that builds a path reads nothing.)
fn reads_a_file(sql: &str) -> bool {
    let code = sql.to_ascii_lowercase();
    if code.trim_start().starts_with("set ") {
        return false;
    }
    ["read_", "glob(", "parquet_", "sniff_csv("]
        .iter()
        .any(|f| code.contains(f))
        || [
            ".parquet'",
            ".csv'",
            ".json'",
            ".jsonl'",
            ".tsv'",
            ".xlsx'",
            ".ndjson'",
        ]
        .iter()
        .any(|e| code.contains(e))
}

/// Check `requires` the way the locked server does (see
/// [`sql::requires::check`]), with the prelude's session state.
fn unmet_requirement<F>(
    run_query: &mut F,
    prelude: &str,
    requires: &[sql::requires::Requirement],
) -> sql::requires::Unmet
where
    F: FnMut(&str) -> Result<Vec<Map<String, Value>>, String>,
{
    sql::requires::check(requires, |q, session| {
        let rows = run_query(&if session {
            format!("{prelude}{q};")
        } else {
            q.to_string()
        })?;
        Ok(rows
            .into_iter()
            .next()
            .unwrap_or_default()
            .into_iter()
            .map(|(k, v)| {
                let v = match v {
                    Value::Null => None,
                    Value::String(s) => Some(s),
                    v => Some(v.to_string()),
                };
                (k, v)
            })
            .collect())
    })
}

/// `requires/tab-always-hidden`: a `${name}` of a tab's `-- @requires` has no
/// value where the tab is decided. When a statement after the tab sets it
/// (`SET VARIABLE name = …`), the browser has it (it runs every SET VARIABLE
/// first) but a locked server decides the tab before that statement: the tab
/// is hidden there whatever the data.
fn tab_always_hidden(stmt: usize, unknown: &[String], after: &[String], short: &str) -> Diagnostic {
    let set_later = unknown.iter().find(|v| {
        after.iter().any(|raw| {
            let w: Vec<&str> = raw.split_whitespace().take(3).collect();
            w.len() == 3
                && w[0].eq_ignore_ascii_case("SET")
                && w[1].eq_ignore_ascii_case("VARIABLE")
                && w[2].trim_end_matches('=').eq_ignore_ascii_case(v)
        })
    });
    Diagnostic {
        severity: Severity::Warning,
        stmt,
        code: if set_later.is_some() {
            "requires/tab-always-hidden"
        } else {
            "requires/unknown-variable"
        },
        message: match set_later {
            Some(v) => format!(
                "`${{{v}}}` in this tab's `-- @requires` is set only below the tab — a locked \
                 server decides the tab before that and always hides it; move the SET VARIABLE \
                 above the ::TAB"
            ),
            None => format!(
                "`${{{}}}` in this tab's `-- @requires` names no variable set here (a SET \
                 VARIABLE above, an input, or `anofox_data`, which `dashboard --check --data \
                 <dir>` and the locked server set) — the tab is hidden",
                unknown.join("}`, `${")
            ),
        },
        sql: short.to_string(),
    }
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
    fn optional_inputs_are_notes_and_empty_messages_silence_empty_panels() {
        // A missing `-- @requires` file: a note, the statement is skipped.
        let d = check(
            "-- @requires data/run/opt.parquet\nSELECT a ::XAXIS, b ::BARCHART FROM 'data/run/opt.parquet';",
            |q| {
                assert!(
                    q.contains("glob(") || q.contains("getvariable("),
                    "the panel itself must not run: {q}"
                );
                Ok(vec![json!({ "n": 0 }).as_object().unwrap().clone()])
            },
        );
        assert_eq!(d.len(), 1, "{d:?}");
        assert_eq!(
            (d[0].code, d[0].severity),
            ("optional/missing", Severity::Note)
        );
        // Present per glob but the query still can't find it → a note too.
        let d = check(
            "-- @requires t\nSELECT a ::XAXIS, b ::BARCHART FROM t;",
            |q| {
                if q.contains("glob(") {
                    Ok(vec![json!({ "n": 1 }).as_object().unwrap().clone()])
                } else {
                    Err("Catalog Error: Table with name t does not exist!".into())
                }
            },
        );
        assert_eq!(
            (d[0].code, d[0].severity),
            ("optional/missing", Severity::Note)
        );
        // Without @requires the same failure is an error.
        let d = check("SELECT a ::XAXIS, b ::BARCHART FROM t;", |_| {
            Err("Catalog Error: Table with name t does not exist!".into())
        });
        assert_eq!((d[0].code, d[0].severity), ("sql-error", Severity::Error));
        // ::EMPTY before a panel: 0 rows are expected, no empty-panel warning.
        let d = lint_one(
            "SELECT 'Nothing yet'::EMPTY;\nSELECT a ::XAXIS, b ::BARCHART FROM t;",
            vec![],
        );
        assert!(!has(&d, "empty-panel"), "{d:?}");
        let d = lint_one("SELECT a ::XAXIS, b ::BARCHART FROM t;", vec![]);
        assert!(has(&d, "empty-panel"), "{d:?}");
    }

    /// A fake session for `-- @requires`: `vars` are the variables the
    /// prelude sets, `files` the paths `glob` finds; any other query returns
    /// one row (so a panel that runs is not empty).
    fn requires_session<'a>(
        vars: &'a [(&'a str, &'a str)],
        files: &'a [&'a str],
        globbed: &'a std::cell::RefCell<Vec<String>>,
    ) -> impl FnMut(&str) -> Result<Vec<Map<String, Value>>, String> + 'a {
        move |q: &str| {
            let q = q.rsplit(";\n").next().unwrap_or(q);
            if let Some(rest) = q.split("FROM glob('").nth(1) {
                let path = rest
                    .trim_end_matches(';')
                    .trim_end_matches("')")
                    .replace("''", "'");
                globbed.borrow_mut().push(path.clone());
                let n = files.iter().filter(|f| **f == path).count();
                return Ok(vec![json!({ "n": n.to_string() })
                    .as_object()
                    .unwrap()
                    .clone()]);
            }
            if q.starts_with("SELECT CAST(getvariable(") {
                let mut row = Map::new();
                for (i, part) in q.split("getvariable('").skip(1).enumerate() {
                    let name = part.split('\'').next().unwrap();
                    if let Some((_, v)) = vars.iter().find(|(n, _)| *n == name) {
                        row.insert(format!("v{i}"), Value::String(v.to_string()));
                    }
                }
                return Ok(vec![row]);
            }
            Ok(vec![json!({ "c0": "a", "c1": 1 })
                .as_object()
                .unwrap()
                .clone()])
        }
    }

    #[test]
    fn box_statistics_are_checked() {
        let row = |q1: f64, m: f64, q3: f64| {
            json!({ "c0": "a", "c1": q1, "c2": m, "c3": q3, "c4": 0.0, "c5": 9.0 })
                .as_object()
                .unwrap()
                .clone()
        };
        let sql = "SELECT g::XAXIS, a::Q1, b::MEDIAN, c::Q3, lo::WHISKER_LOW, hi::WHISKER_HIGH, \
                   'Error by method'::TITLE FROM t;";
        let codes = |d: &[Diagnostic]| d.iter().map(|d| d.code).collect::<Vec<_>>();
        assert!(lint_one(sql, vec![row(1.0, 2.0, 3.0)]).is_empty());
        let d = lint_one(sql, vec![row(1.0, 4.0, 3.0)]);
        assert_eq!(codes(&d), ["box/unordered"], "{d:?}");
        assert_eq!(d[0].severity, Severity::Warning);
        let d = lint_one(sql, vec![row(-1.0, 2.0, 3.0)]);
        assert_eq!(codes(&d), ["box/unordered"], "{d:?}");
        assert!(d[0].message.contains("whisker"), "{}", d[0].message);
        // A missing quartile: one error, not a render error as well.
        let d = lint_one(
            "SELECT g::XAXIS, a::Q1, b::MEDIAN FROM t;",
            vec![row(1.0, 2.0, 3.0)],
        );
        assert_eq!(codes(&d), ["box/missing"], "{d:?}");
        assert_eq!(d[0].severity, Severity::Error);
        assert!(d[0].message.contains("::Q3"), "{}", d[0].message);
    }

    #[test]
    fn requires_expands_variables_and_takes_alternatives() {
        let panel = "SELECT a ::XAXIS, b ::BARCHART FROM t;";
        let globbed = std::cell::RefCell::new(Vec::new());
        let lint = |req: &str, vars: &[(&str, &str)], files: &[&str]| {
            globbed.borrow_mut().clear();
            check_opts(
                &format!("-- @requires {req}\n{panel}"),
                requires_session(vars, files, &globbed),
                LintOptions { design: false },
            )
        };
        let codes = |d: &[Diagnostic]| d.iter().map(|d| d.code).collect::<Vec<_>>();
        let vars = [("rd", "/srv/runs/r1"), ("anofox_data", "/srv/runs")];
        // `${rd}` expands; the second alternative exists → met.
        let d = lint(
            "${rd}/_anofox_meta_forecasts.parquet | ${rd}/forecasts.parquet",
            &vars,
            &["/srv/runs/r1/forecasts.parquet"],
        );
        assert!(d.is_empty(), "{d:?}");
        assert_eq!(
            *globbed.borrow(),
            [
                "/srv/runs/r1/_anofox_meta_forecasts.parquet",
                "/srv/runs/r1/forecasts.parquet"
            ]
        );
        // Neither alternative → the note names the requirement as written.
        let d = lint("${rd}/a.parquet | ${rd}/b.parquet", &vars, &[]);
        assert_eq!(codes(&d), ["optional/missing"]);
        assert!(
            d[0].message.contains("${rd}/a.parquet | ${rd}/b.parquet"),
            "{}",
            d[0].message
        );
        // Every requirement must be met.
        let d = lint("${rd}/a, ${rd}/b", &vars, &["/srv/runs/r1/a"]);
        assert_eq!(codes(&d), ["optional/missing"]);
        // A relative path resolves against anofox_data …
        let d = lint("r1/a", &vars, &["/srv/runs/r1/a"]);
        assert!(d.is_empty(), "{d:?}");
        // … and stays relative (the working directory) without it.
        let d = lint("r1/a", &[], &["r1/a"]);
        assert!(d.is_empty(), "{d:?}");
        // An unknown variable: a warning, and the path counts as missing.
        let d = lint("${nope}/a", &vars, &[]);
        assert_eq!(codes(&d), ["requires/unknown-variable", "optional/missing"]);
        // … unless another alternative is met.
        let d = lint("${nope}/a | ${rd}/a", &vars, &["/srv/runs/r1/a"]);
        assert_eq!(codes(&d), ["requires/unknown-variable"]);
        // On a setup statement that reads no file: a warning; one that
        // reads the file is fine.
        let set = |stmt: &str| {
            check_opts(
                &format!("-- @requires ${{rd}}/a\n{stmt}"),
                requires_session(&vars, &["/srv/runs/r1/a"], &globbed),
                LintOptions { design: false },
            )
        };
        assert_eq!(
            codes(&set("SET VARIABLE region = 'EU';")),
            ["requires/on-setup"]
        );
        assert_eq!(
            codes(&set(
                "CREATE TEMP VIEW v AS SELECT * FROM t WHERE r = 'EU';"
            )),
            ["requires/on-setup"]
        );
        assert!(set(
            "CREATE TEMP VIEW v AS SELECT * FROM read_parquet(getvariable('rd') || '/a');"
        )
        .is_empty());
        assert!(set("CREATE TEMP VIEW v AS SELECT * FROM 'data/a.parquet';").is_empty());
        // Malformed lines.
        let d = lint("${rd}/a |", &vars, &["/srv/runs/r1/a"]);
        assert_eq!(codes(&d), ["requires/empty-alternative"]);
        let d = lint("${rd/a", &vars, &[]);
        assert_eq!(codes(&d), ["requires/bad-variable", "optional/missing"]);
        assert!(d.iter().all(|d| d.severity != Severity::Error), "{d:?}");
    }

    #[test]
    fn a_tab_with_a_missing_input_is_hidden() {
        let globbed = std::cell::RefCell::new(Vec::new());
        let vars = [("rd", "/srv/runs/r1"), ("anofox_data", "/srv/runs")];
        let lint = |script: &str| {
            check_opts(
                script,
                requires_session(&vars, &["/srv/runs/r1/here.parquet"], &globbed),
                LintOptions { design: false },
            )
        };
        let codes = |d: &[Diagnostic]| d.iter().map(|d| (d.stmt, d.code)).collect::<Vec<_>>();
        // One note at the tab; its panels (with or without their own
        // @requires) are left out; the next tab is checked as usual.
        let d = lint(
            "-- @requires ${rd}/gone.parquet\nSELECT 'Forecastability'::TAB;\n\
             SELECT a ::XAXIS, b ::BARCHART FROM t;\n\
             -- @requires ${rd}/gone.parquet\nSELECT a ::XAXIS, b ::BARCHART FROM t;\n\
             SELECT 'Inner'::SUBTAB;\nSELECT a ::XAXIS, b ::BARCHART FROM t;\n\
             SELECT 'Other'::TAB;\n\
             -- @requires ${rd}/gone.parquet\nSELECT a ::XAXIS, b ::BARCHART FROM t;",
        );
        assert_eq!(
            codes(&d),
            [(1, "optional/missing"), (7, "optional/missing")],
            "{d:?}"
        );
        assert!(d[0].message.contains("::TAB is hidden"), "{}", d[0].message);
        assert!(d[1].message.contains("Not available"), "{}", d[1].message);
        // A sub-tab: hidden alone, its siblings stay.
        let d = lint(
            "SELECT 'A'::TAB;\n\
             -- @requires ${rd}/here.parquet\nSELECT 'Shown'::SUBTAB;\nSELECT a ::XAXIS, b ::BARCHART FROM t;\n\
             -- @requires ${rd}/gone.parquet\nSELECT 'Hidden'::SUBTAB;\nSELECT a ::XAXIS, b ::BARCHART FROM t;\n\
             SELECT 'Next'::SUBTAB;\nSELECT a ::XAXIS, b ::BARCHART FROM t;",
        );
        assert_eq!(codes(&d), [(4, "optional/missing")], "{d:?}");
        assert!(
            d[0].message.contains("::SUBTAB is hidden"),
            "{}",
            d[0].message
        );
        // A setup statement in a hidden tab still runs; reading the missing
        // input, it fails as expected (a note, not an error).
        let d = check_opts(
            "-- @requires gone.parquet\nSELECT 'A'::TAB;\n\
             CREATE TEMP VIEW v AS SELECT * FROM 'gone.parquet';",
            |q: &str| {
                if q.contains("glob(") {
                    Ok(vec![json!({ "n": 0 }).as_object().unwrap().clone()])
                } else if q.contains("CREATE") {
                    Err("IO Error: No files found that match the pattern \"gone.parquet\"".into())
                } else {
                    Ok(vec![Map::new()])
                }
            },
            LintOptions { design: false },
        );
        assert_eq!(
            codes(&d),
            [(1, "optional/missing"), (2, "optional/missing")],
            "{d:?}"
        );
        // `${x}` set only below the tab: a locked server always hides it.
        let d = lint(
            "-- @requires ${later}/here.parquet\nSELECT 'A'::TAB;\n\
             SET VARIABLE later = '/srv/runs/r1';\nSELECT a ::XAXIS, b ::BARCHART FROM t;",
        );
        assert_eq!(
            codes(&d),
            [(1, "requires/tab-always-hidden"), (1, "optional/missing")],
            "{d:?}"
        );
        let d = lint(
            "-- @requires ${nope}/x\nSELECT 'A'::TAB;\nSELECT a ::XAXIS, b ::BARCHART FROM t;",
        );
        assert_eq!(
            codes(&d),
            [(1, "requires/unknown-variable"), (1, "optional/missing")],
            "{d:?}"
        );
    }

    #[test]
    fn owner_certified_and_folders_are_checked() {
        let codes = |script: &str| -> Vec<&'static str> {
            header_diags(script).iter().map(|d| d.code).collect()
        };
        assert!(codes("-- @owner Finance\n-- @certified 2026-10-15\nSELECT 1;").is_empty());
        assert_eq!(codes("-- @owner \nSELECT 1;"), ["header/empty-owner"]);
        assert_eq!(
            codes("-- @certified 2026-02-30\n"),
            ["header/bad-certified"]
        );
        assert_eq!(codes("-- @certified\n"), ["header/bad-certified"]);
        assert!(codes(&format!("-- @description {}\n", "x".repeat(160))).is_empty());
        let long = format!(
            "-- @description {}\n-- @description {}\n",
            "x".repeat(100),
            "y".repeat(100)
        );
        assert_eq!(codes(&long), ["header/long-description"]);
        let f = |n: &str, s: &str| (n.to_string(), s.to_string());
        let d = folder_diags(&[
            f("a.sql", "-- @title Sales\nSELECT 1;"),
            f("b.sql", "-- @title sales\nSELECT 1;"),
            f("c.sql", "SELECT 1;"),
            f("c2.sql", "-- @title c\nSELECT 1;"),
            f("bad name.sql", "-- @title Other\nSELECT 1;"),
            f(".hidden.sql", "SELECT 1;"),
        ]);
        let got: Vec<(&str, &str)> = d.iter().map(|(f, d)| (f.as_str(), d.code)).collect();
        assert_eq!(
            got,
            [
                ("b.sql", "folder/duplicate-title"),
                ("c2.sql", "folder/duplicate-title"),
                ("bad name.sql", "folder/bad-id"),
                (".hidden.sql", "folder/bad-id"),
            ]
        );
        assert!(d[0].1.message.contains("a.sql"), "{}", d[0].1.message);
    }

    #[test]
    fn header_group_and_tags_are_checked() {
        let codes = |script: &str| -> Vec<&'static str> {
            header_diags(script).iter().map(|d| d.code).collect()
        };
        assert!(codes("-- @group Sales / EMEA\n-- @tags a, b\nSELECT 1;").is_empty());
        assert_eq!(codes("-- @group\nSELECT 1;"), ["header/empty-group"]);
        assert_eq!(codes("-- @group  / \n"), ["header/empty-group"]);
        let long = format!("-- @group {}\n", "x".repeat(81));
        assert_eq!(codes(&long), ["header/long-group"]);
        assert!(codes(&format!("-- @group {}\n", "ä".repeat(80))).is_empty());
        let tags: Vec<String> = (0..11).map(|i| format!("t{i}")).collect();
        let d = header_diags(&format!("-- @tags {}\nSELECT 1;", tags.join(", ")));
        assert_eq!(d.len(), 1);
        assert_eq!((d[0].code, d[0].stmt), ("header/too-many-tags", 0));
        assert!(d[0].message.contains("t10"), "{}", d[0].message);
        // two lines add up; duplicates (any case) count once
        let a = (0..6)
            .map(|i| format!("T{i}"))
            .collect::<Vec<_>>()
            .join(",");
        let b = (3..10)
            .map(|i| format!("t{i}"))
            .collect::<Vec<_>>()
            .join(",");
        assert!(codes(&format!("-- @tags {a}\n-- @tags {b}\n")).is_empty());
        // …and check() reports them too
        assert!(has(
            &check("-- @group\nSELECT 1;", |_| Ok(vec![])),
            "header/empty-group"
        ));
    }

    #[test]
    fn threshold_rules_are_checked() {
        let kpi = |spec: &str| rows(1, |_| json!({ "c0": 0.12, "c1": spec }));
        let d = lint_one(
            "SELECT 0.12::PERCENT, '<0:purple'::THRESHOLDS;",
            kpi("<0:purple"),
        );
        assert!(has(&d, "thresholds/bad-rule"), "{d:?}");
        let ok = lint_one(
            "SELECT 0.12::PERCENT, '<0:bad; good'::THRESHOLDS;",
            kpi("<0:bad; good"),
        );
        assert!(!has(&ok, "thresholds/bad-rule"), "{ok:?}");
        let lone = rows(1, |_| json!({ "c0": "<0:bad; good" }));
        let d = lint_one("SELECT '<0:bad; good'::THRESHOLDS;", lone);
        assert!(has(&d, "thresholds/no-column"), "{d:?}");
        let d = lint_one(
            "SELECT region::TABLE, m, '<0:bad'::THRESHOLDS FROM t;",
            rows(2, |i| json!({ "region": "N", "m": i, "x": "<0:bad" })),
        );
        assert!(has(&d, "thresholds/in-table"), "{d:?}");
    }

    #[test]
    fn a_colors_level_map_must_parse_and_name_a_category() {
        let data = |spec: &str| {
            rows(
                2,
                |i| json!({ "c0": i, "c1": i, "c2": "Critical", "c3": spec }),
            )
        };
        let sql =
            "SELECT x::XAXIS, y::SCATTER, s::CATEGORY, 'Critical:bad; OK:good'::COLORS FROM t;";
        let ok = lint_one(sql, data("Critical:bad; OK:good"));
        assert!(
            !has(&ok, "colors/bad-map") && !has(&ok, "colors/no-category"),
            "{ok:?}"
        );
        let d = lint_one(sql, data("Critical:purple"));
        assert!(has(&d, "colors/bad-map"), "{d:?}");
        let d = lint_one(
            "SELECT x::XAXIS, y::SCATTER, 'Critical:bad'::COLORS FROM t;",
            rows(2, |i| json!({ "c0": i, "c1": i, "c2": "Critical:bad" })),
        );
        assert!(has(&d, "colors/no-category"), "{d:?}");
    }

    #[test]
    fn an_unpaired_annotation_role_warns() {
        let data = rows(3, |i| json!({ "c0": i, "c1": i, "c2": "Launch", "c3": 1 }));
        let d = lint_one(
            "SELECT x::XAXIS, y::LINECHART, 'Launch'::XLINE_LABEL, 'T'::TITLE FROM t;",
            data.clone(),
        );
        assert!(has(&d, "annotation/unpaired"), "{d:?}");
        let ok = lint_one(
            "SELECT x::XAXIS, y::LINECHART, 'Launch'::XLINE_LABEL, 1::XLINE FROM t;",
            data,
        );
        assert!(!has(&ok, "annotation/unpaired"), "{ok:?}");
    }

    #[test]
    fn unequal_band_edges_warn() {
        let data = rows(
            3,
            |i| json!({ "c0": i, "c1": i, "c2": i, "c3": i, "c4": i, "c5": "T" }),
        );
        let d = lint_one(
            "SELECT x::XAXIS, y::LINECHART, a::BAND_LOWER, b::BAND_UPPER, c::BAND_LOWER, 'T'::TITLE FROM t;",
            data.clone(),
        );
        assert!(has(&d, "annotation/unpaired"), "{d:?}");
        let ok = lint_one(
            "SELECT x::XAXIS, y::LINECHART, a::BAND_LOWER, b::BAND_UPPER, c::BAND_LOWER, d::BAND_UPPER FROM t;",
            data,
        );
        assert!(!has(&ok, "annotation/unpaired"), "{ok:?}");
    }

    #[test]
    fn a_track_follows_a_chart_with_a_continuous_x() {
        let data = rows(
            3,
            |i| json!({ "c0": format!("2026-10-0{}", i + 1), "c1": i, "c2": "Promos" }),
        );
        let chart = "SELECT d::XAXIS, n::LINECHART, 'T'::TITLE FROM t;";
        let track = "SELECT d::XAXIS, 'Promos'::TRACK FROM t;";
        let lint = |sql: &str| check(sql, |_| Ok(data.clone()));
        // Right after the chart, after a ::HEIGHT, and several in a row: fine
        // (and a track needs no title of its own).
        let ok = lint(&format!("{chart}\n{track}\nSELECT 40::HEIGHT;\n{track}"));
        assert!(
            !has(&ok, "track/no-chart") && !has(&ok, "design/untitled-chart"),
            "{ok:?}"
        );
        // Alone, or after a table / a categorical chart: warned.
        assert!(has(&lint(track), "track/no-chart"));
        let table = "SELECT d::TABLE, n FROM t;";
        assert!(has(&lint(&format!("{table}\n{track}")), "track/no-chart"));
        let words = rows(
            3,
            |i| json!({ "c0": format!("W{i}"), "c1": i, "c2": "Promos" }),
        );
        let d = check(&format!("{chart}\n{track}"), |_| Ok(words.clone()));
        assert!(has(&d, "track/no-chart"), "{d:?}");
        // A track without events is no empty panel.
        let d = check(&format!("{chart}\n{track}"), |q: &str| {
            Ok(if q.contains("TRACK") || q.contains("'Promos'") {
                Vec::new()
            } else {
                data.clone()
            })
        });
        assert!(!has(&d, "empty-panel"), "{d:?}");
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
    fn kpi_delta_format_and_sparkline_checks() {
        let d = lint_one(
            "SELECT 5::METRIC, 4::DELTA, 'percentage'::DELTA_FORMAT",
            vec![
                serde_json::json!({"c0": 5.0, "c1": 4.0, "c2": "percentage"})
                    .as_object()
                    .unwrap()
                    .clone(),
            ],
        );
        assert!(has(&d, "kpi/bad-delta-format"), "{d:?}");
        let d = lint_one(
            "SELECT 5::METRIC, 'vs last month'::DELTA_LABEL, 7::SPARKLINE",
            vec![
                serde_json::json!({"c0": 5.0, "c1": "vs last month", "c2": 7.0})
                    .as_object()
                    .unwrap()
                    .clone(),
            ],
        );
        assert!(has(&d, "kpi/no-delta"), "{d:?}");
        assert!(has(&d, "kpi/sparkline-not-list"), "{d:?}");
        let d = lint_one(
            "SELECT 5::METRIC, 4::DELTA, 'both'::DELTA_FORMAT, list(x)::SPARKLINE",
            vec![
                serde_json::json!({"c0": 5.0, "c1": 4.0, "c2": "both", "c3": [1, 2]})
                    .as_object()
                    .unwrap()
                    .clone(),
            ],
        );
        assert!(!d.iter().any(|x| x.code.starts_with("kpi/")), "{d:?}");
    }

    #[test]
    fn variance_charts_need_an_xaxis() {
        let data = rows(3, |i| json!({ "c0": i as f64 }));
        let d = lint_one("SELECT v::WATERFALL, 'Bridge'::TITLE FROM t;", data);
        assert!(has(&d, "chart/needs-xaxis"), "{d:?}");
        let data = rows(3, |i| json!({ "c0": format!("s{i}"), "c1": i as f64 }));
        let d = lint_one("SELECT s::XAXIS, v::FUNNEL, 'Funnel'::TITLE FROM t;", data);
        assert!(!has(&d, "chart/needs-xaxis"), "{d:?}");
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
    fn tables_are_not_kpi_tiles_and_a_missing_panel_keeps_its_place() {
        // #153: a ::TABLE / ::PAGED with ::PERCENT / ::COMPACT columns is a
        // table, not a tile — it ends a run of tiles instead of extending it.
        let one = rows(1, |_| json!({ "c0": 42.0, "c1": "x" }));
        let tables = "SELECT sum(a)::METRIC, 'A'::LABEL FROM t;\n\
                      SELECT sum(b)::METRIC, 'B'::LABEL FROM t;\n\
                      SELECT x::PERCENT, y ::TABLE FROM t;\n\
                      SELECT x::COMPACT, y ::PAGED FROM t;";
        let d = check(tables, |_q| Ok(one.clone()));
        assert!(!has(&d, "design/ungrouped-kpis"), "{d:?}");
        // A panel skipped by `-- @requires` ("Not available") between two
        // tiles ends the run, the same as when its file is there.
        let script = "SELECT sum(a)::METRIC, 'A'::LABEL FROM t;\n\
                      SELECT sum(b)::METRIC, 'B'::LABEL FROM t;\n\
                      -- @requires data/opt.parquet\n\
                      SELECT a ::XAXIS, b ::BARCHART, 'T'::TITLE FROM 'data/opt.parquet';\n\
                      SELECT sum(c)::METRIC, 'C'::LABEL FROM t;";
        for present in [true, false] {
            let d = check(script, |q| {
                if q.contains("glob(") {
                    Ok(vec![json!({ "n": present as i64 })
                        .as_object()
                        .unwrap()
                        .clone()])
                } else {
                    Ok(one.clone())
                }
            });
            assert!(
                !has(&d, "design/ungrouped-kpis"),
                "present={present}: {d:?}"
            );
        }
        // … and a missing tile stays in its run either way.
        let script = "SELECT sum(a)::METRIC, 'A'::LABEL FROM t;\n\
                      SELECT sum(b)::METRIC, 'B'::LABEL FROM t;\n\
                      -- @requires data/opt.parquet\n\
                      SELECT sum(c)::METRIC, 'C'::LABEL FROM 'data/opt.parquet';";
        for present in [true, false] {
            let d = check(script, |q| {
                if q.contains("glob(") {
                    Ok(vec![json!({ "n": present as i64 })
                        .as_object()
                        .unwrap()
                        .clone()])
                } else {
                    Ok(one.clone())
                }
            });
            assert!(has(&d, "design/ungrouped-kpis"), "present={present}: {d:?}");
        }
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
    fn map_modifiers_are_checked() {
        let codes = |sql: &str, rows: Vec<Value>| -> Vec<(Severity, &'static str)> {
            let data: Vec<_> = rows
                .iter()
                .map(|r| r.as_object().unwrap().clone())
                .collect();
            check_opts(sql, |_q| Ok(data.clone()), LintOptions { design: false })
                .into_iter()
                .filter(|d| d.code.starts_with("map/"))
                .map(|d| (d.severity, d.code))
                .collect()
        };
        let poly = "POLYGON((0 0, 1 0, 1 1, 0 1, 0 0))";
        let row = |p: &str, c: &str| json!({ "c0": poly, "c1": 1.0, "c2": p, "c3": c });
        let sql = "SELECT g::MAP, v::BARCHART, 'p'::PROJECTION, 'c'::CLASSES FROM t;";
        assert!(codes(sql, vec![row("conic", "quantile 5")]).is_empty());
        assert_eq!(
            codes(sql, vec![row("robinson", "jenks 4")]),
            vec![(Severity::Warning, "map/unknown-projection")]
        );
        for bad in ["quantile 12", "kmeans 4", "0,10,5"] {
            assert_eq!(
                codes(sql, vec![row("auto", bad)]),
                vec![(Severity::Warning, "map/bad-classes")],
                "{bad}"
            );
        }
        assert_eq!(
            codes(sql, vec![row("mercator", "equal 4")]),
            vec![(Severity::Note, "map/mercator-choropleth")]
        );
        // points in Mercator are fine; too many of them are not
        let pts: Vec<Value> = (0..20_001)
            .map(|i| json!({ "c0": format!("POINT({} 1)", i % 90) }))
            .collect();
        assert_eq!(
            codes("SELECT g::MAP FROM t;", pts),
            vec![(Severity::Warning, "map/too-many-marks")]
        );
    }

    #[test]
    fn motion_directives_are_checked() {
        let codes = |sql: &str, row: Value| -> Vec<&'static str> {
            let data = vec![row.as_object().unwrap().clone()];
            check_opts(sql, |_q| Ok(data.clone()), LintOptions { design: false })
                .into_iter()
                .map(|d| d.code)
                .filter(|c| {
                    ["animate/", "linestyle/", "story/", "transition/"]
                        .iter()
                        .any(|p| c.starts_with(p))
                })
                .collect()
        };
        let row = json!({ "c0": "a", "c1": 1.0, "c2": "wobble" });
        assert_eq!(
            codes(
                "SELECT x::XAXIS, y::LINECHART, 'wobble'::ANIMATE FROM t;",
                row
            ),
            vec!["animate/unknown"]
        );
        let row = json!({ "c0": "a", "c1": 1.0, "c2": "reveal" });
        assert_eq!(
            codes(
                "SELECT x::XAXIS, y::LINECHART, 'reveal'::ANIMATE FROM t;",
                row
            ),
            vec!["animate/reveal-no-xline"]
        );
        let row = json!({ "c0": "a", "c1": 1.0, "c2": "flowing" });
        assert_eq!(
            codes(
                "SELECT x::XAXIS, y::LINECHART, 'flowing'::LINESTYLE FROM t;",
                row
            ),
            vec!["linestyle/unknown"]
        );
        let row = json!({ "c0": "a", "c1": 1.0, "c2": "hi", "c3": "series" });
        assert_eq!(
            codes(
                "SELECT x::XAXIS, y::BARCHART, 'hi'::STORY, 'series'::STORY_BY FROM t;",
                row
            ),
            vec!["story/bad-by"]
        );
        let row = json!({ "c0": "a", "c1": 1.0, "c2": "morph" });
        assert_eq!(
            codes(
                "SELECT x::XAXIS, y::BARCHART, 'morph'::TRANSITION FROM t;",
                row
            ),
            vec!["transition/unknown"]
        );
        let row = json!({ "c0": "a", "c1": 1.0, "c2": "grow" });
        assert!(codes("SELECT x::XAXIS, y::BARCHART, 'grow'::ANIMATE FROM t;", row).is_empty());
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
    fn markdown_placeholders_are_checked() {
        let data = |_q: &str| {
            Ok(rows(
                1,
                |_| json!({ "c0": "Grew {{growth:pct}} to {{revenue:eur}}; {{nope}} {{growth:fancy}}", "md:growth": 12.5, "md:revenue": 1.0 }),
            ))
        };
        let d = check(
            "SELECT 'x'::MARKDOWN, 12.5 AS growth, 1.0 AS revenue;",
            data,
        );
        let msgs: Vec<_> = d.iter().map(|x| (x.code, x.message.clone())).collect();
        let unknown: Vec<_> = msgs
            .iter()
            .filter(|(c, _)| *c == "markdown/unknown-placeholder")
            .collect();
        assert_eq!(unknown.len(), 1, "{msgs:?}");
        assert!(unknown[0].1.contains("{{nope}}"), "{msgs:?}");
        let fmt: Vec<_> = msgs
            .iter()
            .filter(|(c, _)| *c == "markdown/unknown-format")
            .collect();
        assert_eq!(fmt.len(), 1, "{msgs:?}");
        // #138: {{c0}} reads the statement's own `… AS c0` ("md:c0"), never the
        // positional c0 (the Markdown text itself); {{c1}} names nothing.
        let data = |_q: &str| {
            Ok(rows(
                1,
                |_| json!({ "c0": "{{c0:eur}} {{c1}}", "md:c0": 4.5 }),
            ))
        };
        let d = check("SELECT '{{c0:eur}} {{c1}}'::MARKDOWN, 4.5 AS c0;", data);
        let unknown: Vec<_> = d
            .iter()
            .filter(|x| x.code == "markdown/unknown-placeholder")
            .map(|x| x.message.clone())
            .collect();
        assert_eq!(unknown.len(), 1, "{unknown:?}");
        assert!(
            unknown[0].contains("{{c1}}") && unknown[0].contains("internal"),
            "{unknown:?}"
        );
        assert_eq!(
            md_placeholders("a {{ x : money }} {{ 1bad }} {{y}}"),
            vec![("x".into(), Some("money".into())), ("y".into(), None)]
        );
    }

    #[test]
    fn markdown_formulas_are_checked() {
        let data = |_q: &str| {
            Ok(rows(
                1,
                |_| json!({ "c0": "Fine $\\hat{y}_t$, broken $\\frac{a}{b$ and $$\\foo x$$; $5 and $10 is money" }),
            ))
        };
        let d = check("SELECT 'x'::MARKDOWN;", data);
        let bad: Vec<_> = d
            .iter()
            .filter(|x| x.code == "markdown/bad-math")
            .map(|x| x.message.clone())
            .collect();
        assert_eq!(bad.len(), 2, "{bad:?}");
        assert!(bad[0].contains("never closed"), "{bad:?}");
        assert!(bad[1].contains("\\foo"), "{bad:?}");
    }

    #[test]
    fn callout_kind_and_text_are_checked() {
        let one = |c1: &str| {
            let c1 = c1.to_string();
            move |_q: &str| {
                Ok(rows(
                    1,
                    |_| json!({ "c0": "**Heads up**", "c1": c1.clone() }),
                ))
            }
        };
        let d = check("SELECT 'x'::MARKDOWN, 'warning'::CALLOUT;", one("warning"));
        assert!(!d.iter().any(|x| x.code.starts_with("callout/")), "{d:?}");
        let d = check("SELECT 'x'::MARKDOWN, 'alarm'::CALLOUT;", one("alarm"));
        assert!(d.iter().any(|x| x.code == "callout/unknown"), "{d:?}");
        let d = check("SELECT 'x'::TEXT_SMALL, 'note'::CALLOUT;", one("note"));
        assert!(d.iter().any(|x| x.code == "callout/no-text"), "{d:?}");
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
