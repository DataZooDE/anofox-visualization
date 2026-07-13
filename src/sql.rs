//! Shaper-style SQL parsing: strip comments, split statements, and pull `::ROLE`
//! casts off the SELECT list. Shared by the native `dashboard` bin and the wasm
//! binding so the browser and CLI behave identically.

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
            let mut values: Vec<Value> = rows.iter().map(|r| jval(r.get(&key), numeric)).collect();
            // An X axis whose values are all ISO dates/timestamps becomes a
            // continuous datetime scale, so ggplot-rs picks sensible (e.g. yearly)
            // breaks instead of drawing one overlapping label per value.
            if matches!(role, Role::X) {
                values = maybe_datetime(values);
            }
            Column::new(key, *role, values)
        })
        .collect()
}

fn jval(v: Option<&serde_json::Value>, numeric: bool) -> Value {
    match v {
        Some(serde_json::Value::Number(n)) => n.as_f64().map(Value::Float).unwrap_or(Value::Na),
        Some(serde_json::Value::String(s)) => match numeric {
            true => s
                .parse::<f64>()
                .map(Value::Float)
                .unwrap_or_else(|_| Value::Str(s.clone())),
            false => Value::Str(s.clone()),
        },
        Some(serde_json::Value::Bool(b)) => Value::Bool(*b),
        _ => Value::Na,
    }
}

/// If every non-null value is an ISO date/timestamp string, reinterpret the
/// column as [`Value::DateTime`] (epoch seconds); otherwise leave it unchanged.
fn maybe_datetime(vals: Vec<Value>) -> Vec<Value> {
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

/// Parse `YYYY-MM-DD` or `YYYY-MM-DD[ T]HH:MM:SS` into seconds since the Unix
/// epoch (UTC). Returns `None` for anything that isn't that exact shape.
fn parse_iso_epoch(s: &str) -> Option<i64> {
    let s = s.trim();
    let b = s.as_bytes();
    if b.len() < 10 || b[4] != b'-' || b[7] != b'-' {
        return None;
    }
    let y: i64 = s.get(0..4)?.parse().ok()?;
    let mo: u32 = s.get(5..7)?.parse().ok()?;
    let d: u32 = s.get(8..10)?.parse().ok()?;
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) {
        return None;
    }
    let mut secs = days_from_civil(y, mo, d) * 86_400;
    if b.len() >= 19 && (b[10] == b' ' || b[10] == b'T') {
        let hh: i64 = s.get(11..13)?.parse().ok()?;
        let mi: i64 = s.get(14..16)?.parse().ok()?;
        let ss: i64 = s.get(17..19)?.parse().ok()?;
        if hh > 23 || mi > 59 || ss > 60 {
            return None;
        }
        secs += hh * 3600 + mi * 60 + ss;
    }
    Some(secs)
}

/// Days since 1970-01-01 for a civil (Y, M, D) date — Howard Hinnant's algorithm.
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 } as i64;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

const ROLES: &[&str] = &[
    "XAXIS",
    "X",
    "YAXIS",
    "Y",
    "CATEGORY",
    "SERIES",
    "COLOR",
    "COLOUR",
    "LABEL",
    "TITLE",
    "HEADING",
    "BARCHART",
    "BAR",
    "BARCHART_STACKED",
    "BAR_STACKED",
    "STACKED_BAR",
    "LINECHART",
    "LINE",
    "AREACHART",
    "AREA",
    "STEP",
    "STEPLINE",
    "STEP_LINE",
    "SMOOTH",
    "TRENDLINE",
    "TREND_LINE",
    "AREACHART_STACKED",
    "AREA_STACKED",
    "STACKED_AREA",
    "SIZE",
    "BUBBLE",
    "DATALABELS",
    "DATALABEL",
    "VALUELABELS",
    "SHOWLABELS",
    "MARKAREA",
    "MARK_AREA",
    "SHADE",
    "MARKDOWN",
    "MD",
    "TEXTBOX",
    "RICHTEXT",
    "SCATTER",
    "JITTER",
    "JITTERCHART",
    "STRIP",
    "CANDLESTICK",
    "CANDLE",
    "OHLC",
    "RADAR",
    "SPIDER",
    "OPEN",
    "HIGH",
    "LOW",
    "POINT",
    "SCATTERCHART",
    "PIE",
    "DONUT",
    "PIECHART",
    "HISTOGRAM",
    "HIST",
    "BOXPLOT",
    "BOX_PLOT",
    "VIOLIN",
    "VIOLINPLOT",
    "DENSITY",
    "KDE",
    "QQ",
    "QQPLOT",
    "FLIP",
    "COORD_FLIP",
    "YFORMAT",
    "XFORMAT",
    "YCURRENCY",
    "XCURRENCY",
    "HORIZONTAL",
    "ALPHA",
    "OPACITY",
    "HEATMAP",
    "CALENDAR",
    "CALENDAR_HEATMAP",
    "CAL_HEATMAP",
    "TILE",
    "TILES",
    "SPARKLINE",
    "SPARK",
    "REFLINE",
    "TARGET",
    "GOAL",
    "MAP",
    "GEOMETRY",
    "GEO",
    "CHOROPLETH",
    "BASEMAP",
    "MAPBASE",
    "BACKDROP",
    "TABLE",
    "PAGED",
    "TABLE_PAGED",
    "PAGINATED",
    "GRID",
    "METRIC",
    "KPI",
    "BIGNUMBER",
    "DROPDOWN",
    "OPTIONS",
    "SELECT_INPUT",
    "NUMBER",
    "SLIDER",
    "NUMERIC",
    "DATE",
    "DATEPICKER",
    "TEXT",
    "SEARCH",
    "STRING",
    "MULTISELECT",
    "MULTI",
    "DATERANGE",
    "DATE_RANGE",
    "MONEY",
    "DOLLAR",
    "CURRENCY",
    "PERCENT",
    "PCT",
    "COMPACT",
    "DELTA",
    "COMPARE",
    "PREVIOUS",
    "TAB",
    "PAGE",
    "SUBTAB",
    "SUB_TAB",
    "COLUMNS",
    "COLS",
    "GROUP",
    "BOX",
    "ROW",
    "ENDGROUP",
    "ENDBOX",
    "ENDROW",
    "SPAN",
    "WIDTH",
    "COL",
    "HEIGHT",
    "TALL",
    // Shaper-parity additions:
    "BARCHART_PERCENT",
    "BAR_PERCENT",
    "BARCHART_STACKED_PERCENT",
    "BAR_STACKED_PERCENT",
    "LINECHART_PERCENT",
    "LINE_PERCENT",
    "DONUTCHART",
    "DONUTCHART_PERCENT",
    "PIECHART_PERCENT",
    "GAUGE",
    "GAUGE_PERCENT",
    "YLINE",
    "XLINE",
    "BAND_LOWER",
    "BANDLOWER",
    "BAND_UPPER",
    "BANDUPPER",
    "TREND",
    "HINT",
    "TEXT_SMALL",
    "TEXT_MEDIUM",
    "TEXT_LARGE",
    "PLACEHOLDER",
    "HEADER_IMAGE",
    "HEADERIMAGE",
    "FOOTER_LINK",
    "FOOTERLINK",
    "DOWNLOAD_CSV",
    "DOWNLOAD_XLSX",
    "DOWNLOAD_EXCEL",
    "DOWNLOAD_PDF",
    "RELOAD",
    "REFRESH",
    "RANGE",
    "LABELS",
    "COLORS",
    "COLOURS",
    "COLORSCALE",
    "COLOURSCALE",
    "HEAT",
    "GRADIENT",
    "BADGE",
    "STATUS",
    "PILL",
    "PLAIN",
    "NOBAR",
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

/// `(rewritten SQL, [(col idx, role)], [(col idx, display name)])`.
type Rewritten = (String, Vec<(usize, Role)>, Vec<(usize, String)>);

/// Rewrite `<expr>::ROLE` casts in the SELECT list into `<expr> AS c{i}`.
pub fn rewrite(stmt: &str) -> Rewritten {
    let up = stmt.to_ascii_uppercase();
    let sel = match up.find("SELECT") {
        Some(p) => p + 6,
        None => return (stmt.to_string(), Vec::new(), Vec::new()),
    };
    let list_end = top_level_kw(&stmt[sel..], "FROM")
        .map(|o| sel + o)
        .unwrap_or(stmt.len());
    let (head, list, tail) = (&stmt[..sel], &stmt[sel..list_end], &stmt[list_end..]);
    let split = split_top_commas(list);

    // ::TABLE, ::DATERANGE and ::DOWNLOAD_* keep every column and its name — the
    // whole result is the payload; strip only the casts.
    let keep_intact = |r: Role| {
        matches!(
            r,
            Role::Table | Role::PagedTable | Role::Input(InputKind::DateRange) | Role::Download(_)
        )
    };
    let intact = split.iter().find_map(|it| {
        trailing_role(it.trim())
            .and_then(|(_, r)| parse_role(r))
            .filter(|r| keep_intact(*r))
    });
    if let Some(role) = intact {
        // Keep every column and its name; strip the recognised casts. A ::TITLE
        // column is recorded by its output position so the panel shows a title
        // bar (the browser drops that column from a table's cells).
        let mut roles = vec![(0, role)];
        let items: Vec<String> = split
            .iter()
            .enumerate()
            .map(|(idx, it)| match trailing_role(it.trim()) {
                Some((expr, r)) => {
                    // Record per-column table formatting (title bar, trend arrow,
                    // number format, colour scale, badge, sparkline) by output
                    // position so the browser can render each.
                    if let Some(
                        rr @ (Role::Title
                        | Role::Trend
                        | Role::Metric(_)
                        | Role::Value(Kind::Sparkline)
                        | Role::ColorScale
                        | Role::Badge
                        | Role::Plain),
                    ) = parse_role(r)
                    {
                        roles.push((idx, rr));
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
        if let Some((expr, role_str)) = trailing_role(item) {
            if let Some(role) = parse_role(role_str) {
                // Cast measures/metrics to DOUBLE so sum()/BIGINT/HUGEINT come back
                // as real numbers (DuckDB-Wasm otherwise serialises HUGEINT as str).
                let item = match role {
                    Role::Value(_)
                    | Role::Metric(_)
                    | Role::Delta
                    | Role::RefLine
                    | Role::VLine
                    | Role::BandLower
                    | Role::BandUpper
                    | Role::Trend
                    | Role::Reload => {
                        // A charted measure remembers a human name (explicit
                        // trailing `AS alias`, else a bare column name) so a combo
                        // legend can read e.g. "observed"/"trend". The output alias
                        // stays `c{i}` so the render side keeps its by-position
                        // column lookup — the name travels as separate metadata.
                        let (core, alias) = trailing_alias(expr);
                        if matches!(role, Role::Value(_)) {
                            if let Some(nm) = alias.or_else(|| simple_ident(core)) {
                                names.push((i, nm));
                            }
                        }
                        format!("CAST({core} AS DOUBLE) AS c{i}")
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
    (format!("{head} {} {tail}", items.join(", ")), roles, names)
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

/// Split a trailing top-level `AS <alias>` off an expression. The alias may be a
/// bare identifier or a `"quoted"` one. A nested `AS` (as in `CAST(x AS INT)`)
/// stays put — only a top-level, trailing `AS` at paren depth 0 is peeled.
fn trailing_alias(expr: &str) -> (&str, Option<String>) {
    let trimmed = expr.trim_end();
    // The final token: a quoted identifier, or a run of identifier chars.
    let (alias_start, alias) = if let Some(body) = trimmed.strip_suffix('"') {
        match body.rfind('"') {
            Some(o) => (o, body[o + 1..].to_string()),
            None => return (expr, None),
        }
    } else {
        let cut = trimmed.trim_end_matches(|c: char| c.is_alphanumeric() || c == '_');
        if cut.len() == trimmed.len() {
            return (expr, None); // no trailing word
        }
        (cut.len(), trimmed[cut.len()..].to_string())
    };
    if alias.is_empty() {
        return (expr, None);
    }
    let before = trimmed[..alias_start].trim_end();
    if before.len() < 2 || !before[before.len() - 2..].eq_ignore_ascii_case("AS") {
        return (expr, None);
    }
    let head = &before[..before.len() - 2];
    // The `AS` must stand alone (whitespace before it) and at paren depth 0.
    if !head.is_empty() && !head.ends_with(|c: char| c.is_whitespace()) {
        return (expr, None);
    }
    let core = head.trim_end();
    let balanced = {
        let (mut d, mut s) = (0i32, false);
        for c in core.chars() {
            match c {
                '\'' => s = !s,
                '(' if !s => d += 1,
                ')' if !s => d -= 1,
                _ => {}
            }
        }
        d == 0 && !s
    };
    if core.is_empty() || !balanced {
        return (expr, None);
    }
    (core, Some(alias))
}

/// `Some(s)` if `s` is a bare SQL identifier (so a plain `col ::LINECHART` can
/// carry `col` as its series name); `None` for anything computed.
fn simple_ident(s: &str) -> Option<String> {
    let t = s.trim();
    if !t.is_empty()
        && !t.starts_with(|c: char| c.is_ascii_digit())
        && t.chars().all(|c| c.is_alphanumeric() || c == '_')
    {
        Some(t.to_string())
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
    // `to_ascii_uppercase` preserves byte length, so indices map back to `s`.
    // Iterate by char boundaries so multibyte UTF-8 (e.g. `Δ`, `±`) never panics
    // a `&up[i..]` slice.
    let up = s.to_ascii_uppercase();
    let (mut depth, mut in_str) = (0i32, false);
    for (i, c) in up.char_indices() {
        match c {
            '\'' => in_str = !in_str,
            '(' if !in_str => depth += 1,
            ')' if !in_str => depth -= 1,
            _ if !in_str && depth == 0 && up[i..].starts_with(kw) => {
                // `_` is an identifier char, so `from_day` must NOT match `FROM`.
                let ident = |c: char| c.is_alphanumeric() || c == '_';
                let before = up[..i].chars().next_back().is_none_or(|p| !ident(p));
                let after = up[i + kw.len()..].chars().next().is_none_or(|c| !ident(c));
                if before && after {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
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
