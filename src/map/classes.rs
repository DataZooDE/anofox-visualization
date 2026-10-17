//! Class breaks for a map's measure (`::CLASSES`): quantile, Fisher-Jenks
//! natural breaks, equal width or custom edges. Classes are half-open
//! `[a, b)` (the last one closed), and each knows how many values fell in.

/// A `::CLASSES` spec.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Method {
    Quantile(usize),
    Jenks(usize),
    Equal(usize),
    /// Custom edges, increasing: `'0,10,50,100'` (≥ 2 numbers).
    Edges(Vec<f64>),
}

/// The class counts a method may ask for.
pub(crate) const MIN_CLASSES: usize = 2;
pub(crate) const MAX_CLASSES: usize = 9;
/// Above this many values Jenks runs on an even sample of the sorted values.
const JENKS_SAMPLE: usize = 2_000;

impl Method {
    /// Parse `'quantile 5'`, `'jenks 5'`, `'equal 5'` (the count defaults to
    /// 5) or `'0,10,50,100'`. The error says what is wrong.
    pub(crate) fn parse(spec: &str) -> Result<Method, String> {
        let s = spec.trim().to_ascii_lowercase();
        let first = s.chars().next().unwrap_or(' ');
        if first.is_ascii_digit() || first == '-' || first == '.' || first == '+' {
            let edges = s
                .split([',', ';'])
                .map(|t| t.trim().parse::<f64>().ok().filter(|f| f.is_finite()))
                .collect::<Option<Vec<f64>>>()
                .ok_or_else(|| format!("::CLASSES '{spec}': edges must be numbers"))?;
            if edges.len() < 2 {
                return Err(format!("::CLASSES '{spec}': give at least two edges"));
            }
            if edges.windows(2).any(|w| w[1] <= w[0]) {
                return Err(format!("::CLASSES '{spec}': edges must increase"));
            }
            if edges.len() - 1 > MAX_CLASSES {
                return Err(format!("::CLASSES '{spec}': at most {MAX_CLASSES} classes"));
            }
            return Ok(Method::Edges(edges));
        }
        let mut words = s.split_whitespace();
        let name = words.next().unwrap_or("");
        let n = match words.next() {
            None => 5,
            Some(w) => w
                .parse::<usize>()
                .map_err(|_| format!("::CLASSES '{spec}': the count must be a whole number"))?,
        };
        if words.next().is_some() {
            return Err(format!(
                "::CLASSES '{spec}': write a method and a count, e.g. 'quantile 5'"
            ));
        }
        if !(MIN_CLASSES..=MAX_CLASSES).contains(&n) {
            return Err(format!(
                "::CLASSES '{spec}': the count must be {MIN_CLASSES} to {MAX_CLASSES}"
            ));
        }
        match name {
            "quantile" | "quantiles" => Ok(Method::Quantile(n)),
            "jenks" | "natural" | "natural-breaks" => Ok(Method::Jenks(n)),
            "equal" | "equal-interval" | "equal-width" => Ok(Method::Equal(n)),
            _ => Err(format!(
                "::CLASSES '{spec}': not 'quantile n', 'jenks n', 'equal n' or edges like '0,10,50'"
            )),
        }
    }

    /// How a summary names the method ("quantile").
    pub(crate) fn name(&self) -> &'static str {
        match self {
            Method::Quantile(_) => "quantile",
            Method::Jenks(_) => "natural-breaks",
            Method::Equal(_) => "equal-width",
            Method::Edges(_) => "custom",
        }
    }
}

/// Classes over a set of values.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Classes {
    /// Lower and upper end of the classed range.
    pub lo: f64,
    pub hi: f64,
    /// The interior edges, increasing: class `i` is `[edge[i-1], edge[i])`.
    pub edges: Vec<f64>,
    /// Values fell below `lo` / above `hi` (custom edges, a `::RANGE` cap):
    /// the end classes are open.
    pub open_lo: bool,
    pub open_hi: bool,
    /// Values per class.
    pub counts: Vec<usize>,
}

impl Classes {
    /// Classes of `values` (finite ones; none → `None`) by `m`.
    pub(crate) fn build(m: &Method, values: &[f64]) -> Option<Classes> {
        let mut v: Vec<f64> = values.iter().copied().filter(|f| f.is_finite()).collect();
        if v.is_empty() {
            return None;
        }
        v.sort_by(f64::total_cmp);
        let (min, max) = (v[0], v[v.len() - 1]);
        let (lo, hi, mut edges) = match m {
            Method::Edges(e) => (e[0], e[e.len() - 1], e[1..e.len() - 1].to_vec()),
            Method::Equal(n) => {
                let w = (max - min) / *n as f64;
                let edges = if w > 0.0 {
                    (1..*n).map(|i| min + w * i as f64).collect()
                } else {
                    Vec::new()
                };
                (min, max, edges)
            }
            Method::Quantile(n) => {
                let edges = (1..*n).map(|i| v[i * v.len() / n]).collect();
                (min, max, edges)
            }
            Method::Jenks(n) => (min, max, jenks(&v, *n)),
        };
        // Equal edges (ties) collapse; an edge on the low end opens no class.
        edges.dedup_by(|a, b| a == b);
        edges.retain(|e| *e > lo && *e <= hi);
        let mut c = Classes {
            lo,
            hi,
            edges,
            open_lo: min < lo,
            open_hi: max > hi,
            counts: Vec::new(),
        };
        c.counts = vec![0; c.len()];
        for x in &v {
            let i = c.index(*x);
            c.counts[i] += 1;
        }
        Some(c)
    }

    /// The number of classes.
    pub(crate) fn len(&self) -> usize {
        self.edges.len() + 1
    }

    /// The class of `v` (values beyond the ends go to the end classes).
    pub(crate) fn index(&self, v: f64) -> usize {
        self.edges.partition_point(|e| *e <= v)
    }

    /// The bounds `[a, b)` of class `i`.
    pub(crate) fn bounds(&self, i: usize) -> (f64, f64) {
        let a = if i == 0 { self.lo } else { self.edges[i - 1] };
        let b = if i + 1 >= self.len() {
            self.hi
        } else {
            self.edges[i]
        };
        (a, b)
    }

    /// Class `i` as text, each bound through `fmt`: `"10 – 50"`, an open
    /// first class `"< 10"`, an open last class `"≥ 50"`.
    pub(crate) fn label(&self, i: usize, fmt: &dyn Fn(f64) -> String) -> String {
        let (a, b) = self.bounds(i);
        let last = i + 1 == self.len();
        if i == 0 && self.open_lo && !last {
            format!("< {}", fmt(b))
        } else if last && self.open_hi && i > 0 {
            format!("≥ {}", fmt(a))
        } else {
            format!("{} – {}", fmt(a), fmt(b))
        }
    }
}

/// Fisher-Jenks natural breaks of sorted `v` into (at most) `k` classes: the
/// edges (the first value of classes 2..k) that minimise the summed squared
/// deviations within classes. Dynamic programming, O(k·n²) on at most
/// [`JENKS_SAMPLE`] values (an even, deterministic sample of `v`).
pub(crate) fn jenks(v: &[f64], k: usize) -> Vec<f64> {
    let data: Vec<f64> = if v.len() > JENKS_SAMPLE {
        let step = v.len() as f64 / JENKS_SAMPLE as f64;
        (0..JENKS_SAMPLE)
            .map(|i| v[((i as f64 * step) as usize).min(v.len() - 1)])
            .chain(std::iter::once(v[v.len() - 1]))
            .collect()
    } else {
        v.to_vec()
    };
    let n = data.len();
    let k = k.min(n);
    if k < 2 {
        return Vec::new();
    }
    // lower[l][j]: the 1-based start of the last class when the first l
    // values form j classes; cost[l][j]: that partition's squared deviations.
    let mut lower = vec![vec![0usize; k + 1]; n + 1];
    let mut cost = vec![vec![f64::INFINITY; k + 1]; n + 1];
    for j in 1..=k {
        lower[1][j] = 1;
        cost[1][j] = 0.0;
    }
    for l in 2..=n {
        let (mut s1, mut s2, mut w) = (0.0, 0.0, 0.0);
        for m in 1..=l {
            let i3 = l - m + 1;
            let x = data[i3 - 1];
            s1 += x;
            s2 += x * x;
            w += 1.0;
            let var = s2 - s1 * s1 / w;
            let i4 = i3 - 1;
            if i4 != 0 {
                for j in 2..=k {
                    let c = var + cost[i4][j - 1];
                    if cost[l][j] >= c {
                        lower[l][j] = i3;
                        cost[l][j] = c;
                    }
                }
            }
            if m == l {
                lower[l][1] = 1;
                cost[l][1] = var;
            }
        }
    }
    let mut edges = Vec::with_capacity(k - 1);
    let mut end = n;
    for j in (2..=k).rev() {
        let start = lower[end][j];
        if start < 2 {
            break;
        }
        edges.push(data[start - 1]);
        end = start - 1;
    }
    edges.reverse();
    edges
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_specs() {
        assert_eq!(Method::parse("quantile 5"), Ok(Method::Quantile(5)));
        assert_eq!(Method::parse(" Jenks 4 "), Ok(Method::Jenks(4)));
        assert_eq!(Method::parse("equal"), Ok(Method::Equal(5)));
        assert_eq!(
            Method::parse("0, 10,50,100"),
            Ok(Method::Edges(vec![0.0, 10.0, 50.0, 100.0]))
        );
        for bad in [
            "quantile 1",
            "jenks 12",
            "kmeans 5",
            "0,10,5",
            "7",
            "equal five",
            "",
        ] {
            assert!(Method::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn quantile_classes_hold_equal_counts() {
        let v: Vec<f64> = (1..=100).map(f64::from).collect();
        let c = Classes::build(&Method::Quantile(4), &v).unwrap();
        assert_eq!(c.edges, vec![26.0, 51.0, 76.0]);
        assert_eq!(c.counts, vec![25, 25, 25, 25]);
        // ties collapse into fewer classes
        let c = Classes::build(&Method::Quantile(5), &[1.0, 1.0, 1.0, 1.0, 2.0]).unwrap();
        assert_eq!(c.len(), 2);
        assert_eq!(c.counts, vec![4, 1]);
    }

    #[test]
    fn equal_and_custom_edges() {
        let c = Classes::build(&Method::Equal(4), &[0.0, 1.0, 5.0, 10.0]).unwrap();
        assert_eq!(c.edges, vec![2.5, 5.0, 7.5]);
        assert_eq!(c.counts, vec![2, 0, 1, 1]);
        let fmt = |f: f64| format!("{f}");
        assert_eq!(c.label(1, &fmt), "2.5 – 5");
        // values outside custom edges go to the open end classes
        let m = Method::Edges(vec![0.0, 10.0, 50.0, 100.0]);
        let c = Classes::build(&m, &[-5.0, 3.0, 20.0, 150.0]).unwrap();
        assert_eq!(c.counts, vec![2, 1, 1]);
        assert!(c.open_lo && c.open_hi);
        assert_eq!(c.label(0, &fmt), "< 10");
        assert_eq!(c.label(2, &fmt), "≥ 50");
        assert_eq!(c.index(100.0), 2);
        // a constant measure: one class
        let c = Classes::build(&Method::Equal(5), &[3.0, 3.0]).unwrap();
        assert_eq!((c.len(), c.counts.clone()), (1, vec![2]));
        assert!(Classes::build(&Method::Equal(5), &[f64::NAN]).is_none());
    }

    /// The summed squared deviations within the classes cut at `edges`.
    fn sdcm(v: &[f64], edges: &[f64]) -> f64 {
        let mut groups: Vec<Vec<f64>> = vec![Vec::new(); edges.len() + 1];
        for x in v {
            groups[edges.partition_point(|e| e <= x)].push(*x);
        }
        groups
            .iter()
            .filter(|g| !g.is_empty())
            .map(|g| {
                let m = g.iter().sum::<f64>() / g.len() as f64;
                g.iter().map(|x| (x - m) * (x - m)).sum::<f64>()
            })
            .sum()
    }

    /// Jenks finds the optimum an exhaustive search over every cut finds.
    #[test]
    fn jenks_is_optimal() {
        let mut seed = 7u64;
        for case in 0..30 {
            let n = 6 + case % 7;
            let mut v: Vec<f64> = (0..n)
                .map(|_| {
                    seed = seed
                        .wrapping_mul(6364136223846793005)
                        .wrapping_add(1442695040888963407);
                    ((seed >> 33) % 1000) as f64 / 10.0
                })
                .collect();
            v.sort_by(f64::total_cmp);
            let k = 2 + case % 3;
            let got = sdcm(&v, &jenks(&v, k));
            // every choice of k-1 cut positions among the n-1 gaps
            let mut best = f64::INFINITY;
            let mut cuts: Vec<usize> = (1..k).collect();
            loop {
                let edges: Vec<f64> = cuts.iter().map(|&c| v[c]).collect();
                if edges.windows(2).all(|w| w[0] < w[1]) && edges.iter().all(|e| *e > v[0]) {
                    best = best.min(sdcm(&v, &edges));
                }
                let mut i = k - 1;
                while i > 0 && cuts[i - 1] == n - k + i {
                    i -= 1;
                }
                if i == 0 {
                    break;
                }
                cuts[i - 1] += 1;
                for j in i..k - 1 {
                    cuts[j] = cuts[j - 1] + 1;
                }
            }
            assert!(
                (got - best).abs() < 1e-6,
                "case {case}: {got} vs {best} for {v:?}"
            );
        }
    }

    /// Three clear groups come out as three classes.
    #[test]
    fn jenks_finds_clusters() {
        let v = [1.0, 2.0, 2.5, 3.0, 20.0, 21.0, 22.0, 50.0, 51.0, 52.5];
        assert_eq!(jenks(&v, 3), vec![20.0, 50.0]);
        let c = Classes::build(&Method::Jenks(3), &v).unwrap();
        assert_eq!(c.counts, vec![4, 3, 3]);
        // a large set is sampled, and still finds the gaps
        let big: Vec<f64> = (0..5000)
            .map(|i| {
                if i % 2 == 0 {
                    (i % 100) as f64
                } else {
                    1000.0 + (i % 100) as f64
                }
            })
            .collect();
        let mut s = big.clone();
        s.sort_by(f64::total_cmp);
        let e = jenks(&s, 2);
        assert_eq!(e.len(), 1);
        assert!(e[0] >= 1000.0 && e[0] < 1002.0, "{e:?}");
    }
}
