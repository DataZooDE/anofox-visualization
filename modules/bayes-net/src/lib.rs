//! Bayesian networks as a runtime plugin (#1275): `anofox_plot_bayes_net`
//! maps a network's tables (nodes, arcs, beliefs, evidence, conditional
//! probability tables) onto the `::DIAGRAM` roles ([`bayes_net`]) and
//! answers with those columns; the host lays the network out and draws it
//! with its own renderer and the viewer's theme, like factor graphs (#1218).
use anofox_viz_plugin::{
    export_plugin, ContractInput, ContractMacro, ContractOption, ContractOutput, Manifest, Plugin,
    RenderInput,
};
use serde_json::{Map, Value as J};

mod bayes_net;

type Row = Map<String, J>;

struct BayesNet;

impl Plugin for BayesNet {
    const MANIFEST: Manifest = Manifest {
        name: "bayes-net",
        roles: &[],
    };

    fn render(_: &RenderInput) -> Result<String, String> {
        Err("bayes-net has no roles of its own; use anofox_plot_bayes_net".into())
    }

    const MACROS: &'static [ContractMacro] = &[ContractMacro {
        plot: "bayes_net",
        named: &[
            ("arcs", "NULL"),
            ("beliefs", "NULL"),
            ("evidence", "NULL"),
            ("tables", "NULL"),
            ("threshold", "0.5"),
            ("direction", "'TB'"),
            ("width", "900"),
            ("height", "480"),
        ],
        options: &[
            ContractOption { key: "arcs", param: "arcs", fallback_var: None },
            ContractOption { key: "beliefs", param: "beliefs", fallback_var: None },
            ContractOption { key: "evidence", param: "evidence", fallback_var: None },
            ContractOption { key: "tables", param: "tables", fallback_var: None },
            ContractOption { key: "threshold", param: "threshold", fallback_var: None },
            ContractOption { key: "direction", param: "direction", fallback_var: None },
        ],
        description: "A Bayesian network (directed, layered top to bottom) from a `bayes_net` table, one row per node \
(id, name, label, kind: chance | observed | latent | decision | utility | deterministic, states: a LIST of state \
names, layer, group), with arcs := a LIST of (from, to, strength, direction, status: learned | required | \
forbidden), and optionally beliefs := a LIST of (node, state, probability, prior, lo, hi), evidence := a LIST of \
(node, state, likelihood, mode: observe | do) and tables := a LIST of (node, parent_states, state, probability), \
the conditional probability tables. Nodes are drawn Netica / GeNIe style: a box per chance node with a bar per \
state for its belief and the prior as a faint bar behind it; a node with evidence (or of kind observed) is shaded, \
its observed state in bold; a decision is a square-cornered box, a utility a diamond, a deterministic node a \
double outline. Arcs point at the child, wider the stronger they are, dashed below threshold := 0.5 (a required \
arc stays solid), their strength and direction on hover; forbidden arcs are not drawn. Hovering a node shows its \
belief against the prior and its conditional probability table as a heat table (parent_states in the order of \
the node's arcs). layer names tiers drawn in order, group colours a node's accent. Lint: cycles, arcs to unknown \
ids, beliefs that do not sum to 1 per node, states not in the node's states.",
        example: "SELECT anofox_plot_bayes_net(n, arcs := [{'from': 'rain', 'to': 'grass'}], beliefs := \
[{'node': 'rain', 'state': 'yes', 'probability': 0.2}, {'node': 'rain', 'state': 'no', 'probability': 0.8}]) FROM \
(VALUES ('rain', 'Rain', 'chance', ['yes', 'no']), ('grass', 'Wet grass', 'chance', ['yes', 'no'])) \
n(id, label, kind, states)",
    }];

    fn render_contract(input: &ContractInput) -> Result<ContractOutput, String> {
        Ok(ContractOutput::Columns {
            columns: bayes_net::columns(&input.rows, &input.options)?,
            root_attrs: Vec::new(),
            warnings: bayes_net::lint(&input.rows, &input.options),
        })
    }
}

export_plugin!(BayesNet);
