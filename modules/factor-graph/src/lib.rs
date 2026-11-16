//! Factor graphs as a runtime plugin (#1218): `anofox_plot_factor_graph`
//! maps an inference library's nodes, edges and marginals onto the
//! `::DIAGRAM` roles ([`factor_graph`]) and answers with those columns; the
//! host draws them with its own renderer and the viewer's theme.
//!
//! The few core types the mapping uses (a column, its role, a cell) are
//! mirrored here, as the role tokens the host parses back.
use anofox_viz_plugin::{
    export_plugin, ContractInput, ContractMacro, ContractOption, ContractOutput, Manifest,
    OutColumn, Plugin, RenderInput,
};
use serde_json::{Map, Value as J};

mod factor_graph;

type Row = Map<String, J>;

const NEEDS: &str = "needs column(s) ";

/// A cell, as the host reads it back from JSON.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Value {
    Float(f64),
    Str(String),
    Na,
}

impl Value {
    fn json(&self) -> J {
        match self {
            Value::Float(f) => serde_json::Number::from_f64(*f).map_or(J::Null, J::Number),
            Value::Str(s) => J::String(s.clone()),
            Value::Na => J::Null,
        }
    }
}

/// The core's `value_text`: text as is, a number rounded to 3 decimals.
pub(crate) fn value_text(v: &Value) -> String {
    match v {
        Value::Str(s) => s.clone(),
        Value::Na => String::new(),
        Value::Float(f) if f.is_nan() => "NaN".into(),
        Value::Float(f) if *f == f64::INFINITY => "\u{221E}".into(),
        Value::Float(f) if *f == f64::NEG_INFINITY => "\u{2212}\u{221E}".into(),
        Value::Float(f) => format!("{}", (f * 1000.0).round() / 1000.0),
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum MetricFmt {
    Plain,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Kind {
    Sparkline,
}

/// The roles a factor graph answers with.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Role {
    Badge,
    Belief,
    Diagram,
    DiagramCheck,
    Edges,
    Label,
    Lane,
    Metric(MetricFmt),
    Shape,
    Tooltip,
    Value(Kind),
}

impl Role {
    fn token(self) -> &'static str {
        match self {
            Role::Badge => "BADGE",
            Role::Belief => "BELIEF",
            Role::Diagram => "DIAGRAM",
            Role::DiagramCheck => "DIAGRAM_CHECK",
            Role::Edges => "EDGES",
            Role::Label => "LABEL",
            Role::Lane => "LANE",
            Role::Metric(MetricFmt::Plain) => "METRIC",
            Role::Shape => "SHAPE",
            Role::Tooltip => "TOOLTIP",
            Role::Value(Kind::Sparkline) => "SPARKLINE",
        }
    }
}

pub(crate) struct Column {
    pub name: String,
    pub role: Role,
    pub values: Vec<Value>,
}

impl Column {
    pub fn new(name: impl Into<String>, role: Role, values: Vec<Value>) -> Self {
        Column {
            name: name.into(),
            role,
            values,
        }
    }
}

/// The core's number format for a plain `::METRIC` (en-US, ≤ 2 decimals,
/// grouped); the host localizes nothing a plugin writes.
pub(crate) mod format {
    use super::MetricFmt;

    /// The default (en-US) locale only.
    #[derive(Default)]
    pub struct Locale {
        _en_us: (),
    }

    pub fn format_number_l(n: f64, _fmt: MetricFmt, _l: &Locale) -> String {
        if !n.is_finite() {
            return "–".into();
        }
        let r = (n.abs() * 100.0).round() / 100.0;
        let neg = n < 0.0 && r != 0.0;
        let mut s = format!("{r:.2}");
        while s.ends_with('0') {
            s.pop();
        }
        if s.ends_with('.') {
            s.pop();
        }
        let (int, frac) = match s.find('.') {
            Some(i) => (s[..i].to_string(), s[i..].to_string()),
            None => (s.clone(), String::new()),
        };
        let mut g = String::with_capacity(int.len() + int.len() / 3);
        for (i, ch) in int.chars().enumerate() {
            if i > 0 && (int.len() - i) % 3 == 0 {
                g.push(',');
            }
            g.push(ch);
        }
        format!("{}{g}{frac}", if neg { "-" } else { "" })
    }
}

struct FactorGraph;

impl Plugin for FactorGraph {
    const MANIFEST: Manifest = Manifest {
        name: "factor-graph",
        roles: &[],
    };

    fn render(_: &RenderInput) -> Result<String, String> {
        Err("factor-graph has no roles of its own; use anofox_plot_factor_graph".into())
    }

    const MACROS: &'static [ContractMacro] = &[ContractMacro {
        plot: "factor_graph",
        named: &[
            ("edges", "NULL"),
            ("marginals", "NULL"),
            ("schedule", "NULL"),
            ("messages", "NULL"),
            ("predictive", "NULL"),
            ("influence", "NULL"),
            ("slices", "NULL"),
            ("width", "900"),
            ("height", "420"),
        ],
        options: &[
            ContractOption { key: "edges", param: "edges", fallback_var: None },
            ContractOption { key: "marginals", param: "marginals", fallback_var: None },
            ContractOption { key: "schedule", param: "schedule", fallback_var: None },
            ContractOption { key: "messages", param: "messages", fallback_var: None },
            ContractOption { key: "predictive", param: "predictive", fallback_var: None },
            ContractOption { key: "influence", param: "influence", fallback_var: None },
            // the viewer's ⋯ click sets it (#1152)
            ContractOption {
                key: "slices",
                param: "slices",
                fallback_var: Some("anofox_fg_slices"),
            },
        ],
        description: "A factor graph (Forney style) from a `factor_graph` table, one row per node (id, kind: factor | \
variable | data | constant | equality | delta, name, label, slice, layer, plate), with edges := a LIST of edge STRUCTs \
(variable, factor, interface, seq) and optionally marginals := a LIST of (variable, iteration, mean, sd, family). \
Factors are glyph squares, variables circles, observations filled squares at the end of their edge, equality \
nodes small squares, delta nodes filled squares, constants plain text; a variable on exactly two factors with no \
marginals is the edge between them. Layers (control, param, state, obs, pref, …) are horizontal bands: a chain stays one \
straight row, an observation drops below its state. Edges are undirected, labelled with the interface at the \
factor. A variable whose only edge is its equality node is drawn as that node; TeX-ish labels (\\theta, y_{0}) \
read as θ, y₀; the latest marginal is the variable's value (mean, ± sd). schedule := a LIST of (iteration, step, \
variable, factor, interface, direction: to_factor | to_variable) steps through the messages: Step / Play light each \
message's edge, moving its way, with the receiving variable's marginal after that iteration. From the inference \
library, messages := (iteration, step, …, family, mean, sd) says what each message carries; predictive := (variable, \
observed, mean, sd) what the rest of the model expected at each observation (more than 2 sd off is flagged); \
influence := (target, observation, mean, loo_mean) ranks the observations by how far a variable moves without \
each.",
        example: "SELECT anofox_plot_factor_graph(n, edges := [{'variable': 'v0', 'factor': 'f0', 'interface': 'out'}, \
{'variable': 'v1', 'factor': 'f0', 'interface': 'p'}]) FROM (VALUES ('v0', 'data', 'y', 'obs'), ('v1', 'variable', \
'\\theta', 'param'), ('f0', 'factor', 'Bernoulli', 'obs')) n(id, kind, label, layer)",
    }];

    fn render_contract(input: &ContractInput) -> Result<ContractOutput, String> {
        answer(&input.rows, &input.options)
    }
}

/// The macro's answer: the `::DIAGRAM` columns, the root's `data-slices`
/// (read by the browser's ⋯ click) and the model's problems as warnings.
fn answer(rows: &[Row], options: &Row) -> Result<ContractOutput, String> {
    let cols = factor_graph::columns(rows, options)?;
    let columns = cols
        .into_iter()
        .map(|c| OutColumn {
            name: c.name,
            role: c.role.token().into(),
            values: c.values.iter().map(Value::json).collect(),
        })
        .collect();
    let root_attrs = factor_graph::slices_state(rows, options)
        .map(|st| vec![("data-slices".to_string(), st.to_string())])
        .unwrap_or_default();
    Ok(ContractOutput::Columns {
        columns,
        root_attrs,
        warnings: factor_graph::lint(rows, options),
    })
}

export_plugin!(FactorGraph);
