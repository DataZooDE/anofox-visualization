//! #514 review (PR #544): the script-free `serve --dashboards` page and
//! `render_spec`/`render_with_title` go through `lib.rs::render_inner`, a
//! *different*, simpler single-panel renderer than `dashboard.rs`'s own
//! `render_panel_svg` (which the full dashboard / `--svg` path uses). Before
//! this fix, an `::IMAGE` panel there had no measure column, so it fell
//! through to the "no measure column" error — a generic "panel render
//! failed" card on every request. It now mirrors `dashboard.rs`'s own
//! `::IMAGE` rules: a `data:`/`https:` source draws the image, a relative
//! path or a rejected source draws the alt-text placeholder (this render
//! path can't yet serve `assets/…` — PR B). PR B: when the caller gives
//! `RenderOptions.asset_root` (the dashboard's own `assets/` folder,
//! canonicalized — `src/bin/serve.rs`'s script-free page sets it from its
//! `--dashboards <dir>`), a relative source under `assets/` is inlined from
//! it as a `data:` URI too, via the same `crate::image::resolve_relative`
//! `dashboard.rs` uses. `render_spec_checked`'s JSON spec has no path to
//! give one (it's also the wasm entry point, which has no filesystem), so
//! that case is exercised directly through `RenderOptions` below instead.

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

/// A scratch `<dir>/assets/` folder, removed on drop.
struct AssetsDir(std::path::PathBuf);
impl AssetsDir {
    fn new() -> AssetsDir {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "anofox-image-serve-514-test-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(dir.join("assets")).unwrap();
        AssetsDir(dir)
    }
}
impl Drop for AssetsDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// #514 PR B: `src/bin/serve.rs`'s script-free page sets
/// `RenderOptions.asset_root` to the `--dashboards <dir>`'s own `assets/`
/// folder; a relative `::IMAGE` source under it is inlined from there, the
/// same as `dashboard.rs`'s full-dashboard `--svg` path.
#[test]
fn render_with_title_inlines_a_relative_asset_when_a_root_is_given() {
    let d = AssetsDir::new();
    std::fs::write(
        d.0.join("assets").join("x.svg"),
        br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"/>"#,
    )
    .unwrap();
    let root = d.0.join("assets").canonicalize().unwrap();
    let cols = vec![
        Column::new("c0", Role::Image, vec![Value::Str("assets/x.svg".into())]),
        Column::new("c1", Role::Alt, vec![Value::Str("alt text".into())]),
    ];
    let o = RenderOptions {
        asset_root: Some(root),
        ..RenderOptions::default()
    };
    let svg = render_with_title(&cols, &[], 460, 300, &o).unwrap();
    assert!(svg.contains("<image "), "{svg}");
    assert!(svg.contains("data:image/svg+xml;base64,"), "{svg}");

    // No root at all: still the placeholder, exactly as before PR B.
    let svg = render_with_title(&cols, &[], 460, 300, &RenderOptions::default()).unwrap();
    assert!(!svg.contains("<image "), "{svg}");
    assert!(svg.contains("alt text"), "{svg}");
}
