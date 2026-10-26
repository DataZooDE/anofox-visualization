//! `dashboard` — the viewer. Takes a `.sql` file of annotated
//! queries, runs them through the DuckDB CLI, renders each annotated `SELECT`
//! with the anofox-visualization core (ggplot-rs), and writes an interactive HTML dashboard.
//!
//! Usage:  cargo run --bin dashboard -- dashboards/sessions.sql [out.html]
//!
//! The SQL parsing (`::ROLE` casts, comments, statement splitting) lives in
//! `anofox_visualization::sql`, shared with the wasm browser build.

use anofox_visualization::format::{escape_xml, Locale};
use anofox_visualization::host::app_brand::AppBrand;
use anofox_visualization::host::serving as sv;
use anofox_visualization::inputs;
use anofox_visualization::lint::{self, Severity};
use anofox_visualization::theme::{self, DashboardChrome, Variant};
use anofox_visualization::{
    dashboard, error_pre, page, render_with_title, roles, sql, track, RenderOptions, Role,
};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

/// A statement's own `::KPI_SIZE` cell, read from its already-queried JSON
/// rows (#405) — `Some(true)` 'compact', `Some(false)` 'normal', `None` with
/// no (or an unparseable) value or no `::KPI_SIZE` role at all.
fn kpi_size_json(
    roles: &[(usize, Role)],
    rows: &Result<Vec<serde_json::Map<String, serde_json::Value>>, String>,
) -> Option<bool> {
    let idx = roles.iter().find(|(_, r)| *r == Role::KpiSize)?.0;
    let v = rows
        .as_ref()
        .ok()?
        .first()?
        .get(&format!("c{idx}"))?
        .as_str()?;
    match v.trim().to_ascii_lowercase().as_str() {
        "compact" => Some(true),
        "normal" => Some(false),
        _ => None,
    }
}

const USAGE: &str = "usage:\n  \
     dashboard <file.sql> [out.html] [--theme T] [--dark] [--data <dir>]\n  \
                                             render to interactive HTML (T: a built-in theme,\n  \
                                             'acme+dark', or a theme .json file); --data sets\n  \
                                             getvariable('anofox_data'), as a locked server\n  \
     dashboard --svg <file.sql> [out.svg] [--theme T] [--dark] [--width N] [--data <dir>]\n  \
                                             render the whole dashboard headless to one SVG\n  \
     dashboard --check <file.sql> [--json]   lint: which panels break / are empty\n  \
     dashboard --check <folder> [--json]     …every .sql of a served folder, plus duplicate\n  \
                                             titles and file names the servers skip\n  \
               [--db f --policy policy.json]  …and what an access policy refuses / hides\n  \
               [--data <dir>]                 getvariable('anofox_data') = dir, as a locked server\n  \
     dashboard --site <folder> <out-dir>     publish a served folder as a static DuckDB-Wasm\n  \
               [--data <dir|url>]             site (#417) — a GitHub Pages workflow runs this\n  \
               [--options locked.json]        after a scheduled data refresh. --options accepts\n  \
               [--app <web dir|tarball>]      the locked server's own options file unedited:\n  \
                                             path_vars/nav_picker/app/home/theme are used,\n  \
                                             server-only limits (max_rows, threads, …) are\n  \
                                             ignored (named on stderr), auth/policy still refuse.\n  \
                                             --app is the unpacked or packed\n  \
                                             `anofox-visualization-web-*` release asset (default:\n  \
                                             next to this binary) — never downloaded.\n  \
     dashboard --describe <source> [--db f]  schema + stats (SUMMARIZE)\n  \
     dashboard --roles                       list every valid ::ROLE token\n  \
     dashboard --themes                      list the built-in themes\n  \
     dashboard --theme-css <T> [--dark]      print a theme's CSS design tokens\n  \
     dashboard --version                     print the version\n\n\
     Queries run through the `duckdb` CLI on PATH. The render, --svg and --check\n  \
     each run a whole script in one `duckdb` process (#435), so DuckDB session\n  \
     semantics across statements (SET VARIABLE, TEMP views/tables, and other\n  \
     setup all stay in scope for every later statement) comes for free.";

fn main() {
    // Owner decision (#333): the anofox extensions this binary's `duckdb`
    // child processes may LOAD default to no telemetry. Must run before any
    // thread or `duckdb` child exists — first line of `main`, see
    // `host::telemetry` for the default and its thread-safety reasoning.
    anofox_visualization::host::telemetry::ensure_default_once();
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--check") => cmd_check(&args[1..]),
        Some("--site") => cmd_site(&args[1..]),
        Some("--describe") => cmd_describe(&args[1..]),
        Some("--roles") => print!("{}", anofox_visualization::roles::text()),
        Some("--themes") => cmd_themes(),
        Some("--svg") => cmd_svg(&args[1..]),
        Some("--theme-css") => cmd_theme_css(&args[1..]),
        // The DOCS.md role table (generated from the registry).
        Some("--roles-md") => print!("{}", anofox_visualization::roles::markdown_table()),
        Some("--version") | Some("-V") => {
            println!("dashboard {}", anofox_visualization::BUILD_VERSION)
        }
        // An explicit --help is a successful request (exit 0, stdout); no
        // arguments at all is a usage error (exit 2, stderr).
        Some("--help") | Some("-h") => println!("{USAGE}"),
        None => {
            eprintln!("{USAGE}");
            std::process::exit(2);
        }
        Some(_) => cmd_render(&args),
    }
}

/// Render an annotated `.sql` file to an interactive HTML dashboard.
///
/// `dashboard <file.sql> [out.html] [--theme NAME|file.json] [--dark] [--data <dir>]`:
/// the script's own `::THEME` wins over `--theme`; `--dark` picks the dark
/// variant of a theme that follows the viewer; `--data` sets `anofox_data`
/// (a locked server's `options.data`).
fn cmd_render(args: &[String]) {
    let mut pos: Vec<String> = Vec::new();
    let mut theme_arg: Option<String> = None;
    let mut variant = Variant::Light;
    let mut data: Option<String> = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--theme" => theme_arg = it.next().map(|v| theme_spec_arg(v)),
            "--dark" => variant = Variant::Dark,
            "--data" => data = it.next().cloned(),
            _ => pos.push(a.clone()),
        }
    }
    let Some(path) = pos.first().cloned() else {
        eprintln!("{USAGE}");
        std::process::exit(2);
    };
    let out = pos
        .get(1)
        .cloned()
        .unwrap_or_else(|| "dashboard.html".to_string());
    let script = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    let asset_root = dashboard_assets_root(Path::new(&path));

    let (html, n) = render_dashboard_html(
        &script,
        &path,
        data.as_deref(),
        theme_arg.as_deref(),
        variant,
        asset_root.as_deref(),
    );
    std::fs::write(&out, html).unwrap_or_else(|e| panic!("write {out}: {e}"));
    println!("wrote {out} ({n} panels) — open it in a browser");
}

/// Render one annotated dashboard script to a self-contained HTML page, in
/// one DuckDB session: every statement runs with everything before it
/// (setup DDL, `SET VARIABLE`, `LOAD`, TEMP views/tables, input defaults)
/// still in scope, in script order — the same session semantics as the
/// locked server's cold path and `dashboard --check`. #435: a single
/// persistent `duckdb` CLI child (`SessionRunner`) backs this, fed one
/// statement at a time — not a fresh process per statement replaying a
/// growing prelude of everything run so far (#414's original fix for this
/// session semantics, which worked but cost O(panels × setup): a setup
/// statement that reads a large file was re-run for every later panel).
/// An ordinary SQL error on one statement does not disturb the session —
/// later panels still see everything genuine setup before it left in
/// scope, and still render.
///
/// `data`: `anofox_data`, as `--data <dir>` / a locked server's
/// `options.data` set it — applied before any statement of the script runs,
/// so `-- @requires ${anofox_data}/…` and `getvariable('anofox_data')`
/// panels see it from the first statement.
///
/// `path_label` is cosmetic (the page's "rendered from <code>…</code>"
/// subtitle) — pass the dashboard's file name for a single-file render, or
/// its id for a folder/site render (#417), which can call this once per
/// dashboard.
///
/// `asset_root` (#514 PR B): the dashboard's own `assets/` folder,
/// canonicalized — a relative `::IMAGE`/`::IMAGE_DARK` source is inlined
/// from it as a `data:` URI; `None` (no such folder) draws the alt-text
/// placeholder for every relative source, as before PR B.
///
/// Returns the HTML and the number of rendered panels.
fn render_dashboard_html(
    script: &str,
    path_label: &str,
    data: Option<&str>,
    theme_arg: Option<&str>,
    variant: Variant,
    asset_root: Option<&Path>,
) -> (String, usize) {
    // Phase 1: run the script in order; collect chrome directives and the
    // panels' rows (a ::THEME anywhere applies to every panel).
    let mut chrome = DashboardChrome::default();
    type Rows = Result<Vec<serde_json::Map<String, serde_json::Value>>, String>;
    // The third element is each panel's own `::KPI_SIZE` ('compact') (#405).
    let mut queried: Vec<(sql::Panel, Rows, bool)> = Vec::new();
    let mut session = anofox_visualization::host::duckdb_cli::SessionRunner::spawn(None)
        .unwrap_or_else(|e| {
            eprintln!("error: {e}");
            std::process::exit(1);
        });
    let mut exec = |sql: &str| session.run(sql);
    // `SET VARIABLE anofox_data = …` opens the session, before the script's
    // own statements — as `--data` does for `--check` and a locked server.
    if let Some(dir) = data {
        let _ = exec(&data_set_stmt(dir));
    }
    let mut sections = sql::requires::Sections::default();
    // A static HTML render shows only a ::VIEW group's first shown
    // alternative (#214) — the other alternatives' non-setup statements
    // never even run.
    let mut view_pick = sql::views::StaticPick::default();
    // The enclosing ::GROUP's own ::KPI_SIZE (#405) — a tile's own wins.
    let mut group_compact = false;
    // `::SHOW_IF` (#515): each condition's result, by plan index — a
    // condition always precedes what it governs (`sql::show_if::assign`), so
    // by the time a later statement's `show_if` is checked below, its
    // conditions were already run and cached here.
    let mut show_if_cache: std::collections::HashMap<usize, bool> =
        std::collections::HashMap::new();
    // `::GUIDE_STEP` (#516): each group's running count of shown markers —
    // see `dashboard::execute`'s identical `step_n` for why "of N" can't be
    // filled in here (phase 2 does it, once every step is collected).
    let mut step_n: std::collections::HashMap<usize, u32> = std::collections::HashMap::new();
    for (i, p) in sql::plan(script).into_iter().enumerate() {
        // A ::TAB / ::VIEW alternative whose `-- @requires` input is missing
        // is hidden: none of its panels are drawn (the tabs themselves are
        // flattened here).
        // `exec` is backed by a persistent session (#435), so there is no
        // prelude text to replay — it already has everything in scope.
        let hidden = sections.step(&p, |reqs| {
            lint::unmet_requirement(&mut exec, "", reqs).missing
        });
        if hidden.is_some() || view_pick.drop_panel(&p) {
            continue;
        }
        if !p.setup
            && !p.show_if.is_empty()
            && !p
                .show_if
                .iter()
                .all(|c| show_if_cache.get(c).copied().unwrap_or(false))
        {
            continue;
        }
        if p.roles.len() == 1 && matches!(p.roles[0].1, Role::ShowIf) {
            let v = exec(&format!("{};", p.sql))
                .ok()
                .and_then(|rows| rows.first().and_then(|r| r.values().next().cloned()));
            show_if_cache.insert(i, sql::condition::holds_json(v.as_ref()));
            continue;
        }
        if p.setup {
            // Genuine setup (DDL / SET / TEMP views and tables / …): it
            // already ran in the session `exec` backs, so it's already in
            // scope for every later statement — there's no prelude text to
            // grow and replay (#435).
            if let Err(e) = exec(&format!("{};", p.sql)) {
                eprintln!("duckdb error on: {}\n{e}", p.sql);
            }
            continue;
        }
        // An input renders nothing in the static output, but binds its default
        // (as the browser does on first load) for the getvariable() panels.
        if let Some(kind) = inputs::input_kind(&p.roles) {
            if let Ok(rows) = exec(&format!("{};", p.sql)) {
                // #435: sent to the session right away instead of a
                // prelude text to replay before later statements.
                for set in inputs::input_defaults(kind, inputs::input_index(&p.roles), &rows) {
                    let _ = exec(&format!("{set};"));
                }
            }
            continue;
        }
        if let Some(r) = p
            .roles
            .iter()
            .map(|(_, r)| *r)
            .find(|r| roles::is_chrome_directive(*r))
        {
            let rows = exec(&format!("{};", p.sql)).unwrap_or_default();
            let cell = |v: &serde_json::Value| match v {
                serde_json::Value::String(s) => Some(s.clone()),
                serde_json::Value::Null => None,
                v => Some(v.to_string()),
            };
            // The first row's cells (an ::ASOF also reads its 'stale=…' cell).
            let cells = rows
                .first()
                .map(|r| r.values().map(cell).collect())
                .unwrap_or_default();
            chrome.apply_row(r, cells);
            continue;
        }
        // ::GROUP / ::ENDGROUP (#405): no box here (the static CLI output
        // is one figure per panel, no grid), but a ::GROUP's own
        // ::KPI_SIZE still cascades onto its tiles' own cards.
        if p.roles.iter().any(|(_, r)| *r == Role::GroupStart) {
            group_compact = if p.roles.iter().any(|(_, r)| *r == Role::KpiSize) {
                let rows = exec(&format!("{};", p.sql));
                kpi_size_json(&p.roles, &rows).unwrap_or(false)
            } else {
                false
            };
            continue;
        }
        if p.roles.iter().any(|(_, r)| *r == Role::GroupEnd) {
            group_compact = false;
            continue;
        }
        let section = p.roles.iter().any(|(_, r)| *r == Role::Section);
        let step_marker = p.step.as_ref().is_some_and(|s| s.marker);
        // `::GUIDE_STEP` (#516, decision 8): bind this step's 1-based
        // number (among the ones shown so far) before its own panels run,
        // exactly like an input's default (#435: sent to the session right
        // away, no prelude text to replay).
        if step_marker {
            let s = p.step.as_ref().unwrap();
            let n = step_n.entry(s.group).or_insert(0);
            *n += 1;
            let _ = exec(&format!("SET VARIABLE {} = {n};", s.var));
        }
        // Layout directives are interactive — the browser builder / `serve`
        // handle them; the static CLI output skips them (role registry).
        if roles::is_directive_panel(&p.roles) && !section && !step_marker {
            continue;
        }
        let rows = exec(&format!("{};", p.sql));
        let compact = kpi_size_json(&p.roles, &rows).unwrap_or(group_compact);
        queried.push((p, rows, compact));
    }

    let set = chrome.resolve(theme_arg).unwrap_or_else(|e| {
        eprintln!("theme: {e}");
        std::process::exit(2);
    });
    let t = set.pick(variant).clone();
    let opts = RenderOptions {
        locale: t
            .locale
            .as_deref()
            .and_then(Locale::from_tag)
            .unwrap_or_default(),
        theme: Arc::new(t.clone()),
        asset_root: asset_root.map(Path::to_path_buf),
        ..RenderOptions::default()
    };

    // #536: this dashboard's own `-- @markdown_style` default, read once —
    // every ::MARKDOWN panel without its own ::MARKDOWN_STYLE (and without
    // ::CALLOUT, which always draws 'callout') falls back to it. Static
    // output draws the chosen chrome through the same CSS classes the
    // browser uses (web/theme-base.css, embedded by `page::render` below).
    let md_default = sv::parse_dashboard_meta("", script).markdown_style;
    // Phase 2: render.
    let mut panels = String::new();
    let mut n = 0;
    // `::GUIDE_STEP` (#516): `queried` only ever holds what phase 1 kept
    // shown, so its total marker count per group ("of N") is known now.
    let mut static_steps = {
        let shown: Vec<&sql::Panel> = queried.iter().map(|(p, _, _)| p).collect();
        sql::steps::StaticSteps::new(&shown)
    };
    let mut queried = queried.into_iter().peekable();
    while let Some((p, rows, compact)) = queried.next() {
        let rows = match rows {
            Ok(r) => r,
            Err(e) => {
                panels.push_str(&format!(
                    "<figure class=\"panel\">{}</figure>",
                    error_pre(&e)
                ));
                n += 1;
                continue;
            }
        };
        // A contract plot call: one figure per result row (GROUP BY key).
        if let Some(cp) = &p.contract {
            for (_, r) in sql::contract_call::render_rows_opts(cp, &rows, 460, 300, &opts) {
                let svg = r.unwrap_or_else(|e| error_pre(&format!("error: {e}")));
                panels.push_str(&format!("<figure class=\"panel\">{svg}</figure>"));
                n += 1;
            }
            continue;
        }
        // A label-only panel / ::SECTION is a spanning section heading, not
        // a card — so is a ::GUIDE_STEP marker (#516 decision 8: the static
        // output flattens the stepper into a run of these headings).
        let section = p.roles.iter().any(|(_, r)| *r == Role::Section);
        let step_n_of = p.step.as_ref().filter(|s| s.marker).and_then(|_| {
            static_steps.step(&p).into_iter().find_map(|e| match e {
                sql::steps::StaticEvent::Heading { n, of, .. } => Some((n, of)),
                _ => None,
            })
        });
        if section
            || (p.roles.len() == 1 && matches!(p.roles[0].1, Role::Label))
            || step_n_of.is_some()
        {
            let text = match step_n_of {
                Some((n, of)) => {
                    let idx = p
                        .roles
                        .iter()
                        .find(|(_, r)| *r == Role::GuideStep)
                        .map(|(i, _)| *i);
                    let label = idx
                        .and_then(|i| rows.first().and_then(|r| r.get(&format!("c{i}"))))
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    if label.is_empty() {
                        format!("Step {n} of {of}")
                    } else {
                        format!("Step {n} of {of} · {label}")
                    }
                }
                None => rows
                    .first()
                    .and_then(|r| r.values().next())
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
            };
            panels.push_str(&format!("<h2 class=\"section\">{}</h2>", escape_xml(&text)));
            continue;
        }
        let mut cols = anofox_visualization::name_columns(
            sql::columns_from_rows(&rows, &p.roles),
            &p.roles,
            &p.names,
        );
        // A ::GROUP's ::KPI_SIZE cascading onto this tile (#405): a synthetic
        // column, since this isn't a tile's own cast — kpi::Kpi::from_columns
        // reads the first ::KPI_SIZE column it finds, own one first.
        if compact {
            cols.push(anofox_visualization::Column::new(
                "_kpi_size",
                Role::KpiSize,
                vec![anofox_visualization::Value::Str("compact".into())],
            ));
        }
        // ::TRACKs right after a chart sit under it, in its figure.
        let mut tracks = Vec::new();
        while let Some((q, r, _)) = queried.next_if(|(q, _, _)| track::is_track(&q.roles)) {
            if let Ok(r) = r {
                tracks.push((sql::columns_from_rows(&r, &q.roles), 0));
            }
        }
        // A ::TABLE / ::PAGED panel (#427): its rewrite keeps each column's
        // own name rather than aliasing it to `cN` (`sql::rewrite_list`'s
        // `keep_intact`), so the `cols` built above (indexed by `cN`) can't
        // see most of its columns — the same raw, name-keyed rows `--svg`
        // and `serve` already read instead, through the shared table
        // renderer (`dashboard::render_panel_doc_svg`).
        let svg = if p
            .roles
            .iter()
            .any(|(_, r)| matches!(r, Role::Table | Role::PagedTable))
        {
            let named_rows = sql::result_columns(&rows);
            dashboard::render_panel_doc_svg(
                &named_rows,
                &p.roles,
                &p.names,
                460.0,
                300.0,
                &opts,
                false,
                &tracks,
                None,
                compact,
            )
        } else {
            // Its ::TITLE bar, as on a dashboard card (#232).
            render_with_title(&cols, &tracks, 460, 300, &opts)
                .unwrap_or_else(|e| error_pre(&format!("error: {e}")))
        };
        // #536: a ::MARKDOWN panel's chrome — same resolution as the
        // browser's resolveMdStyle(), drawn through the same CSS classes
        // (web/theme-base.css, embedded in this page by `page::render`).
        let text_of = |role: Role| -> Option<String> {
            let (i, _) = p.roles.iter().find(|(_, r)| *r == role)?;
            rows.first()?
                .get(&format!("c{i}"))
                .and_then(|v| v.as_str())
                .map(str::to_string)
        };
        let (class, icon_attrs) = if p.roles.iter().any(|(_, r)| matches!(r, Role::Markdown)) {
            let explicit = text_of(Role::MarkdownStyle);
            let callout_present = p.roles.iter().any(|(_, r)| matches!(r, Role::Callout));
            let style = roles::resolve_markdown_style(
                explicit.as_deref(),
                callout_present,
                md_default.as_deref(),
            );
            match style {
                "plain" => ("panel md-plain".to_string(), String::new()),
                "callout" if callout_present => {
                    let kind = roles::callout_kind(&text_of(Role::Callout).unwrap_or_default());
                    let icon = match kind {
                        "warning" => "⚠",
                        "success" => "✓",
                        _ => "ℹ",
                    };
                    (
                        format!("panel md-box dp-callout dp-callout-{kind}"),
                        format!(" role=\"note\" data-icon=\"{icon}\""),
                    )
                }
                "callout" => ("panel md-box".to_string(), String::new()),
                _ => ("panel".to_string(), String::new()), // 'card'
            }
        } else {
            ("panel".to_string(), String::new())
        };
        panels.push_str(&format!(
            "<figure class=\"{class}\"{icon_attrs}>{svg}</figure>"
        ));
        n += 1;
    }

    let html = page::render(&page::StaticPage {
        theme: &t,
        fallback_title: "anofox-visualization dashboard",
        subtitle_html: format!(
            "rendered from <code>{}</code> · {n} panels",
            escape_xml(path_label)
        ),
        head_extra: String::new(),
        controls_html: String::new(),
        panels_html: panels,
        fit: page::static_fit(&sql::plan(script)),
    });
    (html, n)
}

/// The DuckDB CLI as a headless [`dashboard::DataProvider`]: #435, one
/// persistent `duckdb :memory:` child (`SessionRunner`), fed one statement
/// at a time — a real connection's session state (`SET VARIABLE`, `LOAD`,
/// TEMP views/tables, any other setup) just carries forward on its own, so
/// a `::TEMP VIEW`/`SET VARIABLE` a panel depends on is still in scope
/// however many statements separate them, without replaying anything.
struct CliProvider {
    session: anofox_visualization::host::duckdb_cli::SessionRunner,
}

impl CliProvider {
    /// `sql`'s real DuckDB column types (`DESCRIBE`, as a round in the same
    /// session — #435, no prelude replay needed) — `name → column_type`
    /// (`"VARCHAR"`, `"DECIMAL(10,2)"`, …). Empty on any failure (a
    /// provider-level detail, not a panel error): `query` then falls back
    /// to `result_columns_typed`'s old per-value numeric guess, same as
    /// before #424 fixed it for the common case.
    fn column_types(&mut self, sql: &str) -> std::collections::HashMap<String, String> {
        let describe = format!("DESCRIBE {};", sql.trim().trim_end_matches(';'));
        let Ok(rows) = self.session.run(&describe) else {
            return std::collections::HashMap::new();
        };
        rows.into_iter()
            .filter_map(|r| {
                let name = r.get("column_name")?.as_str()?.to_string();
                let ty = r.get("column_type")?.as_str()?.to_string();
                Some((name, ty))
            })
            .collect()
    }
}

impl dashboard::DataProvider for CliProvider {
    fn execute(&mut self, sql: &str) -> Result<(), String> {
        self.session.run(&format!("{sql};"))?;
        Ok(())
    }
    fn query(
        &mut self,
        sql: &str,
    ) -> Result<Vec<(String, Vec<anofox_visualization::Value>)>, String> {
        // #424: a VARCHAR column whose values happen to parse as numbers
        // (binary-classifier labels `'0'`/`'1'`) must stay discrete — ask
        // DuckDB's own types rather than guessing from the JSON text.
        // #435: both rounds run in the same session, no prelude replay.
        let types = self.column_types(sql);
        let rows = self.session.run(&format!("{sql};"))?;
        Ok(sql::result_columns_typed(&rows, &types))
    }
}

/// `sql_path`'s own `assets/` folder (its parent directory's `assets/`),
/// canonicalized — `None` when there is no such folder next to it. Shared
/// by `cmd_svg` and `cmd_render` ([`render_dashboard_html`]'s `asset_root`
/// argument). `cmd_check`'s `image_root` and `cmd_site`'s asset copy use
/// [`anofox_visualization::image::canonical_assets_dir`] directly — they
/// already have the dashboards *folder*, not a single `.sql` file's path.
fn dashboard_assets_root(sql_path: &Path) -> Option<PathBuf> {
    sql_path
        .parent()
        .and_then(anofox_visualization::image::canonical_assets_dir)
}

/// `dashboard --svg <file.sql> [out.svg] [--theme T] [--dark] [--width N] [--data <dir>]`:
/// the whole dashboard (KPIs, tables, charts, header) as one SVG.
fn cmd_svg(args: &[String]) {
    let mut pos: Vec<String> = Vec::new();
    let mut o = dashboard::DashboardOptions::default();
    let mut data: Option<String> = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--theme" => o.theme = it.next().map(|v| theme_spec_arg(v)),
            "--dark" => o.variant = Variant::Dark,
            "--width" => o.width = it.next().and_then(|v| v.parse().ok()).unwrap_or(o.width),
            "--data" => data = it.next().cloned(),
            _ => pos.push(a.clone()),
        }
    }
    let Some(path) = pos.first().cloned() else {
        eprintln!("{USAGE}");
        std::process::exit(2);
    };
    let out = pos
        .get(1)
        .cloned()
        .unwrap_or_else(|| "dashboard.svg".into());
    let script = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    // #514 PR B: a relative `::IMAGE` source means `assets/` next to the
    // .sql file; missing (no such folder) is not an error — every relative
    // source just draws its placeholder, as before PR B.
    o.asset_root = dashboard_assets_root(Path::new(&path));
    let session = anofox_visualization::host::duckdb_cli::SessionRunner::spawn(None)
        .unwrap_or_else(|e| {
            eprintln!("error: {e}");
            std::process::exit(1);
        });
    let mut p = CliProvider { session };
    // `anofox_data` opens the session, before the script's own statements.
    if let Some(dir) = data.as_deref() {
        if let Err(e) = p.session.run(&data_set_stmt(dir)) {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    }
    let svg = dashboard::render_dashboard_svg(&script, &mut p, &o);
    match svg {
        Ok(svg) => {
            std::fs::write(&out, svg).unwrap_or_else(|e| panic!("write {out}: {e}"));
            println!("wrote {out}");
        }
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    }
}

/// `--theme` value: a `.json` path is read (a theme definition file),
/// anything else is a theme spec (`dark`, `acme+dark`, inline JSON).
fn theme_spec_arg(v: &str) -> String {
    if v.ends_with(".json") && std::path::Path::new(v).exists() {
        std::fs::read_to_string(v).unwrap_or_else(|e| {
            eprintln!("read {v}: {e}");
            std::process::exit(2);
        })
    } else {
        v.to_string()
    }
}

/// `dashboard --theme-css <spec> [--dark]`: print the theme's CSS token block.
fn cmd_theme_css(args: &[String]) {
    let spec = args.first().map(|v| theme_spec_arg(v)).unwrap_or_default();
    let dark = args.iter().any(|a| a == "--dark");
    match theme::resolve(&spec) {
        Ok(set) => println!(
            "{}",
            set.pick(if dark { Variant::Dark } else { Variant::Light })
                .css_vars()
        ),
        Err(e) => {
            eprintln!("theme: {e}");
            std::process::exit(2);
        }
    }
}

/// `dashboard --themes`: the built-in themes.
fn cmd_themes() {
    for (name, about) in theme::BUILTINS {
        let set = theme::resolve(name).expect("built-in");
        let sw: Vec<String> = set
            .default_variant()
            .palette
            .iter()
            .take(6)
            .map(|c| theme::hex(*c))
            .collect();
        println!(
            "{name:<14} {about}\n               palette {}  mode {}",
            sw.join(" "),
            set.mode.as_str()
        );
    }
}

/// Lint a dashboard: report panels that error, render wrong, are silently
/// dropped to setup, or return no rows. Exit 1 if there are errors. A folder
/// (a served folder) lints each `.sql` in it, plus the folder checks
/// (`lint::folder_diags`: a duplicate `-- @title`, a file name the servers
/// skip).
fn cmd_check(args: &[String]) {
    let json_out = args.iter().any(|a| a == "--json");
    // --db <file>: validate against a prebuilt (read-only) database — its views
    // and tables are already in scope, so the dashboards are pure queries.
    let mut db: Option<String> = None;
    // --policy <file.json>: also check the dashboard against an access policy
    // (needs --db: the policy is checked against that database's tables).
    let mut policy: Option<String> = None;
    let mut path = None;
    let mut design = true; // --no-design turns off the design/* advisory pass
    let mut strict = false; // --strict makes design/* warnings fail the exit code
                            // --data <dir>: the data root a locked server sets as anofox_data.
    let mut data: Option<String> = None;
    // --options locked.json (#517 S2): the locked server's own options
    // file, for submit/not-enabled and state/not-enabled — whether this
    // dashboard's id is actually named in that file's options.state.
    let mut options: Option<String> = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--db" => db = it.next().cloned(),
            "--data" => data = it.next().cloned(),
            "--policy" => policy = it.next().cloned(),
            "--options" => options = it.next().cloned(),
            "--json" => {}
            "--no-design" => design = false,
            "--strict" => strict = true,
            _ if !a.starts_with("--") => path = Some(a.clone()),
            _ => {}
        }
    }
    let Some(path) = path else {
        eprintln!(
            "usage: dashboard --check <file.sql | folder> [--json] [--db prebuilt.duckdb] \
             [--policy policy.json] [--data dir] [--options locked.json] [--strict] [--no-design]"
        );
        std::process::exit(2);
    };
    if policy.is_some() && db.is_none() {
        eprintln!("--policy needs --db <the database the server snapshots>");
        std::process::exit(2);
    }
    let read = |p: &str| {
        std::fs::read_to_string(p).unwrap_or_else(|e| {
            eprintln!("read {p}: {e}");
            std::process::exit(2);
        })
    };
    // `SET VARIABLE anofox_data = '<dir>'` opens every session (as on a
    // locked server, before the dashboard's own statements).
    let data_set = data.as_deref().map(data_set_stmt).unwrap_or_default();
    // (submit-enabled dashboards, state-enabled dashboards) from
    // `--options`'s `options.state`, or `None` when `--options` wasn't
    // given — submit/not-enabled and state/not-enabled are skipped
    // entirely then (plan §3: "With --options locked.json").
    let state_enablement: Option<(Vec<String>, Vec<String>)> = options.as_deref().map(|p| {
        let json = read(p);
        let v: serde_json::Value = serde_json::from_str(&json).unwrap_or_else(|e| {
            eprintln!("--options: bad JSON: {e}");
            std::process::exit(2);
        });
        let m = v.as_object().unwrap_or_else(|| {
            eprintln!("--options must be a JSON object");
            std::process::exit(2);
        });
        match m.get("state") {
            Some(state_val) => {
                let parsed = anofox_visualization::host::state::StateOptions::parse(state_val)
                    .unwrap_or_else(|e| {
                        eprintln!("--options: state: {e}");
                        std::process::exit(2);
                    });
                let state_enabled: Vec<String> = parsed
                    .named_dashboards()
                    .into_iter()
                    .map(str::to_string)
                    .collect();
                (parsed.dashboards.clone(), state_enabled)
            }
            None => (Vec::new(), Vec::new()),
        }
    });
    let folder = std::path::Path::new(&path).is_dir();
    // #514 PR B: `image/missing-file` needs the dashboard's own folder —
    // `--check <file.sql>` uses its directory, `--check <folder>` the
    // folder itself (plan-514's own rule, mirrored by `--site` below).
    let image_root = if folder {
        anofox_visualization::image::canonical_assets_dir(Path::new(&path))
    } else {
        dashboard_assets_root(Path::new(&path))
    };
    let lint_one = |script: &str| -> Vec<lint::Diagnostic> {
        // #435: one persistent session per file (in-memory, or read-only
        // against --db) backs the whole check — not a fresh process per
        // statement replaying everything run so far. A folder's dashboards
        // each get their own fresh session (one per `lint_one` call), same
        // isolation the old per-statement replay gave them implicitly.
        let mut session =
            anofox_visualization::host::duckdb_cli::SessionRunner::spawn(db.as_deref())
                .unwrap_or_else(|e| {
                    eprintln!("error: {e}");
                    std::process::exit(1);
                });
        if !data_set.is_empty() {
            let _ = session.run(&data_set);
        }
        let mut diags = lint::check_opts(
            script,
            |sql| session.run(sql),
            lint::LintOptions {
                design,
                stateful_session: true,
                image_root: image_root.clone(),
            },
        );
        if let (Some(p), Some(db)) = (&policy, db.as_deref()) {
            diags.extend(policy_diags(script, db, p));
        }
        diags
    };
    // (file, its diagnostics): one entry for a file, one per .sql for a folder
    let mut files: Vec<(String, Vec<lint::Diagnostic>)> = Vec::new();
    if folder {
        let mut names: Vec<String> = std::fs::read_dir(&path)
            .unwrap_or_else(|e| {
                eprintln!("read {path}: {e}");
                std::process::exit(2);
            })
            .flatten()
            .filter_map(|e| e.file_name().to_str().map(str::to_string))
            .filter(|n| n.ends_with(".sql"))
            .collect();
        names.sort();
        if names.is_empty() {
            eprintln!("no .sql dashboards in {path}");
            std::process::exit(2);
        }
        let scripts: Vec<(String, String)> = names
            .iter()
            .map(|n| {
                (
                    n.clone(),
                    read(&format!("{}/{n}", path.trim_end_matches('/'))),
                )
            })
            .collect();
        let extra = lint::folder_diags(&scripts);
        for (n, script) in &scripts {
            let mut d = lint_one(script);
            d.extend(extra.iter().filter(|(f, _)| f == n).map(|(_, d)| d.clone()));
            // The exact id the running server's own loader would derive
            // for this file (review of #538 round 2: reuse
            // `dashboard_id_from_path`, not a hand-rolled ".sql" trim —
            // `str::trim_end_matches` repeats, `Path::file_stem` doesn't,
            // so they disagree on a name like "d.sql.sql"). `None`: the
            // server would skip this file outright (not even load it as
            // a dashboard), so there is no submit/state enablement to
            // report either way.
            if let (Some((submit_ok, state_ok)), Some(id)) =
                (&state_enablement, sv::dashboard_id_from_path(Path::new(n)))
            {
                d.extend(lint::state_enablement_diags(
                    script, &id, submit_ok, state_ok,
                ));
            }
            files.push((n.clone(), d));
        }
    } else {
        let script = read(&path);
        let mut d = lint_one(&script);
        if let (Some((submit_ok, state_ok)), Some(id)) = (
            &state_enablement,
            sv::dashboard_id_from_path(Path::new(&path)),
        ) {
            d.extend(lint::state_enablement_diags(
                &script, &id, submit_ok, state_ok,
            ));
        }
        files.push((path.clone(), d));
    }

    let all = || files.iter().flat_map(|(_, d)| d.iter());
    // --strict: a design/* advisory or a requires/* warning also fails the
    // exit code (for CI gating).
    let failed = !files.iter().all(|(_, d)| lint::passes(d, strict));
    let json = |diags: &[lint::Diagnostic]| -> String {
        let items: Vec<String> = diags
            .iter()
            .map(|d| {
                format!(
                    "{{\"severity\":{:?},\"stmt\":{},\"code\":{:?},\"message\":{:?},\"sql\":{:?}}}",
                    d.severity.label(),
                    d.stmt,
                    d.code,
                    d.message,
                    d.sql
                )
            })
            .collect();
        items.join(",")
    };
    if json_out {
        if folder {
            let per: Vec<String> = files
                .iter()
                .map(|(f, d)| format!("{{\"file\":{f:?},\"diagnostics\":[{}]}}", json(d)))
                .collect();
            println!("{{\"ok\":{},\"files\":[{}]}}", !failed, per.join(","));
        } else {
            println!(
                "{{\"ok\":{},\"diagnostics\":[{}]}}",
                !failed,
                json(&files[0].1)
            );
        }
    } else {
        for (f, diags) in &files {
            if folder {
                println!("== {f}: {}", lint::summary(diags));
            }
            for d in diags {
                println!(
                    "{}: [stmt {}] {} — {}\n    {}",
                    d.severity.label(),
                    d.stmt,
                    d.code,
                    d.message,
                    d.sql
                );
            }
        }
        let flat: Vec<lint::Diagnostic> = all().cloned().collect();
        println!("\n{}", lint::summary(&flat));
    }
    if failed {
        std::process::exit(1);
    }
}

/// `--check --policy`: compile the policy against `db` (as the locked server
/// does against its snapshot) and lint the script with it.
fn policy_diags(script: &str, db: &str, path: &str) -> Vec<lint::Diagnostic> {
    use anofox_visualization::host::policy::{Enforcer, Policy};
    let fail = |message: String| {
        vec![lint::Diagnostic {
            severity: Severity::Error,
            stmt: 0,
            code: "policy/invalid",
            message,
            sql: String::new(),
        }]
    };
    let policy = match std::fs::read_to_string(path)
        .map_err(|e| format!("read {path}: {e}"))
        .and_then(|t| Policy::parse(&t))
    {
        Ok(p) => p,
        Err(e) => return fail(e),
    };
    let mut duck = |sql: &str| -> Result<Vec<Vec<Option<String>>>, String> {
        Ok(run_query(Some(db), sql)?
            .into_iter()
            .map(|r| {
                r.into_values()
                    .map(|v| match v {
                        serde_json::Value::Null => None,
                        serde_json::Value::String(s) => Some(s),
                        other => Some(other.to_string()),
                    })
                    .collect()
            })
            .collect())
    };
    // The function list was reviewed against the pinned DuckDB; another
    // version parses (and lists macros) its own way.
    let mut out = Vec::new();
    if let Ok(v) = duck("SELECT version()::VARCHAR") {
        let v = v
            .into_iter()
            .next()
            .and_then(|r| r.into_iter().next().flatten())
            .unwrap_or_default();
        if v != anofox_visualization::host::policy::PINNED_DUCKDB {
            out.push(lint::Diagnostic {
                severity: Severity::Warning,
                stmt: 0,
                code: "policy/duckdb-version",
                message: format!(
                    "the duckdb CLI on PATH is {v}, the server's is {}: parse trees and the built-in macros can differ (the server checks again at start)",
                    anofox_visualization::host::policy::PINNED_DUCKDB
                ),
                sql: String::new(),
            });
        }
    }
    let default_db = match duck("SELECT current_database()::VARCHAR") {
        Ok(r) => r
            .into_iter()
            .next()
            .and_then(|r| r.into_iter().next().flatten())
            .unwrap_or_default(),
        Err(e) => return fail(e),
    };
    match Enforcer::compile(&policy, &mut duck, &default_db) {
        Ok(enf) => out.extend(lint::check_policy(script, &enf, &mut duck)),
        Err(e) => out.extend(fail(e)),
    }
    out
}

// ------------------------------------------------------------- --site ---
// #417: turn a served folder (the one `anofox_serve_dashboards(dir, …)`
// would open) into a static site that runs in DuckDB-Wasm — no server, no
// patches by the publisher. Reuses the locked server's own rules
// (`host::serving::{read_only_statement, nav_picker_rows_to_options, …}`,
// `DashboardMeta::nav_json`) through the `duckdb` CLI (this binary has no
// embedded DuckDB), so a published site never accepts a folder, or shows a
// `nav_picker` option, the locked server itself would refuse.

/// A loaded dashboard, enough of `duckext::serve::Dash` for `--site`: no
/// `requires`/`tabs`/`info` (those drive live serving, not the static
/// export — the browser plans the script itself, as the builder does).
struct SiteDash {
    id: String,
    meta: sv::DashboardMeta,
    sql: String,
    /// `(is a setup statement, its SQL)` — [`sql::plan`]'s own split.
    stmts: Vec<(bool, String)>,
}

/// Load every `*.sql` of `dir`, sorted the same way the locked server does
/// (`-- @order`, then id — [`sv::DashboardMeta::order_key`]), so `site.json`'s
/// `dashboards` array is already in nav order.
fn load_site_dashboards(dir: &Path) -> Result<Vec<SiteDash>, String> {
    let entries = std::fs::read_dir(dir)
        .map_err(|e| format!("cannot read dashboard folder '{}': {e}", dir.display()))?;
    let mut out = Vec::new();
    for e in entries.flatten() {
        let p = e.path();
        if p.extension().and_then(|s| s.to_str()) != Some("sql") {
            continue;
        }
        let Some(id) = p.file_stem().and_then(|s| s.to_str()).map(str::to_string) else {
            continue;
        };
        if !sv::valid_dashboard_id(&id) {
            continue; // ids end up in URLs: only [A-Za-z0-9_.-], as the server requires
        }
        let sql =
            std::fs::read_to_string(&p).map_err(|e| format!("cannot read {}: {e}", p.display()))?;
        let meta = sv::parse_dashboard_meta(&id, &sql);
        let stmts = sql::plan(&sql)
            .into_iter()
            .map(|p| (p.setup, p.sql.trim().to_string()))
            .collect();
        out.push(SiteDash {
            id,
            meta,
            sql,
            stmts,
        });
    }
    if out.is_empty() {
        return Err(format!("no .sql dashboards found in '{}'", dir.display()));
    }
    out.sort_by(|a, b| a.meta.order_key(&a.id).cmp(&b.meta.order_key(&b.id)));
    Ok(out)
}

/// `options.nav_picker` for `--site` — the same shape as
/// `LockedOptions.nav_picker` (`duckext/src/serve.rs`), parsed again here
/// because the full `LockedOptions` also carries live-serving-only state
/// (`auth`, `policy`, a `Conn`) this binary has no use for.
#[derive(Debug)]
struct SiteNavPicker {
    var: String,
    sql: String,
    label: Option<String>,
}

/// `--options locked.json` for `--site`: the subset of `anofox_serve_dashboards`'s
/// options that make sense for a site with no server behind it (no `auth`,
/// no `policy` — the plan's own parity point: every query the browser runs
/// is the author's, exactly as in the builder; see docs/secure-serving.md,
/// "Publish as a static site"). `--site` also accepts the locked server's
/// own `options` file unedited (#501): a server-only limit such as
/// `max_rows` is harmless here (it has no effect without a server) and ends
/// up in `ignored` instead of refusing the whole file.
#[derive(Default, Debug)]
struct SiteOptions {
    path_vars: Vec<String>,
    nav_picker: Option<SiteNavPicker>,
    app: Option<serde_json::Value>,
    home: Option<String>,
    theme: Option<String>,
    /// The same cap the locked server would apply to its own `GET
    /// /d/<id>/assets/<rest>` route (review of #554 N3) — `None` when
    /// unset, so `cmd_site` falls back to
    /// [`anofox_visualization::image::DEFAULT_MAX_ASSET_BYTES`] exactly as
    /// `LockedOptions::parse`'s own default does.
    max_asset_bytes: Option<u64>,
    /// Server-only keys this file had, that `--site` skipped — named once
    /// on stderr by the caller ([`cmd_site`]), sorted for a stable message.
    ignored: Vec<String>,
}

fn parse_site_options(json: Option<&str>) -> Result<SiteOptions, String> {
    use anofox_visualization::host::serve_options::{self, SiteOptionKind};
    let mut o = SiteOptions::default();
    let Some(json) = json.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(o);
    };
    let v: serde_json::Value =
        serde_json::from_str(json).map_err(|e| format!("--options: bad JSON: {e}"))?;
    let m = v.as_object().ok_or("--options must be a JSON object")?;
    let valid_var =
        |v: &str| sv::valid_ident(v) && !anofox_visualization::host::access::is_reserved_var(v);
    for (k, val) in m {
        match k.as_str() {
            "path_vars" => {
                let arr = val
                    .as_array()
                    .ok_or("options.path_vars must be a list of variable names")?;
                for v in arr {
                    let v = v
                        .as_str()
                        .filter(|v| valid_var(v))
                        .ok_or("options.path_vars: invalid variable name")?;
                    o.path_vars.push(v.to_string());
                }
            }
            "nav_picker" => {
                let m = val.as_object().ok_or(
                    "options.nav_picker must be an object like {\"var\": \"model\", \"sql\": \"SELECT …\", \"label\": \"Model\"}",
                )?;
                if let Some(other) = m
                    .keys()
                    .find(|k| !matches!(k.as_str(), "var" | "sql" | "label"))
                {
                    return Err(format!(
                        "options.nav_picker: unknown key '{other}' (var, sql, label)"
                    ));
                }
                let var = m
                    .get("var")
                    .and_then(|v| v.as_str())
                    .filter(|v| valid_var(v))
                    .ok_or("options.nav_picker.var must be a variable name")?;
                let sql = m
                    .get("sql")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .ok_or("options.nav_picker.sql must be a SELECT query")?;
                let label = match m.get("label") {
                    None | Some(serde_json::Value::Null) => None,
                    Some(serde_json::Value::String(s))
                        if !s.trim().is_empty() && s.chars().count() <= 100 =>
                    {
                        Some(s.clone())
                    }
                    _ => {
                        return Err(
                            "options.nav_picker.label must be a short (<=100 character) string"
                                .into(),
                        )
                    }
                };
                o.nav_picker = Some(SiteNavPicker {
                    var: var.to_string(),
                    sql: sql.to_string(),
                    label,
                });
            }
            "app" => {
                if !val.is_object() {
                    return Err(
                        "options.app must be an object like {\"name\": \"…\", \"tagline\": \"…\", \"logo\": \"https://…\"}"
                            .into(),
                    );
                }
                let a = AppBrand::from_json(&val.to_string())
                    .map_err(|e| format!("options.app: {e}"))?;
                a.check().map_err(|e| format!("options.app: {e}"))?;
                o.app = Some(val.clone());
            }
            "home" => {
                let h = val
                    .as_str()
                    .map(str::trim)
                    .filter(|h| sv::valid_dashboard_id(h))
                    .ok_or("options.home must be a dashboard id (its file name without .sql)")?;
                o.home = Some(h.to_string());
            }
            "theme" => {
                let spec = match val {
                    serde_json::Value::String(t) => t.clone(),
                    serde_json::Value::Object(_) => val.to_string(),
                    _ => return Err("options.theme must be a theme name or a JSON object".into()),
                };
                theme::resolve(&spec).map_err(|e| format!("options.theme: {e}"))?;
                o.theme = Some(spec);
            }
            // Review of #554 N3: the same cap the locked server would
            // apply to its own asset route, not a fixed `--site`-only
            // value — clamped the same way `LockedOptions::parse`
            // (`duckext/src/serve.rs`) clamps it, its own 64 MiB ceiling.
            "max_asset_bytes" => {
                let n = val
                    .as_u64()
                    .filter(|n| *n > 0)
                    .ok_or("options.max_asset_bytes must be a positive integer (bytes)")?;
                o.max_asset_bytes = Some(n.min(64 << 20));
            }
            "auth" | "policy" => {
                let msg = format!(
                    "options: unknown key '{k}' for --site (path_vars, nav_picker, app, home, theme — \
                     auth/policy make no sense for a site with no server behind it)"
                );
                return Err(msg);
            }
            "state" => {
                return Err(
                    "options.state: a static site cannot store submissions (needs the \
                     dashboard server)"
                        .into(),
                );
            }
            other => match serve_options::lookup(other) {
                // A real `anofox_serve_dashboards` option with no effect on
                // a static site (row caps, timeouts, `data`, …): skip it,
                // `cmd_site` names it on stderr once below.
                Some(SiteOptionKind::ServerOnly) => o.ignored.push(other.to_string()),
                // `Site`/`Refused` keys are all matched by name above; a
                // lookup hit here would mean this match and
                // `SERVER_OPTION_KEYS` disagree — treat it like a typo
                // rather than silently accepting or refusing it.
                Some(SiteOptionKind::Site) | Some(SiteOptionKind::Refused) | None => {
                    return Err(format!(
                        "options: unknown key '{other}' for --site (not one of \
                         anofox_serve_dashboards's own options either: {})",
                        serve_options::all_keys_joined()
                    ))
                }
            },
        }
    }
    o.ignored.sort();
    if let Some(np) = &o.nav_picker {
        if !o.path_vars.iter().any(|p| p.eq_ignore_ascii_case(&np.var)) {
            return Err(format!(
                "options.nav_picker.var '{}' must be one of options.path_vars",
                np.var
            ));
        }
    }
    Ok(o)
}

/// [`run_query`], but as positional `VARCHAR` rows ([`policy::Duck`]) instead
/// of JSON objects — the same adaptation `policy_diags`'s own closure does
/// above.
fn duck_rows(db: Option<&str>, sql: &str) -> Result<Vec<Vec<Option<String>>>, String> {
    Ok(run_query(db, sql)?
        .into_iter()
        .map(|r| {
            r.into_values()
                .map(|v| match v {
                    serde_json::Value::Null => None,
                    serde_json::Value::String(s) => Some(s),
                    other => Some(other.to_string()),
                })
                .collect()
        })
        .collect())
}

/// Every dashboard statement of `dashboards` must run with a data folder
/// open, exactly as the locked server's `check_read_only` requires — a
/// published site must never accept a folder the server would refuse.
/// `json_serialize_sql` only parses (it needs no `anofox_data`), so no
/// prelude is needed here.
fn site_check_read_only(dashboards: &[SiteDash]) -> Result<(), String> {
    let mut duck = |sql: &str| duck_rows(None, sql);
    for d in dashboards {
        for (i, (setup, sql)) in d.stmts.iter().enumerate() {
            sv::read_only_statement(sql, *setup, &mut duck)
                .map_err(|e| format!("dashboard '{}' statement {i}: {e}", d.id))?;
        }
    }
    Ok(())
}

/// Run `np.sql` with `anofox_data` set to `data` (a local path or URL —
/// whatever the generated site's pages will also set it to) and turn its
/// rows into the picker's options, exactly as the locked server's
/// `build_nav_picker` does against its live connection.
fn site_build_nav_picker(
    np: &SiteNavPicker,
    data: Option<&str>,
) -> Result<Vec<(String, String)>, String> {
    let mut duck = |sql: &str| duck_rows(None, sql);
    sv::read_only_statement(&np.sql, false, &mut duck)
        .map_err(|e| format!("options.nav_picker.sql: {e}"))?;
    let prelude = match data {
        Some(d) => format!("SET VARIABLE anofox_data = '{}';\n", d.replace('\'', "''")),
        None => String::new(),
    };
    let wrapped = format!("{prelude}{}", sv::wrap_nav_picker_sql(&np.sql));
    let rows = duck_rows(None, &wrapped).map_err(|e| format!("options.nav_picker.sql: {e}"))?;
    sv::nav_picker_rows_to_options(&rows)
}

/// Copy `src` into `dst` (already created), recursively. Used for `--site`'s
/// `out/data/` (a copy of a local `--data <dir>`) and for an unpacked `--app`
/// web directory.
/// Copy `src` into `dst` (already created), recursively, refusing a symlink
/// anywhere inside `src`: `entry.file_type()` is the symlink's own type (it
/// doesn't follow the link), never the target's, so a symlink would
/// otherwise silently copy whatever it points at — `--site --data` publishes
/// `out/data/` on a public host, so a symlink into `~/.ssh`, `/etc`, or
/// another project would publish that too. The locked server has the same
/// rule in spirit (`too_broad` refuses the obvious footguns on `options.data`
/// itself; this is the per-file backstop a symlink *inside* an otherwise
/// fine folder needs). Refusing (not skipping) matches `too_broad`: a
/// folder this command can't safely publish whole is worse to publish
/// partially and silently than to refuse outright.
fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let ft = entry.file_type()?;
        if ft.is_symlink() {
            return Err(std::io::Error::other(format!(
                "'{}' is a symlink — refusing to publish it (it may point outside this folder)",
                entry.path().display()
            )));
        }
        let to = dst.join(entry.file_name());
        if ft.is_dir() {
            copy_dir_recursive(&entry.path(), &to)?;
        } else {
            std::fs::copy(entry.path(), &to)?;
        }
    }
    Ok(())
}

/// `--site`'s own `assets/` pre-publish check (#514 PR B): recurse `dir`,
/// refusing (not skipping) a symlink anywhere inside it, a file whose
/// extension isn't one of [`anofox_visualization::image::ASSET_EXTENSIONS`],
/// or one over `max` bytes. `copy_dir_recursive`'s own symlink refusal
/// (below) still applies to the actual copy — this is the belt to that
/// braces, and the only thing enforcing the extension/size rule at all
/// (`copy_dir_recursive` is also used for plain `--data`, which has no such
/// rule).
/// The SVG and XLink namespace URIs — the only two namespaces an accepted
/// SVG may use anywhere in the document (review of #554 B1).
const SVG_NS: &str = "http://www.w3.org/2000/svg";
const XLINK_NS: &str = "http://www.w3.org/1999/xlink";

/// Local, SVG-spec-cased element names a published SVG may contain
/// (`svg_active_reason`'s ALLOW-list). No animation element (`animate`,
/// `set`, `animateTransform`, `animateMotion`, `mpath`) is in this list at
/// all: SMIL's own `href`/`xlink:href`/`values` targeting (as in
/// `<animate attributeName="href" values="javascript:…">`, review of #554
/// B1's `anim.svg`) has no legitimate use in a static dashboard asset, so
/// it's simplest to refuse every animation element outright rather than
/// inspect what each one targets or decodes to. No `<a>` either (its
/// `href` would need the same external-reference check `<image>` gets,
/// for no real benefit in a static asset). No `<style>` element: CSS
/// `url()`/`@import`/`expression()` is the same class of problem as a
/// `style=` attribute (checked below), and a `<style>` element's full text
/// content is a much larger surface to re-check than one attribute value.
const SVG_ALLOWED_ELEMENTS: &[&str] = &[
    "svg",
    "g",
    "defs",
    "symbol",
    "use",
    "path",
    "rect",
    "circle",
    "ellipse",
    "line",
    "polyline",
    "polygon",
    "text",
    "tspan",
    "textPath",
    "title",
    "desc",
    "linearGradient",
    "radialGradient",
    "stop",
    "pattern",
    "clipPath",
    "mask",
    "marker",
    "image",
    // Filter primitives: pure pixel math, no script or external-resource
    // surface — unlike `feImage`, deliberately left out here: it takes its
    // own `href`, is rarely needed for a dashboard icon, and would need
    // the same href check `<image>` gets below.
    "filter",
    "feGaussianBlur",
    "feOffset",
    "feMerge",
    "feMergeNode",
    "feComponentTransfer",
    "feFuncR",
    "feFuncG",
    "feFuncB",
    "feFuncA",
    "feColorMatrix",
    "feComposite",
    "feBlend",
    "feFlood",
    "feTile",
    "feDropShadow",
    "feMorphology",
];

/// Raster `data:image/<t>` subtypes allowed on an `<image href="…">` inside
/// an already-SVG asset — narrower than [`anofox_visualization::image::DATA_IMAGE_TYPES`]
/// (no `svg+xml`: nesting another SVG payload here would reopen the same
/// question `svg_active_reason` exists to close, one level down).
const SVG_IMAGE_HREF_RASTER_TYPES: &[&str] = &["png", "jpeg", "jpg", "gif", "webp"];

fn is_allowed_raster_data_uri(v: &str) -> bool {
    let low = v.to_ascii_lowercase();
    low.starts_with("data:image/")
        && SVG_IMAGE_HREF_RASTER_TYPES.iter().any(|t| {
            let p = format!("data:image/{t}");
            low.starts_with(&format!("{p};")) || low.starts_with(&format!("{p},"))
        })
}

/// Does `value` (any attribute's value, already XML-entity-decoded by the
/// parser) try to reach something off-document through CSS? Review of
/// #554 (second re-review, S1): checking only the `style` attribute left
/// every *presentation* attribute (`fill`, `stroke`, `filter`,
/// `clip-path`, `mask`, every `marker-*`, `cursor`, …) as an unguarded
/// fetch — SVG's own CSS cascade applies to a presentation attribute
/// exactly as if it were written in `style`, so `fill="url(https://…)"`
/// fetches just as `style="fill:url(https://…)"` does. A CSS escape also
/// defeated a literal `"url("` substring check (`u\72l(` decodes, in CSS,
/// to `url(`), and `image-set('https://…')` reaches a URL with no `url(`
/// at all. This function is applied to **every** attribute's value
/// ([`svg_active_reason`], below), refusing:
/// - any backslash at all — the escape vector; no legitimate value here
///   needs one;
/// - `url(…)` whose target isn't a same-document `#fragment` (`url(#id)`
///   is how an SVG legitimately references its own
///   `<filter>`/`<clipPath>`/gradient by CSS);
/// - `image-set(` (a CSS image fallback list), `@import`, `src(` (the
///   newer, unprefixed CSS font/image loading function), `javascript:`
///   anywhere, `expression(` (old IE CSS, still worth refusing).
fn external_ref_reason(value: &str) -> Option<String> {
    if value.contains('\\') {
        return Some(
            "contains a backslash (a CSS escape can hide url()/javascript: from a literal scan)"
                .to_string(),
        );
    }
    let low = value.to_ascii_lowercase();
    for bad in [
        "@import",
        "image-set(",
        "src(",
        "javascript:",
        "expression(",
    ] {
        if low.contains(bad) {
            return Some(format!("contains '{bad}'"));
        }
    }
    let mut rest = low.as_str();
    while let Some(i) = rest.find("url(") {
        let after = &rest[i + 4..];
        let end = after.find(')').unwrap_or(after.len());
        let inner = after[..end].trim().trim_matches(['"', '\'']);
        if !inner.starts_with('#') {
            return Some(format!(
                "contains a url() that isn't a local #fragment ('{}')",
                truncate_for_message(inner)
            ));
        }
        rest = &after[end.min(after.len())..];
    }
    None
}

fn truncate_for_message(s: &str) -> String {
    const MAX: usize = 60;
    if s.chars().count() <= MAX {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(MAX).collect::<String>())
    }
}

/// Is `bytes` (already confirmed, by its magic bytes, to be an SVG) "active"
/// — could it run script or fetch something external if a browser opens it
/// as a *document* rather than drawing it through an `<img>`? Review of
/// #554 item 1: the locked server neutralises this with its asset route's
/// own `Content-Security-Policy: sandbox` header on a direct open, but a
/// published static site has no server to send that header — `assets/x.svg`
/// opened directly on GitHub Pages/S3/etc. just runs. So `--site` refuses to
/// publish one instead.
///
/// An ALLOW-list over a *parsed* document, not a deny-list over the raw
/// text (review of #554 B1: the previous text-scan version was bypassed by
/// a namespace-prefixed `<x:script>` where `x` is bound to the SVG
/// namespace under a prefix other than none; a foreign-namespace element
/// — `<h:script>`/`<h:iframe>` bound to the XHTML namespace, needing no
/// `<foreignObject>` wrapper at all; an internal DTD entity expanding to
/// literal `<script>` text; and `&#106;avascript:` — entity-encoded `j` —
/// hiding a `javascript:` URL from a literal substring match in an
/// `<animate>`'s `values` and in a quoted `href`. A real XML parser that
/// resolves namespaces and decodes entities before this function ever sees
/// a string closes all four at once, instead of each needing its own
/// pattern added to a text scan that will always be one bypass behind).
///
/// Every element must resolve to the SVG namespace (`tag_name().namespace()
/// == Some(SVG_NS)`, never guessed from a prefix) and be in
/// [`SVG_ALLOWED_ELEMENTS`]. Every attribute must be unprefixed or in the
/// XLink namespace — any other namespace is refused outright, which is what
/// actually stops `<h:script>`/`<h:iframe>` regardless of what prefix binds
/// them. No attribute name may start with `on` (case-insensitively — SVG
/// alone has dozens of event handlers: `onload`, `onbegin`, `onend`,
/// `onrepeat`, `onactivate`, …). `href`/`xlink:href` may only be a
/// same-document `#fragment`, or, on `<image>` only, a
/// `data:image/(png|jpeg|gif|webp)` payload. A `style` attribute may not
/// contain `@import`, `javascript:`, `expression(`, or a `url(…)` that
/// isn't a `#fragment` ([`style_url_reason`]).
///
/// A literal `<!DOCTYPE`, anywhere in the raw bytes, is refused before
/// parsing even starts — entity expansion is exactly how `ent.svg` (review
/// of #554 B1) smuggled a `<script>` past the previous version; never let
/// the parser expand an entity at all. Redundant with
/// [`roxmltree::ParsingOptions::allow_dtd`]'s own default of `false`
/// (`Document::parse` uses it), kept as a belt-and-braces check that
/// doesn't depend on that default never changing. A processing instruction
/// anywhere in the document is refused too — untested territory for
/// whatever opens this file directly; simplest to disallow.
///
/// Returns the reason, or `None` if none of this matched (still just pixels
/// and markup, not a reason this file is safe in some deeper sense this
/// scan can't see).
fn svg_active_reason(bytes: &[u8]) -> Option<String> {
    let text = match std::str::from_utf8(bytes) {
        Ok(t) => t,
        Err(_) => return Some("is not valid UTF-8 (can't be parsed as XML)".to_string()),
    };
    if text.to_ascii_lowercase().contains("<!doctype") {
        return Some("contains a <!DOCTYPE declaration (entity expansion risk)".to_string());
    }
    let doc = match roxmltree::Document::parse(text) {
        Ok(d) => d,
        Err(e) => return Some(format!("could not be parsed as well-formed XML ({e})")),
    };
    for node in doc.descendants() {
        if node.is_pi() {
            return Some("contains a processing instruction".to_string());
        }
        if !node.is_element() {
            continue;
        }
        let tag = node.tag_name();
        if tag.namespace() != Some(SVG_NS) {
            return Some(format!(
                "contains an element ('{}') outside the SVG namespace",
                tag.name()
            ));
        }
        if !SVG_ALLOWED_ELEMENTS.contains(&tag.name()) {
            return Some(format!("contains a disallowed element ('{}')", tag.name()));
        }
        for attr in node.attributes() {
            let ns = attr.namespace();
            if ns.is_some() && ns != Some(XLINK_NS) {
                return Some(format!(
                    "contains an attribute ('{}') outside the SVG/XLink namespaces",
                    attr.name()
                ));
            }
            let local = attr.name();
            if local.len() >= 2 && local.as_bytes()[..2].eq_ignore_ascii_case(b"on") {
                return Some(format!("contains an event-handler attribute ('{local}')"));
            }
            // Review of #554 (second re-review, S1): checked on *every*
            // attribute, not only `style` — a presentation attribute
            // (`fill`, `filter`, `clip-path`, `mask`, `marker-*`,
            // `cursor`, …) takes the same CSS syntax `style` does.
            if let Some(why) = external_ref_reason(attr.value()) {
                return Some(format!("attribute '{local}' {why}"));
            }
            if local.eq_ignore_ascii_case("href") {
                let v = attr.value().trim();
                let ok =
                    v.starts_with('#') || (tag.name() == "image" && is_allowed_raster_data_uri(v));
                if !ok {
                    return Some(format!(
                        "references an external href/xlink:href ('{}')",
                        truncate_for_message(v)
                    ));
                }
            }
        }
    }
    None
}

/// `--site`'s own `assets/` pre-publish pass (#514 PR B; review of #554
/// items 1/2): recurse `root`'s `rel` subdirectory (`rel` starts `""`),
/// writing each validated file to `out/rel/name`. A dot-leading entry
/// (`.secret.png`, `.DS_Store`, a stray `.git/`, …) is skipped — not
/// copied, not a publish failure, the same as `read_asset`'s own
/// `outside-folder` refusal for a hidden segment, which [`read_asset`]
/// would otherwise report for it below. Every other file goes through
/// [`anofox_visualization::image::read_asset`] — the exact extension
/// allow-list, containment and real magic-byte MIME check the locked
/// server applies per request — so "what the server would serve" and
/// "what the site publishes" can't drift apart; a non-image renamed to an
/// allowed extension fails the whole publish, same as a symlink. An SVG
/// that passes `read_asset` is additionally checked by
/// [`svg_active_reason`], since a published site has no server to send the
/// locked route's own sandboxing CSP header.
fn publish_assets(root: &Path, rel: &str, out: &Path, max: u64) -> Result<(), String> {
    let dir = if rel.is_empty() {
        root.to_path_buf()
    } else {
        root.join(rel)
    };
    for entry in std::fs::read_dir(&dir).map_err(|e| format!("read {}: {e}", dir.display()))? {
        let entry = entry.map_err(|e| format!("read {}: {e}", dir.display()))?;
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str.starts_with('.') {
            continue;
        }
        let ft = entry
            .file_type()
            .map_err(|e| format!("stat {}: {e}", entry.path().display()))?;
        let child_rel = if rel.is_empty() {
            name_str.into_owned()
        } else {
            format!("{rel}/{name_str}")
        };
        if ft.is_symlink() {
            return Err(format!(
                "'{}' is a symlink — refusing to publish it (it may point outside assets/)",
                entry.path().display()
            ));
        }
        if ft.is_dir() {
            publish_assets(root, &child_rel, out, max)?;
            continue;
        }
        let (mime, bytes) =
            anofox_visualization::image::read_asset(root, &child_rel, max).map_err(|reason| {
                format!(
                    "'{}': refused ({reason}) — assets/ only publishes files the locked server itself would serve",
                    entry.path().display()
                )
            })?;
        if mime == "image/svg+xml" {
            if let Some(why) = svg_active_reason(&bytes) {
                return Err(format!(
                    "'{}' {why} — refusing to publish an active SVG to a static site with no CSP/sandbox header to neutralise it (docs/secure-serving.md)",
                    entry.path().display()
                ));
            }
        }
        let dst = out.join(&child_rel);
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
        }
        std::fs::write(&dst, &bytes).map_err(|e| format!("write {}: {e}", dst.display()))?;
    }
    Ok(())
}

/// Is `s` a `--data` URL (as opposed to a local directory)?
fn is_data_url(s: &str) -> bool {
    s.starts_with("https://") || s.starts_with("http://")
}

/// Removes an extracted `--app` tarball's temp directory when dropped. Only
/// covers `cmd_site`'s normal-completion path: every error here still exits
/// through `fail()` (`std::process::exit`), which — like every other
/// `std::process::exit` in this file — skips destructors, so an early
/// failure after extraction can still leave the temp directory behind. A
/// hygiene gap, not a security one (it holds a copy of the public `--app`
/// web build, nothing secret, and the OS reclaims `/tmp` eventually); a
/// full fix would mean reworking `cmd_site`'s whole `fail()`-exits-
/// immediately error style, out of scope here.
struct AppExtractGuard(Option<PathBuf>);
impl Drop for AppExtractGuard {
    fn drop(&mut self) {
        if let Some(p) = self.0.take() {
            let _ = std::fs::remove_dir_all(p);
        }
    }
}

/// `--app <dir|tarball>`: a directory already (no guard — nothing of ours to
/// clean up), or a `.tar.gz`/`.tgz` release asset unpacked (via the `tar` on
/// PATH — no tar/gzip crate: the core stays dependency-light) into a fresh
/// temp directory, removed by the returned [`AppExtractGuard`] once the
/// caller is done with it.
fn resolve_app_dir(app: &Path) -> Result<(PathBuf, AppExtractGuard), String> {
    let is_archive = matches!(
        app.extension().and_then(|e| e.to_str()),
        Some("gz") | Some("tgz")
    );
    if !is_archive {
        if !app.is_dir() {
            return Err(format!("--app '{}' is not a directory", app.display()));
        }
        return Ok((app.to_path_buf(), AppExtractGuard(None)));
    }
    let tmp = std::env::temp_dir().join(format!(
        "anofox-site-app-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    ));
    std::fs::create_dir_all(&tmp).map_err(|e| format!("mkdir {}: {e}", tmp.display()))?;
    let guard = AppExtractGuard(Some(tmp.clone()));
    let status = Command::new("tar")
        .arg("-xzf")
        .arg(app)
        .arg("-C")
        .arg(&tmp)
        .status()
        .map_err(|e| format!("run tar: {e}"))?;
    if !status.success() {
        return Err(format!("tar -xzf {}: failed", app.display()));
    }
    // The app files may sit right at the tarball's root, under a `web/`
    // folder, or (the actual `anofox-visualization-web-<tag>.tar.gz` release
    // asset, scripts/release/build-web.sh) under its own
    // `anofox-visualization-web-<tag>/` folder, as every other release
    // tarball is wrapped (so unpacking it never scatters files into the
    // current directory) — so: already rooted at the app files (has its own
    // index.html), else `web/`, else the only directory the archive has.
    if tmp.join("index.html").is_file() {
        return Ok((tmp, guard));
    }
    let web = tmp.join("web");
    if web.is_dir() {
        return Ok((web, guard));
    }
    let mut dirs = std::fs::read_dir(&tmp)
        .map_err(|e| format!("read {}: {e}", tmp.display()))?
        .flatten()
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false));
    match (dirs.next(), dirs.next()) {
        (Some(only), None) => Ok((only.path(), guard)),
        _ => Err(format!(
            "--app '{}': unpacked but no index.html found (not an anofox-visualization-web-<tag>.tar.gz?)",
            app.display()
        )),
    }
}

/// `dashboard --site <folder> <out-dir> [--data <dir|url>] [--options locked.json] [--app <web dir|tarball>]`
fn cmd_site(args: &[String]) {
    let mut folder: Option<String> = None;
    let mut out: Option<String> = None;
    let mut data: Option<String> = None;
    let mut options: Option<String> = None;
    let mut app: Option<String> = None;
    let mut positional = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--data" => data = it.next().cloned(),
            "--options" => options = it.next().cloned(),
            "--app" => app = it.next().cloned(),
            _ if !a.starts_with("--") => positional.push(a.clone()),
            _ => {
                eprintln!("--site: unknown flag '{a}'");
                std::process::exit(2);
            }
        }
    }
    if positional.len() == 1 {
        folder = positional.first().cloned();
    } else if positional.len() >= 2 {
        folder = positional.first().cloned();
        out = positional.get(1).cloned();
    }
    let (Some(folder), Some(out)) = (folder, out) else {
        eprintln!(
            "usage: dashboard --site <folder> <out-dir> [--data <dir|url>] \
             [--options locked.json] [--app <web dir|tarball>]"
        );
        std::process::exit(2);
    };
    let folder = Path::new(&folder);
    let out = Path::new(&out);
    let fail = |msg: String| -> ! {
        eprintln!("--site: {msg}");
        std::process::exit(1);
    };

    // 1. The folder, as the locked server would open it: the dashboards
    //    load and parse, every statement is read-only ([`site_check_read_only`]),
    //    and `--check`'s own lint (broken/empty panels, header issues) passes.
    let dashboards = load_site_dashboards(folder).unwrap_or_else(|e| fail(e));
    let options_json = options.as_deref().map(|p| {
        std::fs::read_to_string(p).unwrap_or_else(|e| fail(format!("--options '{p}': {e}")))
    });
    let opts = parse_site_options(options_json.as_deref()).unwrap_or_else(|e| fail(e));
    if !opts.ignored.is_empty() {
        // #501: a locked server's own `options` file handed to `--site`
        // unedited carries server-only limits that have no effect here —
        // named once so a typo'd key doesn't silently vanish unnoticed.
        eprintln!("--options: ignored for --site: {}", opts.ignored.join(", "));
    }

    let data_value: Option<String> = match &data {
        Some(d) if is_data_url(d) => Some(d.trim_end_matches('/').to_string()),
        Some(d) => {
            let abs =
                std::fs::canonicalize(d).unwrap_or_else(|e| fail(format!("--data '{d}': {e}")));
            if !abs.is_dir() {
                fail(format!("--data '{d}' is not a directory"));
            }
            // S1 (PR #429 review): a published site copies this folder onto
            // a public host — refusing a too-broad one here is the same
            // rule `options.data` enforces on a locked server
            // (`LockedOptions::parse`), shared via `sv::too_broad`.
            if let Some(why) = sv::too_broad(&abs) {
                fail(format!(
                    "--data '{d}': refusing '{}': {why} — point it at the folder that holds the data only",
                    abs.display()
                ));
            }
            Some(abs.to_string_lossy().into_owned())
        }
        None => None,
    };
    if !opts.path_vars.is_empty() && data_value.is_none() {
        fail("options.path_vars needs --data".into());
    }

    if let Err(e) = site_check_read_only(&dashboards) {
        fail(format!(
            "{e}\n(the locked server would refuse this folder too — fix the dashboard or drop --data)"
        ));
    }

    let mut diags_failed = false;
    {
        let names: Vec<String> = dashboards.iter().map(|d| format!("{}.sql", d.id)).collect();
        let scripts: Vec<(String, String)> = dashboards
            .iter()
            .map(|d| (format!("{}.sql", d.id), d.sql.clone()))
            .collect();
        let data_set = data_value
            .as_ref()
            .map(|d| format!("SET VARIABLE anofox_data = '{}';\n", d.replace('\'', "''")))
            .unwrap_or_default();
        let extra = lint::folder_diags(&scripts);
        // #514 PR B: `--site`'s own pre-publish lint gets the folder, so
        // `image/missing-file` runs here too.
        let image_root = anofox_visualization::image::canonical_assets_dir(folder);
        for (i, d) in dashboards.iter().enumerate() {
            // #435: one persistent session per dashboard, not a fresh
            // process per statement replaying everything run so far.
            let mut session = anofox_visualization::host::duckdb_cli::SessionRunner::spawn(None)
                .unwrap_or_else(|e| fail(e));
            if !data_set.is_empty() {
                let _ = session.run(&data_set);
            }
            let mut diags = lint::check_opts(
                &d.sql,
                |sql| session.run(sql),
                lint::LintOptions {
                    design: false,
                    stateful_session: true,
                    image_root: image_root.clone(),
                },
            );
            diags.extend(
                extra
                    .iter()
                    .filter(|(f, _)| f == &names[i])
                    .map(|(_, d)| d.clone()),
            );
            if !lint::passes(&diags, false) {
                diags_failed = true;
                for diag in &diags {
                    if diag.severity == Severity::Error {
                        eprintln!(
                            "--site: {}.sql [stmt {}] {} — {}",
                            d.id, diag.stmt, diag.code, diag.message
                        );
                    }
                }
            }
        }
    }
    if diags_failed {
        fail("one or more dashboards fail --check — fix them first (dashboard --check <folder> for details)".into());
    }

    // 2. The nav_picker's options, computed now (never from a viewer, never
    //    per-request — the exact rule `build_nav_picker` runs against a live
    //    connection, here against the `duckdb` CLI).
    let nav_picker_options = opts
        .nav_picker
        .as_ref()
        .map(|np| site_build_nav_picker(np, data_value.as_deref()).unwrap_or_else(|e| fail(e)));

    // 3. The app bundle (`--app`, else next to this binary). `_app_guard`
    // removes a tarball's extracted temp directory once it drops at the end
    // of this function (A3, PR #429 review) — see its own doc comment for
    // what it doesn't cover.
    let (app_src, _app_guard) = match app {
        Some(a) => resolve_app_dir(Path::new(&a)).unwrap_or_else(|e| fail(e)),
        None => {
            let exe = std::env::current_exe().unwrap_or_else(|e| fail(format!("current_exe: {e}")));
            let next_to = exe.parent().map(|p| p.join("web"));
            match next_to.filter(|p| p.is_dir()) {
                Some(p) => (p, AppExtractGuard(None)),
                None => fail(
                    "no --app given and no 'web' folder next to this binary — pass --app <web dir|tarball> \
                     (the anofox-visualization-web-<tag>.tar.gz release asset, unpacked or not)"
                        .into(),
                ),
            }
        }
    };
    let index_html = std::fs::read_to_string(app_src.join("index.html"))
        .unwrap_or_else(|e| fail(format!("--app: read index.html: {e}")));
    // The exact markers `page()` (below) rewrites: refuse rather than quietly
    // publish a page with no CSP meta, no published-mode body class, or
    // (`import.meta.url`-relative app.js/theme-base.css aside) broken asset
    // paths on an `--app` from a web build this generator doesn't recognize.
    for marker in [
        "<meta http-equiv=\"Content-Security-Policy\" content=\"object-src 'none'; base-uri 'none'\">",
        "<head>",
        "<body class=\"mode-view\">",
        "href=\"theme-base.css\"",
        "src=\"./app.js\"",
    ] {
        if !index_html.contains(marker) {
            fail(format!(
                "--app: index.html doesn't look like an anofox-visualization web build (missing {marker:?}) — \
                 pass a matching anofox-visualization-web-<tag> release asset"
            ));
        }
    }

    // 4. Write the output tree. The app's own bundled builder content
    // (`data/`, `examples/`, `m5_monthly.parquet` — the in-browser "Examples"
    // library's sample dashboards/data, never read in published mode) is
    // skipped: a published site never needs it, and `data/` would otherwise
    // collide with this command's own `out/data/` (the site's `--data`).
    const APP_SKIP: [&str; 3] = ["data", "examples", "m5_monthly.parquet"];
    std::fs::create_dir_all(out).unwrap_or_else(|e| fail(format!("mkdir {}: {e}", out.display())));
    for entry in std::fs::read_dir(&app_src)
        .unwrap_or_else(|e| fail(format!("read {}: {e}", app_src.display())))
    {
        let entry = entry.unwrap_or_else(|e| fail(format!("read {}: {e}", app_src.display())));
        let name = entry.file_name();
        if APP_SKIP
            .iter()
            .any(|s| std::ffi::OsStr::new(s) == name.as_os_str())
        {
            continue;
        }
        let to = out.join(&name);
        let ft = entry
            .file_type()
            .unwrap_or_else(|e| fail(format!("stat {}: {e}", entry.path().display())));
        // S1: the same symlink refusal as copy_dir_recursive's own, for a
        // symlink sitting right at --app's top level (copy_dir_recursive
        // only ever sees one *inside* a subdirectory).
        if ft.is_symlink() {
            fail(format!(
                "'{}' is a symlink — refusing to publish it (it may point outside --app)",
                entry.path().display()
            ));
        } else if ft.is_dir() {
            copy_dir_recursive(&entry.path(), &to)
                .unwrap_or_else(|e| fail(format!("copy app files: {e}")));
        } else {
            std::fs::copy(entry.path(), &to)
                .unwrap_or_else(|e| fail(format!("copy app files: {e}")));
        }
    }
    let dash_dir = out.join("dashboards");
    std::fs::create_dir_all(&dash_dir)
        .unwrap_or_else(|e| fail(format!("mkdir {}: {e}", dash_dir.display())));
    for d in &dashboards {
        std::fs::write(dash_dir.join(format!("{}.sql", d.id)), &d.sql)
            .unwrap_or_else(|e| fail(format!("write dashboards/{}.sql: {e}", d.id)));
    }

    // #514 PR B: `<folder>/assets/` → `<out>/assets/`, if it's there — the
    // locked server's own `/d/<id>/assets/<rest>` route, and the CLI's own
    // relative-image inlining, both resolve against this same subfolder
    // (decision 5). A published site serves whatever lands here to the
    // world with no login and no server to set any headers at request
    // time, so `publish_assets` is stricter than `copy_dir_recursive`'s
    // own symlink-only refusal: a non-image extension, an oversized file,
    // a renamed non-image (checked by its real magic bytes, not just its
    // extension — review of #554 item 2), or an "active" SVG (item 1)
    // fails the whole publish too, not just that one file. A dot-leading
    // entry is silently skipped, not published and not a failure (`.git`,
    // `.DS_Store`, …).
    let assets_src = folder.join("assets");
    if anofox_visualization::image::assets_dir_is_symlink(folder) {
        fail(format!(
            "'{}' is a symlink — refusing to publish it (it may point outside the dashboards folder)",
            assets_src.display()
        ));
    }
    if let Some(assets_root) = anofox_visualization::image::canonical_assets_dir(folder) {
        // Review of #554 N3: `opts.max_asset_bytes` (from `--options`,
        // when set) rather than always the default — a site built from
        // the same options file a locked server runs with must not
        // refuse a file that server was configured to serve.
        publish_assets(
            &assets_root,
            "",
            &out.join("assets"),
            opts.max_asset_bytes
                .unwrap_or(anofox_visualization::image::DEFAULT_MAX_ASSET_BYTES),
        )
        .unwrap_or_else(|e| fail(format!("assets/: {e}")));
    }

    let site_data = match (&data, &data_value) {
        (Some(d), Some(v)) if is_data_url(d) => Some(v.clone()),
        (Some(_), Some(_)) => {
            let data_out = out.join("data");
            let src = Path::new(data.as_deref().unwrap());
            copy_dir_recursive(src, &data_out)
                .unwrap_or_else(|e| fail(format!("copy --data: {e}")));
            Some("data".to_string())
        }
        _ => None,
    };

    let nav: Vec<serde_json::Value> = dashboards.iter().map(|d| d.meta.nav_json(&d.id)).collect();
    let mut site_json = serde_json::json!({
        "dashboards": nav,
    });
    if !opts.path_vars.is_empty() {
        site_json["pathVars"] = serde_json::json!(opts.path_vars);
    }
    if let (Some(np), Some(options)) = (&opts.nav_picker, &nav_picker_options) {
        site_json["navPicker"] = serde_json::json!({
            "var": np.var,
            "label": np.label,
            "options": options.iter().map(|(v, l)| serde_json::json!({"value": v, "label": l})).collect::<Vec<_>>(),
        });
    }
    if let Some(app) = &opts.app {
        site_json["app"] = app.clone();
    }
    if let Some(theme) = &opts.theme {
        site_json["theme"] = serde_json::json!(theme);
    }
    if let Some(home) = &opts.home {
        site_json["home"] = serde_json::json!(home);
    }
    if let Some(d) = &site_data {
        site_json["data"] = serde_json::json!(d);
    }
    std::fs::write(
        out.join("site.json"),
        serde_json::to_string(&site_json).unwrap_or_else(|e| fail(format!("site.json: {e}"))),
    )
    .unwrap_or_else(|e| fail(format!("write site.json: {e}")));

    // 5. index.html (the home page, below) + one launcher per dashboard
    //    (`d/<id>/index.html`): a GitHub-Pages-friendly real file per
    //    dashboard id, so a refresh, a shared link and back/forward all
    //    land on a page that can bootstrap itself (there are no server
    //    rewrites on static hosting).
    let data_origin = data_value
        .as_deref()
        .filter(|d| is_data_url(d))
        .and_then(|d| {
            let rest = d
                .strip_prefix("https://")
                .or_else(|| d.strip_prefix("http://"))?;
            let host = rest.split('/').next()?;
            Some(format!("{}://{}", d.split("://").next()?, host))
        });
    let page = |dash_id: &str, rel: &str| -> String {
        let csp = site_csp(data_origin.as_deref());
        let html = index_html
            .replacen(
                "<meta http-equiv=\"Content-Security-Policy\" content=\"object-src 'none'; base-uri 'none'\">",
                &format!("<meta http-equiv=\"Content-Security-Policy\" content=\"{csp}\">"),
                1,
            )
            .replacen(
                "<head>",
                // "site.json" always — `sitePrepare()` (web/app.js) resolves it
                // against `import.meta.url` (app.js's own, fixed location: every
                // page loads the same app.js, co-located with site.json), never
                // against this page's own URL, so it must NOT carry `rel`'s
                // document-relative "../../" (a launcher is two directories
                // deeper than the page that serves app.js — only the asset
                // hrefs below need that prefix).
                &format!(
                    "<head>\n<meta name=\"anofox-site\" content=\"site.json\">\n<meta name=\"anofox-dash\" content=\"{}\">",
                    sv::html_escape_attr_roundtrip(dash_id)
                ),
                1,
            )
            .replacen(
                "<body class=\"mode-view\">",
                "<body class=\"mode-view embed published\">",
                1,
            );
        if rel.is_empty() {
            html
        } else {
            html.replace(
                "href=\"theme-base.css\"",
                &format!("href=\"{rel}theme-base.css\""),
            )
            .replace("src=\"./app.js\"", &format!("src=\"{rel}app.js\""))
        }
    };
    for d in &dashboards {
        let launcher_dir = out.join("d").join(&d.id);
        std::fs::create_dir_all(&launcher_dir)
            .unwrap_or_else(|e| fail(format!("mkdir {}: {e}", launcher_dir.display())));
        std::fs::write(launcher_dir.join("index.html"), page(&d.id, "../../"))
            .unwrap_or_else(|e| fail(format!("write d/{}/index.html: {e}", d.id)));
    }
    // A2 (PR #429 review): `/` matches the locked server exactly — straight
    // to `options.home` (a launcher page here; the server's own 302) when
    // it's set, else the same card-grid landing page every dashboard count
    // gets there too (`page::landing`, reused verbatim: the same markup,
    // the same CSS, so there is nothing of its own for this command to keep
    // in sync).
    let valid_home = opts
        .home
        .as_deref()
        .filter(|h| dashboards.iter().any(|d| &d.id == h));
    let index_body = match valid_home {
        Some(h) => page(h, ""),
        None => {
            let theme_set =
                theme::resolve(opts.theme.as_deref().unwrap_or("")).unwrap_or_else(|e| {
                    fail(format!("options.theme: {e}")) // already validated in parse_site_options; defensive
                });
            let items = dashboards
                .iter()
                .map(|d| page::LandingItem {
                    href: format!("d/{}/", d.id),
                    meta: &d.meta,
                    updated: sv::file_date(&folder.join(format!("{}.sql", d.id))),
                })
                .collect();
            let app = opts.app.as_ref().map(|v| {
                AppBrand::from_json(&v.to_string())
                    .unwrap_or_else(|e| fail(format!("options.app: {e}")))
            });
            let html = page::landing(&page::Landing {
                theme: &theme_set,
                app: app.as_ref(),
                items,
            });
            site_landing_csp(&html)
        }
    };
    std::fs::write(out.join("index.html"), index_body)
        .unwrap_or_else(|e| fail(format!("write index.html: {e}")));

    eprintln!(
        "wrote {} ({} dashboard{}) — serve it under any web root, the files are all relative",
        out.display(),
        dashboards.len(),
        if dashboards.len() == 1 { "" } else { "s" }
    );
}

/// `@duckdb/duckdb-wasm`'s own `+esm` build (jsdelivr) pulls in apache-arrow,
/// which pulls in tslib and flatbuffers, each its own
/// `cdn.jsdelivr.net/npm/<pkg>@<version>/+esm` script — found by actually
/// running a published site (#417) and watching the CSP reports name one
/// transitive dependency after another, then confirmed clean (no further
/// ones) the same way. A locked server's own `page_csp` (`host::serving`)
/// never needed any of these: a locked/served page never calls local
/// `conn.query()` at all (it fetches `/api/panel` instead). Each is pinned
/// exact, same as `sv::CDN_DUCKDB_WASM`/`sv::CDN_KATEX` — jsdelivr's `+esm`
/// resolves a *specific* source package version's imports once and serves
/// that resolution long-term (the same property that already makes pinning
/// `CDN_DUCKDB_WASM`'s own exact version meaningful); re-pin these three
/// (and re-run the end-to-end check in the PR this const was added by,
/// watching for a *new* "violates ... script-src" console error) whenever
/// `@duckdb/duckdb-wasm`'s pinned version in tests/web/package.json changes.
const CDN_APACHE_ARROW: &str = "https://cdn.jsdelivr.net/npm/apache-arrow@17.0.0/";
const CDN_TSLIB: &str = "https://cdn.jsdelivr.net/npm/tslib@2.6.3/";
const CDN_FLATBUFFERS: &str = "https://cdn.jsdelivr.net/npm/flatbuffers@24.3.25/";

/// The published site's `<meta http-equiv="Content-Security-Policy">`: the
/// locked server's own header ([`sv::page_csp`]) minus `frame-ancestors` (a
/// `<meta>` can't carry it — GitHub Pages sets no response headers at all),
/// minus the per-response nonce (a static site's pages carry no inline
/// `<script>`: `script-src 'self'` covers `app.js`, the one script every
/// page loads), plus [`CDN_APACHE_ARROW`]/[`CDN_TSLIB`]/[`CDN_FLATBUFFERS`]
/// (needed only because a published site, unlike a locked server, runs
/// every query locally — see their own doc comment). `data_origin`: a
/// remote `--data` URL's own origin, added to `connect-src` — the plan's
/// own rule, "no request to hosts other than its own origin and the data
/// URL".
fn site_csp(data_origin: Option<&str>) -> String {
    format!(
        "default-src 'none'; script-src 'self' 'wasm-unsafe-eval' {} {} {} {} {}; \
         worker-src blob:; connect-src 'self' {} {} {} {} {} {} {}; img-src 'self' data: blob: https:; \
         style-src 'self' 'unsafe-inline' https://fonts.googleapis.com {}; \
         font-src 'self' data: https://fonts.gstatic.com {}; manifest-src 'self'; media-src 'none'; \
         object-src 'none'; frame-src 'none'; base-uri 'self'; form-action 'self'",
        sv::CDN_DUCKDB_WASM,
        sv::CDN_KATEX,
        CDN_APACHE_ARROW,
        CDN_TSLIB,
        CDN_FLATBUFFERS,
        sv::CDN_DUCKDB_WASM,
        CDN_APACHE_ARROW,
        CDN_TSLIB,
        CDN_FLATBUFFERS,
        sv::DUCKDB_EXT,
        sv::USGS,
        data_origin.unwrap_or(""),
        sv::CDN_KATEX,
        sv::CDN_KATEX,
    )
}

/// `page::landing`'s own output (`html`) with a `<meta
/// http-equiv="Content-Security-Policy">` injected (A2, PR #429 review) —
/// much tighter than [`site_csp`]: this page runs no DuckDB-Wasm and makes
/// no `fetch` at all (every dashboard link is a real navigation to its own
/// launcher), so none of `site_csp`'s jsdelivr/`connect-src`/`wasm-unsafe-
/// eval` entries apply. The one loosening: `script-src 'unsafe-inline'`,
/// for `page::landing`'s own fixed `LANDING_SCRIPT` (9+ dashboards only —
/// the search/filter box), which is first-party, author-independent
/// content, never user data (every `::nav`/`::title`/description/tag/owner
/// `page::landing` prints is HTML-escaped, not inlined into script). A hash
/// source (`'sha256-…'`) would be tighter still and needs no
/// `'unsafe-inline'` at all, but the core crate has no sha2/base64-general-
/// purpose dependency to compute one with — the core crate stays
/// dependency-light by design (see Cargo.toml's own comments).
fn site_landing_csp(html: &str) -> String {
    let csp = "default-src 'none'; script-src 'self' 'unsafe-inline'; \
         style-src 'self' 'unsafe-inline'; \
         img-src 'self' data: https:; font-src 'self' data: https://fonts.gstatic.com; \
         manifest-src 'self'; media-src 'none'; object-src 'none'; frame-src 'none'; \
         base-uri 'self'; form-action 'self'";
    html.replacen(
        "<head>",
        &format!("<head><meta http-equiv=\"Content-Security-Policy\" content=\"{csp}\">"),
        1,
    )
}

/// Print `SUMMARIZE` (types, min/max, approx-distinct, null %) for a source —
/// schema grounding so an author never invents columns.
fn cmd_describe(args: &[String]) {
    let mut src = None;
    let mut dbfile = ":memory:".to_string();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--db" => dbfile = it.next().cloned().unwrap_or(dbfile),
            _ if !a.starts_with("--") => src = Some(a.clone()),
            _ => {}
        }
    }
    let Some(src) = src else {
        eprintln!("usage: dashboard --describe <table|'file.parquet'|read_csv(...)> [--db file]");
        std::process::exit(2);
    };
    let mut cmd = Command::new("duckdb");
    cmd.arg(&dbfile)
        .arg("-c")
        .arg(format!("SUMMARIZE SELECT * FROM {src}"));
    anofox_visualization::host::duckdb_cli::prepare("duckdb", &mut cmd);
    let out = cmd.output().unwrap_or_else(|e| panic!("run duckdb: {e}"));
    if out.status.success() {
        print!("{}", String::from_utf8_lossy(&out.stdout));
    } else {
        eprintln!("{}", String::from_utf8_lossy(&out.stderr));
        std::process::exit(1);
    }
}

/// Run `sql` (one or more statements) in a single fresh duckdb process —
/// `:memory:`, or read-only against `db`; return the last statement's rows
/// (empty for a non-SELECT) or the DuckDB error text. #435: this one-shot,
/// no-session-kept call is no longer how the renderers or `--check` run a
/// dashboard's own statements (see `host::duckdb_cli::SessionRunner`,
/// which backs one persistent process for a whole script instead) — it is
/// now used only for the occasional one-off query that needs no session at
/// all (`policy_diags`'s `duck`, `duck_rows`).
fn run_query(
    db: Option<&str>,
    sql: &str,
) -> Result<Vec<serde_json::Map<String, serde_json::Value>>, String> {
    let mut cmd = Command::new("duckdb");
    match db {
        Some(f) => {
            cmd.arg("-readonly").arg(f);
        }
        None => {
            cmd.arg(":memory:");
        }
    }
    cmd.arg("-json").arg("-c").arg(sql);
    anofox_visualization::host::duckdb_cli::prepare("duckdb", &mut cmd);
    let out = cmd.output().map_err(|e| format!("run duckdb: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).to_string());
    }
    let s = String::from_utf8_lossy(&out.stdout);
    let s = s.trim();
    if s.is_empty() {
        return Ok(Vec::new());
    }
    sql::parse_rows_json(s).map_err(|e| format!("parse duckdb json: {e}"))
}

/// `SET VARIABLE anofox_data = '<dir>'`, resolved to an absolute path — as a
/// locked server's `options.data` sets it, before any dashboard statement
/// runs. Shared by `--check --data`, `dashboard --data` and `dashboard --svg
/// --data`: each runs this as the session's first statement (#435; before
/// #435, each prepended it to its own replayed prelude instead).
fn data_set_stmt(dir: &str) -> String {
    let abs = std::fs::canonicalize(dir).unwrap_or_else(|e| {
        eprintln!("--data {dir}: {e}");
        std::process::exit(2);
    });
    format!(
        "SET VARIABLE anofox_data = '{}';\n",
        abs.to_string_lossy().replace('\'', "''")
    )
}

#[cfg(test)]
mod telemetry_default_tests {
    // #333: `main()` (above) calls `telemetry::ensure_default_once()` before
    // any `duckdb` child process can be spawned, so every anofox extension
    // such a child LOADs inherits this process's environment. That call's
    // pure decision — the part worth unit testing — is
    // `anofox_visualization::host::telemetry::default_value`: see its own
    // tests in `src/host/telemetry.rs`. This test just pins down the two
    // cases this binary cares about, against the same function `main` uses.
    use anofox_visualization::host::telemetry::default_value;

    #[test]
    fn unset_is_set_to_disabled() {
        assert_eq!(default_value(None), Some("1"));
    }

    #[test]
    fn an_explicit_value_including_an_opt_in_is_kept() {
        assert_eq!(default_value(Some("1")), None);
        assert_eq!(default_value(Some("0")), None); // opt back into telemetry
        assert_eq!(default_value(Some("false")), None);
    }
}

#[cfg(test)]
mod site_tests {
    // Unit tests for the pure parts of `--site` (#417): option parsing and
    // the CSP builder. The rest (`load_site_dashboards`, the read-only check,
    // `site_build_nav_picker`, writing the output tree) needs the `duckdb`
    // CLI and a real folder — covered by the manual end-to-end check this PR
    // describes (a real site built, served under a sub-path, and compared
    // side by side with the same folder on the locked server) rather than by
    // a unit test here.
    use super::*;

    #[test]
    fn empty_options_are_the_default() {
        let o = parse_site_options(None).unwrap();
        assert!(o.path_vars.is_empty());
        assert!(o.nav_picker.is_none());
        assert!(o.app.is_none());
        assert!(o.home.is_none());
        assert!(o.theme.is_none());
        let o2 = parse_site_options(Some("  ")).unwrap();
        assert!(o2.path_vars.is_empty());
    }

    #[test]
    fn bad_json_is_refused() {
        assert!(parse_site_options(Some("not json")).is_err());
    }

    #[test]
    fn auth_and_policy_are_refused_for_a_site_with_no_server() {
        let e = parse_site_options(Some(r#"{"auth": {}}"#)).unwrap_err();
        assert!(e.contains("unknown key 'auth'"), "{e}");
        let e = parse_site_options(Some(r#"{"policy": "none"}"#)).unwrap_err();
        assert!(e.contains("unknown key 'policy'"), "{e}");
    }

    #[test]
    fn state_is_refused_for_a_site_with_no_server() {
        // #517 S2, plan §2: a static site cannot store submissions.
        let e = parse_site_options(Some(r#"{"state": {"path": "s.jsonl"}}"#)).unwrap_err();
        assert!(e.contains("cannot store submissions"), "{e}");
    }

    #[test]
    fn path_vars_takes_plain_variable_names_only() {
        let o = parse_site_options(Some(r#"{"path_vars": ["region", "model"]}"#)).unwrap();
        assert_eq!(o.path_vars, vec!["region".to_string(), "model".to_string()]);
        assert!(parse_site_options(Some(r#"{"path_vars": ["not a name"]}"#)).is_err());
        // anofox_data / __claim_* are server-reserved — never a `path_vars` name.
        assert!(parse_site_options(Some(r#"{"path_vars": ["anofox_data"]}"#)).is_err());
        assert!(parse_site_options(Some(r#"{"path_vars": ["__claim_role"]}"#)).is_err());
    }

    #[test]
    fn nav_picker_var_must_be_a_path_vars_entry() {
        let json =
            r#"{"path_vars": ["region"], "nav_picker": {"var": "model", "sql": "SELECT 1, 2"}}"#;
        let e = parse_site_options(Some(json)).unwrap_err();
        assert!(e.contains("must be one of options.path_vars"), "{e}");

        let json = r#"{"path_vars": ["region"], "nav_picker": {"var": "region", "sql": "SELECT 1, 2", "label": "Region"}}"#;
        let o = parse_site_options(Some(json)).unwrap();
        let np = o.nav_picker.unwrap();
        assert_eq!(np.var, "region");
        assert_eq!(np.sql, "SELECT 1, 2");
        assert_eq!(np.label.as_deref(), Some("Region"));
    }

    #[test]
    fn home_must_be_a_plain_dashboard_id() {
        assert!(parse_site_options(Some(r#"{"home": "overview"}"#)).is_ok());
        assert!(parse_site_options(Some(r#"{"home": "../etc/passwd"}"#)).is_err());
    }

    #[test]
    fn theme_is_validated_like_the_locked_server() {
        assert!(parse_site_options(Some(r#"{"theme": "not-a-registered-theme"}"#)).is_err());
        assert!(parse_site_options(Some(r#"{"theme": "light"}"#)).is_ok());
        assert!(parse_site_options(Some(r#"{"theme": 5}"#)).is_err());
    }

    #[test]
    fn app_must_be_a_valid_app_brand_object() {
        assert!(parse_site_options(Some(r#"{"app": {"name": "Demo"}}"#)).is_ok());
        assert!(parse_site_options(Some(r#"{"app": "Demo"}"#)).is_err());
        assert!(parse_site_options(Some(r#"{"app": {"logo": "javascript:alert(1)"}}"#)).is_err());
    }

    #[test]
    fn unknown_keys_are_refused() {
        // Not a real `anofox_serve_dashboards` option under any name — a
        // typo, refused like the locked server would refuse it too (#501).
        let e = parse_site_options(Some(r#"{"max_rowz": 10}"#)).unwrap_err();
        assert!(e.contains("unknown key 'max_rowz'"), "{e}");
        assert!(e.contains("max_rows"), "{e}"); // the real option is suggested
    }

    #[test]
    fn server_only_limits_are_ignored_not_refused() {
        // #501: a locked server's own `options` file, used unedited for
        // `--site`, carries limits that have no effect on a static site.
        // They're skipped, not refused — only truly unknown keys and
        // auth/policy still fail the whole file.
        let o = parse_site_options(Some(r#"{"max_rows": 10, "threads": 2}"#)).unwrap();
        assert_eq!(
            o.ignored,
            vec!["max_rows".to_string(), "threads".to_string()]
        );
    }

    #[test]
    fn a_full_locked_json_builds_and_names_every_ignored_key() {
        // A shape like docs/deployment.md's `anofox_serve_dashboards`
        // options: site keys the publisher cares about, plus every
        // server-only limit the locked server also accepts.
        let json = r#"{
            "max_rows": 10000, "timeout_ms": 5000, "threads": 4, "max_queries": 8,
            "cache_mb": 32, "memory_limit": "1GB", "hosts": ["dash.example.com"],
            "path_vars": ["model"], "home": "overview", "theme": "dark"
        }"#;
        let o = parse_site_options(Some(json)).unwrap();
        assert_eq!(o.path_vars, vec!["model".to_string()]);
        assert_eq!(o.home.as_deref(), Some("overview"));
        assert_eq!(o.theme.as_deref(), Some("dark"));
        assert_eq!(
            o.ignored,
            vec![
                "cache_mb".to_string(),
                "hosts".to_string(),
                "max_queries".to_string(),
                "max_rows".to_string(),
                "memory_limit".to_string(),
                "threads".to_string(),
                "timeout_ms".to_string(),
            ]
        );
    }

    #[test]
    fn site_csp_has_no_frame_ancestors_and_no_nonce() {
        let csp = site_csp(None);
        assert!(!csp.contains("frame-ancestors"), "{csp}");
        assert!(!csp.contains("nonce-"), "{csp}");
        let script_src = csp
            .split(';')
            .find(|p| p.trim().starts_with("script-src"))
            .unwrap();
        for exact in [
            "cdn.jsdelivr.net/npm/@duckdb/duckdb-wasm@",
            "cdn.jsdelivr.net/npm/katex@",
            "cdn.jsdelivr.net/npm/apache-arrow@17.0.0/",
            "cdn.jsdelivr.net/npm/tslib@2.6.3/",
            "cdn.jsdelivr.net/npm/flatbuffers@24.3.25/",
        ] {
            assert!(script_src.contains(exact), "{script_src}");
        }
        // every jsdelivr source is pinned to an exact version — never a bare
        // `cdn.jsdelivr.net/npm/` prefix that would allow any package (A1,
        // PR #429 review)
        assert!(
            !script_src.contains("npm/;") && !script_src.contains("npm/ "),
            "{script_src}"
        );
        assert!(csp.contains("object-src 'none'"), "{csp}");
    }

    #[test]
    fn site_csp_adds_a_remote_data_origin_to_connect_src_only() {
        let csp = site_csp(Some("https://data.example.com"));
        assert!(
            csp.contains("connect-src 'self' https://cdn.jsdelivr.net/npm/@duckdb/duckdb-wasm@"),
            "{csp}"
        );
        assert!(csp.contains("https://data.example.com"), "{csp}");
        // the data origin is a connect-src addition, not a script-src one
        let script_src = csp
            .split(';')
            .find(|p| p.trim().starts_with("script-src"))
            .unwrap();
        assert!(!script_src.contains("data.example.com"), "{script_src}");
    }

    #[test]
    fn data_url_is_detected_and_trimmed() {
        assert!(is_data_url("https://example.com/data"));
        assert!(is_data_url("http://example.com/data"));
        assert!(!is_data_url("/local/path"));
        assert!(!is_data_url("data"));
    }

    /// A fresh scratch directory under the system temp dir, removed on drop
    /// — `copy_dir_recursive`'s own tests need real files (it does real
    /// I/O), not a pure-function stand-in.
    struct TempDir(std::path::PathBuf);
    impl TempDir {
        fn new(tag: &str) -> Self {
            let p = std::env::temp_dir().join(format!(
                "anofox-site-test-{tag}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or_default()
            ));
            std::fs::create_dir_all(&p).unwrap();
            TempDir(p)
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn copy_dir_recursive_refuses_a_symlink() {
        // S1 (PR #429 review): a symlink inside --data must never be
        // followed and copied into a public out/data/ — refused outright.
        let src = TempDir::new("symlink-src");
        let dst = TempDir::new("symlink-dst");
        std::fs::write(src.0.join("plain.txt"), b"ok").unwrap();
        let secret = TempDir::new("symlink-secret");
        std::fs::write(secret.0.join("id_rsa"), b"not for publishing").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&secret.0, src.0.join("sneaky")).unwrap();
        #[cfg(unix)]
        {
            let err = copy_dir_recursive(&src.0, &dst.0).unwrap_err();
            assert!(err.to_string().contains("symlink"), "{err}");
            // Nothing from the symlinked target ever lands in the output.
            assert!(!dst.0.join("sneaky").join("id_rsa").exists());
        }
    }

    #[test]
    fn copy_dir_recursive_copies_a_plain_tree() {
        let src = TempDir::new("plain-src");
        let dst = TempDir::new("plain-dst");
        std::fs::create_dir_all(src.0.join("sub")).unwrap();
        std::fs::write(src.0.join("a.txt"), b"a").unwrap();
        std::fs::write(src.0.join("sub/b.txt"), b"b").unwrap();
        copy_dir_recursive(&src.0, &dst.0).unwrap();
        assert_eq!(std::fs::read(dst.0.join("a.txt")).unwrap(), b"a");
        assert_eq!(std::fs::read(dst.0.join("sub/b.txt")).unwrap(), b"b");
    }

    /// Review of #554 item 1: `--site` refuses to publish an SVG that could
    /// run script or fetch something external if opened directly (no
    /// server there to send the locked route's own sandboxing CSP).
    #[test]
    fn svg_active_reason_catches_script_foreignobject_handlers_js_urls_and_external_hrefs() {
        let plain = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"><rect width="10" height="10" fill="red"/></svg>"#;
        assert_eq!(svg_active_reason(plain), None);

        let script = br#"<svg xmlns="http://www.w3.org/2000/svg"><script>alert(1)</script></svg>"#;
        assert!(svg_active_reason(script)
            .unwrap()
            .contains("disallowed element ('script')"));

        let fo = br#"<svg xmlns="http://www.w3.org/2000/svg"><foreignObject><body xmlns="http://www.w3.org/1999/xhtml">hi</body></foreignObject></svg>"#;
        assert!(svg_active_reason(fo)
            .unwrap()
            .contains("disallowed element ('foreignObject')"));

        let onload = br#"<svg xmlns="http://www.w3.org/2000/svg" onload="alert(1)"><rect width="1" height="1"/></svg>"#;
        assert!(svg_active_reason(onload).unwrap().contains("event-handler"));

        // <a> isn't in the allow-list at all (review of #554 B1: no real
        // need for it in a static asset, and one less href to re-check).
        let anchor = br#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink"><a xlink:href="javascript:alert(1)"><rect width="1" height="1"/></a></svg>"#;
        assert!(svg_active_reason(anchor)
            .unwrap()
            .contains("disallowed element ('a')"));

        // An href on an allowed element, pointed off-document — caught by
        // the generic `external_ref_reason` (its own `javascript:` check)
        // before the href-specific check even runs.
        let jsurl =
            br#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink"><use xlink:href="javascript:alert(1)"/></svg>"#;
        assert!(svg_active_reason(jsurl).unwrap().contains("javascript:"));

        let external = br#"<svg xmlns="http://www.w3.org/2000/svg"><image href="https://evil.example/track.png"/></svg>"#;
        assert!(svg_active_reason(external).unwrap().contains("external"));

        let style = br#"<svg xmlns="http://www.w3.org/2000/svg"><rect width="1" height="1" style="fill:url(https://evil.example/x)"/></svg>"#;
        assert!(svg_active_reason(style).unwrap().contains("url()"));

        let import = br#"<svg xmlns="http://www.w3.org/2000/svg"><rect width="1" height="1" style="background:@import url(evil.css)"/></svg>"#;
        assert!(svg_active_reason(import).unwrap().contains("@import"));

        let doctype = br#"<!DOCTYPE svg><svg xmlns="http://www.w3.org/2000/svg"/>"#;
        assert!(svg_active_reason(doctype).unwrap().contains("DOCTYPE"));

        let pi = br#"<?xml-stylesheet href="x.css"?><svg xmlns="http://www.w3.org/2000/svg"/>"#;
        assert!(svg_active_reason(pi)
            .unwrap()
            .contains("processing instruction"));

        // Allowed: a same-document fragment reference, an embedded raster
        // data: image on <image>, a style url() referencing a local
        // fragment, and a filter primitive.
        let frag = br##"<svg xmlns="http://www.w3.org/2000/svg"><use href="#icon"/></svg>"##;
        assert_eq!(svg_active_reason(frag), None);
        let data_img = br#"<svg xmlns="http://www.w3.org/2000/svg"><image href="data:image/png;base64,iVBORw0KGgo="/></svg>"#;
        assert_eq!(svg_active_reason(data_img), None);
        let style_frag = br##"<svg xmlns="http://www.w3.org/2000/svg"><rect width="1" height="1" style="filter:url(#blur)"/><filter id="blur"><feGaussianBlur stdDeviation="2"/></filter></svg>"##;
        assert_eq!(svg_active_reason(style_frag), None);

        // The repo's own example SVG (web/examples/diagram-pipeline.sql's
        // ::IMAGE data: URI, decoded) must still pass — a deny-to-allow
        // rewrite that broke a real, already-shipped asset would be its
        // own kind of regression. In its own fixture file, not inline
        // here, so its hex colour literals don't trip `tests/theme.rs`'s
        // `rust_page_writers_have_no_colour_literals` (no page writer
        // outside `src/theme.rs` may hardcode a colour).
        let example: &[u8] = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/svg-xss-554/benign-example.svg"
        ));
        assert_eq!(svg_active_reason(example), None);
    }

    /// Review of #554 B1: the previous deny-list (a case-insensitive text
    /// scan) was bypassed by every file here, each exercised live in a
    /// real browser by the reviewer (`t.mjs`, Playwright/Chromium) before
    /// being handed to this PR — each one set `document.title` to a
    /// distinct marker when it ran, proving script execution, not just a
    /// pattern match. Copied verbatim into
    /// `tests/fixtures/svg-xss-554/` as the regression fixtures; this test
    /// is the non-browser half (`svg_active_reason` itself refuses all
    /// six) — the live-browser side stays the reviewer's own `t.mjs`,
    /// outside CI like the rest of `scripts/*-e2e.mjs`.
    #[test]
    fn svg_active_reason_refuses_every_554_b1_bypass() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/svg-xss-554");
        let cases: &[(&str, &str)] = &[
            // xmlns:x bound to the SVG namespace itself, under a prefix
            // other than none — still a real SVG <script>, just not
            // spelled "<script" in the raw text.
            ("ns.svg", "disallowed element ('script')"),
            // xmlns:h bound to the XHTML namespace — an XHTML <script>,
            // needing no <foreignObject> wrapper at all.
            ("h.svg", "outside the SVG namespace"),
            // An internal DTD entity expanding to literal <script> text.
            ("ent.svg", "DOCTYPE"),
            // <animate> targeting href with an entity-encoded javascript:
            // URL, wrapped in an <a> — document order means the <a> (also
            // not in the allow-list) is refused first; either element on
            // its own would already be enough.
            ("anim.svg", "disallowed element ('a')"),
            // <a href=…> with an entity-encoded javascript: URL (<a> is
            // not in the allow-list either).
            ("sp.svg", "disallowed element ('a')"),
            // xmlns:h bound to the XHTML namespace again, this time an
            // <iframe> with a javascript: src.
            ("ifr.svg", "outside the SVG namespace"),
        ];
        for (file, expect) in cases {
            let bytes = std::fs::read(dir.join(file)).unwrap_or_else(|e| panic!("{file}: {e}"));
            let reason = svg_active_reason(&bytes);
            assert!(
                reason.as_deref().is_some_and(|r| r.contains(expect)),
                "{file}: expected a reason containing {expect:?}, got {reason:?}"
            );
        }
    }

    /// Review of #554's second re-review (S1): the first `external_ref_reason`
    /// only checked `style`, leaving every presentation attribute, a CSS
    /// escape and `image-set()` as unguarded fetches — all four verified
    /// live in Chromium (opened directly, served as `image/svg+xml`, no
    /// CSP) by the reviewer. Fixtures copied into `tests/fixtures/svg-xss-554/`.
    #[test]
    fn svg_active_reason_refuses_css_external_refs_in_any_attribute() {
        // Copied verbatim from the reviewer's own scratch probes — the
        // ones they confirmed, live in Chromium, actually request
        // `example.invalid` when the shipped (pre-fix) function returned
        // `None` for each.
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/svg-xss-554");
        let cases: &[(&str, &str)] = &[
            // `fill="url(…)"` — a presentation attribute reaches `url(…)`
            // exactly like `style` does.
            ("presattr.svg", "url()"),
            // `cursor="url(…),auto"` (a fallback list) and `filter="url(…)"`
            // on the same element — either is enough on its own.
            ("cursor.svg", "url()"),
            // `u\72l(` is `url(` to a CSS parser, invisible to a literal
            // "url(" scan — caught by refusing any backslash at all.
            ("cssesc.svg", "backslash"),
            // a URL with no `url(` token at all.
            ("imgset.svg", "image-set("),
        ];
        for (file, expect) in cases {
            let bytes = std::fs::read(dir.join(file)).unwrap_or_else(|e| panic!("{file}: {e}"));
            let reason = svg_active_reason(&bytes);
            assert!(
                reason.as_deref().is_some_and(|r| r.contains(expect)),
                "{file}: expected a reason containing {expect:?}, got {reason:?}"
            );
        }
    }

    /// Review of #554 item 2: `publish_assets` skips dot-entries (not a
    /// failure), refuses a renamed non-image by its real magic bytes (not
    /// just its extension — `fake.png` here), and refuses an active SVG.
    #[test]
    fn publish_assets_skips_dotfiles_and_checks_real_content() {
        let src = TempDir::new("publish-src");
        let dst = TempDir::new("publish-dst");
        std::fs::create_dir_all(src.0.join("sub")).unwrap();
        // A real, inert SVG.
        std::fs::write(
            src.0.join("ok.svg"),
            br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"/>"#,
        )
        .unwrap();
        std::fs::write(src.0.join("sub").join(".hidden.png"), b"\x89PNG\r\n\x1a\n").unwrap();
        std::fs::write(src.0.join(".DS_Store"), b"whatever, no extension at all").unwrap();
        let root = src.0.canonicalize().unwrap();
        publish_assets(
            &root,
            "",
            &dst.0,
            anofox_visualization::image::DEFAULT_MAX_ASSET_BYTES,
        )
        .unwrap();
        assert!(dst.0.join("ok.svg").exists());
        assert!(!dst.0.join("sub").join(".hidden.png").exists());
        assert!(!dst.0.join(".DS_Store").exists());

        // A non-image renamed to an allowed extension fails the whole
        // publish (not silently skipped like a dotfile).
        let src2 = TempDir::new("publish-src2");
        let dst2 = TempDir::new("publish-dst2");
        std::fs::write(src2.0.join("fake.png"), b"not a png").unwrap();
        let root2 = src2.0.canonicalize().unwrap();
        let err = publish_assets(
            &root2,
            "",
            &dst2.0,
            anofox_visualization::image::DEFAULT_MAX_ASSET_BYTES,
        )
        .unwrap_err();
        assert!(
            err.contains("not-an-image") || err.contains("refused"),
            "{err}"
        );

        // An active SVG fails the whole publish too.
        let src3 = TempDir::new("publish-src3");
        let dst3 = TempDir::new("publish-dst3");
        std::fs::write(
            src3.0.join("evil.svg"),
            br#"<svg xmlns="http://www.w3.org/2000/svg"><script>alert(1)</script></svg>"#,
        )
        .unwrap();
        let root3 = src3.0.canonicalize().unwrap();
        let err = publish_assets(
            &root3,
            "",
            &dst3.0,
            anofox_visualization::image::DEFAULT_MAX_ASSET_BYTES,
        )
        .unwrap_err();
        assert!(err.contains("active SVG"), "{err}");
    }

    /// Review of #554 N3: `--site` honours `options.max_asset_bytes`
    /// rather than always using [`anofox_visualization::image::DEFAULT_MAX_ASSET_BYTES`]
    /// — parsed, clamped the same way `LockedOptions::parse` clamps it,
    /// and no longer in `ignored` (it's a `Site` key now, not
    /// `ServerOnly`).
    #[test]
    fn max_asset_bytes_is_parsed_clamped_and_honoured_by_site() {
        let o = parse_site_options(Some(r#"{"max_asset_bytes": 1024}"#)).unwrap();
        assert_eq!(o.max_asset_bytes, Some(1024));
        assert!(o.ignored.is_empty(), "{:?}", o.ignored);

        let o = parse_site_options(Some(r#"{"max_asset_bytes": 999999999}"#)).unwrap();
        assert_eq!(o.max_asset_bytes, Some(64 << 20));

        assert!(parse_site_options(Some(r#"{"max_asset_bytes": 0}"#)).is_err());
        assert!(parse_site_options(Some(r#"{"max_asset_bytes": -1}"#)).is_err());
        assert!(parse_site_options(Some(r#"{"max_asset_bytes": "5"}"#)).is_err());
    }

    /// `publish_assets` itself (not just `parse_site_options`) actually
    /// uses whatever cap it's given — the same file passes under the
    /// default and is refused under a smaller one, confirming `cmd_site`'s
    /// plumbing (`opts.max_asset_bytes.unwrap_or(DEFAULT_MAX_ASSET_BYTES)`)
    /// has something real to change.
    #[test]
    fn publish_assets_honours_whatever_cap_it_is_given() {
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"><rect width="10" height="10"/></svg>"#;
        let src = TempDir::new("site-cap-src");
        std::fs::write(src.0.join("ok.svg"), svg).unwrap();
        let root = src.0.canonicalize().unwrap();

        let dst = TempDir::new("site-cap-dst-ok");
        publish_assets(
            &root,
            "",
            &dst.0,
            anofox_visualization::image::DEFAULT_MAX_ASSET_BYTES,
        )
        .unwrap();
        assert!(dst.0.join("ok.svg").exists());

        let dst2 = TempDir::new("site-cap-dst-too-small");
        let err = publish_assets(&root, "", &dst2.0, 10).unwrap_err();
        assert!(
            err.contains("too-large") || err.contains("refused"),
            "{err}"
        );
    }

    #[test]
    fn site_data_cannot_be_home_or_the_filesystem_root() {
        // S1: --site --data shares `too_broad` with the locked server's
        // own `options.data` (full behaviour re-tested in
        // src/host/serving.rs; this just confirms --site actually wired it
        // in, since a `--data` check that exists but is never called is as
        // good as none).
        assert!(sv::too_broad(std::path::Path::new("/")).is_some());
        if let Some(home) =
            std::env::var_os("HOME").and_then(|h| std::path::PathBuf::from(h).canonicalize().ok())
        {
            assert!(sv::too_broad(&home).is_some());
        }
    }
}
