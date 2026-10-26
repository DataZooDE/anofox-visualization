//! #514 review (PR #544): the script-free `serve --dashboards` page and
//! `render_spec`/`render_with_title` go through `lib.rs::render_inner`, a
//! *different*, simpler single-panel renderer than `dashboard.rs`'s own
//! `render_panel_svg` (which the full dashboard / `--svg` path uses). Before
//! this fix, an `::IMAGE` panel there had no measure column, so it fell
//! through to the "no measure column" error — a generic "panel render
//! failed" card on every request. It now mirrors `dashboard.rs`'s own
//! `::IMAGE` rules: a `data:`/`https:` source draws the image, a relative
//! path or a rejected source draws the alt-text placeholder (this render
//! path can't yet serve `assets/…` — PR B).

use anofox_visualization::{
    render_spec_checked, render_with_title, Column, RenderOptions, Role, Value,
};

#[test]
fn an_https_image_renders_not_an_error_card() {
    let spec = r#"{"rows":[{"c0":"https://x/i.png","c1":"alt text"}],"roles":[[0,"IMAGE"],[1,"ALT"]],"width":460,"height":300}"#;
    let svg = render_spec_checked(spec).unwrap();
    assert!(svg.contains("<image "), "{svg}");
    assert!(!svg.contains("render failed"), "{svg}");
}

#[test]
fn a_relative_src_is_the_alt_placeholder_not_an_error_card() {
    let spec = r#"{"rows":[{"c0":"assets/x.svg","c1":"alt text"}],"roles":[[0,"IMAGE"],[1,"ALT"]],"width":460,"height":300}"#;
    let svg = render_spec_checked(spec).unwrap();
    assert!(!svg.contains("<image "), "{svg}");
    assert!(svg.contains("alt text"), "{svg}");
    assert!(svg.contains("stroke-dasharray"), "{svg}");
    assert!(!svg.contains("render failed"), "{svg}");
}

#[test]
fn render_with_title_the_actual_serve_rs_call_site_handles_image_columns() {
    let cols = vec![
        Column::new(
            "c0",
            Role::Image,
            vec![Value::Str("https://x/i.png".into())],
        ),
        Column::new("c1", Role::Alt, vec![Value::Str("alt text".into())]),
    ];
    let svg = render_with_title(&cols, &[], 460, 300, &RenderOptions::default()).unwrap();
    assert!(svg.contains("<image "), "{svg}");
}

/// #544 re-review: without an `::ALT`, the alt text falls back to
/// `::TITLE`, then `::LABEL` — the same order as `dashboard.rs`'s own
/// `image_panel` (not `::LABEL` first). Uses `render_spec_checked` (→
/// `render_with`, not `render_with_title`), so there's no separate title
/// bar drawn from `::LABEL` to confound the assertion.
#[test]
fn alt_falls_back_to_title_then_label_like_dashboard_rs() {
    let title_and_label = r#"{"rows":[{"c0":"javascript:alert(1)","c1":"the title","c2":"the label"}],"roles":[[0,"IMAGE"],[1,"TITLE"],[2,"LABEL"]],"width":460,"height":300}"#;
    let svg = render_spec_checked(title_and_label).unwrap();
    assert!(svg.contains("the title"), "{svg}");
    assert!(!svg.contains("the label"), "{svg}");

    let label_only = r#"{"rows":[{"c0":"javascript:alert(1)","c1":"the label"}],"roles":[[0,"IMAGE"],[1,"LABEL"]],"width":460,"height":300}"#;
    let svg = render_spec_checked(label_only).unwrap();
    assert!(svg.contains("the label"), "{svg}");
}
