//! Tracks (`::TRACK`): thin panels stacked under a chart that share its x
//! axis exactly — a tick per event, a one-row heatmap per lane, or a small
//! line / bar of a feature (promotions, alarms, price) — so a viewer reads
//! them against the chart above.
//!
//! A track is a statement of its own right after the chart: `'Alarms'::TRACK`
//! (the value names it) with an `::XAXIS`, and optionally `::YAXIS` lanes, a
//! `::CATEGORY` colour, a `::LABEL` tooltip and a measure (`::HEATMAP` tiles,
//! or a line / bar / area / step / scatter). [`render_stack`] renders the
//! chart and its tracks at one width, sharing one plot area and x window:
//! the chart's own `GGPlot` and every track's are composed into one
//! single-column `PlotGrid` with `AxisCollect::Align` (sipemu/ggplot-rs#175)
//! and rendered together in a single pass — no render-measure-render loop,
//! and no layout internals of the plotting engine are needed. `Align` only
//! equalises every cell's plot-area margins; it leaves axis text alone and
//! doesn't touch the x domain (unlike `CollectX`, which forces every
//! non-bottom cell's x axis blank — wrong here, since the chart, not the
//! bottom track, is meant to keep its axis, as today). The shared x domain
//! is instead computed directly from the chart's and every track's own data
//! (the union, expanded 5% each side the same way ggplot-rs's own
//! continuous/date scales default to) and given to every cell as explicit
//! `scale_x_*` limits with `expand(0, 0)`, so no cell re-expands on top of
//! it.

use crate::{
    chart_theme, dz_scale, find_role, guard, role_str, value_fmt, value_str, Column, Kind, Panel,
    Place, RenderError, RenderOptions, Role,
};
use ggplot_rs::prelude::*;
use ggplot_rs::scale::Scale;
use ggplot_rs::theme::{ElementLine, ElementText, LegendPosition, ThemeUpdate};

/// The typical step between distinct x positions (their median gap).
pub(crate) fn x_step(values: &[Value]) -> Option<f64> {
    let mut xs: Vec<f64> = values
        .iter()
        .filter_map(Value::as_f64)
        .filter(|f| f.is_finite())
        .collect();
    xs.sort_by(f64::total_cmp);
    xs.dedup();
    let mut gaps: Vec<f64> = xs.windows(2).map(|w| w[1] - w[0]).collect();
    gaps.sort_by(f64::total_cmp);
    gaps.get(gaps.len() / 2).copied()
}

/// Is this statement a track (`::TRACK`)?
pub fn is_track(roles: &[(usize, Role)]) -> bool {
    roles.iter().any(|(_, r)| *r == Role::Track)
}

/// A track's natural height in px: a lane track 20 px per lane (+ padding),
/// a measure track 60 px.
pub fn natural_height(cols: &[Column]) -> u32 {
    let measure = cols.iter().find_map(|c| match c.role {
        Role::Value(k) => Some(k),
        _ => None,
    });
    if matches!(measure, Some(k) if k != Kind::Heatmap) {
        return 60;
    }
    let lanes = find_role(cols, Role::Y).map_or(1, |y| {
        let mut seen: Vec<String> = y
            .values
            .iter()
            .filter(|v| **v != Value::Na)
            .map(value_str)
            .collect();
        seen.sort();
        seen.dedup();
        seen.len().max(1)
    });
    (14 + 20 * lanes.min(40)) as u32
}

const TRACK_X: &str =
    "a ::TRACK needs a numeric or date ::XAXIS (it shares the x axis of the chart above)";

/// Render a track panel on its own (`render_inner`'s dispatch: `cols`
/// already `downsample`/`boxstats`-prepared by its caller,
/// [`crate::render_placed`]) — no chart above it to align to.
pub(crate) fn render(
    o: &RenderOptions,
    cols: &[Column],
    width: u32,
    height: u32,
) -> Result<Panel, String> {
    let plot = plot_for(o, cols, width, height, None)?;
    Ok(track_panel(plot, width, height))
}

/// The track dispatch core, shared by [`render`] (solo) and [`build`] (a
/// [`stack`] `PlotGrid` cell): ticks/tiles ([`lanes`]) or a line / bar /
/// area / step / scatter sparkline ([`measure`]). `step` is the chart's own
/// x step (a heatmap/tick tile's width), `None` solo.
fn plot_for(
    o: &RenderOptions,
    cols: &[Column],
    width: u32,
    height: u32,
    step: Option<f64>,
) -> Result<GGPlot, String> {
    // A track with no rows (nothing happened in the window: no orders, no
    // alarms) is an empty lane — the static pages get no columns at all
    // for an empty result, the browser columns without values.
    let none = Column::new("x", Role::X, Vec::new());
    let x = match find_role(cols, Role::X) {
        Some(x) => x,
        None if cols.iter().all(|c| c.values.is_empty()) => &none,
        None => return Err(TRACK_X.into()),
    };
    if x.values.iter().any(|v| matches!(v, Value::Str(_))) {
        return Err(TRACK_X.into());
    }
    let name = role_str(cols, Role::Track).unwrap_or_default();
    match cols.iter().find(|c| matches!(c.role, Role::Value(_))) {
        None => lanes(o, cols, x, None, &name, step),
        Some(v) if v.role == Role::Value(Kind::Heatmap) => lanes(o, cols, x, Some(v), &name, step),
        Some(v) => match v.role {
            Role::Value(
                kind @ (Kind::Line
                | Kind::Step
                | Kind::Area
                | Kind::Bar
                | Kind::Point
                | Kind::Smooth),
            ) => measure(o, kind, v, cols, &name, width, height),
            r => Err(format!(
                "a ::TRACK draws ticks, ::HEATMAP tiles or a ::LINECHART / ::STEP / \
                 ::AREACHART / ::BARCHART / ::SCATTER, not ::{}",
                r.token()
            )),
        },
    }
}

/// One track's plot for [`stack`]'s `PlotGrid` column: the same
/// `downsample`/`boxstats` prep every panel gets from
/// [`crate::render_placed`], then [`plot_for`]. Returns the columns
/// post-prep too, for the caller's own [`crate::finish_touches`] call once
/// the grid is rendered — [`stack`] itself adds the `data-track` root
/// attribute to the split-out fragment (a `PlotGrid` cell drops a plot's
/// own `svg_attr`, see [`crate::chart_plot_for_track`]'s doc comment).
fn build(
    o: &RenderOptions,
    cols: &[Column],
    width: u32,
    height: u32,
    step: Option<f64>,
) -> Result<(GGPlot, Vec<Column>), String> {
    let cols = crate::downsample::prepare(cols, o);
    let cols = crate::boxstats::prepare(cols)?;
    let plot = plot_for(o, &cols, width, height, step)?.svg_attr("data-track", "true");
    Ok((plot, cols))
}

/// The track look: no x axis (the chart above has it), no grid, no legend,
/// the lane names (or the track's name) as the y labels.
fn track_theme(name_shown: bool) -> ThemeUpdate {
    ThemeUpdate {
        axis_text_x: Some(ElementText::blank()),
        axis_title_x: Some(ElementText::blank()),
        axis_title_y: Some(ElementText::blank()),
        axis_text_y: (!name_shown).then(ElementText::blank),
        panel_grid_major: Some(ElementLine::blank()),
        legend_position: Some(LegendPosition::None),
        ..Default::default()
    }
}

/// Row `i` of an optional column (missing = NA).
fn at(c: Option<&Column>, i: usize) -> &Value {
    static NA: Value = Value::Na;
    c.and_then(|c| c.values.get(i)).unwrap_or(&NA)
}

/// Ticks (no measure) or tiles (`::HEATMAP`) in one lane per `::YAXIS` value
/// (one lane, the track's name, without it). A tile is one x step wide (the
/// chart's step when stacked, via `step`); a tick a sliver of it.
fn lanes(
    o: &RenderOptions,
    cols: &[Column],
    x: &Column,
    value: Option<&Column>,
    name: &str,
    step: Option<f64>,
) -> Result<GGPlot, String> {
    let lane = find_role(cols, Role::Y);
    let cat = find_role(cols, Role::Category).filter(|c| c.values.iter().any(|v| *v != Value::Na));
    let tip = find_role(cols, Role::Label);
    let fmt = value.map(|v| value_fmt(cols, Kind::Heatmap, &v.values));
    let num = |v: &Value| v.as_f64().filter(|f| f.is_finite());

    let rows: Vec<usize> = (0..x.values.len())
        .filter(|&i| num(&x.values[i]).is_some())
        .filter(|&i| match (cat, value) {
            (Some(c), _) => *at(Some(c), i) != Value::Na,
            (None, Some(v)) => num(at(Some(v), i)).is_some(),
            (None, None) => true,
        })
        .collect();
    let lane_of = |i: usize| match at(lane, i) {
        Value::Na if lane.is_some() => None,
        Value::Na => Some(name.to_string()),
        v => Some(value_str(v)),
    };
    let rows: Vec<usize> = rows.into_iter().filter(|&i| lane_of(i).is_some()).collect();
    // Lanes top to bottom in first-seen order.
    let mut levels: Vec<String> = Vec::new();
    for l in (0..x.values.len()).filter_map(lane_of) {
        if !levels.contains(&l) {
            levels.push(l);
        }
    }
    if levels.is_empty() {
        levels.push(name.to_string());
    }

    let xs: Vec<f64> = x.values.iter().filter_map(num).collect();
    let (lo, hi) = {
        let lo = xs.iter().copied().fold(f64::INFINITY, f64::min);
        let hi = xs.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        if lo.is_finite() {
            (lo, hi)
        } else {
            (0.0, 1.0)
        }
    };
    let step = step
        .or_else(|| x_step(&x.values))
        .filter(|s| *s > 0.0)
        .unwrap_or(((hi - lo) / 50.0).max(1.0));
    // A tick stays visible on a long axis: at least 1/600 of the span.
    let half = if value.is_some() {
        step / 2.0
    } else {
        (step * 0.15).max((hi - lo) / 1200.0)
    };
    let date = x.values.iter().any(|v| matches!(v, Value::DateTime(_)));
    let pos = |f: f64| {
        if date {
            Value::DateTime(f.round() as i64)
        } else {
            Value::Float(f)
        }
    };

    let pick = |f: &dyn Fn(usize) -> Value| -> Vec<Value> { rows.iter().map(|&i| f(i)).collect() };
    let mut data = vec![
        ("x".to_string(), pick(&|i| x.values[i].clone())),
        (
            "x0".to_string(),
            pick(&|i| pos(num(&x.values[i]).unwrap_or(0.0) - half)),
        ),
        (
            "x1".to_string(),
            pick(&|i| pos(num(&x.values[i]).unwrap_or(0.0) + half)),
        ),
        (
            "y".to_string(),
            pick(&|i| Value::Str(lane_of(i).unwrap_or_default())),
        ),
        (
            "label".to_string(),
            pick(&|i| {
                let text = match (at(tip, i), at(cat, i), value) {
                    (t, _, _) if *t != Value::Na => value_str(t),
                    (_, c, _) if *c != Value::Na => value_str(c),
                    (_, _, Some(v)) => match (num(at(Some(v), i)), &fmt) {
                        (Some(f), Some(fmt)) => fmt.full(f, &o.locale),
                        _ => String::new(),
                    },
                    _ => String::new(),
                };
                Value::Str(text)
            }),
        ),
    ];
    let mut aes = Aes::new()
        .x("x")
        .y("y")
        .xmin("x0")
        .xmax("x1")
        .label("label")
        // `data-fvalue` is ggplot-rs-reserved (0.21, only
        // `svg_formatted_values` may write it): `data-vtext` carries the
        // lane's text instead (same non-reserved name as the hexbin
        // overlay's own fix, `map/overlay.rs`); `web/app.js`'s markInfo()
        // reads it ahead of a raw `data-value`.
        .data_attr("vtext", "label");
    let filled = match (cat, value) {
        (Some(c), _) => {
            data.push(("fill".to_string(), pick(&|i| c.values[i].clone())));
            Some(dz_scale(o, Aesthetic::Fill, c))
        }
        _ => None,
    };
    let gradient = match (cat, value, &fmt) {
        (None, Some(v), Some(fmt)) => {
            let nums: Vec<f64> = rows.iter().filter_map(|&i| num(&v.values[i])).collect();
            let min = nums.iter().copied().fold(f64::INFINITY, f64::min);
            let max = nums.iter().copied().fold(f64::NEG_INFINITY, f64::max);
            let fill = crate::TileFill::of(o, cols, o.theme.seq_low(), min, max)?;
            data.push(("fill".to_string(), pick(&|i| v.values[i].clone())));
            Some((fill, fmt))
        }
        _ => None,
    };
    if filled.is_some() || gradient.is_some() {
        aes = aes.fill("fill");
    }

    // A blank layer trains the lanes and the x range, so a track without
    // events still shows its lanes.
    let frame = vec![
        ("x".to_string(), vec![pos(lo), pos(hi)]),
        (
            "y".to_string(),
            vec![Value::Str(levels[0].clone()), Value::Str(levels[0].clone())],
        ),
    ];
    let mut plot = GGPlot::new(data)
        .aes(aes)
        .geom_blank()
        .layer_data(frame)
        .layer_aes(Aes::new().x("x").y("y"));
    if !rows.is_empty() {
        plot = plot
            .geom_tile_with(GeomTile {
                fill: o.brand(),
                color: o.theme.colors.card,
                alpha: 1.0,
                width: 1.0,
                height: 0.72,
                line_width: if value.is_some() { 1.0 } else { 0.0 },
                ..Default::default()
            })
            // "lane: text" (the ::LABEL, category or value), not the
            // default tile tooltip's "x, lane: text" — the x is the chart's
            // own tooltip header already, and a category fill is a colour,
            // not a value (B2-5's tooltip template, replacing the old
            // `lane_tips` string surgery, #318 C2).
            .tooltip("{y}: {label}");
    }
    let mut limits = levels.clone();
    limits.reverse();
    plot = plot.scale_y_discrete(
        ScaleDiscrete::new().with_limits(limits.iter().map(String::as_str).collect()),
    );
    if let Some(s) = filled {
        plot = plot.scale_fill(s);
    }
    if let Some((fill, fmt)) = gradient {
        plot = fill.scale(plot, o, fmt);
    }
    let shown = levels.iter().any(|l| !l.is_empty());
    let plot = plot
        .theme(chart_theme(o, 11.0))
        .theme_update(track_theme(shown));
    Ok(plot)
}

/// A line / bar / area / step / scatter track: the chart's own marks, its
/// name as the one y label (values are in the tooltips).
fn measure(
    o: &RenderOptions,
    kind: Kind,
    value: &Column,
    cols: &[Column],
    name: &str,
    width: u32,
    height: u32,
) -> Result<GGPlot, String> {
    let nums: Vec<f64> = value
        .values
        .iter()
        .filter_map(Value::as_f64)
        .filter(|f| f.is_finite())
        .collect();
    let mid = match (
        nums.iter().copied().reduce(f64::min),
        nums.iter().copied().reduce(f64::max),
    ) {
        (Some(a), Some(b)) => (a + b) / 2.0,
        _ => 0.0,
    };
    match crate::render_cartesian(o, kind, value, cols, None, width, height)? {
        Panel::Plot { plot, .. } => {
            // This y scale (the lane name as its one break label) replaces
            // `cartesian`'s own — ggplot-rs keeps only the last one set per
            // aesthetic — so its `data-fvalue` formatter (B2-4, `::YFORMAT`)
            // has to come along too, or a track measure would lose it.
            let vfmt = crate::value_fmt(cols, kind, &value.values);
            let locale = o.locale;
            let plot = plot
                .scale_y_continuous(
                    ggplot_rs::scale::continuous::ScaleContinuous::new()
                        .with_breaks(vec![mid])
                        .with_labels(vec![name.to_string()])
                        .with_value_formatter(move |n: f64| vfmt.full(n, &locale)),
                )
                .theme_update(track_theme(!name.is_empty()));
            Ok(plot)
        }
        // Never happens in practice: every `Kind` `plot_for` dispatches
        // here to is a plain cartesian panel in `render_cartesian`.
        Panel::Own { .. } => Err("a ::TRACK measure chart must be a plain plot".to_string()),
    }
}

/// A track panel, marked for hosts (`data-track`), at a solo render's size.
fn track_panel(plot: GGPlot, width: u32, height: u32) -> Panel {
    let mut panel = Panel::plot(plot, width, height);
    if let Panel::Plot { attrs, .. } = &mut panel {
        attrs.push(("data-track", "true".to_string()));
    }
    panel
}

/// A chart and the tracks under it: `[chart, track 1, …]` SVG documents,
/// rendered at one `width` so that their plot areas and x windows line up. A
/// track height of 0 is its [`natural_height`]. Tracks under a chart without
/// a numeric / date x axis come back as error notes.
pub fn render_stack(
    chart: &[Column],
    tracks: &[(Vec<Column>, u32)],
    width: u32,
    height: u32,
    o: &RenderOptions,
) -> Result<Vec<String>, RenderError> {
    stack(chart, tracks, width, height, o, Place::Doc).map(|(svgs, _)| svgs)
}

/// The track heights of a stack (0 → natural).
pub fn heights(tracks: &[(Vec<Column>, u32)]) -> Vec<u32> {
    tracks
        .iter()
        .map(|(cols, h)| if *h == 0 { natural_height(cols) } else { *h })
        .collect()
}

/// The union of every finite, non-string x value across `chart` and every
/// track's own columns — pure data, no rendering — expanded 5% each side
/// the way ggplot-rs's own continuous/date scales default to (checked
/// against `ggplot-rs/src/scale/{continuous,datetime}.rs`: `expand: (0.05,
/// 0.0, …)`, and none of this crate's track-compatible chart kinds
/// override it). `None` when the chart's own x isn't numeric/date — the
/// same precondition each track's own x already needs ([`TRACK_X`]).
fn shared_x_domain(chart: &[Column], tracks: &[(Vec<Column>, u32)]) -> Option<(bool, f64, f64)> {
    let chart_x = find_role(chart, Role::X)?;
    if chart_x.values.iter().any(|v| matches!(v, Value::Str(_))) {
        return None;
    }
    let date = chart_x
        .values
        .iter()
        .any(|v| matches!(v, Value::DateTime(_)));
    let mut lo = f64::INFINITY;
    let mut hi = f64::NEG_INFINITY;
    let mut widen = |values: &[Value]| {
        for v in values {
            if let Some(f) = v.as_f64().filter(|f| f.is_finite()) {
                lo = lo.min(f);
                hi = hi.max(f);
            }
        }
    };
    widen(&chart_x.values);
    for (cols, _) in tracks {
        if let Some(x) = find_role(cols, Role::X) {
            widen(&x.values);
        }
    }
    if !(lo.is_finite() && hi.is_finite()) {
        return None;
    }
    let range = (hi - lo).max(0.0);
    Some((date, lo - range * 0.05, hi + range * 0.05))
}

/// Pin `plot`'s x scale to `(lo, hi)` (already expanded) with no further
/// expansion of its own — the last `scale_x_*` call wins (ggplot-rs), so
/// this overrides whatever the chart/track's own builder set.
fn with_shared_x(plot: GGPlot, date: bool, lo: f64, hi: f64) -> GGPlot {
    if date {
        let mut s = ggplot_rs::scale::datetime::ScaleDateTime::new();
        Scale::set_limits(&mut s, lo, hi);
        Scale::set_expand(&mut s, 0.0, 0.0);
        plot.scale_x_datetime(s)
    } else {
        plot.scale_x_continuous(
            ggplot_rs::scale::continuous::ScaleContinuous::new()
                .with_limits(lo, hi)
                .with_expand(0.0, 0.0),
        )
    }
}

/// [`render_stack`] at a given [`Place`] (the headless dashboard nests the
/// whole stack as one fragment, via a custom `place`); also returns the
/// track heights.
///
/// The chart's own `GGPlot` and every track's are built, then composed into
/// one single-column [`PlotGrid`] with [`AxisCollect::Align`] and rendered
/// together in **one** pass (sipemu/ggplot-rs#175) — the grid equalises
/// every cell's left/right plot-area margins to the widest requirement (a
/// long lane name's y-axis label, say) and gives a cell thinner than the
/// plotting engine's own 50px panel floor (a `::TRACK` strip) a centred
/// `aspect_ratio` so it comes out exactly as tall as its row — one internal
/// layout pass per cell, not a render-measure-render loop. `Align` leaves
/// axis text and the x domain alone (unlike `CollectX`, which would force
/// every non-bottom cell's x axis blank — wrong here: the chart keeps its
/// own axis, every track hides its own, as today); the shared x domain
/// instead comes from [`shared_x_domain`]/[`with_shared_x`] above. The
/// merged SVG is then split back into one fragment per panel (`::TRACK`s
/// and the wasm/web host still address each one separately) and finished
/// exactly as a lone render would be.
///
/// A chart without a plain `GGPlot` panel (a diagram, map, table, KPI, …),
/// or one whose x isn't numeric/date (categorical bars, say), is left as
/// the chart rendered plainly plus an error note per track.
pub(crate) fn stack(
    chart: &[Column],
    tracks: &[(Vec<Column>, u32)],
    width: u32,
    height: u32,
    o: &RenderOptions,
    place: Place,
) -> Result<(Vec<String>, Vec<u32>), RenderError> {
    guard(|| {
        let hs = heights(tracks);
        if tracks.is_empty() {
            let svg = match place {
                Place::Doc => crate::render_with(chart, width, height, o)?,
                Place::At(x, y) => crate::render_with_at(chart, x, y, width, height, o)?,
            };
            return Ok((vec![svg], hs));
        }
        let Some((date, lo, hi)) = shared_x_domain(chart, tracks) else {
            return fallback_notes(chart, &hs, width, height, o, place);
        };
        let Some((chart_plot, chart_cols)) = crate::chart_plot_for_track(chart, width, height, o)?
        else {
            return fallback_notes(chart, &hs, width, height, o, place);
        };
        let chart_plot = with_shared_x(chart_plot, date, lo, hi);
        let step = find_role(chart, Role::X).and_then(|x| x_step(&x.values));
        let n = tracks.len();
        let mut track_cols: Vec<Vec<Column>> = Vec::with_capacity(n);
        let mut grid = PlotGrid::new().ncol(1).axis_collect(AxisCollect::Align);
        grid = grid.add(chart_plot);
        let mut weights = vec![height as f64];
        for ((cols, _), h) in tracks.iter().zip(&hs) {
            let (plot, prepared) = build(o, cols, width, *h, step).map_err(RenderError::Render)?;
            let plot = with_shared_x(plot, date, lo, hi);
            grid = grid.add(plot);
            track_cols.push(prepared);
            weights.push(*h as f64);
        }
        let total = height.saturating_add(hs.iter().sum());
        let (merged, warnings) = grid
            .heights(&weights)
            .render_svg_native_with_warnings(width, total)
            .map_err(|e| RenderError::Render(format!("render failed: {e:?}")))?;
        let frags = split_fragments(&merged, 1 + n)?;
        let panel_warnings = |tag: usize| -> Vec<String> {
            let prefix = format!("panel {tag}: ");
            warnings
                .iter()
                .filter_map(|w| w.strip_prefix(prefix.as_str()).map(str::to_string))
                .collect()
        };
        let mut out = Vec::with_capacity(1 + n);
        for (i, frag) in frags.iter().enumerate() {
            let cols: &[Column] = if i == 0 {
                &chart_cols
            } else {
                &track_cols[i - 1]
            };
            // Each cell's own `svg_attr`s (the chart's motion attrs, every
            // track's `data-track`) already landed on a `<g>` wrapping its
            // content when the grid rendered (sipemu/ggplot-rs#208) — no
            // string surgery needed here any more.
            let svg = repackage(frag, place);
            let svg = crate::finish_touches(svg, cols, o);
            let svg = match place {
                Place::Doc => {
                    crate::with_warnings_attr(svg, &panel_warnings(i + 1), o.record_warnings)
                }
                Place::At(..) => svg,
            };
            out.push(svg);
        }
        Ok((out, hs))
    })
}

/// `stack`'s fallback when the chart can't be aligned with: the chart
/// rendered plainly, plus the "needs a numeric or date x axis" note once
/// per track (unconditionally at [`Place::Doc`], matching the chart's own
/// `place` only for the chart itself — this mirrors the pre-#175
/// behaviour, which never positioned these notes for a `Place::At` host
/// either).
fn fallback_notes(
    chart: &[Column],
    hs: &[u32],
    width: u32,
    height: u32,
    o: &RenderOptions,
    place: Place,
) -> Result<(Vec<String>, Vec<u32>), RenderError> {
    let note = "a ::TRACK needs a chart with a numeric or date x axis above it";
    let main = match place {
        Place::Doc => crate::render_with(chart, width, height, o)?,
        Place::At(x, y) => crate::render_with_at(chart, x, y, width, height, o)?,
    };
    let notes = hs.iter().map(|_| crate::error_svg(note, width));
    Ok((std::iter::once(main).chain(notes).collect(), hs.to_vec()))
}

/// Split a [`PlotGrid`]'s merged, single-column SVG (one `data-panel="1"`,
/// `"2"`, … fragment per cell, in ggplot-rs's own nested `<svg x y ...>`
/// form) back into `n` fragments, in panel order.
fn split_fragments(merged: &str, n: usize) -> Result<Vec<String>, RenderError> {
    let body_start = merged.find('>').map_or(0, |i| i + 1);
    let body_end = merged.rfind("</svg>").unwrap_or(merged.len());
    let mut rest = &merged[body_start..body_end];
    let mut out = Vec::with_capacity(n);
    while let Some(start) = rest.find("<svg ") {
        let after = &rest[start..];
        let Some(close) = after.find("</svg>") else {
            break;
        };
        let end = close + "</svg>".len();
        out.push(after[..end].to_string());
        rest = &after[end..];
    }
    if out.len() != n {
        return Err(RenderError::Render(format!(
            "track stack: expected {n} panel(s) in the merged grid SVG, found {}",
            out.len()
        )));
    }
    Ok(out)
}

/// One split-out panel fragment — still ggplot-rs's own nested `<svg x=".."
/// y=".." ...>` form, its `(x, y)` relative to the grid's own top-left
/// corner — repackaged as the caller wants it: a standalone document
/// (`Place::Doc`), or shifted to sit at `(x, y)` in a parent SVG
/// (`Place::At(x, y)`, a dashboard card nesting the whole stack as one
/// fragment). The two forms are otherwise identical
/// (ggplot-rs `render/svg_backend.rs`'s `finish`/`finish_fragment`); an
/// unrecognised tag (should never happen — this is always ggplot-rs's own
/// output) is returned unchanged rather than panicking.
fn repackage(frag: &str, place: Place) -> String {
    let Some(gt) = frag.find('>') else {
        return frag.to_string();
    };
    let open = &frag[..gt];
    let Some(rest) = open.strip_prefix("<svg x=\"") else {
        return frag.to_string();
    };
    let Some(x_end) = rest.find('"') else {
        return frag.to_string();
    };
    let fx: f64 = rest[..x_end].parse().unwrap_or(0.0);
    let Some(y_rest) = rest[x_end + 1..].strip_prefix(" y=\"") else {
        return frag.to_string();
    };
    let Some(y_end) = y_rest.find('"') else {
        return frag.to_string();
    };
    let fy: f64 = y_rest[..y_end].parse().unwrap_or(0.0);
    let tail = &y_rest[y_end + 1..];
    match place {
        Place::Doc => format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\"{tail}{}",
            &frag[gt..]
        ),
        Place::At(ax, ay) => format!(
            "<svg x=\"{:.2}\" y=\"{:.2}\"{tail}{}",
            ax + fx,
            ay + fy,
            &frag[gt..]
        ),
    }
}
