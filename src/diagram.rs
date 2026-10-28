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
use crate::theme::{hex, Rgb};
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
        })
    }

    /// The edges drawn against the flow (one per cycle the layout breaks).
    pub fn back_edges(&self) -> Vec<usize> {
        let (dag, _) = break_cycles(self);
        dag.iter()
            .filter(|(_, _, _, rev)| *rev)
            .map(|(_, _, k, _)| *k)
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

/// Box geometry constants (px).
const LABEL_PX: f64 = 13.0;
const VALUE_PX: f64 = 12.0;
const BADGE_PX: f64 = 11.0;
const LINE_H: f64 = 16.0;
const BOX_PAD_X: f64 = 12.0;
const BOX_PAD_Y: f64 = 9.0;
const BOX_MIN_W: f64 = 92.0;
const BOX_MAX_W: f64 = 176.0;
const MARGIN: f64 = 10.0;
const ARROW: f64 = 7.0;
const LOOP: f64 = 16.0;
/// A sparkline's strip in a box.
const SPARK_H: f64 = 22.0;
/// Extra space between swimlanes, and a lane's label strip.
const LANE_GAP: f64 = 22.0;
const LANE_HEAD: f64 = 16.0;

/// Approximate rendered width of `s` at `px` (proportional sans-serif).
fn text_w(s: &str, px: f64, bold: bool) -> f64 {
    let per = if bold { 0.6 } else { 0.56 };
    s.chars().count() as f64 * px * per
}

/// `s` wrapped into at most two lines of `max` px (the second cut with "…").
fn wrap(s: &str, max: f64) -> Vec<String> {
    let cap = ((max / (LABEL_PX * 0.6)).floor() as usize).max(4);
    let mut lines: Vec<String> = Vec::new();
    let mut cur = String::new();
    for word in s.split_whitespace() {
        let mut word = word.to_string();
        while word.chars().count() > cap {
            let head: String = word.chars().take(cap).collect();
            if !cur.is_empty() {
                lines.push(std::mem::take(&mut cur));
            }
            lines.push(head);
            word = word.chars().skip(cap).collect();
        }
        if cur.is_empty() {
            cur = word;
        } else if cur.chars().count() + 1 + word.chars().count() <= cap {
            cur.push(' ');
            cur.push_str(&word);
        } else {
            lines.push(std::mem::replace(&mut cur, word));
        }
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    if lines.len() > 2 {
        let rest = lines[1..].join(" ");
        let mut second: String = rest.chars().take(cap.saturating_sub(1)).collect();
        second.push('…');
        lines.truncate(1);
        lines.push(second);
    }
    lines
}

struct Box_ {
    w: f64,
    h: f64,
    lines: Vec<String>,
}

fn box_of(n: &Node) -> Box_ {
    let lines = wrap(&n.label, BOX_MAX_W - 2.0 * BOX_PAD_X);
    let mut w = lines
        .iter()
        .map(|l| text_w(l, LABEL_PX, true))
        .fold(0.0, f64::max);
    let mut h = 2.0 * BOX_PAD_Y + LINE_H * lines.len() as f64;
    if let Some(v) = &n.fvalue {
        w = w.max(text_w(v, VALUE_PX, true));
        h += LINE_H;
    }
    if let Some(b) = &n.badge {
        w = w.max(text_w(b, BADGE_PX, false) + 22.0);
        h += 20.0;
    }
    if n.spark.len() >= 2 {
        w = w.max(110.0);
        h += SPARK_H;
    }
    Box_ {
        w: (w + 2.0 * BOX_PAD_X).clamp(BOX_MIN_W, BOX_MAX_W),
        h,
        lines,
    }
}

/// A DAG edge: `(u, v, edge index, reversed)`.
type DagEdge = (usize, usize, usize, bool);
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

/// DAG edges `(u, v, edge index, reversed)` — the cycle-free orientation:
/// a DFS in row order reverses every edge that closes a cycle — and the
/// self-loops (edge indices).
fn break_cycles(d: &Diagram) -> (Vec<DagEdge>, Vec<usize>) {
    let n = d.nodes.len();
    let mut out: Vec<Vec<(usize, usize)>> = vec![Vec::new(); n];
    let mut loops = Vec::new();
    for (k, e) in d.edges.iter().enumerate() {
        if e.from == e.to {
            loops.push(k);
        } else {
            out[e.from].push((e.to, k));
        }
    }
    let mut state = vec![0u8; n]; // 0 new, 1 on the DFS stack, 2 done
    let mut rev = vec![false; d.edges.len()];
    for s in 0..n {
        if state[s] != 0 {
            continue;
        }
        state[s] = 1;
        let mut stack = vec![(s, 0usize)];
        while let Some(top) = stack.last_mut() {
            let v = top.0;
            if top.1 < out[v].len() {
                let (w, k) = out[v][top.1];
                top.1 += 1;
                match state[w] {
                    0 => {
                        state[w] = 1;
                        stack.push((w, 0));
                    }
                    1 => rev[k] = true,
                    _ => {}
                }
            } else {
                state[v] = 2;
                stack.pop();
            }
        }
    }
    let dag = d
        .edges
        .iter()
        .enumerate()
        .filter(|(_, e)| e.from != e.to)
        .map(|(k, e)| {
            if rev[k] {
                (e.to, e.from, k, true)
            } else {
                (e.from, e.to, k, false)
            }
        })
        .collect();
    (dag, loops)
}

/// A laid-out diagram, in px with the origin at the top-left margin.
struct Placed {
    w: f64,
    h: f64,
    /// Per node: centre, size, wrapped label, layer, rank in reading order.
    nodes: Vec<PNode>,
    /// Per edge (index as in [`Diagram::edges`]): its points, source → target.
    edges: Vec<Vec<(f64, f64)>>,
    back: Vec<bool>,
    layers: usize,
    /// Swimlane bands: (lane, x0, y0, x1, y1) in px.
    bands: Vec<(usize, f64, f64, f64, f64)>,
}

struct PNode {
    cx: f64,
    cy: f64,
    w: f64,
    h: f64,
    lines: Vec<String>,
    layer: usize,
    order: usize,
}

/// `keys` ordered by `(primary, secondary, index)`, as the indices. The one
/// sort the layout uses: a single instantiation keeps the wasm build small.
fn ordered(mut keys: Vec<(f64, f64, usize)>) -> Vec<usize> {
    keys.sort_unstable_by(|a, b| {
        a.0.total_cmp(&b.0)
            .then(a.1.total_cmp(&b.1))
            .then(a.2.cmp(&b.2))
    });
    keys.into_iter().map(|k| k.2).collect()
}

/// Number of crossings between two adjacent layers (Barth–Mutzel–Jünger:
/// sort the edges by their upper end, count inversions of the lower ends
/// with a Fenwick tree).
fn layer_crossings(upper: &[usize], succ: &[Vec<usize>], pos: &[usize], lower_len: usize) -> u64 {
    let mut ends: Vec<(usize, usize)> = Vec::new();
    for &u in upper {
        for &v in &succ[u] {
            ends.push((pos[u], pos[v]));
        }
    }
    let lower: Vec<usize> = ordered(
        ends.iter()
            .enumerate()
            .map(|(i, &(u, v))| (u as f64, v as f64, i))
            .collect(),
    )
    .into_iter()
    .map(|i| ends[i].1)
    .collect();
    let mut tree = vec![0u64; lower_len + 1];
    let mut c = 0u64;
    for (seen, &p) in lower.iter().enumerate() {
        // already seen ends strictly right of p
        let mut i = p + 1;
        let mut le = 0u64;
        while i > 0 {
            le += tree[i];
            i -= 1 << i.trailing_zeros();
        }
        c += seen as u64 - le;
        let mut i = p + 1;
        while i <= lower_len {
            tree[i] += 1;
            i += 1 << i.trailing_zeros();
        }
    }
    c
}

/// Isotonic placement: positions as close as possible (weighted least
/// squares) to `want`, in the given order, at least `gap[i]` apart from the
/// previous one.
fn place(want: &[f64], weight: &[f64], gap: &[f64]) -> Vec<f64> {
    let n = want.len();
    let mut off = vec![0.0; n];
    for i in 1..n {
        off[i] = off[i - 1] + gap[i];
    }
    // blocks of (weighted sum, weight, count), merged while out of order
    let mut blocks: Vec<(f64, f64, usize)> = Vec::new();
    for i in 0..n {
        blocks.push((weight[i] * (want[i] - off[i]), weight[i], 1));
        while blocks.len() >= 2 {
            let (b, a) = (blocks[blocks.len() - 1], blocks[blocks.len() - 2]);
            if a.0 / a.1 > b.0 / b.1 {
                blocks.pop();
                let last = blocks.last_mut().expect("two blocks");
                *last = (a.0 + b.0, a.1 + b.1, a.2 + b.2);
            } else {
                break;
            }
        }
    }
    let mut out = Vec::with_capacity(n);
    for (s, w, c) in blocks {
        for _ in 0..c {
            let i = out.len();
            out.push(s / w + off[i]);
        }
    }
    out
}

/// Where a line from a box's centre towards `to` leaves the box: the port of
/// a straight edge (pinned nodes, edges against the pinned stages).
fn exit(p: &PNode, to: (f64, f64)) -> (f64, f64) {
    let (dx, dy) = (to.0 - p.cx, to.1 - p.cy);
    let sx = if dx != 0.0 {
        p.w / 2.0 / dx.abs()
    } else {
        f64::INFINITY
    };
    let sy = if dy != 0.0 {
        p.h / 2.0 / dy.abs()
    } else {
        f64::INFINITY
    };
    let k = sx.min(sy);
    if k.is_finite() {
        (p.cx + dx * k, p.cy + dy * k)
    } else {
        (p.cx, p.cy)
    }
}

/// A straight edge between two placed boxes.
fn straight(pn: &[PNode], u: usize, v: usize) -> Vec<(f64, f64)> {
    let (a, b) = (&pn[u], &pn[v]);
    vec![exit(a, (b.cx, b.cy)), exit(b, (a.cx, a.cy))]
}

/// A self-loop over (LR) or beside (TB) its box.
fn self_loop(p: &PNode, lr: bool) -> Vec<(f64, f64)> {
    if lr {
        let top = p.cy - p.h / 2.0;
        vec![
            (p.cx + p.w / 4.0, top),
            (p.cx + p.w / 4.0, top - LOOP),
            (p.cx - p.w / 4.0, top - LOOP),
            (p.cx - p.w / 4.0, top),
        ]
    } else {
        let right = p.cx + p.w / 2.0;
        vec![
            (right, p.cy - p.h / 4.0),
            (right + LOOP, p.cy - p.h / 4.0),
            (right + LOOP, p.cy + p.h / 4.0),
            (right, p.cy + p.h / 4.0),
        ]
    }
}

/// Every node pinned (`::XAXIS` + `::YAXIS`): boxes on a grid of the widest
/// / tallest box, `x` across and `y` down, edges straight.
fn layout_explicit(d: &Diagram) -> Placed {
    let boxes: Vec<Box_> = d.nodes.iter().map(box_of).collect();
    let sx = boxes.iter().map(|b| b.w).fold(0.0, f64::max) + 48.0;
    let sy = boxes.iter().map(|b| b.h).fold(0.0, f64::max) + 24.0;
    let pin = |v: usize| {
        (
            d.nodes[v].pin.0.unwrap_or(0.0),
            d.nodes[v].pin.1.unwrap_or(0.0),
        )
    };
    let mut xs: Vec<f64> = (0..d.nodes.len()).map(|v| pin(v).0).collect();
    xs.sort_by(f64::total_cmp);
    xs.dedup();
    let reading = ordered(
        (0..d.nodes.len())
            .map(|v| (pin(v).0, pin(v).1, v))
            .collect(),
    );
    let mut pn: Vec<PNode> = boxes
        .into_iter()
        .enumerate()
        .map(|(v, b)| PNode {
            cx: pin(v).0 * sx,
            cy: pin(v).1 * sy,
            w: b.w,
            h: b.h,
            lines: b.lines,
            layer: xs.iter().position(|&x| x == pin(v).0).unwrap_or(0),
            order: 0,
        })
        .collect();
    for (i, &v) in reading.iter().enumerate() {
        pn[v].order = i;
    }
    let edges = d
        .edges
        .iter()
        .map(|e| {
            if e.from == e.to {
                self_loop(&pn[e.from], d.dir == Dir::LR)
            } else {
                straight(&pn, e.from, e.to)
            }
        })
        .collect();
    finish(
        pn,
        edges,
        vec![false; d.edges.len()],
        xs.len().max(1),
        Vec::new(),
    )
}

fn layout(d: &Diagram) -> Placed {
    if d.explicit {
        return layout_explicit(d);
    }
    let n = d.nodes.len();
    let boxes: Vec<Box_> = d.nodes.iter().map(box_of).collect();
    let (dag, loops) = break_cycles(d);
    let lr = d.dir == Dir::LR;
    // extent along the rank axis / the order axis
    let along = |b: &Box_| if lr { b.w } else { b.h };
    let across = |b: &Box_| if lr { b.h } else { b.w };
    let (rank_gap, node_gap) = if lr { (64.0, 20.0) } else { (48.0, 28.0) };

    // ranks: longest path in topological order (smallest row first), then
    // sources pulled up to just before their nearest successor
    let mut preds: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut succs: Vec<Vec<usize>> = vec![Vec::new(); n];
    for &(u, v, _, _) in &dag {
        preds[v].push(u);
        succs[u].push(v);
    }
    let mut indeg: Vec<usize> = preds.iter().map(Vec::len).collect();
    let mut done = vec![false; n];
    let mut topo = Vec::with_capacity(n);
    // the first ready node in row order, each time (n ≤ MAX_NODES)
    while let Some(v) = (0..n).find(|&v| !done[v] && indeg[v] == 0) {
        done[v] = true;
        topo.push(v);
        for &w in &succs[v] {
            indeg[w] -= 1;
        }
    }
    // a pinned stage (`::XAXIS`) wins over the longest path
    let pinned = |v: usize| {
        d.nodes[v]
            .pin
            .0
            .map(|x| x.round().clamp(0.0, 1000.0) as usize)
    };
    let any_pin = (0..n).any(|v| pinned(v).is_some());
    let mut rank = vec![0usize; n];
    for &v in &topo {
        rank[v] =
            pinned(v).unwrap_or_else(|| preds[v].iter().map(|&p| rank[p] + 1).max().unwrap_or(0));
    }
    for &v in topo.iter().rev() {
        if preds[v].is_empty() && pinned(v).is_none() {
            if let Some(m) = succs[v].iter().map(|&s| rank[s]).min() {
                rank[v] = rank[v].max(m.saturating_sub(1));
            }
        }
    }
    // drop empty ranks (pinned stages keep their numbers)
    let mut used = vec![false; rank.iter().max().map_or(1, |m| m + 1)];
    for &r in &rank {
        used[r] = true;
    }
    let mut renumber = vec![0usize; used.len()];
    let mut nl = 0;
    for (r, u) in used.iter().enumerate() {
        renumber[r] = nl;
        nl += usize::from(*u || any_pin);
    }
    for r in &mut rank {
        *r = renumber[*r];
    }
    let nl = nl.max(1);
    // edges the pins make run within or against the stages: drawn straight
    let (dag, free): (Vec<DagEdge>, Vec<DagEdge>) =
        dag.into_iter().partition(|&(u, v, _, _)| rank[v] > rank[u]);

    // vertices: the nodes, then one dummy per layer an edge passes
    let mut vrank: Vec<usize> = rank.clone();
    let mut vsucc: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut vpred: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut chains: Vec<Vec<usize>> = Vec::with_capacity(dag.len());
    for &(u, v, _, _) in &dag {
        let mut chain = vec![u];
        let mut prev = u;
        for r in rank[u] + 1..rank[v] {
            let dmy = vrank.len();
            vrank.push(r);
            vsucc.push(Vec::new());
            vpred.push(Vec::new());
            vsucc[prev].push(dmy);
            vpred[dmy].push(prev);
            chain.push(dmy);
            prev = dmy;
        }
        vsucc[prev].push(v);
        vpred[v].push(prev);
        chain.push(v);
        chains.push(chain);
    }
    let nv = vrank.len();
    let dummy = |v: usize| v >= n;
    // a vertex's swimlane (unlaned nodes last; a dummy runs in its source's)
    let mut vlane: Vec<usize> = (0..n)
        .map(|v| d.nodes[v].lane.unwrap_or(d.lanes.len()))
        .collect();
    vlane.resize(nv, 0);
    for ch in &chains {
        for &dm in &ch[1..ch.len() - 1] {
            vlane[dm] = vlane[ch[0]];
        }
    }
    let laned = !d.lanes.is_empty();

    // initial order: DFS from the vertices by (rank, index)
    let mut layers: Vec<Vec<usize>> = vec![Vec::new(); nl];
    let mut seen = vec![false; nv];
    for s in ordered((0..nv).map(|v| (vrank[v] as f64, 0.0, v)).collect()) {
        if seen[s] {
            continue;
        }
        let mut stack = vec![s];
        while let Some(v) = stack.pop() {
            if seen[v] {
                continue;
            }
            seen[v] = true;
            layers[vrank[v]].push(v);
            for &w in vsucc[v].iter().rev() {
                if !seen[w] {
                    stack.push(w);
                }
            }
        }
    }
    if laned {
        for l in &mut layers {
            *l = ordered(
                l.iter()
                    .enumerate()
                    .map(|(i, &v)| (vlane[v] as f64, i as f64, v))
                    .collect(),
            );
        }
    }
    let mut pos = vec![0usize; nv];
    let index = |layers: &[Vec<usize>], pos: &mut [usize]| {
        for l in layers {
            for (i, &v) in l.iter().enumerate() {
                pos[v] = i;
            }
        }
    };
    index(&layers, &mut pos);
    let crossings = |layers: &[Vec<usize>], pos: &[usize]| -> u64 {
        (0..layers.len().saturating_sub(1))
            .map(|r| layer_crossings(&layers[r], &vsucc, pos, layers[r + 1].len()))
            .sum()
    };

    // crossing reduction: barycenter sweeps + adjacent transposition, best kept
    let mut best = layers.clone();
    let mut best_c = crossings(&layers, &pos);
    for it in 0..8 {
        if best_c == 0 {
            break;
        }
        let down = it % 2 == 0;
        let rs: Vec<usize> = if down {
            (1..nl).collect()
        } else {
            (0..nl.saturating_sub(1)).rev().collect()
        };
        for r in rs {
            let nb = if down { &vpred } else { &vsucc };
            let keyed: Vec<(f64, f64, usize)> = layers[r]
                .iter()
                .map(|&v| {
                    let k = if nb[v].is_empty() {
                        pos[v] as f64
                    } else {
                        nb[v].iter().map(|&w| pos[w] as f64).sum::<f64>() / nb[v].len() as f64
                    };
                    (vlane[v] as f64 * 1e6 + k, pos[v] as f64, v)
                })
                .collect();
            layers[r] = ordered(keyed);
            for (i, &v) in layers[r].iter().enumerate() {
                pos[v] = i;
            }
        }
        // transposition
        for _ in 0..4 {
            let mut better = false;
            for layer in layers.iter_mut() {
                for i in 0..layer.len().saturating_sub(1) {
                    let (a, b) = (layer[i], layer[i + 1]);
                    if vlane[a] != vlane[b] {
                        continue;
                    }
                    let pair = |x: usize, y: usize| -> usize {
                        let c = |l: &[Vec<usize>]| {
                            l[x].iter()
                                .flat_map(|&p| l[y].iter().map(move |&q| (p, q)))
                                .filter(|&(p, q)| pos[p] > pos[q])
                                .count()
                        };
                        c(&vpred) + c(&vsucc)
                    };
                    if pair(b, a) < pair(a, b) {
                        layer.swap(i, i + 1);
                        pos[a] = i + 1;
                        pos[b] = i;
                        better = true;
                    }
                }
            }
            if !better {
                break;
            }
        }
        let c = crossings(&layers, &pos);
        if c < best_c {
            best_c = c;
            best = layers.clone();
        }
    }
    let layers = best;
    index(&layers, &mut pos);

    // positions along each layer
    let size = |v: usize| if dummy(v) { 0.0 } else { across(&boxes[v]) };
    let gap_between = |a: usize, b: usize| -> f64 {
        let g = match (dummy(a), dummy(b)) {
            (true, true) => 8.0,
            (false, false) => node_gap,
            _ => 12.0,
        };
        let lane = if vlane[a] != vlane[b] {
            LANE_GAP + LANE_HEAD
        } else {
            0.0
        };
        size(a) / 2.0 + g + lane + size(b) / 2.0
    };
    let mut y = vec![0.0f64; nv];
    for l in &layers {
        let mut at = 0.0;
        for (i, &v) in l.iter().enumerate() {
            if i > 0 {
                at += gap_between(l[i - 1], v);
            }
            y[v] = at;
        }
        let mid = at / 2.0;
        for &v in l {
            y[v] -= mid;
        }
    }
    let relax = |y: &mut Vec<f64>, r: usize, up: bool, down: bool| {
        let l = &layers[r];
        if l.is_empty() {
            return;
        }
        let want: Vec<f64> = l
            .iter()
            .map(|&v| {
                let mut nb: Vec<usize> = Vec::new();
                if up {
                    nb.extend(&vpred[v]);
                }
                if down {
                    nb.extend(&vsucc[v]);
                }
                if nb.is_empty() {
                    y[v]
                } else {
                    nb.iter().map(|&w| y[w]).sum::<f64>() / nb.len() as f64
                }
            })
            .collect();
        let weight: Vec<f64> = l
            .iter()
            .map(|&v| if dummy(v) { 2.0 } else { 1.0 })
            .collect();
        let gaps: Vec<f64> = (0..l.len())
            .map(|i| {
                if i == 0 {
                    0.0
                } else {
                    gap_between(l[i - 1], l[i])
                }
            })
            .collect();
        for (&v, p) in l.iter().zip(place(&want, &weight, &gaps)) {
            y[v] = p;
        }
    };
    for _ in 0..6 {
        for r in 1..nl {
            relax(&mut y, r, true, false);
        }
        for r in (0..nl.saturating_sub(1)).rev() {
            relax(&mut y, r, false, true);
        }
    }
    for r in 0..nl {
        relax(&mut y, r, true, true);
    }
    // swimlanes: shift each lane clear of the one before (across all stages)
    let mut lane_span: Vec<(f64, f64)> = Vec::new();
    if laned {
        let nlanes = d.lanes.len() + 1;
        let span = |y: &[f64], l: usize| {
            (0..nv)
                .filter(|&v| vlane[v] == l)
                .map(|v| (y[v] - size(v) / 2.0, y[v] + size(v) / 2.0))
                .fold(None, |acc: Option<(f64, f64)>, (a, b)| match acc {
                    None => Some((a, b)),
                    Some((x, z)) => Some((x.min(a), z.max(b))),
                })
        };
        let mut floor: Option<f64> = None;
        for l in 0..nlanes {
            let Some((lo, _)) = span(&y, l) else {
                lane_span.push((f64::NAN, f64::NAN));
                continue;
            };
            if let Some(f) = floor {
                let need = f + LANE_GAP + LANE_HEAD - lo;
                if need > 0.0 {
                    for v in 0..nv {
                        if vlane[v] == l {
                            y[v] += need;
                        }
                    }
                }
            }
            let (lo, hi) = span(&y, l).expect("lane has vertices");
            lane_span.push((lo, hi));
            floor = Some(hi);
        }
    }

    // positions along the rank axis: each layer as thick as its widest node
    let mut thick = vec![0.0f64; nl];
    for v in 0..n {
        thick[rank[v]] = thick[rank[v]].max(along(&boxes[v]));
    }
    let mut start_at = vec![0.0f64; nl];
    for r in 1..nl {
        start_at[r] = start_at[r - 1] + thick[r - 1] + rank_gap;
    }
    let centre = |r: usize| start_at[r] + thick[r] / 2.0;
    let xy = |rk: f64, o: f64| if lr { (rk, o) } else { (o, rk) };

    let mut pn: Vec<PNode> = Vec::with_capacity(n);
    for (v, b) in boxes.into_iter().enumerate() {
        let (cx, cy) = xy(centre(rank[v]), y[v]);
        pn.push(PNode {
            cx,
            cy,
            w: b.w,
            h: b.h,
            lines: b.lines,
            layer: rank[v],
            order: 0,
        });
    }
    // reading order: layer, then position along it
    let reading = ordered((0..n).map(|v| (rank[v] as f64, y[v], v)).collect());
    for (i, &v) in reading.iter().enumerate() {
        pn[v].order = i;
    }

    // ports: a node's outgoing (incoming) chains spread along its out (in)
    // side, sorted by where they go (come from)
    let half_along = |v: usize| if lr { pn[v].w / 2.0 } else { pn[v].h / 2.0 };
    let side = |v: usize| if lr { pn[v].h } else { pn[v].w };
    let mut out_ports: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut in_ports: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (c, ch) in chains.iter().enumerate() {
        out_ports[ch[0]].push(c);
        in_ports[ch[ch.len() - 1]].push(c);
    }
    for ports in &mut out_ports {
        *ports = ordered(ports.iter().map(|&c| (y[chains[c][1]], 0.0, c)).collect());
    }
    for ports in &mut in_ports {
        *ports = ordered(
            ports
                .iter()
                .map(|&c| (y[chains[c][chains[c].len() - 2]], 0.0, c))
                .collect(),
        );
    }
    let spread = |list: &[usize], c: usize, v: usize| -> f64 {
        let m = list.len();
        if m <= 1 {
            return 0.0;
        }
        let k = list.iter().position(|&x| x == c).unwrap_or(0);
        let span = (side(v) * 0.6).min(14.0 * (m - 1) as f64);
        -span / 2.0 + span * k as f64 / (m - 1) as f64
    };
    let mut edge_pts: Vec<Vec<(f64, f64)>> = vec![Vec::new(); d.edges.len()];
    let mut back = vec![false; d.edges.len()];
    for (c, ch) in chains.iter().enumerate() {
        let (u, v, k, rev) = dag[c];
        let mut pts = Vec::with_capacity(ch.len() * 2);
        pts.push(xy(
            centre(rank[u]) + half_along(u),
            y[u] + spread(&out_ports[u], c, u),
        ));
        for &dm in &ch[1..ch.len() - 1] {
            let r = vrank[dm];
            pts.push(xy(start_at[r], y[dm]));
            pts.push(xy(start_at[r] + thick[r], y[dm]));
        }
        pts.push(xy(
            centre(rank[v]) - half_along(v),
            y[v] + spread(&in_ports[v], c, v),
        ));
        if rev {
            pts.reverse();
        }
        edge_pts[k] = pts;
        back[k] = rev;
    }
    for k in loops {
        edge_pts[k] = self_loop(&pn[d.edges[k].from], lr);
    }
    for &(u, v, k, rev) in &free {
        let mut pts = straight(&pn, u, v);
        if rev {
            pts.reverse();
        }
        edge_pts[k] = pts;
        back[k] = rev;
    }
    // swimlane bands across every stage: the lane's span along the stages'
    // axis, its label strip before it
    let total = start_at[nl - 1] + thick[nl - 1];
    let bands = lane_span
        .iter()
        .enumerate()
        .filter(|(l, (lo, _))| *l < d.lanes.len() && lo.is_finite())
        .map(|(l, &(lo, hi))| {
            // the label strip: before the lane (LR), before the stages (TB)
            let (a, b) = if lr {
                xy(-12.0, lo - LANE_HEAD - 6.0)
            } else {
                xy(-LANE_HEAD - 10.0, lo - 8.0)
            };
            let (c, e) = xy(total + 12.0, hi + 8.0);
            (l, a, b, c, e)
        })
        .collect();
    finish(pn, edge_pts, back, nl, bands)
}

/// Move a layout to the margin and round it to half pixels.
fn finish(
    mut pn: Vec<PNode>,
    mut edge_pts: Vec<Vec<(f64, f64)>>,
    back: Vec<bool>,
    layers: usize,
    mut bands: Vec<(usize, f64, f64, f64, f64)>,
) -> Placed {
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for p in &pn {
        x0 = x0.min(p.cx - p.w / 2.0);
        x1 = x1.max(p.cx + p.w / 2.0);
        y0 = y0.min(p.cy - p.h / 2.0);
        y1 = y1.max(p.cy + p.h / 2.0);
    }
    for &(px, py) in edge_pts.iter().flatten() {
        x0 = x0.min(px);
        x1 = x1.max(px);
        y0 = y0.min(py);
        y1 = y1.max(py);
    }
    for &(_, a, b, c, e) in &bands {
        x0 = x0.min(a.min(c));
        x1 = x1.max(a.max(c));
        y0 = y0.min(b.min(e));
        y1 = y1.max(b.max(e));
    }
    if pn.is_empty() {
        (x0, y0, x1, y1) = (0.0, 0.0, 0.0, 0.0);
    }
    let r = |v: f64| (v * 2.0).round() / 2.0;
    let (dx, dy) = (MARGIN - x0, MARGIN - y0);
    for p in &mut pn {
        p.cx = r(p.cx + dx);
        p.cy = r(p.cy + dy);
    }
    for pts in &mut edge_pts {
        for pt in pts.iter_mut() {
            *pt = (r(pt.0 + dx), r(pt.1 + dy));
        }
    }
    for b in &mut bands {
        *b = (b.0, r(b.1 + dx), r(b.2 + dy), r(b.3 + dx), r(b.4 + dy));
    }
    Placed {
        w: r(x1 - x0 + 2.0 * MARGIN),
        h: r(y1 - y0 + 2.0 * MARGIN),
        nodes: pn,
        edges: edge_pts,
        back,
        layers,
        bands,
    }
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
// The layout above is ours (graph theory); the drawing is ggplot-rs's
// (#318 step 5, #421): one `GGPlot` over the layout's own pixel space (x
// right, y down, 1 data unit = 1 px, no margins), with a layer per mark
// kind. The mark contract web/anim.js and web/app.js read stays as before:
// `rect[data-node]` boxes and `polyline[data-edge]` edges with a `<title>`,
// arrowheads (`polygon[data-from]`) and edge labels carrying
// `data-from`/`data-to`, and a node's contents (accent, label, value,
// badge, sparkline) inside one `<g data-of="<id>">`, inert
// (`pointer-events="none"`, `aria-hidden="true"`) so hover, click and
// screen readers reach the box below.

/// A column-major frame for one layer; every row pushes every column.
#[derive(Default)]
struct Frame(Vec<(String, Vec<Value>)>);

impl Frame {
    fn col(&mut self, name: &str) -> &mut Vec<Value> {
        let i = match self.0.iter().position(|(n, _)| n == name) {
            Some(i) => i,
            None => {
                self.0.push((name.to_string(), Vec::new()));
                self.0.len() - 1
            }
        };
        &mut self.0[i].1
    }
    fn num(&mut self, name: &str, v: f64) -> &mut Self {
        self.col(name).push(Value::Float(v));
        self
    }
    fn txt(&mut self, name: &str, v: impl Into<String>) -> &mut Self {
        self.col(name).push(Value::Str(v.into()));
        self
    }
    fn opt(&mut self, name: &str, v: Option<String>) -> &mut Self {
        self.col(name).push(v.map_or(Value::Na, Value::Str));
        self
    }
    fn is_empty(&self) -> bool {
        self.0.first().is_none_or(|(_, v)| v.is_empty())
    }
    /// The frame's columns, its pixel y (down) flipped to ggplot-rs's data
    /// y (up) on a canvas `h` tall: `y` → `h - y`, and a rect's `y0`/`y1`
    /// swapped as they flip so `y0` stays the low edge.
    fn take(&mut self, h: f64) -> Vec<(String, Vec<Value>)> {
        let flip = |v: Vec<Value>| -> Vec<Value> {
            v.into_iter()
                .map(|x| match x {
                    Value::Float(y) => Value::Float(h - y),
                    other => other,
                })
                .collect()
        };
        std::mem::take(&mut self.0)
            .into_iter()
            .map(|(n, v)| match n.as_str() {
                "y" => (n, flip(v)),
                "y0" => ("y1".to_string(), flip(v)),
                "y1" => ("y0".to_string(), flip(v)),
                _ => (n, v),
            })
            .collect()
    }
}

/// Frames keyed by a colour a geom only takes as a fixed parameter (a
/// rect's stroke), in first-seen order: one layer per colour.
fn by_colour(v: &mut Vec<(Rgb, Frame)>, c: Rgb) -> &mut Frame {
    let i = match v.iter().position(|(k, _)| *k == c) {
        Some(i) => i,
        None => {
            v.push((c, Frame::default()));
            v.len() - 1
        }
    };
    &mut v[i].1
}

/// The point halfway along a polyline.
fn midpoint(pts: &[(f64, f64)]) -> (f64, f64) {
    let seg = |i: usize| {
        let (a, b) = (pts[i], pts[i + 1]);
        ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt()
    };
    let total: f64 = (0..pts.len().saturating_sub(1)).map(seg).sum();
    let mut left = total / 2.0;
    for i in 0..pts.len().saturating_sub(1) {
        let l = seg(i);
        if l >= left && l > 0.0 {
            let t = left / l;
            let (a, b) = (pts[i], pts[i + 1]);
            return (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t);
        }
        left -= l;
    }
    pts.first().copied().unwrap_or((0.0, 0.0))
}

/// ggplot-rs centres text on its y (`dominant-baseline="middle"`); the
/// layout places lines by their baseline. A cap-height-ish shift between
/// the two.
fn mid_of_baseline(baseline: f64, px: f64) -> f64 {
    baseline - 0.36 * px
}

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

/// The drawing as a nested `<svg>` of `pw` × `ph` (the layout's own size).
fn draw(d: &Diagram, p: &Placed, pw: u32, ph: u32, o: &RenderOptions) -> String {
    use ggplot_rs::geom::arrow::arrow;
    use ggplot_rs::prelude::*;
    use ggplot_rs::render::backend::{LineEnd, Linetype};
    use ggplot_rs::scale::linewidth::ScaleLinewidthContinuous;

    let t = &o.theme;
    let (muted, card, ink) = (t.colors.muted, t.colors.card, t.colors.ink);
    let (w, h) = (pw as f64, ph as f64);
    let mut th = crate::void_theme(o);
    th.legend_position = ggplot_rs::theme::LegendPosition::None;
    let mut plot = GGPlot::new(vec![
        ("x".to_string(), vec![Value::Float(0.0), Value::Float(w)]),
        ("y".to_string(), vec![Value::Float(0.0), Value::Float(h)]),
    ])
    .theme(th)
    // the canvas is the view (a zoom, so a mark reaching past it is clipped
    // rather than dropped); `Frame::take` flips the layout's y-down pixels
    .scale_x_continuous(ScaleContinuous::new().with_expand(0.0, 0.0))
    .scale_y_continuous(ScaleContinuous::new().with_expand(0.0, 0.0))
    .coord_cartesian_zoom(Some((0.0, w)), Some((0.0, h)))
    .scale_color_identity()
    .scale_fill_identity()
    .scale_linewidth(
        ScaleLinewidthContinuous::new()
            .with_limits(0.0, 16.0)
            .with_range(0.0, 16.0),
    )
    .scale_linetype_manual(vec![
        ("solid", Linetype::Solid),
        ("dashed", Linetype::Custom(vec![(6.0, 4.0)])),
        // round caps on a zero-length dash: moving dots
        ("dots", Linetype::Custom(vec![(0.0, 10.0)])),
    ]);
    let rect_aes = || Aes::new().xmin("x0").xmax("x1").ymin("y0").ymax("y1");
    let inert = [("pointer-events", "none")];

    // swimlanes (under everything): a tinted band per lane, its name on top
    let (mut even, mut odd, mut names) = (Frame::default(), Frame::default(), Frame::default());
    for &(l, x0, y0, x1, y1) in &p.bands {
        let f = if l % 2 == 0 { &mut even } else { &mut odd };
        f.num("x0", x0).num("x1", x1).num("y0", y0).num("y1", y1);
        f.txt("lane", d.lanes[l].clone());
        names
            .num("x", x0 + 8.0)
            .num("y", mid_of_baseline(y0 + 13.0, 11.0));
        names.txt("label", d.lanes[l].clone());
    }
    for (mut f, alpha) in [(even, 0.35), (odd, 0.18)] {
        if f.is_empty() {
            continue;
        }
        plot = plot
            .geom_rect_with(
                GeomRect {
                    fill: t.colors.line,
                    alpha,
                    line_width: 0.0,
                    ..Default::default()
                }
                .with_radius(6.0),
            )
            .layer_data(f.take(h))
            .layer_aes(rect_aes().data_attr("lane", "lane"))
            .layer_svg_attrs([("aria-hidden", "true"), ("class", "dp-lane")]);
    }
    if !names.is_empty() {
        plot = plot
            .geom_text_with(GeomText {
                size: 11.0,
                color: muted,
                hjust: 0.0,
                // #228: lane names (dp-lane-label) render bold, matching
                // web/theme-base.css for the same reason as dp-node-label
                // below.
                fontface: FontFace::Bold,
                ..Default::default()
            })
            .layer_data(names.take(h))
            .layer_aes(Aes::new().x("x").y("y").label("label"))
            .layer_svg_attrs([("aria-hidden", "true"), ("class", "dp-lane-label")]);
    }

    // edges (under the nodes): one path per edge, as wide as what it
    // carries (area-true: sqrt), its style a linetype
    let vmax = d.edges.iter().filter_map(|e| e.value).fold(0.0, f64::max);
    let (mut ef, mut lf) = (Frame::default(), Frame::default());
    for (k, e) in d.edges.iter().enumerate() {
        let width = match e.value {
            Some(v) if vmax > 0.0 => 1.4 + 5.6 * (v.max(0.0) / vmax).sqrt(),
            _ => 1.6,
        };
        let width = match e.style {
            EdgeStyle::Dots(_) => width.clamp(3.0, 6.0),
            _ => width,
        };
        let width = (width * 10.0).round() / 10.0;
        let mut pts = p.edges[k].clone();
        if pts.len() < 2 {
            continue;
        }
        // ggplot-rs puts the arrow's tip on the path's last point and
        // strokes the head as wide as the line (its mitred tip reaching
        // about one line width further): end the path one width short of
        // the box so the tip lands on its edge.
        let n = pts.len();
        let ((fx, fy), (tx, ty)) = (pts[n - 2], pts[n - 1]);
        let len = ((tx - fx).powi(2) + (ty - fy).powi(2)).sqrt().max(1e-6);
        let pull = width.min(len / 2.0);
        pts[n - 1] = (tx - (tx - fx) / len * pull, ty - (ty - fy) / len * pull);
        // health first (warn / bad), else a flowing edge in the brand colour
        let colour = match (e.status, e.style) {
            (Some(st @ (Status::Bad | Status::Warn)), _) => st.color(t),
            (_, EdgeStyle::Flow(_) | EdgeStyle::Dots(_)) => t.color(0),
            _ => muted,
        };
        let (lt, class) = match e.style {
            EdgeStyle::Solid => ("solid", None),
            EdgeStyle::Dashed => ("dashed", None),
            EdgeStyle::Flow(_) => ("dashed", Some("dp-flow")),
            EdgeStyle::Dots(_) => ("dots", Some("dp-flow dp-dots")),
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
        let tip = match (&e.label, &e.fvalue) {
            (Some(l), _) => format!("{name}: {l}"),
            (None, Some(v)) => format!("{name}: {v}"),
            _ => name.clone(),
        };
        for &(x, y) in &pts {
            ef.num("x", x).num("y", y).txt("g", format!("e{k:04}"));
            ef.txt("col", hex(colour)).num("lw", width).txt("lt", lt);
            ef.opt("cls", class.clone()).txt("tip", tip.clone());
            ef.txt("edge", format!("{}>{}", a.id, b.id));
            ef.txt("from", a.id.clone()).txt("to", b.id.clone());
            ef.txt("fx", name.clone());
            ef.opt("value", e.value.map(|v| v.to_string()));
            ef.opt(
                "fvalue",
                e.value.map(|_| e.fvalue.clone().unwrap_or_default()),
            );
            ef.opt("status", e.status.map(|s| s.as_str().to_string()));
            ef.opt("back", p.back[k].then(|| "1".to_string()));
        }
        if let Some(l) = &e.label {
            let (mx, my) = midpoint(&p.edges[k]);
            // #519: ggplot-rs 0.31.0's geom_text honours an embedded "\n"
            // natively (sipemu/ggplot-rs#249) — one halo-backed text row for
            // the whole label, centred around the midpoint, instead of
            // #229's hand-stacked row per line. Its block centring (size
            // 11, line_height 1.2) lands each line exactly where the old
            // per-line loop put it: both put line i's drawn y at
            // `my - 7.96 + i * 13.2 - (n - 1) * 6.6`, so this is a
            // byte-identical render, for any number of lines.
            lf.num("x", mx).num("y", mid_of_baseline(my - 4.0, 11.0));
            lf.txt("label", l.clone());
            lf.txt("from", a.id.clone()).txt("to", b.id.clone());
        }
    }
    if !ef.is_empty() {
        plot = plot
            .geom_path_with(
                GeomPath::default()
                    .lineend(LineEnd::Round)
                    // the old 7 px + width head, ~29° half-angle
                    .arrow(
                        arrow()
                            .closed()
                            .angle(29.0)
                            .length(ARROW + 1.0)
                            .length_per_linewidth(1.2),
                    ),
            )
            .layer_data(ef.take(h))
            .layer_aes(
                Aes::new()
                    .x("x")
                    .y("y")
                    .group("g")
                    .color("col")
                    .linewidth("lw")
                    .linetype("lt")
                    .class("cls")
                    .tooltip("tip")
                    .data_attr("edge", "edge")
                    .data_attr("from", "from")
                    .data_attr("to", "to")
                    .data_attr("xtext", "fx")
                    .data_attr("value", "value")
                    .data_attr("vtext", "fvalue")
                    .data_attr("status", "status")
                    .data_attr("back", "back"),
            )
            .layer_svg_attr("class", "dp-edge");
    }
    if !lf.is_empty() {
        // #229: text directly on the line with a card-coloured halo keeps
        // it legible over the line, matching the old hand-written SVG's
        // paint-order:stroke halo (replaces the boxed chip the GeomLabel
        // port used before ggplot-rs grew a halo option).
        plot = plot
            .geom_text_with(
                GeomText {
                    size: 11.0,
                    color: muted,
                    ..Default::default()
                }
                .bg_colour(card)
                .bg_r(2.0),
            )
            .layer_data(lf.take(h))
            .layer_aes(
                Aes::new()
                    .x("x")
                    .y("y")
                    .label("label")
                    .data_attr("from", "from")
                    .data_attr("to", "to"),
            )
            .layer_svg_attrs(inert)
            .layer_svg_attrs([("aria-hidden", "true"), ("class", "dp-edge-label")]);
    }

    // nodes: the box (the hit target, outside any `data-of` group), then
    // what it shows, grouped per node
    let mut boxes: Vec<(Rgb, Frame)> = Vec::new();
    let mut badges: Vec<(Rgb, Frame)> = Vec::new();
    let (mut accent, mut lines, mut values) =
        (Frame::default(), Frame::default(), Frame::default());
    let (mut dots, mut btext, mut spark, mut ends) = (
        Frame::default(),
        Frame::default(),
        Frame::default(),
        Frame::default(),
    );
    for v in reading(p) {
        let nd = &d.nodes[v];
        let pn = &p.nodes[v];
        let id = nd.id.clone();
        let (x, y) = (pn.cx - pn.w / 2.0, pn.cy - pn.h / 2.0);
        let stroke = nd
            .status
            .filter(|s| *s == Status::Bad || *s == Status::Warn)
            .map(|s| s.color(t))
            .unwrap_or(t.chart.line);
        let mut tip = nd.label.clone();
        if let Some(fv) = &nd.fvalue {
            tip.push_str(&format!(": {fv}"));
        }
        if let Some(b) = &nd.badge {
            tip.push_str(&format!(" ({b})"));
        }
        let f = by_colour(&mut boxes, stroke);
        f.num("x0", x)
            .num("x1", x + pn.w)
            .num("y0", y)
            .num("y1", y + pn.h);
        f.txt("node", id.clone())
            .txt("xk", id.clone())
            .txt("series", id.clone());
        f.txt("fx", nd.label.clone());
        f.txt("layer", pn.layer.to_string())
            .txt("order", pn.order.to_string());
        f.opt("value", nd.value.map(|v| v.to_string()));
        f.opt(
            "fvalue",
            nd.value.map(|_| nd.fvalue.clone().unwrap_or_default()),
        );
        f.opt("status", nd.status.map(|s| s.as_str().to_string()));
        f.opt("badge", nd.badge.clone()).txt("tip", tip);

        let accent_c = nd
            .status
            .filter(|s| *s != Status::Neutral)
            .map(|s| s.color(t))
            .or_else(|| nd.category.map(|c| t.color(c)));
        if let Some(a) = accent_c {
            accent.num("x0", x + 1.5).num("x1", x + 4.5);
            accent
                .num("y0", y + 6.0)
                .num("y1", y + 6.0 + (pn.h - 12.0).max(4.0));
            accent.txt("fill", hex(a)).txt("of", id.clone());
        }
        let mut base = y + BOX_PAD_Y + 12.0;
        for line in &pn.lines {
            lines
                .num("x", pn.cx)
                .num("y", mid_of_baseline(base, LABEL_PX));
            lines.txt("label", line.clone()).txt("of", id.clone());
            base += LINE_H;
        }
        if let Some(fv) = &nd.fvalue {
            values
                .num("x", pn.cx)
                .num("y", mid_of_baseline(base, VALUE_PX));
            values.txt("label", fv.clone()).txt("of", id.clone());
            base += LINE_H;
        }
        if let Some(b) = &nd.badge {
            let bw = text_w(b, BADGE_PX, false) + 20.0;
            let dot = nd.status.map(|s| s.color(t)).unwrap_or(muted);
            let (bx, by) = (pn.cx - bw / 2.0, base - 9.0);
            let f = by_colour(&mut badges, dot);
            f.num("x0", bx)
                .num("x1", bx + bw)
                .num("y0", by)
                .num("y1", by + 16.0);
            f.txt("of", id.clone());
            dots.num("x", bx + 9.0).num("y", by + 8.0);
            dots.txt("col", hex(dot)).txt("of", id.clone());
            btext.num("x", bx + 15.0).num("y", by + 8.0);
            btext.txt("label", b.clone()).txt("of", id.clone());
            base += 20.0;
        }
        // a mini trend along the bottom of the box
        if nd.spark.len() >= 2 {
            let (lo, hi) = nd
                .spark
                .iter()
                .fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), &v| {
                    (a.min(v), b.max(v))
                });
            let rng = if hi > lo { hi - lo } else { 1.0 };
            let (sx0, sw) = (x + BOX_PAD_X, pn.w - 2.0 * BOX_PAD_X);
            let (top, sh) = (base - 8.0, SPARK_H - 8.0);
            let last = nd.spark.len() - 1;
            let col = hex(nd
                .status
                .filter(|s| *s == Status::Bad || *s == Status::Warn)
                .map(|s| s.color(t))
                .unwrap_or_else(|| t.color(0)));
            for (i, &sv) in nd.spark.iter().enumerate() {
                let (px, py) = (
                    sx0 + sw * i as f64 / last as f64,
                    top + sh - (sv - lo) / rng * sh,
                );
                spark.num("x", px).num("y", py).txt("g", id.clone());
                spark.txt("col", col.clone()).txt("of", id.clone());
                if i == last {
                    ends.num("x", px).num("y", py);
                    ends.txt("col", col.clone()).txt("of", id.clone());
                }
            }
        }
    }
    for (stroke, mut f) in boxes {
        plot = plot
            .geom_rect_with(
                GeomRect {
                    fill: card,
                    color: stroke,
                    alpha: 1.0,
                    line_width: 1.2,
                    ..Default::default()
                }
                .with_radius(8.0),
            )
            .layer_data(f.take(h))
            .layer_aes(
                rect_aes()
                    .tooltip("tip")
                    .data_attr("node", "node")
                    .data_attr("x", "xk")
                    .data_attr("series", "series")
                    .data_attr("xtext", "fx")
                    .data_attr("layer", "layer")
                    .data_attr("order", "order")
                    .data_attr("value", "value")
                    .data_attr("vtext", "fvalue")
                    .data_attr("status", "status")
                    .data_attr("badge", "badge"),
            )
            .layer_svg_attr("class", "dp-node");
    }
    // everything below: dims / lights with its node (`data-of`), inert
    let of = |a: Aes| a.group_wrap("of", "of");
    if !accent.is_empty() {
        plot = plot
            .geom_rect_with(
                GeomRect {
                    alpha: 1.0,
                    line_width: 0.0,
                    ..Default::default()
                }
                .with_radius(1.5),
            )
            .layer_data(accent.take(h))
            .layer_aes(of(rect_aes().fill("fill")))
            .decorative()
            .layer_svg_attrs(inert);
    }
    for (mut f, size, colour, class) in [
        (lines, LABEL_PX, t.colors.heading, "dp-node-label"),
        (values, VALUE_PX, ink, "dp-node-value"),
    ] {
        if f.is_empty() {
            continue;
        }
        plot = plot
            .geom_text_with(GeomText {
                size,
                color: colour,
                // #228: node names (dp-node-label) and values (dp-node-value)
                // render bold — matches web/theme-base.css's font-weight on
                // those classes, which the browser applies via CSS but a
                // headless/static export needs baked into the SVG itself.
                fontface: FontFace::Bold,
                ..Default::default()
            })
            .layer_data(f.take(h))
            .layer_aes(of(Aes::new().x("x").y("y").label("label")))
            .layer_svg_attrs(inert)
            .layer_svg_attrs([("aria-hidden", "true"), ("class", class)]);
    }
    for (dot, mut f) in badges {
        plot = plot
            .geom_rect_with(
                GeomRect {
                    fill: card,
                    color: dot,
                    alpha: 0.0,
                    line_width: 1.0,
                    ..Default::default()
                }
                .with_radius(8.0),
            )
            .layer_data(f.take(h))
            .layer_aes(of(rect_aes()))
            .decorative()
            .layer_svg_attrs(inert);
    }
    for (mut f, size) in [(dots, 3.0), (ends, 2.2)] {
        if f.is_empty() {
            continue;
        }
        plot = plot
            .geom_point_with(GeomPoint {
                size,
                ..Default::default()
            })
            .layer_data(f.take(h))
            .layer_aes(of(Aes::new().x("x").y("y").color("col")))
            .decorative()
            .layer_svg_attrs(inert);
    }
    if !btext.is_empty() {
        plot = plot
            .geom_text_with(GeomText {
                size: BADGE_PX,
                color: ink,
                hjust: 0.0,
                ..Default::default()
            })
            .layer_data(btext.take(h))
            .layer_aes(of(Aes::new().x("x").y("y").label("label")))
            .layer_svg_attrs(inert)
            .layer_svg_attrs([("aria-hidden", "true"), ("class", "dp-node-badge")]);
    }
    if !spark.is_empty() {
        plot = plot
            .geom_path_with(GeomPath::default().linewidth(1.4))
            .layer_data(spark.take(h))
            .layer_aes(of(Aes::new().x("x").y("y").group("g").color("col")))
            .decorative()
            .layer_svg_attrs(inert)
            .layer_svg_attr("class", "dp-node-spark");
    }
    plot.render_svg_native_at(0.0, 0.0, pw, ph)
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

    #[test]
    fn wraps_long_labels_in_two_lines() {
        let l = wrap(
            "Recommend replenishment orders for every store and every week",
            152.0,
        );
        assert_eq!(l.len(), 2);
        assert!(l[1].ends_with('…'));
        assert_eq!(wrap("Ingest", 152.0), ["Ingest"]);
    }

    #[test]
    fn isotonic_placement_keeps_order_and_gaps() {
        let y = place(&[10.0, 0.0, 5.0], &[1.0, 1.0, 1.0], &[0.0, 10.0, 10.0]);
        assert!(y[1] - y[0] >= 10.0 - 1e-9 && y[2] - y[1] >= 10.0 - 1e-9);
        let y = place(&[0.0, 50.0], &[1.0, 1.0], &[0.0, 10.0]);
        assert_eq!(y, [0.0, 50.0]);
    }
}
