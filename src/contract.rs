//! Contract plots — one chart per *shape* of model output
//! (`docs/plans/integration-contract.md`): `terms`, `prediction`, `curve`,
//! `summary`, `obs` (single diagnostic panels and the 2×2 `diagnostics`), and
//! an `auto` dispatch on the column signature.
//!
//! A spec `{"plot": "terms", "rows": [{term:…, estimate:…}, …], "options":
//! {…}, "width": W, "height": H}` is turned into role-annotated [`Column`]s —
//! exactly what a dashboard author would write by hand (`term::XAXIS,
//! estimate::SCATTER, conf_low::YMIN, …`) — and rendered by the ordinary role
//! renderer. The DuckDB macros (`anofox_plot_terms(tbl)`, …, see
//! [`crate::macros`]) aggregate a table's rows into such a spec, so they work
//! in the render-only community build.

use crate::{Column, Kind, Place, RenderError, RenderOptions, Role};
use ggplot_rs::prelude::Value;
use serde_json::{Map, Value as J};

type Row = Map<String, J>;

/// The plot names a spec's `"plot"` may carry.
pub const PLOTS: &[&str] = &[
    "terms",
    "prediction",
    "curve",
    "summary",
    "obs",
    "diagnostics",
    "auto",
];

/// The `obs` panel kinds (`options.kind`).
pub const OBS_KINDS: &[&str] = &["resid_fitted", "qq", "scale_location", "leverage", "cooks"];

/// Render a contract plot. `place` positions it (a nested fragment for the
/// 2×2 diagnostics).
pub(crate) fn render(
    plot: &str,
    rows: &[Row],
    options: &Row,
    width: u32,
    height: u32,
    o: &RenderOptions,
    place: Place,
) -> Result<(String, Vec<String>), RenderError> {
    let name = match plot {
        "auto" => detect(rows)?,
        p if PLOTS.contains(&p) => p,
        p => {
            return Err(RenderError::BadSpec(format!(
                "unknown plot '{p}' (one of {})",
                PLOTS.join(", ")
            )))
        }
    };
    if rows.is_empty() {
        let _ = o;
        return crate::note_svg(None, "No data", width, height)
            .finish(place)
            .map_err(RenderError::Render);
    }
    let fail = |e: String| RenderError::Render(format!("anofox_plot_{name}: {e}"));
    let t = Table { rows };
    let cols = match name {
        "terms" => terms(&t),
        "prediction" => prediction(&t, options),
        "curve" => curve(&t),
        "summary" => summary(&t),
        "obs" => {
            let kind = opt_str(options, "kind").unwrap_or_else(|| "resid_fitted".into());
            obs(&t, &kind, label_top(options))
        }
        "diagnostics" => {
            return crate::guard(|| {
                diagnostics(&t, label_top(options), width, height, o, place).map_err(fail)
            })
        }
        _ => unreachable!("checked above"),
    }
    .map_err(fail)?;
    crate::render_placed(&cols, place, width, height, o)
}

fn label_top(options: &Row) -> usize {
    options
        .get("label_top")
        .and_then(J::as_f64)
        .map(|k| k.clamp(0.0, 1000.0) as usize)
        .unwrap_or(3)
}

fn opt_str(options: &Row, key: &str) -> Option<String> {
    options
        .get(key)
        .and_then(J::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Which contract schema the rows follow (`anofox_plot(tbl)`).
fn detect(rows: &[Row]) -> Result<&'static str, RenderError> {
    let has = |k: &str| rows.iter().any(|r| r.contains_key(k));
    Ok(if has("term") && has("estimate") {
        "terms"
    } else if has("curve_type") {
        "curve"
    } else if has("yhat") {
        "prediction"
    } else if has("fitted") && has("residual") {
        "diagnostics"
    } else if has("metric") && has("value") {
        "summary"
    } else {
        let keys: Vec<&str> = rows
            .first()
            .map(|r| r.keys().map(String::as_str).collect())
            .unwrap_or_default();
        return Err(RenderError::Render(format!(
            "anofox_plot: the columns ({}) match no plottable schema — expected terms \
             (term, estimate), prediction (yhat), curve (curve_type, x, y), summary \
             (model_id, metric, value) or obs (fitted, residual)",
            keys.join(", ")
        )));
    })
}

/// Row access by contract column name.
struct Table<'a> {
    rows: &'a [Row],
}

impl Table<'_> {
    fn has(&self, k: &str) -> bool {
        self.rows
            .iter()
            .any(|r| r.get(k).is_some_and(|v| !v.is_null()))
    }
    fn keys(&self) -> String {
        self.rows
            .first()
            .map(|r| r.keys().cloned().collect::<Vec<_>>().join(", "))
            .unwrap_or_default()
    }
    fn need(&self, cols: &[&str]) -> Result<(), String> {
        let missing: Vec<&str> = cols.iter().copied().filter(|c| !self.has(c)).collect();
        if missing.is_empty() {
            Ok(())
        } else {
            Err(format!(
                "needs column(s) {} (got: {})",
                missing.join(", "),
                self.keys()
            ))
        }
    }
    /// Numbers (DuckDB writes DECIMAL as a string; booleans count 0/1).
    fn num(&self, k: &str) -> Result<Vec<Value>, String> {
        self.rows
            .iter()
            .map(|r| match r.get(k) {
                None | Some(J::Null) => Ok(Value::Na),
                Some(J::Number(n)) => Ok(n
                    .as_f64()
                    .filter(|f| f.is_finite())
                    .map_or(Value::Na, Value::Float)),
                Some(J::String(s)) => match s.trim().parse::<f64>() {
                    Ok(f) if f.is_finite() => Ok(Value::Float(f)),
                    Ok(_) => Ok(Value::Na),
                    Err(_) => Err(format!("column `{k}` must be numeric, got '{s}'")),
                },
                Some(J::Bool(b)) => Ok(Value::Float(*b as u8 as f64)),
                Some(other) => Err(format!("column `{k}` must be numeric, got {other}")),
            })
            .collect()
    }
    /// Text labels (numbers are formatted).
    fn text(&self, k: &str) -> Vec<Value> {
        self.rows
            .iter()
            .map(|r| match r.get(k) {
                None | Some(J::Null) => Value::Na,
                Some(J::String(s)) => Value::Str(s.clone()),
                Some(v) => Value::Str(v.to_string()),
            })
            .collect()
    }
    /// An x position: numbers stay numeric, ISO dates become datetimes, other
    /// text is discrete.
    fn xpos(&self, k: &str) -> Vec<Value> {
        let v: Vec<Value> = self
            .rows
            .iter()
            .map(|r| match r.get(k) {
                Some(J::Number(n)) => n.as_f64().map_or(Value::Na, Value::Float),
                Some(J::String(s)) => match s.trim().parse::<f64>() {
                    Ok(f) if f.is_finite() => Value::Float(f),
                    _ => Value::Str(s.clone()),
                },
                _ => Value::Na,
            })
            .collect();
        crate::sql::maybe_datetime(v)
    }
    /// Number of distinct non-null values of `k`.
    fn distinct(&self, k: &str) -> usize {
        let mut s = std::collections::HashSet::new();
        for r in self.rows {
            if let Some(v) = r.get(k).filter(|v| !v.is_null()) {
                s.insert(v.to_string());
            }
        }
        s.len()
    }
    /// The model key: `model_id`, else forecast's `model_name`.
    fn model_key(&self) -> Option<&'static str> {
        ["model_id", "model_name"].into_iter().find(|k| self.has(k))
    }
}

fn col(name: &str, role: Role, values: Vec<Value>) -> Column {
    Column::new(name, role, values)
}
fn konst(name: &str, role: Role, v: Value) -> Column {
    Column::new(name, role, vec![v])
}

/// `terms`: a coefficient forest (term on the vertical axis, estimate ±
/// interval, zero line, models dodged/coloured) — or, when `index_value` is
/// set, each term's estimate + band over the index (regularisation path,
/// quantile process, rolling coefficients), one panel per term.
fn terms(t: &Table) -> Result<Vec<Column>, String> {
    t.need(&["term", "estimate"])?;
    let model = t.model_key().filter(|k| t.distinct(k) > 1);
    let ci = t.has("conf_low") && t.has("conf_high");
    let mut cols = Vec::new();
    if t.has("index_value") {
        cols.push(col("index_value", Role::X, t.xpos("index_value")));
        cols.push(col("estimate", Role::Value(Kind::Line), t.num("estimate")?));
        if ci {
            cols.push(col("conf_low", Role::BandLower, t.num("conf_low")?));
            cols.push(col("conf_high", Role::BandUpper, t.num("conf_high")?));
        }
        if t.distinct("term") > 1 {
            cols.push(col("term", Role::FacetFree, t.text("term")));
        }
        if let Some(m) = model {
            cols.push(col(m, Role::Category, t.text(m)));
        }
        // A λ path reads on a log axis.
        let idx = t.text("index_name");
        let lambda = idx
            .iter()
            .any(|v| matches!(v, Value::Str(s) if s.to_ascii_lowercase().contains("lambda")));
        let positive = t
            .num("index_value")?
            .iter()
            .all(|v| v.as_f64().is_none_or(|f| f > 0.0));
        if lambda && positive {
            cols.push(konst("xs", Role::XScale, Value::Str("log10".into())));
        }
        cols.push(konst("zero", Role::RefLine, Value::Float(0.0)));
        if let Some(Value::Str(n)) = idx.iter().find(|v| **v != Value::Na) {
            cols.push(konst(
                "title",
                Role::Label,
                Value::Str(format!("estimate by {n}")),
            ));
        }
        return Ok(cols);
    }
    // Forest: the first term on top (a flipped axis grows upward).
    let rev = |v: Vec<Value>| v.into_iter().rev().collect::<Vec<_>>();
    cols.push(col("term", Role::X, rev(t.text("term"))));
    cols.push(col(
        "estimate",
        Role::Value(Kind::Point),
        rev(t.num("estimate")?),
    ));
    if ci {
        cols.push(col("conf_low", Role::YMin, rev(t.num("conf_low")?)));
        cols.push(col("conf_high", Role::YMax, rev(t.num("conf_high")?)));
    }
    if let Some(m) = model {
        cols.push(col(m, Role::Category, rev(t.text(m))));
    }
    cols.push(konst("flip", Role::Flip, Value::Float(1.0)));
    cols.push(konst("zero", Role::RefLine, Value::Float(0.0)));
    Ok(cols)
}

/// `prediction`: observed `y` points, the `yhat` line and its
/// `yhat_lower`/`yhat_upper` band over `x` (or `ds`), coloured by `split`
/// (train/test/future) — or by model when several models are present.
fn prediction(t: &Table, options: &Row) -> Result<Vec<Column>, String> {
    let x = opt_str(options, "x")
        .or_else(|| ["x", "ds"].into_iter().find(|k| t.has(k)).map(String::from))
        .ok_or_else(|| {
            format!(
                "needs an x position: a column `x` or `ds`, or x := '<column>' (got: {})",
                t.keys()
            )
        })?;
    let y = opt_str(options, "y").unwrap_or_else(|| "y".into());
    t.need(&[x.as_str(), "yhat"])?;
    let has_y = t.has(&y);
    let mut cols = vec![
        col(&x, Role::X, t.xpos(&x)),
        col("yhat", Role::Value(Kind::Line), t.num("yhat")?),
    ];
    if has_y {
        cols.push(col(&y, Role::Value(Kind::Point), t.num(&y)?));
    }
    if t.has("yhat_lower") && t.has("yhat_upper") {
        cols.push(col("yhat_lower", Role::BandLower, t.num("yhat_lower")?));
        cols.push(col("yhat_upper", Role::BandUpper, t.num("yhat_upper")?));
    }
    let facet = opt_str(options, "facet").or_else(|| (t.distinct("id") > 1).then(|| "id".into()));
    if let Some(f) = &facet {
        if !t.has(f) {
            return Err(format!("facet column `{f}` not found (got: {})", t.keys()));
        }
    }
    // Colour by model only when one panel holds several models (forecast's
    // model_name is often one per series, e.g. "AutoETS(A,A,N)").
    let model = t.model_key().filter(|m| {
        let mut per: std::collections::HashMap<String, std::collections::HashSet<String>> =
            std::collections::HashMap::new();
        for r in t.rows {
            let key = facet
                .as_deref()
                .and_then(|f| r.get(f))
                .map(|v| v.to_string())
                .unwrap_or_default();
            if let Some(v) = r.get(*m).filter(|v| !v.is_null()) {
                per.entry(key).or_default().insert(v.to_string());
            }
        }
        per.values().any(|s| s.len() > 1)
    });
    if let Some(m) = model {
        cols.push(col(m, Role::Category, t.text(m)));
    } else {
        // split, else from is_training, else: no y → future, y → train.
        let yv = if has_y { t.num(&y)? } else { Vec::new() };
        let split: Vec<Value> = t
            .rows
            .iter()
            .enumerate()
            .map(|(i, r)| {
                let s = match (r.get("split"), r.get("is_training")) {
                    (Some(J::String(s)), _) => s.clone(),
                    (_, Some(J::Bool(true))) => "train".into(),
                    (_, Some(J::Bool(false))) => {
                        if yv.get(i).is_some_and(|v| *v != Value::Na) {
                            "test".into()
                        } else {
                            "future".into()
                        }
                    }
                    _ if yv.get(i).is_some_and(|v| *v != Value::Na) => "train".into(),
                    _ => "future".into(),
                };
                Value::Str(s)
            })
            .collect();
        cols.push(col("split", Role::Category, split));
    }
    if let Some(f) = facet {
        cols.push(col(&f, Role::FacetFree, t.text(&f)));
    }
    Ok(cols)
}

/// `curve`: the geom and reference line follow `curve_type`.
fn curve(t: &Table) -> Result<Vec<Column>, String> {
    t.need(&["curve_type", "x", "y"])?;
    let types: std::collections::BTreeSet<String> = t
        .text("curve_type")
        .into_iter()
        .filter_map(|v| match v {
            Value::Str(s) => Some(s.to_ascii_lowercase()),
            _ => None,
        })
        .collect();
    if types.len() > 1 {
        return Err(format!(
            "rows mix curve_type {} — GROUP BY curve_type for one plot each",
            types.into_iter().collect::<Vec<_>>().join(", ")
        ));
    }
    let ty = types.into_iter().next().unwrap_or_default();
    let yv = t.num("y")?;
    let band = t.has("y_low") && t.has("y_high");
    let mut cols = vec![col("x", Role::X, t.xpos("x"))];
    let series = ["model_id", "series"]
        .into_iter()
        .find(|k| t.distinct(k) > 1);
    match ty.as_str() {
        "roc" | "calibration" => {
            cols.push(col("y", Role::Value(Kind::Line), yv));
            cols.push(konst("id", Role::Identity, Value::Float(1.0)));
        }
        "pr" | "pdp" | "lift" | "null_dist" => {
            cols.push(col("y", Role::Value(Kind::Line), yv));
            if band {
                cols.push(col("y_low", Role::BandLower, t.num("y_low")?));
                cols.push(col("y_high", Role::BandUpper, t.num("y_high")?));
            }
        }
        "km" => {
            // Step curve, confidence step ribbon, censoring marks.
            cols.push(col("y", Role::Value(Kind::Step), yv));
            if band {
                cols.push(col("y_low", Role::BandLower, t.num("y_low")?));
                cols.push(col("y_high", Role::BandUpper, t.num("y_high")?));
            }
            if let Some(c) = ["n_censor", "censored"].into_iter().find(|c| t.has(c)) {
                cols.push(col(c, Role::Censor, t.num(c)?));
            }
        }
        "acf" | "pacf" => {
            // Lollipops from 0, ± significance bounds from y_low / y_high.
            let zeros = vec![Value::Float(0.0); t.rows.len()];
            cols.push(col("y", Role::Value(Kind::Point), yv.clone()));
            cols.push(col("zero", Role::YMin, zeros));
            cols.push(col("y2", Role::YMax, yv));
            let mut refs = vec![0.0];
            if band {
                for v in t.num("y_low")?.into_iter().chain(t.num("y_high")?) {
                    if let Some(f) = v.as_f64() {
                        if !refs.iter().any(|r: &f64| (r - f).abs() < 1e-12) && refs.len() < 16 {
                            refs.push(f);
                        }
                    }
                }
            }
            cols.push(col(
                "bounds",
                Role::RefLine,
                refs.into_iter().map(Value::Float).collect(),
            ));
        }
        "lambda_cv" => {
            cols.push(col("y", Role::Value(Kind::Point), yv));
            if band {
                cols.push(col("y_low", Role::YMin, t.num("y_low")?));
                cols.push(col("y_high", Role::YMax, t.num("y_high")?));
            }
            if t.num("x")?
                .iter()
                .all(|v| v.as_f64().is_none_or(|f| f > 0.0))
            {
                cols.push(konst("xs", Role::XScale, Value::Str("log10".into())));
            }
        }
        "qq" => {
            // Precomputed (theoretical, sample) pairs: the producer's envelope
            // (y_low/y_high) is drawn as given — stat_qq_band would recompute
            // the theoretical quantiles for a normal reference only.
            cols.push(col("y", Role::Value(Kind::Point), yv));
            cols.push(konst("id", Role::Identity, Value::Float(1.0)));
            if band {
                cols.push(col("y_low", Role::BandLower, t.num("y_low")?));
                cols.push(col("y_high", Role::BandUpper, t.num("y_high")?));
            }
        }
        other => {
            return Err(format!(
                "unknown curve_type '{other}' (roc, pr, calibration, lambda_cv, km, acf, \
                 pacf, qq, null_dist, pdp, lift)"
            ))
        }
    }
    if let Some(s) = series {
        cols.push(col(s, Role::Category, t.text(s)));
    }
    let title = match ty.as_str() {
        "roc" => "ROC curve".to_string(),
        "pr" => "Precision-recall".into(),
        "calibration" => "Calibration".into(),
        "km" => "Kaplan-Meier survival".into(),
        "acf" => "Autocorrelation (ACF)".into(),
        "pacf" => "Partial autocorrelation (PACF)".into(),
        "lambda_cv" => "Cross-validation error by lambda".into(),
        "qq" => "Normal Q-Q".into(),
        "null_dist" => "Null distribution".into(),
        "pdp" => "Partial dependence".into(),
        "lift" => "Lift".into(),
        other => other.to_string(),
    };
    cols.push(konst("title", Role::Label, Value::Str(title)));
    Ok(cols)
}

/// `summary`: one panel per metric, a value dot (± interval) per model.
fn summary(t: &Table) -> Result<Vec<Column>, String> {
    let model = t.model_key().unwrap_or("model_id");
    t.need(&[model, "metric", "value"])?;
    let mut cols = vec![
        col(model, Role::X, t.text(model)),
        col("value", Role::Value(Kind::Point), t.num("value")?),
    ];
    if t.has("conf_low") && t.has("conf_high") {
        cols.push(col("conf_low", Role::YMin, t.num("conf_low")?));
        cols.push(col("conf_high", Role::YMax, t.num("conf_high")?));
    }
    // Free scales: every metric has its own unit. (Not flipped: 0.16's
    // coord_flip does not swap free facet scales.)
    cols.push(col("metric", Role::FacetFree, t.text("metric")));
    Ok(cols)
}

/// `obs`: one diagnostic panel of a fitted model's per-row output.
fn obs(t: &Table, kind: &str, k: usize) -> Result<Vec<Column>, String> {
    let model = t.model_key().filter(|m| t.distinct(m) > 1);
    let n = t.rows.len();
    let label = if t.has("row_id") { "row_id" } else { "" };
    let mut cols = Vec::new();
    // Standardised residuals, else residual / sd.
    let std_resid = |t: &Table| -> Result<Vec<Value>, String> {
        if t.has("std_residual") {
            return t.num("std_residual");
        }
        t.need(&["residual"])?;
        let r = t.num("residual")?;
        let v: Vec<f64> = r.iter().filter_map(|v| v.as_f64()).collect();
        let m = v.iter().sum::<f64>() / v.len().max(1) as f64;
        let sd =
            (v.iter().map(|x| (x - m).powi(2)).sum::<f64>() / (v.len().max(2) - 1) as f64).sqrt();
        Ok(r.iter()
            .map(|x| match x.as_f64() {
                Some(f) if sd > 0.0 => Value::Float(f / sd),
                _ => Value::Na,
            })
            .collect())
    };
    let top = |cols: &mut Vec<Column>, rank: Option<Vec<Value>>| {
        if !label.is_empty() && k > 0 {
            cols.push(col("row_id", Role::Label, t.text(label)));
            cols.push(konst("k", Role::LabelTop, Value::Float(k as f64)));
            if let Some(r) = rank {
                cols.push(col("rank", Role::Rank, r));
            }
        }
    };
    let cooks = if t.has("cooks_d") {
        Some(t.num("cooks_d")?)
    } else {
        None
    };
    match kind {
        "resid_fitted" => {
            t.need(&["fitted", "residual"])?;
            cols.push(col("fitted", Role::X, t.num("fitted")?));
            cols.push(col(
                "residual",
                Role::Value(Kind::Smooth),
                t.num("residual")?,
            ));
            cols.push(konst("zero", Role::RefLine, Value::Float(0.0)));
            top(&mut cols, cooks);
        }
        "qq" => {
            // Normal QQ with a 95 % pointwise envelope (see `render_qq`).
            cols.push(col("std_residual", Role::Value(Kind::QQ), std_resid(t)?));
            return Ok(cols);
        }
        "scale_location" => {
            t.need(&["fitted"])?;
            let s: Vec<Value> = std_resid(t)?
                .into_iter()
                .map(|v| {
                    v.as_f64()
                        .map_or(Value::Na, |f| Value::Float(f.abs().sqrt()))
                })
                .collect();
            cols.push(col("fitted", Role::X, t.num("fitted")?));
            cols.push(col("sqrt_abs_std_residual", Role::Value(Kind::Smooth), s));
        }
        "leverage" => {
            t.need(&["leverage"])?;
            cols.push(col("leverage", Role::X, t.num("leverage")?));
            cols.push(col("std_residual", Role::Value(Kind::Point), std_resid(t)?));
            cols.push(konst("zero", Role::RefLine, Value::Float(0.0)));
            // Cook's distance contours at 0.5 and 1 (plot.lm's which = 5).
            if let Some(p) = t.num("n_params")?.iter().find_map(|v| v.as_f64()) {
                cols.push(konst("p", Role::CooksContour, Value::Float(p)));
            }
            top(&mut cols, cooks);
        }
        "cooks" => {
            t.need(&["cooks_d"])?;
            let x = if t.has("row_id") {
                t.num("row_id")?
            } else {
                (1..=n).map(|i| Value::Float(i as f64)).collect()
            };
            let d = t.num("cooks_d")?;
            cols.push(col("row_id", Role::X, x));
            cols.push(col("cooks_d", Role::Value(Kind::Point), d.clone()));
            cols.push(col("zero", Role::YMin, vec![Value::Float(0.0); n]));
            cols.push(col("d", Role::YMax, d));
            // Cook's distance rule of thumb 4/n.
            cols.push(konst("cut", Role::RefLine, Value::Float(4.0 / n as f64)));
            top(&mut cols, None);
        }
        other => {
            return Err(format!(
                "kind '{other}' is not one of {}",
                OBS_KINDS.join(", ")
            ))
        }
    }
    if let Some(m) = model {
        cols.push(col(m, Role::Category, t.text(m)));
    }
    Ok(cols)
}

/// The plot.lm 2×2: residuals vs fitted, normal QQ, scale-location and
/// residuals vs leverage (Cook's distance when there is no leverage), as a
/// patchwork-style `PlotGrid` (one legend when several models are compared).
fn diagnostics(
    t: &Table,
    k: usize,
    width: u32,
    height: u32,
    o: &RenderOptions,
    place: Place,
) -> Result<(String, Vec<String>), String> {
    t.need(&["fitted", "residual"])?;
    let last = if t.has("leverage") {
        ("leverage", "Residuals vs leverage")
    } else if t.has("cooks_d") {
        ("cooks", "Cook's distance")
    } else {
        ("scale_location", "")
    };
    let mut panels = vec![
        ("resid_fitted", "Residuals vs fitted"),
        ("qq", "Normal Q-Q"),
        ("scale_location", "Scale-location"),
    ];
    if !last.1.is_empty() {
        panels.push(last);
    }
    let (w, h) = (width / 2, height / 2);
    let mut grid = ggplot_rs::compose::PlotGrid::new()
        .ncol(2)
        .collect_legends(true);
    for (kind, title) in &panels {
        let cols = obs(t, kind, k)?;
        grid = match crate::panel_plot(&cols, w, h, o)? {
            Some(plot) => grid.add(plot.title(title)),
            None => grid.add_spacer(),
        };
    }
    crate::finish_grid(grid, place, width, height)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(json: &str) -> Vec<Row> {
        serde_json::from_str(json).unwrap()
    }
    fn go(plot: &str, json: &str, opts: &str) -> Result<String, RenderError> {
        let o: Row = serde_json::from_str(opts).unwrap();
        render(
            plot,
            &rows(json),
            &o,
            640,
            400,
            &RenderOptions::default(),
            Place::Doc,
        )
        .map(|r| r.0)
    }

    #[test]
    fn detects_schemas() {
        let d = |j: &str| detect(&rows(j)).unwrap();
        assert_eq!(d(r#"[{"term":"a","estimate":1}]"#), "terms");
        assert_eq!(d(r#"[{"curve_type":"roc","x":0,"y":0}]"#), "curve");
        assert_eq!(d(r#"[{"ds":"2024-01-01","yhat":1}]"#), "prediction");
        assert_eq!(d(r#"[{"fitted":1,"residual":0}]"#), "diagnostics");
        assert_eq!(
            d(r#"[{"model_id":"a","metric":"r2","value":1}]"#),
            "summary"
        );
        assert!(detect(&rows(r#"[{"a":1}]"#)).is_err());
    }

    #[test]
    fn errors_are_clean() {
        let e = go("terms", r#"[{"term":"a"}]"#, "{}")
            .unwrap_err()
            .to_string();
        assert!(e.contains("needs column(s) estimate"), "{e}");
        let e = go("terms", r#"[{"term":"a","estimate":"big"}]"#, "{}")
            .unwrap_err()
            .to_string();
        assert!(e.contains("must be numeric"), "{e}");
        let e = go(
            "obs",
            r#"[{"fitted":1,"residual":0}]"#,
            r#"{"kind":"nope"}"#,
        )
        .unwrap_err()
        .to_string();
        assert!(e.contains("kind 'nope'"), "{e}");
        let e = go(
            "curve",
            r#"[{"curve_type":"roc","x":0,"y":0},{"curve_type":"pr","x":0,"y":1}]"#,
            "{}",
        )
        .unwrap_err()
        .to_string();
        assert!(e.contains("GROUP BY curve_type"), "{e}");
        assert!(go("nope", "[]", "{}").is_err());
    }

    #[test]
    fn empty_input_is_a_note() {
        let s = go("terms", "[]", "{}").unwrap();
        assert!(s.contains("No data"));
    }
}
