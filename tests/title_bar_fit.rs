//! A static page's ::TITLE bar (`render_with_title`) cuts a title wider than
//! the panel with "…", as a dashboard card's title does (#878), instead of
//! running past the panel's edge.

use anofox_visualization::{render_with_title, Column, RenderOptions, Role, Value};

fn bar(title: &str, width: u32) -> String {
    let cols = vec![
        Column::new("x", Role::X, vec![Value::Str("a".into())]),
        Column::new(
            "y",
            Role::Value(anofox_visualization::Kind::Bar),
            vec![Value::Float(1.0)],
        ),
        Column::new("t", Role::Title, vec![Value::Str(title.into())]),
    ];
    let svg = render_with_title(&cols, &[], width, 300, &RenderOptions::default()).unwrap();
    let text = svg.split("font-weight=\"700\"").nth(1).unwrap();
    text[text.find('>').unwrap() + 1..text.find("</text>").unwrap()].to_string()
}

#[test]
fn a_long_title_is_cut_to_the_bar() {
    let long = "Work-centre hours for the finished-product orders, all BOM levels";
    let cut = bar(long, 460);
    assert!(cut.ends_with('…'), "{cut}");
    assert!(cut.chars().count() <= long.chars().count(), "{cut}");
    // a wide panel keeps it whole, so does a short title
    assert_eq!(bar(long, 928), long);
    assert_eq!(bar("Load", 460), "Load");
}
