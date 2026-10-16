//! Column names: a producer's spelling of a contract column (statsmodels'
//! `coef` / `P>|t|` / `[0.025`, broom's `.fitted`, R's `Std. Error`, …) is
//! renamed once, before the schema lookup, so raw result tables plot without
//! a reshaping step. A missing-column error names the near misses ("did you
//! mean …") and the spellings that would have matched.

use super::Row;
use serde_json::{Map, Value as J};

/// Every column a contract schema reads (`docs/plans/integration-contract.md`).
/// A key that normalises onto one of these is renamed to it (`Estimate`,
/// `conf.low`, `.fitted`).
pub(super) const COLUMNS: &[&str] = &[
    "term",
    "estimate",
    "std_error",
    "statistic",
    "p_value",
    "p_adj",
    "conf_low",
    "conf_high",
    "conf_level",
    "model_id",
    "model_name",
    "index_name",
    "index_value",
    "window_end",
    "ds",
    "yhat",
    "yhat_lower",
    "yhat_upper",
    "split",
    "is_training",
    "focal",
    "focal_name",
    "curve_type",
    "y_low",
    "y_high",
    "series",
    "threshold",
    "weight",
    "auc",
    "n_censor",
    "censored",
    "n_risk",
    "n_event",
    "is_min",
    "is_1se",
    "lambda_min",
    "metric",
    "value",
    "test_id",
    "group1",
    "group2",
    "method",
    "partial_residual",
    "component",
    "x_residual",
    "y_residual",
    "fitted",
    "residual",
    "row_id",
    "std_residual",
    "leverage",
    "cooks_d",
    "n_params",
];

/// Other common spellings of a contract column, as [`norm`] writes them
/// (lowercase, any run of other characters → `_`). statsmodels' `[0.025` /
/// `0.975]` interval columns are matched by their brackets.
pub(super) const ALIASES: &[(&str, &[&str])] = &[
    ("estimate", &["coef", "params", "coefficient"]),
    ("std_error", &["std_err", "bse", "std_errors", "se"]),
    (
        "statistic",
        &[
            "t", "z", "tvalues", "zvalues", "t_value", "z_value", "t_stat",
        ],
    ),
    (
        "p_value",
        &[
            "p_t", "p_z", "pr_t", "pr_z", "p_val", "p_unc", "pvalues", "pval", "p",
        ],
    ),
    ("p_adj", &["p_corr", "padj", "p_adjusted"]),
    (
        "conf_low",
        &["ci_lower", "lower", "mean_ci_lower", "conf_lower"],
    ),
    (
        "conf_high",
        &["ci_upper", "upper", "mean_ci_upper", "conf_upper"],
    ),
    ("fitted", &["fittedvalues", "fitted_values"]),
    ("residual", &["resid", "residuals"]),
    ("std_residual", &["std_resid", "standard_resid"]),
    ("leverage", &["hat", "hat_diag"]),
    ("cooks_d", &["cooksd", "cooks_distance", "cook_d"]),
];

/// In a prediction table (one with `yhat`) an interval bounds the forecast.
const PREDICTION: &[(&str, &[&str])] = &[
    (
        "yhat_lower",
        &["lower", "mean_ci_lower", "ci_lower", "conf_low"],
    ),
    (
        "yhat_upper",
        &["upper", "mean_ci_upper", "ci_upper", "conf_high"],
    ),
];

/// Lowercase, every run of non-alphanumerics → `_`, no leading / trailing
/// `_`: `Std. Error` → `std_error`, `P>|t|` → `p_t`, `.fitted` → `fitted`.
pub(super) fn norm(k: &str) -> String {
    let mut s = String::with_capacity(k.len());
    for c in k.chars().flat_map(char::to_lowercase) {
        if c.is_alphanumeric() {
            s.push(c);
        } else if !s.is_empty() && !s.ends_with('_') {
            s.push('_');
        }
    }
    while s.ends_with('_') {
        s.pop();
    }
    s
}

/// The contract column `k` is read as, if any.
fn target(k: &str, prediction: bool) -> Option<&'static str> {
    let n = norm(k);
    let side = |lo: &'static str, hi: &'static str| {
        let k = k.trim();
        if k.starts_with('[') {
            Some(lo)
        } else if k.ends_with(']') {
            Some(hi)
        } else {
            None
        }
    };
    let find = |table: &[(&'static str, &[&str])]| {
        table
            .iter()
            .find(|(_, a)| a.contains(&n.as_str()))
            .map(|(c, _)| *c)
    };
    if prediction {
        if let Some(c) = side("yhat_lower", "yhat_upper").or_else(|| find(PREDICTION)) {
            return Some(c);
        }
    }
    COLUMNS
        .iter()
        .find(|c| **c == n)
        .copied()
        .or_else(|| side("conf_low", "conf_high"))
        .or_else(|| find(ALIASES))
}

/// Rename the producer spellings of contract columns (`coef` → `estimate`,
/// …), pushing `(original, contract)` onto `renamed`; `None` when nothing
/// changes. A column is kept as it is when the contract name is already
/// present (or claimed by an earlier column), and when an option names it
/// (`x := 'Price'`).
pub(super) fn normalize(
    rows: &[Row],
    options: &Row,
    renamed: &mut Vec<(String, String)>,
) -> Option<Vec<Row>> {
    let mut keys: Vec<&String> = Vec::new();
    for r in rows {
        for k in r.keys() {
            if !keys.contains(&k) {
                keys.push(k);
            }
        }
    }
    let named = |k: &str| {
        options.values().any(|v| match v {
            J::String(s) => s == k,
            J::Array(a) => a.iter().any(|v| v.as_str() == Some(k)),
            _ => false,
        })
    };
    let prediction = keys.iter().any(|k| norm(k) == "yhat");
    let mut map: Vec<(&String, &'static str)> = Vec::new();
    for k in &keys {
        let Some(c) = target(k, prediction) else {
            continue;
        };
        let taken = keys.iter().any(|o| o.as_str() == c) || map.iter().any(|(_, t)| *t == c);
        if k.as_str() != c && !taken && !named(k) {
            map.push((k, c));
        }
    }
    if map.is_empty() {
        return None;
    }
    let out = rows
        .iter()
        .map(|r| {
            r.iter()
                .map(|(k, v)| {
                    let k = map
                        .iter()
                        .find(|(o, _)| *o == k)
                        .map_or(k.as_str(), |(_, c)| c);
                    (k.to_string(), v.clone())
                })
                .collect::<Map<_, _>>()
        })
        .collect();
    for (o, c) in map {
        if !renamed.iter().any(|(_, t)| t == c) {
            renamed.push((o.clone(), c.to_string()));
        }
    }
    Some(out)
}

/// Edit distance (insertions, deletions, substitutions).
fn distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut cur = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            cur.push(
                (prev[j] + (ca != *cb) as usize)
                    .min(prev[j + 1] + 1)
                    .min(cur[j] + 1),
            );
        }
        prev = cur;
    }
    prev[b.len()]
}

/// The present column that looks like a misspelling of the contract column
/// `want` (or of one of its aliases of four letters or more): a small edit
/// distance, or `want` plus a suffix (`estimate_1`). Contract columns
/// themselves are not suggested.
pub(super) fn near<'a>(want: &str, keys: &[&'a str]) -> Option<&'a str> {
    let spellings: Vec<&str> = std::iter::once(want)
        .chain(
            ALIASES
                .iter()
                .filter(|(c, _)| *c == want)
                .flat_map(|(_, a)| a.iter().copied())
                .filter(|a| a.len() >= 4),
        )
        .collect();
    keys.iter()
        .copied()
        .filter(|k| !COLUMNS.contains(k))
        .filter_map(|k| {
            let n = norm(k);
            spellings
                .iter()
                .filter_map(|s| {
                    let d = distance(&n, s);
                    let close = s.len() >= 4 && d <= if s.len() >= 6 { 2 } else { 1 };
                    let prefix = s.len() >= 4 && n.starts_with(s);
                    (close || prefix).then_some(d)
                })
                .min()
                .map(|d| (d, k))
        })
        .min_by_key(|(d, _)| *d)
        .map(|(_, k)| k)
}

/// The other spellings read as `want`, for an error message.
pub(super) fn also(want: &str) -> Option<String> {
    let mut a: Vec<String> = ALIASES
        .iter()
        .filter(|(c, _)| *c == want)
        .flat_map(|(_, a)| a.iter().map(|s| s.to_string()))
        .collect();
    match want {
        "conf_low" => a.push("[0.025".into()),
        "conf_high" => a.push("0.975]".into()),
        _ => {}
    }
    (!a.is_empty()).then(|| a.join(", "))
}

/// The alias table of `docs/DOCS.md` (between the `contract-aliases`
/// markers).
#[cfg(test)]
pub(super) fn markdown_table() -> String {
    let mut s = String::from(
        "| Contract column | Also read from (case, `.`, spaces and symbols ignored) |\n|---|---|\n",
    );
    for (c, _) in ALIASES {
        let a = also(c).unwrap_or_default();
        let a: Vec<String> = a.split(", ").map(|x| format!("`{x}`")).collect();
        s.push_str(&format!("| `{c}` | {} |\n", a.join(", ")));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rename(json: &str, opts: &str) -> (Vec<String>, Vec<(String, String)>) {
        let rows: Vec<Row> = serde_json::from_str(json).unwrap();
        let o: Row = serde_json::from_str(opts).unwrap();
        let mut renamed = Vec::new();
        let out = normalize(&rows, &o, &mut renamed).unwrap_or(rows);
        (out[0].keys().cloned().collect(), renamed)
    }

    #[test]
    fn normalises_spellings() {
        assert_eq!(norm("Std. Error"), "std_error");
        assert_eq!(norm("P>|t|"), "p_t");
        assert_eq!(norm("Pr(>|t|)"), "pr_t");
        assert_eq!(norm(".fitted"), "fitted");
        assert_eq!(norm("std.err."), "std_err");
        assert_eq!(norm("p-unc"), "p_unc");
        assert_eq!(norm("conf.low"), "conf_low");
    }

    #[test]
    fn every_alias_is_read() {
        // Each alias, alone with the columns its schema needs, is renamed.
        for (c, aliases) in ALIASES {
            for a in *aliases {
                let (keys, renamed) = rename(&format!(r#"[{{"term":"x","{a}":1}}]"#), "{}");
                assert!(keys.contains(&c.to_string()), "{a} → {c}: {keys:?}");
                assert_eq!(renamed, vec![(a.to_string(), c.to_string())]);
            }
        }
    }

    #[test]
    fn producer_tables() {
        // statsmodels summary2().tables[1].reset_index()
        let (k, _) = rename(
            r#"[{"index":"x1","Coef.":0.5,"Std.Err.":0.1,"t":5.0,"P>|t|":0.001,"[0.025":0.3,"0.975]":0.7}]"#,
            "{}",
        );
        assert_eq!(
            k,
            [
                "index",
                "estimate",
                "std_error",
                "statistic",
                "p_value",
                "conf_low",
                "conf_high"
            ]
        );
        // statsmodels summary_frame() / params / bse / tvalues / pvalues
        let (k, _) = rename(
            r#"[{"term":"x1","params":0.5,"bse":0.1,"tvalues":5,"pvalues":0.01}]"#,
            "{}",
        );
        assert_eq!(k, ["term", "estimate", "std_error", "statistic", "p_value"]);
        // R summary(lm)$coefficients
        let (k, _) = rename(
            r#"[{"term":"x1","Estimate":0.5,"Std. Error":0.1,"t value":5,"Pr(>|t|)":0.01}]"#,
            "{}",
        );
        assert_eq!(k, ["term", "estimate", "std_error", "statistic", "p_value"]);
        // broom::tidy / augment
        let (k, _) = rename(
            r#"[{"term":"x1","estimate":0.5,"std.error":0.1,"p.value":0.01,"conf.low":0.3,"conf.high":0.7}]"#,
            "{}",
        );
        assert_eq!(
            k,
            [
                "term",
                "estimate",
                "std_error",
                "p_value",
                "conf_low",
                "conf_high"
            ]
        );
        let (k, _) = rename(
            r#"[{".fitted":1,".resid":0.1,".hat":0.2,".cooksd":0.01,".std.resid":0.3}]"#,
            "{}",
        );
        assert_eq!(
            k,
            ["fitted", "residual", "leverage", "cooks_d", "std_residual"]
        );
        // pingouin
        let (k, _) = rename(r#"[{"T":2.1,"p-val":0.04,"p-corr":0.08}]"#, "{}");
        assert_eq!(k, ["statistic", "p_value", "p_adj"]);
        // A prediction: the interval bounds the forecast.
        let (k, _) = rename(
            r#"[{"ds":"2024-01-01","yhat":1,"mean_ci_lower":0,"mean_ci_upper":2}]"#,
            "{}",
        );
        assert_eq!(k, ["ds", "yhat", "yhat_lower", "yhat_upper"]);
    }

    #[test]
    fn keeps_what_it_must() {
        // The contract name already present wins; the alias stays as it is.
        let (k, r) = rename(r#"[{"term":"a","estimate":1,"coef":2}]"#, "{}");
        assert_eq!(k, ["term", "estimate", "coef"]);
        assert!(r.is_empty());
        // Two aliases of one column: the first is read.
        let (k, _) = rename(r#"[{"t":1,"z":2}]"#, "{}");
        assert_eq!(k, ["statistic", "z"]);
        // A column an option names is not renamed.
        let (k, _) = rename(r#"[{"Fitted":1,"yhat":1}]"#, r#"{"x":"Fitted"}"#);
        assert_eq!(k, ["Fitted", "yhat"]);
        // Other columns keep their spelling.
        let (k, _) = rename(r#"[{"Price":1,"yhat":1}]"#, "{}");
        assert_eq!(k, ["Price", "yhat"]);
    }

    #[test]
    fn suggests_near_misses() {
        assert_eq!(near("estimate", &["term", "estimat"]), Some("estimat"));
        assert_eq!(
            near("estimate", &["term", "estimate_1"]),
            Some("estimate_1")
        );
        assert_eq!(near("p_value", &["p_values", "x"]), Some("p_values"));
        assert_eq!(near("estimate", &["coeff"]), Some("coeff"));
        assert_eq!(near("estimate", &["term", "beta"]), None);
        // A contract column is not a misspelling of another.
        assert_eq!(near("fitted", &["residual"]), None);
        // Short names are not guessed at.
        assert_eq!(near("x", &["a"]), None);
    }
}
