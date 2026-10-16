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
pub const EDGE_STYLES: &[&str] = &["solid", "dashed", "flow"];

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
    /// static output.
    Flow,
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
}

#[derive(Clone, Debug)]
pub struct Edge {
    pub from: usize,
    pub to: usize,
    pub value: Option<f64>,
    pub fvalue: Option<String>,
    pub label: Option<String>,
    pub style: EdgeStyle,
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
    /// Nodes / edges in the data before the [`MAX_NODES`] / [`MAX_EDGES`] cut.
    pub total_nodes: usize,
    pub total_edges: usize,
    pub problems: Vec<Problem>,
}

/// A cell as an id: text, or a number written without a trailing `.0`.
fn id_of(v: &Value) -> Option<String> {
    key_text(v)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn json_id(v: &J) -> Option<String> {
    match v {
        J::String(s) => Some(s.trim().to_string()).filter(|s| !s.is_empty()),
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
        Value::Str(s) => Some(s.trim().to_string()),
        v => key_text(v),
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
            .map(|p| p.trim().trim_matches(|c| c == '\'' || c == '"').to_string())
            .filter(|p| !p.is_empty())
            .collect();
    }
    Some(t.to_string())
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
        let category = col(Role::Category);
        let upstream = col(Role::Upstream);
        let edges_col = col(Role::Edges);
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
            let status = value
                .and_then(|v| rules.as_ref().and_then(|r| r.status(v)))
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
                            let style = match field(o, &["style"]).and_then(|v| v.as_str()) {
                                None => EdgeStyle::Solid,
                                Some(s) => match s.trim().to_ascii_lowercase().as_str() {
                                    "" | "solid" => EdgeStyle::Solid,
                                    "dashed" => EdgeStyle::Dashed,
                                    "flow" => EdgeStyle::Flow,
                                    other => {
                                        if !bad_styles.iter().any(|b| b == other) {
                                            bad_styles.push(other.to_string());
                                        }
                                        EdgeStyle::Solid
                                    }
                                },
                            };
                            raw.push((from, to, value.filter(|v| v.is_finite()), label, style));
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
                    raw.push((from, nodes[n].id.clone(), None, None, EdgeStyle::Solid));
                }
            }
        }

        let mut edges: Vec<Edge> = Vec::new();
        let mut unknown: Vec<String> = Vec::new();
        let mut total_edges = 0usize;
        for (from, to, value, label, style) in raw {
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
            match edges.iter_mut().find(|e| e.from == f && e.to == t) {
                Some(e) => {
                    e.value = e.value.or(value);
                    e.label = e.label.take().or(label);
                    if e.style == EdgeStyle::Solid {
                        e.style = style;
                    }
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
                            style,
                        });
                    }
                }
            }
        }
        for e in &mut edges {
            e.fvalue = e.value.map(|v| format_number_l(v, fmt, locale));
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
        Some(Diagram {
            nodes,
            edges,
            dir,
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
fn badge_status(text: &str) -> Option<Status> {
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
        "warn", "warning", "risk", "atrisk", "pending", "review", "amber", "medium", "hold",
        "watch", "flat", "stable", "knapp", "prüfen", "offen",
    ];
    const BAD: &[&str] = &[
        "bad",
        "fail",
        "failed",
        "late",
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
    Box_ {
        w: (w + 2.0 * BOX_PAD_X).clamp(BOX_MIN_W, BOX_MAX_W),
        h,
        lines,
    }
}

/// A DAG edge: `(u, v, edge index, reversed)`.
type DagEdge = (usize, usize, usize, bool);
/// An edge as read: `(from id, to id, value, label, style)`.
type RawEdge = (String, String, Option<f64>, Option<String>, EdgeStyle);

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

fn layout(d: &Diagram) -> Placed {
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
    let mut rank = vec![0usize; n];
    for &v in &topo {
        rank[v] = preds[v].iter().map(|&p| rank[p] + 1).max().unwrap_or(0);
    }
    for &v in topo.iter().rev() {
        if preds[v].is_empty() {
            if let Some(m) = succs[v].iter().map(|&s| rank[s]).min() {
                rank[v] = rank[v].max(m.saturating_sub(1));
            }
        }
    }
    // drop empty ranks
    let mut used = vec![false; rank.iter().max().map_or(1, |m| m + 1)];
    for &r in &rank {
        used[r] = true;
    }
    let mut renumber = vec![0usize; used.len()];
    let mut nl = 0;
    for (r, u) in used.iter().enumerate() {
        renumber[r] = nl;
        nl += usize::from(*u);
    }
    for r in &mut rank {
        *r = renumber[*r];
    }
    let nl = nl.max(1);

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
                    (k, pos[v] as f64, v)
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
        size(a) / 2.0 + g + size(b) / 2.0
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
        let v = d.edges[k].from;
        let p = &pn[v];
        edge_pts[k] = if lr {
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
        };
    }

    // translate to the margin
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
    Placed {
        w: r(x1 - x0 + 2.0 * MARGIN),
        h: r(y1 - y0 + 2.0 * MARGIN),
        nodes: pn,
        edges: edge_pts,
        back,
        layers: nl,
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

// ---- SVG ---------------------------------------------------------------------

fn f1(v: f64) -> String {
    let s = format!("{v:.1}");
    s.strip_suffix(".0").map(str::to_string).unwrap_or(s)
}

fn pts_attr(pts: &[(f64, f64)]) -> String {
    pts.iter()
        .map(|(x, y)| format!("{},{}", f1(*x), f1(*y)))
        .collect::<Vec<_>>()
        .join(" ")
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
    let (aw, ah) = ((w - 4.0).max(10.0), (h - 4.0 - note_h).max(10.0));
    let s = (aw / p.w).min(ah / p.h).min(1.0);
    let s = (s * 1000.0).floor() / 1000.0;
    let tx = ((w - p.w * s) / 2.0).max(0.0);
    let ty = ((h - note_h - p.h * s) / 2.0).max(0.0);

    let mut out = svg_open(place, width, height);
    out.push_str(&format!(
        "<desc>{}</desc>",
        esc(&format!("Diagram: {summary}. Steps: {}", steps_text(d, &p)))
    ));
    out.push_str(&format!(
        "<g class=\"dp-diagram\" transform=\"translate({},{}) scale({})\" font-family=\"{font}\">",
        f1(tx),
        f1(ty),
        s
    ));

    // edges (under the nodes)
    let muted = hex(t.colors.muted);
    let card = hex(t.colors.card);
    out.push_str("<g class=\"dp-edges\">");
    for (k, e) in d.edges.iter().enumerate() {
        let mut pts = p.edges[k].clone();
        if pts.len() < 2 {
            continue;
        }
        let (fx, fy) = pts[pts.len() - 2];
        let (tx_, ty_) = pts[pts.len() - 1];
        let len = ((tx_ - fx).powi(2) + (ty_ - fy).powi(2)).sqrt().max(1e-6);
        let (ux, uy) = ((tx_ - fx) / len, (ty_ - fy) / len);
        let base = (tx_ - ux * ARROW, ty_ - uy * ARROW);
        let last = pts.len() - 1;
        pts[last] = base;
        let colour = match e.style {
            EdgeStyle::Flow => hex(t.color(0)),
            _ => muted.clone(),
        };
        let dash = match e.style {
            EdgeStyle::Solid => "",
            EdgeStyle::Dashed => " stroke-dasharray=\"6 4\"",
            EdgeStyle::Flow => " stroke-dasharray=\"6 4\" class=\"dp-flow\"",
        };
        let (a, b) = (&d.nodes[e.from], &d.nodes[e.to]);
        let name = format!("{} → {}", a.label, b.label);
        let tip = match (&e.label, &e.fvalue) {
            (Some(l), _) => format!("{name}: {l}"),
            (None, Some(v)) => format!("{name}: {v}"),
            _ => name.clone(),
        };
        let value = e
            .value
            .map(|v| {
                format!(
                    " data-value=\"{v}\" data-fvalue=\"{}\"",
                    esc(e.fvalue.as_deref().unwrap_or(""))
                )
            })
            .unwrap_or_default();
        out.push_str(&format!(
            "<polyline points=\"{}\" fill=\"none\" stroke=\"{colour}\" stroke-width=\"1.6\" stroke-linejoin=\"round\" stroke-linecap=\"round\"{dash} data-edge=\"{}&gt;{}\" data-from=\"{}\" data-to=\"{}\" data-fx=\"{}\"{value}{}><title>{}</title></polyline>",
            pts_attr(&pts),
            esc(&a.id),
            esc(&b.id),
            esc(&a.id),
            esc(&b.id),
            esc(&name),
            if p.back[k] { " data-back=\"1\"" } else { "" },
            esc(&tip)
        ));
        let (px, py) = (-uy, ux);
        out.push_str(&format!(
            "<polygon points=\"{},{} {},{} {},{}\" fill=\"{colour}\" aria-hidden=\"true\"/>",
            f1(tx_),
            f1(ty_),
            f1(base.0 + px * 4.0),
            f1(base.1 + py * 4.0),
            f1(base.0 - px * 4.0),
            f1(base.1 - py * 4.0)
        ));
        if let Some(l) = &e.label {
            let (mx, my) = midpoint(&p.edges[k]);
            out.push_str(&format!(
                "<text x=\"{}\" y=\"{}\" text-anchor=\"middle\" font-size=\"11\" fill=\"{muted}\" stroke=\"{card}\" stroke-width=\"3\" paint-order=\"stroke\" pointer-events=\"none\" aria-hidden=\"true\">{}</text>",
                f1(mx),
                f1(my - 4.0),
                esc(l)
            ));
        }
    }
    out.push_str("</g>");

    // nodes
    out.push_str("<g class=\"dp-nodes\">");
    let ink = hex(t.colors.ink);
    for v in reading(&p) {
        let nd = &d.nodes[v];
        let pn = &p.nodes[v];
        let (x, y) = (pn.cx - pn.w / 2.0, pn.cy - pn.h / 2.0);
        let accent = nd
            .status
            .filter(|s| *s != Status::Neutral)
            .map(|s| s.color(t))
            .or_else(|| nd.category.map(|c| t.color(c)));
        let stroke = nd
            .status
            .filter(|s| *s == Status::Bad || *s == Status::Warn)
            .map(|s| hex(s.color(t)))
            .unwrap_or_else(|| hex(t.chart.line));
        let mut tip = nd.label.clone();
        if let Some(fv) = &nd.fvalue {
            tip.push_str(&format!(": {fv}"));
        }
        if let Some(b) = &nd.badge {
            tip.push_str(&format!(" ({b})"));
        }
        let value = nd
            .value
            .map(|val| {
                format!(
                    " data-value=\"{val}\" data-fvalue=\"{}\"",
                    esc(nd.fvalue.as_deref().unwrap_or(""))
                )
            })
            .unwrap_or_default();
        let status = nd
            .status
            .map(|s| format!(" data-status=\"{}\"", s.as_str()))
            .unwrap_or_default()
            + &nd
                .badge
                .as_deref()
                .map(|b| format!(" data-badge=\"{}\"", esc(b)))
                .unwrap_or_default();
        out.push_str(&format!(
            "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" rx=\"8\" fill=\"{card}\" stroke=\"{stroke}\" stroke-width=\"1.2\" data-node=\"{id}\" data-x=\"{id}\" data-series=\"{id}\" data-fx=\"{}\" data-layer=\"{}\" data-order=\"{}\"{value}{status}><title>{}</title></rect>",
            f1(x),
            f1(y),
            f1(pn.w),
            f1(pn.h),
            esc(&nd.label),
            pn.layer,
            pn.order,
            esc(&tip),
            id = esc(&nd.id),
        ));
        if let Some(a) = accent {
            out.push_str(&format!(
                "<rect x=\"{}\" y=\"{}\" width=\"3\" height=\"{}\" rx=\"1.5\" fill=\"{}\" pointer-events=\"none\" aria-hidden=\"true\"/>",
                f1(x + 1.5),
                f1(y + 6.0),
                f1((pn.h - 12.0).max(4.0)),
                hex(a)
            ));
        }
        let mut ty_ = y + BOX_PAD_Y + 12.0;
        for line in &pn.lines {
            out.push_str(&format!(
                "<text x=\"{}\" y=\"{}\" text-anchor=\"middle\" font-size=\"{LABEL_PX}\" font-weight=\"600\" fill=\"{}\" pointer-events=\"none\" aria-hidden=\"true\">{}</text>",
                f1(pn.cx),
                f1(ty_),
                hex(t.colors.heading),
                esc(line)
            ));
            ty_ += LINE_H;
        }
        if let Some(fv) = &nd.fvalue {
            out.push_str(&format!(
                "<text x=\"{}\" y=\"{}\" text-anchor=\"middle\" font-size=\"{VALUE_PX}\" font-weight=\"700\" fill=\"{ink}\" pointer-events=\"none\" aria-hidden=\"true\">{}</text>",
                f1(pn.cx),
                f1(ty_),
                esc(fv)
            ));
            ty_ += LINE_H;
        }
        if let Some(b) = &nd.badge {
            let bw = text_w(b, BADGE_PX, false) + 20.0;
            let dot = nd
                .status
                .map(|s| hex(s.color(t)))
                .unwrap_or_else(|| muted.clone());
            let (bx, by) = (pn.cx - bw / 2.0, ty_ - 9.0);
            out.push_str(&format!(
                "<g pointer-events=\"none\" aria-hidden=\"true\"><rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"16\" rx=\"8\" fill=\"none\" stroke=\"{dot}\" stroke-width=\"1\"/><circle cx=\"{}\" cy=\"{}\" r=\"3\" fill=\"{dot}\"/><text x=\"{}\" y=\"{}\" font-size=\"{BADGE_PX}\" fill=\"{ink}\">{}</text></g>",
                f1(bx),
                f1(by),
                f1(bw),
                f1(bx + 9.0),
                f1(by + 8.0),
                f1(bx + 15.0),
                f1(by + 12.0),
                esc(b)
            ));
        }
    }
    out.push_str("</g></g>");
    if let Some(nt) = note {
        out.push_str(&format!(
            "<text x=\"{}\" y=\"{}\" text-anchor=\"middle\" font-family=\"{font}\" font-size=\"11\" fill=\"{}\">{}</text>",
            f1(w / 2.0),
            f1(h - 5.0),
            hex(t.chart.note),
            esc(&nt)
        ));
    }
    out.push_str("</svg>");
    let out = with_root_attr(out, "data-summary", &summary);
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
        assert_eq!((e.from, e.to, e.style), (0, 1, EdgeStyle::Flow));
        assert_eq!(e.label.as_deref(), Some("9.8K records"));
        let codes: Vec<&str> = d.problems.iter().map(|p| p.code).collect();
        assert_eq!(codes, ["diagram/bad-edges", "diagram/unknown-node"]);
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
            "data-node=\"build\" data-x=\"build\" data-series=\"build\" data-fx=\"Build twin\""
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
