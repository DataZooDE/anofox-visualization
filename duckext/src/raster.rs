//! PNG and PDF exports (#764): the server's own dashboard SVG, rasterised
//! (resvg) or turned into a PDF (svg2pdf), with bundled fonts — Hanken
//! Grotesk, the default theme's (`duckext/fonts/`, SIL Open Font License),
//! and for the glyphs it lacks (Greek, arrows, math, ▲ ▼ ✓ ✗ ⚠ …) a subset
//! of DejaVu Sans 2.37 (`DejaVu-LICENSE.txt`, #814) — so the output is the
//! same on every server, fonts installed or not.
//! Nothing is loaded from the file system or the network: an image in the
//! SVG is a `data:` URI already (relative sources were inlined when the SVG
//! was drawn), anything else is dropped.

use resvg::{tiny_skia, usvg};
use std::sync::{Arc, OnceLock};

/// The default theme's family; every other family falls back to it.
const FAMILY: &str = "Hanken Grotesk";
/// Largest PNG (pixels), against a memory blow-up from a huge `width × scale`.
pub const MAX_PIXELS: u64 = 40_000_000;

fn fonts() -> Arc<usvg::fontdb::Database> {
    static DB: OnceLock<Arc<usvg::fontdb::Database>> = OnceLock::new();
    DB.get_or_init(|| {
        let mut db = usvg::fontdb::Database::new();
        db.load_font_data(include_bytes!("../fonts/HankenGrotesk-Regular.ttf").to_vec());
        db.load_font_data(include_bytes!("../fonts/HankenGrotesk-SemiBold.ttf").to_vec());
        // Only a fallback: usvg picks a face that has the glyph. Unicode blocks
        // 0370-03FF, 1D00-1DBF, 2070-209F, 2100-214F, 2190-23FF, 2460-24FF,
        // 25A0-27BF, 27C0-27EF (pyftsubset --no-hinting).
        db.load_font_data(include_bytes!("../fonts/DejaVuSans-Symbols.ttf").to_vec());
        for set in [
            usvg::fontdb::Database::set_serif_family,
            usvg::fontdb::Database::set_sans_serif_family,
            usvg::fontdb::Database::set_monospace_family,
            usvg::fontdb::Database::set_cursive_family,
            usvg::fontdb::Database::set_fantasy_family,
        ] {
            set(&mut db, FAMILY);
        }
        Arc::new(db)
    })
    .clone()
}

fn tree(svg: &str) -> Result<usvg::Tree, String> {
    // The ⓘ after an `::INFO` title marks a hover definition, which a static
    // image can't show (and no bundled font has the glyph).
    let svg = svg.replace('ⓘ', "");
    let opt = usvg::Options {
        font_family: FAMILY.to_string(),
        fontdb: fonts(),
        // Only `data:` images (the default resolver would also read files).
        image_href_resolver: usvg::ImageHrefResolver {
            resolve_data: usvg::ImageHrefResolver::default_data_resolver(),
            resolve_string: Box::new(|_, _| None),
        },
        ..Default::default()
    };
    usvg::Tree::from_str(&svg, &opt).map_err(|e| format!("could not read the dashboard SVG: {e}"))
}

/// The dashboard as a PNG, `scale` device pixels per SVG pixel.
pub fn png(svg: &str, scale: f32) -> Result<Vec<u8>, String> {
    let t = tree(svg)?;
    let size = t
        .size()
        .to_int_size()
        .scale_by(scale)
        .ok_or("the image would be empty")?;
    if size.width() as u64 * size.height() as u64 > MAX_PIXELS {
        return Err(format!(
            "the image would exceed {MAX_PIXELS} pixels: lower width or scale, or export one tab"
        ));
    }
    let mut pixmap =
        tiny_skia::Pixmap::new(size.width(), size.height()).ok_or("the image would be empty")?;
    resvg::render(
        &t,
        tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    pixmap.encode_png().map_err(|e| e.to_string())
}

/// The dashboard as a one-page PDF (vector; the text stays text).
pub fn pdf(svg: &str) -> Result<Vec<u8>, String> {
    let t = tree(svg)?;
    svg2pdf::to_pdf(
        &t,
        svg2pdf::ConversionOptions::default(),
        svg2pdf::PageOptions::default(),
    )
    .map_err(|e| format!("could not make the PDF: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="80" viewBox="0 0 200 80"><rect width="200" height="80" fill="#fff"/><text x="10" y="40" font-family="'Hanken Grotesk',system-ui,sans-serif" font-size="20" font-weight="600">Revenue 15</text><image href="/etc/passwd" width="10" height="10"/></svg>"##;

    #[test]
    fn png_has_the_text_and_the_size() {
        let b = png(SVG, 2.0).unwrap();
        assert_eq!(&b[..8], b"\x89PNG\r\n\x1a\n");
        // 400 x 160 (IHDR width / height, big-endian)
        assert_eq!(u32::from_be_bytes(b[16..20].try_into().unwrap()), 400);
        assert_eq!(u32::from_be_bytes(b[20..24].try_into().unwrap()), 160);
        // the text drew: some pixels are not white
        let img = tiny_skia::Pixmap::decode_png(&b).unwrap();
        assert!(img.pixels().iter().any(|p| p.red() < 128), "no text drawn");
    }

    #[test]
    fn pdf_is_a_pdf_with_the_font() {
        let b = pdf(SVG).unwrap();
        assert!(b.starts_with(b"%PDF-"));
        let s = String::from_utf8_lossy(&b);
        assert!(
            s.contains("HankenGrotesk") || s.contains("Hanken"),
            "font not embedded"
        );
    }

    /// `text` drawn alone, as RGBA pixels.
    fn drawn(text: &str) -> Vec<u8> {
        let svg = format!(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="40"><rect width="200" height="40" fill="#fff"/><text x="4" y="30" font-size="24">{text}</text></svg>"##
        );
        let b = png(&svg, 1.0).unwrap();
        tiny_skia::Pixmap::decode_png(&b).unwrap().data().to_vec()
    }

    #[test]
    fn symbols_hanken_lacks_come_from_the_fallback() {
        // #814: without a face that has them, each draws the same .notdef box.
        let notdef = drawn("\u{E000}");
        for ch in ["λ", "σ", "▲", "▼", "✓", "✗", "⚠", "→", "≤", "ₜ"] {
            let d = drawn(ch);
            assert_ne!(d, notdef, "{ch} drew as a missing glyph");
            assert!(d.chunks(4).any(|p| p[0] < 128), "{ch} drew nothing");
        }
        // The info mark is left out, not drawn as a box.
        assert_eq!(drawn("A ⓘ"), drawn("A "));
    }

    #[test]
    fn huge_images_are_refused() {
        let big = r#"<svg xmlns="http://www.w3.org/2000/svg" width="10000" height="10000"></svg>"#;
        assert!(png(big, 1.0).unwrap_err().contains("pixels"));
    }
}
