//! #568: the script-free `serve --dashboards` page and `render_spec`/
//! `render_with_title` go through `lib.rs::render_inner`, a *different*,
//! simpler single-panel renderer than `dashboard.rs`'s own `render_panel_svg`
//! (which the full dashboard / `--svg` path uses, and which has always had
//! working `::MARKDOWN` rendering). Before this fix, a `::MARKDOWN` panel on
//! this render path had no measure column, so it fell through every other
//! branch (Track, Image, Kpi, Map) to the "no measure column" case, which
//! just returns a title-only heading — the panel's text never appeared at
//! all on the CLI HTML export or the locked server's script-free page. This
//! now mirrors `dashboard.rs`'s own `::MARKDOWN` branch, reusing its text
//! machinery (`markdown_text`/`markdown_fill`/`markdown_lines`/
//! `markdown_line_svg`) rather than a second SVG text writer.

use anofox_visualization::{
    render_spec_checked, render_with_title, Column, RenderOptions, Role, Value,
};

#[test]
fn a_markdown_panel_renders_its_text_not_an_empty_heading() {
    let spec =
        r#"{"rows":[{"c0":"hello world"}],"roles":[[0,"MARKDOWN"]],"width":460,"height":300}"#;
    let svg = render_spec_checked(spec).unwrap();
    assert!(svg.contains("hello world"), "{svg}");
}

#[test]
fn render_with_title_the_actual_serve_rs_call_site_handles_markdown_columns() {
    let cols = vec![
        Column::new("c0", Role::Markdown, vec![Value::Str("hello world".into())]),
        Column::new("c1", Role::Title, vec![Value::Str("a title".into())]),
    ];
    let svg = render_with_title(&cols, &[], 460, 300, &RenderOptions::default()).unwrap();
    assert!(svg.contains("hello world"), "{svg}");
    // The title bar is drawn separately by render_with_title itself, not a
    // second time inside the markdown body.
    assert!(svg.contains("a title"), "{svg}");
}

// #327-style regression on this new call site: `{{col}}` / `{{col:fmt}}`
// placeholders must be filled from the panel's own row, not printed
// literally.
#[test]
fn placeholders_are_filled_not_printed_literally() {
    let cols = vec![
        Column::new(
            "c0",
            Role::Markdown,
            vec![Value::Str("z is {{z}} at {{lvl}}".into())],
        ),
        // These two carry no chart role of their own (same as a `md:z`
        // helper column from the real `::MARKDOWN` SQL macro) — any inert
        // role works here since `render_inner` returns from the markdown
        // branch before any later role check would see them.
        Column::new("z", Role::Alt, vec![Value::Float(1.64)]),
        Column::new("lvl", Role::Alt, vec![Value::Str("95 %".into())]),
    ];
    let svg = render_with_title(&cols, &[], 460, 300, &RenderOptions::default()).unwrap();
    assert!(!svg.contains("{{z}}"), "{svg}");
    assert!(!svg.contains("{{lvl}}"), "{svg}");
    assert!(svg.contains("95"), "{svg}");
}

// SECURITY (#568 touches a second caller of the markdown text path that
// `src/bin/serve.rs`'s script-free page uses): markdown text must come out
// escaped in the SVG, never as a literal tag, on this path too — same as
// the already-reviewed `--svg` composer (`markdown_line_svg` -> `esc()` ==
// `format::escape_xml`).
#[test]
fn markdown_text_is_escaped_not_injected_as_raw_markup() {
    let spec = r#"{"rows":[{"c0":"<script>alert(1)</script>"}],"roles":[[0,"MARKDOWN"]],"width":460,"height":300}"#;
    let svg = render_spec_checked(spec).unwrap();
    assert!(!svg.contains("<script>"), "{svg}");
    assert!(svg.contains("&lt;script&gt;"), "{svg}");

    let spec = r#"{"rows":[{"c0":"<img src=x onerror=alert(1)>"}],"roles":[[0,"MARKDOWN"]],"width":460,"height":300}"#;
    let svg = render_spec_checked(spec).unwrap();
    assert!(!svg.contains("<img "), "{svg}");
    assert!(svg.contains("&lt;img "), "{svg}");
}

// #514 interplay: a markdown image has no pixels on this headless path
// either (`markdown_lines` turns `![alt](src)` into plain `[alt]` text) —
// confirm that still holds through this new call site, not a raw `<image>`
// tag or the literal markdown syntax.
#[test]
fn a_markdown_image_is_bracketed_alt_text_not_an_image_tag() {
    let spec = r#"{"rows":[{"c0":"![a chart](assets/x.png)"}],"roles":[[0,"MARKDOWN"]],"width":460,"height":300}"#;
    let svg = render_spec_checked(spec).unwrap();
    assert!(svg.contains("[a chart]"), "{svg}");
    assert!(!svg.contains("<image "), "{svg}");
    assert!(!svg.contains("](assets/x.png)"), "{svg}");
}
