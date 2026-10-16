//! Threshold rules (`::THRESHOLDS`, #30): one rule set gives a value a status
//! — good / warn / bad (or neutral) — and the same status colours a KPI value,
//! a gauge's zones, a chart's threshold lines and table cells, in the theme's
//! semantic colours (`positive` / `warning` / `negative` / `muted`).
//!
//! A rule set is `cond:status` rules separated by `;` (or `,`), checked in
//! order — the first match wins:
//!
//! ```text
//! '<0:bad; 0..0.05:warn; >0.05:good'
//! '<80:bad; <95:warn; else:good'       -- `else` / `*` (or a bare status) catches the rest
//! '>=1.2M:good; >=1M:warn; bad'        -- k / M / B suffixes, `5%` = 0.05
//! ```
//!
//! Conditions: `<x`, `<=x`, `>x`, `>=x`, `=x`, `a..b` (inclusive). Statuses:
//! `good` (`ok`, `positive`, `green`), `warn` (`warning`, `amber`, `yellow`),
//! `bad` (`critical`, `negative`, `red`, `alert`), `neutral` (`muted`, `none`).
//!
//! A table directive names columns, `|`-separated:
//! `'Margin %: <10:bad; <15:warn; good | accuracy: <0.85:bad'`.

use crate::theme::{ResolvedTheme, Rgb};

/// A value's status under a rule set.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Status {
    Good,
    Warn,
    Bad,
    Neutral,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Good => "good",
            Status::Warn => "warn",
            Status::Bad => "bad",
            Status::Neutral => "neutral",
        }
    }

    fn parse(s: &str) -> Option<Status> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "good" | "ok" | "positive" | "green" => Status::Good,
            "warn" | "warning" | "amber" | "yellow" => Status::Warn,
            "bad" | "critical" | "negative" | "red" | "alert" => Status::Bad,
            "neutral" | "muted" | "none" => Status::Neutral,
            _ => return None,
        })
    }

    /// Ordering for "the worse of two": neutral < good < warn < bad.
    fn severity(self) -> u8 {
        match self {
            Status::Neutral => 0,
            Status::Good => 1,
            Status::Warn => 2,
            Status::Bad => 3,
        }
    }

    /// The theme's semantic colour for this status.
    pub fn color(self, t: &ResolvedTheme) -> Rgb {
        match self {
            Status::Good => t.colors.positive,
            Status::Warn => t.colors.warning,
            Status::Bad => t.colors.negative,
            Status::Neutral => t.colors.muted,
        }
    }
}

/// One `cond:status` rule; `None` bounds are open (`else` has neither).
#[derive(Clone, Debug, PartialEq)]
pub struct Rule {
    /// Lower bound and whether it is inclusive.
    pub lo: Option<(f64, bool)>,
    /// Upper bound and whether it is inclusive.
    pub hi: Option<(f64, bool)>,
    pub status: Status,
    /// The condition as written (for tooltips).
    pub text: String,
}

impl Rule {
    fn matches(&self, v: f64) -> bool {
        let lo_ok = self
            .lo
            .is_none_or(|(b, inc)| if inc { v >= b } else { v > b });
        let hi_ok = self
            .hi
            .is_none_or(|(b, inc)| if inc { v <= b } else { v < b });
        lo_ok && hi_ok
    }
}

/// An ordered rule set.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Rules {
    pub rules: Vec<Rule>,
}

/// A number with an optional `%` (÷100) or `k` / `M` / `B` suffix.
fn number(s: &str) -> Option<f64> {
    let t = s.trim().replace('_', "");
    let (body, mul) = match t.chars().last()? {
        '%' => (&t[..t.len() - 1], 0.01),
        'k' | 'K' => (&t[..t.len() - 1], 1e3),
        'M' => (&t[..t.len() - 1], 1e6),
        'B' => (&t[..t.len() - 1], 1e9),
        _ => (t.as_str(), 1.0),
    };
    body.trim()
        .parse::<f64>()
        .ok()
        .map(|f| f * mul)
        .filter(|f| f.is_finite())
}

/// Parse a condition into (lo, hi) bounds; `None` when it isn't one.
#[allow(clippy::type_complexity)]
fn condition(c: &str) -> Option<(Option<(f64, bool)>, Option<(f64, bool)>)> {
    let c = c.trim();
    if matches!(
        c.to_ascii_lowercase().as_str(),
        "else" | "*" | "default" | "otherwise"
    ) {
        return Some((None, None));
    }
    for (op, f) in [
        ("<=", (|x| (None, Some((x, true)))) as fn(f64) -> _),
        (">=", |x| (Some((x, true)), None)),
        ("==", |x| (Some((x, true)), Some((x, true)))),
        ("<", |x| (None, Some((x, false)))),
        (">", |x| (Some((x, false)), None)),
        ("=", |x| (Some((x, true)), Some((x, true)))),
    ] {
        if let Some(rest) = c.strip_prefix(op) {
            return number(rest).map(f);
        }
    }
    let (a, b) = c.split_once("..")?;
    let (a, b) = (number(a)?, number(b)?);
    Some((Some((a.min(b), true)), Some((a.max(b), true))))
}

impl Rules {
    /// Parse `'<0:bad; 0..0.05:warn; >0.05:good'`.
    pub fn parse(spec: &str) -> Result<Rules, String> {
        let mut rules = Vec::new();
        for part in spec
            .split([';', ','])
            .map(str::trim)
            .filter(|p| !p.is_empty())
        {
            let (cond, status) = match part.rsplit_once(':') {
                Some((c, s)) => (c.trim(), s.trim()),
                None => ("else", part), // a bare status catches the rest
            };
            let status = Status::parse(status).ok_or_else(|| {
                format!("::THRESHOLDS rule `{part}`: unknown status `{status}` (good, warn, bad, neutral)")
            })?;
            let (lo, hi) = condition(cond).ok_or_else(|| {
                format!("::THRESHOLDS rule `{part}`: the condition must be <x, <=x, >x, >=x, =x, a..b or else")
            })?;
            rules.push(Rule {
                lo,
                hi,
                status,
                text: cond.to_string(),
            });
            if rules.len() > 64 {
                return Err("::THRESHOLDS has more than 64 rules".into());
            }
        }
        if rules.is_empty() {
            return Err("::THRESHOLDS is empty — e.g. '<0:bad; <0.05:warn; good'".into());
        }
        Ok(Rules { rules })
    }

    /// The first matching rule (non-finite values have no status).
    pub fn rule_for(&self, v: f64) -> Option<&Rule> {
        if !v.is_finite() {
            return None;
        }
        self.rules.iter().find(|r| r.matches(v))
    }

    /// The status of `v`: the first matching rule's.
    pub fn status(&self, v: f64) -> Option<Status> {
        self.rule_for(v).map(|r| r.status)
    }

    /// Every finite rule bound, sorted and deduplicated.
    fn cuts(&self) -> Vec<f64> {
        let mut c: Vec<f64> = self
            .rules
            .iter()
            .flat_map(|r| [r.lo, r.hi])
            .flatten()
            .map(|(b, _)| b)
            .collect();
        c.sort_by(f64::total_cmp);
        c.dedup();
        c
    }

    /// Where the status changes: each bound whose two sides differ, with the
    /// worse of the two statuses (the line marks entering the worse zone).
    pub fn boundaries(&self) -> Vec<(f64, Status)> {
        let side = |v: f64| self.status(v).unwrap_or(Status::Neutral);
        self.cuts()
            .into_iter()
            .filter_map(|c| {
                let d = 1e-9 * c.abs().max(1.0);
                let (a, b) = (side(c - d), side(c + d));
                (a != b).then(|| (c, if a.severity() >= b.severity() { a } else { b }))
            })
            .collect()
    }

    /// Contiguous zones over `[min, max]` (a gauge's range): `(from, to,
    /// status)`, neighbours of equal status merged; values without a rule are
    /// neutral.
    pub fn zones(&self, min: f64, max: f64) -> Vec<(f64, f64, Status)> {
        let mut edges = vec![min];
        edges.extend(self.cuts().into_iter().filter(|c| *c > min && *c < max));
        edges.push(max);
        let mut out: Vec<(f64, f64, Status)> = Vec::new();
        for w in edges.windows(2) {
            let s = self.status((w[0] + w[1]) / 2.0).unwrap_or(Status::Neutral);
            match out.last_mut() {
                Some(last) if last.2 == s => last.1 = w[1],
                _ => out.push((w[0], w[1], s)),
            }
        }
        out
    }

    pub fn to_json(&self) -> serde_json::Value {
        let bound = |b: Option<(f64, bool)>| match b {
            Some((x, inc)) => serde_json::json!([x, inc]),
            None => serde_json::Value::Null,
        };
        serde_json::Value::Array(
            self.rules
                .iter()
                .map(|r| {
                    serde_json::json!({
                        "lo": bound(r.lo), "hi": bound(r.hi),
                        "status": r.status.as_str(), "text": r.text,
                    })
                })
                .collect(),
        )
    }
}

/// A table directive: `'Margin %: <10:bad; good | accuracy: <0.85:bad'` →
/// `(column, rules)` per `|` group. A group without a column name has `None`.
pub fn parse_table(spec: &str) -> Result<Vec<(Option<String>, Rules)>, String> {
    let mut out = Vec::new();
    for group in spec.split('|').map(str::trim).filter(|g| !g.is_empty()) {
        let named = group.split_once(':').filter(|(name, _)| {
            let n = name.trim();
            !n.is_empty() && condition(n).is_none() && Status::parse(n).is_none()
        });
        match named {
            Some((name, rules)) => out.push((Some(name.trim().to_string()), Rules::parse(rules)?)),
            None => out.push((None, Rules::parse(group)?)),
        }
    }
    if out.is_empty() {
        return Err("::THRESHOLDS is empty — e.g. 'Margin %: <10:bad; <15:warn; good'".into());
    }
    Ok(out)
}

/// `parse_table` as JSON for the browser: `{"ok":true,"groups":[{"column":…,
/// "rules":[{"lo":[x,incl]|null,"hi":…,"status":"good","text":"<0"}]}]}` or
/// `{"ok":false,"error":"…"}`.
pub fn table_json(spec: &str) -> String {
    match parse_table(spec) {
        Ok(groups) => serde_json::json!({
            "ok": true,
            "groups": groups.iter().map(|(c, r)| serde_json::json!({"column": c, "rules": r.to_json()})).collect::<Vec<_>>(),
        }),
        Err(e) => serde_json::json!({"ok": false, "error": e}),
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_match_wins() {
        let r = Rules::parse("<0:bad; 0..0.05:warn; >0.05:good").unwrap();
        assert_eq!(r.status(-1.0), Some(Status::Bad));
        assert_eq!(r.status(0.0), Some(Status::Warn));
        assert_eq!(r.status(0.05), Some(Status::Warn));
        assert_eq!(r.status(0.2), Some(Status::Good));
        assert_eq!(r.status(f64::NAN), None);
        let r = Rules::parse("<80:bad, <95:warn, else:good").unwrap();
        assert_eq!(r.status(79.9), Some(Status::Bad));
        assert_eq!(r.status(80.0), Some(Status::Warn));
        assert_eq!(r.status(120.0), Some(Status::Good));
    }

    #[test]
    fn suffixes_aliases_and_bare_status() {
        let r = Rules::parse(">=1.2M:ok; >=900k:warning; critical").unwrap();
        assert_eq!(r.status(1.3e6), Some(Status::Good));
        assert_eq!(r.status(1.0e6), Some(Status::Warn));
        assert_eq!(r.status(1.0), Some(Status::Bad));
        let r = Rules::parse("<5%:good; bad").unwrap();
        assert_eq!(r.status(0.04), Some(Status::Good));
        assert_eq!(r.status(0.06), Some(Status::Bad));
    }

    #[test]
    fn bad_rules_are_errors() {
        assert!(Rules::parse("").is_err());
        assert!(Rules::parse("<0:purple").is_err());
        assert!(Rules::parse("around 5:good").is_err());
    }

    #[test]
    fn boundaries_and_zones() {
        let r = Rules::parse("<50:bad; <80:warn; good").unwrap();
        assert_eq!(
            r.boundaries(),
            vec![(50.0, Status::Bad), (80.0, Status::Warn)]
        );
        assert_eq!(
            r.zones(0.0, 100.0),
            vec![
                (0.0, 50.0, Status::Bad),
                (50.0, 80.0, Status::Warn),
                (80.0, 100.0, Status::Good)
            ]
        );
        // A cut outside the range doesn't split it.
        assert_eq!(r.zones(0.0, 40.0), vec![(0.0, 40.0, Status::Bad)]);
    }

    #[test]
    fn table_groups_name_their_columns() {
        let g = parse_table("Margin %: <10:bad; <15:warn; good | accuracy: <0.85:bad").unwrap();
        assert_eq!(g.len(), 2);
        assert_eq!(g[0].0.as_deref(), Some("Margin %"));
        assert_eq!(g[0].1.status(12.0), Some(Status::Warn));
        assert_eq!(g[1].0.as_deref(), Some("accuracy"));
        // No name: the rules alone.
        let g = parse_table("<0:bad; good").unwrap();
        assert_eq!(g[0].0, None);
        assert!(table_json("x: <0:purple").contains("\"ok\":false"));
    }
}
