//! SQL parsing: strip comments, split statements, and pull `::ROLE`
//! casts off the SELECT list. Shared by the native `dashboard` bin, `serve`,
//! the DuckDB extension and the wasm binding so every host behaves identically.
//!
//! All SQL surgery goes through one lexer ([`lex`]), so quotes, `''`/`""`
//! escapes, `E''` strings, `$$`/`$tag$` strings and `--`/`/* */` comments are
//! handled consistently everywhere.
//!
//! ## Which statements are panels
//!
//! Only **query statements** — those whose first keyword is `SELECT`, `WITH`
//! or `FROM` (or that start with `(`) — are inspected for role casts; DDL/DML
//! (`CREATE … AS SELECT`, `INSERT … SELECT`, `SET`, …) is always setup. In a
//! query the **main** `SELECT` list is the first `SELECT` at bracket depth 0,
//! so a leading `WITH` CTE list works (`WITH t AS (…) SELECT x::XAXIS …`).
//!
//! ## Role tokens that are also SQL types
//!
//! `::DATE`, `::TEXT`, `::STRING`, `::NUMERIC` (inputs) and `::MAP`,
//! `::GEOMETRY` (maps) are both DuckDB types and roles
//! ([`crate::roles::SQL_TYPE_TOKENS`]). They are roles only in a query
//! statement, and then:
//! - `::MAP`/`::GEOMETRY` are always the map role;
//! - the input tokens are roles only when the SELECT is an **input
//!   statement** — no other unambiguous role except `::HINT`/`::LABEL`/
//!   `::TITLE`. In a chart panel (`SELECT day::DATE, n::BARCHART …`) they stay
//!   real casts;
//! - inside a `::TABLE`/`::PAGED`/`::DOWNLOAD_*`/`::DATERANGE` panel they stay
//!   real casts.
//!
//! Chain a role after a type cast to get both: `ts::DATE::XAXIS`.

pub mod contract_call;
pub mod deps;
pub mod lex;
pub mod requires;
pub mod shared;
pub mod views;

pub use contract_call::ContractPanel;
pub use lex::{split_top_commas, strip_comments, trailing_alias};
pub use views::ViewAt;

use crate::roles::is_sql_type_token;
use crate::{parse_role, Column, InputKind, Kind, Role};
use ggplot_rs::prelude::Value;

/// A planned statement: either setup (run for effect) or a panel (rewritten SQL
/// plus the role each output column plays).
pub struct Panel {
    pub setup: bool,
    /// SQL with the `::ROLE` casts rewritten to `AS c{i}` aliases.
    pub sql: String,
    pub roles: Vec<(usize, Role)>,
    /// Human display name per charted-measure column (`AS alias`, or the bare
    /// column name) keyed by output position — used to label combo legends.
    pub names: Vec<(usize, String)>,
    /// A contract plot call (`SELECT anofox_plot_terms(t) FROM … t`), rewritten
    /// to return the rows + arguments as JSON — see [`contract_call`].
    pub contract: Option<ContractPanel>,
    /// Optional inputs (`-- @requires <path> | <alternative>, …` comment
    /// lines above the statement, see [`requires`]): when one is missing the
    /// statement is skipped and its panel reads "Not available" instead of an
    /// error.
    pub requires: Vec<requires::Requirement>,
    /// What is malformed in those lines (`dashboard --check` reports it).
    pub requires_issues: Vec<(&'static str, String)>,
    /// Variable names a `-- @shared <name>, …` comment above the statement
    /// names as deliberately shared (#398): when this statement's own
    /// `interaction/rowselect-shared-name` / `interaction/name-clash` claim
    /// would otherwise warn for one of these names, the warning is skipped —
    /// every other clash still is. A listed name this statement doesn't
    /// itself set (a typo) is flagged on its own (`interaction/shared-unused`).
    pub shared: Vec<String>,
    /// The `-- @requires` of the `::TAB` / `::SUBTAB` whose section the
    /// statement is in (a `::SUBTAB` statement: its `::TAB`'s), see
    /// [`section_requires`]. A tab whose own requirements are not met is
    /// hidden: its statements other than setup are not run. (Setup still runs;
    /// a missing-input failure of one in such a section is expected.)
    pub tab_requires: Vec<requires::Requirement>,
    /// The plan indices of those `::TAB` / `::SUBTAB` statements (with
    /// `-- @requires` or not), outermost first. Their requirements are
    /// decided where they stand: with the variables set above the tab.
    pub tabs: Vec<usize>,
    /// This statement's place in a `::VIEW` group (#214), if it is in one —
    /// set on every statement of the group, setup included. See
    /// [`views::assign_views`].
    pub view: Option<views::ViewAt>,
}

impl Panel {
    /// A `::TAB` (`Some(false)`) or `::SUBTAB` (`Some(true)`) statement?
    pub fn tab_kind(&self) -> Option<bool> {
        if self.setup {
            return None;
        }
        self.roles.iter().find_map(|(_, r)| match r {
            Role::Tab => Some(false),
            Role::SubTab => Some(true),
            _ => None,
        })
    }

    /// The requirements a statement runs under: its own, and — unless it is
    /// setup — those of the tab it is in.
    pub fn all_requires(&self) -> Vec<requires::Requirement> {
        let mut out = self.requires.clone();
        if !self.setup {
            out.extend(self.tab_requires.iter().cloned());
        }
        out
    }
}

/// Fill each statement's [`Panel::tab_requires`]: a `::TAB`'s section runs to
/// the next `::TAB`, a `::SUBTAB`'s to the next `::SUBTAB` or `::TAB`, a
/// `::VIEW` alternative's to the next alternative, closer or end of its
/// group (a third level inside a `::SUBTAB`'s, gated exactly like a
/// `::SUBTAB`'s is inside a `::TAB`'s — see [`views::assign_views`], which
/// has already filled [`Panel::view`] by the time this runs). Chrome
/// directives (`::THEME`, `::DASHBOARD_TITLE`, …) apply to the whole
/// dashboard wherever they sit: they inherit nothing.
fn section_requires(panels: &mut [Panel]) {
    let mut tab: Vec<requires::Requirement> = Vec::new();
    let mut sub: Vec<requires::Requirement> = Vec::new();
    let mut view: Vec<requires::Requirement> = Vec::new();
    let (mut tab_at, mut sub_at, mut view_at) = (None, None, None);
    for (i, p) in panels.iter_mut().enumerate() {
        match p.tab_kind() {
            Some(false) => {
                tab = p.requires.clone();
                sub.clear();
                view.clear();
                (tab_at, sub_at, view_at) = (Some(i), None, None);
                continue;
            }
            Some(true) => {
                p.tab_requires = tab.clone();
                p.tabs = tab_at.into_iter().collect();
                sub = p.requires.clone();
                sub_at = Some(i);
                view.clear();
                view_at = None;
                continue;
            }
            None => {}
        }
        if p.roles
            .iter()
            .any(|(_, r)| crate::roles::is_chrome_directive(*r))
        {
            continue;
        }
        // A `::VIEW` marker: like a `::SUBTAB`, its own requires gate
        // everything from here to the next marker/closer in its group.
        if p.view.as_ref().is_some_and(|v| v.marker) {
            p.tab_requires = tab.iter().chain(&sub).cloned().collect();
            p.tabs = tab_at.into_iter().chain(sub_at).collect();
            view = p.requires.clone();
            view_at = Some(i);
            continue;
        }
        // Left the group (or never in one): nothing of its view level
        // carries over to this statement.
        if p.view.is_none() {
            view.clear();
            view_at = None;
        }
        p.tab_requires = tab.iter().chain(&sub).chain(&view).cloned().collect();
        p.tabs = tab_at.into_iter().chain(sub_at).chain(view_at).collect();
    }
}

/// Parse a whole script into ordered [`Panel`]s.
pub fn plan(script: &str) -> Vec<Panel> {
    let mut panels = plan_statements(script);
    views::assign_views(&mut panels);
    section_requires(&mut panels);
    panels
}

fn plan_statements(script: &str) -> Vec<Panel> {
    split_statements(script)
        .into_iter()
        .filter_map(|raw| {
            let requires = requires::parse(&raw);
            let requires_issues = requires::issues(&raw);
            let shared = shared::parse(&raw);
            let clean = strip_comments(&raw);
            let stmt = clean.trim();
            if stmt.is_empty() {
                return None;
            }
            if let Some((mut panel, _)) = contract_call::plan_contract(stmt) {
                panel.requires = requires;
                panel.requires_issues = requires_issues;
                panel.shared = shared;
                return Some(panel);
            }
            let (sql, roles, names) = rewrite(stmt);
            Some(if roles.is_empty() {
                Panel {
                    setup: true,
                    sql: stmt.to_string(),
                    roles,
                    names: Vec::new(),
                    contract: None,
                    requires,
                    requires_issues,
                    shared,
                    tab_requires: Vec::new(),
                    tabs: Vec::new(),
                    view: None,
                }
            } else {
                Panel {
                    setup: false,
                    sql,
                    roles,
                    names,
                    contract: None,
                    requires,
                    requires_issues,
                    shared,
                    tab_requires: Vec::new(),
                    tabs: Vec::new(),
                    view: None,
                }
            })
        })
        .collect()
}

/// Does a query error say an input file / table is missing? (The class of
/// error a `-- @requires` statement turns into "Not available".)
pub fn is_missing_input_error(msg: &str) -> bool {
    let m = msg.to_ascii_lowercase();
    [
        "no files found",
        "cannot open file",
        "no such file",
        "does not exist",
        "http 404",
        "couldn't fetch",
    ]
    .iter()
    .any(|k| m.contains(k))
}

/// Build anofox-visualization [`Column`]s from JSON result rows (`[{c0:…,c1:…}, …]`) and the
/// role mapping. Measure columns are coerced to numeric (DuckDB emits
/// BIGINT/DECIMAL as JSON strings); `"NaN"`/`"inf"` strings become missing.
pub fn columns_from_rows(
    rows: &[serde_json::Map<String, serde_json::Value>],
    roles: &[(usize, Role)],
) -> Vec<Column> {
    roles
        .iter()
        .map(|(i, role)| {
            let key = format!("c{i}");
            let numeric = matches!(role, Role::Value(_));
            let values: Vec<Value> = rows.iter().map(|r| jval(r.get(&key), numeric)).collect();
            let values = coerce_for_role(*role, values);
            Column::new(key, *role, values)
        })
        .collect()
}

/// JSON rows (as the DuckDB CLI's `-json` prints them) → columns in SELECT
/// order, for a headless [`crate::dashboard::DataProvider`]. No DuckDB type
/// info (a `::XAXIS`/`::YAXIS` string column that merely *looks* numeric,
/// `'0'`/`'1'`, would wrongly promote to a continuous axis, #424) — prefer
/// [`result_columns_typed`] when `DESCRIBE`'s column types are available
/// (`CliProvider` in `src/bin/dashboard.rs` always has them).
pub fn result_columns(
    rows: &[serde_json::Map<String, serde_json::Value>],
) -> Vec<(String, Vec<Value>)> {
    result_columns_typed(rows, &std::collections::HashMap::new())
}

/// [`result_columns`], keyed on each column's real DuckDB type (`column_name`
/// → `column_type`, as `DESCRIBE <query>` reports them) rather than guessing
/// numeric-ness from the JSON text: #424, a VARCHAR `::XAXIS`/`::YAXIS` whose
/// values happen to parse as numbers (`'0'`/`'1'`, a binary classifier's
/// labels) must stay discrete, the same way a column of `'cat'`/`'dog'`
/// already does. A type DuckDB serialises as a JSON string despite being
/// numeric (`BIGINT`/`HUGEINT`/`DECIMAL`) still parses to `Value::Float`; a
/// column missing from `types` (e.g. a `DataProvider` with no `DESCRIBE`
/// step) falls back to the old per-value heuristic, unchanged.
pub fn result_columns_typed(
    rows: &[serde_json::Map<String, serde_json::Value>],
    types: &std::collections::HashMap<String, String>,
) -> Vec<(String, Vec<Value>)> {
    let Some(first) = rows.first() else {
        return Vec::new();
    };
    first
        .keys()
        .map(|k| {
            let vals: Vec<Value> = rows.iter().map(|r| jval(r.get(k), false)).collect();
            let numeric = match types.get(k).map(|t| is_discrete_sql_type(t)) {
                Some(discrete) => !discrete,
                // No type info for this column: the old heuristic — every
                // string value parses as a number.
                None => vals.iter().all(|v| match v {
                    Value::Str(s) => s.trim().parse::<f64>().is_ok(),
                    _ => true,
                }),
            };
            let vals = if numeric {
                rows.iter().map(|r| jval(r.get(k), true)).collect()
            } else {
                vals
            };
            (k.clone(), vals)
        })
        .collect()
}

/// Is a DuckDB `column_type` (as `DESCRIBE` reports it, e.g. `"VARCHAR"`,
/// `"DECIMAL(10,2)"`, `"ENUM('a', 'b')"`) a string/text type — so its values
/// stay discrete even when every one of them happens to parse as a number?
fn is_discrete_sql_type(t: &str) -> bool {
    let head = t
        .split(['(', '['])
        .next()
        .unwrap_or(t)
        .trim()
        .to_ascii_uppercase();
    matches!(
        head.as_str(),
        "VARCHAR" | "CHAR" | "BPCHAR" | "TEXT" | "STRING" | "ENUM" | "UUID" | "BLOB" | "BIT"
    )
}

#[cfg(test)]
mod result_columns_typed_tests {
    use super::*;
    use std::collections::HashMap;

    fn rows(vals: &[&str]) -> Vec<serde_json::Map<String, serde_json::Value>> {
        vals.iter()
            .map(|v| {
                let mut m = serde_json::Map::new();
                m.insert("c0".to_string(), serde_json::Value::String(v.to_string()));
                m
            })
            .collect()
    }

    #[test]
    fn a_varchar_column_stays_discrete_even_when_every_value_looks_numeric() {
        // #424: '0'/'1' class labels — the old per-value heuristic (every
        // string parses as a number) would wrongly promote this to a
        // continuous axis; DESCRIBE's real VARCHAR type must win.
        let r = rows(&["0", "1", "0", "1"]);
        let mut types = HashMap::new();
        types.insert("c0".to_string(), "VARCHAR".to_string());
        let cols = result_columns_typed(&r, &types);
        assert_eq!(
            cols[0].1,
            vec![
                Value::Str("0".into()),
                Value::Str("1".into()),
                Value::Str("0".into()),
                Value::Str("1".into()),
            ]
        );
    }

    #[test]
    fn a_numeric_type_serialised_as_a_json_string_still_promotes() {
        // DECIMAL/BIGINT/HUGEINT come back from DuckDB's -json as strings.
        let r = rows(&["1.50", "2.75"]);
        let mut types = HashMap::new();
        types.insert("c0".to_string(), "DECIMAL(10,2)".to_string());
        let cols = result_columns_typed(&r, &types);
        assert_eq!(cols[0].1, vec![Value::Float(1.5), Value::Float(2.75)]);
    }

    #[test]
    fn without_type_info_the_old_heuristic_still_applies() {
        let r = rows(&["0", "1"]);
        let cols = result_columns_typed(&r, &HashMap::new());
        assert_eq!(cols[0].1, vec![Value::Float(0.0), Value::Float(1.0)]);
        // A genuinely non-numeric string stays discrete either way.
        let r2 = rows(&["cat", "dog"]);
        let cols2 = result_columns_typed(&r2, &HashMap::new());
        assert_eq!(
            cols2[0].1,
            vec![Value::Str("cat".into()), Value::Str("dog".into())]
        );
    }

    #[test]
    fn is_discrete_sql_type_covers_common_duckdb_spellings() {
        for t in ["VARCHAR", "varchar", "CHAR(10)", "ENUM('a', 'b')", "TEXT"] {
            assert!(is_discrete_sql_type(t), "{t}");
        }
        for t in ["INTEGER", "DOUBLE", "DECIMAL(10,2)", "BIGINT", "DATE"] {
            assert!(!is_discrete_sql_type(t), "{t}");
        }
    }
}

fn jval(v: Option<&serde_json::Value>, numeric: bool) -> Value {
    match v {
        Some(serde_json::Value::Number(n)) => n
            .as_f64()
            .filter(|f| f.is_finite())
            .map(Value::Float)
            .unwrap_or(Value::Na),
        Some(serde_json::Value::String(s)) => match numeric {
            true => match s.trim().parse::<f64>() {
                Ok(f) if f.is_finite() => Value::Float(f),
                Ok(_) => Value::Na, // "NaN", "inf", "-Infinity"
                Err(_) => Value::Str(s.clone()),
            },
            false => Value::Str(s.clone()),
        },
        Some(serde_json::Value::Bool(b)) => Value::Bool(*b),
        // A list (a KPI's `list(v)::SPARKLINE`, a table's list column) or a
        // struct (a contract call's `json_object(…)` arguments) keeps its JSON
        // text.
        Some(v @ (serde_json::Value::Array(_) | serde_json::Value::Object(_))) => {
            Value::Str(v.to_string())
        }
        _ => Value::Na,
    }
}

/// Replace bare `NaN` / `Infinity` / `-Infinity` / `inf` / `nan` tokens
/// **outside JSON strings** with `null`, so JSON from DuckDB (`to_json`, the
/// CLI's `-json`) that contains non-finite doubles still parses. Returns the
/// input unchanged (borrowed) when there is nothing to fix.
pub fn sanitize_json_numbers(s: &str) -> std::borrow::Cow<'_, str> {
    const BAD: [&str; 3] = ["nan", "infinity", "inf"];
    let b = s.as_bytes();
    let mut out: Option<String> = None;
    let mut last = 0usize;
    let mut i = 0usize;
    let mut in_str = false;
    while i < b.len() {
        let c = b[i];
        if in_str {
            match c {
                b'\\' => i += 2,
                b'"' => {
                    in_str = false;
                    i += 1;
                }
                _ => i += 1,
            }
            continue;
        }
        if c == b'"' {
            in_str = true;
            i += 1;
            continue;
        }
        let prev_ok =
            i == 0 || matches!(b[i - 1], b':' | b',' | b'[' | b' ' | b'\t' | b'\n' | b'\r');
        if prev_ok && matches!(c, b'N' | b'n' | b'I' | b'i' | b'-' | b'+') {
            let mut j = i;
            if matches!(b[j], b'-' | b'+') {
                j += 1;
            }
            let word_start = j;
            while j < b.len() && b[j].is_ascii_alphabetic() {
                j += 1;
            }
            let word = s[word_start..j].to_ascii_lowercase();
            let next_ok =
                j == b.len() || matches!(b[j], b',' | b']' | b'}' | b' ' | b'\t' | b'\n' | b'\r');
            if next_ok && BAD.contains(&word.as_str()) {
                let o = out.get_or_insert_with(|| String::with_capacity(s.len()));
                o.push_str(&s[last..i]);
                o.push_str("null");
                last = j;
                i = j;
                continue;
            }
        }
        i += 1;
    }
    match out {
        Some(mut o) => {
            o.push_str(&s[last..]);
            std::borrow::Cow::Owned(o)
        }
        None => std::borrow::Cow::Borrowed(s),
    }
}

/// Parse a DuckDB JSON result (`[{…},…]`, or empty output) into rows,
/// tolerating bare `NaN`/`Infinity`. Use this in every host instead of
/// `serde_json::from_str(..).unwrap_or_default()` so a non-finite double
/// doesn't silently blank a panel.
pub fn parse_rows_json(s: &str) -> Result<Vec<serde_json::Map<String, serde_json::Value>>, String> {
    let t = s.trim();
    if t.is_empty() {
        return Ok(Vec::new());
    }
    serde_json::from_str(&sanitize_json_numbers(t)).map_err(|e| format!("bad result JSON: {e}"))
}

/// DuckDB's JSON writes DECIMAL values as strings (`"5.2"`): an x column whose
/// values are all such decimal strings is numeric (a continuous axis), not a
/// list of category labels. Integer-looking strings (`"2019"`) stay discrete.
pub(crate) fn maybe_decimal(vals: Vec<Value>) -> Vec<Value> {
    let mut saw_point = false;
    for v in &vals {
        match v {
            Value::Str(s) => {
                let t = s.trim();
                if t.parse::<f64>().map_or(true, |f| !f.is_finite())
                    || !t
                        .bytes()
                        .all(|b| b.is_ascii_digit() || b == b'.' || b == b'-')
                {
                    return vals;
                }
                saw_point |= t.contains('.');
            }
            Value::Na => {}
            _ => return vals,
        }
    }
    if !saw_point {
        return vals;
    }
    vals.into_iter()
        .map(|v| match v {
            Value::Str(s) => s.trim().parse::<f64>().map_or(Value::Na, Value::Float),
            other => other,
        })
        .collect()
}

/// Role-specific value coercion shared by the browser and headless paths:
/// - An X axis whose values are all ISO dates/timestamps becomes a continuous
///   datetime scale, so ggplot-rs picks sensible (e.g. yearly) breaks instead
///   of drawing one overlapping label per value.
/// - An x *position* (`::XLINE`, `::MARKAREA`, `::XBAND_FROM`/`_TO`) is not cast to DOUBLE in SQL (a
///   DATE has no such cast), so it arrives as a number, a numeric string
///   (BIGINT/DECIMAL) or an ISO date — all become the x data's units (a date
///   becomes epoch seconds, like a date x axis).
pub(crate) fn coerce_for_role(role: Role, vals: Vec<Value>) -> Vec<Value> {
    match role {
        Role::X => maybe_decimal(maybe_datetime(vals)),
        // A box from statistics: numbers (DuckDB's JSON prints DECIMAL /
        // HUGEINT as strings).
        Role::Q1 | Role::Median | Role::Q3 | Role::WhiskerLow | Role::WhiskerHigh | Role::Mean => {
            vals.into_iter()
                .map(|v| match v {
                    Value::Str(s) => s
                        .trim()
                        .parse::<f64>()
                        .ok()
                        .filter(|f| f.is_finite())
                        .map_or(Value::Na, Value::Float),
                    other => other,
                })
                .collect()
        }
        Role::VLine | Role::MarkArea | Role::XBandFrom | Role::XBandTo => maybe_datetime(vals)
            .into_iter()
            .map(|v| match v {
                Value::Str(s) => s
                    .trim()
                    .parse::<f64>()
                    .ok()
                    .filter(|f| f.is_finite())
                    .map_or(Value::Na, Value::Float),
                other => other,
            })
            .collect(),
        _ => vals,
    }
}

/// If every non-null value is an ISO date/timestamp string, reinterpret the
/// column as [`Value::DateTime`] (epoch seconds); otherwise leave it unchanged.
pub(crate) fn maybe_datetime(vals: Vec<Value>) -> Vec<Value> {
    let mut saw_date = false;
    for v in &vals {
        match v {
            Value::Str(s) => match parse_iso_epoch(s) {
                Some(_) => saw_date = true,
                None => return vals, // a non-date string → keep it discrete
            },
            Value::Na => {}
            _ => return vals, // already numeric/datetime/etc.
        }
    }
    if !saw_date {
        return vals;
    }
    vals.into_iter()
        .map(|v| match v {
            Value::Str(s) => parse_iso_epoch(&s)
                .map(Value::DateTime)
                .unwrap_or(Value::Na),
            other => other,
        })
        .collect()
}

fn days_in_month(y: i64, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 => 29,
        _ => 28,
    }
}

/// Parse an ISO date / timestamp into seconds since the Unix epoch (UTC):
/// `YYYY-MM-DD`, optionally followed by `[ T]HH:MM[:SS[.fff…]]` and a zone
/// `Z` / `±HH` / `±HHMM` / `±HH:MM` (DuckDB's `TIMESTAMPTZ` text form). The
/// offset is honoured (`12:00+02` → 10:00 UTC). Impossible dates
/// (`2024-02-31`) and trailing garbage return `None`.
pub fn parse_iso_epoch(s: &str) -> Option<i64> {
    let s = s.trim();
    let b = s.as_bytes();
    if b.len() < 10 || b[4] != b'-' || b[7] != b'-' || !s.is_char_boundary(10) {
        return None;
    }
    let digits = |r: std::ops::Range<usize>| -> Option<i64> {
        let t = s.get(r)?;
        if t.is_empty() || !t.bytes().all(|c| c.is_ascii_digit()) {
            return None;
        }
        t.parse().ok()
    };
    let y = digits(0..4)?;
    let mo = digits(5..7)? as u32;
    let d = digits(8..10)? as u32;
    if !(1..=12).contains(&mo) || d < 1 || d > days_in_month(y, mo) {
        return None;
    }
    let mut secs = days_from_civil(y, mo, d) * 86_400;
    let rest = &s[10..];
    if rest.is_empty() {
        return Some(secs);
    }
    let rb = rest.as_bytes();
    if !(rb[0] == b' ' || rb[0] == b'T') || rest.len() < 6 {
        return None;
    }
    let t = &rest[1..];
    let hh = digits_of(t, 0..2)?;
    if t.as_bytes().get(2) != Some(&b':') {
        return None;
    }
    let mi = digits_of(t, 3..5)?;
    let mut k = 5;
    let mut ss = 0;
    if t.as_bytes().get(k) == Some(&b':') {
        ss = digits_of(t, k + 1..k + 3)?;
        k += 3;
        if t.as_bytes().get(k) == Some(&b'.') {
            k += 1;
            while t.as_bytes().get(k).is_some_and(|c| c.is_ascii_digit()) {
                k += 1;
            }
        }
    }
    if hh > 23 || mi > 59 || ss > 60 {
        return None;
    }
    secs += hh * 3600 + mi * 60 + ss;
    let zone = t.get(k..)?.trim_start();
    if zone.is_empty() || zone.eq_ignore_ascii_case("Z") || zone.eq_ignore_ascii_case("UTC") {
        return Some(secs);
    }
    let sign = match zone.as_bytes()[0] {
        b'+' => 1,
        b'-' => -1,
        _ => return None,
    };
    let z = &zone[1..];
    let (oh, om) = match z.len() {
        2 => (digits_of(z, 0..2)?, 0),
        4 => (digits_of(z, 0..2)?, digits_of(z, 2..4)?),
        5 if z.as_bytes()[2] == b':' => (digits_of(z, 0..2)?, digits_of(z, 3..5)?),
        _ => return None,
    };
    if oh > 18 || om > 59 {
        return None;
    }
    Some(secs - sign * (oh * 3600 + om * 60))
}

fn digits_of(s: &str, r: std::ops::Range<usize>) -> Option<i64> {
    let t = s.get(r)?;
    if t.is_empty() || !t.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    t.parse().ok()
}

/// Days since 1970-01-01 for a civil (Y, M, D) date — Howard Hinnant's algorithm.
pub(crate) fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 } as i64;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Strip `--` and `/* */` comments outside literals (kept for compatibility;
/// same as [`strip_comments`], which no longer resets string state per line).
pub fn strip_line_comments(sql: &str) -> String {
    strip_comments(sql)
}

/// Split a script into statements on `;` outside literals and comments.
pub fn split_statements(sql: &str) -> Vec<String> {
    lex::split_statements(sql)
}

/// `(rewritten SQL, [(col idx, role)], [(col idx, display name)])`.
type Rewritten = (String, Vec<(usize, Role)>, Vec<(usize, String)>);

/// Clause keywords that end a SELECT list when there's no `FROM`.
const LIST_END: &[&str] = &[
    "FROM",
    "WHERE",
    "GROUP",
    "HAVING",
    "QUALIFY",
    "WINDOW",
    "ORDER",
    "LIMIT",
    "OFFSET",
    "UNION",
    "INTERSECT",
    "EXCEPT",
];

/// Is this statement a query (the only kind that may be a panel)?
pub fn is_query_statement(stmt: &str) -> bool {
    let first_tok = lex::tokenize(stmt).into_iter().find(|t| !t.is_trivia());
    if first_tok.is_some_and(|t| t.is_punct(stmt, '(')) {
        return true;
    }
    matches!(
        lex::first_keyword(stmt).as_deref(),
        Some("SELECT" | "WITH" | "FROM")
    )
}

/// Locate the main SELECT list: `(byte start, byte end)` of the item list.
fn main_select_list(stmt: &str) -> Option<(usize, usize)> {
    let sel = lex::find_top_level_keyword(stmt, "SELECT", 0)?;
    let start = sel + "SELECT".len();
    let end = lex::find_top_level_any(stmt, LIST_END, start)
        .map(|(p, _)| p)
        .unwrap_or(stmt.len());
    Some((start, end))
}

/// A top-level `UNION [ALL]` / `INTERSECT [ALL]` / `EXCEPT [ALL]` reads as
/// one SELECT list across every branch (#294): locate each branch's own
/// item-list span (`byte start, byte end`), outermost (main) branch first.
/// For a plain query (no set operation) this is just `[main_select_list]`.
fn select_list_spans(stmt: &str) -> Vec<(usize, usize)> {
    const SET_OPS: &[&str] = &["UNION", "INTERSECT", "EXCEPT"];
    let mut spans = Vec::new();
    let mut from = 0usize;
    while let Some(sel) = lex::find_top_level_keyword(stmt, "SELECT", from) {
        let start = sel + "SELECT".len();
        let end = lex::find_top_level_any(stmt, LIST_END, start)
            .map(|(p, _)| p)
            .unwrap_or(stmt.len());
        spans.push((start, end));
        // Another branch follows only if a set-op keyword still shows up
        // (at depth 0) somewhere after this branch's item list — e.g. past
        // its own FROM/WHERE. A trailing ORDER BY/LIMIT on the whole union
        // has none left, so the loop stops there.
        match lex::find_top_level_any(stmt, SET_OPS, end) {
            Some((p, _)) => from = p,
            None => break,
        }
    }
    spans
}

/// A recognised trailing `::ROLE` on a select item, honouring the SQL-type
/// ambiguity rules (see the module docs). `inputs_ok` = this SELECT is an
/// input statement.
fn item_role(item: &str, inputs_ok: bool) -> Option<(&str, Role)> {
    let (expr, tok) = lex::trailing_cast(item)?;
    let role = parse_role(tok)?;
    if is_sql_type_token(tok) && matches!(role, Role::Input(_)) && !inputs_ok {
        return None;
    }
    Some((expr, role))
}

/// The raw text of the expression behind role `want` in `stmt`'s select
/// list(s) — before its `::ROLE` cast and any trailing `AS alias` — for
/// lints that need to see whether a cast's *source* is a plain literal
/// (`submit/dynamic-vars`, #517). `None` when no item in `stmt` carries
/// `want`. Looks at the raw statement text (casts intact), not a
/// [`Panel`]'s rewritten `sql`.
pub(crate) fn role_source_expr(stmt: &str, want: Role) -> Option<String> {
    for (s, e) in select_list_spans(stmt) {
        for item in lex::split_top_commas(&stmt[s..e]) {
            if let Some((expr, role)) = item_role(&item, true) {
                if role == want {
                    let (core, _alias) = lex::trailing_alias(expr);
                    return Some(core.trim().to_string());
                }
            }
        }
    }
    None
}

/// Does `stmt` (an item's expression, see [`role_source_expr`]) have to be
/// computed — anything other than a single `'…'`/`E'…'` string literal?
pub(crate) fn is_string_literal(expr: &str) -> bool {
    let toks: Vec<lex::Token> = lex::tokenize(expr)
        .into_iter()
        .filter(|t| !t.is_trivia())
        .collect();
    matches!(toks.as_slice(), [t] if t.kind == lex::TokKind::String)
}

/// The `::SUBMIT`/`::SUBMIT_VARS`/`::SUBMIT_DONE` server-side temps (#517
/// §5): `viewer_state` (one row per recorded variable) and
/// `viewer_state_latest` (each viewer/dashboard/submit's newest row). Lint
/// and headless renderers create these **empty** — one `run_query`/
/// `execute` call per statement, since not every caller's connection runs
/// several `;`-separated statements in one call — when a script mentions
/// `viewer_state` or carries `::SUBMIT`, so a dashboard reading them
/// doesn't fail just because no real server has populated them yet (S3).
/// See [`script_needs_viewer_state`].
pub fn viewer_state_setup_statements() -> [&'static str; 2] {
    [
        "CREATE TEMP TABLE viewer_state (seq BIGINT, ts TIMESTAMPTZ, \"user\" VARCHAR, \
         dashboard VARCHAR, submit VARCHAR, name VARCHAR, value VARCHAR, value_list VARCHAR[])",
        "CREATE TEMP VIEW viewer_state_latest AS SELECT * FROM viewer_state QUALIFY \
         dense_rank() OVER (PARTITION BY \"user\", dashboard, submit ORDER BY seq DESC) = 1",
    ]
}

/// A query a caller runs **before** [`viewer_state_setup_statements`]:
/// `n > 0` (the first column of its one row) means a real (non-temp)
/// `viewer_state`/`viewer_state_latest` already exists on this connection
/// — seeding our own empty temps on top would shadow it (confirmed in
/// DuckDB: a later `CREATE TABLE viewer_state AS …` makes a `SELECT
/// count(*) FROM viewer_state` return the *temp*'s row count, 0, not the
/// real table's — review of #530 #5). The caller must skip seeding when
/// this is `> 0`, so the dashboard's own `viewer_state` wins instead.
pub fn viewer_state_exists_probe_sql() -> &'static str {
    "SELECT count(*) AS n FROM (SELECT table_name AS name FROM duckdb_tables() \
     WHERE NOT temporary UNION ALL SELECT view_name AS name FROM duckdb_views() \
     WHERE NOT temporary) WHERE name IN ('viewer_state', 'viewer_state_latest')"
}

/// Does `script` reference `viewer_state` (any case) or carry a `::SUBMIT`
/// panel? Both mean a real server would (eventually, #517 S3) seed the temp
/// [`viewer_state_setup_sql`] creates — lint and headless renderers create
/// it themselves, empty, so a check or a static render doesn't fail on a
/// missing table.
pub fn script_needs_viewer_state(script: &str) -> bool {
    script.to_ascii_lowercase().contains("viewer_state")
        || plan(script)
            .iter()
            .any(|p| p.roles.iter().any(|(_, r)| matches!(r, Role::Submit)))
}

/// Alias prefix of a ::MARKDOWN statement's named (placeholder) columns:
/// `revenue` → `revenue AS "md:revenue"`, read by `{{revenue}}`.
pub const MD_PREFIX: &str = "md:";

/// The output name of a `::PAGED` table's `::INFO` column (its ⓘ definition):
/// a fixed name, so a page request can leave exactly that column out (`omit`)
/// and nothing else.
pub const INFO_COLUMN: &str = "__anofox_info";

/// Rewrite `<expr>::ROLE` casts in the SELECT list into `<expr> AS c{i}`.
/// A top-level `UNION`/`INTERSECT`/`EXCEPT` reads its roles from the main
/// (first) branch only, but every later branch's own `::ROLE` casts are
/// stripped too — left alone, DuckDB sees them as real (nonexistent) type
/// casts and raises a raw SQL error instead of rendering the panel (#294).
pub fn rewrite(stmt: &str) -> Rewritten {
    let none = || (stmt.to_string(), Vec::new(), Vec::new());
    if !is_query_statement(stmt) {
        return none();
    }
    let spans = select_list_spans(stmt);
    let Some(&(sel, list_end)) = spans.first() else {
        return none();
    };
    let (items0, roles, names) = rewrite_list(&stmt[sel..list_end]);
    // No early bail-out when `roles` is empty: `rewrite_list` still renamed
    // every item to `c{i}` (or left `::MARKDOWN`'s named columns / `*` alone)
    // even without a recognised role, and callers that use this `sql`
    // directly regardless of `roles` — e.g. `contract_call::plan_contract`,
    // which rewrites a contract call's payload columns this way and may have
    // no other `::ROLE` cast at all — depend on that rewrite happening. It is
    // [`plan_statements`], not this function, that decides setup vs. panel
    // from `roles.is_empty()` and falls back to the original `stmt` text.
    let mut out = String::new();
    out.push_str(&stmt[..sel]);
    out.push(' ');
    out.push_str(&items0);
    let mut prev_end = list_end;
    for &(s, e) in &spans[1..] {
        out.push(' ');
        out.push_str(&stmt[prev_end..s]);
        out.push(' ');
        let (items, _, _) = rewrite_list(&stmt[s..e]);
        out.push_str(&items);
        prev_end = e;
    }
    out.push(' ');
    out.push_str(&stmt[prev_end..]);
    (out, roles, names)
}

/// Strip a leading `DISTINCT` keyword (as DuckDB itself ignores it when
/// naming the column): `"DISTINCT channel"` → `"channel"`. Anything else is
/// returned unchanged.
fn strip_leading_distinct(expr: &str) -> &str {
    let toks = lex::tokenize(expr);
    match toks.iter().find(|t| !t.is_trivia()) {
        Some(t) if t.is_word(expr, "DISTINCT") => expr[t.end..].trim_start(),
        _ => expr,
    }
}

/// Rewrite one SELECT list's items (`<expr>::ROLE` → `<expr> AS c{i}`, same
/// rules as [`rewrite`]) and return the joined item text plus the roles and
/// display names found, keyed by position within this list.
fn rewrite_list(list: &str) -> Rewritten {
    let split = split_top_commas(list);

    // Unambiguous roles present in the list decide whether the SQL-type tokens
    // (DATE/TEXT/STRING/NUMERIC) are input roles or plain casts.
    let unambiguous: Vec<Role> = split
        .iter()
        .filter_map(|it| lex::trailing_cast(it.trim()))
        .filter(|(_, tok)| !is_sql_type_token(tok))
        .filter_map(|(_, tok)| parse_role(tok))
        .collect();
    let inputs_ok = unambiguous.iter().all(|r| {
        matches!(
            r,
            Role::Input(_) | Role::Hint | Role::OptionLabel | Role::Label | Role::Title
        )
    });

    // ::TABLE, ::DATERANGE and ::DOWNLOAD_* keep every column and its name — the
    // whole result is the payload; strip only the casts.
    let keep_intact = |r: Role| {
        matches!(
            r,
            Role::Table | Role::PagedTable | Role::Input(InputKind::DateRange) | Role::Download(_)
        )
    };
    let intact = unambiguous.iter().copied().find(|r| keep_intact(*r));
    if let Some(role) = intact {
        // Keep every column and its name; strip the recognised (unambiguous)
        // role casts — real casts like `name::TEXT` stay. A ::TITLE column is
        // recorded by its output position so the panel shows a title bar (the
        // browser drops that column from a table's cells).
        let mut roles = vec![(0, role)];
        let items: Vec<String> = split
            .iter()
            .enumerate()
            .map(|(idx, it)| match lex::trailing_cast(it.trim()) {
                Some((expr, tok)) if !is_sql_type_token(tok) && parse_role(tok).is_some() => {
                    // Record per-column table formatting (title bar, trend arrow,
                    // number format, colour scale, badge, sparkline) by output
                    // position so the browser can render each.
                    if let Some(rr) = parse_role(tok) {
                        if matches!(rr, Role::Title | Role::Info)
                            || crate::roles::is_table_format(rr)
                        {
                            roles.push((idx, rr));
                        }
                        if rr == Role::Info && role == Role::PagedTable {
                            return format!("{} AS \"{INFO_COLUMN}\"", trailing_alias(expr).0);
                        }
                    }
                    expr.to_string()
                }
                _ => it.trim().to_string(),
            })
            .collect();
        return (items.join(", "), roles, Vec::new());
    }

    let markdown = unambiguous.contains(&Role::Markdown);
    let mut roles = Vec::new();
    let mut names = Vec::new();
    let mut items = Vec::new();
    // A KPI statement (a ::METRIC/::MONEY/… column): its ::SPARKLINE column
    // is the tile's trend line, a list.
    let kpi_stmt = unambiguous.iter().any(|r| matches!(r, Role::Metric(_)));
    for (i, item) in split.into_iter().enumerate() {
        let item = item.trim();
        if let Some((expr, role)) = item_role(item, inputs_ok) {
            // Cast measures/metrics to DOUBLE so sum()/BIGINT/HUGEINT come back
            // as real numbers (DuckDB-Wasm otherwise serialises HUGEINT as str).
            let rewritten = match role {
                // A KPI's sparkline is a list (`list(v ORDER BY d)`): cast
                // to DOUBLE[] (a list of HUGEINT sums would not read as
                // numbers in DuckDB-Wasm).
                Role::Value(Kind::Sparkline) if kpi_stmt => {
                    format!("CAST({} AS DOUBLE[]) AS c{i}", trailing_alias(expr).0)
                }
                // #332: a ::HEATMAP's measure is left as its own type (not
                // forced to DOUBLE, like ::CATEGORY/::Input) so a string
                // column — a status word such as 'blocked', coloured with
                // ::COLORS as a level map — reaches the renderer intact for
                // a categorical legend, instead of erroring (`Conversion
                // Error: Could not convert string … to DOUBLE`) before it
                // ever gets there. A numeric measure still arrives as a real
                // number (DuckDB's own INTEGER/DOUBLE), just not widened.
                Role::Value(Kind::Heatmap) => {
                    let (core, alias) = trailing_alias(expr);
                    if let Some(nm) = alias.or_else(|| lex::simple_ident(core)) {
                        names.push((i, nm));
                    }
                    format!("{core} AS c{i}")
                }
                Role::Value(_)
                | Role::Metric(_)
                | Role::Delta
                | Role::DeltaLowerGood
                | Role::RefLine
                | Role::BandLower
                | Role::BandUpper
                | Role::YMin
                | Role::YMax
                | Role::XMin
                | Role::XMax
                | Role::Rank
                | Role::Censor
                | Role::CooksContour
                | Role::Trend
                | Role::Reload
                | Role::Bivariate => {
                    // A charted measure remembers a human name (explicit
                    // trailing `AS alias`, else a bare column name) so a combo
                    // legend can read e.g. "observed"/"trend". The output alias
                    // stays `c{i}` so the render side keeps its by-position
                    // column lookup — the name travels as separate metadata.
                    // `::BIVARIATE` (M2d) needs its own name too — the
                    // bivariate legend's "{B} →" caption and tooltip read it
                    // the same way `value_scale` reads a measure's.
                    let (core, alias) = trailing_alias(expr);
                    if matches!(role, Role::Value(_) | Role::Bivariate) {
                        if let Some(nm) = alias.or_else(|| lex::simple_ident(core)) {
                            names.push((i, nm));
                        }
                    }
                    format!("CAST({core} AS DOUBLE) AS c{i}")
                }
                // Inputs keep the original column name — it becomes the
                // DuckDB variable name the browser binds the control to.
                // #428: that name is also recorded in `names` (an explicit
                // `AS alias`, else the bare column reference) so the browser
                // can read it from the plan alone, before the statement's
                // own OPTIONS query has ever run — letting it apply a known
                // value (a previous pick, a URL seed) for one input before
                // fetching another's options, regardless of their order in
                // the script. Left out when the column has no simple name
                // (a bare, unaliased expression); the browser falls back to
                // its own, unchanged, run-it-and-read-the-column-name path.
                Role::Input(_) => {
                    let (core, alias) = trailing_alias(expr);
                    // `SELECT DISTINCT channel ::DROPDOWN` (a common way to
                    // list an input's options): DISTINCT is a select-list
                    // modifier, not part of the column's own name.
                    let bare = alias.is_none().then(|| strip_leading_distinct(core));
                    if let Some(nm) = alias.or_else(|| lex::simple_ident(bare.unwrap_or(core))) {
                        names.push((i, nm));
                    }
                    expr.to_string()
                }
                // A `::VIEW` marker's explicit `AS alias` names its group
                // (#214) — kept in `names` (views::assign_views reads it
                // back), the value's own text kept intact (not even
                // re-aliased away) so views::assign_views can tell a literal
                // label from a computed one straight off the rewritten SQL.
                Role::View => {
                    let (core, alias) = trailing_alias(expr);
                    if let Some(nm) = alias {
                        names.push((i, nm));
                    }
                    format!("{core} AS c{i}")
                }
                // Any other role: an explicit `AS alias` is replaced by c{i}.
                _ => format!("{} AS c{i}", trailing_alias(expr).0),
            };
            roles.push((i, role));
            items.push(rewritten);
            continue;
        }
        if lex::is_star_item(item) {
            items.push(item.to_string()); // `*`, `t.*`, COLUMNS(…) expand in place
        } else if let Some(name) = markdown
            .then(|| {
                let (core, alias) = trailing_alias(item);
                alias.or_else(|| lex::simple_ident(core))
            })
            .flatten()
        {
            // A ::MARKDOWN statement's other columns keep their names for the
            // text's {{name}} placeholders, under a reserved prefix so a user
            // column named `c0` never collides with the positional `c{i}`
            // keys (#138).
            let core = trailing_alias(item).0;
            items.push(format!(
                "{core} AS \"{MD_PREFIX}{}\"",
                name.replace('"', "\"\"")
            ));
        } else {
            items.push(format!("{} AS c{i}", trailing_alias(item).0));
        }
    }
    (items.join(", "), roles, names)
}

/// `Some(s)` if `s` is a bare SQL identifier (so a plain `col ::LINECHART` can
/// carry `col` as its series name); `None` for anything computed.
pub fn simple_ident(s: &str) -> Option<String> {
    lex::simple_ident(s)
}

#[cfg(test)]
mod alias_tests {
    use super::*;
    #[test]
    fn markdown_columns_never_collide_with_positional_keys() {
        // #138: a value column named c0 next to the ::MARKDOWN text (c0)
        let (s, roles, _) =
            rewrite("SELECT 'x {{c0:money}}'::MARKDOWN, 4.5 AS c0, n, \"Q \"\"3\"\"\" FROM t");
        assert!(s.contains("'x {{c0:money}}' AS c0,"), "{s}");
        assert!(s.contains(r#"4.5 AS "md:c0""#), "{s}");
        assert!(s.contains(r#"n AS "md:n""#), "{s}");
        assert!(s.contains(r#""Q ""3""" AS "md:Q ""3""""#), "{s}");
        assert_eq!(roles, vec![(0, Role::Markdown)]);
    }
    #[test]
    fn value_columns_keep_names() {
        // Data lookup stays keyed by c{i}; the human name travels as metadata.
        // Explicit quoted alias → name, alias stripped from the DOUBLE cast.
        let (s, _, n) = rewrite(
            r#"SELECT ds ::XAXIS, y AS "sales" ::LINECHART, CASE WHEN prob>0.7 THEN y END AS "changepoint" ::SCATTER FROM cp"#,
        );
        assert!(s.contains("CAST(y AS DOUBLE) AS c1"), "{s}");
        assert!(
            s.contains("CAST(CASE WHEN prob>0.7 THEN y END AS DOUBLE) AS c2"),
            "{s}"
        );
        assert_eq!(
            n,
            vec![(1, "sales".to_string()), (2, "changepoint".to_string())]
        );
        // Bare column name becomes the series name.
        let (_, _, n2) =
            rewrite("SELECT ds ::XAXIS, observed ::LINECHART, trend ::LINECHART FROM d");
        assert_eq!(
            n2,
            vec![(1, "observed".to_string()), (2, "trend".to_string())]
        );
        // A computed expr with no alias has no name (legend falls back to c{i}).
        let (s3, _, n3) = rewrite("SELECT ds ::XAXIS, sum(y) ::BARCHART FROM d");
        assert!(s3.contains("CAST(sum(y) AS DOUBLE) AS c1"), "{s3}");
        assert!(n3.is_empty(), "{n3:?}");
        // Nested CAST(.. AS ..) must not be mistaken for a trailing alias.
        let (s4, _, _) = rewrite("SELECT ds ::XAXIS, CAST(y AS INT) ::LINECHART FROM d");
        assert!(s4.contains("CAST(CAST(y AS INT) AS DOUBLE) AS c1"), "{s4}");
    }
}

#[cfg(test)]
mod decimal_x_tests {
    use super::*;

    #[test]
    fn decimal_strings_on_x_are_numeric() {
        let s = |v: &[&str]| {
            v.iter()
                .map(|x| Value::Str(x.to_string()))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            maybe_decimal(s(&["5.0", "5.2", "-1.5"])),
            vec![Value::Float(5.0), Value::Float(5.2), Value::Float(-1.5)]
        );
        assert_eq!(maybe_decimal(s(&["2019", "2020"])), s(&["2019", "2020"]));
        assert_eq!(maybe_decimal(s(&["1.5", "app"])), s(&["1.5", "app"]));
        assert_eq!(maybe_decimal(s(&["1e3", "2.0"])), s(&["1e3", "2.0"]));
    }
}

#[cfg(test)]
mod requires_tests {
    use super::*;

    #[test]
    fn requires_comments_attach_to_their_statement() {
        let p = plan(
            "SELECT 1 ::METRIC;\n-- optional output of the run\n-- @requires 'data/run/x.parquet', data/run/y.csv\n\
             SELECT a ::XAXIS, b ::BARCHART FROM 'data/run/x.parquet';\n-- @requiresnothing\nSELECT 2 ::METRIC",
        );
        assert_eq!(p.len(), 3);
        assert!(p[0].requires.is_empty());
        assert_eq!(
            p[1].requires,
            vec![vec!["data/run/x.parquet"], vec!["data/run/y.csv"]]
        );
        assert!(p[2].requires.is_empty());
        assert!(!p[1].sql.contains("@requires"));
    }

    #[test]
    fn fit_survives_planning_in_its_section() {
        // #400: a `'fit'::FIT` marker is planned as its own (non-setup)
        // directive statement inside its ::TAB's section, and the wire
        // token the browser matches on is "FIT" — like ::HEIGHT / ::TAB.
        let p = plan(
            "SELECT 'Overview'::TAB;
             SELECT 'fit'::FIT;
             SELECT 1 AS x ::XAXIS, 2 AS y ::BARCHART;
             SELECT 'Details'::TAB;
             SELECT 3 ::METRIC",
        );
        assert_eq!(p.len(), 5);
        assert!(!p[1].setup);
        assert_eq!(p[1].roles, vec![(0, Role::Fit)]);
        assert_eq!(p[1].roles[0].1.token(), "FIT");
        assert!(crate::roles::is_directive_panel(&p[1].roles));
        assert_eq!(p[1].tabs, vec![0], "in the Overview tab's section");
        assert_eq!(p[1].sql.trim(), "SELECT 'fit' AS c0");
        let fits = p
            .iter()
            .filter(|q| q.roles.iter().any(|(_, r)| *r == Role::Fit))
            .count();
        assert_eq!(fits, 1);
        // the alias plans to the same role
        let q = plan("SELECT 'fit'::FIT_SCREEN");
        assert_eq!(q[0].roles, vec![(0, Role::Fit)]);
    }

    #[test]
    fn a_tab_passes_its_requires_to_its_section() {
        let p = plan(
            "SELECT 1 ::METRIC;
             -- @requires a.parquet
SELECT 'A'::TAB;
             SELECT 2 ::METRIC;
             CREATE TEMP VIEW v AS SELECT 1;
             SELECT 'dark'::THEME;
             -- @requires b.parquet
SELECT 'B1'::SUBTAB;
             -- @requires c.parquet
SELECT 3 ::METRIC;
             SELECT 'B2'::SUBTAB;
             SELECT 4 ::METRIC;
             SELECT 'Plain'::TAB;
             SELECT 5 ::METRIC",
        );
        let tab: Vec<Vec<Vec<String>>> = p.iter().map(Panel::all_requires).collect();
        let r = |f: &str| vec![f.to_string()];
        let (a, b, c) = (r("a.parquet"), r("b.parquet"), r("c.parquet"));
        assert_eq!(
            tab,
            vec![
                vec![],                     // before the first tab
                vec![a.clone()],            // the tab: its own
                vec![a.clone()],            // its panel
                vec![],                     // setup: only its own
                vec![],                     // chrome: the whole dashboard
                vec![b.clone(), a.clone()], // the sub-tab: its own + the tab's
                vec![c, a.clone(), b],      // its panel: own + tab + sub-tab
                vec![a.clone()],            // the next sub-tab
                vec![a.clone()],
                vec![], // a new tab starts fresh
                vec![],
            ]
        );
        assert_eq!(p[3].tab_requires, vec![vec!["a.parquet".to_string()]]);
        assert_eq!(
            p.iter().map(Panel::tab_kind).collect::<Vec<_>>()[..3],
            [None, Some(false), None]
        );
        assert_eq!(p[5].tab_kind(), Some(true));
        // where they are decided: the tab, then the sub-tab
        assert_eq!(p[6].tabs, [1, 5]);
        assert_eq!(p[5].tabs, [1]);
        assert_eq!(p[2].tabs, [1]);
        assert!(p[1].tabs.is_empty() && p[4].tabs.is_empty() && p[0].tabs.is_empty());
        assert_eq!(p[10].tabs, [9]);
    }

    #[test]
    fn missing_input_errors_are_recognised() {
        assert!(is_missing_input_error(
            r#"IO Error: No files found that match the pattern "x.parquet""#
        ));
        assert!(is_missing_input_error(
            "Catalog Error: Table with name t does not exist!"
        ));
        assert!(!is_missing_input_error(
            "Binder Error: Referenced column \"y\" not found"
        ));
    }
}

#[cfg(test)]
mod xline_date_tests {
    use super::*;

    // #44: a DATE/TIMESTAMP ::XLINE has no DOUBLE cast — it travels as text and
    // becomes an x position in the date axis's units (epoch seconds).
    #[test]
    fn a_date_xline_is_not_cast_to_double() {
        let (s, _, _) =
            rewrite("SELECT d ::XAXIS, v ::LINECHART, DATE '2026-03-15' ::XLINE FROM t");
        assert!(s.contains("DATE '2026-03-15' AS c2"), "{s}");
        assert!(!s.contains("AS DOUBLE) AS c2"), "{s}");
    }

    #[test]
    fn xline_values_become_x_positions() {
        let vals = vec![
            Value::Str("2026-03-15".into()),
            Value::Na,
            Value::Str("2026-03-15 12:00:00".into()),
        ];
        assert_eq!(
            coerce_for_role(Role::VLine, vals),
            vec![
                Value::DateTime(1_773_532_800),
                Value::Na,
                Value::DateTime(1_773_576_000)
            ]
        );
        // Numbers (BIGINT/DECIMAL arrive as strings) stay numbers.
        let vals = vec![
            Value::Str("12".into()),
            Value::Float(3.5),
            Value::Str("1.25".into()),
        ];
        assert_eq!(
            coerce_for_role(Role::VLine, vals),
            vec![Value::Float(12.0), Value::Float(3.5), Value::Float(1.25)]
        );
    }
}

#[cfg(test)]
mod option_label_tests {
    use super::*;
    use crate::{InputKind, Role};
    #[test]
    fn option_label_is_a_positional_column_of_an_input() {
        let p = plan("SELECT run AS run ::DROPDOWN, strftime(w, '%d %b') || ' — ' || run AS run_label ::OPTION_LABEL, 'Run'::LABEL FROM runs;");
        let p = &p[0];
        assert!(
            p.roles.contains(&(0, Role::Input(InputKind::Dropdown))),
            "{:?}",
            p.roles
        );
        assert!(p.roles.contains(&(1, Role::OptionLabel)), "{:?}", p.roles);
        assert!(
            p.sql.contains("run AS run") && p.sql.contains("AS c1"),
            "{}",
            p.sql
        );
        // an input statement still: ::DATE stays the input role next to it
        let p = plan("SELECT DATE '2026-01-01' AS d ::DATE, 'x' ::OPTION_LABEL;");
        assert!(
            p[0].roles.contains(&(0, Role::Input(InputKind::Date))),
            "{:?}",
            p[0].roles
        );
    }
}

#[cfg(test)]
mod union_tests {
    use super::*;

    // #294: a top-level UNION ALL's later branches left their ::ROLE casts
    // unstripped — DuckDB then saw e.g. `week::XAXIS` as a real (nonexistent)
    // type cast and raised a raw SQL error instead of the panel rendering.
    #[test]
    fn roles_are_stripped_from_every_union_all_branch() {
        let (s, roles, names) = rewrite(
            "SELECT week::XAXIS, metric::CATEGORY, n::LINECHART FROM sessions WHERE metric = 'A'\n\
             UNION ALL\n\
             SELECT week::XAXIS, metric::CATEGORY, n::LINECHART FROM sessions WHERE metric = 'B'",
        );
        assert!(
            !s.contains("::XAXIS") && !s.contains("::CATEGORY") && !s.contains("::LINECHART"),
            "{s}"
        );
        // The main (first) branch's roles decide the panel's shape.
        assert_eq!(
            roles,
            vec![
                (0, Role::X),
                (1, Role::Category),
                (2, Role::Value(Kind::Line))
            ]
        );
        assert_eq!(names, vec![(2, "n".to_string())]);
        // Both branches keep the same positional aliases and DOUBLE cast on
        // the measure column, so DuckDB can union them without a type error.
        assert_eq!(s.matches("week AS c0").count(), 2, "{s}");
        assert_eq!(s.matches("metric AS c1").count(), 2, "{s}");
        assert_eq!(s.matches("CAST(n AS DOUBLE) AS c2").count(), 2, "{s}");
        assert_eq!(s.matches("UNION ALL").count(), 1, "{s}");
    }

    // A later branch need not repeat the casts: its bare columns just take
    // the same positional aliases as the main branch.
    #[test]
    fn a_later_branch_without_casts_still_gets_positional_aliases() {
        let (s, roles, _) =
            rewrite("SELECT week::XAXIS, n::LINECHART FROM a UNION ALL SELECT week, n FROM b");
        assert_eq!(roles, vec![(0, Role::X), (1, Role::Value(Kind::Line))]);
        assert_eq!(s.matches("week AS c0").count(), 2, "{s}");
        assert!(s.trim_end().ends_with("n AS c1 FROM b"), "{s}");
    }

    // Three-way UNION ALL (and plain UNION / INTERSECT / EXCEPT): every
    // branch gets stripped, not just a second one.
    #[test]
    fn three_way_union_and_other_set_ops_all_strip() {
        for op in ["UNION ALL", "UNION", "INTERSECT", "EXCEPT"] {
            let sql = format!(
                "SELECT week::XAXIS, n::LINECHART FROM a {op} SELECT week::XAXIS, n::LINECHART FROM b {op} SELECT week::XAXIS, n::LINECHART FROM c"
            );
            let (s, roles, _) = rewrite(&sql);
            assert!(
                !s.contains("::XAXIS") && !s.contains("::LINECHART"),
                "{op}: {s}"
            );
            assert_eq!(
                roles,
                vec![(0, Role::X), (1, Role::Value(Kind::Line))],
                "{op}"
            );
            assert_eq!(s.matches("CAST(n AS DOUBLE) AS c1").count(), 3, "{op}: {s}");
        }
    }

    // A trailing ORDER BY/LIMIT on the whole UNION result doesn't confuse the
    // branch-finder into treating it as another branch.
    #[test]
    fn order_by_and_limit_after_the_union_are_left_alone() {
        let (s, roles, _) = rewrite(
            "SELECT week::XAXIS, n::LINECHART FROM a UNION ALL SELECT week::XAXIS, n::LINECHART FROM b ORDER BY 1 LIMIT 10",
        );
        assert_eq!(roles, vec![(0, Role::X), (1, Role::Value(Kind::Line))]);
        assert!(s.trim_end().ends_with("ORDER BY 1 LIMIT 10"), "{s}");
        assert_eq!(s.matches("CAST(n AS DOUBLE) AS c1").count(), 2, "{s}");
    }

    // No UNION at all: unchanged behaviour (single-span path).
    #[test]
    fn a_plain_query_is_unaffected() {
        let (s, roles, names) = rewrite("SELECT d ::XAXIS, v ::LINECHART FROM t");
        assert!(s.contains("CAST(v AS DOUBLE) AS c1"), "{s}");
        assert_eq!(roles, vec![(0, Role::X), (1, Role::Value(Kind::Line))]);
        assert_eq!(names, vec![(1, "v".to_string())]);
    }
}
