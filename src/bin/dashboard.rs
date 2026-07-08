//! `dashboard` — the Shaper-style viewer. Takes a `.sql` file of annotated
//! queries, runs them through the DuckDB CLI, renders each annotated `SELECT`
//! with the duckplot core (ggplot-rs), and writes an HTML dashboard.
//!
//! Usage:  cargo run --bin dashboard -- dashboards/sessions.sql [out.html]
//!
//! Statements *without* a `::ROLE` cast (e.g. `CREATE TABLE …`) are setup and run
//! against a shared temp database; statements *with* roles become panels.

use duckplot::{parse_role, render, Column, Role};
use ggplot_rs::prelude::Value;
use std::process::Command;

const ROLES: &[&str] = &[
    "XAXIS", "CATEGORY", "SERIES", "COLOR", "COLOUR", "LABEL", "TITLE", "BARCHART", "BAR",
    "BARCHART_STACKED", "BAR_STACKED", "STACKED_BAR", "LINECHART", "LINE", "AREACHART", "AREA",
    "SCATTER", "POINT", "SCATTERCHART",
];

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().unwrap_or_else(|| {
        eprintln!("usage: dashboard <file.sql> [out.html]");
        std::process::exit(2)
    });
    let out = args.next().unwrap_or_else(|| "dashboard.html".to_string());
    let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    let sql = strip_line_comments(&raw);

    let db = std::env::temp_dir().join(format!("duckplot_{}.db", std::process::id()));
    let _ = std::fs::remove_file(&db);
    let db = db.to_string_lossy().to_string();

    let mut panels = String::new();
    let mut n = 0;
    for stmt in split_statements(&sql) {
        let stmt = stmt.trim();
        if stmt.is_empty() {
            continue;
        }
        let (rewritten, roles) = rewrite(stmt);
        if roles.is_empty() {
            // setup statement (DDL/insert) — run it, keep no output
            run(&db, stmt, false);
            continue;
        }
        let json = run(&db, &rewritten, true);
        let cols = build_columns(&json, &roles);
        let svg = render(&cols, 460, 300).unwrap_or_else(|e| format!("<pre>error: {e}</pre>"));
        panels.push_str(&format!("<figure class=\"panel\">{svg}</figure>"));
        n += 1;
    }
    let _ = std::fs::remove_file(&db);

    let html = format!(
        "<!doctype html><html><head><meta charset=\"utf-8\">\
<title>duckplot dashboard</title><style>\
body{{font:15px/1.5 system-ui,-apple-system,sans-serif;background:#f7f8fa;color:#1f2430;margin:0;padding:2rem}}\
h1{{font-size:1.3rem}} .src{{color:#6b7280;font-size:.85rem;margin-bottom:1.5rem}}\
.grid{{display:grid;grid-template-columns:repeat(auto-fit,minmax(430px,1fr));gap:1rem}}\
.panel{{margin:0;background:#fff;border:1px solid #e6e8ec;border-radius:12px;padding:1rem;box-shadow:0 1px 3px rgba(16,24,40,.06)}}\
.panel svg{{max-width:100%;height:auto;display:block}}</style></head>\
<body><h1>duckplot dashboard</h1><div class=\"src\">rendered from <code>{path}</code> · {n} panels</div>\
<div class=\"grid\">{panels}</div></body></html>"
    );
    std::fs::write(&out, &html).unwrap_or_else(|e| panic!("write {out}: {e}"));
    println!("wrote {out} ({n} panels) — open it in a browser");
}

/// Strip `-- …` line comments (outside single-quoted strings).
fn strip_line_comments(sql: &str) -> String {
    let mut out = String::with_capacity(sql.len());
    for line in sql.lines() {
        let (mut in_str, mut prev_dash) = (false, false);
        let mut cut = None;
        for (i, c) in line.char_indices() {
            match c {
                '\'' => {
                    in_str = !in_str;
                    prev_dash = false;
                }
                '-' if !in_str => {
                    if prev_dash {
                        cut = Some(i - 1);
                        break;
                    }
                    prev_dash = true;
                }
                _ => prev_dash = false,
            }
        }
        out.push_str(cut.map_or(line, |i| &line[..i]));
        out.push('\n');
    }
    out
}

/// Split a script into statements on top-level `;` (naively ignores `;` in
/// single-quoted strings).
fn split_statements(sql: &str) -> Vec<String> {
    let mut out = Vec::new();
    let (mut cur, mut in_str) = (String::new(), false);
    for c in sql.chars() {
        match c {
            '\'' => {
                in_str = !in_str;
                cur.push(c);
            }
            ';' if !in_str => {
                out.push(std::mem::take(&mut cur));
            }
            _ => cur.push(c),
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur);
    }
    out
}

/// Rewrite `<expr>::ROLE` casts in the SELECT list into `<expr> AS c{i}`, and
/// return the (column-index → Role) mapping. Only touches the SELECT list.
fn rewrite(stmt: &str) -> (String, Vec<(usize, Role)>) {
    let up = stmt.to_ascii_uppercase();
    let sel = match up.find("SELECT") {
        Some(p) => p + 6,
        None => return (stmt.to_string(), Vec::new()),
    };
    // End of the select list: the first top-level FROM, or end of statement.
    let list_end = top_level_kw(&stmt[sel..], "FROM").map(|o| sel + o).unwrap_or(stmt.len());
    let head = &stmt[..sel];
    let list = &stmt[sel..list_end];
    let tail = &stmt[list_end..];

    let mut roles = Vec::new();
    let mut new_items = Vec::new();
    for (i, item) in split_top_commas(list).into_iter().enumerate() {
        let item = item.trim();
        if let Some((expr, role_str)) = trailing_role(item) {
            if let Some(role) = parse_role(role_str) {
                roles.push((i, role));
                new_items.push(format!("{expr} AS c{i}"));
                continue;
            }
        }
        new_items.push(format!("{item} AS c{i}"));
    }
    (format!("{head} {} {tail}", new_items.join(", ")), roles)
}

/// If `item` ends in `::ROLE` (a known role), return (expr, ROLE).
fn trailing_role(item: &str) -> Option<(&str, &str)> {
    let idx = item.rfind("::")?;
    let role = item[idx + 2..].trim();
    if ROLES.contains(&role.to_ascii_uppercase().as_str()) {
        Some((item[..idx].trim(), role))
    } else {
        None
    }
}

/// Split on commas that aren't inside parens or strings.
fn split_top_commas(s: &str) -> Vec<String> {
    let (mut out, mut cur, mut depth, mut in_str) = (Vec::new(), String::new(), 0i32, false);
    for c in s.chars() {
        match c {
            '\'' => {
                in_str = !in_str;
                cur.push(c);
            }
            '(' if !in_str => {
                depth += 1;
                cur.push(c);
            }
            ')' if !in_str => {
                depth -= 1;
                cur.push(c);
            }
            ',' if depth == 0 && !in_str => out.push(std::mem::take(&mut cur)),
            _ => cur.push(c),
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur);
    }
    out
}

/// Byte offset of a keyword at paren depth 0 (uppercased search).
fn top_level_kw(s: &str, kw: &str) -> Option<usize> {
    let up = s.to_ascii_uppercase();
    let (mut depth, mut in_str) = (0i32, false);
    let b = up.as_bytes();
    let mut i = 0;
    while i < b.len() {
        match b[i] as char {
            '\'' => in_str = !in_str,
            '(' if !in_str => depth += 1,
            ')' if !in_str => depth -= 1,
            _ if !in_str && depth == 0 && up[i..].starts_with(kw) => {
                let before = i == 0 || !b[i - 1].is_ascii_alphanumeric();
                let after = up[i + kw.len()..].chars().next().map_or(true, |c| !c.is_alphanumeric());
                if before && after {
                    return Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

fn run(db: &str, sql: &str, json: bool) -> Vec<serde_json::Map<String, serde_json::Value>> {
    let mut cmd = Command::new("duckdb");
    cmd.arg(db);
    if json {
        cmd.arg("-json");
    }
    let out = cmd.arg("-c").arg(sql).output().unwrap_or_else(|e| panic!("run duckdb: {e}"));
    if !out.status.success() {
        eprintln!("duckdb error on: {sql}\n{}", String::from_utf8_lossy(&out.stderr));
        return Vec::new();
    }
    if !json {
        return Vec::new();
    }
    let s = String::from_utf8_lossy(&out.stdout);
    serde_json::from_str::<Vec<serde_json::Map<String, serde_json::Value>>>(s.trim())
        .unwrap_or_default()
}

fn build_columns(
    rows: &[serde_json::Map<String, serde_json::Value>],
    roles: &[(usize, Role)],
) -> Vec<Column> {
    roles
        .iter()
        .map(|(i, role)| {
            let key = format!("c{i}");
            // Measure columns must be numeric — DuckDB emits sum()/BIGINT/DECIMAL
            // as JSON *strings* to preserve precision, so coerce them to floats.
            let numeric = matches!(role, Role::Value(_));
            let values = rows.iter().map(|r| jval(r.get(&key), numeric)).collect();
            Column::new(key, *role, values)
        })
        .collect()
}

fn jval(v: Option<&serde_json::Value>, numeric: bool) -> Value {
    match v {
        Some(serde_json::Value::Number(n)) => n.as_f64().map(Value::Float).unwrap_or(Value::Na),
        Some(serde_json::Value::String(s)) => match numeric {
            true => s.parse::<f64>().map(Value::Float).unwrap_or_else(|_| Value::Str(s.clone())),
            false => Value::Str(s.clone()),
        },
        Some(serde_json::Value::Bool(b)) => Value::Bool(*b),
        _ => Value::Na,
    }
}
