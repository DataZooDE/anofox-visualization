//! A static page's ::TITLE bar (`render_with_title`) wraps a title wider
//! than the panel onto up to three lines (#1345; it used to be cut with "…",
//! #878), and cuts only what still doesn't fit — never running past the
//! panel's edge.

use anofox_visualization::{render_with_title, Column, RenderOptions, Role, Value};

/// The bar's lines and its height.
fn bar(title: &str, width: u32) -> (Vec<String>, String) {
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
    let bar = &svg[..svg.find("</svg>").unwrap()];
    let lines = bar
        .split("font-weight=\"700\"")
        .skip(1)
        .map(|t| t[t.find('>').unwrap() + 1..t.find("</text>").unwrap()].to_string())
        .collect();
    let h = bar.split(" height=\"").nth(1).unwrap();
    (lines, h[..h.find('"').unwrap()].to_string())
}

#[test]
fn a_long_title_wraps_in_the_bar() {
    let long = "Work-centre hours for the finished-product orders, all BOM levels";
    let (lines, h) = bar(long, 460);
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert_eq!(lines.join(" "), long);
    assert_eq!(h, "44", "the bar grows a line");
    // a wide panel keeps it on one line, so does a short title
    assert_eq!(bar(long, 928), (vec![long.to_string()], "26".into()));
    assert_eq!(bar("Load", 460).0, vec!["Load"]);
    // past three lines the last is cut
    let (lines, h) = bar(&long.repeat(4), 460);
    assert_eq!(lines.len(), 3);
    assert!(lines[2].ends_with('…'), "{lines:?}");
    assert_eq!(h, "62");
}
