//! `::DIAGRAM` edit mode (#682): the graph a viewer draws by hand — added,
//! renamed and removed nodes, connected and removed edges — as **rows**, not
//! pixels.
//!
//! ```sql
//! SELECT type ::DIAGRAM_PALETTE, label, connects_to FROM node_types;   -- optional
//! SELECT id ::DIAGRAM, name ::LABEL, kind ::CATEGORY, upstream ::UPSTREAM,
//!        'edit' AS graph ::DIAGRAM_MODE
//! FROM network_node;
//! -- elsewhere: the edited graph, one JSON object
//! SELECT n->>'id' AS id, n->>'type' AS type, n->>'label' AS label
//! FROM (SELECT unnest((getvariable('graph')->'nodes')::JSON[]) AS n);
//! ```
//!
//! The variable (the `::DIAGRAM_MODE` column's alias) holds the whole edited
//! graph as a JSON object text — `{"nodes": [{"id", "type", "label"}],
//! "edges": [{"from", "to"}]}` — like a `::FORM` holds its fields: a locked
//! server receives it as an ordinary string variable. The diagram itself is
//! redrawn from its own rows with the edits applied ([`apply`]): an existing
//! node keeps its row (value, badge, lane, sparkline, …), an existing edge
//! its `::EDGES` attributes (value, label, style, status); a new node is a
//! box with its label and type colour; edges come out as one `::EDGES` list.

use crate::diagram::{Diagram, EdgeStyle};
use crate::format::Locale;
use crate::{Column, Role};
use ggplot_rs::prelude::Value;
use serde_json::{json, Map, Value as J};

/// The graph as edited: nodes `(id, type, label)` and edges `(from, to)`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Graph {
    pub nodes: Vec<GNode>,
    pub edges: Vec<(String, String)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GNode {
    pub id: String,
    pub kind: Option<String>,
    pub label: String,
}

/// Most nodes / edges an edited graph may have (the diagram draws at most
/// [`crate::diagram::MAX_NODES`] / [`crate::diagram::MAX_EDGES`]).
pub const MAX_NODES: usize = crate::diagram::MAX_NODES;
pub const MAX_EDGES: usize = crate::diagram::MAX_EDGES;

fn style_word(s: EdgeStyle) -> &'static str {
    match s {
        EdgeStyle::Solid => "solid",
        EdgeStyle::Dashed => "dashed",
        EdgeStyle::Flow(false) => "flow",
        EdgeStyle::Flow(true) => "flow-volume",
        EdgeStyle::Dots(false) => "flow-dots",
        EdgeStyle::Dots(true) => "flow-dots-volume",
    }
}

/// The node types (`::CATEGORY`) of a diagram's rows, by node id.
fn kinds(cols: &[Column]) -> Vec<(String, String)> {
    let (Some(idc), Some(cat)) = (
        cols.iter().find(|c| c.role == Role::Diagram),
        cols.iter().find(|c| c.role == Role::Category),
    ) else {
        return Vec::new();
    };
    idc.values
        .iter()
        .zip(&cat.values)
        .filter_map(|(i, k)| Some((crate::key_text(i)?, crate::key_text(k)?)))
        .collect()
}

impl Graph {
    /// The graph a diagram's rows draw.
    pub fn of(cols: &[Column]) -> Graph {
        let Some(d) = Diagram::from_columns(cols, &Locale::default()) else {
            return Graph::default();
        };
        let kinds = kinds(cols);
        Graph {
            nodes: d
                .nodes
                .iter()
                .map(|n| GNode {
                    id: n.id.clone(),
                    kind: kinds
                        .iter()
                        .find(|(i, _)| *i == n.id)
                        .map(|(_, k)| k.clone()),
                    label: n.label.clone(),
                })
                .collect(),
            edges: d
                .edges
                .iter()
                .map(|e| (d.nodes[e.from].id.clone(), d.nodes[e.to].id.clone()))
                .collect(),
        }
    }

    pub fn to_json(&self) -> String {
        json!({
            "nodes": self.nodes.iter().map(|n| json!({"id": n.id, "type": n.kind, "label": n.label})).collect::<Vec<_>>(),
            "edges": self.edges.iter().map(|(f, t)| json!({"from": f, "to": t})).collect::<Vec<_>>(),
        })
        .to_string()
    }

    /// Parse an edited graph. Refuses anything that isn't the shape above,
    /// a node without an id, a repeated id, an edge naming no node, or more
    /// than [`MAX_NODES`] / [`MAX_EDGES`] (the browser never sends one; a
    /// hand-made `?v.graph=` might).
    pub fn parse(text: &str) -> Result<Graph, String> {
        let v: J = serde_json::from_str(text).map_err(|e| format!("not JSON: {e}"))?;
        let o = v.as_object().ok_or("not a JSON object")?;
        let arr = |k: &str| -> Result<&Vec<J>, String> {
            o.get(k)
                .and_then(J::as_array)
                .ok_or_else(|| format!("`{k}` is not a list"))
        };
        let (ns, es) = (arr("nodes")?, arr("edges")?);
        if ns.len() > MAX_NODES || es.len() > MAX_EDGES {
            return Err(format!("more than {MAX_NODES} nodes or {MAX_EDGES} edges"));
        }
        let s = |m: &Map<String, J>, k: &str| -> Option<String> {
            m.get(k)
                .and_then(J::as_str)
                .map(|x| x.trim().to_string())
                .filter(|x| !x.is_empty() && x.len() <= 200)
        };
        let mut g = Graph::default();
        for n in ns {
            let m = n.as_object().ok_or("a node is not an object")?;
            let id = s(m, "id").ok_or("a node without an id")?;
            if g.nodes.iter().any(|x| x.id == id) {
                return Err(format!("node `{id}` twice"));
            }
            g.nodes.push(GNode {
                label: s(m, "label").unwrap_or_else(|| id.clone()),
                kind: s(m, "type"),
                id,
            });
        }
        for e in es {
            let m = e.as_object().ok_or("an edge is not an object")?;
            let (Some(f), Some(t)) = (s(m, "from"), s(m, "to")) else {
                return Err("an edge without `from` / `to`".into());
            };
            if !g.nodes.iter().any(|n| n.id == f) || !g.nodes.iter().any(|n| n.id == t) {
                return Err(format!("edge {f} → {t} names no node"));
            }
            if !g.edges.contains(&(f.clone(), t.clone())) {
                g.edges.push((f, t));
            }
        }
        Ok(g)
    }
}

/// The diagram's columns with `graph` applied: one row per edited node
/// (an existing node keeps its own row's cells; a new one gets its id,
/// label and type), its label / type from the graph, and every edge in one
/// `::EDGES` list (an existing edge keeps its value, label, style, status;
/// `::UPSTREAM` is dropped — the list replaces it).
pub fn apply(cols: &[Column], graph: &Graph) -> Vec<Column> {
    let Some(idc) = cols.iter().find(|c| c.role == Role::Diagram) else {
        return cols.to_vec();
    };
    let base = Diagram::from_columns(cols, &Locale::default());
    let row_of = |id: &str| {
        idc.values
            .iter()
            .position(|v| crate::key_text(v).as_deref() == Some(id))
    };
    let rows: Vec<Option<usize>> = graph.nodes.iter().map(|n| row_of(&n.id)).collect();
    let mut out: Vec<Column> = Vec::new();
    let mut has_label = false;
    let mut has_cat = false;
    for c in cols {
        if matches!(c.role, Role::Upstream | Role::Edges) {
            continue;
        }
        let values = graph
            .nodes
            .iter()
            .zip(&rows)
            .map(|(n, row)| match c.role {
                Role::Diagram => Value::Str(n.id.clone()),
                Role::Label => Value::Str(n.label.clone()),
                Role::Category => n.kind.clone().map_or(Value::Na, Value::Str),
                // panel-wide settings (::DIRECTION, ::THRESHOLDS, …) read the
                // first non-NULL cell: keep the first row's on every row
                _ if row.is_none() && !per_node(c.role) => {
                    c.values.first().cloned().unwrap_or(Value::Na)
                }
                _ => row
                    .and_then(|r| c.values.get(r).cloned())
                    .unwrap_or(Value::Na),
            })
            .collect();
        has_label |= c.role == Role::Label;
        has_cat |= c.role == Role::Category;
        out.push(Column::new(c.name.clone(), c.role, values));
    }
    let col = |name: &str, role, vals: Vec<Value>| Column::new(name, role, vals);
    if !has_label {
        out.push(col(
            "_label",
            Role::Label,
            graph
                .nodes
                .iter()
                .map(|n| Value::Str(n.label.clone()))
                .collect(),
        ));
    }
    if !has_cat && graph.nodes.iter().any(|n| n.kind.is_some()) {
        out.push(col(
            "_type",
            Role::Category,
            graph
                .nodes
                .iter()
                .map(|n| n.kind.clone().map_or(Value::Na, Value::Str))
                .collect(),
        ));
    }
    let attrs = |f: &str, t: &str| -> Map<String, J> {
        let mut m = Map::new();
        m.insert("from".into(), J::String(f.into()));
        m.insert("to".into(), J::String(t.into()));
        if let Some(d) = &base {
            if let Some(e) = d
                .edges
                .iter()
                .find(|e| d.nodes[e.from].id == f && d.nodes[e.to].id == t)
            {
                if let Some(v) = e.value {
                    m.insert("value".into(), json!(v));
                }
                if let Some(l) = &e.label {
                    m.insert("label".into(), J::String(l.clone()));
                }
                m.insert("style".into(), J::String(style_word(e.style).into()));
                if let Some(s) = e
                    .status
                    .filter(|s| *s != crate::thresholds::Status::Neutral)
                {
                    m.insert("status".into(), J::String(s.as_str().into()));
                }
            }
        }
        m
    };
    let edges = J::Array(
        graph
            .edges
            .iter()
            .map(|(f, t)| J::Object(attrs(f, t)))
            .collect(),
    )
    .to_string();
    let n = graph.nodes.len().max(1);
    let mut ev = vec![Value::Na; n];
    ev[0] = Value::Str(edges);
    out.push(col("_edges", Role::Edges, ev));
    out
}

/// The `::DIAGRAM_MODE` value of a planned diagram's columns, lower-cased
/// (`'edit'`, `'view'`, …), if it has one.
pub fn mode(cols: &[Column]) -> Option<String> {
    let c = cols.iter().find(|c| c.role == Role::DiagramMode)?;
    c.values
        .iter()
        .find_map(crate::key_text)
        .map(|s| s.trim().to_ascii_lowercase())
}

/// The variable an editable diagram's graph lives in: the `::DIAGRAM_MODE`
/// column's alias, else `graph`.
pub fn var_name(roles: &[(usize, Role)], names: &[(usize, String)]) -> String {
    roles
        .iter()
        .find(|(_, r)| *r == Role::DiagramMode)
        .and_then(|(i, _)| names.iter().find(|(j, _)| j == i))
        .map(|(_, n)| n.clone())
        .unwrap_or_else(|| "graph".into())
}

/// The `SET VARIABLE` that gives an editable diagram's variable its rows'
/// own graph — what `dashboard --check` and the static renderers bind, like
/// an input's default (the browser binds it before any panel runs). `None`
/// for a diagram that isn't `'edit'`.
pub fn default_set(
    roles: &[(usize, Role)],
    names: &[(usize, String)],
    cols: &[Column],
) -> Option<String> {
    if mode(cols).as_deref() != Some("edit") || !cols.iter().any(|c| c.role == Role::Diagram) {
        return None;
    }
    Some(format!(
        "SET VARIABLE {} = {}",
        crate::host::serving::quote_ident(&var_name(roles, names)),
        crate::host::serving::sql_string_literal(&Graph::of(cols).to_json())
    ))
}

/// Roles whose cell belongs to one node (left empty on a new node); every
/// other role on a diagram is a panel-wide setting read from any row.
fn per_node(r: Role) -> bool {
    matches!(
        r,
        Role::Metric(_)
            | Role::Badge
            | Role::Story
            | Role::Lane
            | Role::X
            | Role::Y
            | Role::Value(_)
            | Role::LineStyle
            | Role::Link
            | Role::Info
            | Role::Title
    )
}

/// A palette entry (`::DIAGRAM_PALETTE`): a node type, its label, and the
/// types it may connect to (empty: any).
#[derive(Clone, Debug, PartialEq)]
pub struct PaletteEntry {
    pub kind: String,
    pub label: String,
    pub connects_to: Vec<String>,
}

/// The palette from its statement's rows (`kind_col` = the
/// `::DIAGRAM_PALETTE` column's name; `label` and `connects_to` read by
/// name, any case — `connects_to` a list or comma-separated text).
pub fn palette(kind_col: &str, rows: &[Map<String, J>]) -> Vec<PaletteEntry> {
    let col = |r: &Map<String, J>, n: &str| {
        r.iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(n))
            .map(|(_, v)| v.clone())
    };
    let text = |v: Option<J>| match v {
        Some(J::String(s)) => Some(s.trim().to_string()).filter(|s| !s.is_empty()),
        Some(J::Null) | None => None,
        Some(v) => Some(v.to_string()),
    };
    let mut out: Vec<PaletteEntry> = Vec::new();
    for r in rows {
        let Some(kind) = text(r.get(kind_col).cloned()) else {
            continue;
        };
        if out.iter().any(|p| p.kind == kind) {
            continue;
        }
        let connects_to = match col(r, "connects_to") {
            Some(J::Array(a)) => a.into_iter().filter_map(|x| text(Some(x))).collect(),
            Some(J::String(s)) => s
                .trim_matches(|c| c == '[' || c == ']')
                .split(',')
                .map(|x| x.trim().trim_matches('\'').trim_matches('"').to_string())
                .filter(|x| !x.is_empty())
                .collect(),
            _ => Vec::new(),
        };
        out.push(PaletteEntry {
            label: text(col(r, "label")).unwrap_or_else(|| kind.clone()),
            kind,
            connects_to,
        });
    }
    out
}

pub fn palette_json(p: &[PaletteEntry]) -> J {
    J::Array(
        p.iter()
            .map(|e| json!({"type": e.kind, "label": e.label, "connects_to": e.connects_to}))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(x: &str) -> Value {
        Value::Str(x.into())
    }

    fn base() -> Vec<Column> {
        vec![
            Column::new("id", Role::Diagram, vec![s("sup"), s("wh"), s("store")]),
            Column::new(
                "name",
                Role::Label,
                vec![s("Supplier"), s("Warehouse"), s("Store")],
            ),
            Column::new(
                "kind",
                Role::Category,
                vec![s("supplier"), s("stock"), s("demand")],
            ),
            Column::new(
                "stock",
                Role::Metric(crate::MetricFmt::Plain),
                vec![Value::Float(1.0), Value::Float(50.0), Value::Float(7.0)],
            ),
            Column::new(
                "e",
                Role::Edges,
                vec![
                    s(
                        r#"[{"from":"sup","to":"wh","value":5,"style":"flow"},{"from":"wh","to":"store"}]"#,
                    ),
                    Value::Na,
                    Value::Na,
                ],
            ),
            Column::new("d", Role::Direction, vec![s("LR"), Value::Na, Value::Na]),
        ]
    }

    #[test]
    fn graph_round_trip() {
        let g = Graph::of(&base());
        assert_eq!(g.nodes.len(), 3);
        assert_eq!(g.nodes[0].kind.as_deref(), Some("supplier"));
        assert_eq!(
            g.edges,
            vec![("sup".into(), "wh".into()), ("wh".into(), "store".into())]
        );
        assert_eq!(Graph::parse(&g.to_json()).unwrap(), g);
    }

    #[test]
    fn parse_refuses_bad_graphs() {
        assert!(Graph::parse("[]").is_err());
        assert!(Graph::parse(r#"{"nodes":[{"id":"a"},{"id":"a"}],"edges":[]}"#).is_err());
        assert!(Graph::parse(r#"{"nodes":[{"id":"a"}],"edges":[{"from":"a","to":"b"}]}"#).is_err());
        assert!(Graph::parse(r#"{"nodes":[{"label":"x"}],"edges":[]}"#).is_err());
    }

    #[test]
    fn apply_keeps_rows_and_edge_attributes() {
        let mut g = Graph::of(&base());
        // rename the warehouse, add a second supplier feeding it, drop the store
        g.nodes[1].label = "DC North".into();
        g.nodes.retain(|n| n.id != "store");
        g.edges.retain(|(_, t)| t != "store");
        g.nodes.push(GNode {
            id: "sup2".into(),
            kind: Some("supplier".into()),
            label: "Supplier 2".into(),
        });
        g.edges.push(("sup2".into(), "wh".into()));
        let cols = apply(&base(), &g);
        let d = Diagram::from_columns(&cols, &Locale::default()).unwrap();
        assert_eq!(
            d.nodes.iter().map(|n| n.id.as_str()).collect::<Vec<_>>(),
            ["sup", "wh", "sup2"]
        );
        assert_eq!(d.nodes[1].label, "DC North");
        assert_eq!(d.nodes[1].value, Some(50.0)); // its row's metric kept
        assert_eq!(d.nodes[2].value, None); // a new node has none
        assert_eq!(d.edges.len(), 2);
        let kept = d
            .edges
            .iter()
            .find(|e| d.nodes[e.from].id == "sup")
            .unwrap();
        assert_eq!(kept.value, Some(5.0));
        assert_eq!(kept.style, EdgeStyle::Flow(false));
        assert!(d.problems.is_empty(), "{:?}", d.problems);
    }

    #[test]
    fn palette_rows() {
        let rows: Vec<Map<String, J>> = serde_json::from_str(
            r#"[{"type":"supplier","Label":"Supplier","connects_to":["stock"]},{"type":"stock","connects_to":"stock, demand"},{"type":"supplier"}]"#,
        )
        .unwrap();
        let p = palette("type", &rows);
        assert_eq!(p.len(), 2);
        assert_eq!(p[0].label, "Supplier");
        assert_eq!(p[1].connects_to, ["stock", "demand"]);
        assert_eq!(p[1].label, "stock");
    }
}
