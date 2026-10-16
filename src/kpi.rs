//! KPI tiles — the one place that decides what a big-number card shows.
//!
//! A KPI statement has a `::METRIC`/`::MONEY`/`::PERCENT`/`::COMPACT` column
//! and optionally:
//!
//! - `::DELTA` / `::DELTA_LOWER_GOOD` — the comparison value → an arrow and the
//!   change, coloured good/bad (a rise is bad for `::DELTA_LOWER_GOOD`);
//! - `::DELTA_FORMAT` — `'pct'` (default), `'abs'` or `'both'`: the change as
//!   a percentage, as an absolute difference in the KPI's own format (`+$1,200`,
//!   `+2.3 pp` for a `::PERCENT`, `+1.2K`), or both;
//! - `::DELTA_LABEL` — what the change compares against (`'vs last month'`);
//! - `::SPARKLINE` — `list(v ORDER BY d)`, a trend line under the value;
//! - `::GOAL` / `::TARGET` (= `::REFLINE`) — the goal → a progress bar;
//! - `::LABEL` — the caption.
//!
//! [`delta`] and [`goal`] produce the texts for every renderer (the browser
//! calls them through the wasm export `kpi_parts`); [`Kpi::body`] draws the
//! tile as SVG for the headless outputs (`dashboard --svg`, the static HTML).

use crate::format::{escape_xml as esc, format_number_l, Locale};
use crate::theme::hex;
use crate::{value_str, Column, Kind, MetricFmt, RenderOptions, Role};
use ggplot_rs::prelude::Value;

/// How a KPI's change reads (`::DELTA_FORMAT`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DeltaFormat {
    /// `▲ 12.3%` — the relative change (default).
    #[default]
    Pct,
    /// `▲ +$1,200` — the difference in the KPI's own format.
    Abs,
    /// `▲ 12.3% (+$1,200)`.
    Both,
}

impl DeltaFormat {
    /// The accepted spellings (for messages).
    pub const NAMES: &'static [&'static str] = &["pct", "abs", "both"];

    /// Parse `'pct'` / `'abs'` / `'both'` (case-insensitive; also `percent`,
    /// `absolute`, `%`). Empty → `Pct`; anything else → `None`.
    pub fn parse(s: &str) -> Option<DeltaFormat> {
        match s.trim().to_ascii_lowercase().as_str() {
            "" | "pct" | "percent" | "%" | "relative" => Some(DeltaFormat::Pct),
            "abs" | "absolute" | "diff" => Some(DeltaFormat::Abs),
            "both" | "pct+abs" | "abs+pct" => Some(DeltaFormat::Both),
            _ => None,
        }
    }
}

/// A KPI's change against its comparison value.
#[derive(Clone, Debug, PartialEq)]
pub struct Delta {
    /// The value rose (or stayed equal).
    pub up: bool,
    /// The change is in the good direction (`::DELTA_LOWER_GOOD` flips it).
    pub good: bool,
    /// `▲` or `▼`.
    pub arrow: &'static str,
    /// The relative change without sign (`12.3%`), when shown.
    pub pct: Option<String>,
    /// The signed absolute change (`+$1,200`, `−2.3 pp`), when shown.
    pub abs: Option<String>,
    /// The whole line after the arrow: `12.3%`, `+$1,200` or `12.3% (+$1,200)`.
    pub text: String,
}

/// The change from `prev` to `cur`, or `None` when either is missing (e.g. a
/// `::PERIOD` with "No comparison" yields a NULL comparison). With a zero
/// comparison value a percentage is undefined: the absolute part is shown.
pub fn delta(
    cur: f64,
    prev: f64,
    fmt: MetricFmt,
    df: DeltaFormat,
    lower_good: bool,
    l: &Locale,
) -> Option<Delta> {
    if !cur.is_finite() || !prev.is_finite() {
        return None;
    }
    let diff = cur - prev;
    let up = diff >= 0.0;
    let pct = (df != DeltaFormat::Abs && prev != 0.0)
        .then(|| format_number_l((diff / prev.abs() * 100.0).abs(), MetricFmt::Percent, l));
    let abs = (df != DeltaFormat::Pct || pct.is_none()).then(|| signed_abs(diff, fmt, l));
    let text = match (&pct, &abs) {
        (Some(p), Some(a)) => format!("{p} ({a})"),
        (Some(p), None) => p.clone(),
        (None, Some(a)) => a.clone(),
        (None, None) => return None,
    };
    Some(Delta {
        up,
        good: up != lower_good,
        arrow: if up { "▲" } else { "▼" },
        pct,
        abs,
        text,
    })
}

/// `diff` with an explicit sign (`+`, a true minus `−`) in the KPI's format; a
/// `::PERCENT` KPI's difference is in percentage points (`+2.3 pp`).
fn signed_abs(diff: f64, fmt: MetricFmt, l: &Locale) -> String {
    let a = diff.abs();
    let body = match fmt {
        // ≤ 2 decimals, so a small move is not "0 pp"
        MetricFmt::Percent => format!("{} pp", format_number_l(a, MetricFmt::Plain, l)),
        // cents for a small amount ("$0.12", not "$0"): the currency placed
        // as the locale places it around a 1
        MetricFmt::Money if a < 10.0 => format_number_l(1.0, MetricFmt::Money, l).replacen(
            '1',
            &format_number_l(a, MetricFmt::Plain, l),
            1,
        ),
        f => format_number_l(a, f, l),
    };
    // No sign on a change that rounds to zero.
    let zero = !body.chars().any(|c| c.is_ascii_digit() && c != '0');
    let sign = match () {
        _ if zero => "",
        _ if diff > 0.0 => "+",
        _ => "−",
    };
    format!("{sign}{body}")
}

/// Progress toward a KPI's goal.
#[derive(Clone, Debug, PartialEq)]
pub struct Goal {
    /// value / goal (not capped — the bar caps itself at 100%).
    pub frac: f64,
    /// The goal is met (`frac >= 1`).
    pub reached: bool,
    /// `82% of goal $1.2M`.
    pub text: String,
}

/// The progress of `cur` toward `goal`, or `None` for a missing or
/// non-positive goal.
pub fn goal(cur: f64, goal: f64, fmt: MetricFmt, l: &Locale) -> Option<Goal> {
    if !cur.is_finite() || !goal.is_finite() || goal <= 0.0 {
        return None;
    }
    let frac = cur / goal;
    Some(Goal {
        frac,
        reached: frac >= 1.0,
        text: format!(
            "{} of goal {}",
            format_number_l((frac * 100.0).round(), MetricFmt::Percent, l),
            format_number_l(goal, fmt, l)
        ),
    })
}

/// The numbers of a sparkline cell: a JSON list (`[1, 2.5, null]`, as DuckDB
/// prints `list()`), non-numbers skipped.
pub fn spark_values(v: &Value) -> Vec<f64> {
    match v {
        Value::Str(s) => serde_json::from_str::<Vec<serde_json::Value>>(s.trim())
            .map(|xs| {
                xs.iter()
                    .filter_map(|x| match x {
                        serde_json::Value::String(s) => s.trim().parse::<f64>().ok(),
                        x => x.as_f64(),
                    })
                    .filter(|f| f.is_finite())
                    .collect()
            })
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

/// A cell as a number (DuckDB's JSON writes DECIMAL / HUGEINT as strings).
fn num(v: Option<&Value>) -> Option<f64> {
    match v? {
        Value::Str(s) => s.trim().parse::<f64>().ok(),
        v => v.as_f64(),
    }
    .filter(|f| f.is_finite())
}

/// Everything a KPI tile shows, formatted.
#[derive(Clone, Debug, PartialEq)]
pub struct Kpi {
    /// The big value (formatted), or a text card's text.
    pub value: String,
    pub caption: String,
    pub delta: Option<Delta>,
    /// `::DELTA_LABEL` — only set when there is a delta.
    pub label: Option<String>,
    /// `::SPARKLINE` numbers (≥ 2, else empty).
    pub spark: Vec<f64>,
    pub goal: Option<Goal>,
    /// A `::TEXT_*` card (no number).
    pub text_card: bool,
}

impl Kpi {
    /// The KPI / text card in a panel's columns (by role), or `None` when the
    /// panel is not one. A table's `::MONEY` columns are formats, not a KPI.
    pub fn from_columns(cols: &[Column], l: &Locale) -> Option<Kpi> {
        if cols
            .iter()
            .any(|c| matches!(c.role, Role::Table | Role::PagedTable))
        {
            return None;
        }
        let first = |pred: &dyn Fn(Role) -> bool| {
            cols.iter()
                .find(|c| pred(c.role))
                .and_then(|c| c.values.first())
        };
        let text = |pred: &dyn Fn(Role) -> bool| {
            first(pred)
                .filter(|v| !matches!(v, Value::Na))
                .map(value_str)
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        };
        let caption = text(&|r| r == Role::Label).unwrap_or_default();
        let (main, fmt) = cols.iter().find_map(|c| match c.role {
            Role::Metric(f) => Some((c, Some(f))),
            Role::Text(_) => Some((c, None)),
            _ => None,
        })?;
        let Some(fmt) = fmt else {
            return Some(Kpi {
                value: main.values.first().map(value_str).unwrap_or_default(),
                caption,
                delta: None,
                label: None,
                spark: Vec::new(),
                goal: None,
                text_card: true,
            });
        };
        let cur = num(main.values.first());
        let df = text(&|r| r == Role::DeltaFormat)
            .and_then(|s| DeltaFormat::parse(&s))
            .unwrap_or_default();
        let lower_good = cols.iter().any(|c| c.role == Role::DeltaLowerGood);
        let prev = num(first(&|r| matches!(r, Role::Delta | Role::DeltaLowerGood)));
        let delta = cur
            .zip(prev)
            .and_then(|(c, p)| delta(c, p, fmt, df, lower_good, l));
        let label = delta
            .as_ref()
            .and_then(|_| text(&|r| r == Role::DeltaLabel));
        let spark = first(&|r| r == Role::Value(Kind::Sparkline))
            .map(spark_values)
            .filter(|v| v.len() >= 2)
            .unwrap_or_default();
        let goal = cur
            .zip(num(first(&|r| r == Role::RefLine)))
            .and_then(|(c, g)| goal(c, g, fmt, l));
        Some(Kpi {
            value: cur.map_or_else(|| "–".into(), |n| format_number_l(n, fmt, l)),
            caption,
            delta,
            label,
            spark,
            goal,
            text_card: false,
        })
    }

    /// The tile's natural height in px (its content plus padding) — the
    /// headless layouts size a KPI row to the tallest tile.
    pub fn height(&self, w: f64) -> f64 {
        PAD * 2.0
            + VALUE_H
            + if self.delta.is_some() { DELTA_H } else { 0.0 }
            + if self.label_wraps(w) { DELTA_H } else { 0.0 }
            + if self.caption.is_empty() { 0.0 } else { CAP_H }
            + if self.spark.is_empty() { 0.0 } else { SPARK_H }
            + if self.goal.is_some() { GOAL_H } else { 0.0 }
    }

    /// Does the `::DELTA_LABEL` go on its own line (the change and the label
    /// don't fit side by side in a `w`-wide tile)? Estimated at ~0.58 em per
    /// glyph of the 13 px text.
    fn label_wraps(&self, w: f64) -> bool {
        match (&self.delta, &self.label) {
            (Some(d), Some(l)) => {
                let glyphs = (d.text.chars().count() + 2 + l.chars().count()) as f64;
                glyphs * 7.5 + 6.0 > w - 2.0 * PAD
            }
            _ => false,
        }
    }

    /// The tile as SVG content (no `<svg>` wrapper) for a `w`×`h` box, the
    /// content block centred vertically: the value, the change line (arrow +
    /// change in the good/bad colour, then the label muted), the caption, the
    /// sparkline and the goal bar.
    pub fn body(&self, w: f64, h: f64, o: &RenderOptions) -> String {
        let t = &o.theme;
        let c = &t.colors;
        let (fd, fb) = (esc(&t.font.display), esc(&t.font.family));
        let inner = (w - 2.0 * PAD).max(20.0);
        // Long values shrink to fit the tile (~0.6em per glyph at weight 800).
        let chars = self.value.chars().count().max(1) as f64;
        let vsize = (inner / (chars * 0.62)).clamp(16.0, 34.0);
        let mut y = PAD + ((h - self.height(w)).max(0.0) / 2.0);
        let mut s = String::new();
        // the theme's KPI number colour (`chrome.kpi_value`; a gradient → its accent)
        let value_fill = match t.chrome.kpi_value.as_str() {
            _ if self.text_card => c.heading,
            "accent" | "gradient" => c.accent,
            _ => c.ink,
        };
        y += VALUE_H;
        s.push_str(&format!(
            "<text class=\"kpi-value\" x=\"{PAD}\" y=\"{:.1}\" font-family=\"{fd}\" font-size=\"{vsize:.0}\" font-weight=\"800\" fill=\"{}\">{}</text>",
            y - 8.0,
            hex(value_fill),
            esc(&self.value)
        ));
        if let Some(d) = &self.delta {
            y += DELTA_H;
            let col = if d.good { c.positive } else { c.negative };
            // the label beside the change, or on the next line when it
            // doesn't fit (cut with "…" when even that is too narrow)
            let wraps = self.label_wraps(w);
            let label = self
                .label
                .as_deref()
                .map(|lb| {
                    let pos = if wraps {
                        y += DELTA_H;
                        format!("x=\"{PAD}\" dy=\"{DELTA_H}\"")
                    } else {
                        "dx=\"6\"".into()
                    };
                    let max = ((inner / 6.4) as usize).max(4);
                    let lb = if lb.chars().count() > max {
                        format!("{}…", lb.chars().take(max - 1).collect::<String>())
                    } else {
                        lb.to_string()
                    };
                    format!(
                        "<tspan class=\"kpi-delta-label\" {pos} fill=\"{}\" font-weight=\"500\">{}</tspan>",
                        hex(c.muted),
                        esc(&lb)
                    )
                })
                .unwrap_or_default();
            let line_y = if wraps { y - DELTA_H } else { y };
            s.push_str(&format!(
                "<text class=\"kpi-delta {cls}\" x=\"{PAD}\" y=\"{:.1}\" font-family=\"{fb}\" font-size=\"13\" font-weight=\"700\"><tspan fill=\"{}\">{} {}</tspan>{label}</text>",
                line_y - 5.0,
                hex(col),
                d.arrow,
                esc(&d.text),
                cls = if d.good { "good" } else { "bad" },
            ));
        }
        if !self.caption.is_empty() {
            y += CAP_H;
            s.push_str(&format!(
                "<text class=\"kpi-caption\" x=\"{PAD}\" y=\"{:.1}\" font-family=\"{fb}\" font-size=\"12\" font-weight=\"600\" fill=\"{}\">{}</text>",
                y - 5.0,
                hex(c.muted),
                esc(&self.caption)
            ));
        }
        if self.spark.len() >= 2 {
            let (top, sh) = (y + 8.0, SPARK_H - 12.0);
            y += SPARK_H;
            let sw = inner.min(320.0);
            let (mn, mx) = self
                .spark
                .iter()
                .fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), &v| {
                    (a.min(v), b.max(v))
                });
            let rng = if mx > mn { mx - mn } else { 1.0 };
            let n = self.spark.len() as f64;
            let px = |i: usize| PAD + i as f64 * sw / (n - 1.0);
            let py = |v: f64| top + sh - (v - mn) / rng * sh;
            let pts: Vec<String> = self
                .spark
                .iter()
                .enumerate()
                .map(|(i, &v)| format!("{:.1},{:.1}", px(i), py(v)))
                .collect();
            let brand = hex(o.brand());
            let last = self.spark.len() - 1;
            s.push_str(&format!(
                "<g class=\"kpi-spark\"><polygon points=\"{:.1},{b:.1} {p} {:.1},{b:.1}\" fill=\"{brand}\" fill-opacity=\"0.12\"/>\
                 <polyline points=\"{p}\" fill=\"none\" stroke=\"{brand}\" stroke-width=\"1.6\" stroke-linejoin=\"round\" stroke-linecap=\"round\"/>\
                 <circle cx=\"{:.1}\" cy=\"{:.1}\" r=\"2.6\" fill=\"{brand}\"/></g>",
                px(0),
                px(last),
                px(last),
                py(self.spark[last]),
                p = pts.join(" "),
                b = top + sh,
            ));
        }
        if let Some(g) = &self.goal {
            let top = y + 6.0;
            let bw = inner.min(320.0);
            let fill = if g.reached { c.positive } else { c.accent };
            s.push_str(&format!(
                "<g class=\"kpi-goal\"><rect x=\"{PAD}\" y=\"{top:.1}\" width=\"{bw:.1}\" height=\"6\" rx=\"3\" fill=\"{}\"/>\
                 <rect x=\"{PAD}\" y=\"{top:.1}\" width=\"{:.1}\" height=\"6\" rx=\"3\" fill=\"{}\"/>\
                 <text x=\"{PAD}\" y=\"{:.1}\" font-family=\"{fb}\" font-size=\"11\" font-weight=\"600\" fill=\"{}\">{}</text></g>",
                hex(c.line),
                bw * g.frac.clamp(0.0, 1.0),
                hex(fill),
                top + 20.0,
                hex(c.muted),
                esc(&g.text)
            ));
        }
        s
    }
}

/// Tile padding and the heights of its blocks (px).
const PAD: f64 = 14.0;
const VALUE_H: f64 = 40.0;
const DELTA_H: f64 = 20.0;
const CAP_H: f64 = 18.0;
const SPARK_H: f64 = 42.0;
const GOAL_H: f64 = 30.0;

#[cfg(test)]
mod tests {
    use super::*;

    fn en() -> Locale {
        Locale::default()
    }

    #[test]
    fn delta_formats() {
        let l = en();
        let d = delta(
            1100.0,
            1000.0,
            MetricFmt::Money,
            DeltaFormat::Pct,
            false,
            &l,
        )
        .unwrap();
        assert_eq!((d.arrow, d.text.as_str(), d.good), ("▲", "10%", true));
        assert_eq!(d.abs, None);
        let d = delta(
            1100.0,
            1000.0,
            MetricFmt::Money,
            DeltaFormat::Abs,
            false,
            &l,
        )
        .unwrap();
        assert_eq!(d.text, "+$100");
        assert_eq!(d.pct, None);
        let d = delta(
            900.0,
            1000.0,
            MetricFmt::Money,
            DeltaFormat::Both,
            false,
            &l,
        )
        .unwrap();
        assert_eq!(
            (d.arrow, d.text.as_str(), d.good),
            ("▼", "10% (−$100)", false)
        );
        // compact and plain formats
        let d = delta(
            13_400.0,
            12_200.0,
            MetricFmt::Compact,
            DeltaFormat::Abs,
            false,
            &l,
        )
        .unwrap();
        assert_eq!(d.text, "+1.2K");
        let d = delta(3.5, 3.25, MetricFmt::Plain, DeltaFormat::Abs, false, &l).unwrap();
        assert_eq!(d.text, "+0.25");
        // a small money change keeps its cents
        let d = delta(59.12, 59.2, MetricFmt::Money, DeltaFormat::Abs, false, &l).unwrap();
        assert_eq!(d.text, "−$0.08");
        let de = Locale::from_tag("de-DE").unwrap();
        let d = delta(59.2, 59.0, MetricFmt::Money, DeltaFormat::Abs, false, &de).unwrap();
        assert_eq!(d.text, "+0,2 €");
        // money ≥ 1M is compact
        let d = delta(3.0e6, 1.5e6, MetricFmt::Money, DeltaFormat::Both, false, &l).unwrap();
        assert_eq!(d.text, "100% (+$1.5M)");
    }

    #[test]
    fn percent_kpi_changes_in_points() {
        let l = en();
        let d = delta(56.5, 54.2, MetricFmt::Percent, DeltaFormat::Both, false, &l).unwrap();
        assert_eq!(d.text, "4.2% (+2.3 pp)");
        let d = delta(54.2, 56.5, MetricFmt::Percent, DeltaFormat::Abs, false, &l).unwrap();
        assert_eq!(d.text, "−2.3 pp");
        let d = delta(56.5, 56.44, MetricFmt::Percent, DeltaFormat::Abs, false, &l).unwrap();
        assert_eq!(d.text, "+0.06 pp");
        // a locale with a space before % drops it too
        let de = Locale::from_tag("de-DE").unwrap();
        let d = delta(56.5, 54.2, MetricFmt::Percent, DeltaFormat::Abs, false, &de).unwrap();
        assert_eq!(d.text, "+2,3 pp");
    }

    #[test]
    fn lower_good_flips_the_colour_not_the_arrow() {
        let l = en();
        let d = delta(5.0, 4.0, MetricFmt::Percent, DeltaFormat::Pct, true, &l).unwrap();
        assert_eq!((d.arrow, d.up, d.good), ("▲", true, false));
        let d = delta(3.0, 4.0, MetricFmt::Percent, DeltaFormat::Pct, true, &l).unwrap();
        assert_eq!((d.arrow, d.up, d.good), ("▼", false, true));
    }

    #[test]
    fn missing_or_zero_comparison() {
        let l = en();
        // NULL comparison (::PERIOD compare=none) → no delta
        assert!(delta(
            5.0,
            f64::NAN,
            MetricFmt::Plain,
            DeltaFormat::Both,
            false,
            &l
        )
        .is_none());
        // prev = 0: no percentage, the absolute part only (even for 'pct')
        let d = delta(250.0, 0.0, MetricFmt::Money, DeltaFormat::Pct, false, &l).unwrap();
        assert_eq!((d.pct.as_deref(), d.text.as_str()), (None, "+$250"));
        let d = delta(250.0, 0.0, MetricFmt::Money, DeltaFormat::Both, false, &l).unwrap();
        assert_eq!(d.text, "+$250");
        // no change: no sign
        let d = delta(7.0, 7.0, MetricFmt::Plain, DeltaFormat::Both, false, &l).unwrap();
        assert_eq!(d.text, "0% (0)");
    }

    #[test]
    fn delta_format_parses() {
        assert_eq!(DeltaFormat::parse("BOTH"), Some(DeltaFormat::Both));
        assert_eq!(DeltaFormat::parse(" abs "), Some(DeltaFormat::Abs));
        assert_eq!(DeltaFormat::parse(""), Some(DeltaFormat::Pct));
        assert_eq!(DeltaFormat::parse("absolutely"), None);
    }

    #[test]
    fn goal_progress() {
        let l = en();
        let g = goal(984_000.0, 1_200_000.0, MetricFmt::Money, &l).unwrap();
        assert_eq!(g.text, "82% of goal $1.2M");
        assert!(!g.reached);
        let g = goal(130.0, 100.0, MetricFmt::Plain, &l).unwrap();
        assert!(g.reached && (g.frac - 1.3).abs() < 1e-9);
        assert_eq!(g.text, "130% of goal 100");
        assert!(goal(1.0, 0.0, MetricFmt::Plain, &l).is_none());
        assert!(goal(1.0, f64::NAN, MetricFmt::Plain, &l).is_none());
    }

    #[test]
    fn sparkline_lists() {
        assert_eq!(
            spark_values(&Value::Str("[1, 2.5, null, \"4\"]".into())),
            vec![1.0, 2.5, 4.0]
        );
        assert!(spark_values(&Value::Str("12".into())).is_empty());
        assert!(spark_values(&Value::Float(3.0)).is_empty());
    }

    #[test]
    fn kpi_from_columns() {
        let col = |role, v: Value| Column::new("c", role, vec![v]);
        let cols = vec![
            col(Role::Metric(MetricFmt::Money), Value::Float(984_000.0)),
            col(Role::Delta, Value::Float(800_000.0)),
            col(Role::DeltaFormat, Value::Str("both".into())),
            col(Role::DeltaLabel, Value::Str("vs Aug 2026".into())),
            col(Role::Value(Kind::Sparkline), Value::Str("[1,3,2,5]".into())),
            col(Role::RefLine, Value::Float(1_200_000.0)),
            col(Role::Label, Value::Str("Revenue".into())),
        ];
        let k = Kpi::from_columns(&cols, &en()).unwrap();
        assert_eq!(k.value, "$984,000");
        assert_eq!(k.delta.as_ref().unwrap().text, "23% (+$184,000)");
        assert_eq!(k.label.as_deref(), Some("vs Aug 2026"));
        assert_eq!(k.spark, vec![1.0, 3.0, 2.0, 5.0]);
        assert_eq!(k.goal.as_ref().unwrap().text, "82% of goal $1.2M");
        assert_eq!(k.caption, "Revenue");
        // a NULL comparison hides the delta and its label
        let mut c2 = cols.clone();
        c2[1] = col(Role::Delta, Value::Na);
        let k2 = Kpi::from_columns(&c2, &en()).unwrap();
        assert!(k2.delta.is_none() && k2.label.is_none());
        assert!(k2.height(300.0) < k.height(300.0));
        // a long label wraps in a narrow tile
        assert!(k.height(160.0) > k.height(600.0));
        // a table is never a KPI
        let mut t = cols;
        t.push(col(Role::Table, Value::Na));
        assert!(Kpi::from_columns(&t, &en()).is_none());
    }

    #[test]
    fn tile_svg_has_every_part_in_theme_colours() {
        let o = RenderOptions::default();
        let k = Kpi {
            value: "$984,000".into(),
            caption: "Revenue".into(),
            delta: delta(
                900.0,
                1000.0,
                MetricFmt::Money,
                DeltaFormat::Pct,
                true,
                &en(),
            ),
            label: Some("vs <prev>".into()),
            spark: vec![1.0, 2.0, 1.5],
            goal: goal(5.0, 4.0, MetricFmt::Plain, &en()),
            text_card: false,
        };
        let s = k.body(300.0, k.height(300.0), &o);
        let c = &o.theme.colors;
        assert!(s.contains("class=\"kpi-delta good\""), "{s}");
        assert!(
            s.contains(&format!("<tspan fill=\"{}\">▼ 10%", hex(c.positive))),
            "{s}"
        );
        assert!(s.contains("vs &lt;prev&gt;"), "{s}");
        assert!(s.contains("<polyline") && s.contains("<circle"), "{s}");
        assert!(s.contains("125% of goal 4"), "{s}");
        assert!(s.contains(&format!("fill=\"{}\"", hex(c.positive))));
    }
}
