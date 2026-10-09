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

pub mod lex;

pub use lex::{split_top_commas, strip_comments, trailing_alias};

use crate::roles::is_sql_type_token;
use crate::{parse_role, Column, InputKind, Role};
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
}

/// Parse a whole script into ordered [`Panel`]s.
pub fn plan(script: &str) -> Vec<Panel> {
    let clean = strip_comments(script);
    split_statements(&clean)
        .into_iter()
        .filter_map(|stmt| {
            let stmt = stmt.trim();
            if stmt.is_empty() {
                return None;
            }
            let (sql, roles, names) = rewrite(stmt);
            Some(if roles.is_empty() {
                Panel {
                    setup: true,
                    sql: stmt.to_string(),
                    roles,
                    names: Vec::new(),
                }
            } else {
                Panel {
                    setup: false,
                    sql,
                    roles,
                    names,
                }
            })
        })
        .collect()
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
            let mut values: Vec<Value> = rows.iter().map(|r| jval(r.get(&key), numeric)).collect();
            // An X axis whose values are all ISO dates/timestamps becomes a
            // continuous datetime scale, so ggplot-rs picks sensible (e.g. yearly)
            // breaks instead of drawing one overlapping label per value.
            if matches!(role, Role::X) {
                values = maybe_decimal(maybe_datetime(values));
            }
            Column::new(key, *role, values)
        })
        .collect()
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
fn maybe_decimal(vals: Vec<Value>) -> Vec<Value> {
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

/// Rewrite `<expr>::ROLE` casts in the SELECT list into `<expr> AS c{i}`.
pub fn rewrite(stmt: &str) -> Rewritten {
    let none = || (stmt.to_string(), Vec::new(), Vec::new());
    if !is_query_statement(stmt) {
        return none();
    }
    let Some((sel, list_end)) = main_select_list(stmt) else {
        return none();
    };
    let (head, list, tail) = (&stmt[..sel], &stmt[sel..list_end], &stmt[list_end..]);
    let split = split_top_commas(list);

    // Unambiguous roles present in the list decide whether the SQL-type tokens
    // (DATE/TEXT/STRING/NUMERIC) are input roles or plain casts.
    let unambiguous: Vec<Role> = split
        .iter()
        .filter_map(|it| lex::trailing_cast(it.trim()))
        .filter(|(_, tok)| !is_sql_type_token(tok))
        .filter_map(|(_, tok)| parse_role(tok))
        .collect();
    let inputs_ok = unambiguous
        .iter()
        .all(|r| matches!(r, Role::Input(_) | Role::Hint | Role::Label | Role::Title));

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
                        if rr == Role::Title || crate::roles::is_table_format(rr) {
                            roles.push((idx, rr));
                        }
                    }
                    expr.to_string()
                }
                _ => it.trim().to_string(),
            })
            .collect();
        return (
            format!("{head} {} {tail}", items.join(", ")),
            roles,
            Vec::new(),
        );
    }

    let mut roles = Vec::new();
    let mut names = Vec::new();
    let mut items = Vec::new();
    for (i, item) in split.into_iter().enumerate() {
        let item = item.trim();
        if let Some((expr, role)) = item_role(item, inputs_ok) {
            // Cast measures/metrics to DOUBLE so sum()/BIGINT/HUGEINT come back
            // as real numbers (DuckDB-Wasm otherwise serialises HUGEINT as str).
            let rewritten = match role {
                Role::Value(_)
                | Role::Metric(_)
                | Role::Delta
                | Role::RefLine
                | Role::VLine
                | Role::BandLower
                | Role::BandUpper
                | Role::YMin
                | Role::YMax
                | Role::XMin
                | Role::XMax
                | Role::Rank
                | Role::Censor
                | Role::Trend
                | Role::Reload => {
                    // A charted measure remembers a human name (explicit
                    // trailing `AS alias`, else a bare column name) so a combo
                    // legend can read e.g. "observed"/"trend". The output alias
                    // stays `c{i}` so the render side keeps its by-position
                    // column lookup — the name travels as separate metadata.
                    let (core, alias) = trailing_alias(expr);
                    if matches!(role, Role::Value(_)) {
                        if let Some(nm) = alias.or_else(|| lex::simple_ident(core)) {
                            names.push((i, nm));
                        }
                    }
                    format!("CAST({core} AS DOUBLE) AS c{i}")
                }
                // Inputs keep the original column name — it becomes the
                // DuckDB variable name the browser binds the control to.
                Role::Input(_) => expr.to_string(),
                // Any other role: an explicit `AS alias` is replaced by c{i}.
                _ => format!("{} AS c{i}", trailing_alias(expr).0),
            };
            roles.push((i, role));
            items.push(rewritten);
            continue;
        }
        if lex::is_star_item(item) {
            items.push(item.to_string()); // `*`, `t.*`, COLUMNS(…) expand in place
        } else {
            items.push(format!("{} AS c{i}", trailing_alias(item).0));
        }
    }
    (format!("{head} {} {tail}", items.join(", ")), roles, names)
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
