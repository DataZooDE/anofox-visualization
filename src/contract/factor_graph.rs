//! The `factor_graph` contract (#1151) — a factor graph as tables, as an
//! inference library writes it: nodes `id, kind, name, label, slice, layer,
//! plate, deterministic` (kind: `factor`, `variable`, `data`, `constant`,
//! `equality`, `delta`), edges `variable, factor, interface, seq` and,
//! optionally, marginals `variable, iteration, mean, sd, family`.
//! `anofox_plot_factor_graph(nodes, edges := (SELECT list(e) FROM e),
//! marginals := …)` maps them onto the `::DIAGRAM` roles, Forney style:
//!
//! - `kind` → `::SHAPE`, `layer` → `::LANE` (bands in the order control,
//!   param, state, obs, pref, then others as they come);
//! - an edge runs factor → variable on the factor's `out` interface and
//!   variable → factor otherwise, the interface as its label;
//! - a variable whose only edge goes to its equality node is folded into
//!   that node (one mark per variable, labelled with the variable);
//! - TeX-ish labels (`\theta`, `y_{0}`) read as text (`θ`, `y₀`);
//! - the latest marginal is the variable's value (its mean) and badge (`± sd`).

use super::Row;
use crate::{Column, MetricFmt, Role};
use ggplot_rs::prelude::Value;
use serde_json::{Map, Value as J};

/// The node kinds the contract knows.
pub(crate) const KINDS: &[&str] = &[
    "factor", "variable", "data", "constant", "equality", "delta",
];

/// The layers drawn first, in this order (others follow as they come).
const LAYERS: &[&str] = &["control", "param", "state", "obs", "pref"];

fn text(v: Option<&J>) -> Option<String> {
    match v? {
        J::Null => None,
        J::String(s) => Some(s.clone()),
        J::Number(n) => Some(n.to_string()),
        J::Bool(b) => Some(b.to_string()),
        other => Some(other.to_string()),
    }
}

/// The value of one of `names` in `row` (case ignored).
fn get(row: &Row, names: &[&str]) -> Option<String> {
    names.iter().find_map(|n| {
        row.iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(n))
            .and_then(|(_, v)| text(Some(v)))
    })
}

/// TeX-ish producer labels as plain text: Greek letters by name, `_{…}` /
/// `_x` as subscript digits and letters where Unicode has them (else
/// `_x`), braces dropped.
pub(crate) fn tex_lite(s: &str) -> String {
    const GREEK: &[(&str, &str)] = &[
        ("alpha", "α"),
        ("beta", "β"),
        ("gamma", "γ"),
        ("delta", "δ"),
        ("epsilon", "ε"),
        ("zeta", "ζ"),
        ("eta", "η"),
        ("theta", "θ"),
        ("iota", "ι"),
        ("kappa", "κ"),
        ("lambda", "λ"),
        ("mu", "μ"),
        ("nu", "ν"),
        ("xi", "ξ"),
        ("pi", "π"),
        ("rho", "ρ"),
        ("sigma", "σ"),
        ("tau", "τ"),
        ("phi", "φ"),
        ("chi", "χ"),
        ("psi", "ψ"),
        ("omega", "ω"),
        ("Gamma", "Γ"),
        ("Delta", "Δ"),
        ("Theta", "Θ"),
        ("Lambda", "Λ"),
        ("Pi", "Π"),
        ("Sigma", "Σ"),
        ("Phi", "Φ"),
        ("Psi", "Ψ"),
        ("Omega", "Ω"),
    ];
    let sub = |c: char| -> Option<char> {
        Some(match c {
            '0'..='9' => char::from_u32(0x2080 + c as u32 - '0' as u32)?,
            '+' => '₊',
            '-' => '₋',
            'a' => 'ₐ',
            'e' => 'ₑ',
            'i' => 'ᵢ',
            'j' => 'ⱼ',
            'k' => 'ₖ',
            'n' => 'ₙ',
            't' => 'ₜ',
            _ => return None,
        })
    };
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '\\' {
            let name: String = chars[i + 1..]
                .iter()
                .take_while(|c| c.is_ascii_alphabetic())
                .collect();
            if let Some((_, g)) = GREEK.iter().find(|(n, _)| *n == name) {
                out.push_str(g);
                i += 1 + name.len();
                continue;
            }
            i += 1;
            continue;
        }
        // `_x` unbraced is a subscript after a one-letter name only (`x_t`;
        // `big_a` stays a name)
        let one_letter = out
            .chars()
            .rev()
            .take_while(|c| c.is_alphanumeric())
            .count()
            == 1;
        if c == '_' && (chars.get(i + 1) == Some(&'{') || one_letter) {
            let (body, next): (String, usize) = if chars.get(i + 1) == Some(&'{') {
                let end = chars[i + 2..]
                    .iter()
                    .position(|&c| c == '}')
                    .map_or(chars.len(), |p| i + 2 + p);
                (
                    chars[i + 2..end].iter().collect(),
                    (end + 1).min(chars.len()),
                )
            } else {
                (
                    chars.get(i + 1).map(|c| c.to_string()).unwrap_or_default(),
                    i + 2,
                )
            };
            let body = tex_lite(&body);
            match body.chars().map(sub).collect::<Option<String>>() {
                Some(s) => out.push_str(&s),
                None => {
                    out.push('_');
                    out.push_str(&body);
                }
            }
            i = next;
            continue;
        }
        if c != '{' && c != '}' {
            out.push(c);
        }
        i += 1;
    }
    out
}

/// A list-of-structs option (`edges :=`, `marginals :=`) as rows.
fn list(options: &Row, name: &str) -> Result<Vec<Row>, String> {
    let v = match options.get(name) {
        None | Some(J::Null) => return Ok(Vec::new()),
        Some(J::String(s)) => serde_json::from_str::<J>(s)
            .map_err(|e| format!("{name} := is not a list of structs: {e}"))?,
        Some(v) => v.clone(),
    };
    let J::Array(items) = v else {
        return Err(format!(
            "{name} := is not a list of structs — pass (SELECT list(e) FROM {name} e)"
        ));
    };
    Ok(items
        .into_iter()
        .filter_map(|i| i.as_object().cloned())
        .collect())
}

/// What the lint reports about a factor graph (`factor_graph/*`).
fn problems(nodes: &[Row], edges: &[Row]) -> Vec<String> {
    let mut out = Vec::new();
    let ids: Vec<String> = nodes.iter().filter_map(|r| get(r, &["id"])).collect();
    let kind_of = |id: &str| {
        nodes
            .iter()
            .find(|r| get(r, &["id"]).as_deref() == Some(id))
            .and_then(|r| get(r, &["kind"]))
    };
    let bad: Vec<String> = nodes
        .iter()
        .filter_map(|r| get(r, &["kind"]))
        .filter(|k| !KINDS.contains(&k.to_ascii_lowercase().as_str()))
        .collect();
    if !bad.is_empty() {
        out.push(format!(
            "factor_graph/bad-kind: kind {} is not one of {}",
            bad.join(", "),
            KINDS.join(", ")
        ));
    }
    let mut unknown: Vec<String> = Vec::new();
    for e in edges {
        for side in [get(e, &["variable"]), get(e, &["factor"])]
            .into_iter()
            .flatten()
        {
            if !ids.contains(&side) && !unknown.contains(&side) {
                unknown.push(side);
            }
        }
    }
    if !unknown.is_empty() {
        out.push(format!(
            "factor_graph/unknown-id: edges name {} that are not nodes",
            unknown.join(", ")
        ));
    }
    // a variable on more than two factors needs an equality node
    for id in &ids {
        if !matches!(kind_of(id).as_deref(), Some("variable" | "data")) {
            continue;
        }
        let factors = edges
            .iter()
            .filter(|e| get(e, &["variable"]).as_deref() == Some(id))
            .filter(|e| {
                get(e, &["factor"])
                    .and_then(|f| kind_of(&f))
                    .is_some_and(|k| k != "equality")
            })
            .count();
        if factors > 2 {
            out.push(format!(
                "factor_graph/no-equality: variable {id} meets {factors} factors without an equality node"
            ));
        }
    }
    out
}

/// [`problems`] for the macro's own arguments.
pub(super) fn lint(rows: &[Row], options: &Row) -> Vec<String> {
    match list(options, "edges") {
        Ok(edges) => problems(rows, &edges),
        Err(e) => vec![e],
    }
}

pub(super) fn columns(rows: &[Row], options: &Row) -> Result<Vec<Column>, String> {
    if rows.iter().all(|r| get(r, &["id"]).is_none()) {
        return Err(format!(
            "{} nodes with an id column (and kind, label, layer)",
            super::NEEDS
        ));
    }
    let edges = list(options, "edges")?;
    let marginals = list(options, "marginals")?;
    let kind = |r: &Row| get(r, &["kind"]).map(|k| k.to_ascii_lowercase());
    let id_kind: Vec<(String, Option<String>)> = rows
        .iter()
        .filter_map(|r| get(r, &["id"]).map(|id| (id, kind(r))))
        .collect();
    let kind_of = |id: &str| {
        id_kind
            .iter()
            .find(|(i, _)| i == id)
            .and_then(|(_, k)| k.clone())
    };
    // folding: a variable whose only edge goes to an equality node
    let mut folded: Vec<(String, String)> = Vec::new(); // (variable, equality)
    for (id, k) in &id_kind {
        if !matches!(k.as_deref(), Some("variable" | "data")) {
            continue;
        }
        let mine: Vec<&Row> = edges
            .iter()
            .filter(|e| get(e, &["variable"]).as_deref() == Some(id.as_str()))
            .collect();
        if let [only] = mine[..] {
            if let Some(eq) =
                get(only, &["factor"]).filter(|f| kind_of(f).as_deref() == Some("equality"))
            {
                folded.push((id.clone(), eq));
            }
        }
    }
    let folded_into = |eq: &str| folded.iter().find(|(_, e)| e == eq).map(|(v, _)| v.clone());
    let is_folded = |id: &str| folded.iter().any(|(v, _)| v == id);
    let row_of = |id: &str| rows.iter().find(|r| get(r, &["id"]).as_deref() == Some(id));
    let label_of = |r: &Row| {
        get(r, &["label", "name", "id"])
            .map(|l| tex_lite(&l))
            .unwrap_or_default()
    };
    // the latest marginal per variable
    let latest = |id: &str| -> Option<(f64, Option<f64>)> {
        marginals
            .iter()
            .filter(|m| get(m, &["variable"]).as_deref() == Some(id))
            .filter(|m| get(m, &["component"]).is_none())
            .max_by(|a, b| {
                let it = |m: &Row| get(m, &["iteration"]).and_then(|s| s.parse::<f64>().ok());
                it(a).unwrap_or(0.0).total_cmp(&it(b).unwrap_or(0.0))
            })
            .and_then(|m| {
                let n = |k: &str| get(m, &[k]).and_then(|s| s.parse::<f64>().ok());
                Some((n("mean")?, n("sd")))
            })
    };
    // nodes in band order (the lanes come out in first-row order)
    let layer_rank = |r: &Row| {
        let l = get(r, &["layer"]).unwrap_or_default();
        LAYERS.iter().position(|x| *x == l).unwrap_or(LAYERS.len())
    };
    let mut order: Vec<&Row> = rows
        .iter()
        .filter(|r| get(r, &["id"]).is_some_and(|id| !is_folded(&id)))
        .collect();
    order.sort_by_key(|r| layer_rank(r));
    let (mut ids, mut labels, mut shapes, mut lanes, mut values, mut badges) =
        (vec![], vec![], vec![], vec![], vec![], vec![]);
    for r in &order {
        let id = get(r, &["id"]).unwrap_or_default();
        // an equality node with a folded variable shows (and is) that variable
        let var = folded_into(&id);
        let label = match var.as_deref().and_then(row_of) {
            Some(v) => label_of(v),
            None => label_of(r),
        };
        let m = latest(var.as_deref().unwrap_or(&id));
        ids.push(Value::Str(id));
        labels.push(Value::Str(label));
        shapes.push(kind(r).map_or(Value::Na, Value::Str));
        lanes.push(get(r, &["layer"]).map_or(Value::Na, Value::Str));
        values.push(m.map_or(Value::Na, |(mean, _)| Value::Float(mean)));
        badges.push(m.and_then(|(_, sd)| sd).map_or(Value::Na, |sd| {
            Value::Str(format!(
                "± {}",
                crate::format::format_number_l(
                    sd,
                    MetricFmt::Plain,
                    &crate::format::Locale::default()
                )
            ))
        }));
    }
    let n = ids.len();
    let mut cols = vec![
        Column::new("id", Role::Diagram, ids),
        Column::new("label", Role::Label, labels),
        Column::new("shape", Role::Shape, shapes),
        Column::new("layer", Role::Lane, lanes),
    ];
    if values.iter().any(|v| !matches!(v, Value::Na)) {
        cols.push(Column::new("mean", Role::Metric(MetricFmt::Plain), values));
        cols.push(Column::new("sd", Role::Badge, badges));
    }
    // edges: factor → variable on `out`, variable → factor otherwise; a
    // folded variable's edge to its equality node goes
    let list: Vec<J> = edges
        .iter()
        .filter_map(|e| {
            let (v, f) = (get(e, &["variable"])?, get(e, &["factor"])?);
            if is_folded(&v) {
                return None;
            }
            let iface = get(e, &["interface"]).unwrap_or_default();
            let (from, to) = if iface == "out" { (f, v) } else { (v, f) };
            let mut o = Map::new();
            o.insert("from".into(), J::String(from));
            o.insert("to".into(), J::String(to));
            if !iface.is_empty() {
                o.insert("label".into(), J::String(tex_lite(&iface)));
            }
            Some(J::Object(o))
        })
        .collect();
    if !list.is_empty() {
        let mut v = vec![Value::Na; n];
        if let Some(first) = v.first_mut() {
            *first = Value::Str(J::Array(list).to_string());
        }
        cols.push(Column::new("edges", Role::Edges, v));
    }
    Ok(cols)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(pairs: &[(&str, &str)]) -> Row {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), J::String(v.to_string())))
            .collect()
    }

    /// The coin toss as a producer writes it (two tosses).
    fn coin() -> (Vec<Row>, Row) {
        let nodes = vec![
            row(&[
                ("id", "v0"),
                ("kind", "data"),
                ("label", "y_{0}"),
                ("layer", "obs"),
            ]),
            row(&[
                ("id", "v1"),
                ("kind", "data"),
                ("label", "y_{1}"),
                ("layer", "obs"),
            ]),
            row(&[
                ("id", "v4"),
                ("kind", "variable"),
                ("label", "\\theta"),
                ("layer", "param"),
            ]),
            row(&[
                ("id", "f0"),
                ("kind", "factor"),
                ("label", "Beta"),
                ("layer", "param"),
            ]),
            row(&[
                ("id", "f1"),
                ("kind", "factor"),
                ("label", "Bernoulli"),
                ("layer", "obs"),
            ]),
            row(&[
                ("id", "f2"),
                ("kind", "factor"),
                ("label", "Bernoulli"),
                ("layer", "obs"),
            ]),
            row(&[
                ("id", "e4"),
                ("kind", "equality"),
                ("label", "="),
                ("layer", "param"),
            ]),
        ];
        let edges: Vec<J> = [
            ("e4", "f0", "out"),
            ("v0", "f1", "out"),
            ("e4", "f1", "p"),
            ("v1", "f2", "out"),
            ("e4", "f2", "p"),
            ("v4", "e4", ""),
        ]
        .iter()
        .map(|(v, f, i)| serde_json::json!({"variable": v, "factor": f, "interface": i}))
        .collect();
        let marg = vec![
            serde_json::json!({"variable": "v4", "iteration": 1, "mean": 0.4, "sd": 0.2}),
            serde_json::json!({"variable": "v4", "iteration": 2, "mean": 0.5, "sd": 0.1}),
        ];
        let mut options = Row::new();
        options.insert("edges".into(), J::Array(edges));
        options.insert("marginals".into(), J::Array(marg));
        (nodes, options)
    }

    fn col(cols: &[Column], role: Role) -> &[Value] {
        &cols.iter().find(|c| c.role == role).unwrap().values
    }

    /// θ folds into its equality node; the bands come param first; edges
    /// run factor → variable on `out`; the latest marginal is the value.
    #[test]
    fn maps_a_coin_toss_onto_the_diagram() {
        let (nodes, options) = coin();
        let cols = columns(&nodes, &options).unwrap();
        let ids: Vec<String> = col(&cols, Role::Diagram)
            .iter()
            .map(crate::value_text)
            .collect();
        assert!(!ids.contains(&"v4".to_string()), "θ folded: {ids:?}");
        let at = |id: &str| ids.iter().position(|i| i == id).unwrap();
        assert_eq!(col(&cols, Role::Label)[at("e4")], Value::Str("θ".into()));
        assert_eq!(col(&cols, Role::Label)[at("v0")], Value::Str("y₀".into()));
        assert_eq!(col(&cols, Role::Lane)[0], Value::Str("param".into()));
        assert_eq!(
            col(&cols, Role::Shape)[at("e4")],
            Value::Str("equality".into())
        );
        assert_eq!(
            col(&cols, Role::Metric(MetricFmt::Plain))[at("e4")],
            Value::Float(0.5)
        );
        let edges = crate::value_text(&col(&cols, Role::Edges)[0]);
        assert!(
            edges.contains(r#"{"from":"f1","to":"v0","label":"out"}"#),
            "{edges}"
        );
        assert!(
            edges.contains(r#"{"from":"e4","to":"f1","label":"p"}"#),
            "{edges}"
        );
        assert!(!edges.contains("v4"), "the folded edge is gone: {edges}");
    }

    #[test]
    fn lints_kinds_ids_and_missing_equality_nodes() {
        let (mut nodes, options) = coin();
        assert!(
            lint(&nodes, &options).is_empty(),
            "{:?}",
            lint(&nodes, &options)
        );
        nodes.push(row(&[("id", "q"), ("kind", "gizmo")]));
        let mut o2 = options.clone();
        o2.insert(
            "edges".into(),
            serde_json::json!([
                {"variable": "v4", "factor": "f0"}, {"variable": "v4", "factor": "f1"},
                {"variable": "v4", "factor": "f2"}, {"variable": "zz", "factor": "f0"}
            ]),
        );
        let p = lint(&nodes, &o2).join("\n");
        assert!(p.contains("factor_graph/bad-kind"), "{p}");
        assert!(p.contains("factor_graph/unknown-id: edges name zz"), "{p}");
        assert!(
            p.contains("factor_graph/no-equality: variable v4 meets 3"),
            "{p}"
        );
    }

    #[test]
    fn tex_lite_reads_greek_and_subscripts() {
        assert_eq!(tex_lite("\\theta"), "θ");
        assert_eq!(tex_lite("y_{0}"), "y₀");
        assert_eq!(tex_lite("s_{12}"), "s₁₂");
        assert_eq!(tex_lite("x_t"), "xₜ");
        assert_eq!(tex_lite("big_a"), "big_a");
        assert_eq!(tex_lite("\\sigma^2"), "σ^2");
        assert_eq!(tex_lite("matrix[3×3]"), "matrix[3×3]");
    }
}
