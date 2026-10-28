//! Graduated symbols (`::SIZE` on a map, M2a): a dot at each feature's
//! anchor — the point itself, or a polygon's pole of inaccessibility —
//! scaled to a measure's area, drawn largest first.

use crate::format::ValueFmt;
use crate::{cell_num, constant_size, find_role, value_fmt, Column, Kind, Role};

use super::labels::polylabel;
use super::project::Extent;
use super::Shape;

/// Graduated-symbol data for a map: each row's anchor(s) (projected), its
/// raw `::SIZE` value, the size scale and the size legend's title/format.
pub(super) struct Symbols {
    /// Per row, projected anchors (0, 1, or several for a `MultiPoint`).
    pub anchors: Vec<Vec<[f64; 2]>>,
    /// Per row, the raw `::SIZE` value when it is finite, positive and the
    /// row has an anchor (`None`: no value, zero/negative, or no anchor —
    /// none of those are drawn).
    pub size: Vec<Option<f64>>,
    /// The largest size value (for the radius formula); 0 when every row is
    /// a fixed size.
    pub vmax: f64,
    /// A constant `::SIZE`: the fixed radius in px (no size legend).
    pub fixed: Option<f64>,
    pub fmt: ValueFmt,
    /// The size legend's title (the column's name and unit).
    pub title: Option<String>,
}

/// The maximum symbol radius in px for a panel `w`×`h` (independent of
/// zoom, so the legend and the drawn symbols always agree).
pub(super) fn rmax(w: f64, h: f64) -> f64 {
    (0.04 * w.min(h)).clamp(6.0, 28.0)
}

/// Converts a shape's ring (`[f64; 2]` vertices) to `labels::Pt` tuples.
fn as_pts(ring: &[[f64; 2]]) -> Vec<(f64, f64)> {
    ring.iter().map(|c| (c[0], c[1])).collect()
}

/// The anchor(s) of one row's shapes: a point is itself (a `MultiPoint`
/// gives one per point); otherwise the largest (by bounding-box area) core
/// polygon's pole of inaccessibility; a line alone has no anchor.
pub(super) fn anchors_of(shapes: &[Shape]) -> Vec<[f64; 2]> {
    let points: Vec<[f64; 2]> = shapes
        .iter()
        .filter_map(|s| match s {
            Shape::Point(p) => Some(*p),
            _ => None,
        })
        .collect();
    if !points.is_empty() {
        return points;
    }
    let mut best: Option<(f64, &Vec<Vec<[f64; 2]>>)> = None;
    for s in shapes {
        if let Shape::Area(rings) = s {
            if let Some(outer) = rings.first() {
                let mut e = Extent::new();
                outer.iter().for_each(|c| e.add(*c));
                if !e.is_empty() {
                    let a = (e.x1 - e.x0) * (e.y1 - e.y0);
                    if best.as_ref().is_none_or(|b| a > b.0) {
                        best = Some((a, rings));
                    }
                }
            }
        }
    }
    let Some((_, rings)) = best else {
        return Vec::new();
    };
    let outer = &rings[0];
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for c in outer {
        x0 = x0.min(c[0]);
        y0 = y0.min(c[1]);
        x1 = x1.max(c[0]);
        y1 = y1.max(c[1]);
    }
    let precision = ((x1 - x0).max(y1 - y0) / 500.0).max(1e-9);
    let pts: Vec<Vec<(f64, f64)>> = rings.iter().map(|r| as_pts(r)).collect();
    match polylabel(&pts, precision) {
        Some((p, _)) => vec![[p.0, p.1]],
        None => Vec::new(),
    }
}

/// The graduated-symbol model for a map, from its `::SIZE` column and
/// projected features (`None`: no `::SIZE`, no finite value, or no row has
/// an anchor — e.g. a map of lines only).
pub(super) fn of(cols: &[Column], features: &[Vec<Shape>]) -> Option<Symbols> {
    let col = find_role(cols, Role::Size)?;
    if !col.values.iter().any(|v| cell_num(v).is_some()) {
        return None;
    }
    let fixed = constant_size(col);
    let rows = features.len();
    let anchors: Vec<Vec<[f64; 2]>> = (0..rows).map(|i| anchors_of(&features[i])).collect();
    if anchors.iter().all(Vec::is_empty) {
        return None;
    }
    let size: Vec<Option<f64>> = (0..rows)
        .map(|i| {
            if anchors[i].is_empty() {
                return None;
            }
            col.values
                .get(i)
                .and_then(cell_num)
                .filter(|v| v.is_finite() && *v > 0.0)
        })
        .collect();
    let vmax = size.iter().flatten().copied().fold(0.0_f64, f64::max);
    let fmt = value_fmt(cols, Kind::Bar, &col.values);
    let title = super::legend::legend_title(&col.name, &fmt);
    Some(Symbols {
        anchors,
        size,
        vmax,
        fixed,
        fmt,
        title,
    })
}
