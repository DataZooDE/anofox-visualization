//! `matrix`: a correlation-style heatmap — `row, col, value` (also read:
//! `column_a` / `column_b`, `var1` / `var2`, `row_name` / `col_name` for row
//! and col; `pearson` / `spearman` / `kendall` / `correlation` / `corr` /
//! `count` / `n` for value). A pair list (anofox-tabular's
//! `profile_correlations`: `column_a, column_b, pearson`) is mirrored to the
//! full matrix with a diagonal of 1 for a correlation measure (`mirror :=
//! false` keeps it as given). Drawn entirely through the role heatmap
//! renderer ([`crate::render_heatmap`]): `geom_tile`, a diverging fill scale
//! around 0 (−1..1 for correlations, `domain :=` overrides), `geom_text` cell
//! labels when there are at most 144 cells (`labels :=`).

use super::look::Labels;
use super::stats::{fin, text_of, Out};
use super::{konst, opt_str, Row, Table};
use crate::{Column, Kind, Place, RenderOptions, Role};
use ggplot_rs::prelude::*;
use serde_json::Value as J;

/// Row keys read in order of preference (`row :=`, else the first present).
const ROW_KEYS: &[&str] = &["row", "column_a", "var1", "row_name"];
/// Col keys read in order of preference (`col :=`, else the first present).
const COL_KEYS: &[&str] = &["col", "column_b", "var2", "col_name"];
/// Value keys read in order of preference (`value :=`, else the first
/// present).
const VALUE_KEYS: &[&str] = &[
    "value",
    "pearson",
    "correlation",
    "corr",
    "spearman",
    "kendall",
    "count",
    "n",
];
/// Value keys that mean a correlation: −1..1 domain, a diagonal of 1 when
/// mirrored.
const CORRELATION_KEYS: &[&str] = &["pearson", "spearman", "kendall", "correlation", "corr"];

/// One matrix cell (text row, text col, finite value), the sorted row/col
/// levels and the fill domain `anofox_plot_matrix` draws.
#[derive(Debug)]
pub(super) struct Matrix {
    pub(super) row_key: String,
    pub(super) col_key: String,
    pub(super) value_key: String,
    pub(super) levels_r: Vec<String>,
    pub(super) levels_c: Vec<String>,
    pub(super) cells: Vec<(String, String, f64)>,
    pub(super) domain: Option<(f64, f64)>,
    pub(super) labels: bool,
}

/// `opt :=`, else the first of `fallbacks` the table has, else `fallbacks[0]`
/// (so a missing-column error below names a sensible default).
fn resolve(options: &Row, opt: &str, fallbacks: &[&str], t: &Table) -> String {
    opt_str(options, opt)
        .or_else(|| fallbacks.iter().find(|k| t.has(k)).map(|k| k.to_string()))
        .unwrap_or_else(|| fallbacks[0].to_string())
}

/// A numeric-aware sort: values that both parse as numbers compare
/// numerically, else lexicographically.
fn sort_aware(v: &mut [String]) {
    v.sort_by(|a, b| match (a.parse::<f64>(), b.parse::<f64>()) {
        (Ok(p), Ok(q)) => p.total_cmp(&q),
        _ => a.cmp(b),
    });
}

/// The row/col levels: one shared sorted list for both axes when the cells
/// were mirrored, or the two level sets already coincide (a square
/// confusion/setup matrix); else each axis sorted on its own.
fn levels_for(cells: &[(String, String, f64)], mirrored: bool) -> (Vec<String>, Vec<String>) {
    let mut rs: Vec<String> = Vec::new();
    let mut cs: Vec<String> = Vec::new();
    for (r, c, _) in cells {
        if !rs.contains(r) {
            rs.push(r.clone());
        }
        if !cs.contains(c) {
            cs.push(c.clone());
        }
    }
    let same_set = rs.len() == cs.len() && rs.iter().all(|r| cs.contains(r));
    if mirrored || same_set {
        let mut all = rs.clone();
        for c in &cs {
            if !all.contains(c) {
                all.push(c.clone());
            }
        }
        sort_aware(&mut all);
        (all.clone(), all)
    } else {
        let mut rs = rs;
        let mut cs = cs;
        sort_aware(&mut rs);
        sort_aware(&mut cs);
        (rs, cs)
    }
}

/// `domain := [lo, hi]`: a two-element numeric array, `lo < hi`.
fn parse_domain(options: &Row) -> Result<Option<(f64, f64)>, String> {
    match options.get("domain") {
        None | Some(J::Null) => Ok(None),
        Some(J::Array(a)) if a.len() == 2 => match (a[0].as_f64(), a[1].as_f64()) {
            (Some(lo), Some(hi)) if lo < hi => Ok(Some((lo, hi))),
            _ => Err("domain := [lo, hi] needs two numbers with lo < hi".to_string()),
        },
        Some(_) => Err("domain := [lo, hi] needs a 2-element numeric array".to_string()),
    }
}

/// The `matrix` table, as built from `t`: the row/col/value keys resolved
/// from the options or the fallbacks above, one model per call, a cell at
/// most once, a pair list mirrored to the full matrix, sorted levels and the
/// fill domain / cell-label default.
pub(super) fn build(t: &Table, options: &Row) -> Result<Matrix, String> {
    let row_key = resolve(options, "row", ROW_KEYS, t);
    let col_key = resolve(options, "col", COL_KEYS, t);
    let value_key = resolve(options, "value", VALUE_KEYS, t);
    t.need(&[row_key.as_str(), col_key.as_str(), value_key.as_str()])?;
    if let Some(key) = t.model_key() {
        let n = t.distinct(key);
        if n > 1 {
            return Err(format!(
                "rows hold {n} matrices ({key}): GROUP BY {key} for one plot each"
            ));
        }
    }
    let rows_v = t.text(&row_key);
    let cols_v = t.text(&col_key);
    let vals_v = t.num(&value_key)?;
    let mut cells: Vec<(String, String, f64)> = Vec::new();
    for i in 0..t.rows.len() {
        let (Some(r), Some(c), Some(v)) =
            (text_of(&rows_v[i]), text_of(&cols_v[i]), fin(&vals_v[i]))
        else {
            continue;
        };
        if cells.iter().any(|(rr, cc, _)| *rr == r && *cc == c) {
            return Err(format!(
                "two values for the cell ({r}, {c}): one row per cell"
            ));
        }
        cells.push((r, c, v));
    }
    let correlation = CORRELATION_KEYS.contains(&value_key.as_str());
    let mirror_off = matches!(options.get("mirror"), Some(J::Bool(false)));
    let has_transpose = cells
        .iter()
        .any(|(r, c, _)| r != c && cells.iter().any(|(r2, c2, _)| r2 == c && c2 == r));
    let intersects = cells
        .iter()
        .any(|(r, ..)| cells.iter().any(|(_, c, _)| c == r));
    let mirrored = !mirror_off && !has_transpose && intersects;
    if mirrored {
        let base = cells.clone();
        for (r, c, v) in &base {
            if r != c {
                cells.push((c.clone(), r.clone(), *v));
            }
        }
        if correlation {
            let mut levels: Vec<String> = Vec::new();
            for (r, c, _) in &cells {
                if !levels.contains(r) {
                    levels.push(r.clone());
                }
                if !levels.contains(c) {
                    levels.push(c.clone());
                }
            }
            for lvl in levels {
                if !cells.iter().any(|(r, c, _)| *r == lvl && *c == lvl) {
                    cells.push((lvl.clone(), lvl, 1.0));
                }
            }
        }
    }
    let (levels_r, levels_c) = levels_for(&cells, mirrored);
    cells.sort_by_key(|(r, c, _)| {
        (
            levels_r.iter().position(|x| x == r).unwrap_or(usize::MAX),
            levels_c.iter().position(|x| x == c).unwrap_or(usize::MAX),
        )
    });
    let domain = match parse_domain(options)? {
        Some(d) => Some(d),
        None if correlation => Some((-1.0, 1.0)),
        None => None,
    };
    let labels = match options.get("labels").and_then(J::as_bool) {
        Some(b) => b,
        None => levels_r.len() * levels_c.len() <= 144,
    };
    Ok(Matrix {
        row_key,
        col_key,
        value_key,
        levels_r,
        levels_c,
        cells,
        domain,
        labels,
    })
}

/// `anofox_plot_matrix(tbl)`: a heatmap of the `matrix` table, drawn through
/// the role renderer's heatmap ([`crate::render_heatmap`]).
pub(super) fn matrix(
    t: &Table,
    options: &Row,
    w: u32,
    h: u32,
    o: &RenderOptions,
    place: Place,
) -> Out {
    let m = build(t, options)?;
    if m.cells.is_empty() {
        return crate::note_svg(None, "No data", w, h).finish(place);
    }
    let col_data: Vec<Value> = m
        .cells
        .iter()
        .map(|(_, c, _)| Value::Str(c.clone()))
        .collect();
    let row_data: Vec<Value> = m
        .cells
        .iter()
        .map(|(r, _, _)| Value::Str(r.clone()))
        .collect();
    let val_data: Vec<Value> = m.cells.iter().map(|(_, _, v)| Value::Float(*v)).collect();
    let mut cols = vec![
        Column::new(m.col_key.as_str(), Role::X, col_data),
        Column::new(m.row_key.as_str(), Role::Y, row_data),
        Column::new(m.value_key.as_str(), Role::Value(Kind::Heatmap), val_data),
    ];
    if m.labels {
        cols.push(konst("labels", Role::DataLabels, Value::Str("true".into())));
    }
    let value_col = cols
        .iter()
        .find(|c| c.role == Role::Value(Kind::Heatmap))
        .expect("the value column just pushed");
    let panel = crate::render_heatmap(o, value_col, &cols, None, w, h, m.domain)?;
    let labels = Labels::from(options);
    // Explicit discrete scales from the sorted levels: render_heatmap infers
    // axis order from each value's first appearance in the cell list, which
    // only matches the sorted order for a complete (or mirrored) grid. A
    // genuinely sparse row/col/value table (not every row has every col)
    // still gets the levels in their proper sorted place this way.
    let mut rows_top_down = m.levels_r.clone();
    rows_top_down.reverse();
    let panel = match panel {
        crate::Panel::Plot {
            plot,
            width,
            height,
            attrs,
            margin,
        } => {
            let plot = plot
                .scale_x_discrete(
                    ScaleDiscrete::new()
                        .with_limits(m.levels_c.iter().map(String::as_str).collect()),
                )
                .scale_y_discrete(
                    ScaleDiscrete::new()
                        .with_limits(rows_top_down.iter().map(String::as_str).collect()),
                );
            crate::Panel::Plot {
                plot: Box::new(labels.apply(plot, "", "")),
                width,
                height,
                attrs,
                margin,
            }
        }
        other => other,
    };
    panel.finish(place)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(json: &str) -> Vec<Row> {
        serde_json::from_str(json).unwrap()
    }

    fn table(rows: &[Row]) -> Table<'_> {
        Table { rows, renamed: &[] }
    }

    const PAIRS: &str = r#"[
        {"column_a":"amount","column_b":"id","pearson":0.4},
        {"column_a":"amount","column_b":"qty","pearson":-0.2},
        {"column_a":"id","column_b":"qty","pearson":0.7}
    ]"#;

    #[test]
    fn mirrors_a_correlation_pair_list() {
        let r = rows(PAIRS);
        let t = table(&r);
        let m = build(&t, &Row::new()).unwrap();
        assert_eq!(m.cells.len(), 9, "{:?}", m.cells);
        assert_eq!(m.levels_r, vec!["amount", "id", "qty"]);
        assert_eq!(m.levels_c, vec!["amount", "id", "qty"]);
        assert_eq!(m.domain, Some((-1.0, 1.0)));
        let diag: Vec<f64> = m
            .cells
            .iter()
            .filter(|(r, c, _)| r == c)
            .map(|(_, _, v)| *v)
            .collect();
        assert_eq!(diag, vec![1.0, 1.0, 1.0]);
    }

    #[test]
    fn mirror_false_keeps_the_pairs_as_given() {
        let r = rows(PAIRS);
        let t = table(&r);
        let opts: Row = serde_json::from_str(r#"{"mirror": false}"#).unwrap();
        let m = build(&t, &opts).unwrap();
        assert_eq!(m.cells.len(), 3);
    }

    #[test]
    fn a_confusion_matrix_is_not_mirrored() {
        let r = rows(
            r#"[{"row":"cat","col":"cat","value":8},{"row":"cat","col":"dog","value":2},
                {"row":"dog","col":"cat","value":1},{"row":"dog","col":"dog","value":9}]"#,
        );
        let t = table(&r);
        let m = build(&t, &Row::new()).unwrap();
        assert_eq!(m.cells.len(), 4);
        assert_eq!(m.domain, None);
        assert_eq!(m.levels_r, vec!["cat", "dog"]);
        assert_eq!(m.levels_c, vec!["cat", "dog"]);
    }

    #[test]
    fn a_duplicate_cell_is_an_error() {
        let r = rows(r#"[{"row":"a","col":"b","value":1},{"row":"a","col":"b","value":2}]"#);
        let t = table(&r);
        let e = build(&t, &Row::new()).unwrap_err();
        assert!(e.contains("one row per cell"), "{e}");
    }

    #[test]
    fn two_model_ids_is_an_error() {
        let r = rows(
            r#"[{"model_id":"m1","row":"a","col":"b","value":1},
                {"model_id":"m2","row":"a","col":"b","value":2}]"#,
        );
        let t = table(&r);
        let e = build(&t, &Row::new()).unwrap_err();
        assert!(e.contains("GROUP BY model_id"), "{e}");
    }

    #[test]
    fn domain_option_overrides() {
        let r = rows(PAIRS);
        let t = table(&r);
        let opts: Row = serde_json::from_str(r#"{"domain": [0, 1]}"#).unwrap();
        let m = build(&t, &opts).unwrap();
        assert_eq!(m.domain, Some((0.0, 1.0)));
    }
}
