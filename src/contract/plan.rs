//! Two more optimizer schemas (#499), drawn with the existing roles:
//!
//! - `sequence`: an order (`model_id?, position, item, setup?,
//!   cumulative_setup?`, anofox-optimize's `opt_sequence_by`) — a step line
//!   of the cumulative setup over the position (running sum of `setup` when
//!   `cumulative_setup` is absent, else the position itself), the item as
//!   each step's label.
//! - `weights`: an allocation (`model_id?, item, weight, expected_return?`,
//!   `opt_portfolio_by`) — a bar per item, largest first; several models
//!   compare as one 100 % stacked bar each. With `expected_return` the
//!   default title is the allocation's expected return (Σ weight × return /
//!   Σ weight; one model only; `title :=` wins).
//!
//! Several `model_id`s facet the sequence; a producer group column (any
//! other column with more than one value) facets both.

use super::stats::fmt_num;
use super::{opt_str, Row, Table};
use crate::{Column, Kind, Role};
use ggplot_rs::prelude::Value;

/// The producer's group column: the first column outside `own` (and the
/// model key) with more than one value.
fn group_key(t: &Table, own: &[&str], model: Option<&str>) -> Option<String> {
    let keys: Vec<String> = t
        .rows
        .first()
        .map(|r| r.keys().cloned().collect())
        .unwrap_or_default();
    keys.into_iter().find(|k| {
        !own.contains(&k.as_str())
            && !matches!(k.as_str(), "model_id" | "model_name")
            && Some(k.as_str()) != model
            && t.distinct(k) > 1
    })
}

fn finite(v: &Value) -> Option<f64> {
    v.as_f64().filter(|f| f.is_finite())
}

/// `anofox_plot_sequence(tbl)`: see the module docs.
pub(super) fn sequence(t: &Table) -> Result<(Vec<Column>, Vec<String>), String> {
    t.need(&["position", "item"])?;
    let n = t.rows.len();
    let pos = t.num("position")?;
    let model = t.model_key().filter(|m| t.distinct(m) > 1);
    let group = group_key(t, &["position", "item", "setup", "cumulative_setup"], model);
    // one sequence per (group, model): order by position inside each
    let part = |i: usize| -> String {
        let g = group.as_deref().map(|g| t.text(g)[i].clone());
        let m = model.map(|m| t.text(m)[i].clone());
        format!("{g:?}|{m:?}")
    };
    let mut order: Vec<usize> = (0..n).filter(|&i| finite(&pos[i]).is_some()).collect();
    order.sort_by(|&a, &b| {
        part(a).cmp(&part(b)).then(
            finite(&pos[a])
                .unwrap_or(0.0)
                .total_cmp(&finite(&pos[b]).unwrap_or(0.0)),
        )
    });
    let (y, yname): (Vec<Value>, &str) = if t.has("cumulative_setup") {
        let c = t.num("cumulative_setup")?;
        (
            order.iter().map(|&i| c[i].clone()).collect(),
            "cumulative setup",
        )
    } else if t.has("setup") {
        let s = t.num("setup")?;
        let mut acc = 0.0;
        let mut last = String::new();
        let mut out = Vec::new();
        for &i in &order {
            if part(i) != last {
                (acc, last) = (0.0, part(i));
            }
            acc += finite(&s[i]).unwrap_or(0.0);
            out.push(Value::Float(acc));
        }
        (out, "cumulative setup")
    } else {
        (order.iter().map(|&i| pos[i].clone()).collect(), "position")
    };
    // the x labels name the step: "1 · a" (a discrete axis, in order)
    let items = t.text("item");
    let lab = |v: &Value| match v {
        Value::Str(s) => s.clone(),
        _ => "NA".into(),
    };
    let xs = order
        .iter()
        .map(|&i| {
            Value::Str(format!(
                "{} · {}",
                fmt_num(finite(&pos[i]).unwrap_or(0.0)),
                lab(&items[i])
            ))
        })
        .collect();
    let mut cols = vec![
        Column::new("position", Role::X, xs),
        Column::new(yname, Role::Value(Kind::Step), y),
    ];
    facet(t, &mut cols, &order, group.as_deref(), model);
    Ok((cols, Vec::new()))
}

/// `anofox_plot_weights(tbl)`: see the module docs.
pub(super) fn weights(t: &Table, options: &Row) -> Result<(Vec<Column>, Vec<String>), String> {
    t.need(&["item", "weight"])?;
    let n = t.rows.len();
    let w = t.num("weight")?;
    let model = t.model_key().filter(|m| t.distinct(m) > 1);
    let group = group_key(t, &["item", "weight", "expected_return"], model);
    let items = t.text("item");
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&a, &b| {
        finite(&w[b])
            .unwrap_or(f64::NEG_INFINITY)
            .total_cmp(&finite(&w[a]).unwrap_or(f64::NEG_INFINITY))
            .then(a.cmp(&b))
    });
    let gather = |v: &[Value]| -> Vec<Value> { order.iter().map(|&i| v[i].clone()).collect() };
    let mut cols = if let Some(m) = model {
        // compare models: one 100 % bar each, an item per segment
        vec![
            Column::new(m, Role::X, gather(&t.text(m))),
            Column::new("weight", Role::Value(Kind::BarStackedPercent), gather(&w)),
            Column::new("item", Role::Category, gather(&items)),
        ]
    } else {
        vec![
            Column::new("item", Role::X, gather(&items)),
            Column::new("weight", Role::Value(Kind::Bar), gather(&w)),
        ]
    };
    if let Some(g) = &group {
        cols.push(Column::new(g.as_str(), Role::Facet, gather(&t.text(g))));
    }
    if model.is_none()
        && group.is_none()
        && t.has("expected_return")
        && opt_str(options, "title").is_none()
    {
        let r = t.num("expected_return")?;
        let (mut sw, mut swr) = (0.0, 0.0);
        for i in 0..n {
            if let (Some(wi), Some(ri)) = (finite(&w[i]), finite(&r[i])) {
                sw += wi;
                swr += wi * ri;
            }
        }
        if sw > 0.0 {
            let title = format!("Expected return {}", fmt_num(swr / sw));
            cols.push(Column::new(
                "_title",
                Role::Title,
                vec![Value::Str(title); n],
            ));
        }
    }
    Ok((cols, Vec::new()))
}

/// A sequence's facet: the group and/or model (combined into one title).
fn facet(
    t: &Table,
    cols: &mut Vec<Column>,
    order: &[usize],
    group: Option<&str>,
    model: Option<&str>,
) {
    let label = |v: &Value| match v {
        Value::Str(s) => s.clone(),
        _ => "NA".into(),
    };
    match (group, model) {
        (Some(g), Some(m)) => {
            let (gv, mv) = (t.text(g), t.text(m));
            let v = order
                .iter()
                .map(|&i| Value::Str(format!("{} · {}", label(&gv[i]), label(&mv[i]))))
                .collect();
            cols.push(Column::new(g, Role::Facet, v));
        }
        (Some(k), None) | (None, Some(k)) => {
            let v = t.text(k);
            cols.push(Column::new(
                k,
                Role::Facet,
                order.iter().map(|&i| v[i].clone()).collect(),
            ));
        }
        (None, None) => {}
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
    fn f(v: &Value) -> f64 {
        v.as_f64().unwrap()
    }

    #[test]
    fn sequence_accumulates_setup_in_position_order() {
        let r = rows(
            r#"[{"position":2,"item":"b","setup":3},{"position":1,"item":"a","setup":0},
                {"position":3,"item":"c","setup":5}]"#,
        );
        let (cols, _) = sequence(&table(&r)).unwrap();
        assert_eq!(cols[1].name, "cumulative setup");
        assert_eq!(
            cols[1].values.iter().map(f).collect::<Vec<_>>(),
            [0.0, 3.0, 8.0]
        );
        assert!(matches!(cols[1].role, Role::Value(Kind::Step)));
        assert_eq!(cols[0].values[0], Value::Str("1 · a".into()));
    }

    #[test]
    fn weights_sort_and_title_and_compare_models() {
        let r = rows(
            r#"[{"item":"x","weight":0.25,"expected_return":0.08},
                {"item":"y","weight":0.75,"expected_return":0.04}]"#,
        );
        let (cols, _) = weights(&table(&r), &Row::new()).unwrap();
        assert_eq!(cols[0].values[0], Value::Str("y".into()));
        let title = cols.iter().find(|c| c.role == Role::Title).unwrap();
        assert_eq!(title.values[0], Value::Str("Expected return 0.05".into()));
        let r = rows(
            r#"[{"model_id":"mv","item":"x","weight":0.5},{"model_id":"mv","item":"y","weight":0.5},
                {"model_id":"eq","item":"x","weight":0.2},{"model_id":"eq","item":"y","weight":0.8}]"#,
        );
        let (cols, _) = weights(&table(&r), &Row::new()).unwrap();
        assert!(matches!(cols[1].role, Role::Value(Kind::BarStackedPercent)));
        assert_eq!(cols[0].name, "model_id");
    }
}
