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

/// The kind of chart, taken from the cast on the *value* column (Shaper's
/// `count()::BARCHART` etc.).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Bar,
    BarStacked,
    Line,
    Area,
    Point,
}

/// The role a result column plays in the visualization.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// The x position (`::XAXIS`).
    X,
    /// A grouping / colour series (`::CATEGORY`).
    Category,
    /// A section heading (`::LABEL`) — becomes the chart title.
    Label,
    /// The measured value, carrying the chart kind (`count()::BARCHART`).
    Value(Kind),
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
        "CATEGORY" | "SERIES" | "COLOR" | "COLOUR" => Some(Role::Category),
        "LABEL" | "TITLE" => Some(Role::Label),
        "BARCHART" | "BAR" => Some(Role::Value(Kind::Bar)),
        "BARCHART_STACKED" | "BAR_STACKED" | "STACKED_BAR" => Some(Role::Value(Kind::BarStacked)),
        "LINECHART" | "LINE" => Some(Role::Value(Kind::Line)),
        "AREACHART" | "AREA" => Some(Role::Value(Kind::Area)),
        "SCATTER" | "POINT" | "SCATTERCHART" => Some(Role::Value(Kind::Point)),
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
    let x = cols.iter().find(|c| c.role == Role::X).ok_or("no XAXIS column")?;
    let category = cols.iter().find(|c| c.role == Role::Category);

    let mut data: Vec<(String, Vec<Value>)> = vec![
        ("x".to_string(), x.values.clone()),
        ("y".to_string(), value.values.clone()),
    ];
    let by_colour = matches!(kind, Kind::Line | Kind::Point);
    let mut aes = Aes::new().x("x").y("y");
    if let Some(cat) = category {
        data.push(("cat".to_string(), cat.values.clone()));
        aes = if by_colour { aes.color("cat") } else { aes.fill("cat") };
    }
    // Richer hover: label each mark with its series (or x). The geom appends the
    // value, so a stacked-bar segment reads e.g. "web: 22". Tooltip-only — not drawn.
    let label_vals = category.map(|c| c.values.clone()).unwrap_or_else(|| x.values.clone());
    data.push(("label".to_string(), label_vals));
    aes = aes.label("label");

    let mut plot = GGPlot::new(data).aes(aes);
    plot = match kind {
        Kind::Bar => plot.geom_col().position(PositionDodge),
        Kind::BarStacked => plot.geom_col().position(PositionStack),
        Kind::Line => plot.geom_line(),
        Kind::Area => plot.geom_area(),
        Kind::Point => plot.geom_point(),
    };
    if category.is_some() {
        // Dark2 — a calmer, more distinct categorical palette than Set1.
        plot = if by_colour {
            plot.scale_color_brewer(PaletteName::Dark2)
        } else {
            plot.scale_fill_brewer(PaletteName::Dark2)
        };
    }
    // A modern teal for single-series marks (matches Dark2's first hue). Set the
    // primary AFTER the theme preset — presets replace the whole theme.
    plot = plot.theme_minimal().primary_color((0x2A, 0x9D, 0x8F));
    if let Some(t) = &title {
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
