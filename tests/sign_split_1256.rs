//! #1256: `::SIGN_SPLIT` colours a line, area or density by the side of a
//! threshold — the theme's negative below, positive above, cut at the
//! crossing; a `::CATEGORY` keeps its series colours.

use anofox_visualization::{render_with_warnings, Column, Kind, RenderOptions, Role};
use ggplot_rs::prelude::Value;

fn hex(c: (u8, u8, u8)) -> String {
    format!("#{:02X}{:02X}{:02X}", c.0, c.1, c.2)
}

fn series(kind: Kind, split: Value) -> Vec<Column> {
    vec![
        Column::new(
            "t",
            Role::X,
            (0..20).map(|i| Value::Float(i as f64)).collect(),
        ),
        Column::new(
            "v",
            Role::Value(kind),
            (0..20)
                .map(|i| Value::Float((i as f64 * 0.6).sin() * 10.0 + 2.0))
                .collect(),
        ),
        Column::new("s", Role::SignSplit, vec![split; 20]),
    ]
}

fn colours(svg: &str, o: &RenderOptions) -> (usize, usize) {
    let t = &o.theme.colors;
    (
        svg.matches(&hex(t.negative)).count(),
        svg.matches(&hex(t.positive)).count(),
    )
}

#[test]
fn an_area_and_a_line_split_at_zero() {
    let o = RenderOptions::default();
    for kind in [Kind::Area, Kind::Line] {
        let r =
            render_with_warnings(&series(kind, Value::Integer(0)), 480, 320, &o).expect("renders");
        let (neg, pos) = colours(&r.svg, &o);
        assert!(
            neg >= 1 && pos >= 1,
            "{kind:?}: negative {neg}, positive {pos}"
        );
    }
}

#[test]
fn a_density_of_outcomes_splits_along_x() {
    let o = RenderOptions::default();
    let cols = vec![
        Column::new(
            "gain",
            Role::Value(Kind::Density),
            (0..300)
                .map(|i| Value::Float((i as f64 * 0.731).sin() * 40.0 + 10.0))
                .collect(),
        ),
        Column::new("s", Role::SignSplit, vec![Value::Integer(0); 300]),
    ];
    let r = render_with_warnings(&cols, 480, 320, &o).expect("renders");
    let (neg, pos) = colours(&r.svg, &o);
    assert!(neg == 1 && pos == 1, "negative {neg}, positive {pos}");
}

#[test]
fn a_spec_sets_axis_threshold_and_colours() {
    let o = RenderOptions::default();
    let r = render_with_warnings(
        &series(
            Kind::Line,
            Value::Str("y 5; below:#112233; above:good".into()),
        ),
        480,
        320,
        &o,
    )
    .expect("renders");
    assert!(r.svg.contains("#112233"), "the below colour");
    assert!(colours(&r.svg, &o).1 >= 1, "good = the theme's positive");
    let bad = render_with_warnings(
        &series(Kind::Line, Value::Str("below:plum".into())),
        480,
        320,
        &o,
    );
    assert!(
        bad.is_err_and(|e| e.to_string().contains("plum")),
        "an unknown colour is an error"
    );
}

#[test]
fn a_plain_line_is_unchanged() {
    let o = RenderOptions::default();
    let mut cols = series(Kind::Line, Value::Na);
    cols.pop();
    let r = render_with_warnings(&cols, 480, 320, &o).expect("renders");
    assert_eq!(colours(&r.svg, &o), (0, 0));
}
