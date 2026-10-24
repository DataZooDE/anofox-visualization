//! #420 slice 1+2+3: build a plain polygon/point choropleth (measure,
//! category or classed fill, graduated `::SIZE` symbols; no flows/hexbin —
//! [`eligible`]) as one real `GGPlot`, instead of our own hand-written SVG.
//!
//! Everything else (flows, hexbin, or any map that still needs the old
//! footer — a scale bar or pack attribution) stays on
//! [`super::render_with_model`] until its own slice.

use ggplot_rs::geom::sf::GeomSf;
use ggplot_rs::prelude::*;
use ggplot_rs::render::backend::FillPattern;
use ggplot_rs::scale::binned::ScaleFillBinned;
use ggplot_rs::scale::size::ScaleSizeContinuous;
use ggplot_rs::scale::Scale;
use ggplot_rs::theme::LegendPosition;

use crate::theme::mix;
use crate::{Column, RenderOptions};

use super::fill::MapFill;
use super::legend::legend_of;
use super::wkt::row_wkt;
use super::{foot_of, frame_of, legend_override, symbols, Fill, Model, Shape};

/// A `GGPlot::new`/`layer_data` DataFrame: `(column name, column values)`
/// pairs.
type Df = Vec<(String, Vec<Value>)>;

/// Whether `m` (already built from `cols`) is simple enough for the new
/// `geom_sf` path: no flows/hexbin (slice 4), and no scale bar/attribution
/// furniture (slice 5 — [`foot_of`]'s own height check, reused rather than
/// duplicated, since "does this map need a footer" already depends on pack
/// layers *and* the projection/span). `::CLASSES` (slice 2) and graduated
/// `::SIZE` symbols (slice 3) are both allowed — [`build`] routes a classed
/// measure through `ggplot_rs::scale::binned::ScaleFillBinned` instead of
/// [`MapFill`], and a symbol map's anchor rows through `GeomSf`'s own
/// `size` aesthetic (sipemu/ggplot-rs#194) instead of our hand-rolled
/// circle drawing.
pub(super) fn eligible(o: &RenderOptions, m: &Model, w: f64) -> bool {
    m.flows.is_none() && m.hexbin.is_none() && foot_of(o, m, w).h <= 0.0
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
        // A row with a graduated-symbol anchor (slice 3) takes over: its
        // own area/point geometry is suppressed here (`Value::Na` — the
        // same "no geometry, no mark" convention `GeomSf::draw` already
        // uses) and it's drawn instead, as a sized circle at its anchor,
        // by the separate symbols layer built after this loop. A row with
        // no anchor (a line, or `m.symbols` absent) draws here unchanged.
        let suppressed_by_symbols = m
            .symbols
            .as_ref()
            .is_some_and(|sym| !sym.anchors[i].is_empty());
        geometry.push(if suppressed_by_symbols {
            Value::Na
        } else {
            row_wkt(&m.features[i]).map(Value::Str).unwrap_or(Value::Na)
        });
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

    // ---- graduated symbols (`::SIZE`, slice 3): a second `geom_sf` layer,
    // one anchor point per qualifying row (the features layer above
    // already suppressed its own area/point geometry for these). `GeomSf`'s
    // own `size` aesthetic (sipemu/ggplot-rs#194) sizes each circle and
    // sorts largest-first automatically (D5) — no hand-rolled anchor-circle
    // drawing or sort needed. Built *before* `features_df` below moves
    // `fill_vals`/`labels`/`rows`/`seriess`/`xs`/`class_idxs` — this layer
    // reuses the same per-row fill/label/series/class values by cloning
    // them out first.
    let symbols_df_aes: Option<(Df, Aes)> = m.symbols.as_ref().and_then(|sym| {
        let idx: Vec<usize> = (0..n).filter(|&i| sym.size[i].is_some()).collect();
        if idx.is_empty() {
            return None;
        }
        let mut sym_geometry = Vec::with_capacity(idx.len());
        let mut sym_value = Vec::with_capacity(idx.len());
        let mut sym_size = Vec::with_capacity(idx.len());
        let mut sym_fsize = Vec::with_capacity(idx.len());
        let mut sym_title = Vec::with_capacity(idx.len());
        for &i in &idx {
            let v = sym.size[i].unwrap();
            let anchor_shapes: Vec<Shape> =
                sym.anchors[i].iter().map(|p| Shape::Point(*p)).collect();
            sym_geometry.push(row_wkt(&anchor_shapes).map(Value::Str).unwrap_or(Value::Na));
            let fsize = sym.fmt.full(v, &o.locale);
            // `data-value`: the fill measure when there is one, else the
            // `::SIZE` value itself — the old writer's own fallback
            // (`mod.rs`'s `let data_value = value.unwrap_or(c.v);`).
            let fill_measure_value = match &m.fill {
                Fill::Measure(s) => s.values[i],
                _ => None,
            };
            sym_value.push(Value::Float(fill_measure_value.unwrap_or(v)));
            sym_size.push(Value::Float(v));
            sym_fsize.push(Value::Str(fsize.clone()));
            // The tooltip: "label: value; Size: fsize" (a colour measure),
            // "label: fsize" (a label, no measure) or just "fsize" — plus
            // the classed suffix — the old writer's own composition
            // (`mod.rs`'s symbol-circle loop): `GeomSf`'s automatic
            // "label: value" tooltip doesn't know about a second (size)
            // channel, so this mark's whole tooltip is computed here too,
            // same as the classed one above.
            let label = m.label_of(i);
            let shown = match &m.fill {
                Fill::Measure(s) => match s.values[i] {
                    Some(v) => s.fmt.full(v, &o.locale),
                    None => "no data".to_string(),
                },
                Fill::Category { .. } => m.text(m.category, i),
                Fill::Plain(_) => String::new(),
            };
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
            if let (Some((_, c, _)), Value::Str(k)) = (classes, &class_idxs[i]) {
                if let Ok(k) = k.parse::<usize>() {
                    title.push_str(&format!(
                        " (class {} of {}: {})",
                        k,
                        c.len(),
                        c.label(k - 1, &|v| match &m.fill {
                            Fill::Measure(s) => s.fmt.short(v, &o.locale),
                            _ => String::new(),
                        })
                    ));
                }
            }
            sym_title.push(na_if_empty(title));
        }
        let select = |src: &[Value]| idx.iter().map(|&i| src[i].clone()).collect::<Vec<_>>();
        let has_fill_aes = !matches!(m.fill, Fill::Plain(_));
        let mut df: Vec<(String, Vec<Value>)> = vec![
            ("geometry".into(), sym_geometry),
            ("label".into(), select(&labels)),
            ("row".into(), select(&rows)),
            ("series".into(), select(&seriess)),
            ("value".into(), sym_value),
            ("xkey".into(), select(&xs)),
            ("class".into(), select(&class_idxs)),
            ("title".into(), sym_title),
            ("size".into(), sym_size),
            ("fsize".into(), sym_fsize),
        ];
        // Only when actually fill-mapped: `GeomSf::draw` (sipemu/ggplot-rs
        // #238) looks up its `data-fvalue` formatter by checking whether a
        // column literally named "fill" exists at all — not whether `fill`
        // is aes-mapped — so always including this column (even full of
        // `Value::Na` for a `Fill::Plain` symbol map) would silently steal
        // its fallback to `size`'s own formatter, the #238 fix's whole
        // point for a size-only graduated-symbol map.
        if has_fill_aes {
            df.push(("fill".into(), select(&fill_vals)));
        }
        let mut aes = Aes::new()
            .label("label")
            .group("row")
            .data_attr("row", "row")
            .data_attr("series", "series")
            .data_attr("value", "value")
            .data_attr("label", "label")
            .data_attr("x", "xkey")
            .data_attr("size", "size")
            .data_attr("fsize", "fsize")
            .tooltip("title");
        if classes.is_some() {
            aes = aes.data_attr("class", "class");
        }
        if has_fill_aes {
            aes = aes.fill("fill");
        }
        if sym.fixed.is_none() {
            aes = aes.size("size");
        }
        Some((df, aes))
    });

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

    // ---- the symbols layer (on top of everything else), if any — a
    // circle's border is the card colour (matching the old writer's
    // `stroke="{card}"`), not the polygon border `feature_edge`.
    if let Some((sym_df, sym_aes)) = symbols_df_aes {
        let fixed = m.symbols.as_ref().and_then(|s| s.fixed);
        let sym_geom = GeomSf {
            fill: plain_fill,
            color: card,
            alpha: m.alpha,
            point_size: fixed.unwrap_or(3.0),
            ..Default::default()
        };
        plot = plot
            .geom_sf_with(sym_geom)
            .layer_data(sym_df)
            .layer_aes(sym_aes);
        // A constant `::SIZE` (every row the same value, `symbols::of`'s
        // own `constant_size` convention) is a fixed radius, not a size
        // aesthetic — no scale, no size legend, matching the old writer
        // (`constant_size_sets_the_radius`).
        if fixed.is_none() {
            let rmax = symbols::rmax(w, h);
            let mut size_scale = ScaleSizeContinuous::new().area(rmax);
            size_scale.set_name(
                m.symbols
                    .as_ref()
                    .and_then(|s| s.title.clone())
                    .unwrap_or_default()
                    .as_str(),
            );
            plot = plot.scale_size(size_scale);
        }
    }

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
                let mut scale = ScaleFillBinned::new(Aesthetic::Fill, breaks, colors.clone())
                    .na_color(hatch_bg)
                    .na_pattern(FillPattern::stripe(hatch_fg, hatch_bg))
                    .na_label("No data");
                // The measure's own title, same as the old writer's
                // stepped-bar legend and `MapFill::measure`'s continuous
                // one — real `legend_margin` (below) gives it room to
                // render without clipping.
                scale.set_name(s.title.clone().unwrap_or_default().as_str());
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

    // `crate::void_theme` (pie/gauge/map/sparkline) zeroes `legend_margin`
    // — fine for those (no legend, or one positioned off the panel
    // entirely), wrong for a map's own legend: at `LegendPosition::Top`
    // (below) or the default `Right`, zero margin puts the first swatch
    // and any title flush against the panel's own top/left edge, clipped
    // by the panel card. Restore `Margin`'s own library default (10px each
    // side) on top of the void theme — a plain `Theme` field, not an SVG
    // post-process.
    let mut theme = crate::void_theme(o);
    theme.legend_margin = ggplot_rs::theme::Margin::default();
    plot = plot
        .coord_sf_with(CoordSf::new().xlim(x0, x1).ylim(y0, y1).expand(false))
        .theme(theme);
    // A discrete legend (`Fill::Category`, or a classed `Fill::Measure` —
    // `ScaleFillBinned::is_discrete()` is also `true`) goes at
    // `LegendPosition::Top` (plain ggplot-rs API, `is_horizontal` for
    // `Top`/`Bottom` is just the one swatch row the old writer also drew)
    // — the old writer put its legend above the map at full width;
    // ggplot-rs's own default (`Right`, a vertical column) narrows the map
    // a lot on a narrow (390px) panel, which `Top` avoids. Must come
    // *after* `.theme(..)`: that call sets the plot's whole `Theme` (incl.
    // `legend_position`), so setting it earlier would just get
    // overwritten. A continuous (`Fill::Measure`, no classes) legend stays
    // on ggplot-rs's default `Right` position for now: `legend_at` above
    // (`layout()`'s own 4-corner, vertex-density-aware pick, meant to
    // convert to `legend_position_inside`'s fraction) renders a continuous
    // legend's tick marks/labels at wildly wrong, off-canvas y positions
    // under `LegendPosition::Inside` — a real ggplot-rs bug (filed:
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
