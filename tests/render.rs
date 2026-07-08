use duckplot::{render, Column, Kind, Role, parse_role};
use ggplot_rs::prelude::Value;

fn strs(v: &[&str]) -> Vec<Value> { v.iter().map(|s| Value::Str(s.to_string())).collect() }
fn nums(v: &[f64]) -> Vec<Value> { v.iter().map(|&f| Value::Float(f)).collect() }

#[test]
fn parses_shaper_roles() {
    assert_eq!(parse_role("XAXIS"), Some(Role::X));
    assert_eq!(parse_role("category"), Some(Role::Category));
    assert_eq!(parse_role("BARCHART_STACKED"), Some(Role::Value(Kind::BarStacked)));
    assert_eq!(parse_role("linechart"), Some(Role::Value(Kind::Line)));
    assert_eq!(parse_role("nope"), None);
}

#[test]
fn renders_stacked_bar_by_category() {
    // SELECT week::XAXIS, category::CATEGORY, count()::BARCHART_STACKED
    let cols = vec![
        Column::new("week", Role::X, strs(&["W1","W1","W2","W2"])),
        Column::new("category", Role::Category, strs(&["a","b","a","b"])),
        Column::new("n", Role::Value(Kind::BarStacked), nums(&[3.0, 5.0, 7.0, 2.0])),
        Column::new("t", Role::Label, strs(&["Sessions per Week"])),
    ];
    let svg = render(&cols, 480, 320).unwrap();
    assert!(svg.contains("<svg") && svg.matches("<rect").count() >= 4, "stacked bars drawn");
    assert!(svg.contains("Sessions per Week"), "title from LABEL");
}

#[test]
fn renders_line_and_heading() {
    let line = vec![
        Column::new("t", Role::X, nums(&[0.0,1.0,2.0,3.0])),
        Column::new("y", Role::Value(Kind::Line), nums(&[1.0,3.0,2.0,5.0])),
    ];
    assert!(render(&line, 400, 260).unwrap().contains("<polyline"));

    let heading = vec![Column::new("t", Role::Label, strs(&["Overview"]))];
    assert!(render(&heading, 400, 40).unwrap().contains("Overview"));
}
