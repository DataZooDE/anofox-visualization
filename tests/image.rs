//! `::IMAGE` / `::IMAGE_DARK` rendering through [`render_dashboard_svg`]
//! (#514 PR A): the SVG `<image>` element's aspect, the dark-variant switch,
//! and the relative-path placeholder (PR B inlines the file instead).

use anofox_visualization::dashboard::{render_dashboard_svg, DashboardOptions, DataProvider};
use anofox_visualization::host::app_brand::data_uri;
use anofox_visualization::theme::Variant;
use ggplot_rs::prelude::Value;

/// A minimal (not fully spec-compliant, but enough for
/// `src/image.rs::intrinsic_size`'s IHDR reader) PNG with the given pixel
/// size.
fn png_bytes(w: u32, h: u32) -> Vec<u8> {
    let mut b = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    b.extend(13u32.to_be_bytes()); // IHDR chunk length
    b.extend(b"IHDR");
    b.extend(w.to_be_bytes());
    b.extend(h.to_be_bytes());
    b.extend([8, 2, 0, 0, 0]); // depth, color type, compression, filter, interlace
    b.extend(0u32.to_be_bytes()); // CRC (unchecked by our reader)
    b
}

/// Returns each statement's canned rows, by call order (one `query` call per
/// rendered statement; `execute` handles setup, not counted here).
struct Steps(std::cell::RefCell<std::vec::IntoIter<Vec<(&'static str, Value)>>>);
impl Steps {
    fn new(steps: Vec<Vec<(&'static str, Value)>>) -> Self {
        Steps(std::cell::RefCell::new(steps.into_iter()))
    }
}
impl DataProvider for Steps {
    fn execute(&mut self, _sql: &str) -> Result<(), String> {
        Ok(())
    }
    fn query(&mut self, _sql: &str) -> Result<Vec<(String, Vec<Value>)>, String> {
        let row = self.0.borrow_mut().next().unwrap_or_default();
        Ok(row
            .into_iter()
            .map(|(name, v)| (name.to_string(), vec![v]))
            .collect())
    }
}

fn s(v: &str) -> Value {
    Value::Str(v.to_string())
}

/// Pull out the first `<image … >` tag's `width`/`height` attributes.
fn image_wh(svg: &str) -> Option<(f64, f64)> {
    let i = svg.find("<image ")?;
    let tag_end = svg[i..].find('>').map(|e| i + e)?;
    let tag = &svg[i..tag_end];
    let attr = |name: &str| -> Option<f64> {
        let needle = format!("{name}=\"");
        let a = tag.find(&needle)? + needle.len();
        let b = tag[a..].find('"')? + a;
        tag[a..b].parse().ok()
    };
    Some((attr("width")?, attr("height")?))
}

#[test]
fn data_png_image_keeps_its_own_aspect() {
    // A 4x2 PNG: height must come out to half the drawn width.
    let src = data_uri("image/png", &png_bytes(4, 2));
    let script = format!("SELECT '{src}'::IMAGE, 'alt text'::ALT;");
    let mut provider = Steps::new(vec![vec![("c0", s(&src)), ("c1", s("alt text"))]]);
    let opts = DashboardOptions {
        width: 400,
        columns: 1,
        ..DashboardOptions::default()
    };
    let svg = render_dashboard_svg(&script, &mut provider, &opts).unwrap();
    assert!(svg.contains("<image "), "no <image> element: {svg}");
    assert!(svg.contains(&src), "href isn't the data: URI verbatim");
    let (w, h) = image_wh(&svg).expect("width/height attrs");
    assert!(
        (h / w - 0.5).abs() < 0.02,
        "aspect not preserved: {w}x{h} (want 2:1)"
    );
    // The alt text is a <title>, not DOM markup.
    assert!(svg.contains("<title>alt text</title>"));
}

#[test]
fn image_dark_is_used_for_the_dark_variant() {
    let light = data_uri("image/png", &png_bytes(4, 2));
    let dark = data_uri("image/png", &png_bytes(2, 2));
    let script = format!("SELECT '{light}'::IMAGE, '{dark}'::IMAGE_DARK, 'alt'::ALT;");
    let opts_light = DashboardOptions {
        variant: Variant::Light,
        ..DashboardOptions::default()
    };
    let opts_dark = DashboardOptions {
        variant: Variant::Dark,
        ..DashboardOptions::default()
    };
    let mut p1 = Steps::new(vec![vec![
        ("c0", s(&light)),
        ("c1", s(&dark)),
        ("c2", s("alt")),
    ]]);
    let svg_light = render_dashboard_svg(&script, &mut p1, &opts_light).unwrap();
    assert!(
        svg_light.contains(&light),
        "light variant should use ::IMAGE"
    );
    assert!(
        !svg_light.contains(&dark),
        "light variant leaked ::IMAGE_DARK"
    );

    let mut p2 = Steps::new(vec![vec![
        ("c0", s(&light)),
        ("c1", s(&dark)),
        ("c2", s("alt")),
    ]]);
    let svg_dark = render_dashboard_svg(&script, &mut p2, &opts_dark).unwrap();
    assert!(
        svg_dark.contains(&dark),
        "dark variant should use ::IMAGE_DARK"
    );
}

#[test]
fn relative_path_draws_the_placeholder_not_a_broken_image() {
    let script = "SELECT 'assets/concept.svg'::IMAGE, 'a drift diagram'::ALT;";
    let mut provider = Steps::new(vec![vec![
        ("c0", s("assets/concept.svg")),
        ("c1", s("a drift diagram")),
    ]]);
    let svg = render_dashboard_svg(script, &mut provider, &DashboardOptions::default()).unwrap();
    assert!(
        !svg.contains("<image "),
        "PR A must not inline a relative path: {svg}"
    );
    assert!(svg.contains("stroke-dasharray"), "no placeholder box");
    assert!(
        svg.contains("a drift diagram"),
        "alt text missing from the placeholder"
    );
}

#[test]
fn bad_src_also_draws_the_placeholder() {
    let script = "SELECT 'javascript:alert(1)'::IMAGE, 'nope'::ALT;";
    let mut provider = Steps::new(vec![vec![
        ("c0", s("javascript:alert(1)")),
        ("c1", s("nope")),
    ]]);
    let svg = render_dashboard_svg(script, &mut provider, &DashboardOptions::default()).unwrap();
    assert!(!svg.contains("<image "));
    assert!(
        !svg.contains("javascript:"),
        "the rejected src must never reach the SVG"
    );
    assert!(svg.contains("nope"));
}
