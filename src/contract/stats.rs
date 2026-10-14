//! Contract plots drawn directly with ggplot-rs — the shapes the role
//! renderer cannot express: post-hoc test forests and significance brackets
//! (`test`), small-multiple model comparison with one scale per metric
//! (`summary`), multi-model fits (`prediction`), Kaplan–Meier and CV curves
//! (`curve`) and partial-residual plots.
//!
//! Every plot here keeps the house style of [`crate::render_with`]: minimal
//! theme, legend on top, the DataZoo palette, the brand colour for single
//! series.

use super::{opt_str, Row, Table};
use crate::{Place, RenderOptions, DZ_COLORS};
use ggplot_rs::prelude::*;
use serde_json::Value as J;
use std::collections::{BTreeMap, HashMap};

type Frame = Vec<(String, Vec<Value>)>;
/// A significance bracket: x of the left / right group, p, test_id.
type Bracket = (f64, f64, Option<f64>, Option<String>);
/// A λ marker: x, label, colour, label height.
type Mark = (f64, &'static str, (u8, u8, u8), f64);
pub(super) type Out = Result<(String, Vec<String>), String>;

/// "Not significant" / secondary marks.
const GREY: (u8, u8, u8) = (0x7a, 0x8b, 0x99);
/// Significant / best / highlighted marks (DataZoo orange).
const ACCENT: (u8, u8, u8) = DZ_COLORS[1];
/// Annotation text.
const INK: (u8, u8, u8) = (55, 62, 75);
/// Reference lines.
const RULE: (u8, u8, u8) = (130, 136, 148);

// ── shared kit ──────────────────────────────────────────────────────────────

fn fin(v: &Value) -> Option<f64> {
    v.as_f64().filter(|f| f.is_finite())
}

fn s(v: impl Into<String>) -> Value {
    Value::Str(v.into())
}

fn text_of(v: &Value) -> Option<String> {
    match v {
        Value::Na => None,
        Value::Str(s) => Some(s.clone()),
        v => v.as_f64().map(fmt_num),
    }
}

/// A number with 3–4 significant digits (no exponent for everyday sizes).
pub(super) fn fmt_num(v: f64) -> String {
    if !v.is_finite() {
        return "NA".into();
    }
    let a = v.abs();
    if a != 0.0 && !(1e-3..1e6).contains(&a) {
        return format!("{v:.2e}");
    }
    if (v - v.round()).abs() < 1e-9 {
        return format!("{}", v.round() as i64);
    }
    let digits = if a >= 100.0 {
        1
    } else if a >= 10.0 {
        2
    } else if a >= 1.0 {
        3
    } else {
        (3.0 - a.log10().floor()).clamp(3.0, 6.0) as usize
    };
    let t = format!("{v:.digits$}");
    let t = t.trim_end_matches('0').trim_end_matches('.');
    t.to_string()
}

/// `p = 0.012` / `p < 1e-4`.
pub(super) fn fmt_p(p: f64) -> String {
    if !p.is_finite() {
        return String::new();
    }
    if p < 1e-4 {
        "p < 1e-4".into()
    } else if p < 0.001 {
        format!("p = {p:.4}")
    } else if p < 0.01 {
        format!("p = {p:.3}")
    } else {
        format!("p = {p:.2}")
    }
}

/// ggpubr's significance stars.
pub(super) fn stars(p: f64) -> &'static str {
    match p {
        p if !p.is_finite() => "",
        p if p <= 1e-4 => "****",
        p if p <= 1e-3 => "***",
        p if p <= 1e-2 => "**",
        p if p <= 0.05 => "*",
        _ => "ns",
    }
}

/// The house theme; `small` shrinks titles for grid cells.
fn styled(plot: GGPlot, o: &RenderOptions, small: bool) -> GGPlot {
    let mut th = ggplot_rs::theme::presets::theme_minimal();
    if small {
        th.title.size = (th.title.size * 0.8).max(10.0);
        th.axis_title_x.size = (th.axis_title_x.size * 0.85).max(9.0);
        th.axis_title_y.size = (th.axis_title_y.size * 0.85).max(9.0);
    }
    plot.primary_color(o.brand())
        .theme(th)
        .legend_position(ggplot_rs::theme::LegendPosition::Top)
}

fn finish(plot: GGPlot, w: u32, h: u32, place: Place) -> Out {
    let (w, h) = (crate::clamp_dim(w as u64), crate::clamp_dim(h as u64));
    crate::Panel::plot(plot, w, h).finish(place)
}

/// Several plots in a patchwork-style grid with one shared legend.
fn grid(plots: Vec<GGPlot>, ncol: usize, title: Option<&str>, w: u32, h: u32, place: Place) -> Out {
    let mut g = PlotGrid::new().ncol(ncol.max(1)).collect_legends(true);
    for p in plots {
        g = g.add(p);
    }
    if let Some(t) = title {
        g = g.title(t);
    }
    crate::finish_grid(g, place, w, h)
}

/// Columns of `cols` grid cells for `n` panels.
fn grid_cols(n: usize, max: usize) -> usize {
    match n {
        0..=1 => 1,
        2 => 2,
        3 | 5 | 6 | 9 => 3.min(max),
        _ => 4.min(max),
    }
}

/// The DataZoo discrete scale (sorted levels).
fn dz(aesthetic: Aesthetic) -> ScaleColorDiscrete {
    ScaleColorDiscrete::new(aesthetic)
        .sorted()
        .with_palette((0..DZ_COLORS.len()).map(crate::dz_color).collect())
}

/// Distinct non-null text values of `k`, first-seen order.
fn levels(t: &Table, k: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for v in t.text(k) {
        if let Value::Str(s) = v {
            if !out.contains(&s) {
                out.push(s);
            }
        }
    }
    out
}

/// One text key per row from the columns in `keys` that vary (joined with
/// " · "), or `None` when none varies.
fn combo(t: &Table, keys: &[&str]) -> Option<Vec<Value>> {
    let vary: Vec<&str> = keys
        .iter()
        .copied()
        .filter(|k| t.has(k) && t.distinct(k) > 1)
        .collect();
    if vary.is_empty() {
        return None;
    }
    let cols: Vec<Vec<Value>> = vary.iter().map(|k| t.text(k)).collect();
    Some(
        (0..t.rows.len())
            .map(|i| {
                let parts: Vec<String> = cols.iter().filter_map(|c| text_of(&c[i])).collect();
                s(parts.join(" · "))
            })
            .collect(),
    )
}

/// A numeric y axis carrying category `labels` at 1..n, top to bottom.
fn label_axis(labels: &[String]) -> ScaleContinuous {
    let n = labels.len();
    ScaleContinuous::new()
        .with_limits(0.4, n as f64 + 0.6)
        .with_expand(0.0, 0.0)
        .with_breaks((1..=n).map(|i| i as f64).collect())
        .with_labels(labels.iter().rev().cloned().collect())
}

/// Row `i` of `n` labels → its y slot (first label on top).
fn slot(i: usize, n: usize) -> f64 {
    (n - i) as f64
}

/// x limits covering `vals` plus room on the right for text labels.
fn text_room(vals: &[f64], right: f64) -> (f64, f64, f64) {
    let lo = vals.iter().copied().fold(f64::INFINITY, f64::min);
    let hi = vals.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    if !lo.is_finite() {
        return (0.0, 1.0, 0.05);
    }
    let span = if hi > lo {
        hi - lo
    } else if hi != 0.0 {
        hi.abs() * 0.4
    } else {
        1.0
    };
    let (lo, hi) = if hi > lo {
        (lo, hi)
    } else {
        (lo - span / 2.0, hi + span / 2.0)
    };
    (lo - span * 0.06, hi + span * right, span * 0.03)
}

// ── test: comparison forest ─────────────────────────────────────────────────

/// Columns of the `test` schema (and of the `*_agg` test STRUCTs) that are
/// never a grouping key.
const TEST_COLS: &[&str] = &[
    "test_id",
    "method",
    "group1",
    "group2",
    "alternative",
    "effect_name",
    "p_adjust_method",
    "statistic",
    "p_value",
    "p_adj",
];

/// The column that tells several test tables apart: `id`, `model_id`, else
/// any other text column that varies (an unnested `*_agg`'s GROUP BY key).
fn test_group(t: &Table) -> Option<String> {
    for k in ["id", "model_id"] {
        if t.has(k) && t.distinct(k) > 1 {
            return Some(k.into());
        }
    }
    let first = t.rows.first()?;
    first
        .keys()
        .filter(|k| !TEST_COLS.contains(&k.as_str()))
        .find(|k| {
            t.rows
                .iter()
                .all(|r| matches!(r.get(k.as_str()), Some(J::String(_)) | Some(J::Null) | None))
                && t.distinct(k) > 1
        })
        .cloned()
}

struct TestRow {
    label: Option<String>,
    group: Option<String>,
    method: Option<String>,
    p: Option<f64>,
    est: Option<f64>,
    lo: Option<f64>,
    hi: Option<f64>,
    stat: Option<f64>,
}

/// `test`: one row per comparison — the estimate with its interval when the
/// test has one, else the statistic, else −log10(p) — coloured by
/// significance at `alpha`, the (adjusted) p-value written beside each row.
/// Several groups (`id` / `model_id` / an unnested aggregate's GROUP BY
/// column) become one panel each.
pub(super) fn tests(
    t: &Table,
    options: &Row,
    w: u32,
    h: u32,
    o: &RenderOptions,
    place: Place,
) -> Out {
    let alpha = options
        .get("alpha")
        .and_then(J::as_f64)
        .filter(|a| *a > 0.0 && *a < 1.0)
        .unwrap_or(0.05);
    let num = |k: &str| -> Result<Vec<Value>, String> {
        if t.rows.iter().any(|r| r.contains_key(k)) {
            t.num(k)
        } else {
            Ok(vec![Value::Na; t.rows.len()])
        }
    };
    let (p_adj, p_value, est, lo, hi, stat) = (
        num("p_adj")?,
        num("p_value")?,
        num("estimate")?,
        num("conf_low")?,
        num("conf_high")?,
        num("statistic")?,
    );
    // The *_agg STRUCTs carry ci_lower/ci_upper next to (or instead of) conf_*.
    let (lo2, hi2) = (num("ci_lower")?, num("ci_upper")?);
    let group = test_group(t);
    let gvals = group.as_deref().map(|g| t.text(g));
    let (tid, g1, g2, meth) = (
        t.text("test_id"),
        t.text("group1"),
        t.text("group2"),
        t.text("method"),
    );
    let rows: Vec<TestRow> = (0..t.rows.len())
        .map(|i| {
            let label = text_of(&tid[i]).or_else(|| match (text_of(&g1[i]), text_of(&g2[i])) {
                (Some(a), Some(b)) => Some(format!("{a} vs {b}")),
                _ => None,
            });
            TestRow {
                label,
                group: gvals.as_ref().and_then(|g| text_of(&g[i])),
                method: text_of(&meth[i]),
                p: fin(&p_adj[i]).or_else(|| fin(&p_value[i])),
                est: fin(&est[i]),
                lo: fin(&lo[i]).or_else(|| fin(&lo2[i])),
                hi: fin(&hi[i]).or_else(|| fin(&hi2[i])),
                stat: fin(&stat[i]),
            }
        })
        .collect();
    let mode = match opt_str(options, "value").as_deref() {
        Some("estimate") => "estimate",
        Some("statistic") => "statistic",
        Some("p") | Some("p_value") => "p",
        Some(other) => {
            return Err(format!(
                "value := '{other}' is not one of 'estimate', 'statistic', 'p'"
            ))
        }
        None if rows.iter().any(|r| r.est.is_some()) => "estimate",
        None if rows.iter().all(|r| r.stat.is_some()) => "statistic",
        None if rows.iter().any(|r| r.p.is_some()) => "p",
        None if rows.iter().any(|r| r.stat.is_some()) => "statistic",
        None => {
            return Err(format!(
                "no estimate, statistic or p_value to plot (got: {})",
                t.keys()
            ))
        }
    };
    let methods: Vec<String> = {
        let mut m: Vec<String> = Vec::new();
        for r in &rows {
            if let Some(x) = &r.method {
                if !m.contains(x) {
                    m.push(x.clone());
                }
            }
        }
        m
    };
    let title = (methods.len() == 1).then(|| methods[0].clone());
    // Odds ratios (Fisher's exact test, …) are compared with 1, not 0.
    let ratio = methods
        .iter()
        .any(|m| m.to_ascii_lowercase().contains("fisher"))
        || levels(t, "effect_name").iter().any(|e| e.contains("ratio"))
        || t.has("odds_ratio");
    // Rows without a comparison label: one test per group → the group is the
    // label; else the method.
    let labelled = rows.iter().any(|r| r.label.is_some());
    let mut panels: Vec<(Option<String>, Vec<&TestRow>)> = Vec::new();
    if labelled && group.is_some() {
        for r in &rows {
            match panels.iter_mut().find(|(g, _)| *g == r.group) {
                Some((_, v)) => v.push(r),
                None => panels.push((r.group.clone(), vec![r])),
            }
        }
    } else {
        panels.push((None, rows.iter().collect()));
    }
    let label_of = |r: &TestRow| -> String {
        r.label
            .clone()
            .or_else(|| r.group.clone())
            .or_else(|| (methods.len() > 1).then(|| r.method.clone()).flatten())
            .unwrap_or_else(|| "test".into())
    };
    let level = t
        .num("conf_level")
        .ok()
        .and_then(|v| v.iter().find_map(fin))
        .map(|l| format!("{:.0}% CI", l * 100.0))
        .unwrap_or_else(|| "CI".into());
    let xlab = match mode {
        "estimate" if rows.iter().any(|r| r.lo.is_some()) => format!("estimate ({level})"),
        "estimate" => "estimate".to_string(),
        "statistic" => "test statistic".to_string(),
        _ => "−log10(p)".to_string(),
    };
    let small = panels.len() > 1;
    let mut plots = Vec::new();
    for (g, prs) in &panels {
        let mut plot = test_panel(prs, mode, ratio, alpha, &label_of, o, small)?
            .xlab(&xlab)
            .ylab("");
        if let Some(g) = g {
            plot = plot.title(g);
        } else if let Some(tt) = &title {
            plot = plot.title(tt);
        }
        plots.push(plot);
    }
    if plots.len() == 1 {
        let plot = plots.pop().expect("one plot");
        return finish(plot, w, h, place);
    }
    let n = plots.len();
    grid(plots, grid_cols(n, 3), title.as_deref(), w, h, place)
}

#[allow(clippy::too_many_arguments)]
fn test_panel(
    rows: &[&TestRow],
    mode: &str,
    ratio: bool,
    alpha: f64,
    label_of: &dyn Fn(&TestRow) -> String,
    o: &RenderOptions,
    small: bool,
) -> Result<GGPlot, String> {
    let n = rows.len();
    let labels: Vec<String> = rows.iter().map(|r| label_of(r)).collect();
    let value = |r: &TestRow| -> Option<f64> {
        match mode {
            "estimate" => r.est,
            "statistic" => r.stat,
            _ => r.p.map(|p| -p.max(1e-300).log10()),
        }
    };
    let sig_label = format!("p ≤ {}", fmt_num(alpha));
    let sig = |r: &TestRow| -> String {
        match r.p {
            Some(p) if p <= alpha => sig_label.clone(),
            Some(_) => "n.s.".into(),
            None => "no p-value".into(),
        }
    };
    let ys: Vec<Value> = (0..n).map(|i| Value::Float(slot(i, n))).collect();
    let xs: Vec<Value> = rows
        .iter()
        .map(|r| value(r).map_or(Value::Float(f64::NAN), Value::Float))
        .collect();
    let sigs: Vec<Value> = rows.iter().map(|r| s(sig(r))).collect();
    let tips: Vec<Value> = rows
        .iter()
        .zip(&labels)
        .map(|(r, l)| s(format!("{l}: {}", r.p.map(fmt_p).unwrap_or_default())))
        .collect();
    let mut extent: Vec<f64> = rows.iter().filter_map(|r| value(r)).collect();
    let interval = mode == "estimate" && rows.iter().any(|r| r.lo.is_some() && r.hi.is_some());
    if interval {
        extent.extend(rows.iter().flat_map(|r| [r.lo, r.hi]).flatten());
    }
    let reference = match mode {
        "p" => Some(-alpha.log10()),
        // An odds ratio's null is 1.
        "estimate" if ratio => Some(1.0),
        _ => Some(0.0),
    };
    // Keep the reference line in view when the data come near it.
    if let Some(r) = reference {
        let lo = extent.iter().copied().fold(f64::INFINITY, f64::min);
        let hi = extent.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        let near = r >= lo - (hi - lo) * 0.25 && r <= hi + (hi - lo) * 0.25;
        if lo.is_finite() && (near || mode != "estimate") {
            extent.push(r);
        }
    }
    let (xlo, xhi, mut pad) = text_room(&extent, if small { 0.55 } else { 0.42 });
    pad *= 1.6;
    let right = extent.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let data: Frame = vec![
        ("x".into(), xs.clone()),
        ("y".into(), ys.clone()),
        ("significance".into(), sigs),
        ("tip".into(), tips),
    ];
    let mut plot =
        GGPlot::new(data).aes(Aes::new().x("x").y("y").color("significance").label("tip"));
    let in_view = reference.filter(|r| *r >= xlo && *r <= xhi);
    if let Some(r) = in_view {
        plot = plot.geom_vline_with(GeomVline {
            xintercept: r,
            color: RULE,
            width: 1.0,
            linetype: if mode == "p" {
                Linetype::Dashed
            } else {
                Linetype::Solid
            },
            alpha: 0.9,
        });
    }
    if interval {
        let (los, his): (Vec<Value>, Vec<Value>) = rows
            .iter()
            .map(|r| match (r.lo, r.hi) {
                (Some(a), Some(b)) => (Value::Float(a), Value::Float(b)),
                _ => (Value::Float(f64::NAN), Value::Float(f64::NAN)),
            })
            .unzip();
        let keep: Vec<usize> = (0..n)
            .filter(|&i| fin(&los[i]).is_some() && fin(&his[i]).is_some())
            .collect();
        let pick = |v: &[Value]| keep.iter().map(|&i| v[i].clone()).collect::<Vec<_>>();
        let sigv: Vec<Value> = keep.iter().map(|&i| s(sig(rows[i]))).collect();
        plot = plot
            .geom_errorbarh_with(GeomErrorbarh {
                color: INK,
                width: 1.4,
                cap_height: 0.0,
                alpha: 1.0,
            })
            .layer_data(vec![
                ("y".into(), pick(&ys)),
                ("lo".into(), pick(&los)),
                ("hi".into(), pick(&his)),
                ("significance".into(), sigv),
            ])
            .layer_aes(
                Aes::new()
                    .y("y")
                    .xmin("lo")
                    .xmax("hi")
                    .color("significance"),
            );
    } else if mode != "estimate" {
        // Lollipops from the reference.
        let base = reference.unwrap_or(0.0);
        plot = plot
            .geom_segment_with(GeomSegment {
                color: INK,
                width: 2.0,
                alpha: 1.0,
            })
            .layer_data(vec![
                ("x".into(), vec![Value::Float(base); n]),
                ("xend".into(), xs.clone()),
                ("y".into(), ys.clone()),
                ("yend".into(), ys.clone()),
                (
                    "significance".into(),
                    rows.iter().map(|r| s(sig(r))).collect(),
                ),
            ])
            .layer_aes(
                Aes::new()
                    .x("x")
                    .xend("xend")
                    .y("y")
                    .yend("yend")
                    .color("significance"),
            );
    }
    plot = plot.geom_point_with(GeomPoint {
        size: 3.2,
        color: INK,
        alpha: 1.0,
    });
    // The p-value (and stars) beside each row, right of everything.
    let notes: Vec<Value> = rows
        .iter()
        .map(|r| match r.p {
            Some(p) => s(format!("{} {}", fmt_p(p), stars(p))),
            None => s(r
                .stat
                .map(|v| format!("stat = {}", fmt_num(v)))
                .unwrap_or_default()),
        })
        .collect();
    if notes
        .iter()
        .any(|v| matches!(v, Value::Str(t) if !t.is_empty()))
    {
        plot = plot
            .geom_text_with(GeomText {
                size: if small { 9.0 } else { 10.0 },
                color: INK,
                hjust: 0.0,
                ..Default::default()
            })
            .layer_data(vec![
                ("x".into(), vec![Value::Float(right + pad); n]),
                ("y".into(), ys),
                ("note".into(), notes),
            ])
            .layer_aes(Aes::new().x("x").y("y").label("note"));
    }
    let present: Vec<String> = rows.iter().map(|r| sig(r)).collect();
    let palette: Vec<(&str, RGBAColor)> = [
        (sig_label.as_str(), rgba(ACCENT)),
        ("n.s.", rgba(GREY)),
        ("no p-value", rgba(o.brand())),
    ]
    .into_iter()
    .filter(|(k, _)| present.iter().any(|p| p == k))
    .collect();
    plot = plot
        .scale_color_manual(palette)
        .scale_x_continuous(ScaleContinuous::new().with_limits(xlo, xhi))
        .scale_y_continuous(label_axis(&labels));
    Ok(styled(plot, o, small))
}

fn rgba((r, g, b): (u8, u8, u8)) -> RGBAColor {
    RGBAColor::new(r, g, b)
}

// ── compare: groups + significance brackets ─────────────────────────────────

/// The `tests` option as rows: a LIST of test STRUCTs (`(SELECT list(t) FROM
/// tukey_hsd_by(…) t)`, `tukey_hsd_agg(y, g)`) or one STRUCT (`t_test_agg`).
fn test_rows(v: Option<&J>) -> Vec<Row> {
    match v {
        Some(J::Array(a)) => a.iter().filter_map(J::as_object).cloned().collect(),
        Some(J::Object(o)) => vec![o.clone()],
        // DuckDB may hand a JSON value over as its text.
        Some(J::String(s)) => serde_json::from_str::<J>(s)
            .ok()
            .map(|j| test_rows(Some(&j)))
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

/// `compare`: the `value` column per level of the `factor` column (box,
/// violin or points) with significance brackets from a precomputed test
/// table (`tests :=`), ggpubr's `stat_pvalue_manual`.
pub(super) fn compare(
    t: &Table,
    options: &Row,
    w: u32,
    h: u32,
    o: &RenderOptions,
    place: Place,
) -> Out {
    let value = opt_str(options, "value")
        .ok_or_else(|| format!("needs value := '<numeric column>' (got: {})", t.keys()))?;
    let factor = opt_str(options, "factor")
        .or_else(|| opt_str(options, "group"))
        .or_else(|| opt_str(options, "by"))
        .ok_or_else(|| format!("needs factor := '<group column>' (got: {})", t.keys()))?;
    t.need(&[value.as_str(), factor.as_str()])?;
    let kind = opt_str(options, "kind").unwrap_or_else(|| "box".into());
    if !["box", "violin", "points"].contains(&kind.as_str()) {
        return Err(format!(
            "kind := '{kind}' is not one of 'box', 'violin', 'points'"
        ));
    }
    let ys = t.num(&value)?;
    let gs = t.text(&factor);
    let keep: Vec<usize> = (0..ys.len())
        .filter(|&i| fin(&ys[i]).is_some() && gs[i] != Value::Na)
        .collect();
    if keep.is_empty() {
        return crate::note_svg(None, "No data", w, h).finish(place);
    }
    let g: Vec<Value> = keep.iter().map(|&i| gs[i].clone()).collect();
    let y: Vec<Value> = keep.iter().map(|&i| ys[i].clone()).collect();
    // Levels in natural order (numbers numerically) at x = 1..k: a numeric
    // axis lets brackets and their labels sit between two groups.
    let mut lv: Vec<String> = g.iter().filter_map(text_of).collect();
    lv.sort_by(|a, b| match (a.parse::<f64>(), b.parse::<f64>()) {
        (Ok(p), Ok(q)) => p.total_cmp(&q),
        _ => a.cmp(b),
    });
    lv.dedup();
    let pos = |name: &str| lv.iter().position(|l| l == name).map(|i| i as f64 + 1.0);
    let x: Vec<Value> = g
        .iter()
        .map(|v| {
            text_of(v)
                .and_then(|s| pos(&s))
                .map_or(Value::Na, Value::Float)
        })
        .collect();
    let brand = o.brand();
    let data: Frame = vec![
        ("x".into(), x.clone()),
        ("v".into(), y.clone()),
        ("g".into(), g.clone()),
    ];
    let mut plot = GGPlot::new(data).aes(Aes::new().x("x").y("v").group("g").label("g"));
    let body = crate::lighten(brand, 0.72);
    let points = GeomJitter {
        size: 1.6,
        color: crate::darken(brand, 0.15),
        alpha: 0.55,
        width: Some(0.12),
        height: Some(0.0),
    };
    plot = match kind.as_str() {
        "violin" => plot
            .geom_violin_with(GeomViolin {
                fill: body,
                color: crate::darken(brand, 0.35),
                alpha: 1.0,
                line_width: 1.0,
            })
            .geom_jitter_with(points),
        "points" => plot.geom_jitter_with(GeomJitter {
            size: 2.2,
            alpha: 0.75,
            ..points
        }),
        _ => plot
            .geom_boxplot_with(GeomBoxplot {
                fill: body,
                color: (60, 60, 60),
                width: 0.55,
                alpha: 1.0,
            })
            .geom_jitter_with(points),
    };
    if kind == "points" {
        // Group means as a reference mark.
        let mut sums: HashMap<String, (f64, usize)> = HashMap::new();
        for (gv, v) in g.iter().zip(&y) {
            if let (Some(gv), Some(v)) = (text_of(gv), fin(v)) {
                let e = sums.entry(gv).or_insert((0.0, 0));
                e.0 += v;
                e.1 += 1;
            }
        }
        let (mx, mv): (Vec<Value>, Vec<Value>) = lv
            .iter()
            .enumerate()
            .filter_map(|(i, l)| {
                sums.get(l)
                    .map(|(s, n)| (Value::Float(i as f64 + 1.0), Value::Float(s / *n as f64)))
            })
            .unzip();
        plot = plot
            .geom_point_with(GeomPoint {
                size: 4.5,
                color: ACCENT,
                alpha: 1.0,
            })
            .layer_data(vec![("x".into(), mx), ("v".into(), mv)])
            .layer_aes(Aes::new().x("x").y("v"));
    }
    let mut warnings = Vec::new();
    // Brackets from the precomputed tests.
    let mut tests = test_rows(options.get("tests"));
    if tests.len() == 1 && lv.len() == 2 {
        // A two-sample test STRUCT (t_test_agg, …) has no group1/group2.
        let r = &mut tests[0];
        if r.get("group1").is_none_or(J::is_null) {
            r.insert("group1".into(), J::String(lv[0].clone()));
            r.insert("group2".into(), J::String(lv[1].clone()));
        }
    }
    let method = tests
        .iter()
        .find_map(|r| r.get("method").and_then(J::as_str))
        .map(str::to_string);
    if !tests.is_empty() {
        let key = |r: &Row, k: &str| match r.get(k) {
            Some(J::String(s)) => Some(s.clone()),
            Some(J::Number(n)) => Some(fmt_num(n.as_f64().unwrap_or(f64::NAN))),
            Some(J::Bool(b)) => Some(b.to_string()),
            _ => None,
        };
        let pnum = |r: &Row, k: &str| match r.get(k) {
            Some(J::Number(n)) => n.as_f64().filter(|p| p.is_finite()),
            Some(J::String(s)) => s.parse::<f64>().ok().filter(|p| p.is_finite()),
            _ => None,
        };
        if !tests
            .iter()
            .any(|r| key(r, "group1").is_some() && key(r, "group2").is_some())
        {
            return Err(
                "tests := needs group1 / group2 (a pairwise test table such as tukey_hsd_by, \
                 or a two-sample test STRUCT for two groups)"
                    .into(),
            );
        }
        let hide_ns = options.get("hide_ns").and_then(J::as_bool).unwrap_or(false);
        // (x1, x2, p, test_id) per drawable bracket, narrow spans first.
        let mut br: Vec<Bracket> = Vec::new();
        let mut unknown: Vec<String> = Vec::new();
        for r in &tests {
            let (Some(a), Some(b)) = (key(r, "group1"), key(r, "group2")) else {
                continue;
            };
            let (Some(pa), Some(pb)) = (pos(&a), pos(&b)) else {
                for gname in [a, b] {
                    if pos(&gname).is_none() && !unknown.contains(&gname) {
                        unknown.push(gname);
                    }
                }
                continue;
            };
            let p = pnum(r, "p_adj").or_else(|| pnum(r, "p_value"));
            if hide_ns && p.is_some_and(|p| p > 0.05) {
                continue;
            }
            br.push((pa.min(pb), pa.max(pb), p, key(r, "test_id")));
        }
        if !unknown.is_empty() {
            warnings.push(format!(
                "anofox_plot_compare: tests name group(s) not in `{factor}` ({}); those brackets \
                 are not drawn — filter the tests to the plotted rows",
                unknown.join(", ")
            ));
        }
        br.sort_by(|a, b| {
            (a.1 - a.0)
                .total_cmp(&(b.1 - b.0))
                .then(a.0.total_cmp(&b.0))
        });
        if !br.is_empty() {
            let yv: Vec<f64> = y.iter().filter_map(fin).collect();
            let (lo, hi) = yv
                .iter()
                .fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), v| {
                    (a.min(*v), b.max(*v))
                });
            let step = if hi > lo {
                (hi - lo) * 0.11
            } else {
                hi.abs().max(1.0) * 0.11
            };
            let at = |k: usize| hi + step * (k as f64 + 0.8);
            let user_label = opt_str(options, "label");
            let mut frame: Frame = vec![
                (
                    "group1".into(),
                    br.iter().map(|b| Value::Float(b.0)).collect(),
                ),
                (
                    "group2".into(),
                    br.iter().map(|b| Value::Float(b.1)).collect(),
                ),
                (
                    "p_value".into(),
                    br.iter()
                        .map(|b| b.2.map_or(Value::Na, Value::Float))
                        .collect(),
                ),
                (
                    "y_position".into(),
                    (0..br.len()).map(|k| Value::Float(at(k))).collect(),
                ),
            ];
            if br.iter().any(|b| b.3.is_some()) {
                frame.push((
                    "test_id".into(),
                    br.iter()
                        .map(|b| b.3.clone().map_or(Value::Na, Value::Str))
                        .collect(),
                ));
            }
            // A user template is resolved by ggplot-rs (its label sits close to
            // the bar); the default label is drawn here, clear above it.
            let spec = BracketTable::new()
                .label(user_label.as_deref().unwrap_or(""))
                .style(GeomBracketStyle {
                    color: INK,
                    line_width: 1.0,
                    tip_length: 5.0,
                    label_size: 10.0,
                });
            plot = plot.geom_bracket_table(frame, spec);
            if user_label.is_none() {
                let notes: Vec<Value> = br
                    .iter()
                    .map(|b| match b.2 {
                        Some(p) => s(format!("{} {}", fmt_p(p), stars(p))),
                        None => s(""),
                    })
                    .collect();
                plot = plot
                    .geom_text_with(GeomText {
                        size: 10.0,
                        color: INK,
                        hjust: 0.5,
                        vjust: 0.0,
                        ..Default::default()
                    })
                    .layer_data(vec![
                        (
                            "x".into(),
                            br.iter().map(|b| Value::Float((b.0 + b.1) / 2.0)).collect(),
                        ),
                        (
                            "v".into(),
                            (0..br.len())
                                .map(|k| Value::Float(at(k) + step * 0.08))
                                .collect(),
                        ),
                        ("note".into(), notes),
                    ])
                    .layer_aes(Aes::new().x("x").y("v").label("note"));
            }
            // Head room for the top label.
            plot = plot
                .geom_blank()
                .layer_data(vec![
                    ("x".into(), vec![Value::Float(1.0)]),
                    (
                        "v".into(),
                        vec![Value::Float(at(br.len() - 1) + step * 0.75)],
                    ),
                ])
                .layer_aes(Aes::new().x("x").y("v"));
        }
    }
    let k = lv.len();
    plot = plot
        .scale_x_continuous(
            ScaleContinuous::new()
                .with_limits(0.4, k as f64 + 0.6)
                .with_expand(0.0, 0.0)
                .with_breaks((1..=k).map(|i| i as f64).collect())
                .with_labels(lv.clone()),
        )
        .xlab(&factor)
        .ylab(&value);
    if let Some(m) = method {
        plot = plot.title(&m);
    }
    let (svg, mut w0) = finish(styled(plot, o, false), w, h, place)?;
    w0.extend(warnings);
    Ok((svg, w0))
}

// ── partial residuals ───────────────────────────────────────────────────────

/// `partial`: one panel per term of a `<model>_partial_residuals_by` table —
/// `component` (component + residual: the partial residuals over x, the
/// term's component line and a loess) or `added_variable` (y- vs
/// x-residuals with the least-squares line through the origin, whose slope is
/// the coefficient).
pub(super) fn partial(
    t: &Table,
    options: &Row,
    w: u32,
    h: u32,
    o: &RenderOptions,
    place: Place,
) -> Out {
    let kind = opt_str(options, "kind").unwrap_or_else(|| "component".into());
    let av = match kind.as_str() {
        "component" | "component_residual" | "cr" => false,
        "added_variable" | "av" => true,
        other => {
            return Err(format!(
                "kind := '{other}' is not one of 'component', 'added_variable'"
            ))
        }
    };
    let (xk, yk) = if av {
        ("x_residual", "y_residual")
    } else {
        ("x", "partial_residual")
    };
    t.need(&["term", xk, yk])?;
    let terms = levels(t, "term");
    let (xs, ys) = (t.num(xk)?, t.num(yk)?);
    let comp = if !av && t.has("component") {
        Some(t.num("component")?)
    } else {
        None
    };
    let term_col = t.text("term");
    let series = combo(t, &["model_id", "id"]);
    let brand = o.brand();
    let small = terms.len() > 1;
    let mut plots = Vec::new();
    let mut titles: Vec<String> = Vec::new();
    for term in &terms {
        let idx: Vec<usize> = (0..t.rows.len())
            .filter(|&i| matches!(&term_col[i], Value::Str(s) if s == term))
            .filter(|&i| fin(&xs[i]).is_some() && fin(&ys[i]).is_some())
            .collect();
        if idx.is_empty() {
            continue;
        }
        let pick = |v: &[Value]| idx.iter().map(|&i| v[i].clone()).collect::<Vec<_>>();
        let mut data: Frame = vec![("x".into(), pick(&xs)), ("y".into(), pick(&ys))];
        let mut aes = Aes::new().x("x").y("y");
        if let Some(sv) = &series {
            data.push(("model".into(), pick(sv)));
            aes = aes.color("model").group("model");
        }
        let mut plot = GGPlot::new(data.clone())
            .aes(aes)
            .geom_point_with(GeomPoint {
                size: 1.8,
                color: brand,
                alpha: 0.7,
            });
        let mut panel_title = term.clone();
        if av {
            // Least squares through the origin, per series.
            let groups: Vec<(Option<String>, Vec<usize>)> = match &series {
                None => vec![(None, (0..idx.len()).collect())],
                Some(sv) => {
                    let mut g: Vec<(Option<String>, Vec<usize>)> = Vec::new();
                    for (k, &i) in idx.iter().enumerate() {
                        let key = text_of(&sv[i]);
                        match g.iter_mut().find(|(gk, _)| *gk == key) {
                            Some((_, v)) => v.push(k),
                            None => g.push((key, vec![k])),
                        }
                    }
                    g
                }
            };
            let mut notes = Vec::new();
            for (gi, (key, ks)) in groups.iter().enumerate() {
                let (mut sxy, mut sxx) = (0.0, 0.0);
                for &k in ks {
                    let (xv, yv) = (
                        fin(&data[0].1[k]).unwrap_or(0.0),
                        fin(&data[1].1[k]).unwrap_or(0.0),
                    );
                    sxy += xv * yv;
                    sxx += xv * xv;
                }
                if sxx <= 0.0 {
                    continue;
                }
                let slope = sxy / sxx;
                let color = if series.is_some() {
                    let lv = {
                        let mut l: Vec<String> =
                            groups.iter().filter_map(|g| g.0.clone()).collect();
                        l.sort();
                        l
                    };
                    let i = key
                        .as_ref()
                        .and_then(|k| lv.iter().position(|l| l == k))
                        .unwrap_or(gi);
                    DZ_COLORS[i % DZ_COLORS.len()]
                } else {
                    crate::darken(brand, 0.25)
                };
                plot = plot.geom_abline_with(GeomAbline {
                    slope,
                    intercept: 0.0,
                    color,
                    width: 1.6,
                    linetype: Linetype::Solid,
                    alpha: 1.0,
                });
                notes.push(match key {
                    Some(k) => format!("{k}: slope {}", fmt_num(slope)),
                    None => format!("slope = {}", fmt_num(slope)),
                });
            }
            plot = plot.xlab(&format!("{term} | others")).ylab("y | others");
            if !notes.is_empty() {
                panel_title = format!("{term} ({})", notes.join(", "));
            }
        } else {
            if let Some(c) = &comp {
                let mut order: Vec<usize> = idx.clone();
                order.sort_by(|&a, &b| {
                    fin(&xs[a])
                        .unwrap_or(0.0)
                        .total_cmp(&fin(&xs[b]).unwrap_or(0.0))
                });
                let mut frame: Frame = vec![
                    ("x".into(), order.iter().map(|&i| xs[i].clone()).collect()),
                    (
                        "y".into(),
                        order
                            .iter()
                            .map(|&i| fin(&c[i]).map_or(Value::Float(f64::NAN), Value::Float))
                            .collect(),
                    ),
                ];
                if let Some(sv) = &series {
                    frame.push((
                        "model".into(),
                        order.iter().map(|&i| sv[i].clone()).collect(),
                    ));
                }
                plot = plot
                    .geom_line_with(GeomLine {
                        color: crate::darken(brand, 0.3),
                        width: 1.8,
                        alpha: 1.0,
                    })
                    .layer_data(frame);
            }
            if idx.len() >= 6 && series.is_none() {
                plot = plot.geom_smooth_with(GeomSmooth {
                    color: ACCENT,
                    se: false,
                    line_width: 1.4,
                    method: ggplot_rs::stat::smooth::SmoothMethod::Loess { span: 0.75 },
                    ..Default::default()
                });
            }
            plot = plot.xlab(term).ylab("component + residual");
        }
        if series.is_some() {
            plot = plot.scale_color(dz(Aesthetic::Color));
        }
        plots.push(styled(plot.title(&panel_title), o, small));
        titles.push(panel_title);
    }
    if plots.is_empty() {
        return crate::note_svg(None, "No data", w, h).finish(place);
    }
    let title = if av {
        "Added-variable plots"
    } else {
        "Component + residual plots"
    };
    if plots.len() == 1 {
        let p = plots
            .pop()
            .expect("one plot")
            .title(&format!("{title}: {}", titles[0]));
        return finish(p, w, h, place);
    }
    let n = plots.len();
    grid(plots, grid_cols(n, 3), Some(title), w, h, place)
}

// ── summary: one scale per metric ───────────────────────────────────────────

/// Glance metrics in display order; anything else follows alphabetically.
const METRIC_ORDER: &[&str] = &[
    "r_squared",
    "adj_r_squared",
    "pseudo_r_squared",
    "auc",
    "accuracy",
    "rmse",
    "mae",
    "residual_std_error",
    "sigma",
    "aic",
    "aicc",
    "bic",
    "log_likelihood",
    "deviance",
    "null_deviance",
    "dispersion",
];
/// Bookkeeping, not fit quality: hidden unless `metrics := […]` asks.
const METRIC_HIDDEN: &[&str] = &[
    "n_observations",
    "n_features",
    "iterations",
    "n_obs",
    "nobs",
    "df",
    "df_residual",
    "converged",
];
const HIGHER_BETTER: &[&str] = &[
    "r_squared",
    "adj_r_squared",
    "pseudo_r_squared",
    "auc",
    "accuracy",
    "log_likelihood",
];
const LOWER_BETTER: &[&str] = &[
    "rmse",
    "mae",
    "residual_std_error",
    "sigma",
    "aic",
    "aicc",
    "bic",
    "deviance",
];

fn metric_title(m: &str) -> String {
    match m {
        "r_squared" => "R²".into(),
        "adj_r_squared" => "adjusted R²".into(),
        "pseudo_r_squared" => "pseudo R²".into(),
        "aic" => "AIC".into(),
        "aicc" => "AICc".into(),
        "bic" => "BIC".into(),
        "auc" => "AUC".into(),
        "rmse" => "RMSE".into(),
        "mae" => "MAE".into(),
        "residual_std_error" => "residual SE".into(),
        "log_likelihood" => "log-likelihood".into(),
        other => other.replace('_', " "),
    }
}

/// `summary`: a grid of small panels, one per metric with its own x scale,
/// the models on the vertical axis, a dot (± conf_low..conf_high) and the
/// value written beside it; the best model of a known-direction metric is
/// highlighted. Bookkeeping metrics (n_observations, n_features,
/// iterations) are left out unless `metrics := […]` lists them.
pub(super) fn summary(
    t: &Table,
    options: &Row,
    w: u32,
    h: u32,
    o: &RenderOptions,
    place: Place,
) -> Out {
    let model = t.model_key().unwrap_or("model_id");
    t.need(&["metric", "value"])?;
    let labels: Vec<Value> = combo(t, &[model, "id"]).unwrap_or_else(|| {
        let m = t.text(model);
        if m.iter().all(|v| *v == Value::Na) {
            vec![s("model"); t.rows.len()]
        } else {
            m
        }
    });
    let label_levels: Vec<String> = {
        let mut l: Vec<String> = Vec::new();
        for v in &labels {
            if let Some(x) = text_of(v) {
                if !l.contains(&x) {
                    l.push(x);
                }
            }
        }
        l
    };
    let metric = t.text("metric");
    let value = t.num("value")?;
    let ci = t.has("conf_low") && t.has("conf_high");
    let (clo, chi) = if ci {
        (t.num("conf_low")?, t.num("conf_high")?)
    } else {
        (Vec::new(), Vec::new())
    };
    let present: Vec<String> = levels(t, "metric");
    let wanted: Vec<String> = match options.get("metrics") {
        Some(J::Array(a)) if !a.is_empty() => {
            let ask: Vec<String> = a.iter().filter_map(J::as_str).map(str::to_string).collect();
            let got: Vec<String> = ask
                .iter()
                .filter(|m| present.contains(m))
                .cloned()
                .collect();
            if got.is_empty() {
                return Err(format!(
                    "metrics := [{}] matches none of the metrics present ({})",
                    ask.join(", "),
                    present.join(", ")
                ));
            }
            got
        }
        _ => {
            let mut shown: Vec<String> = METRIC_ORDER
                .iter()
                .filter(|m| present.iter().any(|p| p == *m))
                .map(|m| m.to_string())
                .collect();
            let mut rest: Vec<String> = present
                .iter()
                .filter(|p| {
                    !METRIC_ORDER.contains(&p.as_str()) && !METRIC_HIDDEN.contains(&p.as_str())
                })
                .cloned()
                .collect();
            rest.sort();
            shown.extend(rest);
            if shown.is_empty() {
                present.clone()
            } else {
                shown
            }
        }
    };
    let n = label_levels.len();
    let many = wanted.len() > 1;
    let ncol = grid_cols(wanted.len(), 4);
    let mut plots = Vec::new();
    for (pi, m) in wanted.iter().enumerate() {
        let idx: Vec<usize> = (0..t.rows.len())
            .filter(|&i| matches!(&metric[i], Value::Str(s) if s == m) && fin(&value[i]).is_some())
            .collect();
        let rank_of = |i: usize| -> usize {
            text_of(&labels[i])
                .and_then(|l| label_levels.iter().position(|x| *x == l))
                .unwrap_or(0)
        };
        let vals: Vec<f64> = idx.iter().filter_map(|&i| fin(&value[i])).collect();
        let best = if n > 1 && HIGHER_BETTER.contains(&m.as_str()) {
            vals.iter()
                .copied()
                .fold(None, |b: Option<f64>, v| Some(b.map_or(v, |b| b.max(v))))
        } else if n > 1 && LOWER_BETTER.contains(&m.as_str()) {
            vals.iter()
                .copied()
                .fold(None, |b: Option<f64>, v| Some(b.map_or(v, |b| b.min(v))))
        } else {
            None
        };
        let mut extent = vals.clone();
        if ci {
            extent.extend(
                idx.iter()
                    .flat_map(|&i| [fin(&clo[i]), fin(&chi[i])])
                    .flatten(),
            );
        }
        // Differences below ~3 % of the values' size are not stretched across
        // the panel (R² 0.9934 vs 0.9929 should look alike).
        if let (Some(lo), Some(hi)) = (
            extent.iter().copied().reduce(f64::min),
            extent.iter().copied().reduce(f64::max),
        ) {
            let min_span = lo.abs().max(hi.abs()) * 0.03;
            if hi - lo < min_span {
                let mid = (lo + hi) / 2.0;
                extent.extend([mid - min_span / 2.0, mid + min_span / 2.0]);
            }
        }
        let (xlo, xhi, pad) = text_room(&extent, 0.6);
        let ys: Vec<Value> = idx
            .iter()
            .map(|&i| Value::Float(slot(rank_of(i), n)))
            .collect();
        let xs: Vec<Value> = idx.iter().map(|&i| value[i].clone()).collect();
        let is_best = |i: usize| best.is_some_and(|b| fin(&value[i]) == Some(b));
        let tips: Vec<Value> = idx
            .iter()
            .map(|&i| {
                s(format!(
                    "{}: {}",
                    text_of(&labels[i]).unwrap_or_default(),
                    fmt_num(fin(&value[i]).unwrap_or(f64::NAN))
                ))
            })
            .collect();
        let mut plot = GGPlot::new(vec![
            ("x".to_string(), xs.clone()),
            ("y".to_string(), ys.clone()),
            ("tip".to_string(), tips),
        ])
        .aes(Aes::new().x("x").y("y").label("tip"));
        if ci {
            let keep: Vec<usize> = idx
                .iter()
                .copied()
                .filter(|&i| fin(&clo[i]).is_some() && fin(&chi[i]).is_some())
                .collect();
            if !keep.is_empty() {
                plot = plot
                    .geom_errorbarh_with(GeomErrorbarh {
                        color: INK,
                        width: 1.2,
                        cap_height: 0.0,
                        alpha: 1.0,
                    })
                    .layer_data(vec![
                        (
                            "y".into(),
                            keep.iter()
                                .map(|&i| Value::Float(slot(rank_of(i), n)))
                                .collect(),
                        ),
                        ("lo".into(), keep.iter().map(|&i| clo[i].clone()).collect()),
                        ("hi".into(), keep.iter().map(|&i| chi[i].clone()).collect()),
                    ])
                    .layer_aes(Aes::new().y("y").xmin("lo").xmax("hi"));
            }
        }
        let right: Vec<Value> = idx
            .iter()
            .map(|&i| {
                let r = if ci { fin(&chi[i]) } else { None };
                Value::Float(r.or_else(|| fin(&value[i])).unwrap_or(0.0) + pad * 2.0)
            })
            .collect();
        plot = plot.geom_point_with(GeomPoint {
            size: 3.0,
            color: o.brand(),
            alpha: 1.0,
        });
        // The best model of a known-direction metric, over its dot.
        let winners: Vec<usize> = idx.iter().copied().filter(|&i| is_best(i)).collect();
        if !winners.is_empty() {
            plot = plot
                .geom_point_with(GeomPoint {
                    size: 3.6,
                    color: ACCENT,
                    alpha: 1.0,
                })
                .layer_data(vec![
                    (
                        "x".into(),
                        winners.iter().map(|&i| value[i].clone()).collect(),
                    ),
                    (
                        "y".into(),
                        winners
                            .iter()
                            .map(|&i| Value::Float(slot(rank_of(i), n)))
                            .collect(),
                    ),
                ])
                .layer_aes(Aes::new().x("x").y("y"));
        }
        plot = plot
            .geom_text_with(GeomText {
                size: 9.0,
                color: INK,
                hjust: 0.0,
                ..Default::default()
            })
            .layer_data(vec![
                ("x".into(), right),
                ("y".into(), ys),
                (
                    "note".into(),
                    idx.iter()
                        .map(|&i| s(fmt_num(fin(&value[i]).unwrap_or(f64::NAN))))
                        .collect(),
                ),
            ])
            .layer_aes(Aes::new().x("x").y("y").label("note"))
            .scale_x_continuous(ScaleContinuous::new().with_limits(xlo, xhi))
            .scale_y_continuous(label_axis(&label_levels))
            .title(&metric_title(m))
            .xlab("")
            .ylab("");
        // The values are written beside the dots: no x tick labels (tiny
        // differences between models would crowd them); model labels once
        // per row of panels.
        let mut plot = styled(plot, o, many).theme_update(ggplot_rs::theme::ThemeUpdate {
            axis_text_x: Some(ggplot_rs::theme::ElementText::blank()),
            ..Default::default()
        });
        if many && pi % ncol != 0 {
            plot = plot.theme_update(ggplot_rs::theme::ThemeUpdate {
                axis_text_y: Some(ggplot_rs::theme::ElementText::blank()),
                ..Default::default()
            });
        }
        plots.push(plot);
    }
    if plots.len() == 1 {
        return finish(plots.pop().expect("one plot"), w, h, place);
    }
    // Equal panel widths would waste the first column on labels; give it room.
    grid(plots, ncol, Some("Model comparison"), w, h, place)
}

// ── prediction with several models ──────────────────────────────────────────

/// `prediction` with several models in one panel: one coloured yhat line (and
/// band) per model; the observed y once, in grey, shaped by split.
#[allow(clippy::too_many_arguments)]
pub(super) fn prediction_models(
    t: &Table,
    x: &str,
    y: &str,
    model: &str,
    facet: Option<&str>,
    w: u32,
    h: u32,
    o: &RenderOptions,
    place: Place,
) -> Out {
    let xs = t.xpos(x);
    let yhat = t.num("yhat")?;
    let models = t.text(model);
    let fac = facet.map(|f| t.text(f));
    let n = t.rows.len();
    let nan = |v: &Value| fin(v).map_or(Value::Float(f64::NAN), Value::Float);
    let mut data: Frame = vec![
        ("x".into(), xs.clone()),
        ("yhat".into(), yhat.iter().map(nan).collect()),
        ("model".into(), models.clone()),
    ];
    if let Some(f) = &fac {
        data.push(("facet".into(), f.clone()));
    }
    let mut plot = GGPlot::new(data).aes(Aes::new().x("x").y("yhat").color("model").group("model"));
    if t.has("yhat_lower") && t.has("yhat_upper") {
        let (lo, hi) = (t.num("yhat_lower")?, t.num("yhat_upper")?);
        let keep: Vec<usize> = (0..n)
            .filter(|&i| fin(&lo[i]).is_some() && fin(&hi[i]).is_some())
            .collect();
        let pick = |v: &[Value]| keep.iter().map(|&i| v[i].clone()).collect::<Vec<_>>();
        let mut frame: Frame = vec![
            ("x".into(), pick(&xs)),
            ("lo".into(), pick(&lo)),
            ("hi".into(), pick(&hi)),
            ("model".into(), pick(&models)),
        ];
        if let Some(f) = &fac {
            frame.push(("facet".into(), pick(f)));
        }
        plot = plot
            .geom_ribbon_with(GeomRibbon {
                fill: o.brand(),
                alpha: 0.15,
            })
            .layer_data(frame)
            .layer_aes(
                Aes::new()
                    .x("x")
                    .ymin("lo")
                    .ymax("hi")
                    .fill("model")
                    .group("model"),
            );
    }
    plot = plot.geom_line_with(GeomLine {
        color: o.brand(),
        width: 1.4,
        alpha: 1.0,
    });
    if t.has(y) {
        // Observed values once per (facet, x).
        let yv = t.num(y)?;
        let split: Vec<Value> = t
            .rows
            .iter()
            .map(|r| match (r.get("split"), r.get("is_training")) {
                (Some(J::String(s)), _) => Value::Str(s.clone()),
                (_, Some(J::Bool(false))) => s("test"),
                _ => s("train"),
            })
            .collect();
        let mut seen = std::collections::HashSet::new();
        let keep: Vec<usize> = (0..n)
            .filter(|&i| fin(&yv[i]).is_some())
            .filter(|&i| {
                let f = fac
                    .as_ref()
                    .map(|f| format!("{:?}", f[i]))
                    .unwrap_or_default();
                seen.insert(format!("{f}|{:?}|{:?}", xs[i], yv[i]))
            })
            .collect();
        let pick = |v: &[Value]| keep.iter().map(|&i| v[i].clone()).collect::<Vec<_>>();
        let splits: Vec<Value> = pick(&split);
        let several = {
            let mut l: Vec<String> = splits.iter().filter_map(text_of).collect();
            l.sort();
            l.dedup();
            l.len() > 1
        };
        let mut frame: Frame = vec![("x".into(), pick(&xs)), ("y".into(), pick(&yv))];
        if let Some(f) = &fac {
            frame.push(("facet".into(), pick(f)));
        }
        let mut aes = Aes::new().x("x").y("y");
        if several {
            frame.push(("split".into(), splits));
            aes = aes.shape("split");
        }
        plot = plot
            .geom_point_with(GeomPoint {
                size: 2.0,
                color: INK,
                alpha: 0.6,
            })
            .layer_data(frame)
            .layer_aes(aes);
        if several {
            plot = plot.scale_shape_manual(vec![
                ("train", PointShape::Circle),
                ("test", PointShape::Triangle),
                ("future", PointShape::Diamond),
            ]);
        }
    }
    plot = plot
        .scale_color(dz(Aesthetic::Color))
        .scale_fill(dz(Aesthetic::Fill))
        .xlab(x)
        .ylab(if t.has(y) { y } else { "yhat" });
    if fac.is_some() {
        plot = plot.facet_wrap_free("facet", Some(1), FacetScales::FreeY);
    }
    finish(styled(plot, o, false), w, h, place)
}

// ── curves: Kaplan–Meier, lambda CV ─────────────────────────────────────────

/// The series key of a curve table: the first of `series` / `model_id` /
/// `id` that varies.
fn curve_series(t: &Table) -> Option<&'static str> {
    ["series", "model_id", "id"]
        .into_iter()
        .find(|k| t.has(k) && t.distinct(k) > 1)
}

/// `curve_type = 'km'`: survival step curves per stratum from (0, 1), the
/// confidence step ribbon where the producer has one (the undefined
/// Greenwood band of the last step is left out, the line is kept), `+`
/// censoring marks from `n_censor`, and n_risk / n_event in the hover text.
pub(super) fn km(t: &Table, w: u32, h: u32, o: &RenderOptions, place: Place) -> Out {
    t.need(&["x", "y"])?;
    let key = curve_series(t);
    let keyv = key.map(|k| t.text(k));
    let (x, y) = (t.num("x")?, t.num("y")?);
    let band = t.has("y_low") && t.has("y_high");
    let (lo, hi) = if band {
        (t.num("y_low")?, t.num("y_high")?)
    } else {
        (Vec::new(), Vec::new())
    };
    let cens = match ["n_censor", "censored"].into_iter().find(|c| t.has(c)) {
        Some(c) => t.num(c)?,
        None => vec![Value::Na; t.rows.len()],
    };
    let risk = if t.has("n_risk") {
        t.num("n_risk")?
    } else {
        vec![Value::Na; t.rows.len()]
    };
    let events = if t.has("n_event") {
        t.num("n_event")?
    } else {
        vec![Value::Na; t.rows.len()]
    };
    // Rows per series, ordered by time.
    let mut by: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for i in 0..t.rows.len() {
        if fin(&x[i]).is_none() || fin(&y[i]).is_none() {
            continue;
        }
        let k = keyv
            .as_ref()
            .and_then(|v| text_of(&v[i]))
            .unwrap_or_default();
        by.entry(k).or_default().push(i);
    }
    let (mut fx, mut fy, mut fk, mut fc, mut ft) = (vec![], vec![], vec![], vec![], vec![]);
    let (mut bx, mut blo, mut bhi, mut bk) = (vec![], vec![], vec![], vec![]);
    for (k, rows) in by.iter_mut() {
        rows.sort_by(|&a, &b| {
            fin(&x[a])
                .unwrap_or(0.0)
                .total_cmp(&fin(&x[b]).unwrap_or(0.0))
        });
        let x0 = rows.first().and_then(|&i| fin(&x[i])).unwrap_or(0.0);
        if x0 > 0.0 {
            // The curve starts at S(0) = 1.
            fx.push(Value::Float(0.0));
            fy.push(Value::Float(1.0));
            fk.push(s(k.clone()));
            fc.push(Value::Float(0.0));
            ft.push(s(format!("{k} t = 0: S = 1")));
            if band {
                bx.push(Value::Float(0.0));
                blo.push(Value::Float(1.0));
                bhi.push(Value::Float(1.0));
                bk.push(s(k.clone()));
            }
        }
        for &i in rows.iter() {
            fx.push(x[i].clone());
            fy.push(y[i].clone());
            fk.push(s(k.clone()));
            fc.push(fin(&cens[i]).map_or(Value::Float(0.0), Value::Float));
            let mut tip = format!(
                "{}t = {}: S = {}",
                if k.is_empty() {
                    String::new()
                } else {
                    format!("{k} ")
                },
                fmt_num(fin(&x[i]).unwrap_or(0.0)),
                fmt_num(fin(&y[i]).unwrap_or(0.0))
            );
            if let Some(r) = fin(&risk[i]) {
                tip.push_str(&format!(", at risk {}", fmt_num(r)));
            }
            if let Some(e) = fin(&events[i]) {
                tip.push_str(&format!(", events {}", fmt_num(e)));
            }
            ft.push(s(tip));
            if band {
                if let (Some(a), Some(b)) = (fin(&lo[i]), fin(&hi[i])) {
                    bx.push(Value::Float(fin(&x[i]).unwrap_or(0.0)));
                    blo.push(Value::Float(a));
                    bhi.push(Value::Float(b));
                    bk.push(s(k.clone()));
                }
            }
        }
    }
    if fx.is_empty() {
        return crate::note_svg(Some("Kaplan-Meier survival"), "No data", w, h).finish(place);
    }
    let brand = o.brand();
    let mut data: Frame = vec![
        ("x".into(), fx),
        ("y".into(), fy),
        ("censor".into(), fc),
        ("tip".into(), ft),
    ];
    let mut aes = Aes::new().x("x").y("y").label("tip");
    if key.is_some() {
        data.push(("stratum".into(), fk));
        aes = aes.color("stratum");
    }
    let mut plot = GGPlot::new(data).aes(aes);
    if !bx.is_empty() {
        let mut frame: Frame = vec![("x".into(), bx), ("lo".into(), blo), ("hi".into(), bhi)];
        let mut baes = Aes::new().x("x").ymin("lo").ymax("hi");
        if key.is_some() {
            frame.push(("stratum".into(), bk));
            baes = baes.fill("stratum");
        }
        plot = plot
            .geom_stepribbon_with(GeomStepribbon {
                fill: brand,
                alpha: 0.18,
                direction: StepDirection::Hv,
            })
            .layer_data(frame)
            .layer_aes(baes);
    }
    plot = plot
        .geom_step_with(GeomStep {
            color: brand,
            width: 1.4,
            ..Default::default()
        })
        .geom_point_with(GeomPoint {
            size: 1.2,
            color: brand,
            alpha: 0.0,
        })
        .geom_censor_marks_with(
            GeomCensorMarks {
                color: brand,
                size: 4.0,
                ..Default::default()
            },
            "censor",
        );
    if key.is_some() {
        plot = plot
            .scale_color(dz(Aesthetic::Color))
            .scale_fill(dz(Aesthetic::Fill));
    }
    plot = plot
        .scale_y_continuous(ScaleContinuous::new().with_limits(0.0, 1.0))
        .xlab("time")
        .ylab("survival probability")
        .title("Kaplan-Meier survival");
    finish(styled(plot, o, false), w, h, place)
}

/// `curve_type = 'lambda_cv'`: CV error ± 1 SE over λ (log x), with dashed
/// lines at λ.min (`is_min`) and λ.1se (`is_1se`); one panel per model / id.
pub(super) fn lambda_cv(t: &Table, w: u32, h: u32, o: &RenderOptions, place: Place) -> Out {
    t.need(&["x", "y"])?;
    let (x, y) = (t.num("x")?, t.num("y")?);
    let band = t.has("y_low") && t.has("y_high");
    let (lo, hi) = if band {
        (t.num("y_low")?, t.num("y_high")?)
    } else {
        (Vec::new(), Vec::new())
    };
    let flag = |k: &str| -> Result<Vec<bool>, String> {
        Ok(if t.has(k) {
            t.num(k)?
                .iter()
                .map(|v| fin(v).is_some_and(|f| f != 0.0))
                .collect()
        } else {
            vec![false; t.rows.len()]
        })
    };
    let (is_min, is_1se) = (flag("is_min")?, flag("is_1se")?);
    let key = combo(t, &["model_id", "id"]);
    let mut groups: Vec<(Option<String>, Vec<usize>)> = Vec::new();
    for i in 0..t.rows.len() {
        if fin(&x[i]).is_none() || fin(&y[i]).is_none() {
            continue;
        }
        let k = key.as_ref().and_then(|kv| text_of(&kv[i]));
        match groups.iter_mut().find(|(g, _)| *g == k) {
            Some((_, v)) => v.push(i),
            None => groups.push((k, vec![i])),
        }
    }
    if groups.is_empty() {
        return crate::note_svg(Some("Cross-validation error by lambda"), "No data", w, h)
            .finish(place);
    }
    let log = x.iter().filter_map(fin).all(|f| f > 0.0);
    let small = groups.len() > 1;
    let brand = o.brand();
    let mut plots = Vec::new();
    for (g, idx) in &groups {
        let pick = |v: &[Value]| idx.iter().map(|&i| v[i].clone()).collect::<Vec<_>>();
        let tips: Vec<Value> = idx
            .iter()
            .map(|&i| {
                s(format!(
                    "λ = {}: CV error {}",
                    fmt_num(fin(&x[i]).unwrap_or(0.0)),
                    fmt_num(fin(&y[i]).unwrap_or(0.0))
                ))
            })
            .collect();
        let mut plot = GGPlot::new(vec![
            ("x".to_string(), pick(&x)),
            ("y".to_string(), pick(&y)),
            ("tip".to_string(), tips),
        ])
        .aes(Aes::new().x("x").y("y").label("tip"));
        let mut top = idx
            .iter()
            .filter_map(|&i| fin(&y[i]))
            .fold(f64::NEG_INFINITY, f64::max);
        let bottom = idx
            .iter()
            .filter_map(|&i| if band { fin(&lo[i]) } else { fin(&y[i]) })
            .fold(f64::INFINITY, f64::min);
        if band {
            let keep: Vec<usize> = idx
                .iter()
                .copied()
                .filter(|&i| fin(&lo[i]).is_some() && fin(&hi[i]).is_some())
                .collect();
            top = keep.iter().filter_map(|&i| fin(&hi[i])).fold(top, f64::max);
            plot = plot
                .geom_errorbar_with(GeomErrorbar {
                    color: GREY,
                    width: 1.0,
                    cap_width: 0.0,
                    alpha: 0.9,
                })
                .layer_data(vec![
                    ("x".into(), keep.iter().map(|&i| x[i].clone()).collect()),
                    ("lo".into(), keep.iter().map(|&i| lo[i].clone()).collect()),
                    ("hi".into(), keep.iter().map(|&i| hi[i].clone()).collect()),
                ])
                .layer_aes(Aes::new().x("x").ymin("lo").ymax("hi"));
        }
        plot = plot.geom_point_with(GeomPoint {
            size: 2.2,
            color: brand,
            alpha: 1.0,
        });
        // λ.min / λ.1se markers.
        let label_y = top + (top - bottom).abs().max(1e-9) * 0.06;
        // Labels right of their lines, staggered so close lines stay legible.
        let step = (top - bottom).abs().max(1e-9) * 0.09;
        let mut marks: Vec<Mark> = Vec::new();
        if let Some(&i) = idx.iter().find(|&&i| is_min[i]) {
            marks.push((fin(&x[i]).unwrap_or(0.0), "λ.min", ACCENT, label_y));
        }
        if let Some(&i) = idx.iter().find(|&&i| is_1se[i]) {
            marks.push((fin(&x[i]).unwrap_or(0.0), "λ.1se", INK, label_y - step));
        }
        // Labels start just right of their line (in data units of the axis).
        let (xmin, xmax) = idx
            .iter()
            .filter_map(|&i| fin(&x[i]))
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), v| {
                (a.min(v), b.max(v))
            });
        let nudge = |v: f64| {
            if log && xmin > 0.0 {
                v * (xmax / xmin).powf(0.012)
            } else {
                v + (xmax - xmin) * 0.012
            }
        };
        for (xv, label, color, ly) in &marks {
            plot = plot
                .geom_vline_with(GeomVline {
                    xintercept: *xv,
                    color: *color,
                    width: 1.2,
                    linetype: Linetype::Dashed,
                    alpha: 0.9,
                })
                .geom_text_with(GeomText {
                    size: if small { 9.0 } else { 10.0 },
                    color: *color,
                    hjust: 0.0,
                    vjust: 0.0,
                    ..Default::default()
                })
                .layer_data(vec![
                    ("x".into(), vec![Value::Float(nudge(*xv))]),
                    ("y".into(), vec![Value::Float(*ly)]),
                    ("note".into(), vec![s(label.to_string())]),
                ])
                .layer_aes(Aes::new().x("x").y("y").label("note"));
        }
        if log {
            plot = plot.scale_x_continuous(
                ScaleContinuous::new()
                    .with_transform(ggplot_rs::scale::transform::ScaleTransform::Log10),
            );
        }
        plot = plot
            .xlab(if log { "lambda (log scale)" } else { "lambda" })
            .ylab(if band { "CV error ± 1 SE" } else { "CV error" });
        if let Some(g) = g {
            plot = plot.title(g);
        } else {
            plot = plot.title("Cross-validation error by lambda");
        }
        plots.push(styled(plot, o, small));
    }
    if plots.len() == 1 {
        return finish(plots.pop().expect("one plot"), w, h, place);
    }
    let n = plots.len();
    grid(
        plots,
        grid_cols(n, 3),
        Some("Cross-validation error by lambda"),
        w,
        h,
        place,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats() {
        assert_eq!(fmt_p(0.5), "p = 0.50");
        assert_eq!(fmt_p(0.0042), "p = 0.004");
        assert_eq!(fmt_p(1e-9), "p < 1e-4");
        assert_eq!(stars(0.003), "**");
        assert_eq!(stars(0.2), "ns");
        assert_eq!(fmt_num(0.98765), "0.9877");
        assert_eq!(fmt_num(72.3456), "72.35");
        assert_eq!(fmt_num(30.0), "30");
        assert_eq!(fmt_num(1234.56), "1234.6");
    }
}
