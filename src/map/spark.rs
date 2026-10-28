//! Tooltip trends on a map (M3b, #642): a `list(v ORDER BY week)::SPARKLINE`
//! (or `::SPARKBAR`) column next to a map's input becomes, per feature, a
//! one-line trend summary in its `<title>` and `data-trend` — the browser
//! adds the mini chart itself (through `render_cell`), static output keeps
//! the text.

use crate::format::{Locale, ValueFmt};

/// `"Trend: 12 → 18 (+50%) · min 9 · max 21 · 12 points"` — the wording of
/// the browser's table-cell sparkline tooltip (`cellSpark`). `None` for
/// fewer than two finite numbers.
pub(super) fn trend_text(values: &[f64], fmt: &ValueFmt, l: &Locale) -> Option<String> {
    let nums: Vec<f64> = values.iter().copied().filter(|v| v.is_finite()).collect();
    if nums.len() < 2 {
        return None;
    }
    let (first, last) = (nums[0], nums[nums.len() - 1]);
    let mn = nums.iter().copied().fold(f64::INFINITY, f64::min);
    let mx = nums.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let f = |v: f64| fmt.short(v, l);
    let chg = if first != 0.0 {
        format!(
            " ({}{:.0}%)",
            if last >= first { "+" } else { "−" },
            ((last - first) / first.abs() * 100.0).abs()
        )
    } else {
        String::new()
    };
    Some(format!(
        "Trend: {} → {}{chg} · min {} · max {} · {} points",
        f(first),
        f(last),
        f(mn),
        f(mx),
        nums.len()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trend_reads_like_the_cell_tooltip() {
        let l = Locale::default();
        let t = trend_text(&[12.0, 9.0, 21.0, 18.0], &ValueFmt::Plain, &l).unwrap();
        assert_eq!(t, "Trend: 12 → 18 (+50%) · min 9 · max 21 · 4 points");
        let t = trend_text(&[20.0, f64::NAN, 10.0], &ValueFmt::Percent, &l).unwrap();
        assert_eq!(t, "Trend: 20% → 10% (−50%) · min 10% · max 20% · 2 points");
        // a zero start has no percent change
        let t = trend_text(&[0.0, 5.0], &ValueFmt::Plain, &l).unwrap();
        assert!(t.starts_with("Trend: 0 → 5 · "), "{t}");
        assert_eq!(trend_text(&[1.0], &ValueFmt::Plain, &l), None);
        assert_eq!(trend_text(&[], &ValueFmt::Plain, &l), None);
    }
}
