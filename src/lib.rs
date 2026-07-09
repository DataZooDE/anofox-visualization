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
    /// Dodged bars with a percent-formatted y-axis (`::BARCHART_PERCENT`).
    BarPercent,
    /// Bars normalised to 100% per x (`::BARCHART_STACKED_PERCENT`).
    BarStackedPercent,
    Line,
    /// A line with a percent-formatted y-axis (`::LINECHART_PERCENT`).
    LinePercent,
    Area,
    Point,
    /// A pie — slices by `CATEGORY`, sized by the measure (`::PIE`).
    Pie,
    /// A donut (pie with a centre hole) (`::DONUTCHART`).
    Donut,
    /// A histogram of the measure column (`::HISTOGRAM`).
    Histogram,
    /// A box plot — `x` groups, `y` = the measure (`::BOXPLOT`).
    Boxplot,
    /// A heatmap — `x` × `y` tiles coloured by the measure (`::HEATMAP`).
    Heatmap,
    /// A minimal inline trend line, no axes (`::SPARKLINE`).
    Sparkline,
    /// A single value as a gauge/progress arc toward a `::RANGE` (`::GAUGE`).
    Gauge,
}

/// Font size for a single-value text card (`::TEXT_SMALL`/`_MEDIUM`/`_LARGE`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextSize {
    Small,
    Medium,
    Large,
}

/// The file format a `::DOWNLOAD_*` button produces.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DownloadFmt {
    Csv,
    Xlsx,
    Pdf,
}

/// The kind of control an `::` input renders.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputKind {
    Dropdown,
    Number,
    Date,
    Text,
    Multiselect,
    DateRange,
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
    /// A section heading (`::LABEL`) when alone; a per-mark / per-feature label
    /// (tooltips, map features) when it accompanies a chart.
    Label,
    /// A per-box title bar (`::TITLE`/`::HEADING`) drawn above a single panel.
    Title,
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
    /// Layout: `::HEIGHT` sets the *next* panel's height in pixels (the value).
    Height,
    /// A data table (`::TABLE`) — the whole result set as an HTML table.
    Table,
    /// A SQL-paginated table (`::PAGED`) — the browser pages it with
    /// `LIMIT`/`OFFSET` + `COUNT(*)`, holding one page at a time.
    PagedTable,
    /// A single big-number KPI (`::METRIC`/`::MONEY`/`::PERCENT`/`::COMPACT`); an
    /// optional `::LABEL` is the caption.
    Metric(MetricFmt),
    /// A comparison value for a KPI (`::DELTA`) — shows the trend arrow + % change.
    Delta,
    /// A horizontal reference/target line on a chart (`::REFLINE`/`::YLINE`).
    RefLine,
    /// A vertical reference line at an x-position (`::XLINE`).
    VLine,
    /// Lower edge of a confidence band around a line (`::BAND_LOWER`).
    BandLower,
    /// Upper edge of a confidence band around a line (`::BAND_UPPER`).
    BandUpper,
    /// A trend arrow rendered inside a table cell (`::TREND`).
    Trend,
    /// Heatmap-colour a table column's cells by value (`::COLORSCALE`).
    ColorScale,
    /// Render a table column's text as coloured status pills (`::BADGE`).
    Badge,
    /// A count/metadata hint shown next to a dropdown option (`::HINT`).
    Hint,
    /// A single-value text card (`::TEXT_SMALL`/`_MEDIUM`/`_LARGE`).
    Text(TextSize),
    /// Layout: reserve an empty grid cell (`::PLACEHOLDER`).
    Placeholder,
    /// A banner image at the top of the dashboard (`::HEADER_IMAGE`).
    HeaderImage,
    /// A link shown at the bottom of the dashboard (`::FOOTER_LINK`).
    FooterLink,
    /// A download button for the query result (`::DOWNLOAD_CSV`/`_XLSX`/`_PDF`).
    Download(DownloadFmt),
    /// Auto-refresh interval in seconds (`::RELOAD`).
    Reload,
    /// A gauge's numeric range `min,max` (`::RANGE`).
    Range,
    /// Gauge zone labels, comma-separated (`::LABELS`).
    GaugeLabels,
    /// Gauge zone colours, comma-separated hex (`::COLORS`).
    GaugeColors,
    /// A WKT geometry column for a map choropleth (`::MAP`); coloured by the measure.
    Geometry,
    /// Layout: `::TAB` starts a new tab; following panels live under it.
    Tab,
    /// Layout: `::SUBTAB` starts a nested tab inside the current `::TAB`.
    SubTab,
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
        "LABEL" => Some(Role::Label),
        "TITLE" | "HEADING" => Some(Role::Title),
        "BARCHART" | "BAR" => Some(Role::Value(Kind::Bar)),
        "BARCHART_STACKED" | "BAR_STACKED" | "STACKED_BAR" => Some(Role::Value(Kind::BarStacked)),
        "BARCHART_PERCENT" | "BAR_PERCENT" => Some(Role::Value(Kind::BarPercent)),
        "BARCHART_STACKED_PERCENT" | "BAR_STACKED_PERCENT" => Some(Role::Value(Kind::BarStackedPercent)),
        "LINECHART" | "LINE" => Some(Role::Value(Kind::Line)),
        "LINECHART_PERCENT" | "LINE_PERCENT" => Some(Role::Value(Kind::LinePercent)),
        "AREACHART" | "AREA" => Some(Role::Value(Kind::Area)),
        "SCATTER" | "POINT" | "SCATTERCHART" => Some(Role::Value(Kind::Point)),
        "PIE" | "PIECHART" | "PIECHART_PERCENT" => Some(Role::Value(Kind::Pie)),
        "DONUT" | "DONUTCHART" | "DONUTCHART_PERCENT" => Some(Role::Value(Kind::Donut)),
        "GAUGE" | "GAUGE_PERCENT" => Some(Role::Value(Kind::Gauge)),
        "HISTOGRAM" | "HIST" => Some(Role::Value(Kind::Histogram)),
        "BOXPLOT" | "BOX_PLOT" => Some(Role::Value(Kind::Boxplot)),
        "HEATMAP" | "TILE" | "TILES" => Some(Role::Value(Kind::Heatmap)),
        "SPARKLINE" | "SPARK" => Some(Role::Value(Kind::Sparkline)),
        "REFLINE" | "TARGET" | "GOAL" | "YLINE" => Some(Role::RefLine),
        "XLINE" => Some(Role::VLine),
        "BAND_LOWER" | "BANDLOWER" => Some(Role::BandLower),
        "BAND_UPPER" | "BANDUPPER" => Some(Role::BandUpper),
        "TREND" => Some(Role::Trend),
        "COLORSCALE" | "COLOURSCALE" | "HEAT" | "GRADIENT" => Some(Role::ColorScale),
        "BADGE" | "STATUS" | "PILL" => Some(Role::Badge),
        "HINT" => Some(Role::Hint),
        "TEXT_SMALL" => Some(Role::Text(TextSize::Small)),
        "TEXT_MEDIUM" => Some(Role::Text(TextSize::Medium)),
        "TEXT_LARGE" => Some(Role::Text(TextSize::Large)),
        "PLACEHOLDER" => Some(Role::Placeholder),
        "HEADER_IMAGE" | "HEADERIMAGE" => Some(Role::HeaderImage),
        "FOOTER_LINK" | "FOOTERLINK" => Some(Role::FooterLink),
        "DOWNLOAD_CSV" => Some(Role::Download(DownloadFmt::Csv)),
        "DOWNLOAD_XLSX" | "DOWNLOAD_EXCEL" => Some(Role::Download(DownloadFmt::Xlsx)),
        "DOWNLOAD_PDF" => Some(Role::Download(DownloadFmt::Pdf)),
        "RELOAD" | "REFRESH" => Some(Role::Reload),
        "RANGE" => Some(Role::Range),
        "LABELS" => Some(Role::GaugeLabels),
        "COLORS" | "COLOURS" => Some(Role::GaugeColors),
        "MAP" | "GEOMETRY" | "GEO" | "CHOROPLETH" => Some(Role::Geometry),
        "TABLE" | "GRID" => Some(Role::Table),
        "PAGED" | "TABLE_PAGED" | "PAGINATED" => Some(Role::PagedTable),
        "METRIC" | "KPI" | "BIGNUMBER" => Some(Role::Metric(MetricFmt::Plain)),
        "MONEY" | "DOLLAR" | "CURRENCY" => Some(Role::Metric(MetricFmt::Money)),
        "PERCENT" | "PCT" => Some(Role::Metric(MetricFmt::Percent)),
        "COMPACT" => Some(Role::Metric(MetricFmt::Compact)),
        "DELTA" | "COMPARE" | "PREVIOUS" => Some(Role::Delta),
        "MULTISELECT" | "MULTI" => Some(Role::Input(InputKind::Multiselect)),
        "DATERANGE" | "DATE_RANGE" => Some(Role::Input(InputKind::DateRange)),
        "TAB" | "PAGE" => Some(Role::Tab),
        "SUBTAB" | "SUB_TAB" => Some(Role::SubTab),
        "DROPDOWN" | "OPTIONS" | "SELECT_INPUT" => Some(Role::Input(InputKind::Dropdown)),
        "NUMBER" | "SLIDER" | "NUMERIC" => Some(Role::Input(InputKind::Number)),
        "DATE" | "DATEPICKER" => Some(Role::Input(InputKind::Date)),
        "TEXT" | "SEARCH" | "STRING" => Some(Role::Input(InputKind::Text)),
        "COLUMNS" | "COLS" => Some(Role::Columns),
        "GROUP" | "BOX" | "ROW" => Some(Role::GroupStart),
        "ENDGROUP" | "ENDBOX" | "ENDROW" => Some(Role::GroupEnd),
        "SPAN" | "WIDTH" | "COL" => Some(Role::Span),
        "HEIGHT" | "TALL" => Some(Role::Height),
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

thread_local! {
    /// Per-render brand/primary colour override (e.g. `?primary=` in the UI, or a
    /// theme from an embedding host). `None` = the DataZoo default.
    static BRAND: std::cell::Cell<Option<(u8, u8, u8)>> = const { std::cell::Cell::new(None) };
}
/// Set the brand/primary colour for subsequent `render()` calls on this thread.
pub fn set_brand(color: Option<(u8, u8, u8)>) {
    BRAND.with(|b| b.set(color));
}
/// The active brand/primary colour (DataZoo steel by default).
fn brand() -> (u8, u8, u8) {
    BRAND.with(|b| b.get()).unwrap_or(DZ_COLORS[0])
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

    // A map is driven by a ::MAP (geometry) column, coloured by an optional measure.
    if cols.iter().any(|c| c.role == Role::Geometry) {
        return render_map(cols, title.as_deref(), width, height);
    }
    let value = cols.iter().find(|c| matches!(c.role, Role::Value(_)));
    let Some(value) = value else {
        // Label-only → a heading element.
        return Ok(heading_svg(title.as_deref().unwrap_or(""), width));
    };
    let Role::Value(kind) = value.role else { unreachable!() };
    match kind {
        Kind::Pie => return render_pie(value, cols, title.as_deref(), 0.0, width, height),
        Kind::Donut => return render_pie(value, cols, title.as_deref(), 0.55, width, height),
        Kind::Gauge => return render_gauge(value, cols, title.as_deref(), width, height),
        Kind::Histogram => return render_histogram(value, title.as_deref(), width, height),
        Kind::Heatmap => return render_heatmap(value, cols, title.as_deref(), width, height),
        Kind::Sparkline => return render_sparkline(value, width, height),
        _ => {}
    }
    let x = cols.iter().find(|c| c.role == Role::X).ok_or("no XAXIS column")?;
    let category = cols.iter().find(|c| c.role == Role::Category);

    let mut data: Vec<(String, Vec<Value>)> = vec![
        ("x".to_string(), x.values.clone()),
        ("y".to_string(), value.values.clone()),
    ];
    // Extra measure columns → additional overlaid layers (combo charts).
    let extras: Vec<&Column> = cols.iter().filter(|c| matches!(c.role, Role::Value(_))).skip(1).collect();
    for (k, ev) in extras.iter().enumerate() {
        data.push((format!("y{}", k + 2), ev.values.clone()));
    }
    let by_colour = matches!(kind, Kind::Line | Kind::LinePercent | Kind::Point);
    let bar = matches!(kind, Kind::Bar | Kind::BarStacked | Kind::BarPercent | Kind::BarStackedPercent);
    let percent = matches!(kind, Kind::BarPercent | Kind::BarStackedPercent | Kind::LinePercent);
    let x_discrete = x.values.iter().any(|v| matches!(v, Value::Str(_)));

    // Optional confidence band around a line (`::BAND_LOWER`/`::BAND_UPPER`).
    let band_lo = cols.iter().find(|c| c.role == Role::BandLower);
    let band_hi = cols.iter().find(|c| c.role == Role::BandUpper);
    if let (Some(lo), Some(hi)) = (band_lo, band_hi) {
        data.push(("bandlo".to_string(), lo.values.clone()));
        data.push(("bandhi".to_string(), hi.values.clone()));
    }
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
    // The band is drawn first so the line sits on top of it.
    if band_lo.is_some() && band_hi.is_some() {
        plot = plot
            .geom_ribbon_with(GeomRibbon { fill: lighten(brand(), 0.5), alpha: 0.4 })
            .layer_aes(Aes::new().x("x").ymin("bandlo").ymax("bandhi"));
    }
    plot = match kind {
        Kind::Bar | Kind::BarPercent => plot.geom_col().position(PositionDodge),
        Kind::BarStacked => plot.geom_col().position(PositionStack),
        Kind::BarStackedPercent => plot.geom_col().position(PositionFill),
        // Lines/areas also get point markers — they carry the per-point `<title>`
        // so every chart is hoverable (and clickable for linking).
        Kind::Line | Kind::LinePercent => plot.geom_line().geom_point(),
        Kind::Area => plot.geom_area().geom_point(),
        Kind::Point => plot.geom_point(),
        Kind::Boxplot => plot.geom_boxplot(),
        Kind::Pie | Kind::Donut | Kind::Gauge | Kind::Histogram | Kind::Heatmap | Kind::Sparkline => {
            unreachable!("handled above")
        }
    };
    // Combo layers: overlay each extra measure with its own geom + y column.
    for (k, ev) in extras.iter().enumerate() {
        if let Role::Value(ekind) = ev.role {
            let yk = format!("y{}", k + 2);
            plot = match ekind {
                Kind::Line => plot.geom_line(),
                Kind::Area => plot.geom_area(),
                Kind::Point => plot.geom_point(),
                _ => plot.geom_col(),
            }
            .layer_aes(Aes::new().x("x").y(&yk));
        }
    }
    // Horizontal reference/target line (`::REFLINE`/`::YLINE`).
    if let Some(rl) = cols.iter().find(|c| c.role == Role::RefLine) {
        if let Some(v) = rl.values.iter().find_map(|x| x.as_f64()) {
            plot = plot.geom_hline(v);
        }
    }
    // Vertical reference line (`::XLINE`) — only meaningful on a continuous x.
    if let Some(vl) = cols.iter().find(|c| c.role == Role::VLine) {
        if let Some(v) = vl.values.iter().find_map(|x| x.as_f64()) {
            plot = plot.geom_vline(v);
        }
    }
    if let Some(levels) = &color_levels {
        // Explicit hex values (`#rrggbb`) are used verbatim; otherwise the levels
        // map to the DataZoo palette in stable/sorted order so a given series is
        // the same colour in every chart.
        let pairs: Vec<(&str, ggplot_rs::scale::color::RGBAColor)> = levels
            .iter()
            .enumerate()
            .map(|(i, s)| (s.as_str(), parse_hex(s).unwrap_or_else(|| dz_color(i))))
            .collect();
        plot = if by_colour { plot.scale_color_manual(pairs) } else { plot.scale_fill_manual(pairs) };
    }
    if x_coloured {
        plot = plot.show_legend(false); // the x axis already labels the colours
    }
    if percent {
        plot = plot.scale_y_continuous(
            ggplot_rs::scale::continuous::ScaleContinuous::new()
                .with_label_formatter(ggplot_rs::scale::format::label_percent),
        );
    }
    // DataZoo steel blue for single-series marks. Set the primary AFTER the theme
    // preset — presets replace the whole theme.
    plot = plot
        .theme_minimal()
        .primary_color(brand())
        .legend_position(ggplot_rs::theme::LegendPosition::Top);
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
        .primary_color(brand());
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
            ggplot_rs::scale::color::RGBAColor::new(brand().0, brand().1, brand().2),
        )
        .theme_minimal();
    if let Some(t) = title {
        plot = plot.title(t);
    }
    plot.render_svg_native_with_size(width, height)
        .map_err(|e| format!("render failed: {e:?}"))
}

/// A minimal inline trend line (no axes) — a sparkline over the row order: a
/// light steel area under a crisp line, with a marker on the latest value.
fn render_sparkline(value: &Column, width: u32, height: u32) -> Result<String, String> {
    let n = value.values.len();
    let xs: Vec<Value> = (0..n).map(|i| Value::Float(i as f64)).collect();
    let data = vec![("x".to_string(), xs), ("y".to_string(), value.values.clone())];

    let steel = brand();
    let fill = lighten(steel, 0.72); // pale wash under the line
    let last = n.saturating_sub(1);
    // A single-point layer marking the most recent value (the eye-catching dot).
    let end = vec![
        ("x".to_string(), vec![Value::Float(last as f64)]),
        ("y".to_string(), vec![value.values.get(last).cloned().unwrap_or(Value::Na)]),
    ];

    GGPlot::new(data)
        .aes(Aes::new().x("x").y("y"))
        .geom_area_with(GeomArea { fill, color: fill, alpha: 0.5, line_width: 0.0 })
        .geom_line_with(GeomLine { color: steel, width: 2.0, alpha: 1.0 })
        .geom_point_with(GeomPoint { size: 3.2, color: DZ_COLORS[2], alpha: 1.0 })
        .layer_data(end) // restrict the point layer to just the endpoint
        .scale_y_continuous(ggplot_rs::scale::continuous::ScaleContinuous::new().with_expand(0.1, 0.0))
        .theme_void()
        .render_svg_native_with_size(width, height)
        .map_err(|e| format!("render failed: {e:?}"))
}

/// Blend a colour toward white by `t` (0 = unchanged, 1 = white).
fn lighten((r, g, b): (u8, u8, u8), t: f64) -> (u8, u8, u8) {
    let f = |c: u8| (c as f64 + (255.0 - c as f64) * t).round() as u8;
    (f(r), f(g), f(b))
}

/// Parse a `#rrggbb` / `rrggbb` string into an RGBA colour (`None` otherwise).
fn parse_hex(s: &str) -> Option<ggplot_rs::scale::color::RGBAColor> {
    let h = s.trim().strip_prefix('#').unwrap_or(s.trim());
    if h.len() != 6 || !h.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let p = |a, b| u8::from_str_radix(&h[a..b], 16).ok();
    Some(ggplot_rs::scale::color::RGBAColor::new(p(0, 2)?, p(2, 4)?, p(4, 6)?))
}

/// A gauge: a 270° arc showing a single value's progress through a `min,max`
/// `::RANGE` (default `0,100`). Optional `::COLORS` paints threshold zones.
fn render_gauge(value: &Column, cols: &[Column], title: Option<&str>, width: u32, height: u32) -> Result<String, String> {
    let val = value.values.iter().find_map(|v| v.as_f64()).unwrap_or(0.0);
    // Range "min,max" (default 0..100).
    let (min, max) = cols
        .iter()
        .find(|c| c.role == Role::Range)
        .and_then(|c| c.values.first())
        .map(value_str)
        .and_then(|s| {
            let p: Vec<f64> = s.split(',').filter_map(|t| t.trim().parse().ok()).collect();
            (p.len() == 2).then_some((p[0], p[1]))
        })
        .unwrap_or((0.0, 100.0));
    let span = if (max - min).abs() < 1e-9 { 1.0 } else { max - min };
    let frac = ((val - min) / span).clamp(0.0, 1.0);

    // Optional zone colours (comma-separated hex); default single steel arc.
    let zone_cols: Vec<ggplot_rs::scale::color::RGBAColor> = cols
        .iter()
        .find(|c| c.role == Role::GaugeColors)
        .and_then(|c| c.values.first())
        .map(value_str)
        .map(|s| s.split(',').filter_map(parse_hex).collect())
        .unwrap_or_default();

    // Geometry: a 270° arc (135° … 405°), opening downward, centred.
    let w = width as f64;
    let h = height as f64;
    let cx = w / 2.0;
    let title_pad = if title.is_some() { 12.0 } else { 0.0 };
    let cy = h * 0.55 + title_pad;
    let r = (w * 0.30).min(h * 0.42).max(20.0);
    let thick = r * 0.16;
    let start = 135.0_f64.to_radians();
    let sweep = 270.0_f64.to_radians();
    let pt = |frac: f64, rad: f64| {
        let a = start + sweep * frac;
        (cx + rad * a.cos(), cy + rad * a.sin())
    };
    let arc = |f0: f64, f1: f64, col: &str, wdt: f64| {
        let (x0, y0) = pt(f0, r);
        let (x1, y1) = pt(f1, r);
        let large = if (f1 - f0) * 270.0 > 180.0 { 1 } else { 0 };
        format!(
            "<path d=\"M {x0:.1} {y0:.1} A {r:.1} {r:.1} 0 {large} 1 {x1:.1} {y1:.1}\" \
             fill=\"none\" stroke=\"{col}\" stroke-width=\"{wdt:.1}\" stroke-linecap=\"round\"/>"
        )
    };
    // The value arc takes the colour of the zone the value falls into (a
    // traffic-light gauge); a single steel arc when no ::COLORS are given.
    let (sr, sg, sb) = brand();
    let vcol = if zone_cols.is_empty() {
        format!("rgb({sr},{sg},{sb})")
    } else {
        let zi = ((frac * zone_cols.len() as f64).floor() as usize).min(zone_cols.len() - 1);
        let c = zone_cols[zi];
        format!("rgb({},{},{})", c.r, c.g, c.b)
    };
    let (tr, tg, tb) = lighten(brand(), 0.87);
    let track = format!("rgb({tr},{tg},{tb})");
    let esc = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;");
    let mut body = String::new();
    // Light full-arc track, then the value arc on top.
    body.push_str(&arc(0.0, 1.0, &track, thick));
    body.push_str(&arc(0.0, frac.max(0.001), &vcol, thick));
    // Zone-boundary ticks across the arc.
    if zone_cols.len() > 1 {
        for i in 1..zone_cols.len() {
            let f = i as f64 / zone_cols.len() as f64;
            let (x0, y0) = pt(f, r - thick * 0.75);
            let (x1, y1) = pt(f, r + thick * 0.75);
            body.push_str(&format!(
                "<line x1=\"{x0:.1}\" y1=\"{y0:.1}\" x2=\"{x1:.1}\" y2=\"{y1:.1}\" stroke=\"#fff\" stroke-width=\"2\"/>"
            ));
        }
    }
    // A marker dot at the current value.
    let (dx, dy) = pt(frac, r);
    body.push_str(&format!(
        "<circle cx=\"{dx:.1}\" cy=\"{dy:.1}\" r=\"{:.1}\" fill=\"{vcol}\" stroke=\"#fff\" stroke-width=\"2.5\"/>",
        thick * 0.6
    ));
    // Big value + "of max" caption.
    let num = if (val - val.round()).abs() < 1e-9 { format!("{}", val.round() as i64) } else { format!("{val:.1}") };
    body.push_str(&format!(
        "<text x=\"{cx:.1}\" y=\"{:.1}\" text-anchor=\"middle\" font-family=\"system-ui,sans-serif\" \
         font-size=\"{:.0}\" font-weight=\"800\" fill=\"#1f2937\">{num}</text>",
        cy - r * 0.02,
        r * 0.5
    ));
    body.push_str(&format!(
        "<text x=\"{cx:.1}\" y=\"{:.1}\" text-anchor=\"middle\" font-family=\"system-ui,sans-serif\" font-size=\"11\" font-weight=\"600\" fill=\"#8a93a6\">of {}</text>",
        cy + r * 0.24,
        esc(&fmt_g(max))
    ));
    // Min / max labels at the arc ends.
    let (minx, miny) = pt(0.0, r);
    let (maxx, maxy) = pt(1.0, r);
    body.push_str(&format!(
        "<text x=\"{:.1}\" y=\"{:.1}\" text-anchor=\"middle\" font-family=\"system-ui,sans-serif\" font-size=\"11\" fill=\"#8a93a6\">{}</text>\
         <text x=\"{:.1}\" y=\"{:.1}\" text-anchor=\"middle\" font-family=\"system-ui,sans-serif\" font-size=\"11\" fill=\"#8a93a6\">{}</text>",
        minx, miny + 15.0, esc(&fmt_g(min)),
        maxx, maxy + 15.0, esc(&fmt_g(max)),
    ));
    if let Some(t) = title {
        body.push_str(&format!(
            "<text x=\"{cx:.1}\" y=\"20\" text-anchor=\"middle\" font-family=\"system-ui,sans-serif\" font-size=\"14\" font-weight=\"700\" fill=\"#1f2430\">{}</text>",
            esc(t)
        ));
    }
    Ok(format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"{height}\" viewBox=\"0 0 {width} {height}\">{body}</svg>"
    ))
}

/// Compact number formatting for gauge range labels.
fn fmt_g(v: f64) -> String {
    if (v - v.round()).abs() < 1e-9 {
        format!("{}", v.round() as i64)
    } else {
        format!("{v:.1}")
    }
}

/// A choropleth map from a WKT `::MAP` geometry column, optionally coloured by a
/// measure (light → steel blue).
fn render_map(cols: &[Column], _title: Option<&str>, width: u32, height: u32) -> Result<String, String> {
    let geom = cols.iter().find(|c| c.role == Role::Geometry).ok_or("map needs a ::MAP column")?;
    let fill = cols.iter().find(|c| matches!(c.role, Role::Value(_)));
    let label = cols.iter().find(|c| c.role == Role::Label);
    let mut data: Vec<(String, Vec<Value>)> = vec![("geometry".to_string(), geom.values.clone())];
    let mut aes = Aes::new();
    if let Some(f) = fill {
        data.push(("fill".to_string(), f.values.clone()));
        aes = aes.fill("fill");
    }
    let lab = label.map(|l| l.values.clone()).or_else(|| fill.map(|f| f.values.clone()));
    if let Some(lv) = lab {
        data.push(("label".to_string(), lv));
        aes = aes.label("label");
    }
    let mut plot = GGPlot::new(data).aes(aes).geom_sf().coord_sf().theme_void();
    if fill.is_some() {
        plot = plot.scale_fill_gradient(
            ggplot_rs::scale::color::RGBAColor::new(0xed, 0xf1, 0xf7),
            ggplot_rs::scale::color::RGBAColor::new(brand().0, brand().1, brand().2),
        );
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
    inner: f64,
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
    let pairs: Vec<(&str, ggplot_rs::scale::color::RGBAColor)> = levels
        .iter()
        .enumerate()
        .map(|(i, s)| (s.as_str(), parse_hex(s).unwrap_or_else(|| dz_color(i))))
        .collect();
    let mut plot = GGPlot::new(data)
        .aes(Aes::new().x("x").y("y").fill("cat").label("label"))
        .geom_col()
        .position(PositionStack)
        .scale_fill_manual(pairs)
        // No y-axis padding, so the stack maps to a full 360° (closes the pie).
        .scale_y_continuous(ggplot_rs::scale::continuous::ScaleContinuous::new().with_expand(0.0, 0.0))
        .coord_polar_with(ggplot_rs::coord::polar::CoordPolar::new().theta("y").inner_radius(inner))
        .theme_void()
        .legend_position(ggplot_rs::theme::LegendPosition::Top);
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
#[allow(clippy::not_unsafe_ptr_arg_deref)]
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
