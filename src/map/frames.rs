//! `::FRAME`: time maps (M2e-A, #149). One result holds every frame (a week,
//! a date); a "frame view" ([`view`]) reduces it to one row per feature,
//! with that frame's values, so every other map kind renders it unchanged
//! (D20). Scales are shared across every frame ([`Shared`]/[`apply`]) so
//! colours, classes and sizes never change meaning when stepping through
//! them (D21) — a frame without a row for a feature draws it as no data
//! (areas) or not at all (points/symbols), it is never a jump in the
//! legend.
//!
//! Plan: `~/projects/duckdb/plans/plan-maps-m2ef.md` §M2e-A, decisions
//! D11-D22.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use crate::{cell_num, find_role, value_str, Column, RenderOptions, Role, Value};

use super::{category_fill, measure_of, value_scale, Fill, Model};

/// Above this many distinct `::FRAME` values, the last this-many are kept
/// (D15); the renderer warns.
pub(super) const MAX_FRAMES: usize = 200;
/// Above features × frames, the last frames that fit are kept (D15).
const MAX_CELLS: usize = 100_000;

/// A time map's frames (D11/D13/D20/D22): the distinct `::FRAME` keys, in
/// order, and — per distinct feature, in first-appearance order — which
/// result row (if any) supplies each frame's values.
pub(super) struct Frames {
    /// The `::FRAME` column's own display name (for the subtitle, D19).
    pub name: String,
    /// Frame labels, in frame order (D11).
    pub labels: Vec<String>,
    /// Per distinct feature (in first-appearance order), its D22 key.
    pub keys: Vec<String>,
    /// Per distinct feature, the result row its geometry/region/lat-lon
    /// comes from (its first row).
    pub first_row: Vec<usize>,
    /// `[frame][feature] -> result row`, `None` when the feature has no row
    /// in that frame (D20).
    pub row_in: Vec<Vec<Option<usize>>>,
    /// Rows that repeated an already-seen (feature, frame) pair (D13).
    pub duplicates: usize,
    /// How many distinct `::FRAME` values there were before the D15 cap.
    pub total_frames: usize,
}

impl Frames {
    /// The frame to show: `want` (clamped to the last), else the last
    /// (D12/D19).
    pub fn resolve_k(&self, want: Option<usize>) -> usize {
        let last = self.labels.len().saturating_sub(1);
        want.map(|k| k.min(last)).unwrap_or(last)
    }
}

/// A stable hash of a WKT string, hex-encoded (D22) — short enough for a
/// `data-x` attribute, unlike the WKT text itself.
fn hash_wkt(s: &str) -> String {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    format!("{:x}", h.finish())
}

/// A row's D22 feature key: a normalised `::REGION` code, a `"{lat:.5},
/// {lon:.5}"` pair, or a WKT geometry's hash — whichever column this map
/// uses. `None` when the row has none (dropped from every frame).
fn row_key(cols: &[Column], i: usize) -> Option<String> {
    if let Some(region) = find_role(cols, Role::Region) {
        let s = region.values.get(i).map(value_str).unwrap_or_default();
        let s = s.trim();
        return (!s.is_empty()).then(|| s.to_ascii_uppercase());
    }
    if let (Some(lat), Some(lon)) = (find_role(cols, Role::Lat), find_role(cols, Role::Lon)) {
        let a = lat.values.get(i).and_then(cell_num);
        let b = lon.values.get(i).and_then(cell_num);
        return match (a, b) {
            (Some(a), Some(b)) => Some(format!("{a:.5},{b:.5}")),
            _ => None,
        };
    }
    if let Some(geom) = find_role(cols, Role::Geometry) {
        let s = geom.values.get(i).map(value_str).unwrap_or_default();
        return (!s.trim().is_empty()).then(|| hash_wkt(s.trim()));
    }
    None
}

/// Build [`Frames`] from a map's columns — `None` without a `::FRAME`
/// column, or when every `::FRAME` value is empty/no feature key resolves.
pub(super) fn of(cols: &[Column]) -> Option<Frames> {
    let fcol = find_role(cols, Role::Frame)?;
    let n = fcol.values.len();

    // Frame order (D11): first appearance, re-sorted ascending when every
    // label parses as a number, or chronologically when every label looks
    // like an ISO-ish date (`YYYY-MM-DD` text already sorts that way).
    let mut order: Vec<String> = Vec::new();
    let mut seen: HashMap<String, ()> = HashMap::new();
    for i in 0..n {
        let v = fcol.values.get(i).map(value_str).unwrap_or_default();
        if v.is_empty() {
            continue;
        }
        if seen.insert(v.clone(), ()).is_none() {
            order.push(v);
        }
    }
    if order.is_empty() {
        return None;
    }
    let total_frames = order.len();
    if order.iter().all(|l| l.parse::<f64>().is_ok()) {
        order.sort_by(|a, b| {
            a.parse::<f64>()
                .unwrap()
                .total_cmp(&b.parse::<f64>().unwrap())
        });
    } else if order.iter().all(|l| {
        let b = l.as_bytes();
        b.len() >= 8 && b[0].is_ascii_digit() && l.contains('-')
    }) {
        order.sort();
    }
    // D15 cap: keep the last `MAX_FRAMES`.
    if order.len() > MAX_FRAMES {
        order.drain(0..order.len() - MAX_FRAMES);
    }
    let labels = order;
    let label_index: HashMap<&str, usize> = labels
        .iter()
        .enumerate()
        .map(|(i, l)| (l.as_str(), i))
        .collect();

    // Distinct features, first-appearance order (D22).
    let mut keys: Vec<String> = Vec::new();
    let mut first_row: Vec<usize> = Vec::new();
    let mut key_index: HashMap<String, usize> = HashMap::new();
    let mut feature_of_row: Vec<Option<usize>> = vec![None; n];
    // `i` indexes both `cols` (via `row_key`) and `feature_of_row` — not a
    // plain `feature_of_row.iter_mut().enumerate()` candidate.
    #[allow(clippy::needless_range_loop)]
    for i in 0..n {
        let Some(k) = row_key(cols, i) else { continue };
        let fi = *key_index.entry(k.clone()).or_insert_with(|| {
            keys.push(k.clone());
            first_row.push(i);
            keys.len() - 1
        });
        feature_of_row[i] = Some(fi);
    }
    if first_row.is_empty() {
        return None;
    }

    let mut row_in: Vec<Vec<Option<usize>>> = vec![vec![None; first_row.len()]; labels.len()];
    let mut duplicates = 0usize;
    // Same: `i` also looks up `fcol.values`, not just `feature_of_row`.
    #[allow(clippy::needless_range_loop)]
    for i in 0..n {
        let Some(fi) = feature_of_row[i] else {
            continue;
        };
        let v = fcol.values.get(i).map(value_str).unwrap_or_default();
        let Some(&k) = label_index.get(v.as_str()) else {
            continue; // dropped by the D15 cap
        };
        if row_in[k][fi].is_some() {
            duplicates += 1;
        } else {
            row_in[k][fi] = Some(i);
        }
    }
    // D15 feature x frame cell cap: keep the last frames that fit.
    if labels.len() * first_row.len() > MAX_CELLS && !first_row.is_empty() {
        let max_frames = (MAX_CELLS / first_row.len()).max(1);
        if max_frames < labels.len() {
            let drop = labels.len() - max_frames;
            let labels2 = labels[drop..].to_vec();
            let row_in2 = row_in[drop..].to_vec();
            return Some(Frames {
                name: fcol.name.clone(),
                labels: labels2,
                keys,
                first_row,
                row_in: row_in2,
                duplicates,
                total_frames,
            });
        }
    }
    Some(Frames {
        name: fcol.name.clone(),
        labels,
        keys,
        first_row,
        row_in,
        duplicates,
        total_frames,
    })
}

/// One result row per distinct feature, with that feature's geometry/
/// region/lat-lon (its first row) and every other column's value from
/// frame `k`'s row — `Value::Na` when the feature has none (D20).
pub(super) fn view(cols: &[Column], fr: &Frames, k: usize) -> Vec<Column> {
    let is_geometry_role =
        |r: Role| matches!(r, Role::Region | Role::Geometry | Role::Lat | Role::Lon);
    cols.iter()
        .map(|c| {
            let values: Vec<Value> = (0..fr.first_row.len())
                .map(|f| {
                    let row = if is_geometry_role(c.role) {
                        Some(fr.first_row[f])
                    } else {
                        fr.row_in[k][f]
                    };
                    row.and_then(|r| c.values.get(r).cloned())
                        .unwrap_or(Value::Na)
                })
                .collect();
            Column::new(c.name.clone(), c.role, values)
        })
        .collect()
}

/// `k`'s per-feature source row — the row that actually supplied the
/// feature's frame-`k` values, else its first row (for `::LINK`/`data-row`,
/// D22's own "the key's first row when absent").
pub(super) fn result_rows(fr: &Frames, k: usize) -> Vec<usize> {
    (0..fr.first_row.len())
        .map(|f| fr.row_in[k][f].unwrap_or(fr.first_row[f]))
        .collect()
}

/// What `fr`/`k` tells the rendered root `<svg>` and its subtitle (D19).
pub(super) struct FrameCtx {
    pub k: usize,
    pub name: String,
    pub labels: Vec<String>,
    pub keys: Vec<String>,
    pub result_row: Vec<usize>,
}

impl FrameCtx {
    pub(super) fn at(fr: &Frames, k: usize) -> FrameCtx {
        FrameCtx {
            k,
            name: fr.name.clone(),
            labels: fr.labels.clone(),
            keys: fr.keys.clone(),
            result_row: result_rows(fr, k),
        }
    }
}

/// The scales shared across every frame (D21): the measure's domain and
/// classes, or the category's levels — computed once from every row of the
/// *original*, unreduced result — plus `::SIZE`'s shared domain and the
/// classed legend's widest per-class count (for `count_reserve`, U3).
pub(super) struct Shared {
    pub fill: Option<Fill>,
    pub size_limits: Option<(f64, f64)>,
    pub max_count: usize,
}

/// Build [`Shared`] from every row of `cols` (not a frame view) and `fr`'s
/// own per-frame row index (for `max_count`).
pub(super) fn shared_of(o: &RenderOptions, cols: &[Column], fr: &Frames) -> Shared {
    let n = cols.first().map(|c| c.values.len()).unwrap_or(0);
    let fill = if let Some(cat) = find_role(cols, Role::Category) {
        Some(category_fill(o, cols, cat, n))
    } else {
        measure_of(cols, find_role(cols, Role::Geometry))
            .map(|m| Fill::Measure(Box::new(value_scale(o, cols, &m, false, n))))
    };
    let size_limits = find_role(cols, Role::Size).map(|c| {
        let hi = (0..n)
            .filter_map(|i| c.values.get(i).and_then(cell_num))
            .filter(|v| v.is_finite() && *v > 0.0)
            .fold(0.0_f64, f64::max);
        (0.0, hi)
    });
    let max_count = match &fill {
        Some(Fill::Measure(scale)) if scale.classes.is_some() => max_class_count(cols, fr, scale),
        _ => 0,
    };
    Shared {
        fill,
        size_limits,
        max_count,
    }
}

/// The widest per-class feature count over every frame (U3: a frame map's
/// legend lays out as if every class carried this count's digit width, so
/// the legend never re-wraps as counts change frame to frame).
fn max_class_count(cols: &[Column], fr: &Frames, scale: &super::ValueScale) -> usize {
    let Some((_, classes, _)) = &scale.classes else {
        return 0;
    };
    let Some(measure) = measure_of(cols, find_role(cols, Role::Geometry)) else {
        return 0;
    };
    let mut max_count = 0usize;
    for k in 0..fr.labels.len() {
        let mut counts = vec![0usize; classes.len()];
        for f in 0..fr.first_row.len() {
            if let Some(row) = fr.row_in[k][f] {
                if let Some(v) = measure.values.get(row).and_then(cell_num) {
                    let idx = classes.index(v.clamp(scale.lo, scale.hi));
                    counts[idx] += 1;
                }
            }
        }
        max_count = max_count.max(counts.into_iter().max().unwrap_or(0));
    }
    max_count
}

/// Force `m`'s own fill (built from one frame's view) onto `shared`'s —
/// same domain/classes/ramp or the same category levels — keeping only
/// this frame's own per-feature `values`/formatter/title (D21); and widen
/// `m`'s graduated-symbol size domain to `shared.size_limits` (U2).
pub(super) fn apply(m: &mut Model, shared: &Shared) {
    match (&mut m.fill, &shared.fill) {
        (Fill::Measure(view), Some(Fill::Measure(all))) => {
            let values = std::mem::take(&mut view.values);
            let fmt = view.fmt.clone();
            let title = view.title.clone();
            **view = (**all).clone();
            view.values = values;
            view.fmt = fmt;
            view.title = title;
        }
        (
            Fill::Category { of_row, levels },
            Some(Fill::Category {
                levels: all_levels, ..
            }),
        ) => {
            let remapped: Vec<Option<usize>> = of_row
                .iter()
                .map(|idx| {
                    idx.and_then(|i| levels.get(i))
                        .and_then(|(label, _)| all_levels.iter().position(|(l, _)| l == label))
                })
                .collect();
            *of_row = remapped;
            *levels = all_levels.clone();
        }
        _ => {}
    }
    if let (Some(sym), Some((_, hi))) = (&mut m.symbols, shared.size_limits) {
        sym.vmax = hi;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Column, Role};

    fn region_col(vals: &[&str]) -> Column {
        Column::new(
            "r",
            Role::Region,
            vals.iter().map(|s| Value::Str(s.to_string())).collect(),
        )
    }
    fn week_col(vals: &[&str]) -> Column {
        Column::new(
            "week",
            Role::Frame,
            vals.iter().map(|s| Value::Str(s.to_string())).collect(),
        )
    }
    fn value_col(vals: &[f64]) -> Column {
        Column::new(
            "v",
            Role::Value(crate::Kind::Bar),
            vals.iter().map(|v| Value::Float(*v)).collect(),
        )
    }

    #[test]
    fn frame_order_is_first_appearance_when_not_numeric_or_date() {
        let cols = vec![
            region_col(&["DE-BY", "DE-BW", "DE-BY", "DE-BW"]),
            week_col(&["b", "a", "b", "a"]),
            value_col(&[1.0, 2.0, 3.0, 4.0]),
        ];
        let fr = of(&cols).unwrap();
        assert_eq!(fr.labels, vec!["b".to_string(), "a".to_string()]);
    }

    #[test]
    fn frame_order_is_ascending_when_every_label_is_numeric() {
        let cols = vec![
            region_col(&["DE-BY", "DE-BW", "DE-BY", "DE-BW"]),
            week_col(&["2", "1", "2", "1"]),
            value_col(&[1.0, 2.0, 3.0, 4.0]),
        ];
        let fr = of(&cols).unwrap();
        assert_eq!(fr.labels, vec!["1".to_string(), "2".to_string()]);
    }

    #[test]
    fn view_fills_a_feature_absent_from_a_frame_with_na() {
        // DE-BY only has a row in frame "1".
        let cols = vec![
            region_col(&["DE-BY", "DE-BW"]),
            week_col(&["1", "2"]),
            value_col(&[10.0, 20.0]),
        ];
        let fr = of(&cols).unwrap();
        let k2 = fr.labels.iter().position(|l| l == "2").unwrap();
        let v = view(&cols, &fr, k2);
        let region = v.iter().find(|c| c.role == Role::Region).unwrap();
        let measure = v.iter().find(|c| matches!(c.role, Role::Value(_))).unwrap();
        // Both features' geometry is always present (first row).
        assert_eq!(region.values.len(), 2);
        assert!(region.values.iter().all(|v| !matches!(v, Value::Na)));
        // DE-BY (feature 0) has no row in frame "2": its measure is NA.
        let by_idx = fr.keys.iter().position(|k| k == "DE-BY").unwrap();
        assert_eq!(measure.values[by_idx], Value::Na);
    }

    #[test]
    fn duplicate_feature_in_one_frame_keeps_the_first_row() {
        let cols = vec![
            region_col(&["DE-BY", "DE-BY"]),
            week_col(&["1", "1"]),
            value_col(&[10.0, 99.0]),
        ];
        let fr = of(&cols).unwrap();
        assert_eq!(fr.duplicates, 1);
        let v = view(&cols, &fr, 0);
        let measure = v.iter().find(|c| matches!(c.role, Role::Value(_))).unwrap();
        assert_eq!(measure.values[0], Value::Float(10.0));
    }

    #[test]
    fn no_frame_column_is_none() {
        let cols = vec![region_col(&["DE-BY"]), value_col(&[1.0])];
        assert!(of(&cols).is_none());
    }
}
