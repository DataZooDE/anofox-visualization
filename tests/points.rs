//! #631 `::POINTS`: line/area markers on a dense series are left uninked by
//! ggplot-rs (`PointVisibility`) but stay in the SVG as hover/click targets.
use anofox_visualization::{render, Column, Kind, Role};
use ggplot_rs::prelude::Value;

/// `n` points per series (15-minute steps), `series` series, `kind` chart,
/// plus `extra` columns.
fn chart(kind: Kind, n: usize, series: usize, extra: Option<&str>) -> String {
    let rows = n * series;
    let mut cols = vec![
        Column::new(
            "t",
            Role::X,
            (0..rows)
                .map(|i| Value::Float((i % n) as f64 * 900.0))
                .collect(),
        ),
        Column::new(
            "v",
            Role::Value(kind),
            (0..rows).map(|i| Value::Float((i % 97) as f64)).collect(),
        ),
    ];
    if series > 1 {
        cols.push(Column::new(
            "s",
            Role::Category,
            (0..rows)
                .map(|i| Value::Str(format!("s{}", i / n)))
                .collect(),
        ));
    }
    if let Some(mode) = extra {
        cols.push(Column::new(
            "p",
            Role::Points,
            (0..rows).map(|_| Value::Str(mode.into())).collect(),
        ));
    }
    render(&cols, 480, 300).unwrap()
}

/// The point marks (`data-geom="point"`).
fn points(svg: &str) -> Vec<&str> {
    svg.split('<')
        .filter(|t| t.starts_with("circle") && t.contains(r#"data-geom="point""#))
        .collect()
}

fn hidden(svg: &str) -> bool {
    let p = points(svg);
    !p.is_empty() && p.iter().all(|c| c.contains(r#"fill-opacity="0.000""#))
}

fn visible(svg: &str) -> bool {
    let p = points(svg);
    !p.is_empty() && p.iter().all(|c| !c.contains("fill-opacity"))
}

#[test]
fn a_dense_series_hides_its_markers_but_keeps_hover_targets() {
    for kind in [Kind::Line, Kind::Step, Kind::Area] {
        for series in [1, 2] {
            let svg = chart(kind, 672, series, None);
            assert!(hidden(&svg), "{kind:?} × {series}");
            // every target still carries its tooltip
            assert!(
                svg.matches("<title>").count() >= points(&svg).len(),
                "{kind:?} × {series}"
            );
        }
    }
}

#[test]
fn a_short_series_keeps_visible_markers() {
    assert!(visible(&chart(Kind::Line, 12, 1, None)));
    assert!(visible(&chart(Kind::Line, 12, 2, Some("auto"))));
}

#[test]
fn points_role_overrides_the_rule_both_ways() {
    assert!(hidden(&chart(Kind::Line, 12, 1, Some("none"))));
    assert!(hidden(&chart(Kind::Line, 12, 1, Some("NONE"))));
    assert!(visible(&chart(Kind::Line, 672, 1, Some("always"))));
    // an unknown value falls back to 'auto' (and --check warns)
    assert!(hidden(&chart(Kind::Line, 672, 1, Some("dots"))));
}
