//! #1062: an explicit built-in `::BASEMAP` set the map's extent to the whole
//! layer: 'world' turned points in Germany into a dot and an 'auto' hexbin
//! vanished. A built-in backdrop is unfitted now; the map fits its data.

use anofox_visualization::{render_with, Column, Kind, RenderOptions, Role};
use ggplot_rs::prelude::Value;

fn points(basemap: Option<&str>, hexbin: bool) -> String {
    let n = 60;
    let lat: Vec<Value> = (0..n)
        .map(|i| Value::Float(47.5 + (i % 10) as f64 * 0.7))
        .collect();
    let lon: Vec<Value> = (0..n)
        .map(|i| Value::Float(6.5 + (i / 10) as f64 * 1.4))
        .collect();
    let mut cols = vec![
        Column::new("lat", Role::Lat, lat),
        Column::new("lon", Role::Lon, lon),
        Column::new("n", Role::Value(Kind::Bar), vec![Value::Float(1.0); n]),
    ];
    if let Some(b) = basemap {
        cols.push(Column::new(
            "",
            Role::Basemap,
            vec![Value::Str(b.into()); n],
        ));
    }
    if hexbin {
        cols.push(Column::new(
            "",
            Role::Hexbin,
            vec![Value::Str("auto".into()); n],
        ));
    }
    render_with(&cols, 480, 320, &RenderOptions::default()).expect("renders")
}

/// The x coordinates of each drawn data feature (an element with a
/// `data-row`): its polygon's points, or its circle's centre.
fn feature_xs(svg: &str) -> Vec<Vec<f64>> {
    svg.split('<')
        .filter(|t| t.contains(" data-row=\""))
        .filter_map(|t| {
            let attr = |n: &str| {
                t.find(&format!(" {n}=\""))
                    .map(|k| &t[k + n.len() + 3..])
                    .map(|r| &r[..r.find('"').unwrap()])
            };
            if let Some(p) = attr("points") {
                Some(
                    p.split_whitespace()
                        .filter_map(|xy| xy.split(',').next()?.parse().ok())
                        .collect(),
                )
            } else {
                attr("cx").and_then(|c| c.parse().ok()).map(|x| vec![x])
            }
        })
        .collect()
}

fn spread(svg: &str) -> f64 {
    let xs: Vec<f64> = feature_xs(svg).into_iter().flatten().collect();
    xs.iter().cloned().fold(f64::MIN, f64::max) - xs.iter().cloned().fold(f64::MAX, f64::min)
}

#[test]
fn a_world_basemap_does_not_shrink_the_data_to_a_dot() {
    let plain = spread(&points(None, false));
    let world = spread(&points(Some("world"), false));
    assert!(
        plain > 100.0,
        "the points spread without a basemap: {plain}"
    );
    assert!(
        world > plain * 0.6,
        "on a world basemap the points still spread: {world} vs {plain}"
    );
}

#[test]
fn an_auto_hexbin_on_a_world_basemap_stays_visible() {
    let cells = feature_xs(&points(Some("world"), true));
    assert!(!cells.is_empty(), "hexagons drawn");
    let width = |c: &Vec<f64>| {
        c.iter().cloned().fold(f64::MIN, f64::max) - c.iter().cloned().fold(f64::MAX, f64::min)
    };
    let w = width(&cells[0]);
    assert!(w > 2.0, "a hexagon is visible, {w:.2} px wide");
}
