//! Shaper-style SQL parsing: strip comments, split statements, and pull `::ROLE`
//! casts off the SELECT list. Shared by the native `dashboard` bin and the wasm
//! binding so the browser and CLI behave identically.

use crate::{parse_role, Column, InputKind, Role};
use ggplot_rs::prelude::Value;

/// A planned statement: either setup (run for effect) or a panel (rewritten SQL
/// plus the role each output column plays).
pub struct Panel {
    pub setup: bool,
    /// SQL with the `::ROLE` casts rewritten to `AS c{i}` aliases.
    pub sql: String,
    pub roles: Vec<(usize, Role)>,
}

/// Parse a whole script into ordered [`Panel`]s.
pub fn plan(script: &str) -> Vec<Panel> {
    let clean = strip_line_comments(script);
    split_statements(&clean)
        .into_iter()
        .filter_map(|stmt| {
            let stmt = stmt.trim();
            if stmt.is_empty() {
                return None;
            }
            let (sql, roles) = rewrite(stmt);
            Some(if roles.is_empty() {
                Panel { setup: true, sql: stmt.to_string(), roles }
            } else {
                Panel { setup: false, sql, roles }
            })
        })
        .collect()
}

/// Build duckplot [`Column`]s from JSON result rows (`[{c0:…,c1:…}, …]`) and the
/// role mapping. Measure columns are coerced to numeric (DuckDB emits
/// BIGINT/DECIMAL as JSON strings).
pub fn columns_from_rows(
    rows: &[serde_json::Map<String, serde_json::Value>],
    roles: &[(usize, Role)],
) -> Vec<Column> {
    roles
        .iter()
        .map(|(i, role)| {
            let key = format!("c{i}");
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

const ROLES: &[&str] = &[
    "XAXIS", "X", "YAXIS", "Y", "CATEGORY", "SERIES", "COLOR", "COLOUR", "LABEL", "TITLE", "BARCHART",
    "BAR", "BARCHART_STACKED", "BAR_STACKED", "STACKED_BAR", "LINECHART", "LINE", "AREACHART", "AREA",
    "SCATTER", "POINT", "SCATTERCHART", "PIE", "DONUT", "PIECHART", "HISTOGRAM", "HIST", "BOXPLOT",
    "BOX_PLOT", "HEATMAP", "TILE", "TILES", "SPARKLINE", "SPARK", "REFLINE", "TARGET", "GOAL", "MAP",
    "GEOMETRY", "GEO", "CHOROPLETH", "TABLE", "GRID", "METRIC", "KPI", "BIGNUMBER", "DROPDOWN",
    "OPTIONS", "SELECT_INPUT", "NUMBER", "SLIDER", "NUMERIC", "DATE", "DATEPICKER", "TEXT", "SEARCH",
    "STRING", "MULTISELECT", "MULTI", "DATERANGE", "DATE_RANGE", "MONEY", "DOLLAR", "CURRENCY",
    "PERCENT", "PCT", "COMPACT",
    "DELTA", "COMPARE", "PREVIOUS", "TAB", "PAGE", "COLUMNS",
    "COLS", "GROUP", "BOX", "ROW", "ENDGROUP", "ENDBOX", "ENDROW", "SPAN", "WIDTH", "COL",
];

/// Strip `-- …` line comments (outside single-quoted strings).
pub fn strip_line_comments(sql: &str) -> String {
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

/// Split a script into statements on top-level `;`.
pub fn split_statements(sql: &str) -> Vec<String> {
    let mut out = Vec::new();
    let (mut cur, mut in_str) = (String::new(), false);
    for c in sql.chars() {
        match c {
            '\'' => {
                in_str = !in_str;
                cur.push(c);
            }
            ';' if !in_str => out.push(std::mem::take(&mut cur)),
            _ => cur.push(c),
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur);
    }
    out
}

/// Rewrite `<expr>::ROLE` casts in the SELECT list into `<expr> AS c{i}`.
pub fn rewrite(stmt: &str) -> (String, Vec<(usize, Role)>) {
    let up = stmt.to_ascii_uppercase();
    let sel = match up.find("SELECT") {
        Some(p) => p + 6,
        None => return (stmt.to_string(), Vec::new()),
    };
    let list_end = top_level_kw(&stmt[sel..], "FROM").map(|o| sel + o).unwrap_or(stmt.len());
    let (head, list, tail) = (&stmt[..sel], &stmt[sel..list_end], &stmt[list_end..]);
    let split = split_top_commas(list);

    // ::TABLE and ::DATERANGE keep every column and its name — strip only the cast.
    let keep_intact = |r: Role| matches!(r, Role::Table | Role::Input(InputKind::DateRange));
    let intact = split
        .iter()
        .find_map(|it| trailing_role(it.trim()).and_then(|(_, r)| parse_role(r)).filter(|r| keep_intact(*r)));
    if let Some(role) = intact {
        let items: Vec<String> = split
            .iter()
            .map(|it| match trailing_role(it.trim()) {
                Some((expr, r)) if parse_role(r) == Some(role) => expr.to_string(),
                _ => it.trim().to_string(),
            })
            .collect();
        return (format!("{head} {} {tail}", items.join(", ")), vec![(0, role)]);
    }

    let mut roles = Vec::new();
    let mut items = Vec::new();
    for (i, item) in split.into_iter().enumerate() {
        let item = item.trim();
        if let Some((expr, role_str)) = trailing_role(item) {
            if let Some(role) = parse_role(role_str) {
                // Cast measures/metrics to DOUBLE so sum()/BIGINT/HUGEINT come back
                // as real numbers (DuckDB-Wasm otherwise serialises HUGEINT as str).
                let item = match role {
                    Role::Value(_) | Role::Metric(_) | Role::Delta | Role::RefLine => {
                        format!("CAST({expr} AS DOUBLE) AS c{i}")
                    }
                    // Inputs keep the original column name — it becomes the
                    // DuckDB variable name the browser binds the control to.
                    Role::Input(_) => expr.to_string(),
                    _ => format!("{expr} AS c{i}"),
                };
                roles.push((i, role));
                items.push(item);
                continue;
            }
        }
        items.push(format!("{item} AS c{i}"));
    }
    (format!("{head} {} {tail}", items.join(", ")), roles)
}

fn trailing_role(item: &str) -> Option<(&str, &str)> {
    let idx = item.rfind("::")?;
    let role = item[idx + 2..].trim();
    if ROLES.contains(&role.to_ascii_uppercase().as_str()) {
        Some((item[..idx].trim(), role))
    } else {
        None
    }
}

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

fn top_level_kw(s: &str, kw: &str) -> Option<usize> {
    let up = s.to_ascii_uppercase();
    let (mut depth, mut in_str) = (0i32, false);
    let b = up.as_bytes();
    for i in 0..b.len() {
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
    }
    None
}
