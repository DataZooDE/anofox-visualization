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
    /// Irish English — English text with the euro (`€12,400`, `12.5%`,
    /// day-first dates): an English dashboard of a euro-area business (#276).
    pub const EN_IE: Locale = Locale {
        tag: "en-IE",
        currency: "€",
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
    pub const TAGS: &'static [&'static str] = &["en-US", "en-GB", "en-IE", "de-DE", "fr-FR"];

    /// Look up a BCP-47 tag (case-insensitive; `_` accepted). A language-only
    /// or other-region tag falls back to its language (`de`, `de-AT` → de-DE,
    /// `fr-CA` → fr-FR, `en` → en-US). English with a euro-area region
    /// (`en-DE`, `en-NL`, `en-EU` → en-IE (English with the euro)). `None`
    /// for an unknown language.
    pub fn from_tag(tag: &str) -> Option<Locale> {
        let t = tag.trim().replace('_', "-").to_ascii_lowercase();
        let exact = match t.as_str() {
            "en-us" => Some(Locale::EN_US),
            "en-gb" => Some(Locale::EN_GB),
            "en-ie" => Some(Locale::EN_IE),
            "de-de" => Some(Locale::DE_DE),
            "fr-fr" => Some(Locale::FR_FR),
            _ => None,
        };
        exact.or_else(|| {
            // English with a euro-area region (or `en-EU`) is English with the euro.
            const EURO: &[&str] = &[
                "at", "be", "bg", "cy", "de", "ee", "es", "eu", "fi", "fr", "gr", "hr", "ie", "it",
                "lt", "lu", "lv", "mt", "nl", "pt", "si", "sk",
            ];
            let mut parts = t.split('-');
            if parts.next() == Some("en") {
                if let Some(r) = parts.find(|p| p.len() != 4) {
                    if EURO.contains(&r) {
                        return Some(Locale::EN_IE);
                    }
                }
            }
            match t.split('-').next().unwrap_or("") {
                "en" => Some(Locale::EN_US),
                "de" => Some(Locale::DE_DE),
                "fr" => Some(Locale::FR_FR),
                _ => None,
            }
        })
    }

    /// The matching ggplot-rs locale (chart axes, legends, date labels).
    pub fn gg(&self) -> ggplot_rs::locale::Locale {
        if self.tag == "en-IE" {
            let mut g = ggplot_rs::locale::Locale::en_gb().with_currency(self.currency);
            g.tag = "en-IE".into();
            return g;
        }
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

/// Whether `c` is a valid XML 1.0 character. XML forbids every C0 control
/// character except tab/`\n`/`\r`, and (within the BMP) the two permanently
/// non-characters `U+FFFE`/`U+FFFF`; a lone surrogate can't be represented
/// in a Rust `char` at all, since `str`/`char` are always valid Unicode
/// scalar values. A raw invalid char written into SVG/XML text makes the
/// whole document fail to parse, so every escaper drops/replaces it.
fn is_xml_valid_char(c: char) -> bool {
    matches!(c,
        '\t' | '\n' | '\r'
        | '\u{20}'..='\u{D7FF}'
        | '\u{E000}'..='\u{FFFD}'
        | '\u{10000}'..='\u{10FFFF}'
    )
}

/// Escape text for HTML/XML **text and attribute** contexts: `& < > " '`,
/// and replace any character XML 1.0 disallows (stray control characters
/// below `0x20`, `U+FFFE`, `U+FFFF`) with `U+FFFD` so the result is always
/// well-formed XML. Use this for every user-controlled string written into
/// SVG/HTML — it is the one shared escaper every writer in the crate calls
/// (directly, or via [`crate::host::serving::html_escape`]).
pub fn escape_xml(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c if !is_xml_valid_char(c) => out.push('\u{FFFD}'),
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
    // English-style locales (en-US, en-GB, en-IE: `.` decimal) write the
    // compact unit right against the number (`53.8M`); continental locales
    // (`,` decimal) put a space before it (`53,8 Mio.`, `1,2 M`).
    let sep = if l.decimal == '.' { "" } else { UNIT_SPACE };
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

/// How a chart value reads: the panel's `::YFORMAT` spec (the same keywords
/// and prefix/suffix rule as the axis labels), plus the share kinds whose
/// values are fractions.
#[derive(Clone, Debug, PartialEq)]
pub enum ValueFmt {
    /// Grouped number.
    Plain,
    /// A currency symbol (placed per locale: `$1,200.50`, `1.200,50 €`).
    Currency(String),
    /// Already in percent units (`12.5` → `12.5%`).
    Percent,
    /// A fraction shown as percent (`0.125` → `12.5%`).
    Share,
    /// A literal prefix (`CHF 1,200`).
    Prefix(String),
    /// A literal suffix (`12 kg`).
    Suffix(String),
}

impl ValueFmt {
    /// Parse a `::YFORMAT` spec (`'$'`, `'€'`, `'percent'`, `'comma'`,
    /// `' kg'`, `'CHF '`, …); empty → plain.
    pub fn parse(spec: &str) -> ValueFmt {
        let s = spec.trim_end_matches(';');
        let low = s.trim().to_ascii_lowercase();
        match low.as_str() {
            "" | "," | "comma" | "thousands" | "number" => ValueFmt::Plain,
            "$" | "usd" | "dollar" | "dollars" => ValueFmt::Currency("$".into()),
            "€" | "eur" | "euro" | "euros" => ValueFmt::Currency("€".into()),
            "£" | "gbp" | "pound" | "pounds" => ValueFmt::Currency("£".into()),
            "¥" | "jpy" | "yen" | "cny" | "yuan" => ValueFmt::Currency("¥".into()),
            "%" | "percent" | "pct" => ValueFmt::Percent,
            _ => match s.strip_prefix(' ') {
                Some(suffix) => ValueFmt::Suffix(suffix.trim().to_string()),
                None => ValueFmt::Prefix(s.to_string()),
            },
        }
    }

    /// The full value, as a tooltip shows it: grouped, ≤ 2 decimals (a
    /// currency amount with cents keeps both digits: `$1,234.50`).
    pub fn full(&self, n: f64, l: &Locale) -> String {
        self.write(n, l, false)
    }

    /// The short value, as a label on a mark shows it: compact from 10 000
    /// on (`12K`, `1.2 Mio.`), fewer decimals as the number grows.
    pub fn short(&self, n: f64, l: &Locale) -> String {
        self.write(n, l, true)
    }

    fn write(&self, n: f64, l: &Locale, short: bool) -> String {
        if !n.is_finite() {
            return "–".into();
        }
        let v = if *self == ValueFmt::Share {
            n * 100.0
        } else {
            n
        };
        let a = v.abs();
        let body = if short && a >= 1e4 {
            compact_l(a, l)
        } else if short {
            grouped(
                a,
                if a >= 100.0 {
                    0
                } else if a >= 10.0 {
                    1
                } else {
                    2
                },
            )
        } else if matches!(self, ValueFmt::Currency(_)) {
            // An amount with cents keeps both digits: $12.50, not $12.5.
            let g = grouped(a, 2);
            match g.find('.') {
                Some(i) if g.len() - i == 2 => format!("{g}0"),
                _ => g,
            }
        } else {
            grouped(a, 2)
        };
        let neg = v < 0.0 && body.chars().any(|c| c.is_ascii_digit() && c != '0');
        let sign = if neg { "-" } else { "" };
        let body = l.localize(&body);
        let pct = if l.percent_space { " %" } else { "%" };
        match self {
            ValueFmt::Plain => format!("{sign}{body}"),
            ValueFmt::Percent | ValueFmt::Share => format!("{sign}{body}{pct}"),
            ValueFmt::Currency(sym) if l.currency_suffix => {
                format!("{sign}{body}{UNIT_SPACE}{sym}")
            }
            ValueFmt::Currency(sym) | ValueFmt::Prefix(sym) => format!("{sign}{sym}{body}"),
            ValueFmt::Suffix(unit) => format!("{sign}{body} {unit}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn value_formats() {
        let us = Locale::EN_US;
        let de = Locale::DE_DE;
        let f = |spec: &str, n: f64| ValueFmt::parse(spec).full(n, &us);
        assert_eq!(f("$", 1234.5), "$1,234.50");
        assert_eq!(f("$", 12.5), "$12.50");
        assert_eq!(f("usd", -12.0), "-$12");
        assert_eq!(f("€", 1_200_000.0), "€1,200,000");
        assert_eq!(f("percent", 12.345), "12.35%");
        assert_eq!(f(" kg", 1200.0), "1,200 kg");
        assert_eq!(f("CHF ", 3.0), "CHF 3");
        assert_eq!(f("", 121.94), "121.94");
        assert_eq!(ValueFmt::Share.full(0.125, &us), "12.5%");
        assert_eq!(ValueFmt::parse("€").full(1234.5, &de), "1.234,50 €");
        assert_eq!(ValueFmt::parse("€").full(12.5, &de), "12,50 €");
        assert_eq!(ValueFmt::Percent.full(12.5, &de), "12,5 %");
        let s = |spec: &str, n: f64| ValueFmt::parse(spec).short(n, &us);
        assert_eq!(s("$", 1_234_567.0), "$1.2M");
        assert_eq!(s("", 121.94), "122");
        assert_eq!(s("", 12.34), "12.3");
        assert_eq!(s("", 0.1234), "0.12");
        assert_eq!(s("", 9999.0), "9,999");
        assert_eq!(ValueFmt::Plain.short(15_000.0, &de), "15.000");
        assert_eq!(f("$", f64::NAN), "–");
    }

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

    #[test]
    fn escape_xml_replaces_xml_invalid_control_characters() {
        // \t \n \r stay as-is: they're valid XML 1.0 characters.
        assert_eq!(escape_xml("a\tb\nc\rd"), "a\tb\nc\rd");
        // Every other C0 control character is XML-invalid and would make
        // the document fail to parse — replace it with U+FFFD.
        assert_eq!(
            escape_xml("a\u{0}b\u{1}c\u{1F}d\u{7F}"),
            "a\u{FFFD}b\u{FFFD}c\u{FFFD}d\u{7F}"
        );
        // U+FFFE / U+FFFF are permanent BMP non-characters, also invalid.
        assert_eq!(escape_xml("x\u{FFFE}y\u{FFFF}z"), "x\u{FFFD}y\u{FFFD}z");
        // Ordinary text (including astral chars) is untouched.
        assert_eq!(escape_xml("caf\u{e9} \u{1F600}"), "caf\u{e9} \u{1F600}");
    }

    #[test]
    fn english_with_the_euro() {
        for tag in ["en-IE", "en-de", "en_NL", "en-EU"] {
            assert_eq!(Locale::from_tag(tag), Some(Locale::EN_IE), "{tag}");
        }
        assert_eq!(Locale::from_tag("en-CH"), Some(Locale::EN_US));
        assert_eq!(Locale::from_tag("en"), Some(Locale::EN_US));
        assert_eq!(Locale::from_tag("en-GB"), Some(Locale::EN_GB));
        assert_eq!(Locale::EN_IE.gg().currency_symbol, "€");
        assert_eq!(Locale::EN_IE.gg().tag, "en-IE");
    }
}
