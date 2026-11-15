//! The drawing-only SVG check (review of #554 B1): `--site`'s published
//! assets and a runtime plugin's panels (#1213) — natively and in the
//! browser build, so both hold a plugin's SVG to the same allow-list.

/// The SVG and XLink namespace URIs — the only two namespaces an accepted
/// SVG may use anywhere in the document (review of #554 B1).
const SVG_NS: &str = "http://www.w3.org/2000/svg";
const XLINK_NS: &str = "http://www.w3.org/1999/xlink";

/// Local, SVG-spec-cased element names a published SVG may contain
/// (`svg_active_reason`'s ALLOW-list). No animation element (`animate`,
/// `set`, `animateTransform`, `animateMotion`, `mpath`) is in this list at
/// all: SMIL's own `href`/`xlink:href`/`values` targeting (as in
/// `<animate attributeName="href" values="javascript:…">`, review of #554
/// B1's `anim.svg`) has no legitimate use in a static dashboard asset, so
/// it's simplest to refuse every animation element outright rather than
/// inspect what each one targets or decodes to. No `<a>` either (its
/// `href` would need the same external-reference check `<image>` gets,
/// for no real benefit in a static asset). No `<style>` element: CSS
/// `url()`/`@import`/`expression()` is the same class of problem as a
/// `style=` attribute (checked below), and a `<style>` element's full text
/// content is a much larger surface to re-check than one attribute value.
const SVG_ALLOWED_ELEMENTS: &[&str] = &[
    "svg",
    "g",
    "defs",
    "symbol",
    "use",
    "path",
    "rect",
    "circle",
    "ellipse",
    "line",
    "polyline",
    "polygon",
    "text",
    "tspan",
    "textPath",
    "title",
    "desc",
    "linearGradient",
    "radialGradient",
    "stop",
    "pattern",
    "clipPath",
    "mask",
    "marker",
    "image",
    // Filter primitives: pure pixel math, no script or external-resource
    // surface — unlike `feImage`, deliberately left out here: it takes its
    // own `href`, is rarely needed for a dashboard icon, and would need
    // the same href check `<image>` gets below.
    "filter",
    "feGaussianBlur",
    "feOffset",
    "feMerge",
    "feMergeNode",
    "feComponentTransfer",
    "feFuncR",
    "feFuncG",
    "feFuncB",
    "feFuncA",
    "feColorMatrix",
    "feComposite",
    "feBlend",
    "feFlood",
    "feTile",
    "feDropShadow",
    "feMorphology",
];

/// Raster `data:image/<t>` subtypes allowed on an `<image href="…">` inside
/// an already-SVG asset — narrower than [`crate::image::DATA_IMAGE_TYPES`]
/// (no `svg+xml`: nesting another SVG payload here would reopen the same
/// question `svg_active_reason` exists to close, one level down).
const SVG_IMAGE_HREF_RASTER_TYPES: &[&str] = &["png", "jpeg", "jpg", "gif", "webp"];

fn is_allowed_raster_data_uri(v: &str) -> bool {
    let low = v.to_ascii_lowercase();
    low.starts_with("data:image/")
        && SVG_IMAGE_HREF_RASTER_TYPES.iter().any(|t| {
            let p = format!("data:image/{t}");
            low.starts_with(&format!("{p};")) || low.starts_with(&format!("{p},"))
        })
}

/// Does `value` (any attribute's value, already XML-entity-decoded by the
/// parser) try to reach something off-document through CSS? Review of
/// #554 (second re-review, S1): checking only the `style` attribute left
/// every *presentation* attribute (`fill`, `stroke`, `filter`,
/// `clip-path`, `mask`, every `marker-*`, `cursor`, …) as an unguarded
/// fetch — SVG's own CSS cascade applies to a presentation attribute
/// exactly as if it were written in `style`, so `fill="url(https://…)"`
/// fetches just as `style="fill:url(https://…)"` does. A CSS escape also
/// defeated a literal `"url("` substring check (`u\72l(` decodes, in CSS,
/// to `url(`), and `image-set('https://…')` reaches a URL with no `url(`
/// at all. This function is applied to **every** attribute's value
/// ([`svg_active_reason`], below), refusing:
/// - any backslash at all — the escape vector; no legitimate value here
///   needs one;
/// - `url(…)` whose target isn't a same-document `#fragment` (`url(#id)`
///   is how an SVG legitimately references its own
///   `<filter>`/`<clipPath>`/gradient by CSS);
/// - `image-set(` (a CSS image fallback list), `@import`, `src(` (the
///   newer, unprefixed CSS font/image loading function), `javascript:`
///   anywhere, `expression(` (old IE CSS, still worth refusing).
fn external_ref_reason(value: &str) -> Option<String> {
    if value.contains('\\') {
        return Some(
            "contains a backslash (a CSS escape can hide url()/javascript: from a literal scan)"
                .to_string(),
        );
    }
    let low = value.to_ascii_lowercase();
    for bad in [
        "@import",
        "image-set(",
        "src(",
        "javascript:",
        "expression(",
    ] {
        if low.contains(bad) {
            return Some(format!("contains '{bad}'"));
        }
    }
    let mut rest = low.as_str();
    while let Some(i) = rest.find("url(") {
        let after = &rest[i + 4..];
        let end = after.find(')').unwrap_or(after.len());
        let inner = after[..end].trim().trim_matches(['"', '\'']);
        if !inner.starts_with('#') {
            return Some(format!(
                "contains a url() that isn't a local #fragment ('{}')",
                truncate_for_message(inner)
            ));
        }
        rest = &after[end.min(after.len())..];
    }
    None
}

fn truncate_for_message(s: &str) -> String {
    const MAX: usize = 60;
    if s.chars().count() <= MAX {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(MAX).collect::<String>())
    }
}

/// Is `bytes` (already confirmed, by its magic bytes, to be an SVG) "active"
/// — could it run script or fetch something external if a browser opens it
/// as a *document* rather than drawing it through an `<img>`? Review of
/// #554 item 1: the locked server neutralises this with its asset route's
/// own `Content-Security-Policy: sandbox` header on a direct open, but a
/// published static site has no server to send that header — `assets/x.svg`
/// opened directly on GitHub Pages/S3/etc. just runs. So `--site` refuses to
/// publish one instead.
///
/// An ALLOW-list over a *parsed* document, not a deny-list over the raw
/// text (review of #554 B1: the previous text-scan version was bypassed by
/// a namespace-prefixed `<x:script>` where `x` is bound to the SVG
/// namespace under a prefix other than none; a foreign-namespace element
/// — `<h:script>`/`<h:iframe>` bound to the XHTML namespace, needing no
/// `<foreignObject>` wrapper at all; an internal DTD entity expanding to
/// literal `<script>` text; and `&#106;avascript:` — entity-encoded `j` —
/// hiding a `javascript:` URL from a literal substring match in an
/// `<animate>`'s `values` and in a quoted `href`. A real XML parser that
/// resolves namespaces and decodes entities before this function ever sees
/// a string closes all four at once, instead of each needing its own
/// pattern added to a text scan that will always be one bypass behind).
///
/// Every element must resolve to the SVG namespace (`tag_name().namespace()
/// == Some(SVG_NS)`, never guessed from a prefix) and be in
/// [`SVG_ALLOWED_ELEMENTS`]. Every attribute must be unprefixed or in the
/// XLink namespace — any other namespace is refused outright, which is what
/// actually stops `<h:script>`/`<h:iframe>` regardless of what prefix binds
/// them. No attribute name may start with `on` (case-insensitively — SVG
/// alone has dozens of event handlers: `onload`, `onbegin`, `onend`,
/// `onrepeat`, `onactivate`, …). `href`/`xlink:href` may only be a
/// same-document `#fragment`, or, on `<image>` only, a
/// `data:image/(png|jpeg|gif|webp)` payload. A `style` attribute may not
/// contain `@import`, `javascript:`, `expression(`, or a `url(…)` that
/// isn't a `#fragment` ([`style_url_reason`]).
///
/// A literal `<!DOCTYPE`, anywhere in the raw bytes, is refused before
/// parsing even starts — entity expansion is exactly how `ent.svg` (review
/// of #554 B1) smuggled a `<script>` past the previous version; never let
/// the parser expand an entity at all. Redundant with
/// [`roxmltree::ParsingOptions::allow_dtd`]'s own default of `false`
/// (`Document::parse` uses it), kept as a belt-and-braces check that
/// doesn't depend on that default never changing. A processing instruction
/// anywhere in the document is refused too — untested territory for
/// whatever opens this file directly; simplest to disallow.
///
/// Returns the reason, or `None` if none of this matched (still just pixels
/// and markup, not a reason this file is safe in some deeper sense this
/// scan can't see).
pub fn svg_active_reason(bytes: &[u8]) -> Option<String> {
    let text = match std::str::from_utf8(bytes) {
        Ok(t) => t,
        Err(_) => return Some("is not valid UTF-8 (can't be parsed as XML)".to_string()),
    };
    if text.to_ascii_lowercase().contains("<!doctype") {
        return Some("contains a <!DOCTYPE declaration (entity expansion risk)".to_string());
    }
    let doc = match roxmltree::Document::parse(text) {
        Ok(d) => d,
        Err(e) => return Some(format!("could not be parsed as well-formed XML ({e})")),
    };
    for node in doc.descendants() {
        if node.is_pi() {
            return Some("contains a processing instruction".to_string());
        }
        if !node.is_element() {
            continue;
        }
        let tag = node.tag_name();
        if tag.namespace() != Some(SVG_NS) {
            return Some(format!(
                "contains an element ('{}') outside the SVG namespace",
                tag.name()
            ));
        }
        if !SVG_ALLOWED_ELEMENTS.contains(&tag.name()) {
            return Some(format!("contains a disallowed element ('{}')", tag.name()));
        }
        for attr in node.attributes() {
            let ns = attr.namespace();
            if ns.is_some() && ns != Some(XLINK_NS) {
                return Some(format!(
                    "contains an attribute ('{}') outside the SVG/XLink namespaces",
                    attr.name()
                ));
            }
            let local = attr.name();
            if local.len() >= 2 && local.as_bytes()[..2].eq_ignore_ascii_case(b"on") {
                return Some(format!("contains an event-handler attribute ('{local}')"));
            }
            // Review of #554 (second re-review, S1): checked on *every*
            // attribute, not only `style` — a presentation attribute
            // (`fill`, `filter`, `clip-path`, `mask`, `marker-*`,
            // `cursor`, …) takes the same CSS syntax `style` does.
            if let Some(why) = external_ref_reason(attr.value()) {
                return Some(format!("attribute '{local}' {why}"));
            }
            if local.eq_ignore_ascii_case("href") {
                let v = attr.value().trim();
                let ok =
                    v.starts_with('#') || (tag.name() == "image" && is_allowed_raster_data_uri(v));
                if !ok {
                    return Some(format!(
                        "references an external href/xlink:href ('{}')",
                        truncate_for_message(v)
                    ));
                }
            }
        }
    }
    None
}
