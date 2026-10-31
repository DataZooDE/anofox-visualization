//! The operator's stylesheet (`options.css_file`, #773): read once at start,
//! served at [`PATH`] and linked after the theme on every page of the locked
//! server.
//!
//! The operator is trusted (the file sits next to the server options, not in
//! the dashboards folder), so this is not a sanitiser. It keeps the
//! stylesheet from fetching anything off the server, which the page's CSP
//! alone would allow (`img-src https:`): every viewer's browser would
//! otherwise call a third party on each page view. Refused: `@import`, any
//! `url(…)` that is not a `data:` URL, and `image-set(…)` / `src(…)` (they
//! take bare strings). CSS escapes are decoded first, so `u\72l(` counts as
//! `url(`.

/// Where the stylesheet is served.
pub const PATH: &str = "/static/site.css";

/// The largest stylesheet accepted.
pub const MAX_BYTES: usize = 256 * 1024;

/// Check an operator stylesheet; the error names the first problem.
pub fn check(css: &str) -> Result<(), String> {
    if css.len() > MAX_BYTES {
        return Err(format!("larger than {} KiB", MAX_BYTES / 1024));
    }
    if css.contains('\0') {
        return Err("contains a NUL byte".into());
    }
    if css.to_ascii_lowercase().contains("</style") {
        return Err("contains '</style'".into());
    }
    let flat = normalise(css);
    if flat.contains("@import") {
        return Err("@import is not allowed (put the rules in this file)".into());
    }
    if flat.contains("image-set(") {
        return Err("image-set(…) is not allowed (use url(data:…))".into());
    }
    // The `src()` function, not a longer name ending in `src` (`foosrc(`).
    for (i, _) in flat.match_indices("src(") {
        if !flat[..i].ends_with(|c: char| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
            return Err("src(…) is not allowed (use url(data:…))".into());
        }
    }
    let mut rest = flat.as_str();
    while let Some(i) = rest.find("url(") {
        let arg = rest[i + 4..].trim_start().trim_start_matches(['"', '\'']);
        if !arg.starts_with("data:") {
            let shown: String = rest[i..].chars().take(40).collect();
            return Err(format!(
                "'{shown}': only data: URLs are allowed — the stylesheet may not fetch from other servers"
            ));
        }
        rest = &rest[i + 4..];
    }
    Ok(())
}

/// The stylesheet with comments removed, CSS escapes decoded and ASCII
/// lower-cased: what the checks above scan.
fn normalise(css: &str) -> String {
    let mut out = String::with_capacity(css.len());
    let cs: Vec<char> = css.chars().collect();
    let mut i = 0;
    while i < cs.len() {
        let c = cs[i];
        if c == '/' && cs.get(i + 1) == Some(&'*') {
            // A comment: skip to its end (an unclosed one runs to the end).
            i += 2;
            while i < cs.len() && !(cs[i] == '*' && cs.get(i + 1) == Some(&'/')) {
                i += 1;
            }
            i += 2;
            continue;
        }
        if c == '\\' {
            // `\` + 1–6 hex digits (+ one optional whitespace), or `\` + any
            // other character standing for itself.
            let mut j = i + 1;
            let mut hex = String::new();
            while j < cs.len() && hex.len() < 6 && cs[j].is_ascii_hexdigit() {
                hex.push(cs[j]);
                j += 1;
            }
            if !hex.is_empty() {
                if j < cs.len() && cs[j].is_whitespace() {
                    j += 1;
                }
                let ch = u32::from_str_radix(&hex, 16)
                    .ok()
                    .and_then(char::from_u32)
                    .unwrap_or('\u{fffd}');
                out.push(ch.to_ascii_lowercase());
                i = j;
                continue;
            }
            if let Some(&n) = cs.get(i + 1) {
                if n != '\n' {
                    out.push(n.to_ascii_lowercase());
                }
            }
            i += 2;
            continue;
        }
        out.push(c.to_ascii_lowercase());
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_ordinary_stylesheet_passes() {
        let css = r#"
            :root { --ax-accent: #0f766e; }
            .ax-c-kpi-hero { font-size: 1.4em; background: url(data:image/png;base64,iVBORw0KGgo=); }
            .ax-c-note::before { content: "\2014  "; }
            @font-face { font-family: Brand; src: url("data:font/woff2;base64,d09GMgAB"); }
            img { max-width: 100%; }
        "#;
        assert_eq!(check(css), Ok(()));
    }

    #[test]
    fn fetches_off_the_server_are_refused() {
        for bad in [
            "@import url(https://x.example/a.css);",
            "@IMPORT 'a.css';",
            "a { background: url(https://x.example/p.png) }",
            "a { background: url( 'http://x.example/p.png' ) }",
            "a { background: URL(//x.example/p.png) }",
            "a { background: url(/assets/p.png) }",
            "a { background: u\\72l(https://x.example/p.png) }",
            "a { background: u\\000072 l(https://x.example/p.png) }",
            "a { background: url/**/(https://x.example) }",
            "@im/**/port 'x.css';",
            "a { background: image-set('https://x.example/p.png' 1x) }",
            "a { background: -webkit-image-set('p.png' 1x) }",
            "@font-face { src: src('https://x.example/f.woff2') }",
            "</style><script>alert(1)</script>",
        ] {
            assert!(check(bad).is_err(), "{bad}");
        }
        assert!(check(&"a{}".repeat(MAX_BYTES)).is_err());
    }

    #[test]
    fn a_property_ending_in_src_is_not_a_function() {
        // `src: url(data:…)` inside @font-face is fine; only `src(` is refused.
        assert_eq!(
            check("@font-face { src: url(data:font/woff2;base64,AA) }"),
            Ok(())
        );
    }
}
