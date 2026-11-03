//! #959: bar value labels (`::DATALABELS`) placed as ggplot2 would — past the
//! end of a positive bar (above; right when flipped), past a negative one's
//! end (below / left), inside or centred on request (a stack's default, in
//! the fills' contrast colour), one number style per series.

use anofox_visualization::{render_with, Column, Kind, RenderOptions, Role};
use ggplot_rs::prelude::Value;

fn s(v: &[&str]) -> Vec<Value> {
    v.iter().map(|x| Value::Str(x.to_string())).collect()
}
fn f(v: &[f64]) -> Vec<Value> {
    v.iter().map(|x| Value::Float(*x)).collect()
}
/// (label, x, y, text-anchor, fill) of every data label among `want` (not
/// the axis' tick labels, drawn in the theme's muted grey).
fn labels(svg: &str, want: &[&str]) -> Vec<(String, f64, f64, String, String)> {
    svg.split("<text")
        .skip(1)
        .filter_map(|t| {
            let a = |k: &str| {
                t.split(&format!(" {k}=\""))
                    .nth(1)?
                    .split('"')
                    .next()
                    .map(str::to_string)
            };
            let text = t.split('>').nth(1)?.split('<').next()?.to_string();
            want.contains(&text.as_str()).then_some(())?;
            Some((
                text,
                a("x")?.parse().ok()?,
                a("y")?.parse().ok()?,
                a("text-anchor").unwrap_or_default(),
                a("fill").unwrap_or_default(),
            ))
        })
        .filter(|l| !l.4.eq_ignore_ascii_case("#6B7280"))
        .collect()
}
fn bars(extra: Vec<Column>, v: &[f64]) -> String {
    let mut cols = vec![
        Column::new("x", Role::X, s(&["a", "b", "c", "d"])),
        Column::new("v", Role::Value(Kind::Bar), f(v)),
    ];
    cols.extend(extra);
    render_with(&cols, 600, 360, &RenderOptions::default()).unwrap()
}

#[test]
fn one_number_style_per_series() {
    let svg = bars(
        vec![Column::new("d", Role::DataLabels, f(&[11.0; 4]))],
        &[15100.0, 12400.0, 9800.0, 7300.0],
    );
    assert_eq!(
        labels(&svg, &["15.1K", "12.4K", "9.8K", "7.3K"]).len(),
        4,
        "every label compact"
    );
    assert!(labels(&svg, &["9,800", "7,300"]).is_empty());
}

#[test]
fn flipped_labels_sit_right_of_the_bar_end() {
    let svg = bars(
        vec![
            Column::new("d", Role::DataLabels, f(&[11.0; 4])),
            Column::new("fl", Role::Flip, f(&[1.0; 4])),
        ],
        &[4.0, 3.0, 2.0, 1.0],
    );
    let l = labels(&svg, &["4", "1"]);
    assert_eq!(l.len(), 2, "{l:?}");
    assert!(
        l.iter().all(|x| x.3 == "start"),
        "left-anchored, past the end: {l:?}"
    );
    let (big, small) = (
        l.iter().find(|x| x.0 == "4").unwrap(),
        l.iter().find(|x| x.0 == "1").unwrap(),
    );
    assert!(big.1 > small.1 + 50.0, "each at its own bar's end: {l:?}");
}

#[test]
fn a_negative_bars_label_sits_below_it() {
    let svg = bars(
        vec![Column::new("d", Role::DataLabels, f(&[11.0; 4]))],
        &[12.0, -8.0, 15.0, -3.0],
    );
    let l = labels(&svg, &["12", "-8"]);
    let (pos, neg) = (
        l.iter().find(|x| x.0 == "12").unwrap(),
        l.iter().find(|x| x.0 == "-8").unwrap(),
    );
    assert!(
        neg.2 > pos.2 + 50.0,
        "the negative label is far below the positive one: {l:?}"
    );
}

#[test]
fn stacked_and_inside_labels_take_the_fills_contrast_colour() {
    let cols = vec![
        Column::new("x", Role::X, s(&["Q1", "Q1", "Q2", "Q2"])),
        Column::new("c", Role::Category, s(&["A", "B", "A", "B"])),
        Column::new(
            "v",
            Role::Value(Kind::BarStacked),
            f(&[30.0, 20.0, 25.0, 35.0]),
        ),
        Column::new("d", Role::DataLabels, f(&[11.0; 4])),
    ];
    let svg = render_with(&cols, 600, 360, &RenderOptions::default()).unwrap();
    let l = labels(&svg, &["30", "20", "25", "35"]);
    assert_eq!(l.len(), 4, "{l:?}");
    let fills: std::collections::BTreeSet<_> = l.iter().map(|x| x.4.to_ascii_lowercase()).collect();
    assert_eq!(fills.len(), 1, "one ink for the stack: {fills:?}");
    let inside = bars(
        vec![Column::new("d", Role::DataLabels, s(&["inside"; 4]))],
        &[4.0, 3.0, 2.0, 1.0],
    );
    let outside = bars(
        vec![Column::new("d", Role::DataLabels, f(&[11.0; 4]))],
        &[4.0, 3.0, 2.0, 1.0],
    );
    let (i, o) = (labels(&inside, &["4"]), labels(&outside, &["4"]));
    // (the outside chart has headroom above its bars, so only the order holds)
    assert!(
        i[0].2 > o[0].2,
        "'inside' sits below the outside place: {i:?} {o:?}"
    );
    assert_ne!(i[0].4, o[0].4, "…in the bar's contrast colour");
}
