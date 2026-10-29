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
use ggplot_rs::compose::{AxisCollect, GridCell, GridElement, PlotGrid, TagLevels};

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
#[derive(Clone)]
pub struct Cell {
    pub name: Option<String>,
    pub cols: Vec<Column>,
    /// The browser's zoom window (#695, `?zoom=1`): x and y for the cell
    /// zoomed, x only for a cell that shares its x axis with it (its y
    /// auto-fits). `None`: auto-fit.
    pub zoom: Option<CellZoom>,
    /// The cell's whole result, its roles and display names (#694): a cell
    /// that isn't a chart — a table, a KPI, a note, Markdown — is drawn from
    /// these, as the static dashboard draws that panel. `None`: an empty cell.
    pub raw: Option<CellRows>,
}

/// A figure cell's result as the headless dashboard renders a panel from it.
#[derive(Debug, Clone, Default)]
pub struct CellRows {
    pub rows: Vec<(String, Vec<ggplot_rs::prelude::Value>)>,
    pub roles: Vec<(usize, Role)>,
    pub names: Vec<(usize, String)>,
}

/// A figure cell's zoom window: `(x0, x1)` and, unless only shared, `(y0, y1)`.
pub type CellZoom = ((f64, f64), Option<(f64, f64)>);

/// Which cells share one x axis (#695: zooming one moves them all) — the
/// plot cells `AxisCollect::CollectX` puts in one column of a grid, under
/// `::AXES_COLLECT 'x'`: in a plain grid a column (cell `k` is in column
/// `k % ncol`, row-major), in a `::LAYOUT` the direct cells of one `/`
/// stack; a group only with two charts or more. `None`: the cell zooms alone.
fn x_share(
    spec: &FigureSpec,
    names: &[Option<String>],
    is_plot: &[bool],
    ncol: usize,
) -> Vec<Option<usize>> {
    let mut out = vec![None; is_plot.len()];
    if spec.axes != AxesMode::X {
        return out;
    }
    let mut groups: Vec<Vec<usize>> = Vec::new();
    match &spec.layout {
        None => {
            for c in 0..ncol {
                groups.push(
                    (c..is_plot.len())
                        .step_by(ncol)
                        .filter(|&k| is_plot[k])
                        .collect(),
                );
            }
        }
        Some(layout) => {
            fn walk(
                n: &Node,
                names: &[Option<String>],
                is_plot: &[bool],
                groups: &mut Vec<Vec<usize>>,
            ) {
                if let Node::Row(items) | Node::Col(items) = n {
                    if matches!(n, Node::Col(_)) {
                        groups.push(
                            items
                                .iter()
                                .filter_map(|it| match it {
                                    Node::Cell(r) => resolve(r, names).filter(|&i| is_plot[i]),
                                    _ => None,
                                })
                                .collect(),
                        );
                    }
                    for it in items {
                        walk(it, names, is_plot, groups);
                    }
                }
            }
            walk(layout, names, is_plot, &mut groups);
        }
    }
    for (g, members) in groups.into_iter().filter(|m| m.len() >= 2).enumerate() {
        for k in members {
            out[k] = Some(g);
        }
    }
    out
}

/// Render a figure: every cell's plot composed into one `PlotGrid`, as one
/// SVG `width`×`height` at `place`. A cell that isn't a chart (a table, a
/// KPI, a note, Markdown) is a non-plot cell drawn at its real size from its
/// [`Cell::raw`] rows, the way the static dashboard draws that panel (#694,
/// patchwork's `wrap_elements`); without them it is empty.
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
        let mut elements: Vec<Option<GridElement>> = Vec::with_capacity(cells.len());
        for (i, (c, (w, h))) in cells.iter().zip(&sizes).enumerate() {
            let mut co = o.clone();
            match c.zoom {
                Some((x, Some(y))) => co.zoom = Some((x, y)),
                Some((x, None)) => co.zoom_x = Some(x),
                None => {}
            }
            // #695: each cell's <svg> says which cell it is (statement order)
            // and its `-- @name`, so the browser finds its rows and roles
            let p = crate::panel_plot(&c.cols, w.max(1.0) as u32, h.max(1.0) as u32, &co)
                .map_err(RenderError::Render)?
                .map(|p| {
                    let p = p.svg_attr("data-cell", i.to_string());
                    match &c.name {
                        Some(n) => p.svg_attr("data-dp-name", n.clone()),
                        None => p,
                    }
                });
            elements.push(match (&p, &c.raw) {
                (None, Some(raw)) => Some(element(i, c.name.as_deref(), raw, &co)),
                _ => None,
            });
            plots.push(p);
        }
        // #695: the cells a zoom moves together carry their group
        let ncol = spec
            .ncol
            .unwrap_or_else(|| default_ncol(cells.len()))
            .max(1);
        let is_plot: Vec<bool> = plots.iter().map(Option::is_some).collect();
        for (p, g) in plots.iter_mut().zip(x_share(spec, &names, &is_plot, ncol)) {
            if let Some(g) = g {
                *p = p.take().map(|p| p.svg_attr("data-xshare", g.to_string()));
            }
        }
        let mut grid = if spec.layout.is_some() {
            match build(&layout, spec, &names, &mut plots, &mut elements) {
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
            for (p, e) in plots.iter_mut().zip(elements.iter_mut()) {
                g = match (p.take(), e.take()) {
                    (Some(p), _) => g.add(p),
                    (None, Some(e)) => g.add(e),
                    (None, None) => g.add_spacer(),
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

/// A non-chart cell (#694): drawn by the headless dashboard's panel renderer
/// at the cell's real size, wrapped like a plot cell's content (`data-cell`
/// = its statement order, and its `-- @name`).
fn element(i: usize, name: Option<&str>, raw: &CellRows, o: &RenderOptions) -> GridElement {
    let (raw, o) = (raw.clone(), o.clone());
    let open = match name {
        Some(n) => format!(
            "<g data-cell=\"{i}\" data-dp-name=\"{}\">",
            crate::format::escape_xml(n)
        ),
        None => format!("<g data-cell=\"{i}\">"),
    };
    GridElement::fill(move |w, h| {
        let svg = crate::dashboard::panel_svg_at_origin(&raw.rows, &raw.roles, &raw.names, w, h, &o);
        format!("{open}{svg}</g>")
    })
}

/// The `PlotGrid` cell for `node`, taking each referenced cell's plot (or
/// non-chart element) out of `plots` / `elements` (a missing or repeated
/// reference is a spacer).
fn build(
    node: &Node,
    spec: &FigureSpec,
    names: &[Option<String>],
    plots: &mut [Option<ggplot_rs::prelude::GGPlot>],
    elements: &mut [Option<GridElement>],
) -> GridCell {
    match node {
        Node::Spacer => GridCell::Spacer,
        Node::Cell(r) => match resolve(r, names) {
            Some(i) => match (plots[i].take(), elements[i].take()) {
                (Some(p), _) => GridCell::Plot(Box::new(p)),
                (None, Some(e)) => GridCell::Element(Box::new(e)),
                (None, None) => GridCell::Spacer,
            },
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
                g = g.add(build(it, spec, names, plots, elements));
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

    fn line(k: f64) -> Vec<Column> {
        use crate::{Kind, Value};
        vec![
            Column::new(
                "x",
                Role::X,
                (0..10).map(|i| Value::Float(i as f64)).collect(),
            ),
            Column::new(
                "y",
                Role::Value(Kind::Line),
                (0..10).map(|i| Value::Float(k * i as f64)).collect(),
            ),
        ]
    }

    fn attr(svg: &str, cell: usize, a: &str) -> Option<String> {
        // the cell's own <svg data-panel> (its x/y domain) holds `<g data-cell="i" …>`
        let at = svg.find(&format!("data-cell=\"{cell}\""))?;
        let open = svg[..at].rfind("<svg")?;
        let tag = &svg[open..svg[open..].find('>')? + open];
        let g = &svg[at..svg[at..].find('>')? + at];
        [tag, g].iter().find_map(|t| {
            let i = t.find(&format!(" {a}=\""))? + a.len() + 3;
            Some(t[i..i + t[i..].find('"')?].to_string())
        })
    }

    #[test]
    fn tables_and_kpis_are_cells_too() {
        use crate::{MetricFmt, Value};
        // #694: a chart, a table and a KPI composed into one figure
        let spec = FigureSpec {
            layout: Some(parse_layout("chart | (verdicts / kpi)").unwrap()),
            ..Default::default()
        };
        let table = CellRows {
            rows: vec![
                (
                    "check".into(),
                    vec![Value::Str("drift".into()), Value::Str("nulls".into())],
                ),
                (
                    "c1".into(),
                    vec![Value::Str("FAIL".into()), Value::Str("PASS".into())],
                ),
            ],
            roles: vec![(1, Role::Table)],
            names: vec![],
        };
        let kpi = CellRows {
            rows: vec![("c0".into(), vec![Value::Float(1234.0)])],
            roles: vec![(0, Role::Metric(MetricFmt::Plain))],
            names: vec![],
        };
        let cells = [
            Cell {
                name: Some("chart".into()),
                cols: line(1.0),
                zoom: None,
                raw: None,
            },
            Cell {
                name: Some("verdicts".into()),
                cols: vec![],
                zoom: None,
                raw: Some(table),
            },
            Cell {
                name: Some("kpi".into()),
                cols: vec![Column::new(
                    "c0",
                    Role::Metric(MetricFmt::Plain),
                    vec![Value::Float(1234.0)],
                )],
                zoom: None,
                raw: Some(kpi),
            },
        ];
        let svg = render(&spec, &cells, 800, 400, &RenderOptions::default(), Place::Doc).unwrap();
        assert!(svg.contains("drift") && svg.contains("nulls") && svg.contains("FAIL"), "{svg}");
        assert!(svg.contains("1,234") || svg.contains("1.2K") || svg.contains("1234"), "{svg}");
        assert!(svg.contains(r#"<g data-cell="1" data-dp-name="verdicts">"#));
        assert!(svg.contains(r#"<g data-cell="2" data-dp-name="kpi">"#));
        // the chart is still a plot cell
        assert!(svg.contains(r#"data-dp-name="chart""#));
        // without its rows a non-chart cell is empty (no error)
        let empty = [Cell {
            raw: None,
            ..cells[1].clone()
        }];
        assert!(render(&FigureSpec::default(), &empty, 400, 300, &RenderOptions::default(), Place::Doc).is_ok());
    }

    #[test]
    fn x_share_groups_follow_axes_collect() {
        let names = vec![
            Some("hist".to_string()),
            Some("scatter".into()),
            Some("rule".into()),
        ];
        let mut spec = FigureSpec {
            layout: Some(parse_layout("(hist / scatter) | rule").unwrap()),
            axes: AxesMode::X,
            ..Default::default()
        };
        let all = [true, true, true];
        assert_eq!(
            x_share(&spec, &names, &all, 2),
            vec![Some(0), Some(0), None]
        );
        assert_eq!(
            x_share(&spec, &names, &[true, false, true], 2),
            vec![None; 3],
            "one chart in the stack shares nothing"
        );
        spec.axes = AxesMode::Align;
        assert_eq!(
            x_share(&spec, &names, &all, 2),
            vec![None; 3],
            "'align' keeps each x its own"
        );
        let grid = FigureSpec {
            axes: AxesMode::X,
            ..Default::default()
        };
        let four = [true; 4];
        assert_eq!(
            x_share(&grid, &[None, None, None, None], &four, 2),
            vec![Some(0), Some(1), Some(0), Some(1)]
        );
        assert_eq!(
            x_share(&grid, &[None, None], &[true; 2], 2),
            vec![None; 2],
            "one row: no column of two"
        );
    }

    #[test]
    fn a_zoom_moves_the_cells_sharing_x_and_their_y_still_fits() {
        let spec = FigureSpec {
            axes: AxesMode::X,
            ncol: Some(1),
            ..Default::default()
        };
        let cell = |k: f64, zoom| Cell {
            name: None,
            cols: line(k),
            zoom,
            raw: None,
        };
        let o = RenderOptions::default();
        let base = render(
            &spec,
            &[cell(1.0, None), cell(10.0, None)],
            600,
            400,
            &o,
            Place::Doc,
        )
        .unwrap();
        assert_eq!(attr(&base, 0, "data-xshare").as_deref(), Some("0"));
        assert_eq!(attr(&base, 1, "data-xshare").as_deref(), Some("0"));
        let zoomed = render(
            &spec,
            &[
                cell(1.0, Some(((2.0, 4.0), Some((2.0, 4.0))))),
                cell(10.0, Some(((2.0, 4.0), None))),
            ],
            600,
            400,
            &o,
            Place::Doc,
        )
        .unwrap();
        let xd = |svg: &str, i| attr(svg, i, "data-xdomain").unwrap();
        assert_ne!(xd(&base, 0), xd(&zoomed, 0), "the zoomed cell's x changed");
        assert_eq!(
            xd(&zoomed, 0),
            xd(&zoomed, 1),
            "the sharing cell's x moved with it"
        );
        assert_eq!(
            attr(&base, 1, "data-ydomain"),
            attr(&zoomed, 1, "data-ydomain"),
            "its y auto-fits as before"
        );
    }

    #[test]
    fn a_histogram_stacked_with_a_zoomed_chart_follows_its_x() {
        use crate::{Kind, Value};
        let spec = FigureSpec {
            axes: AxesMode::X,
            ncol: Some(1),
            ..Default::default()
        };
        let hist = vec![Column::new(
            "v",
            Role::Value(Kind::Histogram),
            (0..50).map(|i| Value::Float((i % 10) as f64)).collect(),
        )];
        let cells = |z: Option<CellZoom>, zx: Option<CellZoom>| {
            vec![
                Cell {
                    name: None,
                    cols: hist.clone(),
                    zoom: zx,
                    raw: None,
                },
                Cell {
                    name: None,
                    cols: line(1.0),
                    zoom: z,
                    raw: None,
                },
            ]
        };
        let o = RenderOptions::default();
        let base = render(&spec, &cells(None, None), 600, 400, &o, Place::Doc).unwrap();
        let z = render(
            &spec,
            &cells(
                Some(((2.0, 4.0), Some((2.0, 4.0)))),
                Some(((2.0, 4.0), None)),
            ),
            600,
            400,
            &o,
            Place::Doc,
        )
        .unwrap();
        let xd = |svg: &str, i| attr(svg, i, "data-xdomain").unwrap();
        assert_ne!(xd(&base, 1), xd(&z, 1));
        assert_eq!(
            xd(&z, 0),
            xd(&z, 1),
            "the histogram's x follows the zoom, so the shared column does"
        );
    }
}
