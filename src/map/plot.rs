//! #420 slice 1+2+3+4+5+6: build a plain polygon/point choropleth (measure,
//! category or classed fill; graduated `::SIZE` symbols; flows/hexbin as
//! extra layers; region labels, a scale bar and a pack attribution of any
//! line count, including the BKG/VG2500-style multi-line stacked note —
//! [`eligible`]) as one real `GGPlot`, instead of our own hand-written
//! SVG.
//!
//! The multi-line attribution uses `GeomAttribution::new(..).runs(..)`
//! for the first line and `.line(..)` for each further line (ggplot-rs
//! 0.31.0, sipemu/ggplot-rs#250) — same corner, stacked without
//! overlapping.
//!
//! What's still unported: labels anchored to a symbol/flow-end point
//! rather than a region polygon (`geom_sf_text`'s own job stops at
//! polygons — ggplot2's convention too; see its module doc — a point/line
//! anchor needs `geom_text`'s own offset/overlap support, not audited
//! this slice).
use ggplot_rs::geom::annotation_scale::{GeomAnnotationScale, GeomAttribution, MapCorner};
use ggplot_rs::geom::sf::GeomSf;
use ggplot_rs::geom::sf_text::{GeomSfText, SfLabelAnchor};
use ggplot_rs::prelude::*;
use ggplot_rs::scale::size::ScaleSizeContinuous;
use ggplot_rs::scale::Scale;
use ggplot_rs::theme::LegendPosition;

use crate::theme::mix;
use crate::{cell_num, find_role, role_num, Column, RenderOptions, Role};

use super::fill;
use super::legend::legend_of;
use super::overlay;
use super::wkt::row_wkt;
use super::{foot_of, frame_of, legend_override, symbols, Fill, Model, Shape};

/// A `GGPlot::new`/`layer_data` DataFrame: `(column name, column values)`
/// pairs.
type Df = Vec<(String, Vec<Value>)>;

/// [`build`]'s result: the plot, its extra root attributes (`data-map`,
/// …) and any extra warnings (a flow's over-cap/hexbin's over-budget
/// note — `MARK_WARN`'s own check in `mod.rs` only covers `m.rows`).
type Built = (GGPlot, Vec<(&'static str, String)>, Vec<String>);

/// Whether `m` (already built from `cols`) is simple enough for the new
/// `geom_sf` path. `::CLASSES` (slice 2) is allowed — [`build`] routes a
/// classed measure through `ggplot_rs::scale::binned::ScaleFillBinned`
/// instead of [`MapFill`]. Graduated `::SIZE` symbols (slice 3) are allowed
/// too — a symbol map's anchor rows route through `GeomSf`'s own `size`
/// aesthetic (sipemu/ggplot-rs#194) instead of our hand-rolled circle
/// drawing. Flows/hexbin (slice 4) are allowed as well — [`build`] folds
/// them in as extra layers on the same plot, through
/// [`overlay::flow_layers`]/[`overlay::hexbin_layer`], instead of a
/// separately-rendered-and-spliced fragment. A footer (slice 5) is allowed
/// too, now at any line count: a scale bar (`annotation_scale`) plus any
/// number of attribution lines (`annotation_attribution`/`.line(..)`,
/// ggplot-rs 0.31.0, sipemu/ggplot-rs#250) — [`foot_of`]'s own line list,
/// reused rather than duplicated, since "does this map need a footer, and
/// how many lines" already depends on pack layers *and* the
/// projection/span. A `de-*`/BKG pack's 2-line "© BKG … Datenquellen:" +
/// sources note stacks via `.line(..)` same as any other footer now.
pub(super) fn eligible(_o: &RenderOptions, _m: &Model, _w: f64) -> bool {
    true
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
) -> Option<Built> {
    let mut warnings: Vec<String> = Vec::new();
    let (w, h) = (width as f64, height as f64);
    let mut lg = legend_of(o, m, w, h);
    let forced = legend_override(cols, &mut lg);
    // `legend_override` zeroes `lg` for `::LEGEND 'none'` and returns
    // `None` — the same `None` it returns for "no override, pick a
    // default" (`lg` is also what `frame_of`'s own layout math reads).
    // `forced_position` below is the ggplot-rs-facing signal: `Some(...)`
    // wins over the discrete-fill default, with its own `None` variant
    // standing for "hide the legend" specifically, distinct from "no
    // override" (`Option::None` on this line).
    let forced_position = if lg.w <= 0.0 && lg.h <= 0.0 {
        Some(ggplot_rs::theme::LegendPosition::None)
    } else {
        forced.clone()
    };
    // `fill::scale_for` (much further down) decides between a discrete
    // (`Category`/classed `Measure`) and continuous `Measure` fill scale —
    // computed here too, early, only to size the footer below; the later
    // `discrete_fill` binding is the same expression, kept local to where
    // it drives the legend position/scale construction.
    let continuous_fill = !matches!(m.fill, Fill::Category { .. })
        && !matches!(&m.fill, Fill::Measure(s) if s.classes.is_some());
    // `eligible` only allows a footer this path can draw itself (a scale
    // bar and/or any number of attribution lines, via `annotation_scale`/
    // `annotation_attribution` below) — reuse the old writer's own height
    // calc so layout reserves exactly the same room it always did (the
    // `frame.window` trick frame_of callers rely on: fitting into a
    // shorter area then reporting the *unshrunk* `w`×`h` window stretches
    // the trained coord_sf domain so the unused band lands at the bottom,
    // without this geom_sf path needing its own pixel-perfect margin).
    // A continuous fill's legend lands at ggplot-rs's own default
    // `LegendPosition::Right` (below), not the old writer's horizontal
    // top bar `lg`/`foot_of` were both sized around — it isn't forced
    // elsewhere (`forced_position` wins when given) — consuming ~90px of
    // panel width that `foot_of`'s own room/shrink-to-fit math doesn't
    // know about unless told. Found by rendering a continuous-fill
    // `de-*` map at 390px width: the (now 2-line, since this slice)
    // BKG/VG2500 attribution overflowed past the panel's own left edge,
    // underneath the legend, without this. 90px is this build's own
    // measured reservation (bar + tick labels + margins, fairly stable
    // across locales/fonts since it's digits-only); there's no exposed
    // pre-render API for ggplot-rs's actual legend geometry to use
    // instead (same gap as the `legend_position_inside` one already
    // noted below, sipemu/ggplot-rs#225).
    let legend_reserve = if continuous_fill && forced_position.is_none() {
        90.0
    } else {
        0.0
    };
    let ft = foot_of(o, m, w - legend_reserve);
    let (frame, legend_at) = frame_of(m, o, &lg, w, h, ft.h, forced.clone())?;
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
    // `::BIVARIATE` (M2d): a second measure's raw/formatted value and the
    // composite class, plus the bivariate-specific tooltip (`Bivar::title_of`,
    // which replaces the generic classed/measure one built above).
    let mut value2s: Vec<Value> = Vec::with_capacity(n);
    let mut fvalue2s: Vec<Value> = Vec::with_capacity(n);
    let mut bivars: Vec<Value> = Vec::with_capacity(n);
    // `::HEXBIN` (M2c) takes over the point pass entirely — the per-row
    // features layer draws nothing (`m.fill`'s `ValueScale.values` is
    // indexed by *cell*, not row, when hexbin is active — same convention,
    // same comment, as the old writer's own skip, `mod.rs`'s per-feature
    // loop). Suppress every row's geometry/fill/class/title below rather
    // than index `m.fill` by row at all.
    let hexbin_takes_over = m.hexbin.is_some();
    // Whether any row actually keeps its own geometry here (`Value::Str`,
    // not suppressed to `Value::Na` by hexbin or a symbol anchor below) —
    // if every row is suppressed (hexbin always suppresses all of them;
    // an all-symbol-anchored map, slice 3, can too — #420 slice 5's own
    // lint-parity fix, see the features-layer gate below), the layer
    // would never draw a mark, only report `StatSf`'s own "removed N
    // rows" warning for every row.
    let mut any_feature_geometry = false;
    for i in 0..n {
        // A row with a graduated-symbol anchor (slice 3) takes over: its
        // own area/point geometry is suppressed here (`Value::Na` — the
        // same "no geometry, no mark" convention `GeomSf::draw` already
        // uses) and it's drawn instead, as a sized circle at its anchor,
        // by the separate symbols layer built after this loop. A row with
        // no anchor (a line, or `m.symbols` absent) draws here unchanged —
        // unless `::HEXBIN` (slice 4) takes over the whole pass instead.
        let suppressed_by_symbols = m
            .symbols
            .as_ref()
            .is_some_and(|sym| !sym.anchors[i].is_empty());
        geometry.push(if hexbin_takes_over || suppressed_by_symbols {
            Value::Na
        } else {
            match row_wkt(&m.features[i]) {
                Some(wkt) => {
                    any_feature_geometry = true;
                    Value::Str(wkt)
                }
                None => Value::Na,
            }
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
        let (fill, measure) = if hexbin_takes_over {
            (Value::Na, Value::Na)
        } else {
            match &m.fill {
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
                // `ScaleFillBivariate`/`fill::BivariateScale::map_to_color`
                // read a 1-indexed `"{i}-{j}"` composite class string
                // (biscale's own convention); `measure` carries the row's
                // own `a` value, same contract as `Fill::Measure` above, so
                // `data-value`/`data-fvalue` keep working unchanged.
                Fill::Bivariate(b) => (
                    match (b.ia[i], b.jb[i]) {
                        (Some(ii), Some(jj)) => Value::Str(format!("{}-{}", ii + 1, jj + 1)),
                        _ => Value::Na,
                    },
                    b.a[i].map(Value::Float).unwrap_or(Value::Na),
                ),
            }
        };
        fill_vals.push(fill);
        measure_vals.push(measure);
        // `data-class` (1-based) + the classed tooltip suffix — only when
        // this map has classes at all; `Classes::index` is the exact same
        // half-open-bin convention `ScaleFillBinned::bin_index` uses below
        // (both: an edge value belongs to the *upper* class, out-of-range
        // clamps to an end class), so the row's class and its legend
        // swatch always agree.
        let (class_idx, title) = match classes.filter(|_| !hexbin_takes_over) {
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
        if let Fill::Bivariate(b) = &m.fill {
            let label = m.label_of(i);
            titles.push(na_if_empty(b.title_of(i, &label, &o.locale)));
            match b.b[i] {
                Some(v) => {
                    value2s.push(Value::Float(v));
                    fvalue2s.push(Value::Str(b.fmt_b.full(v, &o.locale)));
                }
                None => {
                    value2s.push(Value::Na);
                    fvalue2s.push(Value::Na);
                }
            }
            bivars.push(match (b.ia[i], b.jb[i]) {
                (Some(ii), Some(jj)) => Value::Str(format!("{ii},{jj}")),
                _ => Value::Na,
            });
        } else {
            titles.push(title);
            value2s.push(Value::Na);
            fvalue2s.push(Value::Na);
            bivars.push(Value::Na);
        }
    }
    let has_bivariate = matches!(m.fill, Fill::Bivariate(_));

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
                // Never built alongside graduated symbols (`Model::new`
                // only builds `Fill::Bivariate` when `symbols.is_none()`).
                Fill::Plain(_) | Fill::Bivariate(_) => String::new(),
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
        ("value2".into(), value2s),
        ("fvalue2".into(), fvalue2s),
        ("bivar".into(), bivars),
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
    if has_bivariate {
        features_aes = features_aes
            .tooltip("title")
            .data_attr("value2", "value2")
            .data_attr("fvalue2", "fvalue2")
            .data_attr("bivar", "bivar");
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

    // `m.base` has one slot per row whether or not there's a real
    // backdrop (most slots are `[]`, no shapes at all) — `row_wkt`
    // returns `None` for an empty slot, so the *filtered* WKT list, not
    // `m.base`'s own (row-count-driven, near-always-non-empty) length, is
    // what actually tells us whether anything will be drawn here. Found
    // this slice: with no real backdrop, the old `!m.base.is_empty()`
    // check still picked this branch, building a `GGPlot` whose *plot-
    // level* data was a `"geometry"` column with zero actual rows — which
    // silently drew no `annotation_scale`/`annotation_attribution`
    // furniture at all later in this function (their own layers have no
    // override of their own, so they inherit a 0-row base) despite
    // reaching their own `add_geom` calls (confirmed directly, not
    // assumed). A map with a real backdrop was never affected (its base
    // WKT list is non-empty, so this was invisible until slice 5 added
    // furniture layers that have no row loop of their own to mask it).
    let base_wkt: Vec<Value> = m
        .base
        .iter()
        .filter_map(|f| row_wkt(f))
        .map(Value::Str)
        .collect();
    let has_base = !base_wkt.is_empty();

    // ---- the basemap layer (drawn first — under the features), if any —
    // then the features layer, on top. Skipped when `!any_feature_
    // geometry`: every row's own geometry was suppressed to `Value::Na`
    // above (hexbin always suppresses all of them; an all-symbol-
    // anchored map, every row's own anchor already drawn by the separate
    // symbols layer below, can too) — adding a `geom_sf` layer for an
    // all-`Na` `features_df` anyway would never draw a mark, only report
    // `StatSf`'s own "removed N rows containing missing values" warning
    // for every single row. Found via a native/browser lint parity
    // failure (`tests/web/lint_test.mjs`) on two different `de-states`-
    // backed statements (a hexbin one, then a graduated-symbols one with
    // every row anchored — the same bug, same fix) that resolved their
    // pack natively (2 BKG attribution lines, `eligible` keeps it on the
    // old writer) but not in the browser's lint pass (no pack registered
    // there yet, 0 lines, `eligible` routes it here instead) — exposing
    // this pre-existing slice-3/4 gap for the first time; not specific to
    // that resolution mismatch, so fixed for both suppression kinds
    // uniformly, not just hexbin. `features_df` still backs the plot's
    // own base data when there's no real basemap either (a non-empty
    // `GGData`, not the `0`-row kind `annotation_scale`/`annotation_
    // attribution` need to avoid, see their own comment below) — just
    // with no `geom_sf` layer of its own.
    let mut plot = if has_base {
        let base_df: Vec<(String, Vec<Value>)> = vec![("geometry".into(), base_wkt)];
        let mut p = GGPlot::new(base_df).aes(Aes::new()).geom_sf_with(GeomSf {
            fill: land,
            color: land_edge,
            alpha: 1.0,
            line_width: 0.5,
            ..Default::default()
        });
        if any_feature_geometry {
            p = p
                .geom_sf_with(features_geom())
                .layer_data(features_df)
                .layer_aes(features_aes);
        }
        p
    } else if !any_feature_geometry {
        GGPlot::new(features_df).aes(features_aes)
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

    // `fill::scale_for` builds whichever `Scale` the fill needs (classed:
    // `ScaleFillBinned`, sipemu/ggplot-rs#180; continuous/category:
    // `MapFill`) — `None` for `Fill::Plain`. Shared with `overlay`'s flow
    // `Color` aesthetic (#420 slice 4): a clone of the *same* scale, not a
    // second hand-resolved identity colour, so a flow map's colour legend
    // is real too, not just its features'.
    let discrete_fill = matches!(m.fill, Fill::Category { .. })
        || matches!(&m.fill, Fill::Measure(s) if s.classes.is_some());
    let mut fill_scale = fill::scale_for(&m.fill, o.locale, hatch_bg, hatch_fg);
    if let Fill::Measure(s) = &m.fill {
        if s.classes.is_some() {
            if let Some(scale) = &mut fill_scale {
                // The measure's own title, same as the old writer's
                // stepped-bar legend and `MapFill::measure`'s continuous
                // one — real `legend_margin` (below) gives it room to
                // render without clipping.
                scale.set_name(s.title.clone().unwrap_or_default().as_str());
            }
        }
    }
    // Clone for the flow layer below *before* `.scale_fill` consumes the
    // original — flows need their own copy to re-aim at `Color`.
    let flow_color_scale = m
        .flows
        .as_ref()
        .and_then(|_| fill_scale.as_ref().map(|s| s.clone_box()));
    if has_fill_aes {
        if let Some(scale) = fill_scale {
            plot = plot.scale_fill(fill::Reaimed(Aesthetic::Fill, scale));
        }
    }
    // `::BIVARIATE` (M2d): the ordinary per-aesthetic Fill legend doesn't
    // draw a 2-D grid, so it's hidden (`LegendPosition::None`, unless a
    // `::LEGEND` override already forced a position above) in favour of
    // `GeomBivariateLegend` — built from the *same* base/color_a/color_b
    // `bivariate.rs::of` used, so its 9 swatches match the features' exactly.
    let bivariate_legend = match &m.fill {
        Fill::Bivariate(b) if forced_position.is_none() => {
            use ggplot_rs::geom::bivariate_legend::GeomBivariateLegend;
            Some(GeomBivariateLegend {
                nx: 3,
                ny: 3,
                base: b.base,
                color_x: b.color_a,
                color_y: b.color_b,
                x_name: b.title_a.clone().unwrap_or_default(),
                y_name: b.title_b.clone().unwrap_or_default(),
                ..Default::default()
            })
        }
        _ => None,
    };
    if let Some(geom) = bivariate_legend {
        plot = plot.bivariate_legend(geom);
    }
    let _ = legend_at;

    // ---- flows (::ORIGIN/::DEST, M2b) and hexbin (::HEXBIN, M2c), slice
    // 4: extra layers on this same plot (overlay.rs's `flow_layers`/
    // `hexbin_layer`), not a separately-rendered-and-spliced fragment.
    let mut extra_self = 0usize; // self/unplaced flows, for the summary below
    if let Some(fl) = &m.flows {
        let plain_fill_for_flows = match &m.fill {
            Fill::Plain(c) => *c,
            _ => plain_fill,
        };
        let (new_plot, _n, extra, cap_warn) = overlay::flow_layers(
            plot,
            o,
            cols,
            m,
            &frame,
            fl,
            t,
            card,
            flow_color_scale,
            plain_fill_for_flows,
        );
        plot = new_plot;
        extra_self = extra;
        if let Some(w) = cap_warn {
            warnings.push(w);
        }
    }
    if let Some(hb) = &m.hexbin {
        if let Fill::Measure(scale) = &m.fill {
            let hex_color_scale = fill::scale_for(&m.fill, o.locale, hatch_bg, hatch_fg);
            let (new_plot, _n) = overlay::hexbin_layer(plot, o, hb, scale, card, hex_color_scale);
            plot = new_plot;
            if hb.cells.len() > super::hexbin::HEX_WARN {
                warnings.push(format!(
                    "map: {} hexagons — use a larger ::HEXBIN",
                    hb.cells.len()
                ));
            }
        }
        // The "Hexagons N across" note (M2c), restored via `.caption()`
        // (team-lead, 2026-10-24/25) rather than hand-drawn text — it went
        // missing when slice 4 moved the legend onto a real
        // `ScaleFillBinned` guide (`legend.rs`'s old `hexbin_note_extra`
        // stacked it under the swatches instead; a caption draws at the
        // bottom of the whole figure, not tucked under the legend — a
        // visible layout change, worth a before/after look, not a bug).
        let german = o.locale.tag.starts_with("de");
        let width_text = super::hexbin::fmt_width(hb.width, hb.planar);
        let note = if german {
            format!("Sechsecke {width_text} breit")
        } else {
            format!("Hexagons {width_text} across")
        };
        plot = plot.caption(&note);
    }

    // ---- region labels (`::LABEL_TOP`/`::DATALABELS`, slice 5): each
    // region's name at its pole of inaccessibility (`geom_sf_text`'s own
    // `SfLabelAnchor::Polylabel`, ggplot-rs's public port of our retired
    // `labels::polylabel`), haloed, dropping a lower-priority label that
    // collides, falls outside the panel or doesn't fit (`check_overlap`).
    // `geom_sf_text` only labels polygons (ggplot2's own convention, see
    // its module doc) — a symbol/flow-anchor label isn't this slice's
    // job (module doc above). `::LABEL_TOP n` additionally caps *which*
    // n regions are even offered to the geom (it has no count cap of its
    // own): same priority order the old `labels_svg` used — `::RANK`,
    // else |value|, else the region's own screen area (computed from
    // `frame` here, the same transform `labels_svg` used) — highest first.
    let top_n = role_num(cols, Role::LabelTop).map(|n| n.max(0.0) as usize);
    let want_datalabels = find_role(cols, Role::DataLabels).is_some();
    if (top_n.is_some() || want_datalabels) && !hexbin_takes_over {
        let rank_col = find_role(cols, Role::Rank);
        let mut order: Vec<(usize, f64)> = Vec::new();
        for i in 0..n {
            if m.features[i].is_empty() {
                continue;
            }
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
            let areas: Vec<&Shape> = m.features[i]
                .iter()
                .filter(|s| matches!(s, Shape::Area(_)))
                .collect();
            if areas.is_empty() {
                // geom_sf_text's own job stops at polygons — no point in
                // offering it a line/point row, it'll just skip it.
                continue;
            }
            let value = match &m.fill {
                Fill::Measure(s) => s.values[i],
                _ => None,
            };
            let rank_v = rank_col.and_then(|c| c.values.get(i)).and_then(cell_num);
            let area = areas
                .iter()
                .filter_map(|s| match s {
                    Shape::Area(rings) => rings.first().map(|r| {
                        let scr: Vec<(f64, f64)> = r.iter().map(|p| frame.at(*p)).collect();
                        super::labels::area(&scr)
                    }),
                    _ => None,
                })
                .fold(0.0, f64::max);
            let priority = rank_v.or(value.map(f64::abs)).unwrap_or(area);
            order.push((i, priority));
        }
        order.sort_by(|a, b| b.1.total_cmp(&a.1));
        let keep: std::collections::HashSet<usize> = match top_n {
            Some(k) => order.iter().take(k).map(|(i, _)| *i).collect(),
            None => order.iter().map(|(i, _)| *i).collect(),
        };
        if !keep.is_empty() {
            // Only the kept rows go into this layer's own data (`layer_data`
            // is independent of the base plot's row count, see
            // `GGPlot::layer_data`) — not all `n` rows padded with `Value::Na`
            // for the ones `::LABEL_TOP`/`::DATALABELS` capped out. Padding
            // with `Na` made `geom_sf_text`'s own `StatSf` report "removed N
            // rows containing missing values" for every capped row — real
            // for genuinely missing geometry, but spurious here: these rows
            // were already filtered into `order` *because* they had real
            // geometry, and are only absent from `keep` by our own
            // intentional cap, not a data problem. Found via a native/
            // browser lint parity failure on `delivery-coverage.sql`'s
            // `::LABEL_TOP 8` panel (373 of 381 districts capped, each
            // flagged as "missing" even though every one has a real
            // boundary). Ascending by row index, same relative order as the
            // old `(0..n)` map so `check_overlap`'s tie-breaking is
            // unaffected.
            let mut kept: Vec<usize> = keep.iter().copied().collect();
            kept.sort_unstable();
            let label_geo: Vec<Value> = kept
                .iter()
                .map(|&i| row_wkt(&m.features[i]).map(Value::Str).unwrap_or(Value::Na))
                .collect();
            let label_text: Vec<Value> = kept
                .iter()
                .map(|&i| {
                    na_if_empty(
                        m.label_of(i)
                            .lines()
                            .next()
                            .unwrap_or("")
                            .trim()
                            .to_string(),
                    )
                })
                .collect();
            let label_df: Df = vec![("geometry".into(), label_geo), ("label".into(), label_text)];
            let label_aes = Aes::new().label("label");
            plot = plot
                .geom_sf_text_with(GeomSfText {
                    size: super::FONT,
                    anchor: SfLabelAnchor::Polylabel,
                    check_overlap: true,
                    halo: Some((card, 1.4)),
                    ..Default::default()
                })
                .layer_data(label_df)
                .layer_aes(label_aes);
        }
    }

    // ---- the scale bar and the pack attribution (slice 5): drawn
    // straight into whatever panel rect the rest of this layout already
    // computed — neither reserves its own room (unlike `.caption()`
    // above), which is exactly why `ft.h` was fed into `frame_of` earlier
    // instead: that's what actually carves out the blank band these two
    // then draw into. No `.layer_data()`/`.layer_aes()` override on
    // either — they inherit whatever data/aes `plot`'s base carries, but
    // their own `draw()` never reads it (no `required_aes()`, no row
    // loop — just `backend.plot_area()` and the trained X scale), so the
    // inherited values are inert. Giving them an *empty* (0-row) override
    // instead was tried and tracked down to a real ggplot-rs rendering
    // gap: a layer whose data has 0 rows draws nothing at all, even when
    // its own `draw()` ignores `data` entirely — worse than inheriting,
    // not better, so it's not done here.
    if ft.scale {
        plot = plot.annotation_scale_with(
            GeomAnnotationScale::default()
                .corner(MapCorner::BottomLeft)
                .max_width_px(90.0),
        );
    }
    // `ft.lines` holds every attribution line this map needs, already in
    // top-to-bottom reading order (a `de-*`/BKG pack's own "© BKG …
    // Datenquellen:" line, then its sources line, or a `world` layer's
    // single "Made with Natural Earth" line). `GeomAttribution`'s row 0
    // (`.runs(..)`) is the row closest to the corner's own edge — for
    // `BottomRight` that's the *bottom* line, i.e. `ft.lines`' *last*
    // entry — and each further `.line(..)` call stacks one row further
    // away (upward), so feeding `ft.lines` in reverse here is what
    // reproduces the old writer's top-to-bottom order (ggplot-rs 0.31.0,
    // sipemu/ggplot-rs#250).
    if let Some((closest, farther)) = ft.lines.split_last() {
        fn as_runs<'a>(
            line: &'a [(String, Option<&'static str>)],
        ) -> Vec<(&'a str, Option<&'static str>)> {
            line.iter().map(|(s, u)| (s.as_str(), *u)).collect()
        }
        let mut attribution = GeomAttribution::new(String::new())
            .corner(MapCorner::BottomRight)
            .runs(as_runs(closest));
        for line in farther.iter().rev() {
            attribution = attribution.line(as_runs(line));
        }
        // `foot_of`'s own `ft.size` already shrinks the font to fit this
        // panel's width (incl. the scale bar's own reservation) — reuse
        // it instead of `GeomAttribution::new`'s fixed 7.5px default,
        // same as the old writer's `foot_svg` did. `text_size` is a plain
        // `pub` field; `#[non_exhaustive]` only blocks struct-literal
        // construction, not mutating fields on an already-built value.
        attribution.text_size = ft.size;
        plot = plot.annotation_attribution_with(attribution);
    }

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
    //
    // `::LEGEND none`/`'top'`/`'bottom'`/`'left'`/`'right'`
    // (`forced_position`, derived above from the same `lg`/`forced` that
    // feed `frame_of`'s own layout math) wins over the discrete-fill
    // default when given — found missing entirely while widening
    // `eligible` this slice: neither was ever actually applied to this
    // `GGPlot`'s own `LegendPosition` before, so `::LEGEND` silently did
    // nothing for any already-eligible map (every kind slices 1-4
    // shipped) — untested until this slice's broader eligibility finally
    // routed a `::LEGEND`-bearing map fixture through here for the first
    // time.
    if let Some(pos) = forced_position {
        plot = plot.legend_position(pos);
    } else if matches!(m.fill, Fill::Bivariate(_)) {
        // The dedicated `GeomBivariateLegend` furniture layer above is the
        // real legend; the ordinary per-aesthetic one would just list the
        // 9 composite classes as a vertical bar list.
        plot = plot.legend_position(LegendPosition::None);
    } else if discrete_fill {
        plot = plot.legend_position(LegendPosition::Top);
    }

    // The screen-reader summary (`super::summary`, shared by every map
    // kind: choropleth, classed, symbols, flows, hexbin) as a root
    // attribute, same text content as the old writer's own `data-summary`
    // — found missing here entirely while widening `eligible` this slice
    // (every map kind this path already drew, slices 1-4, was missing it
    // too; `GGPlot` has no `<desc>` API yet for the old writer's own
    // *child-element* `<desc>{summary}</desc>` though, so that part of
    // the a11y contract stays a real, reported gap across the whole
    // geom_sf path, not something this attribute alone fixes).
    let summary_text = super::summary(o, m, extra_self);
    let mut attrs: Vec<(&'static str, String)> = vec![
        ("data-map", String::new()),
        ("data-proj", m.projection.name().to_string()),
        ("data-summary", summary_text),
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
    // `data-hexbin`: the hex width (D7 — the zoom UI needs it anchored to
    // the unzoomed data box), same as the old writer's own `data-hexbin`.
    if let Some(hb) = &m.hexbin {
        attrs.push(("data-hexbin", super::hexbin::fmt_num(hb.width)));
    }
    Some((plot, attrs, warnings))
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
