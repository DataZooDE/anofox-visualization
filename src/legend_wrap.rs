//! Wrap a top or bottom legend into rows that fit the chart (#175).
//!
//! ggplot-rs (0.19) lays a horizontal legend out on one line and reserves
//! one row for it, so with many categories (twelve cartons, thirty stores)
//! the entries run past the SVG's right edge and are clipped, without a
//! warning. This pass fixes the rendered SVG:
//! - [`plan`] measures the first draw: how many rows the entries need
//!   inside the panel's width (none when they fit);
//! - the panel is drawn again with the extra rows reserved in its plot
//!   margin ([`Panel::legend_rows`](crate::Panel::legend_rows)), so the
//!   panel shrinks and nothing moves under the legend;
//! - [`wrap`] moves the entries onto those rows, keeping ggplot's spacing.
//!   When even [`MAX_SHARE`] of the chart's height is not enough, the last
//!   row ends in "+N more" and the render warns.
//!
//! A legend that fits is not touched, so its SVG is unchanged.
//!
//! TODO(ggplot-rs): wrap horizontal legends in `draw_legend` and reserve
//! their rows in `PlotLayout`, then drop this pass.

use crate::ticks::text_width;

/// Height (px) of one legend row: ggplot-rs's `legend_key_height` in the
/// house themes (`chart_theme`, `void_theme`).
pub(crate) const ROW: f64 = 18.0;

/// The largest share of the chart's height the legend may take.
pub(crate) const MAX_SHARE: f64 = 1.0 / 3.0;

/// Gap (px) kept between a legend entry and the chart's edge.
const EDGE: f64 = 1.0;

/// Smallest gap (px) between one entry's label and the next entry.
const GAP: f64 = 8.0;

/// What the second draw reserves: `rows` legend rows, above the panel
/// (`top`) or below it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Rows {
    pub(crate) rows: usize,
    pub(crate) top: bool,
}

/// One legend entry: its key (swatch) and label.
#[derive(Clone, Debug)]
struct Entry {
    /// Byte range of the entry (key elements and label) in the SVG.
    span: (usize, usize),
    /// Byte range of the label's `<text>` element.
    text: (usize, usize),
    label: String,
    /// Left edge of the entry (its key).
    left: f64,
    /// Label `x` and font size.
    x: f64,
    size: f64,
}

impl Entry {
    /// Width from the key's left edge to the end of the label.
    fn width(&self) -> f64 {
        self.x - self.left + text_width(&self.label, self.size)
    }
}

/// A horizontal legend found in a rendered chart.
struct Legend {
    entries: Vec<Entry>,
    top: bool,
    /// Where entries may sit: from the first entry's left edge to `right`.
    x0: f64,
    right: f64,
    /// The chart's height (its `viewBox`).
    height: f64,
}

impl Legend {
    /// Entry `i`'s pitch in ggplot's layout (left edge to the next entry's
    /// left edge), at least its width plus [`GAP`].
    fn advance(&self, i: usize) -> f64 {
        let e = &self.entries[i];
        let pitch = self
            .entries
            .get(i + 1)
            .map_or(0.0, |next| next.left - e.left);
        pitch.max(e.width() + GAP)
    }

    /// Does every entry sit inside the chart already?
    fn fits(&self) -> bool {
        self.entries
            .iter()
            .all(|e| e.left >= 0.0 && e.left + e.width() <= self.right)
    }

    /// The `(row, left)` of each of the first `n` entries, packed into rows
    /// from `x0` to `right`; `tail` px are kept free at the end of the last
    /// row (for "+N more").
    fn pack(&self, n: usize, tail: f64) -> Vec<(usize, f64)> {
        let mut out = Vec::with_capacity(n);
        let (mut row, mut cur) = (0, self.x0);
        for i in 0..n {
            let w = self.entries[i].width() + if i + 1 == n { tail } else { 0.0 };
            if cur > self.x0 && cur + w > self.right {
                row += 1;
                cur = self.x0;
            }
            out.push((row, cur));
            cur += self.advance(i);
        }
        out
    }

    /// Rows the whole legend needs.
    fn rows(&self) -> usize {
        self.pack(self.entries.len(), 0.0)
            .last()
            .map_or(1, |(r, _)| r + 1)
    }

    /// Rows the chart can give the legend.
    fn max_rows(&self) -> usize {
        1 + (self.height * MAX_SHARE / ROW).floor() as usize
    }
}

/// The rows a chart's legend needs, when it does not fit on one line (see
/// the module docs). `None` when there is no horizontal legend or it fits.
pub(crate) fn plan(svg: &str) -> Option<Rows> {
    let l = find(svg)?;
    if l.fits() {
        return None;
    }
    let rows = l.rows().min(l.max_rows());
    (rows > 1).then_some(Rows { rows, top: l.top })
}

/// Move the legend's entries onto `reserved` rows (the rows the panel was
/// drawn with). Entries that don't fit are dropped for a "+N more" label,
/// and `warnings` says so. A legend that fits is left as it is.
pub(crate) fn wrap(svg: String, reserved: usize, warnings: &mut Vec<String>) -> String {
    let Some(l) = find(&svg) else {
        return svg;
    };
    if l.fits() {
        return svg;
    }
    let total = l.entries.len();
    let reserved = reserved.max(1);
    let mut shown = total;
    let mut more = String::new();
    let mut place = l.pack(total, 0.0);
    while shown > 0 && place.last().is_some_and(|(r, _)| *r >= reserved) {
        shown -= 1;
        more = format!("+{} more", total - shown);
        let size = l.entries[0].size;
        place = l.pack(shown, GAP + text_width(&more, size));
    }
    let mut edits: Vec<((usize, usize), String)> = Vec::new();
    for (e, (row, left)) in l.entries.iter().zip(&place) {
        let dy = *row as f64 * ROW;
        edits.push((e.span, shifted(&svg[e.span.0..e.span.1], left - e.left, dy)));
    }
    for e in &l.entries[shown..] {
        edits.push((e.span, String::new()));
    }
    if shown < total {
        // "+N more" in the label style, after the last entry shown.
        let last = &l.entries[shown.saturating_sub(1)];
        let (row, x) = match place.last() {
            Some((row, left)) => (*row, left + last.width() + GAP),
            None => (0, l.x0),
        };
        let el = &svg[last.text.0..last.text.1];
        let tag_end = el.find('>').unwrap_or(el.len());
        let tag = set_num(&el[..tag_end], "x", x);
        let tag = set_num(
            &tag,
            "y",
            attr_num(&tag, "y").unwrap_or(0.0) + row as f64 * ROW,
        );
        let at = l.entries[total - 1].span.1;
        edits.push(((at, at), format!("{tag}>{more}</text>")));
        warnings.push(format!(
            "legend: {} of {total} entries don't fit the chart; the legend shows {shown} and \"{more}\"",
            total - shown
        ));
    }
    edits.sort_by_key(|((a, b), _)| (*a, *b));
    let mut out = String::with_capacity(svg.len());
    let mut at = 0;
    for ((a, b), new) in edits {
        out.push_str(&svg[at..a]);
        out.push_str(&new);
        at = b;
    }
    out.push_str(&svg[at..]);
    out
}

/// The horizontal legend of a single chart: the longest run of
/// start-anchored labels on one baseline above (or below) the panel, each
/// right after its key. Charts with nested charts (a grid) are skipped.
fn find(svg: &str) -> Option<Legend> {
    if !svg.starts_with("<svg") || svg[4..].contains("<svg") {
        return None;
    }
    let open = &svg[..svg.find('>')?];
    let plot = nums(attr(open, "data-plot")?);
    let view = nums(attr(open, "viewBox")?);
    let (&px, &py, &pw, &ph) = (plot.first()?, plot.get(1)?, plot.get(2)?, plot.get(3)?);
    let (&w, &h) = (view.get(2)?, view.get(3)?);
    // Labels after a key, grouped by baseline and font size.
    let mut runs: Vec<(String, Vec<(usize, usize)>)> = Vec::new();
    let mut at = 0;
    while let Some(s) = svg[at..].find("<text ").map(|s| s + at) {
        let e = s + svg[s..].find("</text>")? + "</text>".len();
        at = e;
        let tag = &svg[s..s + svg[s..].find('>')?];
        if attr(tag, "text-anchor") != Some("start")
            || attr(tag, "transform").is_some()
            || !svg[..s].ends_with("/>")
        {
            continue;
        }
        let y = attr_num(tag, "y")?;
        if y >= py && y <= py + ph {
            continue;
        }
        let key = format!(
            "{} {}",
            attr(tag, "y").unwrap_or(""),
            attr(tag, "font-size").unwrap_or("")
        );
        match runs.iter_mut().find(|(k, _)| *k == key) {
            Some((_, v)) => v.push((s, e)),
            None => runs.push((key, vec![(s, e)])),
        }
    }
    let (_, texts) = runs.into_iter().max_by_key(|(_, v)| v.len())?;
    if texts.len() < 2 {
        return None;
    }
    // Each entry is its key elements, then its label; the keys of entry 0
    // are as many elements as entry 1 has before its label.
    let keys = elements(&svg[texts[0].1..texts[1].0])?;
    let mut start = texts[0].0;
    for _ in 0..keys {
        start = svg[..start].rfind('<')?;
    }
    let mut entries = Vec::with_capacity(texts.len());
    for (i, &(s, e)) in texts.iter().enumerate() {
        let from = if i == 0 { start } else { texts[i - 1].1 };
        let body = &svg[from..s];
        if elements(body)? != keys {
            return None; // not one legend (another text in between)
        }
        let tag = &svg[s..s + svg[s..].find('>')?];
        let x = attr_num(tag, "x")?;
        entries.push(Entry {
            span: (from, e),
            text: (s, e),
            label: unescape(&svg[s + tag.len() + 1..e - "</text>".len()]),
            left: left_edge(body).unwrap_or(x),
            x,
            size: attr_num(tag, "font-size")?,
        });
    }
    let top = attr_num(&svg[texts[0].0..], "y")? < py;
    Some(Legend {
        x0: entries[0].left,
        right: (px + pw).min(w - EDGE),
        entries,
        top,
        height: h,
    })
}

/// The number of self-closing elements in `body` (`None` if it holds
/// anything else).
fn elements(body: &str) -> Option<usize> {
    let mut n = 0;
    let mut rest = body;
    while !rest.is_empty() {
        if !rest.starts_with('<') || rest.starts_with("</") || rest.starts_with("<text") {
            return None;
        }
        let end = rest.find("/>")? + 2;
        if rest[1..end].contains('<') {
            return None;
        }
        n += 1;
        rest = &rest[end..];
    }
    (n > 0).then_some(n)
}

/// The smallest x of the elements in `body` (a legend key).
fn left_edge(body: &str) -> Option<f64> {
    let mut min = f64::INFINITY;
    for el in body.split_inclusive("/>") {
        let x = if let Some(x) = attr_num(el, "x") {
            x
        } else if let Some(cx) = attr_num(el, "cx") {
            cx - attr_num(el, "r").unwrap_or(0.0)
        } else if let Some(p) = attr(el, "points") {
            p.split([' ', ','])
                .step_by(2)
                .filter_map(|v| v.parse::<f64>().ok())
                .fold(f64::INFINITY, f64::min)
        } else {
            continue;
        };
        min = min.min(x);
    }
    min.is_finite().then_some(min)
}

/// `el` (a legend entry) moved by `(dx, dy)`.
fn shifted(el: &str, dx: f64, dy: f64) -> String {
    if dx == 0.0 && dy == 0.0 {
        return el.to_string();
    }
    let mut out = String::with_capacity(el.len() + 16);
    let mut rest = el;
    while let Some(s) = rest.find('<') {
        out.push_str(&rest[..s]);
        rest = &rest[s..];
        let end = rest.find('>').map_or(rest.len(), |e| e + 1);
        let tag = &rest[..end];
        out.push_str(&if tag.starts_with("</") {
            tag.to_string()
        } else if tag.starts_with("<path") {
            // `+` / `×` keys: move the whole element
            format!("<g transform=\"translate({dx:.2} {dy:.2})\">{tag}</g>")
        } else {
            moved_tag(tag, dx, dy)
        });
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

/// A start tag with its position attributes moved by `(dx, dy)`.
fn moved_tag(tag: &str, dx: f64, dy: f64) -> String {
    let mut t = tag.to_string();
    for (k, d) in [("x", dx), ("y", dy), ("cx", dx), ("cy", dy)] {
        if let Some(v) = attr_num(&t, k) {
            t = set_num(&t, k, v + d);
        }
    }
    if let Some(p) = attr(&t, "points") {
        let moved: Vec<String> = p
            .split_whitespace()
            .map(|pt| match pt.split_once(',') {
                Some((x, y)) => match (x.parse::<f64>(), y.parse::<f64>()) {
                    (Ok(x), Ok(y)) => format!("{:.2},{:.2}", x + dx, y + dy),
                    _ => pt.to_string(),
                },
                None => pt.to_string(),
            })
            .collect();
        t = set_attr(&t, "points", &moved.join(" "));
    }
    t
}

fn attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let pat = format!(" {name}=\"");
    let i = tag.find(&pat)? + pat.len();
    tag[i..].find('"').map(|e| &tag[i..i + e])
}

fn attr_num(tag: &str, name: &str) -> Option<f64> {
    attr(tag, name)?.parse().ok()
}

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

fn set_num(tag: &str, name: &str, v: f64) -> String {
    set_attr(tag, name, &format!("{v:.2}"))
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

    /// A 640×400 chart with a one-line top legend of `n` entries, laid out
    /// the way ggplot-rs draws it.
    fn chart(n: usize, width: f64) -> String {
        let mut body = String::new();
        for i in 0..n {
            let x = 72.05 + i as f64 * 77.1;
            body += &format!(
                "<rect x=\"{x:.2}\" y=\"10.00\" width=\"12.00\" height=\"12.00\" fill=\"#2563EB\"/>\
                 <text x=\"{:.2}\" y=\"16.00\" font-size=\"8.80\" text-anchor=\"start\" \
                 fill=\"#2C2E33\">carton {:02}</text>",
                x + 16.0,
                i + 1
            );
        }
        format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"400\" \
             viewBox=\"0 0 {width} 400\" data-plot=\"62.05 47.50 {:.2} 312.45\">{body}</svg>",
            width - 67.55
        )
    }

    fn labels(svg: &str) -> Vec<(f64, f64, String)> {
        svg.split("<text ")
            .skip(1)
            .map(|t| {
                let end = t.find('>').unwrap();
                let tag = format!(" {}", &t[..end]);
                let text = &t[end + 1..t.find("</text>").unwrap()];
                (
                    attr_num(&tag, "x").unwrap(),
                    attr_num(&tag, "y").unwrap(),
                    text.to_string(),
                )
            })
            .collect()
    }

    #[test]
    fn a_fitting_legend_is_untouched() {
        let svg = chart(3, 640.0);
        assert_eq!(plan(&svg), None);
        assert_eq!(wrap(svg.clone(), 1, &mut Vec::new()), svg);
    }

    #[test]
    fn twelve_entries_wrap_onto_two_rows() {
        let svg = chart(12, 640.0);
        assert_eq!(plan(&svg), Some(Rows { rows: 2, top: true }));
        let mut warnings = Vec::new();
        let out = wrap(svg, 2, &mut warnings);
        assert!(warnings.is_empty(), "{warnings:?}");
        let l = labels(&out);
        assert_eq!(l.len(), 12);
        for (x, _, t) in &l {
            assert!(x + text_width(t, 8.8) <= 634.45 + 1e-6, "{t} at {x}");
        }
        assert_eq!(l[0], (88.05, 16.0, "carton 01".into()), "first entry stays");
        assert!(l.iter().any(|(_, y, _)| *y == 16.0 + ROW), "a second row");
        // keys move with their labels
        assert_eq!(
            out.matches("y=\"28.00\"").count(),
            l.iter().filter(|e| e.1 > 16.0).count()
        );
    }

    #[test]
    fn too_many_entries_end_in_n_more_and_warn() {
        let svg = chart(30, 320.0);
        let rows = plan(&svg).unwrap().rows;
        assert_eq!(rows, 1 + (400.0 * MAX_SHARE / ROW) as usize);
        let mut warnings = Vec::new();
        let out = wrap(svg, rows, &mut warnings);
        let l = labels(&out);
        let last = l.last().unwrap();
        assert!(
            last.2.starts_with('+') && last.2.ends_with(" more"),
            "{last:?}"
        );
        assert_eq!(warnings.len(), 1);
        assert!(l.iter().all(|(y, _, _)| *y >= 0.0));
        assert!(l.iter().all(|(_, y, _)| *y < 16.0 + rows as f64 * ROW));
    }

    #[test]
    fn without_reserved_rows_it_keeps_one_row() {
        let mut warnings = Vec::new();
        let out = wrap(chart(12, 640.0), 1, &mut warnings);
        assert!(labels(&out).iter().all(|(_, y, _)| *y == 16.0));
        assert_eq!(warnings.len(), 1);
    }
}
