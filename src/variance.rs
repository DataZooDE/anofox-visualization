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

/// The value-axis formatter (`::YFORMAT`) as a continuous scale.
fn value_scale(cols: &[Column], o: &RenderOptions) -> ScaleContinuous {
    let mut s = ScaleContinuous::new();
    if let Some(f) = role_fmt(cols, Role::YFormat).and_then(|s| axis_formatter(&s, &o.locale)) {
        s = s.with_label_formatter(f);
    }
    s
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
            .layer_aes(Aes::new().x("x").y("hi").ymin("lo").label("tip").group("x"));
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
        .scale_y_continuous(value_scale(cols, o))
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
            .layer_aes(Aes::new().x("x").y("hi").ymin("lo").label("tip").group("x"));
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
            .layer_aes(Aes::new().x("x").y("hi").ymin("lo").label("tip").group("x"));
    }
    // Flipped, the first discrete level sits at the bottom: reverse so the
    // first row reads on top.
    let mut order = levels(&labels[..n]);
    order.reverse();
    plot = plot
        .scale_x_discrete(
            ScaleDiscrete::new().with_limits(order.iter().map(String::as_str).collect()),
        )
        .scale_y_continuous(value_scale(cols, o))
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
        .layer_aes(Aes::new().x("x").y("hi").ymin("lo").label("tip").group("x"))
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
        .scale_y_continuous(
            ScaleContinuous::new()
                .with_limits(-max * 0.5, max * 0.95)
                .with_expand(0.0, 0.0),
        )
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
