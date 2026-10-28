//! Figures (`::FIGURE` … `::ENDFIGURE`, #661): several charts composed into
//! **one** picture, patchwork-style — aligned plot areas, one shared x axis,
//! one legend, panel tags — instead of separate cards.
//!
//! ```sql
//! SELECT 'Data drift: the inputs move, the rule holds' ::FIGURE,
//!        '(hist / scatter) | rule' ::LAYOUT, '1,4' ::HEIGHTS,
//!        'x' ::AXES_COLLECT, true ::COLLECT_LEGENDS, 'A' ::TAG_LEVELS;
//! -- @name hist
//! SELECT bin ::XAXIS, period ::CATEGORY, share ::BARCHART FROM …;
//! -- @name scatter
//! SELECT x ::XAXIS, y ::SCATTER, period ::CATEGORY FROM …;
//! -- @name rule
//! SELECT x ::XAXIS, fitted ::LINECHART FROM …;
//! SELECT 1 ::ENDFIGURE;
//! ```
//!
//! The statements between the markers are the cells, each an ordinary chart
//! statement; the `::FIGURE` statement's own columns are the options
//! ([`FigureSpec`]). A cell is named by its `-- @name` (or its 1-based
//! position) in [`Node`]'s `::LAYOUT` expression. Rendering builds each
//! cell's plot with the same pipeline a lone panel uses
//! ([`crate::panel_plot`]) and composes them with ggplot-rs's
//! `compose::PlotGrid` — the composition itself (alignment, collected axes
//! and legends, tags) is ggplot-rs's, not ours.

use crate::{sql, Column, Place, RenderError, RenderOptions, Role};
use ggplot_rs::compose::{AxisCollect, GridCell, PlotGrid, TagLevels};

/// A `::LAYOUT` expression: `|` puts cells side by side, `/` stacks them
/// (`/` binds tighter, as in patchwork), parentheses group, `_` is an
/// empty cell. A chain of one operator is one n-ary group (`a | b | c` is
/// one row of three).
#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    /// A cell: its `-- @name`, or its 1-based position as text.
    Cell(String),
    Spacer,
    Row(Vec<Node>),
    Col(Vec<Node>),
}

impl Node {
    /// The cell references in reading order.
    pub fn cells(&self) -> Vec<&str> {
        match self {
            Node::Cell(c) => vec![c.as_str()],
            Node::Spacer => Vec::new(),
            Node::Row(v) | Node::Col(v) => v.iter().flat_map(Node::cells).collect(),
        }
    }

    /// The sizes of every side-by-side (`|`) and stacked (`/`) group:
    /// `(rows, cols)` cell counts, for matching `::WIDTHS` / `::HEIGHTS`.
    fn group_sizes(&self, rows: &mut Vec<usize>, cols: &mut Vec<usize>) {
        match self {
            Node::Row(v) => {
                rows.push(v.len());
                v.iter().for_each(|n| n.group_sizes(rows, cols));
            }
            Node::Col(v) => {
                cols.push(v.len());
                v.iter().for_each(|n| n.group_sizes(rows, cols));
            }
            _ => {}
        }
    }
}

/// Parse a `::LAYOUT` expression.
pub fn parse_layout(s: &str) -> Result<Node, String> {
    let toks = tokenize(s)?;
    if toks.is_empty() {
        return Err("::LAYOUT is empty".into());
    }
    let mut p = Parser { toks, at: 0 };
    let node = p.row()?;
    if p.at < p.toks.len() {
        return Err(format!(
            "::LAYOUT `{s}`: unexpected `{}`",
            p.toks[p.at].text()
        ));
    }
    Ok(node)
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Name(String),
    Bar,
    Slash,
    Open,
    Close,
}

impl Tok {
    fn text(&self) -> &str {
        match self {
            Tok::Name(n) => n,
            Tok::Bar => "|",
            Tok::Slash => "/",
            Tok::Open => "(",
            Tok::Close => ")",
        }
    }
}

fn tokenize(s: &str) -> Result<Vec<Tok>, String> {
    let mut out = Vec::new();
    let mut chars = s.chars().peekable();
    while let Some(&c) = chars.peek() {
        match c {
            c if c.is_whitespace() => {
                chars.next();
            }
            '|' => {
                chars.next();
                out.push(Tok::Bar);
            }
            '/' => {
                chars.next();
                out.push(Tok::Slash);
            }
            '(' => {
                chars.next();
                out.push(Tok::Open);
            }
            ')' => {
                chars.next();
                out.push(Tok::Close);
            }
            c if c.is_alphanumeric() || c == '_' || c == '-' => {
                let mut name = String::new();
                while let Some(&c) = chars.peek() {
                    if c.is_alphanumeric() || c == '_' || c == '-' {
                        name.push(c);
                        chars.next();
                    } else {
                        break;
                    }
                }
                out.push(Tok::Name(name));
            }
            c => return Err(format!("::LAYOUT `{s}`: unexpected `{c}`")),
        }
    }
    Ok(out)
}

struct Parser {
    toks: Vec<Tok>,
    at: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.at)
    }

    /// row := col ('|' col)*
    fn row(&mut self) -> Result<Node, String> {
        let mut items = vec![self.col()?];
        while self.peek() == Some(&Tok::Bar) {
            self.at += 1;
            items.push(self.col()?);
        }
        Ok(if items.len() == 1 {
            items.pop().unwrap()
        } else {
            Node::Row(items)
        })
    }

    /// col := atom ('/' atom)*
    fn col(&mut self) -> Result<Node, String> {
        let mut items = vec![self.atom()?];
        while self.peek() == Some(&Tok::Slash) {
            self.at += 1;
            items.push(self.atom()?);
        }
        Ok(if items.len() == 1 {
            items.pop().unwrap()
        } else {
            Node::Col(items)
        })
    }

    /// atom := name | '_' | '(' row ')'
    fn atom(&mut self) -> Result<Node, String> {
        match self.toks.get(self.at).cloned() {
            Some(Tok::Name(n)) => {
                self.at += 1;
                Ok(if n == "_" {
                    Node::Spacer
                } else {
                    Node::Cell(n)
                })
            }
            Some(Tok::Open) => {
                self.at += 1;
                let inner = self.row()?;
                if self.peek() != Some(&Tok::Close) {
                    return Err("::LAYOUT: a `(` without its `)`".into());
                }
                self.at += 1;
                Ok(inner)
            }
            Some(t) => Err(format!("::LAYOUT: expected a cell, found `{}`", t.text())),
            None => Err("::LAYOUT: ends after an operator".into()),
        }
    }
}

/// A `::FIGURE` statement's options.
#[derive(Debug, Clone, Default)]
pub struct FigureSpec {
    /// The `::FIGURE` value: the figure's title (drawn as the card's title).
    pub title: Option<String>,
    pub layout: Option<Node>,
    pub widths: Vec<f64>,
    pub heights: Vec<f64>,
    pub axes: AxesMode,
    pub collect_legends: bool,
    pub tags: Option<String>,
    pub ncol: Option<usize>,
}

/// `::AXES_COLLECT`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum AxesMode {
    #[default]
    None,
    Align,
    X,
}

impl AxesMode {
    fn grid(self) -> AxisCollect {
        match self {
            AxesMode::None => AxisCollect::Keep,
            AxesMode::Align => AxisCollect::Align,
            AxesMode::X => AxisCollect::CollectX,
        }
    }
}

fn tag_levels(v: &str) -> Option<TagLevels> {
    Some(match v.trim() {
        "A" => TagLevels::Upper,
        "a" => TagLevels::Lower,
        "1" => TagLevels::Numeric,
        "i" => TagLevels::LowerRoman,
        "I" => TagLevels::UpperRoman,
        _ => return None,
    })
}

fn weights(v: &str) -> Option<Vec<f64>> {
    v.split(',')
        .map(|w| {
            w.trim()
                .parse::<f64>()
                .ok()
                .filter(|w| *w > 0.0 && w.is_finite())
        })
        .collect()
}

fn truthy(v: &ggplot_rs::prelude::Value) -> bool {
    match v {
        ggplot_rs::prelude::Value::Bool(b) => *b,
        v => {
            v.as_f64().is_some_and(|f| f != 0.0)
                || crate::value_str(v).trim().eq_ignore_ascii_case("true")
        }
    }
}

impl FigureSpec {
    /// Read the options from the `::FIGURE` statement's columns. Returns the
    /// spec (bad options left at their defaults) and what was wrong, one
    /// message per problem (`dashboard --check` reports them).
    pub fn from_columns(cols: &[Column]) -> (Self, Vec<String>) {
        let mut spec = FigureSpec::default();
        let mut issues = Vec::new();
        let first = |c: &Column| c.values.first().filter(|v| !v.is_na()).cloned();
        for c in cols {
            let Some(v) = first(c) else { continue };
            let text = crate::value_str(&v);
            match c.role {
                Role::FigureStart => {
                    let t = text.trim();
                    if !t.is_empty() && t != "1" && t != "true" {
                        spec.title = Some(t.to_string());
                    }
                }
                Role::FigureLayout => match parse_layout(&text) {
                    Ok(n) => spec.layout = Some(n),
                    Err(e) => issues.push(e),
                },
                Role::FigureWidths | Role::FigureHeights => {
                    let w = Role::FigureWidths == c.role;
                    match weights(&text) {
                        Some(ws) if w => spec.widths = ws,
                        Some(ws) => spec.heights = ws,
                        None => issues.push(format!(
                            "::{} `{text}` — positive numbers, comma-separated ('1,2')",
                            if w { "WIDTHS" } else { "HEIGHTS" }
                        )),
                    }
                }
                Role::AxesCollect => match text.trim().to_ascii_lowercase().as_str() {
                    "x" => spec.axes = AxesMode::X,
                    "align" => spec.axes = AxesMode::Align,
                    "none" | "" => spec.axes = AxesMode::None,
                    _ => issues.push(format!(
                        "::AXES_COLLECT `{text}` — use 'x', 'align' or 'none'"
                    )),
                },
                Role::CollectLegends => spec.collect_legends = truthy(&v),
                Role::TagLevels => {
                    if tag_levels(&text).is_some() {
                        spec.tags = Some(text.trim().to_string());
                    } else {
                        issues.push(format!(
                            "::TAG_LEVELS `{text}` — use 'A', 'a', '1', 'i' or 'I'"
                        ));
                    }
                }
                Role::FigureNcol => match v.as_f64().filter(|n| *n >= 1.0 && n.fract() == 0.0) {
                    Some(n) => spec.ncol = Some(n as usize),
                    None => issues.push(format!("::FIGURE_NCOL `{text}` — a whole number ≥ 1")),
                },
                _ => {}
            }
        }
        (spec, issues)
    }

    /// Check the spec against the figure's cell names (`names[i]` is cell
    /// `i`'s `-- @name`, if any): every `::LAYOUT` reference names exactly
    /// one cell, once; every cell is placed; `::WIDTHS` / `::HEIGHTS` fit
    /// some group. Messages for `dashboard --check`; empty when it renders
    /// as written.
    pub fn check(&self, names: &[Option<String>]) -> Vec<(&'static str, String)> {
        let mut out = Vec::new();
        let n = names.len();
        let (row_sizes, col_sizes) = match &self.layout {
            Some(layout) => {
                let refs = layout.cells();
                let mut used = vec![false; n];
                for r in &refs {
                    match resolve(r, names) {
                        Some(i) if used[i] => out.push((
                            "figure/duplicate-cell",
                            format!("::LAYOUT places cell `{r}` twice"),
                        )),
                        Some(i) => used[i] = true,
                        None => out.push((
                            "figure/unknown-cell",
                            format!(
                                "::LAYOUT names `{r}`, which is no cell of this figure — {}",
                                cell_list(names)
                            ),
                        )),
                    }
                }
                for (i, u) in used.iter().enumerate() {
                    if !u {
                        out.push((
                            "figure/unplaced-cell",
                            format!(
                                "cell {} is not in ::LAYOUT, so it is not drawn",
                                label(i, names)
                            ),
                        ));
                    }
                }
                let (mut rows, mut cols) = (Vec::new(), Vec::new());
                layout.group_sizes(&mut rows, &mut cols);
                (rows, cols)
            }
            None => {
                let ncol = self.ncol.unwrap_or_else(|| default_ncol(n)).max(1);
                (vec![ncol], vec![n.div_ceil(ncol)])
            }
        };
        for (ws, sizes, what) in [
            (&self.widths, &row_sizes, "WIDTHS"),
            (&self.heights, &col_sizes, "HEIGHTS"),
        ] {
            if !ws.is_empty() && !sizes.contains(&ws.len()) {
                out.push((
                    "figure/bad-weights",
                    format!(
                        "::{what} has {} value(s), but no {} group has that many cells ({})",
                        ws.len(),
                        if what == "WIDTHS" {
                            "side-by-side"
                        } else {
                            "stacked"
                        },
                        if sizes.is_empty() {
                            "there is none".to_string()
                        } else {
                            sizes
                                .iter()
                                .map(usize::to_string)
                                .collect::<Vec<_>>()
                                .join(", ")
                        }
                    ),
                ));
            }
        }
        out
    }
}

/// ggplot-rs's own default for a grid without `ncol`/`nrow`: near-square.
fn default_ncol(n: usize) -> usize {
    (n as f64).sqrt().ceil().max(1.0) as usize
}

/// The cell a `::LAYOUT` reference names: a `-- @name`, else a 1-based
/// position.
fn resolve(r: &str, names: &[Option<String>]) -> Option<usize> {
    names
        .iter()
        .position(|n| n.as_deref() == Some(r))
        .or_else(|| {
            r.parse::<usize>()
                .ok()
                .filter(|i| (1..=names.len()).contains(i))
                .map(|i| i - 1)
        })
}

fn label(i: usize, names: &[Option<String>]) -> String {
    match &names[i] {
        Some(n) => format!("`{n}`"),
        None => format!("{}", i + 1),
    }
}

fn cell_list(names: &[Option<String>]) -> String {
    if names.is_empty() {
        return "it has no cells".into();
    }
    format!(
        "its cells are {}",
        (0..names.len())
            .map(|i| label(i, names))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

/// One cell: its `-- @name` and its columns (the query result, by role).
pub struct Cell {
    pub name: Option<String>,
    pub cols: Vec<Column>,
}

/// Render a figure: every cell's plot composed into one `PlotGrid`, as one
/// SVG `width`×`height` at `place`. A cell that isn't a chart (a table, a
/// KPI, a note) is drawn as an empty cell; `dashboard --check` flags it
/// (`figure/not-a-chart`).
pub(crate) fn render(
    spec: &FigureSpec,
    cells: &[Cell],
    width: u32,
    height: u32,
    o: &RenderOptions,
    place: Place,
) -> Result<String, RenderError> {
    crate::guard(|| {
        let names: Vec<Option<String>> = cells.iter().map(|c| c.name.clone()).collect();
        let layout = spec.layout.clone().unwrap_or_else(|| {
            Node::Row(
                (1..=cells.len())
                    .map(|i| Node::Cell(i.to_string()))
                    .collect(),
            )
        });
        let mut sizes = vec![(width as f64, height as f64); cells.len()];
        if spec.layout.is_some() {
            cell_sizes(
                &layout,
                width as f64,
                height as f64,
                spec,
                &names,
                &mut sizes,
            );
        } else {
            let ncol = spec
                .ncol
                .unwrap_or_else(|| default_ncol(cells.len()))
                .max(1);
            let nrow = cells.len().div_ceil(ncol).max(1);
            sizes.fill((width as f64 / ncol as f64, height as f64 / nrow as f64));
        }
        let mut plots: Vec<Option<ggplot_rs::prelude::GGPlot>> = Vec::with_capacity(cells.len());
        for (c, (w, h)) in cells.iter().zip(&sizes) {
            plots.push(
                crate::panel_plot(&c.cols, w.max(1.0) as u32, h.max(1.0) as u32, o)
                    .map_err(RenderError::Render)?,
            );
        }
        let mut grid = if spec.layout.is_some() {
            match build(&layout, spec, &names, &mut plots) {
                GridCell::Grid(g) => *g,
                cell => PlotGrid::new().add(cell),
            }
        } else {
            let mut g = PlotGrid::new().axis_collect(spec.axes.grid());
            let ncol = spec
                .ncol
                .unwrap_or_else(|| default_ncol(cells.len()))
                .max(1);
            g = g.ncol(ncol);
            let nrow = cells.len().div_ceil(ncol);
            if spec.widths.len() == ncol {
                g = g.widths(&spec.widths);
            }
            if spec.heights.len() == nrow {
                g = g.heights(&spec.heights);
            }
            for p in plots.iter_mut() {
                g = match p.take() {
                    Some(p) => g.add(p),
                    None => g.add_spacer(),
                };
            }
            g
        };
        grid = grid.collect_legends(spec.collect_legends);
        if let Some(t) = spec.tags.as_deref().and_then(tag_levels) {
            grid = grid.tag_levels(t);
        }
        let (svg, warnings) =
            crate::finish_grid(grid, place, width, height).map_err(RenderError::Render)?;
        Ok(crate::finish_contract_svg(svg, &warnings, o))
    })
}

/// A static page's figure (`dashboard file.sql out.html`, `serve
/// --dashboards`): [`render`] as a standalone SVG, under the `::TITLE` bar a
/// dashboard card draws (its own `width`×26 `<svg>` before the figure, like
/// [`crate::render_with_title`]) when the figure has a title. `figure` is
/// the `::FIGURE` statement's own columns.
pub fn render_with_title(
    figure: &[Column],
    cells: &[Cell],
    width: u32,
    height: u32,
    o: &RenderOptions,
) -> Result<String, RenderError> {
    let (spec, _) = FigureSpec::from_columns(figure);
    let title = spec.title.clone().map(crate::cap_page_title);
    let h = if title.is_some() {
        height.saturating_sub(crate::TITLE_BAR_H).max(60)
    } else {
        height
    };
    let body = render(&spec, cells, width, h, o, Place::Doc)?;
    Ok(match title {
        Some(t) => format!(
            "{open}{text}</svg>{body}",
            open = crate::svg_open(Place::Doc, width, crate::TITLE_BAR_H),
            text = crate::title_bar_text(&t, o),
        ),
        None => body,
    })
}

/// The `PlotGrid` cell for `node`, taking each referenced cell's plot out
/// of `plots` (a missing, repeated or non-chart reference is a spacer).
fn build(
    node: &Node,
    spec: &FigureSpec,
    names: &[Option<String>],
    plots: &mut [Option<ggplot_rs::prelude::GGPlot>],
) -> GridCell {
    match node {
        Node::Spacer => GridCell::Spacer,
        Node::Cell(r) => match resolve(r, names).and_then(|i| plots[i].take()) {
            Some(p) => GridCell::Plot(Box::new(p)),
            None => GridCell::Spacer,
        },
        Node::Row(items) | Node::Col(items) => {
            let row = matches!(node, Node::Row(_));
            let mut g = PlotGrid::new().axis_collect(spec.axes.grid());
            g = if row {
                g.nrow(1).ncol(items.len())
            } else {
                g.ncol(1)
            };
            if row && spec.widths.len() == items.len() {
                g = g.widths(&spec.widths);
            }
            if !row && spec.heights.len() == items.len() {
                g = g.heights(&spec.heights);
            }
            for it in items {
                g = g.add(build(it, spec, names, plots));
            }
            GridCell::Grid(Box::new(g))
        }
    }
}

/// Each cell's approximate size in px (its share of the figure by the
/// layout's weights) — what its own plot is built for (legend placement,
/// tick density); the grid itself decides the final layout.
fn cell_sizes(
    node: &Node,
    w: f64,
    h: f64,
    spec: &FigureSpec,
    names: &[Option<String>],
    out: &mut [(f64, f64)],
) {
    match node {
        Node::Spacer => {}
        Node::Cell(r) => {
            if let Some(i) = resolve(r, names) {
                out[i] = (w, h);
            }
        }
        Node::Row(items) | Node::Col(items) => {
            let row = matches!(node, Node::Row(_));
            let ws = if row { &spec.widths } else { &spec.heights };
            let ws: Vec<f64> = if ws.len() == items.len() {
                ws.clone()
            } else {
                vec![1.0; items.len()]
            };
            let total: f64 = ws.iter().sum();
            for (it, wt) in items.iter().zip(&ws) {
                let share = wt / total;
                if row {
                    cell_sizes(it, w * share, h, spec, names, out);
                } else {
                    cell_sizes(it, w, h * share, spec, names, out);
                }
            }
        }
    }
}

/// A `::FIGURE` block in a plan: the marker's index, the cells' indices
/// (the statements that draw a panel — setup and other directives in
/// between are not cells), and the `::ENDFIGURE`'s index (`None`: open to
/// the end of the script).
#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    pub start: usize,
    pub cells: Vec<usize>,
    pub end: Option<usize>,
}

/// Every `::FIGURE` block of `plan`, in order. A `::FIGURE` inside an open
/// one closes it first (figures don't nest — `dashboard --check` flags it).
pub fn blocks(plan: &[sql::Panel]) -> Vec<Block> {
    let has = |p: &sql::Panel, r: Role| p.roles.iter().any(|(_, x)| *x == r);
    let mut out: Vec<Block> = Vec::new();
    let mut open: Option<Block> = None;
    for (i, p) in plan.iter().enumerate() {
        if has(p, Role::FigureStart) {
            if let Some(b) = open.take() {
                out.push(b);
            }
            open = Some(Block {
                start: i,
                cells: Vec::new(),
                end: None,
            });
        } else if has(p, Role::FigureEnd) {
            if let Some(mut b) = open.take() {
                b.end = Some(i);
                out.push(b);
            }
        } else if let Some(b) = open.as_mut() {
            if !p.setup && !crate::roles::is_directive_panel(&p.roles) {
                b.cells.push(i);
            }
        }
    }
    out.extend(open);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(n: &str) -> Node {
        Node::Cell(n.into())
    }

    #[test]
    fn layout_precedence_and_groups() {
        assert_eq!(parse_layout("a").unwrap(), c("a"));
        assert_eq!(
            parse_layout("a | b | c").unwrap(),
            Node::Row(vec![c("a"), c("b"), c("c")])
        );
        // `/` binds tighter than `|`.
        assert_eq!(
            parse_layout("a | b / c").unwrap(),
            Node::Row(vec![c("a"), Node::Col(vec![c("b"), c("c")])])
        );
        assert_eq!(
            parse_layout("(hist / scatter) | checks").unwrap(),
            Node::Row(vec![Node::Col(vec![c("hist"), c("scatter")]), c("checks")])
        );
        assert_eq!(
            parse_layout("(1 | _) / 2").unwrap(),
            Node::Col(vec![Node::Row(vec![c("1"), Node::Spacer]), c("2")])
        );
        assert_eq!(parse_layout("a|b").unwrap().cells(), ["a", "b"]);
    }

    #[test]
    fn layout_errors() {
        assert!(parse_layout("").is_err());
        assert!(parse_layout("a |").is_err());
        assert!(parse_layout("(a | b").is_err());
        assert!(parse_layout("a b").is_err());
        assert!(parse_layout("a + b").is_err());
    }

    #[test]
    fn check_names_and_weights() {
        let names = vec![Some("hist".to_string()), Some("scatter".into()), None];
        let spec = FigureSpec {
            layout: Some(parse_layout("(hist / scatter) | 3").unwrap()),
            widths: vec![2.0, 1.0],
            heights: vec![1.0, 4.0],
            ..Default::default()
        };
        assert!(spec.check(&names).is_empty(), "{:?}", spec.check(&names));
        let codes = |s: &FigureSpec| {
            s.check(&names)
                .into_iter()
                .map(|(c, _)| c)
                .collect::<Vec<_>>()
        };
        let bad = FigureSpec {
            layout: Some(parse_layout("hist | hist | nope").unwrap()),
            heights: vec![1.0, 2.0, 3.0],
            ..Default::default()
        };
        assert_eq!(
            codes(&bad),
            [
                "figure/duplicate-cell",
                "figure/unknown-cell",
                "figure/unplaced-cell",
                "figure/unplaced-cell",
                "figure/bad-weights"
            ]
        );
        // No layout: a near-square grid (2 columns for 3 cells).
        let grid = FigureSpec {
            widths: vec![1.0, 2.0],
            heights: vec![1.0, 1.0],
            ..Default::default()
        };
        assert!(codes(&grid).is_empty());
    }
}
