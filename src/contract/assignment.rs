//! `assignment`: an optimizer's item → bin/wave assignment, one row per item
//! (`model_id?, bin | wave, item, size, capacity?`) — anofox-optimize's bin
//! packing (`pack_*`) and pick-wave batching (`wave_*`) as they come. Drawn
//! with the existing stacked-bar roles: items stacked per bin/wave
//! (`::CATEGORY` = item, `::BARCHART_STACKED` = size), several `model_id`s
//! faceted (`::FACET`) to compare algorithms.
//!
//! Ordering: the bin/wave axis sorts numerically when every row's key parses
//! as a number (`1, 2, 10`, not the row order's `1, 10, 2`), else
//! lexicographically as text. A row whose key is NULL cannot be placed on
//! that axis, so it is dropped; the count is reported as a data-warning.
//! `item` may be NULL (it still stacks, as its own unlabelled segment).
//!
//! Capacity: a value shared by every (kept) row draws once, as a quiet
//! `::REFLINE`. One that varies by bin and/or model would make a shared
//! line misleading, so this module hands `contract::render_plot` explicit
//! per-bin markers instead (returned alongside the columns, not inferred
//! from a `RefLine` column's value count — that count means something
//! different for other schemas, e.g. several threshold lines, which must
//! stay lines): a quiet point per bin, in the reference colour (never a
//! palette slot `::CATEGORY` is already using), named by a caption since it
//! has no legend entry. Still ggplot-rs's own point geom (drawn in
//! `contract::tweak`); no hand-written SVG.

use super::{opt_str, Row, Table};
use crate::{Column, Kind, Role};
use ggplot_rs::prelude::Value;
use serde_json::Value as J;

/// The bin/wave columns a row may carry, in order of preference.
const KEYS: &[&str] = &["bin", "wave"];

/// A bin/wave value's axis text: a JSON number keeps its integral form
/// (`1`, not `1.0`, the way `to_json` can write an exact DOUBLE) when it is
/// one; otherwise its own text.
fn label_of(v: Option<&J>) -> Option<String> {
    match v {
        None | Some(J::Null) => None,
        Some(J::String(s)) => Some(s.clone()),
        Some(J::Number(n)) => match n.as_f64() {
            Some(f) if f.is_finite() && (f - f.round()).abs() < 1e-9 => {
                Some(format!("{}", f.round() as i64))
            }
            _ => Some(n.to_string()),
        },
        Some(other) => Some(other.to_string()),
    }
}

/// `key`'s value at row `i`, as a number — `None` when it is missing or not
/// numeric text.
fn num_of(t: &Table, key: &str, i: usize) -> Option<f64> {
    match t.rows[i].get(key) {
        Some(J::Number(n)) => n.as_f64(),
        Some(J::String(s)) => s.trim().parse::<f64>().ok(),
        _ => None,
    }
}

/// The rows with a non-NULL `key`, in axis order (numeric ascending when
/// every kept row's key is a number, else lexicographic text), their axis
/// labels in that order, and how many rows a NULL `key` dropped.
fn ordered_rows(t: &Table, key: &str) -> (Vec<usize>, Vec<String>, usize) {
    let n = t.rows.len();
    let mut kept: Vec<usize> = Vec::new();
    let mut dropped = 0usize;
    for i in 0..n {
        match t.rows[i].get(key) {
            Some(v) if !v.is_null() => kept.push(i),
            _ => dropped += 1,
        }
    }
    let all_numeric = !kept.is_empty()
        && kept
            .iter()
            .all(|&i| num_of(t, key, i).is_some_and(f64::is_finite));
    if all_numeric {
        kept.sort_by(|&a, &b| {
            num_of(t, key, a)
                .unwrap()
                .total_cmp(&num_of(t, key, b).unwrap())
                .then(a.cmp(&b))
        });
    } else {
        kept.sort_by(|&a, &b| {
            let la = label_of(t.rows[a].get(key)).unwrap_or_default();
            let lb = label_of(t.rows[b].get(key)).unwrap_or_default();
            la.cmp(&lb).then(a.cmp(&b))
        });
    }
    let labels: Vec<String> = kept
        .iter()
        .map(|&i| label_of(t.rows[i].get(key)).unwrap_or_default())
        .collect();
    (kept, labels, dropped)
}

/// Per-x markers for `contract::Refs::marks` (named by [`columns`]'s doc):
/// one quiet point per bin — `(x label, y)`, deduped by label, in bin order.
pub(super) type Marks = (String, Vec<(Value, f64)>);

/// [`columns`]'s return: the role columns, any data-warnings, and —
/// `Some(("capacity", marks))` exactly when capacity varies across the kept
/// rows — the per-bin markers `contract::render_plot` draws instead of
/// relying on `Refs::marks` being inferred from a `RefLine` column.
pub(super) type Columns = (Vec<Column>, Vec<String>, Option<Marks>);

/// `anofox_plot_assignment(tbl)`: see the module docs and [`Columns`].
pub(super) fn columns(t: &Table, options: &Row) -> Result<Columns, String> {
    let key = opt_str(options, "x")
        .or_else(|| KEYS.iter().find(|k| t.has(k)).map(|k| k.to_string()))
        .ok_or_else(|| {
            format!(
                "needs a `bin` or `wave` column, or x := '<column>' (got: {})",
                t.keys()
            )
        })?;
    t.need(&[key.as_str(), "item", "size"])?;

    let (order, labels, dropped) = ordered_rows(t, &key);
    if order.is_empty() {
        return Err(format!("every row has a NULL `{key}`"));
    }
    let mut warnings = Vec::new();
    if dropped > 0 {
        let row = if dropped == 1 { "row" } else { "rows" };
        warnings.push(format!(
            "anofox_plot_assignment: {dropped} {row} with a NULL `{key}` dropped"
        ));
    }

    let item_v = t.text("item");
    let size_v = t.num("size")?;
    let gather = |v: &[Value]| -> Vec<Value> { order.iter().map(|&i| v[i].clone()).collect() };

    let mut cols = vec![
        Column::new(
            key.as_str(),
            Role::X,
            labels.iter().cloned().map(Value::Str).collect(),
        ),
        Column::new("item", Role::Category, gather(&item_v)),
        Column::new("size", Role::Value(Kind::BarStacked), gather(&size_v)),
    ];

    // A value shared by every kept row draws once, as a quiet reference
    // line; one that varies draws as a marker per bin instead (see the
    // module doc) — decided here, over the kept (`order`-filtered) rows
    // only, so a dropped row's own capacity never counts (#375 review).
    let mut marks = None;
    if t.has("capacity") {
        let cap = gather(&t.num("capacity")?);
        let mut distinct: Vec<f64> = Vec::new();
        for v in &cap {
            if let Some(f) = v.as_f64().filter(|f| f.is_finite()) {
                if !distinct.iter().any(|x: &f64| (x - f).abs() < 1e-12) {
                    distinct.push(f);
                }
            }
        }
        if distinct.len() == 1 {
            cols.push(Column::new("capacity", Role::RefLine, cap));
        } else if distinct.len() > 1 {
            let mut pairs: Vec<(Value, f64)> = Vec::new();
            for (label, v) in labels.iter().zip(&cap) {
                let Some(f) = v.as_f64().filter(|f| f.is_finite()) else {
                    continue;
                };
                let xv = Value::Str(label.clone());
                if !pairs
                    .iter()
                    .any(|(mx, my): &(Value, f64)| *mx == xv && (my - f).abs() < 1e-12)
                {
                    pairs.push((xv, f));
                }
            }
            marks = Some(("capacity".to_string(), pairs));
        }
    }
    if let Some(m) = t.model_key().filter(|m| t.distinct(m) > 1) {
        cols.push(Column::new(m, Role::Facet, gather(&t.text(m))));
    }
    Ok((cols, warnings, marks))
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

    #[test]
    fn bins_stack_items_with_capacity_line() {
        let r = rows(
            r#"[{"bin":1,"item":"a","size":3,"capacity":10},
                {"bin":1,"item":"b","size":4,"capacity":10},
                {"bin":2,"item":"c","size":6,"capacity":10}]"#,
        );
        let t = table(&r);
        let (cols, warnings, _) = columns(&t, &Row::new()).unwrap();
        assert_eq!(cols[0].name, "bin");
        assert_eq!(cols[0].role, Role::X);
        assert_eq!(cols[1].role, Role::Category);
        assert!(matches!(cols[2].role, Role::Value(Kind::BarStacked)));
        let cap = cols.iter().find(|c| c.role == Role::RefLine).unwrap();
        assert_eq!(cap.values.len(), 3);
        assert!(cols.iter().all(|c| c.role != Role::Facet));
        assert!(warnings.is_empty());
    }

    #[test]
    fn waves_facet_by_model() {
        let r = rows(
            r#"[{"model_id":"as_given","wave":1,"item":"a","size":3},
                {"model_id":"best_of","wave":1,"item":"a","size":2}]"#,
        );
        let t = table(&r);
        let (cols, _, _) = columns(&t, &Row::new()).unwrap();
        assert_eq!(cols[0].name, "wave");
        let facet = cols.iter().find(|c| c.role == Role::Facet).unwrap();
        assert_eq!(facet.name, "model_id");
    }

    #[test]
    fn single_model_is_not_faceted() {
        let r = rows(r#"[{"model_id":"as_given","bin":1,"item":"a","size":3}]"#);
        let t = table(&r);
        let (cols, _, _) = columns(&t, &Row::new()).unwrap();
        assert!(cols.iter().all(|c| c.role != Role::Facet));
    }

    #[test]
    fn missing_bin_and_wave_is_an_error() {
        let r = rows(r#"[{"item":"a","size":3}]"#);
        let t = table(&r);
        let e = columns(&t, &Row::new()).err().unwrap();
        assert!(e.contains("bin"), "{e}");
    }

    /// Unsorted numeric bins (as a producer might hand them over, e.g.
    /// ordered by insertion rather than bin number) still draw 0, 1, 2, …
    /// left to right, and an integral DOUBLE bin shows "1", not "1.0".
    #[test]
    fn numeric_bins_sort_ascending_regardless_of_row_order() {
        let r = rows(
            r#"[{"bin":10.0,"item":"j","size":1},
                {"bin":2,"item":"b","size":1},
                {"bin":1,"item":"a","size":1},
                {"bin":0,"item":"z","size":1}]"#,
        );
        let t = table(&r);
        let (cols, _, _) = columns(&t, &Row::new()).unwrap();
        let x = &cols[0];
        let labels: Vec<String> = x
            .values
            .iter()
            .map(|v| match v {
                Value::Str(s) => s.clone(),
                _ => panic!("expected a text x value"),
            })
            .collect();
        assert_eq!(labels, vec!["0", "1", "2", "10"], "{labels:?}");
    }

    /// A text bin/wave key (not every value numeric) sorts lexicographically.
    #[test]
    fn text_bins_sort_lexicographically() {
        let r = rows(
            r#"[{"wave":"W10","item":"a","size":1},
                {"wave":"W2","item":"b","size":1},
                {"wave":"W1","item":"c","size":1}]"#,
        );
        let t = table(&r);
        let (cols, _, _) = columns(&t, &Row::new()).unwrap();
        let labels: Vec<String> = cols[0]
            .values
            .iter()
            .map(|v| match v {
                Value::Str(s) => s.clone(),
                _ => panic!("expected a text x value"),
            })
            .collect();
        // Lexicographic, not numeric: "W1" < "W10" < "W2".
        assert_eq!(labels, vec!["W1", "W10", "W2"], "{labels:?}");
    }

    /// A NULL bin/wave cannot be placed on the axis: the row is dropped and
    /// the count is reported as a data-warning. A NULL item is kept.
    #[test]
    fn null_bin_rows_are_dropped_with_a_warning_null_items_are_kept() {
        let r = rows(
            r#"[{"bin":1,"item":"a","size":3},
                {"bin":null,"item":"b","size":4},
                {"bin":1,"item":null,"size":2}]"#,
        );
        let t = table(&r);
        let (cols, warnings, _) = columns(&t, &Row::new()).unwrap();
        assert_eq!(cols[2].values.len(), 2, "the NULL-bin row was dropped");
        assert_eq!(warnings.len(), 1);
        assert!(
            warnings[0].contains("1 row with a NULL `bin` dropped"),
            "{warnings:?}"
        );
    }

    /// A `bin`/`wave` that is NULL in every row is no different from one
    /// that is missing entirely (`t.has` already says so) — the same "needs
    /// a `bin` or `wave` column" error, not a panic or an empty chart.
    #[test]
    fn every_row_null_bin_is_an_error() {
        let r = rows(r#"[{"bin":null,"item":"a","size":1}]"#);
        let t = table(&r);
        let e = columns(&t, &Row::new()).err().unwrap();
        assert!(e.contains("needs a `bin` or `wave` column"), "{e}");
    }

    /// Some rows NULL, others not: the non-NULL ones draw; `ordered_rows`
    /// itself (not just the `t.has` precheck) is what guards against an
    /// empty frame.
    #[test]
    fn ordered_rows_guards_an_all_null_key_directly() {
        let r = rows(r#"[{"bin":null,"item":"a","size":1}]"#);
        let t = table(&r);
        let (order, labels, dropped) = ordered_rows(&t, "bin");
        assert!(order.is_empty());
        assert!(labels.is_empty());
        assert_eq!(dropped, 1);
    }

    /// Capacity — constant or varying — is always a RefLine column, one
    /// value per row; `contract::take_refs`/`tweak` (not this module) decide
    /// whether that becomes a single shared line or a marker per bin, so
    /// both cases are covered at the render level
    /// (`assignment_bin_order_capacity_and_nulls` in `contract.rs`).
    #[test]
    fn constant_capacity_is_a_refline_column_no_marks() {
        let r = rows(
            r#"[{"bin":1,"item":"a","size":3,"capacity":10},
                {"bin":2,"item":"b","size":4,"capacity":10}]"#,
        );
        let t = table(&r);
        let (cols, _, marks) = columns(&t, &Row::new()).unwrap();
        let cap = cols
            .iter()
            .find(|c| c.name == "capacity")
            .expect("a capacity column");
        assert_eq!(cap.role, Role::RefLine);
        assert_eq!(cap.values.len(), 2);
        assert!(marks.is_none());
    }

    /// A capacity that varies across the kept rows is returned as explicit
    /// marks (not a `RefLine` column): one `(x, y)` pair per bin, deduped.
    #[test]
    fn varying_capacity_is_returned_as_explicit_marks() {
        let r = rows(
            r#"[{"bin":1,"item":"a","size":3,"capacity":10},
                {"bin":2,"item":"b","size":4,"capacity":20}]"#,
        );
        let t = table(&r);
        let (cols, _, marks) = columns(&t, &Row::new()).unwrap();
        assert!(
            cols.iter().all(|c| c.name != "capacity"),
            "a capacity column should not exist when it varies",
        );
        let (name, pairs) = marks.expect("marks when capacity varies");
        assert_eq!(name, "capacity");
        assert_eq!(pairs.len(), 2);
        assert!(pairs.contains(&(Value::Str("1".into()), 10.0)));
        assert!(pairs.contains(&(Value::Str("2".into()), 20.0)));
    }

    /// #375 review: a NULL-bin row (dropped) with a *different* capacity
    /// from every surviving row must not count — the capacity column only
    /// ever carries the kept rows' values, so a table that is constant among
    /// the kept rows stays a single RefLine, not marks.
    #[test]
    fn a_dropped_null_bin_rows_capacity_does_not_count() {
        let r = rows(
            r#"[{"bin":1,"item":"a","size":3,"capacity":10},
                {"bin":1,"item":"b","size":4,"capacity":10},
                {"bin":null,"item":"z","size":1,"capacity":999}]"#,
        );
        let t = table(&r);
        let (cols, warnings, marks) = columns(&t, &Row::new()).unwrap();
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(marks.is_none(), "{marks:?}");
        let cap = cols
            .iter()
            .find(|c| c.name == "capacity")
            .expect("a capacity column");
        assert_eq!(cap.values.len(), 2, "only the two kept rows");
        assert!(
            cap.values.iter().all(|v| v.as_f64() == Some(10.0)),
            "the dropped row's 999 must not appear: {:?}",
            cap.values
        );
    }
}
