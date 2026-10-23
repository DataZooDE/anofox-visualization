//! `::LEGEND` (#407 part 1): hide or move a chart's legend / colourbar.
use anofox_visualization::{render, Column, Kind, Role};
use ggplot_rs::prelude::Value;

fn strs(v: &[&str]) -> Vec<Value> {
    v.iter().map(|s| Value::Str(s.to_string())).collect()
}
fn nums(v: &[f64]) -> Vec<Value> {
    v.iter().map(|&f| Value::Float(f)).collect()
}

/// SELECT w::XAXIS, s::CATEGORY, n::BARCHART [, l::LEGEND] FROM t;
fn bar_by_category(legend: Option<&str>) -> String {
    let mut cols = vec![
        Column::new("w", Role::X, strs(&["W1", "W1", "W2", "W2"])),
        Column::new("s", Role::Category, strs(&["a", "b", "a", "b"])),
        Column::new("n", Role::Value(Kind::Bar), nums(&[3.0, 5.0, 7.0, 2.0])),
    ];
    if let Some(l) = legend {
        cols.push(Column::new("l", Role::Legend, strs(&[l])));
    }
    render(&cols, 480, 320).unwrap()
}

/// SELECT w::XAXIS, c::YAXIS, v::HEATMAP [, l::LEGEND] FROM t;
fn heatmap(legend: Option<&str>) -> String {
    let mut cols = vec![
        Column::new("w", Role::X, strs(&["a", "b", "c"])),
        Column::new("c", Role::Y, strs(&["r", "r", "r"])),
        Column::new("v", Role::Value(Kind::Heatmap), nums(&[10.0, 30.0, 60.0])),
    ];
    if let Some(l) = legend {
        cols.push(Column::new("l", Role::Legend, strs(&[l])));
    }
    render(&cols, 480, 320).unwrap()
}

#[test]
fn no_role_leaves_a_categorised_bar_chart_unchanged() {
    assert_eq!(bar_by_category(None), bar_by_category(None));
}

#[test]
fn no_role_leaves_a_heatmap_unchanged() {
    assert_eq!(heatmap(None), heatmap(None));
}

#[test]
fn none_removes_a_categorised_bar_charts_legend() {
    let with = bar_by_category(None);
    let without = bar_by_category(Some("none"));
    // The category swatches (one `<rect>` per level) and their names go.
    assert!(with.contains(">a<") && with.contains(">b<"), "{with}");
    assert!(
        !without.contains(">a<") && !without.contains(">b<"),
        "{without}"
    );
    assert!(
        without.matches("<rect").count() < with.matches("<rect").count(),
        "fewer rects without a legend: {without}"
    );
}

#[test]
fn none_removes_a_heatmaps_colourbar() {
    let with = heatmap(None);
    let without = heatmap(Some("none"));
    // The colourbar is drawn as ~50 thin `<rect>` slices plus a border; a
    // 3-cell heatmap without one is only the 3 tiles.
    let with_rects = with.matches("<rect").count();
    let without_rects = without.matches("<rect").count();
    assert_eq!(without_rects, 3, "{without}");
    assert!(
        with_rects > without_rects + 20,
        "with={with_rects} without={without_rects}"
    );
}

#[test]
fn bottom_moves_a_categorised_bar_charts_legend() {
    let top = bar_by_category(None);
    let bottom = bar_by_category(Some("bottom"));
    assert_ne!(top, bottom);
    // Still drawn — same category swatches/names — just not at the top:
    // the plot area's own y0 (in `data-plot`, "x0 y0 x1 y1") moves up once
    // the legend no longer sits above it.
    assert!(bottom.contains(">a<") && bottom.contains(">b<"), "{bottom}");
    let plot_rect = |svg: &str| -> [f64; 4] {
        let i = svg.find("data-plot=\"").unwrap() + "data-plot=\"".len();
        let s = &svg[i..];
        let s = &s[..s.find('"').unwrap()];
        let mut it = s.split_whitespace().map(|n| n.parse().unwrap());
        [
            it.next().unwrap(),
            it.next().unwrap(),
            it.next().unwrap(),
            it.next().unwrap(),
        ]
    };
    let [_, top_y0, _, _] = plot_rect(&top);
    let [_, bottom_y0, _, _] = plot_rect(&bottom);
    assert!(
        bottom_y0 < top_y0,
        "the panel grows upward once the legend moves off the top: top={top_y0} bottom={bottom_y0}"
    );
}

#[test]
fn an_unknown_value_is_ignored_the_legend_stays_put() {
    assert_eq!(bar_by_category(None), bar_by_category(Some("sideways")));
}

#[test]
fn case_insensitive() {
    assert_eq!(bar_by_category(Some("none")), bar_by_category(Some("NONE")));
    assert_eq!(
        bar_by_category(Some("bottom")),
        bar_by_category(Some("Bottom"))
    );
}

/// SELECT g::MAP, v::BARCHART [, l::LEGEND] FROM t; — a continuous
/// choropleth, which always draws a gradient legend (`src/map`'s own
/// hand-rolled box, not a ggplot-rs one — see `map::legend_override`).
fn choropleth(legend: Option<&str>) -> String {
    let regions = strs(&[
        "POLYGON((9 49, 10 49, 10 50, 9 50, 9 49))",
        "POLYGON((10 49, 11 49, 11 50, 10 50, 10 49))",
        "POLYGON((9 50, 10 50, 10 51, 9 51, 9 50))",
        "POLYGON((10 50, 11 50, 11 51, 10 51, 10 50))",
    ]);
    let mut cols = vec![
        Column::new("g", Role::Geometry, regions),
        Column::new("v", Role::Value(Kind::Bar), nums(&[1.0, 2.0, 3.0, 4.0])),
    ];
    if let Some(l) = legend {
        cols.push(Column::new("l", Role::Legend, strs(&[l])));
    }
    render(&cols, 480, 320).unwrap()
}

/// The map legend `<g>`'s own `translate(x,y)`, `None` without one.
fn legend_translate(svg: &str) -> Option<&str> {
    let i = svg.find("class=\"dp-legend\"")?;
    let rest = &svg[i..];
    let start = rest.find("translate(")? + "translate(".len();
    let end = start + rest[start..].find(')')?;
    Some(&rest[start..end])
}

#[test]
fn none_removes_a_maps_legend() {
    let with = choropleth(None);
    let without = choropleth(Some("none"));
    assert!(legend_translate(&with).is_some(), "{with}");
    assert!(legend_translate(&without).is_none(), "{without}");
}

#[test]
fn bottom_and_right_move_a_maps_legend() {
    let default = choropleth(None);
    let bottom = choropleth(Some("bottom"));
    let right = choropleth(Some("right"));
    let (dx, dy) = split(legend_translate(&default).unwrap());
    let (bx, by) = split(legend_translate(&bottom).unwrap());
    let (rx, ry) = split(legend_translate(&right).unwrap());
    // default and 'left' both sit at the top-left corner.
    assert_eq!((dx, dy), (4.0, 4.0), "{default}");
    // 'bottom' keeps the same x, moves down.
    assert_eq!(bx, dx, "{bottom}");
    assert!(by > dy, "{bottom}");
    // 'right' keeps the same y, moves right.
    assert_eq!(ry, dy, "{right}");
    assert!(rx > dx, "{right}");
}

fn split(xy: &str) -> (f64, f64) {
    let (x, y) = xy.split_once(',').unwrap();
    (x.parse().unwrap(), y.parse().unwrap())
}

#[test]
fn an_unknown_value_leaves_a_maps_legend_where_it_was() {
    // Not byte-identical: the extra (otherwise no-op) ::LEGEND column shifts
    // `map::id_of`'s hash, which only ever names a `<linearGradient>` def —
    // the legend's own position is what matters here.
    assert_eq!(
        legend_translate(&choropleth(None)),
        legend_translate(&choropleth(Some("sideways")))
    );
}
