//! #420 slice 1+2: build a plain polygon/point choropleth (measure,
//! category or classed fill; no flows, hex or symbols — [`eligible`]) as
//! one real `GGPlot`, instead of our own hand-written SVG.
//!
//! Everything else (flows, hexbin, symbols, or any map that still needs the
//! old footer — a scale bar or pack attribution) stays on
//! [`super::render_with_model`] until its own slice.

use ggplot_rs::geom::sf::GeomSf;
use ggplot_rs::prelude::*;
use ggplot_rs::render::backend::FillPattern;
use ggplot_rs::scale::binned::ScaleFillBinned;
use ggplot_rs::theme::LegendPosition;

use crate::theme::mix;
use crate::{Column, HouseTheme, RenderOptions};

use super::fill::MapFill;
use super::legend::legend_of;
use super::wkt::row_wkt;
use super::{foot_of, frame_of, legend_override, Fill, Model};

/// Whether `m` (already built from `cols`) is simple enough for the new
/// `geom_sf` path: no flows/hexbin (slice 4) or graduated symbols (slice
/// 3), and no scale bar/attribution furniture (slice 5 — [`foot_of`]'s own
/// height check, reused rather than duplicated, since "does this map need
/// a footer" already depends on pack layers *and* the projection/span).
/// `::CLASSES` (slice 2) is allowed — [`build`] routes a classed measure
/// through `ggplot_rs::scale::binned::ScaleFillBinned` instead of
/// [`MapFill`].
pub(super) fn eligible(o: &RenderOptions, m: &Model, w: f64) -> bool {
    m.flows.is_none() && m.hexbin.is_none() && m.symbols.is_none() && foot_of(o, m, w).h <= 0.0
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
    // The no-data hatch (`ScaleFillBinned::na_pattern`, #206) — same colours
    // as the old writer's `<pattern>` def (`mod.rs`'s `hatch_bg`/`hatch_fg`).
    let hatch_bg = mix(t.colors.muted, card, 0.85);
    let hatch_fg = mix(t.colors.muted, card, 0.35);
    let feature_edge = if matches!(m.fill, Fill::Plain(_)) {
        plain_edge
    } else {
        border
    };

    // A classed measure (`::CLASSES`): the per-row class index (1-based,
    // `data-class`) and the `(label, formatted value) -> title` tooltip
    // text the old writer composed (`mod.rs`'s own "label: value (class K
    // of N: boundary)" — `GeomSf`'s own automatic `label: value` tooltip
    // doesn't know about the class suffix, so this map's whole tooltip is
    // computed here and fed through `Aes::tooltip`, not left automatic).
    let classes = match &m.fill {
        Fill::Measure(s) => s.classes.as_ref(),
        _ => None,
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
    let mut class_idxs: Vec<Value> = Vec::with_capacity(n);
    let mut titles: Vec<Value> = Vec::with_capacity(n);
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
        // `data-class` (1-based) + the classed tooltip suffix — only when
        // this map has classes at all; `Classes::index` is the exact same
        // half-open-bin convention `ScaleFillBinned::bin_index` uses below
        // (both: an edge value belongs to the *upper* class, out-of-range
        // clamps to an end class), so the row's class and its legend
        // swatch always agree.
        let (class_idx, title) = match classes {
            Some((_, c, _)) => {
                let label = m.label_of(i);
                let shown = match &m.fill {
                    Fill::Measure(s) => match s.values[i] {
                        Some(v) => s.fmt.full(v, &o.locale),
                        None => "no data".to_string(),
                    },
                    _ => String::new(),
                };
                let mut title = match (label.is_empty(), shown.is_empty()) {
                    (false, false) => format!("{label}: {shown}"),
                    (false, true) => label.clone(),
                    (true, _) => shown,
                };
                let k = match &m.fill {
                    Fill::Measure(s) => s.values[i].map(|v| c.index(v.clamp(s.lo, s.hi))),
                    _ => None,
                };
                if let Some(k) = k {
                    title.push_str(&format!(
                        " (class {} of {}: {})",
                        k + 1,
                        c.len(),
                        c.label(k, &|v| match &m.fill {
                            Fill::Measure(s) => s.fmt.short(v, &o.locale),
                            _ => String::new(),
                        })
                    ));
                }
                (
                    k.map(|k| Value::Str((k + 1).to_string()))
                        .unwrap_or(Value::Na),
                    na_if_empty(title),
                )
            }
            None => (Value::Na, Value::Na),
        };
        class_idxs.push(class_idx);
        titles.push(title);
    }
    let features_df: Vec<(String, Vec<Value>)> = vec![
        ("geometry".into(), geometry),
        ("fill".into(), fill_vals),
        ("label".into(), labels),
        ("row".into(), rows),
        ("series".into(), seriess),
        ("value".into(), measure_vals),
        ("xkey".into(), xs),
        ("class".into(), class_idxs),
        ("title".into(), titles),
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
    if classes.is_some() {
        features_aes = features_aes.data_attr("class", "class").tooltip("title");
    }
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
        match &m.fill {
            // A classed measure: `ScaleFillBinned` (sipemu/ggplot-rs#180,
            // built for exactly this) bins against `classes.rs`'s own
            // edges/colours, tallies each bin's *trained* row count for the
            // legend ("10–50 (12)", `class_label`'s own format — no hand
            // rolled count text needed) and draws a `#206` pattern-filled
            // "no data" key for any `Value::Na` row, matching the old
            // writer's hatch. The break *maths* stays entirely ours
            // (`classes.rs`); only the scale/legend object changes.
            Fill::Measure(s) if s.classes.is_some() => {
                discrete_fill = true;
                let (_, c, colors) = s.classes.as_ref().unwrap();
                let breaks: Vec<f64> = std::iter::once(c.lo)
                    .chain(c.edges.iter().copied())
                    .chain(std::iter::once(c.hi))
                    .collect();
                // No legend title: `house_void`'s theme (`#459`) zeroes
                // `legend_margin.top` for `LegendPosition::Top`'s one-row
                // layout, so a non-empty title would draw at y=0 — cut off
                // above the panel. `MapFill::category`'s legend (already
                // shipped, #459) avoids this the same way, leaving its own
                // `name` empty — same choice here, not a new workaround.
                let scale = ScaleFillBinned::new(Aesthetic::Fill, breaks, colors.clone())
                    .na_color(hatch_bg)
                    .na_pattern(FillPattern::stripe(hatch_fg, hatch_bg))
                    .na_label("No data");
                plot = plot.scale_fill(scale);
            }
            // A continuous measure (no classes): stays on `MapFill`'s own
            // default legend placement — see the `legend_position` note
            // below (sipemu/ggplot-rs#225).
            Fill::Measure(s) => plot = plot.scale_fill(MapFill::measure((**s).clone(), o.locale)),
            Fill::Category { levels, .. } => {
                discrete_fill = true;
                plot = plot.scale_fill(MapFill::category(levels.clone()));
            }
            Fill::Plain(_) => unreachable!("has_fill_aes excludes Fill::Plain"),
        }
    }
    let _ = legend_at;

    plot = plot
        .coord_sf_with(CoordSf::new().xlim(x0, x1).ylim(y0, y1).expand(false))
        .house_void(o);
    // A discrete legend (`Fill::Category`, or a classed `Fill::Measure` —
    // `ScaleFillBinned::is_discrete()` is also `true`) goes at
    // `LegendPosition::Top` (plain ggplot-rs API, `is_horizontal` for
    // `Top`/`Bottom` is just the one swatch row the old writer also drew)
    // — the old writer put its legend above the map at full width;
    // ggplot-rs's own default (`Right`, a vertical column) narrows the map
    // a lot on a narrow (390px) panel, which `Top` avoids. Must come
    // *after* `.house_void(o)`: that call sets the plot's whole `Theme`
    // (incl. `legend_position`), so setting it earlier would just get
    // overwritten. A continuous (`Fill::Measure`, no classes) legend stays
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

    let mut attrs: Vec<(&'static str, String)> = vec![
        ("data-map", String::new()),
        ("data-proj", m.projection.name().to_string()),
    ];
    // `data-classes`: every class's short-formatted label, low to high, as
    // JSON (`mapTableHtml`'s "k of n: label" a11y row — `app.js:15454`) —
    // same shape as the old writer's own `data-classes` (`mod.rs`).
    if let Some((_, c, _)) = classes {
        let labels: Vec<String> = (0..c.len())
            .map(|k| {
                c.label(k, &|v| match &m.fill {
                    Fill::Measure(s) => s.fmt.short(v, &o.locale),
                    _ => String::new(),
                })
            })
            .collect();
        attrs.push((
            "data-classes",
            serde_json::to_string(&labels).unwrap_or_default(),
        ));
    }
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
