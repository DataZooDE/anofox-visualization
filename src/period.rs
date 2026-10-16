//! `::PERIOD` — one input for "this period vs the previous one": a date range
//! picked from presets (last 7/30/90 days, MTD/QTD/YTD, last month, custom), a
//! time grain (day/week/month/quarter) and a comparison (previous period, same
//! period last year, none).
//!
//! The statement carries a small spec string; the column's alias is the
//! variable prefix:
//!
//! ```sql
//! SELECT 'presets=7d,30d,90d,mtd,qtd,ytd; default=30d; grain=auto; compare=prev' AS p ::PERIOD,
//!        'Period' ::LABEL;
//! ```
//!
//! which sets (all VARCHAR, dates ISO `YYYY-MM-DD`, ranges inclusive):
//! `p_from`, `p_to`, `p_prev_from`, `p_prev_to` (NULL without a comparison),
//! `p_grain` (`day`|`week`|`month`|`quarter` — plugs into
//! `date_trunc(getvariable('p_grain'), d)`), `p_compare` (`none`|`prev`|`yoy`),
//! `p_label` ("Sep 1 – 30, 2026") and `p_prev_label` ("vs Aug 2026", NULL
//! without a comparison).
//!
//! All the date arithmetic lives here, once: the browser calls it through the
//! wasm export `period_resolve`, the linter and the headless renderers through
//! [`default_sets`]. Dependency-free (a small proleptic-Gregorian civil date).

use serde_json::{json, Value};

/// Variable suffixes a `::PERIOD` sets, in order (`<prefix>_<suffix>`).
pub const SUFFIXES: [&str; 8] = [
    "from",
    "to",
    "prev_from",
    "prev_to",
    "grain",
    "compare",
    "label",
    "prev_label",
];

/// Spec keys a `::PERIOD` understands.
pub const KEYS: [&str; 8] = [
    "presets", "default", "grain", "grains", "compare", "today", "from", "to",
];

/// Presets when the spec lists none.
pub const DEFAULT_PRESETS: &str = "7d,30d,90d,mtd,qtd,ytd";

/// The variable prefix for a `::PERIOD` column: its name when that is a plain
/// identifier (the alias), else `period`.
pub fn var_prefix(column: &str) -> String {
    let mut cs = column.chars();
    let ok = matches!(cs.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && column.len() <= 48
        && cs.all(|c| c.is_ascii_alphanumeric() || c == '_');
    if ok {
        column.to_string()
    } else {
        "period".into()
    }
}

// ---- civil dates -----------------------------------------------------------

/// A proleptic-Gregorian calendar date (years 1..=9999).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Date {
    pub y: i32,
    pub m: u32,
    pub d: u32,
}

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

fn is_leap(y: i32) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

fn days_in_month(y: i32, m: u32) -> u32 {
    match m {
        2 if is_leap(y) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

impl Date {
    /// A valid date, or `None`.
    pub fn new(y: i32, m: u32, d: u32) -> Option<Date> {
        ((1..=9999).contains(&y) && (1..=12).contains(&m) && d >= 1 && d <= days_in_month(y, m))
            .then_some(Date { y, m, d })
    }

    /// Parse `YYYY-MM-DD` (a trailing time part, `2026-09-30 00:00:00`, is
    /// ignored — DuckDB timestamps as text).
    pub fn parse(s: &str) -> Option<Date> {
        let s = s.trim();
        let s = s
            .get(..10)
            .filter(|_| s.len() == 10 || s[10..].starts_with([' ', 'T']))?;
        let b = s.as_bytes();
        if b[4] != b'-' || b[7] != b'-' {
            return None;
        }
        let num = |r: std::ops::Range<usize>| -> Option<u32> {
            let p = &s[r];
            p.bytes()
                .all(|c| c.is_ascii_digit())
                .then(|| p.parse().ok())?
        };
        Date::new(num(0..4)? as i32, num(5..7)?, num(8..10)?)
    }

    /// `YYYY-MM-DD`.
    pub fn iso(self) -> String {
        format!("{:04}-{:02}-{:02}", self.y, self.m, self.d)
    }

    /// Days since 1970-01-01 (H. Hinnant's `days_from_civil`).
    pub fn days(self) -> i64 {
        let y = self.y as i64 - i64::from(self.m <= 2);
        let era = if y >= 0 { y } else { y - 399 } / 400;
        let yoe = y - era * 400;
        let m = self.m as i64;
        let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + self.d as i64 - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        era * 146097 + doe - 719468
    }

    /// The date `z` days after 1970-01-01 (clamped to years 1..=9999).
    pub fn from_days(z: i64) -> Date {
        let z = z.clamp(-719162, 2932896) + 719468;
        let era = if z >= 0 { z } else { z - 146096 } / 146097;
        let doe = z - era * 146097;
        let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
        let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
        let y = (yoe + era * 400 + i64::from(m <= 2)) as i32;
        Date { y, m, d }
    }

    pub fn add_days(self, n: i64) -> Date {
        Date::from_days(self.days() + n)
    }

    /// 0 = Monday … 6 = Sunday (ISO weeks).
    pub fn weekday(self) -> i64 {
        (self.days() + 3).rem_euclid(7)
    }

    fn first_of_month(self) -> Date {
        Date { d: 1, ..self }
    }

    fn last_of_month(self) -> Date {
        Date {
            d: days_in_month(self.y, self.m),
            ..self
        }
    }

    fn is_month_end(self) -> bool {
        self.d == days_in_month(self.y, self.m)
    }

    fn quarter(self) -> u32 {
        (self.m - 1) / 3 + 1
    }

    fn first_of_quarter(self) -> Date {
        Date {
            m: (self.quarter() - 1) * 3 + 1,
            d: 1,
            ..self
        }
    }

    /// Shift by `n` months, clamping the day to the target month's length
    /// (Jan 31 + 1 month = Feb 28/29).
    pub fn add_months(self, n: i64) -> Date {
        let idx = (self.y as i64 * 12 + self.m as i64 - 1 + n).clamp(12, 9999 * 12 + 11);
        let (y, m) = ((idx / 12) as i32, (idx % 12) as u32 + 1);
        Date {
            y,
            m,
            d: self.d.min(days_in_month(y, m)),
        }
    }

    /// Shift by `n` months keeping a month end a month end (Apr 30 − 1 month =
    /// Mar 31; Feb 28, 2027 − 12 months = Feb 29, 2026 is not a thing, but Feb
    /// 28, 2025 − 12 months = Feb 29, 2024).
    pub fn add_months_eom(self, n: i64) -> Date {
        let t = self.add_months(n);
        if self.is_month_end() {
            t.last_of_month()
        } else {
            t
        }
    }

    /// `Sep 30, 2026`.
    fn long(self) -> String {
        format!("{} {}, {}", MONTHS[self.m as usize - 1], self.d, self.y)
    }

    /// `Sep 30`.
    fn short(self) -> String {
        format!("{} {}", MONTHS[self.m as usize - 1], self.d)
    }
}

/// Today's date in UTC from the system clock (`None` on wasm, where the
/// browser passes its local date instead).
pub fn utc_today() -> Option<Date> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_secs() as i64;
        Some(Date::from_days(secs.div_euclid(86_400)))
    }
    #[cfg(target_arch = "wasm32")]
    {
        None
    }
}

/// A human name for an inclusive range: `Sep 30, 2026`, `Sep 2026`,
/// `Q3 2026`, `2026`, `Sep 1 – 15, 2026`, `Aug 2 – Sep 30, 2026`,
/// `Dec 1, 2025 – Jan 15, 2026`.
pub fn range_label(from: Date, to: Date) -> String {
    if from == to {
        return from.long();
    }
    let whole_months = from.d == 1 && to.is_month_end() && from.y == to.y;
    if whole_months && from.m == 1 && to.m == 12 {
        return from.y.to_string();
    }
    if whole_months && from.m == to.m {
        return format!("{} {}", MONTHS[from.m as usize - 1], from.y);
    }
    if whole_months && from.m % 3 == 1 && to.m == from.m + 2 {
        return format!("Q{} {}", from.quarter(), from.y);
    }
    if from.y != to.y {
        return format!("{} – {}", from.long(), to.long());
    }
    if from.m == to.m {
        return format!("{} – {}, {}", from.short(), to.d, to.y);
    }
    format!("{} – {}, {}", from.short(), to.short(), to.y)
}

// ---- spec ------------------------------------------------------------------

/// The time grain a `::PERIOD` hands to `date_trunc`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Grain {
    Day,
    Week,
    Month,
    Quarter,
}

impl Grain {
    pub const ALL: [Grain; 4] = [Grain::Day, Grain::Week, Grain::Month, Grain::Quarter];

    pub fn as_str(self) -> &'static str {
        match self {
            Grain::Day => "day",
            Grain::Week => "week",
            Grain::Month => "month",
            Grain::Quarter => "quarter",
        }
    }

    pub fn parse(s: &str) -> Option<Grain> {
        Grain::ALL
            .into_iter()
            .find(|g| g.as_str().eq_ignore_ascii_case(s.trim()))
    }

    /// The grain for a range of `days` days when none is chosen: ≤ 31 days
    /// daily, ≤ 120 weekly, ≤ 730 monthly, else quarterly.
    pub fn auto(days: i64) -> Grain {
        match days {
            i64::MIN..=31 => Grain::Day,
            32..=120 => Grain::Week,
            121..=730 => Grain::Month,
            _ => Grain::Quarter,
        }
    }
}

/// What the range is compared with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Compare {
    None,
    Prev,
    Yoy,
}

impl Compare {
    pub const ALL: [Compare; 3] = [Compare::None, Compare::Prev, Compare::Yoy];

    pub fn as_str(self) -> &'static str {
        match self {
            Compare::None => "none",
            Compare::Prev => "prev",
            Compare::Yoy => "yoy",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Compare::None => "No comparison",
            Compare::Prev => "Previous period",
            Compare::Yoy => "Same period last year",
        }
    }

    pub fn parse(s: &str) -> Option<Compare> {
        match s.trim().to_ascii_lowercase().as_str() {
            "none" | "off" | "no" => Some(Compare::None),
            "prev" | "previous" | "pop" => Some(Compare::Prev),
            "yoy" | "year" | "last_year" => Some(Compare::Yoy),
            _ => None,
        }
    }
}

/// A date-range preset.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Preset {
    /// The last N days, ending today (inclusive).
    Days(u32),
    /// The last N weeks (7N days), ending today.
    Weeks(u32),
    /// The last N months, ending today (from = today − N months + 1 day).
    Months(u32),
    Today,
    Yesterday,
    /// Week to date (ISO week, from Monday).
    Wtd,
    Mtd,
    Qtd,
    Ytd,
    LastWeek,
    LastMonth,
    LastQuarter,
    LastYear,
    /// From / to picked by hand.
    Custom,
}

impl Preset {
    /// Parse a preset id (`7d`, `12w`, `6m`, `mtd`, `last_month`, `custom`, …).
    pub fn parse(s: &str) -> Option<Preset> {
        let s = s.trim().to_ascii_lowercase();
        let named = match s.as_str() {
            "today" => Some(Preset::Today),
            "yesterday" => Some(Preset::Yesterday),
            "wtd" => Some(Preset::Wtd),
            "mtd" => Some(Preset::Mtd),
            "qtd" => Some(Preset::Qtd),
            "ytd" => Some(Preset::Ytd),
            "last_week" => Some(Preset::LastWeek),
            "last_month" => Some(Preset::LastMonth),
            "last_quarter" => Some(Preset::LastQuarter),
            "last_year" => Some(Preset::LastYear),
            "custom" => Some(Preset::Custom),
            _ => None,
        };
        if named.is_some() {
            return named;
        }
        let unit = s.chars().last()?;
        let n: u32 = s[..s.len() - 1].parse().ok()?;
        match unit {
            'd' if (1..=3660).contains(&n) => Some(Preset::Days(n)),
            'w' if (1..=520).contains(&n) => Some(Preset::Weeks(n)),
            'm' if (1..=120).contains(&n) => Some(Preset::Months(n)),
            _ => None,
        }
    }

    pub fn id(self) -> String {
        match self {
            Preset::Days(n) => format!("{n}d"),
            Preset::Weeks(n) => format!("{n}w"),
            Preset::Months(n) => format!("{n}m"),
            Preset::Today => "today".into(),
            Preset::Yesterday => "yesterday".into(),
            Preset::Wtd => "wtd".into(),
            Preset::Mtd => "mtd".into(),
            Preset::Qtd => "qtd".into(),
            Preset::Ytd => "ytd".into(),
            Preset::LastWeek => "last_week".into(),
            Preset::LastMonth => "last_month".into(),
            Preset::LastQuarter => "last_quarter".into(),
            Preset::LastYear => "last_year".into(),
            Preset::Custom => "custom".into(),
        }
    }

    pub fn label(self) -> String {
        let n = |n: u32, unit: &str| {
            if n == 1 {
                format!("Last {unit}")
            } else {
                format!("Last {n} {unit}s")
            }
        };
        match self {
            Preset::Days(1) => "Last day".into(),
            Preset::Days(k) => n(k, "day"),
            Preset::Weeks(1) => "Last 7 days".into(),
            Preset::Weeks(k) => n(k, "week"),
            Preset::Months(1) => "Last month (rolling)".into(),
            Preset::Months(k) => n(k, "month"),
            Preset::Today => "Today".into(),
            Preset::Yesterday => "Yesterday".into(),
            Preset::Wtd => "Week to date".into(),
            Preset::Mtd => "Month to date".into(),
            Preset::Qtd => "Quarter to date".into(),
            Preset::Ytd => "Year to date".into(),
            Preset::LastWeek => "Last week".into(),
            Preset::LastMonth => "Last month".into(),
            Preset::LastQuarter => "Last quarter".into(),
            Preset::LastYear => "Last year".into(),
            Preset::Custom => "Custom range".into(),
        }
    }

    /// The range `[from, to]` this preset means on `today`, and the previous
    /// period's range (`compare=prev`). `Custom` has no range of its own.
    fn ranges(self, t: Date) -> Option<((Date, Date), (Date, Date))> {
        let rolling = |days: i64| {
            let from = t.add_days(1 - days);
            ((from, t), (from.add_days(-days), from.add_days(-1)))
        };
        Some(match self {
            Preset::Days(n) => rolling(n as i64),
            Preset::Weeks(n) => rolling(7 * n as i64),
            Preset::Months(n) => {
                let from = t.add_months(-(n as i64)).add_days(1);
                ((from, t), (from.add_months(-(n as i64)), from.add_days(-1)))
            }
            Preset::Today => rolling(1),
            Preset::Yesterday => {
                let y = t.add_days(-1);
                ((y, y), (y.add_days(-1), y.add_days(-1)))
            }
            Preset::Wtd => {
                let from = t.add_days(-t.weekday());
                ((from, t), (from.add_days(-7), t.add_days(-7)))
            }
            Preset::Mtd => {
                let from = t.first_of_month();
                ((from, t), (from.add_months(-1), t.add_months_eom(-1)))
            }
            Preset::Qtd => {
                let from = t.first_of_quarter();
                ((from, t), (from.add_months(-3), t.add_months_eom(-3)))
            }
            Preset::Ytd => {
                let from = Date { m: 1, d: 1, ..t };
                ((from, t), (from.add_months(-12), t.add_months_eom(-12)))
            }
            Preset::LastWeek => {
                let from = t.add_days(-t.weekday() - 7);
                (
                    (from, from.add_days(6)),
                    (from.add_days(-7), from.add_days(-1)),
                )
            }
            Preset::LastMonth => {
                let from = t.first_of_month().add_months(-1);
                let prev = from.add_months(-1);
                ((from, from.last_of_month()), (prev, prev.last_of_month()))
            }
            Preset::LastQuarter => {
                let from = t.first_of_quarter().add_months(-3);
                let prev = from.add_months(-3);
                (
                    (from, from.add_months(2).last_of_month()),
                    (prev, from.add_days(-1)),
                )
            }
            Preset::LastYear => {
                let from = Date {
                    y: t.y - 1,
                    m: 1,
                    d: 1,
                };
                let to = Date {
                    y: t.y - 1,
                    m: 12,
                    d: 31,
                };
                ((from, to), (from.add_months(-12), to.add_months(-12)))
            }
            Preset::Custom => return None,
        })
    }
}

/// A parsed `::PERIOD` spec (see the module docs). Missing keys take defaults.
#[derive(Clone, Debug, PartialEq)]
pub struct Spec {
    /// The preset choices, in order (`custom` is always offered on top).
    pub presets: Vec<Preset>,
    /// The preset selected initially.
    pub default: Preset,
    /// `None` = automatic (by the range's length).
    pub grain: Option<Grain>,
    /// The grains the switch offers (`grains=`; empty = no switch).
    pub grains: Vec<Grain>,
    pub compare: Compare,
    /// A fixed "today" (reproducible examples / tests); `None` = the clock.
    pub today: Option<Date>,
    /// The initial custom range (`default=custom`).
    pub from: Option<Date>,
    pub to: Option<Date>,
}

/// Parse a spec `key=value; key=value` (keys: [`KEYS`]). Lenient: every
/// problem is reported in the error list and the rest still applies, so a
/// dashboard with a typo keeps working on the defaults.
pub fn parse_spec(spec: &str) -> (Spec, Vec<String>) {
    let mut errs = Vec::new();
    let mut presets: Option<Vec<Preset>> = None;
    let mut default: Option<Preset> = None;
    let mut grain = None;
    let mut grains = Grain::ALL.to_vec();
    let mut compare = Compare::Prev;
    let (mut today, mut from, mut to) = (None, None, None);
    let date = |k: &str, v: &str, errs: &mut Vec<String>| {
        let d = Date::parse(v);
        if d.is_none() {
            errs.push(format!("`{k}={v}` is not a date (YYYY-MM-DD)"));
        }
        d
    };
    for part in spec.split([';', '\n']) {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let Some((k, v)) = part.split_once(['=', ':']) else {
            errs.push(format!("`{part}` is not key=value"));
            continue;
        };
        let (k, v) = (k.trim().to_ascii_lowercase(), v.trim());
        match k.as_str() {
            "presets" => {
                let mut ps = Vec::new();
                for id in v.split([',', '|', ' ']).filter(|s| !s.trim().is_empty()) {
                    match Preset::parse(id) {
                        Some(Preset::Custom) => {} // always offered
                        Some(p) if !ps.contains(&p) => ps.push(p),
                        Some(_) => {}
                        None => errs.push(unknown_preset(id)),
                    }
                }
                presets = Some(ps);
            }
            "default" => match Preset::parse(v) {
                Some(p) => default = Some(p),
                None => errs.push(unknown_preset(v)),
            },
            "grain" => {
                if !v.eq_ignore_ascii_case("auto") {
                    grain = Grain::parse(v);
                    if grain.is_none() {
                        errs.push(format!(
                            "unknown grain `{v}` (auto, day, week, month, quarter)"
                        ));
                    }
                }
            }
            "grains" => {
                grains.clear();
                if !v.eq_ignore_ascii_case("none") {
                    for g in v.split([',', '|', ' ']).filter(|s| !s.trim().is_empty()) {
                        match Grain::parse(g) {
                            Some(g) if !grains.contains(&g) => grains.push(g),
                            Some(_) => {}
                            None => errs.push(format!(
                                "unknown grain `{}` (day, week, month, quarter, or grains=none)",
                                g.trim()
                            )),
                        }
                    }
                    grains.sort_by_key(|g| Grain::ALL.iter().position(|x| x == g));
                }
            }
            "compare" => match Compare::parse(v) {
                Some(c) => compare = c,
                None => errs.push(format!("unknown compare `{v}` (prev, yoy, none)")),
            },
            "today" => today = date(&k, v, &mut errs),
            "from" => from = date(&k, v, &mut errs),
            "to" => to = date(&k, v, &mut errs),
            _ => errs.push(format!("unknown key `{k}` (known: {})", KEYS.join(", "))),
        }
    }
    let mut presets = presets.unwrap_or_else(|| {
        DEFAULT_PRESETS
            .split(',')
            .filter_map(Preset::parse)
            .collect()
    });
    if (from.is_some() || to.is_some()) && default.is_none() {
        default = Some(Preset::Custom);
    }
    if presets.is_empty() && default.is_none_or(|d| d == Preset::Custom) && from.is_none() {
        errs.push("`presets=` lists no preset".into());
        presets = vec![Preset::Days(30)];
    }
    let default = match default {
        Some(Preset::Custom) if from.is_none() || to.is_none() => {
            errs.push("`default=custom` needs `from=` and `to=` dates".into());
            presets.first().copied().unwrap_or(Preset::Days(30))
        }
        Some(p) => {
            if p != Preset::Custom && !presets.contains(&p) {
                presets.insert(0, p); // the default is always a choice
            }
            p
        }
        None => presets.first().copied().unwrap_or(Preset::Days(30)),
    };
    (
        Spec {
            presets,
            default,
            grain,
            grains,
            compare,
            today,
            from,
            to,
        },
        errs,
    )
}

fn unknown_preset(id: &str) -> String {
    format!(
        "unknown preset `{}` (Nd, Nw, Nm, today, yesterday, wtd, mtd, qtd, ytd, last_week, \
         last_month, last_quarter, last_year, custom)",
        id.trim()
    )
}

// ---- resolve ---------------------------------------------------------------

/// The viewer's current choice (all optional; missing = the spec's default).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct State {
    pub preset: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
    /// `day`|`week`|`month`|`quarter`, or `auto`.
    pub grain: Option<String>,
    pub compare: Option<String>,
    /// The viewer's local date (the browser); the spec's `today=` wins.
    pub today: Option<String>,
}

impl State {
    pub fn from_json(v: &Value) -> State {
        let s = |k: &str| {
            v.get(k)
                .and_then(|x| x.as_str())
                .map(str::to_string)
                .filter(|s| !s.trim().is_empty())
        };
        State {
            preset: s("preset"),
            from: s("from"),
            to: s("to"),
            grain: s("grain"),
            compare: s("compare"),
            today: s("today"),
        }
    }
}

/// A resolved period: what the variables are set to.
#[derive(Clone, Debug, PartialEq)]
pub struct Resolved {
    pub preset: Preset,
    pub from: Date,
    pub to: Date,
    pub prev: Option<(Date, Date)>,
    pub grain: Grain,
    /// The grain was picked automatically from the range length.
    pub grain_auto: bool,
    pub compare: Compare,
    pub today: Date,
}

impl Resolved {
    pub fn label(&self) -> String {
        range_label(self.from, self.to)
    }

    pub fn prev_label(&self) -> Option<String> {
        self.prev.map(|(f, t)| format!("vs {}", range_label(f, t)))
    }

    /// `(suffix, value)` for every variable in [`SUFFIXES`] order (`None` = NULL).
    pub fn vars(&self) -> Vec<(&'static str, Option<String>)> {
        vec![
            ("from", Some(self.from.iso())),
            ("to", Some(self.to.iso())),
            ("prev_from", self.prev.map(|p| p.0.iso())),
            ("prev_to", self.prev.map(|p| p.1.iso())),
            ("grain", Some(self.grain.as_str().into())),
            ("compare", Some(self.compare.as_str().into())),
            ("label", Some(self.label())),
            ("prev_label", self.prev_label()),
        ]
    }
}

/// Year-over-year: both ends one year back, a month end staying a month end
/// (Feb 29 → Feb 28; Feb 28, 2025 → Feb 29, 2024).
fn yoy((f, t): (Date, Date)) -> (Date, Date) {
    (f.add_months(-12), t.add_months_eom(-12))
}

/// Resolve `spec` + the viewer's `state` to a concrete period. `fallback_today`
/// is used when neither the spec (`today=`) nor the state carries a date.
/// Problems with the state (a bad custom date, an unknown preset) are pushed to
/// `errs` and the spec's default applies.
pub fn resolve(
    spec: &Spec,
    state: &State,
    fallback_today: Option<Date>,
    errs: &mut Vec<String>,
) -> Result<Resolved, String> {
    let today = spec
        .today
        .or_else(|| state.today.as_deref().and_then(Date::parse))
        .or(fallback_today)
        .ok_or("no date for today (pass today=YYYY-MM-DD)")?;

    let mut preset = spec.default;
    if let Some(id) = state.preset.as_deref() {
        match Preset::parse(id) {
            Some(p) if p == Preset::Custom || spec.presets.contains(&p) => preset = p,
            _ => errs.push(format!("preset `{id}` is not offered")),
        }
    }
    let mut ranges = preset.ranges(today);
    if preset == Preset::Custom {
        let pick = |s: &Option<String>, d: Option<Date>, errs: &mut Vec<String>| match s {
            Some(v) => Date::parse(v).or_else(|| {
                errs.push(format!("`{v}` is not a date (YYYY-MM-DD)"));
                d
            }),
            None => d,
        };
        let from = pick(&state.from, spec.from, errs);
        let to = pick(&state.to, spec.to, errs);
        if let (Some(a), Some(b)) = (from, to) {
            let (a, b) = if a <= b { (a, b) } else { (b, a) };
            let n = b.days() - a.days() + 1;
            ranges = Some(((a, b), (a.add_days(-n), a.add_days(-1))));
        } else {
            preset = spec.presets.first().copied().unwrap_or(Preset::Days(30));
            ranges = preset.ranges(today);
        }
    }
    let ((from, to), prev_range) = ranges.ok_or("no range")?;

    let compare = match state.compare.as_deref() {
        Some(c) => Compare::parse(c).unwrap_or_else(|| {
            errs.push(format!("unknown compare `{c}`"));
            spec.compare
        }),
        None => spec.compare,
    };
    let chosen = match state.grain.as_deref() {
        Some(g) if g.eq_ignore_ascii_case("auto") => None,
        Some(g) => Grain::parse(g).or_else(|| {
            errs.push(format!("unknown grain `{g}`"));
            spec.grain
        }),
        None => spec.grain,
    };
    let grain = chosen.unwrap_or_else(|| {
        // Automatic: by the range's length, the nearest grain the switch offers.
        let g = Grain::auto(to.days() - from.days() + 1);
        let pos = |g: &Grain| Grain::ALL.iter().position(|x| x == g).unwrap_or(0) as i64;
        spec.grains
            .iter()
            .copied()
            .min_by_key(|o| (pos(o) - pos(&g)).abs())
            .unwrap_or(g)
    });
    let prev = match compare {
        Compare::None => None,
        Compare::Prev => Some(prev_range),
        Compare::Yoy => Some(yoy((from, to))),
    };
    Ok(Resolved {
        preset,
        from,
        to,
        prev,
        grain,
        grain_auto: chosen.is_none(),
        compare,
        today,
    })
}

/// The JSON the browser control reads (wasm `period_resolve`):
/// `{ok, errors, prefix, presets:[{id,label}], grains, compares:[{id,label}],
///   preset, grain, grain_auto, compare, from, to, today,
///   vars:{suffix: value|null}}`. `state_json` = `{preset?, from?, to?, grain?,
/// compare?, today?, column?}` (`column` = the ::PERIOD column's name →
/// `prefix`, see [`var_prefix`]).
pub fn resolve_json(spec: &str, state_json: &str) -> Value {
    let (sp, mut errs) = parse_spec(spec);
    let sv: Value = serde_json::from_str(state_json).unwrap_or(Value::Null);
    let state = State::from_json(&sv);
    let column = sv.get("column").and_then(|c| c.as_str()).unwrap_or("");
    let mut presets: Vec<Value> = sp
        .presets
        .iter()
        .map(|p| json!({"id": p.id(), "label": p.label()}))
        .collect();
    presets.push(json!({"id": "custom", "label": Preset::Custom.label()}));
    let base = json!({
        "prefix": var_prefix(column),
        "presets": presets,
        "grains": sp.grains.iter().map(|g| g.as_str()).collect::<Vec<_>>(),
        "compares": Compare::ALL.iter().map(|c| json!({"id": c.as_str(), "label": c.label()})).collect::<Vec<_>>(),
    });
    let mut out = base;
    match resolve(&sp, &state, utc_today(), &mut errs) {
        Ok(r) => {
            let vars: serde_json::Map<String, Value> = r
                .vars()
                .into_iter()
                .map(|(k, v)| (k.to_string(), v.map(Value::String).unwrap_or(Value::Null)))
                .collect();
            out["preset"] = json!(r.preset.id());
            out["grain"] = json!(r.grain.as_str());
            out["grain_auto"] = json!(r.grain_auto);
            out["compare"] = json!(r.compare.as_str());
            out["from"] = json!(r.from.iso());
            out["to"] = json!(r.to.iso());
            out["today"] = json!(r.today.iso());
            out["vars"] = Value::Object(vars);
        }
        Err(e) => errs.push(e),
    }
    out["ok"] = json!(errs.is_empty());
    out["errors"] = json!(errs);
    out
}

/// The `SET VARIABLE` statements giving a `::PERIOD` named `column` its
/// default (the spec's default preset, today = `today=` or the UTC date) —
/// for the linter and the headless renderers. Empty when no date is known.
pub fn default_sets(column: &str, spec: &str) -> Vec<String> {
    let prefix = var_prefix(column);
    let (sp, mut errs) = parse_spec(spec);
    let Ok(r) = resolve(&sp, &State::default(), utc_today(), &mut errs) else {
        return Vec::new();
    };
    r.vars()
        .into_iter()
        .map(|(k, v)| {
            format!(
                "SET VARIABLE \"{prefix}_{k}\" = {}",
                v.map(|s| format!("'{}'", s.replace('\'', "''")))
                    .unwrap_or_else(|| "NULL".into())
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> Date {
        Date::parse(s).unwrap()
    }

    #[test]
    fn civil_round_trip() {
        for z in [-719162i64, -1, 0, 1, 11016, 20727, 2932896] {
            assert_eq!(Date::from_days(z).days(), z, "{z}");
        }
        assert_eq!(d("1970-01-01").days(), 0);
        assert_eq!(d("2026-10-16").weekday(), 4); // a Friday
        assert_eq!(d("2026-10-12").weekday(), 0); // Monday
        assert!(Date::parse("2026-02-29").is_none());
        assert!(Date::parse("2024-02-29").is_some());
        assert!(Date::parse("2026-9-1").is_none());
        assert_eq!(d("2026-09-30 00:00:00"), d("2026-09-30"));
        assert_eq!(d("2026-03-31").add_months(-1).iso(), "2026-02-28");
        assert_eq!(d("2026-04-30").add_months_eom(-1).iso(), "2026-03-31");
        assert_eq!(d("2025-02-28").add_months_eom(-12).iso(), "2024-02-29");
    }

    #[test]
    fn labels() {
        assert_eq!(range_label(d("2026-09-01"), d("2026-09-30")), "Sep 2026");
        assert_eq!(range_label(d("2026-07-01"), d("2026-09-30")), "Q3 2026");
        assert_eq!(range_label(d("2025-01-01"), d("2025-12-31")), "2025");
        assert_eq!(
            range_label(d("2026-09-01"), d("2026-09-15")),
            "Sep 1 – 15, 2026"
        );
        assert_eq!(
            range_label(d("2026-08-02"), d("2026-09-30")),
            "Aug 2 – Sep 30, 2026"
        );
        assert_eq!(
            range_label(d("2025-12-01"), d("2026-01-15")),
            "Dec 1, 2025 – Jan 15, 2026"
        );
        assert_eq!(
            range_label(d("2026-09-30"), d("2026-09-30")),
            "Sep 30, 2026"
        );
    }

    #[test]
    fn spec_errors_are_reported_and_defaults_kept() {
        let (s, e) = parse_spec("presets=7d,bogus; grain=hour; color=red; compare=maybe");
        assert_eq!(e.len(), 4, "{e:?}");
        assert_eq!(s.presets, vec![Preset::Days(7)]);
        assert_eq!(s.grain, None);
        assert_eq!(s.compare, Compare::Prev);
        let (s, e) = parse_spec("");
        assert!(e.is_empty());
        assert_eq!(s.presets.len(), 6);
        assert_eq!(s.default, Preset::Days(7));
        let (_, e) = parse_spec("default=custom");
        assert_eq!(e.len(), 1);
        let (s, e) = parse_spec("from=2026-01-01; to=2026-03-31");
        assert!(e.is_empty(), "{e:?}");
        assert_eq!(s.default, Preset::Custom);
    }

    #[test]
    fn default_sets_prime_every_variable() {
        let s = default_sets("p", "default=mtd; today=2026-03-31");
        assert_eq!(s.len(), 8);
        assert_eq!(s[0], "SET VARIABLE \"p_from\" = '2026-03-01'");
        assert_eq!(s[3], "SET VARIABLE \"p_prev_to\" = '2026-02-28'");
        assert_eq!(s[7], "SET VARIABLE \"p_prev_label\" = 'vs Feb 2026'");
        let s = default_sets("'odd name'", "compare=none; today=2026-03-31");
        assert_eq!(s[2], "SET VARIABLE \"period_prev_from\" = NULL");
    }
}
