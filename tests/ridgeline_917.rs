//! #917: `::RIDGELINE` — one density per ::CATEGORY as overlapping ridges.

use anofox_visualization::{render_with, Column, Kind, RenderOptions, Role};
use ggplot_rs::prelude::Value;

fn cols(months: &[&str], per: usize) -> Vec<Column> {
    let (mut m, mut v) = (Vec::new(), Vec::new());
    for (k, name) in months.iter().enumerate() {
        for i in 0..per {
            m.push(Value::Str(name.to_string()));
            v.push(Value::Float(
                k as f64 * 2.0 + ((i * 37 % per) as f64 / per as f64) * 6.0,
            ));
        }
    }
    vec![
        Column::new("month", Role::Category, m),
        Column::new("delay", Role::Value(Kind::Ridgeline), v),
    ]
}

#[test]
fn a_ridge_per_category_first_on_top() {
    let svg = render_with(
        &cols(&["Jan", "Feb", "Mar"], 40),
        560,
        360,
        &RenderOptions::default(),
    )
    .unwrap();
    if let Ok(p) = std::env::var("RIDGE_SVG") {
        std::fs::write(p, &svg).unwrap();
    }
    let ridges: Vec<&str> = svg
        .split("<polygon ")
        .skip(1)
        .filter(|p| p.contains("data-x="))
        .collect();
    assert_eq!(ridges.len(), 3, "{svg}");
    // the y axis lists Jan at the top: its tick label is the highest (smallest y)
    let label_y = |t: &str| -> f64 {
        let i = svg.find(&format!(">{t}</text>")).expect(t);
        let tag = &svg[svg[..i].rfind("<text").unwrap()..i];
        let k = tag.find(" y=\"").unwrap() + 4;
        tag[k..k + tag[k..].find('"').unwrap()].parse().unwrap()
    };
    assert!(label_y("Jan") < label_y("Feb") && label_y("Feb") < label_y("Mar"));
}

#[test]
fn too_many_rows_say_so() {
    let names: Vec<String> = (0..41).map(|i| format!("g{i}")).collect();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let svg = render_with(&cols(&refs, 3), 560, 360, &RenderOptions::default()).unwrap();
    assert!(svg.contains("at most 40 rows"), "{svg}");
}
