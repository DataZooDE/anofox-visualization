//! `dashboard` — the viewer. Takes a `.sql` file of annotated
//! queries, runs them through the DuckDB CLI, renders each annotated `SELECT`
//! with the anofox-visualization core (ggplot-rs), and writes an interactive HTML dashboard.
//!
//! Usage:  cargo run --bin dashboard -- dashboards/sessions.sql [out.html]
//!
//! The SQL parsing (`::ROLE` casts, comments, statement splitting) lives in
//! `anofox_visualization::sql`, shared with the wasm browser build.

use anofox_visualization::lint::{self, Severity};
use anofox_visualization::{render, sql, Role};
use std::process::Command;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--check") => cmd_check(&args[1..]),
        Some("--describe") => cmd_describe(&args[1..]),
        Some("--roles") => print!("{}", anofox_visualization::roles::text()),
        None | Some("--help") | Some("-h") => {
            eprintln!(
                "usage:\n  \
                 dashboard <file.sql> [out.html]         render to interactive HTML\n  \
                 dashboard --check <file.sql> [--json]   lint: which panels break / are empty\n  \
                 dashboard --describe <source> [--db f]  schema + stats (SUMMARIZE)\n  \
                 dashboard --roles                       list every valid ::ROLE token"
            );
            std::process::exit(2);
        }
        Some(_) => cmd_render(&args),
    }
}

/// Render an annotated `.sql` file to an interactive HTML dashboard.
fn cmd_render(args: &[String]) {
    let path = args[0].clone();
    let out = args.get(1).cloned().unwrap_or_else(|| "dashboard.html".to_string());
    let script = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"));

    let db = std::env::temp_dir().join(format!("anofox_{}.db", std::process::id()));
    let _ = std::fs::remove_file(&db);
    let db = db.to_string_lossy().to_string();

    let mut panels = String::new();
    let mut n = 0;
    for p in sql::plan(&script) {
        if p.setup {
            run(&db, &p.sql, false); // DDL/insert — run for effect
            continue;
        }
        // Inputs + layout directives are interactive — the browser builder /
        // `serve` handle them; the static CLI output skips them.
        if p.roles.iter().any(|(_, r)| {
            matches!(
                r,
                Role::Input(_)
                    | Role::Columns
                    | Role::GroupStart
                    | Role::GroupEnd
                    | Role::Span
                    | Role::Tab
            )
        }) {
            continue;
        }
        let json = run(&db, &p.sql, true);
        let rows: Vec<serde_json::Map<String, serde_json::Value>> =
            serde_json::from_str(json.trim()).unwrap_or_default();
        // A label-only panel is a spanning section heading, not a card.
        if p.roles.len() == 1 && matches!(p.roles[0].1, Role::Label) {
            let text = rows
                .first()
                .and_then(|r| r.values().next())
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let esc = text
                .replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;");
            panels.push_str(&format!("<h2 class=\"section\">{esc}</h2>"));
            continue;
        }
        let cols = sql::columns_from_rows(&rows, &p.roles);
        let svg = render(&cols, 460, 300).unwrap_or_else(|e| format!("<pre>error: {e}</pre>"));
        panels.push_str(&format!("<figure class=\"panel\">{svg}</figure>"));
        n += 1;
    }
    let _ = std::fs::remove_file(&db);

    std::fs::write(&out, page(&path, n, &panels)).unwrap_or_else(|e| panic!("write {out}: {e}"));
    println!("wrote {out} ({n} panels) — open it in a browser");
}

/// Lint a dashboard: report panels that error, render wrong, are silently
/// dropped to setup, or return no rows. Exit 1 if there are errors.
fn cmd_check(args: &[String]) {
    let json_out = args.iter().any(|a| a == "--json");
    // --db <file>: validate against a prebuilt (read-only) database — its views
    // and tables are already in scope, so the dashboards are pure queries.
    let mut db: Option<String> = None;
    let mut path = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--db" => db = it.next().cloned(),
            "--json" => {}
            _ if !a.starts_with("--") => path = Some(a.clone()),
            _ => {}
        }
    }
    let Some(path) = path else {
        eprintln!("usage: dashboard --check <file.sql> [--json] [--db prebuilt.duckdb]");
        std::process::exit(2);
    };
    let script = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        eprintln!("read {path}: {e}");
        std::process::exit(2);
    });

    // Each call runs a full script (prelude + one statement) in a fresh session
    // (in-memory, or read-only against --db) — the linter replays session state.
    let diags = lint::check(&script, |sql| run_query(db.as_deref(), sql));

    let errors = diags.iter().filter(|d| d.severity == Severity::Error).count();
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
        println!("{{\"ok\":{},\"diagnostics\":[{}]}}", errors == 0, items.join(","));
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
    if errors > 0 {
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
    serde_json::from_str(s).map_err(|e| format!("parse duckdb json: {e}"))
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

/// The dashboard HTML shell + interactive hover layer (shared visual language
/// with the browser builder).
fn page(path: &str, n: usize, panels: &str) -> String {
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\">\
<title>anofox-visualization dashboard</title><style>{STYLE}</style></head>\
<body><h1>anofox-visualization dashboard</h1><div class=\"src\">rendered from <code>{path}</code> · {n} panels</div>\
<div class=\"grid\">{panels}</div><div id=\"dp-tip\" class=\"dp-tip\"></div>\
<script>{SCRIPT}</script></body></html>"
    )
}

const STYLE: &str = r#":root{--bg:#f4f6f9;--card:#fff;--ink:#1f2937;--muted:#6b7280;--line:#e5e7eb}
body{font:15px/1.55 system-ui,-apple-system,'Segoe UI',Roboto,sans-serif;background:var(--bg);color:var(--ink);margin:0;padding:2rem}
h1{font-size:1.35rem;font-weight:650;letter-spacing:-.01em;margin:0 0 .25rem}
.src{color:var(--muted);font-size:.85rem;margin-bottom:1.5rem}
.grid{display:grid;grid-template-columns:repeat(auto-fit,minmax(440px,1fr));gap:1.1rem;align-content:start}
.section{grid-column:1/-1;margin:.6rem 0 -.4rem;font-size:1.1rem;font-weight:650;letter-spacing:-.01em;color:var(--ink)}
.panel{margin:0;background:var(--card);border:1px solid var(--line);border-radius:14px;padding:1rem 1.1rem;box-shadow:0 1px 2px rgba(16,24,40,.04),0 1px 3px rgba(16,24,40,.06)}
.panel svg{width:100%;height:auto;display:block}
.dp-tip{position:fixed;pointer-events:none;background:#111827;color:#fff;padding:.35rem .55rem;border-radius:7px;font-size:.8rem;font-weight:500;box-shadow:0 4px 12px rgba(0,0,0,.25);opacity:0;transform:translateY(2px);transition:opacity .09s,transform .09s;z-index:20;white-space:nowrap}
.dp-tip.show{opacity:1;transform:translateY(0)}
.dp-hit{transition:filter .1s}.dp-hit:hover{filter:brightness(1.09) saturate(1.05)}"#;

const SCRIPT: &str = r#"(function(){
  var tip=document.getElementById('dp-tip'), selected=null;
  var marks=[].slice.call(document.querySelectorAll('.panel svg rect,.panel svg circle,.panel svg polygon,.panel svg polyline'))
    .filter(function(el){var t=el.querySelector('title');return t&&t.textContent.trim();});
  function apply(){marks.forEach(function(el){
    var s=el.getAttribute('data-series'); el.style.opacity=(!selected||s===selected)?'':'0.15';});}
  marks.forEach(function(el){
    var t=el.querySelector('title'), txt=t.textContent;
    var series=txt.indexOf(': ')>=0?txt.slice(0,txt.lastIndexOf(': ')):txt;
    el.removeChild(t); el.setAttribute('data-series',series); el.classList.add('dp-hit'); el.style.cursor='pointer';
    el.addEventListener('mouseenter',function(){tip.textContent=txt;tip.classList.add('show');});
    el.addEventListener('mousemove',function(e){tip.style.left=(e.clientX+14)+'px';tip.style.top=(e.clientY+14)+'px';});
    el.addEventListener('mouseleave',function(){tip.classList.remove('show');});
    el.addEventListener('click',function(e){e.stopPropagation();selected=(selected===series)?null:series;apply();});
  });
  document.addEventListener('click',function(){if(selected!==null){selected=null;apply();}});
})();"#;
