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

/// A factor's glyph (#1166): the usual symbol of its distribution, else its
/// capitals (`NormalMixture` → `NM`), else the name cut to four letters.
pub(crate) fn glyph(name: &str) -> String {
    let n = name.trim();
    let l = n.to_ascii_lowercase();
    let known: &[(&str, &str)] = &[
        ("mvnormal", "𝒩"),
        ("normalmixture", "NM"),
        ("normal", "𝒩"),
        ("gaussian", "𝒩"),
        ("gamma", "Γ"),
        ("beta", "Beta"),
        ("bernoulli", "Ber"),
        ("binomial", "Bin"),
        ("categorical", "Cat"),
        ("matrixdirichlet", "Dir"),
        ("dirichlet", "Dir"),
        ("discretetransition", "T"),
        ("transition", "T"),
        ("poisson", "Poi"),
        ("exponential", "Exp"),
        ("uniform", "U"),
        ("wishart", "𝒲"),
        ("probit", "Φ"),
    ];
    if let Some((_, g)) = known.iter().find(|(k, _)| l.starts_with(k)) {
        return g.to_string();
    }
    let caps: String = n.chars().filter(|c| c.is_ascii_uppercase()).collect();
    if (2..=3).contains(&caps.len()) {
        return caps;
    }
    n.chars().take(4).collect()
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

/// #1172: a prior's mean and sd from its family and its constant
/// parameters (in interface order), for the families with closed forms.
fn moments(family: &str, p: &[f64]) -> Option<(f64, f64)> {
    let (m, var) = match (family, p) {
        ("Beta", [a, b]) if *a > 0.0 && *b > 0.0 => {
            (a / (a + b), a * b / ((a + b).powi(2) * (a + b + 1.0)))
        }
        ("Normal" | "NormalMeanVariance", [m, v]) if *v > 0.0 => (*m, *v),
        ("NormalMeanPrecision", [m, w]) if *w > 0.0 => (*m, 1.0 / w),
        ("GammaShapeRate", [a, b]) if *a > 0.0 && *b > 0.0 => (a / b, a / (b * b)),
        ("Gamma" | "GammaShapeScale", [a, t]) if *a > 0.0 && *t > 0.0 => (a * t, a * t * t),
        ("Uniform", [a, b]) if b > a => ((a + b) / 2.0, (b - a).powi(2) / 12.0),
        _ => return None,
    };
    Some((m, var.sqrt()))
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
    // #1172: what the producer adds — each message's value, the predictive
    // at the data, each observation's influence on a variable
    let messages = list(options, "messages")?;
    let predictive = list(options, "predictive")?;
    let influence = list(options, "influence")?;
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
    // #1172 Forney style: a variable on exactly two factors, with no
    // marginals of its own, is the edge between them — (variable, from, to),
    // from the factor whose `out` it is
    let mut forney: Vec<(String, String, String)> = Vec::new();
    for (id, k) in &id_kind {
        if k.as_deref() != Some("variable") {
            continue;
        }
        let mine: Vec<&Row> = edges
            .iter()
            .filter(|e| get(e, &["variable"]).as_deref() == Some(id.as_str()))
            .collect();
        let factors: Vec<(String, bool)> = mine
            .iter()
            .filter_map(|e| {
                let f = get(e, &["factor"])?;
                (kind_of(&f).as_deref() == Some("factor"))
                    .then(|| (f, get(e, &["interface"]).as_deref() == Some("out")))
            })
            .collect();
        let has_marginals = marginals
            .iter()
            .any(|m| get(m, &["variable"]).as_deref() == Some(id.as_str()));
        if let ([a, b], 2, false) = (&factors[..], mine.len(), has_marginals) {
            let (from, to) = if b.1 && !a.1 { (b, a) } else { (a, b) };
            forney.push((id.clone(), from.0.clone(), to.0.clone()));
        }
    }
    let forney_of = |v: &str| forney.iter().find(|(x, _, _)| x == v);
    let folded_into = |eq: &str| folded.iter().find(|(_, e)| e == eq).map(|(v, _)| v.clone());
    let is_folded = |id: &str| folded.iter().any(|(v, _)| v == id) || forney_of(id).is_some();
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
    // the posterior mean over the iterations (scalar marginals), oldest first
    let trend = |id: &str| -> Vec<f64> {
        let mut pts: Vec<(f64, f64)> = marginals
            .iter()
            .filter(|m| get(m, &["variable"]).as_deref() == Some(id))
            .filter(|m| get(m, &["component"]).is_none())
            .filter_map(|m| {
                let n = |k: &str| get(m, &[k]).and_then(|s| s.parse::<f64>().ok());
                Some((n("iteration").unwrap_or(0.0), n("mean")?))
            })
            .collect();
        pts.sort_by(|a, b| a.0.total_cmp(&b.0));
        pts.into_iter().map(|p| p.1).collect()
    };
    // #1166: the belief per iteration (scalar marginals): a range covering
    // every iteration's mean ± 3 sd, and (iteration, mean, sd)
    let belief = |id: &str, prior: Option<(f64, f64)>, n: usize| -> Option<String> {
        let mut its: Vec<(f64, f64, f64)> = marginals
            .iter()
            .filter(|m| get(m, &["variable"]).as_deref() == Some(id))
            .filter(|m| get(m, &["component"]).is_none())
            .filter_map(|m| {
                let n = |k: &str| get(m, &[k]).and_then(|s| s.parse::<f64>().ok());
                Some((
                    n("iteration").unwrap_or(0.0),
                    n("mean")?,
                    n("sd").filter(|v| *v > 0.0)?,
                ))
            })
            .collect();
        if its.is_empty() {
            return None;
        }
        its.sort_by(|a, b| a.0.total_cmp(&b.0));
        let lo = its
            .iter()
            .map(|t| t.1 - 3.0 * t.2)
            .fold(f64::INFINITY, f64::min);
        let hi = its
            .iter()
            .map(|t| t.1 + 3.0 * t.2)
            .fold(f64::NEG_INFINITY, f64::max);
        // #1172: the range takes in the prior, at most three times the
        // posterior's width (a vague prior is a flat line across it)
        let (lo, hi) = match prior {
            Some((m, sd)) => {
                let (c, w) = ((lo + hi) / 2.0, 1.5 * (hi - lo));
                (
                    (m - 3.0 * sd).min(lo).max(c - w),
                    (m + 3.0 * sd).max(hi).min(c + w),
                )
            }
            None => (lo, hi),
        };
        let mut b = serde_json::json!({ "lo": lo, "hi": hi, "its": its.iter().map(|t| [t.0, t.1, t.2]).collect::<Vec<_>>() });
        if let Some((m, sd)) = prior {
            b["prior"] = serde_json::json!([m, sd]);
        }
        if n > 0 {
            b["n"] = n.into();
        }
        Some(b.to_string())
    };
    // #1172: the edges at a variable — its own and, folded, its equality
    // node's
    let at = |touch: &[&str]| -> Vec<&Row> {
        edges
            .iter()
            .filter(|e| get(e, &["variable"]).is_some_and(|v| touch.contains(&v.as_str())))
            .collect()
    };
    let factor_edges = |f: &str| -> Vec<&Row> {
        let mut v: Vec<&Row> = edges
            .iter()
            .filter(|e| get(e, &["factor"]).as_deref() == Some(f))
            .collect();
        v.sort_by_key(|e| {
            get(e, &["seq"])
                .and_then(|s| s.parse::<i64>().ok())
                .unwrap_or(0)
        });
        v
    };
    // the prior: the factor whose `out` is the variable and whose inputs
    // are all constants, its mean and sd from the constants' values
    let prior_of = |touch: &[&str]| -> Option<(f64, f64)> {
        at(touch).into_iter().find_map(|e| {
            if get(e, &["interface"]).as_deref() != Some("out") {
                return None;
            }
            let f = get(e, &["factor"])?;
            let family = row_of(&f).and_then(|r| get(r, &["name", "label"]))?;
            let params: Option<Vec<f64>> = factor_edges(&f)
                .into_iter()
                .filter(|e| get(e, &["interface"]).as_deref() != Some("out"))
                .map(|e| {
                    let r = row_of(&get(e, &["variable"])?)?;
                    (kind(r).as_deref() == Some("constant"))
                        .then(|| get(r, &["name", "label"])?.parse::<f64>().ok())
                        .flatten()
                })
                .collect();
            moments(&family, &params?)
        })
    };
    // how many observations reach the variable: data on its factors
    let observed = |touch: &[&str]| -> usize {
        let mut seen: Vec<String> = vec![];
        for e in at(touch) {
            for d in factor_edges(&get(e, &["factor"]).unwrap_or_default()) {
                if let Some(v) = get(d, &["variable"]) {
                    if row_of(&v).and_then(kind).as_deref() == Some("data") && !seen.contains(&v) {
                        seen.push(v);
                    }
                }
            }
        }
        seen.len()
    };
    let mut beliefs = vec![];
    let mut learned: Vec<Option<String>> = vec![];
    let mut checks: Vec<Value> = vec![];
    let mut sparks = vec![];
    let (mut ids, mut labels, mut shapes, mut lanes, mut values, mut badges) =
        (vec![], vec![], vec![], vec![], vec![], vec![]);
    for r in &order {
        let id = get(r, &["id"]).unwrap_or_default();
        // an equality node with a folded variable shows (and is) that variable
        let var = folded_into(&id);
        let is_factor = kind(r).as_deref() == Some("factor");
        let label = match var.as_deref().and_then(row_of) {
            Some(v) => label_of(v),
            None if is_factor => glyph(&get(r, &["name", "label"]).unwrap_or_default()),
            None => label_of(r),
        };
        let m = latest(var.as_deref().unwrap_or(&id));
        let tr = trend(var.as_deref().unwrap_or(&id));
        let touch: Vec<&str> = [Some(id.as_str()), var.as_deref()]
            .into_iter()
            .flatten()
            .collect();
        let prior = prior_of(&touch);
        let n_obs = observed(&touch);
        beliefs.push(
            belief(var.as_deref().unwrap_or(&id), prior, n_obs).map_or(Value::Na, Value::Str),
        );
        // #1172: what the data did to the prior, in words
        let num = |v: f64| {
            crate::format::format_number_l(v, MetricFmt::Plain, &crate::format::Locale::default())
        };
        learned.push(match (prior, m, n_obs) {
            (Some((pm, psd)), Some((mean, Some(sd))), n) if n > 0 => Some(format!(
                "{} {} {} {} from {} ± {} to {} ± {}.",
                n,
                if n == 1 {
                    "observation"
                } else {
                    "observations"
                },
                if sd < psd { "narrowed" } else { "moved" },
                label,
                num(pm),
                num(psd),
                num(mean),
                num(sd)
            )),
            _ => None,
        });
        // #1172: what the producer's tables say about this node
        let target = var.clone().unwrap_or_else(|| id.clone());
        let fnum = |r: &Row, k: &str| {
            get(r, &[k])
                .and_then(|s| s.parse::<f64>().ok())
                .filter(|v| v.is_finite())
        };
        let scalar = |r: &Row| get(r, &["component"]).is_none();
        let mut more: Vec<String> = Vec::new();
        let mut check = Value::Na;
        // an observation: what the rest of the model expected — a surprise
        // more than 2 sd off is flagged (warn), more than 3 (bad)
        if kind(r).as_deref() == Some("data") {
            if let Some(p) = predictive
                .iter()
                .find(|p| get(p, &["variable"]).as_deref() == Some(id.as_str()) && scalar(p))
            {
                if let (Some(o), Some(mu)) = (fnum(p, "observed"), fnum(p, "mean")) {
                    match fnum(p, "sd").filter(|v| *v > 0.0) {
                        Some(sd) => {
                            let z = (o - mu).abs() / sd;
                            more.push(format!(
                                "Expected {} ± {}, measured {}{}.",
                                num(mu),
                                num(sd),
                                num(o),
                                if z > 2.0 {
                                    format!(
                                        ": {} sd off, a surprise",
                                        num((z * 10.0).round() / 10.0)
                                    )
                                } else {
                                    String::new()
                                }
                            ));
                            if z > 3.0 {
                                check = Value::Str("bad".into());
                            } else if z > 2.0 {
                                check = Value::Str("warn".into());
                            }
                        }
                        None => more.push(format!("Expected {}, measured {}.", num(mu), num(o))),
                    }
                }
            }
        }
        // a variable: the share of its certainty from its prior, from the
        // latest message each factor sent it (a prior's may only go out in
        // the first iteration). Precisions (1/sd²) add only for normal
        // messages: for any other family no share is given.
        let mut latest_in: Vec<(String, f64, f64)> = Vec::new(); // (factor, iteration, precision)
        let mut all_normal = true;
        for m in messages
            .iter()
            .filter(|m| get(m, &["variable"]).is_some_and(|v| touch.contains(&v.as_str())))
            .filter(|m| scalar(m))
            .filter(|m| {
                get(m, &["direction"]).is_some_and(|d| d.eq_ignore_ascii_case("to_variable"))
            })
        {
            let (Some(f), Some(sd)) = (get(m, &["factor"]), fnum(m, "sd").filter(|v| *v > 0.0))
            else {
                continue;
            };
            all_normal &= get(m, &["family"]).is_some_and(|fam| {
                matches!(
                    fam.as_str(),
                    "Normal" | "NormalMeanVariance" | "NormalMeanPrecision" | "Gaussian"
                )
            });
            let it = fnum(m, "iteration").unwrap_or(0.0);
            match latest_in.iter_mut().find(|x| x.0 == f) {
                Some(x) if it >= x.1 => *x = (f, it, 1.0 / (sd * sd)),
                Some(_) => {}
                None => latest_in.push((f, it, 1.0 / (sd * sd))),
            }
        }
        let incoming: Vec<(String, f64)> = latest_in.into_iter().map(|(f, _, p)| (f, p)).collect();
        let is_prior = |f: &str| {
            let ins: Vec<&Row> = factor_edges(f)
                .into_iter()
                .filter(|e| get(e, &["interface"]).as_deref() != Some("out"))
                .collect();
            !ins.is_empty()
                && ins.iter().all(|e| {
                    get(e, &["variable"])
                        .and_then(|v| row_of(&v))
                        .and_then(kind)
                        .as_deref()
                        == Some("constant")
                })
        };
        let total: f64 = incoming.iter().map(|x| x.1).sum();
        let from_prior: f64 = incoming
            .iter()
            .filter(|x| is_prior(&x.0))
            .map(|x| x.1)
            .sum();
        if all_normal && incoming.len() >= 2 && total > 0.0 && from_prior > 0.0 {
            let p = (100.0 * from_prior / total).round();
            more.push(if p < 1.0 {
                "Almost all of its certainty comes from the data (under 1 % from the prior)."
                    .to_string()
            } else {
                format!(
                    "About {} % of its certainty comes from the prior, {} % from the data.",
                    num(p),
                    num(100.0 - p)
                )
            });
        }
        // which observations moved it most: its mean without each
        let mut drivers: Vec<(f64, String, f64)> = influence
            .iter()
            .filter(|r| get(r, &["target"]).as_deref() == Some(target.as_str()) && scalar(r))
            .filter_map(|r| {
                let (mean, loo) = (fnum(r, "mean")?, fnum(r, "loo_mean")?);
                let o = get(r, &["observation"])?;
                Some((
                    (mean - loo).abs(),
                    row_of(&o).map(label_of).unwrap_or(o),
                    loo,
                ))
            })
            .collect();
        drivers.sort_by(|a, b| b.0.total_cmp(&a.0));
        if let Some(first) = drivers.first() {
            let rest: Vec<String> = drivers[1..]
                .iter()
                .take(2)
                .map(|(_, o, loo)| format!("{o} ({})", num(*loo)))
                .collect();
            more.push(format!(
                "Most influential: {} (without it {}){}.",
                first.1,
                num(first.2),
                if rest.is_empty() {
                    String::new()
                } else {
                    format!(", then {}", rest.join(", "))
                }
            ));
        }
        // still moving: the last iteration changed its mean by over 1 % of
        // its sd
        {
            let its = marginals
                .iter()
                .filter(|m| get(m, &["variable"]).as_deref() == Some(target.as_str()) && scalar(m))
                .filter_map(|m| Some((fnum(m, "iteration")?, fnum(m, "mean")?, fnum(m, "sd"))))
                .collect::<Vec<_>>();
            let mut its = its;
            its.sort_by(|a, b| a.0.total_cmp(&b.0));
            if let [.., a, b] = its[..] {
                let ch = (b.1 - a.1).abs();
                if b.2.is_some_and(|sd| ch > 0.01 * sd) {
                    more.push(format!(
                        "Still moving: the last iteration changed it by {}.",
                        num(ch)
                    ));
                }
            }
        }
        checks.push(check);
        if !more.is_empty() {
            let l = learned.last_mut().expect("pushed above");
            let joined = more.join(" ");
            *l = Some(match l.take() {
                Some(x) => format!("{x} {joined}"),
                None => joined,
            });
        }
        sparks.push(if tr.len() >= 2 {
            Value::Str(J::from(tr).to_string())
        } else {
            Value::Na
        });
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
        // a factor's full name goes with its glyph (in the tooltip)
        if let Some(name) = is_factor.then(|| get(r, &["name", "label"])).flatten() {
            if let Some(b) = badges.last_mut() {
                *b = Value::Str(name);
            }
        }
    }
    // #1166: what each node does, one plain sentence for its tooltip
    let shown_id = |id: &str| -> String {
        let id = folded
            .iter()
            .find(|(_, e)| e == id)
            .map_or(id, |(v, _)| v.as_str());
        row_of(id).map(label_of).unwrap_or_else(|| id.to_string())
    };
    let kind_of_id = |id: &str| row_of(id).and_then(kind).unwrap_or_default();
    let layer_of = |id: &str| row_of(id).and_then(|r| get(r, &["layer"]));
    let names = |v: &[String]| match v {
        [] => String::new(),
        [a] => a.clone(),
        _ => format!("{} and {}", v[..v.len() - 1].join(", "), v[v.len() - 1]),
    };
    let notes: Vec<Value> = ids
        .iter()
        .map(|idv| {
            let id = crate::value_text(idv);
            let k = kind_of_id(&id);
            let label = shown_id(&id);
            let mine: Vec<&Row> = edges
                .iter()
                .filter(|e| get(e, &["factor"]).as_deref() == Some(id.as_str()))
                .collect();
            let note = match k.as_str() {
                "data" => "Observed: a measured value, entering the model here.".to_string(),
                "constant" => "A fixed number given to the model.".to_string(),
                "delta" => "Deterministic: computes its output exactly from its inputs.".to_string(),
                "variable" => "A hidden quantity the model infers: its belief combines the messages from its factors.".to_string(),
                "equality" => {
                    let n = edges
                        .iter()
                        .filter(|e| get(e, &["variable"]).as_deref() == Some(id.as_str()))
                        .count();
                    if folded.iter().any(|(_, e)| *e == id) {
                        format!("{label}, a hidden quantity shared by {n} factors: their messages meet here and combine into its belief.")
                    } else {
                        format!("Equality node: {n} factors share one variable; their messages meet here.")
                    }
                }
                "factor" => {
                    let name = row_of(&id).and_then(|r| get(r, &["name", "label"])).unwrap_or_default();
                    let out = mine
                        .iter()
                        .find(|e| get(e, &["interface"]).as_deref() == Some("out"))
                        .and_then(|e| get(e, &["variable"]));
                    let ins: Vec<String> = mine
                        .iter()
                        .filter(|e| get(e, &["interface"]).as_deref() != Some("out"))
                        .filter_map(|e| get(e, &["variable"]))
                        .collect();
                    let in_labels: Vec<String> = ins.iter().map(|i| shown_id(i)).collect();
                    let all_const = ins.iter().all(|i| kind_of_id(i) == "constant");
                    match out {
                        Some(o) if kind_of_id(&o) == "data" => format!(
                            "Likelihood ({name}): connects {} to what we measure, {}. Observations enter the model here.",
                            names(&in_labels),
                            shown_id(&o)
                        ),
                        Some(o) if all_const => format!(
                            "Prior ({name}): what we believe about {} before any data arrives.",
                            shown_id(&o)
                        ),
                        Some(o) => match ins.iter().find(|i| kind_of_id(i) != "constant" && layer_of(i) == layer_of(&o)) {
                            Some(prev) => format!(
                                "Dynamics ({name}): how {} follows from {}.",
                                shown_id(&o),
                                shown_id(prev)
                            ),
                            None => format!("Factor ({name}): relates {} to {}.", names(&in_labels), shown_id(&o)),
                        },
                        None => format!("Factor ({name})."),
                    }
                }
                _ => String::new(),
            };
            if note.is_empty() { Value::Na } else { Value::Str(note) }
        })
        .zip(&learned)
        .map(|(note, l)| match (note, l) {
            (Value::Str(n), Some(l)) => Value::Str(format!("{n} {l}")),
            (note, _) => note,
        })
        .collect();
    let n = ids.len();
    let mut cols = vec![
        Column::new("id", Role::Diagram, ids),
        Column::new("label", Role::Label, labels),
        Column::new("shape", Role::Shape, shapes),
        Column::new("layer", Role::Lane, lanes),
        Column::new("note", Role::Tooltip, notes),
    ];
    if beliefs.iter().any(|v| !matches!(v, Value::Na)) {
        cols.push(Column::new("belief", Role::Belief, beliefs));
    }
    // #1172: a surprising observation's status (warn / bad)
    if checks.iter().any(|v| !matches!(v, Value::Na)) {
        cols.push(Column::new("check", Role::DiagramCheck, checks));
    }
    if sparks.iter().any(|v| !matches!(v, Value::Na)) {
        cols.push(Column::new(
            "trend",
            Role::Value(crate::Kind::Sparkline),
            sparks,
        ));
    }
    if values.iter().any(|v| !matches!(v, Value::Na)) {
        cols.push(Column::new("mean", Role::Metric(MetricFmt::Plain), values));
        cols.push(Column::new("sd", Role::Badge, badges));
    }
    // edges: factor → variable on `out`, variable → factor otherwise; a
    // folded variable's edge to its equality node goes
    // #1152: the message schedule as story steps along the edges, in
    // (iteration, step) order
    let schedule = list(options, "schedule")?;
    let num = |r: &Row, k: &str| get(r, &[k]).and_then(|s| s.parse::<f64>().ok());
    let mut order: Vec<&Row> = schedule.iter().collect();
    order.sort_by(|a, b| {
        let key = |r: &Row| {
            (
                num(r, "iteration").unwrap_or(0.0),
                num(r, "step").unwrap_or(0.0),
            )
        };
        let (ka, kb) = (key(a), key(b));
        ka.0.total_cmp(&kb.0).then(ka.1.total_cmp(&kb.1))
    });
    let shown = |id: &str| -> String {
        // a node's label as drawn (a folded variable shows on its equality node)
        let id = folded
            .iter()
            .find(|(_, e)| e == id)
            .map_or(id, |(v, _)| v.as_str());
        row_of(id).map(label_of).unwrap_or_else(|| id.to_string())
    };
    let at_iteration = |id: &str, it: f64| -> Option<String> {
        let id = folded
            .iter()
            .find(|(_, e)| e == id)
            .map_or(id, |(v, _)| v.as_str());
        marginals
            .iter()
            .filter(|m| get(m, &["variable"]).as_deref() == Some(id))
            .filter(|m| get(m, &["component"]).is_none())
            .find(|m| num(m, "iteration") == Some(it))
            .and_then(|m| {
                let mean = num(m, "mean")?;
                let f = |x: f64| {
                    crate::format::format_number_l(
                        x,
                        MetricFmt::Plain,
                        &crate::format::Locale::default(),
                    )
                };
                Some(match num(m, "sd") {
                    Some(sd) => format!("{} ± {}", f(mean), f(sd)),
                    None => f(mean),
                })
            })
    };
    let mut steps_of: Vec<((String, String), Vec<J>)> = Vec::new();
    for (i, r) in order.iter().enumerate() {
        let (Some(v), Some(f)) = (get(r, &["variable"]), get(r, &["factor"])) else {
            continue;
        };
        let iface = get(r, &["interface"]).unwrap_or_default();
        let to_factor = get(r, &["direction"]).is_some_and(|d| d.eq_ignore_ascii_case("to_factor"));
        let (src, dst) = if to_factor { (&v, &f) } else { (&f, &v) };
        let (drawn, back) = if let Some((_, from, to)) = forney_of(&v) {
            // along the variable's edge: forward when it runs from → to
            let forward = if to_factor { f == *to } else { f == *from };
            ((from.clone(), to.clone()), !forward)
        } else if is_folded(&v) {
            continue;
        } else {
            let drawn = if iface == "out" {
                (f.clone(), v.clone())
            } else {
                (v.clone(), f.clone())
            };
            let back = *src != drawn.0;
            (drawn, back)
        };
        let it = num(r, "iteration").unwrap_or(0.0);
        let mut t = format!(
            "Iteration {} · message {}: {} → {}",
            crate::format::format_number_l(it, MetricFmt::Plain, &crate::format::Locale::default()),
            num(r, "step").map_or(String::new(), |s| {
                crate::format::format_number_l(
                    s,
                    MetricFmt::Plain,
                    &crate::format::Locale::default(),
                )
            }),
            shown(src),
            shown(dst)
        );
        if !iface.is_empty() {
            t.push_str(&format!(" ({})", tex_lite(&iface)));
        }
        // #1172: what the message carries
        if let Some(m) = messages.iter().find(|m| {
            num(m, "iteration") == Some(it)
                && num(m, "step") == num(r, "step")
                && get(m, &["component"]).is_none()
        }) {
            let f = |x: f64| {
                crate::format::format_number_l(
                    x,
                    MetricFmt::Plain,
                    &crate::format::Locale::default(),
                )
            };
            if let Some(mean) = num(m, "mean").filter(|v| v.is_finite()) {
                t.push_str(&format!(
                    ", carrying {}{}",
                    get(m, &["family"])
                        .map(|fam| format!("{fam} "))
                        .unwrap_or_default(),
                    match num(m, "sd").filter(|v| v.is_finite()) {
                        Some(sd) => format!("{} ± {}", f(mean), f(sd)),
                        None => f(mean),
                    }
                ));
            }
        }
        if !to_factor {
            if let Some(m) = at_iteration(dst, it) {
                t.push_str(&format!(
                    "; {} after iteration {}: {m}",
                    shown(dst),
                    crate::format::format_number_l(
                        it,
                        MetricFmt::Plain,
                        &crate::format::Locale::default()
                    )
                ));
            }
        }
        let step = serde_json::json!({ "n": i, "t": t, "dir": if back { "back" } else { "forward" }, "i": it });
        match steps_of.iter_mut().find(|(k, _)| *k == drawn) {
            Some((_, v)) => v.push(step),
            None => steps_of.push((drawn, vec![step])),
        }
    }
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
            if let Some((_, st)) = steps_of.iter().find(|(k, _)| k.0 == from && k.1 == to) {
                o.insert("steps".into(), J::Array(st.clone()));
            }
            o.insert("from".into(), J::String(from));
            o.insert("to".into(), J::String(to));
            if !iface.is_empty() {
                o.insert("label".into(), J::String(tex_lite(&iface)));
            }
            Some(J::Object(o))
        })
        .chain(forney.iter().map(|(v, from, to)| {
            // the variable's edge, labelled with its name as mathematics
            let mut o = Map::new();
            if let Some((_, st)) = steps_of.iter().find(|(k, _)| k.0 == *from && k.1 == *to) {
                o.insert("steps".into(), J::Array(st.clone()));
            }
            o.insert("from".into(), J::String(from.clone()));
            o.insert("to".into(), J::String(to.clone()));
            o.insert("label".into(), J::String(shown(v)));
            o.insert("math".into(), J::Bool(true));
            J::Object(o)
        }))
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
        // #1166: each node's role in one sentence (its tooltip)
        let note = |id: &str| crate::value_text(&col(&cols, Role::Tooltip)[at(id)]);
        assert_eq!(
            note("f0"),
            "Prior (Beta): what we believe about θ before any data arrives."
        );
        assert!(
            note("f1").starts_with("Likelihood (Bernoulli): connects θ to what we measure, y₀."),
            "{}",
            note("f1")
        );
        assert!(
            note("e4").starts_with("θ, a hidden quantity shared by 3 factors"),
            "{}",
            note("e4")
        );
        assert_eq!(
            note("v0"),
            "Observed: a measured value, entering the model here."
        );
        // #1152: θ's posterior mean over the iterations, oldest first
        assert_eq!(
            col(&cols, Role::Value(crate::Kind::Sparkline))[at("e4")],
            Value::Str("[0.4,0.5]".into())
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

    /// #1152: the schedule becomes story steps on the drawn edges, in
    /// (iteration, step) order, `back` for a message against the drawn
    /// direction, the receiving variable's marginal in the caption.
    #[test]
    fn a_schedule_steps_along_the_edges() {
        let (nodes, mut options) = coin();
        options.insert(
            "schedule".into(),
            serde_json::json!([
                {"iteration": 2, "step": 1, "variable": "e4", "factor": "f1", "interface": "p", "direction": "to_factor"},
                {"iteration": 1, "step": 2, "variable": "e4", "factor": "f1", "interface": "p", "direction": "to_variable"},
                {"iteration": 1, "step": 1, "variable": "e4", "factor": "f0", "interface": "out", "direction": "to_variable"}
            ]),
        );
        let cols = columns(&nodes, &options).unwrap();
        let edges: J =
            serde_json::from_str(&crate::value_text(&col(&cols, Role::Edges)[0])).unwrap();
        let steps = |from: &str, to: &str| {
            edges
                .as_array()
                .unwrap()
                .iter()
                .find(|e| e["from"] == from && e["to"] == to)
                .map(|e| e["steps"].clone())
                .unwrap()
        };
        // Beta's out: drawn f0 → e4, the message runs that way: step 0
        let beta = steps("f0", "e4");
        assert_eq!(beta[0]["n"], 0);
        assert_eq!(beta[0]["dir"], "forward");
        assert!(
            beta[0]["t"]
                .as_str()
                .unwrap()
                .starts_with("Iteration 1 · message 1: Beta → θ (out)"),
            "{beta}"
        );
        // e4 → f1 drawn; to_variable runs back, iteration 1 step 2 is n 1
        let p = steps("e4", "f1");
        assert_eq!(
            (p[0]["n"].clone(), p[0]["dir"].clone()),
            (J::from(1), J::from("back"))
        );
        assert_eq!(
            (p[1]["n"].clone(), p[1]["dir"].clone()),
            (J::from(2), J::from("forward"))
        );
        // θ (folded into e4) gets iteration 1's marginal
        assert!(
            p[0]["t"]
                .as_str()
                .unwrap()
                .ends_with("; θ after iteration 1: 0.4 ± 0.2"),
            "{p}"
        );
    }

    /// #1172: θ's Beta(2, 3) prior from its constants, in the belief and
    /// in words on θ's card.
    #[test]
    fn the_prior_comes_from_the_constants() {
        let (mut nodes, mut options) = coin();
        nodes.push(row(&[("id", "c0"), ("kind", "constant"), ("name", "2")]));
        nodes.push(row(&[("id", "c1"), ("kind", "constant"), ("name", "3")]));
        let edges = options.get_mut("edges").unwrap().as_array_mut().unwrap();
        edges.push(
            serde_json::json!({"variable": "c1", "factor": "f0", "interface": "b", "seq": 2}),
        );
        edges.push(
            serde_json::json!({"variable": "c0", "factor": "f0", "interface": "a", "seq": 1}),
        );
        let cols = columns(&nodes, &options).unwrap();
        let ids: Vec<String> = col(&cols, Role::Diagram)
            .iter()
            .map(crate::value_text)
            .collect();
        let at = ids.iter().position(|i| i == "e4").unwrap();
        let b: J = serde_json::from_str(&crate::value_text(&col(&cols, Role::Belief)[at])).unwrap();
        let (m, sd) = (
            b["prior"][0].as_f64().unwrap(),
            b["prior"][1].as_f64().unwrap(),
        );
        assert!((m - 0.4).abs() < 1e-12 && (sd - 0.2).abs() < 1e-12, "{b}");
        // the range takes in the prior's ± 3 sd (−0.2), the posterior's
        assert!((b["lo"].as_f64().unwrap() + 0.2).abs() < 1e-9, "{b}");
        let note = crate::value_text(&col(&cols, Role::Tooltip)[at]);
        assert!(
            note.contains("2 observations narrowed θ from 0.4 ± 0.2 to 0.5 ± 0.1."),
            "{note}"
        );
        // #1172: the fixture's last iteration moved θ by 0.1 (sd 0.1)
        assert!(
            note.ends_with("Still moving: the last iteration changed it by 0.1."),
            "{note}"
        );
    }

    #[test]
    fn prior_moments_by_family() {
        let close = |a: Option<(f64, f64)>, b: (f64, f64)| {
            let a = a.unwrap();
            (a.0 - b.0).abs() < 1e-12 && (a.1 - b.1).abs() < 1e-12
        };
        assert!(close(
            moments("NormalMeanVariance", &[0.0, 100.0]),
            (0.0, 10.0)
        ));
        assert!(close(
            moments("NormalMeanPrecision", &[1.0, 4.0]),
            (1.0, 0.5)
        ));
        assert!(close(moments("GammaShapeRate", &[4.0, 2.0]), (2.0, 1.0)));
        assert!(close(moments("Gamma", &[4.0, 2.0]), (8.0, 4.0)));
        assert_eq!(moments("Beta", &[0.0, 1.0]), None);
        assert_eq!(moments("Wishart", &[1.0, 1.0]), None);
    }

    /// #1172 Forney style: a variable on two factors with no marginals is
    /// the edge between them, labelled with its name as mathematics, its
    /// messages along it; one with marginals stays a node.
    #[test]
    fn a_two_factor_variable_is_an_edge() {
        let nodes = vec![
            row(&[
                ("id", "f0"),
                ("kind", "factor"),
                ("name", "Categorical"),
                ("layer", "state"),
            ]),
            row(&[
                ("id", "f1"),
                ("kind", "factor"),
                ("name", "DiscreteTransition"),
                ("layer", "state"),
            ]),
            row(&[
                ("id", "s0"),
                ("kind", "variable"),
                ("label", "s_{0}"),
                ("layer", "state"),
            ]),
            row(&[
                ("id", "s1"),
                ("kind", "variable"),
                ("label", "s_{1}"),
                ("layer", "state"),
            ]),
        ];
        let mut options = Row::new();
        options.insert(
            "edges".into(),
            serde_json::json!([
                {"variable": "s0", "factor": "f0", "interface": "out"},
                {"variable": "s0", "factor": "f1", "interface": "in"},
                {"variable": "s1", "factor": "f1", "interface": "out"}
            ]),
        );
        options.insert(
            "marginals".into(),
            serde_json::json!([{"variable": "s1", "iteration": 1, "mean": 0.5, "sd": 0.1}]),
        );
        options.insert(
            "schedule".into(),
            serde_json::json!([
                {"iteration": 1, "step": 1, "variable": "s0", "factor": "f0", "interface": "out", "direction": "to_variable"},
                {"iteration": 1, "step": 2, "variable": "s0", "factor": "f1", "interface": "in", "direction": "to_factor"},
                {"iteration": 1, "step": 3, "variable": "s0", "factor": "f1", "interface": "in", "direction": "to_variable"}
            ]),
        );
        let cols = columns(&nodes, &options).unwrap();
        let ids: Vec<String> = col(&cols, Role::Diagram)
            .iter()
            .map(crate::value_text)
            .collect();
        assert!(!ids.contains(&"s0".to_string()), "s₀ is an edge: {ids:?}");
        assert!(ids.contains(&"s1".to_string()), "s₁ has marginals: a node");
        let edges: J =
            serde_json::from_str(&crate::value_text(&col(&cols, Role::Edges)[0])).unwrap();
        let e = edges
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["from"] == "f0" && e["to"] == "f1")
            .expect("the edge f0 → f1");
        assert_eq!(
            (e["label"].clone(), e["math"].clone()),
            (J::from("s₀"), J::from(true))
        );
        let dirs: Vec<&str> = e["steps"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["dir"].as_str().unwrap())
            .collect();
        assert_eq!(dirs, ["forward", "forward", "back"]);
    }

    /// #1172: the producer's tables in the cards — an observation far from
    /// its prediction is flagged, a variable's drivers are ranked, its share
    /// from the prior given for normal messages only, and a step says what
    /// its message carries.
    #[test]
    fn predictive_influence_and_messages_reach_the_cards() {
        let (mut nodes, mut options) = coin();
        // the Beta prior's constants
        nodes.push(row(&[("id", "c0"), ("kind", "constant"), ("name", "2")]));
        nodes.push(row(&[("id", "c1"), ("kind", "constant"), ("name", "3")]));
        let edges = options.get_mut("edges").unwrap().as_array_mut().unwrap();
        edges.push(
            serde_json::json!({"variable": "c0", "factor": "f0", "interface": "a", "seq": 1}),
        );
        edges.push(
            serde_json::json!({"variable": "c1", "factor": "f0", "interface": "b", "seq": 2}),
        );
        options.insert(
            "predictive".into(),
            serde_json::json!([
                {"variable": "v0", "observed": 1, "mean": 0.6, "sd": 0.49, "family": "Bernoulli"},
                {"variable": "v1", "observed": 9, "mean": 4.0, "sd": 1.0, "family": "Normal"}
            ]),
        );
        options.insert(
            "influence".into(),
            serde_json::json!([
                {"target": "v4", "observation": "v0", "mean": 0.5, "sd": 0.1, "loo_mean": 0.45, "loo_sd": 0.12},
                {"target": "v4", "observation": "v1", "mean": 0.5, "sd": 0.1, "loo_mean": 0.6, "loo_sd": 0.12}
            ]),
        );
        options.insert(
            "messages".into(),
            serde_json::json!([
                {"iteration": 1, "step": 1, "variable": "e4", "factor": "f0", "direction": "to_variable", "family": "Normal", "mean": 0.4, "sd": 0.2},
                {"iteration": 1, "step": 2, "variable": "e4", "factor": "f1", "direction": "to_variable", "family": "Normal", "mean": 1.0, "sd": 0.1},
                {"iteration": 1, "step": 3, "variable": "e4", "factor": "f2", "direction": "to_variable", "family": "Normal", "mean": 0.0, "sd": 0.1}
            ]),
        );
        options.insert(
            "schedule".into(),
            serde_json::json!([
                {"iteration": 1, "step": 2, "variable": "e4", "factor": "f1", "interface": "p", "direction": "to_variable"}
            ]),
        );
        let cols = columns(&nodes, &options).unwrap();
        let ids: Vec<String> = col(&cols, Role::Diagram)
            .iter()
            .map(crate::value_text)
            .collect();
        let at = |id: &str| ids.iter().position(|i| i == id).unwrap();
        let note = |id: &str| crate::value_text(&col(&cols, Role::Tooltip)[at(id)]);
        assert!(
            note("v0").ends_with("Expected 0.6 ± 0.49, measured 1."),
            "{}",
            note("v0")
        );
        assert!(
            note("v1").ends_with("Expected 4 ± 1, measured 9: 5 sd off, a surprise."),
            "{}",
            note("v1")
        );
        let check = |id: &str| crate::value_text(&col(&cols, Role::DiagramCheck)[at(id)]);
        assert_eq!(
            (check("v0"), check("v1")),
            (String::new(), "bad".to_string())
        );
        // θ: precisions 25 (the Beta prior f0) against 100 + 100
        let theta = note("e4");
        assert!(
            theta.contains("About 11 % of its certainty comes from the prior, 89 % from the data."),
            "{theta}"
        );
        assert!(
            theta.contains("Most influential: y₁ (without it 0.6), then y₀ (0.45)."),
            "{theta}"
        );
        let edges = crate::value_text(&col(&cols, Role::Edges)[0]);
        assert!(
            edges.contains("Bernoulli → θ (p), carrying Normal 1 ± 0.1"),
            "{edges}"
        );
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
    fn factors_read_as_glyphs() {
        assert_eq!(glyph("NormalMeanPrecision"), "𝒩");
        assert_eq!(glyph("GammaShapeRate"), "Γ");
        assert_eq!(glyph("DiscreteTransition"), "T");
        assert_eq!(glyph("MatrixDirichlet"), "Dir");
        assert_eq!(glyph("NormalMixture"), "NM");
        assert_eq!(glyph("SoftMaxGate"), "SMG");
        assert_eq!(glyph("Foo"), "Foo");
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
