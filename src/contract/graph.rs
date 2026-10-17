//! The `graph` contract schema — a pipeline, lineage or process as two
//! tables (`docs/plans/integration-contract.md`), named after Grafana's node
//! graph: nodes `id, title, mainstat, status, group, lane, story, x, y,
//! sparkline` (+ `seq` for the order), edges `source, target, value, label,
//! style, status`. `anofox_plot_diagram(n, edges := (SELECT list(e) FROM e))`
//! maps them onto the `::DIAGRAM` role columns, so the diagram is the one the
//! dashboard draws.
//!
//! Producers' own spellings are read too (inventory-rs's run manifest:
//! `stage`, `records_in`; its edges: `from_stage`, `to_stage`, `count`), and
//! a named argument (`id := 'step'`, `mainstat := 'records_accepted'`) picks
//! any other column.

use super::Row;
use crate::{Column, Kind, MetricFmt, Role};
use ggplot_rs::prelude::Value;
use serde_json::{Map, Value as J};

/// Node columns: (contract name, other spellings).
pub(super) const NODE: &[(&str, &[&str])] = &[
    ("id", &["stage", "node", "node_id", "step", "name"]),
    (
        "title",
        &["label", "stage_name", "node_title", "display_name"],
    ),
    ("mainstat", &["main_stat", "value", "records", "count", "n"]),
    ("status", &["health", "state", "badge"]),
    ("group", &["category", "kind", "type"]),
    ("lane", &["swimlane", "team", "owner"]),
    ("story", &["caption"]),
    ("x", &[]),
    ("y", &[]),
    ("sparkline", &["trend", "history"]),
    ("seq", &["order", "position", "sort"]),
    ("upstream", &["parents", "inputs"]),
];

/// Edge columns: (contract name, other spellings).
pub(super) const EDGE: &[(&str, &[&str])] = &[
    (
        "source",
        &["from", "from_stage", "src", "from_node", "parent"],
    ),
    ("target", &["to", "to_stage", "dst", "to_node", "child"]),
    ("value", &["mainstat", "count", "n", "records", "weight"]),
    ("label", &[]),
    ("style", &[]),
    ("status", &["health", "state"]),
];

/// The key of `rows` read as contract column `name`: the column a named
/// argument gives (`id := 'step'`), else the contract name, else its other
/// spellings (case and separators ignored), in table order.
fn key_for(
    keys: &[&String],
    table: &[(&str, &[&str])],
    name: &str,
    options: &Row,
) -> Option<String> {
    let norm = super::names::norm;
    if let Some(k) = options.get(name).and_then(J::as_str) {
        return keys
            .iter()
            .find(|c| norm(c) == norm(k))
            .map(|c| c.to_string());
    }
    let spellings = table
        .iter()
        .find(|(c, _)| *c == name)
        .map_or(&[][..], |(_, a)| a);
    std::iter::once(name)
        .chain(spellings.iter().copied())
        .find_map(|s| keys.iter().find(|c| norm(c) == s))
        .map(|c| c.to_string())
}

fn keys(rows: &[Row]) -> Vec<&String> {
    let mut out: Vec<&String> = Vec::new();
    for r in rows {
        for k in r.keys() {
            if !out.contains(&k) {
                out.push(k);
            }
        }
    }
    out
}

fn cell(v: Option<&J>) -> Value {
    match v {
        None | Some(J::Null) => Value::Na,
        Some(J::String(s)) => Value::Str(s.clone()),
        Some(J::Number(n)) => n.as_f64().map_or(Value::Na, Value::Float),
        Some(J::Bool(b)) => Value::Bool(*b),
        Some(v) => Value::Str(v.to_string()),
    }
}

/// The role columns of a `graph` node table + its edges (`options.edges`).
pub(super) fn columns(rows: &[Row], options: &Row) -> Result<Vec<Column>, String> {
    let ks = keys(rows);
    let key = |name: &str| key_for(&ks, NODE, name, options);
    key("id").ok_or_else(|| {
        format!(
            "{} an id column (id, stage, node, step or name; or id := '<column>'), got {}",
            super::NEEDS,
            ks.iter().map(|k| k.as_str()).collect::<Vec<_>>().join(", ")
        )
    })?;
    // nodes in their own order (`seq`), else as given
    let mut order: Vec<usize> = (0..rows.len()).collect();
    if let Some(seq) = key("seq") {
        let at = |i: usize| {
            rows[i]
                .get(&seq)
                .and_then(J::as_f64)
                .unwrap_or(f64::INFINITY)
        };
        order.sort_by(|&a, &b| at(a).total_cmp(&at(b)).then(a.cmp(&b)));
    }
    let col = |name: &str, role: Role| {
        key(name).map(|k| {
            Column::new(
                name,
                role,
                order.iter().map(|&i| cell(rows[i].get(&k))).collect(),
            )
        })
    };
    let fmt = match super::opt_str(options, "format")
        .as_deref()
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("money" | "currency") => MetricFmt::Money,
        Some("percent" | "pct") => MetricFmt::Percent,
        Some("metric" | "plain" | "number") => MetricFmt::Plain,
        _ => MetricFmt::Compact,
    };
    let mut cols: Vec<Column> = [
        col("id", Role::Diagram),
        col("title", Role::Label),
        key("mainstat").and_then(|_| col("mainstat", Role::Metric(fmt))),
        col("status", Role::Badge),
        col("group", Role::Category),
        col("lane", Role::Lane),
        col("story", Role::Story),
        col("x", Role::X),
        col("y", Role::Y),
        col("sparkline", Role::Value(Kind::Sparkline)),
        col("upstream", Role::Upstream),
    ]
    .into_iter()
    .flatten()
    .collect();
    let n = order.len();
    let constant = |name: &str, role: Role, v: Option<String>| {
        v.map(|v| Column::new(name, role, vec![Value::Str(v); n]))
    };
    cols.extend(constant(
        "direction",
        Role::Direction,
        super::opt_str(options, "direction"),
    ));
    cols.extend(constant(
        "style",
        Role::LineStyle,
        super::opt_str(options, "style"),
    ));
    if let Some(edges) = edges(options)? {
        let mut v = vec![Value::Na; n];
        if let Some(first) = v.first_mut() {
            *first = Value::Str(edges);
        }
        cols.push(Column::new("edges", Role::Edges, v));
    }
    Ok(cols)
}

/// `options.edges` (a list of edge structs, or its JSON text) as the
/// `::EDGES` JSON (`from, to, value, label, style, status`).
fn edges(options: &Row) -> Result<Option<String>, String> {
    let list = match options.get("edges") {
        None | Some(J::Null) => return Ok(None),
        Some(J::String(s)) => serde_json::from_str::<J>(s)
            .map_err(|e| format!("edges := is not a list of structs: {e}"))?,
        Some(v) => v.clone(),
    };
    let J::Array(items) = list else {
        return Err(
            "edges := is not a list of structs — pass (SELECT list(e) FROM edges e)".into(),
        );
    };
    let rows: Vec<Row> = items
        .into_iter()
        .filter_map(|i| i.as_object().cloned())
        .collect();
    let ks = keys(&rows);
    let key = |name: &str| key_for(&ks, EDGE, name, options);
    let (Some(src), Some(dst)) = (key("source"), key("target")) else {
        return Err(format!(
            "{} edges with source and target columns (or from / to, from_stage / to_stage; or source := / target :=), got {}",
            super::NEEDS,
            ks.iter().map(|k| k.as_str()).collect::<Vec<_>>().join(", ")
        ));
    };
    let pick: Vec<(&str, Option<String>)> = vec![
        ("from", Some(src)),
        ("to", Some(dst)),
        ("value", key("value")),
        ("label", key("label")),
        ("style", key("style")),
        ("status", key("status")),
    ];
    let out: Vec<J> = rows
        .iter()
        .map(|r| {
            let mut o = Map::new();
            for (name, k) in &pick {
                if let Some(v) = k.as_ref().and_then(|k| r.get(k)) {
                    o.insert((*name).to_string(), v.clone());
                }
            }
            J::Object(o)
        })
        .collect();
    Ok(Some(J::Array(out).to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(json: &str) -> Vec<Row> {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn grafana_names_and_inventory_rs_spellings() {
        // inventory-rs#28: run_manifest + run_edges, as they are written
        let nodes = rows(
            r#"[{"stage":"stream","seq":4,"records_in":1440},
                {"stage":"ingest","seq":1,"records_in":9133},
                {"stage":"build_twin","seq":2,"records_in":12}]"#,
        );
        let mut opts = Row::new();
        opts.insert("mainstat".into(), J::String("records_in".into()));
        opts.insert(
            "edges".into(),
            serde_json::json!([{"from_stage":"ingest","to_stage":"build_twin","count":9128},
                               {"from_stage":"build_twin","to_stage":"stream","count":12}]),
        );
        let cols = columns(&nodes, &opts).unwrap();
        let ids: Vec<String> = cols[0].values.iter().map(crate::value_str).collect();
        assert_eq!(ids, ["ingest", "build_twin", "stream"], "ordered by seq");
        assert!(cols
            .iter()
            .any(|c| c.role == Role::Metric(MetricFmt::Compact)));
        let e = cols.iter().find(|c| c.role == Role::Edges).unwrap();
        assert_eq!(
            crate::value_str(&e.values[0]),
            r#"[{"from":"ingest","to":"build_twin","value":9128},{"from":"build_twin","to":"stream","value":12}]"#
        );
    }

    #[test]
    fn grafana_columns_and_errors() {
        let nodes = rows(
            r#"[{"id":"a","title":"A","mainstat":3,"lane":"L"},{"id":"b","title":"B","mainstat":5}]"#,
        );
        let mut opts = Row::new();
        opts.insert(
            "edges".into(),
            serde_json::json!([{"source":"a","target":"b","value":2}]),
        );
        opts.insert("direction".into(), J::String("TB".into()));
        let roles: Vec<Role> = columns(&nodes, &opts)
            .unwrap()
            .iter()
            .map(|c| c.role)
            .collect();
        assert_eq!(
            roles,
            [
                Role::Diagram,
                Role::Label,
                Role::Metric(MetricFmt::Compact),
                Role::Lane,
                Role::Direction,
                Role::Edges
            ]
        );
        let err = columns(&rows(r#"[{"what":1}]"#), &Row::new())
            .err()
            .unwrap();
        assert!(err.contains("an id column"), "{err}");
        opts.insert("edges".into(), serde_json::json!([{"a":1}]));
        assert!(columns(&nodes, &opts)
            .err()
            .unwrap()
            .contains("source and target"));
    }
}
