//! The builder's app bar, per installation: the name, tagline and logo shown at
//! the top of the browser builder (not a dashboard's own `::BRAND` header).
//!
//! The serving processes take it from flags (`serve --app-name …`) or options
//! (`anofox_serve(port, '{"name": …}')`), validate it here, and put it in
//! `index.html` as `window.__anofoxApp`; the builder merges it over its
//! defaults and an optional `app-config.json` (web/app.js `appBrandMerge`,
//! which repeats these rules). The sidebar footer is not part of it.

use serde_json::{Map, Value};

/// The builder's settings file for static hosting. `include_dir!` embeds
/// whatever is in `web/` at build time, so the servers refuse to serve it:
/// a binary is configured by its flags / options only.
pub const STATIC_CONFIG: &str = "app-config.json";

/// Largest logo / favicon *file* the CLI inlines.
pub const MAX_IMAGE_BYTES: usize = 512 << 10;
/// Longest image value (a 512 KiB file as a base64 `data:` URI, plus the prefix).
const MAX_IMAGE_CHARS: usize = (MAX_IMAGE_BYTES / 3 + 1) * 4 + 64;
const MAX_NAME: usize = 80;
const MAX_TAGLINE: usize = 160;

/// The text keys, in the order the builder documents them (plus the boolean
/// `name_visible`).
pub const KEYS: [&str; 5] = ["name", "tagline", "logo", "link", "favicon"];

/// Validated app-bar settings; `None` = not set here (the builder's default
/// applies). An empty tagline hides it; `name_visible: false` shows the logo
/// alone (a wordmark), with the name as its alt text.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct AppBrand {
    pub name: Option<String>,
    pub tagline: Option<String>,
    pub logo: Option<String>,
    pub link: Option<String>,
    pub favicon: Option<String>,
    pub name_visible: Option<bool>,
}

fn has_control(s: &str) -> bool {
    s.chars().any(char::is_control)
}

/// An image the app bar may draw: `data:image/(png|jpeg|gif|webp|svg+xml)` or
/// an absolute http(s) URL, no quotes / brackets / control characters.
pub fn image_url_ok(u: &str) -> bool {
    let low = u.to_ascii_lowercase();
    if u.is_empty() || u.len() > MAX_IMAGE_CHARS || has_control(u) {
        return false;
    }
    if u.contains(['"', '\'', '<', '>', '(', ')', '\\', '`', ' ']) {
        return false;
    }
    let data = ["png", "jpeg", "jpg", "gif", "webp", "svg+xml"]
        .iter()
        .any(|t| {
            low.starts_with(&format!("data:image/{t};"))
                || low.starts_with(&format!("data:image/{t},"))
        });
    data || link_ok(u)
}

/// A link target: an absolute http(s) URL.
pub fn link_ok(u: &str) -> bool {
    let low = u.to_ascii_lowercase();
    let host = low
        .strip_prefix("https://")
        .or_else(|| low.strip_prefix("http://"));
    host.is_some_and(|h| !h.is_empty() && !h.starts_with('/'))
        && u.len() <= 2048
        && !has_control(u)
        && !u.contains(['"', '\'', '<', '>', '\\', '`', ' '])
}

/// The image type of `bytes` by its signature (an SVG by its root element).
pub fn sniff_image(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.len() > 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        let head = String::from_utf8_lossy(&bytes[..bytes.len().min(4096)]).to_ascii_lowercase();
        let head = head.trim_start_matches('\u{feff}').trim_start();
        (head.starts_with('<') && head.contains("<svg")).then_some("image/svg+xml")
    }
}

/// `bytes` as a base64 `data:` URI (standard alphabet, padded).
pub fn data_uri(mime: &str, bytes: &[u8]) -> String {
    const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = format!("data:{mime};base64,");
    out.reserve(bytes.len().div_ceil(3) * 4);
    for c in bytes.chunks(3) {
        let n = (c[0] as u32) << 16
            | (*c.get(1).unwrap_or(&0) as u32) << 8
            | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= c.len() {
                out.push(B64[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// An image file as a `data:` URI: refuses anything that isn't a PNG, JPEG,
/// GIF, WebP or SVG, and files over [`MAX_IMAGE_BYTES`].
pub fn image_file(path: &std::path::Path) -> Result<String, String> {
    let shown = path.display();
    let len = std::fs::metadata(path)
        .map_err(|e| format!("'{shown}': {e}"))?
        .len();
    if len > MAX_IMAGE_BYTES as u64 {
        return Err(format!(
            "'{shown}' is {} KB — the limit is {} KB (shrink or compress the image)",
            len.div_ceil(1024),
            MAX_IMAGE_BYTES >> 10
        ));
    }
    let bytes = std::fs::read(path).map_err(|e| format!("'{shown}': {e}"))?;
    let mime = sniff_image(&bytes)
        .ok_or_else(|| format!("'{shown}' is not a PNG, JPEG, GIF, WebP or SVG image"))?;
    Ok(data_uri(mime, &bytes))
}

impl AppBrand {
    /// Set one key from text. `logo` / `favicon` take an image URL or
    /// `data:image/…` URI; with `files`, any other value is read as an image
    /// file and inlined (the CLI — the extension passes `false`, so files go
    /// through DuckDB's own `read_blob` and its access settings).
    pub fn set(&mut self, key: &str, value: &str, files: bool) -> Result<(), String> {
        let v = value.trim();
        match key {
            "name" => {
                if v.is_empty() || v.chars().count() > MAX_NAME || has_control(v) {
                    return Err(format!(
                        "name must be 1–{MAX_NAME} characters of plain text"
                    ));
                }
                self.name = Some(v.into());
            }
            "tagline" => {
                if v.chars().count() > MAX_TAGLINE || has_control(v) {
                    return Err(format!("tagline must be at most {MAX_TAGLINE} characters of plain text (empty hides it)"));
                }
                self.tagline = Some(v.into());
            }
            "link" => {
                if !v.is_empty() && !link_ok(v) {
                    return Err(format!(
                        "link must be an http:// or https:// URL, not '{v}'"
                    ));
                }
                self.link = Some(v.into());
            }
            "logo" | "favicon" => {
                let url = if v.is_empty() || image_url_ok(v) {
                    v.to_string()
                } else if files && !v.contains("://") && !v.starts_with("data:") {
                    image_file(std::path::Path::new(v)).map_err(|e| format!("{key}: {e}"))?
                } else {
                    return Err(format!(
                        "{key} must be an http(s) URL or a data:image/(png|jpeg|gif|webp|svg+xml) URI{}",
                        if files {
                            ", or an image file"
                        } else {
                            " (inline a file: 'data:image/png;base64,' || to_base64(content) FROM read_blob('logo.png'))"
                        }
                    ));
                };
                if key == "logo" {
                    self.logo = Some(url);
                } else {
                    self.favicon = Some(url);
                }
            }
            "name_visible" => {
                self.name_visible = Some(match v {
                    "true" => true,
                    "false" => false,
                    _ => return Err("name_visible must be true or false".into()),
                })
            }
            other => {
                return Err(format!(
                    "unknown key '{other}' ({}, name_visible)",
                    KEYS.join(", ")
                ))
            }
        }
        Ok(())
    }

    /// The bar must never be empty: hiding the name needs a logo.
    pub fn check(&self) -> Result<(), String> {
        if self.name_visible == Some(false) && self.logo.as_deref().unwrap_or("").is_empty() {
            return Err("name_visible: false needs a logo (the bar would be empty)".into());
        }
        Ok(())
    }

    /// From a JSON object (the extension's options): strings, and a boolean
    /// `name_visible`.
    pub fn from_json(json: &str) -> Result<AppBrand, String> {
        let v: Value = serde_json::from_str(json).map_err(|e| format!("bad JSON: {e}"))?;
        let m = v.as_object().ok_or("options must be a JSON object")?;
        let mut b = AppBrand::default();
        for (k, val) in m {
            let s = match (k.as_str(), val) {
                ("name_visible", Value::Bool(on)) => on.to_string(),
                ("name_visible", _) => return Err("name_visible must be true or false".into()),
                (_, Value::String(s)) => s.clone(),
                _ => return Err(format!("{k} must be a string")),
            };
            b.set(k, &s, false)?;
        }
        b.check()?;
        Ok(b)
    }

    pub fn is_empty(&self) -> bool {
        *self == AppBrand::default()
    }

    /// The set keys as a JSON object.
    pub fn to_json(&self) -> Value {
        let mut m = Map::new();
        for (k, v) in KEYS.iter().zip([
            &self.name,
            &self.tagline,
            &self.logo,
            &self.link,
            &self.favicon,
        ]) {
            if let Some(v) = v {
                m.insert((*k).into(), Value::String(v.clone()));
            }
        }
        if let Some(on) = self.name_visible {
            m.insert("name_visible".into(), Value::Bool(on));
        }
        Value::Object(m)
    }

    /// The `<script>` that hands the settings to the builder (empty when
    /// nothing is set), for `index.html`'s `<head>`.
    pub fn script(&self) -> String {
        if self.is_empty() {
            return String::new();
        }
        format!(
            "<script>window.__anofoxApp={};</script>",
            super::serving::json_for_script(&self.to_json())
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls() {
        assert!(image_url_ok("https://example.com/logo.svg"));
        assert!(image_url_ok("data:image/png;base64,iVBORw0KGgo="));
        assert!(image_url_ok("data:image/svg+xml,%3Csvg%3E"));
        assert!(!image_url_ok("data:text/html;base64,PGgxPg=="));
        assert!(!image_url_ok("javascript:alert(1)"));
        assert!(!image_url_ok("logo.png"));
        assert!(!image_url_ok("https://x/a\"onerror=1"));
        assert!(!image_url_ok("https://x/a b.png"));
        assert!(link_ok("https://intranet.example.com/"));
        assert!(!link_ok("javascript:alert(1)"));
        assert!(!link_ok("https://"));
        assert!(!link_ok("mailto:a@b"));
    }

    #[test]
    fn sniff_and_encode() {
        assert_eq!(sniff_image(b"\x89PNG\r\n\x1a\n...."), Some("image/png"));
        assert_eq!(
            sniff_image(b"<?xml version=\"1.0\"?>\n<svg xmlns=\"\"/>"),
            Some("image/svg+xml")
        );
        assert_eq!(sniff_image(b"RIFF\0\0\0\0WEBPVP8 "), Some("image/webp"));
        assert_eq!(sniff_image(b"<html><body>"), None);
        assert_eq!(sniff_image(b"hello"), None);
        assert_eq!(data_uri("image/png", b"Man"), "data:image/png;base64,TWFu");
        assert_eq!(data_uri("image/png", b"Ma"), "data:image/png;base64,TWE=");
        assert_eq!(data_uri("image/png", b"M"), "data:image/png;base64,TQ==");
    }

    #[test]
    fn set_and_inline() {
        let dir = std::env::temp_dir().join(format!("ax-brand-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let svg = dir.join("logo.svg");
        std::fs::write(&svg, "<svg xmlns='http://www.w3.org/2000/svg'/>").unwrap();
        let txt = dir.join("notes.txt");
        std::fs::write(&txt, "not an image").unwrap();
        let big = dir.join("big.png");
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.resize(MAX_IMAGE_BYTES + 1, 0);
        std::fs::write(&big, &png).unwrap();

        let mut b = AppBrand::default();
        b.set("logo", svg.to_str().unwrap(), true).unwrap();
        assert!(b
            .logo
            .as_deref()
            .unwrap()
            .starts_with("data:image/svg+xml;base64,"));
        let e = b.set("logo", txt.to_str().unwrap(), true).unwrap_err();
        assert!(e.contains("not a PNG"), "{e}");
        let e = b.set("logo", big.to_str().unwrap(), true).unwrap_err();
        assert!(e.contains("the limit is 512 KB"), "{e}");
        // the extension never reads files
        assert!(b.set("logo", svg.to_str().unwrap(), false).is_err());
        assert!(b.set("name", "", true).is_err());
        assert!(b.set("name", &"x".repeat(81), true).is_err());
        assert!(b.set("link", "ftp://x", true).is_err());
        b.set("tagline", "", true).unwrap();
        assert_eq!(b.tagline.as_deref(), Some(""));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn json_and_script() {
        assert_eq!(AppBrand::default().script(), "");
        let b = AppBrand::from_json(r#"{"name":"Acme </script> BI","tagline":""}"#).unwrap();
        assert_eq!(b.name.as_deref(), Some("Acme </script> BI"));
        let s = b.script();
        assert!(!s.contains("</script> BI"), "{s}");
        assert!(s.contains(r#""tagline":"""#), "{s}");
        assert!(AppBrand::from_json(r#"{"colour":"red"}"#)
            .unwrap_err()
            .contains("unknown key"));
        assert!(AppBrand::from_json(r#"{"name":3}"#).is_err());
        // a wordmark: the logo alone, never an empty bar
        let w =
            AppBrand::from_json(r#"{"name":"Acme","logo":"https://x/l.svg","name_visible":false}"#)
                .unwrap();
        assert!(w.script().contains(r#""name_visible":false"#));
        let e = AppBrand::from_json(r#"{"name":"Acme","name_visible":false}"#).unwrap_err();
        assert!(e.contains("needs a logo"), "{e}");
        assert!(AppBrand::from_json(r#"{"logo":"","name_visible":false}"#).is_err());
        assert!(AppBrand::from_json(r#"{"name_visible":"no"}"#).is_err());
        assert!(AppBrand::from_json("[]").is_err());
    }
}
