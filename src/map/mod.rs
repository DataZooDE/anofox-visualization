//! Maps: data prep for `::MAP` panels, drawn by ggplot-rs's `geom_sf`
//! ([`plot`]) — see issue #420 (the hand-written SVG writer this module
//! used to describe here is gone; #318 tracks the same move for every
//! other panel kind).
//!
//! A map panel's rows carry WKT (`::MAP`), a region code looked up in the
//! built-in boundary pack (`::REGION`, see [`boundaries`]) or a point
//! (`::LAT` / `::LON`); optionally a backdrop (`::BASEMAP`: WKT or a pack
//! layer such as `'world'`), a measure or a `::CATEGORY` to colour by, a
//! `::LABEL`, a layer opacity (`::ALPHA`), labels on the map (`::LABEL_TOP`,
//! `::DATALABELS`) and the map modifiers `::PROJECTION`, `::CLASSES`,
//! `::MIDPOINT`, `::RANGE` and `::BOUNDARIES`. This module:
//!
//! 1. resolves each row's geometry once (WKT via
//!    `ggplot_rs::spatial::parse_wkt`, a pack region, or a point),
//! 2. resolves the projection (`'auto'` by the data's extent) and projects
//!    every vertex, densifying long edges where the projection curves them,
//! 3. picks the legend's sized footprint and the layout corner it goes in
//!    (the data box fitted around it, or around the browser's zoom window —
//!    [`layout`]/[`frame_of`], still this module's own job: [`plot::build`]
//!    only draws the window these hand it),
//! 4. resolves the fill (a themed sequential/diverging ramp, continuous or
//!    classed, or the theme palette) and the graduated-symbol/flow/hexbin
//!    models `plot::build` turns into `GeomSf`/`geom_curve`/`geom_hex`
//!    layers,
//! 5. composes the one-sentence summary (`data-summary`) and the
//!    attribution/scale-bar text [`plot::build`] draws via
//!    `annotation_attribution`/`annotation_scale`.
//!
//! Each feature mark carries `data-row`, `data-series` (its `::CATEGORY`,
//! else its `::LABEL`), `data-value` (the raw measure), `data-label`,
//! `data-x` (its `::XAXIS` — a `::STORY` step key — else, when it has no
//! label, the row) and, when classed, `data-class` (1-based), plus a
//! `<title>` — written by [`plot::build`] via `Aes::data_attr`/`GeomSf`'s own
//! tooltip now, not hand-formatted here; the root carries `data-map`,
//! `data-proj` and `data-classes` (the class labels as JSON).

mod bivariate;
pub(crate) mod boundaries;
pub(crate) mod classes;
mod facets;
mod fill;
mod flows;
mod frames;
pub(crate) mod hexbin;
mod labels;
mod legend;
mod overlay;
mod plot;
pub(crate) mod project;
mod spark;
mod symbols;
mod wkt;

use crate::format::ValueFmt;
use crate::theme::{contrast, mix};
use crate::{
    cell_num, find_role, legend_position_of, role_num, role_str, value_fmt, value_str, Column,
    Kind, Ramp, RenderOptions, Role,
};
use bivariate::Bivar;
use classes::{Classes, Method};
use flows::Flows;
use ggplot_rs::prelude::Value;
use ggplot_rs::spatial::{parse_wkt, Geometry};
use hexbin::Hexbin;
use legend::{legend_of, legend_title, LegendBox};
use project::{Extent, Proj, Projection};
use symbols::Symbols;

type Rgb = (u8, u8, u8);

/// Drawn marks above which a render warning suggests aggregating.
pub(crate) const MARK_WARN: usize = 100_000;
/// Legend and summary text size (as the plotting engine's legends).
const FONT: f64 = 8.8;
/// Floor size (px) for the pack attribution strip's `GeomAttribution::fit`
/// (`plot.rs`, ggplot-rs 0.31.4, #546) — the smallest this crate considers
/// still legible for the BKG/VG2500-style licence note. Shared with
/// [`foot_of`]'s own `size` baseline below, which must stay at or above
/// this (`fit`'s `min_size` clamps to `text_size`, never enlarging).
pub(super) const ATTRIBUTION_FIT_FLOOR: f64 = 7.0;

/// One drawable part of a feature, in lon/lat or projected units.
#[derive(Clone, Debug)]
pub(crate) enum Shape {
    /// One polygon: its exterior ring, then its holes.
    Area(Vec<Vec<[f64; 2]>>),
    Line(Vec<[f64; 2]>),
    Point([f64; 2]),
}

pub(crate) fn shapes(g: &Geometry, out: &mut Vec<Shape>) {
    match g {
        Geometry::Empty => {}
        Geometry::Point(c) => out.push(Shape::Point(*c)),
        Geometry::MultiPoint(cs) => out.extend(cs.iter().map(|c| Shape::Point(*c))),
        Geometry::LineString(cs) => out.push(Shape::Line(cs.clone())),
        Geometry::MultiLineString(ls) => out.extend(ls.iter().map(|l| Shape::Line(l.clone()))),
        Geometry::Polygon(rings) => out.push(Shape::Area(rings.clone())),
        Geometry::MultiPolygon(ps) => out.extend(ps.iter().map(|p| Shape::Area(p.clone()))),
        Geometry::Collection(gs) => gs.iter().for_each(|g| shapes(g, out)),
    }
}

fn for_coords(s: &Shape, f: &mut impl FnMut([f64; 2])) {
    match s {
        Shape::Area(rings) => rings.iter().flatten().for_each(|c| f(*c)),
        Shape::Line(cs) => cs.iter().for_each(|c| f(*c)),
        Shape::Point(c) => f(*c),
    }
}

/// Project one shape (vertices without an image are dropped).
fn project_shape(s: &Shape, p: &Proj) -> Option<Shape> {
    let path = |cs: &[[f64; 2]]| -> Vec<[f64; 2]> {
        let dense;
        let cs = if p.curves() {
            dense = project::densify(cs, 1.0);
            &dense[..]
        } else {
            cs
        };
        cs.iter().filter_map(|c| p.forward(*c)).collect()
    };
    match s {
        Shape::Point(c) => p.forward(*c).map(Shape::Point),
        Shape::Line(cs) => {
            Some(Shape::Line(path(cs))).filter(|s| matches!(s, Shape::Line(v) if v.len() > 1))
        }
        Shape::Area(rings) => {
            let rings: Vec<_> = rings
                .iter()
                .map(|r| path(r))
                .filter(|r| r.len() > 2)
                .collect();
            (!rings.is_empty()).then_some(Shape::Area(rings))
        }
    }
}

/// The WKT column of a map: the first `::MAP`-role column holding text.
/// (`::CHOROPLETH` is an alias of `::MAP`, so `v::CHOROPLETH` next to a
/// geometry is a *second* geometry-role column — a numeric one is the measure.)
pub(crate) fn geometry(cols: &[Column]) -> Option<&Column> {
    cols.iter().find(|c| {
        c.role == Role::Geometry
            && c.values
                .iter()
                .any(|v| matches!(v, Value::Str(s) if !s.is_empty() && crate::num_str(s).is_none()))
    })
}

/// Parsed geometry per row (`None`: missing or unparseable WKT).
fn parse_column(c: Option<&Column>, n: usize) -> Vec<Vec<Shape>> {
    (0..n)
        .map(|i| {
            let mut out = Vec::new();
            if let Some(g) = c.and_then(|c| c.values.get(i)).and_then(|v| match v {
                Value::Str(s) => parse_wkt(s),
                _ => None,
            }) {
                shapes(&g, &mut out);
            }
            out
        })
        .collect()
}

/// Whether the columns make a map: WKT, region codes or a lat/lon pair.
pub(crate) fn is_map(cols: &[Column]) -> bool {
    let has =
        |r: Role| find_role(cols, r).is_some_and(|c| c.values.iter().any(|v| *v != Value::Na));
    geometry(cols).is_some()
        || has(Role::Region)
        || (has(Role::Lat) && has(Role::Lon))
        || (has(Role::Origin) && has(Role::Dest))
}

/// A `::REGION` column's codes (trimmed text; `None` when missing).
pub(crate) fn region_codes(c: &Column) -> Vec<Option<String>> {
    c.values
        .iter()
        .map(|v| match v {
            Value::Na => None,
            v => Some(value_str(v).trim().to_string()).filter(|s| !s.is_empty()),
        })
        .collect()
}

/// The pack layer a `::BOUNDARIES` / `::BASEMAP` value names, if any.
pub(crate) fn layer_name(s: &str) -> Option<&'static str> {
    let k = s.trim().to_ascii_lowercase();
    boundaries::LAYERS.iter().copied().find(|l| *l == k)
}

/// The pack layers a panel draws from (the browser fetches them before it
/// renders): its `::BOUNDARIES`, a named `::BASEMAP`, else the layers its
/// region codes look like.
#[cfg_attr(not(feature = "wasm"), allow(dead_code))]
pub(crate) fn needed_layers(cols: &[Column]) -> Vec<&'static str> {
    let mut out = Vec::new();
    let forced = role_str(cols, Role::Boundaries).and_then(|s| layer_name(&s));
    if let Some(r) = find_role(cols, Role::Region) {
        match forced {
            Some(f) => out.push(f),
            None => out.extend(boundaries::guess_layers(&region_codes(r))),
        }
    }
    if let Some(b) = role_str(cols, Role::Basemap).and_then(|s| layer_name(&s)) {
        if !out.contains(&b) {
            out.push(b);
        }
    }
    // ::ORIGIN/::DEST cells that look like region codes (not a JSON struct
    // or a WKT POINT) also guess layers, the same as ::REGION.
    let flow_codes = |r: Role| -> Vec<Option<String>> {
        find_role(cols, r)
            .map(|c| {
                c.values
                    .iter()
                    .map(|v| match v {
                        Value::Str(s) => {
                            let t = s.trim();
                            if t.starts_with('{')
                                || (t.len() >= 5 && t[..5].eq_ignore_ascii_case("POINT"))
                            {
                                None
                            } else {
                                Some(t.to_string()).filter(|s| !s.is_empty())
                            }
                        }
                        Value::Na => None,
                        v => Some(value_str(v)).filter(|s| !s.is_empty()),
                    })
                    .collect()
            })
            .unwrap_or_default()
    };
    if forced.is_none() {
        let mut codes = flow_codes(Role::Origin);
        codes.extend(flow_codes(Role::Dest));
        if codes.iter().any(Option::is_some) {
            for l in boundaries::guess_layers(&codes) {
                if !out.contains(&l) {
                    out.push(l);
                }
            }
        }
    }
    out
}

/// The rows' geometry from the map's input — WKT, region codes or lat/lon —
/// with each row's pack name ("" without one) and the pack layer used.
struct Input {
    features: Vec<Vec<Shape>>,
    names: Vec<String>,
    layer: Option<String>,
}

fn input(cols: &[Column], german: bool) -> Option<Input> {
    if let Some(g) = geometry(cols) {
        let n = g.values.len();
        return Some(Input {
            features: parse_column(Some(g), n),
            names: vec![String::new(); n],
            layer: None,
        });
    }
    if let Some(r) = find_role(cols, Role::Region) {
        let codes = region_codes(r);
        let forced = role_str(cols, Role::Boundaries).and_then(|s| layer_name(&s));
        let m = boundaries::match_codes(&codes, forced)?;
        let features = m
            .rows
            .iter()
            .map(|ids| {
                ids.iter()
                    .flat_map(|i| m.layer.regions[*i].shapes.iter().cloned())
                    .collect()
            })
            .collect();
        let names = m
            .rows
            .iter()
            .zip(&codes)
            .map(|(ids, c)| match ids.as_slice() {
                [i] => m.layer.regions[*i].name(german).to_string(),
                [] => String::new(),
                // a union (a German NUTS 2 code): its code
                _ => c.clone().unwrap_or_default(),
            })
            .collect();
        return Some(Input {
            features,
            names,
            layer: Some(m.layer.name.clone()),
        });
    }
    let (lat, lon) = (find_role(cols, Role::Lat)?, find_role(cols, Role::Lon)?);
    let n = lat.values.len().min(lon.values.len());
    let features = (0..n)
        .map(
            |i| match (cell_num(&lat.values[i]), cell_num(&lon.values[i])) {
                (Some(y), Some(x)) => vec![Shape::Point([x, y])],
                _ => Vec::new(),
            },
        )
        .collect();
    Some(Input {
        features,
        names: vec![String::new(); n],
        layer: None,
    })
}

/// A feature without its far-off parts: polygons under a tenth of its
/// largest one and more than 15° away from it (overseas territories).
fn core_parts(f: &[Shape]) -> Vec<Shape> {
    let boxed: Vec<Option<(f64, [f64; 2])>> = f
        .iter()
        .map(|s| match s {
            Shape::Area(r) => {
                let mut e = Extent::new();
                r[0].iter().for_each(|c| e.add(*c));
                (!e.is_empty()).then(|| {
                    (
                        (e.x1 - e.x0) * (e.y1 - e.y0),
                        [(e.x0 + e.x1) / 2.0, (e.y0 + e.y1) / 2.0],
                    )
                })
            }
            _ => None,
        })
        .collect();
    let Some((big, at)) = boxed
        .iter()
        .flatten()
        .copied()
        .max_by(|a, b| a.0.total_cmp(&b.0))
    else {
        return f.to_vec();
    };
    f.iter()
        .zip(&boxed)
        .filter(|(_, b)| match b {
            Some((a, c)) => !(*a < big * 0.1 && (c[0] - at[0]).hypot(c[1] - at[1]) > 15.0),
            None => true,
        })
        .map(|(s, _)| s.clone())
        .collect()
}

/// The backdrop: a pack layer named by `::BASEMAP` (a world backdrop leaves
/// Antarctica out), else the column's WKT.
fn backdrop(cols: &[Column], rows: usize) -> (Vec<Vec<Shape>>, Option<String>) {
    let b = find_role(cols, Role::Basemap);
    if let Some(l) = role_str(cols, Role::Basemap)
        .and_then(|s| layer_name(&s))
        .and_then(boundaries::layer)
    {
        let regions = l
            .regions
            .iter()
            .filter(|r| !(l.name.starts_with("world") && r.key == "ATA"))
            .map(|r| r.shapes.clone())
            .collect();
        return (regions, Some(l.name.clone()));
    }
    (parse_column(b, rows), None)
}

/// What a map is coloured by.
enum Fill {
    /// A measure: per-row values and the colour scale.
    Measure(Box<ValueScale>),
    /// A `::CATEGORY`: the level of each row and the legend entries.
    Category {
        of_row: Vec<Option<usize>>,
        levels: Vec<(String, Rgb)>,
    },
    /// Nothing: a light brand tint.
    Plain(Rgb),
    /// `::BIVARIATE` (M2d): two measures, each in tertiles — a 3×3 palette.
    Bivariate(Box<Bivar>),
}

/// The colour scale of a measure.
#[derive(Clone)]
struct ValueScale {
    /// The value of each row (`None` = no data).
    values: Vec<Option<f64>>,
    ramp: Ramp,
    /// The diverging centre (the ramp runs over `value - mid`).
    mid: f64,
    diverging: bool,
    /// Where a sequential ramp starts (a quarter in over a basemap, so pale
    /// points don't vanish into it).
    start: f64,
    /// The colour domain (after a `::RANGE` cap).
    lo: f64,
    hi: f64,
    /// Values beyond the `::RANGE` cap on either side.
    capped_lo: bool,
    capped_hi: bool,
    classes: Option<(Method, Classes, Vec<Rgb>)>,
    fmt: ValueFmt,
    /// The legend title: the measure's name and unit ("Revenue (€)").
    title: Option<String>,
}

impl ValueScale {
    fn color_at(&self, v: f64) -> Rgb {
        let v = v.clamp(self.lo, self.hi);
        if self.diverging {
            return self.ramp.color_of(v - self.mid);
        }
        let span = self.hi - self.lo;
        let p = if span > 0.0 {
            (v - self.lo) / span
        } else {
            1.0
        };
        self.ramp.color_at(self.start + (1.0 - self.start) * p)
    }

    /// The colour and class (0-based) of a value.
    fn color(&self, v: f64) -> (Rgb, Option<usize>) {
        match &self.classes {
            Some((_, c, colors)) => {
                let i = c.index(v.clamp(self.lo, self.hi));
                (colors[i], Some(i))
            }
            None => (self.color_at(v), None),
        }
    }
}

/// The parsed modifiers and columns of a map panel.
struct Model<'a> {
    rows: usize,
    features: Vec<Vec<Shape>>,
    base: Vec<Vec<Shape>>,
    /// Each row's name from the pack (its `::LABEL` wins).
    names: Vec<String>,
    /// The features without their far-off parts, when some have them (the
    /// map is fitted to these).
    core: Option<Vec<Vec<Shape>>>,
    /// The pack layers drawn (they need an attribution line).
    layers: Vec<String>,
    /// The lon/lat extent (for the scale bar).
    lonlat: Extent,
    label: Option<&'a Column>,
    category: Option<&'a Column>,
    alpha: f64,
    projection: Projection,
    proj: Proj,
    fill: Fill,
    /// Graduated symbols (`::SIZE`), when it applies (M2a).
    symbols: Option<Symbols>,
    /// A flow map (`::ORIGIN`/`::DEST`), when it applies (M2b).
    flows: Option<Flows>,
    /// Hexagonal density binning (`::HEXBIN`), when it applies (M2c) —
    /// takes over the point-drawing pass; `::SIZE` is ignored alongside it.
    hexbin: Option<Hexbin>,
    /// Whether `base` counts toward the fitted extent — true unless a
    /// symbol map's backdrop was built automatically from a region layer
    /// (D4): that backdrop isn't used for fitting, only an explicit
    /// `::BASEMAP` is.
    base_fitted: bool,
    /// Review of #595 BLOCKER 1: a flow map's *projected* fit box, built
    /// from every resolved endpoint (`RawFlows::all_points`) — present
    /// this frame or not — not just the ones `flows` actually draws.
    /// `bbox()` reads this instead of re-deriving from `flows` so the
    /// zoom-to-fit window (not just the projection choice) stays frame-
    /// stable too: `flows` alone drawing fewer arcs on a quieter frame
    /// must never zoom the map in around them. `None` outside a flow map.
    flow_bbox: Option<(f64, f64, f64, f64)>,
}

/// The `::RANGE 'lo,hi'` of a map (a colour-domain cap).
fn range_of(cols: &[Column]) -> Option<(f64, f64)> {
    let s = find_role(cols, Role::Range)?
        .values
        .iter()
        .find(|v| !matches!(v, Value::Na))
        .map(value_str)?;
    let p: Vec<f64> = s
        .trim_matches(|c| c == '[' || c == ']')
        .split([',', ';'])
        .filter_map(|t| t.trim().parse::<f64>().ok())
        .filter(|f| f.is_finite())
        .collect();
    match p.as_slice() {
        [a, b] if a != b => Some((a.min(*b), a.max(*b))),
        _ => None,
    }
}

/// A map's measure column: the first chart-kind column that isn't a
/// per-feature `::SPARKLINE`/`::SPARKBAR` list (M3b, #642 — those draw in
/// the tooltip, never colour the map).
pub(crate) fn measure_col(cols: &[Column]) -> Option<&Column> {
    cols.iter().find(
        |c| matches!(c.role, Role::Value(k) if !matches!(k, Kind::Sparkline | Kind::Sparkbar)),
    )
}

/// A map's tooltip trend column (`list(..)::SPARKLINE` / `::SPARKBAR`, M3b).
pub(crate) fn spark_col(cols: &[Column]) -> Option<&Column> {
    cols.iter()
        .find(|c| matches!(c.role, Role::Value(k) if matches!(k, Kind::Sparkline | Kind::Sparkbar)))
}

/// The measure column of a map: a chart-kind column, or a numeric second
/// `::MAP`/`::CHOROPLETH` column (`region::MAP, sales::CHOROPLETH`).
fn measure_of(cols: &[Column], geom: Option<&Column>) -> Option<Column> {
    let numeric_geometry = cols
        .iter()
        .find(|c| {
            c.role == Role::Geometry
                && geom.is_none_or(|g| !std::ptr::eq(*c, g))
                && c.values.iter().any(|v| cell_num(v).is_some())
                && c.values
                    .iter()
                    .all(|v| *v == Value::Na || cell_num(v).is_some())
        })
        .map(|c| {
            let vals = c
                .values
                .iter()
                .map(|v| cell_num(v).map_or(Value::Na, Value::Float))
                .collect();
            Column::new(c.name.clone(), Role::Value(Kind::Bar), vals)
        });
    measure_col(cols)
        .cloned()
        .or(numeric_geometry)
        .filter(|c| c.values.iter().any(|v| v.as_f64().is_some()))
}

impl<'a> Model<'a> {
    /// `present` (M2e-B, D20): on a `::FRAME` map, which view rows (one per
    /// feature) actually had a row in the frame being built — threaded
    /// through to [`hexbin::of`] and [`flows::of`] (review of #595,
    /// BLOCKER 1), the places frame presence needs anything beyond the
    /// per-row `Value::Na` `view` already leaves behind. `None` outside a
    /// frame map.
    ///
    /// `shared` (review of #595, SHOULD-FIX 6): the quantities a frame map
    /// decides once over every frame rather than its own subset — a
    /// flow's fixed-width/has-size choice and vmax, and whether the map is
    /// `::BIVARIATE` at all. `None` outside a frame map (same reasoning as
    /// `present`: the only frame there is *is* every value there is).
    fn new(
        o: &RenderOptions,
        cols: &'a [Column],
        present: Option<&[bool]>,
        shared: Option<&frames::Shared>,
    ) -> Option<Model<'a>> {
        let german = o.locale.tag.starts_with("de");
        let is_flow = find_role(cols, Role::Origin)
            .is_some_and(|c| c.values.iter().any(|v| *v != Value::Na))
            && find_role(cols, Role::Dest)
                .is_some_and(|c| c.values.iter().any(|v| *v != Value::Na));
        if is_flow {
            let flow_forced = shared.map(|s| (s.flow_has_size, s.flow_fixed));
            let raw = flows::of(cols, german, present, flow_forced)?;
            let rows = raw.rows;
            let (mut base, mut base_layer) = backdrop(cols, rows);
            let has_explicit_basemap = find_role(cols, Role::Basemap).is_some();
            // D4: flow ends from region codes, no explicit ::BASEMAP -> the
            // whole matched layer as an unfitted backdrop (M2a's region_backdrop).
            let flow_backdrop = !has_explicit_basemap
                && raw.layer.as_deref().is_some_and(|l| {
                    if let Some(layer) = boundaries::layer(l) {
                        base = layer
                            .regions
                            .iter()
                            .filter(|r| !(layer.name.starts_with("world") && r.key == "ATA"))
                            .map(|r| r.shapes.clone())
                            .collect();
                        base_layer = Some(layer.name.clone());
                        true
                    } else {
                        false
                    }
                });
            let base_fitted = !flow_backdrop;
            let mut layers: Vec<String> = Vec::new();
            if let Some(l) = &raw.layer {
                layers.push(l.clone());
            }
            if let Some(l) = &base_layer {
                if !layers.contains(l) {
                    layers.push(l.clone());
                }
            }
            let projection = role_str(cols, Role::Projection)
                .and_then(|s| Projection::parse(&s))
                .unwrap_or(Projection::Auto);
            let mut extent = Extent::new();
            // Review #595 BLOCKER 1 / re-review R1: on a `::FRAME` map
            // (`present.is_some()`), every resolved, non-self-flow
            // endpoint (`all_points`, present this frame or not) — D20's
            // extent/projection are computed over every frame at once.
            // Outside a frame map, the *original* rule, unchanged: a
            // drawn flow's own endpoints (`raw.flows`, already self-flow-
            // filtered) — not `all_points`, which doesn't additionally
            // drop a `::SIZE`-invalid or beyond-`FLOW_CAP` flow the way
            // `raw.flows`/`flows::project` do, so using it unconditionally
            // zoomed a plain (non-frame) flow map out around a flow that
            // was never actually drawn.
            if present.is_some() {
                for p in &raw.all_points {
                    extent.add(*p);
                }
            } else {
                for f in &raw.flows {
                    extent.add(f.o);
                    extent.add(f.d);
                }
            }
            if base_fitted {
                for s in base.iter().flatten() {
                    for_coords(s, &mut |c| extent.add(c));
                }
            }
            let (projection, proj) = Proj::setup(projection, &extent);
            let lonlat = extent;
            // Review #595 BLOCKER 1 / re-review R1: the *projected* fit
            // box from every resolved (non-self-flow) endpoint, before
            // `raw` moves into `flows::project` (which keeps only this
            // frame's drawn flows) — frame-map only (`bbox()` falls back
            // to the drawn `Flows` itself, the original rule, when this is
            // `None`), for the same reason as the extent above.
            let flow_bbox = if present.is_some() {
                let mut e = Extent::new();
                for p in &raw.all_points {
                    if let Some(pp) = proj.forward(*p) {
                        e.add(pp);
                    }
                }
                (!e.is_empty()).then_some((e.x0, e.y0, e.x1, e.y1))
            } else {
                None
            };
            let base: Vec<Vec<Shape>> = base
                .iter()
                .map(|f| f.iter().filter_map(|s| project_shape(s, &proj)).collect())
                .collect();
            let flows_model = flows::project(
                raw,
                &proj,
                shared.and_then(|s| s.size_limits).map(|(_, hi)| hi),
            );
            let category = find_role(cols, Role::Category)
                .filter(|c| c.values.iter().any(|v| *v != Value::Na));
            let measure = measure_of(cols, None);
            let has_base = has_explicit_basemap || flow_backdrop;
            let fill = match (&measure, category) {
                (Some(m), _) => Fill::Measure(Box::new(value_scale(o, cols, m, has_base, rows))),
                (None, Some(c)) => category_fill(o, cols, c, rows),
                (None, None) => Fill::Plain(crate::wash(o, o.brand(), 0.55)),
            };
            return Some(Model {
                rows,
                features: vec![Vec::new(); rows],
                base,
                names: vec![String::new(); rows],
                core: None,
                layers,
                lonlat,
                label: find_role(cols, Role::Label),
                category,
                alpha: role_num(cols, Role::Alpha)
                    .map(|a| a.clamp(0.05, 1.0))
                    .unwrap_or(1.0),
                projection,
                proj,
                fill,
                symbols: None,
                flows: Some(flows_model),
                hexbin: None,
                base_fitted,
                flow_bbox,
            });
        }
        let inp = input(cols, german)?;
        let rows = inp.features.len();
        let (mut base, mut base_layer) = backdrop(cols, rows);
        let has_explicit_basemap = find_role(cols, Role::Basemap).is_some();
        let want_symbols = find_role(cols, Role::Size)
            .is_some_and(|c| c.values.iter().any(|v| cell_num(v).is_some()));
        // D4: a symbol map built from region codes, with no explicit
        // ::BASEMAP, draws the whole matched layer as an unfitted backdrop
        // instead of colouring the rows' own regions.
        let region_backdrop = want_symbols
            && !has_explicit_basemap
            && inp.layer.as_deref().is_some_and(|l| {
                if let Some(layer) = boundaries::layer(l) {
                    base = layer
                        .regions
                        .iter()
                        .filter(|r| !(layer.name.starts_with("world") && r.key == "ATA"))
                        .map(|r| r.shapes.clone())
                        .collect();
                    base_layer = Some(layer.name.clone());
                    true
                } else {
                    false
                }
            });
        let base_fitted = !region_backdrop;
        let mut layers: Vec<String> = Vec::new();
        if let Some(l) = &inp.layer {
            layers.push(l.clone());
        }
        if let Some(l) = &base_layer {
            if !layers.contains(l) {
                layers.push(l.clone());
            }
        }
        let features = inp.features;
        // an unknown name draws as 'auto' (lint: map/unknown-projection)
        let projection = role_str(cols, Role::Projection)
            .and_then(|s| Projection::parse(&s))
            .unwrap_or(Projection::Auto);
        // a pack region's far-off parts (French Guiana for France) are drawn
        // but don't stretch the map
        let core: Option<Vec<Vec<Shape>>> = inp.layer.as_ref().and_then(|_| {
            let core: Vec<Vec<Shape>> = features.iter().map(|f| core_parts(f)).collect();
            (core.iter().map(Vec::len).sum::<usize>()
                < features.iter().map(Vec::len).sum::<usize>())
            .then_some(core)
        });
        let mut extent = Extent::new();
        let fit_base: &[Vec<Shape>] = if base_fitted { &base } else { &[] };
        for s in core
            .as_ref()
            .unwrap_or(&features)
            .iter()
            .chain(fit_base)
            .flatten()
        {
            for_coords(s, &mut |c| extent.add(c));
        }
        let (projection, proj) = Proj::setup(projection, &extent);
        let core = core.map(|c| {
            c.iter()
                .map(|f| f.iter().filter_map(|s| project_shape(s, &proj)).collect())
                .collect()
        });
        let lonlat = extent;
        let features: Vec<Vec<Shape>> = features
            .iter()
            .map(|f| f.iter().filter_map(|s| project_shape(s, &proj)).collect())
            .collect();
        let base = base
            .iter()
            .map(|f| f.iter().filter_map(|s| project_shape(s, &proj)).collect())
            .collect();
        // ::HEXBIN (M2c) takes over the point pass entirely — no graduated
        // symbols alongside it (::SIZE is ignored when both are present).
        let hexbin = hexbin::of(
            cols,
            &proj,
            present,
            match shared {
                Some(s) if !s.hex_points.is_empty() => hexbin::Grid::Over(&s.hex_points),
                _ => hexbin::Grid::Own,
            },
        );
        let symbols = if hexbin.is_none() {
            symbols::of(cols, &features)
        } else {
            None
        };
        let measure = measure_of(cols, geometry(cols));
        let category =
            find_role(cols, Role::Category).filter(|c| c.values.iter().any(|v| *v != Value::Na));
        // a backdrop (explicit, or the automatic one under a symbol map)
        // makes the sequential ramp start a quarter in, so pale symbols
        // don't vanish into it
        let has_base = has_explicit_basemap || region_backdrop;
        // `::BIVARIATE` (M2d): a second measure, crossed with the first in
        // tertiles — only when there's a measure to cross it with, and only
        // over areas/points (symbols take the point pass; hexbin takes over
        // entirely); both are checked by `map_diags`, which warns instead of
        // silently drawing a plain/measure map.
        // Review #595 SHOULD-FIX 6: whether this is a bivariate map at all
        // is decided once over every frame (`shared.is_bivariate`), not
        // just whether frame k's own `b` values happen to have a finite
        // one — otherwise a frame whose `b` is all NA would fall back to
        // a plain measure ramp (a different legend) instead of the 3×3
        // grid with every feature "no data".
        let force_bivariate = shared.is_some_and(|s| s.is_bivariate);
        let bivariate_col = find_role(cols, Role::Bivariate)
            .filter(|c| force_bivariate || c.values.iter().any(|v| cell_num(v).is_some()));
        let fill = if let Some(hb) = &hexbin {
            hexbin::fill_of(o, cols, hb)
        } else if let (Some(m), Some(b), true) = (&measure, &bivariate_col, symbols.is_none()) {
            match bivariate::of(o, cols, m, b, rows, force_bivariate) {
                Some(biv) => Fill::Bivariate(Box::new(biv)),
                None => Fill::Measure(Box::new(value_scale(o, cols, m, has_base, rows))),
            }
        } else {
            match (&measure, category) {
                (Some(m), _) => Fill::Measure(Box::new(value_scale(o, cols, m, has_base, rows))),
                (None, Some(c)) => category_fill(o, cols, c, rows),
                (None, None) => Fill::Plain(crate::wash(o, o.brand(), 0.55)),
            }
        };
        Some(Model {
            rows,
            features,
            base,
            names: inp.names,
            core,
            layers,
            lonlat,
            label: find_role(cols, Role::Label),
            category,
            alpha: role_num(cols, Role::Alpha)
                .map(|a| a.clamp(0.05, 1.0))
                .unwrap_or(1.0),
            projection,
            proj,
            fill,
            symbols,
            flows: None,
            hexbin,
            base_fitted,
            flow_bbox: None,
        })
    }

    /// The projected box of everything drawn (data and backdrop; the
    /// backdrop only when it's fitted — [`Model::base_fitted`]).
    fn bbox(&self) -> Option<(f64, f64, f64, f64)> {
        let mut e = Extent::new();
        if let Some(fl) = &self.flows {
            // Review #595 BLOCKER 1 / re-review R1: `flow_bbox` (every
            // resolved endpoint, present this frame or not) on a `::FRAME`
            // map — not `fl`'s own drawn-only arcs, which would zoom the
            // fit in/out per frame. `None` outside a frame map (the
            // original rule, unaffected by BLOCKER 1's fix): the drawn
            // flows themselves, same as before `::FRAME` existed.
            if let Some((x0, y0, x1, y1)) = self.flow_bbox {
                e.add([x0, y0]);
                e.add([x1, y1]);
            } else {
                for f in &fl.flows {
                    e.add(f.o);
                    e.add(f.d);
                }
            }
        } else {
            let features = self.core.as_ref().unwrap_or(&self.features);
            for s in features.iter().flatten() {
                for_coords(s, &mut |c| e.add(c));
            }
        }
        let base: &[Vec<Shape>] = if self.base_fitted { &self.base } else { &[] };
        for s in base.iter().flatten() {
            for_coords(s, &mut |c| e.add(c));
        }
        (!e.is_empty()).then_some((e.x0, e.y0, e.x1, e.y1))
    }

    /// A row's label: its `::LABEL`, else its name from the pack.
    fn label_of(&self, i: usize) -> String {
        match self.text(self.label, i) {
            l if l.is_empty() => self.names.get(i).cloned().unwrap_or_default(),
            l => l,
        }
    }

    fn text(&self, c: Option<&Column>, i: usize) -> String {
        match c.and_then(|c| c.values.get(i)) {
            None | Some(Value::Na) => String::new(),
            Some(v) => value_str(v),
        }
    }
}

/// RGB (0..255) to HSL (`h` in 0..360, `s`/`l` in 0..1).
fn rgb_to_hsl(c: Rgb) -> (f64, f64, f64) {
    let (r, g, b) = (c.0 as f64 / 255.0, c.1 as f64 / 255.0, c.2 as f64 / 255.0);
    let (cmax, cmin) = (r.max(g).max(b), r.min(g).min(b));
    let l = (cmax + cmin) / 2.0;
    let d = cmax - cmin;
    if d < 1e-9 {
        return (0.0, 0.0, l);
    }
    let s = d / (1.0 - (2.0 * l - 1.0).abs());
    let h = 60.0
        * if cmax == r {
            ((g - b) / d).rem_euclid(6.0)
        } else if cmax == g {
            (b - r) / d + 2.0
        } else {
            (r - g) / d + 4.0
        };
    (h, s, l)
}

/// HSL (`h` in 0..360, `s`/`l` in 0..1) to RGB.
fn hsl_to_rgb(h: f64, s: f64, l: f64) -> Rgb {
    if s <= 1e-9 {
        let v = (l.clamp(0.0, 1.0) * 255.0).round() as u8;
        return (v, v, v);
    }
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - ((h / 60.0).rem_euclid(2.0) - 1.0).abs());
    let m = l - c / 2.0;
    let (r1, g1, b1) = match (h.rem_euclid(360.0) / 60.0) as i32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let to_u8 = |v: f64| ((v + m).clamp(0.0, 1.0) * 255.0).round() as u8;
    (to_u8(r1), to_u8(g1), to_u8(b1))
}

/// #311: lift `c`'s HSL *lightness* — hue and saturation held fixed, so the
/// colour never greys out — until it clears a WCAG non-text contrast floor
/// (`min`, 1.4.11's 3:1) against `bg`. A binary search over `l` (increasing
/// only: every caller here lifts a colour that reads too dark against a
/// dark `bg`). `c` unchanged once it already clears `min`, so a light theme
/// (or an already-light colour) never gets this treatment; a colour whose
/// hue/saturation can't reach `min` even at full lightness returns as light
/// as it can go (white has the lowest achievable contrast gap, never none).
fn lift_lightness_for_contrast(c: Rgb, bg: Rgb, min: f64) -> Rgb {
    if contrast(c, bg) >= min {
        return c;
    }
    let (h, s, l) = rgb_to_hsl(c);
    let (mut lo, mut hi) = (l, 1.0_f64);
    for _ in 0..20 {
        let mid = (lo + hi) / 2.0;
        if contrast(hsl_to_rgb(h, s, mid), bg) >= min {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    hsl_to_rgb(h, s, hi)
}

/// The colour scale of a measure: classes when `::CLASSES` is set, a
/// diverging ramp when the values span the midpoint (0, or `::MIDPOINT`),
/// else sequential; `::RANGE` caps the domain.
fn value_scale(
    o: &RenderOptions,
    cols: &[Column],
    m: &Column,
    over_base: bool,
    rows: usize,
) -> ValueScale {
    let values: Vec<Option<f64>> = (0..rows)
        .map(|i| {
            m.values
                .get(i)
                .and_then(Value::as_f64)
                .filter(|f| f.is_finite())
        })
        .collect();
    let finite: Vec<f64> = values.iter().flatten().copied().collect();
    let (min, max) = finite
        .iter()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), v| {
            (a.min(*v), b.max(*v))
        });
    let (lo, hi) = match range_of(cols) {
        Some((a, b)) => (a, b),
        None => (min, max),
    };
    let explicit_mid = role_num(cols, Role::Midpoint);
    let mid = explicit_mid.unwrap_or(0.0);
    let diverging = explicit_mid.is_some() || (lo < mid && hi > mid);
    let ramp = if diverging {
        // the ramp diverges only across its zero: keep both arms present
        let span = (hi - lo).abs().max(1e-9);
        Ramp::new(
            o,
            o.theme.seq_low(),
            (lo - mid).min(-1e-9 * span),
            (hi - mid).max(1e-9 * span),
        )
    } else {
        Ramp::new(o, o.theme.seq_low(), lo, hi)
    };
    let kind = match m.role {
        Role::Value(k) => k,
        _ => Kind::Bar,
    };
    let mut scale = ValueScale {
        values,
        ramp,
        mid,
        diverging,
        // (in the dark variant the low end recedes into the card: start later)
        start: match (over_base && !diverging, o.theme.variant) {
            (false, _) => 0.0,
            (true, crate::theme::Variant::Dark) => 0.4,
            (true, _) => 0.25,
        },
        lo,
        hi,
        capped_lo: min < lo,
        capped_hi: max > hi,
        classes: None,
        fmt: value_fmt(cols, kind, &m.values),
        title: None,
    };
    scale.title = legend_title(&m.name, &scale.fmt);
    // a bad spec draws a continuous ramp (lint: map/bad-classes)
    if let Some(spec) = role_str(cols, Role::Classes) {
        match Method::parse(&spec) {
            Err(_) => {}
            Ok(method) => {
                let clamped: Vec<f64> = finite.iter().map(|v| v.clamp(lo, hi)).collect();
                if let Some(mut c) = Classes::build(&method, &clamped) {
                    c.open_lo |= scale.capped_lo;
                    c.open_hi |= scale.capped_hi;
                    let k = c.len();
                    // sequential steps spread evenly in lightness: pale → brand
                    // → a clearly deeper brand (the continuous ramp's top end
                    // is too close to the brand to tell two classes apart);
                    // a theme's own sequential pair is used as given
                    let t = &o.theme;
                    let deep = mix(o.brand(), t.colors.ink, 0.5);
                    let step = |p: f64| -> Rgb {
                        let p = scale.start.max(0.12) + (1.0 - scale.start.max(0.12)) * p;
                        if t.seq_pair().is_some() {
                            scale.ramp.color_at(p)
                        } else if p <= 0.6 {
                            mix(t.seq_low(), o.brand(), p / 0.6)
                        } else {
                            mix(o.brand(), deep, (p - 0.6) / 0.4)
                        }
                    };
                    let dark = t.variant == crate::theme::Variant::Dark;
                    let colors: Vec<Rgb> = if dark && !scale.diverging && k > 1 {
                        // #311: a classed choropleth/hexbin's low-value
                        // classes (this (non-diverging) ramp's darkest) read
                        // as near-invisible against a dark page. Lifting
                        // each class independently broke the ramp's
                        // monotonic lightness (a lifted low class could end
                        // up as light as, or lighter than, its unlifted
                        // neighbour) — instead, lift only the low end's
                        // *lightness* (hue/saturation held fixed, so it
                        // never greys out) to clear the floor, then
                        // re-derive every class as a plain interpolation
                        // between that lifted low end and the (already
                        // high-contrast) high end, the way the planned
                        // bivariate palette does (D10, docs/plans/plan-maps.md
                        // §15: "lifted over the card colour in dark").
                        // Per-channel interpolation between two fixed
                        // endpoints is monotonic in each channel, and WCAG
                        // relative luminance is a non-negative weighted sum
                        // of (gamma-corrected, monotonic) channels, so the
                        // resulting ramp is provably monotonic in luminance.
                        let low = lift_lightness_for_contrast(step(0.0), t.colors.card, 3.0);
                        let high = lift_lightness_for_contrast(step(1.0), t.colors.card, 3.0);
                        (0..k)
                            .map(|i| mix(low, high, i as f64 / (k - 1) as f64))
                            .collect()
                    } else {
                        (0..k)
                            .map(|i| {
                                let c = if scale.diverging {
                                    let (a, b) = c.bounds(i);
                                    scale.color_at((a + b) / 2.0)
                                } else if k == 1 {
                                    step(1.0)
                                } else {
                                    step(i as f64 / (k - 1) as f64)
                                };
                                if dark {
                                    lift_lightness_for_contrast(c, t.colors.card, 3.0)
                                } else {
                                    c
                                }
                            })
                            .collect()
                    };
                    scale.classes = Some((method, c, colors));
                }
            }
        }
    }
    scale
}

/// Colours by `::CATEGORY`: the theme palette over the levels in sorted
/// order (so a level keeps its colour across charts); a level that is
/// itself `#rrggbb` is drawn in that colour. A `::COLORS` level map
/// (`'depot:neutral; late stop:bad'`) pins the levels it names to their
/// status colour or `#rrggbb`, first in the legend in the written order;
/// the other levels follow, sorted, in palette order — slot 0 up, as if the
/// named levels weren't there, so a named level that is missing from one
/// map doesn't shift the colours of the rest.
fn category_fill(o: &RenderOptions, cols: &[Column], c: &Column, rows: usize) -> Fill {
    let levels = crate::distinct_labels(c);
    let levels: Vec<String> = levels.into_iter().filter(|l| !l.is_empty()).collect();
    let named: Vec<(String, crate::thresholds::LevelColor)> = find_role(cols, Role::GaugeColors)
        .and_then(|c| c.values.first())
        .map(value_str)
        .filter(|s| crate::thresholds::is_level_map(s))
        .and_then(|s| crate::thresholds::parse_level_colors(&s).ok())
        .unwrap_or_default();
    let mut colors: Vec<(String, Rgb)> = named
        .iter()
        .filter(|(l, _)| levels.contains(l))
        .map(|(l, lc)| (l.clone(), lc.rgb(&o.theme)))
        .collect();
    colors.extend(
        levels
            .iter()
            .filter(|l| !named.iter().any(|(m, _)| m == *l))
            .enumerate()
            .map(|(i, l)| {
                let col = crate::parse_rgb(l).unwrap_or_else(|| o.theme.color(i));
                (l.clone(), col)
            }),
    );
    let levels: Vec<String> = colors.iter().map(|(l, _)| l.clone()).collect();
    let of_row = (0..rows)
        .map(|i| {
            let v = c.values.get(i).map(value_str).unwrap_or_default();
            levels.iter().position(|l| *l == v)
        })
        .collect();
    Fill::Category {
        of_row,
        levels: colors,
    }
}

/// Screen transform: projected `(x, y)` (y up) → panel pixels.
#[derive(Clone, Copy, Debug)]
struct Frame {
    s: f64,
    ox: f64,
    oy: f64,
    x0: f64,
    y1: f64,
}

impl Frame {
    /// Fit the box `(x0, y0, x1, y1)` into the rectangle `(ax, ay, aw, ah)`,
    /// centred, keeping the aspect.
    fn fit(b: (f64, f64, f64, f64), area: (f64, f64, f64, f64)) -> Frame {
        let (x0, y0, x1, y1) = b;
        let (ax, ay, aw, ah) = area;
        let (bw, bh) = ((x1 - x0).max(1e-12), (y1 - y0).max(1e-12));
        let s = (aw / bw).min(ah / bh);
        Frame {
            s,
            ox: ax + (aw - bw * s) / 2.0,
            oy: ay + (ah - bh * s) / 2.0,
            x0,
            y1,
        }
    }

    fn at(&self, [x, y]: [f64; 2]) -> (f64, f64) {
        (
            self.ox + (x - self.x0) * self.s,
            self.oy + (self.y1 - y) * self.s,
        )
    }

    /// The projected window `[x0, x1, y0, y1]` the whole `w`×`h` panel shows.
    fn window(&self, w: f64, h: f64) -> [f64; 4] {
        let x0 = self.x0 - self.ox / self.s;
        let y1 = self.y1 + self.oy / self.s;
        [x0, x0 + w / self.s, y1 - h / self.s, y1]
    }
}

/// The box with a margin (a lone point gets a small neighbourhood); flow
/// maps use a wider 8% margin so the arcs' bulge fits.
fn padded(
    (x0, y0, x1, y1): (f64, f64, f64, f64),
    planar: bool,
    margin: f64,
) -> (f64, f64, f64, f64) {
    let (w, h) = (x1 - x0, y1 - y0);
    let unit = if planar { 1.0 } else { 1_000.0 };
    let m = margin * w.max(h);
    let (mx, my) = if w <= 0.0 && h <= 0.0 {
        (unit, unit)
    } else {
        (m.max(margin * w), m.max(margin * h))
    };
    (x0 - mx, y0 - my, x1 + mx, y1 + my)
}

/// `::LEGEND` on a map: `'none'` hides it outright (`lg` zeroed so
/// [`layout`] reserves it no room); `'top'`/`'right'`/`'bottom'`/`'left'` is
/// instead returned for [`layout`] to place it on that side.
fn legend_override(
    cols: &[Column],
    lg: &mut LegendBox,
) -> Option<ggplot_rs::theme::LegendPosition> {
    match legend_position_of(cols) {
        Some(ggplot_rs::theme::LegendPosition::None) => {
            lg.w = 0.0;
            lg.h = 0.0;
            None
        }
        other => other,
    }
}

/// Where the map and its legend go when not zoomed: the data's box (with
/// its margin) fitted to the whole panel, the legend over the corner that
/// covers the fewest vertices — or, when every corner would cover some,
/// beside or above the map (whichever keeps it larger). `forced` (`::LEGEND`
/// `'top'`/`'right'`/`'bottom'`/`'left'`) skips that auto corner-pick and
/// puts the legend on the given side instead (`'none'` is handled by the
/// caller zeroing `lg` before this runs).
fn layout(
    m: &Model,
    lg: &LegendBox,
    w: f64,
    h: f64,
    forced: Option<ggplot_rs::theme::LegendPosition>,
) -> Option<(Frame, (f64, f64))> {
    let margin = if m.flows.is_some() || m.hexbin.is_some() {
        0.08
    } else {
        0.04
    };
    let b = padded(m.bbox()?, matches!(m.proj, Proj::Identity), margin);
    let full = Frame::fit(b, (2.0, 2.0, w - 4.0, h - 4.0));
    if lg.w <= 0.0 {
        return Some((full, (0.0, 0.0)));
    }
    let (lw, lh) = (lg.w, lg.h);
    use ggplot_rs::theme::LegendPosition as L;
    match forced {
        Some(L::Top) => {
            let top = Frame::fit(b, (2.0, lh + 8.0, w - 4.0, (h - lh - 10.0).max(1.0)));
            return Some((top, (4.0, 4.0)));
        }
        Some(L::Bottom) => {
            let bottom = Frame::fit(b, (2.0, 2.0, w - 4.0, (h - lh - 10.0).max(1.0)));
            return Some((bottom, (4.0, h - 4.0 - lh)));
        }
        Some(L::Left) => {
            let side = Frame::fit(b, (lw + 8.0, 2.0, (w - lw - 10.0).max(1.0), h - 4.0));
            return Some((side, (4.0, 4.0)));
        }
        Some(L::Right) => {
            let side = Frame::fit(b, (2.0, 2.0, (w - lw - 10.0).max(1.0), h - 4.0));
            return Some((side, (w - 4.0 - lw, 4.0)));
        }
        // `None` (no role) or `L::None`/`L::Inside` (not a map side): auto-pick.
        _ => {}
    }
    let corners = [
        (4.0, 4.0),
        (w - 4.0 - lw, 4.0),
        (4.0, h - 4.0 - lh),
        (w - 4.0 - lw, h - 4.0 - lh),
    ];
    // vertices under each corner (an even sample of at most ~20k)
    let total: usize = m
        .features
        .iter()
        .chain(&m.base)
        .flatten()
        .map(|s| match s {
            Shape::Area(r) => r.iter().map(Vec::len).sum(),
            Shape::Line(c) => c.len(),
            Shape::Point(_) => 1,
        })
        .sum();
    let stride = (total / 20_000).max(1);
    let mut hits = [0usize; 4];
    let mut k = 0usize;
    for s in m.features.iter().chain(&m.base).flatten() {
        for_coords(s, &mut |c| {
            k += 1;
            if !k.is_multiple_of(stride) {
                return;
            }
            let (x, y) = full.at(c);
            for (i, (cx, cy)) in corners.iter().enumerate() {
                if x >= cx - 2.0 && x <= cx + lw + 2.0 && y >= cy - 2.0 && y <= cy + lh + 2.0 {
                    hits[i] += 1;
                }
            }
        });
    }
    let best = (0..4).min_by_key(|i| hits[*i]).unwrap_or(0);
    if hits[best] == 0 {
        return Some((full, corners[best]));
    }
    let top = Frame::fit(b, (2.0, lh + 8.0, w - 4.0, (h - lh - 10.0).max(1.0)));
    let side = Frame::fit(b, (lw + 8.0, 2.0, (w - lw - 10.0).max(1.0), h - 4.0));
    Some(if side.s > top.s {
        (side, (4.0, 4.0))
    } else {
        (top, (4.0, 4.0))
    })
}

/// The map's frame: the zoom window over the whole panel, else the
/// [`layout`]'s; and where the legend goes (the same either way, so it
/// doesn't jump on the first zoom step).
fn frame_of(
    m: &Model,
    o: &RenderOptions,
    lg: &LegendBox,
    w: f64,
    h: f64,
    foot: f64,
    forced: Option<ggplot_rs::theme::LegendPosition>,
) -> Option<(Frame, (f64, f64))> {
    let (auto, at) = layout(m, lg, w, h - foot, forced)?;
    if let Some(((x0, x1), (y0, y1))) = o.zoom {
        if [x0, x1, y0, y1].iter().all(|f| f.is_finite()) && x1 > x0 && y1 > y0 {
            return Some((Frame::fit((x0, y0, x1, y1), (0.0, 0.0, w, h)), at));
        }
    }
    Some((auto, at))
}

/// The projected window `[x0, x1, y0, y1]` the panel shows when not zoomed
/// (the browser's zoom starts from it); `w`/`h` 0 = the data's box alone.
#[cfg_attr(not(feature = "wasm"), allow(dead_code))]
pub(crate) fn view_bounds(cols: &[Column], o: &RenderOptions, w: u32, h: u32) -> Option<[f64; 4]> {
    // A `::FRAME` map's extent never moves between frames (D20: every
    // feature is in every frame's view, geometry from its first row) — any
    // resolved frame's view gives the same bbox, so this reuses `build`'s
    // own view resolution rather than re-deriving which `k` to use.
    let fr = frames::of(cols);
    let view_cols: Option<Vec<Column>> = fr
        .as_ref()
        .map(|fr| frames::view(cols, fr, fr.resolve_k(o.frame)));
    let cols: &[Column] = view_cols.as_deref().unwrap_or(cols);
    // `None, None`: `present` only changes which points a hexbin cell sums
    // or which flows draw, never the grid/bbox this function reports (every
    // feature's geometry/endpoints are already stable across frames,
    // review of #595 BLOCKER 1); `shared`'s flow width/bivariate decisions
    // don't affect the bbox either.
    let m = Model::new(o, cols, None, None)?;
    if w == 0 || h == 0 {
        let (x0, y0, x1, y1) = m.bbox()?;
        return Some([x0, x1, y0, y1]);
    }
    let (w, h) = (w as f64, h as f64);
    let mut lg = legend_of(o, &m, w, h);
    let forced = legend_override(cols, &mut lg);
    let ft = foot_of(o, &m, w);
    layout(&m, &lg, w, h - ft.h, forced).map(|(f, _)| f.window(w, h))
}

/// Build a map panel's `GGPlot` and its extra root attributes
/// (`data-map`, …) — `None` when no row has geometry.
pub(crate) fn build(
    o: &RenderOptions,
    cols: &[Column],
    width: u32,
    height: u32,
) -> Option<plot::Built> {
    // `::FRAME` (M2e-A/M2e-B, #149): one result holds every frame; reduce
    // it to one row per feature with frame `k`'s values (D20) and build
    // the ordinary `Model` from that view, so every map kind renders a
    // frame unchanged — then force its fill/size/width/tertile scales onto
    // the quantities shared across every frame (D21, `frames::apply`).
    // Hexbin's own shared cell-colour domain needs `m.proj` (`m` built
    // already), so it's applied separately just below (M2e-B).
    let fr = frames::of(cols);
    // Small multiples (M3a-1, #644): `::FACET` on an area/point/symbol map
    // (not a time map — D-M3-3 — and not yet flows/hexbin/bivariate, M3a-2).
    if fr.is_none() && facet_eligible(cols) {
        if let Some(ff) = facets::of(cols) {
            let (st, present, ctx) = facets::stacked(cols, &ff);
            let m = Model::new(o, &st, Some(&present), None)?;
            let (p, attrs, mut warnings) =
                plot::build(o, &st, &m, width, height, None, None, Some(&ctx))?;
            if ctx.total > facets::MAX_FACETS {
                warnings.push(format!(
                    "map: {} ::FACET values on one map (over {}) — the first {} are drawn; filter or use a coarser grouping",
                    ctx.total,
                    facets::MAX_FACETS,
                    facets::MAX_FACETS
                ));
            }
            return Some((p, attrs, warnings));
        }
    }
    let k = fr.as_ref().map(|fr| fr.resolve_k(o.frame));
    let mut shared = fr.as_ref().map(|fr| frames::shared_of(o, cols, fr));
    let present: Option<Vec<bool>> = fr.as_ref().zip(k).map(|(fr, k)| frames::presence(fr, k));
    let view_cols: Option<Vec<Column>> =
        fr.as_ref().zip(k).map(|(fr, k)| frames::view(cols, fr, k));
    let use_cols: &[Column] = view_cols.as_deref().unwrap_or(cols);
    let mut m = Model::new(o, use_cols, present.as_deref(), shared.as_ref())?;
    if let Some(shared) = &shared {
        frames::apply(&mut m, shared);
    }
    // review of #595 BLOCKER 2: needs `&mut Shared` (also fills in
    // `shared.max_count`, U3, from the hexbin-specific distribution
    // `shared_of` deliberately skips) and `fr` (bins every frame
    // separately) — both already on hand here.
    if let (Some(fr), Some(shared)) = (&fr, &mut shared) {
        if m.hexbin.is_some() {
            frames::apply_hexbin_shared(&mut m, o, cols, fr, shared);
        }
    }
    let frame_ctx = fr
        .as_ref()
        .zip(k)
        .map(|(fr, k)| frames::FrameCtx::at(fr, k));
    let (p, attrs, mut warnings) = plot::build(
        o,
        use_cols,
        &m,
        width,
        height,
        frame_ctx.as_ref(),
        shared.as_ref(),
        None,
    )?;
    if m.rows > MARK_WARN {
        warnings.push(format!(
            "map: {} marks drawn (over {MARK_WARN}) — aggregate in SQL (e.g. to H3 cells or regions) to keep the map light",
            m.rows
        ));
    }
    if let Some(fr) = &fr {
        if fr.total_frames > frames::MAX_FRAMES {
            warnings.push(format!(
                "map: {} ::FRAME values on one map (over {}) — the last {} are kept; use a coarser period",
                fr.total_frames,
                frames::MAX_FRAMES,
                fr.labels.len()
            ));
        }
        if fr.duplicates > 0 {
            warnings.push(format!(
                "map: {} rows repeat a feature within one frame — the first row is used; aggregate per feature and frame",
                fr.duplicates
            ));
        }
    }
    Some((p, attrs, warnings))
}

/// Whether `::FACET`/`::FACET_FREE` draws small multiples on this map
/// (M3a-1): a facet column, and not a flow, hexbin or bivariate map (those
/// come in M3a-2 — lint `map/facet-type`).
pub(crate) fn facet_eligible(cols: &[Column]) -> bool {
    (find_role(cols, Role::Facet).is_some() || find_role(cols, Role::FacetFree).is_some())
        && find_role(cols, Role::Frame).is_none()
        && find_role(cols, Role::Hexbin).is_none()
        && find_role(cols, Role::Bivariate).is_none()
        && !(find_role(cols, Role::Origin).is_some() && find_role(cols, Role::Dest).is_some())
}

/// A small-multiples map's natural height at `width` (M3a-1): ggplot-rs's
/// `natural_height` for its panels (the data box's aspect, the same column
/// rule the render uses) plus room for the legend and the attribution band.
/// `None` for any other map.
#[cfg_attr(not(feature = "wasm"), allow(dead_code))]
pub(crate) fn facet_natural_height(cols: &[Column], o: &RenderOptions, width: u32) -> Option<u32> {
    if !facet_eligible(cols) {
        return None;
    }
    let ff = facets::of(cols)?;
    let (st, present, ctx) = facets::stacked(cols, &ff);
    let m = Model::new(o, &st, Some(&present), None)?;
    let planar = m.projection == project::Projection::None;
    let (x0, y0, x1, y1) = padded(m.bbox()?, planar, 0.04);
    let aspect = (y1 - y0) / (x1 - x0);
    if !(aspect.is_finite() && aspect > 0.0) {
        return None;
    }
    let w = width as f64;
    let n = ctx.levels.len();
    let ncol = ctx.ncol.unwrap_or_else(|| plot::facet_ncol(n, aspect, w));
    let (_, h) = ggplot_rs::facet::wrap::natural_height(
        n,
        Some(ncol),
        aspect,
        w,
        plot::FACET_STRIP + 8.0,
        plot::FACET_GAP,
        plot::FACET_GAP,
        plot::FACET_MIN_PANEL,
    );
    // a Top legend (~44 px) and an attribution band (~36 px)
    Some((h + 80.0).clamp(220.0, 1400.0).round() as u32)
}

/// The attribution (and scale bar) strip at the bottom of a map.
struct Foot {
    /// Lines of `(text, link)` runs.
    lines: Vec<Vec<(String, Option<&'static str>)>>,
    size: f64,
    h: f64,
    /// Room for a scale bar on the left.
    scale: bool,
}

/// The BKG's source list (part of its required source note).
const BKG_SOURCES: &str =
    "https://sgx.geodatenzentrum.de/web_public/gdz/datenquellen/datenquellen_vg_nuts.pdf";

/// The attribution a map's pack layers require: "Made with Natural Earth"
/// for the world layers; for the BKG layers the source note "© BKG (year)
/// dl-de/by-2-0, Datenquellen: <list>" (BKG and the licence linked, as its
/// terms ask on a web page) and the note that the geometry was modified.
/// Sized to fit the panel's width.
fn foot_of(o: &RenderOptions, m: &Model, w: f64) -> Foot {
    let german = o.locale.tag.starts_with("de");
    let mut lines: Vec<Vec<(String, Option<&'static str>)>> = Vec::new();
    if m.layers.iter().any(|l| l.starts_with("world")) {
        lines.push(vec![(
            "Made with Natural Earth".into(),
            Some("https://www.naturalearthdata.com"),
        )]);
    }
    if m.layers.iter().any(|l| l.starts_with("de-")) {
        lines.push(vec![
            ("© ".into(), None),
            ("BKG".into(), Some("https://www.bkg.bund.de")),
            (format!(" ({}) ", boundaries::BKG_YEAR), None),
            (
                "dl-de/by-2-0".into(),
                Some("https://www.govdata.de/dl-de/by-2-0"),
            ),
            (", Datenquellen:".into(), None),
        ]);
        lines.push(vec![
            (BKG_SOURCES.into(), Some(BKG_SOURCES)),
            (
                if german {
                    " · Geometrie verändert (vereinfacht)"
                } else {
                    " · geometry modified (simplified)"
                }
                .into(),
                None,
            ),
        ]);
    }
    if m.layers.iter().any(|l| l.starts_with("nuts")) {
        // GISCO's terms: this notice in the legend of every map
        lines.push(vec![(
            if german {
                "© EuroGeographics bezüglich der Verwaltungsgrenzen"
            } else {
                "© EuroGeographics for the administrative boundaries"
            }
            .into(),
            None,
        )]);
    }
    let scale = !matches!(m.proj, Proj::Identity) && span_km(m).is_some_and(|k| k < 3000.0);
    // `plot.rs` now fits each attribution row against the *actual*
    // rendered panel rect at draw time (`GeomAttribution::fit`/
    // `.fit_overflow(Wrap)`, ggplot-rs 0.31.4, #546), so this no longer
    // pre-shrinks `size` itself down to whatever a panel-width guess
    // allows — that's what used to crush the BKG/VG2500 note to ~1px on
    // a narrow panel in the first place. `size` is just the un-shrunk
    // ceiling `fit` starts from; it must stay at or above
    // `ATTRIBUTION_FIT_FLOOR` (`fit`'s `min_size` clamps to `text_size`,
    // so a ceiling below the floor would make the floor a no-op).
    let size = ATTRIBUTION_FIT_FLOOR + 0.5;
    // Still used to pre-reserve enough footer height for the one row this
    // crate's own content can make `fit` wrap even at the floor size: the
    // BKG sources line (a long bare URL run plus a trailing note). `fit`
    // decides wrapping against the real panel at draw time, which this
    // pre-layout estimate can't see — this guess (same width heuristic
    // `fit` itself uses) only has to be right often enough to avoid
    // `frame_of` leaving too short a band, not pixel-perfect;
    // `fit_overflow(Wrap)` caps any one row at 3 physical lines, but none
    // of this crate's rows come close to needing more than one extra
    // line at any width checked for #546.
    let room = (w - 8.0 - if scale { 70.0 } else { 0.0 }).max(40.0);
    let widest = lines
        .iter()
        .map(|l| {
            let t: String = l.iter().map(|(s, _)| s.as_str()).collect();
            ggplot_rs::render::text::text_width(&t, 1.0)
        })
        .fold(0.0, f64::max);
    let extra_wrapped_row = widest > 0.0 && widest * ATTRIBUTION_FIT_FLOOR > room;
    let rows = lines.len() as f64 + if extra_wrapped_row { 1.0 } else { 0.0 };
    let h = if lines.is_empty() {
        if scale {
            16.0
        } else {
            0.0
        }
    } else {
        (rows * size * 1.3 + 4.0).max(if scale { 16.0 } else { 0.0 })
    };
    Foot {
        lines,
        size,
        h,
        scale,
    }
}

/// The data's east-west span in km (lon/lat data only).
fn span_km(m: &Model) -> Option<f64> {
    let e = &m.lonlat;
    if e.is_empty() || matches!(m.proj, Proj::Identity) {
        return None;
    }
    let lat = ((e.y0 + e.y1) / 2.0).to_radians();
    Some((e.x1 - e.x0) * 111.32 * lat.cos())
}

/// The classes part of a map summary, shared by the choropleth, symbol and
/// hexbin summaries: `". k method classes: n1 / n2 / …"` when `::CLASSES` is
/// active, else empty.
fn classes_suffix(s: &ValueScale) -> String {
    match &s.classes {
        Some((method, c, _)) => {
            let counts: Vec<String> = c.counts.iter().map(|n| n.to_string()).collect();
            format!(
                ". {} {} classes: {}",
                c.len(),
                method.name(),
                counts.join(" / ")
            )
        }
        None => String::new(),
    }
}

/// One sentence for screen readers: what the map shows, its projection,
/// the highest and lowest feature, the classes and their counts, and how
/// many rows have no value.
fn summary(o: &RenderOptions, m: &Model, extra_self: usize) -> String {
    if let Some(fl) = &m.flows {
        return flows_summary(o, m, fl, extra_self);
    }
    if let Some(hb) = &m.hexbin {
        return hexbin_summary(o, m, hb);
    }
    if let Some(sym) = &m.symbols {
        return symbols_summary(o, m, sym);
    }
    let drawn: Vec<usize> = (0..m.rows).filter(|i| !m.features[*i].is_empty()).collect();
    let (mut areas, mut lines, mut points) = (0, 0, 0);
    for i in &drawn {
        match m.features[*i].first() {
            Some(Shape::Area(_)) => areas += 1,
            Some(Shape::Line(_)) => lines += 1,
            Some(Shape::Point(_)) => points += 1,
            None => {}
        }
    }
    let n = drawn.len();
    let what = if areas == n {
        if n == 1 {
            "region"
        } else {
            "regions"
        }
    } else if points == n {
        if n == 1 {
            "point"
        } else {
            "points"
        }
    } else if lines == n {
        if n == 1 {
            "line"
        } else {
            "lines"
        }
    } else if n == 1 {
        "feature"
    } else {
        "features"
    };
    let mut out = format!("{n} {what} ({})", m.projection.describe());
    let name = |i: usize| {
        let l = m.label_of(i);
        if !l.is_empty() {
            l.lines().next().unwrap_or("").to_string()
        } else {
            let c = m.text(m.category, i);
            if c.is_empty() {
                format!("row {}", i + 1)
            } else {
                c
            }
        }
    };
    match &m.fill {
        Fill::Measure(s) => {
            let vals: Vec<(usize, f64)> = drawn
                .iter()
                .filter_map(|i| s.values[*i].map(|v| (*i, v)))
                .collect();
            if let (Some(hi), Some(lo)) = (
                vals.iter().max_by(|a, b| a.1.total_cmp(&b.1)),
                vals.iter().min_by(|a, b| a.1.total_cmp(&b.1)),
            ) {
                let f = |v: f64| s.fmt.full(v, &o.locale);
                if hi.0 == lo.0 {
                    out.push_str(&format!(". {} {}", name(hi.0), f(hi.1)));
                } else {
                    out.push_str(&format!(
                        ". Highest: {} {}; lowest: {} {}",
                        name(hi.0),
                        f(hi.1),
                        name(lo.0),
                        f(lo.1)
                    ));
                }
            }
            out.push_str(&classes_suffix(s));
            let missing = drawn.iter().filter(|i| s.values[**i].is_none()).count();
            if missing > 0 {
                out.push_str(&format!("; {missing} without data"));
            }
        }
        Fill::Category { levels, of_row } => {
            let mut counts = vec![0usize; levels.len()];
            for i in &drawn {
                if let Some(k) = of_row[*i] {
                    counts[k] += 1;
                }
            }
            let parts: Vec<String> = levels
                .iter()
                .zip(&counts)
                .take(6)
                .map(|((l, _), c)| format!("{l} ({c})"))
                .collect();
            let more = levels.len().saturating_sub(6);
            out.push_str(&format!(
                ". {} groups: {}{}",
                levels.len(),
                parts.join(", "),
                if more > 0 {
                    format!(", and {more} more")
                } else {
                    String::new()
                }
            ));
        }
        Fill::Plain(_) => {}
        // `summary()` is shared by both render paths (`plot::build`'s
        // `geom_sf` path calls it too, via `super::summary`) — unlike the
        // per-row render loop above, this sentence is live for maps.
        Fill::Bivariate(b) => {
            let a_name = b.title_a.as_deref().unwrap_or("the first measure");
            let b_name = b.title_b.as_deref().unwrap_or("the second measure");
            let cell = |i: usize, j: usize| b.counts[i][j].to_string();
            out.push_str(&format!(
                ". {a_name} × {b_name} in tertiles: high–high {}, low–low {}, high {a_name}–low {b_name} {}, low {a_name}–high {b_name} {}",
                cell(2, 2),
                cell(0, 0),
                cell(2, 0),
                cell(0, 2),
            ));
            let missing = drawn
                .iter()
                .filter(|i| b.ia[**i].is_none() || b.jb[**i].is_none())
                .count();
            if missing > 0 {
                out.push_str(&format!("; {missing} without data"));
            }
        }
    }
    out.push('.');
    out
}

/// One sentence for a graduated-symbol map: the count, the largest and
/// smallest, the size measure, then the colour measure's classes / no-data
/// (as [`summary`]) and how many rows weren't drawn for size.
fn symbols_summary(o: &RenderOptions, m: &Model, sym: &Symbols) -> String {
    let name = |i: usize| {
        let l = m.label_of(i);
        if !l.is_empty() {
            l.lines().next().unwrap_or("").to_string()
        } else {
            let c = m.text(m.category, i);
            if c.is_empty() {
                format!("row {}", i + 1)
            } else {
                c
            }
        }
    };
    let drawn: Vec<(usize, f64)> = (0..m.rows)
        .filter_map(|i| sym.size[i].map(|v| (i, v)))
        .collect();
    let n = drawn.len();
    let mut out = format!(
        "{n} symbol{} ({})",
        if n == 1 { "" } else { "s" },
        m.projection.describe()
    );
    if let (Some(hi), Some(lo)) = (
        drawn.iter().max_by(|a, b| a.1.total_cmp(&b.1)),
        drawn.iter().min_by(|a, b| a.1.total_cmp(&b.1)),
    ) {
        let f = |v: f64| sym.fmt.full(v, &o.locale);
        if hi.0 == lo.0 {
            out.push_str(&format!(". {} {}", name(hi.0), f(hi.1)));
        } else {
            out.push_str(&format!(
                ". Largest: {} {}; smallest: {} {}",
                name(hi.0),
                f(hi.1),
                name(lo.0),
                f(lo.1)
            ));
        }
    }
    let size_title = sym.title.clone().unwrap_or_else(|| "size".to_string());
    out.push_str(&format!(". Area ∝ {size_title}"));
    if let Fill::Measure(s) = &m.fill {
        out.push_str(&classes_suffix(s));
        let missing = drawn.iter().filter(|(i, _)| s.values[*i].is_none()).count();
        if missing > 0 {
            out.push_str(&format!("; {missing} without data"));
        }
    }
    let skipped = (0..m.rows)
        .filter(|i| !sym.anchors[*i].is_empty() && sym.size[*i].is_none())
        .count();
    if skipped > 0 {
        out.push_str(&format!("; {skipped} not drawn (zero or negative size)"));
    }
    out.push('.');
    out
}

/// One sentence for a `::HEXBIN` map: the cell and point counts, the grid
/// width, the densest cell, then the colour classes/no-data fragment a
/// choropleth summary uses.
fn hexbin_summary(o: &RenderOptions, m: &Model, hb: &Hexbin) -> String {
    let Fill::Measure(s) = &m.fill else {
        return String::new();
    };
    let n = hb.cells.len();
    let mut out = format!(
        "{n} hexagon{} {} across ({}), {} point{}",
        if n == 1 { "" } else { "s" },
        hexbin::fmt_width(hb.width, hb.planar),
        m.projection.describe(),
        hb.n_points,
        if hb.n_points == 1 { "" } else { "s" }
    );
    if let Some((i, v)) = hb
        .cells
        .iter()
        .enumerate()
        .map(|(i, (_, c))| (i, if hb.has_measure { c.sum } else { c.n as f64 }))
        .max_by(|a, b| a.1.total_cmp(&b.1))
    {
        let label = hexbin::label_of(&hb.cells[i].1, hb.planar);
        out.push_str(&format!(". Densest: {label} {}", s.fmt.full(v, &o.locale)));
    }
    out.push_str(&classes_suffix(s));
    out.push('.');
    out
}

/// One sentence for a flow map: the count and distinct origins/destinations,
/// the largest and smallest flow, then how many weren't drawn (no location,
/// self-flows).
fn flows_summary(o: &RenderOptions, m: &Model, fl: &Flows, extra_self: usize) -> String {
    let n = fl.flows.len();
    let origins: std::collections::HashSet<&str> = fl.flows.iter().map(|f| f.ok.as_str()).collect();
    let dests: std::collections::HashSet<&str> = fl.flows.iter().map(|f| f.dk.as_str()).collect();
    let mut out = format!(
        "{n} flow{} between {} origin{} and {} destination{} ({})",
        if n == 1 { "" } else { "s" },
        origins.len(),
        if origins.len() == 1 { "" } else { "s" },
        dests.len(),
        if dests.len() == 1 { "" } else { "s" },
        m.projection.describe()
    );
    if !fl.flows.is_empty() {
        let hi = fl.flows.iter().max_by(|a, b| a.w.total_cmp(&b.w)).unwrap();
        let lo = fl.flows.iter().min_by(|a, b| a.w.total_cmp(&b.w)).unwrap();
        let f = |v: Option<f64>, w: f64| fl.fmt.full(v.unwrap_or(w), &o.locale);
        if std::ptr::eq(hi, lo) {
            out.push_str(&format!(
                ". {} → {} {}",
                hi.o_name,
                hi.d_name,
                f(hi.raw, hi.w)
            ));
        } else {
            out.push_str(&format!(
                ". Largest: {} → {} {}; smallest: {} → {} {}",
                hi.o_name,
                hi.d_name,
                f(hi.raw, hi.w),
                lo.o_name,
                lo.d_name,
                f(lo.raw, lo.w)
            ));
        }
    }
    let unplaced = fl.unplaced;
    if unplaced > 0 {
        out.push_str(&format!("; {unplaced} not drawn (no location)"));
    }
    let selfs = fl.self_flows + extra_self;
    if selfs > 0 {
        out.push_str(&format!("; {selfs} start and end at the same place"));
    }
    out.push('.');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_window_round_trips() {
        let f = Frame::fit((0.0, 0.0, 10.0, 5.0), (2.0, 30.0, 96.0, 68.0));
        let [x0, x1, y0, y1] = f.window(100.0, 100.0);
        // the window spans the panel at the same scale
        assert!(((x1 - x0) * f.s - 100.0).abs() < 1e-9);
        assert!(((y1 - y0) * f.s - 100.0).abs() < 1e-9);
        // and fitting it back over the whole panel gives the same transform
        let g = Frame::fit((x0, y0, x1, y1), (0.0, 0.0, 100.0, 100.0));
        let p = [3.0, 4.0];
        let (a, b) = (f.at(p), g.at(p));
        assert!((a.0 - b.0).abs() < 1e-9 && (a.1 - b.1).abs() < 1e-9);
    }
}
