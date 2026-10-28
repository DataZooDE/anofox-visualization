//! Legend text shared by the map kinds. ggplot-rs draws every map legend
//! and picks where it goes (`legend_inside_auto`, #666); this module no
//! longer measures one.

/// The legend title of a measure: its name (not a generated `c3`) and its
/// unit from the value format — "Revenue (€)", "Share (%)".
pub(super) fn legend_title(name: &str, fmt: &crate::format::ValueFmt) -> Option<String> {
    use crate::format::ValueFmt;
    let generated = name.is_empty()
        || (name.starts_with('c')
            && name.len() > 1
            && name[1..].bytes().all(|b| b.is_ascii_digit()));
    if generated {
        return None;
    }
    let unit = match fmt {
        ValueFmt::Currency(u) => Some(u.trim().to_string()),
        ValueFmt::Percent | ValueFmt::Share => Some("%".to_string()),
        ValueFmt::Prefix(u) | ValueFmt::Suffix(u) => Some(u.trim().to_string()),
        _ => None,
    }
    .filter(|u| !u.is_empty() && !name.contains(u.as_str()));
    Some(match unit {
        Some(u) => format!("{name} ({u})"),
        None => name.to_string(),
    })
}

/// The largest "nice" value (1, 2, 2.5, 5 × 10ⁿ) that is ≤ `x` (`x > 0`).
pub(super) fn nice_floor(x: f64) -> f64 {
    if x <= 0.0 || !x.is_finite() {
        return 0.0;
    }
    let mag = 10f64.powf(x.log10().floor());
    [5.0, 2.5, 2.0, 1.0]
        .iter()
        .map(|m| m * mag)
        .find(|v| *v <= x + x * 1e-9)
        .unwrap_or(mag / 10.0)
}
