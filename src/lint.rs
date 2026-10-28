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

use crate::inputs::{default_index, input_defaults_with, input_index, input_kind};
use crate::{render_with_warnings, roles, sql, InputKind, Kind, RenderOptions, Role, TableOpt};
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
    /// `transition/unknown` | `hover-focus/unknown` | `points/unknown` (warnings) | `diagram/null-id` |
    /// `diagram/duplicate-node` | `diagram/unknown-node` | `diagram/bad-edges` |
    /// `diagram/bad-style` | `diagram/bad-direction` | `diagram/too-large`
    /// (warnings) | `box/missing` (error: a box from statistics without
    /// ::Q1 / ::MEDIAN / ::Q3, unpaired whiskers, no ::XAXIS, or beside a chart
    /// measure) | `box/unordered` | `box/null` (warnings) |
    /// `requires/unknown-variable` | `requires/empty-alternative` |
    /// `requires/bad-variable` | `requires/on-setup` |
    /// `requires/tab-always-hidden` (warnings) | `optional/missing` (a note: a
    /// `-- @requires` input is missing, so the panel shows "Not available" —
    /// a ::TAB / ::SUBTAB / ::VIEW alternative is hidden with its panels).
    /// `::VIEW` groups ([`view_diags`], #214): `view/duplicate-label` |
    /// `view/empty-label` | `view/duplicate-name` | `view/bad-name` |
    /// `view/unbalanced-group` (errors) | `view/lone` | `view/computed-label` |
    /// `view/mixed-names` | `view/empty` | `view/open-ended` |
    /// `view/stray-end` | `view/height-conflict` (an alternative's own
    /// `::HEIGHT` differs from another's, so the card still resizes when
    /// switching between them, #573) (warnings) | `view/input-inside` |
    /// `view/unnamed` (notes). `::VIEW_PLACEMENT` ([`view_diags`], #534):
    /// `view-placement/unknown-value` (an error: not `'row'`/`'header'`) |
    /// `view-placement/not-single-panel` | `view-placement/ambiguous` |
    /// `view-placement/orphan` (warnings: each falls back to `'row'`) |
    /// `view-placement/computed-value` (a warning: can't be checked here,
    /// still works at render time, like `view/computed-label`).
    /// `::FIT` ([`fit_diags`], #400, warnings): `layout/fit-ambiguous` (before
    /// the first ::TAB of a tabbed dashboard, a second ::FIT for one section,
    /// or cast beside a panel's columns) | `layout/fit-too-many` (more rows
    /// than a 1366×768 screen holds at the 120 px minimum panel height).
    /// Access policy ([`check_policy`]): `policy/invalid` | `policy/refused`
    /// (errors) | `policy/denied-column` | `policy/duckdb-version` (warnings:
    /// the CLI isn't the pinned DuckDB) | `policy/masked-column` (a note).
    /// Header comments ([`header_diags`], warnings): `header/empty-group` |
    /// `header/long-group` | `header/too-many-tags` | `header/empty-owner` |
    /// `header/bad-certified` | `header/bad-order` | `header/long-description` |
    /// `header/empty-nav` | `header/long-nav` | `header/bad-astables` (not
    /// `on`/`off`, so the author's `-- @astables` is ignored — the bar stays
    /// off unless a viewer turns it on) | `header/bad-markdown-style` (not
    /// `plain`/`card`/`callout`, so `-- @markdown_style` is ignored —
    /// `::MARKDOWN` panels without their own `::MARKDOWN_STYLE` fall back to
    /// 'callout'). A served folder
    /// ([`folder_diags`], warnings): `folder/duplicate-title` (two
    /// dashboards that would show the same label in the nav / `/` list —
    /// their `-- @nav`, or `-- @title` without one) | `folder/bad-id` |
    /// `folder/setup-table` (a setup `CREATE TABLE … AS`, rebuilt on every
    /// new server connection when served).
    /// `::MARKDOWN_TONE` (#659): `markdown_tone/unknown` (an error) |
    /// `markdown_tone/no-text` | `markdown_tone/ignored` (warnings).
    /// Design advisories
    /// (prefix `design/`): `pie-slices` | `unsorted-bars` | `untitled-chart` |
    /// `many-series` | `too-many-panels` | `ungrouped-kpis` | `raw-table`.
    /// `::IMAGE` / `::IMAGE_DARK` (#514, [`image_src_diag`]): `image/bad-src`
    /// | `image/outside-folder` (errors) | `image/not-in-assets` |
    /// `image/svg-no-size` | `image/too-large` | `image/empty` |
    /// `image/no-alt` | `image/many` | `image/missing-file` (PR B — only
    /// with [`LintOptions::image_root`]) (warnings) | `image/https-offline`
    /// (a note). Markdown images
    /// (`![alt](src)`): `markdown/image-bad-src` (error) |
    /// `markdown/image-not-in-assets` | `markdown/image-svg-no-size` |
    /// `markdown/image-too-large` | `markdown/image-no-alt` |
    /// `markdown/placeholder-in-image` (warnings).
    /// `::SHOW_IF` ([`show_if_diags`], #515): `show_if/not-alone` |
    /// `show_if/bad-target` (errors) | `show_if/no-target` |
    /// `show_if/not-boolean` | `show_if/no-row` | `show_if/unknown-variable` |
    /// `show_if/constant` (warnings).
    /// `-- @name` panel names ([`name_diags`], #518): `name/bad` |
    /// `name/reserved` | `name/duplicate` (errors: not a plain identifier,
    /// `page` — reserved for a guided tour's intro/outro, or reused on
    /// another statement) | `name/not-drawn` (warning: the statement is
    /// setup, or a layout directive with no element of its own, so the name
    /// never reaches the page).
    /// `-- @tour` guided tours ([`tour_diags`], #518 PR B): `tour/bad-id` |
    /// `tour/duplicate-id` | `tour/unknown-panel` (errors: not a plain
    /// identifier, reused by another tour, or a step target that is
    /// neither `page` nor any `-- @name`) | `tour/autostart-unknown` (an
    /// error: `-- @tour_autostart` names no tour) | `tour/empty` |
    /// `tour/long-text` | `tour/too-many-steps` | `tour/too-many` |
    /// `tour/stray-step` (warnings: no steps, a step's text over 400
    /// characters, over 20 steps, over 10 tours, a `--  x: y` line right
    /// after a non-tour header line — likely mis-indented).
    /// `::GUIDE_STEP` groups ([`step_diags`], #516): `step/inside-subtab` |
    /// `step/var-clash` | `step/var-reserved` | `step/gate-without-step`
    /// (errors) | `step/mixed-with-subtab` | `step/lone` |
    /// `step/var-not-allowed` | `step/empty` | `step/fit-inside` |
    /// `step/gate-on-last` | `step/gate-constant` |
    /// `step/gate-unknown-variable` | `step/duplicate-label` (warnings) |
    /// `step/computed-label` (a note) | `step/gate-not-boolean` |
    /// `step/gate-no-row` (warnings, checked once the step closes — see
    /// `finalize_pending_gate`).
    /// `-- @lint_allow` ([`apply_lint_allow`], #639): `lint/allowed` (a
    /// note: a `design/*` advisory the statement allows on purpose; the
    /// message names it) | `lint/allow-not-design` | `lint/allow-unused`
    /// (warnings).
    pub code: &'static str,
    pub message: String,
    /// The offending statement, trimmed to one line.
    pub sql: String,
}

/// Options for [`check_opts`].
#[derive(Debug, Clone)]
pub struct LintOptions {
    /// Also emit `design/*` advisory warnings (layout / chart-choice quality).
    /// On by default; the CLI's `--no-design` turns it off.
    pub design: bool,
    /// Is `run_query` backed by one **persistent** session that keeps state
    /// (TEMP views/tables, `SET VARIABLE`, `LOAD`, plain DDL) across calls,
    /// so an earlier statement stays in scope for a later one exactly as a
    /// real connection would?
    ///
    /// `true`: each statement is sent to `run_query` alone — setup and
    /// input/interaction default `SET VARIABLE`s are executed once, right
    /// away, since the session already has them in scope for whatever runs
    /// next. This is the CLI's persistent `duckdb` session (#435:
    /// `host::duckdb_cli::SessionRunner`, used by `dashboard --check`, the
    /// static/`--svg`/`--site` renderers), which backs one process for a
    /// whole script instead of a fresh process per statement.
    ///
    /// `false` (the default): every call to `run_query` is a **fresh**
    /// session — nothing survives between calls — so this instead replays
    /// an accumulated `prelude` (setup + input defaults + ::SELECT_AS /
    /// ::DRILL `sets`, none of which emit rows) before each statement, so
    /// session state is faithfully in scope without a long-lived
    /// connection. This is the wasm `lint_step` export and web/lint.js's
    /// `scratchRunner`, which drive DuckDB-Wasm with one fresh scratch
    /// connection per query. The query text sent must match what
    /// `origin/master` sends (the browser's `known` cache is keyed on the
    /// literal query string, and `lint_test.mjs` byte-compares it against
    /// `dashboard --check`'s own queries), so this path must not change
    /// shape even as the stateful one does.
    pub stateful_session: bool,
    /// The dashboard's own folder (#514 PR B), canonicalized, if a relative
    /// `::IMAGE`/`::IMAGE_DARK`/markdown-image source under `assets/` should
    /// be checked for `image/missing-file` (a warning: the file isn't
    /// there). `None` (the default — the browser's own lint, which has no
    /// folder to check against) skips that one diagnostic entirely; every
    /// other image diagnostic is unaffected.
    pub image_root: Option<std::path::PathBuf>,
}

impl Default for LintOptions {
    fn default() -> Self {
        LintOptions {
            design: true,
            stateful_session: false,
            image_root: None,
        }
    }
}

/// Lint `script`. See [`LintOptions::stateful_session`] for what `run_query`
/// is expected to do across calls — a persistent session (true) or a fresh
/// one each time, replaying a growing prelude (false, the default here).
///
/// An ordinary SQL error from one call does not need special handling: a
/// real session (`SessionRunner` included) isolates it to that one
/// statement and keeps going, so the next call still sees everything before
/// it, in scope, same as if the failing statement were simply absent — in
/// the `false` case, the prelude only ever grows after a statement
/// *succeeds*, so a failed one is simply never added to it either.
pub fn check<F>(script: &str, run_query: F) -> Vec<Diagnostic>
where
    F: FnMut(&str) -> Result<Vec<Map<String, Value>>, String>,
{
    check_opts(script, run_query, LintOptions::default())
}

/// The text to send for `sql` (not yet `;`-terminated): with
/// [`LintOptions::stateful_session`], `sql` alone (the persistent session
/// already has `prelude`'s statements in scope); otherwise `prelude` replayed
/// in front of it (a fresh session each call needs it replayed every time).
fn stmt_text(stateful_session: bool, prelude: &str, sql: &str) -> String {
    if stateful_session {
        format!("{sql};")
    } else {
        format!("{prelude}{sql};")
    }
}

/// A `::GUIDE_STEP`'s `::GATE`, waiting for its step to close so it can be
/// re-checked once that step's own inputs have bound their defaults — see
/// `check_opts`'s `pending_gate` and [`finalize_pending_gate`].
struct PendingGate {
    /// 1-based statement index to report against.
    stmt: usize,
    short: String,
    /// The marker's own (rewritten) SQL — re-run as-is at close time.
    sql: String,
    /// The `::GATE` column's output position.
    gate_idx: usize,
}

/// A `run_query` closure, type-erased so [`finalize_pending_gate`] can be
/// called from both the in-loop and end-of-script sites without repeating
/// the generic `F: FnMut(...)` bound `check_opts` carries.
type RunQuery<'a> = &'a mut dyn FnMut(&str) -> Result<Vec<Map<String, Value>>, String>;

/// Re-run a pending `::GATE` now that its step has closed (`prelude` —or,
/// stateful, the session— has had the chance to pick up that step's own
/// input defaults since the marker first ran) and report `step/gate-no-row`
/// / `step/gate-not-boolean` exactly as `show_if_diags`' twins do. A
/// re-query error is silently skipped: the marker's own `sql-error` already
/// covers a genuinely broken statement (reported at the earlier,
/// end-of-its-pre-pass point where `rows` is first read), so this would
/// only double it.
fn finalize_pending_gate(
    diags: &mut Vec<Diagnostic>,
    pending: PendingGate,
    stateful_session: bool,
    prelude: &str,
    run_query: RunQuery,
) {
    let text = stmt_text(stateful_session, prelude, &pending.sql);
    let Ok(rows) = run_query(&text) else {
        return;
    };
    match rows
        .first()
        .and_then(|row| row.values().nth(pending.gate_idx))
    {
        None => diags.push(Diagnostic {
            severity: Severity::Warning,
            stmt: pending.stmt,
            code: "step/gate-no-row",
            message: "this ::GATE's query returned no row — Next stays disabled with the \
                      defaults used here"
                .into(),
            sql: pending.short,
        }),
        Some(v)
            if !matches!(v, Value::Bool(_))
                && !matches!(v, Value::String(s) if s == "true" || s == "false") =>
        {
            diags.push(Diagnostic {
                severity: Severity::Warning,
                stmt: pending.stmt,
                code: "step/gate-not-boolean",
                message: format!(
                    "::GATE's value (`{}`) is not a BOOLEAN — with the defaults used here \
                     Next stays disabled (fail closed)",
                    one_line(&v.to_string())
                ),
                sql: pending.short,
            });
        }
        _ => {}
    }
}

/// Like [`check`], but with [`LintOptions`] (e.g. to disable the `design/*`
/// advisory pass, or set [`LintOptions::stateful_session`]).
pub fn check_opts<F>(script: &str, mut run_query: F, opts: LintOptions) -> Vec<Diagnostic>
where
    F: FnMut(&str) -> Result<Vec<Map<String, Value>>, String>,
{
    let mut diags = header_diags(script);
    // Replayed before each statement when `!opts.stateful_session` (a fresh
    // session each call); left empty (and never grown) when
    // `opts.stateful_session` — the persistent session already has every
    // earlier statement in scope, so there is nothing to replay.
    let mut prelude = String::new();
    // #517 S1: a dashboard reading `viewer_state` (read-back, S3) or
    // carrying `::SUBMIT` gets the server's own temps, empty — created
    // here so neither this lint nor a headless render (`dashboard.rs`'s
    // `render_dashboard_svg`) fails on a table no real server has
    // populated yet. Skipped when a real (non-temp) `viewer_state`/
    // `viewer_state_latest` already exists on this connection — seeding
    // ours on top would shadow it (review of #530 #5).
    if sql::script_needs_viewer_state(script) {
        let shadows_a_real_table = run_query(&format!("{};", sql::viewer_state_exists_probe_sql()))
            .ok()
            .and_then(|rows| rows.first().and_then(|r| r.values().next()?.as_u64()))
            .is_some_and(|n| n > 0);
        if !shadows_a_real_table {
            for stmt in sql::viewer_state_setup_statements() {
                if opts.stateful_session {
                    let _ = run_query(&format!("{stmt};"));
                } else {
                    prelude.push_str(stmt);
                    prelude.push_str(";\n");
                }
            }
        }
    }
    let mut themes = 0usize; // ::THEME directives seen (theme/multiple)
                             // Variables set by inputs / ::SELECT_AS / ::DRILL: name -> (stmt, kind),
                             // so two different directives writing one variable are reported.
    let mut owners: std::collections::HashMap<String, (usize, &'static str)> =
        std::collections::HashMap::new();
    // #517: every input-declared variable name (lower case) -> the
    // statement(s) that declare it, its kind, and that statement's (plan-
    // rewritten) SQL — read by the `::SUBMIT` branch below to validate
    // `SUBMIT_VARS` (`submit/unknown-variable`, `submit/unsupported-input`,
    // `submit/duplicate-input`, `submit/options-from-variable`). A name can
    // have more than one entry when two input statements declare it
    // (ambiguous for SUBMIT_VARS, even though `owners`/`claim` above don't
    // themselves flag two inputs sharing a name).
    let mut declared_inputs: std::collections::HashMap<String, Vec<(usize, InputKind, String)>> =
        std::collections::HashMap::new();
    // #398: `-- @shared <name>, …` directly above a ::ROWSELECT / ::SELECT_AS
    // / ::DRILL / input statement documents that *this* directive deliberately
    // reuses `<name>` — so the two name-clash lints below skip it (every other
    // clash still is flagged). A name named here this statement doesn't
    // itself claim (a typo) gets its own warning instead of silencing nothing.
    let mut claim = |diags: &mut Vec<Diagnostic>,
                     names: &[String],
                     stmt,
                     kind,
                     sql: &str,
                     shared: &[String]| {
        for n in names {
            match owners.get(n) {
                // ::DRILL / ::ROWSELECT: always flagged, even repeated by the
                // same kind — a drill level's own name is reserved, and a
                // ::ROWSELECT's variable is never chosen by the author (it is
                // the table's first column), so a repeat is always an
                // accident, not the deliberate shared name ::SELECT_AS allows.
                Some(&(other, k))
                    if other != stmt
                        && (k != kind || kind == "::DRILL" || kind == "::ROWSELECT") =>
                {
                    if shared.iter().any(|s| s == n) {
                        continue;
                    }
                    let (code, hint) = if kind == "::ROWSELECT" && k == "::ROWSELECT" {
                        (
                            "interaction/rowselect-shared-name",
                            format!(
                                "clicking a row in either table sets the same getvariable() \
                                     and silently toggles the other's selection too — alias one \
                                     table's first column to a different name, or add \
                                     `-- @shared {n}` above this ::ROWSELECT if the fan-in is \
                                     deliberate"
                            ),
                        )
                    } else {
                        (
                            "interaction/name-clash",
                            format!(
                                "each overwrites the other; rename one, or add \
                                     `-- @shared {n}` above this statement if the hand-off is \
                                     deliberate"
                            ),
                        )
                    };
                    diags.push(Diagnostic {
                            severity: Severity::Warning,
                            stmt,
                            code,
                            message: format!(
                                "variable `{n}` is set by this {kind} and by statement {other} ({k}) — {hint}"
                            ),
                            sql: sql.to_string(),
                        })
                }
                Some(_) => {}
                None => {
                    owners.insert(n.clone(), (stmt, kind));
                }
            }
        }
        for want in shared {
            if !names.iter().any(|n| n == want) {
                diags.push(Diagnostic {
                    severity: Severity::Warning,
                    stmt,
                    code: "interaction/shared-unused",
                    message: format!(
                        "`-- @shared {want}` names a variable this {kind} doesn't set — {} \
                             fix the typo, or drop the line",
                        if names.is_empty() {
                            "it sets no variable here.".to_string()
                        } else {
                            format!("this {kind} sets {}.", names.join(", "))
                        }
                    ),
                    sql: sql.to_string(),
                });
            }
        }
    };
    // A ::EMPTY message is waiting for the next panel (its 0 rows are expected).
    let mut empty_msg = false;
    // A ::TOTALS value waiting for the next table (totals/summed-rate).
    let mut next_totals: Option<String> = None;
    // A ::ROWSELECT directive waiting for the next table (#330): its click-to-
    // variable binding is named after that table's first (non-TITLE/INFO)
    // column, never written by the author, so two tables that happen to
    // share one (e.g. the same alias, or no alias on the same source column)
    // silently share one selection — a click in either toggles both. Unlike
    // ::SELECT_AS / ::DRILL (an explicit, chosen name — reuse there is
    // allowed), any repeat here is flagged — unless the directive itself
    // carries `-- @shared <name>` (#398: its statement index, and that
    // comment's names).
    let mut pending_rowselect: Option<(usize, Vec<String>)> = None;
    // #516, review-537: a `::GUIDE_STEP`'s own `::GATE` is checked once its
    // step *closes* (the next marker, or end of its group/script), not at
    // the marker itself — the canonical pattern puts the gate's quiz in
    // the same step (plan decision 2), so checking immediately would
    // always see the quiz's input still unset and warn on every author's
    // first, correct attempt. By the time the step closes, that input's
    // own `SET VARIABLE` default has joined `prelude` (or run on the
    // session) exactly as the browser's own pre-pass has it bound before
    // any query — see [`finalize_pending_gate`].
    let mut pending_gate: Option<PendingGate> = None;
    // What a ::TRACK here would sit under: `Some(true)` a chart with a numeric
    // / date x, `Some(false)` one with a discrete x, `None` no chart.
    let mut track_host: Option<bool> = None;
    // Cross-panel design counters (only used when opts.design).
    let mut rendered_panels = 0usize; // visible panels (charts/tables/kpis/text/markdown)
    let mut has_tabs = false;
    let mut sections = sql::requires::Sections::default(); // hidden tabs
                                                           // design/too-many-panels counts only a ::VIEW group's shown (first
                                                           // reached) alternative (#214) — a switcher only ever shows one.
    let mut view_pick = sql::views::StaticPick::default();
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

        // #685: a layout role (`n::COL`, `n::HEIGHT`, `n::COLUMNS`) makes its
        // whole statement a layout directive — written inside a chart's own
        // SELECT, the chart is never drawn (an error: it's always lost).
        let is_layout = |r: &Role| matches!(r, Role::Span | Role::Height | Role::Columns);
        if let Some(lr) = p.roles.iter().map(|(_, r)| *r).find(is_layout) {
            let others: Vec<Role> = p
                .roles
                .iter()
                .map(|(_, r)| *r)
                .filter(|r| !is_layout(r))
                .collect();
            // name the role that would draw the panel (else any other)
            let shown = others
                .iter()
                .find(|r| matches!(roles::spec_of(**r).category, "chart" | "kpi" | "table"))
                .or(others.first());
            if let Some(other) = shown {
                // `::COL` is how the docs write the width (::SPAN's alias)
                let layout = match lr {
                    Role::Span => "::COL".to_string(),
                    r => format!("::{}", r.token()),
                };
                diags.push(Diagnostic {
                    severity: Severity::Error,
                    stmt,
                    code: "layout/inline-layout-role",
                    message: format!(
                        "{layout} makes this whole statement a layout directive, so its ::{} is never drawn — put `SELECT n{layout};` on its own line before the panel",
                        other.token()
                    ),
                    sql: short.clone(),
                });
            }
        }

        // This statement closes the open step (the next marker, or having
        // left the group entirely) — see `pending_gate`'s doc comment.
        if p.step.is_none() || p.step.as_ref().is_some_and(|s| s.marker) {
            if let Some(pending) = pending_gate.take() {
                finalize_pending_gate(
                    &mut diags,
                    pending,
                    opts.stateful_session,
                    &prelude,
                    &mut run_query,
                );
            }
        }

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
                let what = if p.view.as_ref().is_some_and(|v| v.marker) {
                    "::VIEW"
                } else if tab == Some(true) {
                    "::SUBTAB"
                } else {
                    "::TAB"
                };
                diags.push(Diagnostic {
                    severity: Severity::Note,
                    stmt,
                    code: "optional/missing",
                    message: format!(
                        "optional input `{path}` is missing — this {what} is hidden, with its \
                         panels (`-- @requires`)",
                    ),
                    sql: short.clone(),
                });
            }
            continue;
        }
        // Count only the group's shown (first-reached) alternative toward
        // the cross-panel design counters — a switcher only ever shows one
        // of them (#214). Computed right after the hidden-section check, so
        // a requires-hidden alternative's marker never advances this (the
        // first alternative StaticPick does see is the first *shown* one).
        let counts_for_design = !view_pick.drop_panel(&p);
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
            if opts.design && !p.setup && is_content_panel(&p.roles) && counts_for_design {
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
            // #504: a ::ROWSELECT / ::TOTALS / ::EMPTY directive right above
            // this statement was waiting for *this* table — which just got
            // gated out by `-- @requires`, not skipped itself (blueprints put
            // `@requires` on the content statement, after its directives).
            // Its "Not available" card gets no row-select / totals-check /
            // empty-is-expected; without this, the pending state would carry
            // on to the next real table and get checked against *that* one
            // instead (a false `interaction/shared-unused` et al.).
            pending_rowselect = None;
            next_totals = None;
            empty_msg = false;
            continue;
        }

        // Setup statements: a query-shaped one that still carries ::ROLE casts
        // is a panel that lost its roles (the #1 silent failure) — flag it, and
        // don't run it (the casts would just raise a confusing type error).
        // Genuine setup (DDL / SET / …) is run so later panels see its effects.
        if p.setup {
            // #517 §5: `viewer_state`/`viewer_state_latest` are server-
            // provided temps (read-back, S3) — a dashboard defining its own
            // table/view of that name would be silently shadowed by the
            // server's once that lands (or would collide at start, S2/S3).
            if let Some(reserved) = creates_reserved_state_name(&p.sql) {
                diags.push(Diagnostic {
                    severity: Severity::Error,
                    stmt,
                    code: "state/shadowed",
                    message: format!(
                        "`{reserved}` is reserved for the server's own per-viewer state (#517) \
                         — this dashboard's own table/view of that name would be shadowed by \
                         it once a server with options.state serves this dashboard"
                    ),
                    sql: short.clone(),
                });
            }
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
            } else if let Err(e) = run_query(&stmt_text(opts.stateful_session, &prelude, &p.sql)) {
                // In a hidden tab, a setup statement that reads its missing
                // input fails as expected. A stateful session isolates
                // this to this one statement (#435) — later panels still
                // see everything genuine setup before it left in scope;
                // in a fresh-session run, the failed statement simply
                // never joins `prelude` below, so it's left out of every
                // later replay exactly the same way.
                let reqs = if sections.hiding() {
                    [p.requires.clone(), p.tab_requires.clone()].concat()
                } else {
                    p.requires.clone()
                };
                diags.push(sql_error(stmt, &reqs, &e, &short));
            } else if !opts.stateful_session {
                // Genuine setup succeeded: with a persistent session it's
                // already in scope for later panels (#435) and there's
                // nothing further to do; with a fresh session each call,
                // keep it in `prelude` to replay before every later
                // statement.
                prelude.push_str(&p.sql);
                prelude.push_str(";\n");
            }
            continue;
        }

        // Input panel: run its option query and prime the variable(s) to a
        // default, so downstream getvariable() panels aren't spuriously
        // empty — mirrors the browser's input pre-pass.
        if let Some(kind) = input_kind(&p.roles) {
            match run_query(&stmt_text(opts.stateful_session, &prelude, &p.sql)) {
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
                    claim(&mut diags, &names, stmt, "input", &short, &p.shared);
                    for n in &names {
                        declared_inputs
                            .entry(n.to_ascii_lowercase())
                            .or_default()
                            .push((stmt, kind, p.sql.clone()));
                    }
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
                    // #638: ::DEFAULT picks a listed input's default row(s)
                    if default_index(&p.roles).is_some()
                        && !matches!(
                            kind,
                            InputKind::Dropdown | InputKind::Buttons | InputKind::Multiselect
                        )
                    {
                        diags.push(Diagnostic {
                            severity: Severity::Warning,
                            stmt,
                            code: "input/default-ignored",
                            message: "::DEFAULT only marks the default option of a ::DROPDOWN, ::BUTTONS or ::MULTISELECT — this input's own value is its default".to_string(),
                            sql: short.clone(),
                        });
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
                    if kind == InputKind::Dropdown {
                        if let Some((idx, _)) =
                            p.roles.iter().find(|(_, r)| *r == Role::DropdownStyle)
                        {
                            if let Some(v) = rows
                                .first()
                                .and_then(|r| r.values().nth(*idx))
                                .and_then(|v| v.as_str())
                            {
                                let lv = v.to_ascii_lowercase();
                                if !crate::DROPDOWN_STYLES.contains(&lv.as_str()) {
                                    diags.push(Diagnostic {
                                        severity: Severity::Warning,
                                        stmt,
                                        code: "dropdown-style/unknown",
                                        message: format!(
                                            "::DROPDOWN_STYLE `{lv}` is not 'search' or 'native' ({}) — the option count decides",
                                            crate::DROPDOWN_STYLES.join(", ")
                                        ),
                                        sql: short.clone(),
                                    });
                                }
                            }
                        }
                    }
                    // A default failing to apply isn't checked here, same
                    // as before — these are always well-formed
                    // `SET VARIABLE` text this function built itself.
                    // Stateful (#435): send each default to the session
                    // right away, since it already carries everything
                    // before it. Fresh-session (the default — a browser
                    // `run_query`): fold it into `prelude` to replay
                    // before every later statement instead.
                    for setv in input_defaults_with(
                        kind,
                        input_index(&p.roles),
                        default_index(&p.roles),
                        &rows,
                    ) {
                        if opts.stateful_session {
                            let _ = run_query(&format!("{setv};"));
                        } else {
                            prelude.push_str(&setv);
                            prelude.push_str(";\n");
                        }
                    }
                }
                Err(e) => diags.push(sql_error(stmt, &p.requires, &e, &short)),
            }
            continue;
        }

        // `::SUBMIT` panel (#517 S1): run it as the viewer and check it's
        // self-consistent — a real server isn't wired to any of this yet
        // (no endpoint, no `options.state`), so this only checks that the
        // dashboard's own authoring agrees with itself (plan §3).
        if p.roles.iter().any(|(_, r)| matches!(r, Role::Submit)) {
            match run_query(&stmt_text(opts.stateful_session, &prelude, &p.sql)) {
                Ok(rows) if rows.len() > 1 => diags.push(Diagnostic {
                    severity: Severity::Error,
                    stmt,
                    code: "submit/rows",
                    message: format!(
                        "this ::SUBMIT returns {} rows with the current defaults — exactly \
                         one makes the button available, zero hides it, more than one is an \
                         error",
                        rows.len()
                    ),
                    sql: short.clone(),
                }),
                Ok(rows) if rows.is_empty() => diags.push(Diagnostic {
                    severity: Severity::Warning,
                    stmt,
                    code: "submit/rows",
                    message: "this ::SUBMIT returns no rows with the current defaults — \
                              hidden by default (the author's own SQL gate)"
                        .into(),
                    sql: short.clone(),
                }),
                Ok(rows) => {
                    let row = &rows[0];
                    let vars_text = p
                        .roles
                        .iter()
                        .find(|(_, r)| *r == Role::SubmitVars)
                        .and_then(|(i, _)| row.values().nth(*i))
                        .and_then(|v| v.as_str());
                    match vars_text {
                        None => diags.push(Diagnostic {
                            severity: Severity::Error,
                            stmt,
                            code: "submit/no-vars",
                            message: "this ::SUBMIT has no ::SUBMIT_VARS (or its value isn't a \
                                      string) — nothing would be recorded"
                                .into(),
                            sql: short.clone(),
                        }),
                        Some(text) => {
                            let names: Vec<String> = text
                                .split(',')
                                .map(str::trim)
                                .filter(|s| !s.is_empty())
                                .map(str::to_string)
                                .collect();
                            if names.is_empty() {
                                diags.push(Diagnostic {
                                    severity: Severity::Error,
                                    stmt,
                                    code: "submit/no-vars",
                                    message: "::SUBMIT_VARS is empty — nothing would be recorded"
                                        .into(),
                                    sql: short.clone(),
                                });
                            } else if names.len() > 32 {
                                diags.push(Diagnostic {
                                    severity: Severity::Error,
                                    stmt,
                                    code: "submit/too-many-vars",
                                    message: format!(
                                        "::SUBMIT_VARS names {} variables — at most 32 may be \
                                         recorded",
                                        names.len()
                                    ),
                                    sql: short.clone(),
                                });
                            }
                            for n in &names {
                                match declared_inputs.get(&n.to_ascii_lowercase()) {
                                    None => diags.push(Diagnostic {
                                        severity: Severity::Error,
                                        stmt,
                                        code: "submit/unknown-variable",
                                        message: format!(
                                            "::SUBMIT_VARS names `{n}`, which no input \
                                             statement of this dashboard declares"
                                        ),
                                        sql: short.clone(),
                                    }),
                                    Some(entries) => {
                                        let stmts: std::collections::HashSet<usize> =
                                            entries.iter().map(|(s, _, _)| *s).collect();
                                        if stmts.len() > 1 {
                                            diags.push(Diagnostic {
                                                severity: Severity::Error,
                                                stmt,
                                                code: "submit/duplicate-input",
                                                message: format!(
                                                    "`{n}` is declared by more than one input \
                                                     statement (statements {}) — the server \
                                                     wouldn't know which option list to \
                                                     validate it against",
                                                    stmts
                                                        .iter()
                                                        .map(usize::to_string)
                                                        .collect::<Vec<_>>()
                                                        .join(", ")
                                                ),
                                                sql: short.clone(),
                                            });
                                        } else if entries.iter().any(|(_, k, _)| {
                                            matches!(k, InputKind::DateRange | InputKind::Period)
                                        }) {
                                            diags.push(Diagnostic {
                                                severity: Severity::Error,
                                                stmt,
                                                code: "submit/unsupported-input",
                                                message: format!(
                                                    "`{n}` comes from a ::DATERANGE/::PERIOD \
                                                     input — ::SUBMIT can't record those yet \
                                                     (several variables each)"
                                                ),
                                                sql: short.clone(),
                                            });
                                        } else if let Some((_, _, input_sql)) = entries.first() {
                                            let deps = sql::deps::reads(
                                                input_sql,
                                                &std::collections::HashMap::new(),
                                            );
                                            let depends_on_a_variable = !matches!(
                                                &deps,
                                                sql::deps::Deps::Vars(v) if v.is_empty()
                                            );
                                            if depends_on_a_variable {
                                                diags.push(Diagnostic {
                                                    severity: Severity::Warning,
                                                    stmt,
                                                    code: "submit/options-from-variable",
                                                    message: format!(
                                                        "`{n}`'s own option query reads a \
                                                         viewer-settable variable — the viewer \
                                                         then controls its option list, so \
                                                         \"validated against the options\" \
                                                         means less (fine when that cascade is \
                                                         intended)"
                                                    ),
                                                    sql: short.clone(),
                                                });
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                Err(e) => diags.push(sql_error(stmt, &p.requires, &e, &short)),
            }
            // `submit/dynamic-vars`: syntactic, on the raw statement text —
            // checked whether or not the query above succeeded.
            if let Some(raw) = raws.get(i) {
                if let Some(expr) = sql::role_source_expr(raw, Role::SubmitVars) {
                    if !sql::is_string_literal(&expr) {
                        diags.push(Diagnostic {
                            severity: Severity::Warning,
                            stmt,
                            code: "submit/dynamic-vars",
                            message: "::SUBMIT_VARS is not a string literal — it can't be \
                                      checked statically; only what it evaluates to just now \
                                      was checked"
                                .into(),
                            sql: short.clone(),
                        });
                    }
                }
            }
            continue;
        }

        // #517 §8: a learner's own TEXT answer, read back through
        // `viewer_state`, can carry a phishing link or HTML/markdown a
        // trainer's page would then render — advisory only (the app already
        // escapes these; see docs/secure-serving.md).
        if p.sql.to_ascii_lowercase().contains("viewer_state")
            && p.roles
                .iter()
                .any(|(_, r)| matches!(r, Role::Markdown | Role::Link | Role::Download(_)))
        {
            diags.push(Diagnostic {
                severity: Severity::Warning,
                stmt,
                code: "state/markup-from-viewer",
                message: "this ::MARKDOWN/::LINK/::DOWNLOAD_* column reads viewer_state.value \
                          — a learner's own TEXT answer can carry a link or markup that would \
                          then render on whoever reads this panel (e.g. a trainer); the app \
                          escapes it, but double check the content is meant to be shown that way"
                    .into(),
                sql: short.clone(),
            });
        }

        // Panel: run it (stateful: everything before it is already in
        // scope, #435; fresh-session: with `prelude` replayed in front),
        // then check rows and render.
        let rows = match run_query(&stmt_text(opts.stateful_session, &prelude, &p.sql)) {
            Ok(rows) => rows,
            Err(e) => {
                diags.push(sql_error(stmt, &p.requires, &e, &short));
                // (its "Not available" / error card takes the panel's place)
                if opts.design && is_content_panel(&p.roles) && counts_for_design {
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

        // ::MARKDOWN_STYLE (#536): an unknown value falls back to 'callout'
        // (today's look); it needs a ::MARKDOWN column in the same SELECT;
        // and combined with ::CALLOUT it only makes sense as 'callout' —
        // 'plain'/'card' would silently drop the chosen look.
        if let Some((i, _)) = p.roles.iter().find(|(_, r)| *r == Role::MarkdownStyle) {
            let v = rows
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
            if !crate::roles::MARKDOWN_STYLE_KINDS.contains(&v.as_str()) {
                warn(
                    "markdown_style/unknown",
                    format!(
                        "::MARKDOWN_STYLE `{v}` — use 'plain', 'card' or 'callout' (drawn as \
                         'callout')"
                    ),
                );
            }
            if !p.roles.iter().any(|(_, r)| *r == Role::Markdown) {
                warn(
                    "markdown_style/no-text",
                    "::MARKDOWN_STYLE needs a ::MARKDOWN column in the same SELECT".into(),
                );
            }
            if p.roles.iter().any(|(_, r)| *r == Role::Callout) && (v == "plain" || v == "card") {
                warn(
                    "markdown_style/conflicts-with-callout",
                    format!(
                        "::CALLOUT together with '{v}'::MARKDOWN_STYLE — the explicit style wins, \
                         so ::CALLOUT's tint, bar and icon are dropped (drop ::CALLOUT, or set \
                         ::MARKDOWN_STYLE to 'callout')"
                    ),
                );
            }
        }

        // ::MARKDOWN_TONE (#659): an unknown tone is an error (it would draw
        // no accent at all); it needs a ::MARKDOWN column; and next to an
        // explicit 'plain'/'callout' style or a ::CALLOUT it is dropped.
        if let Some((i, _)) = p.roles.iter().find(|(_, r)| *r == Role::MarkdownTone) {
            let v = rows
                .first()
                .and_then(|row| row.values().nth(*i))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let mut push = |severity, code: &'static str, message: String| {
                diags.push(Diagnostic {
                    severity,
                    stmt,
                    code,
                    message,
                    sql: short.clone(),
                })
            };
            if crate::roles::markdown_tone_of(&v).is_none() {
                push(
                    Severity::Error,
                    "markdown_tone/unknown",
                    format!(
                        "::MARKDOWN_TONE `{v}` — use one of {} (optionally 'tone: label')",
                        crate::roles::MARKDOWN_TONE_KINDS.join(", ")
                    ),
                );
            }
            if !p.roles.iter().any(|(_, r)| *r == Role::Markdown) {
                push(
                    Severity::Warning,
                    "markdown_tone/no-text",
                    "::MARKDOWN_TONE needs a ::MARKDOWN column in the same SELECT".into(),
                );
            }
            let style = p
                .roles
                .iter()
                .find(|(_, r)| *r == Role::MarkdownStyle)
                .and_then(|(j, _)| rows.first()?.values().nth(*j)?.as_str())
                .and_then(crate::roles::markdown_style_of);
            let callout = p.roles.iter().any(|(_, r)| *r == Role::Callout);
            if matches!(style, Some("plain" | "callout")) || (style.is_none() && callout) {
                push(
                    Severity::Warning,
                    "markdown_tone/ignored",
                    format!(
                        "::MARKDOWN_TONE only applies to a 'card' box — here {} wins and the \
                         tone is dropped (drop it, or use 'card'::MARKDOWN_STYLE)",
                        style.map_or("::CALLOUT".to_string(), |s| format!(
                            "'{s}'::MARKDOWN_STYLE"
                        ))
                    ),
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

        // ::IMAGE / ::IMAGE_DARK (#514): the same source rules as
        // web/app.js's imageSrc() (parity: tests/fixtures/image_src_vectors.json).
        {
            let mut push = |severity: Severity, code: &'static str, message: String| {
                diags.push(Diagnostic {
                    severity,
                    stmt,
                    code,
                    message,
                    sql: short.clone(),
                })
            };
            let mut any_image = false;
            for role in [Role::Image, Role::ImageDark] {
                let Some((i, _)) = p.roles.iter().find(|(_, r)| *r == role) else {
                    continue;
                };
                any_image = true;
                let texts = column_texts(&rows, *i);
                if texts.is_empty() {
                    push(
                        Severity::Warning,
                        "image/empty",
                        "empty ::IMAGE value — nothing is drawn".into(),
                    );
                    continue;
                }
                for src in &texts {
                    if let Some((sev, suffix, msg)) = image_src_diag(src) {
                        let code = match suffix {
                            "bad-src" => "image/bad-src",
                            "outside-folder" => "image/outside-folder",
                            "not-in-assets" => "image/not-in-assets",
                            "svg-no-size" => "image/svg-no-size",
                            _ => "image/too-large",
                        };
                        push(sev, code, msg);
                    } else if let (crate::image::SrcKind::Relative, Some(root)) =
                        (crate::image::src_ok(src), &opts.image_root)
                    {
                        // `image_src_diag` already returned `Some` (and this
                        // branch never ran) for a Relative src outside
                        // `assets/` — reaching here means it is under
                        // `assets/`, so `None` can only mean the file isn't
                        // actually there.
                        if crate::image::resolve_relative(src, Some(root)).is_none() {
                            push(
                                Severity::Warning,
                                "image/missing-file",
                                format!(
                                    "`{}` doesn't exist under this folder's assets/",
                                    one_line(src)
                                ),
                            );
                        }
                    } else if matches!(crate::image::src_ok(src), crate::image::SrcKind::Https) {
                        // The static HTML and `--svg` keep an https: source
                        // as a link rather than embedding it (it may not
                        // show offline), and the assistant's preview CSP
                        // blocks it — worth knowing, not worth a warning.
                        push(
                            Severity::Note,
                            "image/https-offline",
                            format!(
                                "`{}` stays a link in static output (--svg, the CLI HTML) — it may not show offline",
                                one_line(src)
                            ),
                        );
                    }
                }
            }
            if any_image {
                if !p.roles.iter().any(|(_, r)| *r == Role::Alt) {
                    push(
                        Severity::Warning,
                        "image/no-alt",
                        "no ::ALT — falls back to ::TITLE, then ::LABEL, then \"\" (add ''::ALT if the image is decorative)"
                            .into(),
                    );
                }
                if rows.len() > 6 {
                    push(
                        Severity::Warning,
                        "image/many",
                        format!(
                            "{} rows — more than 6 images stack in one panel",
                            rows.len()
                        ),
                    );
                }
            }
        }

        // Markdown images (`![alt](src)`, #514): the same source rules,
        // under `markdown/image-*` codes so authors know which panel kind
        // (::MARKDOWN vs ::IMAGE) a warning is about.
        if let (Some((i, _)), Some(row)) = (
            p.roles.iter().find(|(_, r)| *r == Role::Markdown),
            rows.first(),
        ) {
            let md = row.values().nth(*i).and_then(Value::as_str).unwrap_or("");
            let mut push = |severity: Severity, code: &'static str, message: String| {
                diags.push(Diagnostic {
                    severity,
                    stmt,
                    code,
                    message,
                    sql: short.clone(),
                })
            };
            for (alt, src) in md_images(md) {
                // A `{{col}}` placeholder in the src is never filled here
                // (markdownWithValues strips tokens inside tags) — report
                // just that, not also whatever the literal, unfilled text
                // happens to look like as a path.
                if !md_placeholders(&src).is_empty() {
                    push(
                        Severity::Warning,
                        "markdown/placeholder-in-image",
                        "{{col}} inside an image src is not filled (markdownWithValues strips tokens inside tags) — compute the path in an ::IMAGE instead".into(),
                    );
                } else {
                    match image_src_diag(&src) {
                        Some((Severity::Error, _, msg)) => {
                            push(Severity::Error, "markdown/image-bad-src", msg)
                        }
                        Some((Severity::Warning, "not-in-assets", msg)) => {
                            push(Severity::Warning, "markdown/image-not-in-assets", msg)
                        }
                        Some((Severity::Warning, "svg-no-size", msg)) => {
                            push(Severity::Warning, "markdown/image-svg-no-size", msg)
                        }
                        Some((Severity::Warning, _, msg)) => {
                            push(Severity::Warning, "markdown/image-too-large", msg)
                        }
                        _ => {}
                    }
                }
                if alt.is_empty() {
                    push(
                        Severity::Warning,
                        "markdown/image-no-alt",
                        format!(
                            "![](…) has no alt text — `{}` shown with empty alt",
                            one_line(&src)
                        ),
                    );
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
                sql::vars::drill_prelude(&v)
            } else {
                sql::vars::select_as_prelude(&v)
            };
            if let Ok(sets) = &sets {
                // the variable names the directive sets (`SET VARIABLE <name> = …`)
                let names = sql::vars::names_from_sets(sets);
                let kind = if *r == Role::Drill {
                    "::DRILL"
                } else {
                    "::SELECT_AS"
                };
                claim(&mut diags, &names, stmt, kind, &short, &p.shared);
            }
            match sets {
                // Already complete, semicolon-terminated `SET VARIABLE`
                // statements. Stateful (#435): send them to the session
                // right away. Fresh-session (the default): fold them into
                // `prelude` to replay before every later statement.
                Ok(sets) => {
                    if opts.stateful_session {
                        let _ = run_query(&sets);
                    } else {
                        prelude.push_str(&sets);
                    }
                }
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
        // ::COLORSCALE… / ::THRESHOLDS colour a ::HEATMAP / ::CALENDAR's tiles
        // (or table cells): elsewhere a colour scale does nothing, both at once
        // is one too many, and rules whose cuts miss the values paint every
        // tile alike (percent vs fraction).
        diags.extend(tile_scale_diags(&p.roles, &rows, stmt, &short));
        // ::XBREAKS / ::YBREAKS: numeric entries, and only on a cartesian chart.
        diags.extend(breaks_diags(&p.roles, &rows, stmt, &short));
        // ::FACET/::FACET_FREE on a chart kind that still ignores it (#423,
        // #424); ::CATEGORY on ::HISTOGRAM (a ggplot-rs gap, #423);
        // ::HIST_BINS/::HIST_NORM anywhere but a ::HISTOGRAM.
        diags.extend(histogram_qq_diags(&p.roles, stmt, &short));
        // #424: ::IDENTITY / ::REFLINE are still dropped on a ::HEATMAP
        // (cartesian()'s only drawn there, and a heatmap never reaches it).
        diags.extend(heatmap_line_diags(&p.roles, stmt, &short));
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
                } else if !p
                    .roles
                    .iter()
                    .any(|(_, r)| matches!(r, Role::Category | Role::Value(Kind::Heatmap)))
                {
                    warn(
                        Severity::Warning,
                        "colors/no-category",
                        "a ::COLORS level map colours the ::CATEGORY levels it names (or a \
                         string ::HEATMAP measure, #332) — add the column: status::CATEGORY, \
                         'Critical:bad; OK:good'::COLORS"
                            .into(),
                    );
                } else if chart_kind(&p.roles) == Some(Kind::Waterfall) {
                    // #535: a ::WATERFALL's ::CATEGORY marks which steps are
                    // running totals (`waterfall_steps`'s `is_total_tag`), not
                    // a colour group — the step/increase/decrease colours are
                    // fixed, so ::COLORS never reaches a scale.
                    warn(
                        Severity::Warning,
                        "colors/no-effect",
                        "::COLORS has no effect on ::WATERFALL — its ::CATEGORY marks which \
                         steps are totals, not colour groups; bars keep the theme's \
                         total/increase/decrease colours"
                            .into(),
                    );
                } else if matches!(chart_kind(&p.roles), Some(Kind::Point) | Some(Kind::Bubble))
                    && p.roles.iter().any(|(_, r)| *r == Role::Hexbin)
                {
                    // #535: `cartesian` drops ::CATEGORY entirely for a
                    // ::HEXBIN scatter (one density layer for the whole
                    // panel, see `scatter_hexbin_diags`), so a ::COLORS level
                    // map has nothing left to colour.
                    warn(
                        Severity::Warning,
                        "colors/no-effect",
                        "::COLORS has no effect on a ::HEXBIN scatter — hexagons are coloured \
                         by point count alone; ::CATEGORY is dropped"
                            .into(),
                    );
                } else if p.roles.iter().any(|(_, r)| *r == Role::Hexbin) && {
                    let has = |want: Role| p.roles.iter().any(|(_, r)| *r == want);
                    has(Role::Geometry)
                        || has(Role::Region)
                        || has(Role::Origin)
                        || has(Role::Dest)
                        || (has(Role::Lat) && has(Role::Lon))
                } {
                    // #535: same as above, for a ::LAT/::LON or geometry map's
                    // ::HEXBIN (`hexbin_diags`'s "map/hexbin-category" note).
                    warn(
                        Severity::Warning,
                        "colors/no-effect",
                        "::COLORS has no effect on a ::HEXBIN map — hexagons are coloured by \
                         count or sum alone; ::CATEGORY is dropped"
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

        // ::ROWSELECT / ::TOTALS / ::COLGROUP / ::FLAG / ::HIGHLIGHT / ::SUBTOTAL /
        // ::TOOLBAR / ::SELECT_AS / ::DRILL / ::EMPTY are "next panel" layout directives — their
        // own preceding statement, the same shape as ::COL/::HEIGHT (#291).
        // Embedded as an extra column on a panel's own SELECT instead, the
        // failure is silent: inside a ::TABLE/::PAGED the cast is stripped
        // without being read as a role (a literal extra column, like
        // `thresholds/in-table` above); beside any other panel's own content
        // it *is* read, but applied to this panel instead of the next one.
        let is_next_panel_directive = |r: Role| {
            matches!(
                r,
                Role::RowSelect | Role::TableOpt(_) | Role::SelectAs | Role::Drill | Role::Empty
            )
        };
        let is_table_panel = p
            .roles
            .iter()
            .any(|(_, r)| matches!(r, Role::Table | Role::PagedTable));
        let mut misplaced: Vec<Role> = Vec::new();
        if is_table_panel {
            for t in cast_tokens(raws.get(i).map_or("", String::as_str)) {
                if let Some(role) = roles::lookup(&t).map(|s| s.role) {
                    if is_next_panel_directive(role) && !misplaced.contains(&role) {
                        misplaced.push(role);
                    }
                }
            }
        } else if p.roles.iter().any(|(_, r)| r.renders()) {
            for (_, r) in &p.roles {
                if is_next_panel_directive(*r) && !misplaced.contains(r) {
                    misplaced.push(*r);
                }
            }
        }
        for role in misplaced {
            diags.push(Diagnostic {
                severity: Severity::Warning,
                stmt,
                code: "layout/next-panel",
                message: format!(
                    "::{tok} is a layout directive for the *next* panel — its own preceding \
                     statement, like ::COL/::HEIGHT — not a column on this panel's own \
                     SELECT, where it renders wrong (a literal extra column) or is silently \
                     misapplied to this panel instead of the next one: move it above, e.g. \
                     SELECT 'value'::{tok};",
                    tok = role.token()
                ),
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

        diags.extend(scatter_hexbin_diags(&p.roles, &rows).into_iter().map(
            |(severity, code, message)| Diagnostic {
                severity,
                stmt,
                code,
                message,
                sql: short.clone(),
            },
        ));

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
            if opts.design && counts_for_design {
                rendered_panels += 1;
                flush_kpi_run(&mut diags, &mut kpi_run, kpi_run_stmt);
            }
            continue;
        }

        // Layout directives don't render a chart — nothing to check, except
        // a ::GROUP's own ::KPI_SIZE (#405: it never reaches the per-panel
        // checks below, since this branch always `continue`s).
        if roles::is_directive_panel(&p.roles) {
            if p.roles.iter().any(|(_, r)| *r == Role::KpiSize) {
                diags.extend(kpi_size_checks(&p.roles, &rows).into_iter().map(
                    |(severity, code, message)| Diagnostic {
                        severity,
                        stmt,
                        code,
                        message,
                        sql: short.clone(),
                    },
                ));
            }
            empty_msg |= p.roles.iter().any(|(_, r)| *r == Role::Empty);
            if p.roles
                .iter()
                .any(|(_, r)| *r == Role::TableOpt(TableOpt::Totals))
            {
                next_totals = rows
                    .first()
                    .and_then(|row| row.values().next())
                    .and_then(Value::as_str)
                    .map(str::to_string);
            }
            if p.roles.iter().any(|(_, r)| *r == Role::RowSelect) {
                pending_rowselect = Some((stmt, p.shared.clone()));
            }
            // #515: with the defaults this condition's cell evaluated to, is
            // its target shown? Only a JSON boolean `true` is (a `"true"`
            // string is accepted too, like `condition::holds_json`); anything
            // else — including no row at all — hides the target, which is
            // worth a note if it looks like an oversight rather than intent.
            if let Some((idx, _)) = p.roles.iter().find(|(_, r)| *r == Role::ShowIf) {
                match rows.first().and_then(|row| row.values().nth(*idx)) {
                    None => diags.push(Diagnostic {
                        severity: Severity::Warning,
                        stmt,
                        code: "show_if/no-row",
                        message: "this condition's query returned no row — its target is \
                                  always hidden"
                            .into(),
                        sql: short.clone(),
                    }),
                    Some(v)
                        if !matches!(v, Value::Bool(_))
                            && !matches!(v, Value::String(s) if s == "true" || s == "false") =>
                    {
                        diags.push(Diagnostic {
                            severity: Severity::Warning,
                            stmt,
                            code: "show_if/not-boolean",
                            message: format!(
                                "::SHOW_IF's value (`{}`) is not a BOOLEAN — with the defaults \
                                 used here its target is always hidden (fail closed)",
                                one_line(&v.to_string())
                            ),
                            sql: short.clone(),
                        });
                    }
                    _ => {}
                }
            }
            // #516, review-537: a ::GATE is checked once its step closes,
            // not here — see `pending_gate`'s doc comment and
            // `finalize_pending_gate`. (`rows`, from this marker's own
            // query above, still needs no use here: the re-query at close
            // time is the one that counts.)
            if let Some((idx, _)) = p.roles.iter().find(|(_, r)| *r == Role::Gate) {
                pending_gate = Some(PendingGate {
                    stmt,
                    short: short.clone(),
                    sql: p.sql.clone(),
                    gate_idx: *idx,
                });
            }
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
        let totals_spec = next_totals.take();
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
        if let Some(spec) = totals_spec.as_deref() {
            if p.roles
                .iter()
                .any(|(_, r)| matches!(r, Role::Table | Role::PagedTable))
            {
                diags.extend(summed_rate_diag(&p.roles, &rows, spec, stmt, &short));
                diags.extend(totals_unknown_column_diag(
                    &p.roles, &rows, spec, stmt, &short,
                ));
            }
        }
        // #330: a ::ROWSELECT's click-to-variable binding is named after
        // this table's first (non-::TITLE/::INFO) column — claim it so a
        // second ::ROWSELECT table with the same first-column name is
        // flagged (interaction/rowselect-shared-name), not silently shared.
        if let Some((rs_stmt, rs_shared)) = pending_rowselect.take() {
            if is_table_panel {
                if let Some(name) = rowselect_key_column(&p.roles, &rows) {
                    claim(
                        &mut diags,
                        &[name],
                        rs_stmt,
                        "::ROWSELECT",
                        &short,
                        &rs_shared,
                    );
                }
            }
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

        // `::KPI_SIZE` (#405): checked on every panel, not only a KPI's —
        // it can also sit misplaced on a chart / table.
        diags.extend(kpi_size_checks(&p.roles, &rows).into_iter().map(
            |(severity, code, message)| Diagnostic {
                severity,
                stmt,
                code,
                message,
                sql: short.clone(),
            },
        ));

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
            // top-level KPI tiles. Only a ::VIEW group's shown alternative
            // counts (#214) — the checks below still run for every
            // alternative; only the scan-budget total excludes the rest.
            if counts_for_design {
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
                // #441: a pie/donut's slice order IS its layout — a `GROUP BY`
                // with no `ORDER BY` leaves DuckDB free to return the groups
                // in any order (and, under parallel execution, a different
                // one from run to run — seen in practice with two near-tied
                // values). A quiet Note, not a Warning: plenty of queries
                // already order further up a CTE, which this (deliberately
                // simple) text scan can't see, so a false positive here must
                // never fail a plain `dashboard --check`.
                if matches!(k, Kind::Pie | Kind::Donut) && has_group_by_no_order_by(&p.sql) {
                    diags.push(Diagnostic {
                        severity: Severity::Note,
                        stmt,
                        code: "design/pie-no-order",
                        message: "GROUP BY with no ORDER BY — the slice order isn't guaranteed \
                                  (and can vary run to run); add ORDER BY 2 DESC"
                            .into(),
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
    // A step still open at end of script (its group ran to EOF with no
    // closer) — same close as the in-loop check.
    if let Some(pending) = pending_gate.take() {
        finalize_pending_gate(
            &mut diags,
            pending,
            opts.stateful_session,
            &prelude,
            &mut run_query,
        );
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

    let plan = sql::plan(script);
    diags.extend(view_diags(&plan));
    diags.extend(fit_diags(&plan));
    diags.extend(show_if_diags(&plan));
    diags.extend(name_diags(&plan));
    diags.extend(tour_diags(script, &plan));
    diags.extend(step_diags(&plan, script));
    apply_lint_allow(&mut diags, &plan, opts.design);
    diags
}

/// `-- @lint_allow <code>` (#639): turn each matching `design/*` advisory on
/// that statement into a `lint/allowed` note (still listed, so the exception
/// stays visible in review, but no longer failing `--strict`). The first
/// statement's allowances (the header block) also cover the script-wide
/// `stmt: 0` ones (`design/too-many-panels`). An allowance outside `design/`
/// (`lint/allow-not-design`) or one nothing on its statement triggers
/// (`lint/allow-unused`) is a warning, so it never silently does nothing.
fn apply_lint_allow(diags: &mut Vec<Diagnostic>, plan: &[sql::Panel], design: bool) {
    let mut used: Vec<(usize, String)> = Vec::new();
    for d in diags.iter_mut() {
        if !d.code.starts_with("design/") {
            continue;
        }
        let at = d.stmt.max(1);
        let Some(p) = plan.get(at - 1) else { continue };
        if p.lint_allow.iter().any(|c| c == d.code) {
            used.push((at, d.code.to_string()));
            d.message = format!("{} allowed by -- @lint_allow: {}", d.code, d.message);
            d.code = "lint/allowed";
            d.severity = Severity::Note;
        }
    }
    for (i, p) in plan.iter().enumerate() {
        for code in &p.lint_allow {
            let (code_name, message) = if !code.starts_with("design/") {
                (
                    "lint/allow-not-design",
                    format!(
                        "-- @lint_allow {code}: only design/* advisories can be allowed — \
                         errors and the other checks always apply"
                    ),
                )
            } else if design && !used.iter().any(|(s, c)| *s == i + 1 && c == code) {
                (
                    "lint/allow-unused",
                    format!(
                        "-- @lint_allow {code}: this statement doesn't trigger {code} — \
                         fix the code, or drop the line"
                    ),
                )
            } else {
                continue;
            };
            diags.push(Diagnostic {
                severity: Severity::Warning,
                stmt: i + 1,
                code: code_name,
                message,
                sql: one_line(p.sql.trim()),
            });
        }
    }
}

/// `::SHOW_IF` checks (#515) that need the whole plan at once (a condition's
/// target/range, and "every variable this dashboard sets" for
/// `unknown-variable`) — see [`Diagnostic::code`] for the full `show_if/*`
/// list. `show_if/no-row` / `show_if/not-boolean` are checked per-statement
/// in the main loop above instead, since they need the condition's own
/// query result.
fn show_if_diags(plan: &[sql::Panel]) -> Vec<Diagnostic> {
    let mut diags = Vec::new();
    let short = |i: usize| one_line(plan[i].sql.trim());
    let declared = sql::vars::declared(plan);
    for (i, p) in plan.iter().enumerate() {
        if !p.roles.iter().any(|(_, r)| *r == Role::ShowIf) {
            continue;
        }
        if p.roles.len() > 1 {
            diags.push(Diagnostic {
                severity: Severity::Error,
                stmt: i + 1,
                code: "show_if/not-alone",
                message: "::SHOW_IF must be this statement's only role — put the condition on \
                          its own statement; beside other roles it is ignored (the panel shows)"
                    .into(),
                sql: short(i),
            });
            continue; // not a real condition below (see sql::show_if::is_marker)
        }
        let reads = sql::deps::reads(&p.sql, &std::collections::HashMap::new());
        if matches!(&reads, sql::deps::Deps::Vars(v) if v.is_empty()) {
            diags.push(Diagnostic {
                severity: Severity::Warning,
                stmt: i + 1,
                code: "show_if/constant",
                message: "this condition reads no variable, so it never changes — filter with \
                          WHERE instead, or drop the ::SHOW_IF"
                    .into(),
                sql: short(i),
            });
        }
        if let sql::deps::Deps::Vars(vars) = &reads {
            for v in vars {
                if !v.starts_with("__claim_") && !declared.contains(v) {
                    diags.push(Diagnostic {
                        severity: Severity::Warning,
                        stmt: i + 1,
                        code: "show_if/unknown-variable",
                        message: format!(
                            "getvariable('{v}') names no variable this dashboard sets (an \
                             input, a SET VARIABLE, ::SELECT_AS/::DRILL, ::ROWSELECT's key \
                             column, or anofox_data) — this condition is always false"
                        ),
                        sql: short(i),
                    });
                }
            }
        }
    }
    for c in sql::show_if::scan(plan) {
        let (severity, message) = match c.problem {
            Some("show_if/bad-target") => (
                Severity::Error,
                "::SHOW_IF can't govern this statement (::TAB/::SUBTAB/::VIEW/::ENDVIEW/\
                 ::ENDGROUP/::FIT/::COLUMNS, a chrome directive or ::RELOAD) — it always \
                 shows; use `-- @requires`, or #516's ::GATE, instead"
                    .to_string(),
            ),
            Some(_) => (
                Severity::Warning,
                "nothing follows this ::SHOW_IF — it has no target".to_string(),
            ),
            None => continue,
        };
        diags.push(Diagnostic {
            severity,
            stmt: c.at + 1,
            code: c.problem.unwrap(),
            message,
            sql: short(c.at),
        });
    }
    diags
}

/// Does this statement draw an element `-- @name` can anchor (#518 decision
/// 3): a panel (including a contract plot call), an input, a `::GROUP` box,
/// a `::TAB`/`::SUBTAB` button, a `::VIEW` switcher, or a `::LABEL`/
/// `::SECTION` heading — not setup, and not an invisible directive (`::SPAN`,
/// `::RELOAD`, chrome, a lone `::THRESHOLDS`, …).
fn is_name_target(p: &sql::Panel) -> bool {
    if p.setup {
        return false;
    }
    if p.contract.is_some() {
        return true;
    }
    if roles::is_table_thresholds(&p.roles) {
        return false;
    }
    p.roles.iter().any(|(_, r)| {
        r.renders()
            || matches!(
                r,
                Role::Input(_)
                    | Role::GroupStart
                    | Role::Tab
                    | Role::SubTab
                    | Role::View
                    | Role::Section
            )
    })
}

/// `-- @name` panel names (#518 PR A): see [`Diagnostic::code`] for the full
/// `name/*` list. Run once over the whole plan so a reused name is caught
/// wherever its other statement sits.
fn name_diags(plan: &[sql::Panel]) -> Vec<Diagnostic> {
    let mut diags = Vec::new();
    let short = |i: usize| one_line(plan[i].sql.trim());
    let mut seen: Vec<(&str, usize)> = Vec::new();
    for (i, p) in plan.iter().enumerate() {
        let Some(name) = p.name.as_deref() else {
            continue;
        };
        if name == "page" {
            diags.push(Diagnostic {
                severity: Severity::Error,
                stmt: i + 1,
                code: "name/reserved",
                message: "-- @name `page` is reserved for a guided tour's intro/outro \
                          callout — pick another name"
                    .into(),
                sql: short(i),
            });
        } else if !sql::name::valid(name) {
            diags.push(Diagnostic {
                severity: Severity::Error,
                stmt: i + 1,
                code: "name/bad",
                message: format!(
                    "-- @name `{name}` is not a plain identifier — use \
                     `^[A-Za-z_][A-Za-z0-9_-]{{0,63}}$`"
                ),
                sql: short(i),
            });
        } else if let Some(&(_, other)) = seen.iter().find(|(n, _)| *n == name) {
            diags.push(Diagnostic {
                severity: Severity::Error,
                stmt: i + 1,
                code: "name/duplicate",
                message: format!(
                    "-- @name `{name}` is also statement {other}'s — names must be unique \
                     on one dashboard"
                ),
                sql: short(i),
            });
        } else {
            seen.push((name, i + 1));
        }
        if !is_name_target(p) {
            diags.push(Diagnostic {
                severity: Severity::Warning,
                stmt: i + 1,
                code: "name/not-drawn",
                message: "-- @name on a statement that draws nothing (setup, or a layout \
                          directive with no element of its own) — it never reaches the page"
                    .into(),
                sql: short(i),
            });
        }
    }
    diags
}

/// Is `l` (trimmed at its outer edges, like [`crate::host::serving::parse_dashboard_meta`]
/// passes it) some OTHER recognized header-comment line — any key
/// `parse_dashboard_meta` itself understands, besides `@tour`/
/// `@tour_autostart`? Used only by [`tour_diags`]'s `tour/stray-step` check.
fn other_header_line(l: &str) -> bool {
    use crate::host::serving::header_value;
    [
        "nav",
        "group",
        "description",
        "owner",
        "certified",
        "order",
        "astables",
        "tags",
    ]
    .iter()
    .any(|k| header_value(l, k).is_some())
        || l.strip_prefix("-- @title ").is_some()
        || l.strip_prefix("-- @refresh ").is_some()
        || l.strip_prefix("-- @load ").is_some()
        || l.strip_prefix("-- @vars ").is_some()
        || l.strip_prefix("-- @param ").is_some()
}

/// `-- @tour` guided tours (#518 PR B): see [`Diagnostic::code`] for the
/// full `tour/*` list. Needs the raw `script` (a tour's header comments
/// aren't statements, so [`sql::plan`] never sees them) and `plan` (to
/// cross-check a step's target against every `-- @name`, #518 PR A).
/// Replays [`crate::host::serving::parse_dashboard_meta`]'s own tour/step
/// adjacency so every diagnostic points at its own line — that parser
/// itself doesn't enforce any of this: a bad id, an unknown target, an
/// over-long tour, or a mis-indented step line all still parse, silently.
fn tour_diags(script: &str, plan: &[sql::Panel]) -> Vec<Diagnostic> {
    use crate::host::serving::{
        header_value, tour_header_line, tour_step_line, MAX_TOURS, MAX_TOUR_STEPS,
        MAX_TOUR_TEXT_CHARS, TOUR_PAGE_TARGET,
    };
    let mut diags = Vec::new();
    let names: std::collections::HashSet<&str> =
        plan.iter().filter_map(|p| p.name.as_deref()).collect();

    // One tour: its id, header line (for diagnostics with no better line to
    // point at) and its steps (target, text, step line).
    struct T<'a> {
        id: &'a str,
        line: &'a str,
        steps: Vec<(&'a str, &'a str, &'a str)>,
    }
    let mut tours: Vec<T> = Vec::new();
    let mut current: Option<usize> = None;
    let mut ids_seen: Vec<&str> = Vec::new();
    let mut autostart: Option<(&str, &str)> = None; // (id, its line)
                                                    // Did the PREVIOUS line recognizably belong to the header (another key,
                                                    // or this tour's own header/step lines)? Only a step-shaped line right
                                                    // after one of THOSE is flagged `tour/stray-step` — a blank line, or an
                                                    // ordinary comment, in between means it's probably not a mis-indented
                                                    // tour at all.
    let mut prev_recognized = false;
    for line in script.lines() {
        let l = line.trim();
        if let Some(i) = current {
            if let Some((target, text)) = tour_step_line(l) {
                tours[i].steps.push((target, text, l));
                prev_recognized = true;
                continue;
            }
            current = None;
        }
        if let Some((tid, _)) = tour_header_line(l) {
            if ids_seen.contains(&tid) {
                diags.push(Diagnostic {
                    severity: Severity::Error,
                    stmt: 0,
                    code: "tour/duplicate-id",
                    message: format!(
                        "-- @tour `{tid}` is already used by another tour on this dashboard"
                    ),
                    sql: one_line(l),
                });
            } else {
                ids_seen.push(tid);
            }
            if !sql::name::valid(tid) {
                diags.push(Diagnostic {
                    severity: Severity::Error,
                    stmt: 0,
                    code: "tour/bad-id",
                    message: format!(
                        "-- @tour `{tid}` is not a plain identifier — use \
                         `^[A-Za-z_][A-Za-z0-9_-]{{0,63}}$`"
                    ),
                    sql: one_line(l),
                });
            }
            tours.push(T {
                id: tid,
                line: l,
                steps: Vec::new(),
            });
            current = Some(tours.len() - 1);
            prev_recognized = true;
        } else if let Some(r) = header_value(l, "tour_autostart") {
            if !r.is_empty() {
                autostart = Some((r, l));
            }
            prev_recognized = true;
        } else if tour_step_line(l).is_some() && prev_recognized {
            diags.push(Diagnostic {
                severity: Severity::Warning,
                stmt: 0,
                code: "tour/stray-step",
                message: "a `--  <target>: <text>` line right after a header line, with no \
                          `-- @tour` above it — likely a mis-indented tour (as written, it's \
                          just a comment)"
                    .into(),
                sql: one_line(l),
            });
            prev_recognized = false;
        } else {
            prev_recognized = other_header_line(l);
        }
    }
    if tours.len() > MAX_TOURS {
        diags.push(Diagnostic {
            severity: Severity::Warning,
            stmt: 0,
            code: "tour/too-many",
            message: format!(
                "{} tours on this dashboard — keep it to {MAX_TOURS} or fewer (too many \
                 crowd the toolbar's Tour menu)",
                tours.len()
            ),
            sql: one_line(tours.last().map_or("", |t| t.line)),
        });
    }
    for t in &tours {
        if t.steps.is_empty() {
            diags.push(Diagnostic {
                severity: Severity::Warning,
                stmt: 0,
                code: "tour/empty",
                message: format!("-- @tour `{}` has no steps — it never starts", t.id),
                sql: one_line(t.line),
            });
        }
        if t.steps.len() > MAX_TOUR_STEPS {
            diags.push(Diagnostic {
                severity: Severity::Warning,
                stmt: 0,
                code: "tour/too-many-steps",
                message: format!(
                    "-- @tour `{}` has {} steps — keep it to {MAX_TOUR_STEPS} or fewer (a long \
                     tour loses the viewer)",
                    t.id,
                    t.steps.len()
                ),
                sql: one_line(t.line),
            });
        }
        for (target, text, line) in &t.steps {
            if *target != TOUR_PAGE_TARGET && !names.contains(target) {
                diags.push(Diagnostic {
                    severity: Severity::Error,
                    stmt: 0,
                    code: "tour/unknown-panel",
                    message: format!(
                        "-- @tour `{}`: `{target}` is neither `page` nor any `-- @name` on this \
                         dashboard — this step is skipped",
                        t.id
                    ),
                    sql: one_line(line),
                });
            }
            if text.chars().count() > MAX_TOUR_TEXT_CHARS {
                diags.push(Diagnostic {
                    severity: Severity::Warning,
                    stmt: 0,
                    code: "tour/long-text",
                    message: format!(
                        "-- @tour `{}`: this step's text is {} characters — keep it to \
                         {MAX_TOUR_TEXT_CHARS} or fewer (the callout is meant to be read at a \
                         glance)",
                        t.id,
                        text.chars().count()
                    ),
                    sql: one_line(line),
                });
            }
        }
    }
    if let Some((id, line)) = autostart {
        if !ids_seen.contains(&id) {
            diags.push(Diagnostic {
                severity: Severity::Error,
                stmt: 0,
                code: "tour/autostart-unknown",
                message: format!("-- @tour_autostart `{id}` names no `-- @tour` on this dashboard"),
                sql: one_line(line),
            });
        }
    }
    diags
}

/// `::FIT` (#400) — the heuristic sanity checks: the reference screen and the
/// panel / row sizes the browser's fit-to-screen mode works with.
mod fit_ref {
    /// The reference viewport height (a 1366×768 laptop).
    pub const VIEWPORT_H: f64 = 768.0;
    /// What sits above and below a fitted tab's panels there: the app bar,
    /// the toolbar, the tab bar and the page padding (measured in the
    /// browser at 1366×768: 211 px above the tab pane, 42 below).
    pub const CHROME_H: f64 = 255.0;
    /// The smallest panel the browser fits; anything smaller falls back to
    /// scrolling (web/app.js FIT_MIN_PANEL).
    pub const PANEL_MIN: f64 = 120.0;
    /// A KPI tile's minimum height under fit (web/app.js FIT_MIN_KPI).
    pub const KPI_MIN: f64 = 96.0;
    /// A `::KPI_SIZE` `'compact'` tile's minimum height under fit (#405;
    /// web/app.js `MIN_KPI_COMPACT`, src/kpi.rs `PAD_COMPACT`/`LINE_H_COMPACT`).
    pub const KPI_MIN_COMPACT: f64 = 36.0;
    /// The grid's row gap (theme-base.css `.grid` gap: 1.15rem).
    pub const ROW_GAP: f64 = 18.0;
    /// A ::SECTION / lone ::LABEL heading row.
    pub const HEADING_H: f64 = 40.0;
    /// A ::GROUP box's padding and title around its tiles.
    pub const GROUP_PAD: f64 = 40.0;
    /// A ::SUBTAB / ::VIEW ::FIT fits only its own pane, scrolled into view:
    /// the window minus the sticky app bar and the page's bottom padding
    /// (measured: 85 px) …
    pub const SCOPED_CHROME_H: f64 = 85.0;
    /// … and minus its own sub-tab / view bar with its margin and the grid
    /// gap under it.
    pub const BAR_H: f64 = 70.0;
}

/// A statement's `::VIEW` (group, alternative), if any.
type ViewAlt = Option<(usize, usize)>;

/// One statement of a `::FIT` scope as the row estimate sees it.
#[derive(Debug, Clone, Copy, PartialEq)]
enum FitItem {
    /// A panel: its grid span (of 12), whether it is a KPI tile, whether its
    /// `::KPI_SIZE` is 'compact' (#405), whether it sits inside a `::GROUP`
    /// box.
    Panel {
        span: u32,
        kpi: bool,
        compact: bool,
        grouped: bool,
    },
    Heading,
    GroupStart,
    GroupEnd,
}

/// A literal directive's number (`SELECT 6 AS c0` → 6).
fn literal_number(sql: &str) -> Option<f64> {
    let t = sql.trim().trim_end_matches(';').trim();
    if !t.get(..7)?.eq_ignore_ascii_case("SELECT ") {
        return None;
    }
    let body = t[7..].trim();
    let num = body.strip_suffix("AS c0").unwrap_or(body).trim();
    num.trim_matches('\'').trim().parse().ok()
}

/// A pending `::HEIGHT` literal set directly before plan index `start` (a
/// `::VIEW` marker) — backs up over setup and other next-panel modifiers in
/// between (`::COL`/`::SPAN` and friends), the same walk
/// [`sql::views::placement_before`]'s private twin does for a pending
/// `::VIEW_PLACEMENT`. `None` when no `::HEIGHT` sits in that chain, or its
/// value isn't a plain literal number — used by `view/height-conflict`
/// (#573) to read each alternative's own pending height, the same value
/// `web/app.js`'s `nextHeight` would carry into it.
fn height_before(plan: &[sql::Panel], start: usize) -> Option<f64> {
    let mut k = start;
    while k > 0 && (plan[k - 1].setup || sql::show_if::is_modifier(&plan[k - 1].roles)) {
        k -= 1;
    }
    (k..start)
        .rev()
        .find(|&j| !plan[j].setup && plan[j].roles.iter().any(|(_, r)| *r == Role::Height))
        .and_then(|j| literal_number(&plan[j].sql))
}

/// A rewritten (`::ROLE` → `AS c{idx}`) statement's `c{idx}` output column,
/// when the whole statement is a plain `SELECT <list>` with no `FROM` — a
/// directive's literal value, readable without a `DataProvider` (`fit_diags`
/// never queries). `None` on a data-driven statement too: the caller treats
/// that as "assume compact", the common case (an explicit, data-driven
/// `'normal'` override only mis-sizes this estimate, never the real render).
fn literal_text_col(sql: &str, idx: usize) -> Option<String> {
    let t = sql.trim().trim_end_matches(';').trim();
    let body = t
        .strip_prefix("SELECT ")
        .or_else(|| t.strip_prefix("select "))?;
    if body.to_ascii_uppercase().contains(" FROM ") {
        return None;
    }
    let suffix = format!("AS c{idx}");
    sql::split_top_commas(body).into_iter().find_map(|item| {
        let rest = item.trim().strip_suffix(&suffix)?.trim();
        rest.strip_prefix('\'')
            .and_then(|s| s.strip_suffix('\''))
            .map(|s| s.replace("''", "'"))
    })
}

/// A statement's `::KPI_SIZE` ('compact' = `true`), read from its literal
/// value when possible, else assumed 'compact' (see [`literal_text_col`]).
/// `None` when it carries no `::KPI_SIZE` at all.
fn kpi_size_compact(p: &sql::Panel) -> Option<bool> {
    let idx = p.roles.iter().find(|(_, r)| *r == Role::KpiSize)?.0;
    Some(
        literal_text_col(&p.sql, idx)
            .map(|v| v.trim().eq_ignore_ascii_case("compact"))
            .unwrap_or(true),
    )
}

/// The height (px) a `::FIT` scope's panels need at their minimum readable
/// size: the 12-column grid packed in order (a `::GROUP` box or an
/// automatic strip of ≥ 2 KPI tiles is one block whose tiles wrap as in its
/// flex box), each row as tall as its tallest minimum, plus the row gaps.
fn fit_needed_height(items: &[FitItem]) -> (f64, usize) {
    use fit_ref::*;
    let min_h = |kpi: bool, compact: bool| match (kpi, compact) {
        (true, true) => KPI_MIN_COMPACT,
        (true, false) => KPI_MIN,
        (false, _) => PANEL_MIN,
    };
    let mut rows: Vec<f64> = Vec::new();
    let (mut used, mut cur) = (0u32, 0.0f64);
    // An open box (explicit ::GROUP, or an automatic KPI strip): its own rows.
    let mut boxed: Option<(Vec<f64>, u32, f64, bool)> = None; // (rows, used, cur, auto)
    let flush = |rows: &mut Vec<f64>, used: &mut u32, cur: &mut f64| {
        if *used > 0 {
            rows.push(*cur);
        }
        *used = 0;
        *cur = 0.0;
    };
    let close_box = |rows: &mut Vec<f64>, b: (Vec<f64>, u32, f64, bool)| {
        let (mut inner, used, cur, _) = b;
        if used > 0 {
            inner.push(cur);
        }
        if !inner.is_empty() {
            let gaps = ROW_GAP * (inner.len() as f64 - 1.0);
            rows.push(inner.iter().sum::<f64>() + gaps + GROUP_PAD);
        }
    };
    for (k, it) in items.iter().enumerate() {
        // an automatic KPI strip ends at the first non-KPI statement
        if let Some(b) = &boxed {
            let auto_ends = b.3
                && !matches!(
                    it,
                    FitItem::Panel {
                        kpi: true,
                        grouped: false,
                        ..
                    }
                );
            if auto_ends {
                close_box(&mut rows, boxed.take().unwrap());
            }
        }
        match *it {
            FitItem::Heading => {
                flush(&mut rows, &mut used, &mut cur);
                rows.push(HEADING_H);
            }
            FitItem::GroupStart => {
                flush(&mut rows, &mut used, &mut cur);
                if let Some(b) = boxed.take() {
                    close_box(&mut rows, b);
                }
                boxed = Some((Vec::new(), 0, 0.0, false));
            }
            FitItem::GroupEnd => {
                if let Some(b) = boxed.take() {
                    close_box(&mut rows, b);
                }
            }
            FitItem::Panel {
                span,
                kpi,
                compact,
                grouped,
            } => {
                let run = items[k..]
                    .iter()
                    .take_while(|x| {
                        matches!(
                            x,
                            FitItem::Panel {
                                kpi: true,
                                grouped: false,
                                ..
                            }
                        )
                    })
                    .count();
                if boxed.is_none() && !grouped && kpi && run >= 2 {
                    flush(&mut rows, &mut used, &mut cur);
                    boxed = Some((Vec::new(), 0, 0.0, true));
                }
                if let Some(b) = boxed.as_mut() {
                    // a box is a flex row: tiles ~1/6, panels ~1/3 of it
                    let w = if kpi { 2 } else { 4 };
                    if b.1 + w > 12 {
                        b.0.push(b.2);
                        b.1 = 0;
                        b.2 = 0.0;
                    }
                    b.1 += w;
                    b.2 = b.2.max(min_h(kpi, compact));
                } else {
                    let span = span.clamp(1, 12);
                    if used + span > 12 {
                        flush(&mut rows, &mut used, &mut cur);
                    }
                    used += span;
                    cur = cur.max(min_h(kpi, compact));
                }
            }
        }
    }
    if let Some(b) = boxed.take() {
        close_box(&mut rows, b);
    }
    flush(&mut rows, &mut used, &mut cur);
    let gaps = ROW_GAP * (rows.len().max(1) as f64 - 1.0);
    (rows.iter().sum::<f64>() + gaps, rows.len())
}

/// `::FIT` checks (#400), over the whole plan:
///
/// - `layout/fit-ambiguous` — a `::FIT` whose scope is unclear: before the
///   first `::TAB` of a dashboard that has tabs (the whole dashboard, or the
///   first tab?), a second `::FIT` in the same section or inside a section
///   an outer `::FIT` already fits, or a `::FIT` cast beside a panel's own
///   columns instead of its own `SELECT 'fit'::FIT;`.
/// - `layout/fit-too-many` — the fitted section has more rows of panels
///   than a 1366×768 screen holds at the minimum readable panel height
///   ([`fit_ref`]): the browser falls back to scrolling there.
fn fit_diags(plan: &[sql::Panel]) -> Vec<Diagnostic> {
    use fit_ref::*;
    let mut diags = Vec::new();
    let short = |i: usize| one_line(plan[i].sql.trim());
    let has_role = |p: &sql::Panel, r: Role| p.roles.iter().any(|(_, x)| *x == r);
    if !plan.iter().any(|p| !p.setup && has_role(p, Role::Fit)) {
        return diags;
    }
    let has_tabs = plan.iter().any(|p| !p.setup && p.tab_kind().is_some());
    // Each statement's section (top tab, sub-tab) and its place in the grid.
    let mut cols = 2.0f64; // web/app.js dpCols
    let (mut pending_span, mut depth) = (0u32, 0i32);
    // The enclosing ::GROUP's ::KPI_SIZE (#405); a tile's own wins.
    let mut group_compact = false;
    let (mut tab, mut sub): (Option<usize>, Option<usize>) = (None, None);
    let mut at: Vec<(Option<usize>, Option<usize>, Option<FitItem>)> = Vec::new();
    // each statement's ::VIEW alternative and its item even when that
    // alternative is not the shown one (for a ::FIT inside it)
    let mut raw: Vec<(ViewAlt, Option<FitItem>)> = Vec::new();
    for (i, p) in plan.iter().enumerate() {
        let mut item = None;
        if !p.setup {
            match p.tab_kind() {
                Some(false) => (tab, sub, depth) = (Some(i), None, 0),
                Some(true) => (sub, depth) = (Some(i), 0),
                None => {}
            }
            // only the first (default) alternative of a ::VIEW group shows
            let hidden_alt = p.view.as_ref().is_some_and(|v| v.alt > 0);
            if has_role(p, Role::Columns) {
                if let Some(n) = literal_number(&p.sql).filter(|n| *n >= 1.0) {
                    cols = n;
                }
            } else if has_role(p, Role::Span) {
                pending_span = literal_number(&p.sql).unwrap_or(0.0).max(0.0) as u32;
            } else if has_role(p, Role::GroupStart) {
                depth += 1;
                group_compact = kpi_size_compact(p).unwrap_or(false);
                item = Some(FitItem::GroupStart);
            } else if has_role(p, Role::GroupEnd) {
                depth = (depth - 1).max(0);
                group_compact = false;
                item = Some(FitItem::GroupEnd);
            } else if has_role(p, Role::Section)
                || (p.roles.len() == 1 && matches!(p.roles[0].1, Role::Label))
            {
                item = Some(FitItem::Heading);
            } else if p.contract.is_some() || is_content_panel(&p.roles) {
                let span = if pending_span > 0 {
                    pending_span
                } else {
                    ((12.0 / cols).round() as u32).max(1)
                };
                pending_span = 0;
                item = Some(FitItem::Panel {
                    span,
                    kpi: is_kpi(&p.roles),
                    compact: kpi_size_compact(p).unwrap_or(group_compact),
                    grouped: depth > 0,
                });
            }
            raw.push((p.view.as_ref().map(|v| (v.group, v.alt)), item));
            if hidden_alt && matches!(item, Some(FitItem::Panel { .. } | FitItem::Heading)) {
                item = None;
            }
        } else {
            raw.push((None, None));
        }
        at.push((tab, sub, item));
    }
    let items_where = |f: &dyn Fn(Option<usize>, Option<usize>) -> bool| -> Vec<FitItem> {
        at.iter()
            .filter(|(t, s, _)| f(*t, *s))
            .filter_map(|(_, _, it)| *it)
            .collect()
    };

    // The ::FIT markers and their sections: (stmt, tab, sub, view alt).
    type Scope = (Option<usize>, Option<usize>, Option<(usize, usize)>);
    let mut fits: Vec<(usize, Scope)> = Vec::new();
    for (i, p) in plan.iter().enumerate() {
        if p.setup || !has_role(p, Role::Fit) {
            continue;
        }
        if p.roles.len() > 1 {
            diags.push(Diagnostic {
                severity: Severity::Warning,
                stmt: i + 1,
                code: "layout/fit-ambiguous",
                message: "::FIT is a section directive of its own — `SELECT 'fit'::FIT;` after \
                          the ::TAB it fits — not a column beside a panel's own columns, where \
                          the panel is not drawn"
                    .into(),
                sql: short(i),
            });
            continue;
        }
        let (t, s, _) = at[i];
        let view = p.view.as_ref().map(|v| (v.group, v.alt));
        if has_tabs && t.is_none() {
            diags.push(Diagnostic {
                severity: Severity::Warning,
                stmt: i + 1,
                code: "layout/fit-ambiguous",
                message: "::FIT before the first ::TAB of a tabbed dashboard: it is unclear \
                          whether the whole dashboard or only the first tab should fit the \
                          window (it fits every tab) — move it under each ::TAB that should \
                          fit"
                .into(),
                sql: short(i),
            });
        }
        let inside = |outer: &Scope, inner: &Scope| -> bool {
            // `outer` fits a section that contains `inner`'s
            (outer.0 == inner.0 || outer.0.is_none())
                && (outer.1.is_none() || outer.1 == inner.1)
                && (outer.2.is_none() || outer.2 == inner.2)
        };
        let me: Scope = (t, s, view);
        if let Some((j, _)) = fits.iter().find(|(_, o)| inside(o, &me) || inside(&me, o)) {
            diags.push(Diagnostic {
                severity: Severity::Warning,
                stmt: i + 1,
                code: "layout/fit-ambiguous",
                message: format!(
                    "a second ::FIT for a section statement {} already fits — drop one (each \
                     ::TAB / ::SUBTAB / ::VIEW needs at most one, and an outer ::FIT already \
                     covers its sub-tabs and views)",
                    j + 1
                ),
                sql: short(i),
            });
            continue;
        }
        fits.push((i, me));
    }

    // layout/fit-too-many: the panels shown at once under each ::FIT.
    let scoped_budget = VIEWPORT_H - SCOPED_CHROME_H - BAR_H;
    for &(i, (t, s, view)) in &fits {
        if has_tabs && t.is_none() {
            continue; // already reported as ambiguous
        }
        // a ::VIEW alternative's / a ::SUBTAB's ::FIT: only its own panels,
        // under its own bar (the parent tab's panels are not fitted)
        let scoped: Option<Vec<FitItem>> = match (view, s) {
            (Some(v), _) => Some(
                raw.iter()
                    .zip(&at)
                    .filter(|((vv, _), (tt, ss, _))| *vv == Some(v) && *tt == t && *ss == s)
                    .filter_map(|((_, it), _)| *it)
                    .collect(),
            ),
            (None, Some(sb)) => Some(items_where(&|tt, ss| tt == t && ss == Some(sb))),
            (None, None) => None,
        };
        if let Some(items) = scoped {
            let need = fit_needed_height(&items);
            if need.0 > scoped_budget {
                diags.push(fit_too_many(i, need, scoped_budget, short(i)));
            }
            continue;
        }
        let budget = VIEWPORT_H - CHROME_H;
        // the sub-tabs whose screens this ::FIT covers
        let subs: Vec<Option<usize>> = match (t, s) {
            (_, Some(s)) => vec![Some(s)],
            (Some(t), None) => {
                let mut v: Vec<Option<usize>> = at
                    .iter()
                    .filter(|(tt, ss, _)| *tt == Some(t) && ss.is_some())
                    .map(|(_, ss, _)| *ss)
                    .collect();
                v.dedup();
                if v.is_empty() {
                    v.push(None);
                }
                v
            }
            (None, None) => vec![None],
        };
        let mut worst = (0.0f64, 0usize);
        for sb in subs {
            let items = if t.is_none() && !has_tabs {
                items_where(&|_, _| true)
            } else {
                items_where(&|tt, ss| tt == t && (ss.is_none() || ss == sb))
            };
            let need = fit_needed_height(&items);
            if need.0 > worst.0 {
                worst = need;
            }
        }
        if worst.0 > budget {
            diags.push(fit_too_many(i, worst, budget, short(i)));
        }
    }
    diags
}

/// The `layout/fit-too-many` warning for the ::FIT at plan index `i`.
fn fit_too_many(i: usize, (need, rows): (f64, usize), budget: f64, sql: String) -> Diagnostic {
    use fit_ref::*;
    Diagnostic {
        severity: Severity::Warning,
        stmt: i + 1,
        code: "layout/fit-too-many",
        message: format!(
            "this ::FIT section needs ~{need:.0} px for its {rows} rows of panels at the \
             {min:.0} px minimum, more than the ~{budget:.0} px a {w}×{h} screen leaves — \
             it falls back to scrolling there; move panels to another ::TAB or put \
             more on a row (::COLUMNS / ::COL)",
            min = PANEL_MIN,
            w = 1366,
            h = VIEWPORT_H,
        ),
        sql,
    }
}

/// `::VIEW` group structure checks (#214): see [`Diagnostic::code`] for the
/// full `view/*` list. Run once over the whole plan (not interleaved into
/// the main statement loop above, since several of these need to see a
/// whole group, or every group, at once).
fn view_diags(plan: &[sql::Panel]) -> Vec<Diagnostic> {
    let mut diags = Vec::new();
    let short = |i: usize| one_line(plan[i].sql.trim());
    let scan = sql::views::scan(plan);

    for &i in &scan.stray_ends {
        diags.push(Diagnostic {
            severity: Severity::Warning,
            stmt: i + 1,
            code: "view/stray-end",
            message: "::ENDVIEW with no open ::VIEW group to close".into(),
            sql: short(i),
        });
    }

    // `::VIEW_PLACEMENT` (#534), over the whole plan: a statement combining
    // it with other roles (`view-placement/ambiguous`, like `::FIT`'s own
    // check), or one `scan` never attached to any group as its
    // `placement_stmt` (`view-placement/orphan`) — not directly before a
    // `::VIEW` group's first marker, so it never reaches one. Both fall
    // back to `'row'`.
    let consumed: Vec<usize> = scan
        .groups
        .iter()
        .filter_map(|g| g.placement_stmt)
        .collect();
    for (i, p) in plan.iter().enumerate() {
        if p.setup || !p.roles.iter().any(|(_, r)| *r == Role::ViewPlacement) {
            continue;
        }
        if p.roles.len() > 1 {
            diags.push(Diagnostic {
                severity: Severity::Warning,
                stmt: i + 1,
                code: "view-placement/ambiguous",
                message: "::VIEW_PLACEMENT is a directive of its own — `SELECT 'header'::VIEW_PLACEMENT;` \
                          directly before the ::VIEW group's first marker — not a column beside a \
                          panel's own columns; this one is ignored, the group stays 'row'"
                    .into(),
                sql: short(i),
            });
        } else if !consumed.contains(&i) {
            diags.push(Diagnostic {
                severity: Severity::Warning,
                stmt: i + 1,
                code: "view-placement/orphan",
                message: "this ::VIEW_PLACEMENT has no ::VIEW group to set — it needs to sit \
                          directly before the group's first ::VIEW marker (setup and other \
                          next-panel modifiers, like ::COL, may sit between them)"
                    .into(),
                sql: short(i),
            });
        }
    }

    let mut seen_ids: Vec<(String, usize)> = Vec::new();
    for g in &scan.groups {
        let marker_stmt = g.markers[0] + 1;
        if g.markers.len() == 1 {
            diags.push(Diagnostic {
                severity: Severity::Warning,
                stmt: marker_stmt,
                code: "view/lone",
                message: "a ::VIEW group with a single alternative draws a switcher with one \
                          option — add another alternative, or drop the ::VIEW"
                    .into(),
                sql: short(g.markers[0]),
            });
        }
        if let Some(stmt) = g.placement_stmt {
            match &g.placement_requested {
                None => diags.push(Diagnostic {
                    severity: Severity::Warning,
                    stmt: stmt + 1,
                    code: "view-placement/computed-value",
                    message: "this ::VIEW_PLACEMENT's value is a computed expression, not a \
                              plain string literal — whether it ends up 'header' or 'row' can't \
                              be checked here"
                        .into(),
                    sql: short(stmt),
                }),
                Some(v) if v != "row" && v != "header" => diags.push(Diagnostic {
                    severity: Severity::Error,
                    stmt: stmt + 1,
                    code: "view-placement/unknown-value",
                    message: format!(
                        "::VIEW_PLACEMENT's value must be 'row' or 'header', not '{v}' — it \
                         falls back to 'row'"
                    ),
                    sql: short(stmt),
                }),
                Some(v) if v == "header" && !g.single_panel => diags.push(Diagnostic {
                    severity: Severity::Warning,
                    stmt: stmt + 1,
                    code: "view-placement/not-single-panel",
                    message: "'header' placement needs every alternative of this ::VIEW group \
                              to be exactly one panel — this group has an alternative with more \
                              than one, or none at all, so it falls back to 'row'"
                        .into(),
                    sql: short(stmt),
                }),
                _ => {}
            }
        }
        if g.unbalanced_group {
            diags.push(Diagnostic {
                severity: Severity::Error,
                stmt: marker_stmt,
                code: "view/unbalanced-group",
                message: "a ::GROUP opened inside this ::VIEW group is never closed before it \
                          ends — add its ::ENDGROUP, or close the box after the ::VIEW group \
                          instead"
                    .into(),
                sql: short(g.markers[0]),
            });
        }
        if !g.named && scan.groups.len() > 1 {
            diags.push(Diagnostic {
                severity: Severity::Note,
                stmt: marker_stmt,
                code: "view/unnamed",
                message: "this ::VIEW group has no alias (`AS name`) — with more than one group \
                          on the dashboard, name each so `?view.<name>=` can address it"
                    .into(),
                sql: short(g.markers[0]),
            });
        }
        if let Some((_, other)) = seen_ids.iter().find(|(id, _)| *id == g.id) {
            diags.push(Diagnostic {
                severity: Severity::Error,
                stmt: marker_stmt,
                code: "view/duplicate-name",
                message: format!(
                    "this ::VIEW group's id `{}` is also statement {other}'s — group ids must \
                     be unique on one dashboard",
                    g.id
                ),
                sql: short(g.markers[0]),
            });
        } else {
            seen_ids.push((g.id.clone(), marker_stmt));
        }

        // Per-alternative: alias validity, label diagnostics, and whether it
        // has anything to show.
        let mut aliases: Vec<String> = Vec::new();
        let mut labels: Vec<(String, usize)> = Vec::new(); // (label text, marker stmt), non-empty only
        for (alt, &mi) in g.markers.iter().enumerate() {
            let p = &plan[mi];
            if let Some(alias) = sql::views::marker_alias(p) {
                if !sql::views::valid_id(&alias) {
                    diags.push(Diagnostic {
                        severity: Severity::Error,
                        stmt: mi + 1,
                        code: "view/bad-name",
                        message: format!(
                            "::VIEW alias `{alias}` is not a plain identifier — use \
                             `^[A-Za-z_][A-Za-z0-9_]*$`"
                        ),
                        sql: short(mi),
                    });
                }
                aliases.push(alias);
            }
            match p.view.as_ref().and_then(|v| v.label.clone()) {
                None => diags.push(Diagnostic {
                    severity: Severity::Warning,
                    stmt: mi + 1,
                    code: "view/computed-label",
                    message: "this alternative's label is a computed expression, not a plain \
                              string literal — its button text can't be checked here"
                        .into(),
                    sql: short(mi),
                }),
                Some(t) if t.is_empty() => diags.push(Diagnostic {
                    severity: Severity::Error,
                    stmt: mi + 1,
                    code: "view/empty-label",
                    message: "this alternative's label is empty (or NULL) — it needs text for \
                              its switcher button"
                        .into(),
                    sql: short(mi),
                }),
                Some(t) => labels.push((t, mi + 1)),
            }
            // Does this alternative have anything that actually draws?
            let end = g.markers.get(alt + 1).copied().unwrap_or(g.end);
            let has_content = (mi + 1..end).any(|j| {
                let m = &plan[j];
                !m.setup && !roles::is_directive_panel(&m.roles)
            });
            if !has_content {
                diags.push(Diagnostic {
                    severity: Severity::Warning,
                    stmt: mi + 1,
                    code: "view/empty",
                    message: "this alternative has no rendering statement — it would show a \
                              blank view"
                        .into(),
                    sql: short(mi),
                });
            }
            // An input inside a view: it would need its own getvariable()
            // binding re-primed per alternative, which no renderer does.
            for (j, m) in plan.iter().enumerate().take(end).skip(mi + 1) {
                if input_kind(&m.roles).is_some() {
                    diags.push(Diagnostic {
                        severity: Severity::Note,
                        stmt: j + 1,
                        code: "view/input-inside",
                        message: "an input inside a ::VIEW group only takes effect while that \
                                  alternative happens to be the one shown"
                            .into(),
                        sql: short(j),
                    });
                }
            }
        }
        // `view/height-conflict` (#573): a pending ::HEIGHT right before the
        // group's first marker now applies to every alternative (fixed in
        // web/app.js) — but an alternative with its OWN pending ::HEIGHT
        // still wins for just that one, so two alternatives asking for
        // different explicit heights still make the card resize when
        // switching between them. Only distinct EXPLICIT values conflict;
        // an alternative with no ::HEIGHT of its own simply inherits the
        // group's (no warning for it).
        let own_heights: Vec<Option<f64>> = g
            .markers
            .iter()
            .map(|&mi| height_before(plan, mi))
            .collect();
        if let Some(base) = own_heights.iter().flatten().next().copied() {
            for (alt, &h) in own_heights.iter().enumerate() {
                if let Some(h) = h {
                    if h != base {
                        let mi = g.markers[alt];
                        diags.push(Diagnostic {
                            severity: Severity::Warning,
                            stmt: mi + 1,
                            code: "view/height-conflict",
                            message: format!(
                                "this alternative's own ::HEIGHT ({h}) differs from another \
                                 alternative's ({base}) in the same ::VIEW group — the card \
                                 still resizes when switching to/from it; drop one or match \
                                 the values so every alternative shares one height"
                            ),
                            sql: short(mi),
                        });
                    }
                }
            }
        }
        if aliases
            .iter()
            .collect::<std::collections::HashSet<_>>()
            .len()
            > 1
        {
            diags.push(Diagnostic {
                severity: Severity::Warning,
                stmt: marker_stmt,
                code: "view/mixed-names",
                message: "this ::VIEW group's alternatives use different aliases — one name \
                          identifies the whole group; the others are ignored"
                    .into(),
                sql: short(g.markers[0]),
            });
        }
        for (k, (text, stmt)) in labels.iter().enumerate() {
            if let Some((_, other)) = labels[..k].iter().find(|(t, _)| t == text) {
                diags.push(Diagnostic {
                    severity: Severity::Error,
                    stmt: *stmt,
                    code: "view/duplicate-label",
                    message: format!(
                        "this alternative's label `{text}` is also statement {other}'s — each \
                         alternative needs a distinct label"
                    ),
                    sql: short(*stmt - 1),
                });
            }
        }
        // No ::ENDVIEW, and the last alternative holds more panels than any
        // other: likely a missing ::ENDVIEW swallowed what followed it.
        if !g.closed_by_endview && g.markers.len() > 1 {
            let counts: Vec<usize> = g
                .markers
                .iter()
                .enumerate()
                .map(|(alt, &mi)| {
                    let end = g.markers.get(alt + 1).copied().unwrap_or(g.end);
                    (mi + 1..end).filter(|&j| !plan[j].setup).count()
                })
                .collect();
            if let (Some(&last), rest) = (counts.last(), &counts[..counts.len() - 1]) {
                if rest.iter().all(|&n| n < last) {
                    diags.push(Diagnostic {
                        severity: Severity::Warning,
                        stmt: *g.markers.last().unwrap() + 1,
                        code: "view/open-ended",
                        message: "no ::ENDVIEW closes this group, and its last alternative holds \
                                  more panels than any other — check whether a missing \
                                  ::ENDVIEW swallowed statements meant to follow it"
                            .into(),
                        sql: short(*g.markers.last().unwrap()),
                    });
                }
            }
        }
    }
    diags
}

/// `::GUIDE_STEP` group structure checks (#516): see [`Diagnostic::code`]
/// for the full `step/*` list. Run once over the whole plan (mirrors
/// [`view_diags`]). `step/gate-not-boolean` / `step/gate-no-row` are
/// checked per-statement in the main loop above instead (like `::SHOW_IF`'s
/// twins), since they need the gate's own query result.
fn step_diags(plan: &[sql::Panel], script: &str) -> Vec<Diagnostic> {
    let mut diags = Vec::new();
    let short = |i: usize| one_line(plan[i].sql.trim());
    let scan = sql::steps::scan(plan);

    for &i in &scan.inside_subtab {
        diags.push(Diagnostic {
            severity: Severity::Error,
            stmt: i + 1,
            code: "step/inside-subtab",
            message: "::GUIDE_STEP can't open inside a ::SUBTAB — a step sequence is a \
                      sibling of ::SUBTAB under a ::TAB, never nested inside one; this \
                      marker is drawn as a plain ::SECTION heading instead"
                .into(),
            sql: short(i),
        });
    }

    // `::GATE` / `::GATE_MESSAGE` only mean anything beside `::GUIDE_STEP`
    // on the same statement — elsewhere they're stray columns.
    for (i, p) in plan.iter().enumerate() {
        if p.roles.iter().any(|(_, r)| *r == Role::GuideStep) {
            continue;
        }
        if p.roles
            .iter()
            .any(|(_, r)| matches!(r, Role::Gate | Role::GateMessage))
        {
            diags.push(Diagnostic {
                severity: Severity::Error,
                stmt: i + 1,
                code: "step/gate-without-step",
                message: "::GATE / ::GATE_MESSAGE only take effect on the same statement as \
                          a ::GUIDE_STEP marker — add one, or drop these columns"
                    .into(),
                sql: short(i),
            });
        }
    }

    let other_vars = sql::vars::declared_without_steps(plan);
    let allowed_vars = crate::host::serving::parse_dashboard_meta("lint", script).vars;
    let gate_declared = sql::vars::declared(plan);

    for g in &scan.groups {
        let marker_stmt = g.markers[0] + 1;
        if g.mixed_with_subtab {
            diags.push(Diagnostic {
                severity: Severity::Warning,
                stmt: marker_stmt,
                code: "step/mixed-with-subtab",
                message: "a ::SUBTAB closed this guided sequence — a step sequence is a \
                          sibling of ::SUBTAB under a ::TAB, never nested inside one; move \
                          the ::SUBTAB, or drop it, so they don't mix"
                    .into(),
                sql: short(g.markers[0]),
            });
        }
        if g.markers.len() == 1 {
            diags.push(Diagnostic {
                severity: Severity::Warning,
                stmt: marker_stmt,
                code: "step/lone",
                message: "a guided sequence with a single step has no Back/Next to show — add \
                          another ::GUIDE_STEP, or drop it"
                    .into(),
                sql: short(g.markers[0]),
            });
        }
        if other_vars.iter().any(|v| v.eq_ignore_ascii_case(&g.var)) {
            diags.push(Diagnostic {
                severity: Severity::Error,
                stmt: marker_stmt,
                code: "step/var-clash",
                message: format!(
                    "this guided sequence's variable `{}` is also set by an input, a \
                     `SET VARIABLE`, ::SELECT_AS, ::DRILL or ::ROWSELECT elsewhere — give the \
                     group its own alias (`'…' AS name ::GUIDE_STEP`)",
                    g.var
                ),
                sql: short(g.markers[0]),
            });
        }
        if let Some(reserved) = &g.reserved_alias {
            diags.push(Diagnostic {
                severity: Severity::Error,
                stmt: marker_stmt,
                code: "step/var-reserved",
                message: format!(
                    "this guided sequence's alias `{reserved}` is a reserved name \
                     (`__claim_*`/`anofox_data`) — the server sets it, so a step can't claim \
                     it (the group fell back to `{}`); pick a different alias",
                    g.var
                ),
                sql: short(g.markers[0]),
            });
        }
        if allowed_vars
            .as_ref()
            .is_some_and(|vs| !vs.iter().any(|v| v.eq_ignore_ascii_case(&g.var)))
        {
            diags.push(Diagnostic {
                severity: Severity::Warning,
                stmt: marker_stmt,
                code: "step/var-not-allowed",
                message: format!(
                    "this dashboard declares `-- @vars`, but it doesn't list `{}` — on a \
                     locked server a viewer can't navigate this guided sequence by URL \
                     (`?v.{}=`) until it's added",
                    g.var, g.var
                ),
                sql: short(g.markers[0]),
            });
        }

        // Per-step: label dedup/computed, a ::FIT inside, an empty step, a
        // gate on the last step.
        let mut labels: Vec<(String, usize)> = Vec::new();
        for (idx, &mi) in g.markers.iter().enumerate() {
            let p = &plan[mi];
            match &p.step.as_ref().and_then(|s| s.label.clone()) {
                None => diags.push(Diagnostic {
                    severity: Severity::Note,
                    stmt: mi + 1,
                    code: "step/computed-label",
                    message: "this step's label is a computed expression, not a plain string \
                              literal — its heading text can't be checked here"
                        .into(),
                    sql: short(mi),
                }),
                Some(t) if !t.is_empty() => labels.push((t.clone(), mi + 1)),
                Some(_) => {}
            }
            let end = g.markers.get(idx + 1).copied().unwrap_or(g.end);
            if (mi + 1..end).all(|j| plan[j].setup || roles::is_directive_panel(&plan[j].roles)) {
                diags.push(Diagnostic {
                    severity: Severity::Warning,
                    stmt: mi + 1,
                    code: "step/empty",
                    message: "this step has no rendering statement — it would show a blank pane"
                        .into(),
                    sql: short(mi),
                });
            }
            if (mi + 1..end).any(|j| plan[j].roles.iter().any(|(_, r)| *r == Role::Fit)) {
                diags.push(Diagnostic {
                    severity: Severity::Warning,
                    stmt: mi + 1,
                    code: "step/fit-inside",
                    message: "::FIT inside a ::GUIDE_STEP is ignored — a step's content is \
                              never the whole screen's fit target"
                        .into(),
                    sql: short(mi),
                });
            }
            let has_gate = p.roles.iter().any(|(_, r)| *r == Role::Gate);
            if has_gate && idx + 1 == g.markers.len() {
                diags.push(Diagnostic {
                    severity: Severity::Warning,
                    stmt: mi + 1,
                    code: "step/gate-on-last",
                    message: "::GATE on the last step does nothing — there is no further step \
                              for it to hold back"
                        .into(),
                    sql: short(mi),
                });
            }
            if has_gate {
                let reads = sql::deps::reads(&p.sql, &std::collections::HashMap::new());
                if matches!(&reads, sql::deps::Deps::Vars(v) if v.is_empty()) {
                    diags.push(Diagnostic {
                        severity: Severity::Warning,
                        stmt: mi + 1,
                        code: "step/gate-constant",
                        message: "this ::GATE reads no variable, so it never changes — filter \
                                  with WHERE instead, or drop the ::GATE"
                            .into(),
                        sql: short(mi),
                    });
                }
                if let sql::deps::Deps::Vars(vars) = &reads {
                    for v in vars {
                        if !v.starts_with("__claim_") && !gate_declared.contains(v) {
                            diags.push(Diagnostic {
                                severity: Severity::Warning,
                                stmt: mi + 1,
                                code: "step/gate-unknown-variable",
                                message: format!(
                                    "getvariable('{v}') names no variable this dashboard sets \
                                     — this ::GATE is always false (Next stays disabled)"
                                ),
                                sql: short(mi),
                            });
                        }
                    }
                }
            }
        }
        for (k, (text, stmt)) in labels.iter().enumerate() {
            if let Some((_, other)) = labels[..k].iter().find(|(t, _)| t == text) {
                diags.push(Diagnostic {
                    severity: Severity::Warning,
                    stmt: *stmt,
                    code: "step/duplicate-label",
                    message: format!(
                        "this step's label `{text}` is also statement {other}'s — a repeated \
                         label makes the step list harder to scan"
                    ),
                    sql: short(*stmt - 1),
                });
            }
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
    // #517 S3: this check has no `options.state` to say whether the real
    // server would enable read-back here (that's `--options`'s own
    // submit/not-enabled and state/not-enabled diagnostics) — optimistic,
    // same as the empty-temp seeding `check_opts` already does: if the
    // script mentions `viewer_state` at all, accept it as the server's own
    // temp rather than an unknown table.
    let server_temps: &[&str] = if sql::script_needs_viewer_state(script) {
        &["viewer_state", "viewer_state_latest"]
    } else {
        &[]
    };
    let mut diags = Vec::new();
    for (i, r) in enf
        .rewrite_each(duck, &stmts, server_temps)
        .into_iter()
        .enumerate()
    {
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

/// The header comments that place and describe a dashboard (`-- @nav`,
/// `-- @group`, `-- @tags`, `-- @description`, `-- @owner`, `-- @certified`,
/// `-- @order`): an empty `@nav`, an over-long `@nav`, an empty or
/// over-long group, more tags than are kept, a description over
/// [`MAX_DESCRIPTION_CHARS`], an empty owner, a certification date that
/// isn't `YYYY-MM-DD`, an `@order` that isn't a plain integer. Reported as
/// statement 0 (the header comes before the first statement).
pub fn header_diags(script: &str) -> Vec<Diagnostic> {
    use crate::host::serving::{
        header_value, split_tags, valid_iso_date, MAX_GROUP_CHARS, MAX_NAV_CHARS, MAX_TAGS,
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
        if header_value(line, "nav") == Some("") {
            warn(
                "header/empty-nav",
                "`-- @nav` has no label — the nav bar and start-page card fall back to \
                 `-- @title`, or drop the line"
                    .into(),
                line,
            );
        } else if let Some(n) =
            header_value(line, "nav").filter(|n| n.chars().count() > MAX_NAV_CHARS)
        {
            warn(
                "header/long-nav",
                format!(
                    "`-- @nav` is {} characters long — keep it to {MAX_NAV_CHARS} or fewer \
                     (it is a nav bar / card label, `-- @title` can stay the long form)",
                    n.chars().count()
                ),
                line,
            );
        } else if let Some(g) = header_value(line, "group") {
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
        } else if let Some(o) = header_value(line, "order").filter(|o| o.parse::<i64>().is_err()) {
            warn(
                "header/bad-order",
                if o.is_empty() {
                    "`-- @order` has no value — the dashboard sorts after every explicitly \
                     ordered one, alphabetically by id"
                        .into()
                } else {
                    format!(
                        "`-- @order {o}` is not a plain integer — the dashboard sorts after every \
                         explicitly ordered one, alphabetically by id"
                    )
                },
                line,
            );
        } else if let Some(t) = header_value(line, "tags") {
            for t in split_tags(t) {
                if !tags.contains(&t) {
                    tags.push(t);
                }
            }
            tags_line = line;
        } else if let Some(a) = header_value(line, "astables").filter(|a| *a != "on" && *a != "off")
        {
            warn(
                "header/bad-astables",
                if a.is_empty() {
                    "`-- @astables` has no value — write `on` or `off` (without it, the \
                     bar is off unless a viewer switches it on themselves)"
                        .into()
                } else {
                    format!(
                        "`-- @astables {a}` is not `on` or `off` — the \"▦ Charts as tables\" \
                         bar stays off on this dashboard unless a viewer switches it on \
                         themselves"
                    )
                },
                line,
            );
        } else if let Some(v) = header_value(line, "markdown_style")
            .filter(|v| crate::roles::markdown_style_of(v).is_none())
        {
            warn(
                "header/bad-markdown-style",
                if v.is_empty() {
                    "`-- @markdown_style` has no value — write `plain`, `card` or `callout` \
                     (without it, every ::MARKDOWN panel without its own ::MARKDOWN_STYLE \
                     falls back to 'callout')"
                        .into()
                } else {
                    format!(
                        "`-- @markdown_style {v}` is not `plain`, `card` or `callout` — every \
                         ::MARKDOWN panel without its own ::MARKDOWN_STYLE falls back to \
                         'callout'"
                    )
                },
                line,
            );
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
/// gets the diagnostics only the folder shows — two dashboards that would
/// show the same label in the nav / `/` list (`DashboardMeta::nav_label`:
/// `-- @nav` when set, else `-- @title`, compared case-insensitively — so
/// two dashboards with the same `@title` but distinct `@nav` labels no
/// longer collide, while one's `@nav` matching another's `@title` does),
/// and a file name the servers skip (its id must be `[A-Za-z0-9_.-]`, at
/// most 128, not starting with `.`). Returned with the file each belongs
/// to.
pub fn folder_diags(files: &[(String, String)]) -> Vec<(String, Diagnostic)> {
    use crate::host::serving::{parse_dashboard_meta, valid_dashboard_id};
    let mut out = Vec::new();
    let id_of = |f: &str| f.strip_suffix(".sql").unwrap_or(f).to_string();
    let mut labels: Vec<(String, String)> = Vec::new(); // (nav label, file)
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
        let meta = parse_dashboard_meta(&id, script);
        let label = meta.nav_label().to_string();
        if let Some((_, first)) = labels.iter().find(|(l, _)| l.eq_ignore_ascii_case(&label)) {
            out.push((
                file.clone(),
                Diagnostic {
                    severity: Severity::Warning,
                    stmt: 0,
                    code: "folder/duplicate-title",
                    message: format!(
                        "`{label}` is also how `{first}` shows in the nav and the `/` list — \
                         viewers can't tell the two apart there; give one of them its own \
                         `-- @nav` or `-- @title`"
                    ),
                    sql: String::new(),
                },
            ));
        } else {
            labels.push((label, file.clone()));
        }
        // The locked server runs a panel's setup on every connection that
        // serves it (#387; warm connections, #388, cut that to a new
        // connection or a changed variable the setup reads), so a setup
        // statement that materialises a table is rebuilt there and held in
        // memory per kept connection; a view costs only what each panel reads.
        for (i, p) in sql::plan(script).iter().enumerate() {
            if p.setup && materialises_table(&p.sql) {
                out.push((
                    file.clone(),
                    Diagnostic {
                        severity: Severity::Warning,
                        stmt: i,
                        code: "folder/setup-table",
                        message: "served, this setup `CREATE TABLE … AS` is rebuilt on every \
                                  new server connection (each kept connection, and whenever a \
                                  variable it reads changes) and held in memory by each; use \
                                  `CREATE VIEW` instead, or build the table into the database \
                                  the server opens"
                            .to_string(),
                        sql: p.sql.clone(),
                    },
                ));
            }
        }
    }
    out
}

/// `submit/not-enabled` / `state/not-enabled` (#517 S2, plan §3: "With
/// `--options locked.json`"), deferred from S1 since they need a real
/// server's `options.state` to check a dashboard's id against —
/// `dashboard --check --options locked.json` reads that file itself
/// (`cmd_check`) and calls this per dashboard. `enabled_for_submit` is
/// `options.state.dashboards`; `enabled_for_state` the union of
/// `dashboards`/`read`/`trainers.dashboards`
/// ([`crate::host::state::StateOptions::named_dashboards`]) — both empty
/// when `--options` named no `options.state` at all, which still flags
/// every `::SUBMIT`/`viewer_state` dashboard as not enabled (there is no
/// `state` for it to be enabled under).
pub fn state_enablement_diags(
    script: &str,
    dashboard_id: &str,
    enabled_for_submit: &[String],
    enabled_for_state: &[String],
) -> Vec<Diagnostic> {
    let mut diags = Vec::new();
    let has_submit = sql::plan(script)
        .iter()
        .any(|p| p.roles.iter().any(|(_, r)| matches!(r, Role::Submit)));
    if has_submit && !enabled_for_submit.iter().any(|d| d == dashboard_id) {
        diags.push(Diagnostic {
            severity: Severity::Error,
            stmt: 0,
            code: "submit/not-enabled",
            message: format!(
                "this dashboard carries ::SUBMIT, but '{dashboard_id}' is not in \
                 options.state.dashboards — its button would have nowhere to post to"
            ),
            sql: String::new(),
        });
    }
    if sql::script_needs_viewer_state(script)
        && !enabled_for_state.iter().any(|d| d == dashboard_id)
    {
        diags.push(Diagnostic {
            severity: Severity::Error,
            stmt: 0,
            code: "state/not-enabled",
            message: format!(
                "this dashboard reads viewer_state, but '{dashboard_id}' is in none of \
                 options.state's dashboards/read/trainers.dashboards — it would always read \
                 its own (empty) seeded temp, never a real viewer's rows"
            ),
            sql: String::new(),
        });
    }
    diags
}

/// `CREATE [OR REPLACE] [TEMP | TEMPORARY] TABLE [IF NOT EXISTS] … AS …`:
/// a statement that materialises a query's result (leading comments allowed).
fn materialises_table(sql: &str) -> bool {
    let body: String = sql
        .lines()
        .filter(|l| !l.trim_start().starts_with("--"))
        .collect::<Vec<_>>()
        .join(" ");
    let toks: Vec<String> = body
        .split(|c: char| c.is_whitespace() || c == '(' || c == ';')
        .filter(|t| !t.is_empty())
        .map(str::to_ascii_uppercase)
        .collect();
    let mut t = toks.iter().map(String::as_str).peekable();
    if t.next() != Some("CREATE") {
        return false;
    }
    if t.peek() == Some(&"OR") {
        t.next();
        if t.next() != Some("REPLACE") {
            return false;
        }
    }
    if matches!(t.peek(), Some(&"TEMP") | Some(&"TEMPORARY")) {
        t.next();
    }
    t.next() == Some("TABLE") && t.any(|w| w == "AS")
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
            // Each query runs in a fresh DuckDB-Wasm scratch session
            // (web/lint.js's `scratchRunner`) — not a persistent one.
            stateful_session: false,
            // The browser lint has no folder to check `assets/` against
            // (#514 PR B) — `image/missing-file` never fires here.
            image_root: None,
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

/// Does a setup statement `CREATE` a table/view named `viewer_state` or
/// `viewer_state_latest` (`state/shadowed`, #517 §5)? A simple forward scan
/// for `TABLE`/`VIEW` (skipping `TEMP`/`OR REPLACE`/`IF NOT EXISTS` before
/// it, and those words too) then the following (possibly dotted,
/// `main.viewer_state`/`"x".viewer_state`) identifier, of which only the
/// last segment is the table/view's own name — a schema or catalog
/// qualifier never changes what the bare name resolves to for a later
/// unqualified `FROM viewer_state` (review of #530, nits).
fn creates_reserved_state_name(sql: &str) -> Option<&'static str> {
    use sql::lex::{tokenize, TokKind};
    let toks: Vec<_> = tokenize(sql)
        .into_iter()
        .filter(|t| !t.is_trivia())
        .collect();
    if !toks.first().is_some_and(|t| t.is_word(sql, "CREATE")) {
        return None;
    }
    let table_or_view = toks
        .iter()
        .position(|t| t.is_word(sql, "TABLE") || t.is_word(sql, "VIEW"))?;
    let mut j = table_or_view + 1;
    while toks
        .get(j)
        .is_some_and(|t| t.is_word(sql, "IF") || t.is_word(sql, "NOT") || t.is_word(sql, "EXISTS"))
    {
        j += 1;
    }
    // The dotted name's last segment: `a.b.c` tokenizes as `a . b . c`, so
    // keep consuming `. <ident>` pairs and remember the last identifier.
    let mut name_tok = toks.get(j)?;
    if !matches!(name_tok.kind, TokKind::Word | TokKind::QuotedIdent) {
        return None;
    }
    j += 1;
    while toks.get(j).is_some_and(|t| t.is_punct(sql, '.')) {
        let Some(next) = toks.get(j + 1) else { break };
        if !matches!(next.kind, TokKind::Word | TokKind::QuotedIdent) {
            break;
        }
        name_tok = next;
        j += 2;
    }
    let name = name_tok.text(sql).trim_matches('"');
    ["viewer_state", "viewer_state_latest"]
        .into_iter()
        .find(|reserved| name.eq_ignore_ascii_case(reserved))
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
/// `pub(crate)`: the headless/served renderer (`crate::dashboard`) fills the
/// same placeholders from the same row, so it shares this lookup.
pub(crate) fn md_column<'a>(
    keys: impl Iterator<Item = &'a String> + Clone,
    name: &str,
) -> Option<&'a String> {
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

/// One `::IMAGE`/`::IMAGE_DARK`/markdown-image `src`'s diagnostic, if any:
/// `(severity, machine suffix, message)`. Callers prefix the suffix with
/// `image/` or `markdown/image-` (decision: markdown images get their own
/// code family so an author knows which panel kind a warning is about).
/// `None` when `src` is fine as given (still possibly worth a lint-level
/// `not-in-assets` nudge, which this *does* report — only a clean relative
/// `assets/…` / `https:` / `data:` source with no size issue is silent).
fn image_src_diag(src: &str) -> Option<(Severity, &'static str, String)> {
    use crate::image::{decode_data_uri, intrinsic_size, is_under_assets, src_ok, SrcKind};
    match src_ok(src) {
        SrcKind::Bad("too-long") => Some((
            Severity::Error,
            "too-large",
            format!(
                "image source is {} characters — the cap is {}",
                src.chars().count(),
                crate::image::MAX_SRC_CHARS
            ),
        )),
        SrcKind::Bad(
            reason @ ("protocol-relative" | "absolute-path" | "outside-folder" | "percent-encoded"),
        ) => {
            Some((
                Severity::Error,
                "outside-folder",
                format!(
                    "`{}` leaves the dashboard's own folder ({reason})",
                    one_line(src)
                ),
            ))
        }
        SrcKind::Bad(reason) => Some((
            Severity::Error,
            "bad-src",
            format!(
                "`{}` is not an allowed image source ({reason}) — use a relative path, data:image/… or https:",
                one_line(src)
            ),
        )),
        SrcKind::Relative if !is_under_assets(src) => Some((
            Severity::Warning,
            "not-in-assets",
            format!("`{src}` is outside assets/ — relative images are only served from there"),
        )),
        SrcKind::Data("svg+xml") => decode_data_uri(src)
            .filter(|(m, bytes)| intrinsic_size(bytes, m).is_none())
            .map(|_| {
                (
                    Severity::Warning,
                    "svg-no-size",
                    "this SVG has no viewBox and no width+height on its root element — it draws at 300×150".to_string(),
                )
            })
            .or_else(|| too_large(src)),
        _ => too_large(src),
    }
}

/// The >512 000 character soft-warning shared by every accepted `src` kind
/// ([`image_src_diag`]) — still drawn, just worth a smaller file.
fn too_large(src: &str) -> Option<(Severity, &'static str, String)> {
    (src.chars().count() > 512_000).then(|| {
        (
            Severity::Warning,
            "too-large",
            format!(
                "image source is {} characters — consider a smaller file or a relative assets/ path",
                src.chars().count()
            ),
        )
    })
}

/// `![alt](src)` occurrences in Markdown text, in order — the lint-time
/// counterpart of `web/app.js::renderMarkdown`'s image regex
/// (`/!\[([^\]]*)\]\(([^)\s]+)\)/g`), over the raw (un-rendered) text.
fn md_images(md: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut rest = md;
    while let Some(bang) = rest.find("![") {
        let after = &rest[bang + 2..];
        let Some(alt_end) = after.find(']') else {
            break;
        };
        let alt = &after[..alt_end];
        let rest2 = &after[alt_end + 1..];
        let Some(paren) = rest2.strip_prefix('(') else {
            rest = rest2;
            continue;
        };
        let src_end = paren
            .find(|c: char| c == ')' || c.is_whitespace())
            .unwrap_or(paren.len());
        let src = &paren[..src_end];
        if src.is_empty() || paren.as_bytes().get(src_end) != Some(&b')') {
            rest = rest2;
            continue;
        }
        out.push((alt.to_string(), src.to_string()));
        rest = &paren[src_end + 1..];
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
/// `::CATEGORY`), an unknown `::TRANSITION`, an unknown `::HOVER_FOCUS`, an unknown
/// `::POINTS`.
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
        if *r == Role::Points && !crate::POINTS_MODES.contains(&v.as_str()) {
            out.push((
                "points/unknown",
                format!("::POINTS `{v}` is not 'auto', 'none' or 'always' — markers follow 'auto'"),
            ));
        }
        if *r == Role::HoverFocus && !crate::HOVER_FOCUS_MODES.contains(&v.as_str()) {
            out.push((
                "hover-focus/unknown",
                format!("::HOVER_FOCUS `{v}` is not 'series' or 'none' — hovering a mark focuses its series"),
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
        if *r == Role::Legend {
            if !crate::LEGEND_POSITIONS.contains(&v.as_str()) {
                out.push((
                    "legend/bad-value",
                    format!(
                        "::LEGEND `{v}` is not 'none', 'top', 'right', 'bottom' or 'left' — \
                         the legend is drawn where it always has"
                    ),
                ));
            } else if !panel_has_legend(roles) {
                out.push((
                    "legend/no-legend",
                    "::LEGEND has no legend or colourbar to hide or move on this panel".into(),
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
    let mut out = region_diags(roles, rows);
    let has = |want: Role| roles.iter().any(|(_, r)| *r == want);
    let geom = roles
        .iter()
        .find(|(i, r)| {
            *r == Role::Geometry
                && column_texts(rows, *i)
                    .iter()
                    .any(|t| t.trim().parse::<f64>().is_err())
        })
        .map(|(i, _)| *i);
    // M2a fix: the projection/classes/too-many-marks checks used to run only
    // on WKT (::MAP) panels, so a ::REGION or ::LAT/::LON map's
    // ::PROJECTION, ::CLASSES and point count went unchecked.
    let is_map = geom.is_some()
        || has(Role::Region)
        || (has(Role::Lat) && has(Role::Lon))
        || has(Role::Origin)
        || has(Role::Dest);
    // D25: `::FRAME` plays on a map, and on a bar/line/area/point chart
    // with an ::XAXIS (#663) — on any other chart the rows of every frame
    // are drawn together instead, which is almost never what was meant.
    let chart_plays = has(Role::X)
        && roles
            .iter()
            .find_map(|(_, r)| match r {
                Role::Value(k) => Some(*k),
                _ => None,
            })
            .is_some_and(crate::chart_frames::kind_plays);
    if !is_map && has(Role::Frame) && !chart_plays {
        out.push((
            Severity::Warning,
            "frame/not-a-map",
            "::FRAME plays on maps and on bar, line, area and scatter charts; this chart draws the rows of every frame together"
                .to_string(),
        ));
    }
    if !is_map {
        return out;
    }
    out.extend(flow_diags(roles, rows));
    out.extend(hexbin_diags(roles, rows));
    out.extend(frame_diags(roles, rows));
    // #664: a frame group only means something on a time map
    if has(Role::SyncFrame) && !has(Role::Frame) {
        out.push((
            Severity::Warning,
            "map/sync-frame-no-frame",
            "::SYNC_FRAME only links time maps — add a ::FRAME column, or drop it".to_string(),
        ));
    }
    // M3b (#642): a tooltip trend column.
    if let Some((i, _)) = roles
        .iter()
        .find(|(_, r)| matches!(r, Role::Value(Kind::Sparkline | Kind::Sparkbar)))
    {
        let first = rows
            .iter()
            .filter_map(|r| r.get(&format!("c{i}")).or_else(|| r.values().nth(*i)))
            .find(|v| !v.is_null());
        let list = match first {
            None => true,
            Some(Value::Array(_)) => true,
            Some(Value::String(s)) => serde_json::from_str::<Vec<Value>>(s).is_ok(),
            Some(_) => false,
        };
        if !list {
            out.push((
                Severity::Warning,
                "map/sparkline-not-list",
                "a map's ::SPARKLINE must be a list of numbers per feature — e.g. \
                 list(v ORDER BY week)::SPARKLINE"
                    .to_string(),
            ));
        }
        if has(Role::Hexbin) {
            out.push((
                Severity::Note,
                "map/sparkline-hexbin",
                "::SPARKLINE is ignored on a ::HEXBIN map — cells aggregate rows".to_string(),
            ));
        }
    }
    let first = |want: Role| {
        roles
            .iter()
            .find(|(_, r)| *r == want)
            .and_then(|(i, _)| column_texts(rows, *i).into_iter().next())
    };
    // M3e-1 (#646): `::NORTH_ARROW` takes 'print' or 'always'.
    if let Some(v) = first(Role::NorthArrow) {
        if !crate::NORTH_ARROW_MODES.contains(&v.to_ascii_lowercase().as_str()) {
            out.push((
                Severity::Warning,
                "map/north-arrow-value",
                format!("::NORTH_ARROW `{v}` is not 'print' or 'always' — no north arrow is drawn"),
            ));
        }
    }
    // M3a-1 (#644): small multiples on a map.
    if has(Role::Facet) || has(Role::FacetFree) {
        let fi = roles
            .iter()
            .find(|(_, r)| matches!(r, Role::Facet | Role::FacetFree))
            .map(|(i, _)| *i);
        if has(Role::Frame) {
            out.push((
                Severity::Warning,
                "map/facet-frame",
                "::FACET is ignored on a time map (::FRAME) — use one or the other".to_string(),
            ));
        } else {
            if let Some(i) = fi {
                let mut seen = std::collections::HashSet::new();
                for t in column_texts(rows, i) {
                    seen.insert(t);
                }
                if seen.len() > 12 {
                    out.push((
                        Severity::Warning,
                        "map/too-many-facets",
                        format!(
                            "{} ::FACET values on one map (over 12) — the first 12 are drawn; filter or use a coarser grouping",
                            seen.len()
                        ),
                    ));
                }
            }
            if has(Role::FacetFree) {
                out.push((
                    Severity::Note,
                    "map/facet-free",
                    "::FACET_FREE on a map shares the extent like ::FACET — maps keep one scale so panels compare".to_string(),
                ));
            }
            if has(Role::Story) {
                out.push((
                    Severity::Warning,
                    "map/facet-story",
                    "::STORY is ignored on small multiples".to_string(),
                ));
            }
        }
    }
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
    let wkt = geom.map(|g| column_texts(rows, g)).unwrap_or_default();
    let filled = roles.iter().any(|(i, r)| {
        matches!(r, Role::Value(_))
            || (*r == Role::Geometry && Some(*i) != geom && !column_texts(rows, *i).is_empty())
    });
    let areas = has(Role::Region)
        || wkt
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
    let wkt_points: usize = wkt
        .iter()
        .map(|t| match ggplot_rs::spatial::parse_wkt(t) {
            Some(ggplot_rs::spatial::Geometry::Point(_)) => 1,
            Some(ggplot_rs::spatial::Geometry::MultiPoint(p)) => p.len(),
            _ => 0,
        })
        .sum();
    // ::LAT/::LON rows with both numbers
    let latlon_points = match (
        roles.iter().find(|(_, r)| *r == Role::Lat).map(|(i, _)| *i),
        roles.iter().find(|(_, r)| *r == Role::Lon).map(|(i, _)| *i),
    ) {
        (Some(li), Some(lo)) => {
            let lats = cells(rows, li);
            let lons = cells(rows, lo);
            lats.iter()
                .zip(lons.iter())
                .filter(|(a, b)| {
                    a.as_deref().and_then(|s| s.parse::<f64>().ok()).is_some()
                        && b.as_deref().and_then(|s| s.parse::<f64>().ok()).is_some()
                })
                .count()
        }
        _ => 0,
    };
    let points = wkt_points + latlon_points;
    // M2c: a ::HEXBIN map aggregates its points into hexagons, so the raw
    // point count no longer reflects what's drawn.
    if points > MAP_POINTS_WARN && !has(Role::Hexbin) {
        out.push((
            Severity::Warning,
            "map/too-many-marks",
            format!(
                "{points} points on one map (over {MAP_POINTS_WARN}) — aggregate in SQL, e.g. to H3 \
                 cells (the h3 extension) or to regions, to keep the map light"
            ),
        ));
    }
    // M2a: ::SIZE rows that a graduated-symbol map won't draw (zero,
    // negative or missing — size is the symbol's area, not a position).
    if let Some(si) = roles
        .iter()
        .find(|(_, r)| *r == Role::Size)
        .map(|(i, _)| *i)
    {
        let k = cells(rows, si)
            .iter()
            .filter(|v| match v.as_deref().and_then(|s| s.parse::<f64>().ok()) {
                Some(n) => !(n.is_finite() && n > 0.0),
                None => true,
            })
            .count();
        // only when at least one row *does* have a usable size (otherwise
        // ::SIZE isn't driving symbols at all — no map/* lint for that)
        if k > 0 && k < rows.len() {
            out.push((
                Severity::Warning,
                "map/size-not-positive",
                format!(
                    "{k} rows have a zero, negative or missing ::SIZE and are not drawn — size is the symbol's area"
                ),
            ));
        }
    }
    // M2d: ::BIVARIATE.
    if let Some(bi) = roles
        .iter()
        .find(|(_, r)| *r == Role::Bivariate)
        .map(|(i, _)| *i)
    {
        let has_measure = roles.iter().any(|(_, r)| matches!(r, Role::Value(_)));
        if !has_measure {
            out.push((
                Severity::Warning,
                "map/bivariate-needs-measure",
                "::BIVARIATE needs a first measure (e.g. sales::BARCHART) — drawn as a plain map"
                    .to_string(),
            ));
        }
        if has(Role::Classes) {
            out.push((
                Severity::Note,
                "map/bivariate-classes",
                "::CLASSES is ignored on a bivariate map — both measures are split into tertiles"
                    .to_string(),
            ));
        }
        if has(Role::Origin) || has(Role::Dest) || has(Role::Hexbin) || has(Role::Size) {
            out.push((
                Severity::Warning,
                "map/bivariate-ignored",
                "::BIVARIATE has no effect with ::ORIGIN/::DEST, ::HEXBIN or ::SIZE".to_string(),
            ));
        }
        let distinct_count = |idx: usize| -> usize {
            cells(rows, idx)
                .iter()
                .filter_map(|v| v.as_deref()?.parse::<f64>().ok())
                .map(|f| (f * 1e6).round() as i64)
                .collect::<std::collections::HashSet<_>>()
                .len()
        };
        if has_measure && !has(Role::Hexbin) && !has(Role::Size) {
            if distinct_count(bi) > 0 && distinct_count(bi) < 3 {
                out.push((
                    Severity::Note,
                    "map/bivariate-few-levels",
                    "::BIVARIATE has fewer than 3 distinct values — fewer classes".to_string(),
                ));
            }
            if let Some(mi) = roles
                .iter()
                .find(|(_, r)| matches!(r, Role::Value(_)))
                .map(|(i, _)| *i)
            {
                if distinct_count(mi) > 0 && distinct_count(mi) < 3 {
                    out.push((
                        Severity::Note,
                        "map/bivariate-few-levels",
                        "the measure column has fewer than 3 distinct values — fewer classes"
                            .to_string(),
                    ));
                }
            }
        }
    }
    out
}

/// A raw cell of result column `idx` (clones a struct/object cell too,
/// unlike [`cells`], which only captures strings/numbers).
fn raw_cell(rows: &[Map<String, Value>], idx: usize, i: usize) -> Option<Value> {
    rows.get(i)?
        .get(&format!("c{idx}"))
        .or_else(|| rows[i].values().nth(idx))
        .cloned()
}

/// How one `::ORIGIN`/`::DEST` cell resolves, read straight from its raw
/// JSON cell (a struct column's cell is a JSON object, not a string — see
/// [`cells`]'s doc comment on that limitation).
enum FlowEnd {
    Code(String),
    Point,
    Missing,
}

fn flow_end_kind(v: &Value) -> FlowEnd {
    match v {
        Value::Null => FlowEnd::Missing,
        Value::Object(o) => {
            let has = |keys: &[&str]| {
                o.iter()
                    .any(|(k, _)| keys.iter().any(|w| k.eq_ignore_ascii_case(w)))
            };
            if has(&["lon", "lng", "x"]) && has(&["lat", "y"]) {
                FlowEnd::Point
            } else {
                FlowEnd::Missing
            }
        }
        Value::String(s) => {
            let t = s.trim();
            if t.is_empty() {
                FlowEnd::Missing
            } else if t.starts_with('{') || t.to_ascii_uppercase().starts_with("POINT") {
                FlowEnd::Point
            } else {
                FlowEnd::Code(t.to_string())
            }
        }
        Value::Number(n) => FlowEnd::Code(n.to_string()),
        _ => FlowEnd::Missing,
    }
}

/// Flow-map lint (`::ORIGIN`/`::DEST`, M2b): one end without the other, an
/// end that matches no region, flows that start and end at the same place,
/// and too many flows on one map.
fn flow_diags(
    roles: &[(usize, Role)],
    rows: &[Map<String, Value>],
) -> Vec<(Severity, &'static str, String)> {
    let mut out = Vec::new();
    let oi = roles
        .iter()
        .find(|(_, r)| *r == Role::Origin)
        .map(|(i, _)| *i);
    let di = roles
        .iter()
        .find(|(_, r)| *r == Role::Dest)
        .map(|(i, _)| *i);
    let has_values = |idx: Option<usize>| {
        idx.is_some_and(|i| {
            (0..rows.len()).any(|r| raw_cell(rows, i, r).is_some_and(|v| !v.is_null()))
        })
    };
    let (has_o, has_d) = (has_values(oi), has_values(di));
    if has_o != has_d {
        out.push((
            Severity::Warning,
            "map/flow-needs-both",
            "::ORIGIN without ::DEST (or the reverse) — a flow needs both ends; nothing is drawn"
                .to_string(),
        ));
    }
    let (Some(oi), Some(di)) = (oi, di) else {
        return out;
    };
    if !(has_o && has_d) {
        return out;
    }
    let n = rows.len();
    let o_ends: Vec<FlowEnd> = (0..n)
        .map(|i| {
            raw_cell(rows, oi, i)
                .map(|v| flow_end_kind(&v))
                .unwrap_or(FlowEnd::Missing)
        })
        .collect();
    let d_ends: Vec<FlowEnd> = (0..n)
        .map(|i| {
            raw_cell(rows, di, i)
                .map(|v| flow_end_kind(&v))
                .unwrap_or(FlowEnd::Missing)
        })
        .collect();
    let forced = roles
        .iter()
        .find(|(_, r)| *r == Role::Boundaries)
        .and_then(|(i, _)| column_texts(rows, *i).into_iter().next())
        .and_then(|s| crate::map::layer_name(&s));
    let mut codes: Vec<Option<String>> = Vec::new();
    for e in o_ends.iter().chain(d_ends.iter()) {
        if let FlowEnd::Code(c) = e {
            codes.push(Some(c.clone()));
        }
    }
    if !codes.is_empty() {
        if let Some(m) = crate::map::boundaries::match_codes(&codes, forced) {
            if m.unmatched_count > 0 {
                let list: Vec<String> = m
                    .unmatched
                    .iter()
                    .take(5)
                    .map(|c| format!("'{c}'"))
                    .collect();
                out.push((
                    Severity::Warning,
                    "map/flow-missing-end",
                    format!(
                        "{} flows have an end that matches nothing and are not drawn: {}{}",
                        m.unmatched_count,
                        list.join(", "),
                        if m.unmatched_count > list.len() {
                            ", …"
                        } else {
                            ""
                        }
                    ),
                ));
            }
        }
    }
    // self-flows: both ends resolve to the same region code (a Point end's
    // coordinates aren't parsed at lint time — the renderer's own
    // self-flow count, in the panel's a11y summary, is exact).
    let self_flows = (0..n)
        .filter(|&i| {
            matches!(
                (&o_ends[i], &d_ends[i]),
                (FlowEnd::Code(a), FlowEnd::Code(b)) if a.eq_ignore_ascii_case(b)
            )
        })
        .count();
    if self_flows > 0 {
        out.push((
            Severity::Note,
            "map/flow-self",
            format!("{self_flows} flows start and end at the same place and are not drawn"),
        ));
    }
    let both = (0..n)
        .filter(|&i| {
            !matches!(o_ends[i], FlowEnd::Missing) && !matches!(d_ends[i], FlowEnd::Missing)
        })
        .count();
    if both > 2_000 {
        out.push((
            Severity::Warning,
            "map/too-many-flows",
            format!(
                "{both} flows on one map (over 2,000) — aggregate to fewer origin/destination pairs (e.g. states) to keep it readable"
            ),
        ));
    }
    out
}

/// Density-binning lint (`::HEXBIN`, M2c): a bad size spec, no points to
/// bin (region/WKT-area maps, lines), Mercator's area distortion towards
/// the poles, and `::CATEGORY` being ignored (hexagons are coloured by
/// count or sum, never by a category).
fn hexbin_diags(
    roles: &[(usize, Role)],
    rows: &[Map<String, Value>],
) -> Vec<(Severity, &'static str, String)> {
    let mut out = Vec::new();
    let Some(hi) = roles
        .iter()
        .find(|(_, r)| *r == Role::Hexbin)
        .map(|(i, _)| *i)
    else {
        return out;
    };
    if let Some(v) = column_texts(rows, hi).into_iter().next() {
        if crate::map::hexbin::HexSize::parse(&v).is_err() {
            out.push((
                Severity::Warning,
                "map/hexbin-bad-size",
                format!("::HEXBIN '{v}' is not 'auto', '25 km' or a number — the map uses 'auto'"),
            ));
        }
    }
    let has = |want: Role| roles.iter().any(|(_, r)| *r == want);
    let geom_points = roles
        .iter()
        .find(|(_, r)| *r == Role::Geometry)
        .map(|(i, _)| *i)
        .is_some_and(|i| {
            column_texts(rows, i).iter().any(|t| {
                matches!(
                    ggplot_rs::spatial::parse_wkt(t),
                    Some(ggplot_rs::spatial::Geometry::Point(_))
                        | Some(ggplot_rs::spatial::Geometry::MultiPoint(_))
                )
            })
        });
    let has_points = (has(Role::Lat) && has(Role::Lon)) || geom_points;
    if !has_points {
        out.push((
            Severity::Warning,
            "map/hexbin-needs-points",
            "::HEXBIN aggregates points (::LAT/::LON or WKT POINTs); this map has none — ignored"
                .to_string(),
        ));
        return out;
    }
    let projection = roles
        .iter()
        .find(|(_, r)| *r == Role::Projection)
        .and_then(|(i, _)| column_texts(rows, *i).into_iter().next());
    if projection
        .as_deref()
        .and_then(crate::map::project::Projection::parse)
        == Some(crate::map::project::Projection::Mercator)
    {
        out.push((
            Severity::Note,
            "map/hexbin-mercator",
            "hexagons in 'mercator' cover less ground towards the poles — 'auto' keeps them \
             equal-area"
                .to_string(),
        ));
    }
    if has(Role::Category) {
        out.push((
            Severity::Note,
            "map/hexbin-category",
            "::CATEGORY is ignored on a ::HEXBIN map — hexagons are coloured by count or sum"
                .to_string(),
        ));
    }
    out
}

/// `::FRAME` time maps (M2e-A, #149, plan-maps-m2ef.md D13/D15/D25/D26):
/// too many distinct frames or features×frames cells, a duplicate
/// (feature, frame) row, `::STORY`/`::XAXIS` ignored on a frame map, and a
/// large WKT payload repeated per frame. Only runs for a map panel that
/// already carries `::FRAME` (`map_diags` calls this after confirming
/// `is_map`; the `!is_map` case is `frame/not-a-map`, checked by the
/// caller before that gate).
fn frame_diags(
    roles: &[(usize, Role)],
    rows: &[Map<String, Value>],
) -> Vec<(Severity, &'static str, String)> {
    let mut out = Vec::new();
    let has = |want: Role| roles.iter().any(|(_, r)| *r == want);
    let Some(fi) = roles
        .iter()
        .find(|(_, r)| *r == Role::Frame)
        .map(|(i, _)| *i)
    else {
        return out;
    };
    let frame_vals = cells(rows, fi);
    let distinct_frames: std::collections::HashSet<&str> =
        frame_vals.iter().filter_map(|v| v.as_deref()).collect();
    let n_frames = distinct_frames.len();
    if n_frames > 200 {
        out.push((
            Severity::Warning,
            "map/too-many-frames",
            format!(
                "{n_frames} ::FRAME values on one map (over 200) — the last 200 are kept; use a coarser period"
            ),
        ));
    }
    // The feature key: whichever identity column a time map uses, in the
    // same priority as `src/map/frames.rs::row_key` (D22) — ::REGION else
    // a WKT ::MAP geometry (::LAT/::LON pairs aren't a single column, so
    // this count is approximate for that case and skipped rather than
    // guessed at).
    let feature_idx = roles
        .iter()
        .find(|(_, r)| matches!(r, Role::Region | Role::Geometry))
        .map(|(i, _)| *i);
    let feature_vals: Vec<Option<String>> = match feature_idx {
        Some(i) => cells(rows, i),
        None => vec![None; rows.len()],
    };
    let distinct_features: std::collections::HashSet<&str> =
        feature_vals.iter().filter_map(|v| v.as_deref()).collect();
    let n_features = distinct_features.len();
    if feature_idx.is_some() && n_features > 0 && n_frames > 0 && n_features * n_frames > 100_000 {
        let max_frames = (100_000 / n_features).max(1).min(n_frames);
        out.push((
            Severity::Warning,
            "map/frame-cells",
            format!(
                "{n_features} features × {n_frames} frames is over 100,000 — the last {max_frames} frames are kept; aggregate or use fewer frames"
            ),
        ));
    }
    let mut seen = std::collections::HashSet::new();
    let mut duplicates = 0usize;
    for (f, w) in feature_vals.iter().zip(frame_vals.iter()) {
        if let (Some(f), Some(w)) = (f, w) {
            if !seen.insert((f.as_str(), w.as_str())) {
                duplicates += 1;
            }
        }
    }
    if duplicates > 0 {
        out.push((
            Severity::Warning,
            "map/frame-duplicate",
            format!(
                "{duplicates} rows repeat a feature within one frame — the first row is used; aggregate per feature and frame"
            ),
        ));
    }
    if has(Role::Story) {
        out.push((
            Severity::Warning,
            "map/frame-story",
            "::STORY is ignored on a time map (::FRAME) — Play steps through the frames instead"
                .to_string(),
        ));
    }
    if has(Role::X) {
        out.push((
            Severity::Note,
            "map/frame-xaxis",
            "::XAXIS is ignored on a time map — features are keyed by their region, point or geometry"
                .to_string(),
        ));
    }
    if !has(Role::Region) && feature_idx.is_some() {
        let wkt_bytes: usize = feature_vals.iter().flatten().map(|s| s.len()).sum();
        if wkt_bytes > 5_000_000 {
            out.push((
                Severity::Note,
                "map/frame-wkt",
                "WKT repeats per frame and this map's geometry is over 5 MB total — join a ::REGION code or one geometry table instead"
                    .to_string(),
            ));
        }
    }
    out
}

/// `::HEXBIN` on a planar `::SCATTER`/bubble chart (#426) — a chart-side
/// sibling of [`hexbin_diags`] (the map case), which only runs for a map
/// panel. Two checks: a bad size spec, and `::CATEGORY` being ignored (the
/// density layer drops it, same rule the map's own `::HEXBIN` has — one
/// hexbin for the whole panel, not one per category; see `src/lib.rs`'s
/// `cartesian`).
fn scatter_hexbin_diags(
    roles: &[(usize, Role)],
    rows: &[Map<String, Value>],
) -> Vec<(Severity, &'static str, String)> {
    let mut out = Vec::new();
    let Some(hi) = roles
        .iter()
        .find(|(_, r)| *r == Role::Hexbin)
        .map(|(i, _)| *i)
    else {
        return out;
    };
    // A map panel's `::HEXBIN` is `hexbin_diags`'s job (via `map_diags`), not
    // this one — conservatively: any role `map_diags` would treat as a map
    // trigger routes there instead, even on the rare false positive (a
    // numeric column mistakenly tagged `::GEOMETRY`), rather than risk
    // duplicate diagnostics on the same column.
    let is_map = roles.iter().any(|(_, r)| {
        matches!(
            r,
            Role::Geometry | Role::Region | Role::Lat | Role::Lon | Role::Origin | Role::Dest
        )
    });
    if is_map || !matches!(chart_kind(roles), Some(Kind::Point) | Some(Kind::Bubble)) {
        return out;
    }
    if let Some(v) = column_texts(rows, hi).into_iter().next() {
        if crate::map::hexbin::HexSize::parse(&v).is_err() {
            out.push((
                Severity::Warning,
                "chart/hexbin-bad-size",
                format!("::HEXBIN '{v}' is not 'auto' or a number — the chart uses 'auto'"),
            ));
        }
    }
    if roles.iter().any(|(_, r)| *r == Role::Category) {
        out.push((
            Severity::Note,
            "chart/hexbin-category",
            "::CATEGORY is ignored on a ::HEXBIN scatter — hexagons are coloured by point count \
             alone"
                .to_string(),
        ));
    }
    out
}

/// A column's cells as text (numbers too; `None` for NULL).
fn cells(rows: &[Map<String, Value>], idx: usize) -> Vec<Option<String>> {
    rows.iter()
        .map(|row| {
            match row
                .get(&format!("c{idx}"))
                .or_else(|| row.values().nth(idx))
            {
                Some(Value::String(s)) if !s.trim().is_empty() => Some(s.trim().to_string()),
                Some(Value::Number(n)) => Some(n.to_string()),
                _ => None,
            }
        })
        .collect()
}

/// `totals/summed-rate` (#207): a `::TOTALS` value whose all-columns
/// aggregation is `sum` ('sum', or a bare 'sum' in a list) over a table with a
/// rate or percentage column it does not name — a sum of rates means nothing.
/// A column named in the value ('Per 10k: sum') is deliberate and passes.
fn summed_rate_diag(
    roles: &[(usize, Role)],
    rows: &[Map<String, Value>],
    spec: &str,
    stmt: usize,
    short: &str,
) -> Option<Diagnostic> {
    use crate::table::{names_column, parse_totals, profile_column, Cell, ColumnSpec};
    let t = parse_totals(spec);
    if t.all.as_deref() != Some("sum") {
        return None;
    }
    let first = rows.first()?;
    let skip = |i: usize| {
        roles
            .iter()
            .any(|(j, r)| *j == i && matches!(r, Role::Title | Role::Info))
    };
    let fmt_of = |i: usize| {
        roles
            .iter()
            .find(|(j, r)| *j == i && crate::roles::is_table_format(*r))
            .map(|(_, r)| r.token())
    };
    let named = |name: &str| t.col.iter().any(|(n, _)| names_column(name, n));
    let l = crate::format::Locale::default();
    let mut hits: Vec<String> = Vec::new();
    let mut k = 0usize; // visible position: the first column is the row key
    for (i, name) in first.keys().enumerate() {
        if skip(i) {
            continue;
        }
        k += 1;
        if k == 1 || named(name) {
            continue;
        }
        let cells: Vec<Cell> = rows
            .iter()
            .map(|r| r.values().nth(i).map_or(Cell::Null, Cell::from_json))
            .collect();
        let fmt = fmt_of(i);
        let spec_c = ColumnSpec {
            name,
            fmt,
            first: false,
            range: None,
            many: rows.len() > 1,
        };
        if matches!(fmt, Some("SPARKLINE" | "SPARKBAR" | "BADGE")) {
            continue;
        }
        if profile_column(&spec_c, &cells, &l).rate {
            hits.push(name.clone());
        }
    }
    let first_hit = hits.first()?.clone();
    Some(Diagnostic {
        severity: Severity::Note,
        stmt,
        code: "totals/summed-rate",
        message: format!(
            "::TOTALS '{spec}' sums every column, `{}` too — a rate or percentage does not \
             add up; give it its own total: '{first_hit}: none' (or mean)",
            hits.join("`, `")
        ),
        sql: short.to_string(),
    })
}

/// `totals/unknown-column` (#228): a `::TOTALS 'name: agg'` entry that
/// names no column of the next table (raw name or header, case-insensitive
/// — the browser's `colNamed`): the browser and `--svg` skip it silently.
/// With `,` separators a header holding a comma splits ('Customers per
/// 10,000 people'): the note then says to use `;`.
fn totals_unknown_column_diag(
    roles: &[(usize, Role)],
    rows: &[Map<String, Value>],
    spec: &str,
    stmt: usize,
    short: &str,
) -> Option<Diagnostic> {
    use crate::table::{names_column, parse_totals};
    let t = parse_totals(spec);
    let first = rows.first()?;
    let skip = |i: usize| {
        roles
            .iter()
            .any(|(j, r)| *j == i && matches!(r, Role::Title | Role::Info))
    };
    let cols: Vec<&String> = first
        .keys()
        .enumerate()
        .filter(|(i, _)| !skip(*i))
        .map(|(_, k)| k)
        .collect();
    let mut missing: Vec<&str> = Vec::new();
    for (n, _) in &t.col {
        if !cols.iter().any(|c| names_column(c, n)) && !missing.contains(&n.as_str()) {
            missing.push(n);
        }
    }
    if missing.is_empty() {
        return None;
    }
    let mut list: Vec<String> = cols.iter().take(8).map(|c| format!("`{c}`")).collect();
    if cols.len() > 8 {
        list.push("…".into());
    }
    let comma = !spec.contains(';') && spec.contains(',') && cols.iter().any(|c| c.contains(','));
    Some(Diagnostic {
        severity: Severity::Note,
        stmt,
        code: "totals/unknown-column",
        message: format!(
            "::TOTALS '{spec}' names `{}` — no column of this table, so that entry is ignored; \
             name a column by its raw name or its header (case-insensitive): {}{}",
            missing.join("`, `"),
            list.join(", "),
            if comma {
                " — a header here holds a comma: separate the entries with `;`"
            } else {
                ""
            }
        ),
        sql: short.to_string(),
    })
}

/// Region and point inputs of a map: an unknown `::BOUNDARIES` layer,
/// `::REGION` codes the built-in boundaries don't know (up to five listed),
/// regions matched only by name, and latitudes beyond ±90 that would be
/// valid longitudes (`::LAT` and `::LON` swapped).
fn region_diags(
    roles: &[(usize, Role)],
    rows: &[Map<String, Value>],
) -> Vec<(Severity, &'static str, String)> {
    let mut out = Vec::new();
    let col = |want: Role| {
        roles
            .iter()
            .find(|(_, r)| *r == want)
            .map(|(i, _)| cells(rows, *i))
    };
    let forced = col(Role::Boundaries).and_then(|c| c.into_iter().flatten().next());
    let layer = match &forced {
        Some(f) => match crate::map::layer_name(f) {
            Some(l) => Some(l),
            None => {
                out.push((
                    Severity::Warning,
                    "map/unknown-region",
                    format!(
                        "::BOUNDARIES '{f}' is not a built-in layer ({}) — codes are matched against the default layers",
                        crate::map::boundaries::LAYERS.join(", ")
                    ),
                ));
                None
            }
        },
        None => None,
    };
    if let Some(codes) = col(Role::Region) {
        if let Some(m) = crate::map::boundaries::match_codes(&codes, layer) {
            if m.unmatched_count > 0 {
                let list: Vec<String> = m.unmatched.iter().map(|c| format!("'{c}'")).collect();
                out.push((
                    Severity::Warning,
                    "map/unknown-region",
                    format!(
                        "{} ::REGION code(s) match nothing in '{}' and are not drawn: {}{}",
                        m.unmatched_count,
                        m.layer.name,
                        list.join(", "),
                        if m.unmatched_count > list.len() {
                            ", …"
                        } else {
                            ""
                        }
                    ),
                ));
            }
            if m.by_name && m.rows.iter().any(|r| !r.is_empty()) {
                out.push((
                    Severity::Note,
                    "map/region-by-name",
                    format!(
                        "::REGION matched '{}' by name — codes (ISO 3166, AGS, NUTS) are unambiguous; names may not be",
                        m.layer.name
                    ),
                ));
            }
        }
    }
    if let (Some(lat), Some(lon)) = (col(Role::Lat), col(Role::Lon)) {
        let num = |v: &Option<String>| v.as_deref().and_then(|s| s.parse::<f64>().ok());
        let lats: Vec<f64> = lat.iter().filter_map(num).collect();
        let lons: Vec<f64> = lon.iter().filter_map(num).collect();
        if lats.iter().any(|v| v.abs() > 90.0)
            && lats.iter().all(|v| v.abs() <= 180.0)
            && lons.iter().all(|v| v.abs() <= 90.0)
        {
            out.push((
                Severity::Warning,
                "map/lat-lon-swapped",
                "::LAT has values beyond ±90 that would be valid longitudes, and ::LON stays within ±90 — are ::LAT and ::LON swapped?"
                    .to_string(),
            ));
        }
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

/// `::KPI_SIZE` (#405): `kpi/bad-kpi-size` (a value that isn't 'compact' or
/// 'normal' — treated as 'normal'), `kpi/kpi-size-misplaced` (not on a
/// `::METRIC`/`::MONEY`/`::PERCENT`/`::COMPACT` KPI tile or a `::GROUP` box
/// — e.g. a chart or table). Checked on every panel, not only a KPI's.
fn kpi_size_checks(
    roles: &[(usize, Role)],
    rows: &[Map<String, Value>],
) -> Vec<(Severity, &'static str, String)> {
    let mut out = Vec::new();
    let Some((idx, _)) = roles.iter().find(|(_, r)| *r == Role::KpiSize) else {
        return out;
    };
    if let Some(Some(Value::String(v))) = rows.first().map(|r| r.get(&format!("c{idx}"))) {
        if !matches!(v.trim().to_ascii_lowercase().as_str(), "compact" | "normal") {
            out.push((
                Severity::Warning,
                "kpi/bad-kpi-size",
                format!("::KPI_SIZE `{v}` is not 'compact' or 'normal' — treated as 'normal'"),
            ));
        }
    }
    let on_kpi_or_group = is_kpi(roles) || roles.iter().any(|(_, r)| *r == Role::GroupStart);
    if !on_kpi_or_group {
        out.push((
            Severity::Warning,
            "kpi/kpi-size-misplaced",
            "::KPI_SIZE only applies to a ::METRIC/::MONEY/::PERCENT/::COMPACT KPI tile or a ::GROUP box".into(),
        ));
    }
    out
}

/// The panel's chart kind, if it's a chart (`count()::BARCHART` → `Kind::Bar`).
fn chart_kind(roles: &[(usize, Role)]) -> Option<Kind> {
    // On a map a `list(..)::SPARKLINE`/`::SPARKBAR` is a tooltip trend
    // (M3b, #642), never the panel's kind — skip it there.
    let is_map = roles.iter().any(|(_, r)| {
        matches!(
            r,
            Role::Geometry | Role::Region | Role::Lat | Role::Lon | Role::Origin | Role::Dest
        )
    });
    roles.iter().find_map(|(_, r)| match r {
        Role::Value(k) if !(is_map && matches!(k, Kind::Sparkline | Kind::Sparkbar)) => Some(*k),
        _ => None,
    })
}

/// Does this panel ever draw a legend or colourbar `::LEGEND` could move or
/// hide? Maps (best-effort: any geometry/region/lat-lon panel may show a
/// category legend or a colourbar) and heatmaps/pies/donuts always do;
/// cartesian charts and small multiples only with a `::CATEGORY`. Everything
/// else — KPIs, tables, text, gauges, candlesticks, calendars, sparklines,
/// histograms, box/violin without a category, waterfalls, bullets, funnels,
/// diagrams, tracks — never shows one.
fn panel_has_legend(roles: &[(usize, Role)]) -> bool {
    let is_map = roles
        .iter()
        .any(|(_, r)| matches!(r, Role::Geometry | Role::Region | Role::Origin | Role::Dest))
        || (roles.iter().any(|(_, r)| *r == Role::Lat)
            && roles.iter().any(|(_, r)| *r == Role::Lon));
    if is_map {
        return true;
    }
    match chart_kind(roles) {
        Some(Kind::Heatmap | Kind::Pie | Kind::Donut) => true,
        Some(
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
            | Kind::Jitter
            | Kind::Density
            | Kind::Radar
            | Kind::Boxplot
            | Kind::Violin,
        ) => roles.iter().any(|(_, r)| *r == Role::Category),
        _ => false,
    }
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

/// #330: the column a ::ROWSELECT table's click binds to — the first output
/// column, skipping a ::TITLE (the card caption) or ::INFO (dropped before
/// the table ever renders) column, the same "first column" the browser
/// (`web/app.js` `k0 = cols[0]`) picks.
fn rowselect_key_column(roles: &[(usize, Role)], rows: &[Map<String, Value>]) -> Option<String> {
    let skip_title = role_index(roles, Role::Title);
    let skip_info = role_index(roles, Role::Info);
    rows.first()?
        .keys()
        .enumerate()
        .find(|(i, _)| Some(*i) != skip_title && Some(*i) != skip_info)
        .map(|(_, k)| k.clone())
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

/// #424: `::IDENTITY` / `::REFLINE` are read inside `cartesian()` only — a
/// `::HEATMAP` dispatches to `render_heatmap`/`render_heatmap_categorical`
/// before ever reaching it (`lib.rs`'s `match kind` in `render_with`), so
/// both are silently dropped there today. Drawing a diagonal/reference line
/// over a tile grid needs the grid's own x/y to be numeric-binned (not the
/// usual discrete row/column labels) for the line to mean anything, which
/// `render_heatmap` doesn't distinguish yet — so this warns instead of
/// drawing a line that would be wrong on a discrete grid.
fn heatmap_line_diags(roles: &[(usize, Role)], stmt: usize, short: &str) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    if chart_kind(roles) != Some(Kind::Heatmap) {
        return out;
    }
    for role in [Role::Identity, Role::RefLine] {
        if roles.iter().any(|(_, r)| *r == role) {
            out.push(Diagnostic {
                severity: Severity::Warning,
                stmt,
                code: "heatmap/line-dropped",
                message: format!(
                    "::{} is dropped on ::HEATMAP — reference/identity lines aren't drawn over \
                     tiles yet",
                    role.token()
                ),
                sql: short.to_string(),
            });
        }
    }
    out
}

/// Status colour scales on tiles (see the call site).
fn tile_scale_diags(
    roles: &[(usize, Role)],
    rows: &[Map<String, Value>],
    stmt: usize,
    short: &str,
) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    let mut warn = |code, message: String| {
        out.push(Diagnostic {
            severity: Severity::Warning,
            stmt,
            code,
            message,
            sql: short.to_string(),
        })
    };
    let tiles = roles
        .iter()
        .find(|(_, r)| matches!(r, Role::Value(Kind::Heatmap | Kind::Calendar)));
    let scale = roles.iter().find(|(_, r)| matches!(r, Role::ColorScale(_)));
    let table = roles
        .iter()
        .any(|(_, r)| matches!(r, Role::Table | Role::PagedTable));
    let rules = roles
        .iter()
        .find(|(_, r)| *r == Role::Thresholds)
        .and_then(|(idx, _)| column_texts(rows, *idx).into_iter().next())
        .and_then(|spec| crate::thresholds::Rules::parse(&spec).ok());
    match (scale, tiles) {
        (Some((_, r)), None) if !table => warn(
            "colorscale/no-tiles",
            format!(
                "::{} colours a table column's cells or a ::HEATMAP / ::CALENDAR's tiles — \
                 this panel has neither, so it does nothing",
                r.token()
            ),
        ),
        (Some((_, r)), Some(_)) if rules.is_some() => warn(
            "colorscale/with-thresholds",
            format!(
                "::THRESHOLDS colour the tiles in steps, so the ::{} is ignored — keep one",
                r.token()
            ),
        ),
        _ => {}
    }
    let (Some((vidx, _)), Some(rules)) = (tiles, rules) else {
        return out;
    };
    let key = format!("c{vidx}");
    let vals: Vec<f64> = rows
        .iter()
        .filter_map(|r| r.get(&key).or_else(|| r.values().nth(*vidx)))
        .filter_map(|v| match v {
            Value::Number(n) => n.as_f64(),
            Value::String(s) => s.parse::<f64>().ok(),
            _ => None,
        })
        .filter(|f| f.is_finite())
        .collect();
    let bands = rules.bands();
    let band_of = |v: f64| bands.iter().position(|b| b.contains(v));
    let cuts: Vec<f64> = bands
        .iter()
        .flat_map(|b| [b.lo, b.hi])
        .flatten()
        .map(|(c, _)| c.abs())
        .filter(|c| *c > 0.0)
        .collect();
    let fractions = |xs: &[f64]| xs.iter().all(|x| x.abs() <= 1.0);
    // Only a unit mismatch: fractions against cuts above 1, or the reverse.
    let unit_mismatch = !cuts.is_empty() && fractions(&vals) != fractions(&cuts);
    if unit_mismatch
        && bands.len() > 1
        && vals.len() > 1
        && vals.iter().all(|&v| band_of(v) == band_of(vals[0]))
    {
        let (lo, hi) = vals
            .iter()
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), &v| {
                (a.min(v), b.max(v))
            });
        let band = band_of(vals[0]).map(|i| &bands[i]);
        warn(
            "thresholds/one-band",
            format!(
                "every tile is in the same ::THRESHOLDS band ({}, {}) — the values run {} to {}; \
                 are the rules in the values' unit (percent 0–100 vs a fraction 0–1)?",
                band.map_or_else(String::new, |b| b.label(|v| format!("{v}"))),
                band.map_or("neutral", |b| b.status.as_str()),
                fmt_num(lo),
                fmt_num(hi),
            ),
        );
    }
    out
}

/// Does `::XBREAKS`/`::YBREAKS` have any effect on this panel? Only the
/// cartesian x/y chart kinds route through `render_cartesian`'s continuous
/// scale wiring (`lib.rs`'s `match kind` dispatch before that call) —
/// pies/donuts, gauges, histograms, density, QQ, heatmaps, calendars,
/// candlesticks, radar, sparklines, waterfalls, bullets, funnels, and
/// non-chart panels (KPIs, tables, maps, markdown — no `Role::Value` at all)
/// ignore it.
fn kind_uses_axis_breaks(roles: &[(usize, Role)]) -> bool {
    match chart_kind(roles) {
        None => false,
        Some(
            Kind::Pie
            | Kind::Donut
            | Kind::Gauge
            | Kind::Histogram
            | Kind::Density
            | Kind::QQ
            | Kind::Heatmap
            | Kind::Calendar
            | Kind::Candlestick
            | Kind::Radar
            | Kind::Sparkline
            | Kind::Sparkbar
            | Kind::Waterfall
            | Kind::Bullet
            | Kind::Funnel,
        ) => false,
        Some(_) => true,
    }
}

/// `::XBREAKS`/`::YBREAKS` ('100,1000,10000'): every entry must parse as a
/// number (anofox_visualization::role_breaks silently drops bad ones, which
/// would otherwise fail without explanation), and the role only does
/// anything on a cartesian chart (see [`kind_uses_axis_breaks`]).
fn breaks_diags(
    roles: &[(usize, Role)],
    rows: &[Map<String, Value>],
    stmt: usize,
    short: &str,
) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    let mut warn = |code, message: String| {
        out.push(Diagnostic {
            severity: Severity::Warning,
            stmt,
            code,
            message,
            sql: short.to_string(),
        })
    };
    for role in [Role::XBreaks, Role::YBreaks] {
        let Some((idx, _)) = roles.iter().find(|(_, r)| *r == role) else {
            continue;
        };
        let Some(spec) = column_texts(rows, *idx).into_iter().next() else {
            continue;
        };
        let bad: Vec<&str> = spec
            .split(',')
            .map(str::trim)
            .filter(|t| !t.is_empty() && t.parse::<f64>().is_err())
            .collect();
        if !bad.is_empty() {
            warn(
                "breaks/non-numeric",
                format!(
                    "::{} '{spec}' has non-numeric entries ({}) — expected a comma-separated \
                     list of numbers, e.g. '100,1000,10000'",
                    role.token(),
                    bad.join(", "),
                ),
            );
        }
        if !kind_uses_axis_breaks(roles) {
            warn(
                "breaks/non-cartesian",
                format!(
                    "::{} only places ticks on a cartesian x/y chart — this panel's kind \
                     ignores it",
                    role.token()
                ),
            );
        }
    }
    out
}

/// Does `::FACET`/`::FACET_FREE` do anything on this panel's chart kind?
/// Every cartesian kind (bars/lines/areas/scatter/box/violin/…), plus
/// `::HISTOGRAM` and `::QQ` (#423) and `::HEATMAP` (#424), honour it; the
/// rest still draw one overlapping panel and ignore it.
fn kind_uses_facet(roles: &[(usize, Role)]) -> bool {
    // a map draws small multiples (M3a-1) — its own lint is map_diags'
    if roles.iter().any(|(_, r)| {
        matches!(
            r,
            Role::Geometry | Role::Region | Role::Lat | Role::Lon | Role::Origin | Role::Dest
        )
    }) {
        return true;
    }
    match chart_kind(roles) {
        None => false,
        Some(
            Kind::Pie
            | Kind::Donut
            | Kind::Gauge
            | Kind::Calendar
            | Kind::Candlestick
            | Kind::Radar
            | Kind::Sparkline
            | Kind::Sparkbar
            | Kind::Waterfall
            | Kind::Bullet
            | Kind::Funnel,
        ) => false,
        Some(_) => true,
    }
}

/// #423/#446: `::FACET`/`::FACET_FREE` dropped on a chart kind that doesn't
/// honour it yet (see [`kind_uses_facet`]); `::HIST_BINS`/`::HIST_NORM`
/// anywhere but a `::HISTOGRAM`. `::CATEGORY` on `::HISTOGRAM` is no longer
/// warned about — ggplot-rs 0.28's `GeomHistogram` reads a per-row fill
/// aesthetic (#446), same as `GeomDensity` already did.
fn histogram_qq_diags(roles: &[(usize, Role)], stmt: usize, short: &str) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    let mut warn = |code, message: String| {
        out.push(Diagnostic {
            severity: Severity::Warning,
            stmt,
            code,
            message,
            sql: short.to_string(),
        })
    };
    for role in [Role::Facet, Role::FacetFree] {
        if roles.iter().any(|(_, r)| *r == role) && !kind_uses_facet(roles) {
            warn(
                "facet/unsupported-kind",
                format!(
                    "::{} draws one panel per value — this panel's chart kind draws them all \
                     on top of each other instead",
                    role.token()
                ),
            );
        }
    }
    let kind = chart_kind(roles);
    for role in [Role::HistBins, Role::HistNorm] {
        if roles.iter().any(|(_, r)| *r == role) && kind != Some(Kind::Histogram) {
            warn(
                "histogram/wrong-kind",
                format!("::{} only does anything on a ::HISTOGRAM", role.token()),
            );
        }
    }
    out
}

/// A number for a lint message (no float noise).
fn fmt_num(v: f64) -> String {
    let r = (v * 1e4).round() / 1e4;
    format!("{r}")
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
/// (`requires/on-setup`.) A `SET` is read token by token (#166): one whose
/// subquery reads a file reads it, one that builds a path reads nothing.
/// #441: a plain text scan (same style as [`reads_a_file`], no real parsing)
/// for `GROUP BY` with no `ORDER BY` anywhere in the statement — deliberately
/// simple, so it can only ever under-report (a query that orders inside a
/// CTE, or on a later line this doesn't see, is missed, never flagged).
fn has_group_by_no_order_by(sql: &str) -> bool {
    let code = sql.to_ascii_lowercase();
    code.contains("group by") && !code.contains("order by")
}

fn reads_a_file(sql: &str) -> bool {
    let code = sql.to_ascii_lowercase();
    if sql::lex::first_keyword(&code).as_deref() == Some("SET") {
        return set_reads_a_file(&code);
    }
    ["read_", "glob(", "parquet_", "sniff_csv("]
        .iter()
        .any(|f| code.contains(f))
        || DATA_FILE_EXTS
            .iter()
            .any(|e| code.contains(&format!("{e}'")))
}

/// The extensions of a quoted data-file path (`FROM 'x.parquet'`).
const DATA_FILE_EXTS: [&str; 7] = [
    ".parquet", ".csv", ".json", ".jsonl", ".tsv", ".xlsx", ".ndjson",
];

/// A `SET` (lowercased) reads a file when it calls a read function
/// (`read_parquet(…)`, `glob(…)`, …) or selects `FROM` a quoted data-file
/// path. Strings are tokens of their own, so `… || '/a.parquet'` or
/// `'read_parquet(x)'` read nothing.
fn set_reads_a_file(code: &str) -> bool {
    use sql::lex::TokKind;
    let toks: Vec<_> = sql::lex::tokenize(code)
        .into_iter()
        .filter(|t| !t.is_trivia())
        .collect();
    toks.windows(2).any(|w| {
        let (word, next) = (w[0].text(code), &w[1]);
        match w[0].kind {
            TokKind::Word if next.is_punct(code, '(') => {
                word.starts_with("read_")
                    || word.starts_with("parquet_")
                    || word == "glob"
                    || word == "sniff_csv"
            }
            TokKind::Word if word == "from" && next.kind == TokKind::String => {
                let path = next.text(code).trim_end_matches('\'');
                DATA_FILE_EXTS.iter().any(|e| path.ends_with(e))
            }
            _ => false,
        }
    })
}

/// Check `requires` the way the locked server does (see
/// [`sql::requires::check`]), with `prelude`'s session state. Shared with
/// the headless CLI renderers (`dashboard` static HTML, `dashboard --svg`)
/// and `--check`, all of which back `run_query` with one persistent
/// session (#435) and so always pass `prelude = ""` (there is nothing to
/// replay — the session already has it), and with the wasm `lint_step`
/// (via [`check_opts`]), which backs `run_query` with a fresh session each
/// call and so passes the real accumulated prelude text to replay in
/// front of `q`. A `session: true` query reads session variables
/// (`getvariable()`), which needs `prelude` replayed in the fresh-session
/// case; a `session: false` one (`glob()` file checks) never depended on
/// session state either way, so `q` goes alone regardless — but always
/// `;`-terminated: a CI hang traced to `tests/fixtures/lint/requires-setup.sql`
/// found that `sql::requires::glob_sql`'s query has no trailing `;` of its
/// own, which this `else` branch used to send verbatim. That's harmless to a
/// one-shot `run_query` (a fresh process per call, or `-c`), but
/// `host::duckdb_cli::OneProcessSession::round` (the CLI's persistent
/// session, #435) appends its own sentinel statements straight after `q` in
/// the same write — without a `;`, the sentinel merges into `q`'s own
/// (now two-statement, erroring) text and is never actually run, so its
/// marker never comes back and the round hangs. (`OneProcessSession::round`
/// now also carries its own timeout backstop for any such bug, but the
/// query text itself must be correct regardless.)
pub fn unmet_requirement<F>(
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
            format!("{q};")
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
    use crate::inputs::input_defaults;
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
                LintOptions {
                    design: false,
                    ..Default::default()
                },
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
                LintOptions {
                    design: false,
                    ..Default::default()
                },
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
        // A SET VARIABLE whose subquery reads the file reads it (#166) …
        assert!(set(
            "SET VARIABLE x = (SELECT max(v) FROM read_parquet(getvariable('rd') || '/a'));"
        )
        .is_empty());
        assert!(set("SET VARIABLE x = (SELECT max(v) FROM 'data/a.parquet');").is_empty());
        assert!(
            set("SET VARIABLE n = (SELECT count(*) FROM glob(getvariable('rd') || '/*'));")
                .is_empty()
        );
        // … one that builds a path, reads a table, or names a read function
        // only in a string reads nothing.
        for stmt in [
            "SET VARIABLE f = getvariable('rd') || '/a.parquet';",
            "SET\nVARIABLE f = 'data/a.parquet';",
            "SET VARIABLE f = 'read_parquet(x)';",
            "SET VARIABLE x = (SELECT max(v) FROM t);",
        ] {
            assert_eq!(codes(&set(stmt)), ["requires/on-setup"], "{stmt}");
        }
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
                LintOptions {
                    design: false,
                    ..Default::default()
                },
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
            LintOptions {
                design: false,
                ..Default::default()
            },
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
    fn folder_setup_table_warns() {
        // #387: served, a materialising setup is rebuilt per panel request.
        let script = "CREATE OR REPLACE TEMP TABLE sx AS SELECT 1 AS a;\n\
                      CREATE TEMP VIEW v AS SELECT 1 AS a;\n\
                      SELECT a::BARCHART, 'x'::XAXIS FROM sx;";
        let d = folder_diags(&[("a.sql".to_string(), script.to_string())]);
        let hits: Vec<_> = d
            .iter()
            .filter(|(_, x)| x.code == "folder/setup-table")
            .collect();
        assert_eq!(hits.len(), 1, "{d:?}");
        assert_eq!(hits[0].1.stmt, 0);
        assert!(materialises_table("-- note\ncreate table t as select 1"));
        assert!(materialises_table(
            "CREATE TABLE IF NOT EXISTS t AS (SELECT 1)"
        ));
        assert!(!materialises_table("CREATE TABLE t (a INT)"));
        assert!(!materialises_table("CREATE VIEW v AS SELECT 1"));
        assert!(!materialises_table("SELECT 'CREATE TABLE x AS'"));
    }

    #[test]
    fn owner_certified_and_folders_are_checked() {
        let codes = |script: &str| -> Vec<&'static str> {
            header_diags(script).iter().map(|d| d.code).collect()
        };
        assert!(codes("-- @owner Finance\n-- @certified 2026-10-15\nSELECT 1;").is_empty());
        assert_eq!(codes("-- @owner \nSELECT 1;"), ["header/empty-owner"]);
        assert!(codes("-- @nav Weekly sales\nSELECT 1;").is_empty());
        assert_eq!(codes("-- @nav \nSELECT 1;"), ["header/empty-nav"]);
        assert_eq!(
            codes(&format!("-- @nav {}\n", "x".repeat(25))),
            ["header/long-nav"]
        );
        assert!(codes(&format!("-- @nav {}\n", "x".repeat(24))).is_empty());
        assert_eq!(
            codes("-- @certified 2026-02-30\n"),
            ["header/bad-certified"]
        );
        assert_eq!(codes("-- @certified\n"), ["header/bad-certified"]);
        assert!(codes("-- @order 1\nSELECT 1;").is_empty());
        assert!(codes("-- @order -3\nSELECT 1;").is_empty());
        assert_eq!(codes("-- @order\n"), ["header/bad-order"]);
        assert_eq!(codes("-- @order one\n"), ["header/bad-order"]);
        assert_eq!(codes("-- @order 1.5\n"), ["header/bad-order"]);
        assert!(codes("-- @astables on\nSELECT 1;").is_empty());
        assert!(codes("-- @astables off\nSELECT 1;").is_empty());
        assert_eq!(codes("-- @astables\n"), ["header/bad-astables"]);
        assert_eq!(codes("-- @astables yes\n"), ["header/bad-astables"]);
        assert_eq!(codes("-- @astables On\n"), ["header/bad-astables"]);
        assert!(codes("-- @markdown_style plain\nSELECT 1;").is_empty());
        assert!(codes("-- @markdown_style card\nSELECT 1;").is_empty());
        assert!(codes("-- @markdown_style callout\nSELECT 1;").is_empty());
        assert_eq!(codes("-- @markdown_style\n"), ["header/bad-markdown-style"]);
        assert_eq!(
            codes("-- @markdown_style boxed\n"),
            ["header/bad-markdown-style"]
        );
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
        // #355: the folder check is on nav_label() (nav, else title), not
        // the raw title — so an @nav tells two same-titled dashboards
        // apart, and a colliding @nav still catches them even with
        // different titles (either direction).
        let d = folder_diags(&[
            f("x.sql", "-- @title Sales\n-- @nav Weekly\nSELECT 1;"),
            f("y.sql", "-- @title Sales\n-- @nav Monthly\nSELECT 1;"),
        ]);
        assert!(
            d.is_empty(),
            "same @title, distinct @nav: no longer a collision: {d:?}"
        );
        let d = folder_diags(&[
            f("x.sql", "-- @title Weekly revenue\n-- @nav Risk\nSELECT 1;"),
            f("y.sql", "-- @title Margin trend\n-- @nav risk\nSELECT 1;"),
        ]);
        let got: Vec<(&str, &str)> = d.iter().map(|(f, d)| (f.as_str(), d.code)).collect();
        assert_eq!(
            got,
            [("y.sql", "folder/duplicate-title")],
            "different titles, the same @nav (case-insensitive): a collision"
        );
        assert!(d[0].1.message.contains("x.sql"), "{}", d[0].1.message);
        let d = folder_diags(&[
            f("x.sql", "-- @title Supplier risk\nSELECT 1;"),
            f(
                "y.sql",
                "-- @title Weekly revenue\n-- @nav Supplier risk\nSELECT 1;",
            ),
        ]);
        let got: Vec<(&str, &str)> = d.iter().map(|(f, d)| (f.as_str(), d.code)).collect();
        assert_eq!(
            got,
            [("y.sql", "folder/duplicate-title")],
            "one's @nav matches the other's plain @title: still a collision"
        );
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

    const FIT_CHART: &str = "SELECT 1 AS x ::XAXIS, 2 AS y ::BARCHART;\n";

    fn fit_codes(script: &str) -> Vec<Diagnostic> {
        check(script, |_| Ok(vec![]))
            .into_iter()
            .filter(|d| d.code.starts_with("layout/fit-"))
            .collect()
    }

    /// #400: a ::FIT tab with more rows of panels than a 1366×768 screen
    /// holds at the 120 px minimum is flagged (it would fall back to
    /// scrolling); one that fits, a packed one and an unfitted tab are not.
    #[test]
    fn fit_too_many_panels_is_a_warning() {
        let ok = format!(
            "SELECT 'A'::TAB;\nSELECT 'fit'::FIT;\n{}SELECT 'B'::TAB;\n{}",
            FIT_CHART.repeat(4),
            FIT_CHART.repeat(20)
        );
        assert!(fit_codes(&ok).is_empty(), "{:?}", fit_codes(&ok));

        let over = format!(
            "SELECT 'A'::TAB;\nSELECT 'fit'::FIT;\n{}",
            FIT_CHART.repeat(12)
        );
        let d = fit_codes(&over);
        assert_eq!(d.len(), 1, "{d:?}");
        assert_eq!(d[0].code, "layout/fit-too-many");
        assert_eq!(d[0].severity, Severity::Warning);
        assert_eq!(d[0].stmt, 2, "reported on the ::FIT");
        assert!(d[0].message.contains("6 rows"), "{}", d[0].message);

        // four to a row: 3 rows fit
        let packed = format!(
            "SELECT 4::COLUMNS;\nSELECT 'A'::TAB;\nSELECT 'fit'::FIT;\n{}",
            FIT_CHART.repeat(12)
        );
        assert!(fit_codes(&packed).is_empty(), "{:?}", fit_codes(&packed));

        // no tabs: the ::FIT covers the whole dashboard
        let flat = format!("{}SELECT 'fit'::FIT;\n{}", FIT_CHART, FIT_CHART.repeat(11));
        assert!(has(&fit_codes(&flat), "layout/fit-too-many"));

        // a sub-tab's ::FIT counts only the sub-tab's own panels (the
        // parent tab's above it are not fitted), under its own bar: 8
        // panels = 4 rows fit 613 px, 10 = 5 rows don't
        let sub = |own: usize, mine: usize| {
            format!(
                "SELECT 'A'::TAB;\n{}SELECT 'S1'::SUBTAB;\nSELECT 'fit'::FIT;\n{}SELECT 'S2'::SUBTAB;\n{}",
                FIT_CHART.repeat(own),
                FIT_CHART.repeat(mine),
                FIT_CHART.repeat(30)
            )
        };
        assert!(
            fit_codes(&sub(20, 8)).is_empty(),
            "{:?}",
            fit_codes(&sub(20, 8))
        );
        assert!(has(&fit_codes(&sub(0, 10)), "layout/fit-too-many"));
        // a ::VIEW alternative's ::FIT: only that alternative's panels
        let view = |mine: usize| {
            format!(
                "SELECT 'A'::TAB;\n{}SELECT 'Lines' AS v ::VIEW;\nSELECT 'fit'::FIT;\n{}SELECT 'Bars' AS v ::VIEW;\n{}SELECT 1::ENDVIEW;\n",
                FIT_CHART.repeat(12),
                FIT_CHART.repeat(mine),
                FIT_CHART.repeat(30)
            )
        };
        assert!(fit_codes(&view(6)).is_empty(), "{:?}", fit_codes(&view(6)));
        assert!(has(&fit_codes(&view(10)), "layout/fit-too-many"));
    }

    #[test]
    fn fit_needed_height_packs_rows_and_kpi_strips() {
        let chart = FitItem::Panel {
            span: 6,
            kpi: false,
            compact: false,
            grouped: false,
        };
        let kpi = FitItem::Panel {
            span: 6,
            kpi: true,
            compact: false,
            grouped: false,
        };
        // 4 charts two to a row: 2 × 120 + 1 gap
        assert_eq!(fit_needed_height(&[chart; 4]), (258.0, 2));
        // a strip of 4 KPI tiles is one boxed row (96 + its padding), then a chart row
        let (h, rows) = fit_needed_height(&[kpi, kpi, kpi, kpi, chart]);
        assert_eq!(rows, 2);
        assert_eq!(h, 96.0 + 40.0 + 18.0 + 120.0);
        // a heading is a row of its own
        assert_eq!(fit_needed_height(&[FitItem::Heading, chart]).1, 2);
        assert_eq!(literal_number("SELECT 4 AS c0 "), Some(4.0));
        assert_eq!(literal_number("SELECT 'x' AS c0"), None);
        // #405: a strip of 4 ::KPI_SIZE 'compact' tiles needs only its own
        // (36 px) floor, not the normal 96 px one
        let kpi_c = FitItem::Panel {
            span: 6,
            kpi: true,
            compact: true,
            grouped: false,
        };
        let (hc, rowsc) = fit_needed_height(&[kpi_c, kpi_c, kpi_c, kpi_c, chart]);
        assert_eq!(rowsc, 2);
        assert_eq!(hc, 36.0 + 40.0 + 18.0 + 120.0);
        assert!(hc < h);
    }

    #[test]
    fn kpi_size_literal_reads_a_directive_statement() {
        // a plain literal (the common case): the real value decides
        assert_eq!(
            literal_text_col("SELECT 'Status' AS c0, 'compact' AS c1", 1),
            Some("compact".into())
        );
        assert_eq!(
            literal_text_col("SELECT 'compact' AS c0", 0),
            Some("compact".into())
        );
        // a data-driven column: unreadable without a DataProvider
        assert_eq!(
            literal_text_col("SELECT x AS c0, 'compact' AS c1 FROM t", 1),
            None
        );
        assert_eq!(literal_text_col("SELECT count(*) AS c0 FROM t", 0), None);
    }

    /// #400: ::FIT placement that leaves its scope unclear.
    #[test]
    fn fit_ambiguous_placement_is_a_warning() {
        let tabs = |pre: &str, a: &str, b: &str| {
            format!("{pre}SELECT 'A'::TAB;\n{a}{FIT_CHART}SELECT 'B'::TAB;\n{b}{FIT_CHART}")
        };
        let fit = "SELECT 'fit'::FIT;\n";
        // before the first ::TAB of a tabbed dashboard
        let d = fit_codes(&tabs(fit, "", ""));
        assert!(has(&d, "layout/fit-ambiguous"), "{d:?}");
        assert_eq!(d[0].severity, Severity::Warning);
        assert_eq!(d[0].stmt, 1);
        // one per tab is fine
        assert!(fit_codes(&tabs("", fit, fit)).is_empty());
        // two in one tab
        let twice = format!("{fit}{fit}");
        let d = fit_codes(&tabs("", &twice, ""));
        assert!(has(&d, "layout/fit-ambiguous"), "{d:?}");
        assert_eq!(d[0].stmt, 3, "the second one");
        // a tab's ::FIT already covers its sub-tabs
        let nested = format!(
            "SELECT 'A'::TAB;\n{fit}SELECT 'S1'::SUBTAB;\n{fit}{FIT_CHART}SELECT 'S2'::SUBTAB;\n{FIT_CHART}"
        );
        assert!(has(&fit_codes(&nested), "layout/fit-ambiguous"));
        // sibling sub-tabs each with their own is fine
        let siblings = format!(
            "SELECT 'A'::TAB;\nSELECT 'S1'::SUBTAB;\n{fit}{FIT_CHART}SELECT 'S2'::SUBTAB;\n{fit}{FIT_CHART}"
        );
        assert!(
            fit_codes(&siblings).is_empty(),
            "{:?}",
            fit_codes(&siblings)
        );
        // no tabs: anywhere is the whole dashboard, unambiguous
        assert!(fit_codes(&format!("{FIT_CHART}{fit}{FIT_CHART}")).is_empty());
        // cast beside a panel's own columns
        let d = fit_codes("SELECT 1 AS x ::XAXIS, 2 AS y ::BARCHART, 'fit'::FIT;");
        assert!(has(&d, "layout/fit-ambiguous"), "{d:?}");
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

    /// #291: `::ROWSELECT` / `::TOTALS` / `::EMPTY` (and the rest of the
    /// `[layout]` "next panel" directives) are their own preceding
    /// statement, not a column on the panel they modify.
    #[test]
    fn next_panel_layout_directives_embedded_in_their_own_panel_are_flagged() {
        // (A) from #291: ::ROWSELECT and ::TOTALS embedded as extra columns
        // on the table they were meant to modify — silently dropped by the
        // ::TABLE rewrite (shown as literal extra columns), not read as
        // roles at all.
        let d = lint_one(
            "SELECT id AS id ::TABLE, name AS \"Name\", 'single'::ROWSELECT, 'none'::TOTALS FROM t;",
            rows(2, |i| json!({ "id": i, "Name": "a", "c2": "single", "c3": "none" })),
        );
        let next_panel: Vec<_> = d.iter().filter(|x| x.code == "layout/next-panel").collect();
        assert_eq!(next_panel.len(), 2, "{d:?}");
        assert!(
            next_panel.iter().any(|x| x.message.contains("::ROWSELECT"))
                && next_panel.iter().any(|x| x.message.contains("::TOTALS")),
            "{d:?}"
        );

        // (B) the correct form: each directive is its own statement before
        // the table — no warning.
        let d = lint_one(
            "SELECT 'single'::ROWSELECT;\nSELECT 'none'::TOTALS;\nSELECT id AS id ::TABLE, name AS \"Name\" FROM t;",
            rows(2, |i| json!({ "id": i, "Name": "a" })),
        );
        assert!(!has(&d, "layout/next-panel"), "{d:?}");

        // (#354) ::TOOLBAR embedded as an extra column on the table it was
        // meant to modify is flagged the same way as ::ROWSELECT / ::TOTALS
        // above; as its own preceding statement, it is not.
        let d = lint_one(
            "SELECT id AS id ::TABLE, name AS \"Name\", 'none'::TOOLBAR FROM t;",
            rows(2, |i| json!({ "id": i, "Name": "a", "c2": "none" })),
        );
        assert!(
            d.iter()
                .any(|x| x.code == "layout/next-panel" && x.message.contains("::TOOLBAR")),
            "{d:?}"
        );
        let d = lint_one(
            "SELECT 'none'::TOOLBAR;\nSELECT id AS id ::TABLE, name AS \"Name\" FROM t;",
            rows(2, |i| json!({ "id": i, "Name": "a" })),
        );
        assert!(!has(&d, "layout/next-panel"), "{d:?}");

        // ::EMPTY embedded on a chart's own SELECT, beside its measure —
        // kept as a role (unlike the ::TABLE case above) but applied to the
        // wrong panel.
        let d = lint_one(
            "SELECT day::XAXIS, n::LINECHART, 'No data'::EMPTY FROM t;",
            rows(2, |i| json!({ "day": i, "n": i, "msg": "No data" })),
        );
        assert!(has(&d, "layout/next-panel"), "{d:?}");

        // A lone ::EMPTY (its own statement, the documented form) never
        // warns — covered already by `optional_inputs_are_notes_and_empty_messages_silence_empty_panels`.
        let d = lint_one("SELECT 'No data'::EMPTY;", rows(0, |_| json!({})));
        assert!(!has(&d, "layout/next-panel"), "{d:?}");
    }

    /// Lint `'<spec>'::TOTALS` before a table (district, Per 10k, people, on_time ::PERCENT).
    fn lint_totals(spec: &str, tables: usize) -> Vec<Diagnostic> {
        let table = rows(3, |i| {
            json!({ "district": format!("D{i}"), "Per 10k": 0.1 * i as f64 + 0.05,
                    "people": 1000 + i, "on_time": 0.8 + 0.01 * i as f64 })
        });
        let t = "SELECT district::TABLE, \"Per 10k\", people, on_time::PERCENT FROM t;\n";
        let sql = format!("SELECT '{spec}'::TOTALS;\n{}", t.repeat(tables));
        let spec = spec.to_string();
        check(&sql, move |q| {
            Ok(if q.contains("FROM t") {
                table.clone()
            } else {
                rows(1, |_| json!({ "c0": spec }))
            })
        })
    }

    #[test]
    fn totals_that_sum_a_rate_are_noted() {
        let n = |d: &[Diagnostic]| d.iter().filter(|x| x.code == "totals/summed-rate").count();
        let d = lint_totals("sum", 1);
        assert_eq!(n(&d), 1, "{d:?}");
        let m = &d.iter().find(|x| x.code == "totals/summed-rate").unwrap();
        assert!(
            m.severity == Severity::Note
                && m.message.contains("Per 10k")
                && m.message.contains("on_time"),
            "{m:?}"
        );
        // named columns are deliberate; no all-columns sum, no note
        for spec in [
            "Per 10k: sum; on_time: mean; sum",
            "mean",
            "none",
            "people: sum",
            "Per 10k: none; sum; on_time: none",
        ] {
            assert_eq!(n(&lint_totals(spec, 1)), 0, "{spec}");
        }
        // the value is for the next table only
        assert_eq!(n(&lint_totals("sum", 2)), 1);
        // no ::TOTALS: the defaults never sum a rate
        let plain = rows(
            3,
            |i| json!({ "district": format!("D{i}"), "Per 10k": 0.1 * i as f64 }),
        );
        assert_eq!(
            n(&lint_one(
                "SELECT district::TABLE, \"Per 10k\" FROM t;",
                plain
            )),
            0
        );
    }

    #[test]
    fn totals_naming_no_column_are_noted() {
        let n = |d: &[Diagnostic]| {
            d.iter()
                .filter(|x| x.code == "totals/unknown-column")
                .count()
        };
        let d = lint_totals("peoples: sum; Per 10k: none", 1);
        assert_eq!(n(&d), 1, "{d:?}");
        let m = d
            .iter()
            .find(|x| x.code == "totals/unknown-column")
            .unwrap();
        assert!(
            m.severity == Severity::Note
                && m.message.contains("`peoples`")
                && !m.message.contains("`;`"),
            "{m:?}"
        );
        // raw names or headers, any case; bare values name nothing
        for spec in [
            "PEOPLE: sum; per 10K: none; On time: mean; district: count",
            "sum",
            "none",
            "Per 10k: none; sum",
        ] {
            assert_eq!(n(&lint_totals(spec, 1)), 0, "{spec}");
        }
        // the value is for the next table only
        assert_eq!(n(&lint_totals("peoples: sum", 2)), 1);
        // a header holding a comma, split by ',': the note says to use ';'
        let wide = rows(3, |i| {
            json!({ "district": format!("D{i}"), "Customers per 10,000 people": 0.1 * i as f64,
                    "people": 1000 + i })
        });
        let t = "SELECT district::TABLE, \"Customers per 10,000 people\", people FROM t;";
        let lint = |spec: &str| {
            let (wide, spec) = (wide.clone(), spec.to_string());
            check(&format!("SELECT '{spec}'::TOTALS;\n{t}\n"), move |q| {
                Ok(if q.contains("FROM t") {
                    wide.clone()
                } else {
                    rows(1, |_| json!({ "c0": spec }))
                })
            })
        };
        let d = lint("Customers per 10,000 people: none, people: sum");
        let m = d
            .iter()
            .find(|x| x.code == "totals/unknown-column")
            .expect("note");
        assert!(m.message.contains("`;`"), "{m:?}");
        assert_eq!(
            n(&lint("Customers per 10,000 people: none; people: sum")),
            0
        );
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

    /// #535: a `::WATERFALL`'s `::CATEGORY` tags "total" steps, not a colour
    /// group (`waterfall_steps`'s `is_total_tag`) — a `::COLORS` level map
    /// there has nothing to paint.
    #[test]
    fn colors_have_no_effect_on_a_waterfall_category_tag() {
        let data = rows(
            3,
            |i| json!({ "c0": format!("s{i}"), "c1": (i + 1) as f64, "c2": "", "c3": "A:bad; B:good" }),
        );
        let d = lint_one(
            "SELECT s::XAXIS, v::WATERFALL, tag::CATEGORY, 'A:bad; B:good'::COLORS FROM t;",
            data,
        );
        assert!(has(&d, "colors/no-effect"), "{d:?}");
    }

    /// #535: `cartesian` drops `::CATEGORY` entirely for a `::HEXBIN` scatter
    /// (`chart/hexbin-category`) and a `::HEXBIN` map (`map/hexbin-category`)
    /// — a `::COLORS` level map there is just as void, and `--check` should
    /// say so.
    #[test]
    fn colors_have_no_effect_on_a_hexbin_scatter_or_map() {
        let data = rows(3, |i| {
            json!({
                "c0": i as f64,
                "c1": i as f64,
                "c2": "auto",
                "c3": if i % 2 == 0 { "A" } else { "B" },
                "c4": "A:bad; B:good",
            })
        });
        let d = lint_one(
            "SELECT x::XAXIS, y::SCATTER, h::HEXBIN, cat::CATEGORY, \
             'A:bad; B:good'::COLORS FROM t;",
            data.clone(),
        );
        assert!(has(&d, "colors/no-effect"), "{d:?}");
        let data = rows(3, |i| {
            json!({
                "c0": i as f64,
                "c1": i as f64,
                "c2": "auto",
                "c3": if i % 2 == 0 { "A" } else { "B" },
                "c4": "A:bad; B:good",
            })
        });
        let d = lint_one(
            "SELECT lat::LAT, lon::LON, h::HEXBIN, cat::CATEGORY, \
             'A:bad; B:good'::COLORS FROM t;",
            data,
        );
        assert!(has(&d, "colors/no-effect"), "{d:?}");
    }

    #[test]
    fn heatmap_status_scales_are_checked() {
        let heat = |v: fn(usize) -> f64| {
            rows(
                3,
                move |i| json!({ "c0": i, "c1": "r", "c2": v(i), "c3": ">=50:bad; >=20:warn; good", "c4": "" }),
            )
        };
        let sql = "SELECT x::XAXIS, 'r'::YAXIS, v::HEATMAP, '>=50:bad; >=20:warn; good'::THRESHOLDS FROM t;";
        // Percent values against percent rules: quiet, even when all are good.
        let ok = lint_one(sql, heat(|i| i as f64));
        assert!(!has(&ok, "thresholds/one-band"), "{ok:?}");
        // Fractions against percent cuts: every tile alike.
        let d = lint_one(sql, heat(|i| i as f64 / 10.0));
        assert!(has(&d, "thresholds/one-band"), "{d:?}");
        // A colour scale beside the rules is ignored.
        let d = lint_one(
            "SELECT x::XAXIS, 'r'::YAXIS, v::HEATMAP, '>=50:bad; good'::THRESHOLDS, ''::COLORSCALE FROM t;",
            heat(|i| 10.0 * i as f64),
        );
        assert!(has(&d, "colorscale/with-thresholds"), "{d:?}");
        // On a bar chart a colour scale does nothing; on a heatmap it is fine.
        let data = rows(2, |i| json!({ "c0": i, "c1": i, "c2": "" }));
        let d = lint_one(
            "SELECT x::XAXIS, y::BARCHART, ''::COLORSCALE FROM t;",
            data.clone(),
        );
        assert!(has(&d, "colorscale/no-tiles"), "{d:?}");
        let d = lint_one(
            "SELECT x::XAXIS, 'r'::YAXIS, y::HEATMAP, ''::COLORSCALE_HIGH_GOOD FROM t;",
            rows(2, |i| json!({ "c0": i, "c1": "r", "c2": i, "c3": "" })),
        );
        assert!(!has(&d, "colorscale/no-tiles"), "{d:?}");
    }

    #[test]
    fn legend_value_and_no_legend_panels_are_checked() {
        // A categorised bar chart has a legend: 'none' is fine, a typo warns.
        let data = rows(2, |i| json!({ "c0": i, "c1": i, "c2": "a", "c3": "none" }));
        let ok = lint_one(
            "SELECT x::XAXIS, y::BARCHART, s::CATEGORY, 'none'::LEGEND FROM t;",
            data.clone(),
        );
        assert!(
            !has(&ok, "legend/bad-value") && !has(&ok, "legend/no-legend"),
            "{ok:?}"
        );
        let d = lint_one(
            "SELECT x::XAXIS, y::BARCHART, s::CATEGORY, 'sideways'::LEGEND FROM t;",
            rows(
                2,
                |i| json!({ "c0": i, "c1": i, "c2": "a", "c3": "sideways" }),
            ),
        );
        assert!(has(&d, "legend/bad-value"), "{d:?}");
        // A bar chart without a ::CATEGORY never draws a legend.
        let d = lint_one(
            "SELECT x::XAXIS, y::BARCHART, 'none'::LEGEND FROM t;",
            rows(2, |i| json!({ "c0": i, "c1": i, "c2": "none" })),
        );
        assert!(has(&d, "legend/no-legend"), "{d:?}");
        // A ::HEATMAP always has a colourbar.
        let ok = lint_one(
            "SELECT x::XAXIS, 'r'::YAXIS, v::HEATMAP, 'none'::LEGEND FROM t;",
            rows(2, |i| json!({ "c0": i, "c1": "r", "c2": i, "c3": "none" })),
        );
        assert!(!has(&ok, "legend/no-legend"), "{ok:?}");
        // A KPI never has one.
        let d = lint_one(
            "SELECT v::METRIC, 'none'::LEGEND FROM t;",
            rows(1, |_| json!({ "c0": 1, "c1": "none" })),
        );
        assert!(has(&d, "legend/no-legend"), "{d:?}");
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
        // #435: the priming `SET VARIABLE`s are sent in their own round
        // (eagerly, to the session) rather than bundled into the text of
        // whatever statement comes next — so look for them among every
        // call made, not specifically the last one.
        assert!(
            seen.iter()
                .any(|q| q.contains("SET VARIABLE week = []::VARCHAR[];")),
            "{seen:?}"
        );
        assert!(
            seen.iter()
                .any(|q| q.contains("SET VARIABLE week_from = NULL;")),
            "{seen:?}"
        );
        assert_eq!(
            sql::vars::select_as_prelude("region").unwrap(),
            "SET VARIABLE region = '';\nSET VARIABLE region_from = NULL;\nSET VARIABLE region_to = NULL;\n"
        );
        for bad in ["", "1x", "a b c", "region many", "x; DROP"] {
            assert!(sql::vars::select_as_prelude(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn drill_primes_its_variables_and_links_are_checked() {
        assert_eq!(
            sql::vars::drill_prelude("geo: region > country").unwrap(),
            "SET VARIABLE geo_level = 'region';\nSET VARIABLE geo_depth = 0;\n\
             SET VARIABLE geo_path = []::VARCHAR[];\nSET VARIABLE region = '';\n\
             SET VARIABLE country = '';\n"
        );
        assert!(sql::vars::drill_prelude("region > country > city")
            .unwrap()
            .starts_with("SET VARIABLE drill_level = 'region';"));
        for bad in ["region", "a > ", "a > b c", "x y: a > b", "a > b; DROP"] {
            assert!(sql::vars::drill_prelude(bad).is_err(), "{bad}");
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

    /// #330: two `::ROWSELECT` tables whose first column has the same name
    /// (so the browser derives the same `getvariable()` for both) share one
    /// selection — a click in either silently toggles the other too. Unlike
    /// `::SELECT_AS`'s explicit, chosen name (reuse allowed above), this
    /// name is never typed by the author, so it is always flagged.
    #[test]
    fn two_rowselect_tables_sharing_a_first_column_name_clash() {
        let d = check(
            "SELECT 'single'::ROWSELECT;\n\
             SELECT v AS picked ::TABLE, n AS \"N\" FROM a;\n\
             SELECT 'single'::ROWSELECT;\n\
             SELECT v AS picked ::TABLE, n AS \"N\" FROM b;",
            |q| {
                if q.contains("'single'") {
                    Ok(vec![json!({ "c0": "single" }).as_object().unwrap().clone()])
                } else {
                    Ok(rows(1, |_| json!({ "picked": "x1", "N": 1 })))
                }
            },
        );
        assert!(has(&d, "interaction/rowselect-shared-name"), "{d:?}");

        // Different first-column aliases: each table's variable is its own — clean.
        let ok = check(
            "SELECT 'single'::ROWSELECT;\n\
             SELECT v AS picked_a ::TABLE, n AS \"N\" FROM a;\n\
             SELECT 'single'::ROWSELECT;\n\
             SELECT v AS picked_b ::TABLE, n AS \"N\" FROM b;",
            |q| {
                if q.contains("'single'") {
                    Ok(vec![json!({ "c0": "single" }).as_object().unwrap().clone()])
                } else if q.contains("picked_a") {
                    Ok(rows(1, |_| json!({ "picked_a": "x1", "N": 1 })))
                } else {
                    Ok(rows(1, |_| json!({ "picked_b": "x1", "N": 1 })))
                }
            },
        );
        assert!(!has(&ok, "interaction/rowselect-shared-name"), "{ok:?}");
    }

    /// #398: a `-- @shared <name>` directly above a `::ROWSELECT` documents
    /// that it deliberately reuses `<name>` with another one — the issue's
    /// own repro (two tables fanning in to one `item` selection).
    #[test]
    fn shared_header_silences_the_rowselect_repro_from_398() {
        let build = |hdr: &str| {
            format!(
                "SELECT 'single'::ROWSELECT;\n\
                 SELECT item ::TABLE, id AS \"ID\" FROM items;\n\
                 {hdr}SELECT 'single'::ROWSELECT;\n\
                 SELECT item ::TABLE, id * 2 AS \"Doubled\" FROM items;"
            )
        };
        let run = |s: &str| {
            check(s, |q| {
                if q.contains("'single'") {
                    Ok(vec![json!({ "c0": "single" }).as_object().unwrap().clone()])
                } else {
                    Ok(rows(1, |_| json!({ "item": "a", "ID": 1, "Doubled": 2 })))
                }
            })
        };

        // Without the header: flagged, same as #330.
        let bare = run(&build(""));
        assert!(has(&bare, "interaction/rowselect-shared-name"), "{bare:?}");

        // With `-- @shared item` above the second ::ROWSELECT: silenced, and
        // `--check --strict` passes clean.
        let ok = run(&build("-- @shared item\n"));
        assert!(!has(&ok, "interaction/rowselect-shared-name"), "{ok:?}");
        assert!(passes(&ok, true), "{ok:?}");
    }

    /// #398: three `::ROWSELECT` tables fanning in to one variable (the
    /// supplier-risk `material` case from the issue) — every clash past the
    /// first owner needs its own `-- @shared`, since each is reported
    /// against the first table, never against each other.
    #[test]
    fn shared_header_silences_a_three_way_rowselect_fan_in() {
        let build = |hdr_b: &str, hdr_c: &str| {
            format!(
                "SELECT 'single'::ROWSELECT;\n\
                 SELECT item ::TABLE, id AS \"A\" FROM a;\n\
                 {hdr_b}SELECT 'single'::ROWSELECT;\n\
                 SELECT item ::TABLE, id AS \"B\" FROM b;\n\
                 {hdr_c}SELECT 'single'::ROWSELECT;\n\
                 SELECT item ::TABLE, id AS \"C\" FROM c;"
            )
        };
        let run = |s: &str| {
            check(s, |q| {
                if q.contains("'single'") {
                    Ok(vec![json!({ "c0": "single" }).as_object().unwrap().clone()])
                } else {
                    Ok(rows(1, |_| json!({ "item": "a", "x": 1 })))
                }
            })
        };

        let bare = run(&build("", ""));
        assert_eq!(
            bare.iter()
                .filter(|d| d.code == "interaction/rowselect-shared-name")
                .count(),
            2,
            "{bare:?}"
        );

        let ok = run(&build("-- @shared item\n", "-- @shared item\n"));
        assert!(!has(&ok, "interaction/rowselect-shared-name"), "{ok:?}");
        assert!(passes(&ok, true), "{ok:?}");
    }

    /// #398: a dropdown input and a `::ROWSELECT` table meant to hand a
    /// selection to each other either way (the supplier-risk `product` case).
    #[test]
    fn shared_header_silences_a_dropdown_rowselect_handoff() {
        let answer = |q: &str| {
            if q.contains("DISTINCT") {
                rows(1, |_| json!({ "product": "A" }))
            } else if q.contains("'single'") {
                vec![json!({ "c0": "single" }).as_object().unwrap().clone()]
            } else {
                rows(1, |_| json!({ "product": "A", "ID": 1 }))
            }
        };
        let build = |hdr: &str| {
            format!(
                "SELECT DISTINCT product ::DROPDOWN FROM t;\n\
                 {hdr}SELECT 'single'::ROWSELECT;\n\
                 SELECT product ::TABLE, id AS \"ID\" FROM items;"
            )
        };

        let bare = check(&build(""), |q| Ok(answer(q)));
        assert!(has(&bare, "interaction/name-clash"), "{bare:?}");

        let ok = check(&build("-- @shared product\n"), |q| Ok(answer(q)));
        assert!(!has(&ok, "interaction/name-clash"), "{ok:?}");
        assert!(passes(&ok, true), "{ok:?}");
    }

    /// #398: a `-- @shared` name this directive doesn't itself set (a typo)
    /// is flagged on its own, instead of silently disabling nothing.
    #[test]
    fn shared_header_with_an_unknown_name_warns_itself() {
        let d = check(
            "-- @shared itme\n\
             SELECT 'single'::ROWSELECT;\n\
             SELECT item ::TABLE, id AS \"ID\" FROM a;",
            |q| {
                if q.contains("'single'") {
                    Ok(vec![json!({ "c0": "single" }).as_object().unwrap().clone()])
                } else {
                    Ok(rows(1, |_| json!({ "item": "x1", "ID": 1 })))
                }
            },
        );
        assert!(has(&d, "interaction/shared-unused"), "{d:?}");
        // The real name is unaffected — no clash to report here anyway since
        // there's only one claimant, but the typo'd name itself is flagged.
        assert!(!has(&d, "interaction/rowselect-shared-name"), "{d:?}");
    }

    /// #504: `-- @requires` gates the table in front of which a
    /// `::ROWSELECT` (with its own `-- @shared`) sits, but not the
    /// `::ROWSELECT` itself — the directive's pending state must not then
    /// leak onto the *next* (unrelated) table and get checked against it.
    /// The issue's own repro: a gated `series` table with `-- @shared series`
    /// above its `::ROWSELECT`, followed by a plain `Method` table.
    #[test]
    fn requires_gating_a_table_also_drops_its_rowselects_pending_shared_check() {
        let d = check(
            "-- @shared series\n\
             SELECT 'single'::ROWSELECT;\n\
             -- @requires missing.parquet\n\
             SELECT unique_id AS series ::TABLE FROM read_parquet('missing.parquet');\n\
             \n\
             SELECT method AS \"Method\" ::TABLE, 1 AS \"Series\" FROM (VALUES ('a'), ('b')) t(method);",
            |q| {
                if q.contains("missing.parquet") {
                    // glob(): no files match → the table is gated out.
                    Ok(vec![json!({ "n": "0" }).as_object().unwrap().clone()])
                } else if q.contains("'single'") {
                    Ok(vec![json!({ "c0": "single" }).as_object().unwrap().clone()])
                } else {
                    let methods = ["a", "b"];
                    Ok(rows(2, |i| {
                        json!({ "Method": methods[i], "Series": 1 })
                    }))
                }
            },
        );
        // The gated statement: a note, not an error.
        assert!(has(&d, "optional/missing"), "{d:?}");
        // Before the fix, the ::ROWSELECT's pending `-- @shared series`
        // carried over and was checked against the Method table (which sets
        // Method, not series) — a false shared-unused. Gated out, it must
        // not be checked against anything else either.
        assert!(!has(&d, "interaction/shared-unused"), "{d:?}");
        assert!(!has(&d, "interaction/rowselect-shared-name"), "{d:?}");
        assert!(passes(&d, true), "{d:?}");
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

    /// #405: `::KPI_SIZE`'s own value and placement.
    #[test]
    fn kpi_size_checks_value_and_placement() {
        // an unknown value warns but still lints clean otherwise
        let d = lint_one(
            "SELECT 5::METRIC, 'dense'::KPI_SIZE",
            vec![json!({"c0": 5.0, "c1": "dense"})
                .as_object()
                .unwrap()
                .clone()],
        );
        assert!(has(&d, "kpi/bad-kpi-size"), "{d:?}");
        // 'compact' / 'normal' on a ::METRIC: no warning
        for v in ["compact", "normal"] {
            let d = lint_one(
                "SELECT 5::METRIC, 'v'::KPI_SIZE",
                vec![json!({"c0": 5.0, "c1": v}).as_object().unwrap().clone()],
            );
            assert!(!d.iter().any(|x| x.code.starts_with("kpi/")), "{v}: {d:?}");
        }
        // on a ::GROUP box: no warning either
        let d = lint_one(
            "SELECT 'Status'::GROUP, 'compact'::KPI_SIZE",
            vec![json!({"c0": "Status", "c1": "compact"})
                .as_object()
                .unwrap()
                .clone()],
        );
        assert!(!d.iter().any(|x| x.code.starts_with("kpi/")), "{d:?}");
        // misplaced on a chart: a warning
        let d = lint_one(
            "SELECT x::XAXIS, y::BARCHART, 'compact'::KPI_SIZE FROM t",
            rows(
                3,
                |i| json!({"c0": i as f64, "c1": i as f64, "c2": "compact"}),
            ),
        );
        assert!(has(&d, "kpi/kpi-size-misplaced"), "{d:?}");
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
    fn pie_group_by_without_order_by_hints_quietly() {
        let data = rows(4, |i| json!({ "c0": format!("s{i}"), "c1": i as f64 }));
        let d = lint_one(
            "SELECT cat::CATEGORY, sum(n)::DONUTCHART, 'Share'::TITLE FROM t GROUP BY 1;",
            data.clone(),
        );
        assert!(has(&d, "design/pie-no-order"), "{d:?}");
        assert_eq!(
            d.iter()
                .find(|x| x.code == "design/pie-no-order")
                .unwrap()
                .severity,
            Severity::Note,
            "a hint, not a warning that fails a plain check"
        );
        // ordered — no hint.
        let d = lint_one(
            "SELECT cat::CATEGORY, sum(n)::DONUTCHART, 'Share'::TITLE FROM t GROUP BY 1 ORDER BY 2 DESC;",
            data.clone(),
        );
        assert!(!has(&d, "design/pie-no-order"), "{d:?}");
        // a bar chart's GROUP BY without ORDER BY is a different concern
        // (design/unsorted-bars, data-driven) — this hint stays pie/donut-only.
        let d = lint_one(
            "SELECT cat::XAXIS, sum(n)::BARCHART, 'Share'::TITLE FROM t GROUP BY 1;",
            data,
        );
        assert!(!has(&d, "design/pie-no-order"), "{d:?}");
    }

    #[test]
    fn ybreaks_non_numeric_entry_warns() {
        let bad = rows(
            3,
            |i| json!({ "c0": i as f64, "c1": i as f64 * 2.0, "c2": "100,oops,1000" }),
        );
        let d = lint_one(
            "SELECT x::XAXIS, y::LINECHART, '100,oops,1000'::YBREAKS FROM t;",
            bad,
        );
        assert!(has(&d, "breaks/non-numeric"), "{d:?}");
        // A clean numeric list, same cartesian chart, is fine.
        let ok = rows(
            3,
            |i| json!({ "c0": i as f64, "c1": i as f64 * 2.0, "c2": "100,1000,10000" }),
        );
        let d = lint_one(
            "SELECT x::XAXIS, y::LINECHART, '100,1000,10000'::YBREAKS FROM t;",
            ok,
        );
        assert!(!has(&d, "breaks/non-numeric"), "{d:?}");
        assert!(!has(&d, "breaks/non-cartesian"), "{d:?}");
    }

    #[test]
    fn breaks_on_a_non_cartesian_panel_warns() {
        // A pie has no x/y axis to place ticks on.
        let data = rows(
            4,
            |i| json!({ "c0": format!("s{i}"), "c1": i as f64, "c2": "100,200" }),
        );
        let d = lint_one(
            "SELECT cat::CATEGORY, sum(n)::PIE, '100,200'::YBREAKS FROM t GROUP BY 1;",
            data,
        );
        assert!(has(&d, "breaks/non-cartesian"), "{d:?}");
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

    /// #639: `-- @lint_allow design/many-series` turns that one advisory into
    /// a `lint/allowed` note (still listed, passes `--strict`); a non-design
    /// code or one the statement doesn't trigger is flagged.
    #[test]
    fn lint_allow_turns_a_design_advisory_into_a_note() {
        let data = || {
            rows(
                8,
                |i| json!({ "c0": "w", "c1": format!("series{i}"), "c2": i as f64 }),
            )
        };
        let sql = "SELECT wk::XAXIS, ch::CATEGORY, n::BARCHART, 'T'::TITLE FROM t GROUP BY ALL;";
        assert!(!passes(&lint_one(sql, data()), true));
        let d = lint_one(
            &format!("-- @lint_allow design/many-series (8 on purpose)\n{sql}"),
            data(),
        );
        assert!(!has(&d, "design/many-series"), "{d:?}");
        let allowed = d.iter().find(|x| x.code == "lint/allowed").expect("listed");
        assert_eq!(allowed.severity, Severity::Note);
        assert!(
            allowed.message.starts_with("design/many-series allowed"),
            "{d:?}"
        );
        assert!(passes(&d, true), "{d:?}");
        assert!(!has(&d, "lint/allow-unused"), "{d:?}");

        // Not a design lint, and a design lint this statement doesn't trigger.
        let d = lint_one(
            &format!("-- @lint_allow sql-error, design/pie-slices\n{sql}"),
            data(),
        );
        assert!(has(&d, "design/many-series"), "{d:?}");
        assert!(has(&d, "lint/allow-not-design"), "{d:?}");
        assert!(has(&d, "lint/allow-unused"), "{d:?}");

        // Only the statement right below: a second chart still warns.
        let d = lint_one(
            &format!("-- @lint_allow design/many-series\n{sql}\n{sql}"),
            data(),
        );
        assert_eq!(d.iter().filter(|x| x.code == "lint/allowed").count(), 1);
        assert!(has(&d, "design/many-series"), "{d:?}");

        // With --no-design there is nothing to allow: no unused warning.
        let d = check_opts(
            &format!("-- @lint_allow design/many-series\n{sql}"),
            |_q| Ok(data()),
            LintOptions {
                design: false,
                ..LintOptions::default()
            },
        );
        assert!(!has(&d, "lint/allow-unused"), "{d:?}");
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

    /// #214: `design/too-many-panels` counts only a ::VIEW group's shown
    /// (first) alternative — a switcher only ever shows one at a time.
    #[test]
    fn too_many_panels_counts_only_the_shown_alternative() {
        let panel = "SELECT a::XAXIS, b::BARCHART, 'T'::TITLE FROM t;\n";
        let five = panel.repeat(5); // under the threshold alone; 2x would not be
        let script = format!("SELECT 'A'::VIEW;\n{five}SELECT 'B'::VIEW;\n{five}");
        let data = rows(
            3,
            |i| json!({ "c0": format!("c{i}"), "c1": (3 - i) as f64 }),
        );
        let d = check(&script, |_q| Ok(data.clone()));
        assert!(!has(&d, "design/too-many-panels"), "{d:?}");
    }

    #[test]
    fn view_lone_warns() {
        let d = check(
            "SELECT 'Lines'::VIEW;\nSELECT a::XAXIS, b::BARCHART FROM t;",
            |_q| Ok(vec![]),
        );
        assert!(has(&d, "view/lone"), "{d:?}");
    }

    #[test]
    fn view_duplicate_label_errors() {
        let d = check(
            "SELECT 'Lines'::VIEW;\nSELECT a::XAXIS, b::BARCHART FROM t;\n\
             SELECT 'Lines'::VIEW;\nSELECT a::XAXIS, c::BARCHART FROM t;",
            |_q| Ok(vec![]),
        );
        assert!(has(&d, "view/duplicate-label"), "{d:?}");
    }

    #[test]
    fn view_empty_label_errors() {
        let d = check(
            "SELECT ''::VIEW;\nSELECT a::XAXIS, b::BARCHART FROM t;",
            |_q| Ok(vec![]),
        );
        assert!(has(&d, "view/empty-label"), "{d:?}");
        let d = check(
            "SELECT NULL::VIEW;\nSELECT a::XAXIS, b::BARCHART FROM t;",
            |_q| Ok(vec![]),
        );
        assert!(has(&d, "view/empty-label"), "{d:?}");
    }

    #[test]
    fn view_computed_label_warns() {
        let d = check(
            "SELECT a||'x'::VIEW FROM t;\nSELECT a::XAXIS, b::BARCHART FROM t;",
            |_q| Ok(vec![]),
        );
        assert!(has(&d, "view/computed-label"), "{d:?}");
    }

    #[test]
    fn view_mixed_names_warns() {
        let d = check(
            "SELECT 'Lines' AS yoy ::VIEW;\nSELECT a::XAXIS, b::BARCHART FROM t;\n\
             SELECT 'Bars' AS qoq ::VIEW;\nSELECT a::XAXIS, c::BARCHART FROM t;",
            |_q| Ok(vec![]),
        );
        assert!(has(&d, "view/mixed-names"), "{d:?}");
    }

    #[test]
    fn view_duplicate_name_errors() {
        let d = check(
            "SELECT 'Lines' AS g1 ::VIEW;\nSELECT a::XAXIS, b::BARCHART FROM t;\n\
             SELECT 1::ENDVIEW;\n\
             SELECT 'Bars' AS g1 ::VIEW;\nSELECT a::XAXIS, c::BARCHART FROM t;",
            |_q| Ok(vec![]),
        );
        assert!(has(&d, "view/duplicate-name"), "{d:?}");
    }

    #[test]
    fn view_bad_name_errors() {
        let d = check(
            "SELECT 'Lines' AS \"2x\" ::VIEW;\nSELECT a::XAXIS, b::BARCHART FROM t;",
            |_q| Ok(vec![]),
        );
        assert!(has(&d, "view/bad-name"), "{d:?}");
    }

    #[test]
    fn view_empty_alternative_warns() {
        let d = check(
            "SELECT 'Lines'::VIEW;\nSELECT 'Bars'::VIEW;\nSELECT a::XAXIS, b::BARCHART FROM t;",
            |_q| Ok(vec![]),
        );
        assert!(has(&d, "view/empty"), "{d:?}");
    }

    #[test]
    fn view_open_ended_warns() {
        let d = check(
            "SELECT 'Lines'::VIEW;\nSELECT a::XAXIS, b::BARCHART FROM t;\n\
             SELECT 'Bars'::VIEW;\nSELECT a::XAXIS, c::BARCHART FROM t;\n\
             SELECT a::XAXIS, d::LINECHART FROM t;",
            |_q| Ok(vec![]),
        );
        assert!(has(&d, "view/open-ended"), "{d:?}");
    }

    // #573: two alternatives of one group pending different explicit
    // ::HEIGHT values — the card still resizes when switching between them,
    // even once a shared ::HEIGHT propagates to every alternative.
    #[test]
    fn view_height_conflict_warns() {
        let d = check(
            "SELECT 112::HEIGHT;\nSELECT 'A'::VIEW;\nSELECT a::XAXIS, b::BARCHART FROM t;\n\
             SELECT 300::HEIGHT;\nSELECT 'B'::VIEW;\nSELECT a::XAXIS, c::BARCHART FROM t;",
            |_q| Ok(vec![]),
        );
        assert!(has(&d, "view/height-conflict"), "{d:?}");
    }

    // The common, now-fixed case: a ::HEIGHT pending before the group's
    // first marker, no alternative setting its own — every alternative
    // inherits it (web/app.js), nothing to flag.
    #[test]
    fn view_height_shared_before_group_does_not_conflict() {
        let d = check(
            "SELECT 112::HEIGHT;\nSELECT 'A'::VIEW;\nSELECT a::XAXIS, b::BARCHART FROM t;\n\
             SELECT 'B'::VIEW;\nSELECT a::XAXIS, c::BARCHART FROM t;",
            |_q| Ok(vec![]),
        );
        assert!(!has(&d, "view/height-conflict"), "{d:?}");
    }

    // Every alternative repeating the SAME explicit ::HEIGHT (today's
    // documented workaround) is consistent, not a conflict.
    #[test]
    fn view_height_matching_per_alternative_does_not_conflict() {
        let d = check(
            "SELECT 112::HEIGHT;\nSELECT 'A'::VIEW;\nSELECT a::XAXIS, b::BARCHART FROM t;\n\
             SELECT 112::HEIGHT;\nSELECT 'B'::VIEW;\nSELECT a::XAXIS, c::BARCHART FROM t;",
            |_q| Ok(vec![]),
        );
        assert!(!has(&d, "view/height-conflict"), "{d:?}");
    }

    #[test]
    fn view_unbalanced_group_errors() {
        let d = check(
            "SELECT 'Lines'::VIEW;\nSELECT 1::GROUP;\nSELECT 1::ENDVIEW;",
            |_q| Ok(vec![]),
        );
        assert!(has(&d, "view/unbalanced-group"), "{d:?}");
    }

    #[test]
    fn view_stray_end_warns() {
        let d = check(
            "SELECT a::XAXIS, b::BARCHART FROM t;\nSELECT 1::ENDVIEW;",
            |_q| Ok(vec![]),
        );
        assert!(has(&d, "view/stray-end"), "{d:?}");
    }

    #[test]
    fn view_input_inside_notes() {
        let d = check(
            "SELECT 'Lines'::VIEW;\nSELECT DISTINCT zone::DROPDOWN FROM t;",
            |_q| Ok(vec![]),
        );
        assert!(has(&d, "view/input-inside"), "{d:?}");
    }

    #[test]
    fn view_unnamed_notes_with_multiple_groups() {
        let d = check(
            "SELECT 'Lines'::VIEW;\nSELECT a::XAXIS, b::BARCHART FROM t;\n\
             SELECT 1::ENDVIEW;\n\
             SELECT 'Bars' AS g2 ::VIEW;\nSELECT a::XAXIS, c::BARCHART FROM t;",
            |_q| Ok(vec![]),
        );
        assert!(has(&d, "view/unnamed"), "{d:?}");
        // the single-group case is not flagged — nothing else to tell it apart from
        let d2 = check(
            "SELECT 'Lines'::VIEW;\nSELECT a::XAXIS, b::BARCHART FROM t;\n\
             SELECT 'Bars'::VIEW;\nSELECT a::XAXIS, c::BARCHART FROM t;",
            |_q| Ok(vec![]),
        );
        assert!(!has(&d2, "view/unnamed"), "{d2:?}");
    }

    #[test]
    fn view_placement_header_is_clean_when_every_alt_is_one_panel() {
        let d = check(
            "SELECT 'header'::VIEW_PLACEMENT;\n\
             SELECT 'Lines'::VIEW;\nSELECT a::XAXIS, b::BARCHART FROM t;\n\
             SELECT 'Bars'::VIEW;\nSELECT a::XAXIS, c::BARCHART FROM t;",
            |_q| Ok(vec![]),
        );
        assert!(!has(&d, "view-placement/not-single-panel"), "{d:?}");
        assert!(!has(&d, "view-placement/unknown-value"), "{d:?}");
        assert!(!has(&d, "view-placement/orphan"), "{d:?}");
    }

    #[test]
    fn view_placement_not_single_panel_warns_and_still_renders() {
        let d = check(
            "SELECT 'header'::VIEW_PLACEMENT;\n\
             SELECT 'Lines'::VIEW;\n\
             SELECT a::XAXIS, b::BARCHART FROM t;\nSELECT a::XAXIS, c::LINECHART FROM t;\n\
             SELECT 'Bars'::VIEW;\nSELECT a::XAXIS, c::BARCHART FROM t;",
            |_q| Ok(vec![]),
        );
        assert!(has(&d, "view-placement/not-single-panel"), "{d:?}");
    }

    #[test]
    fn view_placement_unknown_value_errors() {
        let d = check(
            "SELECT 'sidebar'::VIEW_PLACEMENT;\n\
             SELECT 'Lines'::VIEW;\nSELECT a::XAXIS, b::BARCHART FROM t;\n\
             SELECT 'Bars'::VIEW;\nSELECT a::XAXIS, c::BARCHART FROM t;",
            |_q| Ok(vec![]),
        );
        assert!(has(&d, "view-placement/unknown-value"), "{d:?}");
    }

    #[test]
    fn view_placement_computed_value_warns() {
        let d = check(
            "SELECT lower('HEADER')::VIEW_PLACEMENT;\n\
             SELECT 'Lines'::VIEW;\nSELECT a::XAXIS, b::BARCHART FROM t;\n\
             SELECT 'Bars'::VIEW;\nSELECT a::XAXIS, c::BARCHART FROM t;",
            |_q| Ok(vec![]),
        );
        assert!(has(&d, "view-placement/computed-value"), "{d:?}");
    }

    #[test]
    fn view_placement_ambiguous_warns() {
        let d = check(
            "SELECT 'header'::VIEW_PLACEMENT, a::XAXIS FROM t;\n\
             SELECT 'Lines'::VIEW;\nSELECT a::XAXIS, b::BARCHART FROM t;\n\
             SELECT 'Bars'::VIEW;\nSELECT a::XAXIS, c::BARCHART FROM t;",
            |_q| Ok(vec![]),
        );
        assert!(has(&d, "view-placement/ambiguous"), "{d:?}");
    }

    #[test]
    fn view_placement_orphan_warns() {
        let d = check(
            "SELECT 'header'::VIEW_PLACEMENT;\nSELECT a::XAXIS, b::BARCHART FROM t;",
            |_q| Ok(vec![]),
        );
        assert!(has(&d, "view-placement/orphan"), "{d:?}");
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
            LintOptions {
                design: false,
                ..Default::default()
            },
        );
        let w: Vec<_> = d.iter().filter(|x| x.code == "render-warning").collect();
        assert_eq!(w.len(), 1, "{d:?}");
        assert_eq!(w[0].severity, Severity::Warning);
        assert!(w[0].message.contains("stat_density"), "{}", w[0].message);
    }

    #[test]
    fn map_north_arrow_value_is_checked() {
        let codes = |v: &str| -> Vec<&'static str> {
            let row = json!({ "c0": "POLYGON((0 0, 1 0, 1 1, 0 1, 0 0))", "c1": v });
            check_opts(
                "SELECT g ::MAP, n ::NORTH_ARROW FROM t;",
                |_q| Ok(vec![row.as_object().unwrap().clone()]),
                LintOptions {
                    design: false,
                    ..Default::default()
                },
            )
            .into_iter()
            .map(|d| d.code)
            .filter(|c| c.starts_with("map/north"))
            .collect()
        };
        assert!(codes("print").is_empty());
        assert!(codes("Always").is_empty());
        assert_eq!(codes("yes"), vec!["map/north-arrow-value"]);
    }

    #[test]
    fn map_facets_are_checked() {
        let codes = |sql: &str, rows: Vec<Value>| -> Vec<&'static str> {
            let data: Vec<_> = rows
                .iter()
                .map(|r| r.as_object().unwrap().clone())
                .collect();
            check_opts(
                sql,
                |_q| Ok(data.clone()),
                LintOptions {
                    design: false,
                    ..Default::default()
                },
            )
            .into_iter()
            .map(|d| d.code)
            .filter(|c| {
                c.starts_with("map/facet")
                    || c.starts_with("map/too-many")
                    || c.starts_with("facet/")
            })
            .collect()
        };
        let row = |w: &str| json!({ "c0": "DE-BY", "c1": w, "c2": 1.0 });
        let sql = "SELECT r ::REGION, w ::FACET, v ::BARCHART FROM t;";
        assert!(codes(sql, vec![row("1"), row("2")]).is_empty());
        let many: Vec<Value> = (0..13).map(|i| row(&i.to_string())).collect();
        assert_eq!(codes(sql, many), vec!["map/too-many-facets"]);
        assert_eq!(
            codes(
                "SELECT r ::REGION, w ::FACET, f ::FRAME FROM t;",
                vec![row("1")]
            ),
            vec!["map/facet-frame"]
        );
        assert_eq!(
            codes(
                "SELECT r ::REGION, w ::FACET_FREE, v ::BARCHART FROM t;",
                vec![row("1")]
            ),
            vec!["map/facet-free"]
        );
        // a map without a measure is still a facet-able panel (no facet/unsupported-kind)
        assert!(codes(
            "SELECT r ::REGION, w ::FACET FROM t;",
            vec![json!({"c0":"DE-BY","c1":"1"})]
        )
        .is_empty());
        // M3a-2 (#645): flow, hexbin and bivariate maps facet too
        let hex = json!({ "c0": 50.0, "c1": 10.0, "c2": "auto", "c3": "a" });
        assert!(codes(
            "SELECT lat ::LAT, lon ::LON, 'auto'::HEXBIN, f ::FACET FROM t;",
            vec![hex]
        )
        .is_empty());
        let biv = json!({ "c0": "DE-BY", "c1": 1.0, "c2": 2.0, "c3": "a" });
        assert!(codes(
            "SELECT r ::REGION, a ::BARCHART, b ::BIVARIATE, f ::FACET FROM t;",
            vec![biv]
        )
        .is_empty());
    }

    #[test]
    fn map_sparkline_is_checked() {
        let codes = |sql: &str, rows: Vec<Value>| -> Vec<(Severity, &'static str)> {
            let data: Vec<_> = rows
                .iter()
                .map(|r| r.as_object().unwrap().clone())
                .collect();
            check_opts(
                sql,
                |_q| Ok(data.clone()),
                LintOptions {
                    design: false,
                    ..Default::default()
                },
            )
            .into_iter()
            .filter(|d| d.code.starts_with("map/sparkline"))
            .map(|d| (d.severity, d.code))
            .collect()
        };
        let poly = "POLYGON((0 0, 1 0, 1 1, 0 1, 0 0))";
        let sql = "SELECT g::MAP, v::BARCHART, w::SPARKLINE FROM t;";
        let row = |w: Value| json!({ "c0": poly, "c1": 1.0, "c2": w });
        assert!(codes(sql, vec![row(json!([1, 2, 3]))]).is_empty());
        assert!(codes(sql, vec![row(json!("[1, 2, 3]"))]).is_empty());
        assert_eq!(
            codes(sql, vec![row(json!(7))]),
            vec![(Severity::Warning, "map/sparkline-not-list")]
        );
        let sql = "SELECT g::MAP, w::SPARKBAR, true::HEXBIN FROM t;";
        let row = json!({ "c0": "POINT(1 1)", "c1": [1, 2], "c2": true });
        assert_eq!(
            codes(sql, vec![row]),
            vec![(Severity::Note, "map/sparkline-hexbin")]
        );
    }

    #[test]
    fn map_modifiers_are_checked() {
        let codes = |sql: &str, rows: Vec<Value>| -> Vec<(Severity, &'static str)> {
            let data: Vec<_> = rows
                .iter()
                .map(|r| r.as_object().unwrap().clone())
                .collect();
            check_opts(
                sql,
                |_q| Ok(data.clone()),
                LintOptions {
                    design: false,
                    ..Default::default()
                },
            )
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

    /// M2a fix: `map_diags` used to return early (no WKT) before checking
    /// `::PROJECTION`/`::CLASSES`/too-many-marks on `::REGION` or
    /// `::LAT`/`::LON` maps.
    #[test]
    fn map_modifiers_are_checked_on_region_and_point_maps() {
        let codes = |sql: &str, rows: Vec<Value>| -> Vec<(Severity, &'static str)> {
            let data: Vec<_> = rows
                .iter()
                .map(|r| r.as_object().unwrap().clone())
                .collect();
            check_opts(
                sql,
                |_q| Ok(data.clone()),
                LintOptions {
                    design: false,
                    ..Default::default()
                },
            )
            .into_iter()
            .filter(|d| d.code.starts_with("map/"))
            .map(|d| (d.severity, d.code))
            .collect()
        };
        let sql = "SELECT c::REGION, v::BARCHART, 'robinson'::PROJECTION FROM t;";
        assert_eq!(
            codes(
                sql,
                vec![json!({ "c0": "DE-BY", "c1": 1.0, "c2": "robinson" })]
            ),
            vec![(Severity::Warning, "map/unknown-projection")]
        );
        let pts: Vec<Value> = (0..20_001)
            .map(|i| json!({ "c0": (i % 80) as f64, "c1": 10.0 }))
            .collect();
        assert_eq!(
            codes("SELECT lat::LAT, lon::LON FROM t;", pts),
            vec![(Severity::Warning, "map/too-many-marks")]
        );
    }

    /// #149 M2e-A: `::FRAME` time maps — too many frames, a duplicate
    /// (feature, frame) row, `::STORY`/`::XAXIS` ignored, and `::FRAME` off
    /// a map entirely.
    #[test]
    fn frame_lints_cover_too_many_frames_duplicates_and_misuse() {
        let codes = |sql: &str, rows: Vec<Value>| -> Vec<(Severity, &'static str)> {
            let data: Vec<_> = rows
                .iter()
                .map(|r| r.as_object().unwrap().clone())
                .collect();
            check_opts(
                sql,
                |_q| Ok(data.clone()),
                LintOptions {
                    design: false,
                    ..Default::default()
                },
            )
            .into_iter()
            .filter(|d| d.code.contains("frame"))
            .map(|d| (d.severity, d.code))
            .collect()
        };
        let sql = "SELECT c::REGION, w::FRAME, v::BARCHART FROM t;";
        let row = |c: &str, w: &str, v: f64| json!({ "c0": c, "c1": w, "c2": v });
        assert_eq!(
            codes(sql, vec![row("DE-BY", "1", 1.0), row("DE-BY", "1", 2.0)]),
            vec![(Severity::Warning, "map/frame-duplicate")]
        );
        assert_eq!(
            codes(
                "SELECT w::FRAME, v::BARCHART FROM t;",
                vec![json!({ "c0": "1", "c1": 1.0 })]
            ),
            vec![(Severity::Warning, "frame/not-a-map")]
        );
        // #663: a bar chart with an ::XAXIS plays its frames; a pie doesn't
        assert!(codes(
            "SELECT r::XAXIS, w::FRAME, v::BARCHART FROM t;",
            vec![json!({ "c0": "N", "c1": "1", "c2": 1.0 })]
        )
        .is_empty());
        assert_eq!(
            codes(
                "SELECT r::XAXIS, w::FRAME, v::PIE FROM t;",
                vec![json!({ "c0": "N", "c1": "1", "c2": 1.0 })]
            ),
            vec![(Severity::Warning, "frame/not-a-map")]
        );
        assert_eq!(
            codes(
                "SELECT c::REGION, w::FRAME, v::BARCHART, l::STORY FROM t;",
                vec![json!({ "c0": "DE-BY", "c1": "1", "c2": 1.0, "c3": "step" })]
            ),
            vec![(Severity::Warning, "map/frame-story")]
        );
        // #664: ::SYNC_FRAME needs a ::FRAME; with one it's quiet
        assert_eq!(
            codes(
                "SELECT c::REGION, v::BARCHART, 'weeks'::SYNC_FRAME FROM t;",
                vec![json!({ "c0": "DE-BY", "c1": 1.0, "c2": "weeks" })]
            ),
            vec![(Severity::Warning, "map/sync-frame-no-frame")]
        );
        assert!(codes(
            "SELECT c::REGION, w::FRAME, v::BARCHART, 'weeks'::SYNC_FRAME FROM t;",
            vec![json!({ "c0": "DE-BY", "c1": "1", "c2": 1.0, "c3": "weeks" })]
        )
        .is_empty());
        assert_eq!(
            codes(
                "SELECT c::REGION, w::FRAME, v::BARCHART, x::XAXIS FROM t;",
                vec![json!({ "c0": "DE-BY", "c1": "1", "c2": 1.0, "c3": "x" })]
            ),
            vec![(Severity::Note, "map/frame-xaxis")]
        );
        let many: Vec<Value> = (0..201)
            .map(|i| row("DE-BY", &i.to_string(), 1.0))
            .collect();
        assert_eq!(
            codes(sql, many),
            vec![(Severity::Warning, "map/too-many-frames")]
        );
    }

    #[test]
    fn size_not_positive_is_flagged() {
        let codes = |rows: Vec<Value>| -> Vec<(Severity, &'static str)> {
            let data: Vec<_> = rows
                .iter()
                .map(|r| r.as_object().unwrap().clone())
                .collect();
            check_opts(
                "SELECT lat::LAT, lon::LON, n::SIZE FROM t;",
                |_q| Ok(data.clone()),
                LintOptions {
                    design: false,
                    ..Default::default()
                },
            )
            .into_iter()
            .filter(|d| d.code == "map/size-not-positive")
            .map(|d| (d.severity, d.code))
            .collect()
        };
        let row = |lat: f64, lon: f64, n: Value| json!({ "c0": lat, "c1": lon, "c2": n });
        assert!(codes(vec![
            row(50.0, 10.0, json!(1.0)),
            row(51.0, 10.0, json!(2.0))
        ])
        .is_empty());
        assert_eq!(
            codes(vec![
                row(50.0, 10.0, json!(1.0)),
                row(51.0, 10.0, json!(0.0)),
                row(52.0, 10.0, Value::Null),
            ]),
            vec![(Severity::Warning, "map/size-not-positive")]
        );
    }

    /// M2d: the four `::BIVARIATE` lint codes.
    #[test]
    fn bivariate_lints() {
        let codes = |sql: &str, rows: Vec<Value>| -> Vec<(Severity, &'static str)> {
            let data: Vec<_> = rows
                .iter()
                .map(|r| r.as_object().unwrap().clone())
                .collect();
            check_opts(
                sql,
                |_q| Ok(data.clone()),
                LintOptions {
                    design: false,
                    ..Default::default()
                },
            )
            .into_iter()
            .filter(|d| d.code.starts_with("map/bivariate"))
            .map(|d| (d.severity, d.code))
            .collect()
        };
        let row = |region: &str, a: Value, b: Value| json!({ "c0": region, "c1": a, "c2": b });

        // no first measure: drawn as a plain map, warned.
        assert_eq!(
            codes(
                "SELECT region::REGION, a::MAP, b::BIVARIATE FROM t;",
                vec![row("DE-BY", json!("x"), json!(1.0))]
            ),
            vec![(Severity::Warning, "map/bivariate-needs-measure")]
        );

        // a well-formed bivariate map over enough distinct values: no lint.
        let rows9: Vec<Value> = (0..9)
            .map(|i| row("DE-BY", json!((i % 3) as f64), json!((i / 3) as f64)))
            .collect();
        assert!(codes(
            "SELECT region::REGION, a::BARCHART, b::BIVARIATE FROM t;",
            rows9.clone()
        )
        .is_empty());

        // ::CLASSES is ignored.
        let mut with_classes = rows9.clone();
        for r in &mut with_classes {
            r.as_object_mut()
                .unwrap()
                .insert("c3".to_string(), json!("quantile 3"));
        }
        assert_eq!(
            codes(
                "SELECT region::REGION, a::BARCHART, b::BIVARIATE, c::CLASSES FROM t;",
                with_classes
            ),
            vec![(Severity::Note, "map/bivariate-classes")]
        );

        // ::SIZE makes ::BIVARIATE a no-op.
        let mut with_size = rows9.clone();
        for r in &mut with_size {
            r.as_object_mut()
                .unwrap()
                .insert("c3".to_string(), json!(2.0));
        }
        assert_eq!(
            codes(
                "SELECT region::REGION, a::BARCHART, b::BIVARIATE, n::SIZE FROM t;",
                with_size
            ),
            vec![(Severity::Warning, "map/bivariate-ignored")]
        );

        // fewer than 3 distinct values in ::BIVARIATE.
        assert_eq!(
            codes(
                "SELECT region::REGION, a::BARCHART, b::BIVARIATE FROM t;",
                vec![
                    row("DE-BY", json!(1.0), json!(1.0)),
                    row("DE-BW", json!(2.0), json!(1.0)),
                    row("DE-SN", json!(3.0), json!(1.0)),
                ]
            ),
            vec![(Severity::Note, "map/bivariate-few-levels")]
        );
    }

    #[test]
    fn flow_needs_both_ends() {
        let codes = |sql: &str, rows: Vec<Value>| -> Vec<(Severity, &'static str)> {
            let data: Vec<_> = rows
                .iter()
                .map(|r| r.as_object().unwrap().clone())
                .collect();
            check_opts(
                sql,
                |_q| Ok(data.clone()),
                LintOptions {
                    design: false,
                    ..Default::default()
                },
            )
            .into_iter()
            .filter(|d| d.code.starts_with("map/flow"))
            .map(|d| (d.severity, d.code))
            .collect()
        };
        let d = codes("SELECT o::ORIGIN FROM t;", vec![json!({ "c0": "DE-BY" })]);
        assert_eq!(d, vec![(Severity::Warning, "map/flow-needs-both")]);
    }

    #[test]
    fn flow_missing_end_is_flagged() {
        let codes = |rows: Vec<Value>| -> Vec<(Severity, &'static str)> {
            let data: Vec<_> = rows
                .iter()
                .map(|r| r.as_object().unwrap().clone())
                .collect();
            check_opts(
                "SELECT o::ORIGIN, d::DEST FROM t;",
                |_q| Ok(data.clone()),
                LintOptions {
                    design: false,
                    ..Default::default()
                },
            )
            .into_iter()
            .filter(|d| d.code == "map/flow-missing-end")
            .map(|d| (d.severity, d.code))
            .collect()
        };
        assert!(codes(vec![json!({ "c0": "DE-BY", "c1": "DE-BW" })]).is_empty());
        assert_eq!(
            codes(vec![json!({ "c0": "DE-BY", "c1": "ZZ-NOPE" })]),
            vec![(Severity::Warning, "map/flow-missing-end")]
        );
    }

    #[test]
    fn flow_self_is_flagged() {
        let codes = |rows: Vec<Value>| -> Vec<(Severity, &'static str)> {
            let data: Vec<_> = rows
                .iter()
                .map(|r| r.as_object().unwrap().clone())
                .collect();
            check_opts(
                "SELECT o::ORIGIN, d::DEST FROM t;",
                |_q| Ok(data.clone()),
                LintOptions {
                    design: false,
                    ..Default::default()
                },
            )
            .into_iter()
            .filter(|d| d.code == "map/flow-self")
            .map(|d| (d.severity, d.code))
            .collect()
        };
        assert!(codes(vec![json!({ "c0": "DE-BY", "c1": "DE-BW" })]).is_empty());
        assert_eq!(
            codes(vec![json!({ "c0": "DE-BY", "c1": "de-by" })]),
            vec![(Severity::Note, "map/flow-self")]
        );
    }

    #[test]
    fn hexbin_bad_size_is_flagged() {
        let codes = |v: &str| -> Vec<(Severity, &'static str)> {
            let data = vec![json!({ "c0": 50.0, "c1": 10.0, "c2": v })
                .as_object()
                .unwrap()
                .clone()];
            check_opts(
                "SELECT lat::LAT, lon::LON, h::HEXBIN FROM t;",
                |_q| Ok(data.clone()),
                LintOptions {
                    design: false,
                    ..Default::default()
                },
            )
            .into_iter()
            .filter(|d| d.code == "map/hexbin-bad-size")
            .map(|d| (d.severity, d.code))
            .collect()
        };
        assert!(codes("auto").is_empty());
        assert!(codes("25 km").is_empty());
        assert_eq!(
            codes("huge"),
            vec![(Severity::Warning, "map/hexbin-bad-size")]
        );
        assert_eq!(
            codes("-5"),
            vec![(Severity::Warning, "map/hexbin-bad-size")]
        );
    }

    #[test]
    fn hexbin_needs_points_is_flagged() {
        let data = vec![json!({ "c0": "DE-BY", "c1": "auto" })
            .as_object()
            .unwrap()
            .clone()];
        let d = check_opts(
            "SELECT c::REGION, h::HEXBIN FROM t;",
            |_q| Ok(data.clone()),
            LintOptions {
                design: false,
                ..Default::default()
            },
        );
        let codes: Vec<_> = d
            .into_iter()
            .filter(|d| d.code.starts_with("map/hexbin"))
            .map(|d| (d.severity, d.code))
            .collect();
        assert_eq!(codes, vec![(Severity::Warning, "map/hexbin-needs-points")]);
    }

    #[test]
    fn hexbin_mercator_is_noted() {
        let data = vec![
            json!({ "c0": 50.0, "c1": 10.0, "c2": "auto", "c3": "mercator" })
                .as_object()
                .unwrap()
                .clone(),
        ];
        let d = check_opts(
            "SELECT lat::LAT, lon::LON, h::HEXBIN, 'mercator'::PROJECTION FROM t;",
            |_q| Ok(data.clone()),
            LintOptions {
                design: false,
                ..Default::default()
            },
        );
        let codes: Vec<_> = d
            .into_iter()
            .filter(|d| d.code == "map/hexbin-mercator")
            .map(|d| (d.severity, d.code))
            .collect();
        assert_eq!(codes, vec![(Severity::Note, "map/hexbin-mercator")]);
    }

    #[test]
    fn hexbin_category_is_noted() {
        let data = vec![json!({ "c0": 50.0, "c1": 10.0, "c2": "auto", "c3": "A" })
            .as_object()
            .unwrap()
            .clone()];
        let d = check_opts(
            "SELECT lat::LAT, lon::LON, h::HEXBIN, cat::CATEGORY FROM t;",
            |_q| Ok(data.clone()),
            LintOptions {
                design: false,
                ..Default::default()
            },
        );
        let codes: Vec<_> = d
            .into_iter()
            .filter(|d| d.code == "map/hexbin-category")
            .map(|d| (d.severity, d.code))
            .collect();
        assert_eq!(codes, vec![(Severity::Note, "map/hexbin-category")]);
    }

    #[test]
    fn scatter_hexbin_category_is_noted() {
        let data = vec![json!({ "c0": 1.0, "c1": 2.0, "c2": "auto", "c3": "A" })
            .as_object()
            .unwrap()
            .clone()];
        let d = check_opts(
            "SELECT x::XAXIS, y::SCATTER, h::HEXBIN, cat::CATEGORY FROM t;",
            |_q| Ok(data.clone()),
            LintOptions {
                design: false,
                ..Default::default()
            },
        );
        let codes: Vec<_> = d
            .into_iter()
            .filter(|d| d.code == "chart/hexbin-category")
            .map(|d| (d.severity, d.code))
            .collect();
        assert_eq!(codes, vec![(Severity::Note, "chart/hexbin-category")]);
    }

    #[test]
    fn scatter_hexbin_bad_size_is_flagged() {
        let data = vec![json!({ "c0": 1.0, "c1": 2.0, "c2": "huge" })
            .as_object()
            .unwrap()
            .clone()];
        let d = check_opts(
            "SELECT x::XAXIS, y::SCATTER, h::HEXBIN FROM t;",
            |_q| Ok(data.clone()),
            LintOptions {
                design: false,
                ..Default::default()
            },
        );
        let codes: Vec<_> = d
            .into_iter()
            .filter(|d| d.code == "chart/hexbin-bad-size")
            .map(|d| (d.severity, d.code))
            .collect();
        assert_eq!(codes, vec![(Severity::Warning, "chart/hexbin-bad-size")]);
    }

    #[test]
    fn scatter_hexbin_on_a_map_defers_to_the_map_diags() {
        // A map panel's own `::HEXBIN` is `hexbin_diags`'s job (via
        // `map_diags`) — `scatter_hexbin_diags` must stay quiet so the two
        // don't double-report the same column.
        let data = vec![json!({ "c0": 50.0, "c1": 10.0, "c2": "auto", "c3": "A" })
            .as_object()
            .unwrap()
            .clone()];
        let d = check_opts(
            "SELECT lat::LAT, lon::LON, h::HEXBIN, cat::CATEGORY FROM t;",
            |_q| Ok(data.clone()),
            LintOptions {
                design: false,
                ..Default::default()
            },
        );
        assert!(
            !d.iter().any(|d| d.code.starts_with("chart/hexbin")),
            "{d:?}"
        );
    }

    #[test]
    fn too_many_marks_skips_hexbin_maps() {
        let pts: Vec<Value> = (0..20_001)
            .map(|i| json!({ "c0": (i % 80) as f64, "c1": 10.0, "c2": "auto" }))
            .collect();
        let data: Vec<_> = pts.iter().map(|r| r.as_object().unwrap().clone()).collect();
        let d = check_opts(
            "SELECT lat::LAT, lon::LON, h::HEXBIN FROM t;",
            |_q| Ok(data.clone()),
            LintOptions {
                design: false,
                ..Default::default()
            },
        );
        assert!(!d.iter().any(|d| d.code == "map/too-many-marks"), "{d:?}");
    }

    #[test]
    fn too_many_flows_is_flagged() {
        let codes = |rows: Vec<Value>| -> Vec<(Severity, &'static str)> {
            let data: Vec<_> = rows
                .iter()
                .map(|r| r.as_object().unwrap().clone())
                .collect();
            check_opts(
                "SELECT o::ORIGIN, d::DEST FROM t;",
                |_q| Ok(data.clone()),
                LintOptions {
                    design: false,
                    ..Default::default()
                },
            )
            .into_iter()
            .filter(|d| d.code == "map/too-many-flows")
            .map(|d| (d.severity, d.code))
            .collect()
        };
        let few: Vec<Value> = (0..10)
            .map(|i| json!({ "c0": format!("POINT({i} 1)"), "c1": "POINT(0 0)" }))
            .collect();
        assert!(codes(few).is_empty());
        let many: Vec<Value> = (0..2_001)
            .map(|i| json!({ "c0": format!("POINT({i} 1)"), "c1": "POINT(0 0)" }))
            .collect();
        assert_eq!(codes(many), vec![(Severity::Warning, "map/too-many-flows")]);
    }

    #[test]
    fn map_regions_and_points_are_checked() {
        let codes = |sql: &str, rows: Vec<Value>| -> Vec<(Severity, &'static str, String)> {
            let data: Vec<_> = rows
                .iter()
                .map(|r| r.as_object().unwrap().clone())
                .collect();
            check_opts(
                sql,
                |_q| Ok(data.clone()),
                LintOptions {
                    design: false,
                    ..Default::default()
                },
            )
            .into_iter()
            .filter(|d| d.code.starts_with("map/"))
            .map(|d| (d.severity, d.code, d.message))
            .collect()
        };
        let sql = "SELECT c::REGION, v::BARCHART FROM t;";
        let rows = |cs: &[&str]| cs.iter().map(|c| json!({ "c0": c, "c1": 1.0 })).collect();
        assert!(codes(sql, rows(&["DE-BY", "DE-BE"])).is_empty());
        let d = codes(sql, rows(&["DE-BY", "DE-XX", "DE-YY"]));
        assert_eq!(d.len(), 1, "{d:?}");
        assert_eq!((d[0].0, d[0].1), (Severity::Warning, "map/unknown-region"));
        assert!(
            d[0].2.contains("'DE-XX', 'DE-YY'") && d[0].2.contains("de-states"),
            "{}",
            d[0].2
        );
        let d = codes(sql, rows(&["Bayern", "Hessen"]));
        assert_eq!(
            d.iter().map(|x| x.1).collect::<Vec<_>>(),
            vec!["map/region-by-name"]
        );
        let d = codes(
            "SELECT c::REGION, 'states'::BOUNDARIES FROM t;",
            vec![json!({ "c0": "DE-BY", "c1": "states" })],
        );
        assert!(
            d.iter()
                .any(|x| x.1 == "map/unknown-region" && x.2.contains("BOUNDARIES")),
            "{d:?}"
        );
        // ::LAT beyond ±90 that would be longitudes: swapped
        let pts = |la: f64, lo: f64| json!({ "c0": la, "c1": lo });
        let sql = "SELECT a::LAT, b::LON FROM t;";
        assert_eq!(
            codes(sql, vec![pts(13.4, 52.5), pts(120.5, 30.0)])
                .iter()
                .map(|x| x.1)
                .collect::<Vec<_>>(),
            vec!["map/lat-lon-swapped"]
        );
        assert!(codes(sql, vec![pts(52.5, 13.4), pts(30.0, 120.5)]).is_empty());
    }

    #[test]
    fn motion_directives_are_checked() {
        let codes = |sql: &str, row: Value| -> Vec<&'static str> {
            let data = vec![row.as_object().unwrap().clone()];
            check_opts(
                sql,
                |_q| Ok(data.clone()),
                LintOptions {
                    design: false,
                    ..Default::default()
                },
            )
            .into_iter()
            .map(|d| d.code)
            .filter(|c| {
                [
                    "animate/",
                    "linestyle/",
                    "story/",
                    "transition/",
                    "points/",
                    "hover-focus/",
                ]
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
        let row = json!({ "c0": "a", "c1": 1.0, "c2": "dim" });
        assert_eq!(
            codes(
                "SELECT x::XAXIS, y::BARCHART, 'dim'::HOVER_FOCUS FROM t;",
                row
            ),
            vec!["hover-focus/unknown"]
        );
        let row = json!({ "c0": "a", "c1": 1.0, "c2": "none" });
        assert!(codes(
            "SELECT x::XAXIS, y::BARCHART, 'none'::HOVER_FOCUS FROM t;",
            row
        )
        .is_empty());
        let row = json!({ "c0": "a", "c1": 1.0, "c2": "hidden" });
        assert_eq!(
            codes(
                "SELECT x::XAXIS, y::LINECHART, 'hidden'::POINTS FROM t;",
                row
            ),
            vec!["points/unknown"]
        );
        for ok in ["auto", "None", "always"] {
            let row = json!({ "c0": "a", "c1": 1.0, "c2": ok });
            assert!(codes(
                &format!("SELECT x::XAXIS, y::LINECHART, '{ok}'::POINTS FROM t;"),
                row
            )
            .is_empty());
        }
        let row = json!({ "c0": "a", "c1": 1.0, "c2": "grow" });
        assert!(codes("SELECT x::XAXIS, y::BARCHART, 'grow'::ANIMATE FROM t;", row).is_empty());
    }

    #[test]
    fn dropdown_style_unknown_value_warns() {
        let data = vec![json!({ "c0": "a", "c1": "bogus" })
            .as_object()
            .unwrap()
            .clone()];
        let sql = "SELECT 'a' AS v ::DROPDOWN, 'bogus' AS style ::DROPDOWN_STYLE FROM t;";
        let d = check_opts(
            sql,
            |_q| Ok(data.clone()),
            LintOptions {
                design: false,
                ..Default::default()
            },
        );
        assert!(
            d.iter().any(|x| x.code == "dropdown-style/unknown"),
            "{d:?}"
        );

        let data = vec![json!({ "c0": "a", "c1": "search" })
            .as_object()
            .unwrap()
            .clone()];
        let sql = "SELECT 'a' AS v ::DROPDOWN, 'search' AS style ::DROPDOWN_STYLE FROM t;";
        let d = check_opts(
            sql,
            |_q| Ok(data.clone()),
            LintOptions {
                design: false,
                ..Default::default()
            },
        );
        assert!(
            !d.iter().any(|x| x.code == "dropdown-style/unknown"),
            "{d:?}"
        );
    }

    #[test]
    fn design_pass_can_be_disabled() {
        let data = rows(
            3,
            |i| json!({ "c0": format!("c{i}"), "c1": (3 - i) as f64 }),
        );
        let flat = "SELECT x::XAXIS, y::BARCHART FROM t;\n".repeat(9);
        let d = check_opts(
            &flat,
            |_q| Ok(data.clone()),
            LintOptions {
                design: false,
                ..Default::default()
            },
        );
        assert!(!d.iter().any(|x| x.code.starts_with("design/")), "{d:?}");
    }

    #[test]
    fn an_inline_layout_role_is_flagged() {
        // #685: `6::COL` inside a chart's SELECT drops the chart
        let data = rows(
            3,
            |i| json!({ "c0": i, "c1": i as f64, "c2": "Line", "c3": 6 }),
        );
        let codes = |sql: &str| -> Vec<&'static str> {
            check(sql, |_q| Ok(data.clone()))
                .into_iter()
                .map(|d| d.code)
                .filter(|c| c.starts_with("layout/"))
                .collect()
        };
        assert_eq!(
            codes("SELECT x::XAXIS, y ::LINECHART, 'Line'::TITLE, 6::COL FROM t;"),
            vec!["layout/inline-layout-role"]
        );
        assert_eq!(
            codes("SELECT x::XAXIS, y ::BARCHART, 400::HEIGHT FROM t;"),
            vec!["layout/inline-layout-role"]
        );
        // on its own line: fine
        assert!(codes("SELECT 6::COL;\nSELECT x::XAXIS, y ::LINECHART FROM t;").is_empty());
    }

    #[test]
    fn default_on_an_unlisted_input_is_flagged() {
        let one = rows(1, |_| json!({ "n": 5, "c1": true }));
        let codes = |sql: &str| -> Vec<&'static str> {
            check(sql, |_q| Ok(one.clone()))
                .into_iter()
                .map(|d| d.code)
                .filter(|c| c.starts_with("input/"))
                .collect()
        };
        assert_eq!(
            codes("SELECT 5 AS n ::NUMBER, true ::DEFAULT;"),
            vec!["input/default-ignored"]
        );
        assert!(codes("SELECT 'a' AS n ::DROPDOWN, true ::DEFAULT;").is_empty());
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
        // #435: the default is sent in its own round (eagerly, to the
        // session), not bundled into the next statement's own text.
        assert!(
            seen.iter()
                .any(|q| q.contains("SET VARIABLE \"zone\" = 'EU';")),
            "variable not primed: {seen:?}"
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
        // #638: a ::DEFAULT column picks the marked row(s) instead.
        let marked = rows(3, |i| {
            let w = ["1 day", "7 days", "30 days"][i];
            json!({ "w": w, "d": i == 1 })
        });
        let d = input_defaults_with(InputKind::Buttons, 0, Some(1), &marked);
        assert_eq!(d, vec!["SET VARIABLE \"w\" = '7 days'".to_string()]);
        let d = input_defaults_with(InputKind::Dropdown, 0, Some(1), &marked);
        assert_eq!(d, vec!["SET VARIABLE \"w\" = '7 days'".to_string()]);
        let multi = rows(3, |i| {
            let v = ["a", "b", "c"][i];
            json!({ "s": v, "d": i != 1 })
        });
        let d = input_defaults_with(InputKind::Multiselect, 0, Some(1), &multi);
        assert_eq!(d, vec!["SET VARIABLE \"s\" = ['a', 'c']".to_string()]);
        // none marked: today's first option / all
        let none = rows(2, |i| {
            let v = ["a", "b"][i];
            json!({ "s": v, "d": false })
        });
        let d = input_defaults_with(InputKind::Multiselect, 0, Some(1), &none);
        assert_eq!(d, vec!["SET VARIABLE \"s\" = ['a', 'b']".to_string()]);
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

    /// #659: an unknown ::MARKDOWN_TONE is an error; a tone beside a
    /// 'plain'/'callout' style or a ::CALLOUT is dropped (a warning).
    #[test]
    fn markdown_tone_is_checked() {
        let row =
            |cells: serde_json::Value| move |_q: &str| Ok(vec![cells.as_object().unwrap().clone()]);
        let codes = |d: &[Diagnostic]| -> Vec<&'static str> {
            d.iter()
                .map(|x| x.code)
                .filter(|c| c.starts_with("markdown_tone/"))
                .collect()
        };
        let d = check(
            "SELECT 'x'::MARKDOWN, 'example: Beispiel'::MARKDOWN_TONE;",
            row(json!({ "c0": "x", "c1": "example: Beispiel" })),
        );
        assert!(codes(&d).is_empty(), "{d:?}");
        let d = check(
            "SELECT 'x'::MARKDOWN, 'warning'::MARKDOWN_TONE;",
            row(json!({ "c0": "x", "c1": "warning" })),
        );
        assert_eq!(codes(&d), ["markdown_tone/unknown"], "{d:?}");
        assert!(!passes(&d, false));
        let d = check(
            "SELECT 'x'::MARKDOWN, 'plain'::MARKDOWN_STYLE, 'tip'::MARKDOWN_TONE;",
            row(json!({ "c0": "x", "c1": "plain", "c2": "tip" })),
        );
        assert_eq!(codes(&d), ["markdown_tone/ignored"], "{d:?}");
        let d = check(
            "SELECT 'x'::MARKDOWN, 'note'::CALLOUT, 'tip'::MARKDOWN_TONE;",
            row(json!({ "c0": "x", "c1": "note", "c2": "tip" })),
        );
        assert_eq!(codes(&d), ["markdown_tone/ignored"], "{d:?}");
        let d = check(
            "SELECT 'x'::MARKDOWN, 'card'::MARKDOWN_STYLE, 'tip'::MARKDOWN_TONE;",
            row(json!({ "c0": "x", "c1": "card", "c2": "tip" })),
        );
        assert!(codes(&d).is_empty(), "{d:?}");
        let d = check(
            "SELECT 'x'::TEXT_SMALL, 'tip'::MARKDOWN_TONE;",
            row(json!({ "c0": "x", "c1": "tip" })),
        );
        assert_eq!(codes(&d), ["markdown_tone/no-text"], "{d:?}");
    }

    #[test]
    fn markdown_style_kind_and_text_are_checked() {
        let one = |v: &str| {
            let v = v.to_string();
            move |_q: &str| {
                Ok(rows(
                    1,
                    |_| json!({ "c0": "**Heads up**", "c1": v.clone() }),
                ))
            }
        };
        let d = check(
            "SELECT 'x'::MARKDOWN, 'plain'::MARKDOWN_STYLE;",
            one("plain"),
        );
        assert!(
            !d.iter().any(|x| x.code.starts_with("markdown_style/")),
            "{d:?}"
        );
        let d = check(
            "SELECT 'x'::MARKDOWN, 'invisible'::MARKDOWN_STYLE;",
            one("invisible"),
        );
        assert!(
            d.iter().any(|x| x.code == "markdown_style/unknown"),
            "{d:?}"
        );
        let d = check(
            "SELECT 'x'::TEXT_SMALL, 'plain'::MARKDOWN_STYLE;",
            one("plain"),
        );
        assert!(
            d.iter().any(|x| x.code == "markdown_style/no-text"),
            "{d:?}"
        );
    }

    #[test]
    fn markdown_style_conflicting_with_callout_is_checked() {
        let d = check(
            "SELECT 'x'::MARKDOWN, 'plain'::MARKDOWN_STYLE, 'warning'::CALLOUT;",
            |_q| {
                Ok(rows(
                    1,
                    |_| json!({ "c0": "x", "c1": "plain", "c2": "warning" }),
                ))
            },
        );
        assert!(
            d.iter()
                .any(|x| x.code == "markdown_style/conflicts-with-callout"),
            "{d:?}"
        );
        // 'callout' together with ::CALLOUT is the intended, non-conflicting
        // combination.
        let d = check(
            "SELECT 'x'::MARKDOWN, 'callout'::MARKDOWN_STYLE, 'warning'::CALLOUT;",
            |_q| {
                Ok(rows(
                    1,
                    |_| json!({ "c0": "x", "c1": "callout", "c2": "warning" }),
                ))
            },
        );
        assert!(
            !d.iter()
                .any(|x| x.code == "markdown_style/conflicts-with-callout"),
            "{d:?}"
        );
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

    #[test]
    fn name_diags_flags_bad_reserved_duplicate_and_not_drawn() {
        let codes = |script: &str| -> Vec<(usize, &'static str)> {
            name_diags(&sql::plan(script))
                .iter()
                .map(|d| (d.stmt, d.code))
                .collect()
        };
        // A well-formed, unique name on a drawn statement: clean.
        assert!(codes("-- @name kpi\nSELECT 1::METRIC;").is_empty());
        // A name on a `::GROUP`, a `::TAB`, an input and a heading: all draw
        // something, so none of them is `name/not-drawn`.
        assert!(codes("-- @name box\nSELECT 'Filters'::GROUP;").is_empty());
        assert!(codes("-- @name t1\nSELECT 'Overview'::TAB;").is_empty());
        assert!(
            codes("-- @name region_filter\nSELECT DISTINCT region::DROPDOWN FROM t;").is_empty()
        );
        assert!(codes("-- @name heading\nSELECT 'Section'::LABEL;").is_empty());
        // Not a plain identifier.
        assert_eq!(codes("-- @name 1bad\nSELECT 1::METRIC;"), [(1, "name/bad")]);
        assert_eq!(
            codes("-- @name has space\nSELECT 1::METRIC;"),
            [(1, "name/bad")]
        );
        // `page` is reserved for the guided tour's intro/outro.
        assert_eq!(
            codes("-- @name page\nSELECT 1::METRIC;"),
            [(1, "name/reserved")]
        );
        // The same name on two statements.
        assert_eq!(
            codes("-- @name kpi\nSELECT 1::METRIC;\n-- @name kpi\nSELECT 2::METRIC;"),
            [(2, "name/duplicate")]
        );
        // Setup, and an invisible layout directive, draw nothing.
        assert_eq!(
            codes("-- @name setup\nSET VARIABLE x = 1;"),
            [(1, "name/not-drawn")]
        );
        assert_eq!(
            codes("-- @name span\nSELECT 6::SPAN;\nSELECT 1::METRIC;"),
            [(1, "name/not-drawn")]
        );
        // A bad name on an undrawn statement: both fire.
        assert_eq!(
            codes("-- @name 1bad\nSELECT 6::SPAN;\nSELECT 1::METRIC;"),
            [(1, "name/bad"), (1, "name/not-drawn")]
        );
    }

    #[test]
    fn tour_diags_flags_every_code() {
        let codes = |script: &str| -> Vec<&'static str> {
            tour_diags(script, &sql::plan(script))
                .iter()
                .map(|d| d.code)
                .collect()
        };
        // A clean tour pointing at `page` and a real `-- @name`: no diagnostic.
        assert!(codes(
            "-- @tour t Intro\n--   page: hello\n--   kpi: a kpi\n-- @name kpi\nSELECT 1::METRIC;"
        )
        .is_empty());
        // Not a plain identifier.
        assert_eq!(codes("-- @tour 1bad\n--   page: x\n"), ["tour/bad-id"]);
        // The same id on two tours.
        assert_eq!(
            codes("-- @tour t\n--   page: x\n-- @tour t\n--   page: y\n"),
            ["tour/duplicate-id"]
        );
        // No steps.
        assert_eq!(codes("-- @tour t\n"), ["tour/empty"]);
        // Neither `page` nor any `-- @name`.
        assert_eq!(codes("-- @tour t\n--   nope: x\n"), ["tour/unknown-panel"]);
        // Over 400 characters.
        assert_eq!(
            codes(&format!("-- @tour t\n--   page: {}\n", "x".repeat(401))),
            ["tour/long-text"]
        );
        // Over 20 steps.
        let many_steps = (0..21)
            .map(|i| format!("--   page: step {i}\n"))
            .collect::<String>();
        assert_eq!(
            codes(&format!("-- @tour t\n{many_steps}")),
            ["tour/too-many-steps"]
        );
        // Over 10 tours.
        let many_tours = (0..11)
            .map(|i| format!("-- @tour t{i}\n--   page: x\n"))
            .collect::<String>();
        assert_eq!(codes(&many_tours), ["tour/too-many"]);
        // `-- @tour_autostart` naming no tour.
        assert_eq!(
            codes("-- @tour_autostart nope\n-- @tour t\n--   page: x\n"),
            ["tour/autostart-unknown"]
        );
        // `-- @tour_autostart` naming a real tour: clean.
        assert!(codes("-- @tour_autostart t\n-- @tour t\n--   page: x\n").is_empty());
        // A step-shaped line right after another header line, with no
        // `-- @tour` above it — likely mis-indented.
        assert_eq!(codes("-- @owner Ops\n--   x: y\n"), ["tour/stray-step"]);
        // The same, but after a blank/ordinary comment line: not flagged
        // (most likely just a comment, not a mis-indented tour).
        assert!(codes("-- plain comment\n--   x: y\n").is_empty());
    }
}

/// `::SUBMIT`/`::SUBMIT_VARS`/`::SUBMIT_DONE` (#517 S1): roles + lint only —
/// no endpoint, no `options.state`, no read-back yet. The fake session
/// matches queries by a literal substring that survives the `::ROLE` → `AS
/// c{i}` rewrite (the role casts, not the literals, are stripped).
#[cfg(test)]
mod submit_tests {
    use super::*;
    use serde_json::json;

    fn row(obj: serde_json::Value) -> Map<String, Value> {
        obj.as_object().unwrap().clone()
    }

    fn has(diags: &[Diagnostic], code: &str) -> bool {
        diags.iter().any(|d| d.code == code)
    }

    fn of<'a>(diags: &'a [Diagnostic], code: &str) -> Vec<&'a Diagnostic> {
        diags.iter().filter(|d| d.code == code).collect()
    }

    const INPUT: &str = "SELECT q1 ::BUTTONS FROM (VALUES ('A'), ('B'), ('C')) t(q1);\n";

    /// A one-input, one-submit script; `submit_vars` is the `::SUBMIT_VARS`
    /// column's literal expression text (inserted as-is, so a test can pass
    /// something other than a plain string literal).
    fn script(submit_vars: &str) -> String {
        format!(
            "{INPUT}SELECT 'Check my answers' AS quiz_1::SUBMIT, {submit_vars} AS r::SUBMIT_VARS;\n"
        )
    }

    /// Lints [`script`]`(submit_vars)`. `vars_value` is what the `::SUBMIT`
    /// row's `SUBMIT_VARS` cell evaluates to at runtime — the test supplies
    /// it directly (it already knows, having built `submit_vars`) rather
    /// than this fake session re-deriving it from the rewritten query text.
    fn lint_vars(submit_vars: &str, vars_value: serde_json::Value) -> Vec<Diagnostic> {
        check(&script(submit_vars), |q| {
            if q.contains("VALUES ('A')") {
                // Input roles keep their original column name (`q1`), not a
                // positional `c{i}` alias — see `sql::rewrite`'s
                // `Role::Input(_)` branch.
                Ok(vec![
                    row(json!({"q1": "A"})),
                    row(json!({"q1": "B"})),
                    row(json!({"q1": "C"})),
                ])
            } else if q.contains("Check my answers") {
                Ok(vec![row(
                    json!({"c0": "Check my answers", "c1": vars_value}),
                )])
            } else {
                // The injected `viewer_state` temp (setup) and anything else.
                Ok(vec![])
            }
        })
    }

    /// [`lint_vars`] for the common case: `submit_vars` is a plain string
    /// literal and also the value it evaluates to.
    fn lint(submit_vars_literal: &str) -> Vec<Diagnostic> {
        let value = submit_vars_literal
            .trim()
            .trim_start_matches('\'')
            .trim_end_matches('\'')
            .replace("''", "'");
        lint_vars(submit_vars_literal, json!(value))
    }

    #[test]
    fn valid_submit_is_clean() {
        let d = lint("'q1'");
        for code in [
            "submit/unknown-variable",
            "submit/unsupported-input",
            "submit/duplicate-input",
            "submit/no-vars",
            "submit/too-many-vars",
            "submit/rows",
            "submit/dynamic-vars",
        ] {
            assert!(!has(&d, code), "unexpected {code} in {d:?}");
        }
    }

    #[test]
    fn unknown_variable_is_an_error() {
        let d = lint("'q1, nope'");
        let found = of(&d, "submit/unknown-variable");
        assert_eq!(found.len(), 1, "{d:?}");
        assert_eq!(found[0].severity, Severity::Error);
        assert!(found[0].message.contains("nope"), "{d:?}");
    }

    #[test]
    fn no_vars_is_an_error_when_empty() {
        let d = lint("''");
        assert!(has(&d, "submit/no-vars"), "{d:?}");
    }

    #[test]
    fn no_vars_is_an_error_when_not_a_string() {
        let script = format!("{INPUT}SELECT 'Check' AS quiz_1::SUBMIT, 1 AS r::SUBMIT_VARS;\n");
        let d = check(&script, |q| {
            if q.contains("VALUES ('A')") {
                Ok(vec![row(json!({"q1": "A"}))])
            } else {
                Ok(vec![row(json!({"c0": "Check", "c1": 1}))])
            }
        });
        assert!(has(&d, "submit/no-vars"), "{d:?}");
    }

    #[test]
    fn too_many_vars_is_an_error() {
        let names: Vec<String> = (0..40).map(|i| format!("q1_{i}")).collect();
        let d = lint(&format!("'{}'", names.join(", ")));
        assert!(has(&d, "submit/too-many-vars"), "{d:?}");
    }

    #[test]
    fn unsupported_daterange_input_is_an_error() {
        let script = "SELECT d0 ::DATERANGE, d1 FROM (VALUES (DATE '2026-01-01', DATE '2026-01-31')) t(d0, d1);\n\
                       SELECT 'Check' AS quiz_1::SUBMIT, 'd0' AS r::SUBMIT_VARS;\n";
        let d = check(script, |q| {
            if q.contains("2026-01-01") && !q.contains("Check") {
                Ok(vec![row(json!({"d0": "2026-01-01", "d1": "2026-01-31"}))])
            } else {
                Ok(vec![row(json!({"c0": "Check", "c1": "d0"}))])
            }
        });
        assert!(has(&d, "submit/unsupported-input"), "{d:?}");
    }

    #[test]
    fn duplicate_input_is_an_error() {
        let script = format!(
            "{INPUT}SELECT q1 ::DROPDOWN FROM (VALUES ('X'), ('Y')) t2(q1);\n\
             SELECT 'Check' AS quiz_1::SUBMIT, 'q1' AS r::SUBMIT_VARS;\n"
        );
        let d = check(&script, |q| {
            if q.contains("VALUES ('A')") {
                Ok(vec![row(json!({"q1": "A"}))])
            } else if q.contains("VALUES ('X')") {
                Ok(vec![row(json!({"q1": "X"}))])
            } else {
                Ok(vec![row(json!({"c0": "Check", "c1": "q1"}))])
            }
        });
        assert!(has(&d, "submit/duplicate-input"), "{d:?}");
    }

    #[test]
    fn dynamic_vars_is_a_warning() {
        // A computed expression instead of a literal — can't be checked
        // statically, even though it happens to evaluate to 'q1' right now.
        let d = lint_vars("(SELECT 'q1')", json!("q1"));
        assert!(has(&d, "submit/dynamic-vars"), "{d:?}");
    }

    #[test]
    fn zero_rows_is_a_warning_hidden_by_default() {
        let d = check(&script("'q1'"), |q| {
            if q.contains("VALUES ('A')") {
                Ok(vec![row(json!({"q1": "A"}))])
            } else {
                // Both the ::SUBMIT statement and the injected viewer_state
                // setup return no rows here — ::SUBMIT's 0 rows is the
                // thing under test (hidden by default).
                Ok(vec![])
            }
        });
        let found = of(&d, "submit/rows");
        assert_eq!(found.len(), 1, "{d:?}");
        assert_eq!(found[0].severity, Severity::Warning);
    }

    #[test]
    fn more_than_one_row_is_an_error() {
        let d = check(&script("'q1'"), |q| {
            if q.contains("VALUES ('A')") {
                Ok(vec![row(json!({"q1": "A"}))])
            } else if q.contains("Check my answers") {
                Ok(vec![
                    row(json!({"c0": "Check my answers", "c1": "q1"})),
                    row(json!({"c0": "Check my answers", "c1": "q1"})),
                ])
            } else {
                Ok(vec![])
            }
        });
        let found = of(&d, "submit/rows");
        assert_eq!(found.len(), 1, "{d:?}");
        assert_eq!(found[0].severity, Severity::Error);
    }

    #[test]
    fn options_from_variable_is_a_warning() {
        let script = "SELECT q1 ::BUTTONS FROM (VALUES ('A'), ('B')) t(q1) \
                       WHERE q1 = getvariable('region') OR 1 = 1;\n\
                       SELECT 'Check' AS quiz_1::SUBMIT, 'q1' AS r::SUBMIT_VARS;\n";
        let d = check(script, |q| {
            if q.contains("VALUES ('A')") {
                Ok(vec![row(json!({"q1": "A"}))])
            } else {
                Ok(vec![row(json!({"c0": "Check", "c1": "q1"}))])
            }
        });
        assert!(has(&d, "submit/options-from-variable"), "{d:?}");
    }

    #[test]
    fn viewer_state_temp_is_seeded_so_reading_it_does_not_fail() {
        // No ::SUBMIT at all — only a read-back reference — still gets the
        // empty temp (headless/lint parity, plan §3 "Headless renderers and
        // lint create an empty viewer_state").
        let script = "SELECT value::TEXT_SMALL FROM viewer_state;\n";
        let mut saw_create = false;
        let d = check(script, |q| {
            if q.contains("CREATE TEMP TABLE viewer_state") {
                saw_create = true;
            }
            Ok(vec![row(json!({"c0": "hi"}))])
        });
        assert!(saw_create, "viewer_state temp was never created");
        assert!(!has(&d, "sql-error"), "{d:?}");
    }

    #[test]
    fn a_pre_existing_real_viewer_state_is_not_shadowed() {
        // The probe (`sql::viewer_state_exists_probe_sql`) reports a real
        // (non-temp) `viewer_state` already on the connection — e.g. an
        // attached database's table — so lint must not create its own
        // empty temp on top of it (review of #530 #5: that would shadow
        // the real one for the rest of the session).
        let script = "SELECT value::TEXT_SMALL FROM viewer_state;\n";
        let mut saw_create = false;
        let d = check(script, |q| {
            if q.contains("duckdb_tables()") {
                return Ok(vec![row(json!({"n": 1}))]);
            }
            if q.contains("CREATE TEMP TABLE viewer_state") {
                saw_create = true;
            }
            Ok(vec![row(json!({"c0": "hi"}))])
        });
        assert!(
            !saw_create,
            "must not seed an empty viewer_state over a real one: {d:?}"
        );
    }

    #[test]
    fn shadowing_viewer_state_is_an_error() {
        let d = check("CREATE TEMP TABLE viewer_state AS SELECT 1;\n", |_| {
            Ok(vec![])
        });
        assert!(has(&d, "state/shadowed"), "{d:?}");
    }

    #[test]
    fn shadowing_a_dotted_viewer_state_is_also_an_error() {
        // `CREATE TABLE main.viewer_state …` — a schema/catalog qualifier
        // doesn't change what a later bare `FROM viewer_state` resolves to
        // (review of #530, nits).
        let d = check("CREATE TABLE main.viewer_state AS SELECT 1;\n", |_| {
            Ok(vec![])
        });
        assert!(has(&d, "state/shadowed"), "{d:?}");
    }

    #[test]
    fn markup_from_viewer_state_is_a_warning() {
        let script = "SELECT value::MARKDOWN FROM viewer_state;\n";
        let d = check(script, |_| Ok(vec![row(json!({"c0": "hello"}))]));
        assert!(has(&d, "state/markup-from-viewer"), "{d:?}");
    }

    // ── state_enablement_diags (#517 S2): submit/not-enabled, state/not-
    // enabled, deferred from S1 since they need --options locked.json ──

    #[test]
    fn submit_not_enabled_when_dashboard_is_missing_from_state_dashboards() {
        let script = "SELECT 'Check' AS quiz_1::SUBMIT, 'q1'::SUBMIT_VARS;\n";
        let d = state_enablement_diags(script, "tutorial", &[], &[]);
        assert!(has(&d, "submit/not-enabled"), "{d:?}");
    }

    #[test]
    fn submit_enabled_when_dashboard_is_in_state_dashboards() {
        let script = "SELECT 'Check' AS quiz_1::SUBMIT, 'q1'::SUBMIT_VARS;\n";
        let d = state_enablement_diags(
            script,
            "tutorial",
            &["tutorial".to_string()],
            &["tutorial".to_string()],
        );
        assert!(!has(&d, "submit/not-enabled"), "{d:?}");
    }

    #[test]
    fn state_not_enabled_when_dashboard_reads_viewer_state_but_is_in_no_list() {
        let script = "SELECT value::TEXT_SMALL FROM viewer_state;\n";
        let d = state_enablement_diags(script, "progress", &[], &[]);
        assert!(has(&d, "state/not-enabled"), "{d:?}");
    }

    #[test]
    fn state_enabled_via_read_only_list_does_not_need_submit_dashboards() {
        // `read`-listed (my-progress overview): may read, not submit — so
        // it must not need to be in `dashboards` too.
        let script = "SELECT value::TEXT_SMALL FROM viewer_state;\n";
        let d = state_enablement_diags(script, "my-progress", &[], &["my-progress".to_string()]);
        assert!(!has(&d, "state/not-enabled"), "{d:?}");
        assert!(!has(&d, "submit/not-enabled"), "{d:?}");
    }

    #[test]
    fn a_dashboard_with_neither_submit_nor_viewer_state_is_clean() {
        let script = "SELECT week::XAXIS, sum(n)::BARCHART FROM t GROUP BY ALL;\n";
        let d = state_enablement_diags(script, "weekly", &[], &[]);
        assert!(d.is_empty(), "{d:?}");
    }
}

/// `::SHOW_IF` lint checks (#515): one test per `show_if/*` code.
#[cfg(test)]
mod show_if_tests {
    use super::*;
    use serde_json::json;

    fn codes(d: &[Diagnostic]) -> Vec<&'static str> {
        d.iter().map(|x| x.code).collect()
    }

    fn has(d: &[Diagnostic], code: &str) -> bool {
        d.iter().any(|x| x.code == code)
    }

    /// `show_if_diags` alone (no query execution) — covers the plan-only
    /// codes: `not-alone`, `bad-target`, `no-target`, `constant`,
    /// `unknown-variable`.
    fn static_diags(sql: &str) -> Vec<Diagnostic> {
        show_if_diags(&crate::sql::plan(sql))
    }

    #[test]
    fn not_alone() {
        let d = static_diags("SELECT true::SHOW_IF, 1::COL; SELECT 'x'::MARKDOWN;");
        assert_eq!(codes(&d), ["show_if/not-alone"], "{d:?}");
        assert_eq!(d[0].severity, Severity::Error);
    }

    #[test]
    fn bad_target() {
        let d = static_diags(
            "SELECT getvariable('anofox_data') IS NOT NULL ::SHOW_IF; SELECT 'p'::TAB;",
        );
        assert_eq!(codes(&d), ["show_if/bad-target"], "{d:?}");
        assert_eq!(d[0].severity, Severity::Error);
    }

    #[test]
    fn no_target() {
        let d = static_diags(
            "SELECT 'x'::MARKDOWN; SELECT getvariable('anofox_data') IS NOT NULL ::SHOW_IF;",
        );
        assert_eq!(codes(&d), ["show_if/no-target"], "{d:?}");
        assert_eq!(d[0].severity, Severity::Warning);
    }

    #[test]
    fn constant() {
        let d = static_diags("SELECT true::SHOW_IF; SELECT 'x'::MARKDOWN;");
        assert_eq!(codes(&d), ["show_if/constant"], "{d:?}");
    }

    #[test]
    fn unknown_variable() {
        let d = static_diags("SELECT getvariable('nope') = 'a' ::SHOW_IF; SELECT 'x'::MARKDOWN;");
        assert_eq!(codes(&d), ["show_if/unknown-variable"], "{d:?}");
        assert!(d[0].message.contains("nope"), "{}", d[0].message);
    }

    #[test]
    fn a_known_variable_reads_clean() {
        let d = static_diags(
            "SELECT v AS q1 ::DROPDOWN FROM (VALUES ('a')) t(v);\n\
             SELECT getvariable('q1') = 'a' ::SHOW_IF;\nSELECT 'x'::MARKDOWN;",
        );
        assert!(d.is_empty(), "{d:?}");
    }

    #[test]
    fn claim_variables_are_never_unknown() {
        let d = static_diags(
            "SELECT getvariable('__claim_user') = 'alice' ::SHOW_IF; SELECT 'x'::MARKDOWN;",
        );
        assert!(!has(&d, "show_if/unknown-variable"), "{d:?}");
    }

    #[test]
    fn not_boolean() {
        let d = check("SELECT 1::SHOW_IF; SELECT 'x'::MARKDOWN;", |_q| {
            Ok(vec![json!({ "c0": 1 }).as_object().unwrap().clone()])
        });
        assert!(has(&d, "show_if/not-boolean"), "{d:?}");
    }

    #[test]
    fn a_true_boolean_cell_is_clean() {
        let d = check("SELECT true::SHOW_IF; SELECT 'x'::MARKDOWN;", |q| {
            if q.contains("'x'") {
                Ok(vec![json!({ "c0": "x" }).as_object().unwrap().clone()])
            } else {
                Ok(vec![json!({ "c0": true }).as_object().unwrap().clone()])
            }
        });
        assert!(!has(&d, "show_if/not-boolean"), "{d:?}");
        assert!(!has(&d, "show_if/no-row"), "{d:?}");
    }

    #[test]
    fn no_row() {
        let d = check(
            "SELECT true::SHOW_IF WHERE false; SELECT 'x'::MARKDOWN;",
            |q| {
                if q.contains("WHERE false") {
                    Ok(vec![])
                } else {
                    Ok(vec![json!({ "c0": "x" }).as_object().unwrap().clone()])
                }
            },
        );
        assert!(has(&d, "show_if/no-row"), "{d:?}");
    }
}

#[cfg(test)]
mod step_tests {
    use super::*;
    use serde_json::json;

    fn codes(d: &[Diagnostic]) -> Vec<&'static str> {
        d.iter().map(|x| x.code).collect()
    }

    fn has(d: &[Diagnostic], code: &str) -> bool {
        d.iter().any(|x| x.code == code)
    }

    /// `step_diags` alone (no query execution, no `-- @vars`).
    fn static_diags(sql: &str) -> Vec<Diagnostic> {
        step_diags(&crate::sql::plan(sql), sql)
    }

    #[test]
    fn inside_subtab() {
        let d =
            static_diags("SELECT 'sub'::SUBTAB; SELECT 'S1'::GUIDE_STEP; SELECT 'x'::MARKDOWN;");
        assert_eq!(codes(&d), ["step/inside-subtab"], "{d:?}");
        assert_eq!(d[0].severity, Severity::Error);
    }

    #[test]
    fn mixed_with_subtab() {
        let d =
            static_diags("SELECT 'S1'::GUIDE_STEP; SELECT 'x'::MARKDOWN; SELECT 'sub'::SUBTAB;");
        assert!(has(&d, "step/mixed-with-subtab"), "{d:?}");
    }

    #[test]
    fn lone_step() {
        let d = static_diags("SELECT 'S1'::GUIDE_STEP; SELECT 'x'::MARKDOWN;");
        assert!(has(&d, "step/lone"), "{d:?}");
    }

    #[test]
    fn fit_inside() {
        let d = static_diags(
            "SELECT 'S1'::GUIDE_STEP; SELECT 'fit'::FIT; SELECT 'S2'::GUIDE_STEP; SELECT 'x'::MARKDOWN;",
        );
        assert!(has(&d, "step/fit-inside"), "{d:?}");
    }

    #[test]
    fn empty_step() {
        let d =
            static_diags("SELECT 'S1'::GUIDE_STEP; SELECT 'S2'::GUIDE_STEP; SELECT 'x'::MARKDOWN;");
        assert!(has(&d, "step/empty"), "{d:?}");
    }

    #[test]
    fn duplicate_label() {
        let d = static_diags(
            "SELECT 'S1'::GUIDE_STEP; SELECT 'x'::MARKDOWN; SELECT 'S1'::GUIDE_STEP; SELECT 'y'::MARKDOWN;",
        );
        assert!(has(&d, "step/duplicate-label"), "{d:?}");
    }

    #[test]
    fn computed_label() {
        let d = static_diags("SELECT a || 'x' ::GUIDE_STEP FROM (VALUES ('p')) t(a);");
        assert!(has(&d, "step/computed-label"), "{d:?}");
    }

    #[test]
    fn var_clash_with_an_input() {
        let d = static_diags(
            "SELECT v AS step ::DROPDOWN FROM (VALUES ('a')) t(v);\nSELECT 'S1'::GUIDE_STEP; SELECT 'S2'::GUIDE_STEP;",
        );
        assert!(has(&d, "step/var-clash"), "{d:?}");
    }

    /// review-537: a `__claim_*` alias is refused as the step variable
    /// (defense in depth alongside `serve.rs`'s own guard) and reported.
    #[test]
    fn var_reserved_claim_alias() {
        let d =
            static_diags("SELECT 'S1' AS __claim_tenant ::GUIDE_STEP; SELECT 'S2'::GUIDE_STEP;");
        assert!(has(&d, "step/var-reserved"), "{d:?}");
    }

    #[test]
    fn var_not_allowed() {
        let sql = "-- @vars other\nSELECT 'S1'::GUIDE_STEP; SELECT 'x'::MARKDOWN;\n\
                   SELECT 'S2'::GUIDE_STEP; SELECT 'y'::MARKDOWN;";
        let d = step_diags(&crate::sql::plan(sql), sql);
        assert!(has(&d, "step/var-not-allowed"), "{d:?}");
    }

    #[test]
    fn var_allowed_reads_clean() {
        let sql = "-- @vars step\nSELECT 'S1'::GUIDE_STEP; SELECT 'x'::MARKDOWN;\n\
                   SELECT 'S2'::GUIDE_STEP; SELECT 'y'::MARKDOWN;";
        let d = step_diags(&crate::sql::plan(sql), sql);
        assert!(!has(&d, "step/var-not-allowed"), "{d:?}");
    }

    #[test]
    fn gate_without_step() {
        let d = static_diags("SELECT true ::GATE, 'x' ::MARKDOWN;");
        assert_eq!(codes(&d), ["step/gate-without-step"], "{d:?}");
    }

    #[test]
    fn gate_on_last_step() {
        let d = static_diags(
            "SELECT 'S1'::GUIDE_STEP; SELECT 'x'::MARKDOWN;\n\
             SELECT 'S2' ::GUIDE_STEP, true ::GATE; SELECT 'y'::MARKDOWN;",
        );
        assert!(has(&d, "step/gate-on-last"), "{d:?}");
    }

    #[test]
    fn gate_constant() {
        let d = static_diags(
            "SELECT 'S1' ::GUIDE_STEP, true ::GATE; SELECT 'x'::MARKDOWN; SELECT 'S2'::GUIDE_STEP; SELECT 'y'::MARKDOWN;",
        );
        assert!(has(&d, "step/gate-constant"), "{d:?}");
    }

    #[test]
    fn gate_unknown_variable() {
        let d = static_diags(
            "SELECT 'S1' ::GUIDE_STEP, getvariable('nope') = 'a' ::GATE;\n\
             SELECT 'x'::MARKDOWN; SELECT 'S2'::GUIDE_STEP; SELECT 'y'::MARKDOWN;",
        );
        assert!(has(&d, "step/gate-unknown-variable"), "{d:?}");
    }

    #[test]
    fn gate_not_boolean() {
        let d = check(
            "SELECT 'S1' ::GUIDE_STEP, 1 ::GATE; SELECT 'x'::MARKDOWN;\n\
             SELECT 'S2'::GUIDE_STEP; SELECT 'y'::MARKDOWN;",
            |q| {
                if q.contains("'S1'") {
                    Ok(vec![json!({ "c0": "S1", "c1": 1 })
                        .as_object()
                        .unwrap()
                        .clone()])
                } else {
                    Ok(vec![json!({ "c0": "x" }).as_object().unwrap().clone()])
                }
            },
        );
        assert!(has(&d, "step/gate-not-boolean"), "{d:?}");
    }

    /// review-537: the canonical pattern — a ::GATE on the same step as its
    /// own quiz (plan decision 2) — must lint clean, not warn, once that
    /// quiz's own `::DROPDOWN` default has bound by the time the step
    /// closes (the next marker). `q2`'s binding only reaches later calls'
    /// `prelude`, so this also exercises that `finalize_pending_gate` is
    /// re-querying with it, not reusing the marker's first (unbound) read.
    #[test]
    fn gate_in_its_own_step_with_its_quiz_is_clean_once_the_step_closes() {
        let sql = "SELECT 'Quiz' ::GUIDE_STEP, getvariable('q2') = 'measured error' ::GATE;\n\
                   SELECT v AS q2 ::DROPDOWN FROM (VALUES ('measured error')) t(v);\n\
                   SELECT 'Next' ::GUIDE_STEP;\nSELECT 'y' ::MARKDOWN;";
        let d = check(sql, |q| {
            let q2_bound = q.contains("SET VARIABLE \"q2\"");
            if q.contains("getvariable('q2')") {
                let v = if q2_bound { json!(true) } else { Value::Null };
                return Ok(vec![json!({ "c0": "Quiz", "c1": v })
                    .as_object()
                    .unwrap()
                    .clone()]);
            }
            if q.contains("VALUES ('measured error')") {
                return Ok(vec![json!({ "q2": "measured error" })
                    .as_object()
                    .unwrap()
                    .clone()]);
            }
            Ok(vec![json!({ "c0": "ok" }).as_object().unwrap().clone()])
        });
        assert!(!has(&d, "step/gate-not-boolean"), "{d:?}");
        assert!(!has(&d, "step/gate-no-row"), "{d:?}");
    }

    /// The same pattern, but the quiz is never answered correctly in this
    /// checker run (the dropdown's own default stays empty/never matches)
    /// — a real BOOLEAN `false`, not a warning: the gate works, it's just
    /// closed, same as #515's `a_known_variable_reads_clean`-style cases.
    #[test]
    fn gate_in_its_own_step_evaluates_a_real_false_not_a_warning() {
        let sql = "SELECT 'Quiz' ::GUIDE_STEP, getvariable('q2') = 'measured error' ::GATE;\n\
                   SELECT v AS q2 ::DROPDOWN FROM (VALUES ('')) t(v);\n\
                   SELECT 'Next' ::GUIDE_STEP;\nSELECT 'y' ::MARKDOWN;";
        let d = check(sql, |q| {
            let q2_bound = q.contains("SET VARIABLE \"q2\"");
            if q.contains("getvariable('q2')") {
                let v = if q2_bound { json!(false) } else { Value::Null };
                return Ok(vec![json!({ "c0": "Quiz", "c1": v })
                    .as_object()
                    .unwrap()
                    .clone()]);
            }
            if q.contains("VALUES ('')") {
                return Ok(vec![json!({ "q2": "" }).as_object().unwrap().clone()]);
            }
            Ok(vec![json!({ "c0": "ok" }).as_object().unwrap().clone()])
        });
        assert!(!has(&d, "step/gate-not-boolean"), "{d:?}");
        assert!(!has(&d, "step/gate-no-row"), "{d:?}");
    }
}
