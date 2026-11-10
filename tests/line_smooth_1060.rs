//! #1060: `units::LINECHART, units::SMOOTH` — a line and its trend — drew
//! bars: an extra measure of a kind the combo loop didn't know fell back to
//! a bar layer, and no trend was fitted.

use anofox_visualization::{render_with, Column, Kind, RenderOptions, Role};
use ggplot_rs::prelude::Value;

fn chart(method: Option<&str>) -> String {
    let n = 24;
    let x: Vec<Value> = (0..n).map(|i| Value::Float(i as f64)).collect();
    let y: Vec<Value> = (0..n)
        .map(|i| Value::Float(100.0 - 2.0 * i as f64 + (i % 3) as f64 * 5.0))
        .collect();
    let mut cols = vec![
        Column::new("m", Role::X, x),
        Column::new("units", Role::Value(Kind::Line), y.clone()),
        Column::new("units", Role::Value(Kind::Smooth), y),
    ];
    if let Some(m) = method {
        cols.push(Column::new(
            "",
            Role::SmoothMethod,
            vec![Value::Str(m.into()); n],
        ));
    }
    render_with(&cols, 480, 320, &RenderOptions::default()).expect("renders")
}

#[test]
fn a_line_with_its_trend_draws_a_line_and_a_fitted_trend_not_bars() {
    for m in [None, Some("lm"), Some("gam")] {
        let svg = chart(m);
        assert!(!svg.contains("gg-bar"), "{m:?}: no bars");
        assert!(
            svg.contains("data-geom=\"smooth\""),
            "{m:?}: a fitted trend"
        );
        assert!(svg.contains("data-geom=\"line\""), "{m:?}: the line");
    }
}
