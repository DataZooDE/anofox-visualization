//! Narrow charts keep every x tick label inside the SVG, and labels don't
//! overlap (#108). The fit/overlap logic itself now lives in ggplot-rs
//! (`Theme::axis_text_x_fit`, `axis_text_x_check_overlap`, turned on in the
//! house theme); this just checks the rendered result.
mod common;
use anofox_visualization::{render, Column, Kind, Role};
use common::tick_labels::{x_tick_labels, TickLabel};
use ggplot_rs::prelude::Value;

/// 2026-01-05, a Monday.
const T0: i64 = 1_767_571_200;

fn charts() -> Vec<(&'static str, Vec<Column>)> {
    let weekly: Vec<Value> = (0..52)
        .map(|i| Value::DateTime(T0 + i * 7 * 86_400))
        .collect();
    let daily: Vec<Value> = (0..30).map(|i| Value::DateTime(T0 + i * 86_400)).collect();
    let month_ends: Vec<Value> = (1..=12)
        .map(|m| Value::Str(format!("2026-{m:02}-28")))
        .collect();
    let regions: Vec<Value> = [
        "North America",
        "Western Europe",
        "Southeast Asia",
        "Latin America",
        "Middle East & Africa",
        "Oceania",
    ]
    .iter()
    .map(|s| Value::Str(s.to_string()))
    .collect();
    let ramp = |n: usize, k: f64| (0..n).map(|i| Value::Float(i as f64 * k)).collect();
    vec![
        (
            "weekly date x",
            vec![
                Column::new("week", Role::X, weekly),
                Column::new("sales", Role::Value(Kind::Line), ramp(52, 1234.5)),
            ],
        ),
        (
            "daily date x",
            vec![
                Column::new("day", Role::X, daily),
                Column::new("calls", Role::Value(Kind::Area), ramp(30, 3.0)),
            ],
        ),
        (
            "date-string x",
            vec![
                Column::new("month", Role::X, month_ends),
                Column::new("n", Role::Value(Kind::Bar), ramp(12, 10.0)),
            ],
        ),
        (
            "long-category x",
            vec![
                Column::new("region", Role::X, regions),
                Column::new("revenue", Role::Value(Kind::Bar), ramp(6, 1e6)),
            ],
        ),
        (
            "numeric x",
            vec![
                Column::new("price", Role::X, ramp(50, 2500.0)),
                Column::new("units", Role::Value(Kind::Point), ramp(50, 1.0)),
            ],
        ),
    ]
}

fn inside(l: &TickLabel) -> bool {
    let [x0, y0, x1, y1] = l.bbox();
    // the SVG's numbers carry two decimals
    let e = 0.01;
    x0 >= -e && y0 >= -e && x1 <= l.chart_width + e && y1 <= l.chart_height + e
}

#[test]
fn narrow_charts_keep_tick_labels_inside_and_apart() {
    for (name, cols) in charts() {
        for w in [280, 320, 390] {
            let svg = render(&cols, w, 260).unwrap();
            let labels = x_tick_labels(&svg);
            assert!(labels.len() >= 2, "{name} @ {w}: {} x labels", labels.len());
            for l in &labels {
                assert!(
                    inside(l),
                    "{name} @ {w}: {:?} {:?} outside",
                    l.text,
                    l.bbox()
                );
            }
            for p in labels.windows(2) {
                assert!(
                    !p[0].overlaps(&p[1]),
                    "{name} @ {w}: {:?} overlaps {:?}",
                    p[0].text,
                    p[1].text
                );
            }
        }
    }
}

/// A wide chart whose labels fit is left alone: every category is named.
#[test]
fn wide_charts_keep_every_label() {
    for (name, cols) in charts() {
        let svg = render(&cols, 960, 320).unwrap();
        let labels = x_tick_labels(&svg);
        assert!(labels.iter().all(inside), "{name}");
        if matches!(cols[0].values[0], Value::Str(_)) {
            assert_eq!(
                labels.len(),
                cols[0].values.len(),
                "{name}: a label per category"
            );
        }
    }
}
