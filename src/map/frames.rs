//! `::FRAME`: time maps (M2e-A, #149). One result holds every frame (a week,
//! a date); a "frame view" ([`view`]) reduces it to one row per feature,
//! with that frame's values, so every other map kind renders it unchanged
//! (D20). Scales are shared across every frame ([`Shared`]/[`apply`]) so
//! colours, classes and sizes never change meaning when stepping through
//! them (D21) — a frame without a row for a feature draws it as no data
//! (areas) or not at all (points/symbols), it is never a jump in the
//! legend.
//!
//! M2e-B extends the same machinery to flows, hexbin and bivariate maps:
//! a flow's feature key is its `ok`/`dk` ([`row_key`], reusing
//! [`flows::end_of`], joined on a separator that can't occur in either —
//! not the "→" the *drawn* `data-x` uses, which a region code could in
//! principle contain); a flow's shared linewidth domain and a graduated
//! symbol's shared radius domain are the same `shared.size_limits` (both
//! read `::SIZE`, [`apply`]); a hexbin map's shared cell-colour domain
//! comes from binning every frame's points at once
//! ([`apply_hexbin_shared`]), while each frame's own binning pass only
//! bins its own present points ([`presence`], D20 — hexbin's point
//! coordinates come from `view`'s always-present geometry fill, unlike a
//! value column); `::BIVARIATE`'s tertile breaks are built once over every
//! frame ([`Shared::bivariate`]) and forced onto each frame's own raw
//! values ([`bivariate::apply_shared`]).
//!
//! Plan: `~/projects/duckdb/plans/plan-maps-m2ef.md` §M2e-A/§M2e-B,
//! decisions D11-D22.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use crate::{cell_num, find_role, value_str, Column, RenderOptions, Role, Value};

use super::classes::{Classes, Method};
use super::{bivariate, category_fill, flows, hexbin, measure_of, value_scale, Fill, Model};

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
    /// A hexbin map's raw points over every kept frame (#608), for the
    /// frame-stable grid (D7: the projected bbox of all frames' points);
    /// empty otherwise. A hexbin map's "features" are frame-local slots,
    /// not points: see [`hexbin_of`].
    pub hex_points: Vec<[f64; 2]>,
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

/// A flow endpoint's own D22 key (M2e-B): the same normalisation
/// `flows::of`'s `resolve` uses before any boundary match — a region
/// code upper-cased, or `"{lat:.5},{lon:.5}"` — computed straight from the
/// raw `::ORIGIN`/`::DEST` cell, so a flow's frame identity doesn't need
/// the boundary pack at all.
fn flow_end_key(v: &Value) -> Option<String> {
    match flows::end_of(v) {
        // review of #595 NIT: `to_uppercase` (Unicode), matching
        // `flows::of`'s own `resolve` exactly (`flows.rs`'s `c.to_
        // uppercase()`, the value that becomes the drawn `data-x`) — a
        // mismatched case-folding here and there could disagree for a
        // non-ASCII code, splitting one feature's identity in two.
        flows::End::Code(c) => Some(c.to_uppercase()),
        flows::End::Point(p, _) => Some(format!("{:.5},{:.5}", p[1], p[0])),
        flows::End::Missing => None,
    }
}

/// A row's D22 feature key: for a flow map, `ok`/`dk` ([`flow_end_key`] of
/// each end — M2b's own key, so the *drawn* `data-x`, built separately in
/// `overlay.rs` as `"{ok}→{dk}"`, already matches) joined on a control
/// character (review of #595 NIT: a code could itself contain "→", making
/// origin "A→B"/dest "C" collide with origin "A"/dest "B→C" — `\u{1F}`
/// (unit separator) can't occur in a region code or a formatted lat/lon
/// pair, so this internal identity key stays unambiguous even though the
/// *displayed* `data-x` still uses "→"); otherwise a normalised `::REGION`
/// code, a `"{lat:.5}, {lon:.5}"` pair, or a WKT geometry's hash —
/// whichever column this map uses. `None` when the row has none (dropped
/// from every frame).
fn row_key(cols: &[Column], i: usize) -> Option<String> {
    if let (Some(oc), Some(dc)) = (find_role(cols, Role::Origin), find_role(cols, Role::Dest)) {
        let ok = oc.values.get(i).and_then(flow_end_key)?;
        let dk = dc.values.get(i).and_then(flow_end_key)?;
        return Some(format!("{ok}\u{1f}{dk}"));
    }
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
    let hexbin_points = find_role(cols, Role::Hexbin).is_some()
        && !(find_role(cols, Role::Origin).is_some() && find_role(cols, Role::Dest).is_some());
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
    if hexbin_points {
        return hexbin_of(cols, fcol, &labels, &label_index, total_frames);
    }

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
                hex_points: Vec::new(),
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
        hex_points: Vec::new(),
    })
}

/// [`of`] for a `::HEXBIN` map (#608): its cells, not its points, are what
/// a frame shows (D22's key is the cell's `"q,r"`), and points usually move
/// from frame to frame — keyed as features, 200 frames x 500 moving points
/// made 100,000 "features", so the D15 cell cap kept a single frame and
/// every frame's view carried all 100,000 rows. Here feature `j` is simply
/// frame k's `j`-th point (`row_in[k][j]`), and [`view`] takes its location
/// from that row. A few extra non-present "anchor" features at the extreme
/// longitude/latitude over every frame keep `Model::new`'s extent (and so
/// projection `'auto'`, D20) the same in every frame; the grid itself comes
/// from [`Frames::hex_points`] (every frame's points, D7).
fn hexbin_of(
    cols: &[Column],
    fcol: &Column,
    labels: &[String],
    label_index: &HashMap<&str, usize>,
    total_frames: usize,
) -> Option<Frames> {
    // (frame, slot, row, point), rows in result order; a MULTIPOINT row
    // contributes several points but takes one slot.
    let mut cells: Vec<(usize, usize, usize, [f64; 2])> = Vec::new();
    let mut slots = vec![0usize; labels.len()];
    let mut last_row = None;
    for (row, p) in hexbin::raw_points(cols) {
        let v = fcol.values.get(row).map(value_str).unwrap_or_default();
        let Some(&k) = label_index.get(v.as_str()) else {
            continue; // no frame, or dropped by the D15 frame cap
        };
        if last_row == Some(row) {
            cells.push((k, slots[k] - 1, row, p));
            continue;
        }
        last_row = Some(row);
        cells.push((k, slots[k], row, p));
        slots[k] += 1;
    }
    let width = slots.iter().copied().max().unwrap_or(0);
    if width == 0 {
        return None;
    }
    // D15 cell cap, on slots x frames: keep the last frames that fit.
    let max_frames = (MAX_CELLS / width).max(1);
    let drop = labels.len().saturating_sub(max_frames);
    let labels = labels[drop..].to_vec();
    cells.retain(|c| c.0 >= drop);
    let mut first_row: Vec<Option<usize>> = vec![None; width];
    let mut row_in: Vec<Vec<Option<usize>>> = vec![vec![None; width]; labels.len()];
    let (mut lo_x, mut hi_x, mut lo_y, mut hi_y) = (None, None, None, None);
    let mut hex_points = Vec::with_capacity(cells.len());
    for &(k, j, row, p) in &cells {
        first_row[j].get_or_insert(row);
        row_in[k - drop][j] = Some(row);
        hex_points.push(p);
        let pick = |cur: &mut Option<(f64, usize)>, v: f64, better: fn(f64, f64) -> bool| {
            if cur.is_none_or(|(c, _)| better(v, c)) {
                *cur = Some((v, row));
            }
        };
        pick(&mut lo_x, p[0], |a, b| a < b);
        pick(&mut hi_x, p[0], |a, b| a > b);
        pick(&mut lo_y, p[1], |a, b| a < b);
        pick(&mut hi_y, p[1], |a, b| a > b);
    }
    // A slot only frames before the cap used: no row left (never shown).
    let used: Vec<usize> = (0..width).filter(|&j| first_row[j].is_some()).collect();
    let mut first_row: Vec<usize> = used.iter().map(|&j| first_row[j].unwrap()).collect();
    let mut row_in: Vec<Vec<Option<usize>>> = row_in
        .into_iter()
        .map(|r| used.iter().map(|&j| r[j]).collect())
        .collect();
    let mut keys: Vec<String> = (0..first_row.len()).map(|j| j.to_string()).collect();
    let mut anchors: Vec<usize> = [lo_x, hi_x, lo_y, hi_y]
        .into_iter()
        .flatten()
        .map(|(_, r)| r)
        .collect();
    anchors.sort_unstable();
    anchors.dedup();
    for (a, row) in anchors.into_iter().enumerate() {
        first_row.push(row);
        keys.push(format!("extent-{a}"));
        for r in &mut row_in {
            r.push(None);
        }
    }
    Some(Frames {
        name: fcol.name.clone(),
        labels,
        keys,
        first_row,
        row_in,
        duplicates: 0,
        total_frames,
        hex_points,
    })
}

/// One result row per distinct feature, with that feature's geometry/
/// region/lat-lon (its first row) and every other column's value from
/// frame `k`'s row — `Value::Na` when the feature has none (D20).
pub(super) fn view(cols: &[Column], fr: &Frames, k: usize) -> Vec<Column> {
    // Review #595 BLOCKER 1: `::ORIGIN`/`::DEST` are a flow map's own
    // "geometry" — always taken from the feature's first row, same as an
    // area/point's, so the extent and `Proj::setup('auto', …)` built from
    // them (`Model::new`'s flow branch) never move between frames (D20).
    // A flow absent from frame k is hidden by [`presence`] at draw time
    // instead (mirroring hexbin's own point-presence filter), not by
    // blanking its endpoints here.
    let is_geometry_role = |r: Role| {
        matches!(
            r,
            Role::Region | Role::Geometry | Role::Lat | Role::Lon | Role::Origin | Role::Dest
        )
    };
    cols.iter()
        .map(|c| {
            let values: Vec<Value> = (0..fr.first_row.len())
                .map(|f| {
                    let row = if is_geometry_role(c.role) {
                        // #608: a hexbin slot's point is frame k's own row.
                        let own = (!fr.hex_points.is_empty())
                            .then(|| fr.row_in[k][f])
                            .flatten();
                        Some(own.unwrap_or(fr.first_row[f]))
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
/// *original*, unreduced result — plus `::SIZE`'s shared domain (a
/// graduated symbol's radius and a flow's linewidth both read the same
/// `::SIZE` role column, M2e-B — [`apply`] applies it to either), the
/// classed legend's widest per-class count (for `count_reserve`, U3), and
/// `::BIVARIATE`'s shared tertile breaks (M2e-B).
pub(super) struct Shared {
    pub fill: Option<Fill>,
    pub size_limits: Option<(f64, f64)>,
    pub max_count: usize,
    /// Tertile `Classes` of the map's own measure and of `::BIVARIATE`,
    /// built once over every frame's finite values — a cell's grid
    /// position then means the same thing whichever frame is shown; only
    /// the 3×3 counts are frame k's own ([`bivariate::apply_shared`]).
    pub bivariate: Option<(Classes, Classes)>,
    /// Review #595 SHOULD-FIX 6: a flow's fixed-width/has-size decision,
    /// made once over every frame's `::SIZE` values — a flow map never
    /// flips between a fixed-width line and a sqrt-scaled one frame to
    /// frame just because one frame's own rows happen to share a value
    /// (or happen to be all NA). Passed into `flows::of`/`project`
    /// directly (`Model::new`'s flow branch), not applied after the fact —
    /// unlike `size_limits`/`fill`, the *per-row* widths these decide are
    /// baked in during construction, too late for `apply` to fix up.
    pub flow_has_size: bool,
    pub flow_fixed: Option<f64>,
    /// Review #595 SHOULD-FIX 6: whether the map is `::BIVARIATE` at all,
    /// decided once over every frame's `b` values — a frame whose own `b`
    /// values are all NA still renders the 3×3 bivariate fill (every
    /// feature "no data" that frame), not a plain measure ramp (which
    /// would also silently skip `bivariate::apply_shared` since `m.fill`
    /// wouldn't match `Fill::Bivariate`).
    pub is_bivariate: bool,
    /// [`Frames::hex_points`] (#608): every frame's points, which size and
    /// anchor a hexbin map's grid (`hexbin::Grid::Over`).
    pub hex_points: Vec<[f64; 2]>,
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
    // Review #595 NIT: a hexbin map's shared fill is replaced wholesale by
    // `apply_hexbin_shared` (a *cell*-value distribution, not this
    // per-row one) — `max_class_count` below would size `count_reserve`
    // from the wrong numbers, so it's skipped outright for a hexbin map
    // rather than computing a value nothing ends up using correctly.
    let max_count = if find_role(cols, Role::Hexbin).is_some() {
        0
    } else {
        match &fill {
            Some(Fill::Measure(scale)) if scale.classes.is_some() => {
                max_class_count(cols, fr, scale)
            }
            _ => 0,
        }
    };
    let bivariate = shared_bivariate(cols, fr);
    let flow_has_size = find_role(cols, Role::Size)
        .is_some_and(|c| (0..n).any(|i| c.values.get(i).and_then(cell_num).is_some()));
    let flow_fixed = find_role(cols, Role::Size)
        .filter(|_| flow_has_size)
        .and_then(flows::constant_width);
    let is_bivariate = find_role(cols, Role::Bivariate)
        .is_some_and(|c| (0..n).any(|i| c.values.get(i).and_then(cell_num).is_some()));
    Shared {
        fill,
        size_limits,
        max_count,
        bivariate,
        flow_has_size,
        flow_fixed,
        is_bivariate,
        hex_points: fr.hex_points.clone(),
    }
}

/// `::BIVARIATE`'s shared tertile breaks (M2e-B, D21): `Classes::build`
/// (quantile, 3) over every frame's finite `a` (the map's own measure) and
/// `b` (`::BIVARIATE`) values — `None` without both, mirroring
/// `bivariate::of`'s own "no finite value at all" bail-out.
/// Review of #595 NIT: built only from the rows a frame can actually show
/// (`fr.row_in`'s own selection) — not blindly every row of `cols`, which
/// would also count a row with no `::FRAME` value (dropped entirely,
/// never in `fr`), a frame cut by the D15 cap, and a D13 duplicate's
/// second+ row (first row wins). None of those ever reach the screen, so
/// they shouldn't skew the shared tertile breaks either.
fn shared_bivariate(cols: &[Column], fr: &Frames) -> Option<(Classes, Classes)> {
    let a_col = measure_of(cols, super::geometry(cols))?;
    let b_col = find_role(cols, Role::Bivariate)?;
    let shown_rows = || fr.row_in.iter().flatten().filter_map(|r| *r);
    let finite = |c: &Column| -> Vec<f64> {
        shown_rows()
            .filter_map(|i| c.values.get(i).and_then(cell_num).filter(|v| v.is_finite()))
            .collect()
    };
    let b_vals = finite(b_col);
    if b_vals.is_empty() {
        return None;
    }
    let ca = Classes::build(&Method::Quantile(3), &finite(&a_col))?;
    let cb = Classes::build(&Method::Quantile(3), &b_vals)?;
    Some((ca, cb))
}

/// Per feature, whether frame `k` had its own row (D20). Hexbin's own
/// binning pass needs this directly ([`hexbin::of`]'s `present`): `view`
/// always fills a slot's (or extent anchor's) location, so the extent
/// stays frame-stable (D20), unlike a value column, which `view` already
/// turns into `Value::Na` when a feature has no row in frame `k`.
pub(super) fn presence(fr: &Frames, k: usize) -> Vec<bool> {
    fr.row_in[k].iter().map(Option::is_some).collect()
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
/// this frame's own per-feature `values`/formatter/title (D21); widen
/// `m`'s graduated-symbol size domain, and a flow map's linewidth domain
/// (M2e-B — both read `::SIZE`), to `shared.size_limits` (U2); and
/// recompute a `::BIVARIATE` fill's per-row tertile levels and 3×3 counts
/// from `shared.bivariate`'s breaks (M2e-B).
pub(super) fn apply(m: &mut Model, shared: &Shared) {
    if let Some(all) = &shared.fill {
        merge_fill(&mut m.fill, all);
    }
    if let (Some(sym), Some((_, hi))) = (&mut m.symbols, shared.size_limits) {
        sym.vmax = hi;
    }
    if let (Some(fl), Some((_, hi))) = (&mut m.flows, shared.size_limits) {
        fl.vmax = hi;
    }
    if let (Fill::Bivariate(biv), Some((ca, cb))) = (&mut m.fill, &shared.bivariate) {
        bivariate::apply_shared(biv, ca, cb);
    }
}

/// Force `view`'s own fill onto `all`'s domain/classes/ramp (a measure) or
/// levels (a category), keeping `view`'s own per-feature `values` (plus a
/// measure's formatter/title) — the merge [`apply`] does for the ordinary
/// `Shared::fill`, factored out so [`apply_hexbin_shared`] can reuse it for
/// a hexbin map's own, differently-computed shared fill.
fn merge_fill(view: &mut Fill, all: &Fill) {
    match (view, all) {
        (Fill::Measure(v), Fill::Measure(a)) => {
            let values = std::mem::take(&mut v.values);
            let fmt = v.fmt.clone();
            let title = v.title.clone();
            **v = (**a).clone();
            v.values = values;
            v.fmt = fmt;
            v.title = title;
        }
        (
            Fill::Category { of_row, levels },
            Fill::Category {
                levels: all_levels, ..
            },
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
}

/// `::HEXBIN`'s shared cell-colour domain (M2e-B, D21; review of #595,
/// fixing BLOCKER 2). A frame's own [`hexbin::of`] (inside
/// [`Model::new`](super::Model::new)) already bins only its own present
/// points on the frame-stable grid (D7/[`presence`]); this bins *every
/// frame separately*, the same way (same grid, same D13/D15 row
/// selection via [`view`]/[`presence`]), and takes the shared domain/
/// classes over the **union of every frame's own cell values** — not one
/// combined bin of every frame's points at once, which would sum
/// unrelated frames' values into the same cell and badly overstate the
/// domain (e.g. three frames each with one cell at 10 would read as a
/// single cell at 30). [`merge_fill`]s `m`'s own (frame k's) fill onto
/// that shared domain, the same way [`apply`] does for a plain measure: a
/// cell's colour means the same magnitude whichever frame is shown; only
/// its own cells' values stay frame k's own. Also fills in `shared.
/// max_count` (U3's `count_reserve`) from the same per-frame cell values,
/// classed the same way — `shared_of` skips it for a hexbin map precisely
/// because only this function has the right distribution to compute it
/// from.
pub(super) fn apply_hexbin_shared(
    m: &mut Model,
    o: &RenderOptions,
    cols: &[Column],
    fr: &Frames,
    shared: &mut Shared,
) {
    let mut per_frame: Vec<Vec<f64>> = Vec::with_capacity(fr.labels.len());
    let mut has_measure = false;
    // #608: every frame on frame k's own grid (`Model::new` sized it from
    // every frame's points), not re-derived per frame.
    let grid = m
        .hexbin
        .as_ref()
        .map_or(hexbin::Grid::Own, |hb| hexbin::Grid::Fixed {
            width: hb.width,
            origin: hb.origin,
        });
    for k in 0..fr.labels.len() {
        let view_k = view(cols, fr, k);
        let present = presence(fr, k);
        let values = hexbin::of(&view_k, &m.proj, Some(&present), grid).map(|hb| {
            has_measure = has_measure || hb.has_measure;
            hb.cells
                .iter()
                .map(|(_, c)| if hb.has_measure { c.sum } else { c.n as f64 })
                .collect::<Vec<f64>>()
        });
        per_frame.push(values.unwrap_or_default());
    }
    let all_values: Vec<f64> = per_frame.iter().flatten().copied().collect();
    if all_values.is_empty() {
        return;
    }
    let union_fill = hexbin::fill_of_values(o, cols, &all_values, has_measure);
    merge_fill(&mut m.fill, &union_fill);
    if let Fill::Measure(scale) = &m.fill {
        if let Some((_, classes, _)) = &scale.classes {
            shared.max_count = per_frame
                .iter()
                .map(|vals| {
                    let mut counts = vec![0usize; classes.len()];
                    for v in vals {
                        counts[classes.index(v.clamp(scale.lo, scale.hi))] += 1;
                    }
                    counts.into_iter().max().unwrap_or(0)
                })
                .max()
                .unwrap_or(0);
        }
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
