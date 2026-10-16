//! Chart motion directives (#39 ::ANIMATE, #42 ::LINESTYLE 'flow', #40
//! ::STORY, #41 ::TRANSITION): what the renderer hands the browser on the SVG.
use anofox_visualization::{render, Column, Kind, Role};
use ggplot_rs::prelude::Value;

fn strs(v: &[&str]) -> Vec<Value> {
    v.iter().map(|s| Value::Str(s.to_string())).collect()
}
fn nums(v: &[f64]) -> Vec<Value> {
    v.iter().map(|&f| Value::Float(f)).collect()
}
fn root(svg: &str) -> &str {
    &svg[..svg.find('>').unwrap()]
}
fn line(extra: Vec<Column>) -> String {
    let mut cols = vec![
        Column::new(
            "d",
            Role::X,
            strs(&["2026-10-05", "2026-10-12", "2026-10-19", "2026-10-26"]),
        ),
        Column::new(
            "s",
            Role::Category,
            strs(&["Observed", "Observed", "Forecast", "Forecast"]),
        ),
        Column::new(
            "v",
            Role::Value(Kind::Line),
            nums(&[10.0, 12.0, 11.0, 14.0]),
        ),
    ];
    cols.extend(extra);
    render(&cols, 480, 300).unwrap()
}

#[test]
fn animate_mode_rides_on_the_root() {
    let svg = line(vec![Column::new(
        "a",
        Role::Animate,
        strs(&["Grow", "Grow", "Grow", "Grow"]),
    )]);
    assert!(
        root(&svg).contains(r#"data-animate="grow""#),
        "{}",
        root(&svg)
    );
    // no directive, an unknown mode: nothing (the browser plays 'auto')
    assert!(!line(vec![]).contains("data-animate"));
    let bad = line(vec![Column::new("a", Role::Animate, strs(&["wobble"; 4]))]);
    assert!(!bad.contains("data-animate"));
}

#[test]
fn reveal_starts_at_a_date_xline() {
    let svg = line(vec![
        Column::new("t", Role::VLine, strs(&["2026-10-12"; 4])),
        Column::new("a", Role::Animate, strs(&["reveal"; 4])),
    ]);
    let r = root(&svg);
    assert!(r.contains(r#"data-animate="reveal""#), "{r}");
    // 2026-10-12 00:00 UTC in epoch seconds — the date axis' unit
    assert!(r.contains(r#"data-xline="1791763200""#), "{r}");
    // the date rule is drawn, and says its date rather than epoch seconds
    assert!(
        svg.contains("<title>2026-10-12</title>"),
        "date XLINE tooltip"
    );
    assert!(!svg.contains("x = 1791763200"));
}

#[test]
fn numeric_xline_still_draws() {
    let cols = vec![
        Column::new("x", Role::X, nums(&[1.0, 2.0, 3.0])),
        Column::new("v", Role::Value(Kind::Line), nums(&[1.0, 4.0, 2.0])),
        Column::new("t", Role::VLine, strs(&["2", "2", "2"])),
    ];
    let svg = render(&cols, 400, 300).unwrap();
    assert!(
        svg.contains("stroke-dasharray"),
        "the text-typed XLINE '2' is drawn"
    );
}

#[test]
fn flow_dashes_the_series_that_asks_for_it() {
    let svg = line(vec![Column::new(
        "ls",
        Role::LineStyle,
        vec![
            Value::Na,
            Value::Na,
            Value::Str("flow".into()),
            Value::Str("flow".into()),
        ],
    )]);
    let tags: Vec<&str> = svg
        .split("<polyline ")
        .skip(1)
        .map(|t| &t[..t.find('>').unwrap()])
        .collect();
    let fc: Vec<_> = tags
        .iter()
        .filter(|t| t.contains(r#"data-series="Forecast""#))
        .collect();
    let ob: Vec<_> = tags
        .iter()
        .filter(|t| t.contains(r#"data-series="Observed""#))
        .collect();
    assert!(!fc.is_empty() && !ob.is_empty());
    assert!(
        fc.iter()
            .all(|t| t.contains(r#"class="dp-flow""#) && t.contains(r#"stroke-dasharray="6 4""#)),
        "{fc:?}"
    );
    assert!(
        ob.iter()
            .all(|t| !t.contains("dp-flow") && !t.contains("stroke-dasharray")),
        "{ob:?}"
    );
    // grid lines are untouched
    assert_eq!(svg.matches("dp-flow").count(), fc.len());
}

#[test]
fn dashed_without_a_category_styles_the_one_line() {
    let cols = vec![
        Column::new("x", Role::X, nums(&[1.0, 2.0, 3.0, 4.0])),
        Column::new("v", Role::Value(Kind::Line), nums(&[1.0, 4.0, 2.0, 3.0])),
        Column::new("ls", Role::LineStyle, strs(&["dashed"; 4])),
    ];
    let svg = render(&cols, 400, 300).unwrap();
    assert_eq!(
        svg.matches(r#"stroke-dasharray="6 4""#).count(),
        1,
        "one data line dashed"
    );
    assert!(!svg.contains("dp-flow"));
}
