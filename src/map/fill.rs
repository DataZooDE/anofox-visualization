//! `MapFill`: wraps the map's own fill colour logic (`ValueScale`,
//! `Classes`, `Fill::Category`) as a `ggplot_rs::scale::Scale`, so
//! `geom_sf`'s native Fill legend draws it — the pattern `crate::FmtGradient`
//! / `crate::StepFill` already use for heatmap/calendar (`src/lib.rs:4526-
//! 4733`). `classes.rs`'s class-break maths is untouched by this; it only
//! stops hand-writing hex strings into SVG (DataZooDE/anofox-visualization#420).
//!
//! A discrete legend's swatch colour comes from
//! `map_to_color(&Value::Str(label))` — `ggplot_rs::guide::legend`'s
//! `draw_legend_swatch` builds that `Value` from the break's *label text*,
//! not its numeric key or a raw data value (confirmed by reading
//! `guide/legend.rs` directly, not assumed) — so the classed branches below
//! match a `Value::Str` by label text, the same way `StepFill` does for
//! `::THRESHOLDS` bands.

use ggplot_rs::aes::Aesthetic;
use ggplot_rs::data::Value;
use ggplot_rs::scale::Scale;

use crate::format::Locale;

use super::{Rgb, ValueScale};

/// The `::MAP` fill scale: a continuous or classed measure, or a
/// `::CATEGORY`'s fixed level colours. Built once per render from the
/// already-resolved `ValueScale`/level list (`super::value_scale`/
/// `super::category_fill`) — `train` is a no-op, since the domain, classes
/// and dark-mode lift are already fixed before this exists.
// #420 WIP: not wired into `render()` yet (next slice) — the render
// rewrite is still in progress, so nothing constructs this type outside
// its own tests.
#[allow(dead_code)]
// Built once per render (not per row/mark), so the size gap to `Category`
// — `Locale`'s dozen month names, `format::Locale`, `src/format.rs:12-32`
// — costs nothing in practice; boxing a `Copy` locale just to appease the
// lint would be a net loss.
#[allow(clippy::large_enum_variant)]
#[derive(Clone)]
pub(super) enum MapFill {
    Measure {
        scale: Box<ValueScale>,
        locale: Locale,
        name: String,
    },
    Category {
        levels: Vec<(String, Rgb)>,
        name: String,
    },
}

#[allow(dead_code)] // #420 WIP, see the enum's own note above
impl MapFill {
    pub(super) fn measure(scale: ValueScale, locale: Locale) -> MapFill {
        let name = scale.title.clone().unwrap_or_default();
        MapFill::Measure {
            scale: Box::new(scale),
            locale,
            name,
        }
    }

    pub(super) fn category(levels: Vec<(String, Rgb)>) -> MapFill {
        MapFill::Category {
            levels,
            name: String::new(),
        }
    }

    /// The class whose [`super::classes::Classes::label`] equals `s`
    /// exactly (a classed measure's legend swatch lookup).
    fn class_by_label(scale: &ValueScale, locale: &Locale, s: &str) -> Option<usize> {
        let (_, c, _) = scale.classes.as_ref()?;
        (0..c.len()).find(|&i| c.label(i, &|v| scale.fmt.short(v, locale)) == s)
    }
}

impl Scale for MapFill {
    fn aesthetic(&self) -> Aesthetic {
        Aesthetic::Fill
    }

    fn train(&mut self, _values: &[Value]) {}

    fn is_discrete(&self) -> bool {
        match self {
            MapFill::Measure { scale, .. } => scale.classes.is_some(),
            MapFill::Category { .. } => true,
        }
    }

    fn map(&self, value: &Value) -> f64 {
        match self {
            MapFill::Measure { scale, locale, .. } => match (&scale.classes, value) {
                (Some(_), Value::Str(s)) => {
                    Self::class_by_label(scale, locale, s).map_or(f64::NAN, |i| i as f64)
                }
                (Some((_, c, _)), v) => v
                    .as_f64()
                    .map_or(f64::NAN, |f| c.index(f.clamp(scale.lo, scale.hi)) as f64),
                (None, v) => v.as_f64().unwrap_or(f64::NAN),
            },
            MapFill::Category { levels, .. } => match value {
                Value::Str(s) => levels
                    .iter()
                    .position(|(l, _)| l == s)
                    .map_or(f64::NAN, |i| i as f64),
                _ => f64::NAN,
            },
        }
    }

    /// Classed: every class's label, high to low (a colourbar-like legend
    /// reads top to bottom as high to low — `crate::StepFill::scale`'s own
    /// `.rev()` for `::THRESHOLDS` matches this). Continuous: "nice" ticks
    /// over the domain (`ggplot_rs::scale::util::extended_breaks`, the same
    /// helper `legend.rs`'s own gradient-bar ticks use since #419 —
    /// `extended_breaks` can pick a round tick a little outside `[lo, hi]`,
    /// filtered back since this legend's `domain()` is fixed at `[lo, hi]`).
    /// Category: each level in its fixed order.
    fn breaks(&self) -> Vec<(f64, String)> {
        match self {
            MapFill::Measure { scale, locale, .. } => match &scale.classes {
                Some((_, c, _)) => (0..c.len())
                    .rev()
                    .map(|i| (i as f64, c.label(i, &|v| scale.fmt.short(v, locale))))
                    .collect(),
                None => {
                    let eps = (scale.hi - scale.lo).abs() * 1e-9;
                    ggplot_rs::scale::util::extended_breaks(scale.lo, scale.hi, 5)
                        .into_iter()
                        .filter(|v| *v >= scale.lo - eps && *v <= scale.hi + eps)
                        .map(|v| (v, scale.fmt.short(v, locale)))
                        .collect()
                }
            },
            MapFill::Category { levels, .. } => levels
                .iter()
                .enumerate()
                .map(|(i, (l, _))| (i as f64, l.clone()))
                .collect(),
        }
    }

    fn map_to_color(&self, value: &Value) -> Option<(u8, u8, u8)> {
        match self {
            MapFill::Measure { scale, locale, .. } => match (&scale.classes, value) {
                (Some((_, _, colors)), Value::Str(s)) => {
                    Self::class_by_label(scale, locale, s).map(|i| colors[i])
                }
                (_, v) => v.as_f64().map(|f| scale.color(f).0),
            },
            MapFill::Category { levels, .. } => match value {
                Value::Str(s) => levels.iter().find(|(l, _)| l == s).map(|(_, c)| *c),
                _ => None,
            },
        }
    }

    fn name(&self) -> &str {
        match self {
            MapFill::Measure { name, .. } | MapFill::Category { name, .. } => name,
        }
    }

    fn set_name(&mut self, name: &str) {
        match self {
            MapFill::Measure { name: n, .. } | MapFill::Category { name: n, .. } => {
                *n = name.to_string();
            }
        }
    }

    fn domain(&self) -> Option<(f64, f64)> {
        match self {
            MapFill::Measure { scale, .. } if scale.classes.is_none() => Some((scale.lo, scale.hi)),
            _ => None,
        }
    }

    /// The measure's locale-formatted value (our own `ValueFmt`, e.g. a
    /// currency/percent string) — ggplot-rs writes this natively as
    /// `data-fvalue` on every mark (B2-4 `svg_formatted_values`, on for
    /// every `GGPlot` panel), so a map's hover tooltip reads the same
    /// formatted value the old hand-written writer gave it, without this
    /// module writing `data-fvalue` itself (reserved — ggplot-rs's own
    /// job). `None` for a `::CATEGORY` fill, same as every other discrete
    /// scale in this crate.
    fn format_value(&self, value: &Value) -> Option<String> {
        match self {
            MapFill::Measure { scale, locale, .. } => {
                value.as_f64().map(|f| scale.fmt.full(f, locale))
            }
            MapFill::Category { .. } => None,
        }
    }

    fn clone_box(&self) -> Box<dyn Scale> {
        Box::new(self.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::ValueFmt;
    use crate::map::classes::{Classes, Method};
    use crate::RenderOptions;

    /// A minimal `ValueScale` built directly (not through `value_scale()`,
    /// which needs a full `Column`/`Model` pipeline) — enough to exercise
    /// `MapFill` on its own: a plain sequential ramp over `[lo, hi]`,
    /// `classes` set only when the test gives one.
    fn scale(lo: f64, hi: f64, classes: Option<(Method, Classes, Vec<Rgb>)>) -> ValueScale {
        let o = RenderOptions::default();
        ValueScale {
            values: Vec::new(),
            ramp: crate::Ramp::new(&o, o.theme.seq_low(), lo, hi),
            mid: 0.0,
            diverging: false,
            start: 0.0,
            lo,
            hi,
            capped_lo: false,
            capped_hi: false,
            classes,
            fmt: ValueFmt::Plain,
            title: Some("Revenue".to_string()),
        }
    }

    #[test]
    fn continuous_breaks_are_nice_ticks_in_domain() {
        let fill = MapFill::measure(scale(0.0, 100.0, None), RenderOptions::default().locale);
        let breaks = fill.breaks();
        assert!(!breaks.is_empty());
        for (v, _) in &breaks {
            assert!((0.0..=100.0).contains(v), "{v} outside [0,100]");
        }
        assert!(!fill.is_discrete());
        assert_eq!(fill.domain(), Some((0.0, 100.0)));
    }

    #[test]
    fn continuous_map_to_color_matches_value_scale_directly() {
        let s = scale(0.0, 100.0, None);
        let direct = s.color_at(42.0);
        let fill = MapFill::measure(s, RenderOptions::default().locale);
        assert_eq!(fill.map_to_color(&Value::Float(42.0)), Some(direct));
    }

    #[test]
    fn format_value_uses_the_measures_own_fmt_not_a_raw_number() {
        let o = RenderOptions::default();
        let mut s = scale(0.0, 100.0, None);
        s.fmt = ValueFmt::Currency("€".to_string());
        let fill = MapFill::measure(s, o.locale);
        let formatted = fill.format_value(&Value::Float(42.0)).unwrap();
        assert!(formatted.contains('€'), "{formatted}");
        assert_ne!(formatted, "42", "a plain number, not the house format");
    }

    #[test]
    fn category_fill_has_no_formatted_value() {
        let fill = MapFill::category(vec![("a".to_string(), (0, 0, 0))]);
        assert_eq!(fill.format_value(&Value::Str("a".to_string())), None);
    }

    #[test]
    fn classed_legend_swatch_is_looked_up_by_label_text_not_position() {
        let classes = Classes::build(&Method::Equal(2), &[0.0, 25.0, 75.0, 100.0]).unwrap();
        let colors = vec![(10u8, 20, 30), (200, 210, 220)];
        let method = Method::Equal(2);
        let s = scale(0.0, 100.0, Some((method, classes.clone(), colors.clone())));
        let fill = MapFill::measure(s, RenderOptions::default().locale);
        assert!(fill.is_discrete());

        let breaks = fill.breaks();
        assert_eq!(breaks.len(), 2);
        // ggplot-rs's own legend swatch code looks the colour up by the
        // break's *label text*, not its numeric key — exercise exactly
        // that call shape.
        for (_, label) in &breaks {
            let color = fill
                .map_to_color(&Value::Str(label.clone()))
                .unwrap_or_else(|| panic!("no colour for label {label:?}"));
            assert!(colors.contains(&color), "{label} -> {color:?}");
        }
        // A raw row value still resolves straight through the classes.
        assert_eq!(fill.map_to_color(&Value::Float(10.0)), Some(colors[0]));
        assert_eq!(fill.map_to_color(&Value::Float(90.0)), Some(colors[1]));
    }

    #[test]
    fn category_breaks_and_swatches_use_the_fixed_level_colours() {
        let levels = vec![
            ("north".to_string(), (1u8, 2, 3)),
            ("south".to_string(), (4u8, 5, 6)),
        ];
        let fill = MapFill::category(levels.clone());
        assert!(fill.is_discrete());
        let breaks = fill.breaks();
        assert_eq!(
            breaks,
            vec![(0.0, "north".to_string()), (1.0, "south".to_string())]
        );
        assert_eq!(
            fill.map_to_color(&Value::Str("south".to_string())),
            Some((4, 5, 6))
        );
        assert_eq!(fill.map_to_color(&Value::Str("unknown".to_string())), None);
    }

    #[test]
    fn name_round_trips() {
        let mut fill = MapFill::category(vec![("a".to_string(), (0, 0, 0))]);
        assert_eq!(fill.name(), "");
        fill.set_name("Region");
        assert_eq!(fill.name(), "Region");
    }
}
