//! `distribution`: a categorical distribution over options, one row per
//! option (`id, option, probability, model_id?`) — anofox-decide's
//! `decide_choice_distribution` MAP and `decide_table`'s `distribution`
//! column reach this shape with `unnest(map_entries(m))`:
//! `SELECT id, m.key ::VARCHAR AS option, m.value AS probability FROM t,
//! unnest(map_entries(distribution)) m`. Drawn as bars per option (one
//! `id`), or a 100% stacked bar per `id` (several ids, `::CATEGORY` =
//! option) — several `model_id`s are faceted (`::FACET`) to compare models.

use super::Table;
use crate::{Column, Kind, Role};

/// `anofox_plot_distribution(tbl)`: see the module docs.
pub(super) fn columns(t: &Table) -> Result<Vec<Column>, String> {
    t.need(&["id", "option", "probability"])?;
    let many_ids = t.distinct("id") > 1;
    let mut cols = if many_ids {
        vec![
            Column::new("id", Role::X, t.text("id")),
            Column::new("option", Role::Category, t.text("option")),
            Column::new(
                "probability",
                Role::Value(Kind::BarStackedPercent),
                t.num("probability")?,
            ),
        ]
    } else {
        vec![
            Column::new("option", Role::X, t.text("option")),
            Column::new("probability", Role::Value(Kind::Bar), t.num("probability")?),
        ]
    };
    if let Some(m) = t.model_key().filter(|m| t.distinct(m) > 1) {
        cols.push(Column::new(m, Role::Facet, t.text(m)));
    }
    Ok(cols)
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
        let cols = columns(&t).unwrap();
        assert_eq!(cols[0].name, "option");
        assert_eq!(cols[0].role, Role::X);
        assert!(matches!(cols[1].role, Role::Value(Kind::Bar)));
        assert!(cols.iter().all(|c| c.role != Role::Category));
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
        let cols = columns(&t).unwrap();
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
        let cols = columns(&t).unwrap();
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
}
