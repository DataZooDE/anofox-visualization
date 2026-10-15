//! Shared text helpers: the one XML/HTML escaper every SVG/HTML writer in the
//! crate uses, and the KPI / table number formatter shared with the browser
//! (exported to wasm as `format_number`, so `web/app.js` and the headless
//! renderer print identical numbers).

use crate::MetricFmt;

/// Number/date conventions of a dashboard locale (`::LOCALE 'de-DE'` or the
/// theme's `locale` token). A small built-in table — no ICU dependency. The
/// browser delegates to the same table (wasm `format_number_locale`), so a
/// KPI, a table cell and an axis label read the same in the browser and in
/// the CLI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Locale {
    /// The BCP-47 tag (`de-DE`).
    pub tag: &'static str,
    /// Decimal mark.
    pub decimal: char,
    /// Thousands separator (may be a narrow no-break space).
    pub group: &'static str,
    /// Currency symbol for `::MONEY`.
    pub currency: &'static str,
    /// `1.234 €` (true) vs `€1,234` (false).
    pub currency_suffix: bool,
    /// `12,5 %` (true) vs `12.5%` (false).
    pub percent_space: bool,
    /// Compact suffixes for 1e3, 1e6, 1e9, 1e12 (`""` = no compact unit at
    /// that magnitude, the number is printed grouped instead).
    pub compact: [&'static str; 4],
    /// Short month names.
    pub months: [&'static str; 12],
    /// Date pattern: `Y`, `M`, `D` placeholders (`D.M.Y`, `M/D/Y`, …).
    pub date: &'static str,
}

/// The space between amount and unit (`1.234 €`, `12,5 %`, `1,2 Mio.`) —
/// a plain space, as ggplot-rs's locale presets use on axes.
const UNIT_SPACE: &str = " ";

impl Locale {
    /// US English — the default (unchanged output of earlier releases).
    pub const EN_US: Locale = Locale {
        tag: "en-US",
        decimal: '.',
        group: ",",
        currency: "$",
        currency_suffix: false,
        percent_space: false,
        compact: ["K", "M", "B", "T"],
        months: [
            "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
        ],
        date: "M/D/Y",
    };
    /// British English (`£`, day-first dates).
    pub const EN_GB: Locale = Locale {
        tag: "en-GB",
        currency: "£",
        date: "D/M/Y",
        ..Locale::EN_US
    };
    /// German (Germany): `1.234.567,5`, `1.234 €`, `12,5 %`, `1,2 Mio.`.
    pub const DE_DE: Locale = Locale {
        tag: "de-DE",
        decimal: ',',
        group: ".",
        currency: "€",
        currency_suffix: true,
        percent_space: true,
        compact: ["", "Mio.", "Mrd.", "Bio."],
        months: [
            "Jan.", "Feb.", "März", "Apr.", "Mai", "Juni", "Juli", "Aug.", "Sept.", "Okt.", "Nov.",
            "Dez.",
        ],
        date: "D.M.Y",
    };
    /// French (France): `1 234 567,5`, `1 234 €`, `12,5 %`.
    pub const FR_FR: Locale = Locale {
        tag: "fr-FR",
        decimal: ',',
        group: " ",
        currency: "€",
        currency_suffix: true,
        percent_space: true,
        compact: ["k", "M", "Md", "Bn"],
        months: [
            "janv.", "févr.", "mars", "avr.", "mai", "juin", "juil.", "août", "sept.", "oct.",
            "nov.", "déc.",
        ],
        date: "D/M/Y",
    };

    /// Every supported tag.
    pub const TAGS: &'static [&'static str] = &["en-US", "en-GB", "de-DE", "fr-FR"];

    /// Look up a BCP-47 tag (case-insensitive; `_` accepted). A language-only
    /// or other-region tag falls back to its language (`de`, `de-AT` → de-DE,
    /// `fr-CA` → fr-FR, `en` → en-US). `None` for an unknown language.
    pub fn from_tag(tag: &str) -> Option<Locale> {
        let t = tag.trim().replace('_', "-").to_ascii_lowercase();
        let exact = match t.as_str() {
            "en-us" => Some(Locale::EN_US),
            "en-gb" => Some(Locale::EN_GB),
            "de-de" => Some(Locale::DE_DE),
            "fr-fr" => Some(Locale::FR_FR),
            _ => None,
        };
        exact.or_else(|| match t.split('-').next().unwrap_or("") {
            "en" => Some(Locale::EN_US),
            "de" => Some(Locale::DE_DE),
            "fr" => Some(Locale::FR_FR),
            _ => None,
        })
    }

    /// The matching ggplot-rs locale (chart axes, legends, date labels).
    pub fn gg(&self) -> ggplot_rs::locale::Locale {
        ggplot_rs::locale::Locale::from_tag(self.tag)
    }

    /// Is this the default (en-US) locale?
    pub fn is_default(&self) -> bool {
        *self == Locale::EN_US
    }

    /// Re-punctuate an en-US formatted number (`1,234.5`) for this locale.
    /// Non-numeric text passes through unchanged.
    pub fn localize(&self, en: &str) -> String {
        if self.is_default() {
            return en.to_string();
        }
        let mut out = String::with_capacity(en.len() + 4);
        let chars: Vec<char> = en.chars().collect();
        for (i, &c) in chars.iter().enumerate() {
            let digit_before = i > 0 && chars[i - 1].is_ascii_digit();
            let digit_after = chars.get(i + 1).is_some_and(|d| d.is_ascii_digit());
            match c {
                ',' if digit_before && digit_after => out.push_str(self.group),
                '.' if digit_after => out.push(self.decimal),
                c => out.push(c),
            }
        }
        out
    }

    /// Format an ISO date (`2026-10-15`, optionally with a time part) per
    /// the locale's date pattern. Anything else passes through unchanged.
    pub fn format_date(&self, iso: &str) -> String {
        let s = iso.trim();
        let b = s.as_bytes();
        let iso_like = b.len() >= 10
            && b[4] == b'-'
            && b[7] == b'-'
            && b[..4].iter().all(u8::is_ascii_digit)
            && b[5..7].iter().all(u8::is_ascii_digit)
            && b[8..10].iter().all(u8::is_ascii_digit)
            && (b.len() == 10 || b[10] == b' ' || b[10] == b'T');
        if !iso_like {
            return iso.to_string();
        }
        let (y, m, d) = (&s[0..4], &s[5..7], &s[8..10]);
        let date = self.date.replace('Y', y).replace('M', m).replace('D', d);
        let rest = &s[10..];
        let time = rest.trim_start_matches(['T', ' ']);
        if time.is_empty() || time.trim_start_matches("00:00:00").is_empty() {
            date
        } else {
            format!("{date} {}", &time[..time.len().min(8)])
        }
    }
}

impl Default for Locale {
    fn default() -> Self {
        Locale::EN_US
    }
}

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
    compact_l(n, &Locale::EN_US)
}

/// [`compact`] with the locale's units (en-US punctuation; the caller
/// re-punctuates). A locale without a thousands unit (de-DE) prints values
/// below a million grouped.
fn compact_l(n: f64, l: &Locale) -> String {
    let mags = [1e3, 1e6, 1e9, 1e12];
    let round1 = |v: f64| (v * 10.0).round() / 10.0;
    let a = n.abs();
    let first = l.compact.iter().position(|u| !u.is_empty()).unwrap_or(0);
    if round1(a) < mags[first] {
        return grouped(n, if first == 0 { 1 } else { 0 });
    }
    let sep = if l.is_default() { "" } else { UNIT_SPACE };
    let units: Vec<(f64, String)> = mags
        .iter()
        .zip(l.compact.iter())
        .filter(|(_, u)| !u.is_empty())
        .map(|(m, u)| (*m, format!("{sep}{u}")))
        .collect();
    #[allow(non_snake_case)]
    let UNITS: Vec<(f64, &str)> = units.iter().map(|(m, u)| (*m, u.as_str())).collect();
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

/// [`format_number`] for a dashboard locale (`::LOCALE`). en-US is exactly
/// [`format_number`].
///
/// | fmt | en-US | de-DE | fr-FR |
/// |-----|-------|-------|-------|
/// | `Plain` | `1,234,567.5` | `1.234.567,5` | `1 234 567,5` |
/// | `Money` | `$12,400` | `12.400 €` | `12 400 €` |
/// | `Percent` | `12.5%` | `12,5 %` | `12,5 %` |
/// | `Compact` | `1.2M` | `1,2 Mio.` | `1,2 M` |
pub fn format_number_l(n: f64, fmt: MetricFmt, l: &Locale) -> String {
    if l.is_default() {
        return format_number(n, fmt);
    }
    if !n.is_finite() {
        return "–".into();
    }
    match fmt {
        MetricFmt::Plain => l.localize(&grouped(n, 2)),
        MetricFmt::Percent => {
            let sp = if l.percent_space { UNIT_SPACE } else { "" };
            format!("{}{sp}%", l.localize(&grouped(n, 1)))
        }
        MetricFmt::Compact => l.localize(&compact_l(n, l)),
        MetricFmt::Money => {
            let body = if n.abs() >= 1e6 {
                compact_l(n.abs(), l)
            } else {
                grouped(n.abs(), 0)
            };
            let neg = n < 0.0 && body.chars().any(|c| c.is_ascii_digit() && c != '0');
            let body = l.localize(&body);
            let sign = if neg { "-" } else { "" };
            if l.currency_suffix {
                format!("{sign}{body}{UNIT_SPACE}{}", l.currency)
            } else {
                format!("{sign}{}{body}", l.currency)
            }
        }
    }
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
