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

pub(super) type Frame = Vec<(String, Vec<Value>)>;
/// A significance bracket: x of the left / right group, p, test_id.
type Bracket = (f64, f64, Option<f64>, Option<String>);
/// A λ marker: x, label, colour, label height.
type Mark = (f64, &'static str, (u8, u8, u8), f64);
pub(super) type Out = Result<(String, Vec<String>), String>;

/// "Not significant" / secondary marks.
pub(super) const GREY: (u8, u8, u8) = (0x7a, 0x8b, 0x99);
/// Significant / best / highlighted marks (DataZoo orange).
pub(super) const ACCENT: (u8, u8, u8) = DZ_COLORS[1];
/// Annotation text.
pub(super) const INK: (u8, u8, u8) = (55, 62, 75);

// ── shared kit ──────────────────────────────────────────────────────────────

pub(super) fn fin(v: &Value) -> Option<f64> {
    v.as_f64().filter(|f| f.is_finite())
}

pub(super) fn s(v: impl Into<String>) -> Value {
    Value::Str(v.into())
}

pub(super) fn text_of(v: &Value) -> Option<String> {
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

use super::look::{self, styled, Labels};

pub(super) fn finish(plot: GGPlot, w: u32, h: u32, place: Place) -> Out {
    let (w, h) = (crate::clamp_dim(w as u64), crate::clamp_dim(h as u64));
    crate::Panel::plot(plot, w, h).finish(place)
}

/// Several plots in a patchwork-style grid with one shared legend.
pub(super) fn grid(
    plots: Vec<GGPlot>,
    ncol: usize,
    title: Option<&str>,
    w: u32,
    h: u32,
    place: Place,
) -> Out {
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
pub(super) fn grid_cols(n: usize, max: usize) -> usize {
    match n {
        0..=1 => 1,
        2 => 2,
        3 | 5 | 6 | 9 => 3.min(max),
        _ => 4.min(max),
    }
}

/// The DataZoo discrete scale (sorted levels).
fn dz(aesthetic: Aesthetic) -> ScaleColorDiscrete {
    ScaleColorDiscrete::new(aesthetic).sorted().with_palette(
        DZ_COLORS
            .iter()
            .map(|&(r, g, b)| RGBAColor::new(r, g, b))
            .collect(),
    )
}

/// Distinct non-null text values of `k`, first-seen order.
pub(super) fn levels(t: &Table, k: &str) -> Vec<String> {
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
pub(super) fn label_axis(labels: &[String]) -> ScaleContinuous {
    let n = labels.len();
    ScaleContinuous::new()
        .with_limits(0.4, n as f64 + 0.6)
        .with_expand(0.0, 0.0)
        .with_breaks((1..=n).map(|i| i as f64).collect())
        .with_labels(labels.iter().rev().cloned().collect())
}

/// Row `i` of `n` labels → its y slot (first label on top).
pub(super) fn slot(i: usize, n: usize) -> f64 {
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
    // Else a text column, such as an unnested aggregate's GROUP BY key. Text
    // that is all numbers (pingouin's BF10 "1.2e+04") is a value, not a group.
    let first = t.rows.first()?;
    first
        .keys()
        .filter(|k| !TEST_COLS.contains(&k.as_str()))
        .find(|k| {
            let vals = t.rows.iter().map(|r| r.get(k.as_str()));
            vals.clone()
                .all(|v| matches!(v, Some(J::String(_)) | Some(J::Null) | None))
                && vals
                    .filter_map(|v| v.and_then(J::as_str))
                    .any(|s| s.trim().parse::<f64>().is_err())
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
/// Several methods (ADF and KPSS) are not on one scale: unless the axis is
/// −log10(p), each method gets a panel — its own x, the groups on a shared y
/// axis (#190).
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
            let label = pair_label(text_of(&tid[i]), text_of(&g1[i]), text_of(&g2[i]));
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
    let labels = Labels::from(options);
    let explicit = opt_str(options, "value").is_some();
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
    // Estimates without any interval (pairwise_t_test_by until
    // anofox-statistics#198): bare points say little — draw −log10(p) bars
    // with the significance line instead.
    let no_ci = !rows.iter().any(|r| r.lo.is_some() && r.hi.is_some());
    let fallback = !explicit && mode == "estimate" && no_ci && rows.iter().any(|r| r.p.is_some());
    let mode = if fallback { "p" } else { mode };
    let adjusted = rows.iter().any(|r| r.p.is_some()) && p_adj.iter().any(|v| fin(v).is_some());
    let note_style = opt_str(options, "label").unwrap_or_else(|| "p".into());
    // Odds ratios (Fisher's exact test, …) are compared with 1, not 0.
    let fisher = |m: &str| m.to_ascii_lowercase().contains("fisher");
    let ratio_cols =
        levels(t, "effect_name").iter().any(|e| e.contains("ratio")) || t.has("odds_ratio");
    let ratio = methods.iter().any(|m| fisher(m)) || ratio_cols;
    // Different tests' statistics (and estimates) are not on one scale (#190):
    // several methods → one panel per method, each its own x, the groups on a
    // shared y. −log10(p) is one scale for every test: one panel.
    let by_method = methods.len() > 1 && mode != "p";
    // Rows without a comparison label: one test per group → the group is the
    // label; else the method; several methods per group → both (`A · ADF`).
    let labelled = rows.iter().any(|r| r.label.is_some());
    let mut panels: Vec<(Option<String>, Vec<&TestRow>)> = Vec::new();
    if by_method {
        for r in &rows {
            match panels.iter_mut().find(|(m, _)| *m == r.method) {
                Some((_, v)) => v.push(r),
                None => panels.push((r.method.clone(), vec![r])),
            }
        }
    } else if labelled && group.is_some() {
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
        let method = (methods.len() > 1).then(|| r.method.clone()).flatten();
        r.label
            .clone()
            .or_else(|| match (&r.group, &method) {
                (Some(g), Some(m)) => Some(format!("{g} · {m}")),
                _ => None,
            })
            .or_else(|| r.group.clone())
            .or(method)
            .unwrap_or_else(|| "test".into())
    };
    // In a method's panel the method is the title: a row is its group and
    // comparison.
    let facet_label = |r: &TestRow| -> String {
        match (&r.group, &r.label) {
            (Some(g), Some(l)) => format!("{g} · {l}"),
            (None, Some(l)) => l.clone(),
            (Some(g), None) => g.clone(),
            (None, None) => "test".into(),
        }
    };
    // Each panel's row labels; by method, a label repeated within a panel is
    // numbered ("test (2)") and all panels share one axis of every label.
    let keyed: Vec<Vec<String>> = panels
        .iter()
        .map(|(_, prs)| {
            if !by_method {
                return prs.iter().map(|r| label_of(r)).collect();
            }
            let mut seen: Vec<String> = Vec::new();
            prs.iter()
                .map(|r| {
                    let base = facet_label(r);
                    let k = seen.iter().filter(|s| **s == base).count();
                    seen.push(base.clone());
                    if k == 0 {
                        base
                    } else {
                        format!("{base} ({})", k + 1)
                    }
                })
                .collect()
        })
        .collect();
    let mut shared: Vec<String> = Vec::new();
    for k in keyed.iter().flatten() {
        if !shared.contains(k) {
            shared.push(k.clone());
        }
    }
    // One legend for the grid: every panel lists the call's classes.
    let present: Vec<String> = rows.iter().map(|r| sig_class(r.p, alpha)).collect();
    let level = t
        .num("conf_level")
        .ok()
        .and_then(|v| v.iter().find_map(fin))
        .map(|l| format!("{:.0}% CI", l * 100.0))
        .unwrap_or_else(|| "CI".into());
    let pname = if adjusted { "adjusted p" } else { "p" };
    let xlab = match mode {
        "estimate" if rows.iter().any(|r| r.lo.is_some()) => format!("difference ({level})"),
        "estimate" => "difference".to_string(),
        "statistic" => "test statistic".to_string(),
        _ if fallback => format!("−log10({pname}) — no CI reported"),
        _ => format!("−log10({pname})"),
    };
    let small = panels.len() > 1;
    let mut plots = Vec::new();
    for ((g, prs), keys) in panels.iter().zip(&keyed) {
        let (axis, slots): (Vec<String>, Vec<f64>) = if by_method {
            let n = shared.len();
            let slots = keys
                .iter()
                .map(|k| {
                    slot(
                        shared.iter().position(|s| s == k).expect("key on the axis"),
                        n,
                    )
                })
                .collect();
            (shared.clone(), slots)
        } else {
            let n = keys.len();
            (keys.clone(), (0..n).map(|i| slot(i, n)).collect())
        };
        let panel_ratio = if by_method {
            ratio_cols || g.as_deref().is_some_and(fisher)
        } else {
            ratio
        };
        // The automatic −log10(p) fallback has its own axis: an x_label
        // written for the estimate would mislabel it.
        let xl = if fallback {
            xlab.clone()
        } else {
            labels.x_or(&xlab)
        };
        let mut plot = test_panel(
            prs,
            keys,
            &slots,
            &axis,
            &present,
            mode,
            panel_ratio,
            alpha,
            &note_style,
            o,
            small,
        )?
        .xlab(&xl)
        .ylab(&labels.y_or(""));
        if let Some(g) = g {
            plot = plot.title(g);
        } else if by_method {
            plot = plot.title("no method");
        } else if let Some(tt) = &labels.title {
            plot = plot.title(tt);
        }
        plots.push(plot);
    }
    if plots.len() == 1 {
        let plot = plots.pop().expect("one plot");
        return finish(plot, w, h, place);
    }
    let n = plots.len();
    grid(plots, grid_cols(n, 3), labels.title.as_deref(), w, h, place)
}

/// "B − A" for a test whose id spells the two groups (`B-A`), else
/// "group1 vs group2", else the id.
pub(super) fn pair_label(
    id: Option<String>,
    g1: Option<String>,
    g2: Option<String>,
) -> Option<String> {
    if let (Some(id), Some(a), Some(b)) = (&id, &g1, &g2) {
        for (x, y) in [(a, b), (b, a)] {
            if *id == format!("{x}-{y}") || *id == format!("{x} - {y}") {
                return Some(format!("{x} − {y}"));
            }
        }
    }
    match (g1, g2) {
        (Some(a), Some(b)) if id.is_none() => Some(format!("{a} vs {b}")),
        (Some(a), Some(b)) => match id.as_deref() {
            Some(i) if i.contains('-') && !i.contains(' ') => {
                let parts: Vec<&str> = i.splitn(2, '-').collect();
                Some(format!("{} − {}", parts[0], parts[1]))
            }
            _ => Some(format!("{a} vs {b}")),
        },
        _ => id,
    }
}

/// A test's significance class at `alpha`: its colour and legend entry.
fn sig_class(p: Option<f64>, alpha: f64) -> String {
    match p {
        Some(p) if p <= alpha => format!("p ≤ {}", fmt_num(alpha)),
        Some(_) => "n.s.".into(),
        None => "no p-value".into(),
    }
}

/// The note beside a test: `p = 0.012 *` (`label := 'p'`, forests) or the
/// stars only (`'stars'`).
fn p_note(p: f64, style: &str) -> String {
    match style {
        "stars" => stars(p).to_string(),
        _ => {
            let st = stars(p);
            if st == "ns" {
                fmt_p(p)
            } else {
                format!("{} {st}", fmt_p(p))
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn test_panel(
    rows: &[&TestRow],
    keys: &[String],
    slots: &[f64],
    axis: &[String],
    present: &[String],
    mode: &str,
    ratio: bool,
    alpha: f64,
    note_style: &str,
    o: &RenderOptions,
    small: bool,
) -> Result<GGPlot, String> {
    let n = rows.len();
    let value = |r: &TestRow| -> Option<f64> {
        match mode {
            "estimate" => r.est,
            "statistic" => r.stat,
            _ => r.p.map(|p| -p.max(1e-300).log10()),
        }
    };
    let sig_label = format!("p ≤ {}", fmt_num(alpha));
    let sig = |r: &TestRow| -> String { sig_class(r.p, alpha) };
    let ys: Vec<Value> = slots.iter().map(|&y| Value::Float(y)).collect();
    let xs: Vec<Value> = rows
        .iter()
        .map(|r| value(r).map_or(Value::Float(f64::NAN), Value::Float))
        .collect();
    let sigs: Vec<Value> = rows.iter().map(|r| s(sig(r))).collect();
    let tips: Vec<Value> = rows
        .iter()
        .zip(keys)
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
        if mode == "p" {
            // The significance line is semantic: drawn and labelled.
            plot = plot
                .geom_vline_with(GeomVline {
                    xintercept: r,
                    color: ACCENT,
                    width: 1.1,
                    linetype: Linetype::Dashed,
                    alpha: 0.9,
                })
                .geom_text_with(GeomText {
                    size: 9.5,
                    color: ACCENT,
                    hjust: 0.0,
                    vjust: 0.0,
                    ..Default::default()
                })
                .layer_data(vec![
                    ("x".into(), vec![Value::Float(r + (xhi - xlo) * 0.01)]),
                    ("y".into(), vec![Value::Float(axis.len() as f64 + 0.42)]),
                    ("note".into(), vec![s(format!("p = {}", fmt_num(alpha)))]),
                ])
                .layer_aes(Aes::new().x("x").y("y").label("note"));
        } else {
            plot = look::vline(plot, r);
        }
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
                width: if mode == "p" { 9.0 } else { 2.0 },
                alpha: if mode == "p" { 0.85 } else { 1.0 },
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
    if mode != "p" {
        plot = plot.geom_point_with(GeomPoint {
            size: 3.2,
            color: INK,
            alpha: 1.0,
        });
    }
    // The p-value (and stars) beside each row, right of everything.
    let notes: Vec<Value> = rows
        .iter()
        .map(|r| match r.p {
            Some(p) => s(p_note(p, note_style)),
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
        .scale_y_continuous(label_axis(axis));
    Ok(styled(plot, o, small))
}

pub(super) fn rgba((r, g, b): (u8, u8, u8)) -> RGBAColor {
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
    let labels = Labels::from(options);
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
        // Many groups → many brackets: only the significant ones by default.
        let hide_ns = options
            .get("hide_ns")
            .and_then(J::as_bool)
            .unwrap_or(lv.len() > 3);
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
            // label := 'stars' (default) | 'p' | a ggpubr template ("{p.adj}").
            let style = opt_str(options, "label").unwrap_or_else(|| "stars".into());
            // ggpubr spells the adjusted p `{p.adj}`; ggplot-rs reads columns.
            let user_label = (!matches!(style.as_str(), "stars" | "p"))
                .then(|| style.replace("{p.adj}", "{p_adj}"));
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
                    "p_adj".into(),
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
                        Some(p) => s(p_note(p, &style)),
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
        .xlab(&labels.x_or(&look::humanize(&factor)))
        .ylab(&labels.y_or(&look::humanize(&value)));
    let _ = method;
    if let Some(t) = &labels.title {
        plot = plot.title(t);
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
    let labels = Labels::from(options);
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
        // A binary / few-level term (≤ 3 distinct x, a dummy): jittered
        // points and the group means — a smoother through 2–3 columns of
        // points only draws noise.
        let distinct_x: Vec<f64> = {
            let mut v: Vec<f64> = idx.iter().filter_map(|&i| fin(&xs[i])).collect();
            v.sort_by(f64::total_cmp);
            v.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
            v
        };
        let few = !av && distinct_x.len() <= 3;
        let mut plot = GGPlot::new(data.clone()).aes(aes);
        plot = if few {
            let span = distinct_x
                .last()
                .zip(distinct_x.first())
                .map(|(a, b)| a - b)
                .filter(|d| *d > 0.0)
                .unwrap_or(1.0);
            plot.geom_jitter_with(GeomJitter {
                size: 1.6,
                color: brand,
                alpha: 0.45,
                width: Some(span * 0.07),
                height: Some(0.0),
            })
        } else {
            plot.geom_point_with(GeomPoint {
                size: 1.8,
                color: brand,
                alpha: 0.7,
            })
        };
        if few && series.is_none() {
            let means: Vec<(f64, f64)> = distinct_x
                .iter()
                .filter_map(|&x0| {
                    let v: Vec<f64> = idx
                        .iter()
                        .filter(|&&i| fin(&xs[i]).is_some_and(|x| (x - x0).abs() < 1e-9))
                        .filter_map(|&i| fin(&ys[i]))
                        .collect();
                    (!v.is_empty()).then(|| (x0, v.iter().sum::<f64>() / v.len() as f64))
                })
                .collect();
            plot = plot
                .geom_point_with(GeomPoint {
                    size: 5.0,
                    color: ACCENT,
                    alpha: 1.0,
                })
                .layer_data(vec![
                    (
                        "x".into(),
                        means.iter().map(|m| Value::Float(m.0)).collect(),
                    ),
                    (
                        "y".into(),
                        means.iter().map(|m| Value::Float(m.1)).collect(),
                    ),
                ])
                .layer_aes(Aes::new().x("x").y("y"));
            if distinct_x.len() > 1 {
                plot =
                    plot.scale_x_continuous(ScaleContinuous::new().with_breaks(distinct_x.clone()));
            }
        }
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
            plot = plot
                .xlab(&format!("{term} | others"))
                .ylab(&labels.y_or("y | others"));
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
            if idx.len() >= 6 && series.is_none() && !few {
                plot = plot.geom_smooth_with(GeomSmooth {
                    color: ACCENT,
                    se: false,
                    line_width: 1.4,
                    method: ggplot_rs::stat::smooth::SmoothMethod::Loess { span: 0.75 },
                    ..Default::default()
                });
            }
            plot = plot
                .xlab(&labels.x_or(term))
                .ylab(&labels.y_or("component + residual"));
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
    if plots.len() == 1 {
        // One term: its name (and slope) as the title only when asked.
        let p = plots.pop().expect("one plot");
        let p = match &labels.title {
            Some(t) => p.title(&format!("{t}: {}", titles[0])),
            None => p.title(&titles[0]),
        };
        return finish(p, w, h, place);
    }
    let n = plots.len();
    grid(plots, grid_cols(n, 3), labels.title.as_deref(), w, h, place)
}

// ── summary: one scale per metric ───────────────────────────────────────────

/// Glance metrics in display order; anything else follows alphabetically.
const METRIC_ORDER: &[&str] = &[
    "r_squared",
    "adj_r_squared",
    "pseudo_r_squared",
    "auc",
    "accuracy",
    "f1",
    "precision",
    "recall",
    "brier",
    "ece",
    "log_loss",
    "rmse",
    "mse",
    "mae",
    "mape",
    "smape",
    "mase",
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
    "r2",
    "r2_score",
    "explained_variance",
    "explained_variance_score",
    "auc",
    "roc_auc",
    "roc_auc_score",
    "average_precision",
    "average_precision_score",
    "accuracy",
    "accuracy_score",
    "balanced_accuracy",
    "balanced_accuracy_score",
    "top_k_accuracy",
    "f1",
    "f1_score",
    "precision",
    "precision_score",
    "recall",
    "recall_score",
    "matthews_corrcoef",
    "mcc",
    "cohen_kappa",
    "cohen_kappa_score",
    "jaccard",
    "jaccard_score",
    "log_likelihood",
];
const LOWER_BETTER: &[&str] = &[
    "rmse",
    "mse",
    "mae",
    "mape",
    "smape",
    "mase",
    "rmsse",
    "crps",
    "brier",
    "brier_score",
    "brier_score_loss",
    "ece",
    "expected_calibration_error",
    "log_loss",
    "logloss",
    "hamming_loss",
    "zero_one_loss",
    "hinge_loss",
    "mean_absolute_error",
    "mean_squared_error",
    "root_mean_squared_error",
    "mean_squared_log_error",
    "root_mean_squared_log_error",
    "median_absolute_error",
    "mean_absolute_percentage_error",
    "max_error",
    "mean_pinball_loss",
    "pinball_loss",
    "mean_poisson_deviance",
    "mean_gamma_deviance",
    "mean_tweedie_deviance",
    "residual_std_error",
    "sigma",
    "aic",
    "aicc",
    "bic",
    "deviance",
];

#[cfg(test)]
pub(super) fn direction_of(m: &str) -> f64 {
    direction(m)
}

/// Which way a metric improves: -1 higher is better, 1 lower is better, 0
/// unknown (case ignored: `AUC`, `MAPE`; `-` and spaces read as `_`). A
/// scikit-learn scorer name `neg_<loss>` (`neg_mean_squared_error`) is the
/// negated loss, so higher is better.
fn direction(m: &str) -> f64 {
    let m = m.trim().to_ascii_lowercase().replace(['-', ' '], "_");
    if let Some(loss) = m.strip_prefix("neg_") {
        return -direction(loss);
    }
    if HIGHER_BETTER.contains(&m.as_str()) {
        -1.0
    } else if LOWER_BETTER.contains(&m.as_str()) {
        1.0
    } else {
        0.0
    }
}

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
        "mse" => "MSE".into(),
        "mae" => "MAE".into(),
        "mape" => "MAPE".into(),
        "smape" => "sMAPE".into(),
        "mase" => "MASE".into(),
        "f1" => "F1".into(),
        "r2" => "R²".into(),
        "average_precision" => "average precision".into(),
        "matthews_corrcoef" | "mcc" => "MCC".into(),
        "brier" => "Brier score".into(),
        "ece" => "ECE".into(),
        "log_loss" => "log loss".into(),
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
    let labels_opt = Labels::from(options);
    let baseline = opt_str(options, "baseline").unwrap_or_else(|| "range".into());
    if !matches!(baseline.as_str(), "range" | "zero") {
        return Err(format!(
            "baseline := '{baseline}' is not one of 'range', 'zero'"
        ));
    }
    // One model order for every panel: best first by the first metric (when
    // its direction is known), models without it last.
    let label_levels: Vec<String> = {
        let m0 = &wanted[0];
        let dir = direction(m0);
        let score = |l: &String| -> Option<f64> {
            (0..t.rows.len())
                .find(|&i| {
                    matches!(&metric[i], Value::Str(s) if s == m0)
                        && text_of(&labels[i]).as_ref() == Some(l)
                })
                .and_then(|i| fin(&value[i]))
        };
        let mut l = label_levels;
        if dir != 0.0 {
            l.sort_by(|a, b| match (score(a), score(b)) {
                (Some(x), Some(y)) => (dir * x).total_cmp(&(dir * y)),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            });
        }
        l
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
        let best = if n > 1 && direction(m) < 0.0 {
            vals.iter()
                .copied()
                .fold(None, |b: Option<f64>, v| Some(b.map_or(v, |b| b.max(v))))
        } else if n > 1 && direction(m) > 0.0 {
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
        let spread = vals
            .iter()
            .copied()
            .reduce(f64::max)
            .zip(vals.iter().copied().reduce(f64::min))
            .map(|(a, b)| a - b);
        if baseline == "zero" {
            extent.push(0.0);
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
            .title(&match spread {
                // The range of the panel, so a stretched tiny difference is
                // not over-read.
                Some(d) if baseline == "range" && vals.len() > 1 => {
                    format!("{}  (Δ = {})", metric_title(m), fmt_num(d))
                }
                _ => metric_title(m),
            })
            .xlab("")
            .ylab("");
        // Models without this metric: "n/a".
        let missing: Vec<usize> = (0..n)
            .filter(|&r| !idx.iter().any(|&i| rank_of(i) == r))
            .collect();
        if !missing.is_empty() {
            plot = plot
                .geom_text_with(GeomText {
                    size: 9.0,
                    color: (150, 156, 168),
                    hjust: 0.0,
                    ..Default::default()
                })
                .layer_data(vec![
                    ("x".into(), vec![Value::Float(xlo + pad); missing.len()]),
                    (
                        "y".into(),
                        missing.iter().map(|&r| Value::Float(slot(r, n))).collect(),
                    ),
                    ("note".into(), vec![s("n/a"); missing.len()]),
                ])
                .layer_aes(Aes::new().x("x").y("y").label("note"));
        }
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
        let p = plots.pop().expect("one plot");
        let p = match &labels_opt.title {
            Some(t) => p.title(t),
            None => p,
        };
        return finish(p, w, h, place);
    }
    grid(plots, ncol, labels_opt.title.as_deref(), w, h, place)
}

// ── prediction ──────────────────────────────────────────────────────────────

/// Per-row split: `split`, else from `is_training` (false + y → test, false
/// without y → future), else y → train, no y → future.
fn splits(t: &Table, yv: &[Value]) -> Vec<String> {
    t.rows
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let has_y = yv.get(i).is_some_and(|v| fin(v).is_some());
            match (r.get("split"), r.get("is_training")) {
                (Some(J::String(s)), _) => s.clone(),
                (_, Some(J::Bool(true))) => "train".into(),
                (_, Some(J::Bool(false))) if has_y => "test".into(),
                (_, Some(J::Bool(false))) => "future".into(),
                _ if has_y => "train".into(),
                _ => "future".into(),
            }
        })
        .collect()
}

fn split_label(s: &str) -> String {
    match s.to_ascii_lowercase().as_str() {
        "test" | "holdout" | "hold_out" | "validation" => "hold-out".into(),
        "future" | "forecast" => "forecast".into(),
        other => other.replace('_', " "),
    }
}

/// `prediction`: observed y as neutral grey points, the fit as a coloured
/// line with a light band, split boundaries as labelled vertical lines.
/// Several models in a panel: one band (`band := 'model'`, default `ols` or
/// the first model) and a line per model — or `kind := 'difference'`: each
/// model minus the reference.
pub(super) fn prediction(
    t: &Table,
    options: &Row,
    w: u32,
    h: u32,
    o: &RenderOptions,
    place: Place,
) -> Out {
    let labels = Labels::from(options);
    let x = opt_str(options, "x")
        .or_else(|| {
            ["x", "ds", "date", "period"]
                .into_iter()
                .find(|k| t.has(k))
                .map(String::from)
        })
        .ok_or_else(|| {
            format!(
                "needs an x position: a column `x`, `ds` or `period`, or x := '<column>' (got: {})",
                t.keys()
            )
        })?;
    let y = opt_str(options, "y").unwrap_or_else(|| "y".into());
    t.need(&[x.as_str(), "yhat"])?;
    let facet = opt_str(options, "facet").or_else(|| (t.distinct("id") > 1).then(|| "id".into()));
    if let Some(f) = &facet {
        if !t.has(f) {
            return Err(format!("facet column `{f}` not found (got: {})", t.keys()));
        }
    }
    let n = t.rows.len();
    let xs = t.xpos(&x);
    let yhat = t.num("yhat")?;
    let has_y = t.has(&y);
    let yv = if has_y {
        t.num(&y)?
    } else {
        vec![Value::Na; n]
    };
    let fac = facet.as_deref().map(|f| t.text(f));
    let fkey = |i: usize| {
        fac.as_ref()
            .map(|f| format!("{:?}", f[i]))
            .unwrap_or_default()
    };
    // Models per panel.
    let model_key = t.model_key().filter(|m| {
        let mut per: HashMap<String, std::collections::HashSet<String>> = HashMap::new();
        for (i, r) in t.rows.iter().enumerate() {
            if let Some(v) = r.get(*m).filter(|v| !v.is_null()) {
                per.entry(fkey(i)).or_default().insert(v.to_string());
            }
        }
        per.values().any(|s| s.len() > 1)
    });
    let models: Vec<Value> = match model_key {
        Some(m) => t.text(m),
        None => vec![s(""); n],
    };
    let mlevels: Vec<String> = match model_key {
        Some(m) => levels(t, m),
        None => Vec::new(),
    };
    let several = mlevels.len() > 1;
    let reference: Option<String> = several.then(|| {
        opt_str(options, "band")
            .filter(|b| mlevels.contains(b))
            .or_else(|| {
                mlevels
                    .iter()
                    .find(|m| m.eq_ignore_ascii_case("ols"))
                    .cloned()
            })
            .unwrap_or_else(|| mlevels[0].clone())
    });
    let kind = opt_str(options, "kind").unwrap_or_else(|| "fit".into());
    if !matches!(kind.as_str(), "fit" | "difference") {
        return Err(format!(
            "kind := '{kind}' is not one of 'fit', 'difference'"
        ));
    }
    let difference = kind == "difference" && several;
    let effects = t.has("focal");
    // Axis labels: effects tables name the response by the model family.
    let family = mlevels
        .first()
        .cloned()
        .or_else(|| t.model_key().and_then(|k| levels(t, k).into_iter().next()))
        .unwrap_or_default()
        .to_ascii_lowercase();
    let ydefault = if effects {
        if ["logistic", "binomial", "probit", "logit"]
            .iter()
            .any(|f| family.contains(f))
        {
            "P(y = 1)".to_string()
        } else if ["poisson", "negbinom", "negative"]
            .iter()
            .any(|f| family.contains(f))
        {
            "expected count".to_string()
        } else {
            format!("predicted {}", look::humanize(&y))
        }
    } else {
        look::humanize(if has_y { &y } else { "yhat" })
    };
    let is_date = xs.iter().any(|v| matches!(v, Value::DateTime(_)));
    let xdefault = if is_date && matches!(x.as_str(), "x" | "ds") {
        "date".to_string()
    } else if effects {
        levels(t, "focal_name")
            .into_iter()
            .next()
            .map(|f| look::humanize(&f))
            .unwrap_or_else(|| look::humanize(&x))
    } else {
        look::humanize(&x)
    };
    let nan = |v: &Value| fin(v).map_or(Value::Float(f64::NAN), Value::Float);
    let brand = o.brand();
    let mut captions: Vec<String> = Vec::new();
    let ycol: Vec<Value>;
    let ylab: String;
    let mut plot;
    if difference {
        let rf = reference.clone().unwrap_or_default();
        let mut base: HashMap<String, f64> = HashMap::new();
        for i in 0..n {
            if text_of(&models[i]).as_deref() == Some(rf.as_str()) {
                if let Some(v) = fin(&yhat[i]) {
                    base.insert(format!("{}|{:?}", fkey(i), xs[i]), v);
                }
            }
        }
        let keep: Vec<usize> = (0..n)
            .filter(|&i| text_of(&models[i]).as_deref() != Some(rf.as_str()))
            .filter(|&i| fin(&yhat[i]).is_some())
            .filter(|&i| base.contains_key(&format!("{}|{:?}", fkey(i), xs[i])))
            .collect();
        ycol = keep
            .iter()
            .map(|&i| {
                Value::Float(
                    fin(&yhat[i]).unwrap_or(0.0) - base[&format!("{}|{:?}", fkey(i), xs[i])],
                )
            })
            .collect();
        let others: Vec<String> = mlevels.iter().filter(|m| **m != rf).cloned().collect();
        let mut data: Frame = vec![
            ("x".into(), keep.iter().map(|&i| xs[i].clone()).collect()),
            ("yhat".into(), ycol.clone()),
            (
                "model".into(),
                keep.iter().map(|&i| models[i].clone()).collect(),
            ),
        ];
        if let Some(f) = &fac {
            data.push(("facet".into(), keep.iter().map(|&i| f[i].clone()).collect()));
        }
        plot = GGPlot::new(data).aes(Aes::new().x("x").y("yhat").color("model").group("model"));
        plot = look::hline(plot, 0.0).geom_line_with(GeomLine {
            color: brand,
            width: 1.6,
            alpha: 1.0,
        });
        plot = plot.scale_color(look::series_scale(Aesthetic::Color, &others));
        ylab = format!("difference vs {rf}");
    } else {
        ycol = yhat.iter().map(nan).collect();
        // The fit line: rows without a yhat (the history unioned in before a
        // forecast) are part of the schema, not lost data — leave them out of
        // the line layer instead of letting it report them as non-finite.
        // One point per (model, panel, x): a long table repeats yhat for
        // each interval level.
        let mut seen = std::collections::HashSet::new();
        let fitted: Vec<usize> = (0..n)
            .filter(|&i| fin(&yhat[i]).is_some())
            .filter(|&i| seen.insert(format!("{:?}|{}|{:?}", models[i], fkey(i), xs[i])))
            .collect();
        let fitted: Vec<usize> = if fitted.is_empty() {
            (0..n).collect()
        } else {
            fitted
        };
        let pick = |v: &[Value]| fitted.iter().map(|&i| v[i].clone()).collect::<Vec<_>>();
        let mut data: Frame = vec![
            ("x".into(), pick(&xs)),
            ("yhat".into(), pick(&ycol)),
            ("model".into(), pick(&models)),
        ];
        if let Some(f) = &fac {
            data.push(("facet".into(), pick(f)));
        }
        let mut aes = Aes::new().x("x").y("yhat");
        if several {
            aes = aes.color("model").group("model");
        }
        plot = GGPlot::new(data).aes(aes);
        if t.has("yhat_lower") && t.has("yhat_upper") {
            let (lo, hi) = (t.num("yhat_lower")?, t.num("yhat_upper")?);
            // Several `level`s: a nested fan, one ribbon per level (the
            // overlaps darken towards the centre).
            let lv = if t.has("level") {
                t.num("level")?
            } else {
                vec![Value::Na; n]
            };
            let mut fan: Vec<f64> = lv.iter().filter_map(fin).collect();
            fan.sort_by(f64::total_cmp);
            fan.dedup();
            let keep: Vec<usize> = (0..n)
                .filter(|&i| fin(&lo[i]).is_some() && fin(&hi[i]).is_some())
                .filter(|&i| match &reference {
                    Some(r) => text_of(&models[i]).as_deref() == Some(r.as_str()),
                    None => true,
                })
                .collect();
            let pick = |v: &[Value]| keep.iter().map(|&i| v[i].clone()).collect::<Vec<_>>();
            let mut frame: Frame = vec![
                ("x".into(), pick(&xs)),
                ("lo".into(), pick(&lo)),
                ("hi".into(), pick(&hi)),
            ];
            if let Some(f) = &fac {
                frame.push(("facet".into(), pick(f)));
            }
            // The band in its model's line colour.
            let fill = match &reference {
                Some(r) => mlevels
                    .iter()
                    .position(|m| m == r)
                    .map(|i| look::level_colors(&mlevels)[i])
                    .unwrap_or_else(|| look::model_color(r)),
                None => brand,
            };
            // One ribbon per facet / split run / level.
            let mut a = Aes::new().x("x").ymin("lo").ymax("hi");
            if fan.len() > 1 {
                frame.push((
                    "band".into(),
                    keep.iter()
                        .map(|&i| s(format!("{}|{:?}", fkey(i), lv[i])))
                        .collect(),
                ));
                a = a.group("band");
                let pct: Vec<String> = fan
                    .iter()
                    .map(|l| format!("{} %", fmt_num(l * 100.0)))
                    .collect();
                let whose = reference
                    .as_ref()
                    .map(|r| format!(" ({r})"))
                    .unwrap_or_default();
                captions.push(format!("bands{whose}: {} intervals", pct.join(", ")));
            } else if fac.is_some() {
                a = a.group("facet");
            }
            let alpha = if fan.len() > 1 { 0.12 } else { 0.16 };
            plot = plot
                .geom_ribbon_with(GeomRibbon { fill, alpha })
                .layer_data(frame)
                .layer_aes(a);
        }
        if has_y {
            // Observed values once per (facet, x): neutral dark grey.
            let mut seen = std::collections::HashSet::new();
            let keep: Vec<usize> = (0..n)
                .filter(|&i| fin(&yv[i]).is_some())
                .filter(|&i| seen.insert(format!("{}|{:?}|{:?}", fkey(i), xs[i], yv[i])))
                .collect();
            let pick = |v: &[Value]| keep.iter().map(|&i| v[i].clone()).collect::<Vec<_>>();
            let mut frame: Frame = vec![("x".into(), pick(&xs)), ("yhat".into(), pick(&yv))];
            if let Some(f) = &fac {
                frame.push(("facet".into(), pick(f)));
            }
            plot = plot
                .geom_point_with(GeomPoint {
                    size: 1.9,
                    color: (88, 94, 106),
                    alpha: 0.6,
                })
                .layer_data(frame)
                .layer_aes(Aes::new().x("x").y("yhat"));
        }
        plot = plot.geom_line_with(GeomLine {
            color: brand,
            width: 1.7,
            alpha: 1.0,
        });
        if several {
            plot = plot.scale_color(look::series_scale(Aesthetic::Color, &mlevels));
        }
        ylab = ydefault.clone();
    }
    // Split boundaries: labelled vertical lines where the split changes
    // along x (the first panel's rows).
    if !effects {
        let sp = splits(t, &yv);
        let mut order: Vec<usize> = (0..n)
            .filter(|&i| fin(&xs[i]).is_some())
            .filter(|&i| fac.as_ref().is_none_or(|f| f[i] == f[0]))
            .collect();
        order.sort_by(|&a, &b| {
            fin(&xs[a])
                .unwrap_or(0.0)
                .total_cmp(&fin(&xs[b]).unwrap_or(0.0))
        });
        let mut marks: Vec<(f64, String)> = Vec::new();
        let mut last: Option<(&str, f64)> = None;
        for &i in &order {
            let xv = fin(&xs[i]).unwrap_or(0.0);
            if let Some((ls, lx)) = last {
                if ls != sp[i] && !marks.iter().any(|m| m.1 == split_label(&sp[i])) {
                    marks.push(((lx + xv) / 2.0, split_label(&sp[i])));
                }
            }
            last = Some((sp[i].as_str(), xv));
        }
        // Splits interleaved along x (x is not time, e.g. observed vs
        // predicted): no boundaries — the non-train points are marked instead.
        let changes = order.windows(2).filter(|w| sp[w[0]] != sp[w[1]]).count();
        let mut kinds: Vec<&str> = order.iter().map(|&i| sp[i].as_str()).collect();
        kinds.sort_unstable();
        kinds.dedup();
        if changes + 1 > kinds.len() {
            marks.clear();
            let other: Vec<usize> = (0..n)
                .filter(|&i| sp[i] != "train" && fin(&yv[i]).is_some())
                .collect();
            if has_y && !other.is_empty() && other.len() < n {
                let names: Vec<String> = {
                    let mut v: Vec<String> = other.iter().map(|&i| split_label(&sp[i])).collect();
                    v.sort();
                    v.dedup();
                    v
                };
                let mut frame: Frame = vec![
                    ("x".into(), other.iter().map(|&i| xs[i].clone()).collect()),
                    (
                        "yhat".into(),
                        other.iter().map(|&i| yv[i].clone()).collect(),
                    ),
                ];
                if let Some(f) = &fac {
                    frame.push((
                        "facet".into(),
                        other.iter().map(|&i| f[i].clone()).collect(),
                    ));
                }
                plot = plot
                    .geom_point_with(GeomPoint {
                        size: 2.2,
                        color: ACCENT,
                        alpha: 0.8,
                    })
                    .layer_data(frame)
                    .layer_aes(Aes::new().x("x").y("yhat"));
                captions.push(format!("orange: {} rows", names.join(", ")));
            }
        }
        if !marks.is_empty() {
            let top = ycol
                .iter()
                .chain(yv.iter())
                .filter_map(fin)
                .fold(f64::NEG_INFINITY, f64::max);
            for (xv, label) in &marks {
                plot = look::vline(plot, *xv);
                if top.is_finite() {
                    plot = plot
                        .geom_text_with(GeomText {
                            size: 10.0,
                            color: (110, 116, 128),
                            hjust: 0.0,
                            vjust: 1.0,
                            ..Default::default()
                        })
                        .layer_data(vec![
                            ("x".into(), vec![Value::Float(*xv)]),
                            ("yhat".into(), vec![Value::Float(top)]),
                            ("note".into(), vec![s(format!(" {label}"))]),
                        ])
                        .layer_aes(Aes::new().x("x").y("yhat").label("note"));
                }
            }
        }
    }
    if let Some(ev) = options.get("events").filter(|v| !v.is_null()) {
        let (p, note) = super::events::overlay(
            plot,
            ev,
            &x,
            is_date,
            facet.as_deref().zip(fac.as_deref()),
            o,
        )?;
        plot = p;
        captions.extend(note);
    }
    let mut plot = labels.apply(plot, &xdefault, &ylab);
    if !captions.is_empty() {
        plot = plot.caption(&captions.join(" · "));
    }
    if fac.is_some() {
        plot = plot.facet_wrap_free("facet", Some(1), FacetScales::FreeY);
    }
    finish(styled(plot, o, false), w, h, place)
}

// ── curves: Kaplan–Meier, lambda CV ─────────────────────────────────────────

/// The series key of a curve table: the first of `series` / `model_id` /
/// `id` that varies.
pub(super) fn curve_series(t: &Table) -> Option<&'static str> {
    ["series", "model_id", "id"]
        .into_iter()
        .find(|k| t.has(k) && t.distinct(k) > 1)
}

/// `curve_type = 'km'`: survival step curves per stratum from (0, 1), the
/// confidence step ribbon where the producer has one (the undefined
/// Greenwood band of the last step is left out, the line is kept), `+`
/// censoring marks from `n_censor`, and n_risk / n_event in the hover text.
pub(super) fn km(t: &Table, options: &Row, w: u32, h: u32, o: &RenderOptions, place: Place) -> Out {
    let labels = Labels::from(options);
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
        return crate::note_svg(None, "No data", w, h).finish(place);
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
                size: 2.6,
                alpha: 0.75,
                ..Default::default()
            },
            "censor",
        );
    let strata: Vec<String> = by.keys().cloned().collect();
    let colors = look::level_colors(&strata);
    let xmax = x.iter().filter_map(fin).fold(0.0, f64::max);
    // The median: a dashed 50 % line, drops at each stratum's median.
    plot = look::hline(plot, 0.5);
    for (si, (_, rows)) in by.iter().enumerate() {
        let med = rows
            .iter()
            .find(|&&i| fin(&y[i]).is_some_and(|v| v <= 0.5))
            .and_then(|&i| fin(&x[i]));
        if let Some(m) = med {
            plot = plot
                .geom_segment_with(GeomSegment {
                    color: if key.is_some() { colors[si] } else { brand },
                    width: 1.0,
                    alpha: 0.7,
                })
                .layer_data(vec![
                    ("x".into(), vec![Value::Float(m)]),
                    ("xend".into(), vec![Value::Float(m)]),
                    ("y".into(), vec![Value::Float(0.5)]),
                    ("yend".into(), vec![Value::Float(0.0)]),
                ])
                .layer_aes(Aes::new().x("x").xend("xend").y("y").yend("yend"));
        }
    }
    // Number at risk under the curves at regular time ticks.
    let ticks = nice_ticks(0.0, xmax, 6);
    let has_risk = risk.iter().any(|v| fin(v).is_some());
    let k = by.len();
    let row_h = 0.075;
    if has_risk && !ticks.is_empty() {
        let (mut tx, mut ty, mut tl, mut tk) = (vec![], vec![], vec![], vec![]);
        for (si, (name, rows)) in by.iter().enumerate() {
            for &tk0 in &ticks {
                // At risk at t: n_risk of the first time ≥ t; past the last
                // time, what is left after it.
                let n = match rows.iter().find(|&&i| fin(&x[i]).is_some_and(|v| v >= tk0)) {
                    Some(&i) => fin(&risk[i]).unwrap_or(0.0),
                    None => rows
                        .last()
                        .map(|&i| {
                            fin(&risk[i]).unwrap_or(0.0)
                                - fin(&events[i]).unwrap_or(0.0)
                                - fin(&cens[i]).unwrap_or(0.0)
                        })
                        .unwrap_or(0.0)
                        .max(0.0),
                };
                tx.push(Value::Float(tk0));
                ty.push(Value::Float(-row_h * (si as f64 + 1.2)));
                tl.push(s(fmt_num(n.round())));
                tk.push(s(name.clone()));
            }
        }
        // One text layer per stratum, in its colour.
        for (si, name) in strata.iter().enumerate() {
            let pick: Vec<usize> = (0..tk.len())
                .filter(|&i| text_of(&tk[i]).as_deref() == Some(name.as_str()))
                .collect();
            let sel = |v: &[Value]| pick.iter().map(|&i| v[i].clone()).collect::<Vec<_>>();
            plot = plot
                .geom_text_with(GeomText {
                    size: 9.5,
                    color: if key.is_some() { colors[si] } else { INK },
                    hjust: 0.5,
                    vjust: 0.5,
                    ..Default::default()
                })
                .layer_data(vec![
                    ("x".into(), sel(&tx)),
                    ("y".into(), sel(&ty)),
                    ("note".into(), sel(&tl)),
                ])
                .layer_aes(Aes::new().x("x").y("y").label("note"));
        }
        plot = plot
            .geom_text_with(GeomText {
                size: 9.0,
                color: (120, 126, 138),
                hjust: 0.0,
                vjust: 0.5,
                ..Default::default()
            })
            .layer_data(vec![
                ("x".into(), vec![Value::Float(0.0)]),
                ("y".into(), vec![Value::Float(-row_h * 0.35)]),
                ("note".into(), vec![s("number at risk")]),
            ])
            .layer_aes(Aes::new().x("x").y("y").label("note"));
    }
    if key.is_some() {
        plot = plot
            .scale_color(look::series_scale(Aesthetic::Color, &strata))
            .scale_fill(look::series_scale(Aesthetic::Fill, &strata));
    }
    let ylo = if has_risk {
        -row_h * (k as f64 + 0.8)
    } else {
        0.0
    };
    plot = plot
        .scale_y_continuous(
            ScaleContinuous::new()
                .with_limits(ylo, 1.0)
                .with_breaks(vec![0.0, 0.25, 0.5, 0.75, 1.0]),
        )
        .scale_x_continuous(ScaleContinuous::new().with_breaks(ticks.clone()));
    let plot = labels.apply(plot, "time", "survival probability");
    finish(styled(plot, o, false), w, h, place)
}

/// `curve_type = 'lambda_cv'`: CV error ± 1 SE over λ (log x), with dashed
/// lines at λ.min (`is_min`) and λ.1se (`is_1se`); one panel per model / id.
pub(super) fn lambda_cv(
    t: &Table,
    options: &Row,
    w: u32,
    h: u32,
    o: &RenderOptions,
    place: Place,
) -> Out {
    let labels = Labels::from(options);
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
        return crate::note_svg(None, "No data", w, h).finish(place);
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
            .xlab(&labels.x_or(if log { "λ (log scale)" } else { "λ" }))
            .ylab(&labels.y_or(if band { "CV error ± 1 SE" } else { "CV error" }));
        if let Some(g) = g {
            plot = plot.title(g);
        } else if let Some(tt) = &labels.title {
            plot = plot.title(tt);
        }
        plots.push(styled(plot, o, small));
    }
    if plots.len() == 1 {
        return finish(plots.pop().expect("one plot"), w, h, place);
    }
    let n = plots.len();
    grid(plots, grid_cols(n, 3), labels.title.as_deref(), w, h, place)
}

/// About `n` round tick positions covering [lo, hi].
pub(super) fn nice_ticks(lo: f64, hi: f64, n: usize) -> Vec<f64> {
    if hi.partial_cmp(&lo) != Some(std::cmp::Ordering::Greater) || n == 0 {
        return Vec::new();
    }
    let raw = (hi - lo) / n as f64;
    let mag = 10f64.powf(raw.log10().floor());
    let step = [1.0, 2.0, 2.5, 5.0, 10.0]
        .iter()
        .map(|m| m * mag)
        .find(|st| *st >= raw)
        .unwrap_or(10.0 * mag);
    let mut v = Vec::new();
    let mut t = (lo / step).ceil() * step;
    while t <= hi + step * 1e-9 {
        v.push(if t.abs() < step * 1e-9 { 0.0 } else { t });
        t += step;
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ticks_are_round() {
        assert_eq!(
            nice_ticks(0.0, 60.0, 6),
            vec![0.0, 10.0, 20.0, 30.0, 40.0, 50.0, 60.0]
        );
        assert!(nice_ticks(1.0, 1.0, 5).is_empty());
    }

    #[test]
    fn pair_labels() {
        let l = |i: &str, a: &str, b: &str| {
            pair_label(Some(i.into()), Some(a.into()), Some(b.into())).unwrap()
        };
        assert_eq!(l("B-A", "A", "B"), "B − A");
        assert_eq!(l("low-high", "high", "low"), "low − high");
        assert_eq!(
            pair_label(None, Some("A".into()), Some("B".into())).unwrap(),
            "A vs B"
        );
    }

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
