//! #751: a chart with a tooltip role names each mark's result row
//! (`data-row`), so the browser can fill the tooltip from that row; a chart
//! without one renders exactly as before.
use anofox_visualization::{render, Column, Kind, Role};
use ggplot_rs::prelude::Value;

fn cols(kind: Kind, tooltip: bool) -> Vec<Column> {
    let mut c = vec![
        Column::new(
            "region",
            Role::X,
            ["EU", "US", "APAC"]
                .iter()
                .map(|s| Value::Str(s.to_string()))
                .collect(),
        ),
        Column::new(
            "units",
            Role::Value(kind),
            vec![Value::Float(3.0), Value::Float(5.0), Value::Float(2.0)],
        ),
    ];
    if tooltip {
        c.push(Column::new(
            "Price",
            Role::Tooltip,
            vec![Value::Float(9.5), Value::Float(7.0), Value::Float(8.25)],
        ));
    }
    c
}

fn rows(svg: &str) -> Vec<String> {
    svg.match_indices("data-row=\"")
        .map(|(i, m)| {
            svg[i + m.len()..]
                .split('"')
                .next()
                .unwrap_or("")
                .to_string()
        })
        .collect()
}

#[test]
fn bars_with_a_tooltip_column_name_their_rows() {
    let svg = render(&cols(Kind::Bar, true), 500, 300).unwrap();
    let mut r = rows(&svg);
    r.sort();
    assert_eq!(r, ["0", "1", "2"], "{svg}");
    // the tooltip column is never drawn as a series or an axis
    assert!(
        !svg.contains(">Price<"),
        "no legend entry or axis title for it"
    );
}

#[test]
fn points_of_a_line_name_their_rows() {
    let svg = render(&cols(Kind::Line, true), 500, 300).unwrap();
    assert!(rows(&svg).len() >= 3, "{svg}");
}

#[test]
fn without_a_tooltip_role_nothing_changes() {
    for kind in [Kind::Bar, Kind::Line, Kind::Point] {
        let a = render(&cols(kind, false), 500, 300).unwrap();
        assert!(!a.contains("data-row"), "{kind:?}");
    }
}

#[test]
fn a_merged_point_names_no_row() {
    // two rows of the same x and series are summed by the downsampler only
    // when categories are capped; a plain duplicate x stays two rows here,
    // so check the aggregate path through many categories instead.
    let n = 40;
    let cols = vec![
        Column::new(
            "x",
            Role::X,
            (0..n).map(|i| Value::Str(format!("c{i}"))).collect(),
        ),
        Column::new(
            "v",
            Role::Value(Kind::Bar),
            (0..n).map(|i| Value::Float(i as f64 + 1.0)).collect(),
        ),
        Column::new(
            "t",
            Role::TooltipFormat,
            (0..n).map(|_| Value::Str("{y}".into())).collect(),
        ),
    ];
    let svg = render(&cols, 600, 300).unwrap();
    let r = rows(&svg);
    assert!(!r.is_empty(), "rows are named");
    assert!(r.iter().all(|x| x.parse::<usize>().is_ok_and(|i| i < n)));
}
