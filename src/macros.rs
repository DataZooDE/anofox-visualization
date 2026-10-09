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
//!   `to_json(list(tbl ORDER BY tbl))` keeps the column names, and the
//!   renderer ([`crate::contract`]) reads the contract columns by name. One
//!   SVG per group (`GROUP BY model_id`), no table-name strings, no
//!   introspection SQL — works in both builds.

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

impl MacroSpec {
    /// `(p, q := default, …) AS body` — the tail of a `CREATE MACRO name…`.
    pub fn create_sql(&self) -> String {
        let mut ps: Vec<String> = self.params.iter().map(|p| p.to_string()).collect();
        ps.extend(self.named.iter().map(|(n, d)| format!("{n} := {d}")));
        format!("({}) AS {}", ps.join(", "), self.body)
    }
}

// Column macros. The role list is built with json_array(), so `kind` is
// JSON-quoted rather than spliced into a JSON string (a quote in `kind` cannot
// break the spec); a NULL kind becomes a JSON null, which anofox_render
// rejects. list(... ORDER BY ...) makes row order deterministic under parallel
// aggregation.
const XY: &str = "anofox_render(json_object('rows', to_json(list({c0: x, c1: y} ORDER BY x, y)), \
'roles', json_array(json_array(0, 'XAXIS'), json_array(1, kind)), 'width', width, 'height', height))";
const XYC: &str = "anofox_render(json_object('rows', to_json(list({c0: x, c1: y, c2: series} ORDER BY x, series, y)), \
'roles', json_array(json_array(0, 'XAXIS'), json_array(1, kind), json_array(2, 'CATEGORY')), \
'width', width, 'height', height))";

const SIZE: &[(&str, &str)] = &[("width", "640"), ("height", "400")];
const CONTRACT_TAG: (&str, &str) = ("anofox.contract", "1");

macro_rules! contract_body {
    ($plot:literal) => {
        concat!(
            "anofox_render(json_object('plot', '",
            $plot,
            "', 'rows', to_json(list(tbl ORDER BY tbl)), 'width', width, 'height', height))"
        )
    };
    ($plot:literal, $opts:literal) => {
        concat!(
            "anofox_render(json_object('plot', '",
            $plot,
            "', 'rows', to_json(list(tbl ORDER BY tbl)), 'options', json_object(",
            $opts,
            "), 'width', width, 'height', height))"
        )
    };
}

/// Every bundled macro.
pub static MACROS: &[MacroSpec] = &[
    MacroSpec {
        name: "anofox_xy",
        params: &["x", "y"],
        named: &[("kind", "'BARCHART'"), ("width", "640"), ("height", "400")],
        body: XY,
        description: "Aggregate two columns into a single-series chart and render it as SVG. 'kind' selects the mark \
(BARCHART, LINECHART, SCATTER, AREACHART); x becomes the axis and y the value.",
        example: "SELECT anofox_xy(x, y, kind := 'LINECHART') FROM (VALUES ('Jan', 10), ('Feb', 20)) t(x, y)",
        tags: &[],
    },
    MacroSpec {
        name: "anofox_xyc",
        params: &["x", "y", "series"],
        named: &[("kind", "'BARCHART_STACKED'"), ("width", "640"), ("height", "400")],
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
        named: SIZE,
        body: contract_body!("terms"),
        description: "Coefficient plot of a `terms` (tidy) table, aggregated over its rows: pass the row, \
`SELECT anofox_plot_terms(c) FROM coefs c`. Columns: term, estimate, conf_low, conf_high (optional), model_id \
(several models are dodged and coloured). With index_value (+ index_name) it draws each term's estimate and band \
over the index instead — regularisation path, quantile process, rolling coefficients — one panel per term.",
        example: "SELECT anofox_plot_terms(c) FROM (VALUES ('x1', 0.5, 0.2, 0.8), ('x2', -0.3, -0.7, 0.1)) \
c(term, estimate, conf_low, conf_high)",
        tags: &[("anofox.consumes", "terms"), CONTRACT_TAG],
    },
    MacroSpec {
        name: "anofox_plot_prediction",
        params: &["tbl"],
        named: &[
            ("x", "NULL"),
            ("y", "NULL"),
            ("facet", "NULL"),
            ("width", "640"),
            ("height", "400"),
        ],
        body: contract_body!("prediction", "'x', x, 'y', y, 'facet', facet"),
        description: "Fit / forecast plot of a `prediction` table: observed y as points, the yhat line and the \
yhat_lower..yhat_upper band over x (or ds), coloured by split (train/test/future; derived from is_training or a \
missing y), or by model_id/model_name when there are several models. x := / y := name other columns (e.g. \
x := 'x1' for a *_fit_predict_by output); several `id` series become panels (facet := overrides).",
        example: "SELECT anofox_plot_prediction(p) FROM (VALUES (1, 1.0, 1.1, 0.9, 1.3), (2, 2.0, 1.9, 1.6, 2.2), \
(3, NULL, 3.1, 2.6, 3.6)) p(x, y, yhat, yhat_lower, yhat_upper)",
        tags: &[("anofox.consumes", "prediction"), CONTRACT_TAG],
    },
    MacroSpec {
        name: "anofox_plot_curve",
        params: &["tbl"],
        named: SIZE,
        body: contract_body!("curve"),
        description: "Plot a `curve` table; the geom follows curve_type: roc/calibration (line + diagonal), pr/pdp/lift \
(line), km (step), acf/pacf (lollipops + bounds from y_low/y_high), lambda_cv (point ± y_low..y_high on a log x), \
qq (points + identity). One curve_type per call (GROUP BY curve_type); several model_id/series are coloured.",
        example: "SELECT anofox_plot_curve(r) FROM (VALUES ('roc', 0.0, 0.0), ('roc', 0.2, 0.7), ('roc', 1.0, 1.0)) \
r(curve_type, x, y)",
        tags: &[("anofox.consumes", "curve"), CONTRACT_TAG],
    },
    MacroSpec {
        name: "anofox_plot_summary",
        params: &["tbl"],
        named: SIZE,
        body: contract_body!("summary"),
        description: "Model comparison from a `summary` (glance) table: one panel per metric, model_id on the \
vertical axis, value as a dot (± conf_low..conf_high).",
        example: "SELECT anofox_plot_summary(s) FROM (VALUES ('ols', 'r2', 0.81), ('ridge', 'r2', 0.79)) \
s(model_id, metric, value)",
        tags: &[("anofox.consumes", "summary"), CONTRACT_TAG],
    },
    MacroSpec {
        name: "anofox_plot_obs",
        params: &["tbl"],
        named: &[
            ("kind", "'resid_fitted'"),
            ("label_top", "3"),
            ("width", "640"),
            ("height", "400"),
        ],
        body: contract_body!("obs", "'kind', kind, 'label_top', label_top"),
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
        named: &[("label_top", "3"), ("width", "820"), ("height", "640")],
        body: contract_body!("diagnostics", "'label_top', label_top"),
        description: "The plot.lm 2x2 from an `obs` (augment) table as one SVG: residuals vs fitted, normal QQ, \
scale-location and residuals vs leverage (or Cook's distance).",
        example: "SELECT anofox_plot_diagnostics(o) FROM (VALUES (1, 1.0, 0.1), (2, 2.0, -0.3), (3, 3.0, 0.2), \
(4, 4.0, 0.0)) o(row_id, fitted, residual)",
        tags: &[("anofox.consumes", "obs"), CONTRACT_TAG],
    },
    MacroSpec {
        name: "anofox_plot",
        params: &["tbl"],
        named: SIZE,
        body: contract_body!("auto"),
        description: "Plot any anofox contract table, choosing the chart from its columns: term+estimate -> terms, \
curve_type -> curve, yhat -> prediction, fitted+residual -> diagnostics 2x2, metric+value -> summary. Use the \
specific anofox_plot_* macro for options.",
        example: "SELECT anofox_plot(c) FROM (VALUES ('x1', 0.5, 0.2, 0.8)) c(term, estimate, conf_low, conf_high)",
        tags: &[("anofox.consumes", "terms,prediction,curve,summary,obs"), CONTRACT_TAG],
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
            format!("(x, y, kind := 'BARCHART', width := 640, height := 400) AS {XY}")
        );
        let t = MACROS
            .iter()
            .find(|m| m.name == "anofox_plot_terms")
            .unwrap();
        assert!(t
            .create_sql()
            .starts_with("(tbl, width := 640, height := 400) AS anofox_render("));
    }

    #[test]
    fn names_unique_and_documented() {
        let mut seen = std::collections::HashSet::new();
        for m in MACROS {
            assert!(seen.insert(m.name), "{}", m.name);
            assert!(!m.description.is_empty() && m.example.contains(m.name));
            assert!(
                m.params.len() + m.named.len() <= 7,
                "C++ DefaultMacro limit"
            );
        }
    }
}
