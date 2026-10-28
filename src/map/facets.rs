//! Small multiples on a map (M3a-1, #644): `<col>::FACET` next to a map's
//! input draws one map per value in one panel — a ggplot-rs `facet_wrap`
//! over a "stacked" result (every facet's per-feature view, one after the
//! other), so one `Model` trains every scale over all panels at once:
//! projection, extent, classes, colours, symbol sizes and the one legend
//! are shared without extra code. A feature missing from a facet draws as
//! no data (areas) or not at all (points), as on a time map (D20).
//!
//! Plan: `~/projects/duckdb/plans/plan-maps-m3.md` §5; decisions D-M3-1…6,
//! 17, 18.

use crate::{find_role, value_str, Column, RenderOptions, Role};

use super::frames::{self, Frames};
use super::{hexbin, Model};

/// Above this many facet values only the first are drawn (D-M3-2).
pub(super) const MAX_FACETS: usize = 12;

/// What the plot needs to know about the facets (per stacked row).
pub(super) struct FacetCtx {
    /// The facet column's display name.
    pub name: String,
    /// Facet values in panel order (D11 rules, D-M3-17).
    pub levels: Vec<String>,
    /// Per stacked row, its facet value.
    pub label_of_row: Vec<String>,
    /// Per stacked row, the result row it came from (`data-row`, `::LINK`).
    pub result_row: Vec<usize>,
    /// `::FACET_NCOL`, clamped to 1..=12.
    pub ncol: Option<usize>,
    /// Distinct facet values before the cap.
    pub total: usize,
}

/// The facet column: `::FACET`, else `::FACET_FREE`.
fn facet_col(cols: &[Column]) -> Option<&Column> {
    find_role(cols, Role::Facet).or_else(|| find_role(cols, Role::FacetFree))
}

/// The facets of a map's columns: `::FACET` (or `::FACET_FREE`, which
/// shares the extent too, D-M3-6), the first [`MAX_FACETS`] kept.
pub(super) fn of(cols: &[Column]) -> Option<Frames> {
    let mut fr = frames::of_role(cols, facet_col(cols)?.role)?;
    if fr.labels.len() > MAX_FACETS {
        fr.labels.truncate(MAX_FACETS);
        fr.row_in.truncate(MAX_FACETS);
    }
    Some(fr)
}

/// Every facet's per-feature view, stacked: the columns, which stacked
/// rows had a row of their own in their facet (`present`), and the facet
/// context.
pub(super) fn stacked(cols: &[Column], fr: &Frames) -> (Vec<Column>, Vec<bool>, FacetCtx) {
    let mut out: Vec<Column> = cols
        .iter()
        .map(|c| Column::new(c.name.clone(), c.role, Vec::new()))
        .collect();
    let mut present = Vec::new();
    let mut label_of_row = Vec::new();
    let mut result_row = Vec::new();
    for (k, label) in fr.labels.iter().enumerate() {
        let view = frames::view(cols, fr, k);
        for ((o, v), c) in out.iter_mut().zip(view).zip(cols) {
            let mut vals = v.values;
            // an absent feature keeps its name (hover reads "South: no data")
            if c.role == Role::Label {
                for (f, val) in vals.iter_mut().enumerate() {
                    if *val == crate::Value::Na {
                        if let Some(first) = c.values.get(fr.first_row[f]) {
                            *val = first.clone();
                        }
                    }
                }
            }
            o.values.extend(vals);
        }
        let p = frames::presence(fr, k);
        label_of_row.extend(std::iter::repeat_n(label.clone(), p.len()));
        present.extend(p);
        result_row.extend(frames::result_rows(fr, k));
    }
    let ctx = FacetCtx {
        name: fr.name.clone(),
        levels: fr.labels.clone(),
        label_of_row,
        result_row,
        ncol: ncol_of(cols),
        total: fr.total_frames,
    };
    (out, present, ctx)
}

/// `::FACET_NCOL`, clamped to 1..=[`MAX_FACETS`].
fn ncol_of(cols: &[Column]) -> Option<usize> {
    find_role(cols, Role::FacetCols)
        .and_then(|c| c.values.iter().find_map(crate::cell_num))
        .map(|n| (n.round() as i64).clamp(1, MAX_FACETS as i64) as usize)
}

/// A hexbin map's small multiples (M3a-2, #645). `m` is built over every
/// row once, so its grid, extent and projection cover every panel's
/// points; this bins each facet's own points on that grid, concatenates
/// the cells in facet order (each with its facet value) and classes the
/// fill over all of them, so a colour means the same count or sum in every
/// panel. The context's rows are the result's own rows.
pub(super) fn hexbin(
    o: &RenderOptions,
    cols: &[Column],
    fr: &Frames,
    m: &mut Model,
) -> Option<FacetCtx> {
    let fcol = facet_col(cols)?;
    let all = m.hexbin.as_ref()?;
    let grid = hexbin::Grid::Fixed {
        width: all.width,
        origin: all.origin,
    };
    // sum or count is decided over every row, not per panel
    let (width, origin, planar, has_measure) = (all.width, all.origin, all.planar, all.has_measure);
    let row_facet: Vec<String> = (0..m.rows)
        .map(|i| fcol.values.get(i).map(value_str).unwrap_or_default())
        .collect();
    let (mut cells, mut facet, mut n_points) = (Vec::new(), Vec::new(), 0);
    for label in &fr.labels {
        let present: Vec<bool> = row_facet.iter().map(|f| f == label).collect();
        let Some(hb) = hexbin::of(cols, &m.proj, Some(&present), grid) else {
            continue;
        };
        n_points += hb.n_points;
        facet.extend(std::iter::repeat_n(label.clone(), hb.cells.len()));
        cells.extend(hb.cells);
    }
    let hb = hexbin::Hexbin {
        width,
        origin,
        cells,
        n_points,
        has_measure,
        planar,
        facet,
    };
    m.fill = hexbin::fill_of(o, cols, &hb);
    m.hexbin = Some(hb);
    // the point rows draw nothing (the cells take over); a row past the
    // facet cap goes with the last panel
    let last = fr.labels.last()?.clone();
    let label_of_row = row_facet
        .into_iter()
        .map(|f| {
            if fr.labels.contains(&f) {
                f
            } else {
                last.clone()
            }
        })
        .collect();
    Some(FacetCtx {
        name: fr.name.clone(),
        levels: fr.labels.clone(),
        label_of_row,
        result_row: (0..m.rows).collect(),
        ncol: ncol_of(cols),
        total: fr.total_frames,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Kind, Value};

    fn strs(v: &[&str]) -> Vec<Value> {
        v.iter().map(|s| Value::Str(s.to_string())).collect()
    }

    #[test]
    fn stacks_one_view_per_facet_in_order() {
        let cols = vec![
            Column::new("r", Role::Region, strs(&["DE-BY", "DE-BW", "DE-BY"])),
            Column::new("w", Role::Facet, strs(&["2", "1", "1"])),
            Column::new(
                "v",
                Role::Value(Kind::Bar),
                vec![Value::Float(3.0), Value::Float(2.0), Value::Float(1.0)],
            ),
        ];
        let fr = of(&cols).unwrap();
        assert_eq!(fr.labels, ["1", "2"]); // numeric facets sort (D11)
        let (st, present, ctx) = stacked(&cols, &fr);
        // two features × two facets
        assert_eq!(st[0].values.len(), 4);
        assert_eq!(ctx.label_of_row, ["1", "1", "2", "2"]);
        // DE-BW has no row in facet 2: absent, value NA
        assert_eq!(present, [true, true, true, false]);
        assert_eq!(st[2].values[3], Value::Na);
        assert_eq!(ctx.result_row, [2, 1, 0, 1]);
    }

    #[test]
    fn caps_at_twelve_and_reads_ncol() {
        let n = 15;
        let cols = vec![
            Column::new("r", Role::Region, strs(&vec!["DE-BY"; n])),
            Column::new(
                "w",
                Role::Facet,
                (0..n).map(|i| Value::Str(format!("f{i:02}"))).collect(),
            ),
            Column::new("c", Role::FacetCols, vec![Value::Float(40.0); n]),
        ];
        let fr = of(&cols).unwrap();
        assert_eq!(fr.labels.len(), MAX_FACETS);
        assert_eq!(fr.labels[0], "f00"); // the first are kept
        let (_, _, ctx) = stacked(&cols, &fr);
        assert_eq!(ctx.total, 15);
        assert_eq!(ctx.ncol, Some(12));
    }
}
