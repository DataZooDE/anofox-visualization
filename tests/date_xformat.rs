//! `::XFORMAT` with a strftime pattern on a date/timestamp x (#640).

use anofox_visualization::{render_with, Column, Kind, RenderOptions, Role};
use ggplot_rs::prelude::Value;

fn svg(cols: &[Column]) -> String {
    render_with(cols, 480, 320, &RenderOptions::default()).expect("renders")
}
fn konst(s: &str, n: usize) -> Vec<Value> {
    vec![Value::Str(s.to_string()); n]
}

/// One day of quarter-hours (2000-01-01 00:00 … 23:45).
fn quarter_hours(fmt: Option<&str>) -> Vec<Column> {
    let t0: i64 = 946_684_800; // 2000-01-01T00:00:00Z
    let n: i64 = 96;
    let mut cols = vec![
        Column::new(
            "t",
            Role::X,
            (0..n).map(|q| Value::DateTime(t0 + q * 900)).collect(),
        ),
        Column::new(
            "p",
            Role::Value(Kind::Line),
            (0..n).map(|q| Value::Float(q as f64)).collect(),
        ),
    ];
    if let Some(f) = fmt {
        cols.push(Column::new("f", Role::XFormat, konst(f, n as usize)));
    }
    cols
}

#[test]
fn a_time_pattern_labels_a_timestamp_axis() {
    let s = svg(&quarter_hours(Some("%H:%M")));
    assert!(
        s.contains(">06:00</text>") || s.contains(">12:00</text>"),
        "{s}"
    );
    assert!(!s.contains("%H"), "the pattern isn't printed as a prefix");
    // no date on a time-of-day axis's ticks (tooltips keep the full time)
    let ticks: Vec<&str> = s
        .split("</text>")
        .filter_map(|t| t.rsplit('>').next())
        .filter(|t| t.contains("2000"))
        .collect();
    assert!(ticks.is_empty(), "{ticks:?}");
}

#[test]
fn without_a_pattern_the_axis_is_unchanged() {
    let plain = svg(&quarter_hours(None));
    let dashed = svg(&quarter_hours(Some("x")));
    // a non-pattern ::XFORMAT on a date axis keeps today's behaviour
    assert!(!plain.contains(">06:00</text>"));
    assert!(!dashed.contains(">06:00</text>"));
}

#[test]
fn number_formats_still_apply_to_numeric_axes() {
    let n = 10;
    let s = svg(&[
        Column::new(
            "x",
            Role::X,
            (0..n).map(|i| Value::Float(i as f64)).collect(),
        ),
        Column::new(
            "v",
            Role::Value(Kind::Line),
            (0..n).map(|i| Value::Float(i as f64)).collect(),
        ),
        Column::new("f", Role::XFormat, konst(" kg", n)),
    ]);
    assert!(s.contains("kg</text>"), "{s}");
}
