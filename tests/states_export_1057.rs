//! #1057: a `::REGION` map with states or provinces outside Germany
//! (`world-states`, ISO 3166-2) drew no region natively — `dashboard --svg`
//! and the server's exports — because only the browser loaded that layer.
//! A host that has the layer's text (the extension and `serve` embed
//! `web/`) now hands it to the renderer with `set_boundary_source`.

use std::sync::OnceLock;

use anofox_visualization::{render_with, set_boundary_source, Column, Kind, RenderOptions, Role};
use ggplot_rs::prelude::Value;

static STATES: OnceLock<String> = OnceLock::new();

fn source(name: &str) -> Option<&'static str> {
    (name == "world-states").then(|| {
        STATES
            .get_or_init(|| {
                std::fs::read_to_string(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/web/data/boundaries/world-states.csv"
                ))
                .expect("web/data/boundaries/world-states.csv")
            })
            .as_str()
    })
}

fn map() -> String {
    let cols = vec![
        Column::new(
            "code",
            Role::Region,
            ["AT-1", "AT-9", "CH-ZG", "CH-ZH"]
                .iter()
                .map(|s| Value::Str(s.to_string()))
                .collect(),
        ),
        Column::new(
            "v",
            Role::Value(Kind::Bar),
            [10.0, 20.0, 5.0, 8.0]
                .iter()
                .map(|&f| Value::Float(f))
                .collect(),
        ),
    ];
    render_with(&cols, 480, 320, &RenderOptions::default()).expect("renders")
}

/// The drawn regions (features carry a `data-row`).
fn regions(svg: &str) -> usize {
    svg.matches(" data-row=\"").count()
}

#[test]
fn state_maps_draw_once_the_host_gives_the_layer() {
    // without a source: the browser-only layer, nothing drawn natively
    assert_eq!(regions(&map()), 0);
    set_boundary_source(source);
    let svg = map();
    assert!(
        regions(&svg) >= 4,
        "the four states are drawn: {} features",
        regions(&svg)
    );
    assert!(!svg.contains(">No data<"), "no 'No data' card");
}
