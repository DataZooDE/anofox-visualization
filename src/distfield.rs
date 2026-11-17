//! A `::FORM` distribution field (#1235): a family and its parameters, as
//! one JSON object (`{"family": "Beta", "a": 5.2, "b": 9.8}`), entered either
//! as the parameters or as a likely value and a 90 % range ("most likely
//! 30 %, surely between 10 and 60 %"). The same code checks it, turns a range
//! into parameters and draws its preview in the browser (wasm), the servers
//! and `dashboard --check`.
//!
//! Parameter names follow common usage, so the object can go to an
//! inference library or SQL as it is: Normal `mean`/`variance`, LogNormal
//! `meanlog`/`sdlog`, Gamma `shape`/`rate`, Beta `a`/`b`, Uniform `lo`/`hi`.
use ggplot_rs::stat::distribution as d;
use serde_json::{json, Value as Json};

/// The families a field offers by default, in this order.
pub const FAMILIES: &[&str] = &["Normal", "LogNormal", "Beta", "Gamma", "Uniform"];

/// The tail mass outside a "90 % range" on each side.
const TAIL: f64 = 0.05;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    Normal,
    LogNormal,
    Beta,
    Gamma,
    Uniform,
}

impl Family {
    pub fn parse(s: &str) -> Option<Family> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "normal" | "gaussian" => Family::Normal,
            "lognormal" | "log-normal" | "lnorm" => Family::LogNormal,
            "beta" => Family::Beta,
            "gamma" => Family::Gamma,
            "uniform" => Family::Uniform,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Family::Normal => "Normal",
            Family::LogNormal => "LogNormal",
            Family::Beta => "Beta",
            Family::Gamma => "Gamma",
            Family::Uniform => "Uniform",
        }
    }

    /// The two parameters' names, in the order of `Dist::p`.
    pub fn params(self) -> [&'static str; 2] {
        match self {
            Family::Normal => ["mean", "variance"],
            Family::LogNormal => ["meanlog", "sdlog"],
            Family::Beta => ["a", "b"],
            Family::Gamma => ["shape", "rate"],
            Family::Uniform => ["lo", "hi"],
        }
    }
}

/// A distribution: a family and its two parameters.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Dist {
    pub family: Family,
    pub p: [f64; 2],
}

/// What a distribution says in the "likely value and 90 % range" view.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Summary {
    /// The most likely value (the mode; a Uniform's middle).
    pub likely: f64,
    pub lo: f64,
    pub hi: f64,
}

impl Dist {
    /// From the field's JSON object or a text such as `'Beta(2, 3)'`
    /// (positional, in [`Family::params`] order, or `name = value`).
    pub fn parse(v: &Json) -> Result<Dist, String> {
        match v {
            Json::Object(o) => {
                let fam = o
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case("family"))
                    .and_then(|(_, v)| v.as_str())
                    .ok_or("a distribution needs a \"family\"")?;
                let family = Family::parse(fam).ok_or_else(|| unknown(fam))?;
                let mut p = [f64::NAN; 2];
                for (i, name) in family.params().iter().enumerate() {
                    p[i] = o
                        .iter()
                        .find(|(k, _)| k.eq_ignore_ascii_case(name))
                        .and_then(|(_, v)| {
                            v.as_f64()
                                .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
                        })
                        .ok_or_else(|| format!("{}: missing \"{name}\"", family.name()))?;
                }
                Dist::new(family, p)
            }
            Json::String(s) => Dist::parse_text(s),
            _ => Err(
                "a distribution is an object {\"family\": …} or a text like 'Beta(2, 3)'".into(),
            ),
        }
    }

    fn parse_text(s: &str) -> Result<Dist, String> {
        let s = s.trim();
        if s.starts_with('{') {
            let v: Json =
                serde_json::from_str(s).map_err(|e| format!("not a distribution: {e}"))?;
            return Dist::parse(&v);
        }
        let (fam, rest) = s
            .split_once('(')
            .ok_or_else(|| format!("'{s}' is no distribution, e.g. 'Beta(2, 3)'"))?;
        let family = Family::parse(fam).ok_or_else(|| unknown(fam))?;
        let args = rest
            .trim_end()
            .strip_suffix(')')
            .ok_or_else(|| format!("'{s}': missing ')'"))?;
        let mut p = [f64::NAN; 2];
        for (i, arg) in args.split(',').enumerate() {
            let (name, val) = match arg.split_once('=') {
                Some((n, v)) => (Some(n.trim()), v.trim()),
                None => (None, arg.trim()),
            };
            let k = match name {
                Some(n) => family
                    .params()
                    .iter()
                    .position(|x| x.eq_ignore_ascii_case(n))
                    .ok_or_else(|| {
                        format!(
                            "{}: no parameter '{n}' (it takes {})",
                            family.name(),
                            family.params().join(", ")
                        )
                    })?,
                None if i < 2 => i,
                None => return Err(format!("{} takes two parameters", family.name())),
            };
            p[k] = val
                .parse()
                .map_err(|_| format!("'{s}': '{val}' is no number"))?;
        }
        Dist::new(family, p)
    }

    /// Checked: each family's parameter domain.
    pub fn new(family: Family, p: [f64; 2]) -> Result<Dist, String> {
        let [x, y] = p;
        let [n0, n1] = family.params();
        if !x.is_finite() || !y.is_finite() {
            return Err(format!(
                "{}: \"{n0}\" and \"{n1}\" must be numbers",
                family.name()
            ));
        }
        let bad = match family {
            Family::Normal => (y <= 0.0).then(|| format!("Normal: variance must be > 0 (got {y})")),
            Family::LogNormal => {
                (y <= 0.0).then(|| format!("LogNormal: sdlog must be > 0 (got {y})"))
            }
            Family::Beta => {
                (x <= 0.0 || y <= 0.0).then(|| format!("Beta: a and b must be > 0 (got {x}, {y})"))
            }
            Family::Gamma => (x <= 0.0 || y <= 0.0)
                .then(|| format!("Gamma: shape and rate must be > 0 (got {x}, {y})")),
            Family::Uniform => (x >= y).then(|| format!("Uniform: lo must be < hi (got {x}, {y})")),
        };
        match bad {
            Some(e) => Err(e),
            None => Ok(Dist { family, p }),
        }
    }

    /// The field's JSON value.
    pub fn to_json(&self) -> Json {
        let [n0, n1] = self.family.params();
        json!({"family": self.family.name(), n0: round_sig(self.p[0]), n1: round_sig(self.p[1])})
    }

    /// `'Beta(5.2, 9.8)'`.
    pub fn text(&self) -> String {
        format!(
            "{}({}, {})",
            self.family.name(),
            round_sig(self.p[0]),
            round_sig(self.p[1])
        )
    }

    pub fn quantile(&self, q: f64) -> f64 {
        let [x, y] = self.p;
        match self.family {
            Family::Normal => d::qnorm_ms(q, x, y.sqrt()),
            Family::LogNormal => d::qlnorm(q, x, y),
            Family::Beta => d::qbeta(q, x, y),
            Family::Gamma => d::qgamma(q, x, y),
            Family::Uniform => x + q * (y - x),
        }
    }

    pub fn density(&self, v: f64) -> f64 {
        let [x, y] = self.p;
        match self.family {
            Family::Normal => d::dnorm_ms(v, x, y.sqrt()),
            Family::LogNormal => d::dlnorm(v, x, y),
            Family::Beta => d::dbeta(v, x, y),
            Family::Gamma => d::dgamma(v, x, y),
            Family::Uniform => {
                if (x..=y).contains(&v) {
                    1.0 / (y - x)
                } else {
                    0.0
                }
            }
        }
    }

    /// The mode (a Uniform's middle; a U-shaped or J-shaped Beta / Gamma:
    /// the edge its density rises to).
    pub fn mode(&self) -> f64 {
        let [x, y] = self.p;
        match self.family {
            Family::Normal => x,
            Family::LogNormal => (x - y * y).exp(),
            Family::Beta => {
                if x > 1.0 && y > 1.0 {
                    (x - 1.0) / (x + y - 2.0)
                } else if x <= 1.0 && y > 1.0 {
                    0.0
                } else if x > 1.0 && y <= 1.0 {
                    1.0
                } else {
                    0.5
                }
            }
            Family::Gamma => ((x - 1.0) / y).max(0.0),
            Family::Uniform => 0.5 * (x + y),
        }
    }

    /// The likely value and the central 90 % range.
    pub fn summary(&self) -> Summary {
        if self.family == Family::Uniform {
            return Summary {
                likely: self.mode(),
                lo: self.p[0],
                hi: self.p[1],
            };
        }
        Summary {
            likely: self.mode(),
            lo: self.quantile(TAIL),
            hi: self.quantile(1.0 - TAIL),
        }
    }

    /// `n` points of the density over its central 99.8 % (a Uniform: just
    /// beyond its ends), for the field's preview.
    pub fn curve(&self, n: usize) -> Vec<(f64, f64)> {
        let (a, b) = match self.family {
            Family::Uniform => {
                let w = self.p[1] - self.p[0];
                (self.p[0] - 0.05 * w, self.p[1] + 0.05 * w)
            }
            _ => (self.quantile(0.001), self.quantile(0.999)),
        };
        let n = n.max(2);
        (0..n)
            .map(|i| {
                let v = a + (b - a) * i as f64 / (n - 1) as f64;
                let y = self.density(v);
                (v, if y.is_finite() { y } else { f64::NAN })
            })
            .collect()
    }
}

/// How a range was turned into parameters: the distribution and how far it
/// misses what was asked (the largest gap of likely value, 5 % and 95 %
/// quantile, as a share of the range's width).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fit {
    pub dist: Dist,
    pub miss: f64,
}

/// A fit that misses by more than this share of the range is refused.
pub const MAX_MISS: f64 = 0.1;

/// "Most likely `likely`, 90 % sure between `lo` and `hi`" → the family's
/// parameters. Normal: the likely value as its mean, the range as ± 1.645
/// sd (a lopsided range does not fit); Uniform: the range as its ends;
/// LogNormal, Gamma, Beta: the parameters whose mode and 5 % / 95 %
/// quantiles come closest (least squares, Nelder–Mead on log-parameters).
/// An error when the input is inconsistent or no member of the family comes
/// within [`MAX_MISS`] of it.
pub fn from_range(family: Family, likely: f64, lo: f64, hi: f64) -> Result<Fit, String> {
    if !(likely.is_finite() && lo.is_finite() && hi.is_finite()) {
        return Err("the likely value and the range must be numbers".into());
    }
    if lo >= hi {
        return Err(format!(
            "the range's low end ({lo}) must be below its high end ({hi})"
        ));
    }
    if family != Family::Uniform && !(lo < likely && likely < hi) {
        return Err(format!(
            "the likely value ({likely}) must lie inside the range ({lo} – {hi})"
        ));
    }
    let w = hi - lo;
    let z = d::qnorm(1.0 - TAIL);
    let dist = match family {
        Family::Uniform => Dist::new(Family::Uniform, [lo, hi])?,
        Family::Normal => Dist::new(Family::Normal, [likely, (w / (2.0 * z)).powi(2)])?,
        Family::LogNormal | Family::Gamma | Family::Beta => {
            match family {
                Family::Beta if !(lo >= 0.0 && hi <= 1.0) => {
                    return Err(format!("a Beta lives in 0 – 1; the range {lo} – {hi} is outside (enter a share as 0.3, not 30)"))
                }
                Family::LogNormal | Family::Gamma if lo <= 0.0 => {
                    return Err(format!("a {} is positive; the range must start above 0 (got {lo})", family.name()))
                }
                _ => {}
            }
            // a start from the log-scale normal approximation
            let s = ((hi / lo).ln() / (2.0 * z)).max(1e-3);
            let start = match family {
                Family::LogNormal => [likely.ln() + s * s, s.ln()],
                Family::Gamma => {
                    let k = (1.0 / (s * s)).max(1.05);
                    [k.ln(), ((k - 1.0) / likely).max(1e-9).ln()]
                }
                _ => {
                    // Beta: mean ≈ likely, concentration from the range
                    let m = likely.clamp(1e-3, 1.0 - 1e-3);
                    let var = (w / (2.0 * z)).powi(2);
                    let c = (m * (1.0 - m) / var - 1.0).max(2.5);
                    [(m * c).ln(), ((1.0 - m) * c).ln()]
                }
            };
            let make = |t: [f64; 2]| -> Option<Dist> {
                let p = match family {
                    Family::LogNormal => [t[0], t[1].exp()],
                    _ => [t[0].exp(), t[1].exp()],
                };
                Dist::new(family, p).ok()
            };
            let loss = |t: [f64; 2]| -> f64 {
                let Some(dd) = make(t) else {
                    return f64::INFINITY;
                };
                let r = [
                    (dd.quantile(TAIL) - lo) / w,
                    (dd.quantile(1.0 - TAIL) - hi) / w,
                    (dd.mode() - likely) / w,
                ];
                r.iter().map(|x| x * x).sum::<f64>()
            };
            let best = nelder_mead(loss, start);
            make(best).ok_or("no fit")?
        }
    };
    let sm = dist.summary();
    let miss = if family == Family::Uniform {
        0.0
    } else {
        [
            (sm.lo - lo).abs(),
            (sm.hi - hi).abs(),
            (sm.likely - likely).abs(),
        ]
        .into_iter()
        .fold(0.0, f64::max)
            / w
    };
    if miss > MAX_MISS {
        return Err(format!(
            "no {} has its most likely value at {likely} and 90 % between {lo} and {hi}; the closest, {}, is most likely {} and 90 % between {} and {}",
            family.name(),
            dist.text(),
            round_sig(sm.likely),
            round_sig(sm.lo),
            round_sig(sm.hi)
        ));
    }
    Ok(Fit { dist, miss })
}

/// Minimise `f` over two parameters from `x0` (Nelder–Mead, standard
/// coefficients, at most 2000 steps).
fn nelder_mead(f: impl Fn([f64; 2]) -> f64, x0: [f64; 2]) -> [f64; 2] {
    let mut s = [x0, [x0[0] + 0.5, x0[1]], [x0[0], x0[1] + 0.5]];
    let mut v = s.map(&f);
    for _ in 0..2000 {
        let mut idx = [0, 1, 2];
        idx.sort_by(|&a, &b| v[a].total_cmp(&v[b]));
        let (b, m, w) = (idx[0], idx[1], idx[2]);
        if (v[w] - v[b]).abs() < 1e-16
            && (s[w][0] - s[b][0]).abs() + (s[w][1] - s[b][1]).abs() < 1e-10
        {
            break;
        }
        let c = [(s[b][0] + s[m][0]) / 2.0, (s[b][1] + s[m][1]) / 2.0];
        let at = |t: f64| [c[0] + t * (s[w][0] - c[0]), c[1] + t * (s[w][1] - c[1])];
        let xr = at(-1.0);
        let fr = f(xr);
        if fr < v[b] {
            let xe = at(-2.0);
            let fe = f(xe);
            (s[w], v[w]) = if fe < fr { (xe, fe) } else { (xr, fr) };
        } else if fr < v[m] {
            (s[w], v[w]) = (xr, fr);
        } else {
            let xc = if fr < v[w] { at(-0.5) } else { at(0.5) };
            let fc = f(xc);
            if fc < v[w].min(fr) {
                (s[w], v[w]) = (xc, fc);
            } else {
                for i in [m, w] {
                    s[i] = [(s[i][0] + s[b][0]) / 2.0, (s[i][1] + s[b][1]) / 2.0];
                    v[i] = f(s[i]);
                }
            }
        }
    }
    let i = (0..3).min_by(|&a, &b| v[a].total_cmp(&v[b])).unwrap_or(0);
    s[i]
}

fn unknown(fam: &str) -> String {
    format!(
        "'{}' is no distribution family ({})",
        fam.trim(),
        FAMILIES.join(", ")
    )
}

/// Four significant digits: what the field shows and stores.
fn round_sig(x: f64) -> f64 {
    if x == 0.0 || !x.is_finite() {
        return x;
    }
    let k = 10f64.powi(3 - x.abs().log10().floor() as i32);
    (x * k).round() / k
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_and_text_round_trip() {
        let d = Dist::parse(&json!({"family": "Beta", "a": 5.2, "b": 9.8})).unwrap();
        assert_eq!(
            d,
            Dist {
                family: Family::Beta,
                p: [5.2, 9.8]
            }
        );
        assert_eq!(Dist::parse(&d.to_json()).unwrap(), d);
        assert_eq!(Dist::parse(&json!(d.text())).unwrap(), d);
        assert_eq!(
            Dist::parse(&json!("gamma(rate = 2, shape = 3)")).unwrap(),
            Dist {
                family: Family::Gamma,
                p: [3.0, 2.0]
            }
        );
        assert_eq!(
            Dist::parse(&json!(
                "{\"family\":\"Normal\",\"mean\":1,\"variance\":\"4\"}"
            ))
            .unwrap()
            .p,
            [1.0, 4.0]
        );
    }

    #[test]
    fn domains_are_checked() {
        for (v, e) in [
            (
                json!({"family": "Beta", "a": 0, "b": 2}),
                "a and b must be > 0",
            ),
            (
                json!({"family": "Normal", "mean": 0, "variance": -1}),
                "variance must be > 0",
            ),
            (
                json!({"family": "Uniform", "lo": 3, "hi": 1}),
                "lo must be < hi",
            ),
            (
                json!({"family": "Weibull", "k": 1}),
                "no distribution family",
            ),
            (json!({"family": "Gamma", "shape": 2}), "missing \"rate\""),
            (json!("Beta(2, 3, 4)"), "takes two parameters"),
        ] {
            let err = Dist::parse(&v).unwrap_err();
            assert!(err.contains(e), "{v}: {err}");
        }
    }

    /// A distribution's own summary turned back into parameters gives it back.
    #[test]
    fn range_round_trips() {
        for d in [
            Dist {
                family: Family::Normal,
                p: [10.0, 4.0],
            },
            Dist {
                family: Family::LogNormal,
                p: [1.0, 0.4],
            },
            Dist {
                family: Family::Gamma,
                p: [3.0, 0.5],
            },
            Dist {
                family: Family::Gamma,
                p: [12.0, 4.0],
            },
            Dist {
                family: Family::Beta,
                p: [2.0, 3.0],
            },
            Dist {
                family: Family::Beta,
                p: [30.0, 70.0],
            },
            Dist {
                family: Family::Uniform,
                p: [2.0, 5.0],
            },
        ] {
            let s = d.summary();
            let fit = from_range(d.family, s.likely, s.lo, s.hi).unwrap();
            assert!(fit.miss < 1e-4, "{d:?}: miss {}", fit.miss);
            for i in 0..2 {
                let rel = (fit.dist.p[i] - d.p[i]).abs() / d.p[i].abs().max(1e-9);
                assert!(rel < 1e-3, "{d:?} → {:?}", fit.dist);
            }
        }
    }

    #[test]
    fn a_share_elicited_as_a_beta() {
        // "most likely 30 %, surely between 10 and 60 %"
        let fit = from_range(Family::Beta, 0.3, 0.1, 0.6).unwrap();
        let s = fit.dist.summary();
        assert!(
            fit.miss <= MAX_MISS && (s.likely - 0.3).abs() < 0.05,
            "{fit:?} {s:?}"
        );
        assert!(from_range(Family::Beta, 30.0, 10.0, 60.0)
            .unwrap_err()
            .contains("0.3, not 30"));
    }

    #[test]
    fn inconsistent_ranges_are_refused() {
        assert!(from_range(Family::Normal, 5.0, 6.0, 9.0)
            .unwrap_err()
            .contains("inside the range"));
        assert!(from_range(Family::Gamma, 1.0, -1.0, 4.0)
            .unwrap_err()
            .contains("above 0"));
        // most likely at the bottom of a wide range: no Normal does that
        let e = from_range(Family::Normal, 1.0, 0.9, 10.0).unwrap_err();
        assert!(e.contains("no Normal") && e.contains("closest"), "{e}");
    }

    #[test]
    fn the_curve_spans_the_distribution() {
        let c = Dist {
            family: Family::Gamma,
            p: [3.0, 2.0],
        }
        .curve(50);
        assert_eq!(c.len(), 50);
        assert!(c
            .iter()
            .all(|&(x, y)| x >= 0.0 && y.is_finite() && y >= 0.0));
        let peak = c
            .iter()
            .cloned()
            .fold((0.0, 0.0), |a, b| if b.1 > a.1 { b } else { a });
        assert!((peak.0 - 1.0).abs() < 0.2, "mode near (3-1)/2: {peak:?}");
    }
}
