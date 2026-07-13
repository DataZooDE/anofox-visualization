//! anofox-visualization — SQL-defined dashboards, Shaper-style.
//!
//! You annotate SQL result columns with *roles* (`XAXIS`, `CATEGORY`, `LABEL`,
//! and a chart kind on the value column such as `BARCHART`/`LINECHART`), and this
//! crate maps that annotated result set onto [`ggplot-rs`](ggplot_rs) and renders
//! an SVG. The core here is dependency-light and wasm-compatible — the DuckDB
//! extension packaging (native + wasm) sits on top and calls [`render`].
//!
//! ```
//! use anofox_visualization::{Column, Role, Kind, render};
//! use ggplot_rs::prelude::Value;
//! let cols = vec![
//!     Column::new("week", Role::X, vec![Value::Str("W1".into()), Value::Str("W2".into())]),
//!     Column::new("n", Role::Value(Kind::Bar), vec![Value::Float(3.0), Value::Float(7.0)]),
//! ];
//! let svg = render(&cols, 480, 320).unwrap();
//! assert!(svg.contains("<svg"));
//! ```

use ggplot_rs::prelude::*;

pub mod dashboard;
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
    /// A step line (`::STEP`).
    Step,
    /// A scatter with a smoothed (LOESS) trend line (`::SMOOTH`).
    Smooth,
    Area,
    /// Stacked area — bands stacked per x by `CATEGORY` (`::AREA_STACKED`).
    AreaStacked,
    Point,
    /// A pie — slices by `CATEGORY`, sized by the measure (`::PIE`).
    Pie,
    /// A donut (pie with a centre hole) (`::DONUTCHART`).
    Donut,
    /// A histogram of the measure column (`::HISTOGRAM`).
    Histogram,
    /// A box plot — `x` groups, `y` = the measure (`::BOXPLOT`).
    Boxplot,
    /// A violin plot — `x` groups, `y` = the measure (`::VIOLIN`).
    Violin,
    /// A kernel-density curve of the measure column (`::DENSITY`).
    Density,
    /// A normal quantile-quantile plot of the measure column (`::QQ`).
    QQ,
    /// A heatmap — `x` × `y` tiles coloured by the measure (`::HEATMAP`).
    Heatmap,
    /// A minimal inline trend line, no axes (`::SPARKLINE`).
    Sparkline,
    /// A single value as a gauge/progress arc toward a `::RANGE` (`::GAUGE`).
    Gauge,
    /// A GitHub-style calendar heatmap — a date `::XAXIS` laid out as weeks ×
    /// weekdays, coloured by the measure (`::CALENDAR`).
    Calendar,
    /// A scatter with jittered positions to reveal overlapping points (`::JITTER`).
    Jitter,
    /// An OHLC candlestick chart — `::XAXIS` + `::OPEN`/`::HIGH`/`::LOW` columns and
    /// the close as the measure (`::CANDLESTICK`).
    Candlestick,
    /// A radar / spider chart — axes from `::XAXIS`, values as `::RADAR`, one
    /// polygon per `::CATEGORY` series.
    Radar,
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
    /// A plain table column with no in-cell bar (`::PLAIN`/`::NOBAR`).
    Plain,
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
    /// A second WKT geometry column drawn as a faint grey backdrop under the
    /// `::MAP` layer (`::BASEMAP`) — e.g. country outlines behind quake points.
    Basemap,
    /// Flip the panel's x/y axes — a horizontal bar chart etc. (`::FLIP`). A
    /// marker column; its values are ignored.
    Flip,
    /// Layer opacity 0..1 for a `::MAP` (`::ALPHA`) — e.g. semi-transparent
    /// earthquake points so overlaps read as density. Read from the first value.
    Alpha,
    /// Layout: `::TAB` starts a new tab; following panels live under it.
    Tab,
    /// Layout: `::SUBTAB` starts a nested tab inside the current `::TAB`.
    SubTab,
    /// Bubble size for a scatter — maps a measure to point area (`::SIZE`).
    Size,
    /// Format the y-axis tick labels (`::YFORMAT`) — the column's (string) value
    /// is a currency symbol / keyword ("$", "€", "comma", "percent"…).
    YFormat,
    /// Format the x-axis tick labels (`::XFORMAT`), like [`Role::YFormat`].
    XFormat,
    /// Draw the value on each mark as a text label (`::DATALABELS`). A marker
    /// column; its values (if any) are ignored — the measure is labelled.
    DataLabels,
    /// Shade a vertical x-region behind the data (`::MARKAREA`): the band spans
    /// [min, max] of this column's (non-null) x-values.
    MarkArea,
    /// A rich-text panel whose (string) value is rendered as Markdown
    /// (`::MARKDOWN`/`::MD`). Presentation-only — handled by the browser.
    Markdown,
    /// Candlestick open price (`::OPEN`).
    Open,
    /// Candlestick high price (`::HIGH`).
    High,
    /// Candlestick low price (`::LOW`).
    Low,
}

/// A single annotated result column: a name, its [`Role`], and its values.
pub struct Column {
    pub name: String,
    pub role: Role,
    pub values: Vec<Value>,
}

impl Column {
    pub fn new(name: impl Into<String>, role: Role, values: Vec<Value>) -> Self {
        Column {
            name: name.into(),
            role,
            values,
        }
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
        "BARCHART_STACKED_PERCENT" | "BAR_STACKED_PERCENT" => {
            Some(Role::Value(Kind::BarStackedPercent))
        }
        "LINECHART" | "LINE" => Some(Role::Value(Kind::Line)),
        "LINECHART_PERCENT" | "LINE_PERCENT" => Some(Role::Value(Kind::LinePercent)),
        "STEP" | "STEPLINE" | "STEP_LINE" => Some(Role::Value(Kind::Step)),
        "SMOOTH" | "TRENDLINE" | "TREND_LINE" => Some(Role::Value(Kind::Smooth)),
        "AREACHART" | "AREA" => Some(Role::Value(Kind::Area)),
        "AREACHART_STACKED" | "AREA_STACKED" | "STACKED_AREA" => {
            Some(Role::Value(Kind::AreaStacked))
        }
        "SCATTER" | "POINT" | "SCATTERCHART" => Some(Role::Value(Kind::Point)),
        "JITTER" | "JITTERCHART" | "STRIP" => Some(Role::Value(Kind::Jitter)),
        "CANDLESTICK" | "CANDLE" | "OHLC" => Some(Role::Value(Kind::Candlestick)),
        "RADAR" | "SPIDER" => Some(Role::Value(Kind::Radar)),
        "OPEN" => Some(Role::Open),
        "HIGH" => Some(Role::High),
        "LOW" => Some(Role::Low),
        "SIZE" | "BUBBLE" => Some(Role::Size),
        "DATALABELS" | "DATALABEL" | "VALUELABELS" | "SHOWLABELS" => Some(Role::DataLabels),
        "MARKAREA" | "MARK_AREA" | "SHADE" => Some(Role::MarkArea),
        "MARKDOWN" | "MD" | "TEXTBOX" | "RICHTEXT" => Some(Role::Markdown),
        "PIE" | "PIECHART" | "PIECHART_PERCENT" => Some(Role::Value(Kind::Pie)),
        "DONUT" | "DONUTCHART" | "DONUTCHART_PERCENT" => Some(Role::Value(Kind::Donut)),
        "GAUGE" | "GAUGE_PERCENT" => Some(Role::Value(Kind::Gauge)),
        "HISTOGRAM" | "HIST" => Some(Role::Value(Kind::Histogram)),
        "BOXPLOT" | "BOX_PLOT" => Some(Role::Value(Kind::Boxplot)),
        "VIOLIN" | "VIOLINPLOT" => Some(Role::Value(Kind::Violin)),
        "DENSITY" | "KDE" => Some(Role::Value(Kind::Density)),
        "QQ" | "QQPLOT" => Some(Role::Value(Kind::QQ)),
        "HEATMAP" | "TILE" | "TILES" => Some(Role::Value(Kind::Heatmap)),
        "CALENDAR" | "CALENDAR_HEATMAP" | "CAL_HEATMAP" => Some(Role::Value(Kind::Calendar)),
        "SPARKLINE" | "SPARK" => Some(Role::Value(Kind::Sparkline)),
        "REFLINE" | "TARGET" | "GOAL" | "YLINE" => Some(Role::RefLine),
        "XLINE" => Some(Role::VLine),
        "BAND_LOWER" | "BANDLOWER" => Some(Role::BandLower),
        "BAND_UPPER" | "BANDUPPER" => Some(Role::BandUpper),
        "TREND" => Some(Role::Trend),
        "COLORSCALE" | "COLOURSCALE" | "HEAT" | "GRADIENT" => Some(Role::ColorScale),
        "BADGE" | "STATUS" | "PILL" => Some(Role::Badge),
        "PLAIN" | "NOBAR" => Some(Role::Plain),
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
        "BASEMAP" | "MAPBASE" | "BACKDROP" => Some(Role::Basemap),
        "FLIP" | "COORD_FLIP" | "HORIZONTAL" => Some(Role::Flip),
        "YFORMAT" | "YAXISFORMAT" | "YUNIT" | "YCURRENCY" => Some(Role::YFormat),
        "XFORMAT" | "XAXISFORMAT" | "XUNIT" | "XCURRENCY" => Some(Role::XFormat),
        "ALPHA" | "OPACITY" => Some(Role::Alpha),
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
        _ => v
            .as_f64()
            .map(|f| format!("{}", (f * 1000.0).round() / 1000.0))
            .unwrap_or_default(),
    }
}

/// The DataZoo palette — anofox-visualization's default categorical + single-series colours.
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

/// A map zoom window: `((x0, x1), (y0, y1))` in geometry (lon/lat) coordinates.
pub type ZoomWindow = ((f64, f64), (f64, f64));

thread_local! {
    /// Per-render map zoom window. `None` = the default equal-aspect `coord_sf` fit.
    static PANEL_ZOOM: std::cell::Cell<Option<ZoomWindow>> = const { std::cell::Cell::new(None) };
}
/// Set the map zoom window for the next `render()` (a `::MAP` panel). `None`
/// restores the auto-fit view.
pub fn set_panel_zoom(window: Option<ZoomWindow>) {
    PANEL_ZOOM.with(|z| z.set(window));
}
fn panel_zoom() -> Option<ZoomWindow> {
    PANEL_ZOOM.with(|z| z.get())
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

/// Render a panel from a single JSON spec — the entry point used by hosts that
/// pass everything as one string (the DuckDB extension, CLI, services). The spec:
/// `{"rows":[{col:val,…},…], "roles":[[idx,"ROLE","name"],…], "width":W,
/// "height":H, "primary":"rrggbb"}`. Returns the SVG (or `<pre>error</pre>`).
pub fn render_spec(spec_json: &str) -> String {
    let spec: serde_json::Value = match serde_json::from_str(spec_json) {
        Ok(v) => v,
        Err(e) => return format!("<pre>bad spec JSON: {e}</pre>"),
    };
    let rows: Vec<serde_json::Map<String, serde_json::Value>> = spec
        .get("rows")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();
    let entries: Vec<(usize, String, String)> = spec
        .get("roles")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|e| {
                    let e = e.as_array()?;
                    let i = e.first()?.as_u64()? as usize;
                    let role = e.get(1)?.as_str()?.to_string();
                    let name = e.get(2).and_then(|v| v.as_str()).unwrap_or("").to_string();
                    Some((i, role, name))
                })
                .collect()
        })
        .unwrap_or_default();
    let roles: Vec<(usize, Role)> = entries
        .iter()
        .filter_map(|(i, s, _)| parse_role(s).map(|r| (*i, r)))
        .collect();
    let mut cols = sql::columns_from_rows(&rows, &roles);
    for (i, _, name) in &entries {
        if !name.is_empty() {
            if let Some(c) = cols.iter_mut().find(|c| c.name == format!("c{i}")) {
                c.name = name.clone();
            }
        }
    }
    let width = spec.get("width").and_then(|v| v.as_u64()).unwrap_or(640) as u32;
    let height = spec.get("height").and_then(|v| v.as_u64()).unwrap_or(400) as u32;
    let primed = spec
        .get("primary")
        .and_then(|v| v.as_str())
        .map(|p| p.trim().trim_start_matches('#').to_string())
        .filter(|h| h.len() == 6 && h.chars().all(|c| c.is_ascii_hexdigit()));
    if let Some(h) = &primed {
        let px = |a, b| u8::from_str_radix(&h[a..b], 16).unwrap_or(0);
        set_brand(Some((px(0, 2), px(2, 4), px(4, 6))));
    }
    let svg = render(&cols, width, height).unwrap_or_else(|e| format!("<pre>{e}</pre>"));
    if primed.is_some() {
        set_brand(None);
    }
    svg
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
    let Role::Value(kind) = value.role else {
        unreachable!()
    };
    match kind {
        Kind::Pie => return render_pie(value, cols, title.as_deref(), 0.0, width, height),
        Kind::Donut => return render_pie(value, cols, title.as_deref(), 0.55, width, height),
        Kind::Gauge => return render_gauge(value, cols, title.as_deref(), width, height),
        Kind::Histogram => return render_histogram(value, title.as_deref(), width, height),
        Kind::Density => return render_density(value, cols, title.as_deref(), width, height),
        Kind::QQ => return render_qq(value, title.as_deref(), width, height),
        Kind::Heatmap => return render_heatmap(value, cols, title.as_deref(), width, height),
        Kind::Calendar => return render_calendar(value, cols, width, height),
        Kind::Candlestick => {
            return render_candlestick(value, cols, title.as_deref(), width, height)
        }
        Kind::Radar => return render_radar(value, cols, title.as_deref(), width, height),
        Kind::Sparkline => return render_sparkline(value, width, height),
        _ => {}
    }
    let x = cols
        .iter()
        .find(|c| c.role == Role::X)
        .ok_or("no XAXIS column")?;
    let category = cols.iter().find(|c| c.role == Role::Category);

    let mut data: Vec<(String, Vec<Value>)> = vec![
        ("x".to_string(), x.values.clone()),
        ("y".to_string(), value.values.clone()),
    ];
    // Extra measure columns → additional overlaid layers (combo charts).
    let extras: Vec<&Column> = cols
        .iter()
        .filter(|c| matches!(c.role, Role::Value(_)))
        .skip(1)
        .collect();
    for (k, ev) in extras.iter().enumerate() {
        data.push((format!("y{}", k + 2), ev.values.clone()));
    }
    let by_colour = matches!(
        kind,
        Kind::Line | Kind::LinePercent | Kind::Point | Kind::Step | Kind::Smooth | Kind::Jitter
    );
    let bar = matches!(
        kind,
        Kind::Bar | Kind::BarStacked | Kind::BarPercent | Kind::BarStackedPercent
    );
    let percent = matches!(
        kind,
        Kind::BarPercent | Kind::BarStackedPercent | Kind::LinePercent
    );
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
        aes = if by_colour {
            aes.color("cat")
        } else {
            aes.fill("cat")
        };
        Some(distinct_labels(cat))
    } else if bar && x_discrete {
        aes = aes.fill("x");
        x_coloured = true;
        Some(distinct_labels(x))
    } else {
        None
    };
    // Richer hover: label each mark with its series. The geom appends the value,
    // so a stacked-bar segment reads e.g. "web: 22". Tooltip-only — not drawn.
    // - a CATEGORY names the series;
    // - a discrete x (bars) names the group, e.g. "app: 22";
    // - a continuous x (line/scatter) uses the measure name, so the point-hover
    //   tooltip reads "sales: 68.2" rather than "6: 68.2" (which duplicates the
    //   x already shown in the axis-pointer header).
    let label_vals = if let Some(cat) = category {
        cat.values.clone()
    } else if x_discrete {
        x.values.clone()
    } else {
        let name = match value.name.as_str() {
            n if n.is_empty()
                || (n.starts_with('c') && n[1..].chars().all(|c| c.is_ascii_digit())) =>
            {
                "value".to_string()
            }
            n => n.to_string(),
        };
        vec![Value::Str(name); value.values.len()]
    };
    data.push(("label".to_string(), label_vals));
    aes = aes.label("label");

    // Bubble scatter: a `::SIZE` measure maps to point area.
    if let Some(sz) = cols.iter().find(|c| c.role == Role::Size) {
        data.push(("size".to_string(), sz.values.clone()));
        aes = aes.size("size");
    }
    // Data labels (`::DATALABELS`): the measure value drawn above each mark. The
    // label column's (first numeric) value, if any, sets the font size — e.g.
    // `14::DATALABELS` — otherwise a readable default.
    let datalabels = cols.iter().find(|c| c.role == Role::DataLabels);
    let show_labels = datalabels.is_some();
    let dlabel_size = datalabels
        .and_then(|c| c.values.iter().find_map(|v| v.as_f64()))
        .filter(|s| *s >= 5.0 && *s <= 40.0)
        .unwrap_or(11.0);
    if show_labels {
        let dl: Vec<Value> = value
            .values
            .iter()
            .map(|v| Value::Str(fmt_label(v)))
            .collect();
        data.push(("dlab".to_string(), dl));
    }

    // Multi-measure combo with no explicit CATEGORY (e.g. observed+trend,
    // actual+predicted, or a line with a changepoint-point overlay): colour each
    // measure distinctly and name it in the legend by its column header, so the
    // series are separable. The default single-series theming would otherwise
    // paint every measure the same brand colour.
    let combo_names: Vec<String> = if category.is_none() && !extras.is_empty() && !bar {
        let n = value.values.len();
        let name_of = |c: &Column, i: usize| {
            if c.name.is_empty() {
                format!("series {}", i + 1)
            } else {
                c.name.clone()
            }
        };
        let mut names = vec![name_of(value, 0)];
        for ev in &extras {
            let idx = names.len();
            names.push(name_of(ev, idx));
        }
        for (k, nm) in names.iter().enumerate() {
            data.push((format!("__s{k}"), vec![Value::Str(nm.clone()); n]));
        }
        aes = aes.color("__s0");
        names
    } else {
        Vec::new()
    };

    let mut plot = GGPlot::new(data).aes(aes);

    // Shaded x-region (`::MARKAREA`): a light band behind the data spanning
    // [min, max] of the mark column's x-values, full plot height.
    if let Some(ma) = cols.iter().find(|c| c.role == Role::MarkArea) {
        let key = |v: &&Value| v.as_f64();
        let lo = ma
            .values
            .iter()
            .filter(|v| v.as_f64().is_some())
            .min_by(|a, b| {
                key(a)
                    .partial_cmp(&key(b))
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        let hi = ma
            .values
            .iter()
            .filter(|v| v.as_f64().is_some())
            .max_by(|a, b| {
                key(a)
                    .partial_cmp(&key(b))
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        let ys: Vec<f64> = value
            .values
            .iter()
            .chain(extras.iter().flat_map(|e| e.values.iter()))
            .filter_map(|v| v.as_f64())
            .collect();
        if let (Some(x0), Some(x1)) = (lo, hi) {
            let ymin = ys.iter().cloned().fold(f64::INFINITY, f64::min);
            let ymax = ys.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
            if ymin.is_finite() && ymax.is_finite() {
                let frame = vec![
                    ("mx0".to_string(), vec![x0.clone()]),
                    ("mx1".to_string(), vec![x1.clone()]),
                    ("my0".to_string(), vec![Value::Float(ymin)]),
                    ("my1".to_string(), vec![Value::Float(ymax)]),
                ];
                plot = plot
                    .geom_rect_with(GeomRect {
                        fill: (148, 160, 178),
                        color: (148, 160, 178),
                        alpha: 0.14,
                        line_width: 0.0,
                    })
                    .layer_data(frame)
                    .layer_aes(Aes::new().xmin("mx0").xmax("mx1").ymin("my0").ymax("my1"));
            }
        }
    }

    // A ::BAND (prediction interval) matches the forecast — the last coloured
    // series — at half opacity, so it reads as that series' uncertainty.
    let band_color: (u8, u8, u8) = color_levels
        .as_ref()
        .filter(|lv| !lv.is_empty())
        .map(|lv| {
            let i = lv.len() - 1;
            parse_hex(&lv[i])
                .map(|c| (c.r, c.g, c.b))
                .unwrap_or(DZ_COLORS[i % DZ_COLORS.len()])
        })
        .unwrap_or_else(brand);
    // The band is drawn first so the line sits on top of it.
    if band_lo.is_some() && band_hi.is_some() {
        plot = plot
            .geom_ribbon_with(GeomRibbon {
                fill: band_color,
                alpha: 0.25,
            })
            .layer_aes(Aes::new().x("x").ymin("bandlo").ymax("bandhi"));
    }
    // Slimmer line + smaller markers so dense series (e.g. a monthly forecast)
    // don't get swamped by the dots.
    let thin_line = || GeomLine {
        width: 1.0,
        ..Default::default()
    };
    let small_point = || GeomPoint {
        size: 1.8,
        ..Default::default()
    };
    plot = match kind {
        Kind::Bar | Kind::BarPercent => plot.geom_col().position(PositionDodge),
        Kind::BarStacked => plot.geom_col().position(PositionStack),
        Kind::BarStackedPercent => plot.geom_col().position(PositionFill),
        // Lines/areas also get point markers — they carry the per-point `<title>`
        // so every chart is hoverable (and clickable for linking).
        Kind::Line | Kind::LinePercent => plot
            .geom_line_with(thin_line())
            .geom_point_with(small_point()),
        Kind::Step => plot
            .geom_step_with(ggplot_rs::geom::step::GeomStep {
                width: 1.2,
                ..Default::default()
            })
            .geom_point_with(small_point()),
        // Scatter + a LOESS trend line (no CI ribbon) — an analytical "smooth".
        Kind::Smooth => plot
            .geom_point_with(small_point())
            .geom_smooth_with(GeomSmooth {
                se: false,
                line_width: 2.0,
                method: ggplot_rs::stat::smooth::SmoothMethod::Loess { span: 0.75 },
                ..Default::default()
            }),
        Kind::Area => plot.geom_area().geom_point_with(small_point()),
        Kind::AreaStacked => plot
            .geom_area_with(GeomArea {
                alpha: 0.85,
                ..Default::default()
            })
            .position(PositionStack),
        Kind::Point => plot.geom_point(),
        Kind::Jitter => plot.geom_jitter(),
        // Box plots are unfilled by default (white box, dark whiskers/outline) —
        // the ggplot idiom; a CATEGORY still colours the outline via the border.
        Kind::Boxplot => plot.geom_boxplot_with(GeomBoxplot {
            fill: (255, 255, 255),
            color: (60, 60, 60),
            width: 0.6,
            alpha: 1.0,
        }),
        // Violins are unfilled too (white body, dark outline), matching boxplots.
        Kind::Violin => plot.geom_violin_with(GeomViolin {
            fill: (255, 255, 255),
            color: (70, 78, 92),
            alpha: 1.0,
            line_width: 1.0,
        }),
        Kind::Pie
        | Kind::Donut
        | Kind::Gauge
        | Kind::Histogram
        | Kind::Density
        | Kind::QQ
        | Kind::Heatmap
        | Kind::Calendar
        | Kind::Candlestick
        | Kind::Radar
        | Kind::Sparkline => {
            unreachable!("handled above")
        }
    };
    // Data labels (`::DATALABELS`): draw the measure value just above each mark.
    if show_labels {
        plot = plot
            .geom_text_with(GeomText {
                size: dlabel_size,
                color: (70, 78, 92),
                // Lift the label clear of the mark (a small gap above the top).
                vjust: -0.35,
                ..Default::default()
            })
            .layer_aes(Aes::new().x("x").y("y").label("dlab"));
    }
    // Combo layers: overlay each extra measure with its own geom + y column.
    for (k, ev) in extras.iter().enumerate() {
        if let Role::Value(ekind) = ev.role {
            let yk = format!("y{}", k + 2);
            let mut lay = Aes::new().x("x").y(&yk);
            if !combo_names.is_empty() {
                lay = lay.color(&format!("__s{}", k + 1));
            }
            plot = match ekind {
                Kind::Line => plot.geom_line(),
                Kind::Area => plot.geom_area(),
                // A point overlay (e.g. detected changepoints/peaks) reads as a
                // marker on the base line, so make it a touch larger.
                Kind::Point => plot.geom_point_with(GeomPoint {
                    size: 3.4,
                    ..Default::default()
                }),
                _ => plot.geom_col(),
            }
            .layer_aes(lay);
        }
    }
    // Horizontal reference/target lines (`::REFLINE`/`::YLINE`) — one per distinct
    // value in the column (an average line, min/max bands, several thresholds…).
    if let Some(rl) = cols.iter().find(|c| c.role == Role::RefLine) {
        for v in distinct_nums(&rl.values) {
            plot = plot.geom_hline(v);
        }
    }
    // Vertical reference lines (`::XLINE`) — only meaningful on a continuous x.
    if let Some(vl) = cols.iter().find(|c| c.role == Role::VLine) {
        for v in distinct_nums(&vl.values) {
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
        plot = if by_colour {
            plot.scale_color_manual(pairs)
        } else {
            plot.scale_fill_manual(pairs)
        };
    }
    if !combo_names.is_empty() {
        // Combo measures → distinct palette colours, keyed by column header.
        let pairs: Vec<(&str, ggplot_rs::scale::color::RGBAColor)> = combo_names
            .iter()
            .enumerate()
            .map(|(i, s)| (s.as_str(), dz_color(i)))
            .collect();
        plot = plot.scale_color_manual(pairs);
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
    // ggplot2-style axis label formatting (`::YFORMAT '€'`, `::XFORMAT '$'`, …).
    let fmt_spec = |role: Role| -> Option<String> {
        cols.iter().find(|c| c.role == role).and_then(|c| {
            c.values.iter().find_map(|v| match v {
                Value::Str(s) if !s.is_empty() => Some(s.clone()),
                _ => None,
            })
        })
    };
    if let Some(f) = fmt_spec(Role::YFormat).and_then(|s| axis_formatter(&s)) {
        plot = plot.scale_y_continuous(
            ggplot_rs::scale::continuous::ScaleContinuous::new().with_label_formatter(f),
        );
    }
    // An x formatter only makes sense on a continuous x (numeric/date), not on the
    // discrete category axis of a bar chart.
    if !x_discrete {
        if let Some(f) = fmt_spec(Role::XFormat).and_then(|s| axis_formatter(&s)) {
            plot = plot.scale_x_continuous(
                ggplot_rs::scale::continuous::ScaleContinuous::new().with_label_formatter(f),
            );
        }
    }
    // `::FLIP` swaps the axes — e.g. a horizontal bar chart.
    let flipped = cols.iter().any(|c| c.role == Role::Flip);
    if flipped {
        plot = plot.coord_flip();
    }
    // Scroll/drag-zoom window (from the UI) — clip a continuous cartesian panel to
    // the given data rectangle. The UI only sets it for continuous/datetime x.
    if !flipped {
        if let Some((xlim, ylim)) = panel_zoom() {
            plot = plot.coord_cartesian_zoom(Some(xlim), Some(ylim));
        }
    }
    // DataZoo steel blue for single-series marks. Set the primary AFTER the theme
    // preset — presets replace the whole theme. Box plots opt out so they stay
    // unfilled (primary would re-colour the box fill).
    plot = plot.theme_minimal();
    // A combo already colours each measure explicitly via the manual scale; the
    // brand primary would flatten them all back to one colour, so skip it there.
    if !matches!(kind, Kind::Boxplot | Kind::Violin) && combo_names.is_empty() {
        plot = plot.primary_color(brand());
    }
    plot = plot.legend_position(ggplot_rs::theme::LegendPosition::Top);
    if let Some(t) = &title {
        plot = plot.title(t);
    }
    plot.render_svg_native_with_size(width, height)
        .map_err(|e| format!("render failed: {e:?}"))
}

/// A histogram of the measure column (ggplot bins + counts).
fn render_histogram(
    value: &Column,
    title: Option<&str>,
    width: u32,
    height: u32,
) -> Result<String, String> {
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

/// A kernel-density curve of the measure column. An optional `CATEGORY` splits
/// it into one filled curve per group (overlaid, semi-transparent).
fn render_density(
    value: &Column,
    cols: &[Column],
    title: Option<&str>,
    width: u32,
    height: u32,
) -> Result<String, String> {
    let category = cols.iter().find(|c| c.role == Role::Category);
    let mut data: Vec<(String, Vec<Value>)> = vec![("x".to_string(), value.values.clone())];
    let mut aes = Aes::new().x("x");
    let mut plot;
    if let Some(cat) = category {
        data.push(("cat".to_string(), cat.values.clone()));
        aes = aes.fill("cat").color("cat");
        let levels = distinct_labels(cat);
        let pairs: Vec<(&str, ggplot_rs::scale::color::RGBAColor)> = levels
            .iter()
            .enumerate()
            .map(|(i, s)| (s.as_str(), parse_hex(s).unwrap_or_else(|| dz_color(i))))
            .collect();
        plot = GGPlot::new(data)
            .aes(aes)
            .geom_density_with(GeomDensity {
                alpha: 0.4,
                ..Default::default()
            })
            .scale_fill_manual(pairs.clone())
            .scale_color_manual(pairs)
            .theme_minimal()
            .legend_position(ggplot_rs::theme::LegendPosition::Top);
    } else {
        plot = GGPlot::new(data)
            .aes(aes)
            .geom_density()
            .theme_minimal()
            .primary_color(brand());
    }
    if let Some(t) = title {
        plot = plot.title(t);
    }
    plot.render_svg_native_with_size(width, height)
        .map_err(|e| format!("render failed: {e:?}"))
}

/// Build an axis tick formatter from a short spec — a keyword or a currency
/// symbol — mirroring ggplot2's `scales::` label helpers. Numbers are always
/// thousands-grouped. A leading space marks a suffix ("` kg`" → "12 kg");
/// otherwise the spec is a prefix ("€" → "€1,200", "CHF " → "CHF 1,200").
fn axis_formatter(spec: &str) -> Option<Box<dyn Fn(f64) -> String + Send + Sync>> {
    use ggplot_rs::scale::format::{label_comma, label_dollar};
    let s = spec.trim_end_matches(|c: char| c == ';');
    let low = s.trim().to_ascii_lowercase();
    if low.is_empty() {
        return None;
    }
    let money = |sym: &'static str| -> Box<dyn Fn(f64) -> String + Send + Sync> {
        Box::new(move |v: f64| {
            if v < 0.0 {
                format!("-{sym}{}", label_comma(-v))
            } else {
                format!("{sym}{}", label_comma(v))
            }
        })
    };
    let f: Box<dyn Fn(f64) -> String + Send + Sync> = match low.as_str() {
        "$" | "usd" | "dollar" | "dollars" => Box::new(label_dollar),
        "€" | "eur" | "euro" | "euros" => money("€"),
        "£" | "gbp" | "pound" | "pounds" => money("£"),
        "¥" | "jpy" | "yen" | "cny" | "yuan" => money("¥"),
        "%" | "percent" | "pct" => Box::new(|v: f64| format!("{}%", label_comma(v))),
        "," | "comma" | "thousands" | "number" => Box::new(label_comma),
        _ => {
            // Literal prefix, or suffix if it starts with a space.
            if let Some(suffix) = s.strip_prefix(' ') {
                let suffix = suffix.to_string();
                Box::new(move |v: f64| format!("{}{}", label_comma(v), suffix))
            } else {
                let prefix = s.to_string();
                Box::new(move |v: f64| {
                    if v < 0.0 {
                        format!("-{}{}", prefix, label_comma(-v))
                    } else {
                        format!("{}{}", prefix, label_comma(v))
                    }
                })
            }
        }
    };
    Some(f)
}

/// A normal quantile-quantile plot of the measure column (`geom_qq` + a
/// reference line) — points on the line ⇒ roughly normal; systematic bowing ⇒
/// skew/heavy tails. Handy for checking a model's residuals.
fn render_qq(
    value: &Column,
    title: Option<&str>,
    width: u32,
    height: u32,
) -> Result<String, String> {
    let data = vec![("y".to_string(), value.values.clone())];
    let mut plot = GGPlot::new(data)
        .aes(Aes::new().y("y"))
        .geom_qq()
        .geom_qq_line()
        .xlab("Theoretical")
        .ylab("Sample")
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
    let x = cols
        .iter()
        .find(|c| c.role == Role::X)
        .ok_or("heatmap needs an XAXIS column")?;
    let y = cols
        .iter()
        .find(|c| c.role == Role::Y)
        .ok_or("heatmap needs a YAXIS column")?;
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
    // A gridded heatmap with numeric axes should tick on the tile positions, not
    // at generic "nice" breaks (0, 2.5, 5…) that fall between tiles. Use the
    // distinct data values (thinned) as breaks so labels sit under each tile.
    // Tiles are 1 unit wide/tall (± 0.5), so expand the domain by half a tile —
    // otherwise the edge tiles spill past the axis and cover the tick labels.
    use ggplot_rs::scale::continuous::ScaleContinuous;
    if let Some(bx) = tile_breaks(&x.values) {
        plot = plot.scale_x_continuous(
            ScaleContinuous::new()
                .with_breaks(bx)
                .with_expand(0.0, 0.55),
        );
    }
    if let Some(by) = tile_breaks(&y.values) {
        plot = plot.scale_y_continuous(
            ScaleContinuous::new()
                .with_breaks(by)
                .with_expand(0.0, 0.55),
        );
    }
    if let Some(t) = title {
        plot = plot.title(t);
    }
    plot.render_svg_native_with_size(width, height)
        .map_err(|e| format!("render failed: {e:?}"))
}

/// Civil (year, month, day) from days since 1970-01-01 — Hinnant's algorithm.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// A GitHub/ECharts-style calendar heatmap: a date `::XAXIS` + a measure laid out
/// as week-columns × weekday-rows, with month labels along the top and weekday
/// labels down the left; cells are coloured light→brand by value.
fn render_calendar(
    value: &Column,
    cols: &[Column],
    width: u32,
    height: u32,
) -> Result<String, String> {
    let x = cols
        .iter()
        .find(|c| c.role == Role::X)
        .ok_or("calendar needs an XAXIS date column")?;
    let mut pts: Vec<(i64, f64)> = Vec::new();
    for (dv, vv) in x.values.iter().zip(value.values.iter()) {
        if let (Some(secs), Some(val)) = (dv.as_f64(), vv.as_f64()) {
            pts.push(((secs as i64).div_euclid(86_400), val));
        }
    }
    if pts.is_empty() {
        return Ok(heading_svg("calendar needs a date axis", width));
    }
    let min_day = pts.iter().map(|(d, _)| *d).min().unwrap();
    let max_day = pts.iter().map(|(d, _)| *d).max().unwrap();
    let vmin = pts.iter().map(|(_, v)| *v).fold(f64::INFINITY, f64::min);
    let vmax = pts
        .iter()
        .map(|(_, v)| *v)
        .fold(f64::NEG_INFINITY, f64::max);
    let vspan = if (vmax - vmin).abs() < 1e-9 {
        1.0
    } else {
        vmax - vmin
    };

    // Sunday = 0 … Saturday = 6 (1970-01-01 was a Thursday → 4).
    let weekday = |d: i64| ((d % 7) + 4).rem_euclid(7);
    let first_sun = min_day - weekday(min_day);
    let col_of = |d: i64| (d - first_sun) / 7;
    let n_cols = (col_of(max_day) + 1).max(1) as f64;

    let (w, h) = (width as f64, height as f64);
    let (left, top, right, bottom) = (30.0, 18.0, 10.0, 6.0);
    let cell = ((w - left - right) / n_cols)
        .min((h - top - bottom) / 7.0)
        .max(3.0);
    let gap = (cell * 0.14).clamp(0.5, 2.5);
    let sz = cell - gap;
    let (x0, y0) = (left, top);

    let lo = (0xebu8, 0xf1u8, 0xf7u8);
    let hi = brand();
    let mix = |t: f64| {
        let t = t.clamp(0.0, 1.0);
        let m = |a: u8, b: u8| (a as f64 + (b as f64 - a as f64) * t).round() as u8;
        format!(
            "#{:02x}{:02x}{:02x}",
            m(lo.0, hi.0),
            m(lo.1, hi.1),
            m(lo.2, hi.2)
        )
    };
    let esc = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;");

    let mut body = String::new();
    for (row, lbl) in [(1u32, "Mon"), (3, "Wed"), (5, "Fri")] {
        let cy = y0 + row as f64 * cell + sz / 2.0;
        body += &format!(
            "<text x=\"{:.1}\" y=\"{cy:.1}\" text-anchor=\"end\" dominant-baseline=\"middle\" font-family=\"system-ui,sans-serif\" font-size=\"9\" fill=\"#7a8496\">{lbl}</text>",
            x0 - 5.0
        );
    }
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let mut last_month = 0u32;
    for d in min_day..=max_day {
        let (_, m, _) = civil_from_days(d);
        if m != last_month {
            let col = col_of(d) as f64;
            body += &format!(
                "<text x=\"{:.1}\" y=\"12\" font-family=\"system-ui,sans-serif\" font-size=\"9\" fill=\"#5a6472\">{}</text>",
                x0 + col * cell,
                MONTHS[(m - 1) as usize]
            );
            last_month = m;
        }
    }
    let round = (sz * 0.18).min(2.5);
    for &(d, v) in &pts {
        let cx = x0 + col_of(d) as f64 * cell;
        let cy = y0 + weekday(d) as f64 * cell;
        let (yr, mo, dom) = civil_from_days(d);
        let tip = format!("{yr:04}-{mo:02}-{dom:02}: {}", fmt_label(&Value::Float(v)));
        body += &format!(
            "<rect class=\"dp-hit\" x=\"{cx:.1}\" y=\"{cy:.1}\" width=\"{sz:.1}\" height=\"{sz:.1}\" rx=\"{round:.1}\" fill=\"{}\" stroke=\"#e3e8ef\" stroke-width=\"0.5\"><title>{}</title></rect>",
            mix((v - vmin) / vspan),
            esc(&tip)
        );
    }

    Ok(format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"{height}\" viewBox=\"0 0 {width} {height}\">{body}</svg>"
    ))
}

/// A date `::XAXIS` value as a calendar date, otherwise its plain string.
fn date_or_str(v: &Value) -> String {
    match v {
        Value::DateTime(s) => ggplot_rs::data::format_epoch_secs(*s),
        other => value_str(other),
    }
}

/// An OHLC candlestick chart: `::XAXIS` period, `::OPEN`/`::HIGH`/`::LOW` prices,
/// and the close as the measure (`::CANDLESTICK`). Up candles green, down red.
fn render_candlestick(
    value: &Column,
    cols: &[Column],
    _title: Option<&str>,
    width: u32,
    height: u32,
) -> Result<String, String> {
    let x = cols
        .iter()
        .find(|c| c.role == Role::X)
        .ok_or("candlestick needs an XAXIS column")?;
    let open = cols
        .iter()
        .find(|c| c.role == Role::Open)
        .ok_or("candlestick needs an ::OPEN column")?;
    let high = cols
        .iter()
        .find(|c| c.role == Role::High)
        .ok_or("candlestick needs a ::HIGH column")?;
    let low = cols
        .iter()
        .find(|c| c.role == Role::Low)
        .ok_or("candlestick needs a ::LOW column")?;
    let n = value.values.len();
    if n == 0 {
        return Ok(heading_svg("candlestick needs data", width));
    }
    let getf = |c: &Column, i: usize| c.values.get(i).and_then(|v| v.as_f64());
    let mut ylo = f64::INFINITY;
    let mut yhi = f64::NEG_INFINITY;
    for i in 0..n {
        if let Some(l) = getf(low, i) {
            ylo = ylo.min(l);
        }
        if let Some(hv) = getf(high, i) {
            yhi = yhi.max(hv);
        }
    }
    if !ylo.is_finite() || !yhi.is_finite() {
        return Ok(heading_svg("candlestick needs numeric OHLC", width));
    }
    let pad = ((yhi - ylo) * 0.05).max(1e-9);
    let (ylo, yhi) = (ylo - pad, yhi + pad);
    let yspan = if (yhi - ylo).abs() < 1e-9 {
        1.0
    } else {
        yhi - ylo
    };

    let (w, h) = (width as f64, height as f64);
    let (left, top, right, bottom) = (46.0, 8.0, 10.0, 22.0);
    let pw = w - left - right;
    let ph = h - top - bottom;
    let x_px = |i: usize| left + (i as f64 + 0.5) / n as f64 * pw;
    let y_px = |v: f64| top + (1.0 - (v - ylo) / yspan) * ph;
    let bw = (pw / n as f64 * 0.6).clamp(1.0, 18.0);
    let esc = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;");
    let hex = |c: (u8, u8, u8)| format!("#{:02x}{:02x}{:02x}", c.0, c.1, c.2);
    let up = (0x0c, 0xa6, 0x78);
    let down = (0xe0, 0x31, 0x31);

    let mut body = String::new();
    for k in 0..=4 {
        let v = ylo + (yhi - ylo) * k as f64 / 4.0;
        let py = y_px(v);
        body += &format!(
            "<line x1=\"{left:.1}\" y1=\"{py:.1}\" x2=\"{:.1}\" y2=\"{py:.1}\" stroke=\"#ececec\" stroke-width=\"1\"/><text x=\"{:.1}\" y=\"{py:.1}\" text-anchor=\"end\" dominant-baseline=\"middle\" font-family=\"system-ui,sans-serif\" font-size=\"9\" fill=\"#7a8496\">{}</text>",
            left + pw, left - 5.0, esc(&fmt_label(&Value::Float(v)))
        );
    }
    let xstep = (n / 8).max(1);
    for i in (0..n).step_by(xstep) {
        body += &format!(
            "<text x=\"{:.1}\" y=\"{:.1}\" text-anchor=\"middle\" font-family=\"system-ui,sans-serif\" font-size=\"8\" fill=\"#7a8496\">{}</text>",
            x_px(i), h - 7.0, esc(&date_or_str(&x.values[i]))
        );
    }
    for i in 0..n {
        let (Some(o), Some(c), Some(hv), Some(lv)) = (
            getf(open, i),
            value.values[i].as_f64(),
            getf(high, i),
            getf(low, i),
        ) else {
            continue;
        };
        let cx = x_px(i);
        let col = if c >= o { up } else { down };
        let ch = hex(col);
        let yt = y_px(o.max(c));
        let bh = (y_px(o.min(c)) - yt).max(1.0);
        let tip = format!(
            "{} — O {} H {} L {} C {}",
            date_or_str(&x.values[i]),
            fmt_label(&Value::Float(o)),
            fmt_label(&Value::Float(hv)),
            fmt_label(&Value::Float(lv)),
            fmt_label(&Value::Float(c))
        );
        body += &format!(
            "<line x1=\"{cx:.1}\" y1=\"{:.1}\" x2=\"{cx:.1}\" y2=\"{:.1}\" stroke=\"{ch}\" stroke-width=\"1\"/><rect class=\"dp-hit\" x=\"{:.1}\" y=\"{yt:.1}\" width=\"{bw:.1}\" height=\"{bh:.1}\" fill=\"{ch}\" stroke=\"{ch}\"><title>{}</title></rect>",
            y_px(hv), y_px(lv), cx - bw / 2.0, esc(&tip)
        );
    }
    Ok(format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"{height}\" viewBox=\"0 0 {width} {height}\">{body}</svg>"
    ))
}

/// A radar / spider chart: axes from the `::XAXIS` (metric names), values from
/// the measure (`::RADAR`), one filled polygon per `::CATEGORY` series.
fn render_radar(
    value: &Column,
    cols: &[Column],
    _title: Option<&str>,
    width: u32,
    height: u32,
) -> Result<String, String> {
    let x = cols
        .iter()
        .find(|c| c.role == Role::X)
        .ok_or("radar needs an XAXIS (axis) column")?;
    let category = cols.iter().find(|c| c.role == Role::Category);
    let mut axes: Vec<String> = Vec::new();
    for v in &x.values {
        let s = value_str(v);
        if !axes.contains(&s) {
            axes.push(s);
        }
    }
    let n_ax = axes.len();
    if n_ax < 3 {
        return Ok(heading_svg("radar needs at least 3 axes", width));
    }
    let series: Vec<String> = match category {
        Some(c) => {
            let mut s = Vec::new();
            for v in &c.values {
                let k = value_str(v);
                if !s.contains(&k) {
                    s.push(k);
                }
            }
            s
        }
        None => vec![String::new()],
    };
    let mut mat: std::collections::HashMap<(usize, usize), f64> = Default::default();
    let mut gmax = 1e-9f64;
    for i in 0..value.values.len() {
        let ax = axes.iter().position(|a| *a == value_str(&x.values[i]));
        let se = match category {
            Some(c) => series.iter().position(|s| *s == value_str(&c.values[i])),
            None => Some(0),
        };
        if let (Some(ax), Some(se), Some(v)) = (ax, se, value.values[i].as_f64()) {
            mat.insert((se, ax), v);
            gmax = gmax.max(v.abs());
        }
    }

    let (w, h) = (width as f64, height as f64);
    let cx = w / 2.0;
    let cy = h / 2.0 + 6.0;
    let radius = (w.min(h) / 2.0 - 42.0).max(20.0);
    let pi = std::f64::consts::PI;
    let angle = |k: usize| -pi / 2.0 + 2.0 * pi * k as f64 / n_ax as f64;
    let pt = |k: usize, frac: f64| {
        (
            cx + radius * frac * angle(k).cos(),
            cy + radius * frac * angle(k).sin(),
        )
    };
    let esc = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;");
    let hexc =
        |c: ggplot_rs::scale::color::RGBAColor| format!("#{:02x}{:02x}{:02x}", c.r, c.g, c.b);

    let mut body = String::new();
    for ring in 1..=4 {
        let frac = ring as f64 / 4.0;
        let pts: Vec<String> = (0..n_ax)
            .map(|k| {
                let (px, py) = pt(k, frac);
                format!("{px:.1},{py:.1}")
            })
            .collect();
        body += &format!(
            "<polygon points=\"{}\" fill=\"none\" stroke=\"#e6eaf0\" stroke-width=\"1\"/>",
            pts.join(" ")
        );
    }
    for (k, axname) in axes.iter().enumerate() {
        let (px, py) = pt(k, 1.0);
        body += &format!(
            "<line x1=\"{cx:.1}\" y1=\"{cy:.1}\" x2=\"{px:.1}\" y2=\"{py:.1}\" stroke=\"#e6eaf0\" stroke-width=\"1\"/>"
        );
        let (lx, ly) = pt(k, 1.14);
        let anchor = if (lx - cx).abs() < radius * 0.05 {
            "middle"
        } else if lx > cx {
            "start"
        } else {
            "end"
        };
        body += &format!(
            "<text x=\"{lx:.1}\" y=\"{ly:.1}\" text-anchor=\"{anchor}\" dominant-baseline=\"middle\" font-family=\"system-ui,sans-serif\" font-size=\"9\" fill=\"#5a6472\">{}</text>",
            esc(axname)
        );
    }
    for (si, sname) in series.iter().enumerate() {
        let ch = hexc(dz_color(si));
        let pts: Vec<String> = (0..n_ax)
            .map(|k| {
                let v = mat.get(&(si, k)).copied().unwrap_or(0.0);
                let (px, py) = pt(k, (v / gmax).clamp(0.0, 1.0));
                format!("{px:.1},{py:.1}")
            })
            .collect();
        body += &format!(
            "<polygon points=\"{}\" fill=\"{ch}\" fill-opacity=\"0.18\" stroke=\"{ch}\" stroke-width=\"1.6\"/>",
            pts.join(" ")
        );
        for (k, axname) in axes.iter().enumerate() {
            let v = mat.get(&(si, k)).copied().unwrap_or(0.0);
            let (px, py) = pt(k, (v / gmax).clamp(0.0, 1.0));
            let tip = if sname.is_empty() {
                format!("{}: {}", axname, fmt_label(&Value::Float(v)))
            } else {
                format!("{sname} · {}: {}", axname, fmt_label(&Value::Float(v)))
            };
            body += &format!(
                "<circle class=\"dp-hit\" cx=\"{px:.1}\" cy=\"{py:.1}\" r=\"2.6\" fill=\"{ch}\"><title>{}</title></circle>",
                esc(&tip)
            );
        }
    }
    if series.iter().any(|s| !s.is_empty()) {
        let mut lx = 12.0;
        for (si, sname) in series.iter().enumerate() {
            if sname.is_empty() {
                continue;
            }
            let ch = hexc(dz_color(si));
            body += &format!(
                "<rect x=\"{lx:.1}\" y=\"6\" width=\"9\" height=\"9\" rx=\"2\" fill=\"{ch}\"/>"
            );
            body += &format!(
                "<text x=\"{:.1}\" y=\"14\" font-family=\"system-ui,sans-serif\" font-size=\"9\" fill=\"#39424f\">{}</text>",
                lx + 12.0, esc(sname)
            );
            lx += 12.0 + sname.len() as f64 * 6.0 + 14.0;
        }
    }
    Ok(format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"{height}\" viewBox=\"0 0 {width} {height}\">{body}</svg>"
    ))
}

/// A minimal inline trend line (no axes) — a sparkline over the row order: a
/// light steel area under a crisp line, with a marker on the latest value.
fn render_sparkline(value: &Column, width: u32, height: u32) -> Result<String, String> {
    let n = value.values.len();
    let xs: Vec<Value> = (0..n).map(|i| Value::Float(i as f64)).collect();
    let data = vec![
        ("x".to_string(), xs),
        ("y".to_string(), value.values.clone()),
    ];

    let steel = brand();
    let fill = lighten(steel, 0.62); // wash under the line
    let last = n.saturating_sub(1);
    // A single-point layer marking the most recent value (the eye-catching dot).
    let end = vec![
        ("x".to_string(), vec![Value::Float(last as f64)]),
        (
            "y".to_string(),
            vec![value.values.get(last).cloned().unwrap_or(Value::Na)],
        ),
    ];

    GGPlot::new(data)
        .aes(Aes::new().x("x").y("y"))
        .geom_area_with(GeomArea {
            fill,
            color: fill,
            alpha: 0.55,
            line_width: 0.0,
        })
        .geom_line_with(GeomLine {
            color: steel,
            width: 2.6,
            alpha: 1.0,
        })
        .geom_point_with(GeomPoint {
            size: 4.2,
            color: DZ_COLORS[2],
            alpha: 1.0,
        })
        .layer_data(end) // restrict the point layer to just the endpoint
        .scale_y_continuous(
            ggplot_rs::scale::continuous::ScaleContinuous::new().with_expand(0.1, 0.0),
        )
        .theme_void()
        // Render at a compact width so strokes stay crisp when the inline
        // sparkline is scaled down into a narrow panel column.
        .render_svg_native_with_size(width.min(240), height)
        .map_err(|e| format!("render failed: {e:?}"))
}

/// Blend a colour toward white by `t` (0 = unchanged, 1 = white).
fn lighten((r, g, b): (u8, u8, u8), t: f64) -> (u8, u8, u8) {
    let f = |c: u8| (c as f64 + (255.0 - c as f64) * t).round() as u8;
    (f(r), f(g), f(b))
}

/// Break positions for a numeric heatmap axis: the distinct data values (sorted,
/// thinned to ≤ ~13) so tick labels align with tile centres. `None` when the
/// column isn't fully numeric (a categorical axis handles its own alignment).
fn tile_breaks(vals: &[Value]) -> Option<Vec<f64>> {
    let mut xs: Vec<f64> = Vec::with_capacity(vals.len());
    for v in vals {
        xs.push(v.as_f64()?); // any non-numeric → treat the axis as discrete
    }
    xs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    xs.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
    if xs.is_empty() {
        return None;
    }
    let step = (xs.len() as f64 / 13.0).ceil().max(1.0) as usize;
    Some(xs.iter().step_by(step).copied().collect())
}

/// Distinct finite numeric values from a column, in first-seen order — used to
/// draw one reference line per value.
fn distinct_nums(vals: &[Value]) -> Vec<f64> {
    let mut out: Vec<f64> = Vec::new();
    for v in vals {
        if let Some(f) = v.as_f64() {
            if f.is_finite() && !out.iter().any(|&e| (e - f).abs() < f64::EPSILON) {
                out.push(f);
            }
        }
    }
    out
}

/// Format a measure value for an on-chart data label: whole numbers without a
/// decimal, otherwise rounded to one place; strings verbatim.
fn fmt_label(v: &Value) -> String {
    match v {
        Value::Str(s) => s.clone(),
        _ => match v.as_f64() {
            Some(f) if f.fract().abs() < 1e-9 => format!("{}", f.round() as i64),
            Some(f) => format!("{:.1}", f),
            None => String::new(),
        },
    }
}

/// Parse a `#rrggbb` / `rrggbb` string into an RGBA colour (`None` otherwise).
fn parse_hex(s: &str) -> Option<ggplot_rs::scale::color::RGBAColor> {
    let h = s.trim().strip_prefix('#').unwrap_or(s.trim());
    if h.len() != 6 || !h.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let p = |a, b| u8::from_str_radix(&h[a..b], 16).ok();
    Some(ggplot_rs::scale::color::RGBAColor::new(
        p(0, 2)?,
        p(2, 4)?,
        p(4, 6)?,
    ))
}

/// A gauge: a 270° arc showing a single value's progress through a `min,max`
/// `::RANGE` (default `0,100`). Optional `::COLORS` paints threshold zones.
fn render_gauge(
    value: &Column,
    cols: &[Column],
    title: Option<&str>,
    width: u32,
    height: u32,
) -> Result<String, String> {
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
    let span = if (max - min).abs() < 1e-9 {
        1.0
    } else {
        max - min
    };
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
    let num = if (val - val.round()).abs() < 1e-9 {
        format!("{}", val.round() as i64)
    } else {
        format!("{val:.1}")
    };
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
fn render_map(
    cols: &[Column],
    _title: Option<&str>,
    width: u32,
    height: u32,
) -> Result<String, String> {
    let geom = cols
        .iter()
        .find(|c| c.role == Role::Geometry)
        .ok_or("map needs a ::MAP column")?;
    let fill = cols.iter().find(|c| matches!(c.role, Role::Value(_)));
    let label = cols.iter().find(|c| c.role == Role::Label);
    let base = cols.iter().find(|c| c.role == Role::Basemap);
    // Optional layer opacity (`::ALPHA`) — e.g. overlapping quake points reading
    // as density. Clamp to a valid 0..1; default fully opaque.
    let alpha = cols
        .iter()
        .find(|c| c.role == Role::Alpha)
        .and_then(|c| c.values.iter().find_map(|v| v.as_f64()))
        .map(|a| a.clamp(0.05, 1.0))
        .unwrap_or(1.0);
    let mut data: Vec<(String, Vec<Value>)> = vec![("geometry".to_string(), geom.values.clone())];
    let mut aes = Aes::new();
    if let Some(f) = fill {
        data.push(("fill".to_string(), f.values.clone()));
        aes = aes.fill("fill");
    }
    let lab = label
        .map(|l| l.values.clone())
        .or_else(|| fill.map(|f| f.values.clone()));
    if let Some(lv) = lab {
        data.push(("label".to_string(), lv));
        aes = aes.label("label");
    }
    let mut plot = GGPlot::new(data).aes(aes);
    // Optional grey basemap (e.g. country outlines) drawn first, behind the data
    // layer — a separate no-fill geom_sf layer that shares the map's scales.
    if let Some(b) = base {
        let base_geom = ggplot_rs::geom::sf::GeomSf {
            fill: (228, 230, 233),
            color: (198, 201, 206),
            ..Default::default()
        };
        plot = plot
            .geom_sf_with(base_geom)
            .layer_data(vec![("geometry".to_string(), b.values.clone())])
            .layer_aes(Aes::new());
    }
    // A zoom window (from scroll/drag in the UI) clips to that lon/lat rectangle;
    // otherwise fit the whole geometry with an equal aspect ratio.
    let plot = plot.geom_sf_with(ggplot_rs::geom::sf::GeomSf {
        alpha,
        ..Default::default()
    });
    let mut plot = match panel_zoom() {
        Some((xlim, ylim)) => plot.coord_cartesian_zoom(Some(xlim), Some(ylim)),
        None => plot.coord_sf(),
    };
    plot = plot.theme_void();
    if fill.is_some() {
        // Viridis gives points/regions strong contrast over the grey basemap;
        // a plain choropleth keeps the on-brand light→primary gradient.
        plot = if base.is_some() {
            plot.scale_fill_viridis_c()
        } else {
            plot.scale_fill_gradient(
                ggplot_rs::scale::color::RGBAColor::new(0xed, 0xf1, 0xf7),
                ggplot_rs::scale::color::RGBAColor::new(brand().0, brand().1, brand().2),
            )
        };
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
    let category = cols
        .iter()
        .find(|c| c.role == Role::Category)
        .ok_or("pie needs a CATEGORY column")?;
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
        .scale_y_continuous(
            ggplot_rs::scale::continuous::ScaleContinuous::new().with_expand(0.0, 0.0),
        )
        .coord_polar_with(
            ggplot_rs::coord::polar::CoordPolar::new()
                .theta("y")
                .inner_radius(inner),
        )
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
/// (free it with [`anofox_free`]). Exercises the whole render path through FFI
/// — the shape the DuckDB extension entrypoint will use.
#[no_mangle]
pub extern "C" fn anofox_smoke() -> *mut c_char {
    let cols = vec![
        Column::new(
            "x",
            Role::X,
            vec![Value::Str("a".into()), Value::Str("b".into())],
        ),
        Column::new(
            "n",
            Role::Value(Kind::Bar),
            vec![Value::Float(3.0), Value::Float(7.0)],
        ),
    ];
    let svg = render(&cols, 300, 200).unwrap_or_default();
    CString::new(svg)
        .map(|s| s.into_raw())
        .unwrap_or(std::ptr::null_mut())
}

/// Free a string returned by the C ABI.
#[no_mangle]
#[allow(clippy::not_unsafe_ptr_arg_deref)]
pub extern "C" fn anofox_free(p: *mut c_char) {
    if !p.is_null() {
        unsafe { drop(CString::from_raw(p)) };
    }
}

/// A minimal SVG heading (for a `::LABEL`-only result).
fn heading_svg(text: &str, width: u32) -> String {
    let esc = text
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"40\" viewBox=\"0 0 {width} 40\">\
         <text x=\"4\" y=\"26\" font-family=\"system-ui,sans-serif\" font-size=\"20\" font-weight=\"600\" fill=\"#1f2430\">{esc}</text></svg>"
    )
}
