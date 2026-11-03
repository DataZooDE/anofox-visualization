//! #889: viewers' notes from the render context — a rule at an x, a shaded
//! band over a range, the text as the tooltip, the id as `data-note`.

use anofox_visualization::{render_with, Column, Kind, RenderOptions, Role};
use ggplot_rs::prelude::Value;

fn days() -> Vec<Column> {
    let d = [
        "2026-02-01",
        "2026-02-02",
        "2026-02-03",
        "2026-02-04",
        "2026-02-05",
    ];
    vec![
        Column::new(
            "d",
            Role::X,
            d.iter().map(|s| Value::Str(s.to_string())).collect(),
        ),
        Column::new(
            "v",
            Role::Value(Kind::Line),
            [3.0, 5.0, 4.0, 6.0, 7.0]
                .iter()
                .map(|&f| Value::Float(f))
                .collect(),
        ),
    ]
}

fn ctx(notes: serde_json::Value) -> RenderOptions {
    RenderOptions::from_primary_arg(&serde_json::json!({ "notes": notes }).to_string()).unwrap()
}

/// The `<title>` text of the mark carrying `data-note="{id}"`.
fn note_tip(svg: &str, id: &str) -> Option<String> {
    let at = svg.find(&format!("data-note=\"{id}\""))?;
    let start = svg[..at].rfind('<')?;
    let rest = &svg[start..];
    let t = rest.find("<title>")? + "<title>".len();
    let end = rest[t..].find("</title>")?;
    Some(rest[t..t + end].to_string())
}

#[test]
fn a_point_note_and_a_range_note_draw_with_their_text() {
    let o = ctx(serde_json::json!([
        {"id": "n1", "x": "2026-02-03", "text": "Promotion", "by": "ada", "at": "2026-11-03"},
        {"id": "n2", "x": "2026-02-04", "x2": "2026-02-05", "text": "Data error <b>", "by": "bob"}
    ]));
    let svg = render_with(&days(), 480, 320, &o).unwrap();
    assert_eq!(
        note_tip(&svg, "n1").as_deref(),
        Some("Promotion — ada, 2026-11-03"),
        "{svg}"
    );
    // the text is escaped like any tooltip, never markup
    assert_eq!(
        note_tip(&svg, "n2").as_deref(),
        Some("Data error &lt;b&gt; — bob")
    );
    assert!(!svg.contains("<b>"));
}

#[test]
fn no_notes_or_unusable_ones_change_nothing() {
    let plain = render_with(&days(), 480, 320, &RenderOptions::default()).unwrap();
    let o = ctx(serde_json::json!([
        {"id": "n1", "x": "not a date", "text": "x"},
        {"x": "2026-02-03", "text": "no id"},
        "junk"
    ]));
    let svg = render_with(&days(), 480, 320, &o).unwrap();
    assert!(!svg.contains("data-note"), "{svg}");
    assert_eq!(svg.len(), plain.len());
}

#[test]
fn a_date_axis_takes_a_rule_and_a_band() {
    let t0: i64 = 1_769_904_000; // 2026-02-01
    let cols = vec![
        Column::new(
            "d",
            Role::X,
            (0..5).map(|i| Value::DateTime(t0 + i * 86_400)).collect(),
        ),
        Column::new(
            "v",
            Role::Value(Kind::Line),
            [3.0, 5.0, 4.0, 6.0, 7.0]
                .iter()
                .map(|&f| Value::Float(f))
                .collect(),
        ),
    ];
    let o = ctx(serde_json::json!([
        {"id": "n1", "x": "2026-02-03", "text": "Promotion"},
        {"id": "n2", "x": "2026-02-04", "x2": "2026-02-02", "text": "Outage"},
        {"id": "n3", "x": "soon", "text": "skipped"}
    ]));
    let svg = render_with(&cols, 480, 320, &o).unwrap();
    assert_eq!(note_tip(&svg, "n1").as_deref(), Some("Promotion"), "{svg}");
    assert_eq!(note_tip(&svg, "n2").as_deref(), Some("Outage"));
    assert!(!svg.contains("data-note=\"n3\""));
}
