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
//! ggplot-rs 0.22 gained `lower`/`middle`/`upper`/`outliers` aesthetics and a
//! `stat = "identity"` boxplot input (ggplot-rs#318 B3-5), so a box from
//! statistics is now `geom_boxplot_with(..).stat(StatIdentity)` mapped
//! straight onto the five-number columns [`data_columns`] computes — no more
//! local `Stat` impl standing in for one. 0.22 also consumes a mapped
//! `tooltip` (and widens the y scale to the outlier extent itself), so the
//! box's hover text is a plain data column ([`data_columns`]'s `btip`,
//! [`layer_aes`]) rather than a post-render `<title>` rewrite.

use crate::{Column, Kind, RenderOptions, Role};
use ggplot_rs::data::Value;
use ggplot_rs::prelude::Aes;

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
/// `bq1`, `bq3`, `bwlo`, `bwhi` (a missing whisker stops at the box), `bout`
/// (the outliers as `"1.5,9"`) and `btip` (the box's full tooltip text,
/// formatted in the panel's locale — the old `with_tips` string, now built
/// as data instead of a post-render `<title>` rewrite).
pub(crate) fn data_columns(cols: &[Column], o: &RenderOptions) -> Vec<(String, Vec<Value>)> {
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
    let btip = tip_text(cols, o);
    vec![
        ("bq1".into(), q1),
        ("bq3".into(), q3),
        ("bwlo".into(), wlo),
        ("bwhi".into(), whi),
        ("bout".into(), out),
        ("btip".into(), btip),
    ]
}

/// The box layer's mapping under `stat = "identity"` (ggplot-rs 0.22's
/// `lower`/`middle`/`upper`/`ymin`/`ymax`/`outliers`/tooltip): `middle` is
/// the plot's own `y` column (the median, already plotted there), the rest
/// are [`data_columns`]'s.
pub(crate) fn layer_aes() -> Aes {
    Aes::new()
        .x("x")
        .lower("bq1")
        .middle("y")
        .upper("bq3")
        .ymin("bwlo")
        .ymax("bwhi")
        .outliers("bout")
        .tooltip("btip")
}

/// Every box's full tooltip text — `A: median 4.1 · Q1 3 · Q3 5.2 ·
/// whiskers 1 – 8 · mean 4.4 · 2 outliers`, in the panel's value format —
/// as a plain data column, [`Value::Na`] where the box has no median/Q1/Q3
/// (ggplot-rs drops the mapped tooltip for an `Na` cell, same as any other
/// mapped aesthetic).
fn tip_text(cols: &[Column], o: &RenderOptions) -> Vec<Value> {
    let median = find(cols, Role::Value(Kind::Boxplot));
    let fmt = crate::value_fmt(cols, Kind::Boxplot, median.map_or(&[][..], |c| &c.values));
    let f = |v: f64| fmt.full(v, &o.locale);
    rows(cols)
        .iter()
        .map(|b| {
            let (Some(m), Some(q1), Some(q3)) = (b.median, b.q1, b.q3) else {
                return Value::Na;
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
            Value::Str(tip)
        })
        .collect()
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
    fn btip_names_every_number_as_data_not_a_rendered_title() {
        // `btip` is now built once as a plain data column (ggplot-rs 0.22
        // consumes it as a mapped tooltip), not a post-render <title> scan
        // over the SVG — so this just checks the column, not rendered text.
        let cols = vec![
            x(&["a", "b"]),
            col(Role::Q1, &[1.0, 3.0]),
            col(Role::Value(Kind::Boxplot), &[2.0, 3.5]),
            col(Role::Q3, &[3.0, 4.0]),
            col(Role::WhiskerLow, &[0.0, 2.0]),
            col(Role::WhiskerHigh, &[5.0, 6.0]),
            col(Role::Mean, &[2.2, 3.6]),
            Column::new(
                "",
                Role::Outliers,
                vec![Value::Str("7, 9".into()), Value::Na],
            ),
        ];
        let btip = tip_text(&cols, &RenderOptions::default());
        assert_eq!(btip.len(), 2);
        let Value::Str(t0) = &btip[0] else {
            panic!("{:?}", btip[0])
        };
        assert_eq!(
            t0,
            "a: median 2 · Q1 1 · Q3 3 · whiskers 0 – 5 · mean 2.2 · 2 outliers"
        );
        let Value::Str(t1) = &btip[1] else {
            panic!("{:?}", btip[1])
        };
        assert_eq!(
            t1,
            "b: median 3.5 · Q1 3 · Q3 4 · whiskers 2 – 6 · mean 3.6"
        );
    }

    #[test]
    fn a_box_with_no_quartile_gets_no_tooltip() {
        // `median` (post-`prepare`, the ::MEDIAN column renamed to a chart
        // measure) is present, but ::Q1/::Q3 are not.
        let cols = vec![x(&["a"]), col(Role::Value(Kind::Boxplot), &[2.0])];
        assert_eq!(tip_text(&cols, &RenderOptions::default()), vec![Value::Na]);
    }
}
