//! The `bayes_net` contract (#1275) — a Bayesian network as tables, as an
//! engine or library (pgmpy, bnlearn, a DuckDB extension) writes it:
//!
//! - nodes `id, name, label, kind, states, layer, group` (kind: `chance`,
//!   `observed`, `latent`, `decision`, `utility`, `deterministic`; states a
//!   LIST of state names);
//! - `arcs := ` `from, to, strength, direction, status` (status: `learned`,
//!   `required`, `forbidden`);
//! - `beliefs := ` `node, state, probability, prior, lo, hi`;
//! - `evidence := ` `node, state` (hard) or `node, state, likelihood`
//!   (soft), `mode` (`observe` | `do`);
//! - `tables := ` `node, parent_states, state, probability`: the
//!   conditional probability tables, `parent_states` in the order of the
//!   node's arcs.
//!
//! `anofox_plot_bayes_net(nodes, arcs := …, beliefs := …)` maps them onto
//! the `::DIAGRAM` roles: `kind` → `::SHAPE`, the states with their belief,
//! prior, evidence and table → `::BELIEF` (state bars in the box, the card),
//! `layer` → `::XAXIS` (tiers in first-appearance order), `group` →
//! `::CATEGORY`, the arcs → `::EDGES` (`value` the strength, dashed below
//! the threshold, `tip` what hover says), drawn top to bottom.

use crate::Row;
use anofox_viz_plugin::OutColumn;
use serde_json::{json, Map, Value as J};

/// The node kinds the contract knows.
pub(crate) const KINDS: &[&str] = &[
    "chance",
    "observed",
    "latent",
    "decision",
    "utility",
    "deterministic",
];

/// How far a node's beliefs may sum from 1.
const SUM_TOLERANCE: f64 = 1e-6;

/// At most this many parent configurations of a table go to the card.
const MAX_TABLE_ROWS: usize = 64;

fn text(v: &J) -> Option<String> {
    match v {
        J::Null => None,
        J::String(s) => Some(s.clone()),
        J::Number(n) => Some(n.to_string()),
        J::Bool(b) => Some(b.to_string()),
        other => Some(other.to_string()),
    }
}

/// The value of one of `names` in `row` (case ignored).
fn field<'a>(row: &'a Row, names: &[&str]) -> Option<&'a J> {
    names.iter().find_map(|n| {
        row.iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(n))
            .map(|(_, v)| v)
            .filter(|v| !v.is_null())
    })
}

fn get(row: &Row, names: &[&str]) -> Option<String> {
    field(row, names)
        .and_then(text)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn num(row: &Row, names: &[&str]) -> Option<f64> {
    match field(row, names)? {
        J::Number(n) => n.as_f64(),
        J::String(s) => s.trim().parse().ok(),
        _ => None,
    }
    .filter(|f| f.is_finite())
}

/// A LIST cell (`states`, `parent_states`): a JSON list, its text, or
/// DuckDB's list text `[a, b]`.
fn names(row: &Row, key: &[&str]) -> Vec<String> {
    let items = |a: &[J]| a.iter().filter_map(text).collect::<Vec<_>>();
    match field(row, key) {
        Some(J::Array(a)) => items(a),
        Some(J::String(s)) => match serde_json::from_str::<J>(s) {
            Ok(J::Array(a)) => items(&a),
            _ => s
                .trim()
                .trim_start_matches('[')
                .trim_end_matches(']')
                .split(',')
                .map(|p| p.trim().trim_matches(|c| c == '\'' || c == '"').to_string())
                .filter(|p| !p.is_empty())
                .collect(),
        },
        _ => Vec::new(),
    }
}

/// A list-of-structs option (`arcs :=`, `beliefs :=`, …) as rows.
fn list(options: &Row, name: &str) -> Result<Vec<Row>, String> {
    let v = match options.get(name) {
        None | Some(J::Null) => return Ok(Vec::new()),
        Some(J::String(s)) => serde_json::from_str::<J>(s)
            .map_err(|e| format!("{name} := is not a list of structs: {e}"))?,
        Some(v) => v.clone(),
    };
    let J::Array(items) = v else {
        return Err(format!(
            "{name} := is not a list of structs — pass (SELECT list(a) FROM {name} a)"
        ));
    };
    Ok(items
        .into_iter()
        .filter_map(|i| i.as_object().cloned())
        .collect())
}

/// The network as read: nodes (id, row) in row order, the drawn arcs.
struct Net<'a> {
    nodes: Vec<(String, &'a Row)>,
    /// (from, to, row), forbidden arcs left out.
    arcs: Vec<(String, String, Row)>,
}

impl<'a> Net<'a> {
    fn read(rows: &'a [Row], arcs: Vec<Row>) -> Net<'a> {
        let mut nodes: Vec<(String, &Row)> = Vec::new();
        for r in rows {
            if let Some(id) = get(r, &["id"]) {
                if !nodes.iter().any(|(i, _)| *i == id) {
                    nodes.push((id, r));
                }
            }
        }
        let arcs = arcs
            .into_iter()
            .filter(|a| !get(a, &["status"]).is_some_and(|s| s.eq_ignore_ascii_case("forbidden")))
            .filter_map(|a| Some((get(&a, &["from"])?, get(&a, &["to"])?, a)))
            .collect();
        Net { nodes, arcs }
    }

    fn row(&self, id: &str) -> Option<&Row> {
        self.nodes.iter().find(|(i, _)| i == id).map(|(_, r)| *r)
    }

    fn has(&self, id: &str) -> bool {
        self.row(id).is_some()
    }

    fn states(&self, id: &str) -> Vec<String> {
        self.row(id)
            .map(|r| names(r, &["states"]))
            .unwrap_or_default()
    }

    fn kind(&self, id: &str) -> String {
        self.row(id)
            .and_then(|r| get(r, &["kind"]))
            .map(|k| k.to_ascii_lowercase())
            .filter(|k| KINDS.contains(&k.as_str()))
            .unwrap_or_else(|| "chance".into())
    }

    fn label(&self, id: &str) -> String {
        self.row(id)
            .and_then(|r| get(r, &["label", "name"]))
            .unwrap_or_else(|| id.to_string())
    }

    /// The node's parents, in arc order.
    fn parents(&self, id: &str) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for (f, t, _) in &self.arcs {
            if t == id && !out.contains(f) {
                out.push(f.clone());
            }
        }
        out
    }

    /// A cycle among the drawn arcs, as its ids (the first one again at
    /// the end), if there is one.
    fn cycle(&self) -> Option<Vec<String>> {
        let ids: Vec<&str> = self.nodes.iter().map(|(i, _)| i.as_str()).collect();
        let at = |id: &str| ids.iter().position(|i| *i == id);
        let succ: Vec<Vec<usize>> = (0..ids.len())
            .map(|u| {
                self.arcs
                    .iter()
                    .filter(|(f, _, _)| at(f) == Some(u))
                    .filter_map(|(_, t, _)| at(t))
                    .collect()
            })
            .collect();
        // 0 unvisited, 1 on the stack, 2 done
        let mut state = vec![0u8; ids.len()];
        for s in 0..ids.len() {
            if state[s] != 0 {
                continue;
            }
            // (vertex, next successor to try)
            let mut stack: Vec<(usize, usize)> = vec![(s, 0)];
            state[s] = 1;
            while let Some(&mut (u, ref mut k)) = stack.last_mut() {
                if let Some(&v) = succ[u].get(*k) {
                    *k += 1;
                    match state[v] {
                        0 => {
                            state[v] = 1;
                            stack.push((v, 0));
                        }
                        1 => {
                            let from = stack.iter().position(|&(w, _)| w == v).unwrap_or(0);
                            let mut path: Vec<String> = stack[from..]
                                .iter()
                                .map(|&(w, _)| ids[w].to_string())
                                .collect();
                            path.push(ids[v].to_string());
                            return Some(path);
                        }
                        _ => {}
                    }
                } else {
                    state[u] = 2;
                    stack.pop();
                }
            }
        }
        None
    }
}

/// What the lint reports about a network (`bayes_net/*`).
fn problems(rows: &[Row], options: &Row) -> Result<Vec<String>, String> {
    let all_arcs = list(options, "arcs")?;
    let beliefs = list(options, "beliefs")?;
    let evidence = list(options, "evidence")?;
    let tables = list(options, "tables")?;
    let net = Net::read(rows, all_arcs.clone());
    let mut out = Vec::new();
    let bad: Vec<String> = rows
        .iter()
        .filter_map(|r| get(r, &["kind"]))
        .filter(|k| !KINDS.contains(&k.to_ascii_lowercase().as_str()))
        .collect();
    if !bad.is_empty() {
        out.push(format!(
            "bayes_net/bad-kind: kind {} is not one of {} — drawn as chance",
            bad.join(", "),
            KINDS.join(", ")
        ));
    }
    // ids the arcs, beliefs, evidence and tables name that are not nodes
    let mut unknown: Vec<String> = Vec::new();
    let mut note = |id: String| {
        if !net.has(&id) && !unknown.contains(&id) {
            unknown.push(id);
        }
    };
    for a in &all_arcs {
        for id in [get(a, &["from"]), get(a, &["to"])].into_iter().flatten() {
            note(id);
        }
    }
    for r in beliefs.iter().chain(&evidence).chain(&tables) {
        if let Some(id) = get(r, &["node"]) {
            note(id);
        }
    }
    if !unknown.is_empty() {
        out.push(format!(
            "bayes_net/unknown-id: arcs, beliefs, evidence or tables name {} that are not nodes",
            unknown.join(", ")
        ));
    }
    if let Some(c) = net.cycle() {
        out.push(format!(
            "bayes_net/cycle: the arcs {} form a cycle — a Bayesian network is acyclic",
            c.join(" → ")
        ));
    }
    // beliefs that don't sum to 1, per node
    let mut sums: Vec<(String, f64)> = Vec::new();
    for b in &beliefs {
        let (Some(id), Some(p)) = (get(b, &["node"]), num(b, &["probability", "p"])) else {
            continue;
        };
        match sums.iter_mut().find(|(i, _)| *i == id) {
            Some(s) => s.1 += p,
            None => sums.push((id, p)),
        }
    }
    let off: Vec<String> = sums
        .iter()
        .filter(|(id, s)| net.has(id) && (s - 1.0).abs() > SUM_TOLERANCE)
        .map(|(id, s)| format!("{id} ({})", (s * 1e6).round() / 1e6))
        .collect();
    if !off.is_empty() {
        out.push(format!(
            "bayes_net/beliefs-sum: the beliefs of {} do not sum to 1",
            off.join(", ")
        ));
    }
    // states the beliefs, evidence and tables name that the node lacks
    let mut strays: Vec<String> = Vec::new();
    for r in beliefs.iter().chain(&evidence).chain(&tables) {
        let (Some(id), Some(s)) = (get(r, &["node"]), get(r, &["state"])) else {
            continue;
        };
        let states = net.states(&id);
        let stray = format!("{id}: {s}");
        if net.has(&id) && !states.contains(&s) && !strays.contains(&stray) {
            strays.push(stray);
        }
    }
    if !strays.is_empty() {
        out.push(format!(
            "bayes_net/unknown-state: {} not among the node's states",
            strays.join(", ")
        ));
    }
    Ok(out)
}

/// [`problems`] for the macro's own arguments.
pub(crate) fn lint(rows: &[Row], options: &Row) -> Vec<String> {
    problems(rows, options).unwrap_or_else(|e| vec![e])
}

/// A number as hover text: at most 3 decimals.
fn short(v: f64) -> String {
    format!("{}", (v * 1000.0).round() / 1000.0)
}

/// The `::DIAGRAM` columns of a network.
pub(crate) fn columns(rows: &[Row], options: &Row) -> Result<Vec<OutColumn>, String> {
    if rows.iter().all(|r| get(r, &["id"]).is_none()) {
        return Err(
            "needs column(s) nodes with an id column (and label, kind, states)".to_string(),
        );
    }
    let net = Net::read(rows, list(options, "arcs")?);
    let beliefs = list(options, "beliefs")?;
    let evidence = list(options, "evidence")?;
    let tables = list(options, "tables")?;
    let threshold = num(options, &["threshold"]).unwrap_or(0.5);
    let direction = get(options, &["direction"])
        .map(|d| d.to_ascii_uppercase())
        .filter(|d| d == "LR" || d == "TB")
        .unwrap_or_else(|| "TB".into());
    let n = net.nodes.len();
    let of = |list: &[Row], id: &str| -> Vec<Row> {
        list.iter()
            .filter(|r| get(r, &["node"]).as_deref() == Some(id))
            .cloned()
            .collect()
    };

    let (mut ids, mut labels, mut shapes, mut cards) = (vec![], vec![], vec![], vec![]);
    let (mut tiers, mut groups) = (vec![], vec![]);
    let mut tier_names: Vec<String> = Vec::new();
    for (id, row) in &net.nodes {
        let kind = net.kind(id);
        ids.push(J::String(id.clone()));
        labels.push(J::String(net.label(id)));
        shapes.push(J::String(
            match kind.as_str() {
                "decision" => "decision",
                "utility" => "utility",
                "deterministic" => "double-box",
                _ => "chance",
            }
            .into(),
        ));
        tiers.push(match get(row, &["layer"]) {
            Some(l) => {
                let k = tier_names.iter().position(|t| *t == l).unwrap_or_else(|| {
                    tier_names.push(l);
                    tier_names.len() - 1
                });
                json!(k)
            }
            None => J::Null,
        });
        groups.push(get(row, &["group"]).map_or(J::Null, J::String));
        let states = net.states(id);
        if kind == "utility" || states.is_empty() {
            cards.push(J::Null);
            continue;
        }
        // the belief per state, in the node's order (states it lacks are
        // the lint's)
        let mine = of(&beliefs, id);
        let state_rows: Vec<J> = states
            .iter()
            .map(|s| {
                let b = mine
                    .iter()
                    .find(|b| get(b, &["state"]).as_deref() == Some(s.as_str()));
                let mut o = Map::new();
                o.insert("state".into(), J::String(s.clone()));
                for (k, names) in [
                    ("p", &["probability", "p"][..]),
                    ("prior", &["prior"][..]),
                    ("lo", &["lo"][..]),
                    ("hi", &["hi"][..]),
                ] {
                    if let Some(v) = b.and_then(|b| num(b, names)) {
                        o.insert(k.into(), json!(v));
                    }
                }
                J::Object(o)
            })
            .collect();
        let mut card = Map::new();
        card.insert("kind".into(), J::String(kind.clone()));
        card.insert("states".into(), J::Array(state_rows));
        // evidence: a hard state, or a likelihood per state (soft)
        let ev = of(&evidence, id);
        if !ev.is_empty() {
            let soft: Map<String, J> = ev
                .iter()
                .filter_map(|e| Some((get(e, &["state"])?, json!(num(e, &["likelihood"])?))))
                .collect();
            let mode = ev
                .iter()
                .find_map(|e| get(e, &["mode"]))
                .map(|m| m.to_ascii_lowercase());
            let mut e = Map::new();
            if soft.is_empty() {
                if let Some(s) = ev.iter().find_map(|e| get(e, &["state"])) {
                    e.insert("state".into(), J::String(s));
                }
                e.insert(
                    "mode".into(),
                    J::String(mode.unwrap_or_else(|| "observe".into())),
                );
            } else {
                e.insert("mode".into(), J::String("soft".into()));
                e.insert("likelihood".into(), J::Object(soft));
            }
            card.insert("evidence".into(), J::Object(e));
        }
        // the conditional probability table: a row per parent configuration
        let tab = of(&tables, id);
        if !tab.is_empty() {
            let mut configs: Vec<(Vec<String>, Vec<Option<f64>>)> = Vec::new();
            for t in &tab {
                let (Some(s), Some(p)) = (get(t, &["state"]), num(t, &["probability", "p"])) else {
                    continue;
                };
                let Some(k) = states.iter().position(|x| *x == s) else {
                    continue;
                };
                let ps = names(t, &["parent_states", "parents"]);
                let at = match configs.iter().position(|(c, _)| *c == ps) {
                    Some(i) => i,
                    None if configs.len() < MAX_TABLE_ROWS => {
                        configs.push((ps, vec![None; states.len()]));
                        configs.len() - 1
                    }
                    None => continue,
                };
                configs[at].1[k] = Some(p);
            }
            let parents: Vec<J> = net
                .parents(id)
                .iter()
                .map(|p| J::String(net.label(p)))
                .collect();
            let total = tab
                .iter()
                .map(|t| names(t, &["parent_states", "parents"]))
                .fold(Vec::<Vec<String>>::new(), |mut acc, c| {
                    if !acc.contains(&c) {
                        acc.push(c);
                    }
                    acc
                })
                .len();
            card.insert(
                "cpt".into(),
                json!({
                    "parents": parents,
                    "rows": configs
                        .into_iter()
                        .map(|(c, p)| json!({"parents": c, "p": p}))
                        .collect::<Vec<_>>(),
                    "total": total,
                }),
            );
        }
        cards.push(J::Object(card));
    }
    // a node's card as the text ::BELIEF reads
    let cards: Vec<J> = cards
        .into_iter()
        .map(|c| match c {
            J::Null => J::Null,
            c => J::String(c.to_string()),
        })
        .collect();

    let arcs: Vec<J> = net
        .arcs
        .iter()
        .filter(|(f, t, _)| net.has(f) && net.has(t))
        .map(|(f, t, a)| {
            let strength = num(a, &["strength"]);
            let status = get(a, &["status"]).map(|s| s.to_ascii_lowercase());
            let required = status.as_deref() == Some("required");
            let mut tip: Vec<String> = Vec::new();
            if let Some(s) = strength {
                tip.push(format!("strength {}", short(s)));
            }
            if let Some(d) = num(a, &["direction"]) {
                tip.push(format!("direction {}", short(d)));
            }
            if let Some(s) = &status {
                tip.push(s.clone());
            }
            let mut o = Map::new();
            o.insert("from".into(), J::String(f.clone()));
            o.insert("to".into(), J::String(t.clone()));
            if let Some(s) = strength {
                o.insert("value".into(), json!(s));
                // 1.2 px (never found) to 2.8 px (always found)
                let w = 1.2 + 1.6 * s.clamp(0.0, 1.0);
                o.insert("width".into(), json!((w * 10.0).round() / 10.0));
            }
            if strength.is_some_and(|s| s < threshold) && !required {
                o.insert("style".into(), J::String("dashed".into()));
            }
            if !tip.is_empty() {
                o.insert("tip".into(), J::String(tip.join(", ")));
            }
            J::Object(o)
        })
        .collect();

    let first = |v: J| {
        let mut c = vec![J::Null; n];
        if let Some(f) = c.first_mut() {
            *f = v;
        }
        c
    };
    let col = |name: &str, role: &str, values: Vec<J>| OutColumn {
        name: name.into(),
        role: role.into(),
        values,
    };
    let mut cols = vec![
        col("id", "DIAGRAM", ids),
        col("label", "LABEL", labels),
        col("kind", "SHAPE", shapes),
        col("direction", "DIRECTION", first(J::String(direction))),
    ];
    if cards.iter().any(|c| !c.is_null()) {
        cols.push(col("belief", "BELIEF", cards));
    }
    if tiers.iter().any(|t| !t.is_null()) {
        cols.push(col("layer", "XAXIS", tiers));
    }
    if groups.iter().any(|g| !g.is_null()) {
        cols.push(col("group", "CATEGORY", groups));
    }
    if !arcs.is_empty() {
        cols.push(col(
            "arcs",
            "EDGES",
            first(J::String(J::Array(arcs).to_string())),
        ));
    }
    Ok(cols)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: &str, label: &str, kind: &str, states: &[&str]) -> Row {
        json!({"id": id, "label": label, "kind": kind, "states": states})
            .as_object()
            .unwrap()
            .clone()
    }

    fn opts(v: J) -> Row {
        v.as_object().unwrap().clone()
    }

    /// The sprinkler network: cloudy → sprinkler, rain → wet grass.
    fn sprinkler() -> (Vec<Row>, Row) {
        let yn = ["yes", "no"];
        let nodes = vec![
            node("cloudy", "Cloudy", "chance", &yn),
            node("sprinkler", "Sprinkler", "chance", &yn),
            node("rain", "Rain", "chance", &yn),
            node("grass", "Wet grass", "observed", &yn),
        ];
        let arc = |f: &str, t: &str, s: f64| json!({"from": f, "to": t, "strength": s});
        let b = |n: &str, s: &str, p: f64, q: f64| json!({"node": n, "state": s, "probability": p, "prior": q});
        let options = opts(json!({
            "arcs": [arc("cloudy", "sprinkler", 0.9), arc("cloudy", "rain", 0.95),
                     arc("sprinkler", "grass", 0.4), arc("rain", "grass", 0.99)],
            "beliefs": [b("cloudy", "yes", 0.576, 0.5), b("cloudy", "no", 0.424, 0.5),
                        b("rain", "yes", 0.708, 0.5), b("rain", "no", 0.292, 0.5),
                        b("sprinkler", "yes", 0.43, 0.3), b("sprinkler", "no", 0.57, 0.7),
                        b("grass", "yes", 1.0, 0.647), b("grass", "no", 0.0, 0.353)],
            "evidence": [{"node": "grass", "state": "yes"}],
            "tables": [
                {"node": "rain", "parent_states": ["yes"], "state": "yes", "probability": 0.8},
                {"node": "rain", "parent_states": ["yes"], "state": "no", "probability": 0.2},
                {"node": "rain", "parent_states": ["no"], "state": "yes", "probability": 0.2},
                {"node": "rain", "parent_states": ["no"], "state": "no", "probability": 0.8},
            ],
        }));
        (nodes, options)
    }

    fn col<'a>(cols: &'a [OutColumn], role: &str) -> &'a [J] {
        &cols.iter().find(|c| c.role == role).unwrap().values
    }

    #[test]
    fn maps_the_sprinkler_onto_the_diagram() {
        let (nodes, options) = sprinkler();
        let cols = columns(&nodes, &options).unwrap();
        let ids: Vec<&str> = col(&cols, "DIAGRAM")
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(ids, ["cloudy", "sprinkler", "rain", "grass"]);
        assert_eq!(col(&cols, "LABEL")[3], "Wet grass");
        assert_eq!(col(&cols, "SHAPE")[0], "chance");
        assert_eq!(col(&cols, "DIRECTION")[0], "TB");
        // rain's card: its states with belief and prior, and its table with
        // the parent named by its label
        let card: J = serde_json::from_str(col(&cols, "BELIEF")[2].as_str().unwrap()).unwrap();
        assert_eq!(card["kind"], "chance");
        assert_eq!(
            card["states"][0],
            json!({"state": "yes", "p": 0.708, "prior": 0.5})
        );
        assert_eq!(card["cpt"]["parents"], json!(["Cloudy"]));
        assert_eq!(
            card["cpt"]["rows"][1],
            json!({"parents": ["no"], "p": [0.2, 0.8]})
        );
        // wet grass observed yes
        let grass: J = serde_json::from_str(col(&cols, "BELIEF")[3].as_str().unwrap()).unwrap();
        assert_eq!(
            grass["evidence"],
            json!({"state": "yes", "mode": "observe"})
        );
        // arcs: strength as value, the weak one dashed, hover text
        let arcs = col(&cols, "EDGES")[0].as_str().unwrap();
        assert!(
            arcs.contains(r#"{"from":"sprinkler","to":"grass","value":0.4,"width":1.8,"style":"dashed","tip":"strength 0.4"}"#),
            "{arcs}"
        );
        assert!(
            arcs.contains(
                r#"{"from":"rain","to":"grass","value":0.99,"width":2.8,"tip":"strength 0.99"}"#
            ),
            "{arcs}"
        );
        assert!(
            lint(&nodes, &options).is_empty(),
            "{:?}",
            lint(&nodes, &options)
        );
    }

    /// Kinds as shapes; layers as tiers in first-appearance order; groups as
    /// categories; a required arc stays solid; a forbidden one is not drawn.
    #[test]
    fn kinds_tiers_groups_and_arc_status() {
        let mut nodes = vec![
            node("d", "Treat?", "decision", &["yes", "no"]),
            node("u", "Cost", "utility", &[]),
            node("x", "Either", "deterministic", &["yes", "no"]),
            node("z", "Odd", "spooky", &["a"]),
        ];
        nodes[0].insert("layer".into(), json!("act"));
        nodes[2].insert("layer".into(), json!("model"));
        nodes[3].insert("layer".into(), json!("act"));
        nodes[2].insert("group".into(), json!("lungs"));
        let options = opts(json!({"arcs": [
            {"from": "d", "to": "u", "strength": 0.2, "status": "required"},
            {"from": "x", "to": "u", "strength": 0.3, "direction": 0.75},
            {"from": "u", "to": "z", "status": "forbidden"},
        ]}));
        let cols = columns(&nodes, &options).unwrap();
        assert_eq!(
            col(&cols, "SHAPE"),
            ["decision", "utility", "double-box", "chance"]
        );
        assert_eq!(col(&cols, "XAXIS"), [json!(0), J::Null, json!(1), json!(0)]);
        assert_eq!(col(&cols, "CATEGORY")[2], "lungs");
        // no states, no beliefs: a utility has no card
        assert!(col(&cols, "BELIEF")[1].is_null());
        let arcs = col(&cols, "EDGES")[0].as_str().unwrap();
        assert!(
            arcs.contains(
                r#"{"from":"d","to":"u","value":0.2,"width":1.5,"tip":"strength 0.2, required"}"#
            ),
            "{arcs}"
        );
        assert!(
            arcs.contains(r#""style":"dashed","tip":"strength 0.3, direction 0.75""#),
            "{arcs}"
        );
        assert!(!arcs.contains(r#""to":"z""#), "forbidden: {arcs}");
        assert_eq!(
            lint(&nodes, &options),
            ["bayes_net/bad-kind: kind spooky is not one of chance, observed, latent, decision, utility, deterministic — drawn as chance"]
        );
    }

    #[test]
    fn lints_cycles_unknown_ids_sums_and_states() {
        let yn = ["yes", "no"];
        let nodes = vec![
            node("a", "A", "chance", &yn),
            node("b", "B", "chance", &yn),
            node("c", "C", "chance", &yn),
        ];
        let options = opts(json!({
            "arcs": [{"from": "a", "to": "b"}, {"from": "b", "to": "c"},
                     {"from": "c", "to": "a"}, {"from": "a", "to": "ghost"}],
            "beliefs": [{"node": "a", "state": "yes", "probability": 0.5},
                        {"node": "a", "state": "no", "probability": 0.4},
                        {"node": "b", "state": "yes", "probability": 0.3},
                        {"node": "b", "state": "no", "probability": 0.7000001},
                        {"node": "c", "state": "maybe", "probability": 1.0}],
            "evidence": [{"node": "b", "state": "perhaps"}],
        }));
        let l = lint(&nodes, &options);
        assert_eq!(l.len(), 4, "{l:?}");
        assert_eq!(
            l[0],
            "bayes_net/unknown-id: arcs, beliefs, evidence or tables name ghost that are not nodes"
        );
        assert_eq!(
            l[1],
            "bayes_net/cycle: the arcs a → b → c → a form a cycle — a Bayesian network is acyclic"
        );
        // b is within 1e-6 of 1; a is not
        assert_eq!(
            l[2],
            "bayes_net/beliefs-sum: the beliefs of a (0.9) do not sum to 1"
        );
        assert_eq!(
            l[3],
            "bayes_net/unknown-state: c: maybe, b: perhaps not among the node's states"
        );
        // a forbidden arc closes no cycle
        let options = opts(json!({"arcs": [{"from": "a", "to": "b"},
            {"from": "b", "to": "a", "status": "forbidden"}]}));
        assert!(lint(&nodes, &options).is_empty());
    }

    /// Soft evidence (a likelihood per state) and an intervention.
    #[test]
    fn soft_evidence_and_do() {
        let yn = ["yes", "no"];
        let nodes = vec![node("a", "A", "chance", &yn), node("b", "B", "chance", &yn)];
        let options = opts(json!({"evidence": [
            {"node": "a", "state": "yes", "likelihood": 0.8},
            {"node": "a", "state": "no", "likelihood": 0.2},
            {"node": "b", "state": "no", "mode": "do"},
        ]}));
        let cols = columns(&nodes, &options).unwrap();
        let card = |i: usize| -> J {
            serde_json::from_str(col(&cols, "BELIEF")[i].as_str().unwrap()).unwrap()
        };
        assert_eq!(
            card(0)["evidence"],
            json!({"mode": "soft", "likelihood": {"yes": 0.8, "no": 0.2}})
        );
        assert_eq!(card(1)["evidence"], json!({"state": "no", "mode": "do"}));
    }

    #[test]
    fn states_arrive_as_list_text_too() {
        let mut r = Row::new();
        r.insert("id".into(), json!("a"));
        r.insert("states".into(), json!("[low, 'mid', high]"));
        assert_eq!(names(&r, &["states"]), ["low", "mid", "high"]);
        r.insert("states".into(), json!("[\"x\",\"y\"]"));
        assert_eq!(names(&r, &["states"]), ["x", "y"]);
    }

    #[test]
    fn needs_node_ids() {
        let e = columns(&[opts(json!({"what": 1}))], &Row::new())
            .err()
            .unwrap();
        assert!(e.contains("an id column"), "{e}");
    }
}
