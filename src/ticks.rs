//! Keep x-axis tick labels inside the chart (#108).
//!
//! ggplot-rs centres each x tick label on its tick and reserves no room for
//! the half of the last (or first) label that sticks out past the panel, so
//! on a narrow chart (a phone, ~300 px) a label such as `2027-01` runs past
//! the SVG's edge and is clipped; crowded continuous breaks also collide,
//! since only a discrete axis is rotated when its labels don't fit.
//!
//! [`fit_x_labels`] corrects the rendered SVG, the way Vega handles label
//! bounds and overlap (`labelBound`, `labelOverlap: "parity"`):
//! - a label that would cross the left or right edge is shifted just enough
//!   to sit inside it;
//! - while neighbouring labels still overlap, every other one is dropped
//!   (the first is kept). Ticks and grid lines stay;
//! - a label whose tick is off the chart is dropped.
//!
//! Labels that fit are not touched, so a wide chart's SVG is unchanged. The
//! plot area is not changed either, so ::TRACK strips stay aligned.
//!
//! TODO(ggplot-rs): reserve the outer labels' half-widths in the layout and
//! thin colliding labels in `draw_x_axis` (ggplot2's `check.overlap`), then
//! drop this pass.

/// Gap (px) kept between a label and the SVG's edge.
const EDGE: f64 = 1.0;

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
    /// Byte range of the `<text>…</text>` element in the SVG.
    span: (usize, usize),
}

impl TickLabel {
    /// Estimated bounding box `[x0, y0, x1, y1]`, rotation included.
    pub fn bbox(&self) -> [f64; 4] {
        let w = text_width(&self.text, self.size);
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
            // Parallel rotated labels on one baseline: their distance across
            // the text direction must leave room for a line of text.
            (next.x - self.x).abs() * self.angle.to_radians().sin().abs() < self.size
        }
    }
}

/// Estimated width (px) of `text` at font size `size` in the house sans
/// (Hanken Grotesk, or the system UI font it falls back to).
pub fn text_width(text: &str, size: f64) -> f64 {
    let em: f64 = text
        .chars()
        .map(|ch| match ch {
            ' ' => 0.27,
            '.' | ',' | ':' | ';' | '\'' | '|' | '!' | 'i' | 'j' | 'l' | 'I' => 0.27,
            'f' | 't' | 'r' | '(' | ')' | '[' | ']' | '-' | '/' => 0.36,
            '0'..='9' | '$' | '€' | '£' | '+' | '=' | '<' | '>' | '#' | '*' | '?' => 0.57,
            'm' | 'w' => 0.8,
            'M' | 'W' | '%' | '@' => 0.86,
            'a'..='z' => 0.53,
            'A'..='Z' | '&' => 0.66,
            _ => 0.62,
        })
        .sum();
    em * size
}

/// The x-axis tick labels of every chart in `svg` (each `<svg>` with a
/// `data-plot` panel rect), in document order.
pub fn x_tick_labels(svg: &str) -> Vec<TickLabel> {
    rows(svg).into_iter().flatten().collect()
}

/// Shift x tick labels that cross the chart's edge back inside and drop
/// every other label while neighbours overlap (see the module docs).
pub(crate) fn fit_x_labels(svg: String) -> String {
    let mut edits: Vec<((usize, usize), Option<String>)> = Vec::new();
    for row in rows(&svg) {
        // A label whose tick is off the chart (a panel squeezed below its
        // minimum size) names nothing that is shown.
        let (mut row, off): (Vec<TickLabel>, Vec<TickLabel>) = row
            .into_iter()
            .partition(|l| (0.0..=l.chart_width).contains(&l.x));
        edits.extend(off.into_iter().map(|l| (l.span, None)));
        for l in row.iter_mut() {
            let [x0, _, x1, _] = l.bbox();
            let room = l.chart_width - EDGE;
            let dx = if x0 < EDGE {
                EDGE - x0
            } else if x1 > room {
                (room - x1).max(EDGE - x0)
            } else {
                0.0
            };
            if dx != 0.0 {
                let el = &svg[l.span.0..l.span.1];
                edits.push((l.span, Some(moved(el, l.y, l.x + dx))));
                l.x += dx;
            }
        }
        let mut keep: Vec<usize> = (0..row.len()).collect();
        while keep.len() > 1 && keep.windows(2).any(|w| row[w[0]].overlaps(&row[w[1]])) {
            keep = keep.into_iter().step_by(2).collect();
        }
        for (i, l) in row.iter().enumerate() {
            if !keep.contains(&i) {
                edits.retain(|(span, _)| *span != l.span);
                edits.push((l.span, None));
            }
        }
    }
    if edits.is_empty() {
        return svg;
    }
    edits.sort_by_key(|(span, _)| span.0);
    let mut out = String::with_capacity(svg.len());
    let mut at = 0;
    for ((a, b), new) in edits {
        out.push_str(&svg[at..a]);
        out.push_str(new.as_deref().unwrap_or(""));
        at = b;
    }
    out.push_str(&svg[at..]);
    out
}

/// A `<text>` element at `(x, y)` with its `x` (and its rotation centre,
/// the anchor point) moved to `x`.
fn moved(el: &str, y: f64, x: f64) -> String {
    let tag_end = el.find('>').unwrap_or(el.len());
    let (tag, rest) = el.split_at(tag_end);
    let tag = set_attr(tag, "x", &format!("{x:.2}"));
    let tag = match attr(&tag, "transform").and_then(|t| t.strip_prefix("rotate(")) {
        Some(t) => {
            let angle = t.split_whitespace().next().unwrap_or("0").to_string();
            set_attr(&tag, "transform", &format!("rotate({angle} {x:.2} {y:.2})"))
        }
        None => tag,
    };
    format!("{tag}{rest}")
}

/// The x tick label rows of each chart in `svg`.
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
        // The chart's own content: up to its end or a nested chart.
        let end = [svg[open_end..].find("</svg>"), svg[open_end..].find("<svg")]
            .into_iter()
            .flatten()
            .min()
            .map_or(svg.len(), |e| open_end + e);
        out.extend(axis_rows(svg, open_end, end, py + ph, w, h));
    }
    out
}

/// Runs of consecutive `<text>`s in `svg[from..to]` that share a baseline
/// below the panel (`bottom`) and a style, left to right: the x axis's tick
/// labels (one run per facet column). Lone texts there (axis title,
/// caption) are not runs.
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
            span: (s, e),
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

/// The value of attribute `name` in a start tag.
fn attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let pat = format!(" {name}=\"");
    let i = tag.find(&pat)? + pat.len();
    tag[i..].find('"').map(|e| &tag[i..i + e])
}

/// `tag` with attribute `name` set to `value` (it must be present).
fn set_attr(tag: &str, name: &str, value: &str) -> String {
    let pat = format!(" {name}=\"");
    match tag.find(&pat) {
        Some(i) => {
            let v = i + pat.len();
            let e = tag[v..].find('"').map_or(tag.len(), |e| v + e);
            format!("{}{value}{}", &tag[..v], &tag[e..])
        }
        None => tag.to_string(),
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn label(x: f64, text: &str) -> String {
        format!(
            "<text x=\"{x:.2}\" y=\"229.10\" font-size=\"8.80\" text-anchor=\"middle\" \
             dominant-baseline=\"middle\" font-family=\"sans-serif\" fill=\"#6B7280\">{text}</text>"
        )
    }

    fn chart(labels: &[(f64, &str)]) -> String {
        let body: String = labels.iter().map(|(x, t)| label(*x, t)).collect();
        format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"280\" height=\"260\" \
             viewBox=\"0 0 280 260\" data-plot=\"62.05 5.50 212.45 214.45\">{body}\
             <text x=\"168.28\" y=\"250.00\" font-size=\"11.00\" text-anchor=\"middle\" \
             fill=\"#6B7280\">month</text></svg>"
        )
    }

    #[test]
    fn fitting_labels_are_untouched() {
        let svg = chart(&[(80.0, "2026-01"), (170.0, "2026-07"), (250.0, "2027-01")]);
        assert_eq!(fit_x_labels(svg.clone()), svg);
    }

    #[test]
    fn last_label_moves_inside() {
        let svg = fit_x_labels(chart(&[(71.71, "2026-01"), (269.17, "2027-01")]));
        let labels = x_tick_labels(&svg);
        assert_eq!(labels.len(), 2);
        assert_eq!(labels[0].x, 71.71, "a fitting label stays on its tick");
        let b = labels[1].bbox();
        assert!(b[2] <= 279.01 && labels[1].x < 269.17, "{b:?}");
        assert!(svg.contains(">month</text>"), "axis title kept");
    }

    #[test]
    fn colliding_labels_are_thinned() {
        let xs: Vec<(f64, &str)> = (0..8)
            .map(|i| (70.0 + i as f64 * 28.0, "2026-01"))
            .collect();
        let labels = x_tick_labels(&fit_x_labels(chart(&xs)));
        assert_eq!(labels.len(), 4, "every other label dropped");
        assert!(labels.windows(2).all(|w| !w[0].overlaps(&w[1])));
        assert_eq!(labels[0].x, 70.0, "the first label is kept");
    }

    #[test]
    fn labels_of_ticks_off_the_chart_are_dropped() {
        let labels = x_tick_labels(&fit_x_labels(chart(&[
            (70.0, "5"),
            (200.0, "10"),
            (295.0, "15"),
        ])));
        let texts: Vec<&str> = labels.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(texts, ["5", "10"]);
    }

    #[test]
    fn labels_above_the_panel_bottom_are_not_axis_labels() {
        // e.g. a row of data labels inside the panel
        let svg = chart(&[(70.0, "x")]).replace(
            "<text x=\"70.00\"",
            &format!(
                "{}<text x=\"70.00\"",
                label(275.0, "edge").replace("229.10", "100.00")
            ),
        );
        assert!(x_tick_labels(&svg).is_empty());
    }
}
