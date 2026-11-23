//! #1359: a ::TREEMAP colours its tiles by a separate measure (a numeric
//! ::COLORSCALE / _HIGH_GOOD / _DIVERGING column, with a colour bar) and
//! honours a ::COLORS level map for its ::CATEGORY groups.

use anofox_visualization::{render, Column, Kind, Role, Scale};
use ggplot_rs::prelude::Value;

fn strs(v: &[&str]) -> Vec<Value> {
    v.iter().map(|s| Value::Str(s.to_string())).collect()
}
fn nums(v: &[f64]) -> Vec<Value> {
    v.iter().map(|&f| Value::Float(f)).collect()
}
/// The fill of the tile labelled `x` (`data-x`).
fn tile_fill(svg: &str, x: &str) -> String {
    let at = svg
        .find(&format!("data-x=\"{x}\""))
        .unwrap_or_else(|| panic!("no tile {x}: {svg}"));
    let rect = &svg[svg[..at].rfind("<rect").unwrap()..at];
    let f = &rect[rect.find("fill=\"").unwrap() + 6..];
    f[..f.find('"').unwrap()].to_string()
}
fn units() -> Vec<Column> {
    vec![
        Column::new("unit", Role::X, strs(&["C01", "C02", "C06", "C11"])),
        Column::new(
            "year_end",
            Role::Value(Kind::Treemap),
            nums(&[2038614.0, 787795.0, 372133.0, 666076.0]),
        ),
    ]
}

#[test]
fn a_colorscale_column_colours_each_tile_by_its_value() {
    let mut cols = units();
    cols.push(Column::new(
        "vs_py_pct",
        Role::ColorScale(Scale::Diverging),
        nums(&[4.5, -2.1, 2.8, -9.0]),
    ));
    let svg = render(&cols, 480, 320).expect("renders");
    let fills: Vec<String> = ["C01", "C02", "C06", "C11"]
        .iter()
        .map(|x| tile_fill(&svg, x))
        .collect();
    // four different values, four different colours; the two below 0 share
    // a hue (red channel dominant), the two above another
    let red = |h: &str| {
        u8::from_str_radix(&h[1..3], 16).unwrap() > u8::from_str_radix(&h[3..5], 16).unwrap()
    };
    assert!(
        !red(&fills[0]) && red(&fills[1]) && !red(&fills[2]) && red(&fills[3]),
        "{fills:?}"
    );
    let distinct: std::collections::HashSet<_> = fills.iter().collect();
    assert_eq!(distinct.len(), 4, "{fills:?}");
    // a colour bar
    assert!(
        svg.contains("linearGradient") || svg.matches("<rect").count() > 20,
        "no colour bar: {svg}"
    );
}

#[test]
fn colors_pins_the_group_colours_whatever_the_row_order() {
    let mut cols = units();
    cols.push(Column::new(
        "band",
        Role::Category,
        strs(&["ahead", "behind", "ahead", "behind"]),
    ));
    cols.push(Column::new(
        "c",
        Role::GaugeColors,
        strs(&["behind:#d0456f; ahead:#1f8a80"]),
    ));
    let svg = render(&cols, 480, 320).expect("renders");
    assert_eq!(tile_fill(&svg, "C01").to_uppercase(), "#1F8A80");
    assert_eq!(tile_fill(&svg, "C02").to_uppercase(), "#D0456F");
    // the same colours when "behind" comes first
    let mut rev = units();
    rev.push(Column::new(
        "band",
        Role::Category,
        strs(&["behind", "behind", "ahead", "ahead"]),
    ));
    rev.push(Column::new(
        "c",
        Role::GaugeColors,
        strs(&["behind:#d0456f; ahead:#1f8a80"]),
    ));
    let svg = render(&rev, 480, 320).expect("renders");
    assert_eq!(tile_fill(&svg, "C06").to_uppercase(), "#1F8A80");
    assert_eq!(tile_fill(&svg, "C01").to_uppercase(), "#D0456F");
}
