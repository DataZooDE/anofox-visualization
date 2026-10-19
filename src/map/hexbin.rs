//! Density binning (`::HEXBIN`, M2c): aggregate a map's points into a grid
//! of pointy-top hexagons, coloured by the sum of a measure, else a count of
//! rows (D8). The grid is anchored to the **unzoomed** projected bounding
//! box of the data, so zooming or exporting never re-bins (D7).
//!
//! Applies only when `::HEXBIN` is present and the input resolves to points
//! (`::LAT`/`::LON`, or WKT `POINT`/`MULTIPOINT`); otherwise it is ignored
//! (`dashboard --check` flags this — `map/hexbin-needs-points`).

use std::collections::BTreeMap;

use crate::{cell_num, find_role, role_str, Column, Kind, RenderOptions, Role};
use ggplot_rs::prelude::Value;

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

/// The binned grid: its width, circumradius, origin and occupied cells, in
/// deterministic `(r, q)` order.
pub(crate) struct Hexbin {
    /// The written width across, metres (or planar units) — D7.
    pub width: f64,
    /// Circumradius `s = width / √3`.
    pub size: f64,
    /// The grid's origin: the unzoomed projected data box's `(x0, y0)`.
    pub origin: (f64, f64),
    /// Occupied cells, `((q, r), agg)`, in the `BTreeMap`'s `(r, q)` order.
    pub cells: Vec<((i32, i32), CellAgg)>,
    pub n_points: usize,
    pub has_measure: bool,
    /// Whether the map is planar (`'none'` projection): coordinates are
    /// native units, and `data-label`/the legend note drop the km unit.
    pub planar: bool,
}

/// Cells above this count get a render warning suggesting a larger
/// `::HEXBIN` (the plan's "Caps").
pub(crate) const HEX_WARN: usize = 20_000;

const SQRT3: f64 = 1.732_050_807_568_877_2;

/// Pointy-top axial hex centre, in the same projected units as `size`.
fn axial_to_xy(q: f64, r: f64, size: f64) -> (f64, f64) {
    (size * (SQRT3 * q + SQRT3 / 2.0 * r), size * (1.5 * r))
}

/// Round fractional cube coordinates to the nearest hex, fixing whichever
/// component had the largest rounding error so `x + y + z == 0` holds
/// (the standard "cube rounding" algorithm).
fn cube_round(x: f64, y: f64, z: f64) -> (i32, i32, i32) {
    let (mut rx, mut ry, mut rz) = (x.round(), y.round(), z.round());
    let (dx, dy, dz) = ((rx - x).abs(), (ry - y).abs(), (rz - z).abs());
    if dx > dy && dx > dz {
        rx = -ry - rz;
    } else if dy > dz {
        ry = -rx - rz;
    } else {
        rz = -rx - ry;
    }
    (rx as i32, ry as i32, rz as i32)
}

/// The axial `(q, r)` cell of a point `size`-relative to the grid's origin
/// (pointy-top orientation).
fn point_to_hex(x: f64, y: f64, size: f64) -> (i32, i32) {
    let q = (SQRT3 / 3.0 * x - y / 3.0) / size;
    let r = (2.0 / 3.0 * y) / size;
    let (cx, cz) = (q, r);
    let cy = -cx - cz;
    let (rx, _, rz) = cube_round(cx, cy, cz);
    (rx, rz)
}

/// The six vertices of hexagon `(q, r)`, in the same projected coordinate
/// space as `origin` — test-only, and used by [`super`]'s SVG writer.
pub(crate) fn ring(q: i32, r: i32, size: f64, origin: (f64, f64)) -> [[f64; 2]; 6] {
    let (cx, cy) = axial_to_xy(q as f64, r as f64, size);
    let (cx, cy) = (cx + origin.0, cy + origin.1);
    let mut pts = [[0.0; 2]; 6];
    for (i, p) in pts.iter_mut().enumerate() {
        let a = (std::f64::consts::PI / 180.0) * (60.0 * i as f64 - 30.0);
        *p = [cx + size * a.cos(), cy + size * a.sin()];
    }
    pts
}

/// Raw (pre-projection) points to bin: `::LAT`/`::LON` pairs, else the
/// `POINT`/`MULTIPOINT` parts of a `::MAP` WKT column — one `(row, [x, y])`
/// per point (a `MULTIPOINT` row contributes several).
fn raw_points(cols: &[Column]) -> Vec<(usize, [f64; 2])> {
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
pub(super) fn of(cols: &[Column], proj: &Proj) -> Option<Hexbin> {
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
    let width = spec.resolve(planar).unwrap_or_else(|| {
        // D7: nice_floor of the longer side of the unzoomed projected data
        // box, over 40 — a lone point (a degenerate box) gets a 1 km default.
        let span = (extent.x1 - extent.x0).max(extent.y1 - extent.y0);
        let span = if span > 0.0 { span } else { 1_000.0 };
        super::legend::nice_floor(span / 40.0).max(f64::MIN_POSITIVE)
    });
    let size = width / SQRT3;
    let origin = (extent.x0, extent.y0);
    let measure = cols.iter().find(|c| matches!(c.role, Role::Value(_)));
    let mut cells: BTreeMap<(i32, i32), CellAgg> = BTreeMap::new();
    for (row, raw, p) in &projected {
        let (q, r) = point_to_hex(p[0] - origin.0, p[1] - origin.1, size);
        let v = measure.and_then(|m| m.values.get(*row)).and_then(cell_num);
        let e = cells.entry((r, q)).or_default();
        e.n += 1;
        if let Some(v) = v {
            e.sum += v;
        }
        e.a_sum += raw[0];
        e.b_sum += raw[1];
    }
    let has_measure = measure.is_some_and(|m| m.values.iter().any(|v| cell_num(v).is_some()));
    // re-key to (q, r) for the public contract (data-x writes "q,r")
    let cells: Vec<((i32, i32), CellAgg)> = cells
        .into_iter()
        .map(|((r, q), agg)| ((q, r), agg))
        .collect();
    Some(Hexbin {
        width,
        size,
        origin,
        cells,
        n_points: projected.len(),
        has_measure,
        planar,
    })
}

/// The colour fill for a hexbin map: a synthetic measure column over the
/// cells' values (sum, else count), through the same [`super::value_scale`]
/// choropleths use — so `::CLASSES`/`::RANGE`/`::MIDPOINT` all work. When
/// counting, `::YFORMAT` is dropped from the columns passed in, so a '€'
/// format meant for the real measure doesn't leak onto a plain count.
pub(super) fn fill_of(o: &RenderOptions, cols: &[Column], hb: &Hexbin) -> Fill {
    let values: Vec<Value> = hb
        .cells
        .iter()
        .map(|(_, c)| Value::Float(if hb.has_measure { c.sum } else { c.n as f64 }))
        .collect();
    let name = if hb.has_measure {
        cols.iter()
            .find(|c| matches!(c.role, Role::Value(_)))
            .map(|c| c.name.clone())
            .unwrap_or_default()
    } else {
        String::new()
    };
    let kind = if hb.has_measure {
        match cols.iter().find_map(|c| match c.role {
            Role::Value(k) => Some(k),
            _ => None,
        }) {
            Some(k) => k,
            None => Kind::Bar,
        }
    } else {
        Kind::Bar
    };
    let synthetic = Column::new(name, Role::Value(kind), values);
    let has_base = find_role(cols, Role::Basemap).is_some();
    let n = hb.cells.len();
    let scale = if hb.has_measure {
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
        let size = 50_000.0; // an arbitrary circumradius, metres
        let at = |lon: f64, lat: f64| -> f64 {
            let p = proj.forward([lon, lat]).unwrap();
            let (q, r) = point_to_hex(p[0], p[1], size);
            let verts = ring(q, r, size, (0.0, 0.0));
            area(&verts.map(|[x, y]| (x, y)))
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
