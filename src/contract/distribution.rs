//! `distribution`: a categorical distribution over options, one row per
//! option (`id, option, probability, model_id?`) — anofox-decide's
//! `decide_choice_distribution` MAP and `decide_table`'s `distribution`
//! column reach this shape with `unnest(map_entries(m))`, naming the
//! unnested STRUCT so its `key`/`value` fields can be reached (bare
//! `u.key` does not parse — `u` is the unnest's own column, not a struct —
//! so the unnest needs its own column alias, `u(e)`, one level in):
//! `SELECT t.question_id AS id, u.e.key AS option, u.e.value AS probability
//! FROM t, unnest(map_entries(t.distribution)) AS u(e)`. `question_id` and
//! `model` (decide's own spellings) are read as `id` / `model_id`, so the
//! bridge can skip the `AS id` / a `model_id` rename too. Drawn as bars per
//! option (one `id`), or a 100% stacked bar per `id` (several ids,
//! `::CATEGORY` = option) — several `model_id`s are faceted (`::FACET`) to
//! compare models.
//!
//! The several-ids stack is percent-normalised (`::BARCHART_STACKED_PERCENT`)
//! rather than a plain stack on a fixed 0..1 axis: the role renderer has no
//! fixed-domain role for a cartesian y axis (only `::RANGE`, for a
//! gauge/bullet/map's own scale), so a fixed 0..1 domain would need
//! hand-written ggplot-rs code, against the "existing roles only" principle
//! this schema otherwise keeps to. The trade-off: a *truncated* distribution
//! (options that do not sum to 1, e.g. a top-k list) is stretched to fill
//! the bar, so its missing mass does not show. Pass every option, not a
//! top-k subset, to keep the stack honest.

use super::Table;
use crate::{Column, Kind, Role};
use ggplot_rs::prelude::Value;

/// Above this many ids, the stacked-bar axis is unreadable — a data-warning
/// says so, but the chart still draws (it is more useful squeezed than not
/// drawn at all).
const MANY_IDS_WARN: usize = 40;

/// `anofox_plot_distribution(tbl)`: see the module docs.
pub(super) fn columns(t: &Table) -> Result<(Vec<Column>, Vec<String>), String> {
    t.need(&["id", "option", "probability"])?;
    let mut warnings = Vec::new();

    let n_ids = t.distinct("id");
    let many_ids = n_ids > 1;
    if many_ids && n_ids > MANY_IDS_WARN {
        warnings.push(format!(
            "anofox_plot_distribution: {n_ids} ids plotted — the axis may be unreadable \
             above {MANY_IDS_WARN}; filter to fewer ids or facet by model_id"
        ));
    }

    let id_v = t.text("id");
    let option_v = t.text("option");
    let prob_v = t.num("probability")?;

    // A NULL id cannot be placed on the (id) axis of a many-ids stack; drop
    // it and say how many. In the one-id plain bar, id is not drawn at all,
    // so a NULL id there costs no rows.
    let kept: Vec<usize> = if many_ids {
        let k: Vec<usize> = (0..t.rows.len())
            .filter(|&i| id_v[i] != Value::Na)
            .collect();
        let dropped = t.rows.len() - k.len();
        if dropped > 0 {
            let row = if dropped == 1 { "row" } else { "rows" };
            warnings.push(format!(
                "anofox_plot_distribution: {dropped} {row} with a NULL `id` dropped"
            ));
        }
        k
    } else {
        (0..t.rows.len()).collect()
    };
    if kept.is_empty() {
        return Err("every row has a NULL `id`".to_string());
    }
    let gather = |v: &[Value]| -> Vec<Value> { kept.iter().map(|&i| v[i].clone()).collect() };

    let mut cols = if many_ids {
        vec![
            Column::new("id", Role::X, gather(&id_v)),
            Column::new("option", Role::Category, gather(&option_v)),
            Column::new(
                "probability",
                Role::Value(Kind::BarStackedPercent),
                gather(&prob_v),
            ),
        ]
    } else {
        vec![
            Column::new("option", Role::X, gather(&option_v)),
            Column::new("probability", Role::Value(Kind::Bar), gather(&prob_v)),
        ]
    };
    if let Some(m) = t.model_key().filter(|m| t.distinct(m) > 1) {
        cols.push(Column::new(m, Role::Facet, gather(&t.text(m))));
    }
    Ok((cols, warnings))
}

#[cfg(test)]
mod tests {
    use super::super::Row;
    use super::*;

    fn rows(json: &str) -> Vec<Row> {
        serde_json::from_str(json).unwrap()
    }

    fn table(rows: &[Row]) -> Table<'_> {
        Table { rows, renamed: &[] }
    }

    #[test]
    fn one_id_is_a_plain_bar() {
        let r = rows(
            r#"[{"id":"q1","option":"yes","probability":0.7},
                {"id":"q1","option":"no","probability":0.3}]"#,
        );
        let t = table(&r);
        let (cols, warnings) = columns(&t).unwrap();
        assert_eq!(cols[0].name, "option");
        assert_eq!(cols[0].role, Role::X);
        assert!(matches!(cols[1].role, Role::Value(Kind::Bar)));
        assert!(cols.iter().all(|c| c.role != Role::Category));
        assert!(warnings.is_empty());
    }

    #[test]
    fn several_ids_stack_to_100_percent() {
        let r = rows(
            r#"[{"id":"q1","option":"yes","probability":0.7},
                {"id":"q1","option":"no","probability":0.3},
                {"id":"q2","option":"yes","probability":0.4},
                {"id":"q2","option":"no","probability":0.6}]"#,
        );
        let t = table(&r);
        let (cols, _) = columns(&t).unwrap();
        assert_eq!(cols[0].name, "id");
        assert_eq!(cols[0].role, Role::X);
        let cat = cols.iter().find(|c| c.role == Role::Category).unwrap();
        assert_eq!(cat.name, "option");
        assert!(matches!(cols[2].role, Role::Value(Kind::BarStackedPercent)));
    }

    #[test]
    fn several_models_are_faceted() {
        let r = rows(
            r#"[{"model_id":"stub","id":"q1","option":"yes","probability":0.7},
                {"model_id":"laya","id":"q1","option":"yes","probability":0.6}]"#,
        );
        let t = table(&r);
        let (cols, _) = columns(&t).unwrap();
        let facet = cols.iter().find(|c| c.role == Role::Facet).unwrap();
        assert_eq!(facet.name, "model_id");
    }

    #[test]
    fn missing_probability_is_an_error() {
        let r = rows(r#"[{"id":"q1","option":"yes"}]"#);
        let t = table(&r);
        let e = columns(&t).err().unwrap();
        assert!(e.contains("probability"), "{e}");
    }

    /// A NULL option is kept (an unlabelled segment of the stack), not
    /// dropped.
    #[test]
    fn null_option_is_kept() {
        let r = rows(
            r#"[{"id":"q1","option":"yes","probability":0.7},
                {"id":"q1","option":null,"probability":0.3}]"#,
        );
        let t = table(&r);
        let (cols, warnings) = columns(&t).unwrap();
        assert_eq!(cols[1].values.len(), 2);
        assert!(warnings.is_empty());
    }

    /// A NULL id in a many-ids table is dropped (it cannot be placed on the
    /// id axis) and reported; the surviving ids are unaffected.
    #[test]
    fn null_id_is_dropped_with_a_warning_when_there_are_several_ids() {
        let r = rows(
            r#"[{"id":"q1","option":"yes","probability":0.7},
                {"id":"q1","option":"no","probability":0.3},
                {"id":null,"option":"yes","probability":0.5},
                {"id":"q2","option":"yes","probability":0.4},
                {"id":"q2","option":"no","probability":0.6}]"#,
        );
        let t = table(&r);
        let (cols, warnings) = columns(&t).unwrap();
        assert_eq!(cols[0].values.len(), 4, "the NULL-id row was dropped");
        assert_eq!(warnings.len(), 1);
        assert!(
            warnings[0].contains("1 row with a NULL `id` dropped"),
            "{warnings:?}"
        );
    }

    #[test]
    fn more_than_40_ids_warns_about_the_axis() {
        let vals: Vec<String> = (0..41)
            .map(|i| format!(r#"{{"id":"q{i}","option":"yes","probability":0.5}}"#))
            .collect();
        let r = rows(&format!("[{}]", vals.join(",")));
        let t = table(&r);
        let (_, warnings) = columns(&t).unwrap();
        assert!(
            warnings.iter().any(|w| w.contains("41 ids plotted")),
            "{warnings:?}"
        );
    }
}
