//! `dashboard` — the viewer. Takes a `.sql` file of annotated
//! queries, runs them through the DuckDB CLI, renders each annotated `SELECT`
//! with the anofox-visualization core (ggplot-rs), and writes an interactive HTML dashboard.
//!
//! Usage:  cargo run --bin dashboard -- dashboards/sessions.sql [out.html]
//!
//! The SQL parsing (`::ROLE` casts, comments, statement splitting) lives in
//! `anofox_visualization::sql`, shared with the wasm browser build.

use anofox_visualization::format::{escape_xml, Locale};
use anofox_visualization::inputs;
use anofox_visualization::lint::{self, Severity};
use anofox_visualization::theme::{self, DashboardChrome, Variant};
use anofox_visualization::{
    dashboard, error_pre, page, render_with_title, roles, sql, track, RenderOptions, Role,
};
use std::process::Command;
use std::sync::Arc;

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
     dashboard --describe <source> [--db f]  schema + stats (SUMMARIZE)\n  \
     dashboard --roles                       list every valid ::ROLE token\n  \
     dashboard --themes                      list the built-in themes\n  \
     dashboard --theme-css <T> [--dark]      print a theme's CSS design tokens\n  \
     dashboard --version                     print the version\n\n\
     Queries run through the `duckdb` CLI on PATH. The render and --svg keep one\n  \
     DuckDB session's semantics across statements (SET VARIABLE, TEMP views/tables,\n  \
     and other setup all stay in scope for every later statement) by replaying the\n  \
     script's own prelude before each `duckdb` call, exactly as --check does.";

fn main() {
    // Owner decision (#333): the anofox extensions this binary's `duckdb`
    // child processes may LOAD default to no telemetry. Must run before any
    // thread or `duckdb` child exists — first line of `main`, see
    // `host::telemetry` for the default and its thread-safety reasoning.
    anofox_visualization::host::telemetry::ensure_default_once();
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--check") => cmd_check(&args[1..]),
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

    let (html, n) = render_dashboard_html(
        &script,
        &path,
        data.as_deref(),
        theme_arg.as_deref(),
        variant,
    );
    std::fs::write(&out, html).unwrap_or_else(|e| panic!("write {out}: {e}"));
    println!("wrote {out} ({n} panels) — open it in a browser");
}

/// Render one annotated dashboard script to a self-contained HTML page, in
/// one DuckDB session: every statement runs with everything before it
/// (setup DDL, `SET VARIABLE`, `LOAD`, TEMP views/tables, input defaults)
/// still in scope, in script order — the same session semantics as the
/// locked server's cold path and `dashboard --check` (#414). There is no
/// long-lived `duckdb` connection to hold that state between the separate
/// `duckdb` CLI calls this binary makes, so it is simulated the way
/// [`lint::check_opts`] does: a growing `prelude` of everything run so far
/// is replayed, in full, before every later statement.
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
/// Returns the HTML and the number of rendered panels.
fn render_dashboard_html(
    script: &str,
    path_label: &str,
    data: Option<&str>,
    theme_arg: Option<&str>,
    variant: Variant,
) -> (String, usize) {
    // Phase 1: run the script in order; collect chrome directives and the
    // panels' rows (a ::THEME anywhere applies to every panel).
    let mut chrome = DashboardChrome::default();
    type Rows = Result<Vec<serde_json::Map<String, serde_json::Value>>, String>;
    let mut queried: Vec<(sql::Panel, Rows)> = Vec::new();
    // `SET VARIABLE anofox_data = …` opens the session, before the script's
    // own statements — as `--data` does for `--check` and a locked server.
    let mut prelude = data.map(data_set_stmt).unwrap_or_default();
    let mut exec = |sql: &str| run_query(None, sql);
    let mut sections = sql::requires::Sections::default();
    // A static HTML render shows only a ::VIEW group's first shown
    // alternative (#214) — the other alternatives' non-setup statements
    // never even run.
    let mut view_pick = sql::views::StaticPick::default();
    for p in sql::plan(script) {
        // A ::TAB / ::VIEW alternative whose `-- @requires` input is missing
        // is hidden: none of its panels are drawn (the tabs themselves are
        // flattened here).
        let hidden = sections.step(&p, |reqs| {
            lint::unmet_requirement(&mut exec, &prelude, reqs).missing
        });
        if hidden.is_some() || view_pick.drop_panel(&p) {
            continue;
        }
        if p.setup {
            // Genuine setup (DDL / SET / TEMP views and tables / …): run it
            // with everything before it in scope, then keep it in scope too
            // — exactly `lint::check_opts`'s prelude.
            match exec(&format!("{prelude}{};", p.sql)) {
                Ok(_) => prelude.push_str(&format!("{};\n", p.sql.trim().trim_end_matches(';'))),
                Err(e) => eprintln!("duckdb error on: {}\n{e}", p.sql),
            }
            continue;
        }
        // An input renders nothing in the static output, but binds its default
        // (as the browser does on first load) for the getvariable() panels.
        if let Some(kind) = inputs::input_kind(&p.roles) {
            if let Ok(rows) = exec(&format!("{prelude}{};", p.sql)) {
                for set in inputs::input_defaults(kind, inputs::input_index(&p.roles), &rows) {
                    prelude.push_str(&set);
                    prelude.push_str(";\n");
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
            let rows = exec(&format!("{prelude}{};", p.sql)).unwrap_or_default();
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
        let section = p.roles.iter().any(|(_, r)| *r == Role::Section);
        // Layout directives are interactive — the browser builder / `serve`
        // handle them; the static CLI output skips them (role registry).
        if roles::is_directive_panel(&p.roles) && !section {
            continue;
        }
        let rows = exec(&format!("{prelude}{};", p.sql));
        queried.push((p, rows));
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
        ..RenderOptions::default()
    };

    // Phase 2: render.
    let mut panels = String::new();
    let mut n = 0;
    let mut queried = queried.into_iter().peekable();
    while let Some((p, rows)) = queried.next() {
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
        // A label-only panel / ::SECTION is a spanning section heading, not a card.
        let section = p.roles.iter().any(|(_, r)| *r == Role::Section);
        if section || (p.roles.len() == 1 && matches!(p.roles[0].1, Role::Label)) {
            let text = rows
                .first()
                .and_then(|r| r.values().next())
                .and_then(|v| v.as_str())
                .unwrap_or("");
            panels.push_str(&format!("<h2 class=\"section\">{}</h2>", escape_xml(text)));
            continue;
        }
        let cols = anofox_visualization::name_columns(
            sql::columns_from_rows(&rows, &p.roles),
            &p.roles,
            &p.names,
        );
        // ::TRACKs right after a chart sit under it, in its figure.
        let mut tracks = Vec::new();
        while let Some((q, r)) = queried.next_if(|(q, _)| track::is_track(&q.roles)) {
            if let Ok(r) = r {
                tracks.push((sql::columns_from_rows(&r, &q.roles), 0));
            }
        }
        // Its ::TITLE bar, as on a dashboard card (#232).
        let svg = render_with_title(&cols, &tracks, 460, 300, &opts)
            .unwrap_or_else(|e| error_pre(&format!("error: {e}")));
        panels.push_str(&format!("<figure class=\"panel\">{svg}</figure>"));
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

/// The DuckDB CLI as a headless [`dashboard::DataProvider`]: each call is a
/// fresh `duckdb :memory:` process, so a long-lived connection's session
/// state (`SET VARIABLE`, `LOAD`, TEMP views/tables, any other setup) is
/// simulated by replaying a growing `prelude` of everything run so far
/// before every statement — the same trick [`render_dashboard_html`] and
/// `--check` use (#414), so a `::TEMP VIEW`/`SET VARIABLE` a panel depends
/// on is still in scope however many statements separate them.
struct CliProvider {
    prelude: String,
}

impl dashboard::DataProvider for CliProvider {
    fn execute(&mut self, sql: &str) -> Result<(), String> {
        run_query(None, &format!("{}{sql};", self.prelude))?;
        self.prelude
            .push_str(&format!("{};\n", sql.trim().trim_end_matches(';')));
        Ok(())
    }
    fn query(
        &mut self,
        sql: &str,
    ) -> Result<Vec<(String, Vec<anofox_visualization::Value>)>, String> {
        let rows = run_query(None, &format!("{}{sql};", self.prelude))?;
        Ok(sql::result_columns(&rows))
    }
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
    // `anofox_data` opens the session, before the script's own statements.
    let mut p = CliProvider {
        prelude: data.as_deref().map(data_set_stmt).unwrap_or_default(),
    };
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
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--db" => db = it.next().cloned(),
            "--data" => data = it.next().cloned(),
            "--policy" => policy = it.next().cloned(),
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
             [--policy policy.json] [--data dir] [--strict] [--no-design]"
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
    let lint_one = |script: &str| -> Vec<lint::Diagnostic> {
        // Each call runs a full script (prelude + one statement) in a fresh session
        // (in-memory, or read-only against --db) — the linter replays session state.
        let mut diags = lint::check_opts(
            script,
            |sql| run_query(db.as_deref(), &format!("{data_set}{sql}")),
            lint::LintOptions { design },
        );
        if let (Some(p), Some(db)) = (&policy, db.as_deref()) {
            diags.extend(policy_diags(script, db, p));
        }
        diags
    };
    // (file, its diagnostics): one entry for a file, one per .sql for a folder
    let mut files: Vec<(String, Vec<lint::Diagnostic>)> = Vec::new();
    let folder = std::path::Path::new(&path).is_dir();
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
            files.push((n.clone(), d));
        }
    } else {
        let script = read(&path);
        files.push((path.clone(), lint_one(&script)));
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

/// Run a full script (a replayed prelude + one statement) in a fresh duckdb
/// session — `:memory:`, or read-only against `db` (the linter's `--db`);
/// return the last statement's rows (empty for a non-SELECT) or the DuckDB
/// error text. The prelude only ever holds non-SELECT statements, so the
/// only JSON on stdout is the final statement's. Shared by `--check`
/// ([`lint::check_opts`]'s `run_query` closure) and the headless renderers
/// ([`render_dashboard_html`], [`CliProvider`]), which replay their own
/// growing prelude the same way to keep one DuckDB session's semantics
/// across separate `duckdb` CLI calls (#414).
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
/// --data` (#414): each prepends this to its own replayed prelude.
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
