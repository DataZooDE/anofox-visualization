//! #1061: a history + forecast result — history rows with a value and no
//! band, forecast rows with a second measure and the band — drew both lines
//! but no band: the band layer inherited the chart's `y` (the history
//! measure), missing on every forecast row, so all of them were dropped.

use anofox_visualization::{render_with_warnings, Column, Kind, RenderOptions, Role};
use ggplot_rs::prelude::Value;

#[test]
fn the_band_around_a_forecast_is_drawn() {
    let x: Vec<Value> = (0..12)
        .map(|i| Value::DateTime(1_700_000_000 + i * 604_800))
        .collect();
    let some = |v: &[Option<f64>]| {
        v.iter()
            .map(|o| o.map_or(Value::Na, Value::Float))
            .collect::<Vec<_>>()
    };
    let hist: Vec<Option<f64>> = (0..12)
        .map(|i| (i < 8).then_some(30.0 + i as f64))
        .collect();
    let fc: Vec<Option<f64>> = (0..12)
        .map(|i| (i >= 8).then_some(40.0 + i as f64))
        .collect();
    let lo: Vec<Option<f64>> = fc.iter().map(|v| v.map(|f| f - 5.0)).collect();
    let hi: Vec<Option<f64>> = fc.iter().map(|v| v.map(|f| f + 5.0)).collect();
    let cols = vec![
        Column::new("week", Role::X, x),
        Column::new("Actual", Role::Value(Kind::Line), some(&hist)),
        Column::new("Forecast", Role::Value(Kind::Line), some(&fc)),
        Column::new("lo", Role::BandLower, some(&lo)),
        Column::new("hi", Role::BandUpper, some(&hi)),
    ];
    let r = render_with_warnings(&cols, 480, 320, &RenderOptions::default()).expect("renders");
    assert!(r.svg.contains("data-geom=\"ribbon\""), "a band is drawn");
    assert!(
        !r.warnings.iter().any(|w| w.contains("geom_ribbon")),
        "no band rows dropped: {:?}",
        r.warnings
    );
}
