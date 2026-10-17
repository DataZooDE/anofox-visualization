//! `curve` plots drawn directly: ROC (square, AUC in the legend), PR
//! (base-rate line, the degenerate recall-0 points trimmed), calibration
//! (square, point size by bin count) and ACF / PACF (lag 0 dropped).

use super::look::{self, Labels};
use super::stats::{curve_series, fin, finish, fmt_num, grid, grid_cols, s, text_of, Frame, Out};
use super::{Row, Table};
use crate::{Place, RenderOptions};
use ggplot_rs::prelude::*;
use serde_json::Value as J;

/// Rows per series (first-seen series order), each sorted by (x, y).
/// Row indices per series, in series order.
type Groups = Vec<(String, Vec<usize>)>;

fn by_series(t: &Table) -> Result<(Option<&'static str>, Groups), String> {
    let key = curve_series(t);
    let kv = key.map(|k| t.text(k));
    let (x, y) = (t.num("x")?, t.num("y")?);
    let mut out: Vec<(String, Vec<usize>)> = Vec::new();
    for i in 0..t.rows.len() {
        if fin(&x[i]).is_none() || fin(&y[i]).is_none() {
            continue;
        }
        let k = kv.as_ref().and_then(|v| text_of(&v[i])).unwrap_or_default();
        match out.iter_mut().find(|(g, _)| *g == k) {
            Some((_, v)) => v.push(i),
            None => out.push((k, vec![i])),
        }
    }
    for (_, v) in out.iter_mut() {
        v.sort_by(|&a, &b| {
            fin(&x[a])
                .unwrap_or(0.0)
                .total_cmp(&fin(&x[b]).unwrap_or(0.0))
                .then(
                    fin(&y[a])
                        .unwrap_or(0.0)
                        .total_cmp(&fin(&y[b]).unwrap_or(0.0)),
                )
        });
    }
    Ok((key, out))
}

/// Trapezoid area under (x, y) sorted by x.
pub(super) fn auc(pts: &[(f64, f64)]) -> f64 {
    pts.windows(2)
        .map(|w| (w[1].0 - w[0].0) * (w[1].1 + w[0].1) / 2.0)
        .sum()
}

fn num_opt(options: &Row, k: &str) -> Option<f64> {
    match options.get(k) {
        Some(J::Number(n)) => n.as_f64(),
        Some(J::String(s)) => s.trim().parse().ok(),
        _ => None,
    }
    .filter(|v| v.is_finite())
}

/// The line plot shared by ROC and PR: one coloured line per series (legend
/// = `names`, in series order).
fn lines(series: &[(String, Vec<usize>)], names: &[String], x: &[Value], y: &[Value]) -> GGPlot {
    let (mut fx, mut fy, mut fk) = (vec![], vec![], vec![]);
    for ((_, idx), name) in series.iter().zip(names) {
        for &i in idx {
            fx.push(x[i].clone());
            fy.push(y[i].clone());
            fk.push(s(name.clone()));
        }
    }
    GGPlot::new(vec![
        ("x".to_string(), fx),
        ("y".to_string(), fy),
        ("model".to_string(), fk),
    ])
    .aes(Aes::new().x("x").y("y").color("model").group("model"))
}

fn unit_axes(plot: GGPlot) -> GGPlot {
    let b = vec![0.0, 0.25, 0.5, 0.75, 1.0];
    plot.scale_x_continuous(
        ScaleContinuous::new()
            .with_limits(0.0, 1.0)
            .with_breaks(b.clone()),
    )
    .scale_y_continuous(ScaleContinuous::new().with_limits(0.0, 1.0).with_breaks(b))
}

fn diagonal(plot: GGPlot) -> GGPlot {
    plot.geom_abline_with(GeomAbline {
        slope: 1.0,
        intercept: 0.0,
        color: look::REF,
        width: look::REF_WIDTH,
        linetype: Linetype::Dashed,
        alpha: 1.0,
    })
}

/// An AUC with 2 decimals, more when that would round a near-perfect one up
/// to 1.00 (0.9952 → "0.995", not "1.00").
fn auc_text(a: f64) -> String {
    (2..=4)
        .map(|d| format!("{a:.d$}"))
        .find(|t| a >= 1.0 || !t.starts_with("1."))
        .unwrap_or_else(|| format!("{a:.4}"))
}

pub(super) fn roc(
    t: &Table,
    options: &Row,
    w: u32,
    h: u32,
    o: &RenderOptions,
    place: Place,
) -> Out {
    let labels = Labels::from(options);
    let (_, series) = by_series(t)?;
    let (x, y) = (t.num("x")?, t.num("y")?);
    let given = if t.has("auc") {
        Some(t.num("auc")?)
    } else {
        None
    };
    let names: Vec<String> = series
        .iter()
        .map(|(k, idx)| {
            let a = given
                .as_ref()
                .and_then(|g| idx.iter().find_map(|&i| fin(&g[i])))
                .unwrap_or_else(|| {
                    let pts: Vec<(f64, f64)> = idx
                        .iter()
                        .filter_map(|&i| Some((fin(&x[i])?, fin(&y[i])?)))
                        .collect();
                    auc(&pts)
                });
            let a = auc_text(a);
            if k.is_empty() {
                format!("AUC {a}")
            } else {
                format!("{k} · AUC {a}")
            }
        })
        .collect();
    let mut plot = diagonal(lines(&series, &names, &x, &y)).geom_line_with(GeomLine {
        color: o.brand(),
        width: 1.7,
        alpha: 1.0,
    });
    let keys: Vec<String> = series.iter().map(|s| s.0.clone()).collect();
    let colors = look::level_colors(&keys);
    plot = plot.scale_color(
        ScaleColorDiscrete::new(Aesthetic::Color)
            .with_levels(names.clone())
            .with_palette(
                colors
                    .iter()
                    .map(|&(r, g, b)| {
                        if keys.len() == 1 {
                            let (r, g, b) = o.brand();
                            RGBAColor::new(r, g, b)
                        } else {
                            RGBAColor::new(r, g, b)
                        }
                    })
                    .collect(),
            ),
    );
    // The legend (with the AUCs) in the empty lower-right corner.
    let plot = labels
        .apply(
            look::styled_aspect(unit_axes(plot), o, false, Some(1.0)),
            "false positive rate",
            "true positive rate",
        )
        .legend_position_inside(0.48, (0.08 * names.len() as f64 + 0.06).min(0.6));
    finish(plot, w, h, place)
}

pub(super) fn pr(t: &Table, options: &Row, w: u32, h: u32, o: &RenderOptions, place: Place) -> Out {
    let labels = Labels::from(options);
    let (_, mut series) = by_series(t)?;
    let (x, y) = (t.num("x")?, t.num("y")?);
    // The recall-0 points (precision undefined or 1 by convention) pin the
    // curve to the top-left corner: trimmed.
    for (_, idx) in series.iter_mut() {
        let keep: Vec<usize> = idx
            .iter()
            .copied()
            .filter(|&i| fin(&x[i]).is_some_and(|r| r > 1e-9))
            .collect();
        if keep.len() >= 2 {
            *idx = keep;
        }
        pr_order(idx, &x, &y);
    }
    // Base rate: `base_rate :=`, else the precision at the lowest threshold
    // with full recall (every case called positive).
    let base = num_opt(options, "base_rate").or_else(|| {
        let th = t.num("threshold").ok()?;
        (0..t.rows.len())
            .filter(|&i| fin(&x[i]).is_some_and(|r| r >= 1.0 - 1e-9))
            .filter_map(|i| Some((fin(&th[i])?, fin(&y[i])?)))
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|v| v.1)
    });
    let names: Vec<String> = series
        .iter()
        .map(|(k, _)| {
            if k.is_empty() {
                "precision".into()
            } else {
                k.clone()
            }
        })
        .collect();
    let mut plot = lines(&series, &names, &x, &y);
    if let Some(b) = base.filter(|b| (0.0..=1.0).contains(b)) {
        plot = look::hline(plot, b)
            .geom_text_with(GeomText {
                size: 10.0,
                color: (120, 126, 138),
                hjust: 0.0,
                vjust: 1.0,
                ..Default::default()
            })
            .layer_data(vec![
                ("x".into(), vec![Value::Float(0.01)]),
                ("y".into(), vec![Value::Float(b - 0.015)]),
                ("note".into(), vec![s(format!("base rate {}", fmt_num(b)))]),
            ])
            .layer_aes(Aes::new().x("x").y("y").label("note"));
    }
    plot = plot.geom_line_with(GeomLine {
        color: o.brand(),
        width: 1.7,
        alpha: 1.0,
    });
    let several = series.len() > 1;
    plot = if several {
        plot.scale_color(look::series_scale(Aesthetic::Color, &names))
    } else {
        plot.scale_color_manual(vec![(names[0].as_str(), super::stats::rgba(o.brand()))])
            .legend_position(ggplot_rs::theme::LegendPosition::None)
    };
    let mut plot = labels.apply(
        look::styled(unit_axes(plot), o, false),
        "recall",
        "precision",
    );
    if !several {
        plot = plot.legend_position(ggplot_rs::theme::LegendPosition::None);
    }
    finish(plot, w, h, place)
}

/// Along recall, points tied on recall run from high to low precision (the
/// threshold falling): sorted by (x, y) they would spike back up at full
/// recall, the base rate first.
fn pr_order(idx: &mut [usize], x: &[Value], y: &[Value]) {
    let v = |c: &[Value], i: usize| fin(&c[i]).unwrap_or(0.0);
    idx.sort_by(|&a, &b| {
        v(x, a)
            .total_cmp(&v(x, b))
            .then(v(y, b).total_cmp(&v(y, a)))
    });
}

pub(super) fn calibration(
    t: &Table,
    options: &Row,
    w: u32,
    h: u32,
    o: &RenderOptions,
    place: Place,
) -> Out {
    let labels = Labels::from(options);
    let (_, series) = by_series(t)?;
    let (x, y) = (t.num("x")?, t.num("y")?);
    let nk = ["weight", "n", "count", "n_obs"]
        .into_iter()
        .find(|k| t.has(k));
    let nv = match nk {
        Some(k) => Some(t.num(k)?),
        None => None,
    };
    let band = t.has("y_low") && t.has("y_high");
    let (lo, hi) = if band {
        (t.num("y_low")?, t.num("y_high")?)
    } else {
        (Vec::new(), Vec::new())
    };
    let names: Vec<String> = series
        .iter()
        .map(|(k, _)| {
            if k.is_empty() {
                "observed".into()
            } else {
                k.clone()
            }
        })
        .collect();
    let mut plot = diagonal(lines(&series, &names, &x, &y));
    if band {
        let (mut bx, mut bl, mut bh, mut bk) = (vec![], vec![], vec![], vec![]);
        for ((_, idx), name) in series.iter().zip(&names) {
            for &i in idx {
                if let (Some(a), Some(b)) = (fin(&lo[i]), fin(&hi[i])) {
                    bx.push(x[i].clone());
                    bl.push(Value::Float(a));
                    bh.push(Value::Float(b));
                    bk.push(s(name.clone()));
                }
            }
        }
        plot = plot
            .geom_errorbar_with(GeomErrorbar {
                color: (160, 166, 178),
                width: 1.0,
                cap_width: 0.0,
                alpha: 0.8,
            })
            .layer_data(vec![
                ("x".into(), bx),
                ("lo".into(), bl),
                ("hi".into(), bh),
                ("model".into(), bk),
            ])
            .layer_aes(Aes::new().x("x").ymin("lo").ymax("hi").color("model"));
    }
    plot = plot.geom_line_with(GeomLine {
        color: o.brand(),
        width: 1.3,
        alpha: 0.9,
    });
    // Points sized by the bin's count.
    let (mut px, mut py, mut pk, mut pn) = (vec![], vec![], vec![], vec![]);
    for ((_, idx), name) in series.iter().zip(&names) {
        for &i in idx {
            px.push(x[i].clone());
            py.push(y[i].clone());
            pk.push(s(name.clone()));
            pn.push(
                nv.as_ref()
                    .and_then(|v| fin(&v[i]))
                    .map_or(Value::Float(1.0), Value::Float),
            );
        }
    }
    let mut aes = Aes::new().x("x").y("y").color("model");
    let mut frame: Frame = vec![("x".into(), px), ("y".into(), py), ("model".into(), pk)];
    if nv.is_some() {
        frame.push(("n".into(), pn));
        aes = aes.size("n");
    }
    plot = plot
        .geom_point_with(GeomPoint {
            size: 3.0,
            color: o.brand(),
            alpha: 0.95,
        })
        .layer_data(frame)
        .layer_aes(aes);
    if nv.is_some() {
        plot = plot.scale_size(
            ggplot_rs::scale::size::ScaleSizeContinuous::new()
                .with_range(1.6, 6.0)
                .with_name("bin n"),
        );
    }
    let several = series.len() > 1;
    plot = if several {
        plot.scale_color(look::series_scale(Aesthetic::Color, &names))
    } else {
        plot.scale_color_manual(vec![(names[0].as_str(), super::stats::rgba(o.brand()))])
    };
    let mut plot = labels.apply(
        look::styled_aspect(unit_axes(plot), o, false, Some(1.0)),
        "mean predicted probability",
        "observed rate",
    );
    if !several && nv.is_none() {
        plot = plot.legend_position(ggplot_rs::theme::LegendPosition::None);
    } else {
        // Well-calibrated points hug the diagonal: the upper-left is free.
        plot = plot.legend_position_inside(0.03, 0.97);
    }
    finish(plot, w, h, place)
}

/// ACF / PACF lollipops (lag 0 of the ACF is 1 by definition: dropped),
/// significance bounds from y_low / y_high; one panel per series.
pub(super) fn acf(
    t: &Table,
    ty: &str,
    options: &Row,
    w: u32,
    h: u32,
    o: &RenderOptions,
    place: Place,
) -> Out {
    let labels = Labels::from(options);
    let (_, series) = by_series(t)?;
    let (x, y) = (t.num("x")?, t.num("y")?);
    let band = t.has("y_low") && t.has("y_high");
    let (lo, hi) = if band {
        (t.num("y_low")?, t.num("y_high")?)
    } else {
        (Vec::new(), Vec::new())
    };
    let ylab = if ty == "pacf" {
        "partial autocorrelation"
    } else {
        "autocorrelation"
    };
    let small = series.len() > 1;
    let mut plots = Vec::new();
    for (k, idx) in &series {
        let idx: Vec<usize> = idx
            .iter()
            .copied()
            .filter(|&i| !(ty == "acf" && fin(&x[i]).is_some_and(|l| l.abs() < 1e-9)))
            .collect();
        if idx.is_empty() {
            continue;
        }
        let pick = |v: &[Value]| idx.iter().map(|&i| v[i].clone()).collect::<Vec<_>>();
        let n = idx.len();
        let mut plot = GGPlot::new(vec![
            ("x".to_string(), pick(&x)),
            ("y".to_string(), pick(&y)),
        ])
        .aes(Aes::new().x("x").y("y"));
        plot = look::hline(plot, 0.0);
        if band {
            let mut seen: Vec<f64> = Vec::new();
            for v in idx
                .iter()
                .flat_map(|&i| [fin(&lo[i]), fin(&hi[i])])
                .flatten()
            {
                if !seen.iter().any(|s| (s - v).abs() < 1e-9) && seen.len() < 8 {
                    seen.push(v);
                }
            }
            for b in seen {
                plot = plot.geom_hline_with(GeomHline {
                    yintercept: b,
                    color: (125, 140, 170),
                    width: 1.0,
                    linetype: Linetype::Dashed,
                    alpha: 1.0,
                });
            }
        }
        plot = plot
            .geom_segment_with(GeomSegment {
                color: o.brand(),
                width: 1.6,
                alpha: 1.0,
            })
            .layer_data(vec![
                ("x".into(), pick(&x)),
                ("xend".into(), pick(&x)),
                ("y".into(), vec![Value::Float(0.0); n]),
                ("yend".into(), pick(&y)),
            ])
            .layer_aes(Aes::new().x("x").xend("xend").y("y").yend("yend"))
            .geom_point_with(GeomPoint {
                size: 2.4,
                color: o.brand(),
                alpha: 1.0,
            });
        let mut plot = look::styled(plot, o, small)
            .xlab(&labels.x_or("lag"))
            .ylab(&labels.y_or(ylab));
        if small && !k.is_empty() {
            plot = plot.title(k);
        } else if let (false, Some(t)) = (small, &labels.title) {
            plot = plot.title(t);
        }
        plots.push(plot);
    }
    if plots.is_empty() {
        return crate::note_svg(None, "No data", w, h).finish(place);
    }
    if plots.len() == 1 {
        return finish(plots.pop().expect("one"), w, h, place);
    }
    let n = plots.len();
    grid(plots, grid_cols(n, 3), labels.title.as_deref(), w, h, place)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trapezoid_auc() {
        assert!((auc(&[(0.0, 0.0), (1.0, 1.0)]) - 0.5).abs() < 1e-12);
        assert!((auc(&[(0.0, 0.0), (0.0, 1.0), (1.0, 1.0)]) - 1.0).abs() < 1e-12);
    }

    #[test]
    fn pr_ties_on_recall_fall_in_precision() {
        let f = |v: &[f64]| v.iter().map(|&n| Value::Float(n)).collect::<Vec<_>>();
        let (x, y) = (f(&[1.0, 0.5, 1.0, 1.0]), f(&[0.25, 1.0, 0.43, 0.28]));
        let mut idx = vec![0, 1, 2, 3];
        pr_order(&mut idx, &x, &y);
        assert_eq!(idx, [1, 2, 3, 0]);
    }
}
