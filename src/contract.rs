//! Contract plots — one chart per *shape* of model output
//! (`docs/plans/integration-contract.md`): `terms`, `prediction`, `curve`,
//! `summary`, `obs` (single diagnostic panels and the 2×2 `diagnostics`),
//! `test` (`tests`: a comparison forest; `compare`: groups with significance
//! brackets), partial-residual plots (`partial`), a cross-validation fold
//! timeline (`cv`, provisional), point-in-time `events` (alarms, parameter
//! changes) and an `auto` dispatch on the column signature.
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

mod curves;
mod cv;
mod events;
mod graph;
mod look;
mod names;
mod stats;
mod terms;

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
    "cv",
    "events",
    "diagram",
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
    render_plot(plot, rows, options, width, height, o, place)
        .map(|(svg, warnings)| (slim_points(svg), warnings))
}

/// ggplot-rs writes `fill-opacity="1.000"` on every point; 1 is the SVG
/// default and no ancestor sets another, so drop it from `<circle>` marks
/// (about a tenth of a scatter's bytes: diagnostics are mostly points).
fn slim_points(svg: String) -> String {
    const OPAQUE: &str = " fill-opacity=\"1.000\"";
    if !svg.contains(OPAQUE) {
        return svg;
    }
    let mut out = String::with_capacity(svg.len());
    let mut rest = svg.as_str();
    while let Some(i) = rest.find("<circle ") {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let end = rest.find('>').map_or(rest.len(), |e| e + 1);
        out.push_str(&rest[..end].replacen(OPAQUE, "", 1));
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

fn render_plot(
    plot: &str,
    rows: &[Row],
    options: &Row,
    width: u32,
    height: u32,
    o: &RenderOptions,
    place: Place,
) -> Result<(String, Vec<String>), RenderError> {
    // Producer spellings (`coef`, `P>|t|`, `.fitted`, …) read as contract
    // columns; `compare` takes raw rows whose columns the options name.
    let mut renamed = Vec::new();
    let mut rename = |rows: &[Row]| match plot {
        // `compare` reads its rows as given, `diagram` with its own names
        "compare" | "diagram" => None,
        _ => names::normalize(rows, options, &mut renamed),
    };
    let normed;
    let rows = match rename(rows) {
        Some(r) => {
            normed = r;
            &normed[..]
        }
        None => rows,
    };
    // A column holding the test STRUCT (or LIST of them) of a `*_agg`
    // aggregate: one row per test.
    let flat;
    let rows = match plot {
        "tests" | "auto" => match flatten(rows) {
            Some(f) if plot == "tests" || detect(rows).is_err() => {
                flat = rename(&f).unwrap_or(f);
                &flat[..]
            }
            _ => rows,
        },
        _ => rows,
    };
    let name = match plot {
        // No rows have no schema: the "No data" note, as for every macro.
        "auto" if rows.is_empty() => "auto",
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
            format!("anofox_plot (anofox contract v{CONTRACT_VERSION}, detected the {name} schema): {e}")
        } else {
            // Missing columns of a table that follows another schema: say which.
            let other = match detect(rows) {
                Ok(d)
                    if e.starts_with(NEEDS)
                        && d != name
                        && !(d == "diagnostics" && name == "obs") =>
                {
                    format!(" — the columns follow the {d} schema (anofox_plot_{d})")
                }
                _ => String::new(),
            };
            format!("anofox_plot_{name}: {e}{other}")
        })
    };
    let t = Table {
        rows,
        renamed: &renamed,
    };
    let (w, h) = (
        crate::clamp_dim(width as u64),
        crate::clamp_dim(height as u64),
    );
    // Plots drawn directly with ggplot-rs (see `stats`).
    let direct = |f: &dyn Fn() -> stats::Out| crate::guard(|| f().map_err(fail));
    let labels = look::Labels::from(options);
    let cols = match name {
        "terms" => return direct(&|| terms::terms(&t, options, w, h, o, place)),
        "prediction" => return direct(&|| stats::prediction(&t, options, w, h, o, place)),
        "curve" => match curve_type(&t).map_err(fail)?.as_str() {
            "km" => return direct(&|| stats::km(&t, options, w, h, o, place)),
            "lambda_cv" => return direct(&|| stats::lambda_cv(&t, options, w, h, o, place)),
            "roc" => return direct(&|| curves::roc(&t, options, w, h, o, place)),
            "pr" => return direct(&|| curves::pr(&t, options, w, h, o, place)),
            "calibration" => return direct(&|| curves::calibration(&t, options, w, h, o, place)),
            ty @ ("acf" | "pacf") => {
                return direct(&|| curves::acf(&t, ty, options, w, h, o, place))
            }
            _ => curve(&t),
        },
        "summary" => return direct(&|| stats::summary(&t, options, w, h, o, place)),
        "tests" => return direct(&|| stats::tests(&t, options, w, h, o, place)),
        "compare" => return direct(&|| stats::compare(&t, options, w, h, o, place)),
        "partial" => return direct(&|| stats::partial(&t, options, w, h, o, place)),
        "cv" => return direct(&|| cv::cv(&t, options, w, h, o, place)),
        "diagram" => {
            let cols = graph::columns(rows, options).map_err(fail)?;
            return crate::render_placed(&cols, place, w, h, o);
        }
        "events" => return direct(&|| events::events(&t, options, w, h, o, place)),
        "obs" => {
            let kind = opt_str(options, "kind").unwrap_or_else(|| "resid_fitted".into());
            obs(&t, &kind, label_top(options))
        }
        "diagnostics" => {
            return crate::guard(|| {
                diagnostics(&t, label_top(options), &labels, width, height, o, place).map_err(fail)
            })
        }
        _ => unreachable!("checked above"),
    }
    .map_err(fail)?;
    // Role-rendered panels (obs, the remaining curves): house look, default
    // axis labels, quiet reference lines.
    let (xl, yl) = match name {
        "curve" => match curve_type(&t).unwrap_or_default().as_str() {
            "qq" => ("theoretical quantile".into(), "sample quantile".into()),
            "pdp" => ("feature value".into(), "partial dependence".into()),
            "lift" => ("fraction targeted".into(), "lift".into()),
            "lowess" => ("x".into(), "smoothed y".into()),
            "null_dist" => ("statistic".into(), "density".into()),
            _ => ("x".into(), "y".into()),
        },
        _ => axis_labels(&cols),
    };
    let refs = take_refs(&cols);
    let cols: Vec<Column> = cols
        .into_iter()
        .filter(|c| !matches!(c.role, Role::RefLine | Role::Identity))
        .filter(|c| {
            refs.tips.is_empty() || !matches!(c.role, Role::LabelTop | Role::Label | Role::Rank)
        })
        .collect();
    match crate::panel_plot(&cols, w, h, o).map_err(fail)? {
        Some(plot) => {
            let plot = tweak(plot, o, &refs, &labels, &xl, &yl, false);
            crate::guard(|| stats::finish(plot, w, h, place).map_err(fail))
        }
        None => crate::render_placed(&cols, place, width, height, o),
    }
}

/// Reference lines taken off role columns (drawn in the quiet style by
/// [`tweak`]): horizontal y values, and whether the identity is wanted.
#[derive(Default)]
struct Refs {
    h: Vec<f64>,
    identity: bool,
    /// Label the first horizontal line (Cook's 4/n) at this x.
    note: Option<String>,
    note_x: Option<f64>,
    /// Make the y range symmetric around 0 to this |y| at x (both signs of
    /// the Cook's contours in view).
    sym_y: Option<(f64, f64)>,
    /// Text above stem tips (Cook's index plot): (x, y, text).
    tips: Vec<(f64, f64, String)>,
}

fn take_refs(cols: &[Column]) -> Refs {
    let mut r = Refs::default();
    for c in cols {
        match c.role {
            Role::RefLine => {
                for v in &c.values {
                    if let Some(f) = v.as_f64().filter(|f| f.is_finite()) {
                        if !r.h.iter().any(|x| (x - f).abs() < 1e-12) {
                            r.h.push(f);
                        }
                    }
                }
                if c.name == "cut" {
                    r.note = Some("4/n".into());
                    r.note_x = cols
                        .iter()
                        .find(|c| c.role == Role::X)
                        .and_then(|c| c.values.iter().filter_map(|v| v.as_f64()).reduce(f64::max));
                }
            }
            Role::Identity => r.identity = true,
            Role::CooksContour => {
                let x = cols
                    .iter()
                    .find(|c| c.role == Role::X)
                    .and_then(|c| c.values.iter().filter_map(|v| v.as_f64()).reduce(f64::min));
                let m = cols
                    .iter()
                    .find(|c| matches!(c.role, Role::Value(_)))
                    .and_then(|c| {
                        c.values
                            .iter()
                            .filter_map(|v| v.as_f64())
                            .filter(|f| f.is_finite())
                            .map(f64::abs)
                            .reduce(f64::max)
                    });
                if let (Some(x), Some(m)) = (x, m) {
                    r.sym_y = Some((x, m));
                }
            }
            _ => {}
        }
    }
    // Cook's index plot: the top-k labels sit above their stems.
    let is_cooks = cols
        .iter()
        .any(|c| c.name == "cooks_d" && matches!(c.role, Role::Value(_)));
    let k = cols
        .iter()
        .find(|c| c.role == Role::LabelTop)
        .and_then(|c| c.values.first()?.as_f64());
    if let (true, Some(k)) = (is_cooks, k) {
        let get = |pred: &dyn Fn(&Column) -> bool| cols.iter().find(|c| pred(c));
        if let (Some(x), Some(y), Some(l)) = (
            get(&|c| c.role == Role::X),
            get(&|c| c.name == "cooks_d"),
            get(&|c| c.role == Role::Label),
        ) {
            let mut idx: Vec<usize> = (0..y.values.len())
                .filter(|&i| y.values[i].as_f64().is_some_and(f64::is_finite))
                .collect();
            idx.sort_by(|&a, &b| {
                let (p, q) = (y.values[a].as_f64(), y.values[b].as_f64());
                q.unwrap_or(0.0).total_cmp(&p.unwrap_or(0.0))
            });
            idx.truncate(k.max(0.0) as usize);
            for i in idx {
                if let (Some(xv), Some(yv)) = (
                    x.values.get(i).and_then(|v| v.as_f64()),
                    y.values[i].as_f64(),
                ) {
                    let t = match l.values.get(i) {
                        Some(Value::Str(s)) => s.clone(),
                        Some(v) => v.as_f64().map(stats::fmt_num).unwrap_or_default(),
                        None => String::new(),
                    };
                    r.tips.push((xv, yv, t));
                }
            }
        }
    }
    r
}

/// Default axis labels of a role-rendered contract panel, from its x / value
/// column names.
fn axis_labels(cols: &[Column]) -> (String, String) {
    let x = cols
        .iter()
        .find(|c| c.role == Role::X)
        .map(|c| c.name.as_str())
        .unwrap_or("");
    let y = cols
        .iter()
        .find(|c| matches!(c.role, Role::Value(_)))
        .map(|c| c.name.as_str())
        .unwrap_or("");
    let name = |n: &str| -> String {
        match n {
            "fitted" => "fitted".into(),
            "residual" => "residual".into(),
            "std_residual" => "standardised residual".into(),
            "sqrt_abs_std_residual" => "√|standardised residual|".into(),
            "leverage" => "leverage".into(),
            "cooks_d" => "Cook's distance".into(),
            "row_id" => "observation".into(),
            other => look::humanize(other),
        }
    };
    (name(x), name(y))
}

/// The house look over a role-rendered plot.
fn tweak(
    plot: ggplot_rs::prelude::GGPlot,
    o: &RenderOptions,
    refs: &Refs,
    labels: &look::Labels,
    xl: &str,
    yl: &str,
    small: bool,
) -> ggplot_rs::prelude::GGPlot {
    use ggplot_rs::prelude::*;
    let mut plot = plot.theme_update(look::theme_update(o, small));
    for (i, y) in refs.h.iter().enumerate() {
        plot = look::hline(plot, *y);
        if i == 0 {
            if let (Some(n), Some(nx)) = (&refs.note, refs.note_x) {
                plot = plot
                    .geom_text_with(GeomText {
                        size: 10.0,
                        color: (120, 126, 138),
                        hjust: 1.0,
                        vjust: 0.0,
                        ..Default::default()
                    })
                    .layer_data(vec![
                        ("x".to_string(), vec![Value::Float(nx)]),
                        ("y".to_string(), vec![Value::Float(*y)]),
                        ("note".to_string(), vec![Value::Str(n.clone())]),
                    ])
                    .layer_aes(Aes::new().x("x").y("y").label("note"));
            }
        }
    }
    if let Some((x, m)) = refs.sym_y {
        plot = plot
            .geom_blank()
            .layer_data(vec![
                ("x".to_string(), vec![Value::Float(x), Value::Float(x)]),
                (
                    "y".to_string(),
                    vec![Value::Float(-m * 1.05), Value::Float(m * 1.05)],
                ),
            ])
            .layer_aes(Aes::new().x("x").y("y"));
    }
    if !refs.tips.is_empty() {
        let top = refs.tips.iter().map(|t| t.1).fold(0.0, f64::max);
        plot = plot
            .geom_text_with(GeomText {
                size: 10.0,
                color: (55, 62, 75),
                hjust: 0.5,
                vjust: 0.0,
                ..Default::default()
            })
            .layer_data(vec![
                (
                    "x".to_string(),
                    refs.tips.iter().map(|t| Value::Float(t.0)).collect(),
                ),
                (
                    "y".to_string(),
                    refs.tips
                        .iter()
                        .map(|t| Value::Float(t.1 + top * 0.025))
                        .collect(),
                ),
                (
                    "note".to_string(),
                    refs.tips.iter().map(|t| Value::Str(t.2.clone())).collect(),
                ),
            ])
            .layer_aes(Aes::new().x("x").y("y").label("note"))
            .geom_blank()
            .layer_data(vec![
                ("x".to_string(), vec![Value::Float(refs.tips[0].0)]),
                ("y".to_string(), vec![Value::Float(top * 1.1)]),
            ])
            .layer_aes(Aes::new().x("x").y("y"));
    }
    if refs.identity {
        plot = plot.geom_abline_with(GeomAbline {
            slope: 1.0,
            intercept: 0.0,
            color: look::REF,
            width: look::REF_WIDTH,
            linetype: Linetype::Dashed,
            alpha: 1.0,
        });
    }
    let plot = plot.xlab(&labels.x_or(xl)).ylab(&labels.y_or(yl));
    match &labels.title {
        Some(t) => plot.title(t),
        None => plot,
    }
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
    } else if has("fold_id") && (has("split") || has("test_start") || has("train_end")) {
        "cv"
    } else if (has("kind") || has("severity")) && (has("ds") || has("period")) {
        "events"
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
             test (p_value / statistic, method), partial residuals (term, x, \
             partial_residual), cv folds (fold_id, split, ds) or events (ds | period, kind){}",
            keys.join(", "),
            closest(&keys)
        )));
    })
}

/// The required columns of each schema [`detect`] tells apart, and those among
/// them that identify it. A schema is only suggested when one identifying
/// column is present or misspelled: a shared generic name (`kind`, `value`,
/// `x`, `y`) alone is no near miss (#180). A short identifying column's near
/// miss counts only as itself plus a suffix (#255).
const SCHEMAS: &[(&str, &[&str], &[&str])] = &[
    ("terms", &["term", "estimate"], &["term", "estimate"]),
    ("prediction", &["yhat"], &["yhat"]),
    ("curve", &["curve_type", "x", "y"], &["curve_type"]),
    ("summary", &["metric", "value"], &["metric"]),
    (
        "diagnostics",
        &["fitted", "residual"],
        &["fitted", "residual"],
    ),
    (
        "partial",
        &["term", "x", "partial_residual"],
        &["partial_residual"],
    ),
    ("cv", &["fold_id", "split", "ds"], &["fold_id"]),
    ("tests", &["p_value"], &["p_value"]),
    ("events", &["ds", "kind"], &["ds"]),
];

/// Columns [`detect`] also reads in place of a required one: `events` takes
/// `period` for `ds` and `severity` for `kind`.
const STAND_INS: &[(&str, &str)] = &[("ds", "period"), ("kind", "severity")];

/// Does the near miss `k` of the identifying column `c` point at its schema
/// on its own? A long name's does (`estimat`, `curve_typ`); a short one
/// (`term`, `yhat`) is one letter from everyday words (`team`, `that`), so
/// only `c` with a suffix counts (`terms`, `yhat_1`) (#255).
fn identifies(c: &str, k: &str) -> bool {
    c.len() >= 6 || names::norm(k).starts_with(c)
}

/// For an undetected table: the schema whose required columns are nearest —
/// present or misspelled — with what is missing (`""` when none is close, or
/// when only generic names match).
fn closest(keys: &[&str]) -> String {
    let present =
        |c: &str| keys.contains(&c) || STAND_INS.iter().any(|(k, a)| *k == c && keys.contains(a));
    // max_by_key keeps the last of equals: walk backwards so the first wins.
    let best = SCHEMAS
        .iter()
        .rev()
        .map(|(name, req, ident)| {
            let found: Vec<(&str, Option<&str>)> = req
                .iter()
                .map(|c| (*c, (!present(c)).then(|| names::near(c, keys)).flatten()))
                .collect();
            let hit = |c: &str| present(c) || found.iter().any(|(f, n)| *f == c && n.is_some());
            let score = found.iter().filter(|(c, _)| hit(c)).count();
            let identified = ident.iter().any(|c| {
                present(c)
                    || found
                        .iter()
                        .any(|&(f, n)| f == *c && n.is_some_and(|k| identifies(c, k)))
            });
            (score, identified, name, found)
        })
        .filter(|(score, identified, ..)| *score > 0 && *identified)
        .max_by_key(|(score, ..)| *score);
    let Some((_, _, name, found)) = best else {
        return String::new();
    };
    let hints: Vec<String> = found
        .iter()
        .filter(|(c, _)| !present(c))
        .map(|(c, n)| match n {
            Some(n) => format!("{c} (did you mean `{n}`?)"),
            None => c.to_string(),
        })
        .collect();
    format!("; closest: {name}, missing {}", hints.join(", "))
}

/// How every missing-column error starts.
const NEEDS: &str = "needs column(s) ";

/// Row access by contract column name.
struct Table<'a> {
    rows: &'a [Row],
    /// Producer columns read as contract columns: (original, contract).
    renamed: &'a [(String, String)],
}

impl Table<'_> {
    fn has(&self, k: &str) -> bool {
        self.rows
            .iter()
            .any(|r| r.get(k).is_some_and(|v| !v.is_null()))
    }
    /// The columns as given (a renamed one as `coef → estimate`).
    fn keys(&self) -> String {
        self.rows
            .first()
            .map(|r| {
                r.keys()
                    .map(|k| match self.renamed.iter().find(|(_, c)| c == k) {
                        Some((o, c)) => format!("{o} → {c}"),
                        None => k.clone(),
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default()
    }
    /// The required `cols`, else an error naming what is missing, the
    /// present column it may be misspelled as and the other accepted names.
    fn need(&self, cols: &[&str]) -> Result<(), String> {
        let missing: Vec<&str> = cols.iter().copied().filter(|c| !self.has(c)).collect();
        if missing.is_empty() {
            return Ok(());
        }
        let keys: Vec<&str> = self
            .rows
            .first()
            .map(|r| r.keys().map(String::as_str).collect())
            .unwrap_or_default();
        let hints: Vec<String> = missing
            .iter()
            .filter_map(|m| match (names::near(m, &keys), names::also(m)) {
                // present, but NULL (or NaN, which to_json writes as null) throughout
                _ if keys.contains(m) => Some(format!("{m} is NULL in every row")),
                (Some(k), _) => Some(format!("did you mean `{k}` for {m}?")),
                (None, Some(a)) => Some(format!("{m} is also read from {a}")),
                (None, None) => None,
            })
            .collect();
        Err(format!(
            "{NEEDS}{} (got: {}){}",
            missing.join(", "),
            self.keys(),
            if hints.is_empty() {
                String::new()
            } else {
                format!(" — {}", hints.join("; "))
            }
        ))
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
                Some(J::Array(_)) => Err(format!(
                    "column `{k}` must be numeric, got a list — take one element ({k}[1]) or \
                     unnest it"
                )),
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

/// `curve` types drawn by the role renderer: qq (points + identity +
/// envelope), null_dist / pdp / lift / lowess (line ± band). roc, pr, calibration,
/// acf / pacf ([`curves`]), km and lambda_cv ([`stats`]) are drawn directly.
fn curve(t: &Table) -> Result<Vec<Column>, String> {
    let ty = curve_type(t)?;
    let yv = t.num("y")?;
    let band = t.has("y_low") && t.has("y_high");
    let mut cols = vec![col("x", Role::X, t.xpos("x"))];
    let series = ["model_id", "series", "id"]
        .into_iter()
        .find(|k| t.distinct(k) > 1);
    match ty.as_str() {
        "pdp" | "lift" | "null_dist" | "lowess" => {
            cols.push(col("y", Role::Value(Kind::Line), yv));
        }
        "qq" => {
            // Precomputed (theoretical, sample) pairs: the producer's envelope
            // (y_low/y_high) is drawn as given.
            cols.push(col("y", Role::Value(Kind::Point), yv));
            cols.push(konst("id", Role::Identity, Value::Float(1.0)));
        }
        other => {
            return Err(format!(
                "unknown curve_type '{other}' (roc, pr, calibration, lambda_cv, km, acf, \
                 pacf, qq, null_dist, pdp, lift, lowess)"
            ))
        }
    }
    if band {
        cols.push(col("y_low", Role::BandLower, t.num("y_low")?));
        cols.push(col("y_high", Role::BandUpper, t.num("y_high")?));
    }
    if let Some(s) = series {
        cols.push(col(s, Role::Category, t.text(s)));
    }
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
    labels: &look::Labels,
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
        let (xl, yl) = axis_labels(&cols);
        let refs = take_refs(&cols);
        let cols: Vec<Column> = cols
            .into_iter()
            .filter(|c| !matches!(c.role, Role::RefLine | Role::Identity))
            .filter(|c| {
                refs.tips.is_empty() || !matches!(c.role, Role::LabelTop | Role::Label | Role::Rank)
            })
            .collect();
        let (xl, yl) = if *kind == "qq" {
            (
                "theoretical quantile".to_string(),
                "standardised residual".to_string(),
            )
        } else {
            (xl, yl)
        };
        grid = match crate::panel_plot(&cols, w, h, o)? {
            Some(plot) => grid
                .add(tweak(plot, o, &refs, &look::Labels::default(), &xl, &yl, true).title(title)),
            None => grid.add_spacer(),
        };
    }
    if let Some(t) = &labels.title {
        grid = grid.title(t);
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
        assert!(svg.contains("p = 0.04 *") && !svg.contains("Shapiro-Wilk"));
        let e = go("tests", r#"[{"method":"m"}]"#, "{}")
            .unwrap_err()
            .to_string();
        assert!(e.contains("no estimate, statistic or p_value"), "{e}");
        // A column holding the aggregate's LIST of STRUCTs is expanded.
        let nested = format!(r#"[{{"r":{TUKEY}}}]"#);
        assert!(go("tests", &nested, "{}").is_ok());
        assert!(go("auto", &nested, "{}").is_ok());
        // pingouin's pairwise_tests: BF10 is text but a number, p-adjust one
        // value; neither splits the forest into panels.
        let pw = r#"[{"Contrast":"g","A":"0","B":"10","T":-3.9,"p-unc":0.0005,"p-corr":0.001,"p-adjust":"holm","BF10":"156.665"},
            {"Contrast":"g","A":"0","B":"20","T":-7.1,"p-unc":1e-9,"p-corr":3e-9,"p-adjust":"holm","BF10":"2.648e+09"},
            {"Contrast":"g","A":"10","B":"20","T":-2.2,"p-unc":0.03,"p-corr":0.03,"p-adjust":"holm","BF10":"1.2"}]"#;
        let svg = go("tests", pw, "{}").unwrap();
        assert_eq!(svg.matches("<svg").count(), 1, "one panel: {svg}");
        assert!(!svg.contains("156.665"), "BF10 is not a panel title");
    }

    #[test]
    fn opaque_points_carry_no_fill_opacity() {
        let s = super::slim_points(
            r#"<circle cx="1" fill-opacity="1.000"><title>a</title></circle><circle fill-opacity="0.500"/><rect fill-opacity="1.000"/>"#
                .into(),
        );
        assert_eq!(
            s,
            r#"<circle cx="1"><title>a</title></circle><circle fill-opacity="0.500"/><rect fill-opacity="1.000"/>"#
        );
        let obs: Vec<String> = (0..30)
            .map(|i| {
                format!(
                    r#"{{"fitted":{i},"residual":{},"row_id":{i}}}"#,
                    (i as f64).sin()
                )
            })
            .collect();
        let svg = go("diagnostics", &format!("[{}]", obs.join(",")), "{}").unwrap();
        let circles: Vec<&str> = svg
            .split("<circle ")
            .skip(1)
            .map(|c| &c[..c.find('>').unwrap()])
            .collect();
        assert!(
            !circles.is_empty()
                && circles
                    .iter()
                    .all(|c| !c.contains("fill-opacity=\"1.000\""))
        );
        assert!(
            svg.contains("data-value=") && svg.contains("<title>"),
            "hover kept"
        );
    }

    #[test]
    fn compare_brackets() {
        let data = r#"[{"g":"a","y":1.0},{"g":"a","y":1.4},{"g":"a","y":0.8},
            {"g":"b","y":2.1},{"g":"b","y":2.6},{"g":"b","y":2.2},
            {"g":"c","y":1.1},{"g":"c","y":1.5},{"g":"c","y":1.2}]"#;
        let tests = format!(r#"{{"value":"y","factor":"g","tests":{TUKEY}}}"#);
        let svg = go("compare", data, &tests).unwrap();
        assert!(
            svg.contains(">**<") && svg.contains("data-series=\"b-a\""),
            "bracket + stars label"
        );
        let with_p = format!(r#"{{"value":"y","factor":"g","label":"p","tests":{TUKEY}}}"#);
        assert!(go("compare", data, &with_p)
            .unwrap()
            .contains("p = 0.003 **"));
        // A two-sample STRUCT for two groups.
        let two = r#"[{"g":"a","y":1.0},{"g":"a","y":1.4},{"g":"b","y":2.1},{"g":"b","y":2.6}]"#;
        let svg = go(
            "compare",
            two,
            r#"{"value":"y","factor":"g","kind":"violin","tests":{"p_value":0.02,"method":"Welch t-test"}}"#,
        )
        .unwrap();
        assert!(!svg.contains("Welch t-test") && svg.contains(">*<"));
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
        let cr = go("partial", pr, "{}").unwrap();
        assert!(cr.contains("component + residual") && !cr.contains("Component + residual plots"));
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
            r.contains("number at risk")
                && r.contains("survival probability")
                && !r.contains("Kaplan-Meier")
                && !r.contains("data-warnings"),
            "{r}"
        );
        let cv = r#"[{"curve_type":"lambda_cv","x":0.1,"y":2.0,"y_low":1.8,"y_high":2.2,"is_min":true,"is_1se":false},
            {"curve_type":"lambda_cv","x":1.0,"y":2.1,"y_low":1.9,"y_high":2.3,"is_min":false,"is_1se":true},
            {"curve_type":"lambda_cv","x":10.0,"y":3.0,"y_low":2.7,"y_high":3.3,"is_min":false,"is_1se":false}]"#;
        let svg = go("curve", cv, "{}").unwrap();
        assert!(svg.contains("λ.min") && svg.contains("λ.1se"));
    }

    #[test]
    fn terms_fade_intervals_that_cross_the_null() {
        // x1 is significant, x2's interval crosses 0, x3 has no interval.
        let rows = r#"[{"term":"x1","estimate":0.5,"conf_low":0.2,"conf_high":0.8},
            {"term":"x2","estimate":-0.3,"conf_low":-0.7,"conf_high":0.1},
            {"term":"x3","estimate":0.4}]"#;
        let svg = go("terms", rows, "{}").unwrap();
        // The start tag of the mark whose tooltip names `term`.
        let marks = |svg: &str, term: &str| -> Vec<String> {
            svg.match_indices(&format!("<title>{term}:"))
                .map(|(i, _)| {
                    let start = svg[..i].rfind('<').unwrap();
                    svg[start..i].to_string()
                })
                .collect()
        };
        let x1 = marks(&svg, "x1");
        let x2 = marks(&svg, "x2");
        let x3 = marks(&svg, "x3");
        assert!(!x1.is_empty() && !x2.is_empty() && !x3.is_empty(), "{svg}");
        assert!(x1.iter().all(|m| !m.contains("fill-opacity")), "{x1:?}");
        assert!(x3.iter().all(|m| !m.contains("fill-opacity")), "{x3:?}");
        assert!(
            x2.iter().all(|m| m.contains("fill-opacity=\"0.500\"")),
            "{x2:?}"
        );
        assert!(
            svg.contains("x2: -0.3 [-0.7, 0.1] · n.s."),
            "tooltip says n.s."
        );
        // Exactly one interval line is faded (x2's).
        assert_eq!(svg.matches("stroke-opacity=\"0.500\"").count(), 1, "{svg}");
        assert!(svg.contains("faded = interval crosses 0"));
        // A numeric `reference :=` moves the null value: against 0.5, x1
        // (0.2..0.8) is not significant and x2 (-0.7..0.1) is.
        let svg = go("terms", rows, r#"{"reference":0.5}"#).unwrap();
        assert!(marks(&svg, "x1")
            .iter()
            .all(|m| m.contains("fill-opacity=\"0.500\"")));
        assert!(marks(&svg, "x2")
            .iter()
            .all(|m| !m.contains("fill-opacity")));
        assert!(svg.contains("faded = interval crosses 0.5"));
        // All significant: no caption about fading.
        let svg = go(
            "terms",
            r#"[{"term":"a","estimate":1,"conf_low":0.5,"conf_high":1.5}]"#,
            "{}",
        )
        .unwrap();
        assert!(!svg.contains("faded"));
    }

    #[test]
    fn producer_spellings_plot() {
        // A raw statsmodels coefficient table (summary2().tables[1]).
        let sm = r#"[{"term":"x1","Coef.":0.5,"Std.Err.":0.1,"t":5.0,"P>|t|":0.001,"[0.025":0.3,"0.975]":0.7},
            {"term":"x2","Coef.":-0.3,"Std.Err.":0.2,"t":-1.5,"P>|t|":0.2,"[0.025":-0.7,"0.975]":0.1}]"#;
        let svg = go("terms", sm, "{}").unwrap();
        assert!(svg.contains("x2: -0.3 [-0.7, 0.1] · n.s."), "interval read");
        assert_eq!(detect(&rows(sm)).ok(), None, "detect sees raw names");
        assert!(go("auto", sm, "{}")
            .unwrap()
            .contains("faded = interval crosses 0"));
        // broom::augment → diagnostics.
        let aug = r#"[{".fitted":1,".resid":0.2,".hat":0.1,".cooksd":0.01},
            {".fitted":2,".resid":-0.1,".hat":0.2,".cooksd":0.02},
            {".fitted":3,".resid":0.0,".hat":0.3,".cooksd":0.03}]"#;
        assert!(go("auto", aug, "{}")
            .unwrap()
            .contains("Residuals vs leverage"));
        // A test table from pingouin.
        let pg = r#"[{"Contrast":"g","A":"a","B":"b","T":2.1,"p-unc":0.04}]"#;
        assert!(go("auto", pg, "{}").unwrap().contains("p = 0.04"));
    }

    /// ts_stationarity_by reshaped: two tests per series, no test_id.
    const STATIONARITY: &str = r#"[{"id":"A","method":"ADF","statistic":-0.7,"p_value":0.85},
        {"id":"A","method":"KPSS","statistic":2.1,"p_value":0.01},
        {"id":"B","method":"ADF","statistic":-0.2,"p_value":0.94},
        {"id":"B","method":"KPSS","statistic":2.0,"p_value":0.01}]"#;

    /// `name="a b"` attributes of `svg`, in document order.
    fn pairs(svg: &str, name: &str) -> Vec<(f64, f64)> {
        svg.split(&format!("{name}=\""))
            .skip(1)
            .map(|r| {
                let v: Vec<f64> = r[..r.find('"').unwrap()]
                    .split(' ')
                    .map(|x| x.parse().unwrap())
                    .collect();
                (v[0], v[1])
            })
            .collect()
    }

    #[test]
    fn tests_label_group_and_method() {
        // On −log10(p), one scale for every test: one panel, rows "A · ADF".
        let s = go("auto", STATIONARITY, r#"{"value":"p"}"#).unwrap();
        assert!(!s.contains("data-panel="), "one panel");
        for l in ["A · ADF", "A · KPSS", "B · ADF", "B · KPSS"] {
            assert!(s.contains(l), "{l}");
        }
    }

    #[test]
    fn tests_panel_per_method() {
        // Statistics of different tests: a panel per method (#190) …
        for plot in ["tests", "auto"] {
            let s = go(plot, STATIONARITY, "{}").unwrap();
            dump(&format!("tests_per_method_{plot}"), &s);
            assert_eq!(s.matches("data-panel=\"").count(), 2, "{plot}");
            assert!(s.contains(">ADF<") && s.contains(">KPSS<"), "method titles");
            assert!(
                !s.contains("A · ADF"),
                "the method is the title, not in the row"
            );
            // … each with its own x: ADF near its statistics, KPSS near 2 …
            let x = pairs(&s, "data-xdomain");
            assert_eq!(x.len(), 2, "{x:?}");
            assert!(x[0].1 < 1.0, "ADF axis: {x:?}");
            assert!(x[1].0 > -1.0 && x[1].1 > 2.0, "KPSS axis: {x:?}");
            // … and the groups on one shared y.
            let y = pairs(&s, "data-ydomain");
            assert_eq!(y.len(), 2, "{y:?}");
            assert_eq!(y[0], y[1], "shared y");
        }
        // A group one test lacks keeps its slot: the axes still match.
        let more = STATIONARITY.replace(
            "]",
            r#",{"id":"C","method":"KPSS","statistic":1.9,"p_value":0.02}]"#,
        );
        let s = go("tests", &more, "{}").unwrap();
        let y = pairs(&s, "data-ydomain");
        assert_eq!(y.len(), 2, "{y:?}");
        assert_eq!(y[0], y[1], "shared y");
        assert!(y[0].1 > 3.0, "three groups: {y:?}");
        // One method keeps today's single statistic axis.
        let one = r#"[{"id":"A","method":"ADF","statistic":-0.7,"p_value":0.85},
            {"id":"B","method":"ADF","statistic":-0.2,"p_value":0.94}]"#;
        let s = go("tests", one, "{}").unwrap();
        assert!(!s.contains("data-panel="), "one panel");
    }

    #[test]
    fn missing_columns_are_explained() {
        // Misnamed: the near miss is suggested.
        let e = go("terms", r#"[{"term":"a","estimat":1}]"#, "{}")
            .unwrap_err()
            .to_string();
        assert!(e.contains("needs column(s) estimate"), "{e}");
        assert!(e.contains("did you mean `estimat` for estimate?"), "{e}");
        // Missing: the accepted spellings are named.
        let e = go("terms", r#"[{"term":"a","beta":1}]"#, "{}")
            .unwrap_err()
            .to_string();
        assert!(e.contains("estimate is also read from coef, params"), "{e}");
        // Present but NULL throughout (a producer's NaN): said so.
        let e = go("summary", r#"[{"metric":"m","value":null}]"#, "{}")
            .unwrap_err()
            .to_string();
        assert!(e.contains("value is NULL in every row"), "{e}");
        // A renamed column shows as such among the columns got.
        let e = go("diagnostics", r#"[{".fitted":1}]"#, "{}")
            .unwrap_err()
            .to_string();
        assert!(e.contains("got: .fitted → fitted"), "{e}");
        assert!(e.contains("residual is also read from resid"), "{e}");
        // A table of another schema: the one it follows is named.
        let e = go("terms", r#"[{"ds":"2024-01-01","yhat":1}]"#, "{}")
            .unwrap_err()
            .to_string();
        assert!(
            e.contains("the columns follow the prediction schema (anofox_plot_prediction)"),
            "{e}"
        );
        // anofox_plot: the detected schema, else the closest one.
        let e = go("auto", r#"[{"curve_type":"roc","x":0}]"#, "{}")
            .unwrap_err()
            .to_string();
        assert!(
            e.contains("detected the curve schema") && e.contains("needs column(s) y"),
            "{e}"
        );
        let e = go("auto", r#"[{"term":"a","estimat":1}]"#, "{}")
            .unwrap_err()
            .to_string();
        assert!(
            e.contains("closest: terms, missing estimate (did you mean `estimat`?)"),
            "{e}"
        );
        let e = go("auto", r#"[{"a":1}]"#, "{}").unwrap_err().to_string();
        assert!(!e.contains("closest"), "{e}");
        // A generic name shared with a schema is no near miss (#180): decide's
        // `kind` column does not make an events table.
        let e = go(
            "auto",
            r#"[{"question_id":"q1","kind":"binary","probability":0.7}]"#,
            "{}",
        )
        .unwrap_err()
        .to_string();
        assert!(
            e.contains("match no plottable schema") && !e.contains("closest"),
            "{e}"
        );
        for generic in [r#"[{"id":"a","value":1}]"#, r#"[{"x":1,"y":2}]"#] {
            let e = go("auto", generic, "{}").unwrap_err().to_string();
            assert!(!e.contains("closest"), "{generic}: {e}");
        }
        // The schema's own column still earns the hint; period stands in for ds.
        let e = go("auto", r#"[{"period":3,"note":"x"}]"#, "{}")
            .unwrap_err()
            .to_string();
        assert!(
            e.contains("; closest: events, missing kind") && !e.contains("missing ds"),
            "{e}"
        );
        let e = go("auto", r#"[{"curve_typ":"roc","x":0,"y":1}]"#, "{}")
            .unwrap_err()
            .to_string();
        assert!(
            e.contains("closest: curve, missing curve_type (did you mean `curve_typ`?)"),
            "{e}"
        );
        // #255: a short identifying name one letter off is no near miss…
        for t in [
            r#"[{"model_id":"fixture-a","id":3,"grp":"g","refund":true,"team":"x","mood":"ok","question_id":"refund","kind":"binary","probability":0.7}]"#,
            r#"[{"team":"x"}]"#,
            r#"[{"that":1}]"#,
        ] {
            let e = go("auto", t, "{}").unwrap_err().to_string();
            assert!(
                e.contains("match no plottable schema") && !e.contains("closest"),
                "{t}: {e}"
            );
        }
        // …but a suffixed one, a long one, or one next to the schema's other column is.
        let hint = |t: &str| go("auto", t, "{}").unwrap_err().to_string();
        assert!(hint(r#"[{"terms":"a"}]"#)
            .contains("; closest: terms, missing term (did you mean `terms`?), estimate"));
        assert!(hint(r#"[{"estimat":1}]"#)
            .contains("; closest: terms, missing term, estimate (did you mean `estimat`?)"));
        assert!(hint(r#"[{"yhat_1":1}]"#)
            .contains("; closest: prediction, missing yhat (did you mean `yhat_1`?)"));
        assert!(hint(r#"[{"team":"x","estimate":1}]"#)
            .contains("; closest: terms, missing term (did you mean `team`?)"));
        // A list where a number belongs.
        let e = go("terms", r#"[{"term":"a","estimate":[1,2]}]"#, "{}")
            .unwrap_err()
            .to_string();
        assert!(e.contains("got a list"), "{e}");
    }

    /// Write `svg` under `$ANOFOX_DUMP/<name>.svg` for a visual check.
    fn dump(name: &str, svg: &str) {
        if let Ok(d) = std::env::var("ANOFOX_DUMP") {
            let _ = std::fs::write(format!("{d}/{name}.svg"), svg);
        }
    }

    #[test]
    fn producer_outputs_plot() {
        // statsforecast's wide forecast: a line per model, a nested fan.
        let sf: String = format!(
            "[{}]",
            (1..=12)
                .map(|h| {
                    let a = 10.0 + h as f64 * 0.5;
                    let e = 11.0 + h as f64 * 0.3;
                    let w = 0.4 * (h as f64).sqrt();
                    format!(
                        r#"{{"unique_id":"A","ds":"2024-01-{h:02}","AutoARIMA":{a},"AutoARIMA-lo-80":{},"AutoARIMA-hi-80":{},"AutoARIMA-lo-95":{},"AutoARIMA-hi-95":{},"AutoETS":{e},"AutoETS-lo-95":{},"AutoETS-hi-95":{}}}"#,
                        a - w,
                        a + w,
                        a - 2.0 * w,
                        a + 2.0 * w,
                        e - 2.0 * w,
                        e + 2.0 * w
                    )
                })
                .collect::<Vec<_>>()
                .join(",")
        );
        let svg = go("auto", &sf, "{}").unwrap();
        dump("statsforecast", &svg);
        assert!(
            svg.contains("AutoARIMA") && svg.contains("AutoETS"),
            "models"
        );
        assert!(
            svg.contains("bands (AutoARIMA): 80 %, 95 % intervals"),
            "nested fan"
        );
        // level 95 and 0.95 are one level; conf_level 95 reads "95% CI".
        let one = r#"[{"x":1,"yhat":1,"yhat_lower":0,"yhat_upper":2,"level":95},
            {"x":1,"yhat":1,"yhat_lower":0,"yhat_upper":2,"level":0.95},
            {"x":2,"yhat":2,"yhat_lower":1,"yhat_upper":3,"level":95}]"#;
        assert!(!go("prediction", one, "{}").unwrap().contains("bands:"));
        let terms = r#"[{"term":"a","estimate":1,"conf_low":0.5,"conf_high":1.5,"conf_level":95}]"#;
        assert!(go("terms", terms, "{}").unwrap().contains("95% CI"));
        // pingouin's CI95% LIST.
        let pg = r#"[{"A":"a","B":"b","diff":1.0,"estimate":1.0,"p-unc":0.04,"CI95%":[0.2,1.8]}]"#;
        assert!(go("tests", pg, "{}")
            .unwrap()
            .contains("difference (95% CI)"));
        // The new metric directions mark the best model.
        let glance = r#"[{"model_id":"a","metric":"mape","value":0.12},{"model_id":"b","metric":"mape","value":0.09},
            {"model_id":"a","metric":"f1","value":0.7},{"model_id":"b","metric":"f1","value":0.8}]"#;
        let svg = go("summary", glance, "{}").unwrap();
        dump("summary_mape_f1", &svg);
        assert!(svg.contains("MAPE") && svg.contains("F1"));
        assert_eq!(super::stats::direction_of("MAPE"), 1.0);
        assert_eq!(super::stats::direction_of("smape"), 1.0);
        assert_eq!(super::stats::direction_of("brier"), 1.0);
        // anofox-decide's decide_ece: expected calibration error.
        assert_eq!(super::stats::direction_of("ece"), 1.0);
        assert_eq!(
            super::stats::direction_of("expected_calibration_error"),
            1.0
        );
        assert_eq!(super::stats::direction_of("log_loss"), 1.0);
        assert_eq!(super::stats::direction_of("auc"), -1.0);
        assert_eq!(super::stats::direction_of("accuracy"), -1.0);
        assert_eq!(super::stats::direction_of("f1"), -1.0);
        // scikit-learn names and scorers.
        for m in [
            "average_precision",
            "roc_auc_score",
            "matthews_corrcoef",
            "r2",
            "explained_variance",
            "balanced_accuracy",
            "neg_mean_squared_error",
        ] {
            assert_eq!(super::stats::direction_of(m), -1.0, "{m}");
        }
        for m in [
            "mean_pinball_loss",
            "mean_absolute_error",
            "hamming_loss",
            "brier_score_loss",
            "Mean Squared Error",
            "neg_r2",
        ] {
            assert_eq!(super::stats::direction_of(m), 1.0, "{m}");
        }
        assert_eq!(super::stats::direction_of("n_trees"), 0.0);
        // A near-perfect AUC keeps its digits.
        let roc = r#"[{"curve_type":"roc","x":0,"y":0,"auc":0.9952},{"curve_type":"roc","x":0.01,"y":0.99,"auc":0.9952},
            {"curve_type":"roc","x":1,"y":1,"auc":0.9952}]"#;
        let svg = go("curve", roc, "{}").unwrap();
        assert!(
            svg.contains("AUC 0.995") && !svg.contains("AUC 1.00"),
            "{svg}"
        );
    }

    const ALARMS: &str = r#"[
        {"period":4,"kind":"stockout","label":"on hand 0","severity":"alarm","value":0,"threshold":5,"series":"SKU-1"},
        {"period":9,"kind":"drift","label":"CUSUM","severity":"warning","value":4.2,"threshold":4,"series":"SKU-1"},
        {"period":12,"kind":"recalibrated","label":"lead time 3 → 4","series":"SKU-2"}]"#;

    #[test]
    fn events_overlay_and_timeline() {
        // An integer-period prediction (no ds) with a nested fan and events.
        let p: String = format!(
            "[{}]",
            (1..=16)
                .flat_map(|i| {
                    let y = if i <= 8 { format!("{}", 20 - i) } else { "null".into() };
                    [(0.8, 2.0), (0.95, 3.5)].map(|(l, w)| {
                        format!(
                            r#"{{"period":{i},"y":{y},"yhat":{},"yhat_lower":{},"yhat_upper":{},"level":{l}}}"#,
                            20 - i,
                            20.0 - i as f64 - w,
                            20.0 - i as f64 + w
                        )
                    })
                })
                .collect::<Vec<_>>()
                .join(",")
        );
        let opts = format!(r#"{{"events":{ALARMS}}}"#);
        let svg = go("prediction", &p, &opts).unwrap();
        dump("prediction_events", &svg);
        assert!(svg.contains(">period</text>"), "x axis is the period");
        assert!(svg.contains("bands: 80 %, 95 % intervals"));
        assert!(
            svg.contains("events: 1 alarm (red), 1 warning (amber), 1 event (grey)"),
            "{svg}"
        );
        assert!(svg.contains("alarm · stockout · SKU-1: on hand 0 0 (threshold 5)"));
        // The events as JSON text (a subquery argument) work the same.
        let text = serde_json::json!({ "events": ALARMS }).to_string();
        assert!(go("prediction", &p, &text)
            .unwrap()
            .contains("events: 1 alarm"));
        // A date event on a period axis is refused.
        let e = go(
            "prediction",
            &p,
            r#"{"events":[{"ds":"2024-01-01","kind":"x"}]}"#,
        )
        .unwrap_err()
        .to_string();
        assert!(
            e.contains("is a date but the prediction's x (`period`) is not"),
            "{e}"
        );
        // On their own: a timeline, one lane per series; anofox_plot finds it.
        assert_eq!(detect(&rows(ALARMS)).unwrap(), "events");
        let svg = go("auto", ALARMS, "{}").unwrap();
        dump("events", &svg);
        assert!(svg.contains(">SKU-1</text>") && svg.contains(">SKU-2</text>"));
        assert!(
            svg.contains("alarm") && svg.contains("warning"),
            "severity legend"
        );
        let e = go("events", r#"[{"kind":"x"}]"#, "{}")
            .unwrap_err()
            .to_string();
        assert!(e.contains("needs an event time"), "{e}");
    }

    const FOLDS: &str = r#"[
        {"fold_id":1,"split":"train","period":0},{"fold_id":1,"split":"train","period":1},
        {"fold_id":1,"split":"test","period":2},
        {"fold_id":2,"split":"train","period":0},{"fold_id":2,"split":"train","period":1},
        {"fold_id":2,"split":"train","period":2},{"fold_id":2,"split":"test","period":3},
        {"fold_id":3,"split":"train","period":0},{"fold_id":3,"split":"train","period":1},
        {"fold_id":3,"split":"train","period":2},{"fold_id":3,"split":"train","period":3},
        {"fold_id":3,"split":"test","period":4}]"#;

    #[test]
    fn cv_fold_timeline() {
        assert_eq!(detect(&rows(FOLDS)).unwrap(), "cv");
        let svg = go("auto", FOLDS, "{}").unwrap();
        dump("cv", &svg);
        assert!(
            svg.contains(">fold 1</text>") && svg.contains(">fold 3</text>"),
            "{svg}"
        );
        assert!(!svg.contains("data-warnings"), "{svg}");
        // A table with fold_id, split, yhat and model_name is still a
        // prediction (ts_cv_forecast_by's backtest output), not cv.
        assert_eq!(
            detect(&rows(
                r#"[{"fold_id":1,"id":"a","ds":"2024-01-01","y":1,"split":"test","yhat":1.1,"model_name":"AutoETS"}]"#
            ))
            .unwrap(),
            "prediction"
        );
    }

    #[test]
    fn docs_alias_table_is_in_sync() {
        let doc = include_str!("../docs/DOCS.md");
        let start = "<!-- contract-aliases:start -->\n";
        let a = doc.find(start).expect("start marker") + start.len();
        let b = doc
            .find("<!-- contract-aliases:end -->")
            .expect("end marker");
        let want = names::markdown_table();
        assert_eq!(
            &doc[a..b],
            want,
            "docs/DOCS.md contract alias table is stale — replace it with:\n{want}"
        );
    }

    #[test]
    fn empty_input_is_a_note() {
        for plot in ["terms", "auto"] {
            let s = go(plot, "[]", "{}").unwrap();
            assert!(s.contains("No data"), "{plot}");
        }
    }
}
