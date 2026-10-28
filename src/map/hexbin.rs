//! Density binning (`::HEXBIN`, M2c): aggregate a map's points into a grid
//! of pointy-top hexagons, coloured by the sum of a measure, else a count of
//! rows (D8). The grid is anchored to the **unzoomed** projected bounding
//! box of the data, so zooming or exporting never re-bins (D7).
//!
//! Applies only when `::HEXBIN` is present and the input resolves to points
//! (`::LAT`/`::LON`, or WKT `POINT`/`MULTIPOINT`); otherwise it is ignored
//! (`dashboard --check` flags this — `map/hexbin-needs-points`).
//!
//! The lattice itself (bin membership, the per-cell `count`/`n`) comes from
//! ggplot-rs's `StatBinHex::compute_group` on a fixed hex lattice (issue
//! #318: visualization logic belongs in ggplot-rs, not hand-written
//! hex-grid math) — the actual drawing is `super::overlay`'s `geom_hex`
//! renderer. This module only does data prep: point extraction/projection,
//! `::HEXBIN` size resolution, and the per-cell mean *raw* (pre-projection)
//! position (`StatBinHex` doesn't compute that; it's specific to this
//! consumer's "around …" label), via `HexLattice::locate`.

use std::collections::BTreeMap;

use crate::{cell_num, find_role, role_str, Column, Kind, RenderOptions, Role};
use ggplot_rs::data::DataFrame;
use ggplot_rs::prelude::Value;
use ggplot_rs::scale::ScaleSet;
use ggplot_rs::stat::binhex::{HexLattice, StatBinHex};
use ggplot_rs::stat::Stat;

use super::project::{Extent, Proj};
use super::{Fill, Shape};

/// A `::HEXBIN` value as written.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum HexSize {
    Auto,
    /// The written number, and whether a `'km'` suffix was given (resolved
    /// against the projection in [`HexSize::resolve`] — a planar map's `km`
    /// is accepted and ignored, D7).
    Width(f64, bool),
}

impl HexSize {
    /// Parse `'auto'`/`''`, `'<n> km'`, `'<n> m'` or a bare `'<n>'`. `n` must
    /// be finite and positive.
    pub fn parse(s: &str) -> Result<HexSize, String> {
        let t = s.trim();
        if t.is_empty() || t.eq_ignore_ascii_case("auto") {
            return Ok(HexSize::Auto);
        }
        let lower = t.to_ascii_lowercase();
        let (digits, km) = if let Some(p) = lower.strip_suffix("km") {
            (p, true)
        } else if let Some(p) = lower.strip_suffix('m') {
            (p, false)
        } else {
            (lower.as_str(), false)
        };
        let n: f64 = digits
            .trim()
            .parse()
            .map_err(|_| format!("::HEXBIN '{s}' is not 'auto', '25 km' or a number"))?;
        if !n.is_finite() || n <= 0.0 {
            return Err(format!(
                "::HEXBIN '{s}': the size must be a positive number"
            ));
        }
        Ok(HexSize::Width(n, km))
    }

    /// The width in metres (or, on a planar map, native units): a bare
    /// number or `'m'` suffix is used as written; `'km'` multiplies by 1,000
    /// unless the map is planar, where it is accepted and ignored (`'none'`
    /// projection: the written number is already the planar width).
    fn resolve(&self, planar: bool) -> Option<f64> {
        match self {
            HexSize::Auto => None,
            HexSize::Width(n, km) => Some(if *km && !planar { n * 1_000.0 } else { *n }),
        }
    }
}

/// One occupied cell's running totals.
#[derive(Default, Clone)]
pub(crate) struct CellAgg {
    pub n: u64,
    /// Sum of the measure (`Role::Value`), when there is one.
    pub sum: f64,
    /// Sum of the member points' raw first coordinate (lon, or x on a
    /// planar map) — for the mean position shown in `data-label`.
    pub a_sum: f64,
    /// Sum of the raw second coordinate (lat, or y).
    pub b_sum: f64,
}

impl CellAgg {
    fn mean(&self) -> (f64, f64) {
        let n = self.n.max(1) as f64;
        (self.a_sum / n, self.b_sum / n)
    }
}

/// The binned grid: its width, origin and occupied cells, in deterministic
/// `(r, q)` order (ggplot-rs's `StatBinHex` output order).
pub(crate) struct Hexbin {
    /// The written width across, metres (or planar units) — D7. Also the
    /// `HexLattice`'s `w == h` centre-to-centre spacing: a regular hexagon.
    pub width: f64,
    /// The grid's origin: the unzoomed projected data box's `(x0, y0)`.
    pub origin: (f64, f64),
    /// Occupied cells, `((q, r), agg)`, in the `BTreeMap`'s `(r, q)` order.
    pub cells: Vec<((i32, i32), CellAgg)>,
    pub n_points: usize,
    pub has_measure: bool,
    /// Whether the map is planar (`'none'` projection): coordinates are
    /// native units, and `data-label`/the legend note drop the km unit.
    pub planar: bool,
    /// Small multiples (M3a-2, #645): each cell's facet value, parallel to
    /// `cells` (every facet binned on the one grid); empty otherwise.
    pub facet: Vec<String>,
}

/// Cells above this count get a render warning suggesting a larger
/// `::HEXBIN` (the plan's "Caps").
pub(crate) const HEX_WARN: usize = 20_000;

/// The `HexLattice` a resolved [`Hexbin`] was binned on — reconstructed
/// (not stored on `Hexbin` itself) since `width`/`origin` already pin it
/// down exactly; used by `super::overlay`'s hexbin renderer to get each
/// cell's centre (`StatBinHex::compute_group`'s `x`/`y` would also give
/// this, but we don't carry those columns through `Hexbin::cells`).
pub(super) fn lattice_of(hb: &Hexbin) -> HexLattice {
    HexLattice::new(hb.width, hb.width, hb.origin)
}

/// What a [`of`] call sizes and anchors its grid from (#608).
#[derive(Clone, Copy)]
pub(super) enum Grid<'a> {
    /// The points it bins (every point of `cols`, present or not).
    Own,
    /// These raw points instead: a `::FRAME` map's every frame (D7).
    Over(&'a [[f64; 2]]),
    /// An already-resolved grid.
    Fixed { width: f64, origin: (f64, f64) },
}

/// Raw (pre-projection) points to bin: `::LAT`/`::LON` pairs, else the
/// `POINT`/`MULTIPOINT` parts of a `::MAP` WKT column — one `(row, [x, y])`
/// per point (a `MULTIPOINT` row contributes several).
pub(super) fn raw_points(cols: &[Column]) -> Vec<(usize, [f64; 2])> {
    if let (Some(lat), Some(lon)) = (find_role(cols, Role::Lat), find_role(cols, Role::Lon)) {
        let n = lat.values.len().min(lon.values.len());
        return (0..n)
            .filter_map(
                |i| match (cell_num(&lon.values[i]), cell_num(&lat.values[i])) {
                    (Some(x), Some(y)) => Some((i, [x, y])),
                    _ => None,
                },
            )
            .collect();
    }
    let Some(g) = super::geometry(cols) else {
        return Vec::new();
    };
    let n = g.values.len();
    super::parse_column(Some(g), n)
        .into_iter()
        .enumerate()
        .flat_map(|(i, shapes)| {
            shapes.into_iter().filter_map(move |s| match s {
                Shape::Point(p) => Some((i, p)),
                _ => None,
            })
        })
        .collect()
}

/// Bin the map's points into hexagons (`None`: no `::HEXBIN`, or the input
/// has no points — the lint, not this function, reports that case).
///
/// `present` (M2e-B, D20): on a `::FRAME` map, which of `cols`'s rows
/// (indexed the same way `raw_points` returns them) actually belong to the
/// frame being bound — `None` outside a frame map, where every row counts.
/// It only gates *which points get binned*: the grid's `width`/`origin`
/// come from `grid` — every point `cols` carries ([`Grid::Own`]), every
/// frame's points on a `::FRAME` map ([`Grid::Over`], #608, so the grid
/// never moves between frames, D7/D20), or a grid already resolved.
pub(super) fn of(
    cols: &[Column],
    proj: &Proj,
    present: Option<&[bool]>,
    grid: Grid,
) -> Option<Hexbin> {
    find_role(cols, Role::Hexbin)?;
    let pts = raw_points(cols);
    if pts.is_empty() {
        return None;
    }
    let mut extent = Extent::new();
    let mut projected: Vec<(usize, [f64; 2], [f64; 2])> = Vec::with_capacity(pts.len());
    for (row, raw) in &pts {
        if let Some(p) = proj.forward(*raw) {
            extent.add(p);
            projected.push((*row, *raw, p));
        }
    }
    if projected.is_empty() {
        return None;
    }
    let planar = matches!(proj, Proj::Identity);
    let spec = role_str(cols, Role::Hexbin)
        .map(|s| HexSize::parse(&s).unwrap_or(HexSize::Auto))
        .unwrap_or(HexSize::Auto);
    if let Grid::Over(pts) = grid {
        let mut all = Extent::new();
        for p in pts.iter().filter_map(|r| proj.forward(*r)) {
            all.add(p);
        }
        if all.x0 <= all.x1 {
            extent = all;
        }
    }
    let (width, origin) = match grid {
        Grid::Fixed { width, origin } => (width, origin),
        Grid::Own | Grid::Over(_) => (
            spec.resolve(planar).unwrap_or_else(|| {
                // D7: nice_floor of the longer side of the unzoomed projected
                // data box, over 40 — a lone point (a degenerate box) gets a
                // 1 km default.
                let span = (extent.x1 - extent.x0).max(extent.y1 - extent.y0);
                let span = if span > 0.0 { span } else { 1_000.0 };
                super::legend::nice_floor(span / 40.0).max(f64::MIN_POSITIVE)
            }),
            (extent.x0, extent.y0),
        ),
    };

    // The points actually binned: every one of them outside a frame map,
    // else only those with a row in this frame (D20 — not drawn, rather
    // than counted as a zero-weight member of their cell, which `locate`
    // would still do since `projected` carries every feature's point
    // regardless of presence).
    let binned: Vec<(usize, [f64; 2], [f64; 2])> = projected
        .iter()
        .filter(|(row, _, _)| present.is_none_or(|p| p.get(*row).copied().unwrap_or(true)))
        .copied()
        .collect();
    // Review #595 BLOCKER 3: on a frame map, a frame with zero *present*
    // points still IS a hexbin map (D20 — not drawn, not "falls back to
    // plain points") — `binned` empty is fine; `cells` ends up empty too,
    // `Model::new`'s `hexbin.is_none()` symbol fallback never triggers.
    // Outside a frame map `present` is `None`, so `binned == projected`
    // (every point passes the filter above) and `projected.is_empty()`
    // already returned `None` if there was truly nothing to bin.
    let measure = super::measure_col(cols);
    // Review #595 NIT: a non-frame map's `has_measure` stays the original
    // "any row of `cols` at all" rule (byte-identical) — only a frame map
    // (which can have a point with no row this frame, D20) narrows it to
    // the rows actually binned, so an absent feature's stale measure value
    // can't flip a frame between "sum" and "count" on its own.
    let has_measure = measure.is_some_and(|m| {
        if present.is_some() {
            binned
                .iter()
                .any(|(row, _, _)| m.values.get(*row).and_then(cell_num).is_some())
        } else {
            m.values.iter().any(|v| cell_num(v).is_some())
        }
    });

    // D7/the ggplot-rs plan's C1 §4: a fixed lattice (`binwidth` *and*
    // `origin` both given), so zoom/export never re-bin. A `weight` column
    // (D8) makes `compute_group`'s `count` output come out as Σ measure
    // when there's one, else a plain point count (weight defaults to 1 per
    // point when the column is absent) — this consumer's own "sum of the
    // measure, else count of points" rule, for free.
    let mut df = DataFrame::new();
    df.add_column(
        "x".to_string(),
        binned.iter().map(|(_, _, p)| Value::Float(p[0])).collect(),
    );
    df.add_column(
        "y".to_string(),
        binned.iter().map(|(_, _, p)| Value::Float(p[1])).collect(),
    );
    if let Some(m) = measure {
        df.add_column(
            "weight".to_string(),
            binned
                .iter()
                .map(|(row, _, _)| {
                    m.values
                        .get(*row)
                        .and_then(cell_num)
                        .map(Value::Float)
                        .unwrap_or(Value::Na)
                })
                .collect(),
        );
    }
    let bins = StatBinHex::new()
        .binwidth(width, width)
        .origin(origin.0, origin.1)
        .compute_group(&df, &ScaleSet::new());

    // The per-cell mean *raw* position, for `label_of`'s "around …" text —
    // `StatBinHex` doesn't compute this (it's specific to this consumer's
    // label): `locate()` each raw point on the same lattice it used.
    let lattice = HexLattice::new(width, width, origin);
    let mut means: BTreeMap<(i64, i64), (f64, f64)> = BTreeMap::new();
    for (_, raw, p) in &binned {
        let (q, r) = lattice.locate(p[0], p[1]);
        let e = means.entry((q, r)).or_insert((0.0, 0.0));
        e.0 += raw[0];
        e.1 += raw[1];
    }

    let col = |name: &str| bins.column(name).unwrap_or(&[]);
    let (qs, rs, counts, ns) = (col("q"), col("r"), col("count"), col("n"));
    let n_rows = bins.nrows();
    let mut cells: Vec<((i32, i32), CellAgg)> = Vec::with_capacity(n_rows);
    for i in 0..n_rows {
        let q = qs[i].as_f64().unwrap_or(0.0) as i32;
        let r = rs[i].as_f64().unwrap_or(0.0) as i32;
        let sum = counts[i].as_f64().unwrap_or(0.0);
        let n = ns[i].as_f64().unwrap_or(0.0) as u64;
        let (a_sum, b_sum) = means
            .get(&(q as i64, r as i64))
            .copied()
            .unwrap_or((0.0, 0.0));
        cells.push((
            (q, r),
            CellAgg {
                n,
                sum,
                a_sum,
                b_sum,
            },
        ));
    }

    Some(Hexbin {
        width,
        origin,
        cells,
        n_points: binned.len(),
        has_measure,
        planar,
        facet: Vec::new(),
    })
}

/// The colour fill for a hexbin map: a synthetic measure column over the
/// cells' values (sum, else count), through the same [`super::value_scale`]
/// choropleths use — so `::CLASSES`/`::RANGE`/`::MIDPOINT` all work. When
/// counting, `::YFORMAT` is dropped from the columns passed in, so a '€'
/// format meant for the real measure doesn't leak onto a plain count.
pub(super) fn fill_of(o: &RenderOptions, cols: &[Column], hb: &Hexbin) -> Fill {
    let values: Vec<f64> = hb
        .cells
        .iter()
        .map(|(_, c)| if hb.has_measure { c.sum } else { c.n as f64 })
        .collect();
    fill_of_values(o, cols, &values, hb.has_measure)
}

/// The value-scale part of [`fill_of`], over an arbitrary list of cell
/// values rather than one `Hexbin`'s own `cells` — shared with
/// `frames::apply_hexbin_shared` (M2e-B, review of #595 BLOCKER 2), whose
/// shared cell-colour domain is the *union* of every frame's own cell
/// values, not one `Hexbin`'s.
pub(super) fn fill_of_values(
    o: &RenderOptions,
    cols: &[Column],
    values: &[f64],
    has_measure: bool,
) -> Fill {
    let vals: Vec<Value> = values.iter().map(|v| Value::Float(*v)).collect();
    let name = if has_measure {
        super::measure_col(cols)
            .map(|c| c.name.clone())
            .unwrap_or_default()
    } else {
        String::new()
    };
    let kind = if has_measure {
        match super::measure_col(cols).and_then(|c| match c.role {
            Role::Value(k) => Some(k),
            _ => None,
        }) {
            Some(k) => k,
            None => Kind::Bar,
        }
    } else {
        Kind::Bar
    };
    let synthetic = Column::new(name, Role::Value(kind), vals);
    let has_base = find_role(cols, Role::Basemap).is_some();
    let n = values.len();
    let scale = if has_measure {
        super::value_scale(o, cols, &synthetic, has_base, n)
    } else {
        let filtered: Vec<Column> = cols
            .iter()
            .filter(|c| c.role != Role::YFormat)
            .cloned()
            .collect();
        super::value_scale(o, &filtered, &synthetic, has_base, n)
    };
    Fill::Measure(Box::new(scale))
}

/// The "around …" position label of a cell's mean member position.
pub(super) fn label_of(cell: &CellAgg, planar: bool) -> String {
    let (a, b) = cell.mean();
    if planar {
        format!("around x {a:.1}, y {b:.1}")
    } else {
        format!("around {:.2}\u{b0} N, {:.2}\u{b0} E", b, a)
    }
}

pub(super) fn fmt_num(v: f64) -> String {
    if (v - v.round()).abs() < 1e-9 {
        format!("{}", v.round() as i64)
    } else {
        // Rust's `f64` Display already picks the shortest exact decimal
        // representation — rounding to a fixed number of places here would
        // corrupt an exact value like 0.25 (half-way ties round away from
        // zero: `(0.25 * 10.0).round() / 10.0` is 0.3, not 0.25).
        format!("{v}")
    }
}

/// The hex width as shown in the legend note / summary: `"2.5 km"` / `"500
/// m"`, or on a planar map the plain number with no unit.
pub(super) fn fmt_width(w: f64, planar: bool) -> String {
    if planar {
        fmt_num(w)
    } else if w >= 1_000.0 {
        format!("{} km", fmt_num(w / 1_000.0))
    } else {
        format!("{} m", fmt_num(w))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hexbin_spec_parsing() {
        assert_eq!(HexSize::parse("auto"), Ok(HexSize::Auto));
        assert_eq!(HexSize::parse(""), Ok(HexSize::Auto));
        assert_eq!(HexSize::parse("  Auto  "), Ok(HexSize::Auto));
        assert_eq!(HexSize::parse("25 km"), Ok(HexSize::Width(25.0, true)));
        assert_eq!(HexSize::parse("25km"), Ok(HexSize::Width(25.0, true)));
        assert_eq!(HexSize::parse("2.5 m"), Ok(HexSize::Width(2.5, false)));
        assert_eq!(HexSize::parse("2.5"), Ok(HexSize::Width(2.5, false)));
        for bad in ["-5", "0", "km", "abc", "5 miles", "NaN", "inf"] {
            assert!(HexSize::parse(bad).is_err(), "{bad}");
        }
        // km resolves ×1000 unless planar, where it's accepted and ignored
        let km = HexSize::parse("25 km").unwrap();
        assert_eq!(km.resolve(false), Some(25_000.0));
        assert_eq!(km.resolve(true), Some(25.0));
        let bare = HexSize::parse("2.5").unwrap();
        assert_eq!(bare.resolve(false), Some(2.5));
        assert_eq!(bare.resolve(true), Some(2.5));
        assert_eq!(HexSize::Auto.resolve(false), None);
    }

    /// D7: hexagons are regular (same size everywhere by construction), so
    /// their projected area under an equal-area projection is identical
    /// whether they sit at the equator or at 60° — the grid math (axial
    /// round-trip, `ring`'s vertices) is consistent regardless of where a
    /// cell is placed.
    #[test]
    fn hexagons_are_equal_area_under_equal_earth() {
        use super::super::labels::area;
        use super::super::project::{Proj, Projection};
        let (_, proj) = Proj::setup(Projection::EqualEarth, &{
            let mut e = Extent::new();
            e.add([-10.0, -5.0]);
            e.add([10.0, 65.0]);
            e
        });
        // One lattice, origin (0,0): `locate()` finds whichever cell each
        // projected point falls in, so the two latitudes don't need to be
        // (and in general won't be) in the same cell.
        let lattice = HexLattice::new(50_000.0, 50_000.0, (0.0, 0.0));
        let at = |lon: f64, lat: f64| -> f64 {
            let p = proj.forward([lon, lat]).unwrap();
            let (q, r) = lattice.locate(p[0], p[1]);
            let verts = lattice.vertices(q, r);
            area(&verts)
        };
        let a0 = at(0.0, 0.0);
        let a60 = at(0.0, 60.0);
        assert!(a0 > 0.0 && a60 > 0.0, "{a0} {a60}");
        assert!(
            ((a0 - a60) / a0).abs() < 1e-6,
            "hex areas differ: {a0} vs {a60}"
        );
    }
}
