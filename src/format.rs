//! Shared text helpers: the one XML/HTML escaper every SVG/HTML writer in the
//! crate uses, and the KPI / table number formatter shared with the browser
//! (exported to wasm as `format_number`, so `web/app.js` and the headless
//! renderer print identical numbers).

use crate::MetricFmt;

/// Escape text for HTML/XML **text and attribute** contexts: `& < > " '`.
/// Use this for every user-controlled string written into SVG/HTML.
pub fn escape_xml(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

/// Round half away from zero to `digits` decimals (matches `Intl.NumberFormat`
/// rounding for the values dashboards show), then print without trailing zeros
/// and with `,` thousands grouping.
fn grouped(n: f64, digits: u32) -> String {
    let p = 10f64.powi(digits as i32);
    let r = (n.abs() * p).round() / p;
    let neg = n < 0.0 && r != 0.0;
    let mut s = format!("{r:.*}", digits as usize);
    if s.contains('.') {
        while s.ends_with('0') {
            s.pop();
        }
        if s.ends_with('.') {
            s.pop();
        }
    }
    let (int, frac) = match s.find('.') {
        Some(i) => (s[..i].to_string(), s[i..].to_string()),
        None => (s.clone(), String::new()),
    };
    let mut g = String::with_capacity(int.len() + int.len() / 3);
    for (i, ch) in int.chars().enumerate() {
        if i > 0 && (int.len() - i) % 3 == 0 {
            g.push(',');
        }
        g.push(ch);
    }
    format!("{}{g}{frac}", if neg { "-" } else { "" })
}

/// Compact notation (`1.2K`, `53.8M`, `1.5B`, `2T`) with ≤ 1 decimal, like
/// `Intl.NumberFormat("en-US", {notation: "compact", maximumFractionDigits: 1})`.
fn compact(n: f64) -> String {
    const UNITS: [(f64, &str); 4] = [(1e3, "K"), (1e6, "M"), (1e9, "B"), (1e12, "T")];
    let round1 = |v: f64| (v * 10.0).round() / 10.0;
    let a = n.abs();
    if round1(a) < 1e3 {
        return grouped(n, 1);
    }
    let sign = if n < 0.0 { "-" } else { "" };
    let mut k = UNITS
        .iter()
        .rposition(|(s, _)| round1(a) >= *s)
        .unwrap_or(0);
    let mut scaled = round1(a / UNITS[k].0);
    // Re-pick the unit after rounding, so 999_950 → "1M" (not "1000K").
    if scaled >= 1e3 && k + 1 < UNITS.len() {
        k += 1;
        scaled = round1(a / UNITS[k].0);
    }
    format!("{sign}{}{}", grouped(scaled, 1), UNITS[k].1)
}

/// Format a KPI / table number the way the dashboard shows it (en-US, pinned so
/// every viewer sees the same text). Non-finite → `–`.
///
/// | fmt | rule | examples |
/// |-----|------|----------|
/// | `Plain` (`::METRIC`) | grouped, ≤ 2 decimals | `1,234.57`, `3` |
/// | `Money` | `$`, grouped, 0 decimals; compact ≥ 1e6 | `$12,400`, `$53.8M`, `-$1,200` |
/// | `Percent` | grouped, ≤ 1 decimal, `%` (value is already ×100) | `46%`, `12.3%` |
/// | `Compact` | K/M/B/T, ≤ 1 decimal | `1.2K`, `12K`, `1.5M` |
pub fn format_number(n: f64, fmt: MetricFmt) -> String {
    if !n.is_finite() {
        return "–".into();
    }
    match fmt {
        MetricFmt::Plain => grouped(n, 2),
        MetricFmt::Percent => format!("{}%", grouped(n, 1)),
        MetricFmt::Compact => compact(n),
        MetricFmt::Money => {
            let body = if n.abs() >= 1e6 {
                compact(n.abs())
            } else {
                grouped(n.abs(), 0)
            };
            let neg = n < 0.0 && body.chars().any(|c| c.is_ascii_digit() && c != '0');
            format!("{}${body}", if neg { "-" } else { "" })
        }
    }
}

/// Parse a format token (`METRIC`, `MONEY`, `PERCENT`, `COMPACT`, or any of
/// their aliases) — used by the wasm `format_number` export.
pub fn metric_fmt_of(token: &str) -> MetricFmt {
    match crate::parse_role(token) {
        Some(crate::Role::Metric(f)) => f,
        _ => MetricFmt::Plain,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Shared test vectors — `web/app.js` delegates `fmtNum` to the wasm
    /// export of [`format_number`], so these pin the browser output too.
    pub const VECTORS: &[(f64, MetricFmt, &str)] = &[
        (12400.0, MetricFmt::Money, "$12,400"),
        (-1200.4, MetricFmt::Money, "-$1,200"),
        (53_800_000.0, MetricFmt::Money, "$53.8M"),
        (2_500_000_000.0, MetricFmt::Money, "$2.5B"),
        (0.0, MetricFmt::Money, "$0"),
        (46.0, MetricFmt::Percent, "46%"),
        (12.345, MetricFmt::Percent, "12.3%"),
        (1234.56, MetricFmt::Percent, "1,234.6%"),
        (999.0, MetricFmt::Compact, "999"),
        (1234.0, MetricFmt::Compact, "1.2K"),
        (12000.0, MetricFmt::Compact, "12K"),
        (999_950.0, MetricFmt::Compact, "1M"),
        (1_500_000.0, MetricFmt::Compact, "1.5M"),
        (-2_000_000_000.0, MetricFmt::Compact, "-2B"),
        (3.0, MetricFmt::Plain, "3"),
        (1234.567, MetricFmt::Plain, "1,234.57"),
        (2.5, MetricFmt::Plain, "2.5"),
        (-1_000_000.0, MetricFmt::Plain, "-1,000,000"),
        (0.125, MetricFmt::Plain, "0.13"),
        (f64::NAN, MetricFmt::Plain, "–"),
        (f64::INFINITY, MetricFmt::Money, "–"),
    ];

    #[test]
    fn number_vectors() {
        for (n, f, want) in VECTORS {
            assert_eq!(format_number(*n, *f), *want, "{n} {f:?}");
        }
    }

    #[test]
    fn escape_is_attribute_safe() {
        assert_eq!(
            escape_xml(r#"<a href="x" onclick='y'>&</a>"#),
            "&lt;a href=&quot;x&quot; onclick=&#39;y&#39;&gt;&amp;&lt;/a&gt;"
        );
    }
}
