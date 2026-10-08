//! Input preparation run by [`crate::render_with`] before any chart is drawn:
//!
//! 1. **Shape**: pad every column to the longest one with missing values, so
//!    row-indexed renderers can never index out of bounds.
//! 2. **Sanitise**: non-finite floats (NaN/±inf) become missing; control
//!    characters are dropped from strings; `"` becomes `”` (the plotting engine
//!    writes some strings into SVG attributes without escaping quotes — this
//!    keeps a crafted category from breaking out of an attribute).
//! 3. **Category cap**: a discrete x axis (bars, box/violin/jitter) or a
//!    `::CATEGORY` with more than [`RenderOptions::max_categories`] levels keeps
//!    the largest `max − 1` (by total |measure|, or row count for raw-row
//!    charts) and folds the rest into an `Other` level; aggregating charts
//!    (bars, lines, areas, pie/donut) sum the folded rows.
//! 4. **LTTB**: a line/area/step series with more than
//!    [`RenderOptions::max_line_points`] points is downsampled with
//!    Largest-Triangle-Three-Buckets (visual shape preserved, O(n)).

use crate::{Column, Kind, RenderOptions, Role};
use ggplot_rs::prelude::Value;
use std::collections::HashMap;

/// The label folded categories receive.
pub const OTHER: &str = "Other";

pub(crate) fn prepare(cols: &[Column], o: &RenderOptions) -> Vec<Column> {
    let n = cols.iter().map(|c| c.values.len()).max().unwrap_or(0);
    let mut out: Vec<Column> = cols
        .iter()
        .map(|c| {
            let mut values: Vec<Value> = c.values.iter().map(sanitize).collect();
            values.resize(n, Value::Na);
            Column::new(c.name.clone(), c.role, values)
        })
        .collect();
    let kind = out.iter().find_map(|c| match c.role {
        Role::Value(k) => Some(k),
        _ => None,
    });
    let Some(kind) = kind else {
        return out;
    };
    if o.max_categories >= 2 && !out.iter().any(|c| c.role == Role::Geometry) {
        out = cap_categories(out, kind, o.max_categories);
    }
    if o.max_line_points >= 3 {
        out = lttb_lines(out, kind, o.max_line_points);
    }
    out
}

fn sanitize(v: &Value) -> Value {
    match v {
        Value::Float(f) if !f.is_finite() => Value::Na,
        Value::Str(s) => {
            if s.chars()
                .any(|c| c == '"' || (c.is_control() && !matches!(c, '\t' | '\n' | '\r')))
            {
                Value::Str(
                    s.chars()
                        .filter(|c| !c.is_control() || matches!(c, '\t' | '\n' | '\r'))
                        .map(|c| if c == '"' { '”' } else { c })
                        .collect(),
                )
            } else {
                v.clone()
            }
        }
        other => other.clone(),
    }
}

/// Roles whose values are one-per-row data (filtered/aggregated together).
/// Everything else (title, formats, ref lines…) is scalar-ish and left as is.
fn row_aligned(r: Role) -> bool {
    matches!(
        r,
        Role::X
            | Role::Y
            | Role::Category
            | Role::Value(_)
            | Role::BandLower
            | Role::BandUpper
            | Role::Size
            | Role::Open
            | Role::High
            | Role::Low
    )
}

/// An exact grouping key for a cell (floats by bit pattern, so distinct x
/// values never merge).
fn key(v: &Value) -> String {
    match v {
        Value::Str(s) => format!("s{s}"),
        Value::Float(f) => format!("f{}", f.to_bits()),
        Value::Integer(i) => format!("i{i}"),
        Value::DateTime(t) => format!("t{t}"),
        Value::Bool(b) => format!("b{b}"),
        Value::Na => "n".into(),
    }
}

fn cap_categories(mut cols: Vec<Column>, kind: Kind, max: usize) -> Vec<Column> {
    let aggregating = matches!(
        kind,
        Kind::Bar
            | Kind::BarStacked
            | Kind::BarPercent
            | Kind::BarStackedPercent
            | Kind::Line
            | Kind::LinePercent
            | Kind::Step
            | Kind::Area
            | Kind::AreaStacked
            | Kind::Pie
            | Kind::Donut
    );
    let raw_rows = matches!(
        kind,
        Kind::Point | Kind::Jitter | Kind::Boxplot | Kind::Violin | Kind::Density | Kind::Smooth
    );
    if !aggregating && !raw_rows {
        return cols; // heatmap, radar, gauge, … keep their levels
    }
    let x_capped = matches!(
        kind,
        Kind::Bar
            | Kind::BarStacked
            | Kind::BarPercent
            | Kind::BarStackedPercent
            | Kind::Boxplot
            | Kind::Violin
            | Kind::Jitter
    );
    let measure: Vec<f64> = cols
        .iter()
        .find(|c| matches!(c.role, Role::Value(_)))
        .map(|c| {
            c.values
                .iter()
                .map(|v| v.as_f64().unwrap_or(0.0).abs())
                .collect()
        })
        .unwrap_or_default();

    let mut changed = false;
    for target in [Role::Category, Role::X] {
        if target == Role::X && !x_capped {
            continue;
        }
        let Some(ci) = cols.iter().position(|c| c.role == target) else {
            continue;
        };
        // Only discrete (string) columns are capped.
        if !cols[ci].values.iter().any(|v| matches!(v, Value::Str(_))) {
            continue;
        }
        let mut weight: HashMap<String, f64> = HashMap::new();
        for (i, v) in cols[ci].values.iter().enumerate() {
            let w = if raw_rows {
                1.0
            } else {
                measure.get(i).copied().unwrap_or(0.0)
            };
            *weight.entry(key(v)).or_insert(0.0) += w;
        }
        if weight.len() <= max {
            continue;
        }
        let mut ranked: Vec<(String, f64)> = weight.into_iter().collect();
        ranked.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.0.cmp(&b.0))
        });
        let keep: std::collections::HashSet<String> =
            ranked.into_iter().take(max - 1).map(|(k, _)| k).collect();
        for v in cols[ci].values.iter_mut() {
            if !keep.contains(&key(v)) {
                *v = Value::Str(OTHER.into());
            }
        }
        changed = true;
    }
    if changed && aggregating {
        cols = aggregate(cols);
    }
    cols
}

/// Group row-aligned columns by (x, category), summing measures; other
/// row-aligned columns keep the group's first value. First-seen order.
fn aggregate(cols: Vec<Column>) -> Vec<Column> {
    let n = cols.iter().map(|c| c.values.len()).max().unwrap_or(0);
    let xi = cols.iter().position(|c| c.role == Role::X);
    let ci = cols.iter().position(|c| c.role == Role::Category);
    let mut group_of: HashMap<String, usize> = HashMap::new();
    let mut groups: Vec<Vec<usize>> = Vec::new();
    for r in 0..n {
        let mut k = String::new();
        for idx in [xi, ci].into_iter().flatten() {
            k.push_str(&key(&cols[idx].values[r]));
            k.push('\u{1}');
        }
        let g = *group_of.entry(k).or_insert_with(|| {
            groups.push(Vec::new());
            groups.len() - 1
        });
        groups[g].push(r);
    }
    cols.into_iter()
        .map(|c| {
            if !row_aligned(c.role) {
                return c;
            }
            let values = groups
                .iter()
                .map(|rows| {
                    if matches!(c.role, Role::Value(_)) {
                        let nums: Vec<f64> =
                            rows.iter().filter_map(|&r| c.values[r].as_f64()).collect();
                        if nums.is_empty() {
                            Value::Na
                        } else {
                            Value::Float(nums.iter().sum())
                        }
                    } else {
                        c.values[rows[0]].clone()
                    }
                })
                .collect();
            Column::new(c.name, c.role, values)
        })
        .collect()
}

fn lttb_lines(cols: Vec<Column>, kind: Kind, max: usize) -> Vec<Column> {
    if !matches!(
        kind,
        Kind::Line | Kind::LinePercent | Kind::Area | Kind::Step
    ) {
        return cols;
    }
    let Some(xi) = cols.iter().position(|c| c.role == Role::X) else {
        return cols;
    };
    let Some(vi) = cols.iter().position(|c| matches!(c.role, Role::Value(_))) else {
        return cols;
    };
    let n = cols[vi].values.len();
    if n <= max || cols[xi].values.iter().any(|v| matches!(v, Value::Str(_))) {
        return cols; // short, or a discrete x (order is meaningful as given)
    }
    // Series = CATEGORY levels (or one series).
    let ci = cols.iter().position(|c| c.role == Role::Category);
    let mut series: HashMap<String, Vec<usize>> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    for r in 0..n {
        let k = ci.map(|c| key(&cols[c].values[r])).unwrap_or_default();
        series
            .entry(k.clone())
            .or_insert_with(|| {
                order.push(k);
                Vec::new()
            })
            .push(r);
    }
    let mut keep: Vec<usize> = Vec::with_capacity(max * order.len().min(64));
    for k in &order {
        let rows = &series[k];
        let mut pts: Vec<(usize, f64, f64)> = rows
            .iter()
            .filter_map(|&r| {
                Some((
                    r,
                    cols[xi].values[r].as_f64()?,
                    cols[vi].values[r].as_f64()?,
                ))
            })
            .collect();
        if pts.len() <= max {
            keep.extend(pts.iter().map(|p| p.0));
            continue;
        }
        pts.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        keep.extend(lttb(&pts, max));
    }
    keep.sort_unstable();
    cols.into_iter()
        .map(|c| {
            if !row_aligned(c.role) {
                return c;
            }
            let values = keep.iter().map(|&r| c.values[r].clone()).collect();
            Column::new(c.name, c.role, values)
        })
        .collect()
}

/// Largest-Triangle-Three-Buckets over x-sorted `(row, x, y)` points; returns
/// the `threshold` selected row indices (first and last always kept).
pub(crate) fn lttb(pts: &[(usize, f64, f64)], threshold: usize) -> Vec<usize> {
    let n = pts.len();
    if threshold >= n || threshold < 3 {
        return pts.iter().map(|p| p.0).collect();
    }
    let mut out = Vec::with_capacity(threshold);
    out.push(pts[0].0);
    let every = (n - 2) as f64 / (threshold - 2) as f64;
    let mut a = 0usize;
    for i in 0..threshold - 2 {
        let start = ((i as f64 * every) as usize + 1).min(n - 1);
        let end = (((i + 1) as f64 * every) as usize + 1).min(n - 1);
        let nstart = end;
        let nend = (((i + 2) as f64 * every) as usize + 1).min(n);
        let (mut ax, mut ay, mut cnt) = (0.0, 0.0, 0.0);
        for p in &pts[nstart..nend.max(nstart + 1).min(n)] {
            ax += p.1;
            ay += p.2;
            cnt += 1.0;
        }
        if cnt > 0.0 {
            ax /= cnt;
            ay /= cnt;
        }
        let (px, py) = (pts[a].1, pts[a].2);
        let mut best = start;
        let mut best_area = -1.0;
        for (j, p) in pts.iter().enumerate().take(end.max(start + 1)).skip(start) {
            let area = ((px - ax) * (p.2 - py) - (px - p.1) * (ay - py)).abs();
            if area > best_area {
                best_area = area;
                best = j;
            }
        }
        out.push(pts[best].0);
        a = best;
    }
    out.push(pts[n - 1].0);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lttb_keeps_endpoints_and_count() {
        let pts: Vec<(usize, f64, f64)> = (0..10_000)
            .map(|i| (i, i as f64, (i as f64 / 50.0).sin()))
            .collect();
        let k = lttb(&pts, 500);
        assert_eq!(k.len(), 500);
        assert_eq!(k[0], 0);
        assert_eq!(*k.last().unwrap(), 9_999);
        assert!(k.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn categories_fold_into_other() {
        let n = 200;
        let cols = vec![
            Column::new(
                "x",
                Role::X,
                (0..n).map(|i| Value::Str(format!("c{i}"))).collect(),
            ),
            Column::new(
                "v",
                Role::Value(Kind::Bar),
                (0..n).map(|i| Value::Float(i as f64)).collect(),
            ),
        ];
        let o = RenderOptions {
            max_categories: 10,
            ..Default::default()
        };
        let out = prepare(&cols, &o);
        let x = &out[0].values;
        assert_eq!(x.len(), 10, "9 kept + Other");
        assert!(x.contains(&Value::Str(OTHER.into())));
        let total: f64 = out[1].values.iter().filter_map(|v| v.as_f64()).sum();
        assert_eq!(
            total,
            (0..n).sum::<usize>() as f64,
            "folded rows are summed"
        );
    }
}
