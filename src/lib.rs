//! duckplot — SQL-defined dashboards, Shaper-style.
//!
//! You annotate SQL result columns with *roles* (`XAXIS`, `CATEGORY`, `LABEL`,
//! and a chart kind on the value column such as `BARCHART`/`LINECHART`), and this
//! crate maps that annotated result set onto [`ggplot-rs`](ggplot_rs) and renders
//! an SVG. The core here is dependency-light and wasm-compatible — the DuckDB
//! extension packaging (native + wasm) sits on top and calls [`render`].
//!
//! ```
//! use duckplot::{Column, Role, Kind, render};
//! use ggplot_rs::prelude::Value;
//! let cols = vec![
//!     Column::new("week", Role::X, vec![Value::Str("W1".into()), Value::Str("W2".into())]),
//!     Column::new("n", Role::Value(Kind::Bar), vec![Value::Float(3.0), Value::Float(7.0)]),
//! ];
//! let svg = render(&cols, 480, 320).unwrap();
//! assert!(svg.contains("<svg"));
//! ```

use ggplot_rs::prelude::*;

pub mod sql;
#[cfg(feature = "wasm")]
pub mod wasm;

/// The kind of chart, taken from the cast on the *value* column (Shaper's
/// `count()::BARCHART` etc.).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Bar,
    BarStacked,
    Line,
    Area,
    Point,
    /// A pie/donut — slices by `CATEGORY`, sized by the measure (`::PIE`).
    Pie,
    /// A histogram of the measure column (`::HISTOGRAM`).
    Histogram,
    /// A box plot — `x` groups, `y` = the measure (`::BOXPLOT`).
    Boxplot,
    /// A heatmap — `x` × `y` tiles coloured by the measure (`::HEATMAP`).
    Heatmap,
}

/// The kind of control an `::` input renders.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputKind {
    Dropdown,
    Number,
    Date,
    Text,
}

/// How a KPI value is formatted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MetricFmt {
    Plain,
    Money,
    Percent,
    Compact,
}

/// The role a result column plays in the visualization.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// The x position (`::XAXIS`).
    X,
    /// The y position, for a heatmap's second axis (`::YAXIS`).
    Y,
    /// A grouping / colour series (`::CATEGORY`).
    Category,
    /// A section heading (`::LABEL`) — becomes the chart title.
    Label,
    /// The measured value, carrying the chart kind (`count()::BARCHART`).
    Value(Kind),
    /// A control input (`::DROPDOWN`/`::NUMBER`/`::DATE`/`::TEXT`) — the output
    /// column name is a DuckDB variable, usable via `getvariable('name')`.
    Input(InputKind),
    /// Layout: `::COLUMNS` sets the grid column count (the value is the number).
    Columns,
    /// Layout: `::GROUP` opens a box; enclosed panels/controls sit together in it.
    GroupStart,
    /// Layout: `::ENDGROUP` closes the current box.
    GroupEnd,
    /// Layout: `::SPAN` makes the *next* panel span N grid columns (the value).
    Span,
    /// A data table (`::TABLE`) — the whole result set as an HTML table.
    Table,
    /// A single big-number KPI (`::METRIC`/`::MONEY`/`::PERCENT`/`::COMPACT`); an
    /// optional `::LABEL` is the caption.
    Metric(MetricFmt),
    /// Layout: `::TAB` starts a new tab; following panels live under it.
    Tab,
}

/// A single annotated result column: a name, its [`Role`], and its values.
pub struct Column {
    pub name: String,
    pub role: Role,
    pub values: Vec<Value>,
}

impl Column {
    pub fn new(name: impl Into<String>, role: Role, values: Vec<Value>) -> Self {
        Column { name: name.into(), role, values }
    }
}

/// Parse a Shaper-style role annotation (the part after `::`) into a [`Role`].
/// Case-insensitive; returns `None` for unknown annotations (plain columns).
pub fn parse_role(annotation: &str) -> Option<Role> {
    match annotation.trim().to_ascii_uppercase().as_str() {
        "XAXIS" | "X" => Some(Role::X),
        "YAXIS" | "Y" => Some(Role::Y),
        "CATEGORY" | "SERIES" | "COLOR" | "COLOUR" => Some(Role::Category),
        "LABEL" | "TITLE" => Some(Role::Label),
        "BARCHART" | "BAR" => Some(Role::Value(Kind::Bar)),
        "BARCHART_STACKED" | "BAR_STACKED" | "STACKED_BAR" => Some(Role::Value(Kind::BarStacked)),
        "LINECHART" | "LINE" => Some(Role::Value(Kind::Line)),
        "AREACHART" | "AREA" => Some(Role::Value(Kind::Area)),
        "SCATTER" | "POINT" | "SCATTERCHART" => Some(Role::Value(Kind::Point)),
        "PIE" | "DONUT" | "PIECHART" => Some(Role::Value(Kind::Pie)),
        "HISTOGRAM" | "HIST" => Some(Role::Value(Kind::Histogram)),
        "BOXPLOT" | "BOX_PLOT" => Some(Role::Value(Kind::Boxplot)),
        "HEATMAP" | "TILE" | "TILES" => Some(Role::Value(Kind::Heatmap)),
        "TABLE" | "GRID" => Some(Role::Table),
        "METRIC" | "KPI" | "BIGNUMBER" => Some(Role::Metric(MetricFmt::Plain)),
        "MONEY" | "DOLLAR" | "CURRENCY" => Some(Role::Metric(MetricFmt::Money)),
        "PERCENT" | "PCT" => Some(Role::Metric(MetricFmt::Percent)),
        "COMPACT" => Some(Role::Metric(MetricFmt::Compact)),
        "TAB" | "PAGE" => Some(Role::Tab),
        "DROPDOWN" | "OPTIONS" | "SELECT_INPUT" => Some(Role::Input(InputKind::Dropdown)),
        "NUMBER" | "SLIDER" | "NUMERIC" => Some(Role::Input(InputKind::Number)),
        "DATE" | "DATEPICKER" => Some(Role::Input(InputKind::Date)),
        "TEXT" | "SEARCH" | "STRING" => Some(Role::Input(InputKind::Text)),
        "COLUMNS" | "COLS" => Some(Role::Columns),
        "GROUP" | "BOX" | "ROW" => Some(Role::GroupStart),
        "ENDGROUP" | "ENDBOX" | "ENDROW" => Some(Role::GroupEnd),
        "SPAN" | "WIDTH" | "COL" => Some(Role::Span),
        _ => None,
    }
}

fn value_str(v: &Value) -> String {
    match v {
        Value::Str(s) => s.clone(),
        Value::Na => String::new(),
        _ => v.as_f64().map(|f| format!("{}", (f * 1000.0).round() / 1000.0)).unwrap_or_default(),
    }
}

/// The DataZoo palette — duckplot's default categorical + single-series colours.
pub const DZ_COLORS: [(u8, u8, u8); 5] = [
    (0x45, 0x64, 0x81), // steel blue
    (0xe8, 0x64, 0x33), // orange
    (0xE8, 0x33, 0x5D), // pink
    (0xef, 0xc9, 0x4c), // yellow
    (0x21, 0x21, 0x21), // near-black
];

fn dz_color(i: usize) -> ggplot_rs::scale::color::RGBAColor {
    let (r, g, b) = DZ_COLORS[i % DZ_COLORS.len()];
    ggplot_rs::scale::color::RGBAColor::new(r, g, b)
}

/// Distinct category labels in a **stable (sorted) order**, so a given series
/// gets the same DataZoo colour in every chart that contains it.
fn distinct_labels(col: &Column) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    for v in &col.values {
        let s = value_str(v);
        if !seen.iter().any(|x| x == &s) {
            seen.push(s);
        }
    }
    seen.sort();
    seen
}

/// Render an annotated result set to an SVG dashboard element.
///
/// Recognises one `X` column, an optional `Category` column, an optional `Label`
/// (→ title), and one `Value(kind)` column that selects the geom. A result with
/// only a `Label` renders as a heading.
pub fn render(cols: &[Column], width: u32, height: u32) -> Result<String, String> {
    let title = cols
        .iter()
        .find(|c| c.role == Role::Label)
        .and_then(|c| c.values.first())
        .map(value_str);

    let value = cols.iter().find(|c| matches!(c.role, Role::Value(_)));
    let Some(value) = value else {
        // Label-only → a heading element.
        return Ok(heading_svg(title.as_deref().unwrap_or(""), width));
    };
    let Role::Value(kind) = value.role else { unreachable!() };
    match kind {
        Kind::Pie => return render_pie(value, cols, title.as_deref(), width, height),
        Kind::Histogram => return render_histogram(value, title.as_deref(), width, height),
        Kind::Heatmap => return render_heatmap(value, cols, title.as_deref(), width, height),
        _ => {}
    }
    let x = cols.iter().find(|c| c.role == Role::X).ok_or("no XAXIS column")?;
    let category = cols.iter().find(|c| c.role == Role::Category);

    let mut data: Vec<(String, Vec<Value>)> = vec![
        ("x".to_string(), x.values.clone()),
        ("y".to_string(), value.values.clone()),
    ];
    let by_colour = matches!(kind, Kind::Line | Kind::Point);
    let bar = matches!(kind, Kind::Bar | Kind::BarStacked);
    let x_discrete = x.values.iter().any(|v| matches!(v, Value::Str(_)));
    let mut aes = Aes::new().x("x").y("y");

    // Colour dimension: an explicit CATEGORY, or — for a bar chart with no
    // category — the (discrete) X itself, so a "total per channel" bar matches
    // the same channel's colour in the other charts. `color_levels` are the
    // stable palette keys; `x_coloured` suppresses the then-redundant legend.
    let mut x_coloured = false;
    let color_levels: Option<Vec<String>> = if let Some(cat) = category {
        data.push(("cat".to_string(), cat.values.clone()));
        aes = if by_colour { aes.color("cat") } else { aes.fill("cat") };
        Some(distinct_labels(cat))
    } else if bar && x_discrete {
        aes = aes.fill("x");
        x_coloured = true;
        Some(distinct_labels(x))
    } else {
        None
    };
    // Richer hover: label each mark with its series (or x). The geom appends the
    // value, so a stacked-bar segment reads e.g. "web: 22". Tooltip-only — not drawn.
    let label_vals = category.map(|c| c.values.clone()).unwrap_or_else(|| x.values.clone());
    data.push(("label".to_string(), label_vals));
    aes = aes.label("label");

    let mut plot = GGPlot::new(data).aes(aes);
    plot = match kind {
        Kind::Bar => plot.geom_col().position(PositionDodge),
        Kind::BarStacked => plot.geom_col().position(PositionStack),
        // Lines/areas also get point markers — they carry the per-point `<title>`
        // so every chart is hoverable (and clickable for linking).
        Kind::Line => plot.geom_line().geom_point(),
        Kind::Area => plot.geom_area().geom_point(),
        Kind::Point => plot.geom_point(),
        Kind::Boxplot => plot.geom_boxplot(),
        Kind::Pie | Kind::Histogram | Kind::Heatmap => unreachable!("handled above"),
    };
    if let Some(levels) = &color_levels {
        // Map the distinct series to the DataZoo palette (stable/sorted order, so
        // a given series is the same colour in every chart).
        let pairs: Vec<(&str, ggplot_rs::scale::color::RGBAColor)> =
            levels.iter().enumerate().map(|(i, s)| (s.as_str(), dz_color(i))).collect();
        plot = if by_colour { plot.scale_color_manual(pairs) } else { plot.scale_fill_manual(pairs) };
    }
    if x_coloured {
        plot = plot.show_legend(false); // the x axis already labels the colours
    }
    // DataZoo steel blue for single-series marks. Set the primary AFTER the theme
    // preset — presets replace the whole theme.
    plot = plot.theme_minimal().primary_color(DZ_COLORS[0]);
    if let Some(t) = &title {
        plot = plot.title(t);
    }
    plot.render_svg_native_with_size(width, height)
        .map_err(|e| format!("render failed: {e:?}"))
}

/// A histogram of the measure column (ggplot bins + counts).
fn render_histogram(value: &Column, title: Option<&str>, width: u32, height: u32) -> Result<String, String> {
    let data = vec![("x".to_string(), value.values.clone())];
    let mut plot = GGPlot::new(data)
        .aes(Aes::new().x("x"))
        .geom_histogram()
        .theme_minimal()
        .primary_color(DZ_COLORS[0]);
    if let Some(t) = title {
        plot = plot.title(t);
    }
    plot.render_svg_native_with_size(width, height)
        .map_err(|e| format!("render failed: {e:?}"))
}

/// A heatmap: `x` × `y` tiles coloured by the measure (light → steel blue).
fn render_heatmap(
    value: &Column,
    cols: &[Column],
    title: Option<&str>,
    width: u32,
    height: u32,
) -> Result<String, String> {
    let x = cols.iter().find(|c| c.role == Role::X).ok_or("heatmap needs an XAXIS column")?;
    let y = cols.iter().find(|c| c.role == Role::Y).ok_or("heatmap needs a YAXIS column")?;
    let data = vec![
        ("x".to_string(), x.values.clone()),
        ("y".to_string(), y.values.clone()),
        ("fill".to_string(), value.values.clone()),
        ("label".to_string(), value.values.clone()),
    ];
    let mut plot = GGPlot::new(data)
        .aes(Aes::new().x("x").y("y").fill("fill").label("label"))
        .geom_tile()
        .scale_fill_gradient(
            ggplot_rs::scale::color::RGBAColor::new(0xed, 0xf1, 0xf7),
            ggplot_rs::scale::color::RGBAColor::new(DZ_COLORS[0].0, DZ_COLORS[0].1, DZ_COLORS[0].2),
        )
        .theme_minimal();
    if let Some(t) = title {
        plot = plot.title(t);
    }
    plot.render_svg_native_with_size(width, height)
        .map_err(|e| format!("render failed: {e:?}"))
}

/// A pie/donut: one stacked bar (x constant) wrapped into polar coords, sliced
/// and coloured by `CATEGORY`.
fn render_pie(
    value: &Column,
    cols: &[Column],
    title: Option<&str>,
    width: u32,
    height: u32,
) -> Result<String, String> {
    let category = cols.iter().find(|c| c.role == Role::Category).ok_or("pie needs a CATEGORY column")?;
    let n = value.values.len();
    let data: Vec<(String, Vec<Value>)> = vec![
        ("x".to_string(), vec![Value::Str(String::new()); n]),
        ("y".to_string(), value.values.clone()),
        ("cat".to_string(), category.values.clone()),
        ("label".to_string(), category.values.clone()),
    ];
    let levels = distinct_labels(category);
    let pairs: Vec<(&str, ggplot_rs::scale::color::RGBAColor)> =
        levels.iter().enumerate().map(|(i, s)| (s.as_str(), dz_color(i))).collect();
    let mut plot = GGPlot::new(data)
        .aes(Aes::new().x("x").y("y").fill("cat").label("label"))
        .geom_col()
        .position(PositionStack)
        .scale_fill_manual(pairs)
        .coord_polar_with(ggplot_rs::coord::polar::CoordPolar::new().theta("y"))
        .theme_void();
    if let Some(t) = title {
        plot = plot.title(t);
    }
    plot.render_svg_native_with_size(width, height)
        .map_err(|e| format!("render failed: {e:?}"))
}

// ── C ABI (for the DuckDB extension side-module) ───────────────────────────
use std::ffi::CString;
use std::os::raw::c_char;

/// C-ABI smoke test: render a fixed bar chart and return a heap SVG string
/// (free it with [`duckplot_free`]). Exercises the whole render path through FFI
/// — the shape the DuckDB extension entrypoint will use.
#[no_mangle]
pub extern "C" fn duckplot_smoke() -> *mut c_char {
    let cols = vec![
        Column::new("x", Role::X, vec![Value::Str("a".into()), Value::Str("b".into())]),
        Column::new("n", Role::Value(Kind::Bar), vec![Value::Float(3.0), Value::Float(7.0)]),
    ];
    let svg = render(&cols, 300, 200).unwrap_or_default();
    CString::new(svg).map(|s| s.into_raw()).unwrap_or(std::ptr::null_mut())
}

/// Free a string returned by the C ABI.
#[no_mangle]
pub extern "C" fn duckplot_free(p: *mut c_char) {
    if !p.is_null() {
        unsafe { drop(CString::from_raw(p)) };
    }
}

/// A minimal SVG heading (for a `::LABEL`-only result).
fn heading_svg(text: &str, width: u32) -> String {
    let esc = text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"40\" viewBox=\"0 0 {width} 40\">\
         <text x=\"4\" y=\"26\" font-family=\"system-ui,sans-serif\" font-size=\"20\" font-weight=\"600\" fill=\"#1f2430\">{esc}</text></svg>"
    )
}
