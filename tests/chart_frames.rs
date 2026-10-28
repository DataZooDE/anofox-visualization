//! `::FRAME` on bar/line charts (#663): one frame drawn, on axes that span
//! every frame, with the frame player's root attributes.

use anofox_visualization::{render_with, Column, Kind, RenderOptions, Role};
use ggplot_rs::prelude::Value;

fn strs(v: &[&str]) -> Vec<Value> {
    v.iter().map(|s| Value::Str(s.to_string())).collect()
}
fn nums(v: &[f64]) -> Vec<Value> {
    v.iter().map(|&f| Value::Float(f)).collect()
}
fn attr<'a>(svg: &'a str, name: &str) -> Option<&'a str> {
    let root = &svg[..svg.find('>').unwrap()];
    let k = root.find(&format!(" {name}=\""))? + name.len() + 3;
    Some(&root[k..k + root[k..].find('"')?])
}
fn render(cols: &[Column], frame: Option<usize>) -> String {
    let o = RenderOptions {
        frame,
        ..RenderOptions::default()
    };
    render_with(cols, 480, 320, &o).expect("renders")
}

fn bars() -> Vec<Column> {
    vec![
        Column::new("r", Role::X, strs(&["North", "South", "North", "East"])),
        Column::new("y", Role::Frame, strs(&["2020", "2020", "2021", "2021"])),
        Column::new("v", Role::Value(Kind::Bar), nums(&[10.0, 30.0, 60.0, 5.0])),
    ]
}

#[test]
fn a_framed_bar_chart_draws_one_frame_on_every_frames_axes() {
    let last = render(&bars(), None);
    let first = render(&bars(), Some(0));
    assert_eq!(
        attr(&last, "data-frames"),
        Some("[&quot;2020&quot;,&quot;2021&quot;]")
    );
    assert_eq!(attr(&last, "data-frame"), Some("1"));
    assert_eq!(attr(&first, "data-frame"), Some("0"));
    assert_eq!(attr(&last, "data-frame-name"), Some("y"));
    assert!(last.contains(">y: 2021<"), "subtitle names the frame");
    // one frame's bars only
    assert!(first.contains("data-value=\"30\"") && !first.contains("data-value=\"60\""));
    // the same y domain and x levels in both frames
    assert_eq!(attr(&first, "data-ydomain"), attr(&last, "data-ydomain"));
    assert_eq!(attr(&first, "data-xlevels"), attr(&last, "data-xlevels"));
    assert_eq!(
        attr(&first, "data-xlevels"),
        Some("[&quot;North&quot;,&quot;South&quot;,&quot;East&quot;]")
    );
}

#[test]
fn a_framed_line_chart_keeps_its_x_and_y_ranges() {
    let x: Vec<f64> = (1..=4).map(f64::from).collect();
    let cols = |frame: &[&str], v: &[f64]| {
        vec![
            Column::new("m", Role::X, nums(&[x.clone(), x.clone()].concat())),
            Column::new("y", Role::Frame, strs(frame)),
            Column::new("v", Role::Value(Kind::Line), nums(v)),
        ]
    };
    let c = cols(
        &["a", "a", "a", "a", "b", "b", "b", "b"],
        &[1.0, 2.0, 3.0, 4.0, 10.0, 20.0, 30.0, 40.0],
    );
    let a = render(&c, Some(0));
    let b = render(&c, Some(1));
    assert_eq!(attr(&a, "data-ydomain"), attr(&b, "data-ydomain"));
    let hi: f64 = attr(&a, "data-ydomain")
        .unwrap()
        .split(' ')
        .nth(1)
        .unwrap()
        .parse()
        .unwrap();
    assert!(hi >= 40.0, "frame a's axis reaches frame b's 40: {hi}");
}

#[test]
fn charts_without_a_frame_are_unchanged() {
    let cols = vec![
        Column::new("r", Role::X, strs(&["North", "South"])),
        Column::new("v", Role::Value(Kind::Bar), nums(&[10.0, 30.0])),
    ];
    let s = render(&cols, None);
    assert!(attr(&s, "data-frames").is_none());
}
