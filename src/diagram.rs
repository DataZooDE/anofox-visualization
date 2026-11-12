//! `::DIAGRAM` — a data-driven node/edge diagram (process, pipeline, lineage;
//! design: `docs/plans/plan-diagram.md`).
//!
//! **One row per node.** The `::DIAGRAM` column is the node id; `::LABEL` the
//! box text; a KPI format (`::METRIC`/`::MONEY`/`::PERCENT`/`::COMPACT`) a
//! value line; `::BADGE` a status pill; `::THRESHOLDS` colours a node by its
//! value; `::CATEGORY` gives nodes a palette accent. Edges come from
//! `::UPSTREAM` (an id or a list of ids feeding the row's node) and/or
//! `::EDGES` (a list of structs `{from, to [, value, label, style]}`, usually a
//! scalar subquery — `source`/`target` are accepted for `from`/`to`).
//! `::DIRECTION` picks `'LR'` (default) or `'TB'`.
//!
//! The layout is a small layered (Sugiyama) layout of our own, deterministic
//! by construction (no hash iteration; ties are broken by row order):
//! cycle breaking (DFS in row order, back edges reversed), longest-path
//! ranking, dummy nodes on long edges, barycenter + transposition ordering,
//! and a least-squares placement (isotonic regression) along each layer.
//! The SVG is written here (not by ggplot-rs): node boxes are `<rect>` marks
//! and edges `<polyline>` marks, both with a `<title>` — the mark contract
//! the browser's hover, selection and accessibility layers already read.

use crate::format::{escape_xml as esc, format_number_l, Locale};
use crate::theme::hex;
use crate::thresholds::{Rules, Status};
use crate::{key_text, svg_open, with_root_attr, Column, MetricFmt, Place, RenderOptions, Role};
use ggplot_rs::prelude::Value;
use serde_json::Value as J;

/// At most this many nodes are drawn (the first ones in row order).
pub const MAX_NODES: usize = 150;
/// At most this many edges are drawn.
pub const MAX_EDGES: usize = 300;
/// The `::DIRECTION` values.
pub const DIRECTIONS: &[&str] = &["LR", "TB"];
/// The edge `style` values of `::EDGES`.
pub const EDGE_STYLES: &[&str] = &[
    "solid",
    "dashed",
    "flow",
    "flow-dots",
    "flow-volume",
    "flow-dots-volume",
];

/// Layout direction: layers run left to right or top to bottom.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    LR,
    TB,
}

impl Dir {
    fn as_str(self) -> &'static str {
        match self {
            Dir::LR => "LR",
            Dir::TB => "TB",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EdgeStyle {
    Solid,
    Dashed,
    /// Moving dashes in the browser (`class="dp-flow"`), still dashes in
    /// static output; `true` = faster the more the edge carries
    /// (`'flow-volume'`).
    Flow(bool),
    /// Moving dots (`'flow-dots'`): dots travelling along the edge, still
    /// dots in static output; `true` = speed by volume (`'flow-dots-volume'`).
    Dots(bool),
}

impl EdgeStyle {
    /// A style word (`solid`, `dashed`, `flow`, `flow-dots`, and the
    /// speed-by-volume `flow-volume`, `flow-dots-volume`); `None` for an
    /// unknown one.
    fn parse(s: &str) -> Option<EdgeStyle> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "" | "solid" => EdgeStyle::Solid,
            "dashed" => EdgeStyle::Dashed,
            "flow" => EdgeStyle::Flow(false),
            "flow-volume" => EdgeStyle::Flow(true),
            "flow-dots" | "dots" => EdgeStyle::Dots(false),
            "flow-dots-volume" | "dots-volume" => EdgeStyle::Dots(true),
            _ => return None,
        })
    }
}

#[derive(Clone, Debug)]
pub struct Node {
    pub id: String,
    pub label: String,
    pub value: Option<f64>,
    /// The value as the box shows it (the column's KPI format, the locale).
    pub fvalue: Option<String>,
    pub badge: Option<String>,
    pub status: Option<Status>,
    /// Palette slot of the node's `::CATEGORY` (first-appearance order).
    pub category: Option<usize>,
    /// The node's `::STORY` caption: a story step that lights it up.
    pub story: Option<String>,
    /// The node's swimlane (`::LANE`), an index into [`Diagram::lanes`].
    pub lane: Option<usize>,
    /// A pinned layer (`::XAXIS`) and, with `::YAXIS`, a pinned position.
    pub pin: (Option<f64>, Option<f64>),
    /// A mini trend in the box (`list(v)::SPARKLINE`).
    pub spark: Vec<f64>,
}

#[derive(Clone, Debug)]
pub struct Edge {
    pub from: usize,
    pub to: usize,
    pub value: Option<f64>,
    pub fvalue: Option<String>,
    pub label: Option<String>,
    pub style: EdgeStyle,
    /// The edge's health (`::EDGES` `status`): warn / bad colour the edge.
    pub status: Option<Status>,
}

/// Something the author should know (`dashboard --check`: `diagram/*`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem {
    pub code: &'static str,
    pub message: String,
}

#[derive(Clone, Debug)]
pub struct Diagram {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub dir: Dir,
    /// Swimlane names (`::LANE`), in first-appearance order.
    pub lanes: Vec<String>,
    /// Every node has `::XAXIS` and `::YAXIS`: drawn where it is placed.
    pub explicit: bool,
    /// The `::ANIMATE` mode, when it is one the browser plays.
    pub animate: Option<String>,
    /// `'tween'::TRANSITION`: a re-run changes widths, colours and values in place.
    pub tween: bool,
    /// Nodes / edges in the data before the [`MAX_NODES`] / [`MAX_EDGES`] cut.
    pub total_nodes: usize,
    pub total_edges: usize,
    pub problems: Vec<Problem>,
    /// A `::COLORS` level map's colour per `::CATEGORY` slot (#699:
    /// `'supplier:#2a9d8f; stock:good'`) — a type keeps its colour however
    /// the rows (or an edit) order the types; `None` = the theme palette.
    pub category_colors: Vec<Option<crate::thresholds::LevelColor>>,
}

/// A cell as an id: text, or a number written without a trailing `.0`.
/// Text as an SVG may carry it: trimmed, without the control characters XML
/// forbids (a stray NUL in a name would break the whole document).
fn clean(s: &str) -> String {
    s.trim()
        .chars()
        .filter(|c| !c.is_control() || matches!(c, '\t' | '\n' | '\r'))
        .collect()
}

fn id_of(v: &Value) -> Option<String> {
    key_text(v).map(|s| clean(&s)).filter(|s| !s.is_empty())
}

fn json_id(v: &J) -> Option<String> {
    match v {
        J::String(s) => Some(clean(s)).filter(|s| !s.is_empty()),
        J::Number(n) => n.as_f64().map(|f| f.to_string()),
        J::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

fn num(v: &Value) -> Option<f64> {
    match v {
        Value::Str(s) => s.trim().parse::<f64>().ok(),
        v => v.as_f64(),
    }
    .filter(|f| f.is_finite())
}

fn text(v: &Value) -> Option<String> {
    match v {
        Value::Na => None,
        Value::Str(s) => Some(clean(s)),
        v => key_text(v).map(|s| clean(&s)),
    }
    .filter(|s| !s.is_empty())
}

/// The ids of an `::UPSTREAM` cell: a JSON list (how a LIST column arrives),
/// DuckDB's list text `[a, b]`, or one id.
fn upstream_ids(v: &Value) -> Vec<String> {
    let Value::Str(s) = v else {
        return id_of(v).into_iter().collect();
    };
    let t = s.trim();
    if t.starts_with('[') {
        if let Ok(J::Array(a)) = serde_json::from_str::<J>(t) {
            return a.iter().filter_map(json_id).collect();
        }
        return t
            .trim_start_matches('[')
            .trim_end_matches(']')
            .split(',')
            .map(|p| clean(p.trim().trim_matches(|c| c == '\'' || c == '"')))
            .filter(|p| !p.is_empty())
            .collect();
    }
    Some(clean(t))
        .filter(|t| !t.is_empty())
        .into_iter()
        .collect()
}

/// A struct field by any of `names` (case-insensitive).
fn field<'a>(o: &'a serde_json::Map<String, J>, names: &[&str]) -> Option<&'a J> {
    o.iter()
        .find(|(k, _)| names.iter().any(|n| k.eq_ignore_ascii_case(n)))
        .map(|(_, v)| v)
        .filter(|v| !v.is_null())
}

impl Diagram {
    /// The diagram of a panel's columns, `None` without a `::DIAGRAM` column.
    pub fn from_columns<'a>(cols: &'a [Column], locale: &Locale) -> Option<Diagram> {
        let idc = cols.iter().find(|c| c.role == Role::Diagram)?;
        let col = |r: Role| cols.iter().find(|c| c.role == r);
        let label = col(Role::Label);
        let metric = cols.iter().find_map(|c| match c.role {
            Role::Metric(f) => Some((c, f)),
            _ => None,
        });
        let badge = col(Role::Badge);
        // #699: a check's status on the node wins over its thresholds / badge
        let check = col(Role::DiagramCheck);
        let category = col(Role::Category);
        let upstream = col(Role::Upstream);
        let edges_col = col(Role::Edges);
        let story = col(Role::Story);
        let line_style = col(Role::LineStyle);
        let lane_col = col(Role::Lane);
        let (xcol, ycol) = (col(Role::X), col(Role::Y));
        let spark_col = col(Role::Value(crate::Kind::Sparkline));
        let first_text = |r: Role| col(r).and_then(|c| c.values.iter().find_map(text));
        let mut problems = Vec::new();
        let mut problem = |code, message: String| problems.push(Problem { code, message });

        let dir = match first_text(Role::Direction) {
            None => Dir::LR,
            Some(d) => match d.to_ascii_uppercase().as_str() {
                "LR" => Dir::LR,
                "TB" => Dir::TB,
                _ => {
                    problem(
                        "diagram/bad-direction",
                        format!("::DIRECTION `{d}` is not 'LR' or 'TB' — drawn left to right"),
                    );
                    Dir::LR
                }
            },
        };
        let rules = match first_text(Role::Thresholds).map(|s| Rules::parse(&s)) {
            Some(Ok(r)) => Some(r),
            _ => None, // a malformed rule set is reported by the thresholds lint
        };
        let fmt = metric.map(|(_, f)| f).unwrap_or(MetricFmt::Plain);

        // Nodes, first row wins; rows are kept for their ::UPSTREAM.
        let mut nodes: Vec<Node> = Vec::new();
        let mut rows: Vec<usize> = Vec::new(); // node → its row
        let mut cats: Vec<String> = Vec::new();
        let mut lanes: Vec<String> = Vec::new();
        let mut bad_pins = 0usize;
        let (mut nulls, mut dups) = (0usize, Vec::new());
        for (i, v) in idc.values.iter().enumerate() {
            let Some(id) = id_of(v) else {
                nulls += 1;
                continue;
            };
            if nodes.iter().any(|n| n.id == id) {
                if !dups.contains(&id) {
                    dups.push(id);
                }
                continue;
            }
            let cell = |c: Option<&'a Column>| c.and_then(|c| c.values.get(i));
            let value = metric.and_then(|(c, _)| c.values.get(i)).and_then(num);
            let badge = cell(badge).and_then(text);
            let status = cell(check)
                .and_then(text)
                .and_then(|w| badge_status(&w))
                .or_else(|| value.and_then(|v| rules.as_ref().and_then(|r| r.status(v))))
                .or_else(|| badge.as_deref().and_then(badge_status));
            let category = cell(category).and_then(text).map(|k| {
                cats.iter().position(|c| *c == k).unwrap_or_else(|| {
                    cats.push(k);
                    cats.len() - 1
                })
            });
            nodes.push(Node {
                label: cell(label).and_then(text).unwrap_or_else(|| id.clone()),
                id,
                fvalue: value.map(|v| format_number_l(v, fmt, locale)),
                value,
                badge,
                status,
                category,
                story: cell(story).and_then(text),
                lane: cell(lane_col).and_then(text).map(|k| {
                    lanes.iter().position(|c| *c == k).unwrap_or_else(|| {
                        lanes.push(k);
                        lanes.len() - 1
                    })
                }),
                pin: {
                    let mut pin = |c: Option<&'a Column>| match cell(c) {
                        None | Some(Value::Na) => None,
                        Some(v) => num(v).or_else(|| {
                            bad_pins += 1;
                            None
                        }),
                    };
                    (pin(xcol), pin(ycol))
                },
                spark: cell(spark_col)
                    .map(crate::kpi::spark_values)
                    .unwrap_or_default(),
            });
            rows.push(i);
        }
        if nulls > 0 {
            problem(
                "diagram/null-id",
                format!("{nulls} row(s) with a NULL ::DIAGRAM id are not drawn"),
            );
        }
        if !dups.is_empty() {
            problem(
                "diagram/duplicate-node",
                format!(
                    "node id(s) {} appear more than once — the first row is drawn",
                    quoted(&dups)
                ),
            );
        }
        if bad_pins > 0 {
            problem(
                "diagram/bad-pin",
                format!("{bad_pins} ::XAXIS / ::YAXIS value(s) are not numbers — those nodes are placed by the layout"),
            );
        }
        // every node pinned in x and y: drawn where placed; some: x pins a layer
        let pinned_xy = nodes
            .iter()
            .filter(|n| n.pin.0.is_some() && n.pin.1.is_some())
            .count();
        let explicit = !nodes.is_empty() && pinned_xy == nodes.len();
        if pinned_xy > 0 && !explicit {
            problem(
                "diagram/partial-pins",
                format!(
                    "{pinned_xy} of {} nodes have ::XAXIS and ::YAXIS — exact positions need both on every node; ::XAXIS only pins a node's stage",
                    nodes.len()
                ),
            );
        }
        let total_nodes = nodes.len();
        if total_nodes > MAX_NODES {
            nodes.truncate(MAX_NODES);
            rows.truncate(MAX_NODES);
        }
        let index = |id: &str, nodes: &[Node]| nodes.iter().position(|n| n.id == id);

        // Edges: (from id, to id, value, label, style), merged by (from, to).
        let mut raw: Vec<RawEdge> = Vec::new();
        let mut bad_styles: Vec<String> = Vec::new();
        if let Some(e) = edges_col {
            let cell = e.values.iter().find(|v| !matches!(v, Value::Na));
            if let Some(cell) = cell {
                let parsed = match cell {
                    Value::Str(s) => serde_json::from_str::<J>(s).ok(),
                    _ => None,
                };
                match parsed {
                    Some(J::Array(items)) => {
                        let mut missing = 0usize;
                        for it in &items {
                            let Some(o) = it.as_object() else {
                                missing += 1;
                                continue;
                            };
                            let from = field(o, &["from", "source"]).and_then(json_id);
                            let to = field(o, &["to", "target"]).and_then(json_id);
                            let (Some(from), Some(to)) = (from, to) else {
                                missing += 1;
                                continue;
                            };
                            let value = field(o, &["value"]).and_then(|v| match v {
                                J::Number(n) => n.as_f64(),
                                J::String(s) => s.trim().parse().ok(),
                                _ => None,
                            });
                            let label =
                                field(o, &["label"]).and_then(json_id).filter(|l| !l.is_empty());
                            let style = field(o, &["style"])
                                .and_then(|v| v.as_str())
                                .and_then(|s| {
                                    let st = EdgeStyle::parse(s);
                                    if st.is_none() && !bad_styles.iter().any(|b| b == s) {
                                        bad_styles.push(s.to_string());
                                    }
                                    st
                                });
                            // a health word (`ok`, `delayed`, `failed`, …) colours the edge
                            let status = field(o, &["status"]).and_then(json_id).and_then(|w| badge_status(&w));
                            raw.push(RawEdge {
                                from,
                                to,
                                value: value.filter(|v| v.is_finite()),
                                label,
                                style,
                                status,
                            });
                        }
                        if missing > 0 {
                            problem(
                                "diagram/bad-edges",
                                format!(
                                    "{missing} ::EDGES item(s) are not a struct with `from` and `to` (or `source` and `target`) — skipped"
                                ),
                            );
                        }
                    }
                    _ => problem(
                        "diagram/bad-edges",
                        "::EDGES is not a list of structs — write (SELECT list({'from': a, 'to': b}) FROM edges)::EDGES"
                            .into(),
                    ),
                }
            }
        }
        // ::LINESTYLE per row: the default style of the edges into the row's
        // node (a constant column styles every edge)
        let row_style: Vec<Option<EdgeStyle>> = rows
            .iter()
            .map(|&r| {
                let s = line_style.and_then(|c| c.values.get(r)).and_then(text)?;
                let st = EdgeStyle::parse(&s);
                if st.is_none() && !bad_styles.contains(&s) {
                    bad_styles.push(s);
                }
                st
            })
            .collect();
        if !bad_styles.is_empty() {
            problem(
                "diagram/bad-style",
                format!(
                    "edge style(s) {} are not {} — drawn solid",
                    quoted(&bad_styles),
                    EDGE_STYLES.join(", ")
                ),
            );
        }
        if let Some(up) = upstream {
            for (n, &row) in rows.iter().enumerate() {
                for from in up.values.get(row).map(upstream_ids).unwrap_or_default() {
                    raw.push(RawEdge {
                        from,
                        to: nodes[n].id.clone(),
                        ..RawEdge::default()
                    });
                }
            }
        }

        let mut edges: Vec<Edge> = Vec::new();
        let mut styles: Vec<Option<EdgeStyle>> = Vec::new(); // as given in ::EDGES
        let mut unknown: Vec<String> = Vec::new();
        let mut total_edges = 0usize;
        for RawEdge {
            from,
            to,
            value,
            label,
            style,
            status,
        } in raw
        {
            let (Some(f), Some(t)) = (index(&from, &nodes), index(&to, &nodes)) else {
                // an id beyond the node cut is no author error
                for id in [&from, &to] {
                    if index(id, &nodes).is_none()
                        && !idc.values.iter().any(|v| id_of(v).as_deref() == Some(id))
                        && !unknown.contains(id)
                    {
                        unknown.push(id.clone());
                    }
                }
                continue;
            };
            match edges.iter().position(|e| e.from == f && e.to == t) {
                Some(k) => {
                    let e = &mut edges[k];
                    e.value = e.value.or(value);
                    e.label = e.label.take().or(label);
                    e.status = e.status.or(status);
                    styles[k] = styles[k].or(style);
                }
                None => {
                    total_edges += 1;
                    if edges.len() < MAX_EDGES {
                        edges.push(Edge {
                            from: f,
                            to: t,
                            value,
                            fvalue: None,
                            label,
                            style: EdgeStyle::Solid,
                            status,
                        });
                        styles.push(style);
                    }
                }
            }
        }
        for (e, st) in edges.iter_mut().zip(styles) {
            e.fvalue = e.value.map(|v| format_number_l(v, fmt, locale));
            e.style = st.or(row_style[e.to]).unwrap_or(EdgeStyle::Solid);
        }
        if !unknown.is_empty() {
            problem(
                "diagram/unknown-node",
                format!(
                    "edges name node id(s) {} that are not ::DIAGRAM rows — those edges are not drawn",
                    quoted(&unknown)
                ),
            );
        }
        if total_nodes > MAX_NODES || total_edges > MAX_EDGES {
            problem(
                "diagram/too-large",
                format!(
                    "{total_nodes} nodes and {total_edges} edges — a diagram draws at most {MAX_NODES} nodes and {MAX_EDGES} edges (the first ones); filter or aggregate the rows"
                ),
            );
        }
        let animate = first_text(Role::Animate)
            .map(|a| a.to_ascii_lowercase())
            .filter(|a| crate::ANIMATE_MODES.contains(&a.as_str()));
        let tween = first_text(Role::Transition).is_some_and(|t| t.eq_ignore_ascii_case("tween"));
        let level_map = first_text(Role::GaugeColors)
            .filter(|s| crate::thresholds::is_level_map(s))
            .and_then(|s| crate::thresholds::parse_level_colors(&s).ok())
            .unwrap_or_default();
        let category_colors = cats
            .iter()
            .map(|c| level_map.iter().find(|(l, _)| l == c).map(|(_, lc)| *lc))
            .collect();
        Some(Diagram {
            nodes,
            edges,
            dir,
            lanes,
            explicit,
            animate,
            tween,
            total_nodes,
            total_edges,
            problems,
            category_colors,
        })
    }

    /// The edges drawn against the flow (one per cycle the layout breaks).
    pub fn back_edges(&self) -> Vec<usize> {
        layout(self)
            .back
            .iter()
            .enumerate()
            .filter(|(_, &b)| b)
            .map(|(k, _)| k)
            .collect()
    }

    fn edge_name(&self, k: usize) -> String {
        let e = &self.edges[k];
        format!("{} → {}", self.nodes[e.from].label, self.nodes[e.to].label)
    }
}

/// A badge's status from its words — the lists the browser's table pills
/// use (`badgeHtml` in `web/app.js`): good words first, then warn, then bad.
pub(crate) fn badge_status(text: &str) -> Option<Status> {
    const GOOD: &[&str] = &[
        "ok",
        "good",
        "ontrack",
        "pass",
        "passed",
        "active",
        "done",
        "up",
        "healthy",
        "nominal",
        "green",
        "low",
        "grow",
        "growing",
        "growth",
        "rise",
        "rising",
        "improved",
        "improving",
        "correct",
        "right",
        "met",
        "achieved",
        "erreicht",
        "gut",
        "aktiv",
        "erledigt",
    ];
    const WARN: &[&str] = &[
        "warn",
        "warning",
        "risk",
        "atrisk",
        "pending",
        "review",
        "amber",
        "medium",
        "hold",
        "watch",
        "flat",
        "stable",
        "knapp",
        "prüfen",
        "offen",
        "delayed",
        "late",
        "slow",
        "verzögert",
    ];
    const BAD: &[&str] = &[
        "bad",
        "fail",
        "failed",
        "error",
        "down",
        "critical",
        "red",
        "overdue",
        "stale",
        "high",
        "breach",
        "alarm",
        "alert",
        "shrink",
        "shrinking",
        "decline",
        "declining",
        "fall",
        "falling",
        "wrong",
        "miss",
        "missed",
        "verfehlt",
        "fehler",
        "kritisch",
    ];
    let t = text.to_lowercase();
    let words: Vec<&str> = t
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();
    // "on track" / "at risk" (any separator) read as one word
    let pairs: Vec<String> = words
        .windows(2)
        .map(|p| format!("{}{}", p[0], p[1]))
        .collect();
    let has = |list: &[&str]| {
        words.iter().any(|w| list.contains(w)) || pairs.iter().any(|p| list.contains(&p.as_str()))
    };
    if has(GOOD) {
        Some(Status::Good)
    } else if has(WARN) {
        Some(Status::Warn)
    } else if has(BAD) {
        Some(Status::Bad)
    } else {
        None
    }
}

fn quoted(v: &[String]) -> String {
    let mut s: Vec<String> = v.iter().take(5).map(|x| format!("`{x}`")).collect();
    if v.len() > 5 {
        s.push(format!("and {} more", v.len() - 5));
    }
    s.join(", ")
}

// ---- layout ----------------------------------------------------------------

/// A laid-out diagram, in px with the origin at the top-left margin: the
/// layered placement (`graph_layout`, #1153) and each node's label wrapped
/// to fit its box.
struct Placed {
    g: ggplot_rs::graph_layout::GraphLayout,
    lines: Vec<Vec<String>>,
}

impl std::ops::Deref for Placed {
    type Target = ggplot_rs::graph_layout::GraphLayout;
    fn deref(&self) -> &Self::Target {
        &self.g
    }
}

/// `keys` ordered by `(primary, secondary, index)`, as the indices.
fn ordered(mut keys: Vec<(f64, f64, usize)>) -> Vec<usize> {
    keys.sort_unstable_by(|a, b| {
        a.0.total_cmp(&b.0)
            .then(a.1.total_cmp(&b.1))
            .then(a.2.cmp(&b.2))
    });
    keys.into_iter().map(|k| k.2).collect()
}

/// The layout: box sizes (the label wrapped, the value, badge and
/// sparkline) and the layered placement, both ggplot-rs's (#1153, #497).
fn layout(d: &Diagram) -> Placed {
    use ggplot_rs::graph::{edge_label_width, node_box};
    use ggplot_rs::graph_layout as gl;
    let boxes: Vec<_> = d
        .nodes
        .iter()
        .map(|n| {
            node_box(
                &n.label,
                n.fvalue.as_deref(),
                n.badge.as_deref(),
                n.spark.len() >= 2,
            )
        })
        .collect();
    let nodes: Vec<gl::GraphNode> = d
        .nodes
        .iter()
        .zip(&boxes)
        .map(|(n, b)| gl::GraphNode {
            w: b.w,
            h: b.h,
            lane: n.lane,
            pin: n.pin,
        })
        .collect();
    let edges: Vec<gl::GraphEdge> = d
        .edges
        .iter()
        .map(|e| gl::GraphEdge {
            from: e.from,
            to: e.to,
            label_w: e.label.as_deref().map(edge_label_width),
        })
        .collect();
    let params = gl::LayeredParams {
        direction: match d.dir {
            Dir::LR => gl::Direction::LeftRight,
            Dir::TB => gl::Direction::TopBottom,
        },
        lanes: d.lanes.len(),
        ..Default::default()
    };
    Placed {
        g: gl::layered(&nodes, &edges, &params),
        lines: boxes.into_iter().map(|b| b.lines).collect(),
    }
}

/// An edge as read, before its ids are resolved.
#[derive(Default)]
struct RawEdge {
    from: String,
    to: String,
    value: Option<f64>,
    label: Option<String>,
    style: Option<EdgeStyle>,
    status: Option<Status>,
}

// ---- text alternative ------------------------------------------------------

/// Nodes in reading order (layer by layer).
fn reading(p: &Placed) -> Vec<usize> {
    ordered(
        (0..p.nodes.len())
            .map(|i| (p.nodes[i].order as f64, 0.0, i))
            .collect(),
    )
}

fn list_names(names: &[String]) -> String {
    match names.len() {
        0 => String::new(),
        1 => names[0].clone(),
        _ if names.len() <= 3 => {
            format!(
                "{} and {}",
                names[..names.len() - 1].join(", "),
                names[names.len() - 1]
            )
        }
        _ => format!("{}, and {} more", names[..3].join(", "), names.len() - 3),
    }
}

/// One sentence for screen readers (without the panel title and the
/// trailing full stop): steps, stages, where it starts and ends, the
/// connections, loops and the largest value.
fn summary(d: &Diagram, p: &Placed) -> String {
    let n = d.nodes.len();
    let order = reading(p);
    let has_in = |v: usize| d.edges.iter().any(|e| e.to == v && e.from != v);
    let has_out = |v: usize| d.edges.iter().any(|e| e.from == v && e.to != v);
    let names = |f: &dyn Fn(usize) -> bool| -> Vec<String> {
        order
            .iter()
            .filter(|&&v| f(v))
            .map(|&v| d.nodes[v].label.clone())
            .collect()
    };
    let mut first = format!("{n} step{}", if n == 1 { "" } else { "s" });
    if p.layers > 1 {
        first.push_str(&format!(" in {} stages", p.layers));
    }
    let starts = names(&|v| !has_in(v) && has_out(v));
    let ends = names(&|v| has_in(v) && !has_out(v));
    if !starts.is_empty() {
        first.push_str(&format!(", from {}", list_names(&starts)));
    }
    if !ends.is_empty() {
        first.push_str(&format!(" to {}", list_names(&ends)));
    }
    let mut parts = vec![first];
    let m = d.edges.len();
    if m > 0 {
        parts.push(format!("{m} connection{}", if m == 1 { "" } else { "s" }));
    }
    let back: Vec<String> = (0..m)
        .filter(|&k| p.back[k] || d.edges[k].from == d.edges[k].to)
        .map(|k| d.edge_name(k))
        .collect();
    if !back.is_empty() {
        parts.push(format!(
            "{} loop{} ({})",
            back.len(),
            if back.len() == 1 { "" } else { "s" },
            list_names(&back)
        ));
    }
    if let Some(top) = d
        .nodes
        .iter()
        .filter(|x| x.value.is_some())
        .max_by(|a, b| a.value.unwrap_or(0.0).total_cmp(&b.value.unwrap_or(0.0)))
    {
        if d.nodes.iter().filter(|x| x.value.is_some()).count() > 1 {
            parts.push(format!(
                "largest {} ({})",
                top.label,
                top.fvalue.clone().unwrap_or_default()
            ));
        }
    }
    if d.total_nodes > n {
        parts.push(format!("showing {n} of {} nodes", d.total_nodes));
    }
    parts.join("; ")
}

/// The steps in reading order, each with its value, status and where it
/// leads: "1. Ingest (10K) → Build twin, Replay."
fn steps_text(d: &Diagram, p: &Placed) -> String {
    reading(p)
        .iter()
        .enumerate()
        .map(|(i, &v)| {
            let nd = &d.nodes[v];
            let mut s = format!("{}. {}", i + 1, nd.label);
            let extra: Vec<&str> = [nd.fvalue.as_deref(), nd.badge.as_deref()]
                .into_iter()
                .flatten()
                .collect();
            if !extra.is_empty() {
                s.push_str(&format!(" ({})", extra.join(", ")));
            }
            let next: Vec<String> = d
                .edges
                .iter()
                .filter(|e| e.from == v)
                .map(|e| {
                    let mut t = d.nodes[e.to].label.clone();
                    if let Some(l) = e.label.as_deref().or(e.fvalue.as_deref()) {
                        t.push_str(&format!(" ({l})"));
                    }
                    t
                })
                .collect();
            if !next.is_empty() {
                s.push_str(&format!(" → {}", next.join(", ")));
            }
            s.push('.');
            s
        })
        .collect::<Vec<_>>()
        .join(" ")
}

// ---- drawing (ggplot-rs) -----------------------------------------------------
//
// ggplot-rs draws the laid-out graph (`ggplot_rs::graph`, #497); here is
// what each mark says and what colour it takes. The mark contract
// web/anim.js and web/app.js read stays as before: `rect[data-node]` boxes
// and `polyline[data-edge]` edges with a `<title>`, arrowheads
// (`polygon[data-from]`) and edge labels carrying `data-from`/`data-to`,
// and a node's contents (accent, label, value, badge, sparkline) inside one
// `<g data-of="<id>">`, inert (`pointer-events="none"`,
// `aria-hidden="true"`) so hover, click and screen readers reach the box
// below.

/// `'flow-volume'`/`'flow-dots-volume'`: an edge's animation speed bucket
/// from what it carries, on the same area-true sqrt scale as its width.
/// Five fixed, equal-width buckets of `sqrt(v / max)` (not quantiles: a
/// diagram's few edges would make quantile buckets jump around between
/// refreshes, and equal steps keep "twice as busy" looking faster
/// everywhere). `flow-speed-1` is the busiest (fastest, 0.6 s a cycle) …
/// `flow-speed-5` a trickle (2.4 s) — the durations live in
/// web/theme-base.css, a class instead of an inline `style`, so a strict
/// CSP holds.
fn speed_class(v: f64, vmax: f64) -> &'static str {
    let r = (v.max(0.0) / vmax).sqrt();
    match r {
        r if r >= 0.8 => "flow-speed-1",
        r if r >= 0.6 => "flow-speed-2",
        r if r >= 0.4 => "flow-speed-3",
        r if r >= 0.2 => "flow-speed-4",
        _ => "flow-speed-5",
    }
}

/// `(name, value)` as a `data-*` attribute.
fn attr(name: &str, v: Option<String>) -> ggplot_rs::graph::DataAttr {
    (name.to_string(), v)
}

/// The drawing as a nested `<svg>` of `pw` × `ph` (the layout's own size).
fn draw(d: &Diagram, p: &Placed, pw: u32, ph: u32, o: &RenderOptions) -> String {
    use ggplot_rs::graph::{self, DrawEdge, DrawNode, EdgeLine, GraphStyle};

    let t = &o.theme;
    let muted = t.colors.muted;
    let vmax = d.edges.iter().filter_map(|e| e.value).fold(0.0, f64::max);
    let edges: Vec<DrawEdge> = d
        .edges
        .iter()
        .enumerate()
        .map(|(k, e)| {
            let line = match e.style {
                EdgeStyle::Solid => EdgeLine::Solid,
                EdgeStyle::Dashed | EdgeStyle::Flow(_) => EdgeLine::Dashed,
                EdgeStyle::Dots(_) => EdgeLine::Dots,
            };
            // health first (warn / bad), else a flowing edge in the brand colour
            let colour = match (e.status, e.style) {
                (Some(st @ (Status::Bad | Status::Warn)), _) => st.color(t),
                (_, EdgeStyle::Flow(_) | EdgeStyle::Dots(_)) => t.color(0),
                _ => muted,
            };
            let class = match e.style {
                EdgeStyle::Solid | EdgeStyle::Dashed => None,
                EdgeStyle::Flow(_) => Some("dp-flow"),
                EdgeStyle::Dots(_) => Some("dp-flow dp-dots"),
            };
            let speed = match (e.style, e.value) {
                (EdgeStyle::Flow(true) | EdgeStyle::Dots(true), Some(v)) if vmax > 0.0 => {
                    Some(speed_class(v, vmax))
                }
                _ => None,
            };
            let class = match (class, speed) {
                (Some(c), Some(s)) => Some(format!("{c} {s}")),
                (c, _) => c.map(str::to_string),
            };
            let (a, b) = (&d.nodes[e.from], &d.nodes[e.to]);
            let name = format!("{} → {}", a.label, b.label);
            let tooltip = match (&e.label, &e.fvalue) {
                (Some(l), _) => format!("{name}: {l}"),
                (None, Some(v)) => format!("{name}: {v}"),
                _ => name.clone(),
            };
            DrawEdge {
                colour,
                width: graph::flow_width(e.value, vmax, line),
                line,
                class,
                tooltip,
                label: e.label.clone(),
                attrs: vec![
                    attr("edge", Some(format!("{}>{}", a.id, b.id))),
                    attr("from", Some(a.id.clone())),
                    attr("to", Some(b.id.clone())),
                    attr("xtext", Some(name)),
                    attr("value", e.value.map(|v| v.to_string())),
                    attr(
                        "vtext",
                        e.value.map(|_| e.fvalue.clone().unwrap_or_default()),
                    ),
                    attr("status", e.status.map(|s| s.as_str().to_string())),
                    attr("back", p.back[k].then(|| "1".to_string())),
                ],
                label_attrs: vec![
                    attr("from", Some(a.id.clone())),
                    attr("to", Some(b.id.clone())),
                ],
                ..Default::default()
            }
        })
        .collect();
    let nodes: Vec<DrawNode> = d
        .nodes
        .iter()
        .enumerate()
        .map(|(v, nd)| {
            let pn = &p.nodes[v];
            let alarm = nd
                .status
                .filter(|s| *s == Status::Bad || *s == Status::Warn);
            let mut tooltip = nd.label.clone();
            if let Some(fv) = &nd.fvalue {
                tooltip.push_str(&format!(": {fv}"));
            }
            if let Some(b) = &nd.badge {
                tooltip.push_str(&format!(" ({b})"));
            }
            let accent = nd
                .status
                .filter(|s| *s != Status::Neutral)
                .map(|s| s.color(t))
                .or_else(|| {
                    nd.category
                        .map(|c| match d.category_colors.get(c).copied().flatten() {
                            Some(lc) => lc.rgb(t),
                            None => t.color(c),
                        })
                });
            DrawNode {
                id: nd.id.clone(),
                lines: p.lines[v].clone(),
                value: nd.fvalue.clone(),
                badge: nd
                    .badge
                    .clone()
                    .map(|b| (b, nd.status.map(|s| s.color(t)).unwrap_or(muted))),
                stroke: alarm.map(|s| s.color(t)).unwrap_or(t.chart.line),
                accent,
                spark: (nd.spark.len() >= 2).then(|| {
                    (
                        nd.spark.clone(),
                        alarm.map(|s| s.color(t)).unwrap_or_else(|| t.color(0)),
                    )
                }),
                tooltip,
                attrs: vec![
                    attr("node", Some(nd.id.clone())),
                    attr("x", Some(nd.id.clone())),
                    attr("series", Some(nd.id.clone())),
                    attr("xtext", Some(nd.label.clone())),
                    attr("layer", Some(pn.layer.to_string())),
                    attr("order", Some(pn.order.to_string())),
                    attr("value", nd.value.map(|v| v.to_string())),
                    attr(
                        "vtext",
                        nd.value.map(|_| nd.fvalue.clone().unwrap_or_default()),
                    ),
                    attr("status", nd.status.map(|s| s.as_str().to_string())),
                    attr("badge", nd.badge.clone()),
                ],
                ..Default::default()
            }
        })
        .collect();
    let style = GraphStyle {
        card: t.colors.card,
        band: t.colors.line,
        muted,
        heading: t.colors.heading,
        ink: t.colors.ink,
        class_prefix: "dp-".to_string(),
        ..Default::default()
    };
    graph::draw(&p.g, &d.lanes, &[], &nodes, &edges, &style, crate::void_theme(o))
        .render_svg_native_at(0.0, 0.0, pw, ph)
        .unwrap_or_default()
}

/// Render the diagram as a whole `<svg>` (or a nested fragment at `place`)
/// of `width` × `height`: the drawing scaled down to fit (never up) and
/// centred, with `data-diagram` (the direction) and `data-summary` on the
/// root and the text alternative as its `<desc>`.
pub(crate) fn render(
    d: &Diagram,
    place: Place,
    width: u32,
    height: u32,
    o: &RenderOptions,
) -> String {
    let t = &o.theme;
    let (w, h) = (width as f64, height as f64);
    let font = esc(&t.font.family);
    if d.nodes.is_empty() {
        let body = format!(
            "<text x=\"{:.1}\" y=\"{:.1}\" text-anchor=\"middle\" dominant-baseline=\"middle\" font-family=\"{font}\" font-size=\"13\" fill=\"{}\">No data</text>",
            w / 2.0,
            h / 2.0,
            hex(t.chart.note)
        );
        return format!("{}{body}</svg>", svg_open(place, width, height));
    }
    let p = layout(d);
    let summary = summary(d, &p);
    let note = (d.total_nodes > d.nodes.len() || d.total_edges > d.edges.len()).then(|| {
        format!(
            "Showing the first {} of {} nodes and {} of {} edges",
            d.nodes.len(),
            d.total_nodes,
            d.edges.len(),
            d.total_edges
        )
    });
    let note_h = if note.is_some() { 18.0 } else { 0.0 };
    // the drawing's own canvas: whole px, so a data unit stays a pixel
    let (pw, ph) = (p.w.ceil().max(1.0), p.h.ceil().max(1.0));
    let (aw, ah) = ((w - 4.0).max(10.0), (h - 4.0 - note_h).max(10.0));
    let s = (aw / pw).min(ah / ph).min(1.0);
    let s = (s * 1000.0).floor() / 1000.0;
    let tx = ((w - pw * s) / 2.0).max(0.0);
    let ty = ((h - note_h - ph * s) / 2.0).max(0.0);

    let mut out = svg_open(place, width, height);
    out.push_str(&format!(
        "<desc>{}</desc>",
        esc(&format!("Diagram: {summary}. Steps: {}", steps_text(d, &p)))
    ));
    out.push_str(&format!(
        "<g class=\"dp-diagram\" transform=\"translate({:.1},{:.1}) scale({})\" font-family=\"{font}\">",
        tx, ty, s
    ));
    out.push_str(&draw(d, &p, pw as u32, ph as u32, o));
    out.push_str("</g>");
    if let Some(nt) = note {
        out.push_str(&format!(
            "<text x=\"{:.1}\" y=\"{:.1}\" text-anchor=\"middle\" font-family=\"{font}\" font-size=\"11\" fill=\"{}\">{}</text>",
            w / 2.0,
            h - 5.0,
            hex(t.chart.note),
            esc(&nt)
        ));
    }
    out.push_str("</svg>");
    let mut out = with_root_attr(out, "data-summary", &summary);
    // motion (web/anim.js): steps keyed by node id, the entrance mode
    let steps: Vec<J> = d
        .nodes
        .iter()
        .filter_map(|n| Some(serde_json::json!({ "k": n.id, "t": n.story.as_ref()? })))
        .collect();
    if !steps.is_empty() {
        out = with_root_attr(out, "data-story-by", "x");
        out = with_root_attr(out, "data-story", &J::Array(steps).to_string());
    }
    if let Some(a) = &d.animate {
        out = with_root_attr(out, "data-animate", a);
    }
    if d.tween {
        out = with_root_attr(out, "data-transition", "tween");
    }
    // the width (px) at which the drawing shows at its own size: a narrow
    // screen scrolls the panel sideways instead of shrinking the text.
    // For an extreme diagram/canvas aspect ratio `s` can floor to exactly
    // 0.0 (#149: case 373 of the spec fuzzer — a huge layout squeezed into a
    // clamped-to-32px canvas), and `w / 0.0` is a non-finite `data-fit`.
    // Fall back to the diagram's own (always-finite) unscaled width.
    let fit = if s > 0.0 { w / s } else { p.w };
    out = with_root_attr(out, "data-fit", &format!("{:.0}", fit));
    with_root_attr(out, "data-diagram", d.dir.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{render_with, Kind};

    fn s(v: &str) -> Value {
        Value::Str(v.into())
    }

    /// The inventory pipeline of inventory-rs#28: 7 stages, a loop
    /// calibrate → build.
    fn pipeline() -> Vec<Column> {
        let ids = [
            "ingest",
            "build",
            "step",
            "monitor",
            "calibrate",
            "recommend",
            "replay",
        ];
        let up = [
            "[]",
            "[\"ingest\",\"calibrate\"]",
            "[\"build\"]",
            "[\"step\",\"replay\"]",
            "[\"monitor\"]",
            "[\"step\"]",
            "[\"build\"]",
        ];
        vec![
            Column::new("c0", Role::Diagram, ids.iter().map(|i| s(i)).collect()),
            Column::new(
                "c1",
                Role::Label,
                [
                    "Ingest",
                    "Build twin",
                    "Step & project",
                    "Monitor drift",
                    "Calibrate",
                    "Recommend orders",
                    "Replay history",
                ]
                .iter()
                .map(|i| s(i))
                .collect(),
            ),
            Column::new(
                "c2",
                Role::Metric(MetricFmt::Compact),
                [10000.0, 9800.0, 9800.0, 412.0, 37.0, 1200.0, 9800.0]
                    .iter()
                    .map(|&v| Value::Float(v))
                    .collect(),
            ),
            Column::new("c3", Role::Upstream, up.iter().map(|i| s(i)).collect()),
        ]
    }

    #[test]
    fn parses_nodes_and_upstream_edges() {
        let d = Diagram::from_columns(&pipeline(), &Locale::default()).unwrap();
        assert_eq!(d.nodes.len(), 7);
        assert_eq!(d.edges.len(), 8);
        assert_eq!(d.nodes[1].fvalue.as_deref(), Some("9.8K"));
        assert!(d.problems.is_empty(), "{:?}", d.problems);
        let back: Vec<String> = d.back_edges().iter().map(|&k| d.edge_name(k)).collect();
        assert_eq!(back, ["Calibrate → Build twin"]);
    }

    #[test]
    fn no_diagram_column_is_none() {
        let cols = vec![Column::new(
            "c0",
            Role::Value(Kind::Bar),
            vec![Value::Float(1.0)],
        )];
        assert!(Diagram::from_columns(&cols, &Locale::default()).is_none());
    }

    #[test]
    fn edges_struct_list_merges_with_upstream() {
        let mut cols = pipeline();
        cols.push(Column::new(
            "c4",
            Role::Edges,
            vec![
                s(r#"[{"from":"ingest","to":"build","value":9800,"label":"9.8K records","style":"flow"},
                      {"source":"monitor","target":"calibrate","value":3},
                      {"from":"nowhere","to":"build"},
                      {"to":"x"}]"#),
                Value::Na,
                Value::Na,
                Value::Na,
                Value::Na,
                Value::Na,
                Value::Na,
            ],
        ));
        let d = Diagram::from_columns(&cols, &Locale::default()).unwrap();
        assert_eq!(d.edges.len(), 8, "merged by (from, to)");
        let e = &d.edges[0];
        assert_eq!((e.from, e.to, e.style), (0, 1, EdgeStyle::Flow(false)));
        assert_eq!(e.label.as_deref(), Some("9.8K records"));
        let codes: Vec<&str> = d.problems.iter().map(|p| p.code).collect();
        assert_eq!(codes, ["diagram/bad-edges", "diagram/unknown-node"]);
    }

    /// #519: an embedded "\n" in an edge label (e.g. "5.4K\norders") is one
    /// `geom_text` row, drawn as two haloed `<text>` lines `size * 1.2` px
    /// apart (ggplot-rs 0.31.0's native multi-line layout, sipemu/
    /// ggplot-rs#249) — not the #229 hand-stacked row per line.
    #[test]
    fn multiline_edge_label_uses_native_line_breaks() {
        let mut cols = pipeline();
        cols.push(Column::new(
            "c4",
            Role::Edges,
            vec![
                s(r#"[{"from":"ingest","to":"build","value":5400,"label":"5.4K\norders","style":"flow"}]"#),
                Value::Na,
                Value::Na,
                Value::Na,
                Value::Na,
                Value::Na,
                Value::Na,
            ],
        ));
        let svg = render_with(&cols, 900, 320, &RenderOptions::default()).unwrap();
        let doc = roxmltree::Document::parse(&svg).expect("well-formed");
        let lines: Vec<_> = doc
            .descendants()
            .filter(|n| n.attribute("class") == Some("gg-mark gg-text dp-edge-label"))
            .collect();
        assert_eq!(lines.len(), 2, "one <text> per line, not a chip per line");
        assert_eq!(lines[0].text(), Some("5.4K"));
        assert_eq!(lines[1].text(), Some("orders"));
        // same x (centred on the midpoint), halo on every line, line height
        // exactly `size * 1.2` = 13.2px apart.
        let y0: f64 = lines[0].attribute("y").unwrap().parse().unwrap();
        let y1: f64 = lines[1].attribute("y").unwrap().parse().unwrap();
        assert_eq!(lines[0].attribute("x"), lines[1].attribute("x"));
        assert!((y1 - y0 - 13.2).abs() < 1e-6);
        for l in &lines {
            assert_eq!(l.attribute("stroke-width"), Some("4.00"), "halo");
            assert_eq!(l.attribute("aria-hidden"), Some("true"));
        }
    }

    #[test]
    fn problems_are_reported() {
        let cols = vec![
            Column::new("c0", Role::Diagram, vec![s("a"), s("a"), Value::Na, s("b")]),
            Column::new(
                "c1",
                Role::Upstream,
                vec![s("b"), Value::Na, Value::Na, s("a")],
            ),
            Column::new(
                "c2",
                Role::Direction,
                vec![s("down"), s("down"), s("down"), s("down")],
            ),
            Column::new(
                "c3",
                Role::Edges,
                vec![s("not json"), Value::Na, Value::Na, Value::Na],
            ),
        ];
        let d = Diagram::from_columns(&cols, &Locale::default()).unwrap();
        let codes: Vec<&str> = d.problems.iter().map(|p| p.code).collect();
        assert_eq!(
            codes,
            [
                "diagram/bad-direction",
                "diagram/null-id",
                "diagram/duplicate-node",
                "diagram/bad-edges"
            ]
        );
    }

    #[test]
    fn limits_cut_and_report() {
        let n = MAX_NODES + 10;
        let cols = vec![
            Column::new(
                "c0",
                Role::Diagram,
                (0..n).map(|i| s(&format!("n{i}"))).collect(),
            ),
            Column::new(
                "c1",
                Role::Upstream,
                (0..n)
                    .map(|i| {
                        if i == 0 {
                            Value::Na
                        } else {
                            s(&format!("n{}", i - 1))
                        }
                    })
                    .collect(),
            ),
        ];
        let d = Diagram::from_columns(&cols, &Locale::default()).unwrap();
        assert_eq!(d.nodes.len(), MAX_NODES);
        assert_eq!(d.total_nodes, n);
        assert!(d.problems.iter().any(|p| p.code == "diagram/too-large"));
        assert!(!d.problems.iter().any(|p| p.code == "diagram/unknown-node"));
        let svg = render_with(&cols, 800, 400, &RenderOptions::default()).unwrap();
        assert!(svg.contains("Showing the first 150 of 160 nodes"));
    }

    #[test]
    fn layout_runs_left_to_right_without_overlaps() {
        let d = Diagram::from_columns(&pipeline(), &Locale::default()).unwrap();
        let p = layout(&d);
        let x = |id: &str| p.nodes[d.nodes.iter().position(|n| n.id == id).unwrap()].cx;
        assert!(x("ingest") < x("build") && x("build") < x("step") && x("step") < x("monitor"));
        assert!(x("monitor") < x("calibrate"));
        for (i, a) in p.nodes.iter().enumerate() {
            for b in &p.nodes[i + 1..] {
                let apart = (a.cx - b.cx).abs() >= (a.w + b.w) / 2.0
                    || (a.cy - b.cy).abs() >= (a.h + b.h) / 2.0;
                assert!(apart, "nodes overlap");
            }
        }
        // every edge starts at its source's box and ends at its target's
        for (k, e) in d.edges.iter().enumerate() {
            let pts = &p.edges[k];
            let near = |pt: (f64, f64), v: usize| {
                let n = &p.nodes[v];
                (pt.0 - n.cx).abs() <= n.w / 2.0 + 0.5 && (pt.1 - n.cy).abs() <= n.h / 2.0 + 0.5
            };
            assert!(
                near(pts[0], e.from) && near(*pts.last().unwrap(), e.to),
                "edge {k}"
            );
        }
    }

    #[test]
    fn top_to_bottom_stacks_layers_vertically() {
        let mut cols = pipeline();
        cols.push(Column::new("c9", Role::Direction, vec![s("tb"); 7]));
        let d = Diagram::from_columns(&cols, &Locale::default()).unwrap();
        assert_eq!(d.dir, Dir::TB);
        let p = layout(&d);
        assert!(p.nodes[0].cy < p.nodes[1].cy && p.nodes[1].cy < p.nodes[2].cy);
    }

    /// A 40-node random DAG: no crossings lost to chance — the output is a
    /// pure function of the rows (same SVG every time).
    #[test]
    fn deterministic() {
        let mut seed: u64 = 7;
        let mut next = || {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (seed >> 33) as usize
        };
        let n = 40;
        let mut up: Vec<Vec<String>> = vec![Vec::new(); n];
        for (i, u) in up.iter_mut().enumerate().skip(1) {
            for _ in 0..2 {
                let j = format!("\"v{}\"", next() % i);
                if !u.contains(&j) {
                    u.push(j);
                }
            }
        }
        let cols = vec![
            Column::new(
                "c0",
                Role::Diagram,
                (0..n).map(|i| s(&format!("v{i}"))).collect(),
            ),
            Column::new(
                "c1",
                Role::Upstream,
                up.iter()
                    .map(|u| s(&format!("[{}]", u.join(","))))
                    .collect(),
            ),
        ];
        let a = render_with(&cols, 1200, 700, &RenderOptions::default()).unwrap();
        for _ in 0..3 {
            assert_eq!(
                render_with(&cols, 1200, 700, &RenderOptions::default()).unwrap(),
                a
            );
        }
        let d = Diagram::from_columns(&cols, &Locale::default()).unwrap();
        assert!(d.back_edges().is_empty());
        roxmltree::Document::parse(&a).expect("well-formed");
    }

    #[test]
    fn svg_carries_the_mark_contract_and_text_alternative() {
        let mut cols = pipeline();
        cols.push(Column::new(
            "c5",
            Role::Badge,
            ["ok", "ok", "ok", "alarm", "ok", "ok", "ok"]
                .iter()
                .map(|i| s(i))
                .collect(),
        ));
        let svg = render_with(&cols, 900, 320, &RenderOptions::default()).unwrap();
        roxmltree::Document::parse(&svg).expect("well-formed");
        assert!(svg.starts_with("<svg") && svg.contains(" data-diagram=\"LR\""));
        assert!(svg.contains(
            "data-node=\"build\" data-x=\"build\" data-series=\"build\" data-xtext=\"Build twin\""
        ));
        assert!(svg.contains("<title>Monitor drift: 412 (alarm)</title>"));
        assert!(svg.contains("data-edge=\"calibrate&gt;build\""));
        assert!(svg.contains(" data-back=\"1\""));
        assert!(svg.contains(
            "data-summary=\"7 steps in 5 stages, from Ingest to Recommend orders; 8 connections; 1 loop (Calibrate → Build twin); largest Ingest (10K)\""
        ));
        assert!(svg.contains("Steps: 1. Ingest (10K, ok) → Build twin."));
        assert!(svg.contains("data-status=\"bad\""));
        // a hostile label stays text
        let mut cols = pipeline();
        cols[1].values[0] = s("<script>alert(1)</script>");
        let svg = render_with(&cols, 900, 320, &RenderOptions::default()).unwrap();
        assert!(!svg.contains("<script>"));
        roxmltree::Document::parse(&svg).expect("well-formed");
    }

    /// #699: a ::COLORS level map pins a type's colour, whatever slot the
    /// rows give it; an unnamed type keeps the theme palette.
    #[test]
    fn colors_map_pins_category_colours() {
        let col =
            |n: &str, r: Role, v: &[&str]| Column::new(n, r, v.iter().map(|x| s(x)).collect());
        let cols = vec![
            col("id", Role::Diagram, &["a", "b", "c"]),
            col("k", Role::Category, &["stock", "supplier", "demand"]),
            col(
                "m",
                Role::GaugeColors,
                &["supplier:#123456; stock:bad", "", ""],
            ),
        ];
        let d = Diagram::from_columns(&cols, &Locale::default()).unwrap();
        assert!(d.category_colors[0].is_some() && d.category_colors[1].is_some());
        assert!(d.category_colors[2].is_none());
        let svg = render_with(&cols, 600, 240, &RenderOptions::default()).unwrap();
        assert!(svg.contains("#123456"), "{svg}");
        let o = RenderOptions::default();
        let bad = crate::theme::hex(Status::Bad.color(&o.theme));
        assert!(
            svg.to_lowercase().contains(&bad.to_lowercase()),
            "stock → {bad}: {svg}"
        );
    }

    #[test]
    fn dark_theme_uses_its_own_card_colour() {
        let o = RenderOptions {
            theme: std::sync::Arc::new(
                crate::theme::resolve("dark")
                    .expect("dark theme")
                    .pick(crate::theme::Variant::Dark)
                    .clone(),
            ),
            ..RenderOptions::default()
        };
        let svg = render_with(&pipeline(), 900, 320, &o).unwrap();
        assert!(svg.contains(&format!("fill=\"{}\"", hex(o.theme.colors.card))));
        let light = render_with(&pipeline(), 900, 320, &RenderOptions::default()).unwrap();
        assert_ne!(svg, light);
    }

    #[test]
    fn badge_words_match_the_table_pills() {
        assert_eq!(badge_status("OK"), Some(Status::Good));
        assert_eq!(badge_status("on track"), Some(Status::Good));
        assert_eq!(badge_status("at-risk"), Some(Status::Warn));
        assert_eq!(badge_status("alarm"), Some(Status::Bad));
        assert_eq!(badge_status("Fehler"), Some(Status::Bad));
        assert_eq!(badge_status("rerun"), None);
    }

    /// #553: "late" is its own warning (amber), distinct from "failed"
    /// (red) — before this, both landed in the BAD bucket and a late node
    /// and a failed node were colour-identical in the diagram. "delayed"
    /// (already warn) and "late" share the warn colour, which is fine:
    /// they're both timing problems, as opposed to a hard failure.
    #[test]
    fn late_is_a_warning_not_a_failure() {
        assert_eq!(badge_status("late"), Some(Status::Warn));
        assert_eq!(badge_status("running late"), Some(Status::Warn));
        assert_eq!(badge_status("delayed"), Some(Status::Warn));
        assert_eq!(badge_status("failed"), Some(Status::Bad));
        assert_ne!(badge_status("late"), badge_status("failed"));
    }

    /// M2 motion: story steps keyed by node id, the entrance mode, edge
    /// styles from ::EDGES over a per-row ::LINESTYLE, and everything a step
    /// dims tagged with the node (`data-of`) or edge (`data-from`) it belongs to.
    #[test]
    fn motion_attributes() {
        let mut cols = pipeline();
        let story = |v: &[Option<&str>]| v.iter().map(|c| c.map_or(Value::Na, s)).collect();
        cols.push(Column::new(
            "c5",
            Role::Story,
            story(&[
                Some("Records arrive"),
                None,
                None,
                Some("Twins drift"),
                None,
                None,
                None,
            ]),
        ));
        cols.push(Column::new("c6", Role::Animate, vec![s("auto"); 7]));
        cols.push(Column::new(
            "c7",
            Role::LineStyle,
            story(&[
                None,
                Some("flow-dots"),
                None,
                None,
                None,
                None,
                Some("wobbly"),
            ]),
        ));
        cols.push(Column::new(
            "c8",
            Role::Edges,
            vec![
                s(r#"[{"from":"calibrate","to":"build","style":"dashed"}]"#),
                Value::Na,
                Value::Na,
                Value::Na,
                Value::Na,
                Value::Na,
                Value::Na,
            ],
        ));
        let d = Diagram::from_columns(&cols, &Locale::default()).unwrap();
        let style = |f: &str, t: &str| {
            d.edges
                .iter()
                .find(|e| d.nodes[e.from].id == f && d.nodes[e.to].id == t)
                .unwrap()
                .style
        };
        assert_eq!(
            style("ingest", "build"),
            EdgeStyle::Dots(false),
            "the row's ::LINESTYLE"
        );
        assert_eq!(
            style("calibrate", "build"),
            EdgeStyle::Dashed,
            "::EDGES wins"
        );
        assert_eq!(style("build", "step"), EdgeStyle::Solid);
        let codes: Vec<&str> = d.problems.iter().map(|p| p.code).collect();
        assert_eq!(codes, ["diagram/bad-style"]);

        let svg = render_with(&cols, 900, 320, &RenderOptions::default()).unwrap();
        roxmltree::Document::parse(&svg).expect("well-formed");
        let root = &svg[..svg.find('>').unwrap()];
        assert!(root.contains(
            r#"data-story="[{&quot;k&quot;:&quot;ingest&quot;,&quot;t&quot;:&quot;Records arrive&quot;},{&quot;k&quot;:&quot;monitor&quot;,&quot;t&quot;:&quot;Twins drift&quot;}]""#
        ));
        assert!(root.contains(r#"data-story-by="x""#) && root.contains(r#"data-animate="auto""#));
        // the moving-dots edge: round-capped zero-length dashes and its
        // classes (beside ggplot-rs's own `gg-mark gg-line`)
        let doc = roxmltree::Document::parse(&svg).unwrap();
        let find = |tag: &str, k: &str, v: &str| {
            doc.descendants()
                .find(|n| n.has_tag_name(tag) && n.attribute(k) == Some(v))
                .unwrap_or_else(|| panic!("no <{tag} {k}={v:?}>"))
        };
        let dots = find("polyline", "data-edge", "ingest>build");
        assert_eq!(dots.attribute("stroke-dasharray"), Some("0,10"));
        assert_eq!(dots.attribute("stroke-linecap"), Some("round"));
        let class = dots.attribute("class").unwrap();
        assert!(
            class.split(' ').any(|c| c == "dp-flow") && class.split(' ').any(|c| c == "dp-dots")
        );
        // a node's contents in one `data-of` group, every part inert
        let g = find("g", "data-of", "build");
        let parts: Vec<_> = g.children().filter(|n| n.is_element()).collect();
        assert!(parts.iter().any(|n| n.text() == Some("Build twin")));
        assert!(parts
            .iter()
            .all(|n| n.attribute("pointer-events") == Some("none")
                && n.attribute("aria-hidden") == Some("true")));
        // the arrowhead: tagged with its edge, hidden from screen readers
        let head = doc
            .descendants()
            .find(|n| {
                n.has_tag_name("polygon")
                    && n.attribute("data-part") == Some("arrow")
                    && n.attribute("data-from") == Some("ingest")
                    && n.attribute("data-to") == Some("build")
            })
            .expect("arrowhead");
        assert_eq!(head.attribute("aria-hidden"), Some("true"));
        // no story, no animate: no motion attributes
        let plain = render_with(&pipeline(), 900, 320, &RenderOptions::default()).unwrap();
        assert!(!plain.contains("data-story") && !plain.contains("data-animate"));
    }

    /// Edges are as wide as what they carry (sqrt, 1.4–7 px) and take their
    /// health colour; `'tween'::TRANSITION` and the fit width reach the root.
    #[test]
    fn edge_width_health_and_tween() {
        let mut cols = pipeline();
        cols.push(Column::new(
            "c5",
            Role::Edges,
            vec![
                s(
                    r#"[{"from":"ingest","to":"build","value":10000,"status":"ok"},
                      {"from":"build","to":"step","value":2500,"status":"delayed"},
                      {"from":"step","to":"monitor","value":0,"status":"failed"}]"#,
                ),
                Value::Na,
                Value::Na,
                Value::Na,
                Value::Na,
                Value::Na,
                Value::Na,
            ],
        ));
        cols.push(Column::new("c6", Role::Transition, vec![s("tween"); 7]));
        let svg = render_with(&cols, 900, 320, &RenderOptions::default()).unwrap();
        roxmltree::Document::parse(&svg).expect("well-formed");
        let t = &RenderOptions::default().theme;
        let edge = |id: &str| {
            let i = svg.find(&format!("data-edge=\"{id}\"")).unwrap();
            let start = svg[..i].rfind("<polyline").unwrap();
            svg[start..i].to_lowercase() // ggplot-rs writes #RRGGBB
        };
        assert!(edge("ingest&gt;build").contains(r#"stroke-width="7.00""#));
        assert!(edge("build&gt;step").contains(r#"stroke-width="4.20""#));
        assert!(edge("build&gt;step").contains(&format!("stroke=\"{}\"", hex(t.colors.warning))));
        assert!(edge("step&gt;monitor").contains(r#"stroke-width="1.40""#));
        assert!(edge("step&gt;monitor").contains(&format!("stroke=\"{}\"", hex(t.colors.negative))));
        assert!(svg.contains(r#"data-status="bad""#));
        // an edge without a value keeps the hairline
        assert!(edge("monitor&gt;calibrate").contains(r#"stroke-width="1.60""#));
        let root = &svg[..svg.find('>').unwrap()];
        assert!(root.contains(r#"data-transition="tween""#) && root.contains(" data-fit=\""));
    }

    /// #149: a wide layout (many unconnected nodes, so none wrap into a
    /// second row) squeezed into a canvas clamped down to `MIN_DIM` used to
    /// floor the fit scale to exactly `0.0`, making `data-fit = w / 0.0`
    /// non-finite (fuzz case 373: `data-fit="inf"`). `data-fit` must always
    /// parse as a finite number, however degenerate the aspect ratio.
    #[test]
    fn data_fit_is_finite_for_a_degenerate_aspect_ratio() {
        let n = 40;
        let ids: Vec<Value> = (0..n).map(|i| s(&format!("n{i}"))).collect();
        let cols = vec![Column::new("c0", Role::Diagram, ids)];
        // width=1 is clamped up to MIN_DIM (32px): far too narrow for 40
        // unconnected nodes laid out side by side.
        let svg = render_with(&cols, 1, 640, &RenderOptions::default()).unwrap();
        let doc = roxmltree::Document::parse(&svg).expect("well-formed");
        let fit = doc.root_element().attribute("data-fit").unwrap();
        let v: f64 = fit.parse().unwrap();
        assert!(v.is_finite(), "data-fit=\"{fit}\" is not finite");
    }

    /// M3: swimlanes keep each lane's nodes in one band (bands do not
    /// overlap), `::XAXIS` pins a stage, `::XAXIS` + `::YAXIS` on every node
    /// places it, a sparkline sits in its box, `'flow-volume'` speeds up with
    /// the value.
    #[test]
    fn lanes_pins_sparklines_and_speed() {
        let lane = |v: &[&str]| v.iter().map(|x| s(x)).collect();
        let mut cols = pipeline();
        cols.push(Column::new(
            "c5",
            Role::Lane,
            lane(&["Data", "Twin", "Twin", "Twin", "Twin", "Stores", "Twin"]),
        ));
        cols.push(Column::new(
            "c6",
            Role::Value(Kind::Sparkline),
            vec![
                s("[1,3,2,5]"),
                Value::Na,
                Value::Na,
                Value::Na,
                Value::Na,
                Value::Na,
                Value::Na,
            ],
        ));
        let d = Diagram::from_columns(&cols, &Locale::default()).unwrap();
        assert_eq!(d.lanes, ["Data", "Twin", "Stores"]);
        let p = layout(&d);
        assert_eq!(p.bands.len(), 3);
        for w in p.bands.windows(2) {
            assert!(w[0].4 <= w[1].2, "bands overlap: {:?}", p.bands);
        }
        for (v, nd) in d.nodes.iter().enumerate() {
            let (_, _, y0, _, y1) = p.bands[nd.lane.unwrap()];
            assert!(
                p.nodes[v].cy > y0 && p.nodes[v].cy < y1,
                "{} outside its lane",
                nd.id
            );
        }
        let svg = render_with(&cols, 900, 500, &RenderOptions::default()).unwrap();
        roxmltree::Document::parse(&svg).expect("well-formed");
        assert!(svg.contains(r#"data-lane="Stores""#));
        let doc = roxmltree::Document::parse(&svg).unwrap();
        let spark = doc
            .descendants()
            .find(|n| {
                n.attribute("class")
                    .is_some_and(|c| c.split(' ').any(|c| c == "dp-node-spark"))
            })
            .expect("a sparkline");
        assert!(spark.has_tag_name("polyline"));
        assert_eq!(
            spark.parent().and_then(|g| g.attribute("data-of")),
            Some("ingest")
        );

        // ::XAXIS pins a stage: `step` forced to stage 4, an empty stage kept
        let mut cols = pipeline();
        let x = |v: usize| if v == 2 { Value::Float(4.0) } else { Value::Na };
        cols.push(Column::new("c7", Role::X, (0..7).map(x).collect()));
        let d = Diagram::from_columns(&cols, &Locale::default()).unwrap();
        let p = layout(&d);
        assert_eq!(p.nodes[2].layer, 4);
        assert!(d.problems.is_empty(), "{:?}", d.problems);

        // ::XAXIS + ::YAXIS everywhere: drawn where placed, edges straight
        let xy = |v: &[f64]| v.iter().map(|&f| Value::Float(f)).collect();
        let cols = vec![
            Column::new("c0", Role::Diagram, lane(&["a", "b", "c"])),
            Column::new("c1", Role::X, xy(&[0.0, 2.0, 1.0])),
            Column::new("c2", Role::Y, xy(&[0.0, 0.0, 1.0])),
            Column::new("c3", Role::Upstream, vec![Value::Na, s("a"), s("a")]),
        ];
        let d = Diagram::from_columns(&cols, &Locale::default()).unwrap();
        assert!(d.explicit);
        let p = layout(&d);
        assert!(p.nodes[0].cx < p.nodes[2].cx && p.nodes[2].cx < p.nodes[1].cx);
        assert!(p.nodes[0].cy == p.nodes[1].cy && p.nodes[2].cy > p.nodes[0].cy);
        assert!(p.edges.iter().all(|e| e.len() == 2));
        // some nodes pinned in x and y: a note, laid out by stages
        let mut cols = cols;
        cols[2].values[2] = Value::Na;
        let d = Diagram::from_columns(&cols, &Locale::default()).unwrap();
        assert!(!d.explicit && d.problems.iter().any(|p| p.code == "diagram/partial-pins"));

        // speed by volume
        let mut cols = pipeline();
        cols.push(Column::new(
            "c8",
            Role::Edges,
            vec![
                s(
                    r#"[{"from":"ingest","to":"build","value":100,"style":"flow-volume"},
                      {"from":"build","to":"step","value":1,"style":"flow-dots-volume"}]"#,
                ),
                Value::Na,
                Value::Na,
                Value::Na,
                Value::Na,
                Value::Na,
                Value::Na,
            ],
        ));
        let svg = render_with(&cols, 900, 320, &RenderOptions::default()).unwrap();
        // a speed bucket class (web/theme-base.css), no inline style
        let doc = roxmltree::Document::parse(&svg).unwrap();
        let class = |id: &str| {
            doc.descendants()
                .find(|n| n.has_tag_name("polyline") && n.attribute("data-edge") == Some(id))
                .and_then(|n| n.attribute("class"))
                .unwrap()
                .to_string()
        };
        assert!(class("ingest>build").ends_with(" dp-flow flow-speed-1"));
        assert!(class("build>step").ends_with(" dp-flow dp-dots flow-speed-5"));
        assert!(!svg.contains("animation-duration"));
        assert_eq!(speed_class(0.5, 1.0), "flow-speed-2");
    }
}
