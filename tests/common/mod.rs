//! Shared checks for rendered output, and for tests that need an external tool.
#![allow(dead_code)]

/// A test needs an external tool (`duckdb`, `node`) that isn't on PATH: skip
/// it locally (with a note), but fail on CI (`CI` set), where the workflow
/// installs the tool — so a missing install can't silently skip the test.
pub fn skip_without(tool: &str, test: &str) {
    if std::env::var_os("CI").is_some() {
        panic!("{test}: `{tool}` is required on CI but was not found on PATH");
    }
    eprintln!("skipped {test}: no `{tool}` on PATH");
}

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
                    // a ::DIAGRAM's node ids, labels and its summary are text
                    | "data-node"
                    | "data-edge"
                    | "data-from"
                    | "data-to"
                    | "data-fx"
                    | "data-summary"
                    | "data-badge"
                    | "data-of"
                    | "data-story"
                    // a map marker's / ::DIAGRAM node's category label and
                    // swimlane name are also free text (#319: a ::LABEL or
                    // ::LANE column may literally hold the string "inf")
                    | "data-label"
                    | "data-lane"
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

/// Parse x-axis tick labels back out of rendered SVG for assertions (#108,
/// ggplot-rs#318 step 2). The fit/overlap *logic* this used to also apply
/// (`fit_x_labels`) now lives in ggplot-rs (`Theme::axis_text_x_fit`,
/// `axis_text_x_check_overlap`); this module keeps only the read-only
/// parser so tests can still check where labels actually landed.
pub mod tick_labels {
    /// An x-axis tick label found in a rendered chart.
    #[derive(Clone, Debug, PartialEq)]
    pub struct TickLabel {
        pub text: String,
        pub x: f64,
        pub y: f64,
        pub size: f64,
        /// Rotation in degrees (SVG `rotate`, clockwise).
        pub angle: f64,
        /// Where `x` sits on the text: 0 = start, 0.5 = middle, 1 = end.
        pub anchor: f64,
        /// The chart's width (its `viewBox`).
        pub chart_width: f64,
        /// The chart's height (its `viewBox`).
        pub chart_height: f64,
    }

    impl TickLabel {
        /// Estimated bounding box `[x0, y0, x1, y1]`, rotation included.
        pub fn bbox(&self) -> [f64; 4] {
            let w = ggplot_rs::render::text::text_width(&self.text, self.size);
            let h = self.size * 1.2;
            let x0 = -w * self.anchor;
            let (s, c) = self.angle.to_radians().sin_cos();
            let mut b = [
                f64::INFINITY,
                f64::INFINITY,
                f64::NEG_INFINITY,
                f64::NEG_INFINITY,
            ];
            for (dx, dy) in [
                (x0, -h / 2.0),
                (x0 + w, -h / 2.0),
                (x0, h / 2.0),
                (x0 + w, h / 2.0),
            ] {
                let (px, py) = (self.x + dx * c - dy * s, self.y + dx * s + dy * c);
                b = [b[0].min(px), b[1].min(py), b[2].max(px), b[3].max(py)];
            }
            b
        }

        /// Does this label collide with `next` (the label to its right)?
        pub fn overlaps(&self, next: &TickLabel) -> bool {
            if self.angle.abs() < 1.0 {
                self.bbox()[2] > next.bbox()[0]
            } else {
                (next.x - self.x).abs() * self.angle.to_radians().sin().abs() < self.size
            }
        }
    }

    /// The x-axis tick labels of every chart in `svg` (each `<svg>` with a
    /// `data-plot` panel rect), in document order.
    pub fn x_tick_labels(svg: &str) -> Vec<TickLabel> {
        rows(svg).into_iter().flatten().collect()
    }

    fn rows(svg: &str) -> Vec<Vec<TickLabel>> {
        let mut out = Vec::new();
        let mut from = 0;
        while let Some(i) = svg[from..].find("<svg").map(|i| i + from) {
            let Some(open_end) = svg[i..].find('>').map(|e| i + e + 1) else {
                break;
            };
            from = open_end;
            let open = &svg[i..open_end];
            let (Some(plot), Some(view)) = (attr(open, "data-plot"), attr(open, "viewBox")) else {
                continue;
            };
            let plot = nums(plot);
            let view = nums(view);
            let (Some(&py), Some(&ph), Some(&w), Some(&h)) =
                (plot.get(1), plot.get(3), view.get(2), view.get(3))
            else {
                continue;
            };
            let end = [svg[open_end..].find("</svg>"), svg[open_end..].find("<svg")]
                .into_iter()
                .flatten()
                .min()
                .map_or(svg.len(), |e| open_end + e);
            out.extend(axis_rows(svg, open_end, end, py + ph, w, h));
        }
        out
    }

    /// Runs of consecutive `<text>`s in `svg[from..to]` that share a
    /// baseline below the panel (`bottom`) and a style, left to right: the x
    /// axis's tick labels (one run per facet column). Lone texts there (axis
    /// title, caption) are not runs.
    fn axis_rows(
        svg: &str,
        from: usize,
        to: usize,
        bottom: f64,
        w: f64,
        h: f64,
    ) -> Vec<Vec<TickLabel>> {
        let mut rows: Vec<Vec<TickLabel>> = Vec::new();
        let mut cur: Vec<(String, TickLabel)> = Vec::new();
        let mut flush = |cur: &mut Vec<(String, TickLabel)>| {
            if cur.len() >= 2 {
                rows.push(cur.drain(..).map(|(_, l)| l).collect());
            }
            cur.clear();
        };
        let mut at = from;
        while let Some(s) = svg[at..to].find("<text ").map(|s| s + at) {
            let Some(e) = svg[s..to].find("</text>").map(|e| s + e + "</text>".len()) else {
                break;
            };
            at = e;
            let el = &svg[s..e];
            let Some(tag_end) = el.find('>') else {
                continue;
            };
            let tag = &el[..tag_end];
            let num = |k: &str| attr(tag, k).and_then(|v| v.parse::<f64>().ok());
            let (Some(x), Some(y), Some(size)) = (num("x"), num("y"), num("font-size")) else {
                flush(&mut cur);
                continue;
            };
            let angle = attr(tag, "transform")
                .and_then(|t| t.strip_prefix("rotate("))
                .and_then(|t| t.split_whitespace().next())
                .and_then(|a| a.parse::<f64>().ok())
                .unwrap_or(0.0);
            let anchor = match attr(tag, "text-anchor") {
                Some("start") => 0.0,
                Some("end") => 1.0,
                _ => 0.5,
            };
            let key = format!(
                "{} {} {:?} {} {}",
                attr(tag, "y").unwrap_or(""),
                attr(tag, "font-size").unwrap_or(""),
                attr(tag, "text-anchor"),
                angle,
                attr(tag, "fill").unwrap_or("")
            );
            let label = TickLabel {
                text: unescape(&el[tag_end + 1..el.len() - "</text>".len()]),
                x,
                y,
                size,
                angle,
                anchor,
                chart_width: w,
                chart_height: h,
            };
            let continues = cur
                .last()
                .is_some_and(|(k, prev)| *k == key && prev.x < label.x);
            if !continues {
                flush(&mut cur);
            }
            if y > bottom {
                cur.push((key, label));
            }
        }
        flush(&mut cur);
        rows
    }

    fn attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
        let pat = format!(" {name}=\"");
        let i = tag.find(&pat)? + pat.len();
        tag[i..].find('"').map(|e| &tag[i..i + e])
    }

    fn nums(s: &str) -> Vec<f64> {
        s.split_whitespace()
            .filter_map(|v| v.parse().ok())
            .collect()
    }

    fn unescape(s: &str) -> String {
        s.replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&quot;", "\"")
            .replace("&#39;", "'")
            .replace("&amp;", "&")
    }
}
