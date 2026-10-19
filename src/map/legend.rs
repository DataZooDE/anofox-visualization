//! The legend box: category swatches, a gradient bar or stepped classes
//! with counts, the no-data hatch — and, stacked under it, extras such as a
//! graduated-symbol size key (`::SIZE`, M2a).

use crate::format::escape_xml as esc;
use crate::theme::hex;
use crate::RenderOptions;

use super::{Fill, Model, Rgb, FONT};

/// Line height of a legend row.
pub(super) const ROW: f64 = 13.0;
/// Inner padding of the legend box.
pub(super) const PAD: f64 = 5.0;
/// Gap between the main legend and its first extra, and between extras.
const EXTRA_GAP: f64 = 4.0;

/// What the legend shows.
pub(super) enum Legend {
    None,
    Category(Vec<(String, Rgb)>),
    Gradient {
        stops: Vec<Rgb>,
        /// `(position 0..1, label)`.
        ticks: Vec<(f64, String)>,
    },
    Steps {
        colors: Vec<Rgb>,
        ticks: Vec<String>,
        counts: Vec<usize>,
    },
}

/// Extra content stacked under the main legend.
pub(super) enum Extra {
    /// A graduated-symbol size key: nested circles with nice values.
    Sizes {
        title: Option<String>,
        /// `(radius px, label)`, largest first.
        items: Vec<(f64, String)>,
    },
    /// A flow-width key (M2b): horizontal line samples with nice values.
    Widths {
        title: Option<String>,
        /// `(width px, label)`, widest first.
        items: Vec<(f64, String)>,
    },
}

/// The legend box: its content, the no-data count, the title and size.
pub(super) struct LegendBox {
    pub legend: Legend,
    pub nodata: usize,
    pub title: Option<String>,
    pub extras: Vec<Extra>,
    pub w: f64,
    pub h: f64,
    /// Gradient bar width / class swatch width.
    pub bar: f64,
}

/// The legend title of a measure: its name (not a generated `c3`) and its
/// unit from the value format — "Revenue (€)", "Share (%)".
pub(super) fn legend_title(name: &str, fmt: &crate::format::ValueFmt) -> Option<String> {
    use crate::format::ValueFmt;
    let generated = name.is_empty()
        || (name.starts_with('c')
            && name.len() > 1
            && name[1..].bytes().all(|b| b.is_ascii_digit()));
    if generated {
        return None;
    }
    let unit = match fmt {
        ValueFmt::Currency(u) => Some(u.trim().to_string()),
        ValueFmt::Percent | ValueFmt::Share => Some("%".to_string()),
        ValueFmt::Prefix(u) | ValueFmt::Suffix(u) => Some(u.trim().to_string()),
        _ => None,
    }
    .filter(|u| !u.is_empty() && !name.contains(u.as_str()));
    Some(match unit {
        Some(u) => format!("{name} ({u})"),
        None => name.to_string(),
    })
}

/// "Nice" ticks (1, 2, 2.5, 5 × 10ⁿ) inside `[lo, hi]`, at most `n`.
pub(super) fn nice_ticks(lo: f64, hi: f64, n: usize) -> Vec<f64> {
    let span = hi - lo;
    if span <= 0.0 || !span.is_finite() {
        return Vec::new();
    }
    let raw = span / n.max(1) as f64;
    let mag = 10f64.powf(raw.log10().floor());
    let step = [1.0, 2.0, 2.5, 5.0, 10.0]
        .iter()
        .map(|m| m * mag)
        .find(|st| span / st <= n as f64)
        .unwrap_or(10.0 * mag);
    let mut out = Vec::new();
    let mut v = (lo / step).ceil() * step;
    while v <= hi + step * 1e-9 {
        out.push(if v.abs() < step * 1e-9 { 0.0 } else { v });
        v += step;
    }
    out
}

/// The largest "nice" value (1, 2, 2.5, 5 × 10ⁿ) that is ≤ `x` (`x > 0`).
pub(super) fn nice_floor(x: f64) -> f64 {
    if x <= 0.0 || !x.is_finite() {
        return 0.0;
    }
    let mag = 10f64.powf(x.log10().floor());
    [5.0, 2.5, 2.0, 1.0]
        .iter()
        .map(|m| m * mag)
        .find(|v| *v <= x + x * 1e-9)
        .unwrap_or(mag / 10.0)
}

/// The size-key extra of a graduated-symbol map (`None` without one, or
/// without room for even the smallest nice value).
fn sizes_extra(o: &RenderOptions, m: &Model, w: f64, h: f64) -> Option<(Extra, f64, f64)> {
    let sym = m.symbols.as_ref()?;
    if sym.fixed.is_some() || sym.vmax <= 0.0 {
        return None;
    }
    let rmax = super::symbols::rmax(w, h);
    let v1 = nice_floor(sym.vmax);
    if v1 <= 0.0 {
        return None;
    }
    let v2 = nice_floor(v1 / 4.0);
    let v3 = nice_floor(v1 / 16.0);
    let mut values = vec![v1, v2, v3];
    values.retain(|v| *v > 0.0);
    values.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
    let tw = |s: &str| crate::ticks::text_width(s, FONT);
    let items: Vec<(f64, String)> = values
        .into_iter()
        .map(|v| {
            (
                super::symbols::radius(rmax, v, sym.vmax),
                sym.fmt.short(v, &o.locale),
            )
        })
        .filter(|(r, _)| *r >= 3.0)
        .collect();
    if items.is_empty() {
        return None;
    }
    let r1 = items[0].0;
    let widest = items.iter().map(|(_, l)| tw(l)).fold(0.0, f64::max);
    let width = 2.0 * r1 + 8.0 + widest;
    let title_row = if sym.title.is_some() { ROW } else { 0.0 };
    let height = 2.0 * r1 + title_row;
    Some((
        Extra::Sizes {
            title: sym.title.clone(),
            items,
        },
        width,
        height,
    ))
}

/// The width-key extra of a flow map (`None` without a varying `::SIZE`,
/// mirroring [`sizes_extra`]'s guard).
fn widths_extra(o: &RenderOptions, m: &Model) -> Option<(Extra, f64, f64)> {
    let fl = m.flows.as_ref()?;
    if fl.fixed.is_some() || fl.vmax <= 0.0 {
        return None;
    }
    let v1 = nice_floor(fl.vmax);
    if v1 <= 0.0 {
        return None;
    }
    let v2 = nice_floor(v1 / 4.0);
    let v3 = nice_floor(v1 / 16.0);
    let mut values = vec![v1, v2, v3];
    values.retain(|v| *v > 0.0);
    values.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
    let tw = |s: &str| crate::ticks::text_width(s, FONT);
    let width_of = |v: f64| -> f64 { 0.75 + 7.25 * (v / fl.vmax).sqrt() };
    let items: Vec<(f64, String)> = values
        .into_iter()
        .map(|v| (width_of(v), fl.fmt.short(v, &o.locale)))
        .filter(|(w, _)| *w >= 1.0)
        .collect();
    if items.is_empty() {
        return None;
    }
    let widest_label = items.iter().map(|(_, l)| tw(l)).fold(0.0, f64::max);
    let line_len = 24.0_f64;
    let width = line_len + 6.0 + widest_label;
    let title_row = if fl.title.is_some() { ROW } else { 0.0 };
    let height = title_row + items.len() as f64 * ROW;
    Some((
        Extra::Widths {
            title: fl.title.clone(),
            items,
        },
        width,
        height,
    ))
}

/// The legend of a map, sized for a panel `w`×`h`.
pub(super) fn legend_of(o: &RenderOptions, m: &Model, w: f64, h: f64) -> LegendBox {
    // drawn rows without a value (rows without geometry — a backdrop's —
    // don't count); with graduated symbols a row's regions may have moved
    // into the backdrop (D4), so a row counts when it has a symbol anchor.
    let nodata = match (&m.fill, &m.symbols) {
        (Fill::Measure(s), Some(sym)) => (0..m.rows)
            .filter(|i| s.values[*i].is_none() && !sym.anchors[*i].is_empty())
            .count(),
        (Fill::Measure(s), None) => (0..m.rows)
            .filter(|i| s.values[*i].is_none() && !m.features[*i].is_empty())
            .count(),
        _ => 0,
    };
    let tw = |s: &str| crate::ticks::text_width(s, FONT);
    let max_w = (w - 16.0).max(60.0);
    let mut title = None;
    let mut bar = 0.0;
    let (legend, mut cw, mut ch) = match &m.fill {
        Fill::Plain(_) => (Legend::None, 0.0, 0.0),
        Fill::Category { levels, .. } => {
            let rows = category_rows(levels, max_w);
            let cw = rows
                .iter()
                .map(|r| {
                    r.last()
                        .map(|(x, i)| x + 13.0 + tw(&levels[*i].0))
                        .unwrap_or(0.0)
                })
                .fold(0.0, f64::max);
            (
                Legend::Category(levels.clone()),
                cw,
                rows.len() as f64 * ROW,
            )
        }
        Fill::Measure(s) => {
            title = s.title.clone();
            let short = |v: f64| s.fmt.short(v, &o.locale);
            match &s.classes {
                Some((_, c, colors)) => {
                    let mut ticks: Vec<String> = (0..=c.len())
                        .map(|i| {
                            let v = if i == 0 {
                                c.lo
                            } else if i == c.len() {
                                c.hi
                            } else {
                                c.edges[i - 1]
                            };
                            short(v)
                        })
                        .collect();
                    if c.open_lo {
                        ticks[0] = format!("≤{}", ticks[0]);
                    }
                    if c.open_hi {
                        let l = ticks.len() - 1;
                        ticks[l] = format!("≥{}", ticks[l]);
                    }
                    let k = colors.len() as f64;
                    let widest = ticks.iter().map(|t| tw(t)).fold(0.0, f64::max);
                    bar = (widest + 8.0).clamp(26.0, 48.0).min(max_w / k);
                    let lw = k * bar + tw(&ticks[ticks.len() - 1]) / 2.0;
                    (
                        Legend::Steps {
                            colors: colors.clone(),
                            ticks,
                            counts: c.counts.clone(),
                        },
                        lw,
                        12.0 + 3.0 + ROW,
                    )
                }
                None => {
                    bar = (w * 0.3).clamp(90.0, 160.0).min(max_w);
                    let stops = (0..=16)
                        .map(|i| s.color_at(s.lo + (s.hi - s.lo) * i as f64 / 16.0))
                        .collect();
                    let span = s.hi - s.lo;
                    let pos = |v: f64| if span > 0.0 { (v - s.lo) / span } else { 0.5 };
                    let cand: Vec<(f64, String)> = nice_ticks(s.lo, s.hi, 5)
                        .into_iter()
                        .map(|v| (pos(v), short(v)))
                        .collect();
                    // a label's extent along the bar (the ends are anchored
                    // inside it)
                    let ext = |p: f64, l: &str| {
                        let (x, w) = (p * bar, tw(l));
                        if p <= 0.0 {
                            (x, x + w)
                        } else if p >= 1.0 {
                            (x - w, x)
                        } else {
                            (x - w / 2.0, x + w / 2.0)
                        }
                    };
                    let clear = |a: (f64, f64), b: (f64, f64)| a.1 + 4.0 <= b.0 || b.1 + 4.0 <= a.0;
                    // the nice ticks that fit side by side
                    let mut ticks: Vec<(f64, String)> = Vec::new();
                    for (p, l) in cand {
                        if ticks
                            .last()
                            .is_none_or(|(q, m)| clear(ext(*q, m), ext(p, &l)))
                        {
                            ticks.push((p, l));
                        }
                    }
                    // the ends: always when capped (≤lo / ≥hi), else when they fit
                    let ends = [
                        (0.0, s.capped_lo, format!("≤{}", short(s.lo)), short(s.lo)),
                        (1.0, s.capped_hi, format!("≥{}", short(s.hi)), short(s.hi)),
                    ];
                    for (p, capped, lc, l) in ends {
                        let l = if capped { lc } else { l };
                        let e = ext(p, &l);
                        if capped {
                            ticks.retain(|(q, m)| clear(ext(*q, m), e));
                        } else if ticks
                            .iter()
                            .any(|(q, m)| (q - p).abs() < 1e-9 || !clear(ext(*q, m), e))
                        {
                            continue;
                        }
                        if p <= 0.0 {
                            ticks.insert(0, (p, l));
                        } else {
                            ticks.push((p, l));
                        }
                    }
                    if span <= 0.0 {
                        ticks = vec![(0.5, short(s.lo))];
                    }
                    let over = ticks
                        .iter()
                        .map(|(p, l)| p * bar + tw(l) / 2.0)
                        .fold(bar, f64::max);
                    (Legend::Gradient { stops, ticks }, over, 8.0 + 3.0 + ROW)
                }
            }
        }
    };
    if let Some(t) = &title {
        cw = cw.max(tw(t));
        ch += ROW;
    }
    if nodata > 0 {
        cw = cw.max(16.0 + tw(&format!("No data ({nodata})")));
        ch += ROW + 2.0;
    }
    let mut extras = Vec::new();
    let mut ew = 0.0_f64;
    let mut eh = 0.0_f64;
    if let Some((extra, xw, xh)) = sizes_extra(o, m, w, h) {
        ew = ew.max(xw);
        eh += xh + EXTRA_GAP;
        extras.push(extra);
    }
    if let Some((extra, xw, xh)) = widths_extra(o, m) {
        ew = ew.max(xw);
        eh += xh + EXTRA_GAP;
        extras.push(extra);
    }
    let has_content = ch > 0.0 || !extras.is_empty();
    let (w, h) = if has_content {
        (
            (cw.max(ew) + 2.0 * PAD).min(w - 4.0),
            ch + eh + 2.0 * PAD - 2.0,
        )
    } else {
        (0.0, 0.0)
    };
    LegendBox {
        legend,
        nodata,
        title,
        extras,
        w,
        h,
        bar,
    }
}

/// Category legend entries packed into rows that fit `w`: `(x, entry)`.
pub(super) fn category_rows(items: &[(String, Rgb)], w: f64) -> Vec<Vec<(f64, usize)>> {
    let mut rows: Vec<Vec<(f64, usize)>> = vec![Vec::new()];
    let mut x = 0.0;
    for (i, (name, _)) in items.iter().enumerate() {
        let iw = 13.0 + crate::ticks::text_width(name, FONT) + 10.0;
        if x + iw - 10.0 > w && !rows.last().unwrap().is_empty() {
            rows.push(Vec::new());
            x = 0.0;
        }
        rows.last_mut().unwrap().push((x, i));
        x += iw;
    }
    rows
}

/// The legend box at `(x, y)`: the title, then category swatches, a
/// gradient bar with ticks, or stepped classes with their counts; the
/// no-data hatch with its count; then any extras (a size key).
pub(super) fn legend_svg(
    o: &RenderOptions,
    lg: &LegendBox,
    id: &str,
    (x, y): (f64, f64),
    font: &str,
) -> String {
    if lg.w <= 0.0 {
        return String::new();
    }
    let t = &o.theme;
    let ink = hex(t.chart_label());
    let muted = hex(t.chart_text());
    let text = |x: f64, y: f64, anchor: &str, fill: &str, weight: &str, s: &str| {
        format!(
            "<text x=\"{x:.1}\" y=\"{y:.1}\" text-anchor=\"{anchor}\" dominant-baseline=\"middle\" font-family=\"{font}\" font-size=\"{FONT}\"{weight} fill=\"{fill}\">{}</text>",
            esc(s)
        )
    };
    let mut g = format!(
        "<g class=\"dp-legend\" transform=\"translate({x:.1},{y:.1})\"><rect width=\"{:.1}\" height=\"{:.1}\" rx=\"3\" fill=\"{}\" fill-opacity=\".85\"/>",
        lg.w,
        lg.h,
        hex(t.colors.card)
    );
    let mut y = PAD;
    if let Some(title) = &lg.title {
        g.push_str(&text(
            PAD,
            y + ROW / 2.0 - 1.0,
            "start",
            &muted,
            " font-weight=\"600\"",
            title,
        ));
        y += ROW;
    }
    let bar = lg.bar;
    match &lg.legend {
        Legend::None => {}
        Legend::Category(items) => {
            let rows = category_rows(items, lg.w - 2.0 * PAD);
            for row in &rows {
                let cy = y + ROW / 2.0;
                for (cx, i) in row {
                    let (name, c) = &items[*i];
                    let cx = PAD + cx;
                    g.push_str(&format!(
                        "<rect x=\"{cx:.1}\" y=\"{:.1}\" width=\"9\" height=\"9\" rx=\"1.5\" fill=\"{}\"/>",
                        cy - 4.5,
                        hex(*c)
                    ));
                    g.push_str(&text(cx + 13.0, cy, "start", &ink, "", name));
                }
                y += ROW;
            }
        }
        Legend::Gradient { ticks, .. } => {
            g.push_str(&format!(
                "<rect x=\"{PAD}\" y=\"{y:.1}\" width=\"{bar:.1}\" height=\"8\" rx=\"1.5\" fill=\"url(#{id}-g)\"/>"
            ));
            for (p, s) in ticks {
                let tx = PAD + p * bar;
                let anchor = if *p <= 0.0 {
                    "start"
                } else if *p >= 1.0 {
                    "end"
                } else {
                    "middle"
                };
                g.push_str(&format!(
                    "<path d=\"M{tx:.1} {:.1}v3\" stroke=\"{muted}\" stroke-width=\".6\"/>",
                    y + 8.0
                ));
                g.push_str(&text(tx, y + 11.0 + ROW / 2.0, anchor, &ink, "", s));
            }
            y += 11.0 + ROW;
        }
        Legend::Steps {
            colors,
            ticks,
            counts,
        } => {
            let tw = |s: &str| crate::ticks::text_width(s, FONT);
            for (i, c) in colors.iter().enumerate() {
                let sx = PAD + i as f64 * bar;
                g.push_str(&format!(
                    "<rect x=\"{sx:.1}\" y=\"{y:.1}\" width=\"{bar:.1}\" height=\"12\" fill=\"{}\"/>",
                    hex(*c)
                ));
                g.push_str(&format!(
                    "<text x=\"{:.1}\" y=\"{:.1}\" text-anchor=\"middle\" dominant-baseline=\"middle\" font-family=\"{font}\" font-size=\"7.5\" fill=\"{}\">{}</text>",
                    sx + bar / 2.0,
                    y + 6.5,
                    hex(t.text_on(*c)),
                    counts[i]
                ));
            }
            // boundary labels, skipping one that would run into the last
            let mut right = f64::NEG_INFINITY;
            let n = ticks.len();
            for (i, s) in ticks.iter().enumerate() {
                let tx = PAD + i as f64 * bar;
                let (anchor, l, r) = if i == 0 {
                    ("start", tx, tx + tw(s))
                } else {
                    ("middle", tx - tw(s) / 2.0, tx + tw(s) / 2.0)
                };
                if l < right + 3.0 && i != n - 1 {
                    continue;
                }
                right = r;
                g.push_str(&format!(
                    "<path d=\"M{tx:.1} {:.1}v3\" stroke=\"{muted}\" stroke-width=\".6\"/>",
                    y + 12.0
                ));
                g.push_str(&text(tx, y + 15.0 + ROW / 2.0, anchor, &ink, "", s));
            }
            y += 15.0 + ROW;
        }
    }
    if lg.nodata > 0 {
        let cy = y + 2.0 + ROW / 2.0;
        g.push_str(&format!(
            "<rect x=\"{PAD}\" y=\"{:.1}\" width=\"11\" height=\"11\" fill=\"url(#{id}-nd)\" stroke=\"{}\" stroke-width=\".5\"/>",
            cy - 5.5,
            hex(t.chart_grid())
        ));
        g.push_str(&text(
            PAD + 16.0,
            cy,
            "start",
            &ink,
            "",
            &format!("No data ({})", lg.nodata),
        ));
        y += ROW + 2.0;
    }
    for extra in &lg.extras {
        y += EXTRA_GAP;
        match extra {
            Extra::Sizes { title, items } => {
                let r1 = items[0].0;
                if let Some(t) = title {
                    g.push_str(&text(
                        PAD,
                        y + ROW / 2.0 - 1.0,
                        "start",
                        &muted,
                        " font-weight=\"600\"",
                        t,
                    ));
                    y += ROW;
                }
                let cx = PAD + r1;
                // nested circles, bottom-aligned on one baseline
                let base_y = y + 2.0 * r1;
                for (r, label) in items {
                    g.push_str(&format!(
                        "<circle cx=\"{cx:.1}\" cy=\"{:.1}\" r=\"{r:.1}\" fill=\"none\" stroke=\"{muted}\" stroke-width=\".75\"/>",
                        base_y - r
                    ));
                    let lx = PAD + 2.0 * r1 + 4.0;
                    g.push_str(&format!(
                        "<path d=\"M{:.1} {:.1}v-3h{:.1}\" stroke=\"{muted}\" stroke-width=\".6\" fill=\"none\"/>",
                        cx,
                        base_y - 2.0 * r,
                        lx - cx
                    ));
                    g.push_str(&text(lx, base_y - 2.0 * r - 3.0, "start", &ink, "", label));
                }
            }
            Extra::Widths { title, items } => {
                if let Some(t) = title {
                    g.push_str(&text(
                        PAD,
                        y + ROW / 2.0 - 1.0,
                        "start",
                        &muted,
                        " font-weight=\"600\"",
                        t,
                    ));
                    y += ROW;
                }
                const LINE: f64 = 24.0;
                for (w, label) in items {
                    let cy = y + ROW / 2.0;
                    g.push_str(&format!(
                        "<line x1=\"{PAD}\" y1=\"{cy:.1}\" x2=\"{:.1}\" y2=\"{cy:.1}\" stroke=\"{muted}\" stroke-width=\"{w:.2}\" stroke-linecap=\"round\"/>",
                        PAD + LINE
                    ));
                    g.push_str(&text(PAD + LINE + 6.0, cy, "start", &ink, "", label));
                    y += ROW;
                }
            }
        }
    }
    g.push_str("</g>");
    g
}
