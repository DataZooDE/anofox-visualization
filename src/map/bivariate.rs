//! `::BIVARIATE`: a second measure next to a map's own measure (`Role::Value`)
//! colours regions/points by **tertiles of both** — a 3×3 palette and legend
//! (M2d, `docs/plans/plan-maps.md` §15 D10, DataZooDE/anofox-visualization#149).
//!
//! The palette itself (which colour a cell `(i, j)` gets) is
//! `ggplot_rs::scale::bivariate::bivariate_cell_color` — the exact function
//! backing `ScaleFillBivariate`/`GeomBivariateLegend`, built upstream for
//! this feature (sipemu/ggplot-rs's own #177–#182) — not a second,
//! hand-rolled colour mix: one source of truth for the 9 cells, whether
//! `plot.rs`'s `geom_sf` path colours a feature or draws the legend swatch
//! for it.

use ggplot_rs::scale::bivariate::bivariate_cell_color;

use crate::format::{Locale, ValueFmt};
use crate::{cell_num, value_fmt, Column, Kind, RenderOptions, Role};

use super::classes::{Classes, Method};
use super::Rgb;

/// A map's bivariate fill: measure `a` (the map's own `Role::Value` column)
/// crossed with measure `b` (`::BIVARIATE`), each split into tertiles
/// (`Classes::build(&Method::Quantile(3), ..)`, same half-open-bin
/// convention as a classed choropleth).
pub(super) struct Bivar {
    /// Row `i`'s raw `a` value (`None` = no data).
    pub a: Vec<Option<f64>>,
    /// Row `i`'s raw `b` value (`::BIVARIATE`, `None` = no data).
    pub b: Vec<Option<f64>>,
    /// Row `i`'s `a`-level, `0..=2` (`None` when `a[i]` is `None`).
    pub ia: Vec<Option<usize>>,
    /// Row `i`'s `b`-level, `0..=2`.
    pub jb: Vec<Option<usize>>,
    /// `colors[i][j]`: the cell colour for `a`-level `i`, `b`-level `j`
    /// (`ggplot_rs::scale::bivariate::bivariate_cell_color(base, color_a,
    /// color_b, 3, 3, i, j)` — `base`/`color_a`/`color_b` kept alongside so
    /// `plot.rs`'s `GeomBivariateLegend` (which recomputes the same 9
    /// cells from those three) always draws the identical grid).
    pub colors: [[Rgb; 3]; 3],
    pub base: Rgb,
    pub color_a: Rgb,
    pub color_b: Rgb,
    /// `counts[i][j]`: how many rows fall in cell `(i, j)`.
    pub counts: [[usize; 3]; 3],
    pub fmt_a: ValueFmt,
    pub fmt_b: ValueFmt,
    pub title_a: Option<String>,
    pub title_b: Option<String>,
}

/// `Classes::build(&Method::Quantile(3), ..)`'s class index (`0..c.len()`)
/// compacted to a level `0..=2` (D10): `k=3` as is; `k=2` -> `0` or `2`;
/// `k=1` -> `1` (the middle cell — "neither high nor low" is the only
/// honest reading of a single class).
fn level_of(k: usize, len: usize) -> usize {
    match len {
        3 => k,
        2 => {
            if k == 0 {
                0
            } else {
                2
            }
        }
        _ => 1,
    }
}

/// The per-row level (`0..=2`, `None` = no data) from its raw per-row
/// values. A linted `::BIVARIATE`/measure column with fewer than 3 distinct
/// values (`map/bivariate-few-levels`, `lint.rs`) still works here — `len`
/// (1 or 2) just means fewer classes (D10's own `level_of` compaction).
fn levels_of(values: &[Option<f64>]) -> Vec<Option<usize>> {
    let finite: Vec<f64> = values.iter().flatten().copied().collect();
    let Some(classes) = Classes::build(&Method::Quantile(3), &finite) else {
        return vec![None; values.len()];
    };
    let len = classes.len();
    values
        .iter()
        .map(|v| v.map(|v| level_of(classes.index(v), len)))
        .collect()
}

/// Builds the bivariate fill when it applies: a numeric `::BIVARIATE`
/// column with at least one finite value, next to the map's own measure
/// `a_col` (`Role::Value`). `cols` is the full column set, used (minus any
/// `::YFORMAT`, which is `a`'s own) to format `b` the way `a` is formatted
/// — `crate::value_fmt`'s own rule, so a currency/percent `b` reads right
/// without `a`'s y-axis format leaking onto it.
pub(super) fn of(
    o: &RenderOptions,
    cols: &[Column],
    a_col: &Column,
    b_col: &Column,
    rows: usize,
) -> Option<Bivar> {
    let b: Vec<Option<f64>> = (0..rows)
        .map(|i| {
            b_col
                .values
                .get(i)
                .and_then(cell_num)
                .filter(|f| f.is_finite())
        })
        .collect();
    if b.iter().all(Option::is_none) {
        return None;
    }
    let a: Vec<Option<f64>> = (0..rows)
        .map(|i| {
            a_col
                .values
                .get(i)
                .and_then(cell_num)
                .filter(|f| f.is_finite())
        })
        .collect();
    let ia = levels_of(&a);
    let jb = levels_of(&b);

    let t = &o.theme;
    let base = if t.variant == crate::theme::Variant::Dark {
        t.colors.card
    } else {
        (243, 243, 243)
    };
    let color_a = t.color(0);
    let color_b = t.color(1);
    let mut colors = [[(0u8, 0u8, 0u8); 3]; 3];
    let mut counts = [[0usize; 3]; 3];
    for (i, row) in colors.iter_mut().enumerate() {
        for (j, cell) in row.iter_mut().enumerate() {
            *cell = bivariate_cell_color(base, color_a, color_b, 3, 3, i, j);
        }
    }
    for k in 0..rows {
        if let (Some(i), Some(j)) = (ia[k], jb[k]) {
            counts[i][j] += 1;
        }
    }

    let b_cols: Vec<Column> = cols
        .iter()
        .filter(|c| c.role != Role::YFormat)
        .cloned()
        .collect();
    let fmt_b = value_fmt(&b_cols, Kind::Bar, &b_col.values);
    let fmt_a = value_fmt(cols, Kind::Bar, &a_col.values);
    let title_a = super::legend::legend_title(&a_col.name, &fmt_a);
    let title_b = super::legend::legend_title(&b_col.name, &fmt_b);

    Some(Bivar {
        a,
        b,
        ia,
        jb,
        colors,
        base,
        color_a,
        color_b,
        counts,
        fmt_a,
        fmt_b,
        title_a,
        title_b,
    })
}

/// `::FRAME` (M2e-B, D21): recompute `biv`'s per-row tertile levels and
/// 3×3 counts from its own per-frame raw `a`/`b` values, using breaks
/// (`ca`/`cb`) built once over every frame (`frames::shared_bivariate`)
/// instead of the frame's own subset — so a cell's `(i, j)` position means
/// the same thing whichever frame is shown; only the counts stay frame k's
/// own. `biv.colors`/`base`/`color_a`/`color_b` (the palette) and
/// `fmt_a`/`fmt_b`/`title_a`/`title_b` are unaffected — they don't depend
/// on which rows fall in which cell.
pub(super) fn apply_shared(biv: &mut Bivar, ca: &Classes, cb: &Classes) {
    let level =
        |v: Option<f64>, c: &Classes| v.map(|v| level_of(c.index(v.clamp(c.lo, c.hi)), c.len()));
    let ia: Vec<Option<usize>> = biv.a.iter().map(|v| level(*v, ca)).collect();
    let jb: Vec<Option<usize>> = biv.b.iter().map(|v| level(*v, cb)).collect();
    let mut counts = [[0usize; 3]; 3];
    for k in 0..ia.len() {
        if let (Some(i), Some(j)) = (ia[k], jb[k]) {
            counts[i][j] += 1;
        }
    }
    biv.ia = ia;
    biv.jb = jb;
    biv.counts = counts;
}

/// `"low"` / `"mid"` / `"high"` (de: `"niedrig"`/`"mittel"`/`"hoch"`) for
/// level `0..=2`.
pub(super) fn level_word(level: usize, locale: &Locale) -> &'static str {
    let german = locale.tag.starts_with("de");
    match (level, german) {
        (0, false) => "low",
        (1, false) => "mid",
        (0, true) => "niedrig",
        (1, true) => "mittel",
        (_, false) => "high",
        (_, true) => "hoch",
    }
}

impl Bivar {
    /// `"{label}: {A title} {fa} ({lvl}), {B title} {fb} ({lvl})"` (D10's
    /// own tooltip shape); `label` may be empty.
    pub(super) fn title_of(&self, i: usize, label: &str, locale: &Locale) -> String {
        let a_name = self.title_a.as_deref().unwrap_or("A");
        let b_name = self.title_b.as_deref().unwrap_or("B");
        let a_part = match (self.a[i], self.ia[i]) {
            (Some(v), Some(lvl)) => format!(
                "{a_name} {} ({})",
                self.fmt_a.full(v, locale),
                level_word(lvl, locale)
            ),
            _ => format!("{a_name}: no data"),
        };
        let b_part = match (self.b[i], self.jb[i]) {
            (Some(v), Some(lvl)) => format!(
                "{b_name} {} ({})",
                self.fmt_b.full(v, locale),
                level_word(lvl, locale)
            ),
            _ => format!("{b_name}: no data"),
        };
        if label.is_empty() {
            format!("{a_part}, {b_part}")
        } else {
            format!("{label}: {a_part}, {b_part}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_of_compacts_fewer_classes_to_the_middle_or_ends() {
        assert_eq!(level_of(0, 3), 0);
        assert_eq!(level_of(1, 3), 1);
        assert_eq!(level_of(2, 3), 2);
        assert_eq!(level_of(0, 2), 0);
        assert_eq!(level_of(1, 2), 2);
        assert_eq!(level_of(0, 1), 1);
    }

    #[test]
    fn of_is_none_without_any_finite_bivariate_value() {
        let o = RenderOptions::default();
        let a = Column::new(
            "sales".to_string(),
            Role::Value(Kind::Bar),
            vec![crate::Value::Float(1.0), crate::Value::Float(2.0)],
        );
        let b = Column::new(
            "share".to_string(),
            Role::Bivariate,
            vec![crate::Value::Na, crate::Value::Na],
        );
        assert!(of(&o, &[a.clone(), b.clone()], &a, &b, 2).is_none());
    }

    #[test]
    fn nine_rows_spanning_the_grid_give_nine_distinct_cells() {
        let o = RenderOptions::default();
        let a_vals: Vec<f64> = vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 2.0, 2.0, 2.0];
        let b_vals: Vec<f64> = vec![0.0, 1.0, 2.0, 0.0, 1.0, 2.0, 0.0, 1.0, 2.0];
        let a = Column::new(
            "a".to_string(),
            Role::Value(Kind::Bar),
            a_vals.iter().map(|v| crate::Value::Float(*v)).collect(),
        );
        let b = Column::new(
            "b".to_string(),
            Role::Bivariate,
            b_vals.iter().map(|v| crate::Value::Float(*v)).collect(),
        );
        let biv = of(&o, &[a.clone(), b.clone()], &a, &b, 9).unwrap();
        let mut seen = std::collections::HashSet::new();
        for i in 0..9 {
            let (ii, jj) = (biv.ia[i].unwrap(), biv.jb[i].unwrap());
            seen.insert(biv.colors[ii][jj]);
        }
        assert_eq!(seen.len(), 9, "{seen:?}");
    }
}
