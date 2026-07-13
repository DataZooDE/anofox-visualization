//! `dashboard` — the Shaper-style viewer. Takes a `.sql` file of annotated
//! queries, runs them through the DuckDB CLI, renders each annotated `SELECT`
//! with the anofox-visualization core (ggplot-rs), and writes an interactive HTML dashboard.
//!
//! Usage:  cargo run --bin dashboard -- dashboards/sessions.sql [out.html]
//!
//! The SQL parsing (`::ROLE` casts, comments, statement splitting) lives in
//! `anofox_visualization::sql`, shared with the wasm browser build.

use anofox_visualization::{render, sql, Role};
use std::process::Command;

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().unwrap_or_else(|| {
        eprintln!("usage: dashboard <file.sql> [out.html]");
        std::process::exit(2)
    });
    let out = args.next().unwrap_or_else(|| "dashboard.html".to_string());
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
