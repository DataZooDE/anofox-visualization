//! #1049: several measure columns of one bar kind, no `::CATEGORY` — each
//! column is a series. Stacked kinds stack them, a plain bar dodges them, the
//! percent kinds stack to 100 %. They used to be drawn on top of each other,
//! so the larger bar hid the smaller one.

use anofox_visualization::{render_with, Column, Kind, RenderOptions, Role};
use ggplot_rs::prelude::Value;

fn strs(v: &[&str]) -> Vec<Value> {
    v.iter().map(|s| Value::Str(s.to_string())).collect()
}
fn nums(v: &[f64]) -> Vec<Value> {
    v.iter().map(|&f| Value::Float(f)).collect()
}

/// (data-x, data-series, x, y, width, height) of every bar.
fn bars(svg: &str) -> Vec<(String, String, f64, f64, f64, f64)> {
    let attr = |r: &str, n: &str| -> String {
        let k = r.find(&format!(" {n}=\"")).map(|k| k + n.len() + 3);
        k.map(|k| r[k..k + r[k..].find('"').unwrap()].to_string())
            .unwrap_or_default()
    };
    svg.split("<rect ")
        .skip(1)
        .map(|r| format!(" {}", &r[..r.find('>').unwrap()]))
        .filter(|r| r.contains("gg-bar") && r.contains(" data-x=\""))
        .map(|r| {
            let f = |n| attr(&r, n).parse::<f64>().unwrap();
            (
                attr(&r, "data-x"),
                attr(&r, "data-series"),
                f("x"),
                f("y"),
                f("width"),
                f("height"),
            )
        })
        .collect()
}

fn chart(kind: Kind) -> String {
    let cols = vec![
        Column::new("p", Role::X, strs(&["A", "B"])),
        Column::new("Holding", Role::Value(kind), nums(&[10.0, 30.0])),
        Column::new("Backlog", Role::Value(kind), nums(&[40.0, 20.0])),
    ];
    render_with(&cols, 480, 320, &RenderOptions::default()).expect("renders")
}

#[test]
fn stacked_measures_stack() {
    let b = bars(&chart(Kind::BarStacked));
    assert_eq!(b.len(), 4, "{b:?}");
    for x in ["A", "B"] {
        let at: Vec<_> = b.iter().filter(|r| r.0 == x).collect();
        assert_eq!(at.len(), 2, "{b:?}");
        let (lo, hi) = if at[0].3 > at[1].3 {
            (at[0], at[1])
        } else {
            (at[1], at[0])
        };
        // one part sits on the other: the upper one ends where the lower begins
        assert!((hi.3 + hi.5 - lo.3).abs() < 0.6, "{x}: {at:?}");
        assert_eq!(lo.2, hi.2, "same column: {at:?}");
    }
    // both series are drawn, named by their columns
    for s in ["Holding", "Backlog"] {
        assert!(b.iter().any(|r| r.1 == s), "{s}: {b:?}");
    }
}

#[test]
fn plain_bar_measures_sit_side_by_side() {
    let b = bars(&chart(Kind::Bar));
    assert_eq!(b.len(), 4, "{b:?}");
    for x in ["A", "B"] {
        let at: Vec<_> = b.iter().filter(|r| r.0 == x).collect();
        assert_eq!(at.len(), 2, "{b:?}");
        let (l, r) = if at[0].2 < at[1].2 {
            (at[0], at[1])
        } else {
            (at[1], at[0])
        };
        assert!(l.2 + l.4 <= r.2 + 0.6, "dodged, not overlaid: {at:?}");
    }
}

#[test]
fn percent_stacked_measures_fill_the_axis() {
    let b = bars(&chart(Kind::BarStackedPercent));
    assert_eq!(b.len(), 4, "{b:?}");
    let total = |x: &str| b.iter().filter(|r| r.0 == x).map(|r| r.5).sum::<f64>();
    // every column is 100 %: the same total height
    assert!((total("A") - total("B")).abs() < 0.6, "{b:?}");
}

/// #1059: on a numeric x (month 1–12, a year) the series sat on top of each
/// other: dodging needed a text x. Bars now dodge there too, in numeric order.
#[test]
fn bar_series_on_a_numeric_x_sit_side_by_side() {
    for wide in [true, false] {
        let cols = if wide {
            vec![
                Column::new("m", Role::X, nums(&[3.0, 1.0, 2.0])),
                Column::new("2024", Role::Value(Kind::Bar), nums(&[10.0, 30.0, 20.0])),
                Column::new("2025", Role::Value(Kind::Bar), nums(&[12.0, 33.0, 25.0])),
            ]
        } else {
            vec![
                Column::new("m", Role::X, nums(&[1.0, 1.0, 2.0, 2.0])),
                Column::new("y", Role::Category, strs(&["2024", "2025", "2024", "2025"])),
                Column::new("v", Role::Value(Kind::Bar), nums(&[30.0, 33.0, 20.0, 25.0])),
            ]
        };
        let b = bars(&render_with(&cols, 480, 320, &RenderOptions::default()).expect("renders"));
        let per_x = if wide { 3 } else { 2 };
        assert_eq!(b.len(), per_x * 2, "{b:?}");
        for x in b
            .iter()
            .map(|r| r.0.clone())
            .collect::<std::collections::BTreeSet<_>>()
        {
            let at: Vec<_> = b.iter().filter(|r| r.0 == x).collect();
            let (l, r) = if at[0].2 < at[1].2 {
                (at[0], at[1])
            } else {
                (at[1], at[0])
            };
            assert!(
                l.2 + l.4 <= r.2 + 0.6,
                "{wide}: dodged, not overlaid at {x}: {at:?}"
            );
        }
        // numeric order left to right: 1, 2(, 3)
        let mut xs: Vec<(f64, String)> = b.iter().map(|r| (r.2, r.0.clone())).collect();
        xs.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        let order: Vec<String> = xs
            .iter()
            .map(|(_, x)| x.clone())
            .fold(Vec::new(), |mut v, x| {
                if !v.contains(&x) {
                    v.push(x)
                }
                v
            });
        assert_eq!(
            order
                .iter()
                .map(|s| s.parse::<f64>().unwrap())
                .collect::<Vec<_>>(),
            (1..=per_x).map(|i| i as f64).collect::<Vec<_>>(),
            "{order:?}"
        );
    }
}
