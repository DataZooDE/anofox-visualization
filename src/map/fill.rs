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
use ggplot_rs::render::backend::FillPattern;
use ggplot_rs::scale::binned::ScaleFillBinned;
use ggplot_rs::scale::Scale;

use crate::format::Locale;

use super::{Fill, Rgb, ValueScale};

/// The `Scale` for `fill` (a classed measure: `ScaleFillBinned`, a
/// continuous one or a category: `MapFill`), or `None` for `Fill::Plain`
/// (no aes at all). Shared by `plot::build`'s own `Fill` aesthetic and
/// `overlay`'s flow `Color` aesthetic (#420 slice 4: flows reuse the exact
/// same scale, wrapped, rather than hand-resolving an identity colour a
/// second time) — one scale, one source of truth for what a map's colours
/// mean.
pub(super) fn scale_for(
    fill: &Fill,
    locale: Locale,
    hatch_bg: Rgb,
    hatch_fg: Rgb,
) -> Option<Box<dyn Scale>> {
    match fill {
        Fill::Measure(s) if s.classes.is_some() => {
            let (_, c, colors) = s.classes.as_ref().unwrap();
            let breaks: Vec<f64> = std::iter::once(c.lo)
                .chain(c.edges.iter().copied())
                .chain(std::iter::once(c.hi))
                .collect();
            let scale = ScaleFillBinned::new(Aesthetic::Fill, breaks, colors.clone())
                .na_color(hatch_bg)
                .na_pattern(FillPattern::stripe(hatch_fg, hatch_bg))
                .na_label("No data");
            Some(Box::new(scale))
        }
        Fill::Measure(s) => Some(Box::new(
            MapFill::measure((**s).clone(), locale).na(hatch_bg, hatch_fg),
        )),
        Fill::Category { levels, .. } => Some(Box::new(MapFill::category(levels.clone()))),
        Fill::Plain(_) => None,
    }
}

/// A boxed `Scale`, re-aimed at a different aesthetic — `Box<dyn Scale>`
/// doesn't implement `Scale` itself (every `GGPlot::scale_*` builder takes
/// `impl Scale + 'static` and boxes it internally), so passing an
/// already-boxed scale through a *different* `scale_*` builder (flows'
/// `Color`, the same scale as the plot's own `Fill`, #420 slice 4) needs
/// this thin pass-through.
pub(super) struct Reaimed(pub Aesthetic, pub Box<dyn Scale>);

impl Clone for Reaimed {
    fn clone(&self) -> Self {
        Reaimed(self.0.clone(), self.1.clone_box())
    }
}

impl Scale for Reaimed {
    fn aesthetic(&self) -> Aesthetic {
        self.0.clone()
    }
    fn train(&mut self, values: &[Value]) {
        self.1.train(values)
    }
    fn map(&self, value: &Value) -> f64 {
        self.1.map(value)
    }
    fn breaks(&self) -> Vec<(f64, String)> {
        self.1.breaks()
    }
    fn name(&self) -> &str {
        self.1.name()
    }
    fn set_name(&mut self, name: &str) {
        self.1.set_name(name)
    }
    fn is_discrete(&self) -> bool {
        self.1.is_discrete()
    }
    fn map_to_color(&self, value: &Value) -> Option<(u8, u8, u8)> {
        self.1.map_to_color(value)
    }
    fn na_fill_pattern(&self) -> Option<FillPattern> {
        self.1.na_fill_pattern()
    }
    fn is_na_break(&self, label: &str) -> bool {
        self.1.is_na_break(label)
    }
    fn domain(&self) -> Option<(f64, f64)> {
        self.1.domain()
    }
    fn format_value(&self, value: &Value) -> Option<String> {
        self.1.format_value(value)
    }
    fn clone_box(&self) -> Box<dyn Scale> {
        Box::new(self.clone())
    }
}

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
        /// `(hatch background, hatch foreground)` for a row without a
        /// value — `None` (the bare `measure` constructor's default)
        /// draws no-data as a flat, wrong-looking colour instead (found
        /// porting #420 slice 5's own map fixtures: a continuous measure
        /// never reached `geom_sf` with a no-data row before — the old
        /// `::CLASSES` path already had its own hatch via
        /// `ScaleFillBinned::na_pattern`/`na_color`, scale_for's other
        /// branch above). Set via [`Self::na`].
        na: Option<(Rgb, Rgb)>,
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
            na: None,
        }
    }

    /// The no-data hatch for a continuous measure (`bg`/`fg` — the same
    /// `hatch_bg`/`hatch_fg` the classed branch of [`scale_for`] already
    /// passes `ScaleFillBinned::na_color`/`na_pattern`), so a row without
    /// a value draws hatched rather than falling through to `geom_sf`'s
    /// own generic "mapping failed" colour.
    pub(super) fn na(mut self, bg: Rgb, fg: Rgb) -> MapFill {
        if let MapFill::Measure { na, .. } = &mut self {
            *na = Some((bg, fg));
        }
        self
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
    /// `.rev()` for `::THRESHOLDS` matches this; the discrete legend draw
    /// path treats this `f64` as a plain swatch key, not a position, same
    /// as `MapFill::Category` below). Continuous: "nice" ticks over the
    /// domain (`ggplot_rs::scale::util::extended_breaks`, the same helper
    /// `legend.rs`'s own gradient-bar ticks use since #419 —
    /// `extended_breaks` can pick a round tick a little outside `[lo, hi]`,
    /// filtered back since this legend's `domain()` is fixed at `[lo, hi]`),
    /// normalised to `[0, 1]` over that domain — unlike the classed/
    /// category cases above, `ggplot_rs::guide::legend::draw_continuous_
    /// legend_at` (the only caller a continuous, non-discrete scale's
    /// `breaks()` has) reads this `f64` as a fraction up the gradient bar
    /// (`tick_y = bar_top + bar_height * (1.0 - pos)`), the same contract
    /// `ggplot_rs::scale::continuous::Continuous::breaks` follows via its
    /// own `self.map(&Value::Float(v))` — found running this slice's own
    /// map fixtures through the geom_sf path for the first time (a
    /// continuous `::MAP` legend was never reachable from that path
    /// before #420 slice 5 widened `plot::eligible`): the raw domain value
    /// was returned unnormalised, so a legend bar fit to few-hundred-px
    /// `bar_height` multiplied by e.g. `(1.0 - 100.0)` landed thousands of
    /// pixels off-canvas. Category: each level in its fixed order (again a
    /// swatch key, not a position).
    fn breaks(&self) -> Vec<(f64, String)> {
        match self {
            MapFill::Measure { scale, locale, .. } => match &scale.classes {
                Some((_, c, _)) => (0..c.len())
                    .rev()
                    .map(|i| (i as f64, c.label(i, &|v| scale.fmt.short(v, locale))))
                    .collect(),
                None => {
                    let eps = (scale.hi - scale.lo).abs() * 1e-9;
                    let span = (scale.hi - scale.lo).abs();
                    ggplot_rs::scale::util::extended_breaks(scale.lo, scale.hi, 5)
                        .into_iter()
                        .filter(|v| *v >= scale.lo - eps && *v <= scale.hi + eps)
                        .map(|v| {
                            let pos = if span > 0.0 {
                                ((v - scale.lo) / span).clamp(0.0, 1.0)
                            } else {
                                0.5
                            };
                            (pos, scale.fmt.short(v, locale))
                        })
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
            MapFill::Measure {
                scale, locale, na, ..
            } => match (&scale.classes, value) {
                (Some((_, _, colors)), Value::Str(s)) => {
                    Self::class_by_label(scale, locale, s).map(|i| colors[i])
                }
                (None, v) if v.as_f64().is_none() => na.map(|(bg, _)| bg),
                (_, v) => v.as_f64().map(|f| scale.color(f).0),
            },
            MapFill::Category { levels, .. } => match value {
                Value::Str(s) => levels.iter().find(|(l, _)| l == s).map(|(_, c)| *c),
                _ => None,
            },
        }
    }

    fn na_fill_pattern(&self) -> Option<FillPattern> {
        match self {
            MapFill::Measure {
                scale,
                na: Some((bg, fg)),
                ..
            } if scale.classes.is_none() => Some(FillPattern::stripe(*fg, *bg)),
            _ => None,
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
        // lo=0/hi=100 alone can't distinguish a normalised [0,1] position
        // from the raw domain value (a 0..100 break is also a valid 0..1
        // one) — a non-zero `lo` forces the two apart, the same gap that
        // let the un-normalised `breaks()` (fixed in #420 slice 5: see its
        // own doc comment) slip past this test before.
        let fill = MapFill::measure(scale(20.0, 120.0, None), RenderOptions::default().locale);
        let breaks = fill.breaks();
        assert!(!breaks.is_empty());
        for (pos, label) in &breaks {
            assert!((0.0..=1.0).contains(pos), "{pos} outside [0,1]: {label}");
        }
        // the end breaks' positions and labels agree with the domain they
        // were normalised from (ggplot_rs's `draw_continuous_legend_at`
        // reads `pos` as a fraction up the gradient bar, `label` as the
        // nice tick text — both must name the same point).
        let (lo_pos, lo_label) = breaks.first().unwrap();
        assert!(*lo_pos < 0.5, "{lo_pos} ({lo_label})");
        let (hi_pos, hi_label) = breaks.last().unwrap();
        assert!(*hi_pos > 0.5, "{hi_pos} ({hi_label})");
        assert!(!fill.is_discrete());
        assert_eq!(fill.domain(), Some((20.0, 120.0)));
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
