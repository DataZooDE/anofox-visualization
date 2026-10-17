//! Box plots from statistics: one row per box, the five numbers computed in
//! SQL (`quantile_cont`), for populations too large to send as raw rows.
//!
//! ```sql
//! SELECT method::XAXIS,
//!        quantile_cont(err, 0.25)::Q1, median(err)::MEDIAN, quantile_cont(err, 0.75)::Q3,
//!        min(err)::WHISKER_LOW, max(err)::WHISKER_HIGH, avg(err)::MEAN
//! FROM backtest GROUP BY ALL;
//! ```
//!
//! ggplot-rs's `geom_boxplot` draws the box from its stat's columns (`lower`,
//! `middle`, `upper`, `ymin`, `ymax`, `outliers`), but it has no aesthetics
//! for `lower` / `middle` / `upper`, so there is no `stat = "identity"` input.
//! [`StatBoxStats`] fills that gap: the quartiles travel in the y-scaled
//! `open` / `close` aesthetics (`y` = the median, `ymin` / `ymax` = the
//! whiskers, `label` = the outliers) and the stat hands them to the geom under
//! its names. The box therefore looks exactly like a raw-rows `::BOXPLOT`
//! (same geom, theme, colours, outlier circles); the tooltip is rewritten to
//! name all the numbers ([`with_tips`]).

use crate::{format, Column, Kind, RenderOptions, Role};
use ggplot_rs::aes::Aesthetic;
use ggplot_rs::data::{DataFrame, Value};
use ggplot_rs::prelude::Aes;
use ggplot_rs::scale::ScaleSet;
use ggplot_rs::stat::Stat;

/// The roles of a box from statistics.
const ROLES: [Role; 7] = [
    Role::Q1,
    Role::Median,
    Role::Q3,
    Role::WhiskerLow,
    Role::WhiskerHigh,
    Role::Mean,
    Role::Outliers,
];

fn find(cols: &[Column], role: Role) -> Option<&Column> {
    cols.iter().find(|c| c.role == role)
}

/// Does the panel carry any box-statistics role?
pub(crate) fn present(cols: &[Column]) -> bool {
    cols.iter().any(|c| ROLES.contains(&c.role))
}

/// A box-statistics panel ready to draw: the `::MEDIAN` column becomes the
/// panel's `::BOXPLOT` measure (so the cartesian pipeline, its formats, flip,
/// facets and reference lines apply). An error names what is missing: `::Q1`,
/// `::MEDIAN` and `::Q3` are required, the whiskers come as a pair, and a
/// chart measure (`::BOXPLOT` on raw rows, …) cannot share the statement.
pub(crate) fn prepare(cols: Vec<Column>) -> Result<Vec<Column>, String> {
    if !present(&cols) {
        return Ok(cols);
    }
    if let Some(m) = cols.iter().find(|c| matches!(c.role, Role::Value(_))) {
        return Err(format!(
            "::{} draws from raw rows; a box from statistics (::Q1, ::MEDIAN, ::Q3) takes no chart measure",
            m.role.token()
        ));
    }
    let missing: Vec<&str> = [Role::Q1, Role::Median, Role::Q3]
        .into_iter()
        .filter(|r| find(&cols, *r).is_none())
        .map(|r| r.token())
        .collect();
    if !missing.is_empty() {
        return Err(format!(
            "a box from statistics needs ::Q1, ::MEDIAN and ::Q3 (missing ::{})",
            missing.join(", ::")
        ));
    }
    if find(&cols, Role::WhiskerLow).is_some() != find(&cols, Role::WhiskerHigh).is_some() {
        return Err("::WHISKER_LOW needs a matching ::WHISKER_HIGH".into());
    }
    if find(&cols, Role::X).is_none() {
        return Err("a box from statistics needs an ::XAXIS (one row per box)".into());
    }
    Ok(cols
        .into_iter()
        .map(|c| match c.role {
            Role::Median => Column::new(c.name, Role::Value(Kind::Boxplot), c.values),
            _ => c,
        })
        .collect())
}

/// Is this (prepared) panel a box from statistics?
pub(crate) fn is_stats(cols: &[Column]) -> bool {
    find(cols, Role::Q1).is_some()
}

/// The numbers of an `::OUTLIERS` cell: a list (`[1.5, 9]`, as DuckDB's JSON
/// prints it) or a comma-separated string.
pub(crate) fn outlier_values(v: &Value) -> Vec<f64> {
    match v {
        Value::Str(s) => s
            .trim()
            .trim_start_matches('[')
            .trim_end_matches(']')
            .split(',')
            .filter_map(|p| p.trim().trim_matches('"').parse::<f64>().ok())
            .filter(|f| f.is_finite())
            .collect(),
        v => v.as_f64().filter(|f| f.is_finite()).into_iter().collect(),
    }
}

/// The plot-data columns of the boxes (pushed beside `x` / `y` = the median):
/// `bq1`, `bq3`, `bwlo`, `bwhi` (a missing whisker stops at the box) and
/// `bout` (the outliers as `"1.5,9"`).
pub(crate) fn data_columns(cols: &[Column]) -> Vec<(String, Vec<Value>)> {
    let n = find(cols, Role::Value(Kind::Boxplot)).map_or(0, |c| c.values.len());
    let vals = |r: Role| -> Vec<Value> {
        find(cols, r).map_or_else(|| vec![Value::Na; n], |c| c.values.clone())
    };
    let (q1, q3) = (vals(Role::Q1), vals(Role::Q3));
    let or = |w: Vec<Value>, edge: &[Value]| -> Vec<Value> {
        w.into_iter()
            .zip(edge)
            .map(|(w, e)| match w.as_f64() {
                Some(f) if f.is_finite() => w,
                _ => e.clone(),
            })
            .collect()
    };
    let wlo = or(vals(Role::WhiskerLow), &q1);
    let whi = or(vals(Role::WhiskerHigh), &q3);
    let out = find(cols, Role::Outliers).map_or_else(
        || vec![Value::Na; n],
        |c| {
            c.values
                .iter()
                .map(|v| {
                    let o = outlier_values(v);
                    if o.is_empty() {
                        Value::Na
                    } else {
                        Value::Str(o.iter().map(f64::to_string).collect::<Vec<_>>().join(","))
                    }
                })
                .collect()
        },
    );
    vec![
        ("bq1".into(), q1),
        ("bq3".into(), q3),
        ("bwlo".into(), wlo),
        ("bwhi".into(), whi),
        ("bout".into(), out),
    ]
}

/// The box layer's mapping on top of the plot's `x` / `y` (see the module
/// docs).
pub(crate) fn layer_aes() -> Aes {
    Aes::new()
        .open("bq1")
        .close("bq3")
        .ymin("bwlo")
        .ymax("bwhi")
        .label("bout")
}

/// The y extent of what the boxes draw beyond their whiskers (the outliers and
/// the means), as `(x, y)` rows for a blank layer: the y scale trains on
/// `y` / `ymin` / `ymax` only.
pub(crate) fn extent_rows(cols: &[Column]) -> (Vec<Value>, Vec<Value>) {
    let (mut xs, mut ys) = (Vec::new(), Vec::new());
    let Some(x) = find(cols, Role::X) else {
        return (xs, ys);
    };
    if let Some(o) = find(cols, Role::Outliers) {
        for (xv, ov) in x.values.iter().zip(&o.values) {
            for f in outlier_values(ov) {
                xs.push(xv.clone());
                ys.push(Value::Float(f));
            }
        }
    }
    if let Some(m) = find(cols, Role::Mean) {
        for (xv, mv) in x.values.iter().zip(&m.values) {
            if let Some(f) = mv.as_f64().filter(|f| f.is_finite()) {
                xs.push(xv.clone());
                ys.push(Value::Float(f));
            }
        }
    }
    (xs, ys)
}

/// The label of a mean dot (its tooltip is removed by [`with_tips`]: the box's
/// tooltip names the mean).
pub(crate) const MEAN_LABEL: &str = "\u{2063}mean";

/// Hands one row per box to `geom_boxplot` under the columns it draws from.
pub(crate) struct StatBoxStats;

impl Stat for StatBoxStats {
    fn compute_group(&self, data: &DataFrame, _scales: &ScaleSet) -> DataFrame {
        let n = data.nrows();
        let col = |name: &str| -> Vec<Value> {
            data.column(name)
                .map_or_else(|| vec![Value::Na; n], |c| c.to_vec())
        };
        let mut out = DataFrame::new();
        for (from, to) in [
            ("x", "x"),
            ("ymin", "ymin"),
            ("open", "lower"),
            ("y", "middle"),
            ("close", "upper"),
            ("ymax", "ymax"),
        ] {
            out.add_column(to.to_string(), col(from));
        }
        if let Some(l) = data.column("label") {
            out.add_column("outliers".to_string(), l.to_vec());
        }
        for keep in ["fill", "color", "group"] {
            if let Some(c) = data.column(keep) {
                out.add_column(keep.to_string(), c.to_vec());
            }
        }
        out
    }

    fn required_aes(&self) -> Vec<Aesthetic> {
        vec![Aesthetic::X, Aesthetic::Y]
    }

    fn name(&self) -> &str {
        "boxstats"
    }
}

/// One box's numbers, for its tooltip and the lint.
struct BoxRow {
    x: String,
    q1: Option<f64>,
    median: Option<f64>,
    q3: Option<f64>,
    lo: Option<f64>,
    hi: Option<f64>,
    mean: Option<f64>,
    outliers: usize,
}

fn rows(cols: &[Column]) -> Vec<BoxRow> {
    let num = |r: Role, i: usize| -> Option<f64> {
        find(cols, r)?
            .values
            .get(i)?
            .as_f64()
            .filter(|f| f.is_finite())
    };
    let median = find(cols, Role::Median).or_else(|| find(cols, Role::Value(Kind::Boxplot)));
    let n = median.map_or(0, |c| c.values.len());
    (0..n)
        .map(|i| BoxRow {
            x: find(cols, Role::X)
                .and_then(|c| c.values.get(i))
                .map(ggplot_rs::format::format_value)
                .unwrap_or_default(),
            q1: num(Role::Q1, i),
            median: median
                .and_then(|c| c.values.get(i))
                .and_then(|v| v.as_f64()),
            q3: num(Role::Q3, i),
            lo: num(Role::WhiskerLow, i),
            hi: num(Role::WhiskerHigh, i),
            mean: num(Role::Mean, i),
            outliers: find(cols, Role::Outliers)
                .and_then(|c| c.values.get(i))
                .map_or(0, |v| outlier_values(v).len()),
        })
        .collect()
}

/// What is wrong with the numbers (`dashboard --check`), as `(code,
/// message)`: `box/unordered` — a row where `Q1 ≤ median ≤ Q3` does not hold
/// or a whisker ends inside the box — and `box/null` — a box without a
/// quartile. One message per kind, naming the first box.
pub(crate) fn issues(cols: &[Column]) -> Vec<(&'static str, String)> {
    if !present(cols) && !is_stats(cols) {
        return Vec::new();
    }
    let mut out = Vec::new();
    let all = rows(cols);
    let first = |f: &dyn Fn(&BoxRow) -> bool| all.iter().filter(|r| f(r)).collect::<Vec<_>>();
    let unordered = first(&|r| match (r.q1, r.median, r.q3) {
        (Some(a), Some(m), Some(b)) => !(a <= m && m <= b),
        _ => false,
    });
    if let Some(r) = unordered.first() {
        out.push((
            "box/unordered",
            format!(
            "box `{}`: Q1 ≤ median ≤ Q3 does not hold ({} / {} / {}){} — the box is drawn as given",
            r.x,
            num_text(r.q1),
            num_text(r.median),
            num_text(r.q3),
            more(unordered.len())
        ),
        ));
    }
    let inside = first(&|r| {
        matches!((r.lo, r.q1), (Some(w), Some(q)) if w > q)
            || matches!((r.hi, r.q3), (Some(w), Some(q)) if w < q)
    });
    if let Some(r) = inside.first() {
        out.push((
            "box/unordered",
            format!(
                "box `{}`: a whisker ends inside the box (whiskers {} – {}, box {} – {}){}",
                r.x,
                num_text(r.lo),
                num_text(r.hi),
                num_text(r.q1),
                num_text(r.q3),
                more(inside.len())
            ),
        ));
    }
    let gaps = first(&|r| r.q1.is_none() || r.median.is_none() || r.q3.is_none());
    if let Some(r) = gaps.first() {
        out.push((
            "box/null",
            format!(
                "box `{}` has no Q1, median or Q3 (NULL) — it is not drawn{}",
                r.x,
                more(gaps.len())
            ),
        ));
    }
    out
}

fn num_text(v: Option<f64>) -> String {
    v.map_or_else(|| "NULL".into(), |f| format!("{f}"))
}

fn more(n: usize) -> String {
    if n > 1 {
        format!(" (and {} more)", n - 1)
    } else {
        String::new()
    }
}

/// Give each box the tooltip that names its numbers — `A: median 4.1 · Q1 3
/// · Q3 5.2 · whiskers 1 – 8 · mean 4.4 · 2 outliers`, in the panel's value
/// format — in place of the engine's `A: median 4.1 (3–5.2)`; and drop the
/// mean dots' own tooltips (the box names the mean).
pub(crate) fn with_tips(mut svg: String, cols: &[Column], o: &RenderOptions) -> String {
    let all = rows(cols);
    let median = find(cols, Role::Value(Kind::Boxplot));
    let fmt = crate::value_fmt(cols, Kind::Boxplot, median.map_or(&[][..], |c| &c.values));
    let f = |v: f64| fmt.full(v, &o.locale);
    let r = |v: f64| (v * 100.0).round() / 100.0;
    for b in &all {
        let (Some(m), Some(q1), Some(q3)) = (b.median, b.q1, b.q3) else {
            continue;
        };
        let engine = format!("median {} ({}–{})", r(m), r(q1), r(q3));
        let engine = if b.x.is_empty() {
            engine
        } else {
            format!("{}: {engine}", b.x)
        };
        let mut tip = format!("median {} · Q1 {} · Q3 {}", f(m), f(q1), f(q3));
        if let (Some(lo), Some(hi)) = (b.lo, b.hi) {
            tip.push_str(&format!(" · whiskers {} – {}", f(lo), f(hi)));
        }
        if let Some(mean) = b.mean {
            tip.push_str(&format!(" · mean {}", f(mean)));
        }
        match b.outliers {
            0 => {}
            1 => tip.push_str(" · 1 outlier"),
            k => tip.push_str(&format!(" · {k} outliers")),
        }
        if !b.x.is_empty() {
            tip = format!("{}: {tip}", b.x);
        }
        // (The engine titles every part of a box: body, median, whiskers.)
        svg = svg.replace(
            &format!("<title>{}</title>", format::escape_xml(&engine)),
            &format!("<title>{}</title>", format::escape_xml(&tip)),
        );
    }
    strip_mean_tips(svg)
}

/// A mean dot is drawn as `<circle … data-…><title>⁣mean: 4.4</title></circle>`:
/// keep the circle, drop its tooltip and mark data (it is no data point of
/// its own).
fn strip_mean_tips(svg: String) -> String {
    let marker = format!("<title>{}: ", format::escape_xml(MEAN_LABEL));
    if !svg.contains(&marker) {
        return svg;
    }
    let mut out = String::with_capacity(svg.len());
    let mut rest = svg.as_str();
    while let Some(t) = rest.find(&marker) {
        let Some(open) = rest[..t].rfind('<') else {
            break;
        };
        let Some(end) = rest[t..].find("</title>").map(|e| t + e + "</title>".len()) else {
            break;
        };
        let tag = &rest[open..t]; // `<circle … data-x="…">`
        let name: String = tag[1..]
            .chars()
            .take_while(|c| c.is_ascii_alphabetic())
            .collect();
        let close = format!("</{name}>");
        if !rest[end..].starts_with(&close) {
            out.push_str(&rest[..end]);
            rest = &rest[end..];
            continue;
        }
        out.push_str(&rest[..open]);
        let attrs = tag.trim_end_matches('>');
        let kept: String = strip_data_attrs(attrs);
        out.push_str(&kept);
        out.push_str("/>");
        rest = &rest[end + close.len()..];
    }
    out.push_str(rest);
    out
}

/// `<circle cx="1" data-x="A" data-value="2"` without its `data-*` attributes.
fn strip_data_attrs(tag: &str) -> String {
    let mut out = String::with_capacity(tag.len());
    let mut rest = tag;
    while let Some(i) = rest.find(" data-") {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        let Some(q1) = after.find('"') else {
            rest = "";
            break;
        };
        let Some(q2) = after[q1 + 1..].find('"') else {
            rest = "";
            break;
        };
        rest = &after[q1 + 1 + q2 + 1..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn col(role: Role, v: &[f64]) -> Column {
        Column::new("", role, v.iter().map(|f| Value::Float(*f)).collect())
    }
    fn x(v: &[&str]) -> Column {
        Column::new(
            "",
            Role::X,
            v.iter().map(|s| Value::Str(s.to_string())).collect(),
        )
    }

    #[test]
    fn prepare_requires_the_quartiles() {
        let ok = vec![
            x(&["a"]),
            col(Role::Q1, &[1.0]),
            col(Role::Median, &[2.0]),
            col(Role::Q3, &[3.0]),
        ];
        let Ok(p) = prepare(ok.clone()) else {
            panic!("prepared")
        };
        assert_eq!(p[2].role, Role::Value(Kind::Boxplot));
        let err = |cols: Vec<Column>| prepare(cols).err().expect("an error");
        assert!(err(ok[..3].to_vec()).contains("missing ::Q3"));
        let mut raw = ok.clone();
        raw.push(col(Role::Value(Kind::Boxplot), &[2.0]));
        assert!(err(raw).contains("takes no chart measure"));
        let mut half = ok.clone();
        half.push(col(Role::WhiskerLow, &[0.0]));
        assert!(err(half).contains("WHISKER_HIGH"));
        assert!(err(ok[1..].to_vec()).contains("XAXIS"));
        let plain = vec![x(&["a"]), col(Role::Value(Kind::Bar), &[1.0])];
        assert_eq!(prepare(plain).ok().map(|c| c.len()), Some(2));
    }

    #[test]
    fn outliers_parse_from_lists_and_strings() {
        assert_eq!(
            outlier_values(&Value::Str("[1.5, -2, 9e1]".into())),
            vec![1.5, -2.0, 90.0]
        );
        assert_eq!(outlier_values(&Value::Str("1,2".into())), vec![1.0, 2.0]);
        assert!(outlier_values(&Value::Str("[]".into())).is_empty());
        assert!(outlier_values(&Value::Na).is_empty());
        assert_eq!(outlier_values(&Value::Float(3.0)), vec![3.0]);
    }

    #[test]
    fn issues_name_unordered_numbers() {
        let mut cols = vec![
            x(&["a", "b", "c"]),
            col(Role::Q1, &[1.0, 3.0, 1.0]),
            col(Role::Median, &[2.0, 2.0, 2.0]),
            col(Role::Q3, &[3.0, 4.0, 3.0]),
            col(Role::WhiskerLow, &[0.0, 0.0, 1.5]),
            col(Role::WhiskerHigh, &[5.0, 5.0, 5.0]),
        ];
        let i = issues(&cols);
        assert_eq!(i.len(), 2, "{i:?}");
        assert!(
            i[0].1.contains("box `b`") && i[0].1.contains("Q1 ≤ median ≤ Q3"),
            "{i:?}"
        );
        assert!(
            i[1].1.contains("box `c`") && i[1].1.contains("whisker"),
            "{i:?}"
        );
        assert!(i.iter().all(|(c, _)| *c == "box/unordered"));
        cols[1] = col(Role::Q1, &[1.0, 1.0, 1.0]);
        cols[4] = col(Role::WhiskerLow, &[0.0, 0.0, 0.0]);
        assert!(issues(&cols).is_empty());
    }

    #[test]
    fn mean_tips_are_removed() {
        let svg = format!(
            "<svg><circle cx=\"1\" data-x=\"a\" data-value=\"2\"><title>{}: 2</title></circle><rect/></svg>",
            MEAN_LABEL
        );
        assert_eq!(strip_mean_tips(svg), "<svg><circle cx=\"1\"/><rect/></svg>");
    }
}
