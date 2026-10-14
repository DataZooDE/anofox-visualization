//! Contract plots — one chart per *shape* of model output
//! (`docs/plans/integration-contract.md`): `terms`, `prediction`, `curve`,
//! `summary`, `obs` (single diagnostic panels and the 2×2 `diagnostics`),
//! `test` (`tests`: a comparison forest; `compare`: groups with significance
//! brackets), partial-residual plots (`partial`) and an `auto` dispatch on the
//! column signature.
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

mod stats;

type Row = Map<String, J>;

/// The anofox integration-contract version these plots implement
/// (`anofox_contract_version()`).
pub const CONTRACT_VERSION: &str = "1";

/// The plot names a spec's `"plot"` may carry.
pub const PLOTS: &[&str] = &[
    "terms",
    "prediction",
    "curve",
    "summary",
    "obs",
    "diagnostics",
    "tests",
    "compare",
    "partial",
    "auto",
];

/// The `obs` panel kinds (`options.kind`).
pub const OBS_KINDS: &[&str] = &["resid_fitted", "qq", "scale_location", "leverage", "cooks"];

/// Render a contract plot. `place` positions it (a nested fragment for the
/// 2×2 diagnostics). This is the single entry point for every plot kind:
/// `plot` is the macro's kind (`anofox_plot_<plot>`), `options` its named
/// arguments.
pub(crate) fn render(
    plot: &str,
    rows: &[Row],
    options: &Row,
    width: u32,
    height: u32,
    o: &RenderOptions,
    place: Place,
) -> Result<(String, Vec<String>), RenderError> {
    // A column holding the test STRUCT (or LIST of them) of a `*_agg`
    // aggregate: one row per test.
    let flat;
    let rows = match plot {
        "tests" | "auto" => match flatten(rows) {
            Some(f) if plot == "tests" || detect(rows).is_err() => {
                flat = f;
                &flat[..]
            }
            _ => rows,
        },
        _ => rows,
    };
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
    let fail = |e: String| {
        RenderError::Render(if plot == "auto" {
            format!("anofox_plot (anofox contract v{CONTRACT_VERSION}, drawn as {name}): {e}")
        } else {
            format!("anofox_plot_{name}: {e}")
        })
    };
    let t = Table { rows };
    let (w, h) = (
        crate::clamp_dim(width as u64),
        crate::clamp_dim(height as u64),
    );
    // Plots drawn directly with ggplot-rs (see `stats`).
    let direct = |f: &dyn Fn() -> stats::Out| crate::guard(|| f().map_err(fail));
    let cols = match name {
        "terms" => terms(&t),
        "prediction" => match prediction(&t, options).map_err(fail)? {
            Pred::Cols(c) => Ok(c),
            Pred::Models { x, y, model, facet } => {
                return direct(&|| {
                    stats::prediction_models(&t, &x, &y, model, facet.as_deref(), w, h, o, place)
                })
            }
        },
        "curve" => match curve_type(&t).map_err(fail)?.as_str() {
            "km" => return direct(&|| stats::km(&t, w, h, o, place)),
            "lambda_cv" => return direct(&|| stats::lambda_cv(&t, w, h, o, place)),
            _ => curve(&t),
        },
        "summary" => return direct(&|| stats::summary(&t, options, w, h, o, place)),
        "tests" => return direct(&|| stats::tests(&t, options, w, h, o, place)),
        "compare" => return direct(&|| stats::compare(&t, options, w, h, o, place)),
        "partial" => return direct(&|| stats::partial(&t, options, w, h, o, place)),
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
    let (svg, mut warnings) = crate::render_placed(&cols, place, width, height, o)?;
    if name == "prediction" {
        // A missing y (future rows) or yhat (history rows) is part of the
        // schema, not lost data: the point layer skipping them is no warning.
        warnings.retain(|w| !w.contains("non-finite"));
    }
    Ok((svg, warnings))
}

/// Rows whose only nested column is a STRUCT, or a LIST of STRUCTs (a test
/// aggregate's result: `SELECT grp, tukey_hsd_agg(y, g) AS r … GROUP BY
/// grp`), expanded to one row per STRUCT with the other columns repeated.
fn flatten(rows: &[Row]) -> Option<Vec<Row>> {
    let nested = |v: &J| match v {
        J::Object(_) => true,
        J::Array(a) => !a.is_empty() && a.iter().all(J::is_object),
        _ => false,
    };
    let first = rows.first()?;
    let keys: Vec<&String> = first
        .iter()
        .filter(|(_, v)| nested(v))
        .map(|(k, _)| k)
        .collect();
    let [key] = keys.as_slice() else {
        return None;
    };
    let mut out = Vec::new();
    for r in rows {
        let base: Row = r
            .iter()
            .filter(|(k, _)| k != key)
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let items: Vec<&Row> = match r.get(*key) {
            Some(J::Object(o)) => vec![o],
            Some(J::Array(a)) => a.iter().filter_map(J::as_object).collect(),
            _ => Vec::new(),
        };
        for it in items {
            let mut m = base.clone();
            for (k, v) in it {
                m.insert(k.clone(), v.clone());
            }
            out.push(m);
        }
    }
    Some(out)
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
    } else if has("partial_residual") || has("x_residual") && has("y_residual") {
        "partial"
    } else if has("p_value") || has("test_id") || has("statistic") && has("method") {
        "tests"
    } else {
        let keys: Vec<&str> = rows
            .first()
            .map(|r| r.keys().map(String::as_str).collect())
            .unwrap_or_default();
        return Err(RenderError::Render(format!(
            "anofox_plot (anofox contract v{CONTRACT_VERSION}): the columns ({}) match no \
             plottable schema — expected terms (term, estimate), prediction (yhat), curve \
             (curve_type, x, y), summary (model_id, metric, value), obs (fitted, residual), \
             test (p_value / statistic, method) or partial residuals (term, x, \
             partial_residual)",
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
    // Several models and/or several groups (`id`): colour by what varies; a
    // forest with both puts each group in its own panel.
    let model_key = t.model_key().filter(|k| t.distinct(k) > 1);
    let ids = t.has("id") && t.distinct("id") > 1;
    let ci = t.has("conf_low") && t.has("conf_high");
    let series: Option<(String, Vec<Value>)> = match (model_key, ids) {
        (Some(m), true) if t.has("index_value") => Some((
            "model".into(),
            t.text(m)
                .into_iter()
                .zip(t.text("id"))
                .map(|(a, b)| match (a, b) {
                    (Value::Str(a), Value::Str(b)) => Value::Str(format!("{a} · {b}")),
                    (a, _) => a,
                })
                .collect(),
        )),
        (Some(m), _) => Some((m.to_string(), t.text(m))),
        (None, true) => Some(("id".into(), t.text("id"))),
        (None, false) => None,
    };
    let forest_facet = model_key.is_some() && ids;
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
        if let Some((name, v)) = &series {
            cols.push(col(name, Role::Category, v.clone()));
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
            // LARS reports upstream's alpha, not glmnet's lambda: say so.
            let title = if n.eq_ignore_ascii_case("alpha") {
                "estimate by alpha (LARS; not on the glmnet lambda scale)".to_string()
            } else {
                format!("estimate by {n}")
            };
            cols.push(konst("title", Role::Label, Value::Str(title)));
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
    if let Some((name, v)) = series {
        let (name, v) = if forest_facet {
            let m = model_key.unwrap_or("model_id");
            cols.push(col("id", Role::Facet, rev(t.text("id"))));
            (m.to_string(), t.text(m))
        } else {
            (name, v)
        };
        cols.push(col(&name, Role::Category, rev(v)));
    }
    cols.push(konst("flip", Role::Flip, Value::Float(1.0)));
    cols.push(konst("zero", Role::RefLine, Value::Float(0.0)));
    Ok(cols)
}

/// `prediction`: observed `y` points, the `yhat` line and its
/// `yhat_lower`/`yhat_upper` band over `x` (or `ds`), coloured by `split`
/// (train/test/future) — or by model when several models are present.
enum Pred {
    Cols(Vec<Column>),
    /// Several models share a panel: drawn by [`stats::prediction_models`].
    Models {
        x: String,
        y: String,
        model: &'static str,
        facet: Option<String>,
    },
}

fn prediction(t: &Table, options: &Row) -> Result<Pred, String> {
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
        return Ok(Pred::Models {
            x,
            y,
            model: m,
            facet,
        });
    } else if !t.has("focal") {
        // (An effects table — `focal` — has no train/test split to show.)
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
        // One split (all train, all future): a legend of one says nothing.
        let one = split.windows(2).all(|w| w[0] == w[1]);
        if !one {
            cols.push(col("split", Role::Category, split));
        }
    }
    if t.has("focal") {
        let f = t
            .num("focal")?
            .iter()
            .find_map(|v| v.as_f64())
            .map(|f| format!("marginal effect of feature {f}"))
            .unwrap_or_else(|| "marginal effect".into());
        cols.push(konst("title", Role::Label, Value::Str(f)));
    }
    if let Some(f) = facet {
        cols.push(col(&f, Role::FacetFree, t.text(&f)));
    }
    Ok(Pred::Cols(cols))
}

/// The one `curve_type` of a curve table (an error when rows mix types).
fn curve_type(t: &Table) -> Result<String, String> {
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
    Ok(types.into_iter().next().unwrap_or_default())
}

/// `curve`: the geom and reference line follow `curve_type` (km and
/// lambda_cv are drawn by [`stats`]).
fn curve(t: &Table) -> Result<Vec<Column>, String> {
    let ty = curve_type(t)?;
    let yv = t.num("y")?;
    let band = t.has("y_low") && t.has("y_high");
    let mut cols = vec![col("x", Role::X, t.xpos("x"))];
    let series = ["model_id", "series", "id"]
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
            // Several series' lollipops at the same lags would overlap: one
            // panel each.
            if let Some(s) = series {
                cols.push(col(s, Role::FacetFree, t.text(s)));
                cols.push(konst(
                    "title",
                    Role::Label,
                    Value::Str(match ty.as_str() {
                        "acf" => "Autocorrelation (ACF)".into(),
                        _ => "Partial autocorrelation (PACF)".into(),
                    }),
                ));
                return Ok(cols);
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
    fn detects_tests_and_partial() {
        let d = |j: &str| detect(&rows(j)).unwrap();
        assert_eq!(d(r#"[{"test_id":"b-a","p_value":0.1}]"#), "tests");
        assert_eq!(
            d(r#"[{"method":"Durbin-Watson","statistic":2.1}]"#),
            "tests"
        );
        assert_eq!(
            d(r#"[{"term":"x1","x":1,"partial_residual":0.2}]"#),
            "partial"
        );
        let e = detect(&rows(r#"[{"a":1}]"#)).unwrap_err().to_string();
        assert!(e.contains("contract v1"), "{e}");
    }

    const TUKEY: &str = r#"[
        {"grp":"A","test_id":"b-a","group1":"a","group2":"b","estimate":1.2,"conf_low":0.4,"conf_high":2.0,"p_adj":0.003,"method":"Tukey"},
        {"grp":"A","test_id":"c-a","group1":"a","group2":"c","estimate":0.3,"conf_low":-0.5,"conf_high":1.1,"p_adj":0.41,"method":"Tukey"},
        {"grp":"B","test_id":"b-a","group1":"a","group2":"b","estimate":0.2,"conf_low":-0.4,"conf_high":0.9,"p_adj":0.6,"method":"Tukey"}]"#;

    #[test]
    fn tests_forest() {
        let svg = go("tests", TUKEY, "{}").unwrap();
        assert!(svg.contains("p = 0.003 **"), "p annotation");
        assert!(svg.contains("p ≤ 0.05"), "significance legend");
        // Two groups (grp) → a grid with one panel per group.
        assert!(svg.matches("<svg").count() > 2);
        // One test without an estimate: the statistic as a lollipop.
        let svg = go(
            "tests",
            r#"[{"grp":"A","statistic":2.1,"p_value":0.04,"method":"Shapiro-Wilk"},
                {"grp":"B","statistic":1.1,"p_value":0.4,"method":"Shapiro-Wilk"}]"#,
            r#"{"alpha":0.01}"#,
        )
        .unwrap();
        assert!(svg.contains("Shapiro-Wilk") && svg.contains("p = 0.04 *"));
        let e = go("tests", r#"[{"method":"m"}]"#, "{}")
            .unwrap_err()
            .to_string();
        assert!(e.contains("no estimate, statistic or p_value"), "{e}");
        // A column holding the aggregate's LIST of STRUCTs is expanded.
        let nested = format!(r#"[{{"r":{TUKEY}}}]"#);
        assert!(go("tests", &nested, "{}").is_ok());
        assert!(go("auto", &nested, "{}").is_ok());
    }

    #[test]
    fn compare_brackets() {
        let data = r#"[{"g":"a","y":1.0},{"g":"a","y":1.4},{"g":"a","y":0.8},
            {"g":"b","y":2.1},{"g":"b","y":2.6},{"g":"b","y":2.2},
            {"g":"c","y":1.1},{"g":"c","y":1.5},{"g":"c","y":1.2}]"#;
        let tests = format!(r#"{{"value":"y","factor":"g","tests":{TUKEY}}}"#);
        let svg = go("compare", data, &tests).unwrap();
        assert!(
            svg.contains("p = 0.003 **") && svg.contains("data-series=\"b-a\""),
            "bracket + label"
        );
        // A two-sample STRUCT for two groups.
        let two = r#"[{"g":"a","y":1.0},{"g":"a","y":1.4},{"g":"b","y":2.1},{"g":"b","y":2.6}]"#;
        let svg = go(
            "compare",
            two,
            r#"{"value":"y","factor":"g","kind":"violin","tests":{"p_value":0.02,"method":"Welch t-test"}}"#,
        )
        .unwrap();
        assert!(svg.contains("Welch t-test") && svg.contains("p = 0.02 *"));
        let e = go("compare", two, r#"{"value":"y"}"#)
            .unwrap_err()
            .to_string();
        assert!(e.contains("factor :="), "{e}");
        // Tests naming other groups: a warning, not silence.
        let r = crate::render_spec_checked(&format!(
            r#"{{"plot":"compare","rows":{two},"options":{{"value":"y","factor":"g","tests":[{{"group1":"a","group2":"z","p_adj":0.1}}]}}}}"#
        ))
        .unwrap();
        assert!(
            r.contains("data-warnings") && r.contains("z"),
            "unknown group warned"
        );
    }

    #[test]
    fn partial_and_summary() {
        let pr = r#"[{"term":"x1","x":1,"partial_residual":0.4,"component":0.5,"x_residual":-1,"y_residual":-0.4},
            {"term":"x1","x":2,"partial_residual":1.1,"component":1.0,"x_residual":0,"y_residual":0.1},
            {"term":"x1","x":3,"partial_residual":1.4,"component":1.5,"x_residual":1,"y_residual":0.5},
            {"term":"x2","x":1,"partial_residual":0.1,"component":0.2,"x_residual":-1,"y_residual":0.2},
            {"term":"x2","x":2,"partial_residual":0.3,"component":0.4,"x_residual":0,"y_residual":0.0},
            {"term":"x2","x":3,"partial_residual":0.7,"component":0.6,"x_residual":1,"y_residual":-0.1}]"#;
        assert!(go("partial", pr, "{}")
            .unwrap()
            .contains("Component + residual"));
        let av = go("partial", pr, r#"{"kind":"added_variable"}"#).unwrap();
        assert!(av.contains("slope"), "slope in the panel title");
        assert!(go("partial", pr, r#"{"kind":"nope"}"#).is_err());
        let glance = r#"[{"model_id":"ols","metric":"r_squared","value":0.81},
            {"model_id":"ridge","metric":"r_squared","value":0.79},
            {"model_id":"ols","metric":"aic","value":72.4},
            {"model_id":"ridge","metric":"aic","value":75.0},
            {"model_id":"ols","metric":"n_observations","value":30},
            {"model_id":"ridge","metric":"n_observations","value":30}]"#;
        let svg = go("summary", glance, "{}").unwrap();
        assert!(svg.contains("R²") && svg.contains("AIC") && !svg.contains("n observations"));
        let svg = go("summary", glance, r#"{"metrics":["n_observations"]}"#).unwrap();
        assert!(svg.contains(">30<"), "asked-for bookkeeping metric");
        let e = go("summary", glance, r#"{"metrics":["auc"]}"#)
            .unwrap_err()
            .to_string();
        assert!(e.contains("matches none"), "{e}");
    }

    #[test]
    fn prediction_models_and_curves() {
        let p = r#"[{"x":1,"y":1.0,"yhat":1.1,"yhat_lower":0.8,"yhat_upper":1.4,"model_id":"ols","split":"train"},
            {"x":2,"y":2.0,"yhat":1.9,"yhat_lower":1.6,"yhat_upper":2.2,"model_id":"ols","split":"test"},
            {"x":1,"y":1.0,"yhat":1.0,"yhat_lower":0.7,"yhat_upper":1.3,"model_id":"huber","split":"train"},
            {"x":2,"y":2.0,"yhat":2.0,"yhat_lower":1.7,"yhat_upper":2.3,"model_id":"huber","split":"test"}]"#;
        let svg = go("prediction", p, "{}").unwrap();
        assert!(
            svg.contains("huber") && svg.contains("ols"),
            "models in the legend"
        );
        let km = r#"[{"curve_type":"km","model_id":"A","x":2,"y":0.9,"y_low":0.8,"y_high":1.0,"n_censor":0,"n_risk":10},
            {"curve_type":"km","model_id":"A","x":5,"y":0.5,"y_low":null,"y_high":null,"n_censor":1,"n_risk":4},
            {"curve_type":"km","model_id":"B","x":3,"y":0.7,"y_low":0.5,"y_high":0.9,"n_censor":0,"n_risk":8}]"#;
        let r = crate::render_spec_checked(&format!(r#"{{"plot":"curve","rows":{km}}}"#)).unwrap();
        assert!(
            r.contains("Kaplan-Meier") && !r.contains("data-warnings"),
            "{r}"
        );
        let cv = r#"[{"curve_type":"lambda_cv","x":0.1,"y":2.0,"y_low":1.8,"y_high":2.2,"is_min":true,"is_1se":false},
            {"curve_type":"lambda_cv","x":1.0,"y":2.1,"y_low":1.9,"y_high":2.3,"is_min":false,"is_1se":true},
            {"curve_type":"lambda_cv","x":10.0,"y":3.0,"y_low":2.7,"y_high":3.3,"is_min":false,"is_1se":false}]"#;
        let svg = go("curve", cv, "{}").unwrap();
        assert!(svg.contains("λ.min") && svg.contains("λ.1se"));
    }

    #[test]
    fn empty_input_is_a_note() {
        let s = go("terms", "[]", "{}").unwrap();
        assert!(s.contains("No data"));
    }
}
