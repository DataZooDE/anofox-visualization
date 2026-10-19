//! `checks`: anofox-tabular's data-quality check results — `run_checks`,
//! `agg_check` and the other one-row checks with their measure renamed to
//! `value` and their bounds to `lower_threshold` / `upper_threshold`: one
//! panel per check (its own x scale) with the value as a dot against the
//! acceptance band, coloured by status (fail red, warn amber, pass green,
//! else muted); over several `run_ts` / `ds` values a status strip instead,
//! one lane per check.

use super::look::{self, Labels};
use super::stats::{self, fin, fmt_num, rgba, s, text_of, Out};
use super::{opt_str, Row, Table};
use crate::{Place, RenderOptions};
use ggplot_rs::prelude::*;

/// The time keys a `checks` row may carry (`x :=`, else `run_ts`, else `ds`).
const KEYS: &[&str] = &["run_ts", "ds"];

/// Normalise a `status` value (case-insensitive, trimmed) to one of the
/// known classes; anything else is kept as given (drawn muted).
pub(super) fn status_of(raw: &str) -> &str {
    let t = raw.trim();
    match t.to_ascii_lowercase().as_str() {
        "pass" | "passed" | "ok" | "success" | "good" => "pass",
        "warn" | "warning" => "warn",
        "fail" | "failed" | "error" | "alarm" | "critical" => "fail",
        _ => t,
    }
}

/// The colour of a status class: fail/warn/pass from the theme, else muted.
fn status_color(status: &str, o: &RenderOptions) -> (u8, u8, u8) {
    match status {
        "fail" => o.theme.colors.negative,
        "warn" => o.theme.colors.warning,
        "pass" => o.theme.colors.positive,
        _ => o.theme.colors.muted,
    }
}

/// `kind := 'band' | 'strip'` and the time column, resolved from the options
/// and the table: `strip` when the time key exists and has more than one
/// distinct value, else `band`. Pure, for the unit tests.
pub(super) fn mode(t: &Table, options: &Row) -> Result<(&'static str, Option<String>), String> {
    let time_key =
        opt_str(options, "x").or_else(|| KEYS.iter().find(|k| t.has(k)).map(|k| k.to_string()));
    let kind = match opt_str(options, "kind") {
        Some(k) => match k.as_str() {
            "band" => "band",
            "strip" => "strip",
            other => return Err(format!("kind := '{other}' is not one of 'band', 'strip'")),
        },
        None => match &time_key {
            Some(k) if t.distinct(k) > 1 => "strip",
            _ => "band",
        },
    };
    Ok((kind, time_key))
}

/// A numeric-aware sort: values that both parse as numbers compare
/// numerically, else lexicographically.
fn sort_aware(v: &mut [String]) {
    v.sort_by(|a, b| match (a.parse::<f64>(), b.parse::<f64>()) {
        (Ok(p), Ok(q)) => p.total_cmp(&q),
        _ => a.cmp(b),
    });
}

/// A single plottable check row (finite `value`).
struct CheckRow {
    label: String,
    identifier: Option<String>,
    /// The normalised class (`pass` / `warn` / `fail` / other).
    status: String,
    /// The status as given, for the tooltip.
    status_raw: String,
    value: f64,
    lower: Option<f64>,
    upper: Option<f64>,
    message: Option<String>,
    time: Option<Value>,
}

/// `<status> · <label>[ · <identifier>]: <message>`.
fn tip(r: &CheckRow) -> String {
    let mut head = format!("{} · {}", r.status_raw, r.label);
    if let Some(id) = r.identifier.as_deref().filter(|s| !s.is_empty()) {
        head = format!("{head} · {id}");
    }
    match r.message.as_deref().filter(|s| !s.is_empty()) {
        Some(m) => format!("{head}: {m}"),
        None => head,
    }
}

/// The shared threshold of every row in `vs` (within 1e-9), or `None` when
/// any row lacks it or they differ.
fn shared_threshold(vs: &[Option<f64>]) -> Option<f64> {
    let v0 = (*vs.first()?)?;
    vs.iter()
        .all(|v| v.is_some_and(|v| (v - v0).abs() < 1e-9))
        .then_some(v0)
}

/// `anofox_plot_checks(tbl)`: a band of checks (default), or a status strip
/// over time (`kind := 'strip'`, or several `run_ts` / `ds` values).
pub(super) fn checks(
    t: &Table,
    options: &Row,
    w: u32,
    h: u32,
    o: &RenderOptions,
    place: Place,
) -> Out {
    t.need(&["status", "value"])?;
    let labels = Labels::from(options);
    let (kind, time_key) = mode(t, options)?;
    if kind == "strip" && time_key.is_none() {
        return Err(format!(
            "kind := 'strip' needs a time column: run_ts, ds, or x := '<column>' (got: {})",
            t.keys()
        ));
    }
    let n = t.rows.len();
    let label_key = ["check_name", "check_type", "agg"]
        .into_iter()
        .find(|k| t.has(k));
    let raw_label = label_key.map(|k| t.text(k));
    let table_multi = t.distinct("table_name") > 1;
    let table_v = t.text("table_name");
    let status_v = t.text("status");
    let value_v = t.num("value")?;
    let lower_v = t.num("lower_threshold")?;
    let upper_v = t.num("upper_threshold")?;
    let id_v = t.text("identifier");
    let msg_v = t.text("message");
    let time_v = time_key.as_deref().map(|k| t.xpos(k));
    let rows: Vec<CheckRow> = (0..n)
        .filter_map(|i| {
            let value = fin(&value_v[i])?;
            let base = raw_label
                .as_ref()
                .and_then(|v| text_of(&v[i]))
                .unwrap_or_else(|| "(unnamed)".into());
            let label = if table_multi {
                format!("{} · {base}", text_of(&table_v[i]).unwrap_or_default())
            } else {
                base
            };
            let status_raw = text_of(&status_v[i]).unwrap_or_else(|| "unknown".into());
            let status = status_of(&status_raw).to_string();
            Some(CheckRow {
                label,
                identifier: text_of(&id_v[i]),
                status,
                status_raw,
                value,
                lower: fin(&lower_v[i]),
                upper: fin(&upper_v[i]),
                message: text_of(&msg_v[i]),
                time: time_v.as_ref().map(|v| v[i].clone()),
            })
        })
        .collect();
    if rows.is_empty() {
        return crate::note_svg(None, "No data", w, h).finish(place);
    }
    // Legend levels: fail, warn, pass, then others as first seen; only the
    // ones present.
    let mut levels: Vec<String> = Vec::new();
    for k in ["fail", "warn", "pass"] {
        if rows.iter().any(|r| r.status == k) {
            levels.push(k.into());
        }
    }
    for r in &rows {
        if !levels.contains(&r.status) {
            levels.push(r.status.clone());
        }
    }
    let palette: Vec<RGBAColor> = levels.iter().map(|l| rgba(status_color(l, o))).collect();
    match kind {
        "band" => band(&rows, &levels, &palette, &labels, o, w, h, place),
        _ => strip(
            &rows,
            time_key.as_deref(),
            &levels,
            &palette,
            &labels,
            o,
            w,
            h,
            place,
        ),
    }
}

#[allow(clippy::too_many_arguments)]
fn band(
    rows: &[CheckRow],
    levels: &[String],
    palette: &[RGBAColor],
    labels: &Labels,
    o: &RenderOptions,
    w: u32,
    h: u32,
    place: Place,
) -> Out {
    let mut panel_labels: Vec<String> = Vec::new();
    for r in rows {
        if !panel_labels.contains(&r.label) {
            panel_labels.push(r.label.clone());
        }
    }
    sort_aware(&mut panel_labels);
    let small = panel_labels.len() > 1;
    let hide_legend = levels.len() < 2;
    let mut plots = Vec::new();
    for pl in &panel_labels {
        let prs: Vec<&CheckRow> = rows.iter().filter(|r| &r.label == pl).collect();
        // Duplicates: several rows sharing (label, identifier) → keep the
        // one with the greatest time key, else the last row.
        let mut dedup: Vec<&CheckRow> = Vec::new();
        for r in prs {
            match dedup.iter_mut().find(|d| d.identifier == r.identifier) {
                Some(slot) => {
                    let at = |c: &CheckRow| c.time.as_ref().and_then(fin);
                    let newer = match (at(slot), at(r)) {
                        (Some(a), Some(b)) => b >= a,
                        _ => true,
                    };
                    if newer {
                        *slot = r;
                    }
                }
                None => dedup.push(r),
            }
        }
        let mut lane_names: Vec<String> =
            dedup.iter().filter_map(|r| r.identifier.clone()).collect();
        sort_aware(&mut lane_names);
        lane_names.dedup();
        let has_ids = !lane_names.is_empty();
        let lane_labels: Vec<String> = if has_ids {
            lane_names
        } else {
            vec![String::new()]
        };
        let n_lanes = lane_labels.len();
        let lane_of = |r: &CheckRow| -> usize {
            if has_ids {
                lane_labels
                    .iter()
                    .position(|l| Some(l) == r.identifier.as_ref())
                    .unwrap_or(0)
            } else {
                0
            }
        };
        let mut extent: Vec<f64> = Vec::new();
        for r in &dedup {
            extent.push(r.value);
            if let Some(v) = r.lower {
                extent.push(v);
            }
            if let Some(v) = r.upper {
                extent.push(v);
            }
        }
        let lo = extent.iter().copied().fold(f64::INFINITY, f64::min);
        let hi = extent.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        let span = if hi > lo {
            hi - lo
        } else if hi != 0.0 {
            hi.abs()
        } else {
            1.0
        };
        let (xlo, xhi) = (lo - 0.08 * span, hi + 0.25 * span);

        let xs: Vec<Value> = dedup.iter().map(|r| Value::Float(r.value)).collect();
        let ys: Vec<Value> = dedup
            .iter()
            .map(|r| Value::Float(stats::slot(lane_of(r), n_lanes)))
            .collect();
        let sts: Vec<Value> = dedup.iter().map(|r| s(r.status.clone())).collect();
        let tips: Vec<Value> = dedup.iter().map(|r| s(tip(r))).collect();
        let mut plot = GGPlot::new(vec![
            ("x".into(), xs.clone()),
            ("y".into(), ys.clone()),
            ("status".into(), sts),
            ("tip".into(), tips),
        ])
        .aes(Aes::new().x("x").y("y").color("status").label("tip"));

        // Band: one rect per lane with at least one threshold.
        let mut xmin = Vec::new();
        let mut xmax = Vec::new();
        let mut ymin = Vec::new();
        let mut ymax = Vec::new();
        for r in &dedup {
            if r.lower.is_none() && r.upper.is_none() {
                continue;
            }
            let slot = stats::slot(lane_of(r), n_lanes);
            xmin.push(Value::Float(r.lower.unwrap_or(xlo)));
            xmax.push(Value::Float(r.upper.unwrap_or(xhi)));
            ymin.push(Value::Float(slot - 0.3));
            ymax.push(Value::Float(slot + 0.3));
        }
        if !xmin.is_empty() {
            plot = plot
                .geom_rect_with(GeomRect {
                    fill: o.theme.colors.positive,
                    color: o.theme.colors.positive,
                    alpha: 0.15,
                    line_width: 0.0,
                })
                .layer_data(vec![
                    ("xmin".into(), xmin),
                    ("xmax".into(), xmax),
                    ("ymin".into(), ymin),
                    ("ymax".into(), ymax),
                ])
                .layer_aes(
                    Aes::new()
                        .x("xmin")
                        .xmin("xmin")
                        .xmax("xmax")
                        .ymin("ymin")
                        .ymax("ymax"),
                );
        }
        // A quiet vline at each threshold every lane of the panel shares.
        let lowers: Vec<Option<f64>> = dedup.iter().map(|r| r.lower).collect();
        let uppers: Vec<Option<f64>> = dedup.iter().map(|r| r.upper).collect();
        for v in [shared_threshold(&lowers), shared_threshold(&uppers)]
            .into_iter()
            .flatten()
        {
            plot = look::vline(plot, v);
        }
        plot = plot.geom_point_with(GeomPoint {
            size: 3.6,
            color: o.brand(),
            alpha: 1.0,
        });
        plot = plot
            .geom_text_with(GeomText {
                size: 10.0,
                color: stats::INK,
                hjust: 0.0,
                vjust: 0.5,
                ..Default::default()
            })
            .layer_data(vec![
                (
                    "x".into(),
                    dedup
                        .iter()
                        .map(|r| Value::Float(r.value + span * 0.02))
                        .collect(),
                ),
                ("y".into(), ys),
                (
                    "note".into(),
                    dedup.iter().map(|r| s(fmt_num(r.value))).collect(),
                ),
            ])
            .layer_aes(Aes::new().x("x").y("y").label("note"));
        plot = plot
            .scale_color(
                ScaleColorDiscrete::new(Aesthetic::Color)
                    .with_levels(levels.to_vec())
                    .with_palette(palette.to_vec()),
            )
            .scale_x_continuous(ScaleContinuous::new().with_limits(xlo, xhi))
            .scale_y_continuous(stats::label_axis(&lane_labels));
        plot = labels.apply(plot, "", "");
        plot = plot.title(pl);
        if hide_legend {
            plot = plot.legend_position(ggplot_rs::theme::LegendPosition::None);
        }
        plots.push(look::styled(plot, o, small));
    }
    if plots.len() == 1 {
        return stats::finish(plots.pop().expect("one"), w, h, place);
    }
    let n = plots.len();
    stats::grid(
        plots,
        stats::grid_cols(n, 4),
        labels.title.as_deref(),
        w,
        h,
        place,
    )
}

#[allow(clippy::too_many_arguments)]
fn strip(
    rows: &[CheckRow],
    time_key: Option<&str>,
    levels: &[String],
    palette: &[RGBAColor],
    labels: &Labels,
    o: &RenderOptions,
    w: u32,
    h: u32,
    place: Place,
) -> Out {
    let lane_of_row = |r: &CheckRow| -> String {
        match r.identifier.as_deref().filter(|s| !s.is_empty()) {
            Some(id) => format!("{} · {id}", r.label),
            None => r.label.clone(),
        }
    };
    let mut lanes: Vec<String> = Vec::new();
    for r in rows {
        let l = lane_of_row(r);
        if !lanes.contains(&l) {
            lanes.push(l);
        }
    }
    sort_aware(&mut lanes);
    let n = lanes.len();
    let xs: Vec<Value> = rows
        .iter()
        .map(|r| r.time.clone().unwrap_or(Value::Na))
        .collect();
    let ys: Vec<Value> = rows
        .iter()
        .map(|r| {
            let at = lanes.iter().position(|l| *l == lane_of_row(r)).unwrap_or(0);
            Value::Float(stats::slot(at, n))
        })
        .collect();
    let sts: Vec<Value> = rows.iter().map(|r| s(r.status.clone())).collect();
    let tips: Vec<Value> = rows.iter().map(|r| s(tip(r))).collect();
    let is_date = xs.iter().any(|v| matches!(v, Value::DateTime(_)));
    let data = vec![
        ("x".into(), xs),
        ("y".into(), ys),
        ("status".into(), sts),
        ("tip".into(), tips),
    ];
    let plot = GGPlot::new(data)
        .aes(Aes::new().x("x").y("y").color("status").label("tip"))
        .geom_point_with(GeomPoint {
            size: 4.0,
            color: o.brand(),
            alpha: 1.0,
        })
        .scale_color(
            ScaleColorDiscrete::new(Aesthetic::Color)
                .with_levels(levels.to_vec())
                .with_palette(palette.to_vec()),
        )
        .scale_y_continuous(stats::label_axis(&lanes));
    let xdefault = if is_date {
        "date".to_string()
    } else {
        time_key.map(look::humanize).unwrap_or_default()
    };
    let plot = labels.apply(look::styled(plot, o, false), &xdefault, "");
    let plot = if levels.len() < 2 {
        plot.legend_position(ggplot_rs::theme::LegendPosition::None)
    } else {
        plot
    };
    stats::finish(plot, w, h, place)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(json: &str) -> Vec<Row> {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn status_normalises() {
        assert_eq!(status_of("Pass"), "pass");
        assert_eq!(status_of(" FAILED "), "fail");
        assert_eq!(status_of("warning"), "warn");
        assert_eq!(status_of(" Weird "), "Weird");
    }

    #[test]
    fn mode_picks_band_or_strip() {
        let band_rows = rows(r#"[{"status":"pass","agg":"avg","value":69.5}]"#);
        let t = Table {
            rows: &band_rows,
            renamed: &[],
        };
        assert_eq!(mode(&t, &Row::new()).unwrap(), ("band", None));

        let strip_rows = rows(
            r#"[{"run_ts":"2026-10-01 00:00:00","status":"pass","value":1},
                {"run_ts":"2026-10-02 00:00:00","status":"pass","value":1}]"#,
        );
        let t = Table {
            rows: &strip_rows,
            renamed: &[],
        };
        assert_eq!(
            mode(&t, &Row::new()).unwrap(),
            ("strip", Some("run_ts".to_string()))
        );

        let opts: Row = serde_json::from_str(r#"{"kind":"nope"}"#).unwrap();
        assert!(mode(&t, &opts).is_err());
    }
}
