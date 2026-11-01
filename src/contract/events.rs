//! `events`: point-in-time events over a time or period axis — a twin's
//! alarms, parameter changes, calibration proposals (`ds | period, kind,
//! label, severity, value, threshold, model_id, series | id`). Drawn on their
//! own as one lane per series (else per kind) by `anofox_plot_events`, or over a
//! forecast as vertical lines by `anofox_plot_prediction(p, events := …)`.

use super::look::{self, Labels};
use super::stats::{self, fin, fmt_num, s, text_of, Out};
use super::{names, Row, Table};
use crate::{Place, RenderOptions};
use ggplot_rs::prelude::*;
use serde_json::Value as J;

/// The x keys an event row may carry, in order of preference.
const KEYS: &[&str] = &["ds", "period", "x"];

/// The colour of a severity: alarms in the theme's negative red, warnings in
/// its amber, anything else muted.
fn severity_color(sev: &str, o: &RenderOptions) -> (u8, u8, u8) {
    match sev.to_ascii_lowercase().as_str() {
        "alarm" | "critical" | "error" | "high" => o.theme.colors.negative,
        "warning" | "warn" | "medium" => o.theme.colors.warning,
        _ => o.theme.colors.muted,
    }
}

/// Severities from the most to the least urgent; others follow as seen.
fn severity_rank(sev: &str) -> usize {
    match sev.to_ascii_lowercase().as_str() {
        "alarm" | "critical" | "error" | "high" => 0,
        "warning" | "warn" | "medium" => 1,
        _ => 2,
    }
}

/// One event's hover text: `alarm · stockout: on hand 3 (threshold 5)`.
fn tip(t: &Table, i: usize) -> String {
    let get = |k: &str| t.rows[i].get(k).filter(|v| !v.is_null());
    let txt = |k: &str| {
        get(k).map(|v| match v {
            J::String(s) => s.clone(),
            v => v.to_string(),
        })
    };
    let num = |k: &str| get(k).and_then(|v| v.as_f64().or_else(|| v.as_str()?.parse().ok()));
    let head: Vec<String> = ["severity", "kind"]
        .iter()
        .filter_map(|k| txt(k))
        .chain(txt("series").or_else(|| txt("id")))
        .collect();
    let mut body = txt("label").unwrap_or_default();
    if let Some(v) = num("value") {
        body = format!("{body} {}", fmt_num(v)).trim().to_string();
    }
    if let Some(th) = num("threshold") {
        body = format!("{body} (threshold {})", fmt_num(th))
            .trim()
            .to_string();
    }
    match (head.is_empty(), body.is_empty()) {
        (true, true) => "event".into(),
        (true, false) => body,
        (false, true) => head.join(" · "),
        (false, false) => format!("{}: {body}", head.join(" · ")),
    }
}

/// The rows of an `events :=` argument: a LIST of event STRUCTs (or its JSON
/// text), read like any contract table.
fn rows_of(v: &J) -> Result<Vec<Row>, String> {
    let parsed;
    let v = match v {
        J::String(text) => {
            parsed = serde_json::from_str::<J>(text)
                .map_err(|_| "events := expects a LIST of event STRUCTs".to_string())?;
            &parsed
        }
        v => v,
    };
    let items: Vec<Row> = match v {
        J::Array(a) => a.iter().filter_map(J::as_object).cloned().collect(),
        J::Object(o) => vec![o.clone()],
        _ => return Err("events := expects a LIST of event STRUCTs".into()),
    };
    let mut renamed = Vec::new();
    Ok(names::normalize(&items, &Row::new(), &mut renamed).unwrap_or(items))
}

/// Draw `events` over a prediction plot whose x column is `x`: a vertical
/// line per event in its severity's colour, its text on hover. With panels
/// (`facet`: the column and its per-row values), an event carrying that
/// column (or `series` / `id`) is drawn in its own panel, else in every one. Returns
/// the plot and a caption naming the colours.
pub(super) fn overlay(
    mut plot: GGPlot,
    events: &J,
    x: &str,
    is_date: bool,
    facet: Option<(&str, &[Value])>,
    o: &RenderOptions,
) -> Result<(GGPlot, Option<String>), String> {
    let rows = rows_of(events)?;
    if rows.is_empty() {
        return Ok((plot, None));
    }
    let renamed = Vec::new();
    let t = Table {
        rows: &rows,
        renamed: &renamed,
    };
    let key = std::iter::once(x)
        .chain(KEYS.iter().copied())
        .find(|k| t.has(k))
        .ok_or_else(|| {
            format!(
                "events := needs a `{x}` (or ds / period) column like the prediction (got: {})",
                t.keys()
            )
        })?;
    let xs = t.xpos(key);
    let dated = xs.iter().any(|v| matches!(v, Value::DateTime(_)));
    if dated != is_date {
        return Err(format!(
            "events := `{key}` is {} but the prediction's x (`{x}`) is {}",
            if dated { "a date" } else { "not a date" },
            if is_date { "a date" } else { "not" }
        ));
    }
    let sev = t.text("severity");
    // Panels: the event's own panel, else every panel.
    let panels: Vec<String> = facet
        .map(|(_, v)| {
            let mut l: Vec<String> = Vec::new();
            for x in v.iter().filter_map(text_of) {
                if !l.contains(&x) {
                    l.push(x);
                }
            }
            l
        })
        .unwrap_or_default();
    let own = facet
        .and_then(|(f, _)| [f, "series", "id"].into_iter().find(|k| t.has(k)))
        .map(|k| t.text(k));
    // Per severity: the number of events and their lines (one per panel).
    type Lines = Vec<(Value, String, Option<String>)>;
    let mut groups: Vec<(String, usize, Lines)> = Vec::new();
    for i in 0..rows.len() {
        if fin(&xs[i]).is_none() {
            continue;
        }
        let sv = text_of(&sev[i]).unwrap_or_default();
        let at: Vec<Option<String>> = if panels.is_empty() {
            vec![None]
        } else {
            match own.as_ref().and_then(|o| text_of(&o[i])) {
                Some(p) if panels.contains(&p) => vec![Some(p)],
                _ => panels.iter().cloned().map(Some).collect(),
            }
        };
        let g = match groups.iter_mut().find(|g| g.0 == sv) {
            Some(g) => g,
            None => {
                groups.push((sv.clone(), 0, Vec::new()));
                groups.last_mut().expect("just pushed")
            }
        };
        g.1 += 1;
        for p in at {
            g.2.push((xs[i].clone(), tip(&t, i), p));
        }
    }
    groups.sort_by_key(|g| severity_rank(&g.0));
    let mut note = Vec::new();
    for (sv, n, marks) in &groups {
        let mut frame: Vec<(String, Vec<Value>)> = vec![
            ("x".into(), marks.iter().map(|m| m.0.clone()).collect()),
            ("tip".into(), marks.iter().map(|m| s(m.1.clone())).collect()),
        ];
        if !panels.is_empty() {
            frame.push((
                "facet".into(),
                marks
                    .iter()
                    .map(|m| m.2.clone().map_or(Value::Na, Value::Str))
                    .collect(),
            ));
        }
        plot = plot
            .geom_vline_aes_with(
                GeomVline {
                    xintercept: f64::NAN,
                    color: severity_color(sv, o),
                    width: 1.2,
                    linetype: Linetype::Solid,
                    alpha: 0.7,
                },
                Aes::new().xintercept("x").group("tip"),
            )
            .layer_data(frame);
        let name = if sv.is_empty() { "event" } else { sv.as_str() };
        let colour = match severity_rank(sv) {
            0 => "red",
            1 => "amber",
            _ => "grey",
        };
        note.push(format!("{n} {name} ({colour})"));
    }
    Ok((plot, Some(format!("events: {}", note.join(", ")))))
}

/// `anofox_plot_events(tbl)`: one lane per series (else per kind), a dot
/// per event at its ds / period coloured by severity, its label, value and
/// threshold on hover.
pub(super) fn events(
    t: &Table,
    options: &Row,
    w: u32,
    h: u32,
    o: &RenderOptions,
    place: Place,
) -> Out {
    let labels = Labels::from(options);
    let key = super::opt_str(options, "x")
        .or_else(|| KEYS.iter().find(|k| t.has(k)).map(|k| k.to_string()))
        .ok_or_else(|| {
            format!(
                "needs an event time: a column `ds` or `period`, or x := '<column>' (got: {})",
                t.keys()
            )
        })?;
    t.need(&[key.as_str()])?;
    let xs = t.xpos(&key);
    // `id` is the series key of anofox_forecast's events (ts_*_events_by).
    let series = if t.has("series") { "series" } else { "id" };
    let lane_key = [series, "kind", "model_id"]
        .into_iter()
        .find(|k| t.distinct(k) > 1)
        .or_else(|| ["kind", series].into_iter().find(|k| t.has(k)));
    let lane_v: Vec<String> = match lane_key {
        Some(k) => t
            .text(k)
            .iter()
            .map(|v| text_of(v).unwrap_or_else(|| "(none)".into()))
            .collect(),
        None => vec!["events".into(); t.rows.len()],
    };
    let mut lanes: Vec<String> = Vec::new();
    for l in &lane_v {
        if !lanes.contains(l) {
            lanes.push(l.clone());
        }
    }
    let sev: Vec<String> = t
        .text("severity")
        .iter()
        .map(|v| text_of(v).unwrap_or_default())
        .collect();
    let mut sevs: Vec<String> = Vec::new();
    for v in &sev {
        if !sevs.contains(v) {
            sevs.push(v.clone());
        }
    }
    sevs.sort_by_key(|v| severity_rank(v));
    let keep: Vec<usize> = (0..t.rows.len())
        .filter(|&i| fin(&xs[i]).is_some())
        .collect();
    let n = lanes.len();
    let ys: Vec<Value> = keep
        .iter()
        .map(|&i| {
            let at = lanes.iter().position(|l| *l == lane_v[i]).unwrap_or(0);
            Value::Float(stats::slot(at, n))
        })
        .collect();
    let named_sev = |v: &str| if v.is_empty() { "event" } else { v }.to_string();
    let data: Vec<(String, Vec<Value>)> = vec![
        ("x".into(), keep.iter().map(|&i| xs[i].clone()).collect()),
        ("y".into(), ys),
        (
            "severity".into(),
            keep.iter().map(|&i| s(named_sev(&sev[i]))).collect(),
        ),
        ("tip".into(), keep.iter().map(|&i| s(tip(t, i))).collect()),
    ];
    let levels: Vec<String> = sevs.iter().map(|v| named_sev(v)).collect();
    let palette = sevs
        .iter()
        .map(|v| {
            let (r, g, b) = severity_color(v, o);
            RGBAColor::new(r, g, b)
        })
        .collect();
    let plot = GGPlot::new(data)
        .aes(Aes::new().x("x").y("y").color("severity").label("tip"))
        .geom_point_with(GeomPoint {
            size: 3.2,
            color: o.brand(),
            alpha: 0.9,
            ..Default::default()
        })
        .scale_color(
            ScaleColorDiscrete::new(Aesthetic::Color)
                .with_levels(levels)
                .with_palette(palette),
        )
        .scale_y_continuous(stats::label_axis(&lanes));
    let xdefault = match key.as_str() {
        "ds" if xs.iter().any(|v| matches!(v, Value::DateTime(_))) => "date".to_string(),
        k => look::humanize(k),
    };
    // The lanes are labelled: no y title by default (it would sit on the
    // labels), as in the coefficient and test forests; ylab := still sets one.
    let plot = labels.apply(look::styled(plot, o, false), &xdefault, "");
    let plot = if sevs.len() < 2 {
        plot.legend_position(ggplot_rs::theme::LegendPosition::None)
    } else {
        plot
    };
    stats::finish(plot, w, h, place)
}
