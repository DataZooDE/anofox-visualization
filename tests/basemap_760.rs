//! #760: the opt-in tile basemap under a `'mercator'` `::MAP` (`::MAPTILES`).
//! The renderer does no networking: a host supplies the tiles
//! (`RenderOptions::basemap`), after asking which ones a render needs
//! (`map_tiles_needed`). See docs/security/basemap-760-review.md.

use std::sync::{Arc, Mutex};

use anofox_visualization::{
    map_tiles_needed, render_with, Basemap, Column, Kind, RenderOptions, Role,
};
use ggplot_rs::prelude::Value;

/// A 1×1 transparent PNG.
const PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4,
    0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x00, 0x01, 0x00, 0x00,
    0x05, 0x00, 0x01, 0x0d, 0x0a, 0x2d, 0xb4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae,
    0x42, 0x60, 0x82,
];

fn konst(s: &str, n: usize) -> Vec<Value> {
    vec![Value::Str(s.to_string()); n]
}

/// Four depots around 10°E 50°N, optionally with a projection and `::MAPTILES`.
fn depots(projection: Option<&str>, tiles: Option<&str>) -> Vec<Column> {
    let pts = [
        "POINT(9.2 49.4)",
        "POINT(10.8 49.6)",
        "POINT(9.6 50.7)",
        "POINT(10.4 50.3)",
    ];
    let mut cols = vec![
        Column::new(
            "g",
            Role::Geometry,
            pts.iter().map(|p| Value::Str(p.to_string())).collect(),
        ),
        Column::new(
            "v",
            Role::Value(Kind::Bar),
            [1.0, 2.0, 3.0, 4.0].map(Value::Float).to_vec(),
        ),
    ];
    if let Some(p) = projection {
        cols.push(Column::new("p", Role::Projection, konst(p, 4)));
    }
    if let Some(t) = tiles {
        cols.push(Column::new("t", Role::MapTiles, konst(t, 4)));
    }
    cols
}

fn with_tiles(seen: Arc<Mutex<Vec<(u32, u32, u32)>>>) -> RenderOptions {
    RenderOptions {
        basemap: Some(Basemap {
            attribution: "© Test tiles".into(),
            tiles: Arc::new(move |z, x, y| {
                seen.lock().unwrap().push((z, x, y));
                Some(PNG.to_vec())
            }),
        }),
        ..RenderOptions::default()
    }
}

#[test]
fn tiles_needed_cover_the_data_on_one_zoom() {
    let cols = depots(Some("mercator"), Some("on"));
    let need = map_tiles_needed(&cols, 480, 320, &RenderOptions::default());
    assert!(!need.is_empty());
    let z = need[0].0;
    assert!(need.iter().all(|t| t.0 == z), "{need:?}");
    assert!((3..=16).contains(&z), "{need:?}");
    let n = 1u32 << z;
    assert!(need.iter().all(|&(_, x, y)| x < n && y < n), "{need:?}");
    // 10°E 50°N is inside the requested block
    let tx = ((10.0 + 180.0) / 360.0 * n as f64) as u32;
    let lat = 50f64.to_radians();
    let ty =
        ((1.0 - (lat.tan() + 1.0 / lat.cos()).ln() / std::f64::consts::PI) / 2.0 * n as f64) as u32;
    assert!(need.contains(&(z, tx, ty)), "{need:?} lacks {z}/{tx}/{ty}");
    // deterministic, sorted and without duplicates
    let mut sorted = need.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(need, sorted);
    assert_eq!(
        need,
        map_tiles_needed(&cols, 480, 320, &RenderOptions::default())
    );
}

#[test]
fn no_tiles_unless_asked_for_on_a_mercator_map() {
    let o = RenderOptions::default();
    for (p, t) in [
        (Some("mercator"), None),
        (Some("mercator"), Some("off")),
        (Some("mercator"), Some("0")),
        (None, Some("on")),
        (Some("equal-earth"), Some("on")),
        (Some("utm"), Some("on")),
    ] {
        assert!(
            map_tiles_needed(&depots(p, t), 480, 320, &o).is_empty(),
            "{p:?} {t:?}"
        );
    }
}

#[test]
fn the_host_tiles_are_embedded_with_their_attribution() {
    let cols = depots(Some("mercator"), Some("on"));
    let need = map_tiles_needed(&cols, 480, 320, &RenderOptions::default());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let svg = render_with(&cols, 480, 320, &with_tiles(seen.clone())).expect("renders");
    roxmltree::Document::parse(&svg).expect("well-formed SVG");
    let mut asked = seen.lock().unwrap().clone();
    asked.sort_unstable();
    asked.dedup();
    assert_eq!(asked, need, "the render asks for exactly the listed tiles");
    assert_eq!(svg.matches("data-tile=").count(), need.len());
    assert!(svg.contains("data:image/png;base64,"));
    assert!(svg.contains("© Test tiles"), "attribution drawn");
    // no URL of any kind reaches the SVG
    assert!(!svg.contains("href=\"http"));
}

#[test]
fn without_a_host_source_the_map_draws_as_before() {
    let plain = render_with(
        &depots(Some("mercator"), None),
        480,
        320,
        &RenderOptions::default(),
    )
    .unwrap();
    let asked = render_with(
        &depots(Some("mercator"), Some("on")),
        480,
        320,
        &RenderOptions::default(),
    )
    .unwrap();
    assert!(!asked.contains("data-tile="));
    assert!(!asked.contains("<image"));
    assert_eq!(
        plain.matches("<path").count(),
        asked.matches("<path").count()
    );
    // a missing tile is left blank, not an error
    let none = RenderOptions {
        basemap: Some(Basemap {
            attribution: "© Test tiles".into(),
            tiles: Arc::new(|_, _, _| None),
        }),
        ..RenderOptions::default()
    };
    let svg = render_with(&depots(Some("mercator"), Some("on")), 480, 320, &none).unwrap();
    assert!(!svg.contains("data-tile="));
}
