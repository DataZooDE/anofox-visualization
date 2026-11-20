//! Influence diagrams (#1279): a decision's policy and a utility's
//! expected value.
//!
//! - `policy := ` `decision, parent_states, option, expected_utility,
//!   chosen`: a row per configuration of the decision's informational
//!   parents (the nodes with an arc into it, `parent_states` in the order of
//!   those arcs) and option, with the option's expected utility given that
//!   configuration; `chosen` marks the option the policy picks (without it,
//!   the one with the highest expected utility). It goes into the decision's
//!   card as `policy`, drawn as a table in its box; the configuration the
//!   evidence picks (every informational parent observed) is `current`.
//! - a utility node's expected value: a `beliefs` row of the node with
//!   `expected_utility` (or `utility`, `value`) and no state, written in its
//!   diamond.
//! - the arcs into a decision are information arcs (what is known when it
//!   is made): drawn dotted.

use super::{field, get, list, names, num, Net};
use crate::Row;
use serde_json::{json, Map, Value as J};

/// At most this many configurations of a policy go to the card.
const MAX_POLICY_ROWS: usize = 32;

/// How far a chosen option's expected utility may fall below the best.
const EU_TOLERANCE: f64 = 1e-9;

/// The policy rows of `decision`.
fn rows_of<'p>(policy: &'p [Row], decision: &str) -> Vec<&'p Row> {
    policy
        .iter()
        .filter(|r| get(r, &["decision", "node"]).as_deref() == Some(decision))
        .collect()
}

/// `chosen` as a flag (true, 1, 'yes', …).
fn chosen(r: &Row) -> Option<bool> {
    match field(r, &["chosen"])? {
        J::Bool(b) => Some(*b),
        J::Number(n) => n.as_f64().map(|v| v != 0.0),
        J::String(s) => Some(matches!(
            s.trim().to_ascii_lowercase().as_str(),
            "true" | "t" | "yes" | "y" | "1"
        )),
        _ => None,
    }
}

/// An expected utility.
fn eu(r: &Row) -> Option<f64> {
    num(r, &["expected_utility", "eu", "utility"])
}

/// One configuration of a policy: the parents' states, the expected
/// utility per option (in the options' order), the chosen option.
struct Config {
    parents: Vec<String>,
    eu: Vec<Option<f64>>,
    flagged: Vec<usize>,
}

impl Config {
    /// The chosen option: the one flagged, else the best.
    fn choice(&self) -> Option<usize> {
        self.flagged.first().copied().or_else(|| self.best())
    }

    fn best(&self) -> Option<usize> {
        self.eu
            .iter()
            .enumerate()
            .filter_map(|(k, v)| Some((k, (*v)?)))
            .fold(None, |b: Option<(usize, f64)>, (k, v)| match b {
                Some((_, bv)) if bv >= v => b,
                _ => Some((k, v)),
            })
            .map(|(k, _)| k)
    }
}

/// The decision's options (its states, then any option only the policy
/// names) and its configurations, in first-appearance order.
fn configs(net: &Net, decision: &str, rows: &[&Row]) -> (Vec<String>, Vec<Config>) {
    let mut options = net.states(decision);
    for r in rows {
        if let Some(o) = get(r, &["option", "state"]) {
            if !options.contains(&o) {
                options.push(o);
            }
        }
    }
    let mut out: Vec<Config> = Vec::new();
    for r in rows {
        let Some(o) = get(r, &["option", "state"]) else {
            continue;
        };
        let k = options.iter().position(|x| *x == o).unwrap_or(0);
        let ps = names(r, &["parent_states", "parents"]);
        let at = match out.iter().position(|c| c.parents == ps) {
            Some(i) => i,
            None => {
                out.push(Config {
                    parents: ps,
                    eu: vec![None; options.len()],
                    flagged: Vec::new(),
                });
                out.len() - 1
            }
        };
        out[at].eu[k] = eu(r);
        if chosen(r) == Some(true) && !out[at].flagged.contains(&k) {
            out[at].flagged.push(k);
        }
    }
    (options, out)
}

/// The observed (or set) state of each of `ids`, if every one has one.
fn observed(ids: &[String], evidence: &[Row]) -> Option<Vec<String>> {
    ids.iter()
        .map(|id| {
            let mine: Vec<&Row> = evidence
                .iter()
                .filter(|e| get(e, &["node"]).as_deref() == Some(id.as_str()))
                .collect();
            // hard evidence only: one state, no likelihood
            match mine.as_slice() {
                [e] if num(e, &["likelihood"]).is_none() => get(e, &["state"]),
                _ => None,
            }
        })
        .collect()
}

/// The `policy` of a decision's card: `{parents, options, rows: [{parents,
/// eu, chosen}], current, total}`; `None` when the policy names it not.
pub(super) fn policy_card(net: &Net, id: &str, policy: &[Row], evidence: &[Row]) -> Option<J> {
    let rows = rows_of(policy, id);
    if rows.is_empty() {
        return None;
    }
    let (options, configs) = configs(net, id, &rows);
    let parents = net.parents(id);
    let current = (!parents.is_empty())
        .then(|| observed(&parents, evidence))
        .flatten()
        .and_then(|seen| configs.iter().position(|c| c.parents == seen));
    let total = configs.len();
    let mut o = Map::new();
    o.insert(
        "parents".into(),
        J::Array(parents.iter().map(|p| J::String(net.label(p))).collect()),
    );
    o.insert("options".into(), json!(options));
    o.insert(
        "rows".into(),
        J::Array(
            configs
                .iter()
                .take(MAX_POLICY_ROWS)
                .map(|c| json!({"parents": c.parents, "eu": c.eu, "chosen": c.choice()}))
                .collect(),
        ),
    );
    if let Some(k) = current.filter(|k| *k < MAX_POLICY_ROWS) {
        o.insert("current".into(), json!(k));
    }
    o.insert("total".into(), json!(total));
    Some(J::Object(o))
}

/// A utility node's expected value (a `beliefs` row of it without a state).
pub(super) fn expected_utility(beliefs: &[Row], id: &str) -> Option<f64> {
    beliefs
        .iter()
        .filter(|b| get(b, &["node"]).as_deref() == Some(id))
        .find_map(|b| num(b, &["expected_utility", "utility", "value"]))
}

/// An information arc: into a decision.
pub(super) fn information(net: &Net, to: &str) -> bool {
    net.kind(to) == "decision"
}

/// What the lint reports about a policy (`bayes_net/policy-*`).
pub(super) fn problems(net: &Net, options: &Row) -> Result<Vec<String>, String> {
    let policy = list(options, "policy")?;
    let mut out = Vec::new();
    let mut decisions: Vec<String> = Vec::new();
    for r in &policy {
        if let Some(d) = get(r, &["decision", "node"]) {
            if !decisions.contains(&d) {
                decisions.push(d);
            }
        }
    }
    let not: Vec<&String> = decisions
        .iter()
        .filter(|d| net.has(d) && net.kind(d) != "decision")
        .collect();
    if !not.is_empty() {
        out.push(format!(
            "bayes_net/policy-not-decision: the policy names {} that are not decision nodes",
            not.iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    let (mut strays, mut arity, mut picks, mut worse) = (vec![], vec![], vec![], vec![]);
    for d in decisions.iter().filter(|d| net.has(d)) {
        let rows = rows_of(&policy, d);
        let states = net.states(d);
        let (opts, configs) = configs(net, d, &rows);
        for o in opts.iter().filter(|o| !states.contains(o)) {
            strays.push(format!("{d}: {o}"));
        }
        let n = net.parents(d).len();
        if configs.iter().any(|c| c.parents.len() != n) {
            arity.push(format!("{d} ({n})"));
        }
        let show = |c: &Config| {
            if c.parents.is_empty() {
                d.clone()
            } else {
                format!("{d} [{}]", c.parents.join(", "))
            }
        };
        for c in &configs {
            if c.flagged.len() > 1 {
                picks.push(show(c));
            }
            if let ([k], Some(b)) = (c.flagged.as_slice(), c.best()) {
                let (Some(v), Some(bv)) = (c.eu[*k], c.eu[b]) else {
                    continue;
                };
                if v < bv - EU_TOLERANCE {
                    worse.push(show(c));
                }
            }
        }
    }
    if !strays.is_empty() {
        out.push(format!(
            "bayes_net/policy-option: {} not among the decision's states",
            strays.join(", ")
        ));
    }
    if !arity.is_empty() {
        out.push(format!(
            "bayes_net/policy-parents: parent_states of {} do not match the number of arcs into the decision",
            arity.join(", ")
        ));
    }
    if !picks.is_empty() {
        out.push(format!(
            "bayes_net/policy-chosen: more than one chosen option for {}",
            picks.join(", ")
        ));
    }
    if !worse.is_empty() {
        out.push(format!(
            "bayes_net/policy-not-best: the chosen option of {} has a lower expected utility than another",
            worse.join(", ")
        ));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::super::{columns, lint};
    use crate::Row;
    use anofox_viz_plugin::OutColumn;
    use serde_json::{json, Value as J};

    fn rows(v: J) -> Vec<Row> {
        v.as_array()
            .unwrap()
            .iter()
            .map(|r| r.as_object().unwrap().clone())
            .collect()
    }

    fn col<'a>(cols: &'a [OutColumn], role: &str) -> &'a [J] {
        &cols.iter().find(|c| c.role == role).unwrap().values
    }

    /// The oil wildcatter: test? → seismic → drill? → payoff.
    fn wildcatter() -> (Vec<Row>, Row) {
        let nodes = rows(json!([
            {"id": "test", "label": "Test?", "kind": "decision", "states": ["test", "no test"]},
            {"id": "oil", "label": "Oil", "kind": "chance", "states": ["dry", "wet", "soaking"]},
            {"id": "seismic", "label": "Seismic", "kind": "chance", "states": ["closed", "open", "diffuse", "none"]},
            {"id": "drill", "label": "Drill?", "kind": "decision", "states": ["drill", "don't drill"]},
            {"id": "payoff", "label": "Payoff", "kind": "utility"},
            {"id": "cost", "label": "Test cost", "kind": "utility"},
        ]));
        let p = |d: &str, ps: J, o: &str, eu: f64, c: bool| json!({"decision": d, "parent_states": ps, "option": o, "expected_utility": eu, "chosen": c});
        let options = json!({
            "arcs": [
                {"from": "oil", "to": "seismic"}, {"from": "test", "to": "seismic"},
                {"from": "test", "to": "drill", "strength": 0.2}, {"from": "seismic", "to": "drill"},
                {"from": "oil", "to": "payoff"}, {"from": "drill", "to": "payoff"},
                {"from": "test", "to": "cost"},
            ],
            "beliefs": [{"node": "payoff", "expected_utility": 22.5}],
            "evidence": [{"node": "test", "state": "test"}, {"node": "seismic", "state": "open"}],
            "policy": [
                p("test", json!([]), "test", 12.5, false), p("test", json!([]), "no test", 20.0, true),
                p("drill", json!(["no test", "none"]), "drill", 20.0, true),
                p("drill", json!(["no test", "none"]), "don't drill", 0.0, false),
                p("drill", json!(["test", "closed"]), "drill", 77.5, true),
                p("drill", json!(["test", "closed"]), "don't drill", -10.0, false),
                p("drill", json!(["test", "open"]), "drill", 12.9, true),
                p("drill", json!(["test", "open"]), "don't drill", -10.0, false),
            ],
        })
        .as_object()
        .unwrap()
        .clone();
        (nodes, options)
    }

    #[test]
    fn a_policy_goes_into_the_decisions_card() {
        let (nodes, options) = wildcatter();
        let cols = columns(&nodes, &options).unwrap();
        let card = |i: usize| -> J {
            serde_json::from_str(col(&cols, "BELIEF")[i].as_str().unwrap()).unwrap()
        };
        // no informational parents: one row, no current one
        assert_eq!(
            card(0)["policy"],
            json!({"parents": [], "options": ["test", "no test"],
                   "rows": [{"parents": [], "eu": [12.5, 20.0], "chosen": 1}], "total": 1})
        );
        // the drill's parents in arc order; the evidence picks [test, open]
        let drill = card(3)["policy"].clone();
        assert_eq!(drill["parents"], json!(["Test?", "Seismic"]));
        assert_eq!(
            drill["rows"][1],
            json!({"parents": ["test", "closed"], "eu": [77.5, -10.0], "chosen": 0})
        );
        assert_eq!(drill["current"], 2);
        // a chance node has none
        assert!(card(1).get("policy").is_none());
        // information arcs dotted (over the strength's dash), the rest not
        let arcs = col(&cols, "EDGES")[0].as_str().unwrap();
        assert!(
            arcs.contains(r#"{"from":"test","to":"drill","value":0.2,"width":1.5,"style":"dotted","tip":"strength 0.2, known when deciding Drill?"}"#),
            "{arcs}"
        );
        assert!(arcs.contains(r#"{"from":"seismic","to":"drill","style":"dotted","tip":"known when deciding Drill?"}"#), "{arcs}");
        assert!(arcs.contains(r#"{"from":"oil","to":"seismic"}"#), "{arcs}");
        // the payoff's expected utility in its diamond; the test cost has none
        assert_eq!(
            col(&cols, "METRIC"),
            [J::Null, J::Null, J::Null, J::Null, json!(22.5), J::Null]
        );
        assert!(
            lint(&nodes, &options).is_empty(),
            "{:?}",
            lint(&nodes, &options)
        );
    }

    /// Without `chosen` the best option is the policy's; partial evidence
    /// picks no configuration.
    #[test]
    fn chosen_defaults_to_the_best_and_partial_evidence_picks_none() {
        let (nodes, mut options) = wildcatter();
        options.insert(
            "policy".into(),
            json!([{"decision": "drill", "parent_states": ["test", "diffuse"], "option": "drill", "expected_utility": -25.6},
                   {"decision": "drill", "parent_states": ["test", "diffuse"], "option": "don't drill", "expected_utility": -10}]),
        );
        options.insert(
            "evidence".into(),
            json!([{"node": "test", "state": "test"}]),
        );
        let cols = columns(&nodes, &options).unwrap();
        let drill: J = serde_json::from_str(col(&cols, "BELIEF")[3].as_str().unwrap()).unwrap();
        assert_eq!(drill["policy"]["rows"][0]["chosen"], 1);
        assert!(drill["policy"].get("current").is_none());
    }

    #[test]
    fn lints_the_policy() {
        let (nodes, mut options) = wildcatter();
        options.insert(
            "policy".into(),
            json!([
                {"decision": "oil", "parent_states": [], "option": "dry", "expected_utility": 1},
                {"decision": "ghost", "parent_states": [], "option": "x", "expected_utility": 1},
                {"decision": "test", "parent_states": [], "option": "maybe", "expected_utility": 1},
                {"decision": "drill", "parent_states": ["closed"], "option": "drill", "expected_utility": 5, "chosen": true},
                {"decision": "drill", "parent_states": ["closed"], "option": "don't drill", "expected_utility": 9, "chosen": true},
                {"decision": "drill", "parent_states": ["test", "open"], "option": "drill", "expected_utility": 1, "chosen": 1},
                {"decision": "drill", "parent_states": ["test", "open"], "option": "don't drill", "expected_utility": 2, "chosen": 0},
            ]),
        );
        let l = lint(&nodes, &options);
        assert_eq!(
            l,
            [
                "bayes_net/unknown-id: arcs, beliefs, evidence, tables or paths name ghost that are not nodes",
                "bayes_net/policy-not-decision: the policy names oil that are not decision nodes",
                "bayes_net/policy-option: test: maybe not among the decision's states",
                "bayes_net/policy-parents: parent_states of drill (2) do not match the number of arcs into the decision",
                "bayes_net/policy-chosen: more than one chosen option for drill [closed]",
                "bayes_net/policy-not-best: the chosen option of drill [test, open] has a lower expected utility than another",
            ],
            "{l:?}"
        );
    }
}
