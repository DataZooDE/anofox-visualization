//! The SQL macros bundled with the DuckDB extension — the **single source**
//! for both builds:
//!
//! * the C++ community build (`csrc/`) reads this table through the FFI
//!   accessors in `crates/anofox-viz-ffi` and registers each entry as an
//!   internal macro with its description, example and tags;
//! * the C-API build (`duckext/`) runs `CREATE OR REPLACE MACRO` with
//!   [`MacroSpec::create_sql`] (that API cannot attach descriptions/tags).
//!
//! So a macro body can never drift between the builds.
//!
//! Two shapes:
//! * **column macros** (`anofox_xy(x, y)` …) aggregate a few columns;
//! * **contract macros** (`anofox_plot_terms(tbl)` …) aggregate *whole rows*:
//!   `SELECT anofox_plot_terms(c) FROM coefs c` passes each row as a STRUCT,
//!   `to_json(list(tbl))` keeps the column names and the producer's own row
//!   order (#445), and the renderer ([`crate::contract`]) reads the contract
//!   columns by name. One
//!   SVG per group (`GROUP BY model_id`), no table-name strings, no
//!   introspection SQL — works in both builds.

/// The anofox integration-contract version the plot macros consume — what
/// `anofox_contract_version()` returns. Both builds register that scalar from
/// this constant (unless another anofox extension, e.g. anofox_statistics,
/// already did: the function is shared by the whole family).
pub use crate::contract::CONTRACT_VERSION;

/// One bundled macro.
#[derive(Debug)]
pub struct MacroSpec {
    pub name: &'static str,
    /// Positional parameters.
    pub params: &'static [&'static str],
    /// Named parameters with their default (SQL expression).
    pub named: &'static [(&'static str, &'static str)],
    /// The macro body (one SQL expression).
    pub body: &'static str,
    pub description: &'static str,
    pub example: &'static str,
    /// `duckdb_functions().tags` (C++ build).
    pub tags: &'static [(&'static str, &'static str)],
}

/// `row` collides with the `ROW(...)` constructor keyword: unquoted, DuckDB's
/// parser rejects it in a parameter-list position (`(tbl, row := NULL)`) and
/// in a named-argument call (`f(row := 1)`) — but not as a bare expression
/// reference inside a macro body (`json_object('row', row)` parses fine), so
/// only the declaration (and any caller using the named form) needs quoting.
fn quote_reserved(name: &str) -> String {
    if name == "row" {
        format!("\"{name}\"")
    } else {
        name.to_string()
    }
}

impl MacroSpec {
    /// `(p, q := default, …) AS body` — the tail of a `CREATE MACRO name…`.
    pub fn create_sql(&self) -> String {
        let mut ps: Vec<String> = self.params.iter().map(|p| quote_reserved(p)).collect();
        ps.extend(
            self.named
                .iter()
                .map(|(n, d)| format!("{} := {d}", quote_reserved(n))),
        );
        format!("({}) AS {}", ps.join(", "), self.body)
    }
}

// Column macros. The role list is built with json_array(), so `kind` is
// JSON-quoted rather than spliced into a JSON string (a quote in `kind` cannot
// break the spec); a NULL kind becomes a JSON null, which anofox_render
// rejects. list(... ORDER BY ...) makes row order deterministic under parallel
// aggregation.
const XY: &str = "anofox_render(json_object('rows', to_json(list({c0: x, c1: y} ORDER BY x, y)), \
'roles', json_array(json_array(0, 'XAXIS'), json_array(1, kind)), 'theme', coalesce(theme, getvariable('anofox_theme')), 'locale', coalesce(locale, getvariable('anofox_locale')), 'width', width, 'height', height))";
const XYC: &str = "anofox_render(json_object('rows', to_json(list({c0: x, c1: y, c2: series} ORDER BY x, series, y)), \
'roles', json_array(json_array(0, 'XAXIS'), json_array(1, kind), json_array(2, 'CATEGORY')), \
'theme', coalesce(theme, getvariable('anofox_theme')), 'locale', coalesce(locale, getvariable('anofox_locale')), 'width', width, 'height', height))";

const CONTRACT_TAG: (&str, &str) = ("anofox.contract", "1");

// `list(tbl)` — no ORDER BY — keeps the producer's own row order (#445).
// `tbl` is an opaque per-row STRUCT built by the caller (`SELECT
// anofox_plot_terms(c) FROM coefs c`): the macro never knows its field set,
// so it cannot reference a named column (`tbl.index_value`) — DuckDB raises
// a bind error for any caller whose row lacks that field, and most contract
// columns besides the aggregate's own grouping keys are optional. That rules
// out an ordinal-column ORDER BY here (anofox_plot_terms's `path()` renderer
// still re-sorts by `index_value`/`window_end` itself once the rows arrive,
// for exactly this reason). A window function (`row_number() OVER ()`) can't
// stand in either: DuckDB rejects a window call inside an aggregate's own
// argument/ORDER BY list ("aggregate function calls cannot contain window
// function calls"), and computing one in a separate SELECT stage would need
// to wrap the *caller's* FROM clause, which this single-expression macro
// body has no access to.
//
// `list(tbl ORDER BY tbl)` (removed here) sorted by the whole row STRUCT,
// which — once the leading, usually-constant grouping fields (model_id,
// model_name, ...) tie — falls through to the first field that varies
// (`term`, `test_id`, `metric`, ...) and alphabetises by it: deterministic,
// but not the producer's order (#445, found via #443's mediation_by case).
// `list(tbl)` instead leans on DuckDB preserving scan order, which holds
// whenever the whole aggregation runs on one thread (measured empirically:
// true through several hundred thousand rows; DuckDB's parallel hash
// aggregate can interleave threads' partial lists above that, same as it
// would for the removed `ORDER BY tbl` once ties also stopped being fully
// ordered, e.g. two models sharing every grouping column). Every table this
// macro family renders is a human-scale summary (model terms, test rows,
// metrics, ...), never a per-observation dump at that size, so this holds in
// practice; a producer-side ordinal (a `row_order` column, say) is the only
// way to make it unconditional — tracked as a follow-up rather than blocking
// this fix (DataZooDE/anofox-visualization#445).
macro_rules! contract_body {
    ($plot:literal) => {
        concat!(
            "anofox_render(json_object('plot', '",
            $plot,
            "', 'rows', to_json(list(tbl)), 'theme', coalesce(theme, getvariable('anofox_theme')), 'locale', coalesce(locale, getvariable('anofox_locale')), 'width', width, 'height', height))"
        )
    };
    ($plot:literal, $opts:literal) => {
        concat!(
            "anofox_render(json_object('plot', '",
            $plot,
            "', 'rows', to_json(list(tbl)), 'options', json_object(",
            $opts,
            "), 'theme', coalesce(theme, getvariable('anofox_theme')), 'locale', coalesce(locale, getvariable('anofox_locale')), 'width', width, 'height', height))"
        )
    };
}

/// Every bundled macro.
pub static MACROS: &[MacroSpec] = &[
    MacroSpec {
        name: "anofox_xy",
        params: &["x", "y"],
        named: &[("kind", "'BARCHART'"), ("theme", "NULL"), ("locale", "NULL"), ("width", "640"), ("height", "400")],
        body: XY,
        description: "Aggregate two columns into a single-series chart and render it as SVG. 'kind' selects the mark \
(BARCHART, LINECHART, SCATTER, AREACHART); x becomes the axis and y the value.",
        example: "SELECT anofox_xy(x, y, kind := 'LINECHART') FROM (VALUES ('Jan', 10), ('Feb', 20)) t(x, y)",
        tags: &[],
    },
    MacroSpec {
        name: "anofox_xyc",
        params: &["x", "y", "series"],
        named: &[("kind", "'BARCHART_STACKED'"), ("theme", "NULL"), ("locale", "NULL"), ("width", "640"), ("height", "400")],
        body: XYC,
        description: "Aggregate three columns into a multi-series chart and render it as SVG, with 'series' splitting the \
data into categories.",
        example: "SELECT anofox_xyc(x, y, s) FROM (VALUES ('Jan', 10, 'EU'), ('Jan', 8, 'US')) t(x, y, s)",
        tags: &[],
    },
    MacroSpec {
        name: "anofox_bar",
        params: &["x", "y"],
        named: &[],
        body: "anofox_xy(x, y, kind := 'BARCHART')",
        description: "Render a bar chart of two columns as SVG. Shorthand for anofox_xy(x, y, kind := 'BARCHART').",
        example: "SELECT anofox_bar(x, y) FROM (VALUES ('Jan', 10), ('Feb', 20)) t(x, y)",
        tags: &[],
    },
    MacroSpec {
        name: "anofox_line",
        params: &["x", "y"],
        named: &[],
        body: "anofox_xy(x, y, kind := 'LINECHART')",
        description: "Render a line chart of two columns as SVG. Shorthand for anofox_xy(x, y, kind := 'LINECHART').",
        example: "SELECT anofox_line(x, y) FROM (VALUES ('Jan', 10), ('Feb', 20)) t(x, y)",
        tags: &[],
    },
    MacroSpec {
        name: "anofox_scatter",
        params: &["x", "y"],
        named: &[],
        body: "anofox_xy(x, y, kind := 'SCATTER')",
        description: "Render a scatter plot of two columns as SVG. Shorthand for anofox_xy(x, y, kind := 'SCATTER').",
        example: "SELECT anofox_scatter(x, y) FROM (VALUES (1.5, 10), (2.5, 20)) t(x, y)",
        tags: &[],
    },
    MacroSpec {
        name: "anofox_area",
        params: &["x", "y"],
        named: &[],
        body: "anofox_xy(x, y, kind := 'AREACHART')",
        description: "Render an area chart of two columns as SVG. Shorthand for anofox_xy(x, y, kind := 'AREACHART').",
        example: "SELECT anofox_area(x, y) FROM (VALUES ('Jan', 10), ('Feb', 20)) t(x, y)",
        tags: &[],
    },
    // ── contract macros (pass the whole row: `SELECT f(t) FROM tbl t`) ──────
    MacroSpec {
        name: "anofox_plot_terms",
        params: &["tbl"],
        named: &[("highlight", "NULL"), ("lambda_min", "NULL"), ("lambda_1se", "NULL"), ("facet", "NULL"), ("reference", "NULL"), ("title", "NULL"), ("x_label", "NULL"), ("y_label", "NULL"), ("theme", "NULL"), ("locale", "NULL"), ("width", "640"), ("height", "400")],
        body: contract_body!("terms", "'highlight', highlight, 'lambda_min', lambda_min, 'lambda_1se', lambda_1se, 'facet', facet, 'reference', reference, 'title', title, 'x_label', x_label, 'y_label', y_label"),
        description: "Coefficient plot of a `terms` (tidy) table, aggregated over its rows: pass the row, \
`SELECT anofox_plot_terms(c) FROM coefs c`. Columns: term, estimate, conf_low, conf_high (optional), model_id \
(several models are dodged and coloured). With index_value (+ index_name) it draws each term's estimate and band \
over the index instead: a λ path in one panel with direct labels (highlight := [...], lambda_min := / lambda_1se := \
mark λ.min / λ.1se, facet := true for a panel per term), a LARS path on the |β|₁ fraction, the quantile process \
and rolling coefficients (window_end dates) per term with a full-sample reference (reference := a terms LIST). \
title := / x_label := / y_label := label the chart (no inner title by default).",
        example: "SELECT anofox_plot_terms(c) FROM (VALUES ('x1', 0.5, 0.2, 0.8), ('x2', -0.3, -0.7, 0.1)) \
c(term, estimate, conf_low, conf_high)",
        tags: &[("anofox.consumes", "terms"), CONTRACT_TAG],
    },
    MacroSpec {
        name: "anofox_plot_prediction",
        params: &["tbl"],
        named: &[("x", "NULL"), ("y", "NULL"), ("facet", "NULL"), ("band", "NULL"), ("kind", "NULL"), ("events", "NULL"), ("title", "NULL"), ("x_label", "NULL"), ("y_label", "NULL"), ("theme", "NULL"), ("locale", "NULL"), ("width", "640"), ("height", "400")],
        body: contract_body!("prediction", "'x', x, 'y', y, 'facet', facet, 'band', band, 'kind', kind, 'events', events, 'title', title, 'x_label', x_label, 'y_label', y_label"),
        description: "Fit / forecast plot of a `prediction` table: observed y as points, the yhat line and the \
yhat_lower..yhat_upper band over x (or ds, or an integer period), a nested fan when there are several `level`s, \
split boundaries as labelled vertical lines (train/test/future; derived from is_training or a \
missing y), or by model_id/model_name when there are several models. x := / y := name other columns (e.g. \
x := 'x1' for a *_fit_predict_by output); several `id` series become panels (facet := overrides). \
events := (SELECT list(e) FROM alarms e) draws an `events` table (ds | period, kind, label, severity) as \
vertical lines coloured by severity. statsforecast's wide <model>-lo-<L> / -hi-<L> columns are read as one \
row per model and level.",
        example: "SELECT anofox_plot_prediction(p) FROM (VALUES (1, 1.0, 1.1, 0.9, 1.3), (2, 2.0, 1.9, 1.6, 2.2), \
(3, NULL, 3.1, 2.6, 3.6)) p(x, y, yhat, yhat_lower, yhat_upper)",
        tags: &[("anofox.consumes", "prediction"), CONTRACT_TAG],
    },
    MacroSpec {
        name: "anofox_plot_curve",
        params: &["tbl"],
        named: &[("base_rate", "NULL"), ("title", "NULL"), ("x_label", "NULL"), ("y_label", "NULL"), ("theme", "NULL"), ("locale", "NULL"), ("width", "640"), ("height", "400")],
        body: contract_body!("curve", "'base_rate', base_rate, 'title', title, 'x_label', x_label, 'y_label', y_label"),
        description: "Plot a `curve` table; the geom follows curve_type: roc/calibration (line + diagonal), pr/pdp/lift/lowess \
(line), km (step), acf/pacf (lollipops + bounds from y_low/y_high), lambda_cv (point ± y_low..y_high on a log x), \
nested_cv (outer-fold points + a mean ± SE band), qq (points + identity). One curve_type per call (GROUP BY curve_type); \
several model_id/series are coloured.",
        example: "SELECT anofox_plot_curve(r) FROM (VALUES ('roc', 0.0, 0.0), ('roc', 0.2, 0.7), ('roc', 1.0, 1.0)) \
r(curve_type, x, y)",
        tags: &[("anofox.consumes", "curve"), CONTRACT_TAG],
    },
    MacroSpec {
        name: "anofox_plot_summary",
        params: &["tbl"],
        named: &[("metrics", "NULL"), ("baseline", "NULL"), ("title", "NULL"), ("x_label", "NULL"), ("y_label", "NULL"), ("theme", "NULL"), ("locale", "NULL"), ("width", "640"), ("height", "400")],
        body: contract_body!("summary", "'metrics', metrics, 'baseline', baseline, 'title', title, 'x_label', x_label, 'y_label', y_label"),
        description: "Model comparison from a `summary` (glance) table: a grid of small panels, one per metric \
with its own scale, the models (model_id, or model_id · id for several groups) on the vertical axis, the value as \
a dot (± conf_low..conf_high) with its number beside it and the best model highlighted for R², AIC, BIC, RMSE, .... \
Bookkeeping metrics (n_observations, n_features, iterations) are left out unless metrics := ['r_squared', \
'n_observations', ...] picks the panels.",
        example: "SELECT anofox_plot_summary(s) FROM (VALUES ('ols', 'r_squared', 0.81), ('ridge', 'r_squared', 0.79), \
('ols', 'aic', 72.4), ('ridge', 'aic', 75.0)) s(model_id, metric, value)",
        tags: &[("anofox.consumes", "summary"), CONTRACT_TAG],
    },
    MacroSpec {
        name: "anofox_plot_obs",
        params: &["tbl"],
        named: &[("kind", "'resid_fitted'"), ("label_top", "3"), ("title", "NULL"), ("x_label", "NULL"), ("y_label", "NULL"), ("theme", "NULL"), ("locale", "NULL"), ("width", "640"), ("height", "400")],
        body: contract_body!("obs", "'kind', kind, 'label_top', label_top, 'title', title, 'x_label', x_label, 'y_label', y_label"),
        description: "One regression diagnostic panel from an `obs` (augment) table. kind: resid_fitted (residual vs \
fitted + loess), qq (normal QQ of std_residual), scale_location (sqrt|std_residual| vs fitted), leverage \
(std_residual vs leverage, 2p/n line when n_params is set) or cooks (Cook's distance by row_id with the 4/n line). \
The label_top rows with the largest cooks_d are labelled by row_id.",
        example: "SELECT anofox_plot_obs(o, kind := 'qq') FROM (VALUES (1, 1.0, 0.1), (2, 2.0, -0.3), (3, 3.0, 0.2)) \
o(row_id, fitted, residual)",
        tags: &[("anofox.consumes", "obs"), CONTRACT_TAG],
    },
    MacroSpec {
        name: "anofox_plot_diagnostics",
        params: &["tbl"],
        named: &[("label_top", "3"), ("title", "NULL"), ("x_label", "NULL"), ("y_label", "NULL"), ("theme", "NULL"), ("locale", "NULL"), ("width", "820"), ("height", "640")],
        body: contract_body!("diagnostics", "'label_top', label_top, 'title', title, 'x_label', x_label, 'y_label', y_label"),
        description: "The plot.lm 2x2 from an `obs` (augment) table as one SVG: residuals vs fitted, normal QQ, \
scale-location and residuals vs leverage (or Cook's distance).",
        example: "SELECT anofox_plot_diagnostics(o) FROM (VALUES (1, 1.0, 0.1), (2, 2.0, -0.3), (3, 3.0, 0.2), \
(4, 4.0, 0.0)) o(row_id, fitted, residual)",
        tags: &[("anofox.consumes", "obs"), CONTRACT_TAG],
    },
    MacroSpec {
        name: "anofox_plot_tests",
        params: &["tbl"],
        named: &[("alpha", "0.05"), ("value", "NULL"), ("label", "NULL"), ("title", "NULL"), ("x_label", "NULL"), ("y_label", "NULL"), ("theme", "NULL"), ("locale", "NULL"), ("width", "640"), ("height", "400")],
        body: contract_body!("tests", "'alpha', alpha, 'value', value, 'label', label, 'title', title, 'x_label', x_label, 'y_label', y_label"),
        description: "Comparison forest of a `test` table (tukey_hsd_by, pairwise_t_test_by, dunn_test_by, or an \
unnested *_test_agg STRUCT): one row per test (test_id, else 'group1 vs group2', else the group / method), the \
estimate with conf_low..conf_high when the test has one, else the statistic, else -log10(p), coloured by \
significance at alpha, the (adjusted) p-value and stars beside each row. Several groups (id, model_id or the \
GROUP BY column of an unnested aggregate) get one panel each. Several methods (ADF and KPSS) get one panel per \
method (own x, groups on a shared y) unless the axis is -log10(p). value := 'estimate' | 'statistic' | 'p' overrides \
the measure. A column holding the aggregate's STRUCT or LIST is expanded: \
SELECT anofox_plot_tests(s) FROM (SELECT tukey_hsd_agg(y, g) AS r FROM t) s.",
        example: "SELECT anofox_plot_tests(t) FROM (VALUES ('b-a', 'a', 'b', 1.2, 0.4, 2.0, 0.003), \
('c-a', 'a', 'c', 0.3, -0.5, 1.1, 0.41)) t(test_id, group1, group2, estimate, conf_low, conf_high, p_adj)",
        tags: &[("anofox.consumes", "test"), CONTRACT_TAG],
    },
    MacroSpec {
        name: "anofox_plot_compare",
        params: &["tbl"],
        named: &[("value", "NULL"), ("factor", "NULL"), ("tests", "NULL"), ("kind", "'box'"), ("hide_ns", "NULL"), ("label", "NULL"), ("title", "NULL"), ("x_label", "NULL"), ("y_label", "NULL"), ("theme", "NULL"), ("locale", "NULL"), ("width", "640"), ("height", "400")],
        body: contract_body!("compare", "'value', value, 'factor', factor, 'tests', tests, 'kind', kind, 'hide_ns', hide_ns, 'label', label, 'title', title, 'x_label', x_label, 'y_label', y_label"),
        description: "Compare a numeric column across groups with significance brackets (ggpubr's \
stat_pvalue_manual): pass the raw rows, value := '<numeric column>', factor := '<group column>' and \
tests := a LIST of test STRUCTs — a subquery (SELECT list(t) FROM tukey_hsd_by(...) t WHERE ...), or the \
aggregate itself (tukey_hsd_agg(y, g), pairwise_t_test_agg, dunn_test_agg; a two-sample STRUCT such as \
t_test_agg(y, g) for two groups). kind: box (default), violin or points (jitter + group means). Brackets are \
labelled '{p.format} {p.signif}' from p_adj (else p_value); label := '{p.signif}' / 'p = {p_adj}' changes it, \
hide_ns drops non-significant pairs (default: above 3 groups); label := 'stars' | 'p' | a template.",
        example: "SELECT anofox_plot_compare(r, value := 'y', factor := 'g', tests := [{'group1': 'a', 'group2': 'b', \
'p_adj': 0.01}]) FROM (VALUES ('a', 1.0), ('a', 1.4), ('b', 2.1), ('b', 2.6)) r(g, y)",
        tags: &[("anofox.consumes", "test"), CONTRACT_TAG],
    },
    MacroSpec {
        name: "anofox_plot_partial",
        params: &["tbl"],
        named: &[("kind", "'component'"), ("title", "NULL"), ("x_label", "NULL"), ("y_label", "NULL"), ("theme", "NULL"), ("locale", "NULL"), ("width", "640"), ("height", "400")],
        body: contract_body!("partial", "'kind', kind, 'title', title, 'x_label', x_label, 'y_label', y_label"),
        description: "Partial-residual plots of a <model>_partial_residuals_by table, one panel per term. \
kind := 'component' (component + residual: partial_residual over x, the term's component line and a loess) or \
'added_variable' (y_residual over x_residual with the least-squares line through the origin, whose slope is the \
term's coefficient).",
        example: "SELECT anofox_plot_partial(p) FROM (VALUES ('x1', 1.0, 0.4, 0.5), ('x1', 2.0, 1.1, 1.0), \
('x1', 3.0, 1.4, 1.5)) p(term, x, partial_residual, component)",
        tags: &[("anofox.consumes", "partial_residuals"), CONTRACT_TAG],
    },
    MacroSpec {
        name: "anofox_plot_cv",
        params: &["tbl"],
        named: &[("x", "NULL"), ("title", "NULL"), ("x_label", "NULL"), ("y_label", "NULL"), ("theme", "NULL"), ("locale", "NULL"), ("width", "640"), ("height", "260")],
        body: contract_body!("cv", "'x', x, 'title', title, 'x_label', x_label, 'y_label', y_label"),
        description: "Cross-validation layout of a fold table (provisional schema): one lane per fold_id, the \
train and test windows as bars along ds (or an integer period), so the number of folds, where each training \
window ends and whether the test windows overlap or leave gaps (said under the plot) show at a glance. Reads \
ts_cv_folds_by / ts_cv_split_by (id, ds, fold_id, split) and ts_cv_split_folds_by (fold_id, train_start, \
train_end, test_start, test_end) as they come. Series with identical folds are drawn once; different folds get \
a panel per id (up to 6).",
        example: "SELECT anofox_plot_cv(f) FROM (SELECT 1 AS fold_id, 'train' AS split, i AS period FROM range(10) r(i) \
UNION ALL SELECT 1, 'test', i FROM range(10, 13) r(i)) f",
        tags: &[("anofox.consumes", "cv"), CONTRACT_TAG],
    },
    MacroSpec {
        name: "anofox_plot_schedule",
        params: &["tbl"],
        named: &[("title", "NULL"), ("x_label", "NULL"), ("y_label", "NULL"), ("theme", "NULL"), ("locale", "NULL"), ("width", "640"), ("height", "400")],
        body: contract_body!("schedule", "'title', title, 'x_label', x_label, 'y_label', y_label"),
        description: "Gantt chart of a `schedule` table: one lane per resource, a bar per task from \
start to end, coloured by status (on time/late, else muted), a late task's connector to its due \
date, faceted by model_id (algorithm) when there are several (up to 6; else GROUP BY model_id or \
WHERE model_id = ...). Columns: resource, task, start, end, model_id, due, status — one row per \
task. Replaces drawing a schedule as a scatter with xmin/xmax error bars.",
        example: "SELECT anofox_plot_schedule(s) FROM (VALUES ('L1', 'O1', 0, 10, 'on time'), \
('L1', 'O2', 10, 25, 'late'), ('L2', 'O3', 0, 15, 'on time')) s(resource, task, start, \"end\", status)",
        tags: &[("anofox.consumes", "schedule"), CONTRACT_TAG],
    },
    MacroSpec {
        name: "anofox_plot_events",
        params: &["tbl"],
        named: &[("x", "NULL"), ("title", "NULL"), ("x_label", "NULL"), ("y_label", "NULL"), ("theme", "NULL"), ("locale", "NULL"), ("width", "640"), ("height", "260")],
        body: contract_body!("events", "'x', x, 'title', title, 'x_label', x_label, 'y_label', y_label"),
        description: "Timeline of an `events` table (alarms, parameter changes, calibration proposals): a dot per \
event at its ds (or integer period) in one lane per series (else per kind), coloured by severity (alarm red, \
warning amber), with kind, label, value and threshold on hover. To draw the events over a forecast, pass them \
to anofox_plot_prediction(p, events := (SELECT list(e) FROM alarms e)).",
        example: "SELECT anofox_plot_events(e) FROM (VALUES (3, 'stockout', 'alarm', 'on hand 0'), \
(7, 'drift', 'warning', 'CUSUM 4.2')) e(period, kind, severity, label)",
        tags: &[("anofox.consumes", "events"), CONTRACT_TAG],
    },
    MacroSpec {
        name: "anofox_plot_checks",
        params: &["tbl"],
        named: &[("x", "NULL"), ("kind", "NULL"), ("title", "NULL"), ("x_label", "NULL"), ("y_label", "NULL"), ("theme", "NULL"), ("locale", "NULL"), ("width", "640"), ("height", "400")],
        body: contract_body!("checks", "'x', x, 'kind', kind, 'title', title, 'x_label', x_label, 'y_label', y_label"),
        description: "Data-quality checks from a `check` table (anofox-tabular run_checks / agg_check as they come; \
the other checks with their measure as value): one panel per check (own scale), the value as a dot coloured by \
status (fail red, warn amber, pass green) against the lower_threshold..upper_threshold acceptance band, one lane \
per identifier, the message on hover. Over several run_ts / ds values (or kind := 'strip') a status strip: one \
lane per check, a dot per run. x := names the time column.",
        example: "SELECT anofox_plot_checks(c) FROM (VALUES ('rows', 'pass', 120.0, 100.0, NULL), \
('customer present', 'fail', 0.083, NULL, 0.05)) c(check_name, status, value, lower_threshold, upper_threshold)",
        tags: &[("anofox.consumes", "check"), CONTRACT_TAG],
    },
    MacroSpec {
        name: "anofox_plot_decomposition",
        params: &["tbl"],
        named: &[("x", "NULL"), ("components", "NULL"), ("title", "NULL"), ("x_label", "NULL"), ("y_label", "NULL"), ("theme", "NULL"), ("locale", "NULL"), ("width", "640"), ("height", "560")],
        body: contract_body!("decomposition", "'x', x, 'components', components, 'title', title, 'x_label', x_label, 'y_label', y_label"),
        description: "Stacked panels of a time-series decomposition sharing one x axis, each with its own y scale: \
observed, level, trend, each seasonal (seasonal_<period>), remainder (as points). Reads the long `decomposition` \
table (ds | period, component, value), wide columns (observed, trend, seasonal, remainder; anofox-orchestration's \
decomposition output) and anofox-forecast's list outputs as they come: ts_mstl_decomposition_by (trend[], \
seasonal[][], periods[]; x = the period index) and ts_forecast_explain_by (its decomposition STRUCT). One series \
per call (GROUP BY id). components := ['trend', 'seasonal_7'] picks and orders the panels. Provisional: the \
long-form column names are to be agreed with anofox-orchestration and anofox-forecast.",
        example: "SELECT anofox_plot_decomposition(d) FROM (SELECT i AS period, 'trend' AS component, 0.5 * i AS value \
FROM range(20) r(i) UNION ALL SELECT i, 'seasonal', sin(i) FROM range(20) r(i)) d",
        tags: &[("anofox.consumes", "decomposition"), CONTRACT_TAG],
    },
    MacroSpec {
        name: "anofox_plot_matrix",
        params: &["tbl"],
        // `row` is a reserved word (the ROW(...) constructor): quoted by
        // `create_sql()` ([`quote_reserved`]) in the declaration; callers of
        // the C-API build override it as `"row" := …`. The FFI/C++ build
        // registers the bare name directly (bypassing the SQL parser), and
        // the body's bare `row` reference parses fine as an expression.
        named: &[("row", "NULL"), ("col", "NULL"), ("value", "NULL"), ("domain", "NULL"), ("mirror", "NULL"), ("labels", "NULL"), ("title", "NULL"), ("x_label", "NULL"), ("y_label", "NULL"), ("theme", "NULL"), ("locale", "NULL"), ("width", "520"), ("height", "440")],
        body: contract_body!("matrix", "'row', row, 'col', col, 'value', value, 'domain', domain, 'mirror', mirror, 'labels', labels, 'title', title, 'x_label', x_label, 'y_label', y_label"),
        description: "Heatmap of a `matrix` table (row, col, value; anofox-tabular profile_correlations as it \
comes: column_a x column_b, pearson): one tile per cell, a diverging scale around 0, -1..1 for correlations \
(domain := [lo, hi] overrides), a pair list mirrored to the full matrix with a diagonal of 1 (mirror := false \
keeps it as given). Cell values are written when there are at most 144 cells (labels := overrides). value := \
picks another measure (value := 'spearman'). Confusion and setup matrices (row, col already the full grid) are \
drawn the same way, without mirroring.",
        example: "SELECT anofox_plot_matrix(m) FROM (VALUES ('a', 'b', 0.4), ('a', 'c', -0.2), ('b', 'c', 0.7)) \
m(row, col, value)",
        tags: &[("anofox.consumes", "matrix"), CONTRACT_TAG],
    },
    MacroSpec {
        name: "anofox_plot_diagram",
        params: &["tbl"],
        named: &[("edges", "NULL"), ("id", "NULL"), ("mainstat", "NULL"), ("format", "NULL"), ("source", "NULL"), ("target", "NULL"), ("value", "NULL"), ("direction", "NULL"), ("style", "NULL"), ("theme", "NULL"), ("locale", "NULL"), ("width", "800"), ("height", "400")],
        body: contract_body!("diagram", "'edges', edges, 'id', id, 'mainstat', mainstat, 'format', format, 'source', source, 'target', target, 'value', value, 'direction', direction, 'style', style"),
        description: "A pipeline, lineage or process diagram from a `graph` table (one row per node: id, title, \
mainstat, status, group, lane, story, x, y, sparkline, seq) and its edges := a LIST of edge STRUCTs (source, \
target, value, label, style, status) — a subquery (SELECT list(e) FROM edges e). Grafana node-graph names; \
producer spellings are read too (stage / label / from_stage / to_stage / count, …), or name the columns: \
id :=, mainstat :=, source :=, target :=, value :=. format := 'compact' (default) | 'money' | \
'percent' | 'metric' for mainstat; direction := 'LR' | 'TB'; style := the edges' default ('flow', \
'flow-volume', …). Edges are as wide as their value and coloured by status; loops run back.",
        example: "SELECT anofox_plot_diagram(n, edges := [{'source': 'a', 'target': 'b', 'value': 3}]) \
FROM (VALUES ('a', 'Load', 10), ('b', 'Clean', 8)) n(id, title, mainstat)",
        tags: &[("anofox.consumes", "graph"), CONTRACT_TAG],
    },
    MacroSpec {
        name: "anofox_plot_assignment",
        params: &["tbl"],
        named: &[("x", "NULL"), ("title", "NULL"), ("x_label", "NULL"), ("y_label", "NULL"), ("theme", "NULL"), ("locale", "NULL"), ("width", "640"), ("height", "400")],
        body: contract_body!("assignment", "'x', x, 'title', title, 'x_label', x_label, 'y_label', y_label"),
        description: "An optimizer's item -> bin/wave assignment from an `assignment` table (anofox-optimize's \
pack_* / wave_* as a long-form bridge): one row per item (model_id, bin | wave, item, size, capacity). Items are \
stacked per bin/wave (CATEGORY: item, BARCHART_STACKED: size), the capacity drawn as a quiet reference line so the \
fill level is visible; several model_id's are faceted to compare algorithms. x := overrides bin/wave when both are \
present.",
        example: "SELECT anofox_plot_assignment(a) FROM (VALUES (1, 'x1', 3, 10), (1, 'x2', 4, 10), (2, 'x3', 6, 10)) \
a(bin, item, size, capacity)",
        tags: &[("anofox.consumes", "assignment"), CONTRACT_TAG],
    },
    MacroSpec {
        name: "anofox_plot_distribution",
        params: &["tbl"],
        named: &[("title", "NULL"), ("x_label", "NULL"), ("y_label", "NULL"), ("theme", "NULL"), ("locale", "NULL"), ("width", "640"), ("height", "400")],
        body: contract_body!("distribution", "'title', title, 'x_label', x_label, 'y_label', y_label"),
        description: "A categorical distribution from a `distribution` table (anofox-decide's decide_choice_distribution \
MAP / decide_table's distribution column, reached with `SELECT t.question_id AS id, u.e.key AS option, u.e.value AS \
probability FROM t, unnest(map_entries(t.distribution)) AS u(e)`; question_id and model are also read as id and \
model_id): one row per option (id, option, probability, model_id optional). One id draws a plain bar per option; \
several ids draw a 100% stacked bar per id (CATEGORY: option). Several model_id's are faceted to compare models.",
        example: "SELECT anofox_plot_distribution(d) FROM (VALUES ('q1', 'yes', 0.7), ('q1', 'no', 0.3)) \
d(id, option, probability)",
        tags: &[("anofox.consumes", "distribution"), CONTRACT_TAG],
    },
    MacroSpec {
        name: "anofox_plot",
        params: &["tbl"],
        named: &[("title", "NULL"), ("x_label", "NULL"), ("y_label", "NULL"), ("theme", "NULL"), ("locale", "NULL"), ("width", "640"), ("height", "400")],
        body: contract_body!("auto", "'title', title, 'x_label', x_label, 'y_label', y_label"),
        description: "Plot any anofox contract table (contract version 1), choosing the chart from its columns: \
term+estimate -> terms, curve_type -> curve, yhat -> prediction, fitted+residual -> diagnostics 2x2, \
metric+value -> summary, check_name|thresholds + status + value -> checks, partial_residual -> partial \
(component + residual), fold_id + (split | test_start | train_end) -> cv (provisional), resource+start+end \
-> schedule (Gantt), component+value | trend -> decomposition, p_value/test_id -> tests, row+col | \
column_a+column_b -> matrix, kind|severity + ds|period -> events, (bin|wave)+item+size -> assignment, \
option+probability -> distribution. Use the specific anofox_plot_* macro \
for options.",
        example: "SELECT anofox_plot(c) FROM (VALUES ('x1', 0.5, 0.2, 0.8)) c(term, estimate, conf_low, conf_high)",
        tags: &[
            (
                "anofox.consumes",
                "terms,prediction,curve,summary,obs,test,partial_residuals,cv,schedule,decomposition,events,check,matrix,assignment,distribution",
            ),
            CONTRACT_TAG,
        ],
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_sql_shape() {
        let xy = MACROS.iter().find(|m| m.name == "anofox_xy").unwrap();
        assert_eq!(
            xy.create_sql(),
            format!(
                "(x, y, kind := 'BARCHART', theme := NULL, locale := NULL, width := 640, height := 400) AS {XY}"
            )
        );
        let t = MACROS
            .iter()
            .find(|m| m.name == "anofox_plot_terms")
            .unwrap();
        assert!(t
            .create_sql()
            .starts_with("(tbl, highlight := NULL, lambda_min := NULL, lambda_1se := NULL"));
        let p = MACROS.iter().find(|m| m.name == "anofox_plot").unwrap();
        assert!(p.create_sql().starts_with(
            "(tbl, title := NULL, x_label := NULL, y_label := NULL, theme := NULL, locale := NULL, width := 640, height := 400) AS anofox_render("
        ));
        // `row` collides with the ROW(...) constructor keyword: the generated
        // SQL quotes the parameter so CREATE MACRO parses.
        let m = MACROS
            .iter()
            .find(|m| m.name == "anofox_plot_matrix")
            .unwrap();
        assert!(
            m.create_sql()
                .starts_with("(tbl, \"row\" := NULL, col := NULL, value := NULL"),
            "{}",
            m.create_sql()
        );
        assert_eq!(quote_reserved("row"), "\"row\"");
        assert_eq!(quote_reserved("col"), "col");
    }

    #[test]
    fn names_unique_and_documented() {
        let mut seen = std::collections::HashSet::new();
        for m in MACROS {
            assert!(seen.insert(m.name), "{}", m.name);
            assert!(!m.description.is_empty() && m.example.contains(m.name));
            // The C++ build registers macros without DuckDB's fixed-size
            // DefaultMacro arrays (see csrc/), so this is a readability cap.
            assert!(m.params.len() + m.named.len() <= 14, "{}", m.name);
        }
    }
}
