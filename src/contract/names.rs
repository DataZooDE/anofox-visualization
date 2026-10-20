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
    "id",
    "index_name",
    "index_value",
    "window_end",
    "ds",
    "yhat",
    "yhat_lower",
    "yhat_upper",
    "level",
    "period",
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
    "kind",
    "label",
    "severity",
    "check_name",
    "status",
    "lower_threshold",
    "upper_threshold",
    "message",
    "identifier",
    "run_ts",
    "fold_id",
    "train_start",
    "train_end",
    "test_start",
    "test_end",
    "observed",
    "trend",
    "seasonal",
    "remainder",
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
    ("id", &["unique_id"]),
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

/// Read a producer's table as the contract's: statsforecast's wide forecast
/// columns are melted ([`melt_wide`]), an interval held as one `[low, high]`
/// column is split ([`split_intervals`]), the producer spellings are renamed
/// ([`rename`]) and `level` / `conf_level` read 95 as 0.95
/// ([`scale_levels`]). `(original, contract)` pairs go onto `renamed`;
/// `None` when nothing changes.
pub(super) fn normalize(
    rows: &[Row],
    options: &Row,
    renamed: &mut Vec<(String, String)>,
) -> Option<Vec<Row>> {
    let mut out = melt_wide(rows, renamed);
    if let Some(next) = split_intervals(out.as_deref().unwrap_or(rows), options, renamed) {
        out = Some(next);
    }
    if let Some(next) = rename(out.as_deref().unwrap_or(rows), options, renamed) {
        out = Some(next);
    }
    if let Some(next) = scale_levels(out.as_deref().unwrap_or(rows)) {
        out = Some(next);
    }
    out
}

/// Every key of `rows`, in first-seen order.
fn all_keys(rows: &[Row]) -> Vec<&String> {
    let mut keys: Vec<&String> = Vec::new();
    for r in rows {
        for k in r.keys() {
            if !keys.contains(&k) {
                keys.push(k);
            }
        }
    }
    keys
}

/// Does an option name the column `k` (`x := 'Price'`)?
fn named(options: &Row, k: &str) -> bool {
    options.values().any(|v| match v {
        J::String(s) => s == k,
        J::Array(a) => a.iter().any(|v| v.as_str() == Some(k)),
        _ => false,
    })
}

/// statsforecast / mlforecast / neuralforecast's wide forecast (`ds,
/// unique_id, AutoARIMA, AutoARIMA-lo-95, AutoARIMA-hi-95, AutoETS, …`) as the
/// long `prediction` schema: one row per model and level with `model_id`,
/// `yhat`, `yhat_lower`, `yhat_upper` and `level`. A model is a column with
/// `<model>-lo-<L>` / `<model>-hi-<L>` siblings; tables that already have
/// `yhat` or `model_id` are left alone.
fn melt_wide(rows: &[Row], renamed: &mut Vec<(String, String)>) -> Option<Vec<Row>> {
    let keys = all_keys(rows);
    if keys
        .iter()
        .any(|k| matches!(norm(k).as_str(), "yhat" | "model_id"))
    {
        return None;
    }
    // (model, level text, lo key, hi key)
    let mut bands: Vec<(&str, &str, String, String)> = Vec::new();
    for k in &keys {
        let Some((model, lvl)) = k.rsplit_once("-lo-") else {
            continue;
        };
        let hi = format!("{model}-hi-{lvl}");
        let model_col = keys.iter().find(|m| m.as_str() == model);
        if lvl.parse::<f64>().is_ok()
            && keys.iter().any(|m| **m == hi)
            && model_col.is_some_and(|m| !COLUMNS.contains(&norm(m).as_str()))
        {
            bands.push((model, lvl, k.to_string(), hi));
        }
    }
    let mut models: Vec<&str> = Vec::new();
    for (m, ..) in &bands {
        if !models.contains(m) {
            models.push(m);
        }
    }
    let first = bands.first()?;
    let wide = |k: &str| {
        models.contains(&k)
            || bands
                .iter()
                .any(|(_, _, lo, hi)| lo.as_str() == k || hi.as_str() == k)
    };
    let mut out = Vec::new();
    for r in rows {
        let base: Row = r
            .iter()
            .filter(|(k, _)| !wide(k))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        for m in &models {
            for (_, lvl, lo, hi) in bands.iter().filter(|b| b.0 == *m) {
                let mut row = base.clone();
                row.insert("model_id".into(), J::String(m.to_string()));
                row.insert("yhat".into(), r.get(*m).cloned().unwrap_or(J::Null));
                row.insert("yhat_lower".into(), r.get(lo).cloned().unwrap_or(J::Null));
                row.insert("yhat_upper".into(), r.get(hi).cloned().unwrap_or(J::Null));
                row.insert(
                    "level".into(),
                    serde_json::Number::from_f64(lvl.parse().unwrap_or(f64::NAN))
                        .map_or(J::Null, J::Number),
                );
                out.push(row);
            }
        }
    }
    renamed.push((first.0.to_string(), "yhat".into()));
    renamed.push((first.2.clone(), "yhat_lower".into()));
    renamed.push((first.3.clone(), "yhat_upper".into()));
    Some(out)
}

/// The level a single-column interval's name carries: `ci95` (pingouin's
/// `CI95%`) → `Some(Some(95))`, `conf_int` → `Some(None)`, other names
/// `None`.
fn interval_name(k: &str) -> Option<Option<f64>> {
    let n = norm(k);
    if matches!(
        n.as_str(),
        "conf_int" | "confint" | "ci" | "conf_interval" | "confidence_interval"
    ) {
        return Some(None);
    }
    let digits = n.strip_prefix("ci")?.trim_start_matches('_');
    digits.parse::<f64>().ok().filter(|l| *l > 0.0).map(Some)
}

/// The (low, high) of one interval cell: a two-element LIST (`[0.3, 0.7]`)
/// or a two-field STRUCT (`{lower: …, upper: …}`, `{0: …, 1: …}`).
fn pair(v: &J) -> Option<(J, J)> {
    match v {
        J::Array(a) if a.len() == 2 => Some((a[0].clone(), a[1].clone())),
        J::Object(o) if o.len() == 2 => {
            let side = |names: &[&str]| {
                o.iter()
                    .find(|(k, _)| names.contains(&norm(k).as_str()))
                    .map(|(_, v)| v.clone())
            };
            match (
                side(&["lower", "low", "lo", "conf_low", "0"]),
                side(&["upper", "high", "hi", "conf_high", "1"]),
            ) {
                (Some(l), Some(h)) => Some((l, h)),
                _ => {
                    let mut it = o.values().cloned();
                    Some((it.next()?, it.next()?))
                }
            }
        }
        _ => None,
    }
}

/// An interval held in one column — pingouin's `CI95%` LIST, statsmodels'
/// `conf_int()` as a STRUCT or LIST, a `conf_int` column — becomes
/// `conf_low` / `conf_high` (`yhat_lower` / `yhat_upper` next to `yhat`); a
/// level in the name (`CI95%`) fills `conf_level` / `level` when absent.
fn split_intervals(
    rows: &[Row],
    options: &Row,
    renamed: &mut Vec<(String, String)>,
) -> Option<Vec<Row>> {
    let keys = all_keys(rows);
    let prediction = keys.iter().any(|k| norm(k) == "yhat");
    let (lo, hi, lvl) = if prediction {
        ("yhat_lower", "yhat_upper", "level")
    } else {
        ("conf_low", "conf_high", "conf_level")
    };
    let present = |c: &str| keys.iter().any(|k| target(k, prediction) == Some(c));
    if present(lo) || present(hi) {
        return None;
    }
    let (key, level) = keys.iter().find_map(|k| {
        let level = interval_name(k)?;
        let cells: Vec<&J> = rows
            .iter()
            .filter_map(|r| r.get(k.as_str()))
            .filter(|v| !v.is_null())
            .collect();
        (!cells.is_empty() && cells.iter().all(|v| pair(v).is_some()) && !named(options, k))
            .then_some((k.to_string(), level))
    })?;
    let fill_level = level.filter(|_| !present(lvl));
    let out = rows
        .iter()
        .map(|r| {
            let mut m = Row::new();
            for (k, v) in r {
                if *k != key {
                    m.insert(k.clone(), v.clone());
                    continue;
                }
                let (l, h) = pair(v).unwrap_or((J::Null, J::Null));
                m.insert(lo.into(), l);
                m.insert(hi.into(), h);
                if let Some(level) = fill_level {
                    m.insert(
                        lvl.into(),
                        serde_json::Number::from_f64(level).map_or(J::Null, J::Number),
                    );
                }
            }
            m
        })
        .collect();
    renamed.push((key.clone(), lo.into()));
    renamed.push((key, hi.into()));
    Some(out)
}

/// `level` / `conf_level` as a fraction: 95 and 0.95 are the same level.
fn scale_levels(rows: &[Row]) -> Option<Vec<Row>> {
    let pct = |v: &J| {
        match v {
            J::Number(n) => n.as_f64(),
            J::String(s) => s.trim().parse().ok(),
            _ => None,
        }
        .filter(|f: &f64| *f > 1.0 && *f <= 100.0)
    };
    let hit = |r: &Row| {
        ["level", "conf_level"]
            .iter()
            .any(|k| r.get(*k).and_then(pct).is_some())
    };
    if !rows.iter().any(hit) {
        return None;
    }
    Some(
        rows.iter()
            .map(|r| {
                let mut m = r.clone();
                for k in ["level", "conf_level"] {
                    if let Some(f) = m.get(k).and_then(pct) {
                        m.insert(
                            k.into(),
                            serde_json::Number::from_f64(f / 100.0).map_or(J::Null, J::Number),
                        );
                    }
                }
                m
            })
            .collect(),
    )
}

/// Rename the producer spellings of contract columns (`coef` → `estimate`,
/// …), pushing `(original, contract)` onto `renamed`; `None` when nothing
/// changes. A column is kept as it is when the contract name is already
/// present (or claimed by an earlier column), and when an option names it
/// (`x := 'Price'`).
fn rename(rows: &[Row], options: &Row, renamed: &mut Vec<(String, String)>) -> Option<Vec<Row>> {
    let keys = all_keys(rows);
    let named = |k: &str| named(options, k);
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
    fn interval_lists_are_split() {
        // pingouin's CI95% LIST: the bounds, and the level from the name.
        let rows: Vec<Row> =
            serde_json::from_str(r#"[{"Contrast":"g","T":2.1,"p-unc":0.04,"CI95%":[0.1,0.9]}]"#)
                .unwrap();
        let mut renamed = Vec::new();
        let out = normalize(&rows, &Row::new(), &mut renamed).unwrap();
        assert_eq!(out[0]["conf_low"], 0.1);
        assert_eq!(out[0]["conf_high"], 0.9);
        assert_eq!(out[0]["conf_level"], 0.95);
        assert!(renamed.contains(&("CI95%".into(), "conf_low".into())));
        // statsmodels conf_int() as a STRUCT; next to yhat it bounds the forecast.
        let (k, _) = rename(
            r#"[{"term":"x","coef":1,"conf_int":{"0":0.5,"1":1.5}}]"#,
            "{}",
        );
        assert_eq!(k, ["term", "estimate", "conf_low", "conf_high"]);
        let (k, _) = rename(r#"[{"ds":1,"yhat":1,"ci":{"lower":0,"upper":2}}]"#, "{}");
        assert_eq!(k, ["ds", "yhat", "yhat_lower", "yhat_upper"]);
        // Bounds already present, or not pairs: left alone.
        let (k, _) = rename(
            r#"[{"term":"x","conf_low":0,"conf_high":1,"ci":[0,1]}]"#,
            "{}",
        );
        assert_eq!(k, ["term", "conf_low", "conf_high", "ci"]);
        let (k, _) = rename(r#"[{"term":"x","ci":[0,1,2]}]"#, "{}");
        assert_eq!(k, ["term", "ci"]);
    }

    #[test]
    fn levels_read_as_fractions() {
        let rows: Vec<Row> = serde_json::from_str(
            r#"[{"yhat":1,"level":95},{"yhat":1,"level":0.8},{"conf_level":"90"}]"#,
        )
        .unwrap();
        let out = normalize(&rows, &Row::new(), &mut Vec::new()).unwrap();
        assert_eq!(out[0]["level"], 0.95);
        assert_eq!(out[1]["level"], 0.8);
        assert_eq!(out[2]["conf_level"], 0.9);
    }

    #[test]
    fn statsforecast_wide_is_melted() {
        let rows: Vec<Row> = serde_json::from_str(
            r#"[{"unique_id":"A","ds":"2024-01-01","AutoARIMA":10,"AutoARIMA-lo-80":8,"AutoARIMA-hi-80":12,
                 "AutoARIMA-lo-95":7,"AutoARIMA-hi-95":13,"AutoETS":11,"AutoETS-lo-95":9,"AutoETS-hi-95":14}]"#,
        )
        .unwrap();
        let mut renamed = Vec::new();
        let out = normalize(&rows, &Row::new(), &mut renamed).unwrap();
        assert_eq!(out.len(), 3, "{out:?}");
        let keys: Vec<&String> = out[0].keys().collect();
        assert_eq!(
            keys,
            [
                "id",
                "ds",
                "model_id",
                "yhat",
                "yhat_lower",
                "yhat_upper",
                "level"
            ]
        );
        assert_eq!(out[0]["model_id"], "AutoARIMA");
        assert_eq!(out[0]["level"], 0.8);
        assert_eq!(out[1]["yhat_lower"], 7);
        assert_eq!(out[2]["model_id"], "AutoETS");
        assert_eq!(out[2]["yhat"], 11);
        assert!(renamed.contains(&("AutoARIMA".into(), "yhat".into())));
        // A table with yhat already, or bands without a model column: kept.
        let (k, _) = rename(r#"[{"yhat":1,"m-lo-95":0,"m-hi-95":2}]"#, "{}");
        assert_eq!(k, ["yhat", "m-lo-95", "m-hi-95"]);
        let (k, _) = rename(r#"[{"ds":1,"m-lo-95":0,"m-hi-95":2}]"#, "{}");
        assert_eq!(k, ["ds", "m-lo-95", "m-hi-95"]);
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
