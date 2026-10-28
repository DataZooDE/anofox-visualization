//! `::XTITLE` / `::YTITLE` (#651): axis titles on charts with axes.

use anofox_visualization::{render_with, Column, Kind, RenderOptions, Role};
use ggplot_rs::prelude::Value;

fn strs(v: &[&str]) -> Vec<Value> {
    v.iter().map(|s| Value::Str(s.to_string())).collect()
}
fn nums(v: &[f64]) -> Vec<Value> {
    v.iter().map(|&f| Value::Float(f)).collect()
}
fn konst(s: &str, n: usize) -> Vec<Value> {
    vec![Value::Str(s.to_string()); n]
}
fn svg(cols: &[Column]) -> String {
    render_with(cols, 480, 320, &RenderOptions::default()).expect("renders")
}
/// The text elements whose content is exactly `t`.
fn texts<'a>(s: &'a str, t: &str) -> Vec<&'a str> {
    s.match_indices(&format!(">{t}</text>"))
        .map(|(i, _)| &s[s[..i].rfind("<text").unwrap()..i])
        .collect()
}

#[test]
fn a_scatter_gets_both_titles_the_y_one_rotated() {
    let s = svg(&[
        Column::new("rl", Role::X, nums(&[10.0, 20.0, 30.0])),
        Column::new("p", Role::Value(Kind::Point), nums(&[50.0, 80.0, 120.0])),
        Column::new("xt", Role::XTitle, konst("Residual load (GW)", 3)),
        Column::new("yt", Role::YTitle, konst("Price (EUR/MWh)", 3)),
    ]);
    assert_eq!(texts(&s, "Residual load (GW)").len(), 1, "{s}");
    let y = texts(&s, "Price (EUR/MWh)");
    assert_eq!(y.len(), 1);
    assert!(y[0].contains("rotate("), "{}", y[0]);
}

#[test]
fn no_title_roles_no_axis_titles() {
    let cols = [
        Column::new("r", Role::X, strs(&["North", "South"])),
        Column::new("v", Role::Value(Kind::Bar), nums(&[1.0, 2.0])),
    ];
    let plain = svg(&cols);
    let mut with = cols.to_vec();
    with.push(Column::new("xt", Role::XTitle, konst("Region", 2)));
    let titled = svg(&with);
    assert!(texts(&plain, "Region").is_empty());
    assert_eq!(texts(&titled, "Region").len(), 1);
}

#[test]
fn a_flipped_chart_moves_the_x_title_with_its_axis() {
    let s = svg(&[
        Column::new("r", Role::X, strs(&["North", "South"])),
        Column::new("v", Role::Value(Kind::Bar), nums(&[1.0, 2.0])),
        Column::new("f", Role::Flip, nums(&[1.0, 1.0])),
        Column::new("xt", Role::XTitle, konst("Region", 2)),
    ]);
    let t = texts(&s, "Region");
    assert_eq!(t.len(), 1);
    assert!(t[0].contains("rotate("), "the x axis is vertical: {}", t[0]);
}

#[test]
fn charts_without_axes_ignore_them() {
    let s = svg(&[
        Column::new("r", Role::Category, strs(&["a", "b"])),
        Column::new("v", Role::Value(Kind::Pie), nums(&[1.0, 2.0])),
        Column::new("xt", Role::XTitle, konst("Region", 2)),
    ]);
    assert!(texts(&s, "Region").is_empty());
}
