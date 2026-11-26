//! #1374: a band around one series of a chart with a `::COLORS` level map
//! took the series' *palette* colour (its sorted position), not the colour
//! the map pins it to — so the chain's band came out in another series'
//! colour.

use anofox_visualization::{render_with_warnings, Column, Kind, RenderOptions, Role};
use ggplot_rs::prelude::Value;

#[test]
fn a_band_takes_its_series_mapped_colour() {
    let days = 6;
    let mut x = Vec::new();
    let mut cat = Vec::new();
    let mut y = Vec::new();
    let mut lo = Vec::new();
    let mut hi = Vec::new();
    for (series, base, band) in [
        ("Actual", 30.0, false),
        ("Chain", 32.0, true),
        ("Shortcut", 20.0, false),
    ] {
        for i in 0..days {
            x.push(Value::DateTime(1_700_000_000 + i * 86_400));
            cat.push(Value::Str(series.into()));
            y.push(Value::Float(base + i as f64));
            lo.push(if band {
                Value::Float(base + i as f64 - 3.0)
            } else {
                Value::Na
            });
            hi.push(if band {
                Value::Float(base + i as f64 + 3.0)
            } else {
                Value::Na
            });
        }
    }
    let n = x.len();
    let cols = vec![
        Column::new("day", Role::X, x),
        Column::new("method", Role::Category, cat),
        Column::new("parcels", Role::Value(Kind::Line), y),
        Column::new("lo", Role::BandLower, lo),
        Column::new("hi", Role::BandUpper, hi),
        Column::new(
            "colors",
            Role::GaugeColors,
            vec![Value::Str("Actual:neutral; Chain:#2563eb; Shortcut:#d97706".into()); n],
        ),
    ];
    let r = render_with_warnings(&cols, 480, 320, &RenderOptions::default()).expect("renders");
    let ribbon = r
        .svg
        .split('<')
        .find(|t| t.contains("data-geom=\"ribbon\"") && t.contains("fill="))
        .expect("a band is drawn");
    assert!(
        ribbon.to_ascii_lowercase().contains("#2563eb"),
        "the band is the chain's colour: {ribbon}"
    );
}

/// #1374: `'end'::LEGEND` names each line at its end (ggplot-rs
/// `direct_labels`) instead of a legend: the label is drawn in its line's
/// colour.
#[test]
fn end_legend_names_each_line_at_its_end() {
    let mut x = Vec::new();
    let mut cat = Vec::new();
    let mut y = Vec::new();
    for (series, base) in [("Chain", 30.0), ("Shortcut", 20.0)] {
        for i in 0..6 {
            x.push(Value::Float(i as f64));
            cat.push(Value::Str(series.into()));
            y.push(Value::Float(base + i as f64));
        }
    }
    let n = x.len();
    let cols = vec![
        Column::new("day", Role::X, x),
        Column::new("method", Role::Category, cat),
        Column::new("parcels", Role::Value(Kind::Line), y),
        Column::new(
            "colors",
            Role::GaugeColors,
            vec![Value::Str("Chain:#2563eb; Shortcut:#d97706".into()); n],
        ),
        Column::new("legend", Role::Legend, vec![Value::Str("end".into()); n]),
    ];
    let r = render_with_warnings(&cols, 480, 320, &RenderOptions::default()).expect("renders");
    let label = r
        .svg
        .split("<text")
        .map(|t| t.split("</text>").next().unwrap_or(""))
        .find(|t| t.ends_with(">Chain"))
        .expect("a label for Chain");
    assert!(
        label.to_ascii_lowercase().contains("#2563eb"),
        "drawn in the line's colour: {label}"
    );
    // a phone-width chart (ggplot-rs 0.67.1): the labels' band would squeeze
    // the panel, so the usual legend goes below and its text is ink
    let r = render_with_warnings(&cols, 290, 320, &RenderOptions::default()).expect("renders");
    let label = r
        .svg
        .split("<text")
        .map(|t| t.split("</text>").next().unwrap_or(""))
        .find(|t| t.ends_with(">Chain"))
        .expect("a legend entry for Chain");
    assert!(
        !label.to_ascii_lowercase().contains("#2563eb"),
        "narrow: a legend, not a label in the line's colour: {label}"
    );
}
