//! Per-feature metadata for `::MAP` marks.
//!
//! ggplot-rs's `geom_sf` gives each drawn feature a `<title>` (its hover
//! text) but none of the `data-series` / `data-value` attributes the other
//! geoms carry, so the browser could only guess a feature's series from its
//! title text. [`render_map`](crate) therefore smuggles each row's metadata
//! through the label aesthetic as a private-use [`marker`] in front of the
//! label text, and [`lift`] moves it out of the rendered `<title>` onto the
//! mark's own element:
//!
//! ```text
//! <circle …><title>{marker}Backstube 3: Van 1</title></circle>
//!   → <circle … data-row="7" data-series="Van 1" data-label="Backstube 3"><title>Backstube 3: Van 1</title></circle>
//! ```
//!
//! The title text (and so the static SVG / CLI tooltip) stays exactly what it
//! was; only the `data-*` attributes are new. ggplot-rs escapes the title
//! text, so the moved values are already attribute-safe; newlines become
//! `&#10;` so they survive XML attribute normalisation.

/// Starts a marker.
const OPEN: char = '\u{E000}';
/// Ends a marker (the label text follows).
const CLOSE: char = '\u{E001}';
/// Separates a key from its value.
const KV: char = '\u{E002}';
/// Separates two fields.
const SEP: char = '\u{E003}';

/// Key of the flag field: the feature had no label text of its own.
const BARE: &str = "bare";

fn clean(s: &str) -> String {
    s.chars().filter(|c| !(OPEN..=SEP).contains(c)).collect()
}

/// The label-aesthetic text of a map row: a marker carrying `fields`
/// (`("row", "3")`, `("series", "Van 1")`, …; empty values are dropped),
/// then the row's own label text `text` (may be empty).
pub(crate) fn marker(fields: &[(&str, &str)], text: &str) -> String {
    let mut s = String::new();
    s.push(OPEN);
    let mut first = true;
    let mut push = |k: &str, v: &str| {
        if !first {
            s.push(SEP);
        }
        first = false;
        s.push_str(k);
        s.push(KV);
        s.push_str(&clean(v));
    };
    for (k, v) in fields.iter().filter(|(_, v)| !v.is_empty()) {
        push(k, v);
    }
    if text.is_empty() {
        push(BARE, "1");
    }
    s.push(CLOSE);
    s.push_str(&clean(text));
    s
}

/// Move every marker of `svg` out of its `<title>` onto the mark element as
/// `data-<key>` attributes, restoring the title geom_sf would have written
/// without it (a feature with neither label nor fill gets no `<title>`).
/// A no-op for an SVG without markers.
pub(crate) fn lift(svg: String) -> String {
    const START: &str = "><title>\u{E000}";
    if !svg.contains(START) {
        return svg;
    }
    let mut out = String::with_capacity(svg.len() + svg.len() / 4);
    let mut rest = svg.as_str();
    while let Some(p) = rest.find(START) {
        let after = &rest[p + START.len()..];
        let (Some(end), Some(close)) = (after.find(CLOSE), after.find("</title>")) else {
            break;
        };
        if close < end {
            break;
        }
        out.push_str(&rest[..p]);
        let mut bare = false;
        for field in after[..end].split(SEP) {
            let Some((k, v)) = field.split_once(KV) else {
                continue;
            };
            if k == BARE {
                bare = true;
            } else if !k.is_empty() && k.bytes().all(|b| b.is_ascii_lowercase() || b == b'-') {
                out.push_str(&format!(" data-{k}=\"{}\"", v.replace('\n', "&#10;")));
            }
        }
        let mut text = &after[end + CLOSE.len_utf8()..close];
        if bare {
            text = text.strip_prefix(": ").unwrap_or(text);
        }
        let tail = &after[close + "</title>".len()..];
        if text.is_empty() {
            // geom_sf draws an untitled mark self-closed: `<circle …/>`.
            match tail.find('>') {
                Some(gt) if tail.starts_with("</") => {
                    out.push_str("/>");
                    rest = &tail[gt + 1..];
                }
                _ => {
                    out.push_str("><title></title>");
                    rest = tail;
                }
            }
        } else {
            out.push_str("><title>");
            out.push_str(text);
            out.push_str("</title>");
            rest = tail;
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifts_fields_onto_the_mark_and_restores_the_title() {
        let m = marker(
            &[("row", "3"), ("series", "Van 1"), ("label", "A\nB")],
            "A\nB",
        );
        let svg = format!("<svg><circle cx=\"1\"><title>{m}: Van 1</title></circle></svg>");
        assert_eq!(
            lift(svg),
            "<svg><circle cx=\"1\" data-row=\"3\" data-series=\"Van 1\" data-label=\"A&#10;B\"><title>A\nB: Van 1</title></circle></svg>"
        );
    }

    #[test]
    fn a_feature_without_label_text_keeps_its_plain_title_or_none() {
        let m = marker(&[("row", "0"), ("value", "")], "");
        let svg = format!("<svg><polygon points=\"\"><title>{m}: 42</title></polygon><circle><title>{m}</title></circle></svg>");
        assert_eq!(
            lift(svg),
            "<svg><polygon points=\"\" data-row=\"0\"><title>42</title></polygon><circle data-row=\"0\"/></svg>"
        );
    }

    #[test]
    fn user_text_cannot_forge_a_marker() {
        let m = marker(&[("series", "x\u{E003}row\u{E002}9")], "\u{E000}y");
        let svg = format!("<svg><circle><title>{m}</title></circle></svg>");
        assert_eq!(
            lift(svg),
            "<svg><circle data-series=\"xrow9\"><title>y</title></circle></svg>"
        );
    }

    #[test]
    fn svg_without_markers_is_untouched() {
        let s = "<svg><circle><title>a: b</title></circle></svg>".to_string();
        assert_eq!(lift(s.clone()), s);
    }
}
