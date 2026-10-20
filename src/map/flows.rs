//! Flow maps (M2b): `::ORIGIN` / `::DEST` arcs between two places, widths by
//! `::SIZE`, colour by a measure or `::CATEGORY`.

use ggplot_rs::prelude::Value;
use serde_json::Value as J;

use crate::format::ValueFmt;
use crate::{cell_num, find_role, value_fmt, value_str, Column, Kind, Role};

use super::boundaries;
use super::legend::legend_title;
use super::project::Proj;
use super::symbols::anchors_of;

/// One resolved end of a flow.
pub(super) enum End {
    Code(String),
    /// `[lon, lat]`, optional name.
    Point([f64; 2], Option<String>),
    Missing,
}

impl std::fmt::Debug for End {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            End::Code(c) => write!(f, "Code({c})"),
            End::Point(p, n) => write!(f, "Point({p:?}, {n:?})"),
            End::Missing => write!(f, "Missing"),
        }
    }
}

/// Parse one `::ORIGIN`/`::DEST` cell. The struct-as-JSON convention: a
/// `Value::Str` is JSON when it starts with `{` (DuckDB structs arrive as
/// serialized JSON text, see `src/sql.rs`).
pub(super) fn end_of(v: &Value) -> End {
    match v {
        Value::Na => End::Missing,
        Value::Str(s) => {
            let t = s.trim();
            if t.starts_with('{') {
                if let Ok(J::Object(o)) = serde_json::from_str::<J>(t) {
                    let num = |keys: &[&str]| -> Option<f64> {
                        o.iter()
                            .find(|(k, _)| keys.iter().any(|w| k.eq_ignore_ascii_case(w)))
                            .and_then(|(_, v)| {
                                v.as_f64()
                                    .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
                            })
                    };
                    let name = o
                        .iter()
                        .find(|(k, _)| k.eq_ignore_ascii_case("name"))
                        .and_then(|(_, v)| v.as_str())
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty());
                    if let (Some(lon), Some(lat)) = (num(&["lon", "lng", "x"]), num(&["lat", "y"]))
                    {
                        if lon.is_finite() && lat.is_finite() {
                            return End::Point([lon, lat], name);
                        }
                    }
                }
                return End::Missing;
            }
            if t.len() >= 5 && t[..5].eq_ignore_ascii_case("POINT") {
                if let Some(ggplot_rs::spatial::Geometry::Point(p)) =
                    ggplot_rs::spatial::parse_wkt(t)
                {
                    return End::Point(p, None);
                }
                return End::Missing;
            }
            let c = t.to_string();
            if c.is_empty() {
                End::Missing
            } else {
                End::Code(c)
            }
        }
        v => match cell_num(v) {
            Some(_) => End::Code(value_str(v)),
            None => End::Missing,
        },
    }
}

/// One row's flow before projection (lon/lat space).
pub(super) struct RawFlow {
    pub row: usize,
    pub o: [f64; 2],
    pub d: [f64; 2],
    pub o_name: String,
    pub d_name: String,
    /// End key: a region code (upper-cased), or `"{lat:.5},{lon:.5}"`.
    pub ok: String,
    pub dk: String,
    /// The raw `::SIZE` value, finite (filtering happens in [`project`]).
    pub size: Option<f64>,
}

/// The raw (unprojected) flow model.
pub(super) struct RawFlows {
    pub rows: usize,
    pub flows: Vec<RawFlow>,
    pub unplaced: usize,
    pub self_flows: usize,
    pub layer: Option<String>,
    pub has_size: bool,
    pub fixed: Option<f64>,
    pub fmt: ValueFmt,
    pub title: Option<String>,
}

/// A `::SIZE` column holding one number on every row, clamped 0.5..12 px (a
/// flow's constant width; `symbols.rs`'s `constant_size` clamps 1..12
/// instead for a symbol radius, so this is its own small helper).
fn constant_width(col: &Column) -> Option<f64> {
    let first = col.values.first()?.as_f64().filter(|f| f.is_finite())?;
    col.values
        .iter()
        .all(|v| v.as_f64() == Some(first))
        .then_some(first.clamp(0.5, 12.0))
}

/// Build the raw flow model from `::ORIGIN`/`::DEST` (`None` only when one
/// of the two role columns is missing — the lint, not this function,
/// reports `::ORIGIN` without `::DEST`).
pub(super) fn of(cols: &[Column], german: bool) -> Option<RawFlows> {
    let oc = find_role(cols, Role::Origin)?;
    let dc = find_role(cols, Role::Dest)?;
    let rows = oc.values.len().max(dc.values.len());
    let forced = crate::role_str(cols, Role::Boundaries)
        .as_deref()
        .and_then(super::layer_name);
    let o_ends: Vec<End> = (0..rows)
        .map(|i| oc.values.get(i).map(end_of).unwrap_or(End::Missing))
        .collect();
    let d_ends: Vec<End> = (0..rows)
        .map(|i| dc.values.get(i).map(end_of).unwrap_or(End::Missing))
        .collect();
    let as_code = |e: &End| match e {
        End::Code(c) => Some(c.clone()),
        _ => None,
    };
    let mut all_codes: Vec<Option<String>> = o_ends.iter().map(as_code).collect();
    all_codes.extend(d_ends.iter().map(as_code));
    let any_codes = all_codes.iter().any(Option::is_some);
    let matched = any_codes
        .then(|| boundaries::match_codes(&all_codes, forced))
        .flatten();
    let layer = matched.as_ref().map(|m| m.layer.name.clone());

    let resolve = |e: &End, code_idx: usize| -> Option<([f64; 2], String, String)> {
        match e {
            End::Missing => None,
            End::Point(p, name) => {
                let label = name.clone().unwrap_or_else(|| {
                    let (lat, lon) = (p[1], p[0]);
                    format!(
                        "{:.2}\u{b0} {}, {:.2}\u{b0} {}",
                        lat.abs(),
                        if lat < 0.0 { "S" } else { "N" },
                        lon.abs(),
                        if lon < 0.0 { "W" } else { "E" }
                    )
                });
                let key = format!("{:.5},{:.5}", p[1], p[0]);
                Some((*p, label, key))
            }
            End::Code(c) => {
                let m = matched.as_ref()?;
                let ids = m.rows.get(code_idx)?;
                if ids.is_empty() {
                    return None;
                }
                let shapes: Vec<super::Shape> = ids
                    .iter()
                    .flat_map(|i| m.layer.regions[*i].shapes.iter().cloned())
                    .collect();
                let anchor = *anchors_of(&shapes).first()?;
                let name = match ids.as_slice() {
                    [i] => m.layer.regions[*i].name(german).to_string(),
                    _ => c.clone(),
                };
                Some((anchor, name, c.to_uppercase()))
            }
        }
    };

    let size_col = find_role(cols, Role::Size);
    let has_size = size_col.is_some_and(|c| c.values.iter().any(|v| cell_num(v).is_some()));
    let fixed = size_col.filter(|_| has_size).and_then(constant_width);
    let fmt = size_col
        .map(|c| value_fmt(cols, Kind::Bar, &c.values))
        .unwrap_or(ValueFmt::Plain);
    let title = size_col.and_then(|c| legend_title(&c.name, &fmt));

    let mut flows = Vec::new();
    let (mut unplaced, mut self_flows) = (0usize, 0usize);
    for i in 0..rows {
        let o = resolve(&o_ends[i], i);
        let d = resolve(&d_ends[i], rows + i);
        let (Some((op, oname, ok)), Some((dp, dname, dk))) = (o, d) else {
            unplaced += 1;
            continue;
        };
        if ok == dk {
            self_flows += 1;
            continue;
        }
        let size = size_col.and_then(|c| c.values.get(i)).and_then(cell_num);
        flows.push(RawFlow {
            row: i,
            o: op,
            d: dp,
            o_name: oname,
            d_name: dname,
            ok,
            dk,
            size,
        });
    }
    Some(RawFlows {
        rows,
        flows,
        unplaced,
        self_flows,
        layer,
        has_size,
        fixed,
        fmt,
        title,
    })
}

/// A projected flow, ready to draw.
pub(super) struct Flow {
    pub row: usize,
    pub o: [f64; 2],
    pub d: [f64; 2],
    pub o_name: String,
    pub d_name: String,
    pub ok: String,
    pub dk: String,
    /// Resolved stroke width, px.
    pub w: f64,
    /// Raw `::SIZE` value (for `data-size`/`data-fsize`); `None` without a
    /// size column.
    pub raw: Option<f64>,
}

pub(super) struct Flows {
    /// Sorted by `w` descending (ties by row); capped to 10,000.
    pub flows: Vec<Flow>,
    /// Count before the 10,000 cap (for the render warning).
    pub total: usize,
    pub unplaced: usize,
    pub self_flows: usize,
    pub vmax: f64,
    pub fixed: Option<f64>,
    pub has_size: bool,
    pub fmt: ValueFmt,
    pub title: Option<String>,
    /// The matched boundary layer, if any end was a code (kept for tests;
    /// `Model::new` reads `RawFlows::layer` directly to set up the backdrop).
    #[allow(dead_code)]
    pub layer: Option<String>,
}

/// Flows above this many are capped, keeping the widest (plan "Caps").
pub(crate) const FLOW_CAP: usize = 10_000;

/// Project `raw`'s ends, resolve widths (fixed, 1.5 without `::SIZE`, else
/// `0.75 + 7.25*sqrt(v/vmax)` for a finite `v > 0`), sort largest-first, cap
/// at [`FLOW_CAP`].
pub(super) fn project(raw: RawFlows, proj: &Proj) -> Flows {
    let vmax = raw
        .flows
        .iter()
        .filter_map(|f| f.size)
        .filter(|v| v.is_finite() && *v > 0.0)
        .fold(0.0_f64, f64::max);
    let width = |size: Option<f64>| -> Option<f64> {
        if let Some(f) = raw.fixed {
            return Some(f);
        }
        if !raw.has_size {
            return Some(1.5);
        }
        let v = size.filter(|v| v.is_finite() && *v > 0.0)?;
        Some(0.75 + 7.25 * (v / vmax).sqrt())
    };
    let mut flows: Vec<Flow> = raw
        .flows
        .iter()
        .filter_map(|f| {
            let w = width(f.size)?;
            let o = proj.forward(f.o)?;
            let d = proj.forward(f.d)?;
            Some(Flow {
                row: f.row,
                o,
                d,
                o_name: f.o_name.clone(),
                d_name: f.d_name.clone(),
                ok: f.ok.clone(),
                dk: f.dk.clone(),
                w,
                raw: f.size,
            })
        })
        .collect();
    flows.sort_by(|a, b| b.w.total_cmp(&a.w).then(a.row.cmp(&b.row)));
    let total = flows.len();
    flows.truncate(FLOW_CAP);
    Flows {
        flows,
        total,
        unplaced: raw.unplaced,
        self_flows: raw.self_flows,
        vmax,
        fixed: raw.fixed,
        has_size: raw.has_size,
        fmt: raw.fmt,
        title: raw.title,
        layer: raw.layer,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn end_of_parses_struct_wkt_and_plain() {
        let s = Value::Str("{'lon': 9.18, 'lat': 48.78, 'name': 'Stuttgart'}".replace('\'', "\""));
        match end_of(&s) {
            End::Point([lon, lat], Some(name)) => {
                assert!((lon - 9.18).abs() < 1e-9);
                assert!((lat - 48.78).abs() < 1e-9);
                assert_eq!(name, "Stuttgart");
            }
            other => panic!("expected a named point, got {other:?}"),
        }
        match end_of(&Value::Str("POINT(9.18 48.78)".to_string())) {
            End::Point([lon, lat], None) => {
                assert!((lon - 9.18).abs() < 1e-9);
                assert!((lat - 48.78).abs() < 1e-9);
            }
            other => panic!("expected an unnamed point, got {other:?}"),
        }
        match end_of(&Value::Str("DE-BY".to_string())) {
            End::Code(c) => assert_eq!(c, "DE-BY"),
            other => panic!("expected a code, got {other:?}"),
        }
        assert!(matches!(end_of(&Value::Na), End::Missing));
        match end_of(&Value::Float(9162.0)) {
            End::Code(c) => assert_eq!(c, "9162"),
            other => panic!("expected a numeric code, got {other:?}"),
        }
    }
}
