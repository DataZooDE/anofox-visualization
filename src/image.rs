//! `::IMAGE` / `::IMAGE_DARK` / markdown `![alt](src)` source rules — shared
//! logic for the browser's panel renderer and the headless/static renderer
//! (`dashboard.rs`).
//!
//! [`src_ok`] is the single source of truth for which image sources are
//! accepted; `web/app.js::imageSrc()` must give the **same** verdict for
//! every case in `tests/fixtures/image_src_vectors.json` (checked by
//! `tests/image.rs` here and `tests/web/image_src_test.mjs` there).
//!
//! No HTML sanitising of SVG is needed anywhere downstream: every accepted
//! source is drawn through an `<img>` (browser) or an SVG `<image href=…>`
//! (static output) — never inlined into the DOM, `<object>` or `<iframe>` —
//! so a browser runs no script and loads no external resources from inside
//! an SVG used this way.

/// What an image `src` resolves to. `Bad` carries a short machine reason
/// (not shown to a viewer; `lint.rs` turns it into a diagnostic code).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SrcKind {
    /// `data:image/<mime>…` — `mime` is one of [`DATA_IMAGE_TYPES`].
    Data(&'static str),
    /// An `https:` URL (never `http:` — mixed content on a served https page).
    Https,
    /// A relative path that stays inside the dashboard's own folder.
    Relative,
    /// Rejected. Reasons: `"empty"`, `"too-long"`, `"forbidden-character"`,
    /// `"scheme"` (anything but `https:`, including `http:`, `javascript:`,
    /// `data:text/html`, a bare `data:` with an unsupported image type, …),
    /// `"protocol-relative"` (`//host/x`), `"absolute-path"` (a leading `/`),
    /// or `"outside-folder"` (`..`, a `.` segment, a hidden segment, `\`).
    Bad(&'static str),
}

/// `data:image/<t>` MIME subtypes accepted for an image `src` (same list as
/// `host::app_brand::image_url_ok`'s `data:image/…` check and
/// `theme::safe_url`'s broader `data:image/`).
pub const DATA_IMAGE_TYPES: [&str; 6] = ["png", "jpeg", "jpg", "gif", "webp", "svg+xml"];

/// Hard length cap for an image `src` (a `data:` SVG infographic can be
/// large). `lint.rs` additionally warns above 512 000 chars but still
/// accepts the source.
pub const MAX_SRC_CHARS: usize = 2_000_000;

/// Characters never allowed in an image `src`, matching `theme::safe_url`.
fn has_forbidden_char(u: &str) -> bool {
    u.chars()
        .any(|c| "\"'<>()\\`".contains(c) || c.is_control())
}

/// Classify an `::IMAGE` / `::IMAGE_DARK` / markdown image `src`. See
/// [`SrcKind`] for the decision; `web/app.js::imageSrc()` mirrors this
/// exactly (parity guarded by `tests/fixtures/image_src_vectors.json`).
pub fn src_ok(u: &str) -> SrcKind {
    if u.is_empty() {
        return SrcKind::Bad("empty");
    }
    if u.chars().count() > MAX_SRC_CHARS {
        return SrcKind::Bad("too-long");
    }
    if has_forbidden_char(u) {
        return SrcKind::Bad("forbidden-character");
    }
    let low = u.to_ascii_lowercase();
    if low.starts_with("data:") {
        return DATA_IMAGE_TYPES
            .iter()
            .find(|t| {
                let p = format!("data:image/{t}");
                low.starts_with(&format!("{p};")) || low.starts_with(&format!("{p},"))
            })
            .map(|t| SrcKind::Data(t))
            .unwrap_or(SrcKind::Bad("scheme"));
    }
    if low.starts_with("//") {
        return SrcKind::Bad("protocol-relative");
    }
    if u.starts_with('/') {
        return SrcKind::Bad("absolute-path");
    }
    // A scheme is `letters:` where the part before the first `:` has no `/`
    // (so `c:\x` on Windows or a real path with a `:` later never counts).
    if let Some(i) = low.find(':') {
        if !low[..i].contains('/') {
            return if low.starts_with("https:") {
                SrcKind::Https
            } else {
                SrcKind::Bad("scheme")
            };
        }
    }
    // A `\` is already caught by `has_forbidden_char` above. A relative path
    // is never percent-encoded (review of #544): a browser's URL resolver
    // decodes `%2e%2e`/`%2f`/`%5c` before this ever reaches a server, so
    // `assets/%2e%2e/%2e%2e/x` would classify as `Relative` here — passing
    // the lint and the browser's own `imageSrc()` — while actually escaping
    // `assets/` once resolved. Simplest correct rule: reject any `%`.
    if u.contains('%') {
        return SrcKind::Bad("percent-encoded");
    }
    for seg in u.split('/') {
        if seg.is_empty() || seg == "." || seg == ".." || seg.starts_with('.') {
            return SrcKind::Bad("outside-folder");
        }
    }
    SrcKind::Relative
}

/// Is `path` a relative image path under `assets/`? (lint `image/not-in-assets`;
/// decision 5 — only meaningful for [`SrcKind::Relative`] sources.)
pub fn is_under_assets(path: &str) -> bool {
    path == "assets" || path.starts_with("assets/")
}

/// Percent-decode (`%XX` → byte; `+` is left as-is — this is a URI data
/// payload, not a form body).
fn percent_decode(s: &str) -> Vec<u8> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            let hex = |c: u8| match c {
                b'0'..=b'9' => Some(c - b'0'),
                b'a'..=b'f' => Some(c - b'a' + 10),
                b'A'..=b'F' => Some(c - b'A' + 10),
                _ => None,
            };
            if let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2])) {
                out.push(h << 4 | l);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    out
}

/// Decode a standard-alphabet base64 string (padded or not); `None` on any
/// invalid character or truncated group.
fn base64_decode(s: &str) -> Option<Vec<u8>> {
    let val = |c: u8| -> Option<u8> {
        match c {
            b'A'..=b'Z' => Some(c - b'A'),
            b'a'..=b'z' => Some(c - b'a' + 26),
            b'0'..=b'9' => Some(c - b'0' + 52),
            b'+' | b'-' => Some(62),
            b'/' | b'_' => Some(63),
            _ => None,
        }
    };
    let clean: Vec<u8> = s
        .bytes()
        .filter(|c| *c != b'=' && !c.is_ascii_whitespace())
        .collect();
    let mut out = Vec::with_capacity(clean.len() / 4 * 3 + 3);
    let mut chunk = [0u8; 4];
    let mut n = 0usize;
    for c in clean {
        chunk[n] = val(c)?;
        n += 1;
        if n == 4 {
            out.push(chunk[0] << 2 | chunk[1] >> 4);
            out.push(chunk[1] << 4 | chunk[2] >> 2);
            out.push(chunk[2] << 6 | chunk[3]);
            n = 0;
        }
    }
    match n {
        0 => {}
        2 => out.push(chunk[0] << 2 | chunk[1] >> 4),
        3 => {
            out.push(chunk[0] << 2 | chunk[1] >> 4);
            out.push(chunk[1] << 4 | chunk[2] >> 2);
        }
        _ => return None, // a lone leftover byte can't be a full base64 group
    }
    Some(out)
}

/// Decode a `data:` URI into `(mime, bytes)`, handling both `;base64,` and a
/// plain (percent-encoded) payload. `None` if `u` isn't `data:…,…`.
pub fn decode_data_uri(u: &str) -> Option<(String, Vec<u8>)> {
    let rest = u
        .strip_prefix("data:")
        .or_else(|| u.to_ascii_lowercase().starts_with("data:").then(|| &u[5..]))?;
    let comma = rest.find(',')?;
    let meta = &rest[..comma];
    let payload = &rest[comma + 1..];
    let is_b64 = meta.split(';').any(|p| p.eq_ignore_ascii_case("base64"));
    let mime = meta
        .split(';')
        .next()
        .filter(|m| !m.is_empty())
        .unwrap_or("text/plain")
        .to_string();
    let bytes = if is_b64 {
        base64_decode(payload)?
    } else {
        percent_decode(payload)
    };
    Some((mime, bytes))
}

/// `bytes`' own pixel size, by its format signature, or `None` when it can't
/// be read (an unrecognised format, a truncated file, or an SVG with
/// neither a `viewBox` nor `width`+`height` on its root element — lint
/// `image/svg-no-size`). Callers fall back to a 9:16 aspect.
pub fn intrinsic_size(bytes: &[u8], mime: &str) -> Option<(f64, f64)> {
    if mime.eq_ignore_ascii_case("image/svg+xml") || sniff_svg(bytes) {
        return svg_size(bytes);
    }
    png_size(bytes)
        .or_else(|| gif_size(bytes))
        .or_else(|| jpeg_size(bytes))
        .or_else(|| webp_size(bytes))
}

fn sniff_svg(bytes: &[u8]) -> bool {
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(4096)]).to_ascii_lowercase();
    let head = head.trim_start_matches('\u{feff}').trim_start();
    head.starts_with('<') && head.contains("<svg")
}

fn png_size(b: &[u8]) -> Option<(f64, f64)> {
    if b.len() < 24 || !b.starts_with(b"\x89PNG\r\n\x1a\n") || &b[12..16] != b"IHDR" {
        return None;
    }
    let w = u32::from_be_bytes([b[16], b[17], b[18], b[19]]);
    let h = u32::from_be_bytes([b[20], b[21], b[22], b[23]]);
    (w > 0 && h > 0).then_some((w as f64, h as f64))
}

fn gif_size(b: &[u8]) -> Option<(f64, f64)> {
    if b.len() < 10 || !(b.starts_with(b"GIF87a") || b.starts_with(b"GIF89a")) {
        return None;
    }
    let w = u16::from_le_bytes([b[6], b[7]]);
    let h = u16::from_le_bytes([b[8], b[9]]);
    (w > 0 && h > 0).then_some((w as f64, h as f64))
}

fn jpeg_size(b: &[u8]) -> Option<(f64, f64)> {
    if b.len() < 4 || b[0] != 0xFF || b[1] != 0xD8 {
        return None;
    }
    let mut i = 2usize;
    while i + 3 < b.len() {
        if b[i] != 0xFF {
            i += 1;
            continue;
        }
        let marker = b[i + 1];
        // Markers with no payload (RST, SOI, EOI, TEM, 0x01).
        if marker == 0x01 || (0xD0..=0xD9).contains(&marker) {
            i += 2;
            continue;
        }
        if i + 4 > b.len() {
            break;
        }
        let len = u16::from_be_bytes([b[i + 2], b[i + 3]]) as usize;
        let is_sof = (0xC0..=0xCF).contains(&marker) && !matches!(marker, 0xC4 | 0xC8 | 0xCC); // DHT / JPG / DAC
        if is_sof {
            if i + 9 > b.len() {
                return None;
            }
            let h = u16::from_be_bytes([b[i + 5], b[i + 6]]);
            let w = u16::from_be_bytes([b[i + 7], b[i + 8]]);
            return (w > 0 && h > 0).then_some((w as f64, h as f64));
        }
        if marker == 0xD9 || len < 2 {
            break;
        }
        i += 2 + len;
    }
    None
}

fn webp_size(b: &[u8]) -> Option<(f64, f64)> {
    if b.len() < 30 || &b[0..4] != b"RIFF" || &b[8..12] != b"WEBP" {
        return None;
    }
    match &b[12..16] {
        b"VP8X" => {
            let w = 1 + u32::from_le_bytes([b[24], b[25], b[26], 0]);
            let h = 1 + u32::from_le_bytes([b[27], b[28], b[29], 0]);
            Some((w as f64, h as f64))
        }
        b"VP8L" if b.len() >= 25 && b[20] == 0x2F => {
            let bits = u32::from_le_bytes([b[21], b[22], b[23], b[24]]);
            let w = 1 + (bits & 0x3FFF);
            let h = 1 + ((bits >> 14) & 0x3FFF);
            Some((w as f64, h as f64))
        }
        b"VP8 " if b.len() >= 30 && b[23..26] == [0x9d, 0x01, 0x2a] => {
            let w = u16::from_le_bytes([b[26], b[27]]) & 0x3FFF;
            let h = u16::from_le_bytes([b[28], b[29]]) & 0x3FFF;
            (w > 0 && h > 0).then_some((w as f64, h as f64))
        }
        _ => None,
    }
}

/// An SVG root element's size, from `viewBox="minx miny w h"` (preferred) or
/// `width="…" height="…"` (bare numbers / `px`; `%` and other units don't
/// give a pixel aspect, so they're skipped) on the first `<svg …>` tag.
fn svg_size(bytes: &[u8]) -> Option<(f64, f64)> {
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(4096)]);
    let tag_start = head.find("<svg")?;
    let tag_end = head[tag_start..].find('>').map(|e| tag_start + e)?;
    let tag = &head[tag_start..tag_end];
    let attr = |name: &str| -> Option<String> {
        let needle = format!("{name}=\"");
        let i = tag.find(&needle)? + needle.len();
        let j = tag[i..].find('"')? + i;
        Some(tag[i..j].to_string())
    };
    if let Some(vb) = attr("viewBox") {
        let nums: Vec<f64> = vb
            .split_whitespace()
            .filter_map(|n| n.parse().ok())
            .collect();
        if let [_, _, w, h] = nums[..] {
            if w > 0.0 && h > 0.0 {
                return Some((w, h));
            }
        }
    }
    let num = |s: String| -> Option<f64> { s.trim_end_matches("px").trim().parse().ok() };
    match (attr("width").and_then(num), attr("height").and_then(num)) {
        (Some(w), Some(h)) if w > 0.0 && h > 0.0 => Some((w, h)),
        _ => None,
    }
}

/// Fallback aspect (width, height) when [`intrinsic_size`] can't read one
/// (decision 8/9 of plan-514): a tall placeholder, not a wide one, so a
/// missing size doesn't collapse the panel to a sliver.
pub const FALLBACK_ASPECT: (f64, f64) = (9.0, 16.0);

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn vectors_match_rust_and_js_contract() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/image_src_vectors.json"
        );
        let text = fs::read_to_string(path).expect("read image_src_vectors.json");
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        let vectors = v["vectors"].as_array().expect("vectors array");
        assert!(!vectors.is_empty());
        for entry in vectors {
            let src = entry["src"].as_str().expect("src");
            let kind = entry["kind"].as_str().expect("kind");
            let got = match src_ok(src) {
                SrcKind::Data(_) => "data",
                SrcKind::Https => "https",
                SrcKind::Relative => "relative",
                SrcKind::Bad(_) => "bad",
            };
            assert_eq!(got, kind, "src_ok({src:?})");
        }
    }

    #[test]
    fn data_uri_roundtrip() {
        let (mime, bytes) = decode_data_uri("data:image/png;base64,iVBORw0KGgo=").unwrap();
        assert_eq!(mime, "image/png");
        assert!(!bytes.is_empty());
        let (mime, bytes) = decode_data_uri("data:image/svg+xml,%3Csvg%3E").unwrap();
        assert_eq!(mime, "image/svg+xml");
        assert_eq!(bytes, b"<svg>");
    }

    #[test]
    fn https_only_not_http() {
        assert_eq!(src_ok("https://a.b/c.png"), SrcKind::Https);
        assert!(matches!(src_ok("http://a.b/c.png"), SrcKind::Bad(_)));
    }

    #[test]
    fn relative_paths() {
        assert_eq!(src_ok("assets/x.svg"), SrcKind::Relative);
        assert_eq!(src_ok("img/ok.png"), SrcKind::Relative);
        assert!(matches!(
            src_ok("assets/../x"),
            SrcKind::Bad("outside-folder")
        ));
        assert!(matches!(
            src_ok("assets/.hidden"),
            SrcKind::Bad("outside-folder")
        ));
        assert!(matches!(src_ok("/abs"), SrcKind::Bad("absolute-path")));
        assert!(matches!(src_ok("//x"), SrcKind::Bad("protocol-relative")));
        assert!(matches!(
            src_ok("a\\b"),
            SrcKind::Bad("forbidden-character")
        ));
    }

    /// #544 review: a browser's URL resolver decodes `%2e`/`%2f`/`%5c`
    /// before a fetch, so these must never classify as plain `Relative` —
    /// `assets/%2e%2e/x` resolves outside `assets/` once fetched.
    #[test]
    fn percent_encoding_is_rejected_in_a_relative_path() {
        for bad in [
            "assets/%2e%2e/x",
            "assets/%2E%2E%2Fx",
            "assets/..%2fx",
            "assets/..%2Fx",
            "assets%2f..%2fx",
            "assets/%5c..%5cx",
            "assets/x%2esvg",
        ] {
            assert!(
                matches!(src_ok(bad), SrcKind::Bad("percent-encoded")),
                "{bad}"
            );
        }
        // Still fine without any `%`.
        assert_eq!(src_ok("assets/ok.svg"), SrcKind::Relative);
    }

    #[test]
    fn rejects_scripts_and_markup() {
        assert!(matches!(src_ok("javascript:alert(1)"), SrcKind::Bad(_)));
        assert!(matches!(
            src_ok("data:text/html,<h1>x</h1>"),
            SrcKind::Bad(_)
        ));
        assert!(matches!(
            src_ok("x\"y"),
            SrcKind::Bad("forbidden-character")
        ));
        assert!(matches!(
            src_ok("x(y)"),
            SrcKind::Bad("forbidden-character")
        ));
        assert!(matches!(src_ok("x<y"), SrcKind::Bad("forbidden-character")));
    }

    #[test]
    fn length_cap() {
        let long = format!("data:image/png;base64,{}", "A".repeat(MAX_SRC_CHARS));
        assert!(matches!(src_ok(&long), SrcKind::Bad("too-long")));
    }

    #[test]
    fn png_header() {
        // 4x2 1-bit PNG IHDR (signature + length/type + width/height + rest irrelevant here).
        let mut b = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        b.extend([0, 0, 0, 13]); // chunk length (unused by our parser)
        b.extend(b"IHDR");
        b.extend(4u32.to_be_bytes()); // width
        b.extend(2u32.to_be_bytes()); // height
        assert_eq!(intrinsic_size(&b, "image/png"), Some((4.0, 2.0)));
    }

    #[test]
    fn svg_viewbox() {
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 300 150"><rect/></svg>"#;
        assert_eq!(intrinsic_size(svg, "image/svg+xml"), Some((300.0, 150.0)));
    }

    #[test]
    fn svg_no_size_is_none() {
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg"><rect/></svg>"#;
        assert_eq!(intrinsic_size(svg, "image/svg+xml"), None);
    }
}
