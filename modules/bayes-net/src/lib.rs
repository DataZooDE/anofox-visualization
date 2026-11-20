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
mod causal;

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
            ("paths", "NULL"),
            ("policy", "NULL"),
            ("adjust", "NULL"),
            ("reference", "NULL"),
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
            ContractOption { key: "paths", param: "paths", fallback_var: None },
            ContractOption { key: "policy", param: "policy", fallback_var: None },
            ContractOption { key: "adjust", param: "adjust", fallback_var: None },
            ContractOption { key: "reference", param: "reference", fallback_var: None },
            ContractOption { key: "threshold", param: "threshold", fallback_var: None },
            ContractOption { key: "direction", param: "direction", fallback_var: None },
        ],
        description: "A Bayesian network (directed, layered top to bottom) from a `bayes_net` table, one row per node \
(id, name, label, kind: chance | observed | latent | decision | utility | deterministic, states: a LIST of state \
names, layer, group), with arcs := a LIST of (from, to, strength, direction, status: learned | required | \
forbidden), and optionally beliefs := a LIST of (node, state, probability, prior, lo, hi, previous), evidence := a \
LIST of (node, state, likelihood, mode: observe | do) or its JSON text, tables := a LIST of (node, parent_states, \
state, probability), the conditional probability tables, and paths := a LIST of (path_id, seq, from, to), the \
active paths between nodes given the evidence. Nodes are drawn Netica / GeNIe style: a box per chance node with a bar per \
state for its belief and the prior as a faint bar behind it; a node with evidence (or of kind observed) is shaded, \
its observed state in bold; a decision is a square-cornered box, a utility a diamond, a deterministic node a \
double outline. Arcs point at the child, wider the stronger they are, dashed below threshold := 0.5 (a required \
arc stays solid), their strength and direction on hover; forbidden arcs are not drawn. Hovering a node shows its \
belief against the prior and its conditional probability table as a heat table (parent_states in the order of \
the node's arcs). layer names tiers, group colours a node's accent. With 'name evidence'::SELECT_AS before it a \
click on a state observes it (again: retracts), shift-click gives soft evidence, a chip toggles observe / do; \
getvariable('name') is the evidence as JSON text, getvariable('name_prev') the one before. previous draws each \
state's change and tints a node by how far it moved; do cuts a node's incoming arcs (a hand on it); hovering a \
node lights its Markov blanket, or the paths to an anchored node. Influence diagrams: policy := a LIST of (decision, \
parent_states, option, expected_utility, chosen) draws a decision's policy as a table in its box (a row per \
configuration of its informational parents, a column per option with its expected utility, the chosen one marked, \
the configuration the evidence picks on a band; a click on an option decides it); arcs into a decision are \
information arcs, drawn dotted; a beliefs row of a utility node with expected_utility and no state is written in \
its diamond. Lint: cycles, unknown ids, beliefs not summing to 1, unknown states, bad modes or likelihoods, path \
steps along no arc or out of order, policy options, parent_states and chosen options that do not fit.",
        example: "SELECT anofox_plot_bayes_net(n, arcs := [{'from': 'rain', 'to': 'grass'}], beliefs := \
[{'node': 'rain', 'state': 'yes', 'probability': 0.2}, {'node': 'rain', 'state': 'no', 'probability': 0.8}]) FROM \
(VALUES ('rain', 'Rain', 'chance', ['yes', 'no']), ('grass', 'Wet grass', 'chance', ['yes', 'no'])) \
n(id, label, kind, states)",
    }];

    fn render_contract(input: &ContractInput) -> Result<ContractOutput, String> {
        // #1278: the counterfactual twin network, when the beliefs come in
        // two worlds
        let twin = causal::twin(&input.rows, &input.options)?;
        let (rows, options) = match &twin {
            Some((r, o)) => (r.as_slice(), o),
            None => (input.rows.as_slice(), &input.options),
        };
        let mut columns = bayes_net::columns(rows, options)?;
        // #1278: adjustment sets, the structure diff
        causal::decorate(&mut columns, rows, options)?;
        let mut root_attrs = bayes_net::root_attrs(options)?;
        root_attrs.extend(causal::root_attrs(options)?);
        // the network as drawn (a twin's two worlds as two nodes each)
        let mut warnings = bayes_net::lint(rows, options);
        warnings.extend(causal::lint(&input.rows, &input.options));
        Ok(ContractOutput::Columns {
            columns,
            root_attrs,
            warnings,
        })
    }
}

export_plugin!(BayesNet);
