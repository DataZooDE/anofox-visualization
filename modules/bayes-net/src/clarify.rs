//! Clarify the graph (#1282): the arcs a structure learner could not
//! orient, and what experts answered about them.
//!
//! - `arcs :=` rows take `status` `directed` (also `learned`, `required`),
//!   `undirected` (in the Markov equivalence class: the data alone can't
//!   tell the direction) or `expert` (set by an answer), plus `lean`, the
//!   share of bootstrap resamples that pointed `from → to` (0–1), and for an
//!   expert arc `who`, `when` and `rationale`.
//! - An undirected arc is drawn without arrowheads, coloured by its lean:
//!   the accent colour when the data lean one way (at least 70 % either
//!   way), amber when they can't tell. An expert arc carries a small badge
//!   (✓ and who) and says who set it, when and why on hover.
//! - `answers :=` `a, b, answer, who, when, rationale` (`answer`: `a_to_b`,
//!   `b_to_a`, `no_arc`, `unsure`) is only linted: answers that name
//!   unknown nodes, an unknown answer, and an answer that would close a
//!   cycle with the directed arcs (the cycle named).
//!
//! The question cards, the progress of the target effect and the history
//! are plain roles over the producer's tables (see `docs/modules.md`).

use crate::bayes_net::{get, list, num};
use crate::Row;
use anofox_viz_plugin::OutColumn;
use serde_json::Value as J;

/// The arc statuses the contract knows.
pub(crate) const STATUSES: &[&str] = &[
    "directed",
    "learned",
    "required",
    "forbidden",
    "undirected",
    "expert",
];

/// The answers an expert can give.
pub(crate) const ANSWERS: &[&str] = &["a_to_b", "b_to_a", "no_arc", "unsure"];

/// How far from 0.5 a lean must be to count as the data leaning one way.
const LEAN_CLEAR: f64 = 0.2;

fn status(a: &Row) -> Option<String> {
    get(a, &["status"]).map(|s| s.to_ascii_lowercase())
}

/// An arc the data can't orient: not part of the directed graph (cycles).
pub(crate) fn undirected(a: &Row) -> bool {
    status(a).as_deref() == Some("undirected")
}

fn pct(v: f64) -> String {
    format!("{}%", (v * 100.0).round())
}

/// Mark the drawn arcs: undirected ones without arrowheads, coloured by
/// their lean; expert ones with a badge and who / when / why on hover.
pub(crate) fn decorate(cols: &mut [OutColumn], options: &Row) -> Result<(), String> {
    let arcs = list(options, "arcs")?;
    if !arcs
        .iter()
        .any(|a| matches!(status(a).as_deref(), Some("undirected" | "expert")))
    {
        return Ok(());
    }
    let ids: Vec<String> = column(cols, "DIAGRAM")
        .iter()
        .map(|v| v.as_str().unwrap_or_default().to_string())
        .collect();
    let labels: Vec<String> = column(cols, "LABEL")
        .iter()
        .map(|v| v.as_str().unwrap_or_default().to_string())
        .collect();
    let label = |id: &str| {
        ids.iter()
            .position(|i| i == id)
            .and_then(|k| labels.get(k).cloned())
            .unwrap_or_else(|| id.to_string())
    };
    let Some(edges) = cols.iter_mut().find(|c| c.role == "EDGES") else {
        return Ok(());
    };
    let Some(cell) = edges.values.first_mut() else {
        return Ok(());
    };
    let Some(mut drawn) = cell
        .as_str()
        .and_then(|s| serde_json::from_str::<Vec<J>>(s).ok())
    else {
        return Ok(());
    };
    for e in drawn.iter_mut() {
        let Some(o) = e.as_object_mut() else { continue };
        let (Some(f), Some(t)) = (
            o.get("from").and_then(J::as_str).map(str::to_string),
            o.get("to").and_then(J::as_str).map(str::to_string),
        ) else {
            continue;
        };
        let Some(a) = arcs.iter().find(|a| {
            get(a, &["from"]).as_deref() == Some(&f) && get(a, &["to"]).as_deref() == Some(&t)
        }) else {
            continue;
        };
        let (lf, lt) = (label(&f), label(&t));
        // the hover text without the bare status word the drawing put there
        let mut tip: Vec<String> = o
            .get("tip")
            .and_then(J::as_str)
            .map(|s| {
                s.split(", ")
                    .filter(|p| !matches!(*p, "undirected" | "expert"))
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        match status(a).as_deref() {
            Some("undirected") => {
                let lean = num(a, &["lean"]).map(|l| l.clamp(0.0, 1.0));
                o.insert("undirected".into(), J::Bool(true));
                // never dashed: open is not weak
                o.remove("style");
                let clear = lean.is_some_and(|l| (l - 0.5).abs() >= LEAN_CLEAR);
                o.insert(
                    "color".into(),
                    J::String(if clear { "accent" } else { "warn" }.into()),
                );
                o.insert(
                    "key".into(),
                    J::String(
                        if clear {
                            "Open: the data lean one way"
                        } else {
                            "Open: the data can't tell"
                        }
                        .into(),
                    ),
                );
                tip.insert(
                    0,
                    match lean {
                        Some(l) if l >= 0.5 => {
                            format!("open: which way? The data lean {} {lf} → {lt}", pct(l))
                        }
                        Some(l) => format!(
                            "open: which way? The data lean {} {lt} → {lf}",
                            pct(1.0 - l)
                        ),
                        None => "open: which way? The data can't tell".into(),
                    },
                );
            }
            Some("expert") => {
                let who = get(a, &["who"]);
                let when = get(a, &["when", "at"]).map(|w| w.chars().take(10).collect::<String>());
                o.insert("color".into(), J::String("good".into()));
                o.insert("key".into(), J::String("Settled by an expert".into()));
                let badge: String = who
                    .as_deref()
                    .map(|w| w.chars().take(14).collect())
                    .unwrap_or_else(|| "expert".into());
                o.insert("label".into(), J::String(format!("✓ {badge}")));
                let mut said = "set by ".to_string() + who.as_deref().unwrap_or("an expert");
                if let Some(w) = &when {
                    said += &format!(" on {w}");
                }
                if let Some(r) = get(a, &["rationale", "why"]) {
                    said += &format!(": {r}");
                }
                tip.insert(0, said);
            }
            _ => continue,
        }
        o.insert("tip".into(), J::String(tip.join(", ")));
    }
    *cell = J::String(J::Array(drawn).to_string());
    Ok(())
}

fn column<'a>(cols: &'a [OutColumn], role: &str) -> &'a [J] {
    cols.iter()
        .find(|c| c.role == role)
        .map_or(&[][..], |c| &c.values[..])
}

/// What the lint says about arc statuses, leans and the answers.
pub(crate) fn problems(rows: &[Row], options: &Row) -> Result<Vec<String>, String> {
    let arcs = list(options, "arcs")?;
    let answers = list(options, "answers")?;
    let mut out = Vec::new();
    let mut bad: Vec<String> = Vec::new();
    for a in &arcs {
        if let Some(s) = get(a, &["status"]) {
            if !STATUSES.contains(&s.to_ascii_lowercase().as_str()) && !bad.contains(&s) {
                bad.push(s);
            }
        }
    }
    if !bad.is_empty() {
        out.push(format!(
            "bayes_net/bad-status: arc status {} is not one of {} — drawn as directed",
            bad.join(", "),
            STATUSES.join(", ")
        ));
    }
    let off: Vec<String> = arcs
        .iter()
        .filter(|a| num(a, &["lean"]).is_some_and(|l| !(0.0..=1.0).contains(&l)))
        .filter_map(|a| Some(format!("{} → {}", get(a, &["from"])?, get(a, &["to"])?)))
        .collect();
    if !off.is_empty() {
        out.push(format!(
            "bayes_net/bad-lean: the lean of {} is not a share between 0 and 1",
            off.join(", ")
        ));
    }
    if answers.is_empty() {
        return Ok(out);
    }
    let ids: Vec<String> = rows.iter().filter_map(|r| get(r, &["id"])).collect();
    let has = |id: &str| ids.iter().any(|i| i == id);
    let mut unknown: Vec<String> = Vec::new();
    let mut words: Vec<String> = Vec::new();
    for r in &answers {
        for id in [get(r, &["a"]), get(r, &["b"])].into_iter().flatten() {
            if !has(&id) && !unknown.contains(&id) {
                unknown.push(id);
            }
        }
        if let Some(w) = get(r, &["answer"]) {
            if !ANSWERS.contains(&w.to_ascii_lowercase().as_str()) && !words.contains(&w) {
                words.push(w);
            }
        }
    }
    if !unknown.is_empty() {
        out.push(format!(
            "bayes_net/answer-unknown-node: answers name {} that are not nodes",
            unknown.join(", ")
        ));
    }
    if !words.is_empty() {
        out.push(format!(
            "bayes_net/bad-answer: answer {} is not one of {}",
            words.join(", "),
            ANSWERS.join(", ")
        ));
    }
    // the directed arcs as they stand (an undirected or forbidden arc is
    // no direction yet)
    let directed: Vec<(String, String)> = arcs
        .iter()
        .filter(|a| !matches!(status(a).as_deref(), Some("undirected" | "forbidden")))
        .filter_map(|a| Some((get(a, &["from"])?, get(a, &["to"])?)))
        .collect();
    let label = |id: &str| {
        rows.iter()
            .find(|r| get(r, &["id"]).as_deref() == Some(id))
            .and_then(|r| get(r, &["label", "name"]))
            .unwrap_or_else(|| id.to_string())
    };
    let mut cycles: Vec<String> = Vec::new();
    for r in &answers {
        let (Some(a), Some(b), Some(w)) = (get(r, &["a"]), get(r, &["b"]), get(r, &["answer"]))
        else {
            continue;
        };
        let (f, t) = match w.to_ascii_lowercase().as_str() {
            "a_to_b" => (a, b),
            "b_to_a" => (b, a),
            _ => continue,
        };
        if !has(&f) || !has(&t) {
            continue;
        }
        // f → t closes a cycle when t already reaches f (not by the arc
        // between the two: an answer that overrules it is a disagreement)
        let others: Vec<(String, String)> = directed
            .iter()
            .filter(|(x, y)| !((*x == f && *y == t) || (*x == t && *y == f)))
            .cloned()
            .collect();
        if let Some(back) = reach(&others, &t, &f) {
            let mut c: Vec<String> = vec![label(&f)];
            c.extend(back.iter().map(|n| label(n)));
            let who = get(r, &["who"])
                .map(|w| format!(" ({w})"))
                .unwrap_or_default();
            let line = format!("{} → {}{who}: {}", label(&f), label(&t), c.join(" → "));
            if !cycles.contains(&line) {
                cycles.push(line);
            }
        }
    }
    if !cycles.is_empty() {
        out.push(format!(
            "bayes_net/answer-cycle: an answer would close a cycle with the directed arcs — {}",
            cycles.join("; ")
        ));
    }
    Ok(out)
}

/// A directed path from `s` to `goal` along `arcs` (`s` first, `goal` last).
fn reach(arcs: &[(String, String)], s: &str, goal: &str) -> Option<Vec<String>> {
    let mut prev: Vec<(String, String)> = Vec::new(); // (node, came from)
    let mut queue = std::collections::VecDeque::from([s.to_string()]);
    let mut seen = vec![s.to_string()];
    while let Some(u) = queue.pop_front() {
        if u == goal {
            let mut path = vec![u.clone()];
            let mut at = u;
            while let Some((_, p)) = prev.iter().find(|(n, _)| *n == at) {
                path.push(p.clone());
                at = p.clone();
            }
            path.reverse();
            return Some(path);
        }
        for (f, t) in arcs {
            if *f == u && !seen.contains(t) {
                seen.push(t.clone());
                prev.push((t.clone(), u.clone()));
                queue.push_back(t.clone());
            }
        }
    }
    None
}

/// `json!` rows for the tests.
#[cfg(test)]
use serde_json::json;

#[cfg(test)]
fn rows(v: J) -> Vec<Row> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|r| r.as_object().unwrap().clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(v: J) -> Row {
        v.as_object().unwrap().clone()
    }

    /// The chest clinic's smoking corner: smoke – lung and smoke – bronc
    /// undirected, either → dysp directed.
    fn chest() -> (Vec<Row>, Row) {
        let yn = json!(["yes", "no"]);
        let nodes = rows(json!([
            {"id": "smoke", "label": "Smoker", "states": yn},
            {"id": "lung", "label": "Lung cancer", "states": yn},
            {"id": "bronc", "label": "Bronchitis", "states": yn},
            {"id": "dysp", "label": "Short of breath", "states": yn},
        ]));
        let options = opts(json!({"arcs": [
            {"from": "smoke", "to": "lung", "status": "undirected", "lean": 0.81, "strength": 0.3},
            {"from": "smoke", "to": "bronc", "status": "undirected", "lean": 0.55},
            {"from": "bronc", "to": "dysp", "status": "directed", "strength": 1.0},
            {"from": "lung", "to": "dysp", "status": "expert", "who": "dr-a",
             "when": "2026-11-18T09:30:00Z", "rationale": "a tumour narrows the airways"},
        ]}));
        (nodes, options)
    }

    #[test]
    fn undirected_arcs_lose_their_heads_and_take_their_lean() {
        let (nodes, options) = chest();
        let mut cols = crate::bayes_net::columns(&nodes, &options).unwrap();
        decorate(&mut cols, &options).unwrap();
        let edges: Vec<J> =
            serde_json::from_str(column(&cols, "EDGES")[0].as_str().unwrap()).unwrap();
        let e = |f: &str, t: &str| {
            edges
                .iter()
                .find(|e| e["from"] == f && e["to"] == t)
                .unwrap()
                .clone()
        };
        let lung = e("smoke", "lung");
        assert_eq!(lung["undirected"], true);
        assert_eq!(lung["color"], "accent");
        assert_eq!(lung["key"], "Open: the data lean one way");
        // open is not weak: never dashed
        assert!(lung.get("style").is_none(), "{lung}");
        assert_eq!(
            lung["tip"],
            "open: which way? The data lean 81% Smoker → Lung cancer, strength 0.3"
        );
        let bronc = e("smoke", "bronc");
        assert_eq!(bronc["color"], "warn");
        assert_eq!(bronc["key"], "Open: the data can't tell");
        let set = e("lung", "dysp");
        assert!(set.get("undirected").is_none());
        assert_eq!(set["color"], "good");
        assert_eq!(set["label"], "✓ dr-a");
        assert_eq!(
            set["tip"],
            "set by dr-a on 2026-11-18: a tumour narrows the airways"
        );
        // a directed arc stays as it was
        let d = e("bronc", "dysp");
        assert!(d.get("color").is_none() && d.get("undirected").is_none());
        assert!(problems(&nodes, &options).unwrap().is_empty());
    }

    #[test]
    fn a_lean_toward_the_other_end_names_it() {
        let (nodes, mut options) = chest();
        options["arcs"][0]["lean"] = json!(0.1);
        let mut cols = crate::bayes_net::columns(&nodes, &options).unwrap();
        decorate(&mut cols, &options).unwrap();
        let edges = column(&cols, "EDGES")[0].as_str().unwrap();
        assert!(
            edges.contains("The data lean 90% Lung cancer → Smoker"),
            "{edges}"
        );
    }

    #[test]
    fn undirected_arcs_are_no_cycle() {
        let (nodes, mut options) = chest();
        // smoke → lung → dysp → smoke would be a cycle, but two of its
        // arcs are undirected: no direction yet, no cycle
        options["arcs"]
            .as_array_mut()
            .unwrap()
            .push(json!({"from": "dysp", "to": "smoke", "status": "undirected"}));
        let lint = crate::bayes_net::lint(&nodes, &options);
        assert!(
            !lint.iter().any(|l| l.contains("bayes_net/cycle")),
            "{lint:?}"
        );
    }

    #[test]
    fn lints_answers_unknown_nodes_words_and_cycles() {
        let (nodes, mut options) = chest();
        options.insert(
            "answers".into(),
            json!([
                {"a": "smoke", "b": "lung", "answer": "a_to_b", "who": "dr-a"},
                {"a": "smoke", "b": "asbestos", "answer": "a_to_b"},
                {"a": "smoke", "b": "bronc", "answer": "maybe"},
                // dysp → smoke closes smoke → lung → dysp (an unknown status counts as directed)
                {"a": "smoke", "b": "dysp", "answer": "b_to_a", "who": "dr-b"},
                // overrules the expert arc lung → dysp: a disagreement, no cycle
                {"a": "lung", "b": "dysp", "answer": "b_to_a", "who": "dr-c"},
            ]),
        );
        options["arcs"][0]["status"] = json!("maybe-later");
        options["arcs"][1]["lean"] = json!(1.4);
        options["arcs"][1]["status"] = json!("directed");
        let p = problems(&nodes, &options).unwrap();
        assert_eq!(
            p,
            [
                "bayes_net/bad-status: arc status maybe-later is not one of directed, learned, required, forbidden, undirected, expert — drawn as directed",
                "bayes_net/bad-lean: the lean of smoke → bronc is not a share between 0 and 1",
                "bayes_net/answer-unknown-node: answers name asbestos that are not nodes",
                "bayes_net/bad-answer: answer maybe is not one of a_to_b, b_to_a, no_arc, unsure",
                "bayes_net/answer-cycle: an answer would close a cycle with the directed arcs — Short of breath → Smoker (dr-b): Short of breath → Smoker → Lung cancer → Short of breath",
            ]
        );
        // through the plugin's lint
        assert!(crate::bayes_net::lint(&nodes, &options)
            .iter()
            .any(|l| l.starts_with("bayes_net/answer-cycle")));
    }

    #[test]
    fn a_longer_cycle_is_named_in_full() {
        let nodes = rows(json!([{"id": "a"}, {"id": "b"}, {"id": "c"}]));
        let options = opts(json!({
            "arcs": [{"from": "a", "to": "b"}, {"from": "b", "to": "c"}],
            "answers": [{"a": "a", "b": "c", "answer": "b_to_a"}],
        }));
        let p = problems(&nodes, &options).unwrap();
        assert_eq!(
            p,
            ["bayes_net/answer-cycle: an answer would close a cycle with the directed arcs — c → a: c → a → b → c"]
        );
    }
}
