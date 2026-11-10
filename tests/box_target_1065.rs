//! #1065: `::TARGET` (`::REFLINE`) on a box from statistics draws a tick
//! across each row's box — one marker per box (a promised lead time), not a
//! line across the chart. #1080: a flipped box keeps its median and whiskers.

use anofox_visualization::{render_with, Column, Role};
use ggplot_rs::prelude::Value;

fn strs(v: &[&str]) -> Vec<Value> {
    v.iter().map(|s| Value::Str(s.to_string())).collect()
}
fn nums(v: &[f64]) -> Vec<Value> {
    v.iter().map(|&f| Value::Float(f)).collect()
}

fn boxes(extra: Vec<Column>) -> String {
    let mut cols = vec![
        Column::new("s", Role::X, strs(&["A", "B", "C"])),
        Column::new("q1", Role::Q1, nums(&[8.0, 11.0, 15.0])),
        Column::new("m", Role::Median, nums(&[10.0, 13.0, 17.0])),
        Column::new("q3", Role::Q3, nums(&[12.0, 15.5, 19.5])),
        Column::new("lo", Role::WhiskerLow, nums(&[5.0, 8.0, 12.0])),
        Column::new("hi", Role::WhiskerHigh, nums(&[15.0, 18.0, 22.0])),
    ];
    cols.extend(extra);
    render_with(&cols, 480, 320, &Default::default()).expect("renders")
}

fn titles(svg: &str, geom: &str) -> Vec<String> {
    svg.split('<')
        .enumerate()
        .filter(|(_, e)| e.contains(&format!("data-geom=\"{geom}\"")))
        .filter_map(|(i, _)| {
            let t = svg.split('<').nth(i + 1)?;
            t.strip_prefix("title>").map(str::to_string)
        })
        .collect()
}

#[test]
fn a_target_per_box_is_a_tick_across_it() {
    // B has no target: no tick
    let svg = boxes(vec![Column::new(
        "promised",
        Role::RefLine,
        vec![Value::Float(7.0), Value::Na, Value::Float(25.0)],
    )]);
    assert_eq!(
        titles(&svg, "errorbar"),
        ["A: Target 7", "C: Target 25"],
        "{svg}"
    );
    // not a line across the chart
    assert!(!svg.contains("data-geom=\"hline\""), "{svg}");
    // the y axis reaches the target above every box, and doesn't drop to 0
    assert!(svg.contains(">25<"), "the 25 target is on the axis");
    assert!(!svg.contains(">0<"), "the axis isn't pulled down to 0");
}

#[test]
fn without_a_target_the_boxes_are_unchanged() {
    let svg = boxes(vec![]);
    assert!(!svg.contains("data-geom=\"errorbar\""));
}

/// The geometry of every boxplot `<polyline>`: (points, length).
fn box_lines(svg: &str) -> Vec<f64> {
    svg.split("<polyline ")
        .skip(1)
        .filter(|e| e[..e.find('>').unwrap()].contains("data-geom=\"boxplot\""))
        .map(|e| {
            let k = e.find("points=\"").unwrap() + 8;
            let pts: Vec<(f64, f64)> = e[k..k + e[k..].find('"').unwrap()]
                .split_whitespace()
                .filter_map(|p| {
                    let (a, b) = p.split_once(',')?;
                    Some((a.parse().ok()?, b.parse().ok()?))
                })
                .collect();
            pts.windows(2)
                .map(|w| ((w[1].0 - w[0].0).powi(2) + (w[1].1 - w[0].1).powi(2)).sqrt())
                .sum()
        })
        .collect()
}

#[test]
fn a_flipped_box_keeps_its_median_and_whiskers() {
    let up = box_lines(&boxes(vec![]));
    let flipped = box_lines(&boxes(vec![Column::new("f", Role::Flip, nums(&[1.0; 3]))]));
    assert!(!up.is_empty());
    assert_eq!(up.len(), flipped.len());
    assert!(flipped.iter().all(|&l| l > 1.0), "{flipped:?}");
}
