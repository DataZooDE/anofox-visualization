//! #420 slice 1: build a plain polygon/point choropleth (measure or
//! category fill; no classes, flows, hex or symbols — [`eligible`]) as one
//! real `GGPlot`, instead of our own hand-written SVG.
//!
//! Everything else (classed, flows, hexbin, symbols, or any map that still
//! needs the old footer — a scale bar or pack attribution) stays on
//! [`super::render_with_model`] until its own slice.

// #420 WIP: not wired into `map::build()` (the lib.rs dispatch point) yet —
// nothing outside this module's own tests calls `build` or `eligible`.
#![allow(dead_code)]

use ggplot_rs::geom::sf::GeomSf;
use ggplot_rs::prelude::*;

use crate::theme::mix;
use crate::{Column, HouseTheme, RenderOptions};

use super::fill::MapFill;
use super::legend::legend_of;
use super::wkt::row_wkt;
use super::{foot_of, frame_of, legend_override, Fill, Model};

/// Whether `m` (already built from `cols`) is simple enough for the new
/// `geom_sf` path: no classes (slice 2), flows/hexbin (slice 4) or
/// graduated symbols (slice 3), and no scale bar/attribution furniture
/// (slice 5 — [`foot_of`]'s own height check, reused rather than
/// duplicated, since "does this map need a footer" already depends on
/// pack layers *and* the projection/span).
pub(super) fn eligible(o: &RenderOptions, m: &Model, w: f64) -> bool {
    m.flows.is_none()
        && m.hexbin.is_none()
        && m.symbols.is_none()
        && !matches!(&m.fill, Fill::Measure(s) if s.classes.is_some())
        && foot_of(o, m, w).h <= 0.0
}

/// Build the `GGPlot` and its extra root attributes (`data-map`, …) for an
/// [`eligible`] map. `None` only on the same degenerate-input cases
/// [`super::render_with_model`] itself bails out on (an empty fitted box).
pub(super) fn build(
    o: &RenderOptions,
    cols: &[Column],
    m: &Model,
    width: u32,
    height: u32,
) -> Option<(GGPlot, Vec<(&'static str, String)>)> {
    let (w, h) = (width as f64, height as f64);
    let mut lg = legend_of(o, m, w, h);
    let forced = legend_override(cols, &mut lg);
    // `eligible` already confirmed no footer, so its height is 0 here.
    let (frame, _legend_at) = frame_of(m, o, &lg, w, h, 0.0, forced)?;
    let [x0, x1, y0, y1] = frame.window(w, h);

    let t = &o.theme;
    let card = t.colors.card;
    let land = mix(t.colors.muted, card, 0.92);
    let land_edge = mix(t.colors.muted, card, 0.7);
    let plain_edge = mix(t.colors.muted, card, 0.35);
    let border = t.chart_grid();
    let feature_edge = if matches!(m.fill, Fill::Plain(_)) {
        plain_edge
    } else {
        border
    };

    // ---- the features layer: one row per `m.features`.
    let n = m.rows;
    let mut geometry = Vec::with_capacity(n);
    let mut fill_vals = Vec::with_capacity(n);
    let mut labels = Vec::with_capacity(n);
    let mut rows = Vec::with_capacity(n);
    let mut seriess = Vec::with_capacity(n);
    for i in 0..n {
        geometry.push(
            row_wkt(&m.features[i])
                .map(Value::Str)
                .unwrap_or(Value::Na),
        );
        let label = m.label_of(i);
        let cat = m.text(m.category, i);
        let series = if cat.is_empty() { label.clone() } else { cat };
        rows.push(Value::Str(i.to_string()));
        seriess.push(na_if_empty(series));
        labels.push(na_if_empty(label));
        fill_vals.push(match &m.fill {
            Fill::Measure(s) => s.values[i].map(Value::Float).unwrap_or(Value::Na),
            Fill::Category { of_row, levels } => of_row[i]
                .map(|k| Value::Str(levels[k].0.clone()))
                .unwrap_or(Value::Na),
            Fill::Plain(_) => Value::Na,
        });
    }
    let features_df: Vec<(String, Vec<Value>)> = vec![
        ("geometry".into(), geometry),
        ("fill".into(), fill_vals.clone()),
        ("label".into(), labels),
        ("row".into(), rows),
        ("series".into(), seriess),
        ("value".into(), fill_vals),
    ];
    let mut features_aes = Aes::new()
        .label("label")
        .data_attr("row", "row")
        .data_attr("series", "series")
        .data_attr("value", "value")
        .data_attr("label", "label");
    let has_fill_aes = !matches!(m.fill, Fill::Plain(_));
    if has_fill_aes {
        features_aes = features_aes.fill("fill");
    }
    let plain_fill = match &m.fill {
        Fill::Plain(c) => *c,
        _ => (97, 156, 255), // GeomSf's own default — never shown (fill aes always wins)
    };
    let features_geom = || GeomSf {
        fill: plain_fill,
        color: feature_edge,
        alpha: m.alpha,
        line_width: 0.6,
        ..Default::default()
    };
    let has_base = !m.base.is_empty();

    // ---- the basemap layer (drawn first — under the features), if any —
    // then the features layer, on top.
    let mut plot = if has_base {
        let base_wkt: Vec<Value> = m
            .base
            .iter()
            .filter_map(|f| row_wkt(f))
            .map(Value::Str)
            .collect();
        let base_df: Vec<(String, Vec<Value>)> = vec![("geometry".into(), base_wkt)];
        GGPlot::new(base_df)
            .aes(Aes::new())
            .geom_sf_with(GeomSf {
                fill: land,
                color: land_edge,
                alpha: 1.0,
                line_width: 0.5,
                ..Default::default()
            })
            .layer_data(features_df)
            .layer_aes(features_aes)
            .geom_sf_with(features_geom())
    } else {
        GGPlot::new(features_df)
            .aes(features_aes)
            .geom_sf_with(features_geom())
    };

    if has_fill_aes {
        plot = plot.scale_fill(match &m.fill {
            Fill::Measure(s) => MapFill::measure((**s).clone(), o.locale.clone()),
            Fill::Category { levels, .. } => MapFill::category(levels.clone()),
            Fill::Plain(_) => unreachable!("has_fill_aes excludes Fill::Plain"),
        });
    }

    plot = plot
        .coord_sf_with(CoordSf::new().xlim(x0, x1).ylim(y0, y1).expand(false))
        .house_void(o);

    let attrs: Vec<(&'static str, String)> = vec![
        ("data-map", String::new()),
        ("data-proj", m.projection.name().to_string()),
    ];
    Some((plot, attrs))
}

/// `Value::Na` for an empty string (our own "no label/series" convention),
/// else `Value::Str(s)`.
fn na_if_empty(s: String) -> Value {
    if s.is_empty() {
        Value::Na
    } else {
        Value::Str(s)
    }
}
