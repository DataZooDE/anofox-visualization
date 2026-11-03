//! #958: a flipped bar chart reads top-down — the query's first row on top
//! (a ranking `ORDER BY value DESC` starts with the largest).
use anofox_visualization::{render_with, Column, Kind, RenderOptions, Role};
use ggplot_rs::prelude::Value;

#[test]
fn the_first_row_is_on_top() {
    let s = |v: &[&str]| {
        v.iter()
            .map(|x| Value::Str(x.to_string()))
            .collect::<Vec<_>>()
    };
    let cols = vec![
        Column::new("x", Role::X, s(&["East", "North", "South", "West"])),
        Column::new(
            "v",
            Role::Value(Kind::Bar),
            [15.0, 12.0, 9.0, 7.0].map(Value::Float).to_vec(),
        ),
        Column::new("f", Role::Flip, vec![Value::Float(1.0); 4]),
    ];
    let svg = render_with(&cols, 600, 300, &RenderOptions::default()).unwrap();
    let y = |name: &str| -> f64 {
        let i = svg.find(&format!(">{name}<")).unwrap();
        let tag = &svg[svg[..i].rfind("<text").unwrap()..i];
        tag.split(" y=\"")
            .nth(1)
            .unwrap()
            .split('"')
            .next()
            .unwrap()
            .parse()
            .unwrap()
    };
    assert!(y("East") < y("North") && y("North") < y("South") && y("South") < y("West"));
}
