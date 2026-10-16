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
    dashboard, error_pre, page, render_with, roles, sql, RenderOptions, Role,
};
use std::process::Command;
use std::sync::Arc;

const USAGE: &str = "usage:\n  \
     dashboard <file.sql> [out.html] [--theme T] [--dark]\n  \
                                             render to interactive HTML (T: a built-in theme,\n  \
                                             'acme+dark', or a theme .json file)\n  \
     dashboard --svg <file.sql> [out.svg] [--theme T] [--dark] [--width N]\n  \
                                             render the whole dashboard headless to one SVG\n  \
     dashboard --check <file.sql> [--json]   lint: which panels break / are empty\n  \
     dashboard --describe <source> [--db f]  schema + stats (SUMMARIZE)\n  \
     dashboard --roles                       list every valid ::ROLE token\n  \
     dashboard --themes                      list the built-in themes\n  \
     dashboard --theme-css <T> [--dark]      print a theme's CSS design tokens\n  \
     dashboard --version                     print the version\n\n\
     Queries run through the `duckdb` CLI on PATH.";

fn main() {
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
        Some("--version") | Some("-V") => println!("dashboard {}", env!("CARGO_PKG_VERSION")),
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
/// `dashboard <file.sql> [out.html] [--theme NAME|file.json] [--dark]`: the
/// script's own `::THEME` wins over `--theme`; `--dark` picks the dark
/// variant of a theme that follows the viewer.
fn cmd_render(args: &[String]) {
    let mut pos: Vec<String> = Vec::new();
    let mut theme_arg: Option<String> = None;
    let mut variant = Variant::Light;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--theme" => theme_arg = it.next().map(|v| theme_spec_arg(v)),
            "--dark" => variant = Variant::Dark,
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

    let db = std::env::temp_dir().join(format!("anofox_{}.db", std::process::id()));
    let _ = std::fs::remove_file(&db);
    let db = db.to_string_lossy().to_string();

    // Phase 1: run the script in order; collect chrome directives and the
    // panels' rows (a ::THEME anywhere applies to every panel).
    let mut chrome = DashboardChrome::default();
    type Rows = Result<Vec<serde_json::Map<String, serde_json::Value>>, String>;
    let mut queried: Vec<(sql::Panel, Rows)> = Vec::new();
    // Every `duckdb` call is a fresh session: bound variables (a script's own
    // SET VARIABLE, the inputs' defaults) are replayed before each query.
    let mut vars = String::new();
    for p in sql::plan(&script) {
        if p.setup {
            if inputs::is_set_variable(&p.sql) {
                vars.push_str(&format!("{};\n", p.sql.trim().trim_end_matches(';')));
            } else {
                run(&db, &p.sql, false); // DDL/insert — run for effect
            }
            continue;
        }
        // An input renders nothing in the static output, but binds its default
        // (as the browser does on first load) for the getvariable() panels.
        if let Some(kind) = inputs::input_kind(&p.roles) {
            let json = run(&db, &format!("{vars}{}", p.sql), true);
            if let Ok(rows) = sql::parse_rows_json(json.trim()) {
                for set in inputs::input_defaults(kind, inputs::input_index(&p.roles), &rows) {
                    vars.push_str(&set);
                    vars.push_str(";\n");
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
            let json = run(&db, &format!("{vars}{}", p.sql), true);
            let cell = |v: &serde_json::Value| match v {
                serde_json::Value::String(s) => Some(s.clone()),
                serde_json::Value::Null => None,
                v => Some(v.to_string()),
            };
            // The first row's cells (an ::ASOF also reads its 'stale=…' cell).
            let cells = sql::parse_rows_json(&json)
                .ok()
                .and_then(|rows| rows.first().map(|r| r.values().map(cell).collect()))
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
        let json = run(&db, &format!("{vars}{}", p.sql), true);
        let rows = sql::parse_rows_json(&json);
        queried.push((p, rows));
    }
    let _ = std::fs::remove_file(&db);

    let set = chrome.resolve(theme_arg.as_deref()).unwrap_or_else(|e| {
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
    for (p, rows) in queried {
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
        let cols = sql::columns_from_rows(&rows, &p.roles);
        let svg = render_with(&cols, 460, 300, &opts)
            .unwrap_or_else(|e| error_pre(&format!("error: {e}")));
        panels.push_str(&format!("<figure class=\"panel\">{svg}</figure>"));
        n += 1;
    }

    let html = page::render(&page::StaticPage {
        theme: &t,
        fallback_title: "anofox-visualization dashboard",
        subtitle_html: format!(
            "rendered from <code>{}</code> · {n} panels",
            escape_xml(&path)
        ),
        head_extra: String::new(),
        controls_html: String::new(),
        panels_html: panels,
    });
    std::fs::write(&out, html).unwrap_or_else(|e| panic!("write {out}: {e}"));
    println!("wrote {out} ({n} panels) — open it in a browser");
}

/// The DuckDB CLI as a headless data provider (one temp database file, so
/// setup statements persist between calls).
struct CliProvider {
    db: String,
    /// `SET VARIABLE`s replayed before every query (each `duckdb` call is a
    /// fresh session; tables persist in the db file, variables don't).
    vars: String,
}

impl dashboard::DataProvider for CliProvider {
    fn execute(&mut self, sql: &str) -> Result<(), String> {
        if inputs::is_set_variable(sql) {
            self.vars
                .push_str(&format!("{};\n", sql.trim().trim_end_matches(';')));
        } else {
            run(&self.db, sql, false);
        }
        Ok(())
    }
    fn query(
        &mut self,
        sql: &str,
    ) -> Result<Vec<(String, Vec<anofox_visualization::Value>)>, String> {
        let json = run(&self.db, &format!("{}{sql}", self.vars), true);
        let rows = sql::parse_rows_json(json.trim())?;
        Ok(sql::result_columns(&rows))
    }
}

/// `dashboard --svg <file.sql> [out.svg] [--theme T] [--dark] [--width N]`:
/// the whole dashboard (KPIs, tables, charts, header, credit) as one SVG.
fn cmd_svg(args: &[String]) {
    let mut pos: Vec<String> = Vec::new();
    let mut o = dashboard::DashboardOptions::default();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--theme" => o.theme = it.next().map(|v| theme_spec_arg(v)),
            "--dark" => o.variant = Variant::Dark,
            "--width" => o.width = it.next().and_then(|v| v.parse().ok()).unwrap_or(o.width),
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
    let db = std::env::temp_dir().join(format!("anofox_svg_{}.db", std::process::id()));
    let _ = std::fs::remove_file(&db);
    let mut p = CliProvider {
        db: db.to_string_lossy().to_string(),
        vars: String::new(),
    };
    let svg = dashboard::render_dashboard_svg(&script, &mut p, &o);
    let _ = std::fs::remove_file(&db);
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
/// dropped to setup, or return no rows. Exit 1 if there are errors.
fn cmd_check(args: &[String]) {
    let json_out = args.iter().any(|a| a == "--json");
    // --db <file>: validate against a prebuilt (read-only) database — its views
    // and tables are already in scope, so the dashboards are pure queries.
    let mut db: Option<String> = None;
    let mut path = None;
    let mut design = true; // --no-design turns off the design/* advisory pass
    let mut strict = false; // --strict makes design/* warnings fail the exit code
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--db" => db = it.next().cloned(),
            "--json" => {}
            "--no-design" => design = false,
            "--strict" => strict = true,
            _ if !a.starts_with("--") => path = Some(a.clone()),
            _ => {}
        }
    }
    let Some(path) = path else {
        eprintln!(
            "usage: dashboard --check <file.sql> [--json] [--db prebuilt.duckdb] \
             [--strict] [--no-design]"
        );
        std::process::exit(2);
    };
    let script = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        eprintln!("read {path}: {e}");
        std::process::exit(2);
    });

    // Each call runs a full script (prelude + one statement) in a fresh session
    // (in-memory, or read-only against --db) — the linter replays session state.
    let diags = lint::check_opts(
        &script,
        |sql| run_query(db.as_deref(), sql),
        lint::LintOptions { design },
    );

    let errors = diags
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .count();
    // --strict: a design/* advisory also fails the exit code (for CI gating).
    let design_hits = diags
        .iter()
        .filter(|d| d.code.starts_with("design/"))
        .count();
    let failed = errors > 0 || (strict && design_hits > 0);
    if json_out {
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
        println!(
            "{{\"ok\":{},\"diagnostics\":[{}]}}",
            !failed,
            items.join(",")
        );
    } else {
        for d in &diags {
            println!(
                "{}: [stmt {}] {} — {}\n    {}",
                d.severity.label(),
                d.stmt,
                d.code,
                d.message,
                d.sql
            );
        }
        println!("\n{}", lint::summary(&diags));
    }
    if failed {
        std::process::exit(1);
    }
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
    let out = Command::new("duckdb")
        .arg(&dbfile)
        .arg("-c")
        .arg(format!("SUMMARIZE SELECT * FROM {src}"))
        .output()
        .unwrap_or_else(|e| panic!("run duckdb: {e}"));
    if out.status.success() {
        print!("{}", String::from_utf8_lossy(&out.stdout));
    } else {
        eprintln!("{}", String::from_utf8_lossy(&out.stderr));
        std::process::exit(1);
    }
}

/// Run a full script in a fresh in-memory duckdb session; return the last
/// statement's rows (empty for a non-SELECT) or the DuckDB error text. The
/// prelude the linter prepends contains only non-SELECT statements, so the only
/// JSON on stdout is the final statement's.
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
    let out = cmd
        .arg("-json")
        .arg("-c")
        .arg(sql)
        .output()
        .map_err(|e| format!("run duckdb: {e}"))?;
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

fn run(db: &str, sql: &str, json: bool) -> String {
    let mut cmd = Command::new("duckdb");
    cmd.arg(db);
    if json {
        cmd.arg("-json");
    }
    let out = cmd
        .arg("-c")
        .arg(sql)
        .output()
        .unwrap_or_else(|e| panic!("run duckdb: {e}"));
    if !out.status.success() {
        eprintln!(
            "duckdb error on: {sql}\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
        return String::new();
    }
    String::from_utf8_lossy(&out.stdout).to_string()
}
