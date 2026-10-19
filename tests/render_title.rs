//! `anofox_render` draws a spec's `::TITLE` as a title bar above the chart,
//! as a dashboard card does (#178).

mod common;

use anofox_visualization::host::{render_spec_checked as host_render, RenderLimits};
use anofox_visualization::{
    parse_role, render_spec_checked, render_with, render_with_title, sql, track, Column,
    RenderOptions, Value,
};

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

// ── The static pages (`dashboard file.sql out.html`, `serve --dashboards`) ──
// (#232): the same bar, as its own <svg> before the chart.

/// Columns as the static pages build them (`sql::columns_from_rows`).
fn page_cols(rows: &str, roles: &[(usize, &str)]) -> Vec<Column> {
    let rows = sql::parse_rows_json(rows).unwrap();
    let roles: Vec<_> = roles
        .iter()
        .map(|(i, r)| (*i, parse_role(r).unwrap()))
        .collect();
    sql::columns_from_rows(&rows, &roles)
}

fn page_bars(t1: &str, t2: &str) -> Vec<Column> {
    page_cols(
        &format!(r#"[{{"c0":"a","c1":1,"c2":{t1}}},{{"c0":"b","c1":2,"c2":{t2}}}]"#),
        &[(0, "XAXIS"), (1, "BARCHART"), (2, "TITLE")],
    )
}

/// The title strip and the rest.
fn split_bar(html: &str) -> (&str, &str) {
    let end = html.find("</svg>").unwrap() + "</svg>".len();
    html.split_at(end)
}

const BAR_OPEN: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="460" height="26" viewBox="0 0 460 26"><text x="14" y="20" "#;

#[test]
fn a_static_page_chart_gets_the_title_bar() {
    let o = RenderOptions::default();
    let cols = page_bars(r#""Bins used""#, r#""Bins used""#);
    let html = render_with_title(&cols, &[], 460, 300, &o).unwrap();
    assert!(html.starts_with(BAR_OPEN), "{html}");
    let (bar, chart) = split_bar(&html);
    common::assert_safe_svg(bar, "title bar");
    let doc = roxmltree::Document::parse(bar).unwrap();
    assert_eq!(
        child(doc.root_element(), "text").and_then(|t| t.text()),
        Some("Bins used")
    );
    // The chart is the card's body: 26 px less, otherwise unchanged.
    assert_eq!(chart, render_with(&cols, 460, 274, &o).unwrap());
}

#[test]
fn a_static_page_uses_the_first_non_null_title() {
    let html = render_with_title(
        &page_bars("null", r#""Second""#),
        &[],
        460,
        300,
        &RenderOptions::default(),
    )
    .unwrap();
    let (bar, _) = split_bar(&html);
    let doc = roxmltree::Document::parse(bar).unwrap();
    assert_eq!(
        child(doc.root_element(), "text").and_then(|t| t.text()),
        Some("Second")
    );
}

#[test]
fn a_static_page_without_a_title_is_unchanged() {
    let o = RenderOptions::default();
    for t in ["null", r#""  ""#] {
        let cols = page_bars(t, t);
        assert_eq!(
            render_with_title(&cols, &[], 460, 300, &o).unwrap(),
            render_with(&cols, 460, 300, &o).unwrap(),
            "{t}"
        );
    }
    let plain = page_cols(
        r#"[{"c0":"a","c1":1},{"c0":"b","c1":2}]"#,
        &[(0, "XAXIS"), (1, "BARCHART")],
    );
    assert_eq!(
        render_with_title(&plain, &[], 460, 300, &o).unwrap(),
        render_with(&plain, 460, 300, &o).unwrap()
    );
}

#[test]
fn a_static_kpi_tile_has_no_title_bar() {
    let o = RenderOptions::default();
    let cols = page_cols(
        r#"[{"c0":1234,"c1":"Revenue"}]"#,
        &[(0, "METRIC"), (1, "TITLE")],
    );
    assert_eq!(
        render_with_title(&cols, &[], 460, 300, &o).unwrap(),
        render_with(&cols, 460, 300, &o).unwrap()
    );
}

#[test]
fn a_static_page_title_is_escaped() {
    let raw = "</svg></figure><script>alert(1)</script>&\"'";
    let json = serde_json::to_string(raw).unwrap();
    let html = render_with_title(
        &page_bars(&json, "null"),
        &[],
        460,
        300,
        &RenderOptions::default(),
    )
    .unwrap();
    assert!(!html.contains("<script"), "{html}");
    assert!(!html.contains("</figure>"), "{html}");
    let (bar, _) = split_bar(&html);
    assert!(
        bar.contains(">&lt;/svg&gt;&lt;/figure&gt;&lt;script&gt;alert(1)&lt;/script&gt;&amp;&quot;&#39;</text></svg>"),
        "{bar}"
    );
    common::assert_safe_svg(bar, "escaped static title");
    let doc = roxmltree::Document::parse(bar).unwrap();
    assert_eq!(
        child(doc.root_element(), "text").and_then(|t| t.text()),
        Some(raw)
    );
}

#[test]
fn a_static_page_keeps_the_tracks_under_the_chart() {
    let o = RenderOptions::default();
    let day = |i: i64| Value::DateTime(1_790_000_000 + 86_400 * i);
    let chart = vec![
        Column::new(
            "day",
            anofox_visualization::Role::X,
            (0..20).map(day).collect(),
        ),
        Column::new(
            "y",
            parse_role("LINECHART").unwrap(),
            (0..20).map(|i| Value::Float(i as f64)).collect(),
        ),
        Column::new(
            "t",
            anofox_visualization::Role::Title,
            vec![Value::Str("Stock".into()); 20],
        ),
    ];
    let tracks = vec![(
        vec![
            Column::new("day", anofox_visualization::Role::X, vec![day(3), day(11)]),
            Column::new(
                "k",
                anofox_visualization::Role::Track,
                vec![Value::Str("Promotions".into()); 2],
            ),
        ],
        0,
    )];
    let html = render_with_title(&chart, &tracks, 460, 300, &o).unwrap();
    assert!(html.starts_with(BAR_OPEN), "{html}");
    let (bar, rest) = split_bar(&html);
    assert!(bar.contains(">Stock</text>"));
    assert_eq!(
        rest,
        track::render_stack(&chart, &tracks, 460, 274, &o)
            .unwrap()
            .concat()
    );
    // Without a title: the stack as before.
    let untitled: Vec<Column> = chart[..2].to_vec();
    assert_eq!(
        render_with_title(&untitled, &tracks, 460, 300, &o).unwrap(),
        track::render_stack(&untitled, &tracks, 460, 300, &o)
            .unwrap()
            .concat()
    );
}
