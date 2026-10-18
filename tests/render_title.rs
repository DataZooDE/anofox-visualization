//! `anofox_render` draws a spec's `::TITLE` as a title bar above the chart,
//! as a dashboard card does (#178).

mod common;

use anofox_visualization::host::{render_spec_checked as host_render, RenderLimits};
use anofox_visualization::render_spec_checked;

const BARS: &str = r#"{"rows":[{"c0":"a","c1":1,"c2":"Bins used"},{"c0":"b","c1":2,"c2":"Bins used"}],"roles":[[0,"XAXIS"],[1,"BARCHART"],[2,"TITLE"]]}"#;

/// The bar chart of [`BARS`] with `t1` / `t2` (JSON values) in its TITLE column.
fn bars_titled(t1: &str, t2: &str) -> String {
    format!(
        r#"{{"rows":[{{"c0":"a","c1":1,"c2":{t1}}},{{"c0":"b","c1":2,"c2":{t2}}}],"roles":[[0,"XAXIS"],[1,"BARCHART"],[2,"TITLE"]]}}"#
    )
}

fn child<'a, 'i>(n: roxmltree::Node<'a, 'i>, tag: &str) -> Option<roxmltree::Node<'a, 'i>> {
    n.children().find(|c| c.tag_name().name() == tag)
}

#[test]
fn title_is_drawn_above_the_chart() {
    let svg = render_spec_checked(BARS).unwrap();
    common::assert_safe_svg(&svg, "titled bars");
    let doc = roxmltree::Document::parse(&svg).unwrap();
    let root = doc.root_element();
    assert_eq!(root.attribute("width"), Some("640"));
    assert_eq!(root.attribute("height"), Some("400"));
    assert_eq!(
        child(root, "text").and_then(|t| t.text()),
        Some("Bins used")
    );
    let g = child(root, "g").expect("the chart group");
    assert_eq!(g.attribute("transform"), Some("translate(0,26)"));
    let chart = child(g, "svg").expect("the nested chart");
    assert_eq!(chart.attribute("height"), Some("374"));
    assert!(
        chart
            .descendants()
            .filter(|n| n.tag_name().name() == "rect")
            .count()
            >= 2
    );
    // The extension's path (anofox_render) draws the same.
    assert_eq!(host_render(BARS, &RenderLimits::default()).unwrap(), svg);
}

#[test]
fn the_first_non_null_title_is_used() {
    let svg = render_spec_checked(&bars_titled("null", r#""Second""#)).unwrap();
    let doc = roxmltree::Document::parse(&svg).unwrap();
    assert_eq!(
        child(doc.root_element(), "text").and_then(|t| t.text()),
        Some("Second")
    );
}

#[test]
fn a_null_or_blank_title_draws_no_bar() {
    let plain = render_spec_checked(
        r#"{"rows":[{"c0":"a","c1":1},{"c0":"b","c1":2}],"roles":[[0,"XAXIS"],[1,"BARCHART"]]}"#,
    )
    .unwrap();
    for t in ["null", r#""  ""#] {
        assert_eq!(
            render_spec_checked(&bars_titled(t, t)).unwrap(),
            plain,
            "{t}"
        );
    }
}

#[test]
fn the_title_is_escaped() {
    let svg = render_spec_checked(&bars_titled(r#""<b>&\"x""#, "null")).unwrap();
    common::assert_safe_svg(&svg, "escaped title");
    assert!(svg.contains("&lt;b&gt;&amp;"), "{svg}");
    let doc = roxmltree::Document::parse(&svg).unwrap();
    assert_eq!(
        child(doc.root_element(), "text").and_then(|t| t.text()),
        Some("<b>&\"x")
    );
}

/// A KPI tile has no title bar (its caption is `::LABEL`), as in a dashboard.
#[test]
fn a_kpi_tile_draws_no_bar() {
    let plain = render_spec_checked(r#"{"rows":[{"c0":1234}],"roles":[[0,"METRIC"]]}"#).unwrap();
    let titled = render_spec_checked(
        r#"{"rows":[{"c0":1234,"c1":"Revenue"}],"roles":[[0,"METRIC"],[1,"TITLE"]]}"#,
    )
    .unwrap();
    assert_eq!(titled, plain);
}

/// The chart's build warnings stay on the root `<svg>` (the conformance
/// harnesses read them there).
#[test]
fn warnings_stay_on_the_root() {
    let svg = render_spec_checked(
        r#"{"rows":[{"c0":"a","c1":1,"c2":"T"},{"c0":"b","c1":1,"c2":"T"}],"roles":[[0,"CATEGORY"],[1,"DENSITY"],[2,"TITLE"]]}"#,
    )
    .unwrap();
    let doc = roxmltree::Document::parse(&svg).unwrap();
    let w = doc
        .root_element()
        .attribute("data-warnings")
        .expect("data-warnings on the root");
    let w: Vec<String> = serde_json::from_str(w).unwrap();
    assert_eq!(w.len(), 1, "{w:?}");
}
