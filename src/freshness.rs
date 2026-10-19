//! The "Data as of …" freshness chip (`::ASOF`, issue #34) for the static
//! outputs — the CLI's HTML page, `serve --dashboards` and headless SVG. The
//! browser (`web/app.js`) draws the same chip from the same rules.
//!
//! `SELECT max(loaded_at)::ASOF, 'stale=24h'` — the value is a timestamp
//! (`2026-10-16 14:02:00`, with an optional `+02` / `Z` offset; one without an
//! offset is read as UTC) or a date; an optional `stale=<n><s|m|h|d|w>` column
//! turns the chip amber once the data is older than that.

use crate::format::Locale;

/// A parsed `::ASOF` value: seconds since the Unix epoch (UTC).
pub fn parse_timestamp(s: &str) -> Option<i64> {
    let s = s.trim().trim_matches('"');
    let num = |a: usize, b: usize| -> Option<i64> {
        let t = s.get(a..b)?;
        t.bytes()
            .all(|c| c.is_ascii_digit())
            .then(|| t.parse().ok())?
    };
    if s.len() < 10 || s.as_bytes()[4] != b'-' || s.as_bytes()[7] != b'-' {
        return None;
    }
    let (y, mo, d) = (num(0, 4)?, num(5, 7)?, num(8, 10)?);
    if !(1..=12).contains(&mo) || d < 1 || d > days_in_month(y, mo) {
        return None;
    }
    let mut secs = days_from_civil(y, mo, d) * 86_400;
    let rest = &s[10..];
    if rest.is_empty() {
        return Some(secs);
    }
    let rest = rest.strip_prefix([' ', 'T'])?;
    let (h, mi) = (num(11, 13)?, num(14, 16)?);
    if rest.as_bytes().get(2) != Some(&b':') || h > 23 || mi > 59 {
        return None;
    }
    secs += h * 3600 + mi * 60;
    // optional :SS[.fff], then an optional offset
    let mut tail = &s[16..];
    if let Some(t) = tail.strip_prefix(':') {
        let sec: i64 = t.get(..2)?.parse().ok()?;
        if sec > 60 {
            return None;
        }
        secs += sec;
        tail = &t[2..];
        if let Some(f) = tail.strip_prefix('.') {
            tail = f.trim_start_matches(|c: char| c.is_ascii_digit());
        }
    }
    let tail = tail.trim();
    if tail.is_empty() || tail == "Z" || tail == "UTC" {
        return Some(secs);
    }
    let (sign, off) = match tail.as_bytes()[0] {
        b'+' => (1, &tail[1..]),
        b'-' => (-1, &tail[1..]),
        _ => return None,
    };
    let digits: String = off.chars().filter(|c| *c != ':').collect();
    if !digits.bytes().all(|c| c.is_ascii_digit()) || !(digits.len() == 2 || digits.len() == 4) {
        return None;
    }
    let oh: i64 = digits[..2].parse().ok()?;
    let om: i64 = if digits.len() == 4 {
        digits[2..].parse().ok()?
    } else {
        0
    };
    Some(secs - sign * (oh * 3600 + om * 60))
}

/// `stale=24h` / `24h` / `90m` / `2d` / `1w` / `30s` → seconds.
pub fn parse_stale(s: &str) -> Option<u64> {
    let s = s.trim().trim_matches('"');
    let s = s
        .strip_prefix("stale")
        .map(|r| r.trim_start().strip_prefix('=').unwrap_or(r).trim())
        .unwrap_or(s);
    let unit = s.chars().last()?;
    let n: u64 = s[..s.len() - unit.len_utf8()].trim().parse().ok()?;
    let mul = match unit.to_ascii_lowercase() {
        's' => 1,
        'm' => 60,
        'h' => 3600,
        'd' => 86_400,
        'w' => 7 * 86_400,
        _ => return None,
    };
    (n > 0).then_some(n * mul)
}

/// Is `s` a `stale=…` option (the extra column of an `::ASOF` statement)?
pub fn is_stale_option(s: &str) -> bool {
    s.trim().trim_matches('"').starts_with("stale")
}

/// "just now", "5 min ago", "2 h ago", "3 d ago" for an age in seconds.
pub fn relative(age: i64) -> String {
    let age = age.max(0);
    match age {
        a if a < 60 => "just now".into(),
        a if a < 3600 => format!("{} min ago", a / 60),
        a if a < 2 * 86_400 => format!("{} h ago", a / 3600),
        a => format!("{} d ago", a / 86_400),
    }
}

/// Is `s` a date without a time (`2026-01-01`, a `DATE`)? It reads as a
/// calendar day: no time, no time-zone shift (#282).
pub fn is_date_only(s: &str) -> bool {
    let s = s.trim().trim_matches('"');
    s.len() == 10 && parse_timestamp(s).is_some()
}

/// `YYYY-MM-DD` of the UTC day of `epoch` (seconds).
pub fn iso_day(epoch: i64) -> String {
    let (y, m, d) = civil_from_days(epoch.div_euclid(86_400));
    format!("{y:04}-{m:02}-{d:02}")
}

/// "today" or "3 d ago" for a calendar day `days` before today.
fn day_relative(days: i64) -> String {
    if days <= 0 {
        "today".into()
    } else {
        format!("{days} d ago")
    }
}

/// The chip text and whether it is stale: "Data as of 14:02 UTC (2 h ago)"
/// on the same day as `now`, else with the date in the dashboard locale. A
/// date-only `::ASOF` (`date_only`) reads as a calendar day instead: "Data as
/// of 01.01.2026 (today)" / "(3 d ago)", with no time and no time-zone shift
/// (#282).
pub fn chip_text(
    asof: i64,
    stale_secs: Option<u64>,
    now: i64,
    l: &Locale,
    date_only: bool,
) -> (String, bool) {
    let age = now - asof;
    let stale = stale_secs.is_some_and(|s| age > s as i64);
    if date_only {
        return (
            format!(
                "Data as of {} ({})",
                l.format_date(&iso_day(asof)),
                day_relative(now.div_euclid(86_400) - asof.div_euclid(86_400))
            ),
            stale,
        );
    }
    let (day, hm) = (asof.div_euclid(86_400), asof.rem_euclid(86_400));
    let time = format!("{:02}:{:02} UTC", hm / 3600, hm % 3600 / 60);
    let when = if day == now.div_euclid(86_400) {
        time
    } else {
        format!("{} {time}", l.format_date(&iso_day(asof)))
    };
    (format!("Data as of {when} ({})", relative(age)), stale)
}

/// The current time in epoch seconds (`None` on wasm, where the browser draws
/// the chip itself).
pub fn now_epoch() -> Option<i64> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()
            .map(|d| d.as_secs() as i64)
    }
    #[cfg(target_arch = "wasm32")]
    {
        None
    }
}

fn is_leap(y: i64) -> bool {
    y % 4 == 0 && (y % 100 != 0 || y % 400 == 0)
}

fn days_in_month(y: i64, m: i64) -> i64 {
    match m {
        2 if is_leap(y) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Days since 1970-01-01 (Howard Hinnant's `days_from_civil`).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (
        if m <= 2 {
            yoe + era * 400 + 1
        } else {
            yoe + era * 400
        },
        m,
        d,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_parse_to_utc_epoch() {
        assert_eq!(parse_timestamp("1970-01-01"), Some(0));
        assert_eq!(parse_timestamp("2026-10-16 14:02:00"), Some(1_792_159_320));
        assert_eq!(parse_timestamp("2026-10-16T14:02"), Some(1_792_159_320));
        assert_eq!(
            parse_timestamp("2026-10-16 14:02:00.123456Z"),
            Some(1_792_159_320)
        );
        assert_eq!(
            parse_timestamp("2026-10-16 16:02:00+02"),
            Some(1_792_159_320)
        );
        assert_eq!(
            parse_timestamp("2026-10-16 12:32:00-01:30"),
            Some(1_792_159_320)
        );
        for bad in [
            "",
            "yesterday",
            "2026-13-01",
            "2026-02-30",
            "2026-10-16 25:00",
            "2026-10-16 14:02 PST",
        ] {
            assert_eq!(parse_timestamp(bad), None, "{bad}");
        }
        for z in [-1000, 0, 19_000, 20_742] {
            let (y, m, d) = civil_from_days(z);
            assert_eq!(days_from_civil(y, m, d), z);
        }
    }

    #[test]
    fn stale_thresholds() {
        assert_eq!(parse_stale("stale=24h"), Some(86_400));
        assert_eq!(parse_stale("stale = 90m"), Some(5400));
        assert_eq!(parse_stale("2d"), Some(172_800));
        assert_eq!(parse_stale("1w"), Some(604_800));
        for bad in ["stale=", "stale=24", "stale=0h", "24x", "soon"] {
            assert_eq!(parse_stale(bad), None, "{bad}");
        }
    }

    #[test]
    fn chip_reads_relative_and_turns_stale() {
        let l = Locale::default();
        let at = parse_timestamp("2026-10-16 14:02:00").unwrap();
        let (t, stale) = chip_text(at, Some(86_400), at + 2 * 3600 + 5, &l, false);
        assert_eq!(t, "Data as of 14:02 UTC (2 h ago)");
        assert!(!stale);
        let (t, stale) = chip_text(at, Some(86_400), at + 3 * 86_400, &l, false);
        assert!(
            t.starts_with("Data as of ") && t.ends_with("14:02 UTC (3 d ago)"),
            "{t}"
        );
        assert!(stale);
        assert_eq!(relative(-30), "just now");
        assert_eq!(relative(600), "10 min ago");
        let de = Locale::from_tag("de-DE").unwrap();
        let (t, _) = chip_text(at, None, at + 86_400 * 2, &de, false);
        assert!(t.contains("16.10.2026"), "{t}");
    }

    #[test]
    fn date_only_reads_as_a_day() {
        assert!(is_date_only("2026-01-01"));
        assert!(is_date_only("\"2026-01-01\""));
        assert!(!is_date_only("2026-01-01 00:00:00"));
        let de = Locale::from_tag("de-DE").unwrap();
        let at = parse_timestamp("2026-01-01").unwrap();
        let now = parse_timestamp("2026-10-19 12:00:00").unwrap();
        assert_eq!(
            chip_text(at, None, now, &de, true).0,
            "Data as of 01.01.2026 (291 d ago)"
        );
        let now = at + 3600;
        assert_eq!(
            chip_text(at, None, now, &de, true).0,
            "Data as of 01.01.2026 (today)"
        );
        assert!(chip_text(at, Some(86_400), at + 3 * 86_400, &de, true).1);
    }
}
