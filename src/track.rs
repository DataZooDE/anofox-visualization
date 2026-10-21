//! Tracks (`::TRACK`): thin panels stacked under a chart that share its x
//! axis exactly — a tick per event, a one-row heatmap per lane, or a small
//! line / bar of a feature (promotions, alarms, price) — so a viewer reads
//! them against the chart above.
//!
//! A track is a statement of its own right after the chart: `'Alarms'::TRACK`
//! (the value names it) with an `::XAXIS`, and optionally `::YAXIS` lanes, a
//! `::CATEGORY` colour, a `::LABEL` tooltip and a measure (`::HEATMAP` tiles,
//! or a line / bar / area / step / scatter). [`render_stack`] renders the
//! chart and its tracks at one width with the same plot area and x window:
//! each SVG is drawn, measured (its `data-plot` and `data-xdomain`) and drawn
//! again with its margins and x window corrected, so no layout internals of
//! the plotting engine are needed.

use crate::{
    chart_theme, dz_scale, find_role, guard, role_str, value_fmt, value_str, Column, Kind, Panel,
    RenderError, RenderOptions, Role,
};
use ggplot_rs::prelude::*;
use ggplot_rs::theme::{ElementLine, ElementText, LegendPosition, Margin, ThemeUpdate};

/// Where a panel's plot area and x window must be, to line up with others.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Align {
    /// Left edge of the plot area (px from the panel's left).
    pub left: f64,
    /// Right edge of the plot area (px from the panel's left).
    pub right: f64,
    /// The x domain to show edge to edge (the chart's expanded domain);
    /// `None` keeps the panel's own.
    pub xdomain: Option<(f64, f64)>,
    /// The chart's x step: a heatmap track's tile width.
    pub step: Option<f64>,
}

/// The margins and x window that put a panel's plot area where an [`Align`]
/// wants it.
#[derive(Clone, Debug)]
pub(crate) struct Fit {
    margin: Margin,
    xlim: Option<(f64, f64)>,
    /// Panel height / width, for a plot area lower than the plotting engine's
    /// 50 px minimum (a thin track).
    aspect: Option<f64>,
}

impl Fit {
    /// The first draw: house margins, the target domain as the x window.
    pub(crate) fn first(o: &RenderOptions, a: &Align) -> Fit {
        Fit {
            margin: chart_theme(o, 11.0).plot_margin,
            xlim: a.xdomain,
            aspect: None,
        }
    }

    /// The second draw, from what the first one drew (`svg`): the plot area
    /// moves by the margin difference (layout is linear in the margins) and
    /// the x window shrinks by the scale's expansion, so the drawn domain is
    /// the target. A margin cannot go below 0: a panel whose axis labels need
    /// more room keeps its larger left edge (see [`render_stack`]).
    ///
    /// The plotting engine keeps a plot area at least 50 px high; a thinner
    /// track (`height` px in all) gets its height from an aspect ratio
    /// instead, whose box (still 50 px, centred) moves up by the difference.
    pub(crate) fn correct(&self, a: &Align, svg: &str, height: f64) -> Fit {
        let mut margin = self.margin.clone();
        let mut aspect = None;
        if let Some([x, y, w, h]) = plot_rect(svg) {
            margin.left = (margin.left + a.left - x).max(0.0);
            margin.right = (margin.right + x + w - a.right).max(0.0);
            let fits = (height - y - self.margin.bottom).max(8.0);
            if y + h > height - self.margin.bottom + 0.5 && a.right > a.left {
                aspect = Some(fits / (a.right - a.left));
                margin.top -= (h - fits) / 2.0;
            }
        }
        let xlim = match (a.xdomain, root_pair(svg, "data-xdomain")) {
            (Some((t0, t1)), Some((d0, d1))) if t1 > t0 => {
                let (el, eu) = ((t0 - d0) / (t1 - t0), (d1 - t1) / (t1 - t0));
                let r = (t1 - t0) / (1.0 + el + eu);
                (r.is_finite() && r > 0.0).then_some((t0 + r * el, t1 - r * eu))
            }
            _ => None,
        };
        Fit {
            margin,
            xlim: xlim.or(self.xlim),
            aspect,
        }
    }
}

impl Panel {
    /// Apply a [`Fit`] (plots only; the extension's own small SVGs keep their
    /// layout).
    pub(crate) fn fitted(self, fit: Fit) -> Panel {
        match self {
            Panel::Plot {
                plot,
                width,
                height,
                attrs,
                ..
            } => {
                let mut plot = plot.theme_update(ThemeUpdate {
                    plot_margin: Some(fit.margin),
                    ..Default::default()
                });
                if let Some(xlim) = fit.xlim {
                    plot = plot.coord_cartesian_zoom(Some(xlim), None);
                }
                if let Some(r) = fit.aspect {
                    plot = plot.aspect_ratio(r);
                }
                Panel::Plot {
                    plot: Box::new(plot),
                    width,
                    height,
                    attrs,
                }
            }
            p => p,
        }
    }
}

/// The root `<svg>` start tag of a rendered panel.
fn root_tag(svg: &str) -> &str {
    &svg[..svg.find('>').unwrap_or(svg.len())]
}

/// Two numbers from a root attribute (`data-xdomain="a b"`).
fn root_pair(svg: &str, attr: &str) -> Option<(f64, f64)> {
    let tag = root_tag(svg);
    let at = tag.find(&format!(" {attr}=\""))? + attr.len() + 3;
    let v: Vec<f64> = tag[at..]
        .split('"')
        .next()?
        .split_whitespace()
        .map(|t| t.parse().ok())
        .collect::<Option<_>>()?;
    match v.as_slice() {
        [a, b, ..] if a.is_finite() && b.is_finite() => Some((*a, *b)),
        _ => None,
    }
}

/// A rendered panel's plot area `[x, y, width, height]` (its root
/// `data-plot`).
fn plot_rect(svg: &str) -> Option<[f64; 4]> {
    let tag = root_tag(svg);
    let at = tag.find(" data-plot=\"")? + 12;
    let v: Vec<f64> = tag[at..]
        .split('"')
        .next()?
        .split_whitespace()
        .filter_map(|t| t.parse().ok())
        .collect();
    v.try_into().ok()
}

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

/// Render a track panel (see the module docs).
pub(crate) fn render(
    o: &RenderOptions,
    cols: &[Column],
    width: u32,
    height: u32,
) -> Result<Panel, String> {
    let x = find_role(cols, Role::X).ok_or(TRACK_X)?;
    if x.values.iter().any(|v| matches!(v, Value::Str(_))) {
        return Err(TRACK_X.into());
    }
    let name = role_str(cols, Role::Track).unwrap_or_default();
    match cols.iter().find(|c| matches!(c.role, Role::Value(_))) {
        None => lanes(o, cols, x, None, &name, width, height),
        Some(v) if v.role == Role::Value(Kind::Heatmap) => {
            lanes(o, cols, x, Some(v), &name, width, height)
        }
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
/// chart's step when stacked); a tick a sliver of it.
fn lanes(
    o: &RenderOptions,
    cols: &[Column],
    x: &Column,
    value: Option<&Column>,
    name: &str,
    width: u32,
    height: u32,
) -> Result<Panel, String> {
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

    let align = o.align.as_ref();
    let xs: Vec<f64> = x.values.iter().filter_map(num).collect();
    let (lo, hi) = align.and_then(|a| a.xdomain).unwrap_or_else(|| {
        let lo = xs.iter().copied().fold(f64::INFINITY, f64::min);
        let hi = xs.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        if lo.is_finite() {
            (lo, hi)
        } else {
            (0.0, 1.0)
        }
    });
    let step = align
        .and_then(|a| a.step)
        .or_else(|| crate::track::x_step(&x.values))
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
    Ok(track_panel(plot, width, height))
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
) -> Result<Panel, String> {
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
            Ok(track_panel(plot, width, height))
        }
        p => Ok(p),
    }
}

/// A track panel, marked for hosts (`data-track`).
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
    stack(chart, tracks, width, height, o, &|cols, w, h, o, _| {
        crate::render_with(cols, w, h, o)
    })
    .map(|(svgs, _)| svgs)
}

/// The track heights of a stack (0 → natural).
pub fn heights(tracks: &[(Vec<Column>, u32)]) -> Vec<u32> {
    tracks
        .iter()
        .map(|(cols, h)| if *h == 0 { natural_height(cols) } else { *h })
        .collect()
}

/// A drawing function: columns, width, height, options, y offset in the stack.
pub(crate) type Draw<'a> =
    &'a dyn Fn(&[Column], u32, u32, &RenderOptions, f64) -> Result<String, RenderError>;

/// [`render_stack`] with a custom `draw` (the headless dashboard places the
/// panels as fragments); also returns the track heights.
pub(crate) fn stack(
    chart: &[Column],
    tracks: &[(Vec<Column>, u32)],
    width: u32,
    height: u32,
    o: &RenderOptions,
    draw: Draw,
) -> Result<(Vec<String>, Vec<u32>), RenderError> {
    guard(|| {
        let hs = heights(tracks);
        let main = draw(chart, width, height, o, 0.0)?;
        let (Some([px, _, pw, _]), Some(xdomain)) =
            (plot_rect(&main), root_pair(&main, "data-xdomain"))
        else {
            let note = "a ::TRACK needs a chart with a numeric or date x axis above it";
            let notes = hs.iter().map(|_| crate::error_svg(note, width));
            return Ok((std::iter::once(main).chain(notes).collect(), hs));
        };
        let step = find_role(chart, Role::X).and_then(|x| x_step(&x.values));
        let right = px + pw;
        let tracks_at = |left: f64| -> Result<Vec<String>, RenderError> {
            let to = RenderOptions {
                zoom: None,
                align: Some(Align {
                    left,
                    right,
                    xdomain: Some(xdomain),
                    step,
                }),
                ..o.clone()
            };
            let mut y = height as f64;
            let mut out = Vec::new();
            for ((cols, _), h) in tracks.iter().zip(&hs) {
                out.push(draw(cols, width, *h, &to, y)?);
                y += *h as f64;
            }
            Ok(out)
        };
        let mut out = tracks_at(px)?;
        // A track whose lane names need more room than the chart's y labels
        // moves everyone's left edge to its own.
        let left = out
            .iter()
            .filter_map(|s| plot_rect(s))
            .map(|r| r[0])
            .fold(px, f64::max);
        let main = if left > px + 0.25 {
            out = tracks_at(left)?;
            let mo = RenderOptions {
                align: Some(Align {
                    left,
                    right,
                    xdomain: None,
                    step: None,
                }),
                ..o.clone()
            };
            draw(chart, width, height, &mo, 0.0)?
        } else {
            main
        };
        Ok((std::iter::once(main).chain(out).collect(), hs))
    })
}
