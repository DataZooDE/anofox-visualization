//! Shared checks for rendered output.
#![allow(dead_code)]

/// Assert `svg` is well-formed XML rooted at `<svg>`, with no script vectors
/// (no `<script>`/`<foreignObject>`/`<img>`/`<iframe>` element, no `on*`
/// attribute, no `javascript:` URL) and no non-finite numbers in attributes.
pub fn assert_safe_svg(svg: &str, ctx: &str) {
    let doc = roxmltree::Document::parse(svg)
        .unwrap_or_else(|e| panic!("{ctx}: not well-formed XML: {e}\n{}", head(svg)));
    assert_eq!(doc.root_element().tag_name().name(), "svg", "{ctx}: root");
    for n in doc.descendants().filter(|n| n.is_element()) {
        let tag = n.tag_name().name().to_ascii_lowercase();
        assert!(
            !matches!(
                tag.as_str(),
                "script" | "foreignobject" | "img" | "iframe" | "object" | "embed"
            ),
            "{ctx}: injected <{tag}>"
        );
        for a in n.attributes() {
            let name = a.name().to_ascii_lowercase();
            assert!(!name.starts_with("on"), "{ctx}: event attribute {name}");
            let v = a.value();
            assert!(
                !v.trim_start()
                    .to_ascii_lowercase()
                    .starts_with("javascript:"),
                "{ctx}: javascript: URL in {name}"
            );
            // Label-carrying attributes hold category text (a category may
            // literally be "inf"); geometry and numeric attributes must be finite.
            if !matches!(
                name.as_str(),
                "data-x"
                    | "data-series"
                    | "data-xlevels"
                    | "data-ylevels"
                    | "data-warnings"
                    | "class"
                    | "id"
            ) {
                for bad in ["NaN", "inf"] {
                    assert!(
                        !v.split(|c: char| !(c.is_alphanumeric() || c == '-' || c == '.'))
                            .any(|tok| tok == bad || tok == format!("-{bad}")),
                        "{ctx}: non-finite number in {name}=\"{}\"",
                        head(v)
                    );
                }
            }
        }
    }
}

/// Assert an HTML fragment (`<pre>…</pre>`) carries no markup beyond the wrapper.
pub fn assert_safe_pre(html: &str, ctx: &str) {
    let inner = html
        .strip_prefix("<pre>")
        .and_then(|s| s.strip_suffix("</pre>"))
        .unwrap_or_else(|| panic!("{ctx}: not a <pre> block: {}", head(html)));
    assert!(
        !inner.contains('<') && !inner.contains('>') && !inner.contains('"'),
        "{ctx}: unescaped markup in error: {inner}"
    );
}

pub fn head(s: &str) -> String {
    s.chars().take(300).collect()
}
