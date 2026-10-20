//! Maps: the extension's own SVG writer for `::MAP` panels.
//!
//! A map panel's rows carry WKT (`::MAP`), a region code looked up in the
//! built-in boundary pack (`::REGION`, see [`boundaries`]) or a point
//! (`::LAT` / `::LON`); optionally a backdrop (`::BASEMAP`: WKT or a pack
//! layer such as `'world'`), a measure or a `::CATEGORY` to colour by, a
//! `::LABEL`, a layer opacity (`::ALPHA`), labels on the map (`::LABEL_TOP`,
//! `::DATALABELS`) and the map modifiers `::PROJECTION`, `::CLASSES`,
//! `::MIDPOINT`, `::RANGE` and `::BOUNDARIES`. The writer:
//!
//! 1. resolves each row's geometry once (WKT via
//!    `ggplot_rs::spatial::parse_wkt`, a pack region, or a point),
//! 2. resolves the projection (`'auto'` by the data's extent) and projects
//!    every vertex, densifying long edges where the projection curves them,
//! 3. fits the projected box into the panel (or the browser's zoom window),
//! 4. simplifies each ring in pixel space (0.5 px) and writes compact
//!    relative `<path>` data — `fill-rule="evenodd"`, so holes are cut out —
//!    and `<circle>`s for points,
//! 5. colours by a themed sequential or diverging ramp (continuous, or in
//!    classes with a stepped legend and counts), or by the theme palette,
//! 6. writes a legend, haloed labels that don't collide, a scale bar, the
//!    attribution a pack layer requires, a hatch for rows without a value
//!    and a one-sentence summary (`<desc>` and `data-summary`).
//!
//! Features are drawn in row order. Each carries `data-row`, `data-series`
//! (its `::CATEGORY`, else its `::LABEL`), `data-value` (the raw measure),
//! `data-label`, `data-x` (its `::XAXIS` — a `::STORY` step key — else, when it
//! has no label, the row) and, when
//! classed, `data-class` (1-based), plus a `<title>`; the root carries
//! `data-map`, `data-proj` and `data-classes` (the class labels as JSON).

pub(crate) mod boundaries;
pub(crate) mod classes;
mod flows;
pub(crate) mod hexbin;
mod labels;
mod legend;
mod overlay;
pub(crate) mod project;
mod symbols;
mod write;

use crate::format::{escape_xml as esc, ValueFmt};
use crate::theme::{hex, mix};
use crate::{
    cell_num, find_role, role_num, role_str, value_fmt, value_str, Column, Kind, Ramp,
    RenderOptions, Role,
};
use classes::{Classes, Method};
use flows::Flows;
use ggplot_rs::prelude::Value;
use ggplot_rs::spatial::{parse_wkt, Geometry};
use hexbin::Hexbin;
use legend::{legend_of, legend_svg, legend_title, Legend, LegendBox};
use project::{Extent, Proj, Projection};
use symbols::Symbols;
use write::{push_path, simplify, tenths};

type Rgb = (u8, u8, u8);

/// Drawn marks above which a render warning suggests aggregating.
pub(crate) const MARK_WARN: usize = 100_000;
/// Pixel tolerance of the screen-space simplification.
const SIMPLIFY_PX: f64 = 0.5;
/// Legend and summary text size (as the plotting engine's legends).
const FONT: f64 = 8.8;
/// Point radius.
const POINT_R: f64 = 3.0;

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
}

/// The colour scale of a measure.
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
    cols.iter()
        .find(|c| matches!(c.role, Role::Value(_)))
        .cloned()
        .or(numeric_geometry)
        .filter(|c| c.values.iter().any(|v| v.as_f64().is_some()))
}

impl<'a> Model<'a> {
    fn new(o: &RenderOptions, cols: &'a [Column]) -> Option<Model<'a>> {
        let german = o.locale.tag.starts_with("de");
        let is_flow = find_role(cols, Role::Origin)
            .is_some_and(|c| c.values.iter().any(|v| *v != Value::Na))
            && find_role(cols, Role::Dest)
                .is_some_and(|c| c.values.iter().any(|v| *v != Value::Na));
        if is_flow {
            let raw = flows::of(cols, german)?;
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
            for f in &raw.flows {
                extent.add(f.o);
                extent.add(f.d);
            }
            if base_fitted {
                for s in base.iter().flatten() {
                    for_coords(s, &mut |c| extent.add(c));
                }
            }
            let (projection, proj) = Proj::setup(projection, &extent);
            let lonlat = extent;
            let base: Vec<Vec<Shape>> = base
                .iter()
                .map(|f| f.iter().filter_map(|s| project_shape(s, &proj)).collect())
                .collect();
            let flows_model = flows::project(raw, &proj);
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
        let hexbin = hexbin::of(cols, &proj);
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
        let fill = if let Some(hb) = &hexbin {
            hexbin::fill_of(o, cols, hb)
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
        })
    }

    /// The projected box of everything drawn (data and backdrop; the
    /// backdrop only when it's fitted — [`Model::base_fitted`]).
    fn bbox(&self) -> Option<(f64, f64, f64, f64)> {
        let mut e = Extent::new();
        if let Some(fl) = &self.flows {
            for f in &fl.flows {
                e.add(f.o);
                e.add(f.d);
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
                    let colors = (0..k)
                        .map(|i| {
                            if scale.diverging {
                                let (a, b) = c.bounds(i);
                                scale.color_at((a + b) / 2.0)
                            } else if k == 1 {
                                step(1.0)
                            } else {
                                step(i as f64 / (k - 1) as f64)
                            }
                        })
                        .collect();
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

/// A stable id prefix for this map's `<defs>` (pages hold many SVGs).
fn id_of(cols: &[Column], w: u32, h: u32) -> String {
    let mut x: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |b: &[u8]| {
        for c in b {
            x ^= *c as u64;
            x = x.wrapping_mul(0x0100_0000_01b3);
        }
    };
    eat(&w.to_le_bytes());
    eat(&h.to_le_bytes());
    for c in cols {
        eat(c.name.as_bytes());
        for v in c.values.iter().take(64) {
            eat(value_str(v).as_bytes());
        }
        eat(&c.values.len().to_le_bytes());
    }
    format!("m{:08x}", x as u32)
}

/// An attribute value: escaped, newlines kept as `&#10;`.
fn attr(s: &str) -> String {
    esc(s).replace('\n', "&#10;").replace('\r', "&#13;")
}

/// A rendered map: the SVG body, its root attributes and render warnings.
pub(crate) struct Drawn {
    pub body: String,
    pub attrs: Vec<(&'static str, String)>,
    pub warnings: Vec<String>,
}

/// Where the map and its legend go when not zoomed: the data's box (with
/// its margin) fitted to the whole panel, the legend over the corner that
/// covers the fewest vertices — or, when every corner would cover some,
/// beside or above the map (whichever keeps it larger).
fn layout(m: &Model, lg: &LegendBox, w: f64, h: f64) -> Option<(Frame, (f64, f64))> {
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
) -> Option<(Frame, (f64, f64))> {
    let (auto, at) = layout(m, lg, w, h - foot)?;
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
    let m = Model::new(o, cols)?;
    if w == 0 || h == 0 {
        let (x0, y0, x1, y1) = m.bbox()?;
        return Some([x0, x1, y0, y1]);
    }
    let (w, h) = (w as f64, h as f64);
    let lg = legend_of(o, &m, w, h);
    let ft = foot_of(o, &m, w);
    layout(&m, &lg, w, h - ft.h).map(|(f, _)| f.window(w, h))
}

/// Render a map panel's body (`None` when no row has geometry).
pub(crate) fn render(o: &RenderOptions, cols: &[Column], width: u32, height: u32) -> Option<Drawn> {
    let m = Model::new(o, cols)?;
    let (w, h) = (width as f64, height as f64);
    let lg = legend_of(o, &m, w, h);
    let nodata = lg.nodata;
    // `nodata` (above) only counts regular features/symbols without a
    // value — a flow row without one (`Fill::Measure` + no value) isn't a
    // "feature", so it needs its own check here to still get the hatch
    // pattern + the `data-nd` CSS override defined (overlay::render sets
    // `data-nd="1"` on those rows regardless of this map having any other
    // no-data features).
    let flow_nodata = m.flows.as_ref().is_some_and(|fl| {
        matches!(&m.fill, Fill::Measure(s) if fl.flows.iter().any(|f| s.values[f.row].is_none()))
    });
    let ft = foot_of(o, &m, w);
    let (frame, legend_at) = frame_of(&m, o, &lg, w, h, ft.h)?;
    let t = &o.theme;
    let id = id_of(cols, width, height);
    let card = t.colors.card;
    let font = esc(&t.svg_font());
    let mut out = String::with_capacity(16 * 1024);
    let mut warnings = Vec::new();

    // ---- defs: the no-data hatch, the gradient
    let hatch_bg = mix(t.colors.muted, card, 0.85);
    let hatch_fg = mix(t.colors.muted, card, 0.35);
    let mut defs = String::new();
    if nodata > 0 || flow_nodata {
        defs.push_str(&format!(
            "<pattern id=\"{id}-nd\" width=\"4\" height=\"4\" patternUnits=\"userSpaceOnUse\" patternTransform=\"rotate(45)\"><rect width=\"4\" height=\"4\" fill=\"{}\"/><path d=\"M0 0v4\" stroke=\"{}\" stroke-width=\"1.2\"/></pattern>",
            hex(hatch_bg),
            hex(hatch_fg)
        ));
    }
    // A flow's no-data rows carry `data-nd="1"` (set by overlay::render, via
    // the ggplot-rs curve layer's own `data_attr`) instead of a hand-picked
    // `fill`/`stroke`; this rule paints them with the hatch above, scoped to
    // this map's own `id` so two maps on one dashboard don't cross-wire.
    if flow_nodata {
        defs.push_str(&format!(
            "<style>#{id} [data-nd]{{stroke:url(#{id}-nd)}}</style>"
        ));
    }
    if let Legend::Gradient { stops, .. } = &lg.legend {
        defs.push_str(&format!("<linearGradient id=\"{id}-g\">"));
        for (i, c) in stops.iter().enumerate() {
            defs.push_str(&format!(
                "<stop offset=\"{:.3}\" stop-color=\"{}\"/>",
                i as f64 / (stops.len() - 1).max(1) as f64,
                hex(*c)
            ));
        }
        defs.push_str("</linearGradient>");
    }
    if !defs.is_empty() {
        out.push_str(&format!("<defs>{defs}</defs>"));
    }

    // ---- the basemap
    let viewport = (-20.0, -20.0, w + 20.0, h + 20.0);
    let mut marks = 0usize;
    let land = mix(t.colors.muted, card, 0.92);
    let land_edge = mix(t.colors.muted, card, 0.7);
    let mut base = String::new();
    for f in &m.base {
        let d = path_d(f, &frame, viewport, true);
        if !d.is_empty() {
            base.push_str(&format!("<path d=\"{d}\"/>"));
            marks += 1;
        }
    }
    if !base.is_empty() {
        out.push_str(&format!(
            "<g class=\"dp-basemap\" fill=\"{}\" fill-rule=\"evenodd\" stroke=\"{}\" stroke-width=\".5\" stroke-linejoin=\"round\">{base}</g>",
            hex(land),
            hex(land_edge)
        ));
    }

    // ---- the features, in row order
    let border = t.chart_grid();
    let plain_edge = mix(t.colors.muted, card, 0.35);
    let alpha = if m.alpha < 1.0 {
        format!("{:.2}", m.alpha)
    } else {
        String::new()
    };
    let fmt_full = |s: &ValueScale, v: f64| s.fmt.full(v, &o.locale);
    // the areas' border and fill rule sit on the group (lines and points
    // set their own stroke)
    let edge = if matches!(m.fill, Fill::Plain(_)) {
        plain_edge
    } else {
        border
    };
    out.push_str(&format!(
        "<g class=\"dp-features\" fill-rule=\"evenodd\" stroke=\"{}\" stroke-width=\".6\" stroke-linejoin=\"round\" stroke-linecap=\"round\">",
        hex(edge)
    ));
    // an ::XAXIS keys the features for a ::STORY (a route stop by stop)
    let step_x = find_role(cols, Role::X);
    for (i, f) in m.features.iter().enumerate() {
        // ::HEXBIN (M2c) takes over the point pass entirely — the per-row
        // features group draws nothing (m.fill is indexed by cell, not row).
        if f.is_empty() || m.hexbin.is_some() {
            continue;
        }
        let label = m.label_of(i);
        let cat = m.text(m.category, i);
        let series = if cat.is_empty() {
            label.clone()
        } else {
            cat.clone()
        };
        let (color, value, class, nd) = match &m.fill {
            Fill::Measure(s) => match s.values[i] {
                Some(v) => {
                    let (c, k) = s.color(v);
                    (c, Some(v), k, false)
                }
                None => (hatch_bg, None, None, true),
            },
            Fill::Category { of_row, levels } => (
                of_row[i].map(|k| levels[k].1).unwrap_or(hatch_bg),
                None,
                None,
                false,
            ),
            Fill::Plain(c) => (*c, None, None, false),
        };
        // the tooltip: "label: value", as the plotting engine wrote it
        let shown = match (&m.fill, value) {
            (Fill::Measure(s), Some(v)) => fmt_full(s, v),
            (Fill::Measure(_), None) => "no data".to_string(),
            (Fill::Category { .. }, _) => cat.clone(),
            _ => String::new(),
        };
        let mut title = match (label.is_empty(), shown.is_empty()) {
            (false, false) => format!("{label}: {shown}"),
            (false, true) => label.clone(),
            (true, _) => shown,
        };
        if let (Fill::Measure(s), Some(k)) = (&m.fill, class) {
            if let Some((_, c, _)) = &s.classes {
                title.push_str(&format!(
                    " (class {} of {}: {})",
                    k + 1,
                    c.len(),
                    c.label(k, &|v| s.fmt.short(v, &o.locale))
                ));
            }
        }
        // the key (when no label names it) and class first, then the row
        // metadata the browser reads
        let mut data = String::new();
        if let Some(x) = step_x
            .and_then(|c| c.values.get(i))
            .and_then(crate::mark_key_text)
        {
            data.push_str(&format!(" data-x=\"{}\"", attr(&x)));
        } else if label.is_empty() {
            data.push_str(&format!(" data-x=\"{i}\""));
        }
        if let Some(k) = class {
            data.push_str(&format!(" data-class=\"{}\"", k + 1));
        }
        data.push_str(&format!(" data-row=\"{i}\""));
        if !series.is_empty() {
            data.push_str(&format!(" data-series=\"{}\"", attr(&series)));
        }
        if let Some(v) = value {
            data.push_str(&format!(" data-value=\"{v}\""));
        }
        if !label.is_empty() {
            data.push_str(&format!(" data-label=\"{}\"", attr(&label)));
        }
        let inner = if title.is_empty() {
            "/>".to_string()
        } else {
            format!("><title>{}</title>", esc(&title))
        };
        let close = |tag: &str| {
            if title.is_empty() {
                String::new()
            } else {
                format!("</{tag}>")
            }
        };
        let fill_attr = if nd {
            format!("url(#{id}-nd)")
        } else {
            hex(color)
        };
        let fo = if alpha.is_empty() {
            String::new()
        } else {
            format!(" fill-opacity=\"{alpha}\"")
        };
        let areas: Vec<&Shape> = f.iter().filter(|s| matches!(s, Shape::Area(_))).collect();
        let lines: Vec<&Shape> = f.iter().filter(|s| matches!(s, Shape::Line(_))).collect();
        // graduated symbols (M2a) take over areas and points — a symbol is
        // drawn for the row below instead, in its own pass (largest first)
        if !areas.is_empty() && m.symbols.is_none() {
            let d = path_d_of(&areas, &frame, viewport, true);
            if !d.is_empty() {
                out.push_str(&format!(
                    "<path d=\"{d}\" fill=\"{fill_attr}\"{fo}{data}{inner}{}",
                    close("path")
                ));
                marks += 1;
            }
        }
        if !lines.is_empty() {
            let d = path_d_of(&lines, &frame, viewport, false);
            if !d.is_empty() {
                let so = if alpha.is_empty() {
                    String::new()
                } else {
                    format!(" stroke-opacity=\"{alpha}\"")
                };
                out.push_str(&format!(
                    "<path d=\"{d}\" fill=\"none\" stroke=\"{}\" stroke-width=\"1.5\"{so}{data}{inner}{}",
                    if nd { hex(hatch_fg) } else { hex(color) },
                    close("path")
                ));
                marks += 1;
            }
        }
        if m.symbols.is_none() && m.hexbin.is_none() {
            for s in f {
                if let Shape::Point(p) = s {
                    let (x, y) = frame.at(*p);
                    if x < viewport.0 || y < viewport.1 || x > viewport.2 || y > viewport.3 {
                        continue;
                    }
                    out.push_str(&format!(
                        "<circle cx=\"{:.1}\" cy=\"{:.1}\" r=\"{POINT_R}\" fill=\"{fill_attr}\"{fo} stroke=\"{}\" stroke-width=\".5\"{data}{inner}{}",
                        x,
                        y,
                        hex(card),
                        close("circle")
                    ));
                    marks += 1;
                }
            }
        }
    }
    out.push_str("</g>");

    // ---- graduated symbols (::SIZE, M2a): one circle per anchor, largest
    // first (D5) — a feature's area, pole of inaccessibility, or point.
    if let Some(sym) = &m.symbols {
        struct Circ {
            r: f64,
            row: usize,
            pt: [f64; 2],
            v: f64,
        }
        let rmax_px = symbols::rmax(w, h);
        let mut circles: Vec<Circ> = Vec::new();
        for i in 0..m.rows {
            if sym.anchors[i].is_empty() {
                continue;
            }
            let Some(v) = sym.size[i] else { continue };
            let r = sym
                .fixed
                .unwrap_or_else(|| symbols::radius(rmax_px, v, sym.vmax));
            for pt in &sym.anchors[i] {
                circles.push(Circ {
                    r,
                    row: i,
                    pt: *pt,
                    v,
                });
            }
        }
        circles.sort_by(|a, b| b.r.total_cmp(&a.r));
        out.push_str("<g class=\"dp-symbols\">");
        for c in &circles {
            let i = c.row;
            let label = m.label_of(i);
            let cat = m.text(m.category, i);
            let series = if cat.is_empty() {
                label.clone()
            } else {
                cat.clone()
            };
            let (color, value, class, nd) = match &m.fill {
                Fill::Measure(s) => match s.values[i] {
                    Some(v) => {
                        let (col, k) = s.color(v);
                        (col, Some(v), k, false)
                    }
                    None => (hatch_bg, None, None, true),
                },
                Fill::Category { of_row, levels } => (
                    of_row[i].map(|k| levels[k].1).unwrap_or(hatch_bg),
                    None,
                    None,
                    false,
                ),
                Fill::Plain(col) => (*col, None, None, false),
            };
            let shown = match (&m.fill, value) {
                (Fill::Measure(s), Some(v)) => fmt_full(s, v),
                (Fill::Measure(_), None) => "no data".to_string(),
                (Fill::Category { .. }, _) => cat.clone(),
                _ => String::new(),
            };
            let fsize = sym.fmt.full(c.v, &o.locale);
            let size_title = sym.title.clone().unwrap_or_else(|| "Size".to_string());
            let mut title = if matches!(m.fill, Fill::Measure(_)) {
                let head = match (label.is_empty(), shown.is_empty()) {
                    (false, false) => format!("{label}: {shown}"),
                    (false, true) => label.clone(),
                    (true, _) => shown.clone(),
                };
                format!("{head}; {size_title}: {fsize}")
            } else if !label.is_empty() {
                format!("{label}: {fsize}")
            } else {
                fsize.clone()
            };
            if let (Fill::Measure(s), Some(k)) = (&m.fill, class) {
                if let Some((_, cl, _)) = &s.classes {
                    title.push_str(&format!(
                        " (class {} of {}: {})",
                        k + 1,
                        cl.len(),
                        cl.label(k, &|v| s.fmt.short(v, &o.locale))
                    ));
                }
            }
            let mut data = String::new();
            if label.is_empty() {
                data.push_str(&format!(" data-x=\"{i}\""));
            }
            if let Some(k) = class {
                data.push_str(&format!(" data-class=\"{}\"", k + 1));
            }
            data.push_str(&format!(" data-row=\"{i}\""));
            if !series.is_empty() {
                data.push_str(&format!(" data-series=\"{}\"", attr(&series)));
            }
            let data_value = value.unwrap_or(c.v);
            data.push_str(&format!(" data-value=\"{data_value}\""));
            if !label.is_empty() {
                data.push_str(&format!(" data-label=\"{}\"", attr(&label)));
            }
            data.push_str(&format!(" data-size=\"{}\"", c.v));
            data.push_str(&format!(" data-fsize=\"{}\"", attr(&fsize)));
            if !matches!(m.fill, Fill::Measure(_)) {
                // no colour measure: with_formatted_values (src/lib.rs) only
                // adds data-fvalue when a ::Value column exists, so write it
                // ourselves here (never both — fvalue_is_written_once).
                data.push_str(&format!(" data-fvalue=\"{}\"", attr(&fsize)));
            }
            let (x, y) = frame.at(c.pt);
            let fill_attr = if nd {
                format!("url(#{id}-nd)")
            } else {
                hex(color)
            };
            let fo = if alpha.is_empty() {
                String::new()
            } else {
                format!(" fill-opacity=\"{alpha}\"")
            };
            let inner = if title.is_empty() {
                "/>".to_string()
            } else {
                format!("><title>{}</title></circle>", esc(&title))
            };
            out.push_str(&format!(
                "<circle cx=\"{x:.1}\" cy=\"{y:.1}\" r=\"{:.1}\" fill=\"{fill_attr}\"{fo} stroke=\"{}\" stroke-width=\".75\"{data}{inner}",
                c.r,
                hex(card)
            ));
            marks += 1;
        }
        out.push_str("</g>");
    }

    // ---- flows (::ORIGIN/::DEST, M2b): a ggplot-rs overlay (`geom_curve` +
    // `arrow()` + the `linewidth`/identity-colour/interactive aesthetics —
    // see `overlay.rs` and issue #318), spliced in here: after the basemap
    // and before `labels_svg` below.
    let mut extra_self = 0usize; // L < 2px screen-coincident flows, counted at render time
    if let Some(fl) = &m.flows {
        let (frag, added_marks, self_ct, cap_warn) =
            overlay::render(o, cols, &m, &frame, fl, t, card, hatch_bg, w, h);
        out.push_str(&frag);
        marks += added_marks;
        extra_self += self_ct;
        if let Some(cw) = cap_warn {
            warnings.push(cw);
        }
    }

    // ---- density binning (::HEXBIN, M2c): a ggplot-rs overlay (`geom_hex`
    // drawing cells `hexbin::of` already binned via `StatBinHex` — see
    // `overlay.rs` and issue #318), spliced in here the same way the flow
    // overlay is above: after the basemap and before `labels_svg` below.
    if let Some(hb) = &m.hexbin {
        if let Fill::Measure(scale) = &m.fill {
            let (frag, added_marks) = overlay::render_hexbin(o, hb, scale, &frame, card, w, h);
            out.push_str(&frag);
            marks += added_marks;
            if hb.cells.len() > hexbin::HEX_WARN {
                warnings.push(format!(
                    "map: {} hexagons — use a larger ::HEXBIN",
                    hb.cells.len()
                ));
            }
        }
    }

    if marks > MARK_WARN {
        warnings.push(format!(
            "map: {marks} marks drawn (over {MARK_WARN}) — aggregate in SQL (e.g. to H3 cells or regions) to keep the map light"
        ));
    }

    // ---- labels on the map
    let legend_box = (
        legend_at.0,
        legend_at.1,
        legend_at.0 + lg.w,
        legend_at.1 + lg.h,
    );
    out.push_str(&labels_svg(
        o,
        cols,
        &m,
        &frame,
        &[legend_box, (0.0, h - ft.h, w, h)],
        (w, h),
        &font,
    ));

    // ---- the legend, on a card-coloured backing
    out.push_str(&legend_svg(o, &lg, &id, legend_at, &font));

    // ---- the scale bar and the attribution the pack layers require
    out.push_str(&foot_svg(o, &m, &ft, &frame, (w, h), &font));

    // ---- summary and root attributes
    let summary = summary(o, &m, extra_self);
    out.insert_str(0, &format!("<desc>{}</desc>", esc(&summary)));
    let mut attrs = vec![
        ("data-map", String::new()),
        ("data-proj", m.projection.name().to_string()),
        ("data-summary", summary),
    ];
    // Only when the `data-nd` CSS rule above needs an anchor to scope to —
    // every other map stays exactly as it was (no stray `id`).
    if flow_nodata {
        attrs.push(("id", id.clone()));
    }
    if let Fill::Measure(s) = &m.fill {
        if let Some((_, c, _)) = &s.classes {
            let labels: Vec<String> = (0..c.len())
                .map(|k| c.label(k, &|v| s.fmt.short(v, &o.locale)))
                .collect();
            attrs.push((
                "data-classes",
                serde_json::to_string(&labels).unwrap_or_default(),
            ));
        }
    }
    if let Some(hb) = &m.hexbin {
        attrs.push(("data-hexbin", hexbin::fmt_num(hb.width)));
    }
    Some(Drawn {
        body: out,
        attrs,
        warnings,
    })
}

/// Path data of one feature's shapes of one kind.
fn path_d_of(shapes: &[&Shape], f: &Frame, vp: (f64, f64, f64, f64), close: bool) -> String {
    let mut d = String::new();
    for s in shapes {
        match s {
            Shape::Area(rings) => {
                for r in rings {
                    push_ring(&mut d, r, f, vp, true);
                }
            }
            Shape::Line(cs) => push_ring(&mut d, cs, f, vp, close),
            Shape::Point(_) => {}
        }
    }
    d
}

/// Path data of a backdrop feature (areas and lines).
fn path_d(shapes: &[Shape], f: &Frame, vp: (f64, f64, f64, f64), close: bool) -> String {
    let refs: Vec<&Shape> = shapes.iter().collect();
    path_d_of(&refs, f, vp, close)
}

/// Append one ring/line, simplified in pixel space; skipped when it lies
/// wholly outside the viewport or shrinks below a pixel.
fn push_ring(d: &mut String, pts: &[[f64; 2]], f: &Frame, vp: (f64, f64, f64, f64), close: bool) {
    let scr: Vec<(f64, f64)> = pts.iter().map(|p| f.at(*p)).collect();
    let (mut x0, mut y0, mut x1, mut y1) = (
        f64::INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NEG_INFINITY,
    );
    for (x, y) in &scr {
        x0 = x0.min(*x);
        y0 = y0.min(*y);
        x1 = x1.max(*x);
        y1 = y1.max(*y);
    }
    if x1 < vp.0 || y1 < vp.1 || x0 > vp.2 || y0 > vp.3 {
        return;
    }
    if close && (x1 - x0).max(y1 - y0) < 0.5 {
        return;
    }
    let s = simplify(&scr, SIMPLIFY_PX);
    if close && s.len() < 3 {
        // a sliver: keep its box corners so it still shows
        let a = tenths((x0, y0));
        let b = tenths((x1, y1));
        push_path(d, &[a, (b.0, a.1), b, (a.0, b.1)], true);
        return;
    }
    push_path(d, &s, close);
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
    let room = (w - 8.0 - if scale { 70.0 } else { 0.0 }).max(40.0);
    let widest = lines
        .iter()
        .map(|l| {
            let t: String = l.iter().map(|(s, _)| s.as_str()).collect();
            crate::ticks::text_width(&t, 1.0)
        })
        .fold(0.0, f64::max);
    let size = if widest > 0.0 {
        (room / widest).min(6.5)
    } else {
        6.5
    };
    let h = if lines.is_empty() {
        if scale {
            16.0
        } else {
            0.0
        }
    } else {
        (lines.len() as f64 * size * 1.3 + 4.0).max(if scale { 16.0 } else { 0.0 })
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

/// The scale bar and the attribution lines (right-aligned, small, muted).
fn foot_svg(
    o: &RenderOptions,
    m: &Model,
    ft: &Foot,
    frame: &Frame,
    (w, h): (f64, f64),
    font: &str,
) -> String {
    if ft.h <= 0.0 {
        return String::new();
    }
    let t = &o.theme;
    let muted = hex(t.chart_text());
    let mut g = String::from("<g class=\"dp-map-foot\">");
    if ft.scale {
        if let Some((px, label)) = scale_bar(m, frame) {
            let (x, y) = (6.0, h - 5.0);
            g.push_str(&format!(
                "<path d=\"M{x:.1} {:.1}v3h{px:.1}v-3\" fill=\"none\" stroke=\"{muted}\" stroke-width=\"1\"/><text x=\"{:.1}\" y=\"{:.1}\" text-anchor=\"middle\" font-family=\"{font}\" font-size=\"7.5\" fill=\"{muted}\">{}</text>",
                y - 3.0,
                x + px / 2.0,
                y - 5.0,
                esc(&label)
            ));
        }
    }
    let lh = ft.size * 1.3;
    let n = ft.lines.len() as f64;
    for (k, line) in ft.lines.iter().enumerate() {
        let y = h - 3.0 - (n - 1.0 - k as f64) * lh;
        g.push_str(&format!(
            "<text x=\"{:.1}\" y=\"{y:.1}\" text-anchor=\"end\" font-family=\"{font}\" font-size=\"{:.2}\" fill=\"{muted}\">",
            w - 4.0,
            ft.size
        ));
        for (s, link) in line {
            match link {
                Some(u) => g.push_str(&format!(
                    "<a href=\"{u}\" target=\"_blank\"><tspan text-decoration=\"underline\">{}</tspan></a>",
                    esc(s)
                )),
                // spaces at a run's ends would collapse: keep them
                None => g.push_str(&format!("<tspan>{}</tspan>", esc(s).replace(' ', "\u{a0}"))),
            }
        }
        g.push_str("</text>");
    }
    g.push_str("</g>");
    g
}

/// A scale bar for the map's centre: `(length in px, "50 km")` — a "nice"
/// 1/2/5 × 10ⁿ distance at most 90 px long.
fn scale_bar(m: &Model, frame: &Frame) -> Option<(f64, String)> {
    let e = &m.lonlat;
    let (lon, lat) = ((e.x0 + e.x1) / 2.0, (e.y0 + e.y1) / 2.0);
    let dlon = 1.0 / (111.32 * lat.to_radians().cos().max(0.01));
    let a = m.proj.forward([lon, lat])?;
    let b = m.proj.forward([lon + dlon, lat])?;
    let px_km = ((b[0] - a[0]).hypot(b[1] - a[1])) * frame.s;
    if px_km.is_nan() || px_km <= 0.0 || !px_km.is_finite() {
        return None;
    }
    let max_km = 90.0 / px_km;
    let mag = 10f64.powf(max_km.log10().floor());
    let km = [5.0, 2.0, 1.0]
        .iter()
        .map(|f| f * mag)
        .find(|k| *k <= max_km)
        .unwrap_or(mag);
    let px = km * px_km;
    if px < 12.0 {
        return None;
    }
    let label = if km >= 1.0 {
        format!("{} km", km.round() as i64)
    } else {
        format!("{} m", (km * 1000.0).round() as i64)
    };
    Some((px, label))
}

/// Labels on the map (`::LABEL_TOP n`: the n most important; `::DATALABELS`:
/// all that fit): each region's name at its pole of inaccessibility, a
/// point's to its right, in the ink with a card-coloured halo; by `::RANK`,
/// else |value|, else area; none overlapping, none wider than its region.
fn labels_svg(
    o: &RenderOptions,
    cols: &[Column],
    m: &Model,
    frame: &Frame,
    blocked: &[(f64, f64, f64, f64)],
    size: (f64, f64),
    font: &str,
) -> String {
    let top = role_num(cols, Role::LabelTop).map(|n| n.max(0.0) as usize);
    if top.is_none() && find_role(cols, Role::DataLabels).is_none() {
        return String::new();
    }
    // flow maps (M2b): ::LABEL_TOP/::DATALABELS label the distinct ends,
    // not the arcs; priority is the sum of the sizes touching each end.
    if let Some(fl) = &m.flows {
        let mut ends: std::collections::HashMap<&str, (labels::Pt, f64, String)> =
            std::collections::HashMap::new();
        for f in &fl.flows {
            let w = f.raw.unwrap_or(f.w);
            let e = ends
                .entry(f.ok.as_str())
                .or_insert((frame.at(f.o), 0.0, f.o_name.clone()));
            e.1 += w;
            let e = ends
                .entry(f.dk.as_str())
                .or_insert((frame.at(f.d), 0.0, f.d_name.clone()));
            e.1 += w;
        }
        let cands: Vec<labels::Candidate> = ends
            .into_values()
            .filter(|(_, _, name)| !name.is_empty())
            .map(|(at, priority, text)| labels::Candidate {
                at,
                text,
                priority,
                room: None,
                dx: 5.0,
            })
            .collect();
        let placed = labels::place(cands, top, blocked, size, FONT);
        if placed.is_empty() {
            return String::new();
        }
        let t = &o.theme;
        let mut g = format!(
            "<g class=\"dp-map-labels\" font-family=\"{font}\" font-size=\"{FONT}\" font-weight=\"600\" fill=\"{}\" stroke=\"{}\" stroke-width=\"2.4\" stroke-linejoin=\"round\" paint-order=\"stroke\" pointer-events=\"none\">",
            hex(t.colors.ink),
            hex(t.colors.card)
        );
        for p in placed {
            g.push_str(&format!(
                "<text x=\"{:.1}\" y=\"{:.1}\" text-anchor=\"{}\" dominant-baseline=\"middle\">{}</text>",
                p.x,
                p.y,
                if p.start { "start" } else { "middle" },
                esc(&p.text)
            ));
        }
        g.push_str("</g>");
        return g;
    }
    let rank = find_role(cols, Role::Rank);
    let rmax_px = m.symbols.as_ref().map(|_| symbols::rmax(size.0, size.1));
    let mut cands = Vec::new();
    for (i, f) in m.features.iter().enumerate() {
        let text = m
            .label_of(i)
            .lines()
            .next()
            .unwrap_or("")
            .trim()
            .to_string();
        if text.is_empty() {
            continue;
        }
        let value = match &m.fill {
            Fill::Measure(s) => s.values[i],
            _ => None,
        };
        // a graduated symbol (M2a): anchor at the symbol, label to its
        // right offset by its radius + 3 px, skipping the area/point logic
        // below entirely (its anchor already is the symbol's)
        if let Some(sym) = &m.symbols {
            let Some(&pt) = sym.anchors[i].first() else {
                continue;
            };
            let Some(v) = sym.size[i] else { continue };
            let r = sym
                .fixed
                .unwrap_or_else(|| symbols::radius(rmax_px.unwrap_or(0.0), v, sym.vmax));
            let priority = rank
                .and_then(|r| r.values.get(i))
                .and_then(cell_num)
                .or(value.map(f64::abs))
                .unwrap_or(v);
            cands.push(labels::Candidate {
                at: frame.at(pt),
                text,
                priority,
                room: None,
                dx: r + 3.0,
            });
            continue;
        }
        if f.is_empty() {
            continue;
        }
        // the largest polygon on screen, else the first point
        let mut best: Option<(f64, Vec<Vec<labels::Pt>>)> = None;
        for s in f {
            if let Shape::Area(rings) = s {
                let scr: Vec<Vec<labels::Pt>> = rings
                    .iter()
                    .map(|r| {
                        let step = (r.len() / 1500).max(1);
                        r.iter().step_by(step).map(|p| frame.at(*p)).collect()
                    })
                    .collect();
                let a = labels::area(&scr[0]);
                if best.as_ref().is_none_or(|b| a > b.0) {
                    best = Some((a, scr));
                }
            }
        }
        let (at, room, area) = match best {
            Some((a, rings)) => {
                let Some((p, _)) = labels::polylabel(&rings, 1.0) else {
                    continue;
                };
                let (x0, x1) = rings[0]
                    .iter()
                    .fold((f64::MAX, f64::MIN), |(a, b), q| (a.min(q.0), b.max(q.0)));
                (p, Some(x1 - x0), a)
            }
            None => match f.iter().find_map(|s| match s {
                Shape::Point(p) => Some(frame.at(*p)),
                _ => None,
            }) {
                Some(p) => (p, None, 0.0),
                None => continue,
            },
        };
        let priority = rank
            .and_then(|r| r.values.get(i))
            .and_then(cell_num)
            .or(value.map(f64::abs))
            .unwrap_or(area);
        cands.push(labels::Candidate {
            at,
            text,
            priority,
            room,
            dx: 5.0,
        });
    }
    let placed = labels::place(cands, top, blocked, size, FONT);
    if placed.is_empty() {
        return String::new();
    }
    let t = &o.theme;
    let mut g = format!(
        "<g class=\"dp-map-labels\" font-family=\"{font}\" font-size=\"{FONT}\" font-weight=\"600\" fill=\"{}\" stroke=\"{}\" stroke-width=\"2.4\" stroke-linejoin=\"round\" paint-order=\"stroke\" pointer-events=\"none\">",
        hex(t.colors.ink),
        hex(t.colors.card)
    );
    for p in placed {
        g.push_str(&format!(
            "<text x=\"{:.1}\" y=\"{:.1}\" text-anchor=\"{}\" dominant-baseline=\"middle\">{}</text>",
            p.x,
            p.y,
            if p.start { "start" } else { "middle" },
            esc(&p.text)
        ));
    }
    g.push_str("</g>");
    g
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
