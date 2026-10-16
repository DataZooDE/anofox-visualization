//! Input defaults outside the browser: the `SET VARIABLE`s that give each
//! `::DROPDOWN`/`::MULTISELECT`/`::NUMBER`/`::DATE`/`::TEXT`/`::DATERANGE`/
//! `::PERIOD`/`::BUTTONS`/`::TOGGLE` input its default value, so downstream `getvariable()` panels
//! read something. Shared by the linter (`dashboard --check`) and the headless
//! renderers (`dashboard` static HTML, `dashboard --svg`,
//! [`crate::dashboard::render_dashboard_svg`]) — the same defaults the
//! browser's input pre-pass picks on first load.

use crate::{InputKind, Role, Value};
use serde_json::{Map, Value as Json};

/// The input kind of a planned panel, if it is an input.
pub fn input_kind(roles: &[(usize, Role)]) -> Option<InputKind> {
    roles.iter().find_map(|(_, r)| match r {
        Role::Input(k) => Some(*k),
        _ => None,
    })
}

/// Output position of a panel's input column.
pub fn input_index(roles: &[(usize, Role)]) -> usize {
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
/// date range, the spec's default period for a `::PERIOD` — so downstream
/// `getvariable()` panels have something to read. `idx` is the input column's
/// output position (rows keep SELECT column order, via serde_json's
/// `preserve_order`); the variable name is quoted.
pub fn input_defaults(kind: InputKind, idx: usize, rows: &[Map<String, Json>]) -> Vec<String> {
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
        InputKind::Period => {
            let spec = match first.get(k) {
                Some(Json::String(s)) => s.clone(),
                _ => String::new(),
            };
            crate::period::default_sets(k, &spec)
        }
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

/// [`input_defaults`] for a planned panel and its query result as columns
/// (`(name, values)` in SELECT order — a [`crate::dashboard::DataProvider`]
/// result). Empty for a non-input panel.
pub fn panel_defaults(roles: &[(usize, Role)], cols: &[(String, Vec<Value>)]) -> Vec<String> {
    let Some(kind) = input_kind(roles) else {
        return Vec::new();
    };
    let n = cols.iter().map(|(_, v)| v.len()).max().unwrap_or(0);
    let rows: Vec<Map<String, Json>> = (0..n)
        .map(|i| {
            cols.iter()
                .map(|(name, vals)| (name.clone(), cell_json(vals.get(i))))
                .collect()
        })
        .collect();
    input_defaults(kind, input_index(roles), &rows)
}

fn cell_json(v: Option<&Value>) -> Json {
    match v {
        Some(Value::Str(s)) => Json::String(s.clone()),
        Some(Value::Float(f)) => serde_json::Number::from_f64(*f)
            .map(Json::Number)
            .unwrap_or(Json::Null),
        Some(Value::Integer(i)) => Json::from(*i),
        Some(Value::Bool(b)) => Json::Bool(*b),
        Some(Value::DateTime(secs)) => {
            let day = crate::period::Date::from_days(secs.div_euclid(86_400)).iso();
            let s = secs.rem_euclid(86_400);
            Json::String(if s == 0 {
                day
            } else {
                format!("{day} {:02}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
            })
        }
        _ => Json::Null,
    }
}

/// A `::TOGGLE` default as a boolean: `true`, a non-zero number, or the text
/// `true` / `1` / `yes` / `on` (as the browser reads it).
fn truthy(v: &Json) -> bool {
    match v {
        Json::Bool(b) => *b,
        Json::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Json::String(s) => matches!(
            s.trim().to_ascii_lowercase().as_str(),
            "true" | "t" | "1" | "yes" | "on"
        ),
        _ => false,
    }
}

/// A JSON cell as a SQL literal.
pub fn lit(v: &Json) -> String {
    match v {
        Json::String(s) => format!("'{}'", s.replace('\'', "''")),
        Json::Number(n) => n.to_string(),
        Json::Bool(b) => {
            if *b {
                "TRUE".into()
            } else {
                "FALSE".into()
            }
        }
        _ => "NULL".into(),
    }
}

/// Is `sql` a `SET VARIABLE …` statement (session state a per-statement
/// runner — the DuckDB CLI — has to replay)?
pub fn is_set_variable(sql: &str) -> bool {
    let w: Vec<String> = sql
        .split_whitespace()
        .take(2)
        .map(|w| w.to_ascii_uppercase())
        .collect();
    w.len() == 2 && w[0] == "SET" && w[1] == "VARIABLE"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn period_defaults_from_columns() {
        let roles = vec![(0, Role::Input(InputKind::Period)), (1, Role::Label)];
        let cols = vec![
            (
                "p".to_string(),
                vec![Value::Str("default=last_month; today=2026-03-15".into())],
            ),
            ("c1".to_string(), vec![Value::Str("Period".into())]),
        ];
        let s = panel_defaults(&roles, &cols);
        assert_eq!(s[0], "SET VARIABLE \"p_from\" = '2026-02-01'");
        assert_eq!(s[1], "SET VARIABLE \"p_to\" = '2026-02-28'");
        assert_eq!(s[2], "SET VARIABLE \"p_prev_from\" = '2026-01-01'");
        assert_eq!(s[6], "SET VARIABLE \"p_label\" = 'Feb 2026'");
    }

    #[test]
    fn dropdown_defaults_from_columns() {
        let roles = vec![(0, Role::Input(InputKind::Dropdown))];
        let cols = vec![(
            "region".to_string(),
            vec![Value::Str("EU".into()), Value::Str("US".into())],
        )];
        assert_eq!(
            panel_defaults(&roles, &cols),
            vec!["SET VARIABLE \"region\" = 'EU'"]
        );
        let roles = vec![(0, Role::Input(InputKind::Date))];
        let cols = vec![("d".to_string(), vec![Value::DateTime(86_400 * 20727)])];
        assert_eq!(
            panel_defaults(&roles, &cols),
            vec!["SET VARIABLE \"d\" = '2026-10-01'"]
        );
        assert!(is_set_variable("  set variable x = 1"));
        assert!(!is_set_variable("SET threads = 4"));
    }
}
