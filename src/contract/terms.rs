//! `terms` plots drawn directly with ggplot-rs: the coefficient forest and
//! the index paths (`index_value`): the regularisation path (λ, one panel,
//! all coefficients overlaid with direct labels and λ.min / λ.1se), the LARS
//! path (on the |β|₁ / max|β|₁ fraction), the quantile process and rolling
//! coefficients (one panel per term, with a dashed full-sample reference).

use super::look::{self, Labels};
use super::stats::{
    fin, finish, fmt_num, grid, grid_cols, label_axis, s, slot, text_of, Frame, Out, ACCENT, INK,
};
use super::{Row, Table};
use crate::{Place, RenderOptions};
use ggplot_rs::prelude::*;
use serde_json::Value as J;

const INTERCEPT: &[&str] = &[
    "(Intercept)",
    "intercept",
    "Intercept",
    "const",
    "(intercept)",
];

fn is_intercept(t: &str) -> bool {
    INTERCEPT.contains(&t)
}

/// One coefficient row.
#[derive(Clone, Debug)]
struct Coef {
    term: String,
    series: Option<String>,
    panel: Option<String>,
    est: Option<f64>,
    lo: Option<f64>,
    hi: Option<f64>,
    /// The index position (numeric or a datetime as epoch seconds).
    x: Option<Value>,
}

fn first_seen<'a>(it: impl Iterator<Item = &'a str>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for v in it {
        if !out.iter().any(|o| o == v) {
            out.push(v.to_string());
        }
    }
    out
}

fn conf_label(t: &Table) -> String {
    t.num("conf_level")
        .ok()
        .and_then(|v| v.iter().find_map(fin))
        .map(|l| format!("{}% CI", fmt_num(l * 100.0)))
        .unwrap_or_else(|| "CI".into())
}

/// Entry point: a forest, or a path when `index_value` is present.
pub(super) fn terms(
    t: &Table,
    options: &Row,
    w: u32,
    h: u32,
    o: &RenderOptions,
    place: Place,
) -> Out {
    t.need(&["term", "estimate"])?;
    if t.has("index_value") || t.has("window_end") {
        path(t, options, w, h, o, place)
    } else {
        forest(t, options, w, h, o, place)
    }
}

// ── forest ──────────────────────────────────────────────────────────────────

fn forest(t: &Table, options: &Row, w: u32, h: u32, o: &RenderOptions, place: Place) -> Out {
    let labels = Labels::from(options);
    let model_key = t.model_key().filter(|k| t.distinct(k) > 1);
    let ids = t.has("id") && t.distinct("id") > 1;
    // Several models and several groups: a panel per group.
    let (series_key, panel_key) = match (model_key, ids) {
        (Some(m), true) => (Some(m), Some("id")),
        (Some(m), false) => (Some(m), None),
        (None, true) => (Some("id"), None),
        (None, false) => (None, None),
    };
    let (term, est) = (t.text("term"), t.num("estimate")?);
    let ci = t.has("conf_low") && t.has("conf_high");
    let (lo, hi) = if ci {
        (t.num("conf_low")?, t.num("conf_high")?)
    } else {
        (vec![Value::Na; t.rows.len()], vec![Value::Na; t.rows.len()])
    };
    let sv = series_key.map(|k| t.text(k));
    let pv = panel_key.map(|k| t.text(k));
    let rows: Vec<Coef> = (0..t.rows.len())
        .filter_map(|i| {
            Some(Coef {
                term: text_of(&term[i])?,
                series: sv.as_ref().and_then(|v| text_of(&v[i])),
                panel: pv.as_ref().and_then(|v| text_of(&v[i])),
                est: fin(&est[i]),
                lo: fin(&lo[i]),
                hi: fin(&hi[i]),
                x: None,
            })
        })
        .collect();
    let terms = first_seen(rows.iter().map(|r| r.term.as_str()));
    let series: Vec<String> = first_seen(rows.iter().filter_map(|r| r.series.as_deref()));
    let panels: Vec<Option<String>> = if panel_key.is_some() {
        first_seen(rows.iter().filter_map(|r| r.panel.as_deref()))
            .into_iter()
            .map(Some)
            .collect()
    } else {
        vec![None]
    };
    let any_ci = rows.iter().any(|r| r.lo.is_some() && r.hi.is_some());
    let missing_ci = any_ci && rows.iter().any(|r| r.lo.is_none() || r.hi.is_none());
    let xlab = if any_ci {
        format!("estimate ({})", conf_label(t))
    } else {
        "estimate".to_string()
    };
    let small = panels.len() > 1;
    let mut plots = Vec::new();
    for p in &panels {
        let prs: Vec<&Coef> = rows.iter().filter(|r| r.panel == *p).collect();
        let mut plot = forest_panel(&prs, &terms, &series, o, small, missing_ci);
        plot = labels.apply(plot, &xlab, "");
        if let Some(p) = p {
            plot = plot.title(p);
        }
        if missing_ci && p == panels.last().unwrap_or(&None) {
            plot = plot.caption("○ = no interval reported");
        }
        plots.push(plot);
    }
    if plots.len() == 1 {
        return finish(plots.pop().expect("one"), w, h, place);
    }
    let n = plots.len();
    grid(plots, grid_cols(n, 3), labels.title.as_deref(), w, h, place)
}

fn forest_panel(
    rows: &[&Coef],
    terms: &[String],
    series: &[String],
    o: &RenderOptions,
    small: bool,
    missing_ci: bool,
) -> GGPlot {
    let n = terms.len();
    let k = series.len().max(1);
    let dodge = 0.7;
    // The first series on top of each term's slot — the legend order.
    let ypos = |r: &Coef| -> Option<f64> {
        let ti = terms.iter().position(|x| *x == r.term)?;
        let j = r
            .series
            .as_ref()
            .and_then(|s| series.iter().position(|x| x == s))
            .unwrap_or(0);
        let off = if k > 1 {
            dodge / 2.0 - (j as f64 + 0.5) * dodge / k as f64
        } else {
            0.0
        };
        Some(slot(ti, n) + off)
    };
    let col = |r: &Coef| s(r.series.clone().unwrap_or_else(|| "estimate".into()));
    let keep: Vec<&&Coef> = rows.iter().filter(|r| r.est.is_some()).collect();
    let data: Frame = vec![
        (
            "x".into(),
            keep.iter()
                .map(|r| Value::Float(r.est.unwrap_or(0.0)))
                .collect(),
        ),
        (
            "y".into(),
            keep.iter()
                .map(|r| ypos(r).map_or(Value::Na, Value::Float))
                .collect(),
        ),
        ("model".into(), keep.iter().map(|r| col(r)).collect()),
        (
            "tip".into(),
            keep.iter()
                .map(|r| {
                    let ci = match (r.lo, r.hi) {
                        (Some(a), Some(b)) => format!(" [{}, {}]", fmt_num(a), fmt_num(b)),
                        _ => " (no CI)".into(),
                    };
                    s(format!(
                        "{}{}: {}{ci}",
                        r.term,
                        r.series
                            .as_ref()
                            .map(|s| format!(" · {s}"))
                            .unwrap_or_default(),
                        fmt_num(r.est.unwrap_or(0.0))
                    ))
                })
                .collect(),
        ),
    ];
    let several = series.len() > 1;
    let mut aes = Aes::new().x("x").y("y").label("tip");
    if several {
        aes = aes.color("model");
    }
    let mut plot = GGPlot::new(data).aes(aes);
    plot = look::vline(plot, 0.0);
    let with_ci: Vec<&&Coef> = keep
        .iter()
        .copied()
        .filter(|r| r.lo.is_some() && r.hi.is_some())
        .collect();
    if !with_ci.is_empty() {
        let mut frame: Frame = vec![
            (
                "y".into(),
                with_ci
                    .iter()
                    .map(|r| ypos(r).map_or(Value::Na, Value::Float))
                    .collect(),
            ),
            (
                "lo".into(),
                with_ci
                    .iter()
                    .map(|r| Value::Float(r.lo.unwrap_or(0.0)))
                    .collect(),
            ),
            (
                "hi".into(),
                with_ci
                    .iter()
                    .map(|r| Value::Float(r.hi.unwrap_or(0.0)))
                    .collect(),
            ),
        ];
        let mut a = Aes::new().y("y").xmin("lo").xmax("hi");
        if several {
            frame.push(("model".into(), with_ci.iter().map(|r| col(r)).collect()));
            a = a.color("model");
        }
        plot = plot
            .geom_errorbarh_with(GeomErrorbarh {
                color: o.brand(),
                width: 1.5,
                cap_height: 0.0,
                alpha: 1.0,
            })
            .layer_data(frame)
            .layer_aes(a);
    }
    plot = plot.geom_point_with(GeomPoint {
        size: 3.2,
        color: o.brand(),
        alpha: 1.0,
    });
    // Rows without an interval: a hollow ring (a white dot over the point).
    if missing_ci {
        let bare: Vec<&&Coef> = keep
            .iter()
            .copied()
            .filter(|r| r.lo.is_none() || r.hi.is_none())
            .collect();
        if !bare.is_empty() {
            plot = plot
                .geom_point_with(GeomPoint {
                    size: 1.7,
                    color: (255, 255, 255),
                    alpha: 1.0,
                })
                .layer_data(vec![
                    (
                        "x".into(),
                        bare.iter()
                            .map(|r| Value::Float(r.est.unwrap_or(0.0)))
                            .collect(),
                    ),
                    (
                        "y".into(),
                        bare.iter()
                            .map(|r| ypos(r).map_or(Value::Na, Value::Float))
                            .collect(),
                    ),
                ])
                .layer_aes(Aes::new().x("x").y("y"));
        }
    }
    if several {
        plot = plot.scale_color(look::series_scale(Aesthetic::Color, series));
    }
    plot = plot.scale_y_continuous(label_axis(terms));
    look::styled(plot, o, small)
}

// ── paths ───────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum PathKind {
    Lambda,
    Lars,
    Tau,
    Rolling,
    Other,
}

fn path_kind(t: &Table) -> (PathKind, String) {
    let name = super::stats::levels(t, "index_name")
        .into_iter()
        .next()
        .unwrap_or_default();
    let lname = name.to_ascii_lowercase();
    let models: Vec<String> = t
        .model_key()
        .map(|k| super::stats::levels(t, k))
        .unwrap_or_default();
    let kind = if t.has("window_end") || lname == "window_end" {
        PathKind::Rolling
    } else if lname.contains("lambda") {
        PathKind::Lambda
    } else if lname == "alpha" && models.iter().any(|m| m.eq_ignore_ascii_case("lars"))
        || models.iter().any(|m| m.eq_ignore_ascii_case("lars"))
    {
        PathKind::Lars
    } else if lname == "tau" || lname == "quantile" {
        PathKind::Tau
    } else {
        PathKind::Other
    };
    (kind, name)
}

/// A number option, or a number in the first non-null value of column `k`
/// (a flag column `is_min`, or a value column `lambda_min`).
fn mark_value(t: &Table, options: &Row, opt: &str, value_col: &str, flag_col: &str) -> Option<f64> {
    if let Some(v) = options.get(opt).and_then(|v| match v {
        J::Number(n) => n.as_f64(),
        J::String(s) => s.trim().parse().ok(),
        _ => None,
    }) {
        return Some(v).filter(|v: &f64| v.is_finite());
    }
    if t.has(value_col) {
        return t.num(value_col).ok()?.iter().find_map(fin);
    }
    if t.has(flag_col) {
        let f = t.num(flag_col).ok()?;
        let x = t.num("index_value").ok()?;
        return (0..f.len())
            .find(|&i| fin(&f[i]).is_some_and(|v| v != 0.0))
            .and_then(|i| fin(&x[i]));
    }
    None
}

/// The `reference :=` rows (a terms table, e.g. `(SELECT list(t) FROM
/// ols_tidy_by(…) t)`), else rows of `t` without an index (an `ols` tidy
/// table unioned with the process).
fn reference_rows(t: &Table, options: &Row) -> Vec<(String, f64, Option<f64>, Option<f64>)> {
    let num = |r: &Row, k: &str| match r.get(k) {
        Some(J::Number(n)) => n.as_f64().filter(|f| f.is_finite()),
        Some(J::String(s)) => s.trim().parse().ok(),
        _ => None,
    };
    let from: Vec<Row> = match options.get("reference") {
        Some(J::Array(a)) => a.iter().filter_map(J::as_object).cloned().collect(),
        Some(J::String(s)) => serde_json::from_str::<J>(s)
            .ok()
            .and_then(|j| j.as_array().cloned())
            .map(|a| a.iter().filter_map(J::as_object).cloned().collect())
            .unwrap_or_default(),
        _ => t
            .rows
            .iter()
            .filter(|r| {
                r.get("index_value").is_none_or(J::is_null)
                    && r.get("window_end").is_none_or(J::is_null)
            })
            .cloned()
            .collect(),
    };
    from.iter()
        .filter_map(|r| {
            let term = r.get("term")?.as_str()?.to_string();
            Some((
                term,
                num(r, "estimate")?,
                num(r, "conf_low"),
                num(r, "conf_high"),
            ))
        })
        .collect()
}

fn path(t: &Table, options: &Row, w: u32, h: u32, o: &RenderOptions, place: Place) -> Out {
    let labels = Labels::from(options);
    let (kind, index_name) = path_kind(t);
    // Index rows only (reference rows without an index are drawn as such).
    let xk = if kind == PathKind::Rolling && t.has("window_end") {
        "window_end"
    } else {
        "index_value"
    };
    let xs = t.xpos(xk);
    let (term, est) = (t.text("term"), t.num("estimate")?);
    let ci = t.has("conf_low") && t.has("conf_high");
    let (lo, hi) = if ci {
        (t.num("conf_low")?, t.num("conf_high")?)
    } else {
        (vec![Value::Na; t.rows.len()], vec![Value::Na; t.rows.len()])
    };
    let models = t.model_key().map(|k| t.text(k));
    let ids = t.text("id");
    let id_vary = t.has("id") && t.distinct("id") > 1;
    let rows: Vec<Coef> = (0..t.rows.len())
        .filter(|&i| xs[i] != Value::Na && fin(&xs[i]).is_some())
        .filter_map(|i| {
            Some(Coef {
                term: text_of(&term[i])?,
                series: models.as_ref().and_then(|m| text_of(&m[i])),
                panel: if id_vary { text_of(&ids[i]) } else { None },
                est: fin(&est[i]),
                lo: fin(&lo[i]),
                hi: fin(&hi[i]),
                x: Some(xs[i].clone()),
            })
        })
        .collect();
    if rows.is_empty() {
        return crate::note_svg(None, "No data", w, h).finish(place);
    }
    // The reference model's rows are not a path series.
    let path_series: Vec<String> = first_seen(rows.iter().filter_map(|r| r.series.as_deref()));
    let facet = options.get("facet").and_then(J::as_bool).unwrap_or(false);
    match kind {
        PathKind::Lambda | PathKind::Lars if !facet => {
            overlay(t, options, &labels, kind, &rows, w, h, o, place)
        }
        _ => {
            let reference = if matches!(kind, PathKind::Tau | PathKind::Rolling | PathKind::Other) {
                reference_rows(t, options)
            } else {
                Vec::new()
            };
            let xlab = match kind {
                PathKind::Tau => "τ (quantile)".to_string(),
                PathKind::Rolling => "window end".to_string(),
                PathKind::Lambda => "λ (log scale)".to_string(),
                PathKind::Lars => "α (LARS)".to_string(),
                PathKind::Other => look::humanize(&index_name),
            };
            let ylab = if ci && rows.iter().any(|r| r.lo.is_some()) {
                format!("estimate ({})", conf_label(t))
            } else {
                "estimate".to_string()
            };
            let log = kind == PathKind::Lambda
                && rows
                    .iter()
                    .all(|r| r.x.as_ref().and_then(fin).is_some_and(|v| v > 0.0));
            per_term(
                &rows,
                &path_series,
                &reference,
                &labels,
                &xlab,
                &ylab,
                log,
                w,
                h,
                o,
                place,
            )
        }
    }
}

/// One panel per term: estimate line + band per series, dashed reference.
#[allow(clippy::too_many_arguments)]
fn per_term(
    rows: &[Coef],
    series: &[String],
    reference: &[(String, f64, Option<f64>, Option<f64>)],
    labels: &Labels,
    xlab: &str,
    ylab: &str,
    log: bool,
    w: u32,
    h: u32,
    o: &RenderOptions,
    place: Place,
) -> Out {
    let mut terms = first_seen(rows.iter().map(|r| r.term.as_str()));
    // The intercept of a path is rarely the question; keep it last.
    terms.sort_by_key(|t| is_intercept(t));
    let panels: Vec<Option<String>> = {
        let p = first_seen(rows.iter().filter_map(|r| r.panel.as_deref()));
        if p.is_empty() {
            vec![None]
        } else {
            p.into_iter().map(Some).collect()
        }
    };
    let several = series.len() > 1;
    let n_cells = terms.len() * panels.len();
    let small = n_cells > 1;
    let mut plots = Vec::new();
    let mut has_ref = false;
    for p in &panels {
        for term in &terms {
            let mut idx: Vec<&Coef> = rows
                .iter()
                .filter(|r| r.term == *term && r.panel == *p && r.est.is_some())
                .collect();
            if idx.is_empty() {
                continue;
            }
            idx.sort_by(|a, b| {
                let (x, y) = (
                    a.x.as_ref().and_then(fin).unwrap_or(0.0),
                    b.x.as_ref().and_then(fin).unwrap_or(0.0),
                );
                x.total_cmp(&y)
            });
            let xv = |r: &Coef| r.x.clone().unwrap_or(Value::Na);
            let sv = |r: &Coef| s(r.series.clone().unwrap_or_default());
            let mut data: Frame = vec![
                ("x".into(), idx.iter().map(|r| xv(r)).collect()),
                (
                    "y".into(),
                    idx.iter()
                        .map(|r| Value::Float(r.est.unwrap_or(0.0)))
                        .collect(),
                ),
            ];
            let mut aes = Aes::new().x("x").y("y");
            if several {
                data.push(("model".into(), idx.iter().map(|r| sv(r)).collect()));
                aes = aes.color("model").group("model");
            }
            let mut plot = GGPlot::new(data).aes(aes);
            // The zero line only when the estimates come near zero: it would
            // otherwise stretch the axis and flatten the path.
            let (lo, hi) = idx
                .iter()
                .flat_map(|r| [r.est, r.lo, r.hi])
                .flatten()
                .fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), v| {
                    (a.min(v), b.max(v))
                });
            if lo.is_finite() && lo - (hi - lo) * 0.25 <= 0.0 && hi + (hi - lo) * 0.25 >= 0.0 {
                plot = look::hline(plot, 0.0);
            }
            // Full-sample reference: grey band + dashed line.
            if let Some((_, e, rl, rh)) = reference.iter().find(|r| r.0 == *term) {
                has_ref = true;
                let (x0, x1) = (idx[0].x.clone(), idx[idx.len() - 1].x.clone());
                if let (Some(a), Some(b), Some(x0), Some(x1)) = (rl, rh, x0, x1) {
                    plot = plot
                        .geom_ribbon_with(GeomRibbon {
                            fill: ACCENT,
                            alpha: 0.10,
                        })
                        .layer_data(vec![
                            ("x".into(), vec![x0, x1]),
                            ("lo".into(), vec![Value::Float(*a); 2]),
                            ("hi".into(), vec![Value::Float(*b); 2]),
                        ])
                        .layer_aes(Aes::new().x("x").ymin("lo").ymax("hi"));
                }
                plot = plot.geom_hline_with(GeomHline {
                    yintercept: *e,
                    color: ACCENT,
                    width: 1.2,
                    linetype: Linetype::Dashed,
                    alpha: 0.85,
                });
            }
            let band: Vec<&&Coef> = idx
                .iter()
                .filter(|r| r.lo.is_some() && r.hi.is_some())
                .collect();
            if !band.is_empty() {
                let mut frame: Frame = vec![
                    ("x".into(), band.iter().map(|r| xv(r)).collect()),
                    (
                        "lo".into(),
                        band.iter()
                            .map(|r| Value::Float(r.lo.unwrap_or(0.0)))
                            .collect(),
                    ),
                    (
                        "hi".into(),
                        band.iter()
                            .map(|r| Value::Float(r.hi.unwrap_or(0.0)))
                            .collect(),
                    ),
                ];
                let mut a = Aes::new().x("x").ymin("lo").ymax("hi");
                if several {
                    frame.push(("model".into(), band.iter().map(|r| sv(r)).collect()));
                    a = a.fill("model").group("model");
                }
                plot = plot
                    .geom_ribbon_with(GeomRibbon {
                        fill: o.brand(),
                        alpha: 0.18,
                    })
                    .layer_data(frame)
                    .layer_aes(a);
            }
            plot = plot.geom_line_with(GeomLine {
                color: o.brand(),
                width: 1.6,
                alpha: 1.0,
            });
            if idx.len() <= 40 {
                plot = plot.geom_point_with(GeomPoint {
                    size: 1.6,
                    color: o.brand(),
                    alpha: 1.0,
                });
            }
            if several {
                plot = plot
                    .scale_color(look::series_scale(Aesthetic::Color, series))
                    .scale_fill(look::series_scale(Aesthetic::Fill, series));
            }
            if log {
                plot = plot.scale_x_continuous(
                    ScaleContinuous::new()
                        .with_transform(ggplot_rs::scale::transform::ScaleTransform::Log10),
                );
            }
            let cell = match p {
                Some(p) if terms.len() > 1 => format!("{term} · {p}"),
                Some(p) => p.clone(),
                None => term.clone(),
            };
            let mut plot = look::styled(plot, o, small)
                .xlab(&labels.x_or(xlab))
                .ylab(&labels.y_or(ylab));
            if small {
                plot = plot.title(&cell);
            }
            plots.push(plot);
        }
    }
    if plots.is_empty() {
        return crate::note_svg(None, "No data", w, h).finish(place);
    }
    let caption = has_ref.then_some("dashed orange: full-sample estimate (shaded: its CI)");
    if plots.len() == 1 {
        let mut p = plots.pop().expect("one");
        if let Some(t) = &labels.title {
            p = p.title(t);
        }
        if let Some(c) = caption {
            p = p.caption(c);
        }
        return finish(p, w, h, place);
    }
    let n = plots.len();
    let mut g = PlotGrid::new()
        .ncol(grid_cols(n, 4).max(1))
        .collect_legends(true);
    for p in plots {
        g = g.add(p);
    }
    if let Some(t) = &labels.title {
        g = g.title(t);
    }
    if let Some(c) = caption {
        g = g.caption(c);
    }
    crate::finish_grid(g, place, w, h)
}

/// The λ path (log λ) or the LARS path (|β|₁ fraction) of every term in one
/// panel, labelled at the line ends; `highlight := [...]` greys the others;
/// λ.min / λ.1se as vertical lines.
#[allow(clippy::too_many_arguments)]
fn overlay(
    t: &Table,
    options: &Row,
    labels: &Labels,
    kind: PathKind,
    rows: &[Coef],
    w: u32,
    h: u32,
    o: &RenderOptions,
    place: Place,
) -> Out {
    let highlight: Vec<String> = match options.get("highlight") {
        Some(J::Array(a)) => a.iter().filter_map(J::as_str).map(str::to_string).collect(),
        Some(J::String(s)) if !s.trim().is_empty() => {
            s.split(',').map(|x| x.trim().to_string()).collect()
        }
        _ => Vec::new(),
    };
    let panels: Vec<Option<String>> = {
        let p = first_seen(rows.iter().filter_map(|r| r.panel.as_deref()));
        if p.is_empty() {
            vec![None]
        } else {
            p.into_iter().map(Some).collect()
        }
    };
    let lmin = mark_value(t, options, "lambda_min", "lambda_min", "is_min");
    let l1se = mark_value(t, options, "lambda_1se", "lambda_1se", "is_1se");
    let small = panels.len() > 1;
    let mut plots = Vec::new();
    for p in &panels {
        let prs: Vec<&Coef> = rows
            .iter()
            .filter(|r| r.panel == *p && !is_intercept(&r.term) && r.est.is_some())
            .collect();
        if prs.is_empty() {
            continue;
        }
        let terms = first_seen(prs.iter().map(|r| r.term.as_str()));
        // x per row: λ, or the L1 fraction at that LARS step.
        let raw = |r: &Coef| r.x.as_ref().and_then(fin).unwrap_or(0.0);
        let xof: Box<dyn Fn(&Coef) -> f64> = if kind == PathKind::Lars {
            let mut l1: Vec<(f64, f64)> = Vec::new();
            for r in &prs {
                let x = raw(r);
                let a = r.est.unwrap_or(0.0).abs();
                match l1.iter_mut().find(|(k, _)| (*k - x).abs() < 1e-12) {
                    Some((_, v)) => *v += a,
                    None => l1.push((x, a)),
                }
            }
            let max = l1.iter().map(|v| v.1).fold(0.0, f64::max);
            Box::new(move |r: &Coef| {
                let x = raw(r);
                let v = l1
                    .iter()
                    .find(|(k, _)| (*k - x).abs() < 1e-12)
                    .map_or(0.0, |v| v.1);
                if max > 0.0 {
                    v / max
                } else {
                    0.0
                }
            })
        } else {
            Box::new(raw)
        };
        let log = kind == PathKind::Lambda && prs.iter().all(|r| raw(r) > 0.0);
        let mut order: Vec<&Coef> = prs.clone();
        order.sort_by(|a, b| a.term.cmp(&b.term).then(xof(a).total_cmp(&xof(b))));
        let on = |term: &str| highlight.is_empty() || highlight.iter().any(|h| h == term);
        let (hot, cold): (Vec<&Coef>, Vec<&Coef>) = order.iter().partition(|r| on(&r.term));
        let frame = |v: &[&Coef]| -> Frame {
            vec![
                ("x".into(), v.iter().map(|r| Value::Float(xof(r))).collect()),
                (
                    "y".into(),
                    v.iter()
                        .map(|r| Value::Float(r.est.unwrap_or(0.0)))
                        .collect(),
                ),
                ("term".into(), v.iter().map(|r| s(r.term.clone())).collect()),
                (
                    "tip".into(),
                    v.iter()
                        .map(|r| {
                            s(format!(
                                "{} at {} = {}",
                                r.term,
                                if kind == PathKind::Lars {
                                    "|β|₁ frac"
                                } else {
                                    "λ"
                                },
                                fmt_num(xof(r))
                            ))
                        })
                        .collect(),
                ),
            ]
        };
        let hot_terms: Vec<String> = terms.iter().filter(|t| on(t)).cloned().collect();
        let mut plot = GGPlot::new(frame(&hot)).aes(
            Aes::new()
                .x("x")
                .y("y")
                .color("term")
                .group("term")
                .label("tip"),
        );
        plot = look::hline(plot, 0.0);
        if !cold.is_empty() {
            plot = plot
                .geom_line_with(GeomLine {
                    color: (196, 200, 208),
                    width: 1.0,
                    alpha: 1.0,
                })
                .layer_data(frame(&cold))
                .layer_aes(Aes::new().x("x").y("y").group("term"));
        }
        plot = plot.geom_line_with(GeomLine {
            color: o.brand(),
            width: 1.7,
            alpha: 1.0,
        });
        // Direct labels at the unshrunk end (small λ / full L1 norm),
        // spread apart vertically.
        let (xlo, xhi) = order
            .iter()
            .map(|r| xof(r))
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), v| {
                (a.min(v), b.max(v))
            });
        let left = kind == PathKind::Lambda;
        let mut ends: Vec<(String, f64, bool)> = terms
            .iter()
            .filter_map(|term| {
                let pts: Vec<&&Coef> = order.iter().filter(|r| r.term == *term).collect();
                let end = if left { pts.first() } else { pts.last() }?;
                Some((term.clone(), end.est.unwrap_or(0.0), on(term)))
            })
            .collect();
        ends.sort_by(|a, b| a.1.total_cmp(&b.1));
        let (ylo, yhi) = order
            .iter()
            .filter_map(|r| r.est)
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), v| {
                (a.min(v), b.max(v))
            });
        let gap = ((yhi - ylo).abs().max(1e-9)) * 0.04;
        let mut ys: Vec<f64> = ends.iter().map(|e| e.1).collect();
        spread(&mut ys, gap);
        for (e, y) in ends.iter_mut().zip(ys) {
            e.1 = y;
        }
        let (lx, room) = if log && xlo > 0.0 {
            let r = (xhi / xlo).max(1.0001);
            (xlo / r.powf(0.012), xlo / r.powf(0.16))
        } else if left {
            let span = (xhi - xlo).max(1e-9);
            (xlo - span * 0.012, xlo - span * 0.16)
        } else {
            let span = (xhi - xlo).max(1e-9);
            (xhi + span * 0.012, xhi + span * 0.16)
        };
        for (hotness, color) in [(true, INK), (false, (150, 156, 168))] {
            let pick: Vec<&(String, f64, bool)> = ends.iter().filter(|e| e.2 == hotness).collect();
            if pick.is_empty() {
                continue;
            }
            plot = plot
                .geom_text_with(GeomText {
                    size: if small { 9.5 } else { 10.5 },
                    color,
                    hjust: if left { 1.0 } else { 0.0 },
                    vjust: 0.5,
                    ..Default::default()
                })
                .layer_data(vec![
                    ("x".into(), vec![Value::Float(lx); pick.len()]),
                    ("y".into(), pick.iter().map(|e| Value::Float(e.1)).collect()),
                    ("note".into(), pick.iter().map(|e| s(e.0.clone())).collect()),
                ])
                .layer_aes(Aes::new().x("x").y("y").label("note"));
        }
        plot = plot
            .geom_blank()
            .layer_data(vec![
                ("x".into(), vec![Value::Float(room)]),
                ("y".into(), vec![Value::Float(ylo)]),
            ])
            .layer_aes(Aes::new().x("x").y("y"));
        if kind == PathKind::Lambda {
            for (v, label, color) in [(lmin, "λ.min", ACCENT), (l1se, "λ.1se", INK)] {
                let Some(v) = v.filter(|v| !log || *v > 0.0) else {
                    continue;
                };
                plot = plot
                    .geom_vline_with(GeomVline {
                        xintercept: v,
                        color,
                        width: 1.2,
                        linetype: Linetype::Dashed,
                        alpha: 0.9,
                    })
                    .geom_text_with(GeomText {
                        size: 10.0,
                        color,
                        hjust: 0.0,
                        vjust: 0.0,
                        ..Default::default()
                    })
                    .layer_data(vec![
                        (
                            "x".into(),
                            vec![Value::Float(if log {
                                v * (xhi / xlo).powf(0.01)
                            } else {
                                v + (xhi - xlo) * 0.01
                            })],
                        ),
                        (
                            "y".into(),
                            vec![Value::Float(
                                yhi + (yhi - ylo).abs()
                                    * if label == "λ.min" { 0.02 } else { 0.08 },
                            )],
                        ),
                        ("note".into(), vec![s(label)]),
                    ])
                    .layer_aes(Aes::new().x("x").y("y").label("note"));
            }
        }
        plot = plot
            .scale_color(look::series_scale(Aesthetic::Color, &hot_terms))
            .legend_position(ggplot_rs::theme::LegendPosition::None);
        if log {
            plot = plot.scale_x_continuous(
                ScaleContinuous::new()
                    .with_transform(ggplot_rs::scale::transform::ScaleTransform::Log10),
            );
        } else if kind == PathKind::Lars {
            plot = plot.scale_x_continuous(
                ScaleContinuous::new().with_breaks(vec![0.0, 0.25, 0.5, 0.75, 1.0]),
            );
        }
        let (xlab, ylab) = match kind {
            PathKind::Lars => ("|β|₁ / max |β|₁ (fraction of L1 norm)", "coefficient"),
            _ if log => ("λ (log scale)", "coefficient"),
            _ => ("λ", "coefficient"),
        };
        let mut plot = look::styled(plot, o, small)
            .legend_position(ggplot_rs::theme::LegendPosition::None)
            .xlab(&labels.x_or(xlab))
            .ylab(&labels.y_or(ylab));
        if let Some(p) = p.as_ref().filter(|_| small) {
            plot = plot.title(p);
        }
        plots.push(plot);
    }
    if plots.is_empty() {
        return crate::note_svg(None, "No data", w, h).finish(place);
    }
    if plots.len() == 1 {
        let mut p = plots.pop().expect("one");
        if let Some(t) = &labels.title {
            p = p.title(t);
        }
        return finish(p, w, h, place);
    }
    let n = plots.len();
    grid(plots, grid_cols(n, 3), labels.title.as_deref(), w, h, place)
}

/// Push sorted label positions apart to at least `gap`, moving each crowded
/// pair symmetrically (labels stay centred on their lines' ends).
pub(super) fn spread(ys: &mut [f64], gap: f64) {
    for _ in 0..200 {
        let mut moved = false;
        for i in 1..ys.len() {
            let d = ys[i] - ys[i - 1];
            if d < gap - 1e-12 {
                let push = (gap - d) / 2.0;
                ys[i - 1] -= push;
                ys[i] += push;
                moved = true;
            }
        }
        if !moved {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_spread_symmetrically() {
        let mut v = vec![0.0, 1.0, 1.0, 1.0, 5.0];
        spread(&mut v, 0.5);
        assert!(v.windows(2).all(|w| w[1] - w[0] >= 0.5 - 1e-9), "{v:?}");
        // The crowded three stay centred on 1.0.
        assert!(((v[1] + v[2] + v[3]) / 3.0 - 1.0).abs() < 0.2, "{v:?}");
        assert_eq!(v[4], 5.0);
    }
}
