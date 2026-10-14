//! The shared look of every contract plot: the house theme (a ~20 % larger
//! base font than ggplot2's 11 pt, bold strip titles, tick labels ≥ 10 px),
//! the quiet reference-line style, stable model colours and the
//! `title :=` / `x_label :=` / `y_label :=` options.
//!
//! **Titles.** A contract plot draws *no* inner title by default — in a
//! dashboard the panel caption (`'…'::TITLE`) is the title, and a standalone
//! SVG from the SQL macro is the same chart. `title := '…'` adds one.
//! Sub-panel titles of a grid (a term, a metric, a group) are kept: they
//! label the cells, not the chart.

use super::{opt_str, Row};
use crate::{RenderOptions, DZ_COLORS};
use ggplot_rs::prelude::*;
use ggplot_rs::theme::{ElementText, ThemeUpdate};

/// Base font size (ggplot2's default 11 pt × 1.2).
pub(super) const BASE_SIZE: f64 = 13.2;
/// Reference lines (zero, 4/n, diagonals, 50 %): light grey, thin, dashed.
pub(super) const REF: (u8, u8, u8) = (178, 184, 194);
pub(super) const REF_WIDTH: f64 = 0.8;
/// Facet strip / sub-panel title text.
const STRIP: (u8, u8, u8) = (40, 44, 52);

/// `title :=`, `x_label :=`, `y_label :=` of a contract macro.
#[derive(Clone, Debug, Default)]
pub(super) struct Labels {
    pub title: Option<String>,
    pub x: Option<String>,
    pub y: Option<String>,
}

impl Labels {
    pub(super) fn from(options: &Row) -> Labels {
        Labels {
            title: opt_str(options, "title"),
            x: opt_str(options, "x_label"),
            y: opt_str(options, "y_label"),
        }
    }
    /// The x label: the option, else `default`.
    pub(super) fn x_or(&self, default: &str) -> String {
        self.x.clone().unwrap_or_else(|| default.to_string())
    }
    pub(super) fn y_or(&self, default: &str) -> String {
        self.y.clone().unwrap_or_else(|| default.to_string())
    }
    /// Apply the axis labels (defaults given) and the optional title.
    pub(super) fn apply(&self, plot: GGPlot, x: &str, y: &str) -> GGPlot {
        let plot = plot.xlab(&self.x_or(x)).ylab(&self.y_or(y));
        match &self.title {
            Some(t) => plot.title(t),
            None => plot,
        }
    }
}

/// A raw column name for an axis: `order_value` → "order value".
pub(super) fn humanize(name: &str) -> String {
    let s = name.trim().replace(['_', '.'], " ");
    let s: Vec<&str> = s.split_whitespace().collect();
    s.join(" ")
}

/// The house theme. `small` (a grid cell) shrinks titles a little but keeps
/// tick labels at ≥ 10 px.
pub(super) fn theme(small: bool) -> Theme {
    let base = if small { BASE_SIZE * 0.9 } else { BASE_SIZE };
    let mut th = ggplot_rs::theme::presets::theme_minimal_base(base);
    let floor = |e: &mut ElementText, min: f64| e.size = e.size.max(min);
    floor(&mut th.axis_text_x, 10.0);
    floor(&mut th.axis_text_y, 10.0);
    floor(&mut th.legend_text, 10.5);
    th.strip_text.face = ggplot_rs::render::backend::FontFace::Bold;
    th.strip_text.color = STRIP;
    th.strip_text.size = th.strip_text.size.max(11.0);
    if small {
        th.title.size = (base * 1.0).max(11.0);
        th.title.face = ggplot_rs::render::backend::FontFace::Bold;
        th.title.color = STRIP;
    }
    th
}

/// The house theme as an update over a role-rendered plot (whose own theme
/// is ggplot2's minimal at 11 pt).
pub(super) fn theme_update(small: bool) -> ThemeUpdate {
    let th = theme(small);
    ThemeUpdate {
        text: Some(th.text),
        title: Some(th.title),
        axis_text_x: Some(th.axis_text_x),
        axis_text_y: Some(th.axis_text_y),
        axis_title_x: Some(th.axis_title_x),
        axis_title_y: Some(th.axis_title_y),
        legend_text: Some(th.legend_text),
        legend_title: Some(th.legend_title),
        strip_text: Some(th.strip_text),
        ..Default::default()
    }
}

/// Style a directly drawn plot: house theme, brand primary, legend on top.
pub(super) fn styled(plot: GGPlot, o: &RenderOptions, small: bool) -> GGPlot {
    styled_aspect(plot, o, small, None)
}

/// [`styled`] with a fixed panel aspect ratio (`Some(1.0)`: a square ROC /
/// calibration panel).
pub(super) fn styled_aspect(
    plot: GGPlot,
    o: &RenderOptions,
    small: bool,
    aspect: Option<f64>,
) -> GGPlot {
    let mut th = theme(small);
    th.aspect_ratio = aspect;
    plot.theme(th)
        .primary_color(o.brand())
        .legend_position(ggplot_rs::theme::LegendPosition::Top)
}

/// A horizontal reference line in the quiet style.
pub(super) fn hline(plot: GGPlot, y: f64) -> GGPlot {
    plot.geom_hline_with(GeomHline {
        yintercept: y,
        color: REF,
        width: REF_WIDTH,
        linetype: Linetype::Dashed,
        alpha: 1.0,
    })
}

/// A vertical reference line in the quiet style.
pub(super) fn vline(plot: GGPlot, x: f64) -> GGPlot {
    plot.geom_vline_with(GeomVline {
        xintercept: x,
        color: REF,
        width: REF_WIDTH,
        linetype: Linetype::Dashed,
        alpha: 1.0,
    })
}

/// The colour of a known model id: the same model is the same colour in
/// every chart of a dashboard. Unknown ids hash onto the palette (slate grey,
/// the "other" colour, excluded).
pub(super) fn model_color(name: &str) -> (u8, u8, u8) {
    let key: String = name
        .trim()
        .to_ascii_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    let i = match key.as_str() {
        "ols" | "lm" | "linear" | "logistic" | "logit" | "binomial" => 0,
        "huber" | "poisson" => 1,
        "lasso" | "probit" => 2,
        "theilsen" | "negbinom" | "negativebinomial" => 3,
        "ridge" | "gamma" => 5,
        "elasticnet" | "enet" | "tweedie" => 6,
        "wls" | "glm" => 7,
        "lars" | "rule" | "rules" => 8,
        "ransac" | "bls" | "nnls" => 9,
        "quantile" | "qr" | "median" => 10,
        _ => {
            // FNV-1a.
            let mut h: u64 = 0xcbf2_9ce4_8422_2325;
            for b in key.bytes() {
                h ^= b as u64;
                h = h.wrapping_mul(0x0100_0000_01b3);
            }
            (h % 11) as usize
        }
    };
    DZ_COLORS[i]
}

/// Colours for `levels` (in this order): known model ids by [`model_color`];
/// unknown ones in palette order skipping colours already taken, so a small
/// set of arbitrary names (strata, groups) stays distinct.
pub(super) fn level_colors(levels: &[String]) -> Vec<(u8, u8, u8)> {
    let known = |l: &str| model_color(l) != hash_only(l);
    let mut used: Vec<(u8, u8, u8)> = levels
        .iter()
        .filter(|l| known(l))
        .map(|l| model_color(l))
        .collect();
    let mut next = 0usize;
    levels
        .iter()
        .map(|l| {
            if known(l) {
                return model_color(l);
            }
            // Free palette slots first (slate grey last).
            while next < 11 && used.contains(&DZ_COLORS[next]) {
                next += 1;
            }
            let c = if next < 11 {
                DZ_COLORS[next]
            } else {
                model_color(l)
            };
            used.push(c);
            c
        })
        .collect()
}

fn hash_only(name: &str) -> (u8, u8, u8) {
    let key: String = name
        .trim()
        .to_ascii_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in key.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    DZ_COLORS[(h % 11) as usize]
}

/// A discrete colour/fill scale with `levels` in the given (legend) order and
/// [`level_colors`].
pub(super) fn series_scale(aesthetic: Aesthetic, levels: &[String]) -> ScaleColorDiscrete {
    ScaleColorDiscrete::new(aesthetic)
        .with_levels(levels.to_vec())
        .with_palette(
            level_colors(levels)
                .into_iter()
                .map(|(r, g, b)| RGBAColor::new(r, g, b))
                .collect(),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn humanizes() {
        assert_eq!(humanize("order_value"), "order value");
        assert_eq!(humanize("tenure_months"), "tenure months");
        assert_eq!(humanize("x"), "x");
    }

    #[test]
    fn model_colors_are_stable() {
        assert_eq!(model_color("ols"), DZ_COLORS[0]);
        assert_eq!(model_color("OLS"), DZ_COLORS[0]);
        assert_eq!(model_color("theil_sen"), DZ_COLORS[3]);
        assert_eq!(model_color("elasticnet"), model_color("elastic_net"));
        assert_eq!(model_color("my-model"), model_color("my-model"));
        assert_ne!(model_color("zzz"), DZ_COLORS[11]);
        // The same model keeps its colour whatever else is in the chart.
        let a = level_colors(&["ols".into(), "ridge".into()]);
        let b = level_colors(&["ridge".into(), "huber".into(), "ols".into()]);
        assert_eq!(a[0], b[2]);
        assert_eq!(a[1], b[0]);
        // Unknown levels stay distinct.
        let c = level_colors(&["Basic".into(), "Plus".into(), "Pro".into()]);
        assert!(c[0] != c[1] && c[1] != c[2] && c[0] != c[2]);
    }

    #[test]
    fn labels_from_options() {
        let o: Row = serde_json::from_str(r#"{"x_label":"price (€)","title":" "}"#).unwrap();
        let l = Labels::from(&o);
        assert_eq!(l.x_or("x"), "price (€)");
        assert_eq!(l.y_or("y"), "y");
        assert!(l.title.is_none());
    }
}
