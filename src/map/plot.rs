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
use ggplot_rs::theme::LegendPosition;

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
    let (frame, legend_at) = frame_of(m, o, &lg, w, h, 0.0, forced)?;
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
    let step_x = crate::find_role(cols, crate::Role::X);
    let mut geometry = Vec::with_capacity(n);
    let mut fill_vals = Vec::with_capacity(n);
    let mut measure_vals = Vec::with_capacity(n);
    let mut labels = Vec::with_capacity(n);
    let mut rows = Vec::with_capacity(n);
    let mut seriess = Vec::with_capacity(n);
    let mut xs = Vec::with_capacity(n);
    for i in 0..n {
        geometry.push(row_wkt(&m.features[i]).map(Value::Str).unwrap_or(Value::Na));
        let label = m.label_of(i);
        let label_is_empty = label.is_empty();
        let cat = m.text(m.category, i);
        let series = if cat.is_empty() { label.clone() } else { cat };
        rows.push(Value::Str(i.to_string()));
        seriess.push(na_if_empty(series));
        labels.push(na_if_empty(label));
        // `::XAXIS`/`::STORY` keying (the old writer's `step_x`/`mod.rs`
        // ~1449): an explicit `::XAXIS` value wins; else, a row with no
        // label falls back to its own row index so a story can still
        // step through unlabeled rows (a labeled row with no `::XAXIS`
        // gets no `data-x` at all — `Value::Na` omits the attribute).
        let x = step_x
            .and_then(|c| c.values.get(i))
            .and_then(crate::mark_key_text)
            .map(Value::Str)
            .or_else(|| label_is_empty.then(|| Value::Str(i.to_string())))
            .unwrap_or(Value::Na);
        xs.push(x);
        // `fill` (the scale input) and `value`/`data-value` (the *raw
        // measure* the browser's a11y table shows) are different things
        // for a `Fill::Category` map — only a `Fill::Measure` row has a
        // measure at all, so `data-value` stays absent otherwise (the old
        // writer's own contract, `map/mod.rs`'s `value: Option<f64>`).
        let (fill, measure) = match &m.fill {
            Fill::Measure(s) => {
                let v = s.values[i].map(Value::Float).unwrap_or(Value::Na);
                (v.clone(), v)
            }
            Fill::Category { of_row, levels } => (
                of_row[i]
                    .map(|k| Value::Str(levels[k].0.clone()))
                    .unwrap_or(Value::Na),
                Value::Na,
            ),
            Fill::Plain(_) => (Value::Na, Value::Na),
        };
        fill_vals.push(fill);
        measure_vals.push(measure);
    }
    let features_df: Vec<(String, Vec<Value>)> = vec![
        ("geometry".into(), geometry),
        ("fill".into(), fill_vals),
        ("label".into(), labels),
        ("row".into(), rows),
        ("series".into(), seriess),
        ("value".into(), measure_vals),
        ("xkey".into(), xs),
    ];
    let mut features_aes = Aes::new()
        .label("label")
        // A `fill`/`color` aesthetic makes ggplot-rs's stat/group pipeline
        // cluster same-category rows together for stacking-style stats;
        // a map has no such concept — every row draws independently, in
        // its own input order. Map `group` to the row index itself so
        // each row is its own group and the draw order this geom sees
        // matches `m.features`'s order exactly (`data-row` would still be
        // correct either way, but the *drawn* order must match too, e.g.
        // for `many_marks_warn`'s and the browser's row-order contract).
        .group("row")
        .data_attr("row", "row")
        .data_attr("series", "series")
        .data_attr("value", "value")
        .data_attr("label", "label")
        .data_attr("x", "xkey");
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
            .geom_sf_with(features_geom())
            .layer_data(features_df)
            .layer_aes(features_aes)
    } else {
        GGPlot::new(features_df)
            .aes(features_aes)
            .geom_sf_with(features_geom())
    };

    let mut discrete_fill = false;
    if has_fill_aes {
        discrete_fill = matches!(m.fill, Fill::Category { .. });
        plot = plot.scale_fill(match &m.fill {
            Fill::Measure(s) => MapFill::measure((**s).clone(), o.locale),
            Fill::Category { levels, .. } => MapFill::category(levels.clone()),
            Fill::Plain(_) => unreachable!("has_fill_aes excludes Fill::Plain"),
        });
    }
    let _ = legend_at;

    plot = plot
        .coord_sf_with(CoordSf::new().xlim(x0, x1).ylim(y0, y1).expand(false))
        .house_void(o);
    // A discrete (`Fill::Category`) legend goes at `LegendPosition::Top`
    // (plain ggplot-rs API, `is_horizontal` for `Top`/`Bottom` is just the
    // one swatch row the old writer also drew) — the old writer put its
    // legend above the map at full width; ggplot-rs's own default (`Right`,
    // a vertical column) narrows the map a lot on a narrow (390px) panel,
    // which `Top` avoids. Must come *after* `.house_void(o)`: that call sets
    // the plot's whole `Theme` (incl. `legend_position`), so setting it
    // earlier would just get overwritten. A continuous (`Fill::Measure`, no
    // classes — `eligible()` already excludes classed measures) legend stays
    // on ggplot-rs's default for now: `legend_at` above (`layout()`'s own
    // 4-corner, vertex-density-aware pick, meant to convert to
    // `legend_position_inside`'s fraction) renders a continuous legend's
    // tick marks/labels at wildly wrong, off-canvas y positions under
    // `LegendPosition::Inside` — a real ggplot-rs bug (filed:
    // sipemu/ggplot-rs#225), not a sign error here — so nothing calls
    // `.legend_position_inside()` yet for either fill kind.
    if discrete_fill {
        plot = plot.legend_position(LegendPosition::Top);
    }

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
