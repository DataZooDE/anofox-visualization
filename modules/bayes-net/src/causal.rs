//! Causal views of a Bayesian network (#1278), on the network's drawing:
//!
//! - **adjustment sets** (`adjust := ` `treatment, outcome, set_id, node,
//!   kind`; kind `minimal` | `optimal` | `canonical`, or `front-door` for a
//!   front-door mediator): the treatment and the outcome are marked X and Y,
//!   the adjustment set's nodes are ringed, front-door mediators marked M;
//!   the back-door paths from the treatment to the outcome are drawn dashed
//!   — open ones red, the ones the set blocks with a bar where they are
//!   blocked — and the causal paths solid;
//! - **the counterfactual twin network** (beliefs with a `world` column,
//!   `factual` | `counterfactual`): the factual and the counterfactual world
//!   side by side as lanes, the nodes the change does not reach (not
//!   downstream of a `do`) shared between them, the changed node set by
//!   `do` in the counterfactual world (its copy's id is the node's id with
//!   [`CF`] after it);
//! - **the structure-learning diff** (`reference := ` `from, to`, the
//!   reference or expert network): the learned arcs (strength at or above
//!   `threshold`) coloured as matching, extra or reversed, the reference's
//!   arcs that were not learned drawn dashed as missing, a legend with the
//!   counts and the structural Hamming distance.
//!
//! [`twin`] rewrites the tables before the drawing ([`crate::bayes_net`]);
//! [`decorate`] marks its columns after it; [`lint`] checks the tables.

use crate::bayes_net::{get, list, num};
use crate::Row;
use anofox_viz_plugin::OutColumn;
use serde_json::{json, Map, Value as J};

/// The counterfactual copy's id: the node's id with this after it.
pub(crate) const CF: &str = "*";

/// The lanes of the twin network.
const FACTUAL: &str = "Factual world";
const SHARED: &str = "Shared by both worlds";
const COUNTERFACTUAL: &str = "Counterfactual world";

/// The adjustment kinds; front-door mediators are a kind of their own.
const ADJUST_KINDS: &[&str] = &["minimal", "optimal", "canonical"];
const FRONT_DOOR: &[&str] = &["front-door", "frontdoor", "front_door", "mediator"];

/// At most this many paths between the treatment and the outcome are
/// followed.
const MAX_TRAILS: usize = 2000;

fn lower(s: Option<String>) -> Option<String> {
    s.map(|s| s.to_ascii_lowercase())
}

/// The drawn arcs `(from, to)`: forbidden ones left out.
fn arcs_of(options: &Row) -> Result<Vec<(String, String)>, String> {
    Ok(list(options, "arcs")?
        .iter()
        .filter(|a| lower(get(a, &["status"])).as_deref() != Some("forbidden"))
        .filter_map(|a| Some((get(a, &["from"])?, get(a, &["to"])?)))
        .collect())
}

fn node_ids(rows: &[Row]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for r in rows {
        if let Some(id) = get(r, &["id"]) {
            if !out.contains(&id) {
                out.push(id);
            }
        }
    }
    out
}

/// `from` and every node downstream of it.
fn downstream(from: &[String], arcs: &[(String, String)]) -> Vec<String> {
    let mut out: Vec<String> = from.to_vec();
    let mut k = 0;
    while k < out.len() {
        let u = out[k].clone();
        for (f, t) in arcs {
            if *f == u && !out.contains(t) {
                out.push(t.clone());
            }
        }
        k += 1;
    }
    out
}

/// A node id without the counterfactual mark.
fn base(id: &str) -> &str {
    id.strip_suffix(CF).unwrap_or(id)
}

fn world(r: &Row) -> Option<String> {
    lower(get(r, &["world"]))
}

/// Whether the beliefs come in two worlds (a `world` column).
pub(crate) fn is_twin(options: &Row) -> bool {
    list(options, "beliefs")
        .map(|b| b.iter().any(|r| world(r).is_some()))
        .unwrap_or(false)
}

/// The nodes the counterfactual change reaches: the nodes set by `do`
/// (in either world) and everything downstream of them.
fn changed(options: &Row, ids: &[String]) -> Result<(Vec<String>, Vec<String>), String> {
    let arcs = arcs_of(options)?;
    let set: Vec<String> = list(options, "evidence")?
        .iter()
        .filter(|e| {
            lower(get(e, &["mode"])).as_deref() == Some("do")
                || world(e).as_deref() == Some("counterfactual")
        })
        .filter_map(|e| get(e, &["node"]))
        .map(|n| base(&n).to_string())
        .filter(|n| ids.contains(n))
        .fold(Vec::new(), |mut acc, n| {
            if !acc.contains(&n) {
                acc.push(n);
            }
            acc
        });
    let reach = downstream(&set, &arcs);
    Ok((set, reach))
}

/// The twin network (#1278) when the beliefs have a `world` column: the
/// nodes the change reaches twice — the factual copy (its id) in the
/// factual lane, the counterfactual copy (id + [`CF`]) in the
/// counterfactual lane — and the rest once, shared, in a lane between them.
/// The arcs, beliefs, evidence and tables follow: a `do` (or a row of the
/// counterfactual world) goes to the counterfactual copy, the rest to the
/// factual one; a shared node takes the factual belief. `None` without a
/// `world` column.
pub(crate) fn twin(rows: &[Row], options: &Row) -> Result<Option<(Vec<Row>, Row)>, String> {
    if !is_twin(options) {
        return Ok(None);
    }
    let ids = node_ids(rows);
    let (set, split) = changed(options, &ids)?;
    let cf = |id: &str| format!("{id}{CF}");
    let label_of = |r: &Row, id: &str| get(r, &["label", "name"]).unwrap_or_else(|| id.to_string());
    let changes: Vec<String> = list(options, "evidence")?
        .iter()
        .filter(|e| lower(get(e, &["mode"])).as_deref() == Some("do"))
        .filter_map(|e| {
            let n = get(e, &["node"])?;
            let r = rows
                .iter()
                .find(|r| get(r, &["id"]).as_deref() == Some(base(&n)))?;
            Some(format!(
                "{} set to {}",
                label_of(r, base(&n)),
                get(e, &["state"])?
            ))
        })
        .collect();
    let what_if = if changes.is_empty() {
        String::new()
    } else {
        format!(" had {}", changes.join(" and "))
    };
    // the nodes: factual copies, then the shared ones, then the
    // counterfactual copies (the lanes in that order, left to right)
    let mut factual = Vec::new();
    let mut shared = Vec::new();
    let mut counter = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for r in rows {
        let Some(id) = get(r, &["id"]) else { continue };
        if seen.contains(&id) {
            continue;
        }
        seen.push(id.clone());
        let mut f = r.clone();
        if split.contains(&id) {
            f.insert("lane".into(), json!(FACTUAL));
            f.insert("world".into(), json!("factual"));
            let mut c = r.clone();
            c.insert("id".into(), json!(cf(&id)));
            c.insert("lane".into(), json!(COUNTERFACTUAL));
            c.insert("world".into(), json!("counterfactual"));
            c.insert(
                "note".into(),
                json!(if set.contains(&id) {
                    format!("Counterfactual world: the changed node{what_if}.")
                } else {
                    format!("Counterfactual world: what it would have been{what_if}, the background (the shared nodes) as it was.")
                }),
            );
            f.insert(
                "note".into(),
                json!("Factual world: what happened, given the evidence."),
            );
            factual.push(f);
            counter.push(c);
        } else {
            f.insert("lane".into(), json!(SHARED));
            f.insert("world".into(), json!("shared"));
            f.insert(
                "note".into(),
                json!("Shared by both worlds: the change does not reach it, so it is the same in both."),
            );
            shared.push(f);
        }
    }
    let nodes: Vec<Row> = factual.into_iter().chain(shared).chain(counter).collect();

    let mut out = options.clone();
    // arcs: into a shared node once; into a split node in both worlds, the
    // counterfactual one from the parent's copy if it has one
    let mut arcs: Vec<J> = Vec::new();
    for a in list(options, "arcs")? {
        let (Some(f), Some(t)) = (get(&a, &["from"]), get(&a, &["to"])) else {
            continue;
        };
        arcs.push(J::Object(a.clone()));
        if split.contains(&t) {
            let mut c = a.clone();
            let from = if split.contains(&f) {
                cf(&f)
            } else {
                f.clone()
            };
            c.insert("from".into(), json!(from));
            c.insert("to".into(), json!(cf(&t)));
            arcs.push(J::Object(c));
        }
    }
    out.insert("arcs".into(), J::Array(arcs));
    // beliefs: the counterfactual world's to the copies, the factual
    // world's to the factual copies and the shared nodes
    let mut beliefs: Vec<J> = Vec::new();
    for b in list(options, "beliefs")? {
        let Some(n) = get(&b, &["node"]) else {
            continue;
        };
        let n = base(&n).to_string();
        let mut b2 = b.clone();
        match world(&b).as_deref() {
            Some("counterfactual") => {
                if !split.contains(&n) {
                    continue;
                }
                b2.insert("node".into(), json!(cf(&n)));
            }
            _ => {
                b2.insert("node".into(), json!(n));
            }
        }
        b2.remove("world");
        beliefs.push(J::Object(b2));
    }
    out.insert("beliefs".into(), J::Array(beliefs));
    // evidence: a do (or the counterfactual world's) to the copy
    let mut evidence: Vec<J> = Vec::new();
    for e in list(options, "evidence")? {
        let Some(n) = get(&e, &["node"]) else {
            continue;
        };
        let to_cf = n.ends_with(CF)
            || lower(get(&e, &["mode"])).as_deref() == Some("do")
            || world(&e).as_deref() == Some("counterfactual");
        let n = base(&n).to_string();
        let mut e2 = e.clone();
        e2.insert(
            "node".into(),
            json!(if to_cf && split.contains(&n) {
                cf(&n)
            } else {
                n
            }),
        );
        e2.remove("world");
        evidence.push(J::Object(e2));
    }
    out.insert("evidence".into(), J::Array(evidence));
    // tables: the copy has its node's table
    let mut tables: Vec<J> = Vec::new();
    for t in list(options, "tables")? {
        let Some(n) = get(&t, &["node"]) else {
            continue;
        };
        tables.push(J::Object(t.clone()));
        if split.contains(&n) {
            let mut c = t.clone();
            c.insert("node".into(), json!(cf(&n)));
            tables.push(J::Object(c));
        }
    }
    out.insert("tables".into(), J::Array(tables));
    // the active paths are one world's: not drawn on the twin
    out.remove("paths");
    Ok(Some((nodes, out)))
}

/// A step of a trail: the node it reaches and whether the arc points along
/// the trail (`from` → `to`).
type Trail = Vec<(String, bool)>;

/// Every trail (a path along the arcs, either way, no node twice) from
/// `a` to `b`, at most [`MAX_TRAILS`].
fn trails(a: &str, b: &str, arcs: &[(String, String)]) -> Vec<Trail> {
    let mut out: Vec<Trail> = Vec::new();
    let mut stack: Vec<(String, Trail)> = vec![(a.to_string(), Vec::new())];
    while let Some((u, path)) = stack.pop() {
        if out.len() >= MAX_TRAILS {
            break;
        }
        if u == b {
            out.push(path);
            continue;
        }
        let visited = |n: &str| n == a || path.iter().any(|(m, _)| m == n);
        for (f, t) in arcs.iter().rev() {
            let step = if *f == u {
                Some((t.clone(), true))
            } else if *t == u {
                Some((f.clone(), false))
            } else {
                None
            };
            if let Some((v, fwd)) = step {
                if !visited(&v) {
                    let mut p = path.clone();
                    p.push((v.clone(), fwd));
                    stack.push((v, p));
                }
            }
        }
    }
    // the short ones first
    out.sort_by_key(|p| p.len());
    out
}

/// Where a trail is blocked given the set `z`: the index (into the trail's
/// steps) of the first node that blocks it — a chain or fork node in `z`, a
/// collider with neither it nor anything downstream of it in `z`.
fn blocked_at(trail: &Trail, z: &[String], arcs: &[(String, String)]) -> Option<usize> {
    (0..trail.len().saturating_sub(1)).find(|&i| {
        let (node, into) = (&trail[i].0, trail[i].1);
        let out_back = !trail[i + 1].1;
        let collider = into && out_back;
        if collider {
            !downstream(std::slice::from_ref(node), arcs)
                .iter()
                .any(|d| z.contains(d))
        } else {
            z.contains(node)
        }
    })
}

/// The adjustment view the `adjust :=` rows ask for.
struct Adjust {
    treatment: String,
    outcome: String,
    set_id: Option<String>,
    kind: Option<String>,
    set: Vec<String>,
    mediators: Vec<String>,
}

fn adjust(options: &Row) -> Result<Option<Adjust>, String> {
    let rows = list(options, "adjust")?;
    let Some((t, y)) = rows
        .iter()
        .find_map(|r| Some((get(r, &["treatment", "exposure"])?, get(r, &["outcome"])?)))
    else {
        return Ok(None);
    };
    let pair: Vec<&Row> = rows
        .iter()
        .filter(|r| {
            get(r, &["treatment", "exposure"]).as_deref() == Some(t.as_str())
                && get(r, &["outcome"]).as_deref() == Some(y.as_str())
        })
        .collect();
    let front =
        |r: &Row| lower(get(r, &["kind"])).is_some_and(|k| FRONT_DOOR.contains(&k.as_str()));
    // the first set listed (filter the rows in SQL to pick another)
    let set_id = pair
        .iter()
        .filter(|r| !front(r))
        .find_map(|r| get(r, &["set_id", "set"]));
    let in_set = |r: &&&Row| !front(r) && get(r, &["set_id", "set"]) == set_id;
    let kind = pair
        .iter()
        .filter(in_set)
        .find_map(|r| lower(get(r, &["kind"])));
    let mut set: Vec<String> = Vec::new();
    for r in pair.iter().filter(in_set) {
        if let Some(n) = get(r, &["node"]) {
            if !set.contains(&n) {
                set.push(n);
            }
        }
    }
    let mut mediators: Vec<String> = Vec::new();
    for r in pair.iter().filter(|r| front(r)) {
        if let Some(n) = get(r, &["node"]) {
            if !mediators.contains(&n) {
                mediators.push(n);
            }
        }
    }
    Ok(Some(Adjust {
        treatment: t,
        outcome: y,
        set_id,
        kind,
        set,
        mediators,
    }))
}

/// A trail as text: "Discount ← Engaged → Stays".
fn trail_text(start: &str, trail: &Trail, label: &dyn Fn(&str) -> String) -> String {
    let mut s = label(start);
    for (n, fwd) in trail {
        s.push_str(if *fwd { " → " } else { " ← " });
        s.push_str(&label(n));
    }
    s
}

/// The back-door paths from `t` to `y` (their first arc points into `t`)
/// and the causal ones (every arc along the path).
fn paths_between(t: &str, y: &str, arcs: &[(String, String)]) -> (Vec<Trail>, Vec<Trail>) {
    let all = trails(t, y, arcs);
    let back = all
        .iter()
        .filter(|p| p.first().is_some_and(|s| !s.1))
        .cloned()
        .collect();
    let causal = all
        .iter()
        .filter(|p| p.iter().all(|s| s.1))
        .cloned()
        .collect();
    (back, causal)
}

/// An edge item's (from, to).
fn ends(e: &Map<String, J>) -> (String, String) {
    let s = |k: &str| e.get(k).and_then(J::as_str).unwrap_or("").to_string();
    (s("from"), s("to"))
}

fn add_tip(e: &mut Map<String, J>, more: &str) {
    let tip = match e.get("tip").and_then(J::as_str) {
        Some(t) if !t.is_empty() => format!("{t}; {more}"),
        _ => more.to_string(),
    };
    e.insert("tip".into(), J::String(tip));
}

/// The node cards (`::BELIEF`) and arcs (`::EDGES`) of the drawing, to
/// mark them.
struct Drawn<'a> {
    cols: &'a mut Vec<OutColumn>,
}

impl Drawn<'_> {
    fn values(&mut self, role: &str) -> Option<&mut Vec<J>> {
        self.cols
            .iter_mut()
            .find(|c| c.role == role)
            .map(|c| &mut c.values)
    }

    fn ids(&self) -> Vec<String> {
        self.cols
            .iter()
            .find(|c| c.role == "DIAGRAM")
            .map(|c| {
                c.values
                    .iter()
                    .map(|v| v.as_str().unwrap_or("").to_string())
                    .collect()
            })
            .unwrap_or_default()
    }

    fn labels(&self) -> Vec<String> {
        self.cols
            .iter()
            .find(|c| c.role == "LABEL")
            .map(|c| {
                c.values
                    .iter()
                    .map(|v| v.as_str().unwrap_or("").to_string())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Add `key: value` to node `id`'s card (a card is made if it has
    /// none).
    fn card(&mut self, id: &str, key: &str, value: J) {
        let Some(k) = self.ids().iter().position(|i| i == id) else {
            return;
        };
        let n = self.ids().len();
        if self.values("BELIEF").is_none() {
            self.cols.push(OutColumn {
                name: "belief".into(),
                role: "BELIEF".into(),
                values: vec![J::Null; n],
            });
        }
        let Some(cards) = self.values("BELIEF") else {
            return;
        };
        let mut card: Map<String, J> = cards[k]
            .as_str()
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or_default();
        if !card.contains_key("states") {
            // a node without states (a utility) keeps no card: the host
            // reads only cards with states
            return;
        }
        // a note adds to the one before
        let value = match (key, card.get(key).and_then(J::as_str), value.as_str()) {
            ("note", Some(old), Some(new)) => J::String(format!("{old} {new}")),
            _ => value,
        };
        card.insert(key.into(), value);
        cards[k] = J::String(J::Object(card).to_string());
    }

    fn edges(&mut self) -> Vec<Map<String, J>> {
        self.values("EDGES")
            .and_then(|v| v.first())
            .and_then(J::as_str)
            .and_then(|s| serde_json::from_str::<Vec<Map<String, J>>>(s).ok())
            .unwrap_or_default()
    }

    fn set_edges(&mut self, edges: Vec<Map<String, J>>) {
        let text = J::String(J::Array(edges.into_iter().map(J::Object).collect()).to_string());
        let n = self.ids().len();
        match self.values("EDGES") {
            Some(v) => {
                if let Some(f) = v.first_mut() {
                    *f = text;
                }
            }
            None => {
                let mut values = vec![J::Null; n.max(1)];
                values[0] = text;
                self.cols.push(OutColumn {
                    name: "arcs".into(),
                    role: "EDGES".into(),
                    values,
                });
            }
        }
    }
}

/// Mark the drawing's columns with the causal views the options ask for:
/// the adjustment set, the twin network's notes, the structure diff.
pub(crate) fn decorate(
    cols: &mut Vec<OutColumn>,
    rows: &[Row],
    options: &Row,
) -> Result<(), String> {
    let mut d = Drawn { cols };
    // the twin network's notes (its rows carry them)
    for r in rows {
        if let (Some(id), Some(note)) = (get(r, &["id"]), get(r, &["note"])) {
            d.card(&id, "note", J::String(note));
        }
        if let (Some(id), Some(w)) = (get(r, &["id"]), get(r, &["world"])) {
            d.card(&id, "world", J::String(w));
        }
    }
    if let Some(a) = adjust(options)? {
        mark_adjustment(&mut d, &a, options)?;
    }
    if options.get("reference").is_some_and(|r| !r.is_null()) {
        mark_diff(&mut d, options)?;
    }
    Ok(())
}

fn mark_adjustment(d: &mut Drawn, a: &Adjust, options: &Row) -> Result<(), String> {
    let ids = d.ids();
    let labels = d.labels();
    let label = |id: &str| {
        ids.iter()
            .position(|i| i == id)
            .and_then(|k| labels.get(k).cloned())
            .unwrap_or_else(|| id.to_string())
    };
    if !ids.contains(&a.treatment) || !ids.contains(&a.outcome) {
        return Ok(());
    }
    let arcs = arcs_of(options)?;
    let (back, causal) = paths_between(&a.treatment, &a.outcome, &arcs);
    let (t, y) = (label(&a.treatment), label(&a.outcome));
    let which = match (&a.kind, &a.set_id) {
        (Some(k), _) => format!("the {k} adjustment set"),
        (None, Some(s)) => format!("adjustment set {s}"),
        (None, None) => "the adjustment set".into(),
    };
    let names = a
        .set
        .iter()
        .map(|n| label(n))
        .collect::<Vec<_>>()
        .join(", ");
    d.card(&a.treatment, "causal", json!("treatment"));
    d.card(
        &a.treatment,
        "note",
        json!(format!(
            "Treatment: what is done — the effect of {t} on {y} is the question."
        )),
    );
    d.card(&a.outcome, "causal", json!("outcome"));
    d.card(
        &a.outcome,
        "note",
        json!(format!("Outcome: what {t} is meant to change.")),
    );
    for n in &a.set {
        d.card(n, "causal", json!("adjust"));
        d.card(
            n,
            "note",
            json!(format!(
                "In {which} ({names}): comparing like with like on it blocks the back-door paths it lies on."
            )),
        );
    }
    for n in &a.mediators {
        d.card(n, "causal", json!("mediator"));
        d.card(
            n,
            "note",
            json!(format!("Front-door mediator: the effect of {t} on {y} runs through it, so it can be measured even with the confounder unseen.")),
        );
    }
    // the arcs: on a causal path, on an open or a blocked back-door path
    let mut edges = d.edges();
    let on = |trail_start: &str, trail: &Trail| -> Vec<(String, String)> {
        let mut out = Vec::new();
        let mut u = trail_start.to_string();
        for (v, fwd) in trail {
            out.push(if *fwd {
                (u.clone(), v.clone())
            } else {
                (v.clone(), u.clone())
            });
            u = v.clone();
        }
        out
    };
    // per arc: (causal, open back-door text, blocked back-door text, a bar)
    type Mark = ((String, String), bool, Option<String>, Option<String>, bool);
    let mut marks: Vec<Mark> = Vec::new();
    fn slot(marks: &mut Vec<Mark>, arc: (String, String)) -> usize {
        match marks.iter().position(|m| m.0 == arc) {
            Some(i) => i,
            None => {
                marks.push((arc, false, None, None, false));
                marks.len() - 1
            }
        }
    }
    for p in &causal {
        for arc in on(&a.treatment, p) {
            let i = slot(&mut marks, arc);
            marks[i].1 = true;
        }
    }
    for p in &back {
        let text = trail_text(&a.treatment, p, &label);
        let steps = on(&a.treatment, p);
        match blocked_at(p, &a.set, &arcs) {
            None => {
                for arc in steps {
                    let i = slot(&mut marks, arc);
                    marks[i]
                        .2
                        .get_or_insert_with(|| format!("back-door path {text}: open"));
                }
            }
            Some(at) => {
                let by = label(&p[at].0);
                for (k, arc) in steps.into_iter().enumerate() {
                    let i = slot(&mut marks, arc);
                    marks[i]
                        .3
                        .get_or_insert_with(|| format!("back-door path {text}: blocked at {by}"));
                    // the bar on the arc that leaves the blocking node
                    if k == at + 1 {
                        marks[i].4 = true;
                    }
                }
            }
        }
    }
    for e in edges.iter_mut() {
        let arc = ends(e);
        let Some(m) = marks.iter().find(|m| m.0 == arc) else {
            continue;
        };
        if let Some(text) = &m.2 {
            e.insert("style".into(), json!("dashed"));
            e.insert("color".into(), json!("bad"));
            e.insert("key".into(), json!("back-door path, open"));
            add_tip(e, text);
        } else if m.1 {
            // a causal arc stays solid even where a blocked back-door
            // path runs along it too
            e.remove("style");
            e.insert("color".into(), json!("accent"));
            e.insert("key".into(), json!("causal path"));
            add_tip(e, &format!("on a causal path from {t} to {y}"));
            if let Some(text) = &m.3 {
                add_tip(e, text);
            }
        } else if let Some(text) = &m.3 {
            e.insert("style".into(), json!("dashed"));
            e.insert("key".into(), json!("back-door path, blocked"));
            if m.4 {
                e.insert("blocked".into(), J::Bool(true));
            }
            add_tip(e, text);
        }
    }
    d.set_edges(edges);
    Ok(())
}

/// The structure diff's kinds of arc, in legend order, with their colour.
const DIFF: &[(&str, &str)] = &[
    ("matching", "neutral"),
    ("extra", "warn"),
    ("missing", "bad"),
    ("reversed", "accent"),
];

/// The learned arcs against the reference: (from, to, kind).
fn diff(options: &Row) -> Result<Vec<(String, String, &'static str)>, String> {
    let threshold = num(options, &["threshold"]).unwrap_or(0.5);
    let learned: Vec<(String, String)> = list(options, "arcs")?
        .iter()
        .filter(|a| lower(get(a, &["status"])).as_deref() != Some("forbidden"))
        // a required arc is in the graph whatever its strength, as it is
        // drawn (#1336)
        .filter(|a| {
            lower(get(a, &["status"])).as_deref() == Some("required")
                || num(a, &["strength"]).is_none_or(|s| s >= threshold)
        })
        .filter_map(|a| Some((get(a, &["from"])?, get(a, &["to"])?)))
        .collect();
    let reference: Vec<(String, String)> = list(options, "reference")?
        .iter()
        .filter_map(|a| Some((get(a, &["from"])?, get(a, &["to"])?)))
        .collect();
    let mut out = Vec::new();
    for (f, t) in &learned {
        let kind = if reference.contains(&(f.clone(), t.clone())) {
            "matching"
        } else if reference.contains(&(t.clone(), f.clone())) {
            "reversed"
        } else {
            "extra"
        };
        out.push((f.clone(), t.clone(), kind));
    }
    for (f, t) in &reference {
        if !learned.contains(&(f.clone(), t.clone())) && !learned.contains(&(t.clone(), f.clone()))
        {
            out.push((f.clone(), t.clone(), "missing"));
        }
    }
    Ok(out)
}

/// The structural Hamming distance: extra, missing and reversed arcs.
fn shd(d: &[(String, String, &str)]) -> usize {
    d.iter().filter(|x| x.2 != "matching").count()
}

fn mark_diff(d: &mut Drawn, options: &Row) -> Result<(), String> {
    let diff = diff(options)?;
    let ids = d.ids();
    let edges = d.edges();
    let count = |k: &str| diff.iter().filter(|x| x.2 == k).count();
    let key = |k: &str| {
        let n = count(k);
        format!("{k} ({n})")
    };
    let mut out: Vec<Map<String, J>> = Vec::new();
    for (f, t, kind) in &diff {
        if !ids.contains(f) || !ids.contains(t) {
            continue;
        }
        let mut e = edges
            .iter()
            .find(|e| ends(e) == (f.clone(), t.clone()))
            .cloned()
            .unwrap_or_else(|| {
                let mut m = Map::new();
                m.insert("from".into(), json!(f));
                m.insert("to".into(), json!(t));
                m
            });
        e.remove("style");
        let colour = DIFF
            .iter()
            .find(|x| x.0 == *kind)
            .map_or("neutral", |x| x.1);
        e.insert("color".into(), json!(colour));
        e.insert("key".into(), json!(key(kind)));
        match *kind {
            "missing" => {
                e.insert("style".into(), json!("dashed"));
                e.insert("width".into(), json!(1.6));
                add_tip(&mut e, "missing: in the reference, not learned");
            }
            "reversed" => add_tip(&mut e, "reversed: the reference has it the other way round"),
            "extra" => add_tip(&mut e, "extra: learned, not in the reference"),
            _ => add_tip(&mut e, "matching the reference"),
        }
        out.push(e);
    }
    // after the legend: the distance
    if let Some(first) = out.first_mut() {
        first.insert(
            "caption".into(),
            json!(format!("structural Hamming distance {}", shd(&diff))),
        );
    }
    d.set_edges(out);
    Ok(())
}

/// The structural Hamming distance of the learned arcs against
/// `reference :=` (#1278), for the root's `data-bn-shd`.
pub(crate) fn root_attrs(options: &Row) -> Result<Vec<(String, String)>, String> {
    if options.get("reference").is_none_or(J::is_null) {
        return Ok(Vec::new());
    }
    Ok(vec![(
        "data-bn-shd".into(),
        shd(&diff(options)?).to_string(),
    )])
}

/// What the lint reports about the causal views (`bayes_net/*`).
pub(crate) fn lint(rows: &[Row], options: &Row) -> Vec<String> {
    problems(rows, options).unwrap_or_else(|e| vec![e])
}

fn problems(rows: &[Row], options: &Row) -> Result<Vec<String>, String> {
    let ids = node_ids(rows);
    let mut out = Vec::new();
    let mut unknown: Vec<String> = Vec::new();
    let mut note = |id: String| {
        if !ids.contains(&id) && !unknown.contains(&id) {
            unknown.push(id);
        }
    };
    let adjust_rows = list(options, "adjust")?;
    for r in &adjust_rows {
        for id in [
            get(r, &["treatment", "exposure"]),
            get(r, &["outcome"]),
            get(r, &["node"]),
        ]
        .into_iter()
        .flatten()
        {
            note(id);
        }
    }
    for r in list(options, "reference")? {
        for id in [get(&r, &["from"]), get(&r, &["to"])].into_iter().flatten() {
            note(id);
        }
    }
    if !unknown.is_empty() {
        out.push(format!(
            "bayes_net/unknown-id: adjust or reference name {} that are not nodes",
            unknown.join(", ")
        ));
    }
    let kinds: Vec<String> = adjust_rows
        .iter()
        .filter_map(|r| lower(get(r, &["kind"])))
        .filter(|k| !ADJUST_KINDS.contains(&k.as_str()) && !FRONT_DOOR.contains(&k.as_str()))
        .fold(Vec::new(), |mut acc, k| {
            if !acc.contains(&k) {
                acc.push(k);
            }
            acc
        });
    if !kinds.is_empty() {
        out.push(format!(
            "bayes_net/bad-adjust-kind: adjustment kind {} is not minimal, optimal, canonical or front-door",
            kinds.join(", ")
        ));
    }
    // each adjustment set: it blocks every back-door path and holds no
    // node downstream of the treatment
    let arcs = arcs_of(options)?;
    let label = |id: &str| {
        rows.iter()
            .find(|r| get(r, &["id"]).as_deref() == Some(id))
            .and_then(|r| get(r, &["label", "name"]))
            .unwrap_or_else(|| id.to_string())
    };
    let mut sets: Vec<(String, String, String, Vec<String>)> = Vec::new();
    for r in &adjust_rows {
        if lower(get(r, &["kind"])).is_some_and(|k| FRONT_DOOR.contains(&k.as_str())) {
            continue;
        }
        let (Some(t), Some(y)) = (get(r, &["treatment", "exposure"]), get(r, &["outcome"])) else {
            continue;
        };
        let s = get(r, &["set_id", "set"]).unwrap_or_default();
        let n = get(r, &["node"]);
        match sets.iter_mut().find(|x| x.0 == t && x.1 == y && x.2 == s) {
            Some(x) => x.3.extend(n),
            None => sets.push((t, y, s, n.into_iter().collect())),
        }
    }
    for (t, y, s, z) in &sets {
        // a set without nodes compares without adjusting, on purpose
        if !ids.contains(t) || !ids.contains(y) || z.is_empty() {
            continue;
        }
        let below = downstream(std::slice::from_ref(t), &arcs);
        let bad: Vec<String> = z
            .iter()
            .filter(|n| below.contains(n))
            .map(|n| label(n))
            .collect();
        if !bad.is_empty() {
            out.push(format!(
                "bayes_net/adjust-descendant: adjustment set {s} holds {}, downstream of {} — adjusting for it biases the effect",
                bad.join(", "),
                label(t)
            ));
        }
        let (back, _) = paths_between(t, y, &arcs);
        let open: Vec<String> = back
            .iter()
            .filter(|p| blocked_at(p, z, &arcs).is_none())
            .map(|p| trail_text(t, p, &label))
            .collect();
        if !open.is_empty() {
            out.push(format!(
                "bayes_net/adjust-open: adjustment set {s} leaves the back-door path {} open",
                open.join(", ")
            ));
        }
    }
    // the twin network: a world that is neither
    let worlds: Vec<String> = list(options, "beliefs")?
        .iter()
        .chain(list(options, "evidence")?.iter())
        .filter_map(world)
        .filter(|w| w != "factual" && w != "counterfactual")
        .fold(Vec::new(), |mut acc, w| {
            if !acc.contains(&w) {
                acc.push(w);
            }
            acc
        });
    if !worlds.is_empty() {
        out.push(format!(
            "bayes_net/bad-world: world {} is not factual or counterfactual — read as factual",
            worlds.join(", ")
        ));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bayes_net::columns;

    fn opts(v: J) -> Row {
        v.as_object().unwrap().clone()
    }

    fn node(id: &str, label: &str) -> Row {
        opts(json!({"id": id, "label": label, "kind": "chance", "states": ["yes", "no"]}))
    }

    /// The discount network: engaged → discount, engaged → stay,
    /// discount → usage → stay, discount → stay, price → stay.
    fn discount() -> (Vec<Row>, Row) {
        let nodes = vec![
            node("engaged", "Engaged"),
            node("price", "Price rose"),
            node("discount", "Discount"),
            node("usage", "Weekly use"),
            node("stay", "Renewed"),
        ];
        let arc = |f: &str, t: &str| json!({"from": f, "to": t});
        let options = opts(json!({"arcs": [
            arc("engaged", "discount"), arc("discount", "usage"), arc("engaged", "usage"),
            arc("engaged", "stay"), arc("discount", "stay"), arc("usage", "stay"), arc("price", "stay"),
        ]}));
        (nodes, options)
    }

    fn render(nodes: &[Row], options: &Row) -> Vec<OutColumn> {
        let mut cols = columns(nodes, options).unwrap();
        decorate(&mut cols, nodes, options).unwrap();
        cols
    }

    fn col<'a>(cols: &'a [OutColumn], role: &str) -> &'a [J] {
        &cols.iter().find(|c| c.role == role).unwrap().values
    }

    fn card(cols: &[OutColumn], k: usize) -> J {
        serde_json::from_str(col(cols, "BELIEF")[k].as_str().unwrap()).unwrap()
    }

    fn edge(cols: &[OutColumn], f: &str, t: &str) -> J {
        let all: Vec<J> = serde_json::from_str(col(cols, "EDGES")[0].as_str().unwrap()).unwrap();
        all.into_iter()
            .find(|e| e["from"] == f && e["to"] == t)
            .unwrap_or_else(|| panic!("no arc {f} → {t}"))
    }

    /// The minimal set {engaged}: X, Y, the ring; the back-door paths
    /// dashed and blocked (a bar on the arc leaving engaged), the causal
    /// paths in the accent colour; no set: the back-door paths open, red.
    #[test]
    fn marks_the_adjustment_set_and_the_back_door_paths() {
        let (nodes, mut options) = discount();
        options.insert(
            "adjust".into(),
            json!([{"treatment": "discount", "outcome": "stay", "set_id": "minimal", "node": "engaged", "kind": "minimal"}]),
        );
        let cols = render(&nodes, &options);
        assert_eq!(card(&cols, 2)["causal"], "treatment");
        assert_eq!(card(&cols, 4)["causal"], "outcome");
        assert_eq!(card(&cols, 0)["causal"], "adjust");
        assert!(card(&cols, 0)["note"]
            .as_str()
            .unwrap()
            .contains("the minimal adjustment set (Engaged)"));
        assert!(card(&cols, 1).get("causal").is_none());
        // Discount ← Engaged → Renewed: blocked at Engaged
        let e = edge(&cols, "engaged", "stay");
        assert_eq!(e["style"], "dashed");
        assert_eq!(e["blocked"], true);
        assert_eq!(e["key"], "back-door path, blocked");
        assert!(
            e["tip"]
                .as_str()
                .unwrap()
                .contains("Discount ← Engaged → Renewed: blocked at Engaged"),
            "{e}"
        );
        let e = edge(&cols, "engaged", "discount");
        assert_eq!(e["style"], "dashed");
        assert!(
            e.get("blocked").is_none(),
            "the bar is on the arc after the blocker"
        );
        assert_eq!(edge(&cols, "engaged", "usage")["blocked"], true);
        // the causal paths
        for (f, t) in [
            ("discount", "stay"),
            ("discount", "usage"),
            ("usage", "stay"),
        ] {
            let e = edge(&cols, f, t);
            assert_eq!(e["color"], "accent", "{f} → {t}");
            assert_eq!(e["key"], "causal path");
        }
        // price only affects the outcome: untouched
        assert!(edge(&cols, "price", "stay").get("key").is_none());
        assert!(
            lint(&nodes, &options).is_empty(),
            "{:?}",
            lint(&nodes, &options)
        );

        // no adjustment: the back-door paths open, red, and no lint
        options.insert(
            "adjust".into(),
            json!([{"treatment": "discount", "outcome": "stay", "set_id": "none", "node": null}]),
        );
        let cols = render(&nodes, &options);
        let e = edge(&cols, "engaged", "stay");
        assert_eq!(
            (e["color"].as_str(), e["key"].as_str()),
            (Some("bad"), Some("back-door path, open"))
        );
        assert!(e.get("blocked").is_none());
        assert!(lint(&nodes, &options).is_empty());
    }

    #[test]
    fn marks_front_door_mediators() {
        let (nodes, mut options) = discount();
        options.insert(
            "adjust".into(),
            json!([{"treatment": "discount", "outcome": "stay", "set_id": "fd", "node": "usage", "kind": "front-door"}]),
        );
        let cols = render(&nodes, &options);
        assert_eq!(card(&cols, 3)["causal"], "mediator");
        assert!(
            card(&cols, 0).get("causal").is_none(),
            "no set: nothing ringed"
        );
        assert_eq!(edge(&cols, "engaged", "stay")["color"], "bad");
    }

    #[test]
    fn lints_bad_sets() {
        let (nodes, mut options) = discount();
        options.insert(
            "adjust".into(),
            json!([{"treatment": "discount", "outcome": "stay", "set_id": "s", "node": "usage", "kind": "minimal"},
                   {"treatment": "discount", "outcome": "stay", "set_id": "t", "node": "ghost", "kind": "smallest"}]),
        );
        options.insert(
            "reference".into(),
            json!([{"from": "engaged", "to": "nowhere"}]),
        );
        let l = lint(&nodes, &options);
        assert_eq!(
            l,
            [
                "bayes_net/unknown-id: adjust or reference name ghost, nowhere that are not nodes",
                "bayes_net/bad-adjust-kind: adjustment kind smallest is not minimal, optimal, canonical or front-door",
                "bayes_net/adjust-descendant: adjustment set s holds Weekly use, downstream of Discount — adjusting for it biases the effect",
                "bayes_net/adjust-open: adjustment set s leaves the back-door path Discount ← Engaged → Renewed open",
                "bayes_net/adjust-open: adjustment set t leaves the back-door path Discount ← Engaged → Renewed, Discount ← Engaged → Weekly use → Renewed open",
            ],
            "{l:?}"
        );
    }

    /// Beliefs in two worlds: the change (do on discount) and what it
    /// reaches split into a factual and a counterfactual copy, the rest
    /// shared; lanes in that order; arcs, beliefs, evidence follow.
    #[test]
    fn builds_the_twin_network() {
        let (nodes, mut options) = discount();
        let b = |n: &str, p: f64, w: &str| {
            [
                json!({"node": n, "state": "yes", "probability": p, "world": w}),
                json!({"node": n, "state": "no", "probability": 1.0 - p, "world": w}),
            ]
        };
        let mut beliefs: Vec<J> = Vec::new();
        for (n, pf, pc) in [
            ("engaged", 0.8, 0.8),
            ("price", 0.4, 0.4),
            ("discount", 1.0, 0.0),
            ("usage", 0.7, 0.5),
            ("stay", 1.0, 0.75),
        ] {
            beliefs.extend(b(n, pf, "factual"));
            beliefs.extend(b(n, pc, "counterfactual"));
        }
        options.insert("beliefs".into(), J::Array(beliefs));
        options.insert(
            "evidence".into(),
            json!([{"node": "discount", "state": "yes"}, {"node": "stay", "state": "yes"},
                   {"node": "discount", "state": "no", "mode": "do"}]),
        );
        let (rows, o) = twin(&nodes, &options).unwrap().unwrap();
        let cols = render(&rows, &o);
        let ids: Vec<&str> = col(&cols, "DIAGRAM")
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(
            ids,
            [
                "discount",
                "usage",
                "stay",
                "engaged",
                "price",
                "discount*",
                "usage*",
                "stay*"
            ]
        );
        let lanes: Vec<&str> = col(&cols, "LANE")
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(lanes[0], FACTUAL);
        assert_eq!(lanes[3], SHARED);
        assert_eq!(lanes[7], COUNTERFACTUAL);
        // the counterfactual copy: set by do, its belief, its note
        let d = card(&cols, 5);
        assert_eq!(d["evidence"], json!({"state": "no", "mode": "do"}));
        assert_eq!(d["states"][0]["p"], 0.0);
        assert!(
            d["note"]
                .as_str()
                .unwrap()
                .contains("had Discount set to no"),
            "{d}"
        );
        assert_eq!(card(&cols, 7)["states"][0]["p"], 0.75);
        // the factual copies keep what was seen
        assert_eq!(
            card(&cols, 0)["evidence"],
            json!({"state": "yes", "mode": "observe"})
        );
        assert_eq!(card(&cols, 2)["states"][0]["p"], 1.0);
        // arcs: the shared parent into both copies, the copy's own chain,
        // the intervened copy's incoming arc cut
        let e = edge(&cols, "engaged", "discount*");
        assert_eq!(e["cut"], true);
        edge(&cols, "discount*", "usage*");
        edge(&cols, "usage*", "stay*");
        edge(&cols, "price", "stay*");
        edge(&cols, "engaged", "stay");
        assert!(
            lint(&nodes, &options).is_empty(),
            "{:?}",
            lint(&nodes, &options)
        );
        // without a world column: no twin
        assert!(twin(&nodes, &discount().1).unwrap().is_none());
    }

    /// The learned arcs against the reference: matching, extra, reversed,
    /// missing (added, dashed); below the threshold an arc is not learned.
    #[test]
    fn diffs_the_learned_structure() {
        let (nodes, _) = discount();
        let options = opts(json!({
            "arcs": [{"from": "engaged", "to": "discount", "strength": 0.9},
                     {"from": "stay", "to": "usage", "strength": 0.7},
                     {"from": "price", "to": "usage", "strength": 0.6},
                     {"from": "discount", "to": "stay", "strength": 0.3}],
            "reference": [{"from": "engaged", "to": "discount"}, {"from": "usage", "to": "stay"},
                          {"from": "discount", "to": "stay"}],
            "threshold": 0.5,
        }));
        let cols = render(&nodes, &options);
        let all: Vec<J> = serde_json::from_str(col(&cols, "EDGES")[0].as_str().unwrap()).unwrap();
        assert_eq!(all.len(), 4, "{all:?}");
        assert_eq!(edge(&cols, "engaged", "discount")["key"], "matching (1)");
        assert_eq!(edge(&cols, "stay", "usage")["color"], "accent");
        assert_eq!(edge(&cols, "price", "usage")["key"], "extra (1)");
        let m = edge(&cols, "discount", "stay");
        assert_eq!(
            (m["key"].as_str(), m["style"].as_str()),
            (Some("missing (1)"), Some("dashed"))
        );
        assert_eq!(all[0]["caption"], "structural Hamming distance 3");
        assert_eq!(
            root_attrs(&options).unwrap(),
            [("data-bn-shd".to_string(), "3".to_string())]
        );
        // at 0.2 the weak arc is learned: matching
        let mut low = options.clone();
        low.insert("threshold".into(), json!(0.2));
        assert_eq!(root_attrs(&low).unwrap()[0].1, "2");
        assert!(root_attrs(&Row::new()).unwrap().is_empty());
        // #1336: a required arc counts whatever its strength
        let mut req = options.clone();
        req.insert(
            "arcs".into(),
            json!([{"from": "engaged", "to": "discount", "strength": 0.9},
                   {"from": "stay", "to": "usage", "strength": 0.7},
                   {"from": "price", "to": "usage", "strength": 0.6},
                   {"from": "discount", "to": "stay", "strength": 0.3, "status": "required"}]),
        );
        assert_eq!(root_attrs(&req).unwrap()[0].1, "2");
    }
}
