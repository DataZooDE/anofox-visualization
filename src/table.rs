//! Table formatting decisions shared by every output: the browser's table
//! (`web/app.js` `renderTable`, through the wasm `table_profile` /
//! `human_header` exports) and the headless SVG table (`dashboard --svg`, the
//! CLI's static HTML). One place decides how a column reads — its header, kind,
//! decimals, unit and default total — so a table looks the same everywhere
//! (the same split as `page_sql`: one builder, every output). Shared vectors:
//! `tests/fixtures/table_vectors.json` (Rust: `tests/table.rs`, browser:
//! `tests/web/table_test.mjs`).

use crate::format::{format_number_l, Locale};
use crate::MetricFmt;

/// Abbreviations a readable header spells out (or keeps in capitals).
const HDR_WORDS: &[(&str, &str)] = &[
    ("pct", "%"),
    ("percent", "%"),
    ("perc", "%"),
    ("avg", "Avg"),
    ("qty", "Qty"),
    ("num", "No."),
    ("nr", "No."),
    ("amt", "Amount"),
    ("id", "ID"),
    ("sku", "SKU"),
    ("kpi", "KPI"),
    ("url", "URL"),
    ("vat", "VAT"),
    ("eu", "EU"),
    ("us", "US"),
    ("uk", "UK"),
    ("mae", "MAE"),
    ("rmse", "RMSE"),
    ("mape", "MAPE"),
    ("smape", "sMAPE"),
    ("mse", "MSE"),
    ("r2", "R²"),
    ("ci", "CI"),
    ("sd", "SD"),
    ("se", "SE"),
    ("yoy", "YoY"),
    ("mom", "MoM"),
    ("ytd", "YTD"),
    ("eur", "EUR"),
    ("usd", "USD"),
    ("api", "API"),
    ("ui", "UI"),
];

fn hdr_word(w: &str) -> Option<&'static str> {
    HDR_WORDS.iter().find(|(k, _)| *k == w).map(|(_, v)| *v)
}

/// A readable header for a raw column name as SQL emits it (`load_pct` →
/// "Load %", `avgMae` → "Avg MAE"). A name that already looks written for
/// people (capitals, spaces, symbols — an `AS "Header"` alias) is kept.
pub fn human_header(raw: &str) -> String {
    let snake = !raw.is_empty()
        && raw
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    if !snake && !is_camel(raw) {
        return raw.to_string();
    }
    // camelCase → camel_Case, then words
    let mut split = String::with_capacity(raw.len() + 4);
    let mut prev: Option<char> = None;
    for c in raw.chars() {
        if c.is_ascii_uppercase()
            && prev.is_some_and(|p| p.is_ascii_lowercase() || p.is_ascii_digit())
        {
            split.push('_');
        }
        split.push(c);
        prev = Some(c);
    }
    let words: Vec<String> = split
        .split('_')
        .filter(|w| !w.is_empty())
        .map(|w| w.to_ascii_lowercase())
        .collect();
    if words.is_empty() {
        return raw.to_string();
    }
    let mut out: Vec<String> = words
        .iter()
        .map(|w| hdr_word(w).map_or_else(|| w.clone(), str::to_string))
        .collect();
    if hdr_word(&words[0]).is_none() {
        let mut cs = out[0].chars();
        out[0] = match cs.next() {
            Some(f) => f.to_uppercase().collect::<String>() + cs.as_str(),
            None => String::new(),
        };
    }
    out.join(" ")
}

/// `^[a-z]+(?:[A-Z][a-z0-9]*)+$`
fn is_camel(s: &str) -> bool {
    let b = s.as_bytes();
    let lead = b.iter().take_while(|c| c.is_ascii_lowercase()).count();
    if lead == 0 || lead == b.len() {
        return false;
    }
    let mut i = lead;
    while i < b.len() {
        if !b[i].is_ascii_uppercase() {
            return false;
        }
        i += 1;
        while i < b.len() && (b[i].is_ascii_lowercase() || b[i].is_ascii_digit()) {
            i += 1;
        }
    }
    true
}

/// Decimals a column needs so its numbers line up: the most any value uses
/// (after rounding away float noise), at most 3 — or 4 when every value is
/// below 1 in magnitude (rates like 0.0405).
pub fn col_decimals(nums: &[f64]) -> u32 {
    let mut d = 0;
    for n in nums {
        let r = (n.abs() * 1e4).round() / 1e4;
        let s = format!("{r}");
        if let Some(i) = s.find('.') {
            d = d.max((s.len() - i - 1) as u32);
        }
    }
    let small = !nums.is_empty() && nums.iter().all(|n| n.abs() < 1.0);
    d.min(if small { 4 } else { 3 })
}

/// A table cell's value as the profile reads it: a number, a boolean, text,
/// or empty.
#[derive(Clone, Debug, PartialEq)]
pub enum Cell {
    Null,
    Num(f64),
    Bool(bool),
    Text(String),
}

impl Cell {
    /// From JSON as the browser holds a row (numbers, strings — possibly
    /// quoted — booleans, null).
    pub fn from_json(v: &serde_json::Value) -> Cell {
        match v {
            serde_json::Value::Null => Cell::Null,
            serde_json::Value::Bool(b) => Cell::Bool(*b),
            serde_json::Value::Number(n) => n.as_f64().map_or(Cell::Null, Cell::Num),
            serde_json::Value::String(s) => Cell::Text(unquote(s).to_string()),
            other => Cell::Text(other.to_string()),
        }
    }
    fn present(&self) -> bool {
        match self {
            Cell::Null => false,
            Cell::Text(s) => !s.is_empty(),
            _ => true,
        }
    }
    /// Reads as a number: a finite number, or text like `-12.5` / `1e3`.
    fn num_like(&self) -> bool {
        match self {
            Cell::Num(n) => n.is_finite(),
            Cell::Text(s) => is_num_text(s),
            _ => false,
        }
    }
    fn bool_like(&self) -> bool {
        match self {
            Cell::Bool(_) => true,
            Cell::Text(s) => s == "true" || s == "false",
            _ => false,
        }
    }
    /// The numeric value (`parseFloat` of text), if any.
    pub fn num(&self) -> Option<f64> {
        match self {
            Cell::Num(n) => Some(*n),
            Cell::Text(s) => parse_float(s),
            _ => None,
        }
    }
}

fn unquote(s: &str) -> &str {
    let s = s.strip_prefix('"').unwrap_or(s);
    s.strip_suffix('"').unwrap_or(s)
}

/// `^-?\d+(\.\d+)?([eE][+-]?\d+)?$`
fn is_num_text(s: &str) -> bool {
    let b = s.as_bytes();
    let mut i = usize::from(b.first() == Some(&b'-'));
    let digits = |i: &mut usize| {
        let st = *i;
        while *i < b.len() && b[*i].is_ascii_digit() {
            *i += 1;
        }
        *i > st
    };
    if !digits(&mut i) {
        return false;
    }
    if i < b.len() && b[i] == b'.' {
        i += 1;
        if !digits(&mut i) {
            return false;
        }
    }
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        i += 1;
        if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
            i += 1;
        }
        if !digits(&mut i) {
            return false;
        }
    }
    i == b.len()
}

/// JavaScript's `parseFloat`: the longest numeric prefix (after whitespace).
fn parse_float(s: &str) -> Option<f64> {
    let t = s.trim_start();
    let b = t.as_bytes();
    let mut end = 0;
    let mut best = None;
    while end < b.len() {
        end += 1;
        // a multi-byte character ('—', '€') is never part of a number
        let Some(prefix) = t.get(..end) else { continue };
        if let Ok(v) = prefix.parse::<f64>() {
            if prefix.bytes().any(|c| c.is_ascii_digit()) {
                best = Some(v);
            }
        } else if !matches!(b[end - 1], b'-' | b'+' | b'.' | b'e' | b'E') {
            break;
        }
    }
    best.filter(|v| v.is_finite())
}

/// One per-column unit for `::MONEY` / `::COMPACT` columns: one scale for the
/// whole column (never "$4M" next to "$5.9M" next to "$812K"), chosen by its
/// largest value.
#[derive(Clone, Debug, PartialEq)]
pub struct Unit {
    /// Divide by this (1, 1e3, 1e6, 1e9).
    pub div: f64,
    /// Fixed decimals of the scaled value.
    pub dec: u32,
    /// Currency before / after the number (`$`, ` €`).
    pub pre: String,
    pub post: String,
    /// The scale word (`M`, ` Mio.`), empty for 1.
    pub word: String,
    /// The unit as the header shows it (`$M`, `Mio. €`).
    pub label: String,
    /// The unit goes into the header once; the cells show plain numbers.
    pub in_head: bool,
}

/// How a column reads.
#[derive(Clone, Debug, PartialEq)]
pub struct ColumnProfile {
    /// The header people read (with the unit when the cells drop it).
    pub head: String,
    /// The humanised name without the unit.
    pub name: String,
    pub numeric: bool,
    pub bool: bool,
    /// An id or a year: a number shown as a label (no grouping, bars, totals).
    pub label: bool,
    pub ints: bool,
    /// Fixed decimals of a plain number column.
    pub dec: u32,
    pub min: f64,
    pub max: f64,
    pub max_abs: f64,
    /// The default total sums the column (amounts, counts).
    pub additive: bool,
    /// A rate or percentage (`::PERCENT`, or a rate name) that is not
    /// additive: its default total is none (#207).
    pub rate: bool,
    /// Slim in-cell bars (plain numeric columns that vary, not the row key).
    pub bars: bool,
    pub unit: Option<Unit>,
}

impl ColumnProfile {
    /// As the browser reads it (`web/app.js` `renderTable`).
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "head": self.head, "name": self.name, "numeric": self.numeric, "bool": self.bool,
            "label": self.label, "ints": self.ints, "dec": self.dec, "min": self.min, "max": self.max,
            "maxAbs": self.max_abs, "additive": self.additive, "rate": self.rate, "bars": self.bars,
            "unit": self.unit.as_ref().map(|u| serde_json::json!({
                "div": u.div, "dec": u.dec, "pre": u.pre, "post": u.post, "word": u.word,
                "label": u.label, "inHead": u.in_head,
            })),
        })
    }
}

/// The inputs of [`profile_column`] besides the values.
#[derive(Clone, Debug, Default)]
pub struct ColumnSpec<'a> {
    /// Raw column name.
    pub name: &'a str,
    /// The column's format role token (`MONEY`, `COMPACT`, `PERCENT`, `METRIC`,
    /// `PLAIN`, `TREND`, `COLORSCALE…`, `BADGE`, `SPARKLINE`), if any.
    pub fmt: Option<&'a str>,
    /// The first (key) column of the table.
    pub first: bool,
    /// Whole-result min / max from SQL (a `::PAGED` table).
    pub range: Option<(f64, f64)>,
    /// More than one row, or a `::PAGED` table.
    pub many: bool,
}

const METRICS: &[&str] = &["METRIC", "MONEY", "PERCENT", "COMPACT"];

fn metric_of(f: &str) -> MetricFmt {
    match f {
        "MONEY" => MetricFmt::Money,
        "PERCENT" => MetricFmt::Percent,
        "COMPACT" => MetricFmt::Compact,
        _ => MetricFmt::Plain,
    }
}

/// Rate and percentage names (#207): no default total — a sum or a plain mean
/// of rates reads as the overall rate and is not. Substrings as before, plus
/// `per` / `pro` as a word (`Per 10k`, `customers_per_10k`, `per10k`, `km_per_h`).
fn rate_word(name: &str) -> bool {
    let n = name.to_lowercase();
    [
        "pct", "percent", "%", "rate", "ratio", "share", "quote", "anteil",
    ]
    .iter()
    .any(|w| n.contains(w))
        || n.split(|c: char| !c.is_alphanumeric()).any(|t| {
            t == "per"
                || t == "pro"
                || t.strip_prefix("per")
                    .is_some_and(|r| r.starts_with(|c: char| c.is_ascii_digit()))
        })
}

/// Average-like names: the default total is the mean.
fn mean_word(name: &str) -> bool {
    let n = name.to_lowercase();
    ["avg", "mean", "median", "score", "index"]
        .iter()
        .any(|w| n.contains(w))
}

/// Rate- or average-like names (never summed by default).
fn rate_like(name: &str) -> bool {
    rate_word(name) || mean_word(name)
}

/// `(^|[_\s-])(id|no|nr|nummer|number|code|key|zip|plz|postcode|postal)$|^#|^id[_\s]`
/// (case-insensitive): ids, codes and postcodes read as labels
fn id_like(name: &str) -> bool {
    let n = name.to_lowercase();
    if n.starts_with('#') || n.starts_with("id_") || n.starts_with("id ") {
        return true;
    }
    [
        "id", "no", "nr", "nummer", "number", "code", "key", "zip", "plz", "postcode", "postal",
    ]
    .iter()
    .any(|w| {
        n.strip_suffix(w).is_some_and(|rest| {
            rest.is_empty() || rest.ends_with(['_', '-']) || rest.ends_with(char::is_whitespace)
        })
    })
}

fn year_named(name: &str) -> bool {
    let n = name.to_lowercase();
    n.contains("year") || n.contains("jahr") || n == "yr"
}

/// Profile one column from its values (a sample is fine) — the decisions the
/// browser table and the headless table share.
pub fn profile_column(spec: &ColumnSpec, values: &[Cell], locale: &Locale) -> ColumnProfile {
    let f = spec.fmt.unwrap_or("");
    let present: Vec<&Cell> = values.iter().filter(|c| c.present()).collect();
    let nums: Vec<f64> = present.iter().filter_map(|c| c.num()).collect();
    // (::PLAIN only drops the bar: whether the column is numeric comes from its
    // values, so a text column with a few number-like cells stays text)
    let num_fmt = METRICS.contains(&f) || f.starts_with("COLORSCALE") || f == "TREND";
    let numeric = num_fmt
        || if present.is_empty() {
            spec.range.is_some()
        } else {
            present.iter().all(|c| c.num_like())
        };
    let bool = f.is_empty() && !present.is_empty() && present.iter().all(|c| c.bool_like());
    let ints = nums.iter().all(|n| n.fract() == 0.0);
    let year_like = ints
        && !nums.is_empty()
        && nums.iter().all(|n| (1800.0..=2200.0).contains(n))
        && year_named(spec.name);
    let label = numeric && ints && (id_like(spec.name) || year_like);
    let (lo, hi) = match spec.range {
        Some(r) => r,
        None if nums.is_empty() => (0.0, 0.0),
        None => (
            nums.iter().copied().fold(f64::INFINITY, f64::min),
            nums.iter().copied().fold(f64::NEG_INFINITY, f64::max),
        ),
    };
    let max_abs = match lo.abs().max(hi.abs()) {
        m if m > 0.0 => m,
        _ => 1.0,
    };
    let name = human_header(spec.name);
    let unit = (f == "MONEY" || f == "COMPACT").then(|| {
        let (div, word) = if max_abs >= 1e9 {
            (1e9, "B")
        } else if max_abs >= 1e6 {
            (1e6, "M")
        } else if f == "COMPACT" && max_abs >= 1e4 {
            (1e3, "K")
        } else {
            (1.0, "")
        };
        let scaled = max_abs / div;
        let dec = if div == 1.0 {
            if f == "MONEY" && max_abs < 100.0 && !ints {
                2
            } else {
                0
            }
        } else if scaled >= 100.0 {
            0
        } else if scaled >= 10.0 {
            1
        } else {
            2
        };
        // the currency's place and symbol, from the locale's money format
        let sample = if f == "MONEY" {
            format_number_l(1234.0, MetricFmt::Money, locale)
        } else {
            "1234".into()
        };
        let first = sample.find(|c: char| c.is_ascii_digit()).unwrap_or(0);
        let last = sample
            .rfind(|c: char| c.is_ascii_digit())
            .map_or(sample.len(), |i| i + 1);
        let pre = sample[..first].to_string();
        let post = sample[last..].to_string();
        let word = scale_word(word, locale);
        let label = format!("{pre}{word}{post}")
            .replace('\u{a0}', " ")
            .trim()
            .to_string();
        let in_head = !label.is_empty() && spec.many && !name.contains(&label);
        Unit {
            div,
            dec,
            pre,
            post,
            word,
            label,
            in_head,
        }
    });
    let head = match &unit {
        Some(u) if u.in_head => format!("{name} ({})", u.label.replace(' ', "\u{a0}")),
        _ => name.clone(),
    };
    let additive = numeric
        && !label
        && (f == "MONEY"
            || f == "COMPACT"
            || ((f.is_empty() || f == "PLAIN") && ints && !rate_like(spec.name)));
    let rate = numeric && !label && !additive && (f == "PERCENT" || rate_word(spec.name));
    ColumnProfile {
        head,
        name,
        numeric,
        bool,
        label,
        ints,
        dec: if numeric && (f.is_empty() || f == "PLAIN") {
            col_decimals(&nums)
        } else {
            0
        },
        min: lo,
        max: hi,
        max_abs,
        additive,
        rate,
        bars: numeric && f.is_empty() && !label && hi != lo && !spec.first && spec.many,
        unit,
    }
}

/// The scale word of a unit in the locale (`M` → ` Mio.` in German).
fn scale_word(w: &str, l: &Locale) -> String {
    if w.is_empty() {
        return String::new();
    }
    match l.tag {
        "de-DE" => match w {
            "K" => "\u{a0}Tsd.",
            "M" => "\u{a0}Mio.",
            _ => "\u{a0}Mrd.",
        }
        .into(),
        "fr-FR" => match w {
            "K" => "\u{a0}k",
            "M" => "\u{a0}M",
            _ => "\u{a0}Md",
        }
        .into(),
        _ => w.into(),
    }
}

/// How negative numbers read (theme `chrome.negatives`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Negatives {
    /// A true minus sign (−).
    #[default]
    Minus,
    /// Accounting parentheses.
    Parens,
    /// A minus sign, coloured by the renderer.
    Red,
}

impl Negatives {
    pub fn parse(s: &str) -> Negatives {
        match s {
            "parens" => Negatives::Parens,
            "red" => Negatives::Red,
            _ => Negatives::Minus,
        }
    }
}

/// Fixed-decimal, locale-punctuated number (half away from zero, like
/// `Intl.NumberFormat`); `group` = thousands grouping.
fn fixed(n: f64, dec: u32, group: bool, l: &Locale) -> String {
    let p = 10f64.powi(dec as i32);
    let r = (n.abs() * p).round() / p;
    let s = format!("{r:.*}", dec as usize);
    let (int, frac) = match s.find('.') {
        Some(i) => (&s[..i], &s[i + 1..]),
        None => (s.as_str(), ""),
    };
    let mut g = String::with_capacity(int.len() + int.len() / 3 * 3);
    for (i, ch) in int.chars().enumerate() {
        if group && i > 0 && (int.len() - i) % 3 == 0 {
            g.push_str(l.group);
        }
        g.push(ch);
    }
    let neg = n < 0.0 && r != 0.0;
    let sign = if neg { "-" } else { "" };
    if frac.is_empty() {
        format!("{sign}{g}")
    } else {
        format!("{sign}{g}{}{frac}", l.decimal)
    }
}

/// A number in a table cell as the column profile formats it: the column's
/// unit (scaled, in the header or the cell), a KPI format, or fixed decimals;
/// a true minus sign or accounting parentheses.
pub fn number_text(
    n: f64,
    p: &ColumnProfile,
    fmt: Option<&str>,
    l: &Locale,
    neg: Negatives,
) -> String {
    let minus = |t: String| -> String {
        match t.strip_prefix('-') {
            Some(rest) if neg == Negatives::Parens => format!("({rest})"),
            Some(rest) => format!("\u{2212}{rest}"),
            None => t,
        }
    };
    if !n.is_finite() {
        return "–".into();
    }
    if let Some(u) = &p.unit {
        let num = fixed(n.abs() / u.div, u.dec, true, l);
        let sign = if n < 0.0 { "-" } else { "" };
        return minus(if u.in_head {
            format!("{sign}{num}")
        } else {
            format!("{sign}{}{num}{}{}", u.pre, u.word, u.post)
        });
    }
    let f = fmt.unwrap_or("");
    if METRICS.contains(&f) {
        return minus(format_number_l(n, metric_of(f), l));
    }
    minus(fixed(n, p.dec, !p.label, l))
}

/// The text of one cell (numbers per [`number_text`]; a year among text never
/// grouped; ISO dates in the locale's date order; booleans ✓ / ✗).
pub fn cell_text(
    c: &Cell,
    p: &ColumnProfile,
    fmt: Option<&str>,
    l: &Locale,
    neg: Negatives,
) -> String {
    match c {
        Cell::Null => String::new(),
        Cell::Bool(b) => (if *b { "✓" } else { "✗" }).into(),
        _ if p.bool => (if matches!(c, Cell::Text(s) if s == "true") {
            "✓"
        } else {
            "✗"
        })
        .into(),
        _ if p.numeric => match c.num() {
            Some(n) => number_text(n, p, fmt, l, neg),
            None => match c {
                Cell::Text(s) => s.clone(),
                _ => String::new(),
            },
        },
        Cell::Text(s) => {
            if s.len() == 4 && s.bytes().all(|b| b.is_ascii_digit()) {
                return s.clone();
            }
            if l.is_default() {
                s.clone()
            } else {
                l.format_date(s)
            }
        }
        Cell::Num(n) => format!("{n}"),
    }
}

/// What a cell *shows* (the browser's cell renderer): [`cell_text`], except a
/// `::TREND` (an arrow and a signed change with one decimal: `▲ +2.7`,
/// `▼ −3.1`, `– 0.0`) and a `::COLORSCALE…` value (≤ 2 decimals; a diverging
/// scale signs positives: `+0.4`).
pub fn display_text(
    c: &Cell,
    p: &ColumnProfile,
    fmt: Option<&str>,
    l: &Locale,
    neg: Negatives,
) -> String {
    let f = fmt.unwrap_or("");
    match c.num() {
        Some(n) if f == "TREND" => {
            let t = fixed(n.abs(), 1, true, l);
            if n.abs() < 0.05 {
                format!("– {t}")
            } else if n > 0.0 {
                format!("▲ +{t}")
            } else {
                format!("▼ \u{2212}{t}")
            }
        }
        Some(n) if f.starts_with("COLORSCALE") => {
            let t = short_number(n, l, neg);
            if f == "COLORSCALE_DIVERGING" && n > 0.0 {
                format!("+{t}")
            } else {
                t
            }
        }
        _ => cell_text(c, p, fmt, l, neg),
    }
}

/// A number with at most 2 decimals (trailing zeros dropped), grouped, in the
/// locale, with a true minus sign — a trend / colour-scale value or total.
pub fn short_number(n: f64, l: &Locale, neg: Negatives) -> String {
    let t = format_number_l(n, MetricFmt::Plain, l);
    match t.strip_prefix('-') {
        Some(rest) if neg == Negatives::Parens => format!("({rest})"),
        Some(rest) => format!("\u{2212}{rest}"),
        None => t,
    }
}

/// The default aggregation of a column's total: `sum` for amounts and counts,
/// `mean` for averages, scores and other measures; `None` when the column has
/// no total — not a number, a label, a sparkline / badge, or a rate or
/// percentage (#207; the browser shows its tag as "none").
pub fn default_total(p: &ColumnProfile, fmt: Option<&str>) -> Option<&'static str> {
    let f = fmt.unwrap_or("");
    if !p.numeric || p.label || p.rate || f == "SPARKLINE" || f == "BADGE" {
        return None;
    }
    Some(if p.additive { "sum" } else { "mean" })
}

/// The aggregations a `::TOTALS` value may name.
pub const TOTAL_AGGS: &[&str] = &["sum", "mean", "median", "min", "max", "count", "none"];

/// A `::TOTALS` value: one aggregation for every numeric column (`all`) and /
/// or one per column (names as written; the caller matches them).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TotalsSpec {
    pub all: Option<String>,
    pub col: Vec<(String, String)>,
}

/// Parse a `::TOTALS` value (#207): entries split on `;` when the value has
/// one (so a header may hold commas: 'Customers per 10,000 people: none'),
/// else on `,`; `name: agg` or a bare `agg`; `avg` / `average` read as `mean`;
/// unknown entries are skipped. The browser's `parseTotals` (web/app.js) is
/// the same rule (shared vectors: tests/fixtures/table_vectors.json).
pub fn parse_totals(spec: &str) -> TotalsSpec {
    let sep = if spec.contains(';') { ';' } else { ',' };
    let mut out = TotalsSpec::default();
    for part in spec.split(sep) {
        let (name, agg) = match part.rsplit_once(':') {
            Some((n, a)) => (Some(n.trim()), a.trim()),
            None => (None, part.trim()),
        };
        let mut a = agg.to_lowercase();
        if a == "avg" || a == "average" {
            a = "mean".into();
        }
        if !TOTAL_AGGS.contains(&a.as_str()) {
            continue;
        }
        match name {
            None => out.all = Some(a),
            Some(n) if !n.is_empty() => out.col.push((n.to_string(), a)),
            Some(_) => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headers() {
        assert_eq!(human_header("load_pct"), "Load %");
        assert_eq!(human_header("avg_mae"), "Avg MAE");
        assert_eq!(human_header("avgMae"), "Avg MAE");
        assert_eq!(human_header("store_id"), "Store ID");
        assert_eq!(human_header("Margin %"), "Margin %");
        assert_eq!(human_header("revenue"), "Revenue");
        assert_eq!(human_header("r2"), "R²");
    }

    #[test]
    fn numbers_and_parse() {
        assert!(
            is_num_text("-12.5") && is_num_text("1e3") && !is_num_text("1.") && !is_num_text("x1")
        );
        assert_eq!(parse_float("12.5abc"), Some(12.5));
        assert_eq!(parse_float("abc"), None);
        assert_eq!(parse_float("—"), None);
        assert_eq!(parse_float("3 €"), Some(3.0));
        assert_eq!(col_decimals(&[1.0, 2.5, 3.25]), 2);
        assert_eq!(col_decimals(&[0.0405, 0.1]), 4);
        assert!(id_like("store_id") && id_like("#") && id_like("id_x") && !id_like("paid"));
        assert!(
            id_like("zip") && id_like("PLZ") && id_like("customer_postcode") && !id_like("zipper")
        );
    }

    #[test]
    fn rate_and_mean_words() {
        for n in [
            "Per 10k",
            "customers_per_10k",
            "per10k",
            "km_per_h",
            "Kunden pro Kopf",
            "load_pct",
            "Margin %",
            "on-time rate",
        ] {
            assert!(rate_word(n), "{n}");
        }
        for n in [
            "Supermarket visits",
            "person_count",
            "period",
            "properties",
            "avg_basket",
            "km to DC",
        ] {
            assert!(!rate_word(n), "{n}");
        }
        assert!(mean_word("avg_basket") && rate_like("avg_basket") && rate_like("Per 10k"));
    }
}
