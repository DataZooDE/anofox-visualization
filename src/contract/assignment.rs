//! `assignment`: an optimizer's item → bin/wave assignment, one row per item
//! (`model_id?, bin | wave, item, size, capacity?`) — anofox-optimize's bin
//! packing (`pack_*`) and pick-wave batching (`wave_*`) as they come. Drawn
//! with the existing stacked-bar roles: items stacked per bin/wave
//! (`::CATEGORY` = item, `::BARCHART_STACKED` = size), the capacity as a
//! quiet reference line so the fill level is visible at a glance; several
//! `model_id`s are faceted (`::FACET`) to compare algorithms.

use super::{opt_str, Row, Table};
use crate::{Column, Kind, Role};

/// The bin/wave columns a row may carry, in order of preference.
const KEYS: &[&str] = &["bin", "wave"];

/// `anofox_plot_assignment(tbl)`: see the module docs.
pub(super) fn columns(t: &Table, options: &Row) -> Result<Vec<Column>, String> {
    let key = opt_str(options, "x")
        .or_else(|| KEYS.iter().find(|k| t.has(k)).map(|k| k.to_string()))
        .ok_or_else(|| {
            format!(
                "needs a `bin` or `wave` column, or x := '<column>' (got: {})",
                t.keys()
            )
        })?;
    t.need(&[key.as_str(), "item", "size"])?;
    // A bin/wave number is a discrete label (one bar per bin), not a
    // continuous axis: text, like the hand-written examples that cast it
    // (`'W' || lpad(wave::VARCHAR, 2, '0')`).
    let mut cols = vec![
        Column::new(key.as_str(), Role::X, t.text(&key)),
        Column::new("item", Role::Category, t.text("item")),
        Column::new("size", Role::Value(Kind::BarStacked), t.num("size")?),
    ];
    if t.has("capacity") {
        cols.push(Column::new("capacity", Role::RefLine, t.num("capacity")?));
    }
    if let Some(m) = t.model_key().filter(|m| t.distinct(m) > 1) {
        cols.push(Column::new(m, Role::Facet, t.text(m)));
    }
    Ok(cols)
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
        let cols = columns(&t, &Row::new()).unwrap();
        assert_eq!(cols[0].name, "bin");
        assert_eq!(cols[0].role, Role::X);
        assert_eq!(cols[1].role, Role::Category);
        assert!(matches!(cols[2].role, Role::Value(Kind::BarStacked)));
        let cap = cols.iter().find(|c| c.role == Role::RefLine).unwrap();
        assert_eq!(cap.values.len(), 3);
        assert!(cols.iter().all(|c| c.role != Role::Facet));
    }

    #[test]
    fn waves_facet_by_model() {
        let r = rows(
            r#"[{"model_id":"as_given","wave":1,"item":"a","size":3},
                {"model_id":"best_of","wave":1,"item":"a","size":2}]"#,
        );
        let t = table(&r);
        let cols = columns(&t, &Row::new()).unwrap();
        assert_eq!(cols[0].name, "wave");
        let facet = cols.iter().find(|c| c.role == Role::Facet).unwrap();
        assert_eq!(facet.name, "model_id");
    }

    #[test]
    fn single_model_is_not_faceted() {
        let r = rows(r#"[{"model_id":"as_given","bin":1,"item":"a","size":3}]"#);
        let t = table(&r);
        let cols = columns(&t, &Row::new()).unwrap();
        assert!(cols.iter().all(|c| c.role != Role::Facet));
    }

    #[test]
    fn missing_bin_and_wave_is_an_error() {
        let r = rows(r#"[{"item":"a","size":3}]"#);
        let t = table(&r);
        let e = columns(&t, &Row::new()).err().unwrap();
        assert!(e.contains("bin"), "{e}");
    }
}
