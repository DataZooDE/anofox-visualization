//! The variance views: `::WATERFALL` (a bridge from a start total to an end
//! total through signed steps), `::BULLET` (a measure against a `::GOAL`
//! marker over qualitative `::RANGE` bands) and `::FUNNEL` (ordered stages
//! with the conversion between them).
//!
//! All three are drawn from plain ggplot-rs columns, segments and text, so
//! headless/static output matches the browser. Every measured mark is a
//! column carrying the usual hover metadata — `data-x` (the step / category /
//! stage), `data-series` (the same — the selection key a click sets) and
//! `data-value` (its own value, via the
//! pre-position `y` ggplot-rs reports) — so tooltips, cross-filtering and
//! `::SELECT_AS` work as on a bar chart.

use crate::{
    axis_formatter, card_col, chart_theme, format, note_svg, role_fmt, value_fmt, value_str,
    Column, Kind, Panel, RenderOptions, Role,
};
use ggplot_rs::prelude::*;
use ggplot_rs::scale::continuous::ScaleContinuous;
use ggplot_rs::scale::discrete::ScaleDiscrete;

/// The layer column ggplot-rs reads a mark's measured value from (its
/// `data-value` and tooltip): the pre-position `y`. Setting it ourselves lets
/// a floating bar (a waterfall step, a centred funnel stage) report its own
/// value instead of its top edge.
const RAW_Y: &str = ggplot_rs::position::RAW_Y_COL;

/// A cool grey of lightness `l` (0..1) as drawn on a light card. Written as a
/// light-look neutral on purpose: the theme's SVG pass maps such greys onto a
/// dark card by inverting their lightness, so a pale band stays a quiet wash in
/// both looks.
fn neutral(l: f64) -> (u8, u8, u8) {
    let v = (l.clamp(0.0, 1.0) * 255.0).round() as u8;
    (v.saturating_sub(5), v.saturating_sub(3), v)
}

/// The category labels of the `::XAXIS` column (numbers and dates as text).
fn x_labels(cols: &[Column]) -> Option<Vec<String>> {
    cols.iter()
        .find(|c| c.role == Role::X)
        .map(|c| c.values.iter().map(value_str).collect())
}

/// The value-axis formatter (`::YFORMAT`) as a continuous scale, plus
/// (B2-4) the same spec's `data-fvalue` formatter — see `cartesian`'s
/// matching comment — so ggplot-rs's native `svg_formatted_values` writes
/// exactly what the old string-surgery `with_formatted_values` used to.
fn value_scale(
    cols: &[Column],
    kind: Kind,
    values: &[Value],
    o: &RenderOptions,
) -> ScaleContinuous {
    let mut s = ScaleContinuous::new();
    if let Some(f) = role_fmt(cols, Role::YFormat).and_then(|s| axis_formatter(&s, &o.locale)) {
        s = s.with_label_formatter(f);
    }
    let vfmt = value_fmt(cols, kind, values);
    let locale = o.locale;
    s.with_value_formatter(move |n: f64| vfmt.full(n, &locale))
}

/// Distinct labels in first-seen order (a discrete axis keeps the SQL order).
fn levels(labels: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for l in labels {
        if !out.contains(l) {
            out.push(l.clone());
        }
    }
    out
}

fn strs(v: impl IntoIterator<Item = String>) -> Vec<Value> {
    v.into_iter().map(Value::Str).collect()
}

fn nums(v: impl IntoIterator<Item = f64>) -> Vec<Value> {
    v.into_iter().map(Value::Float).collect()
}

/// One bar of a waterfall: its label, kind, extent and own value.
struct Step {
    label: String,
    total: bool,
    lo: f64,
    hi: f64,
    /// The step's signed change, or a total's level.
    value: f64,
}

/// Which steps a bar layer draws, and its fill.
type StepClass = (fn(&Step) -> bool, (u8, u8, u8));

/// A row is a total when its value is NULL (the running total so far) or its
/// `::CATEGORY` says `total` / `subtotal` (then its value is the level).
fn is_total_tag(v: Option<&Value>) -> bool {
    matches!(v, Some(Value::Str(s)) if {
        let s = s.trim().to_ascii_lowercase();
        s == "total" || s == "subtotal" || s == "start" || s == "end"
    })
}

/// The bars of a waterfall: the first row is the start total, each further
/// row a signed step (or a total, see [`is_total_tag`]), and the bridge closes
/// with an `End` total unless the last row already is one.
fn waterfall_steps(labels: &[String], values: &[Value], tags: Option<&Column>) -> Vec<Step> {
    let mut steps = Vec::new();
    let mut run = 0.0;
    for (i, label) in labels.iter().enumerate() {
        let v = values
            .get(i)
            .and_then(|v| v.as_f64())
            .filter(|f| f.is_finite());
        let tagged = is_total_tag(tags.and_then(|c| c.values.get(i)));
        if i == 0 || tagged || v.is_none() {
            run = v.unwrap_or(run);
            steps.push(Step {
                label: label.clone(),
                total: true,
                lo: 0.0,
                hi: run,
                value: run,
            });
        } else if let Some(d) = v {
            steps.push(Step {
                label: label.clone(),
                total: false,
                lo: run,
                hi: run + d,
                value: d,
            });
            run += d;
        }
    }
    if steps.last().is_some_and(|s| !s.total) {
        steps.push(Step {
            label: "End".into(),
            total: true,
            lo: 0.0,
            hi: run,
            value: run,
        });
    }
    steps
}

/// `::WATERFALL`: a bridge from a start total to an end total. Totals are
/// full bars in the brand colour, increases / decreases float from the
/// running total in the theme's positive / negative colours, thin connectors
/// carry the level from bar to bar and every bar is labelled with its value
/// (steps signed).
pub(crate) fn render_waterfall(
    o: &RenderOptions,
    value: &Column,
    cols: &[Column],
    title: Option<&str>,
    width: u32,
    height: u32,
) -> Result<Panel, String> {
    let Some(labels) = x_labels(cols) else {
        return Ok(note_svg(
            title,
            "::WATERFALL needs an ::XAXIS step label",
            width,
            height,
        ));
    };
    let tags = cols.iter().find(|c| c.role == Role::Category);
    let steps = waterfall_steps(&labels, &value.values, tags);
    let fmt = value_fmt(cols, Kind::Waterfall, &value.values);
    let short = |s: &Step| {
        let t = fmt.short(s.value, &o.locale);
        if !s.total && s.value > 0.0 {
            format!("+{t}")
        } else {
            t
        }
    };
    let names: Vec<String> = steps.iter().map(|s| s.label.clone()).collect();
    let data = vec![
        ("x".to_string(), strs(names.clone())),
        ("lo".to_string(), nums(steps.iter().map(|s| s.lo))),
        ("hi".to_string(), nums(steps.iter().map(|s| s.hi))),
        (RAW_Y.to_string(), nums(steps.iter().map(|s| s.value))),
        ("tip".to_string(), strs(names.clone())),
        // Labels sit on the bar's upper edge.
        (
            "top".to_string(),
            nums(steps.iter().map(|s| s.lo.max(s.hi))),
        ),
        ("dlab".to_string(), strs(steps.iter().map(short))),
    ];
    // Connectors: the running level from one bar's centre to the next one's
    // (drawn under the bars, so only the gap between them shows).
    let conn: Vec<(String, String, f64)> = steps
        .windows(2)
        .map(|w| (w[0].label.clone(), w[1].label.clone(), w[0].hi))
        .collect();
    let conn_data = vec![
        ("x".to_string(), strs(conn.iter().map(|c| c.0.clone()))),
        ("xend".to_string(), strs(conn.iter().map(|c| c.1.clone()))),
        ("y".to_string(), nums(conn.iter().map(|c| c.2))),
    ];
    let th = &o.theme;
    let mut plot = GGPlot::new(data.clone());
    if !conn.is_empty() {
        plot = plot
            .geom_segment_with(GeomSegment {
                color: neutral(0.62),
                width: 0.8,
                alpha: 1.0,
                ..Default::default()
            })
            .layer_data(conn_data)
            .layer_aes(Aes::new().x("x").xend("xend").y("y").yend("y"));
    }
    // One layer per colour — totals, increases, decreases — rather than a
    // mapped fill, so each bar's `data-series` stays its step (the selection
    // key a click sets) instead of the colour class.
    let classes: [StepClass; 3] = [
        (|s| s.total, o.brand()),
        (|s| !s.total && s.value >= 0.0, th.colors.positive),
        (|s| !s.total && s.value < 0.0, th.colors.negative),
    ];
    for (is, fill) in classes {
        let idx: Vec<usize> = (0..steps.len()).filter(|&i| is(&steps[i])).collect();
        if idx.is_empty() {
            continue;
        }
        let pick = |c: &str| -> Vec<Value> {
            let col = &data.iter().find(|(n, _)| n == c).unwrap().1;
            idx.iter().map(|&i| col[i].clone()).collect()
        };
        plot = plot
            .geom_col_with(GeomCol {
                width: 0.7,
                fill,
                ..card_col(o)
            })
            .layer_data(
                ["x", "lo", "hi", RAW_Y, "tip"]
                    .map(|c| (c.to_string(), pick(c)))
                    .to_vec(),
            )
            .layer_aes(Aes::new().x("x").y("hi").ymin("lo").label("tip").group("x"))
            // Each step is a floating bar (`lo`..`hi`, not 0..`hi`) — one row
            // per x per layer, so ggplot-rs 0.24's new `geom_col()` default
            // (`position_stack()`, #169) is a no-op height-wise where it
            // only ever mattered for >1 group sharing an x, but it also
            // *recomputes* `ymin` from `y` alone (baseline 0), discarding the
            // explicit floating `lo` we mapped. Keep identity so the bar
            // still spans `lo`..`hi`.
            .position(PositionIdentity);
    }
    plot = plot
        .geom_text_with(GeomText {
            size: 11.0,
            color: th.chart_label(),
            vjust: -0.35,
            ..Default::default()
        })
        .layer_aes(Aes::new().x("x").y("top").label("dlab"))
        .scale_x_discrete(
            ScaleDiscrete::new().with_limits(levels(&names).iter().map(String::as_str).collect()),
        )
        .scale_y_continuous(value_scale(cols, Kind::Waterfall, &value.values, o))
        .xlab("")
        .ylab("")
        .theme(chart_theme(o, 11.0))
        .legend_position(LegendPosition::None);
    if let Some(t) = title {
        plot = plot.title(t);
    }
    Ok(Panel::plot(plot, width, height))
}

/// The band edges of a `::RANGE` value for a bullet: comma-separated numbers
/// (`'60,80,100'`), ascending.
fn band_edges(v: Option<&Value>) -> Vec<f64> {
    let mut e: Vec<f64> = match v {
        Some(Value::Str(s)) => s
            .split(',')
            .filter_map(|p| p.trim().parse::<f64>().ok())
            .filter(|f| f.is_finite())
            .collect(),
        Some(v) => v.as_f64().filter(|f| f.is_finite()).into_iter().collect(),
        None => Vec::new(),
    };
    e.sort_by(|a, b| a.total_cmp(b));
    e.dedup();
    e
}

/// `::BULLET`: one horizontal row per `::XAXIS` category — qualitative
/// `::RANGE` bands (`'60,80,100'`, per row or one for all) as graded washes,
/// the measure as a narrow bar in the brand colour and the `::GOAL` as a
/// strong tick across it. First row on top.
pub(crate) fn render_bullet(
    o: &RenderOptions,
    value: &Column,
    cols: &[Column],
    title: Option<&str>,
    width: u32,
    height: u32,
) -> Result<Panel, String> {
    let Some(labels) = x_labels(cols) else {
        return Ok(note_svg(
            title,
            "::BULLET needs an ::XAXIS category",
            width,
            height,
        ));
    };
    let goal = cols.iter().find(|c| c.role == Role::RefLine);
    let range = cols.iter().find(|c| c.role == Role::Range);
    let n = labels.len().min(value.values.len());
    let num = |c: Option<&Column>, i: usize| {
        c.and_then(|c| c.values.get(i))
            .and_then(|v| v.as_f64())
            .filter(|f| f.is_finite())
    };
    // Each row's band edges: its own ::RANGE, else the first one given.
    let first_edges = band_edges(range.and_then(|c| c.values.iter().find(|v| !v.is_na())));
    let edges: Vec<Vec<f64>> = (0..n)
        .map(|i| {
            let e = band_edges(range.and_then(|c| c.values.get(i)));
            if e.is_empty() {
                first_edges.clone()
            } else {
                e
            }
        })
        .collect();
    let span = (0..n)
        .flat_map(|i| {
            [num(Some(value), i), num(goal, i)]
                .into_iter()
                .flatten()
                .chain(edges[i].iter().copied())
        })
        .fold(0.0_f64, |m, f| m.max(f.abs()));
    let span = if span > 0.0 { span } else { 1.0 };
    let th = &o.theme;
    let fmt = value_fmt(cols, Kind::Bullet, &value.values);
    let mut plot = GGPlot::new(vec![
        ("x".to_string(), strs(labels[..n].iter().cloned())),
        ("y".to_string(), value.values[..n].to_vec()),
    ]);
    // Bands: the k-th band of every row in one layer, darkest (lowest) first.
    let n_bands = edges.iter().map(Vec::len).max().unwrap_or(0);
    for k in 0..n_bands {
        let rows: Vec<usize> = (0..n).filter(|&i| edges[i].len() > k).collect();
        let lo = rows
            .iter()
            .map(|&i| if k == 0 { 0.0 } else { edges[i][k - 1] });
        let l = 0.78 + 0.14 * (k as f64 / n_bands.max(1) as f64);
        plot = plot
            .geom_col_with(GeomCol {
                fill: neutral(l),
                color: th.colors.card,
                width: 0.8,
                ..Default::default()
            })
            .layer_data(vec![
                (
                    "x".to_string(),
                    strs(rows.iter().map(|&i| labels[i].clone())),
                ),
                ("lo".to_string(), nums(lo)),
                ("hi".to_string(), nums(rows.iter().map(|&i| edges[i][k]))),
                // No `data-value` (a band edge is not a measure); the
                // tooltip names the band instead.
                (RAW_Y.to_string(), rows.iter().map(|_| Value::Na).collect()),
                (
                    "tip".to_string(),
                    strs(rows.iter().map(|&i| {
                        let lo = if k == 0 { 0.0 } else { edges[i][k - 1] };
                        format!(
                            "Range {} – {}",
                            fmt.full(lo, &o.locale),
                            fmt.full(edges[i][k], &o.locale)
                        )
                    })),
                ),
            ])
            // Each band segment is a floating bar (its own `lo`..`hi`, one
            // per x per layer) — see the waterfall loop above for why 0.24's
            // stacking default (which recomputes `ymin` from `y`/baseline 0)
            // needs an explicit identity to keep it.
            .layer_aes(Aes::new().x("x").y("hi").ymin("lo").label("tip").group("x"))
            .position(PositionIdentity);
    }
    plot = plot
        .geom_col_with(GeomCol {
            width: 0.32,
            ..card_col(o)
        })
        .layer_data(vec![
            ("x".to_string(), strs(labels[..n].iter().cloned())),
            ("y".to_string(), value.values[..n].to_vec()),
            ("tip".to_string(), strs(labels[..n].iter().cloned())),
        ])
        .layer_aes(Aes::new().x("x").y("y").label("tip").group("x"));
    // The goal: a thin dark bar across the measure.
    let goals: Vec<(usize, f64)> = (0..n)
        .filter_map(|i| num(goal, i).map(|g| (i, g)))
        .collect();
    if !goals.is_empty() {
        let eps = span * 0.006;
        plot = plot
            .geom_col_with(GeomCol {
                fill: th.colors.heading,
                color: th.colors.heading,
                width: 0.62,
                ..Default::default()
            })
            .layer_data(vec![
                (
                    "x".to_string(),
                    strs(goals.iter().map(|&(i, _)| labels[i].clone())),
                ),
                ("lo".to_string(), nums(goals.iter().map(|&(_, g)| g - eps))),
                ("hi".to_string(), nums(goals.iter().map(|&(_, g)| g + eps))),
                // The tooltip says "Goal 80%" (no `data-value`, so hosts show
                // it as written); `data-series` stays the row for selection.
                (RAW_Y.to_string(), goals.iter().map(|_| Value::Na).collect()),
                (
                    "tip".to_string(),
                    strs(
                        goals
                            .iter()
                            .map(|&(_, g)| format!("Goal {}", fmt.full(g, &o.locale))),
                    ),
                ),
            ])
            // A floating band across the measure (`g - eps`..`g + eps`), same
            // reasoning as the waterfall/band layers above.
            .layer_aes(Aes::new().x("x").y("hi").ymin("lo").label("tip").group("x"))
            .position(PositionIdentity);
    }
    // Flipped, the first discrete level sits at the bottom: reverse so the
    // first row reads on top.
    let mut order = levels(&labels[..n]);
    order.reverse();
    plot = plot
        .scale_x_discrete(
            ScaleDiscrete::new().with_limits(order.iter().map(String::as_str).collect()),
        )
        .scale_y_continuous(value_scale(cols, Kind::Bullet, &value.values, o))
        .coord_flip()
        .xlab("")
        .ylab("")
        .theme(chart_theme(o, 11.0))
        .legend_position(LegendPosition::None);
    if let Some(t) = title {
        plot = plot.title(t);
    }
    Ok(Panel::plot(plot, width, height))
}

/// `::FUNNEL`: ordered `::XAXIS` stages top to bottom, each a bar centred on
/// one axis with its width by value, labelled with the value and the
/// conversion from the previous stage.
pub(crate) fn render_funnel(
    o: &RenderOptions,
    value: &Column,
    cols: &[Column],
    title: Option<&str>,
    width: u32,
    height: u32,
) -> Result<Panel, String> {
    let Some(labels) = x_labels(cols) else {
        return Ok(note_svg(
            title,
            "::FUNNEL needs an ::XAXIS stage",
            width,
            height,
        ));
    };
    let rows: Vec<(String, f64)> = labels
        .iter()
        .zip(&value.values)
        .filter_map(|(l, v)| {
            v.as_f64()
                .filter(|f| f.is_finite() && *f >= 0.0)
                .map(|f| (l.clone(), f))
        })
        .collect();
    if rows.is_empty() {
        return Ok(note_svg(
            title,
            "::FUNNEL needs non-negative values",
            width,
            height,
        ));
    }
    let max = rows.iter().map(|r| r.1).fold(0.0_f64, f64::max);
    let max = if max > 0.0 { max } else { 1.0 };
    let fmt = value_fmt(cols, Kind::Funnel, &value.values);
    let pct = format::ValueFmt::Share;
    let label = |i: usize| {
        let v = fmt.full(rows[i].1, &o.locale);
        match i.checked_sub(1).map(|p| rows[p].1) {
            Some(prev) if prev > 0.0 => format!("{v} · {}", pct.short(rows[i].1 / prev, &o.locale)),
            _ => v,
        }
    };
    let names: Vec<String> = rows.iter().map(|r| r.0.clone()).collect();
    let data = vec![
        ("x".to_string(), strs(names.clone())),
        ("lo".to_string(), nums(rows.iter().map(|r| -r.1 / 2.0))),
        ("hi".to_string(), nums(rows.iter().map(|r| r.1 / 2.0))),
        (RAW_Y.to_string(), nums(rows.iter().map(|r| r.1))),
        ("tip".to_string(), strs(names.clone())),
        // The label sits just right of the bar.
        (
            "lx".to_string(),
            nums(rows.iter().map(|r| r.1 / 2.0 + max * 0.02)),
        ),
        ("dlab".to_string(), strs((0..rows.len()).map(label))),
    ];
    let mut order = levels(&names);
    order.reverse();
    let mut th = chart_theme(o, 11.0);
    // The value axis means nothing on a centred funnel: no ticks, no grid.
    th.axis_text_x = ggplot_rs::theme::ElementText::blank();
    th.panel_grid_major = ggplot_rs::theme::ElementLine::blank();
    let mut plot = GGPlot::new(data)
        .geom_col_with(GeomCol {
            width: 0.82,
            ..card_col(o)
        })
        // A centred floating bar (`-value/2`..`value/2`), same reasoning as
        // the waterfall/bullet layers above (variance.rs module docs).
        .layer_aes(Aes::new().x("x").y("hi").ymin("lo").label("tip").group("x"))
        .position(PositionIdentity)
        .geom_text_with(GeomText {
            size: 11.0,
            color: o.theme.chart_label(),
            hjust: 0.0,
            ..Default::default()
        })
        .layer_aes(Aes::new().x("x").y("lx").label("dlab"))
        .scale_x_discrete(
            ScaleDiscrete::new().with_limits(order.iter().map(String::as_str).collect()),
        )
        // Room on the right for the labels; centred bars stay centred.
        // B2-4: the stage's own `data-fvalue` (`::YFORMAT`) — the same
        // `fmt` the `dlab` text layer already uses above — see
        // `cartesian`'s matching comment.
        .scale_y_continuous({
            let locale = o.locale;
            ScaleContinuous::new()
                .with_limits(-max * 0.5, max * 0.95)
                .with_expand(0.0, 0.0)
                .with_value_formatter(move |n: f64| fmt.full(n, &locale))
        })
        .coord_flip()
        .xlab("")
        .ylab("")
        .theme(th)
        .legend_position(LegendPosition::None);
    if let Some(t) = title {
        plot = plot.title(t);
    }
    Ok(Panel::plot(plot, width, height))
}

/// `::SANKEY` (#881): `::ORIGIN` → `::DEST` flows, link width by the
/// measure; nodes stack in columns, coloured by name in the theme's series
/// colours (ggplot-rs `geom_sankey`). Rows without a positive value are left
/// out.
pub(crate) fn render_sankey(
    o: &RenderOptions,
    value: &Column,
    cols: &[Column],
    title: Option<&str>,
    width: u32,
    height: u32,
) -> Result<Panel, String> {
    let text = |r: Role| -> Option<Vec<String>> {
        cols.iter()
            .find(|c| c.role == r)
            .map(|c| c.values.iter().map(value_str).collect())
    };
    let (Some(from), Some(to)) = (text(Role::Origin), text(Role::Dest)) else {
        return Ok(note_svg(
            title,
            "::SANKEY needs an ::ORIGIN and a ::DEST column",
            width,
            height,
        ));
    };
    let keep: Vec<usize> = (0..from.len().min(to.len()))
        .filter(|&i| {
            value
                .values
                .get(i)
                .and_then(|v| v.as_f64())
                .is_some_and(|f| f.is_finite() && f > 0.0)
                && from[i] != to[i]
        })
        .collect();
    if keep.is_empty() {
        return Ok(note_svg(
            title,
            "::SANKEY needs positive flows",
            width,
            height,
        ));
    }
    let mut fills: Vec<(String, ggplot_rs::scale::color::RGBAColor)> = Vec::new();
    for &i in &keep {
        for n in [&from[i], &to[i]] {
            if !fills.iter().any(|(k, _)| k == n) {
                fills.push((n.clone(), crate::dz_color(o, fills.len())));
            }
        }
    }
    let data = vec![
        (
            "from".to_string(),
            strs(keep.iter().map(|&i| from[i].clone())),
        ),
        ("to".to_string(), strs(keep.iter().map(|&i| to[i].clone()))),
        (
            "w".to_string(),
            nums(
                keep.iter()
                    .map(|&i| value.values[i].as_f64().unwrap_or(0.0)),
            ),
        ),
    ];
    let mut plot = GGPlot::new(data)
        .aes(Aes::new().from("from").to("to").weight("w"))
        .geom_sankey()
        .scale_fill_manual(fills.iter().map(|(k, c)| (k.as_str(), *c)).collect())
        .theme(chart_theme(o, 11.0))
        .legend_position(LegendPosition::None);
    if let Some(t) = title {
        plot = plot.title(t);
    }
    Ok(Panel::plot(plot, width, height))
}

/// `::TREEMAP` (#880): tiles whose area is the measure, labelled by
/// `::XAXIS` (or a `::LABEL`, with `::XAXIS` kept as the tile's key, #1002);
/// with a `::CATEGORY` the tiles nest in group rectangles, filled by group
/// (ggplot-rs `geom_treemap`). Rows with no positive value are left out.
pub(crate) fn render_treemap(
    o: &RenderOptions,
    value: &Column,
    cols: &[Column],
    title: Option<&str>,
    width: u32,
    height: u32,
) -> Result<Panel, String> {
    let Some(labels) = x_labels(cols) else {
        return Ok(note_svg(
            title,
            "::TREEMAP needs an ::XAXIS naming each tile",
            width,
            height,
        ));
    };
    let groups: Option<Vec<String>> = cols
        .iter()
        .find(|c| c.role == Role::Category)
        .map(|c| c.values.iter().map(value_str).collect());
    let keep: Vec<usize> = (0..labels.len())
        .filter(|&i| {
            value
                .values
                .get(i)
                .and_then(|v| v.as_f64())
                .is_some_and(|f| f.is_finite() && f > 0.0)
        })
        .collect();
    if keep.is_empty() {
        return Ok(note_svg(
            title,
            "::TREEMAP needs positive values",
            width,
            height,
        ));
    }
    let mut data = vec![
        (
            "x".to_string(),
            strs(keep.iter().map(|&i| labels[i].clone())),
        ),
        (
            "w".to_string(),
            nums(
                keep.iter()
                    .map(|&i| value.values[i].as_f64().unwrap_or(0.0)),
            ),
        ),
    ];
    // A ::LABEL names the tile while ::XAXIS stays its key (#1002): the
    // tile's data-x, which a click and ::DRILL read (a code shown as a name).
    let shown: Option<Vec<String>> = cols
        .iter()
        .find(|c| c.role == Role::Label)
        .map(|c| c.values.iter().map(value_str).collect());
    let mut aes = Aes::new().weight("w").label("x");
    if let Some(shown) = &shown {
        data.push((
            "k".to_string(),
            strs(keep.iter().map(|&i| labels[i].clone())),
        ));
        data[0].1 = strs(keep.iter().map(|&i| {
            shown
                .get(i)
                .filter(|s| !s.is_empty())
                .cloned()
                .unwrap_or_else(|| labels[i].clone())
        }));
        aes = aes.key("k");
    }
    let mut fills: Vec<(String, ggplot_rs::scale::color::RGBAColor)> = Vec::new();
    if let Some(g) = &groups {
        let col: Vec<String> = keep.iter().map(|&i| g[i].clone()).collect();
        // the theme's series colours, by group in first-seen order
        for v in &col {
            if !fills.iter().any(|(k, _)| k == v) {
                fills.push((v.clone(), crate::dz_color(o, fills.len())));
            }
        }
        data.push(("g".to_string(), strs(col)));
        aes = aes.subgroup("g").fill("g");
    }
    let c0 = o.theme.color(0);
    let mut plot = GGPlot::new(data)
        .aes(aes)
        .geom_treemap_with(ggplot_rs::geom::treemap::GeomTreemap {
            fill: c0,
            label_size: 11.0,
            group_label_size: if groups.is_some() { 11.0 } else { 0.0 },
            ..Default::default()
        })
        .theme(chart_theme(o, 11.0))
        .legend_position(LegendPosition::None);
    if !fills.is_empty() {
        plot = plot.scale_fill_manual(fills.iter().map(|(k, c)| (k.as_str(), *c)).collect());
    }
    if let Some(t) = title {
        plot = plot.title(t);
    }
    Ok(Panel::plot(plot, width, height))
}

/// `::PARETO` (#879): `::XAXIS` categories as bars sorted largest first (a
/// category's rows summed), with the cumulative share as a line read
/// against a 0–100 % right axis (ggplot-rs `geom_pareto`).
pub(crate) fn render_pareto(
    o: &RenderOptions,
    value: &Column,
    cols: &[Column],
    title: Option<&str>,
    width: u32,
    height: u32,
) -> Result<Panel, String> {
    let Some(labels) = x_labels(cols) else {
        return Ok(note_svg(
            title,
            "::PARETO needs an ::XAXIS category",
            width,
            height,
        ));
    };
    let keep: Vec<usize> = (0..labels.len())
        .filter(|&i| {
            value
                .values
                .get(i)
                .and_then(|v| v.as_f64())
                .is_some_and(|f| f.is_finite() && f >= 0.0)
        })
        .collect();
    if keep.is_empty() {
        return Ok(note_svg(
            title,
            "::PARETO needs non-negative values",
            width,
            height,
        ));
    }
    let data = vec![
        (
            "x".to_string(),
            strs(keep.iter().map(|&i| labels[i].clone())),
        ),
        (
            "y".to_string(),
            nums(
                keep.iter()
                    .map(|&i| value.values[i].as_f64().unwrap_or(0.0)),
            ),
        ),
    ];
    let mut plot = GGPlot::new(data)
        .aes(Aes::new().x("x").y("y"))
        .geom_pareto()
        .xlab("")
        .ylab("")
        .theme(chart_theme(o, 11.0))
        .legend_position(LegendPosition::None);
    if let Some(t) = title {
        plot = plot.title(t);
    }
    Ok(Panel::plot(plot, width, height))
}

/// `::GANTT` (#882): one bar per row, the `::GANTT` column naming the task,
/// from `::XMIN` to `::XMAX` (dates or numbers), in the theme's series
/// colours by `::CATEGORY` (ggplot-rs `geom_xrange`). Tasks stay in row order,
/// top to bottom.
pub(crate) fn render_gantt(
    o: &RenderOptions,
    value: &Column,
    cols: &[Column],
    title: Option<&str>,
    width: u32,
    height: u32,
) -> Result<Panel, String> {
    let col = |r: Role| cols.iter().find(|c| c.role == r);
    let (Some(lo), Some(hi)) = (col(Role::XMin), col(Role::XMax)) else {
        return Ok(note_svg(
            title,
            "::GANTT needs ::XMIN (start) and ::XMAX (end) columns",
            width,
            height,
        ));
    };
    let tasks: Vec<String> = value.values.iter().map(value_str).collect();
    // an ISO date / timestamp as text is a point in time (a continuous date
    // axis), not a category
    let when = |v: &Value| match v {
        Value::Str(t) => crate::freshness::parse_timestamp(t).map_or(v.clone(), Value::DateTime),
        _ => v.clone(),
    };
    let keep: Vec<usize> = (0..tasks.len())
        .filter(|&i| {
            !matches!(lo.values.get(i), None | Some(Value::Na))
                && !matches!(hi.values.get(i), None | Some(Value::Na))
        })
        .collect();
    if keep.is_empty() {
        return Ok(note_svg(
            title,
            "::GANTT has no task with a start and an end",
            width,
            height,
        ));
    }
    let mut data = vec![
        (
            "task".to_string(),
            strs(keep.iter().map(|&i| tasks[i].clone())),
        ),
        (
            "lo".to_string(),
            keep.iter().map(|&i| when(&lo.values[i])).collect(),
        ),
        (
            "hi".to_string(),
            keep.iter().map(|&i| when(&hi.values[i])).collect(),
        ),
    ];
    let mut aes = Aes::new().y("task").xmin("lo").xmax("hi");
    let mut fills: Vec<(String, ggplot_rs::scale::color::RGBAColor)> = Vec::new();
    if let Some(c) = col(Role::Category) {
        let g: Vec<String> = keep.iter().map(|&i| value_str(&c.values[i])).collect();
        for v in &g {
            if !fills.iter().any(|(k, _)| k == v) {
                fills.push((v.clone(), crate::dz_color(o, fills.len())));
            }
        }
        data.push(("g".to_string(), strs(g)));
        aes = aes.fill("g");
    }
    // first task on top
    let mut order: Vec<String> = Vec::new();
    for &i in &keep {
        if !order.contains(&tasks[i]) {
            order.push(tasks[i].clone());
        }
    }
    order.reverse();
    let mut plot = GGPlot::new(data)
        .aes(aes)
        .geom_xrange()
        .scale_y_discrete(
            ScaleDiscrete::new().with_limits(order.iter().map(String::as_str).collect()),
        )
        .xlab("")
        .ylab("")
        .theme(chart_theme(o, 11.0));
    if fills.is_empty() {
        plot = plot.legend_position(LegendPosition::None);
    } else {
        plot = plot.scale_fill_manual(fills.iter().map(|(k, c)| (k.as_str(), *c)).collect());
    }
    if let Some(t) = title {
        plot = plot.title(t);
    }
    Ok(Panel::plot(plot, width, height))
}

/// `::DUMBBELL` (#883): per item (the `::DUMBBELL` column) two values,
/// `::XMIN` and `::XMAX`, as two dots joined by a line, items top down in
/// row order (ggplot-rs `geom_dumbbell`, flipped). Before/after, plan/actual.
pub(crate) fn render_dumbbell(
    o: &RenderOptions,
    value: &Column,
    cols: &[Column],
    title: Option<&str>,
    width: u32,
    height: u32,
) -> Result<Panel, String> {
    let col = |r: Role| cols.iter().find(|c| c.role == r);
    let (Some(a), Some(b)) = (col(Role::XMin), col(Role::XMax)) else {
        return Ok(note_svg(
            title,
            "::DUMBBELL needs ::XMIN and ::XMAX values",
            width,
            height,
        ));
    };
    let items: Vec<String> = value.values.iter().map(value_str).collect();
    let keep: Vec<usize> = (0..items.len())
        .filter(|&i| {
            a.values.get(i).and_then(|v| v.as_f64()).is_some()
                && b.values.get(i).and_then(|v| v.as_f64()).is_some()
        })
        .collect();
    if keep.is_empty() {
        return Ok(note_svg(
            title,
            "::DUMBBELL has no item with both values",
            width,
            height,
        ));
    }
    let data = vec![
        (
            "item".to_string(),
            strs(keep.iter().map(|&i| items[i].clone())),
        ),
        (
            "lo".to_string(),
            nums(keep.iter().map(|&i| a.values[i].as_f64().unwrap_or(0.0))),
        ),
        (
            "hi".to_string(),
            nums(keep.iter().map(|&i| b.values[i].as_f64().unwrap_or(0.0))),
        ),
    ];
    let mut order: Vec<String> = Vec::new();
    for &i in &keep {
        if !order.contains(&items[i]) {
            order.push(items[i].clone());
        }
    }
    order.reverse();
    let mut plot = GGPlot::new(data)
        .aes(Aes::new().x("item").ymin("lo").ymax("hi"))
        .geom_dumbbell()
        .scale_x_discrete(
            ScaleDiscrete::new().with_limits(order.iter().map(String::as_str).collect()),
        )
        .coord_flip()
        .xlab("")
        .ylab("")
        .theme(chart_theme(o, 11.0))
        .legend_position(LegendPosition::None);
    if let Some(t) = title {
        plot = plot.title(t);
    }
    Ok(Panel::plot(plot, width, height))
}
