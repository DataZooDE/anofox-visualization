//! #919: `::Y2` — a second measure on a right-hand axis of its own.

use anofox_visualization::{render_with, Column, Kind, RenderOptions, Role};
use ggplot_rs::prelude::Value;

fn s(v: &[&str]) -> Vec<Value> {
    v.iter().map(|x| Value::Str(x.to_string())).collect()
}
fn n(v: &[f64]) -> Vec<Value> {
    v.iter().map(|&f| Value::Float(f)).collect()
}
fn chart(y2: Option<&str>, fmt: Option<&str>) -> String {
    let months = ["Jan", "Feb", "Mar", "Apr"];
    let mut cols = vec![
        Column::new("month", Role::X, s(&months)),
        Column::new(
            "revenue",
            Role::Value(Kind::Bar),
            n(&[1200.0, 1500.0, 900.0, 2000.0]),
        ),
        Column::new(
            "margin_pct",
            Role::Value(Kind::Line),
            n(&[12.0, 18.0, 9.0, 24.0]),
        ),
    ];
    if let Some(y) = y2 {
        cols.push(Column::new("y2", Role::Y2, s(&[y; 4])));
    }
    if let Some(f) = fmt {
        cols.push(Column::new("f", Role::Y2Format, s(&[f; 4])));
    }
    render_with(&cols, 560, 320, &RenderOptions::default()).unwrap()
}
/// Every `<text>`'s content, trimmed.
fn texts(svg: &str) -> Vec<String> {
    svg.split("<text")
        .skip(1)
        .filter_map(|t| {
            let b = &t[t.find('>')? + 1..];
            Some(b[..b.find("</text>")?].trim().to_string())
        })
        .collect()
}

#[test]
fn the_named_measure_reads_off_a_right_axis_in_its_own_units() {
    let svg = chart(Some("margin_pct"), Some("percent"));
    let t = texts(&svg);
    // the right axis: the measure's name and nice percent ticks up to 24 %
    assert!(t.iter().any(|x| x == "margin_pct"), "{t:?}");
    assert!(t.iter().any(|x| x == "20%"), "{t:?}");
    // its marks keep their own values (hover, value filters), not the
    // rescaled ones they are drawn at
    assert!(svg.contains("data-value=\"24\""), "{svg}");
    assert!(svg.contains("data-vtext=\"24%\""), "{svg}");
    // without ::Y2 the line shares the left axis (no right title)
    let plain = chart(None, None);
    assert!(!texts(&plain).iter().any(|x| x == "margin_pct"));
}

#[test]
fn the_line_spans_the_bars_height() {
    // the largest margin is drawn at the largest revenue's height
    let svg = chart(Some("MARGIN_PCT"), None);
    let top_bar = svg
        .split("<rect ")
        .filter(|r| r.contains("data-x=\"Apr\"") && r.contains("gg-bar"))
        .find_map(|r| {
            let k = r.find(" y=\"")? + 4;
            r[k..k + r[k..].find('"')?].parse::<f64>().ok()
        })
        .expect("April's bar");
    let apr_point = svg
        .split("<circle ")
        .filter(|c| c.contains("data-x=\"Apr\""))
        .find_map(|c| {
            let k = c.find(" cy=\"")? + 5;
            c[k..k + c[k..].find('"')?].parse::<f64>().ok()
        })
        .expect("April's point");
    assert!(
        (top_bar - apr_point).abs() < 1.0,
        "bar top {top_bar}, point {apr_point}"
    );
}

#[test]
fn an_unknown_or_first_measure_changes_nothing() {
    let plain = chart(None, None);
    assert_eq!(chart(Some("nope"), None), plain);
    assert_eq!(chart(Some("revenue"), None), plain);
}

#[test]
fn a_line_combo_keeps_one_colour_per_series_and_names_the_right_one() {
    let cols = vec![
        Column::new("month", Role::X, s(&["Jan", "Feb", "Mar"])),
        Column::new("visits", Role::Value(Kind::Line), n(&[100.0, 300.0, 200.0])),
        Column::new(
            "conversion",
            Role::Value(Kind::Line),
            n(&[0.02, 0.05, 0.03]),
        ),
        Column::new("y2", Role::Y2, s(&["conversion"; 3])),
    ];
    let svg = render_with(&cols, 560, 320, &RenderOptions::default()).unwrap();
    let t = texts(&svg);
    assert!(
        t.iter().any(|x| x == "conversion (right axis)"),
        "legend: {t:?}"
    );
    let line = svg
        .split("<polyline ")
        .find(|p| p.contains("data-vtext"))
        .expect("the right-axis line");
    let stroke = |m: &str| m.split("stroke=\"").nth(1).map(|r| r[..7].to_string());
    let pt = svg
        .split("<circle ")
        .find(|c| c.contains("data-vtext=\"0.05\""))
        .expect("a point per value");
    assert_eq!(
        stroke(line),
        pt.split("fill=\"").nth(1).map(|r| r[..7].to_string())
    );
}
