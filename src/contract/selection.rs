//! `selection` (#499): which items an optimizer picked, one row per item —
//! `model_id?, item, selected | listed, value?, weight?, capacity?,
//! margin?, demand?` — anofox-optimize's knapsack (`selected`) and
//! assortment (`listed`) results. Drawn with the existing roles: a bar per
//! item (`::XAXIS` = item, `::BARCHART_STACKED` = its measure, one segment
//! each) coloured by whether it was picked (`::CATEGORY` with a `::COLORS`
//! level map: picked = the theme's good colour, the rest neutral), picked
//! items first, each part by measure, largest first.
//!
//! The measure: `value`, else `margin × demand` (an assortment's expected
//! margin), else `margin`, else `weight`. With `weight` and `capacity` the
//! default title says how much of the capacity the picked items use (one
//! group only; `title :=` replaces it).
//!
//! Groups: as for `assignment`, several `model_id`s facet, and so does a
//! producer group column (`group_col := …`, any other column with more than
//! one value) — two groups' items are never mixed into one picture.

use super::{opt_str, Row, Table};
use crate::{Column, Kind, Role};
use ggplot_rs::prelude::Value;
use serde_json::Value as J;

/// Every column the schema itself reads (never a group column).
const OWN: &[&str] = &[
    "item",
    "selected",
    "listed",
    "value",
    "weight",
    "capacity",
    "margin",
    "demand",
    "model_id",
    "model_name",
];

/// A picked flag: a JSON bool, a number (non-zero) or text (`true`/`t`/`yes`/`1`).
fn picked(v: Option<&J>) -> bool {
    match v {
        Some(J::Bool(b)) => *b,
        Some(J::Number(n)) => n.as_f64().is_some_and(|f| f != 0.0),
        Some(J::String(s)) => matches!(
            s.trim().to_ascii_lowercase().as_str(),
            "true" | "t" | "yes" | "1"
        ),
        _ => false,
    }
}

/// `anofox_plot_selection(tbl)`: see the module docs. Returns the role
/// columns and any data-warnings.
pub(super) fn columns(t: &Table, options: &Row) -> Result<(Vec<Column>, Vec<String>), String> {
    let flag = ["selected", "listed"]
        .into_iter()
        .find(|k| t.has(k))
        .ok_or_else(|| {
            format!(
                "needs a `selected` (or `listed`) column (got: {})",
                t.keys()
            )
        })?;
    t.need(&["item", flag])?;
    let n = t.rows.len();
    let num = |k: &str| -> Result<Option<Vec<Option<f64>>>, String> {
        if !t.has(k) {
            return Ok(None);
        }
        Ok(Some(
            t.num(k)?
                .iter()
                .map(|v| v.as_f64().filter(|f| f.is_finite()))
                .collect(),
        ))
    };
    let (value, margin, demand, weight) = (
        num("value")?,
        num("margin")?,
        num("demand")?,
        num("weight")?,
    );
    let (measure, mname): (Vec<Option<f64>>, &str) = if let Some(v) = value {
        (v, "value")
    } else if let (Some(m), Some(d)) = (&margin, &demand) {
        (
            m.iter().zip(d).map(|(a, b)| Some((*a)? * (*b)?)).collect(),
            "expected margin",
        )
    } else if let Some(m) = margin {
        (m, "margin")
    } else if let Some(w) = &weight {
        (w.clone(), "weight")
    } else {
        return Err(format!(
            "needs a measure to draw: `value`, `margin` (× `demand`) or `weight` (got: {})",
            t.keys()
        ));
    };
    let is_picked: Vec<bool> = t.rows.iter().map(|r| picked(r.get(flag))).collect();
    let (yes, no) = if flag == "listed" {
        ("listed", "not listed")
    } else {
        ("selected", "not selected")
    };

    // picked first, then the larger measure first; NULL measures last
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&a, &b| {
        is_picked[b]
            .cmp(&is_picked[a])
            .then(
                measure[b]
                    .unwrap_or(f64::NEG_INFINITY)
                    .total_cmp(&measure[a].unwrap_or(f64::NEG_INFINITY)),
            )
            .then(a.cmp(&b))
    });
    let items = t.text("item");
    let gather = |v: &[Value]| -> Vec<Value> { order.iter().map(|&i| v[i].clone()).collect() };
    let mut cols = vec![
        Column::new("item", Role::X, gather(&items)),
        Column::new(
            mname,
            Role::Value(Kind::BarStacked),
            order
                .iter()
                .map(|&i| measure[i].map_or(Value::Na, Value::Float))
                .collect(),
        ),
        Column::new(
            flag,
            Role::Category,
            order
                .iter()
                .map(|&i| Value::Str(if is_picked[i] { yes } else { no }.into()))
                .collect(),
        ),
        Column::new(
            "_colors",
            Role::GaugeColors,
            std::iter::once(Value::Str(format!("{yes}:good; {no}:neutral")))
                .chain(std::iter::repeat_n(Value::Na, n.saturating_sub(1)))
                .collect(),
        ),
    ];

    let model = t.model_key().filter(|m| t.distinct(m) > 1);
    let group = group_key(t, model);
    match (&group, model) {
        (Some(g), Some(m)) => {
            let (gv, mv) = (t.text(g), t.text(m));
            cols.push(Column::new(
                g.as_str(),
                Role::Facet,
                order
                    .iter()
                    .map(|&i| Value::Str(format!("{} · {}", label(&gv[i]), label(&mv[i]))))
                    .collect(),
            ));
        }
        (Some(g), None) => cols.push(Column::new(g.as_str(), Role::Facet, gather(&t.text(g)))),
        (None, Some(m)) => cols.push(Column::new(m, Role::Facet, gather(&t.text(m)))),
        (None, None) => {}
    }

    // one picture: the capacity the picked items use, as the default title
    if group.is_none() && model.is_none() && opt_str(options, "title").is_none() {
        if let (Some(w), true) = (&weight, t.has("capacity")) {
            let cap = t
                .num("capacity")?
                .iter()
                .find_map(|v| v.as_f64().filter(|f| f.is_finite()));
            let used: f64 = (0..n).filter(|&i| is_picked[i]).filter_map(|i| w[i]).sum();
            if let Some(c) = cap.filter(|c| *c > 0.0) {
                let title = format!(
                    "{} weight {} of capacity {} ({:.0} %)",
                    if flag == "listed" {
                        "Listed"
                    } else {
                        "Selected"
                    },
                    super::stats::fmt_num(used),
                    super::stats::fmt_num(c),
                    100.0 * used / c
                );
                cols.push(Column::new(
                    "_title",
                    Role::Title,
                    vec![Value::Str(title); n],
                ));
            }
        }
    }
    Ok((cols, Vec::new()))
}

/// The producer's group column: the first column outside the schema with
/// more than one value (see the module docs and `assignment::group_key`).
fn group_key(t: &Table, model: Option<&str>) -> Option<String> {
    let keys: Vec<String> = t
        .rows
        .first()
        .map(|r| r.keys().cloned().collect())
        .unwrap_or_default();
    keys.into_iter()
        .find(|k| !OWN.contains(&k.as_str()) && Some(k.as_str()) != model && t.distinct(k) > 1)
}

fn label(v: &Value) -> String {
    match v {
        Value::Str(s) => s.clone(),
        _ => "NA".into(),
    }
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

    const KNAPSACK: &str = r#"[{"item":"a","value":10,"weight":5,"selected":true,"capacity":10},
        {"item":"b","value":40,"weight":4,"selected":true,"capacity":10},
        {"item":"c","value":30,"weight":6,"selected":false,"capacity":10},
        {"item":"d","value":50,"weight":3,"selected":false,"capacity":10}]"#;

    #[test]
    fn picked_first_then_by_value_and_capacity_title() {
        let r = rows(KNAPSACK);
        let (cols, _) = columns(&table(&r), &Row::new()).unwrap();
        let x: Vec<String> = cols[0].values.iter().map(label).collect();
        assert_eq!(x, ["b", "a", "d", "c"]);
        assert_eq!(cols[1].name, "value");
        assert_eq!(label(&cols[2].values[0]), "selected");
        assert_eq!(label(&cols[2].values[3]), "not selected");
        let title = cols.iter().find(|c| c.role == Role::Title).unwrap();
        assert_eq!(
            label(&title.values[0]),
            "Selected weight 9 of capacity 10 (90 %)"
        );
        assert!(cols.iter().all(|c| c.role != Role::Facet));
    }

    #[test]
    fn assortment_uses_listed_and_margin_times_demand() {
        let r = rows(
            r#"[{"item":"x","margin":2,"demand":10,"listed":1},
                {"item":"y","margin":5,"demand":1,"listed":0}]"#,
        );
        let (cols, _) = columns(&table(&r), &Row::new()).unwrap();
        assert_eq!(cols[1].name, "expected margin");
        assert_eq!(cols[1].values[0], Value::Float(20.0));
        assert_eq!(label(&cols[2].values[1]), "not listed");
    }

    #[test]
    fn groups_and_models_facet_and_errors_name_the_need() {
        let r = rows(
            r#"[{"site":"n","item":"a","value":1,"selected":true},
                {"site":"s","item":"a","value":2,"selected":false}]"#,
        );
        let (cols, _) = columns(&table(&r), &Row::new()).unwrap();
        assert_eq!(
            cols.iter().find(|c| c.role == Role::Facet).unwrap().name,
            "site"
        );
        let r = rows(r#"[{"item":"a","selected":true}]"#);
        assert!(columns(&table(&r), &Row::new())
            .err()
            .unwrap_or_default()
            .contains("needs a measure"));
        let r = rows(r#"[{"item":"a","value":1}]"#);
        assert!(columns(&table(&r), &Row::new())
            .err()
            .unwrap_or_default()
            .contains("`selected`"));
    }
}
