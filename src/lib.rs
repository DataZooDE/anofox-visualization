//! anofox-visualization — SQL-defined dashboards.
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

/// The cell value type of a [`Column`] (re-exported so callers don't need a
/// direct `ggplot-rs` dependency).
pub use ggplot_rs::prelude::Value;
use ggplot_rs::prelude::*;

pub mod dashboard;
mod downsample;
pub mod format;
pub mod host;
pub mod lint;
pub mod roles;
pub mod sql;
#[cfg(feature = "wasm")]
pub mod wasm;

/// The kind of chart, taken from the cast on the *value* column (e.g.
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
    /// `::BUBBLE` — context-dependent so both documented forms work: alone it is
    /// the scatter's y measure (sized by a `::SIZE` column); next to another
    /// chart measure (`y::SCATTER, pop::BUBBLE`) it is that chart's size.
    /// Resolved before rendering (never reaches a geom).
    Bubble,
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

/// Parse a role annotation (the part after `::`) into a [`Role`].
/// Case-insensitive; returns `None` for unknown annotations (plain columns).
/// Backed by the [`roles::REGISTRY`] — the single source of the vocabulary.
pub fn parse_role(annotation: &str) -> Option<Role> {
    roles::lookup(annotation).map(|s| s.role)
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

/// The DataZoo palette — anofox-visualization's default categorical + single-series
/// colours. The first five are the brand set (index 0 = the default primary);
/// seven more distinct hues follow so a chart with up to ~12 categories keeps
/// every series visually separable (11 GICS sectors used to collide at 5).
pub const DZ_COLORS: [(u8, u8, u8); 12] = [
    (0x45, 0x64, 0x81), // steel blue (brand primary)
    (0xe8, 0x64, 0x33), // orange
    (0xE8, 0x33, 0x5D), // pink
    (0xef, 0xc9, 0x4c), // yellow
    (0x21, 0x21, 0x21), // near-black
    (0x3f, 0xa6, 0x6f), // green
    (0x81, 0x56, 0xa0), // purple
    (0x57, 0xb7, 0xd6), // sky blue
    (0x9c, 0x6b, 0x3f), // brown
    (0xa7, 0xc9, 0x57), // lime
    (0xd0, 0x81, 0xb8), // mauve
    (0x7a, 0x8b, 0x99), // slate grey
];

fn dz_color(i: usize) -> ggplot_rs::scale::color::RGBAColor {
    let (r, g, b) = DZ_COLORS[i % DZ_COLORS.len()];
    ggplot_rs::scale::color::RGBAColor::new(r, g, b)
}

/// A map zoom window: `((x0, x1), (y0, y1))` in geometry (lon/lat) coordinates.
pub type ZoomWindow = ((f64, f64), (f64, f64));

/// Default cap on distinct levels of a discrete axis / colour category before
/// the smallest are folded into an "Other" bucket (see [`RenderOptions`]).
pub const DEFAULT_MAX_CATEGORIES: usize = 30;
/// Default cap on points per series for line/area/step charts before LTTB
/// downsampling kicks in (see [`RenderOptions`]).
pub const DEFAULT_MAX_LINE_POINTS: usize = 5_000;
/// Rendered width/height are clamped into `MIN_DIM..=MAX_DIM` px.
pub const MIN_DIM: u32 = 32;
/// See [`MIN_DIM`].
pub const MAX_DIM: u32 = 8_192;

/// Per-render options — passed explicitly (no hidden thread state).
#[derive(Clone, Debug)]
pub struct RenderOptions {
    /// Brand/primary colour for single-series marks (`None` = DataZoo steel).
    pub brand: Option<(u8, u8, u8)>,
    /// Zoom window for a `::MAP` / continuous cartesian panel (`None` = auto-fit).
    pub zoom: Option<ZoomWindow>,
    /// Max distinct levels on a discrete x axis or `::CATEGORY` before the
    /// smallest (by total |measure|) fold into "Other". `0` disables the cap.
    pub max_categories: usize,
    /// Max points per series for line/area/step charts before LTTB
    /// downsampling. `0` disables it.
    pub max_line_points: usize,
}

impl Default for RenderOptions {
    fn default() -> Self {
        RenderOptions {
            brand: None,
            zoom: None,
            max_categories: DEFAULT_MAX_CATEGORIES,
            max_line_points: DEFAULT_MAX_LINE_POINTS,
        }
    }
}

impl RenderOptions {
    /// The effective brand colour.
    pub fn brand(&self) -> (u8, u8, u8) {
        self.brand.unwrap_or(DZ_COLORS[0])
    }
}

/// Why a render failed — so hosts (the DuckDB extension, services) can surface
/// a real error instead of an SVG.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RenderError {
    /// The JSON spec could not be parsed / has the wrong shape.
    BadSpec(String),
    /// The roles/columns can't make this chart (e.g. no `::XAXIS`).
    Render(String),
    /// An internal panic was caught (a bug — please report the spec).
    Panic(String),
}

impl std::fmt::Display for RenderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RenderError::BadSpec(m) => write!(f, "bad spec: {m}"),
            RenderError::Render(m) => write!(f, "{m}"),
            RenderError::Panic(m) => write!(f, "internal render error: {m}"),
        }
    }
}

impl std::error::Error for RenderError {}

/// Legacy per-thread `(brand, zoom)` defaults.
type LegacyDefaults = (Option<(u8, u8, u8)>, Option<ZoomWindow>);

thread_local! {
    /// Legacy per-thread defaults behind [`set_brand`]/[`set_panel_zoom`]; only
    /// read by the compatibility wrapper [`render`].
    static LEGACY: std::cell::Cell<LegacyDefaults> =
        const { std::cell::Cell::new((None, None)) };
}

/// Set the brand colour used by subsequent [`render`] calls on this thread.
/// Prefer passing [`RenderOptions::brand`] to [`render_with`].
pub fn set_brand(color: Option<(u8, u8, u8)>) {
    LEGACY.with(|c| c.set((color, c.get().1)));
}

/// Set the zoom window used by subsequent [`render`] calls on this thread.
/// Prefer passing [`RenderOptions::zoom`] to [`render_with`].
pub fn set_panel_zoom(window: Option<ZoomWindow>) {
    LEGACY.with(|c| c.set((c.get().0, window)));
}

/// Distinct category labels in a **stable (sorted) order**, so a given series
/// gets the same DataZoo colour in every chart that contains it.
fn distinct_labels(col: &Column) -> Vec<String> {
    let set: std::collections::BTreeSet<String> = col.values.iter().map(value_str).collect();
    set.into_iter().collect()
}

/// The DataZoo discrete colour/fill scale for a series column: levels in
/// sorted order, so a series gets the same palette colour in every chart that
/// contains it. A level that is itself a hex colour (`#rrggbb`) is drawn in
/// that colour (an extension convention ggplot-rs doesn't have), so such
/// columns pin their levels with a per-level palette.
fn dz_scale(aesthetic: Aesthetic, col: &Column) -> ScaleColorDiscrete {
    let scale = ScaleColorDiscrete::new(aesthetic).sorted();
    let hex_level = |v: &Value| matches!(v, Value::Str(s) if parse_hex(s).is_some());
    if !col.values.iter().any(hex_level) {
        return scale.with_palette((0..DZ_COLORS.len()).map(dz_color).collect());
    }
    let levels = distinct_labels(col);
    let palette = levels
        .iter()
        .enumerate()
        .map(|(i, s)| parse_hex(s).unwrap_or_else(|| dz_color(i)))
        .collect();
    scale.with_levels(levels).with_palette(palette)
}

/// The colour [`dz_scale`] gives the last (sorted) level of `col`.
fn last_level_color(col: &Column) -> Option<(u8, u8, u8)> {
    let levels = distinct_labels(col);
    let i = levels.len().checked_sub(1)?;
    let c = parse_hex(&levels[i]).unwrap_or_else(|| dz_color(i));
    Some((c.r, c.g, c.b))
}

/// Turn a caught panic payload into a message.
fn panic_message(p: Box<dyn std::any::Any + Send>) -> String {
    p.downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| p.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "panic".into())
}

/// Run `f`, converting a panic into [`RenderError::Panic`].
pub(crate) fn guard<T>(f: impl FnOnce() -> Result<T, RenderError>) -> Result<T, RenderError> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(r) => r,
        Err(p) => Err(RenderError::Panic(panic_message(p))),
    }
}

/// Parse `[index, "ROLE", name?]` entries (a spec's / plan's `roles` array).
/// Unknown role tokens are reported (`Err`) instead of silently dropped.
pub fn parse_role_entries(v: &serde_json::Value) -> Result<Vec<(usize, Role, String)>, String> {
    let Some(arr) = v.as_array() else {
        return Err("`roles` must be an array of [index, \"ROLE\", name?]".into());
    };
    let mut out = Vec::with_capacity(arr.len());
    for e in arr {
        let (Some(i), Some(tok)) = (
            e.get(0).and_then(|x| x.as_u64()),
            e.get(1).and_then(|x| x.as_str()),
        ) else {
            return Err(format!(
                "bad role entry {e} (want [index, \"ROLE\", name?])"
            ));
        };
        let role = parse_role(tok).ok_or_else(|| format!("unknown role ::{tok}"))?;
        let name = e.get(2).and_then(|x| x.as_str()).unwrap_or("").to_string();
        out.push((usize::try_from(i).unwrap_or(usize::MAX), role, name));
    }
    Ok(out)
}

/// Build columns from JSON rows + role entries, applying the display names
/// (combo legends read these).
pub fn columns_from_entries(
    rows: &[serde_json::Map<String, serde_json::Value>],
    entries: &[(usize, Role, String)],
) -> Vec<Column> {
    let roles: Vec<(usize, Role)> = entries.iter().map(|(i, r, _)| (*i, *r)).collect();
    let mut cols = sql::columns_from_rows(rows, &roles);
    for (c, (_, _, name)) in cols.iter_mut().zip(entries) {
        if !name.is_empty() {
            c.name = name.clone();
        }
    }
    cols
}

/// Clamp a requested size into `MIN_DIM..=MAX_DIM`.
pub fn clamp_dim(v: u64) -> u32 {
    v.clamp(MIN_DIM as u64, MAX_DIM as u64) as u32
}

/// Render a panel from a single JSON spec — the **checked** entry point for
/// hosts that pass everything as one string (the DuckDB extension, CLI,
/// services):
/// `{"rows":[{c0:…,c1:…},…], "roles":[[idx,"ROLE","name"],…], "width":W,
/// "height":H, "primary":"rrggbb", "max_categories":N, "max_line_points":N}`.
///
/// Never panics (a caught panic becomes [`RenderError::Panic`]). Bare
/// `NaN`/`Infinity` tokens (as DuckDB's JSON may emit) are read as `null`.
pub fn render_spec_checked(spec_json: &str) -> Result<String, RenderError> {
    guard(|| {
        let clean = sql::sanitize_json_numbers(spec_json);
        let spec: serde_json::Value =
            serde_json::from_str(&clean).map_err(|e| RenderError::BadSpec(format!("JSON: {e}")))?;
        if !spec.is_object() {
            return Err(RenderError::BadSpec("spec must be a JSON object".into()));
        }
        let rows: Vec<serde_json::Map<String, serde_json::Value>> = match spec.get("rows") {
            None | Some(serde_json::Value::Null) => Vec::new(),
            Some(v) => serde_json::from_value(v.clone())
                .map_err(|_| RenderError::BadSpec("`rows` must be an array of objects".into()))?,
        };
        let entries = match spec.get("roles") {
            None => Vec::new(),
            Some(v) => parse_role_entries(v).map_err(RenderError::BadSpec)?,
        };
        let dim = |key: &str, default: u64| -> Result<u32, RenderError> {
            match spec.get(key) {
                None | Some(serde_json::Value::Null) => Ok(clamp_dim(default)),
                Some(v) => v
                    .as_f64()
                    .filter(|f| f.is_finite())
                    .map(|f| clamp_dim(f.max(0.0) as u64))
                    .ok_or_else(|| RenderError::BadSpec(format!("`{key}` must be a number"))),
            }
        };
        let (width, height) = (dim("width", 640)?, dim("height", 400)?);
        let count = |key: &str, default: usize| {
            spec.get(key)
                .and_then(|v| v.as_u64())
                .map(|n| n.min(1_000_000) as usize)
                .unwrap_or(default)
        };
        let opts = RenderOptions {
            brand: spec
                .get("primary")
                .and_then(|v| v.as_str())
                .and_then(parse_rgb),
            zoom: None,
            max_categories: count("max_categories", DEFAULT_MAX_CATEGORIES),
            max_line_points: count("max_line_points", DEFAULT_MAX_LINE_POINTS),
        };
        let cols = columns_from_entries(&rows, &entries);
        render_with(&cols, width, height, &opts)
    })
}

/// Render a panel from a JSON spec (see [`render_spec_checked`]). Returns the
/// SVG, or an HTML-escaped `<pre>message</pre>` on error — kept for hosts that
/// want a string either way.
pub fn render_spec(spec_json: &str) -> String {
    render_spec_checked(spec_json).unwrap_or_else(|e| error_pre(&e.to_string()))
}

/// An HTML-escaped `<pre>` error block.
pub fn error_pre(msg: &str) -> String {
    format!("<pre>{}</pre>", format::escape_xml(msg))
}

/// A small SVG showing an (escaped) error message — for hosts that need an
/// image either way (the wasm entry points).
pub fn error_svg(msg: &str, width: u32) -> String {
    error_svg_at(Place::Doc, msg, width)
}

/// [`error_svg`] at `place` (a dashboard nests it as a fragment).
pub(crate) fn error_svg_at(place: Place, msg: &str, width: u32) -> String {
    format!(
        "{}<text x=\"4\" y=\"24\" font-family=\"system-ui,sans-serif\" font-size=\"12\" fill=\"#b42318\">{}</text></svg>",
        svg_open(place, clamp_dim(width as u64), 40),
        format::escape_xml(msg)
    )
}

/// Where a rendered panel goes: a standalone SVG document, or a nested
/// `<svg x y …>` fragment at `(x, y)` in a parent SVG (no `xmlns`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Place {
    Doc,
    At(f64, f64),
}

/// The opening `<svg>` tag of a `width`×`height` panel at `place`.
pub(crate) fn svg_open(
    place: Place,
    width: impl std::fmt::Display,
    height: impl std::fmt::Display,
) -> String {
    match place {
        Place::Doc => format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"{height}\" viewBox=\"0 0 {width} {height}\">"
        ),
        Place::At(x, y) => format!(
            "<svg x=\"{x:.1}\" y=\"{y:.1}\" width=\"{width}\" height=\"{height}\" viewBox=\"0 0 {width} {height}\">"
        ),
    }
}

/// A panel ready to be written: a ggplot (rendered by ggplot-rs) or one of
/// the extension's own small SVGs (notes, headings, errors).
pub(crate) enum Panel {
    Plot {
        plot: Box<GGPlot>,
        width: u32,
        height: u32,
    },
    Own {
        width: u32,
        height: u32,
        /// The SVG content (without the `<svg>` wrapper).
        body: String,
    },
}

impl Panel {
    fn plot(plot: GGPlot, width: u32, height: u32) -> Panel {
        Panel::Plot {
            plot: Box::new(plot),
            width,
            height,
        }
    }

    /// Write the panel at `place`: the SVG plus the plotting engine's build
    /// warnings (rows dropped for non-finite values, empty layers, …).
    pub(crate) fn finish(self, place: Place) -> Result<(String, Vec<String>), String> {
        match self {
            Panel::Plot {
                plot,
                width,
                height,
            } => {
                let svg = match place {
                    Place::Doc => plot.render_svg_native_with_warnings(width, height),
                    Place::At(x, y) => plot
                        .render_svg_native_at(x, y, width, height)
                        .map(|s| (s, Vec::new())),
                };
                svg.map_err(|e| format!("render failed: {e:?}"))
            }
            Panel::Own {
                width,
                height,
                body,
            } => Ok((
                format!("{}{body}</svg>", svg_open(place, width, height)),
                Vec::new(),
            )),
        }
    }
}

/// Parse `rrggbb` / `#rrggbb`.
pub fn parse_rgb(s: &str) -> Option<(u8, u8, u8)> {
    parse_hex(s).map(|c| (c.r, c.g, c.b))
}

/// Render an annotated result set to SVG using the legacy thread defaults (see
/// [`set_brand`]). Prefer [`render_with`].
pub fn render(cols: &[Column], width: u32, height: u32) -> Result<String, String> {
    let (brand, zoom) = LEGACY.with(|c| c.get());
    let opts = RenderOptions {
        brand,
        zoom,
        ..RenderOptions::default()
    };
    render_with(cols, width, height, &opts).map_err(|e| e.to_string())
}

/// Render an annotated result set to an SVG panel.
///
/// Recognises one `X` column, an optional `Category` column, an optional `Label`
/// (→ title), and one `Value(kind)` column that selects the geom. A result with
/// only a `Label` renders as a heading. Never panics: sizes are clamped to
/// [`MIN_DIM`]`..=`[`MAX_DIM`], non-finite numbers become missing, discrete
/// axes are capped at [`RenderOptions::max_categories`], long lines are
/// LTTB-downsampled, and an internal panic becomes [`RenderError::Panic`].
pub fn render_with(
    cols: &[Column],
    width: u32,
    height: u32,
    o: &RenderOptions,
) -> Result<String, RenderError> {
    render_placed(cols, Place::Doc, width, height, o).map(|(svg, _)| svg)
}

/// Like [`render_with`], but renders a nested `<svg x y width height
/// viewBox>` fragment (no `xmlns`) positioned at `(x, y)` in a parent SVG —
/// for composing dashboards without string surgery.
pub fn render_with_at(
    cols: &[Column],
    x: f64,
    y: f64,
    width: u32,
    height: u32,
    o: &RenderOptions,
) -> Result<String, RenderError> {
    let (x, y) = (
        if x.is_finite() { x } else { 0.0 },
        if y.is_finite() { y } else { 0.0 },
    );
    render_placed(cols, Place::At(x, y), width, height, o).map(|(svg, _)| svg)
}

fn render_placed(
    cols: &[Column],
    place: Place,
    width: u32,
    height: u32,
    o: &RenderOptions,
) -> Result<(String, Vec<String>), RenderError> {
    guard(|| {
        let (w, h) = (clamp_dim(width as u64), clamp_dim(height as u64));
        let cols = downsample::prepare(cols, o);
        let (svg, warnings) = render_inner(&cols, w, h, o)
            .and_then(|p| p.finish(place))
            .map_err(RenderError::Render)?;
        Ok((strip_nonfinite_marks(svg), warnings))
    })
}

/// Geometry attributes whose values must be finite numbers.
const GEOMETRY_ATTRS: &[&str] = &[
    "x",
    "y",
    "x1",
    "y1",
    "x2",
    "y2",
    "cx",
    "cy",
    "r",
    "rx",
    "ry",
    "width",
    "height",
    "points",
    "d",
    "transform",
    "stroke-width",
    "font-size",
    "opacity",
    "fill-opacity",
    "viewBox",
];

/// Does a start tag carry `NaN`/`inf` in a geometry attribute?
fn tag_has_nonfinite(tag: &str) -> bool {
    let mut rest = tag;
    while let Some(eq) = rest.find("=\"") {
        let name = rest[..eq]
            .rsplit(|c: char| c.is_whitespace())
            .next()
            .unwrap_or("");
        let after = &rest[eq + 2..];
        let Some(end) = after.find('"') else {
            return false;
        };
        let val = &after[..end];
        if GEOMETRY_ATTRS.contains(&name)
            && val
                .split(|c: char| !(c.is_ascii_alphanumeric() || c == '.' || c == '-'))
                .any(|t| {
                    let t = t.trim_start_matches('-').to_ascii_lowercase();
                    t == "nan" || t == "inf" || t == "infinity"
                })
        {
            return true;
        }
        rest = &after[end + 1..];
    }
    false
}

/// Safety net: drop any element whose geometry contains a non-finite number
/// (the plotting engine can emit `NaN` for degenerate inputs, e.g. a violin of
/// a constant group). The SVG we write never contains a raw `>` inside an
/// attribute, so a tag scan is exact.
fn strip_nonfinite_marks(svg: String) -> String {
    if !(svg.contains("NaN") || svg.contains("inf")) {
        return svg;
    }
    let mut out = String::with_capacity(svg.len());
    let mut i = 0;
    while let Some(lt) = svg[i..].find('<') {
        let start = i + lt;
        out.push_str(&svg[i..start]);
        let Some(gt) = svg[start..].find('>') else {
            out.push_str(&svg[start..]);
            return out;
        };
        let tag_end = start + gt + 1;
        let tag = &svg[start..tag_end];
        if !tag.starts_with("</") && !tag.starts_with("<svg") && tag_has_nonfinite(tag) {
            if tag.ends_with("/>") {
                i = tag_end;
                continue;
            }
            let name: String = tag[1..]
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric())
                .collect();
            let close = format!("</{name}>");
            i = svg[tag_end..]
                .find(&close)
                .map(|p| tag_end + p + close.len())
                .unwrap_or(tag_end);
            continue;
        }
        out.push_str(tag);
        i = tag_end;
    }
    out.push_str(&svg[i..]);
    out
}

fn render_inner(
    cols: &[Column],
    width: u32,
    height: u32,
    o: &RenderOptions,
) -> Result<Panel, String> {
    let title = cols
        .iter()
        .find(|c| c.role == Role::Label)
        .and_then(|c| c.values.first())
        .map(value_str);

    // A map is driven by a ::MAP (geometry) column, coloured by an optional measure.
    if let Some(g) = cols.iter().find(|c| c.role == Role::Geometry) {
        if !g
            .values
            .iter()
            .any(|v| matches!(v, Value::Str(s) if !s.is_empty()))
        {
            return Ok(note_svg(None, "No data", width, height));
        }
        return render_map(o, cols, title.as_deref(), width, height);
    }
    let value = cols.iter().find(|c| matches!(c.role, Role::Value(_)));
    let Some(value) = value else {
        // Label-only → a heading element.
        return Ok(heading_svg(title.as_deref().unwrap_or(""), width));
    };
    let Role::Value(kind) = value.role else {
        return Err("no measure column".into());
    };
    // Empty / too-small inputs get a clear note instead of a confusing
    // "requires aesthetic 'x'" error from the plotting engine.
    let n_ok = value.values.iter().filter(|v| v.as_f64().is_some()).count();
    if value.values.is_empty() {
        return Ok(note_svg(title.as_deref(), "No data", width, height));
    }
    if n_ok == 0 {
        return Ok(note_svg(
            title.as_deref(),
            "No numeric values to plot",
            width,
            height,
        ));
    }
    let need = min_values(kind);
    if n_ok < need {
        return Ok(note_svg(
            title.as_deref(),
            &format!(
                "::{} needs at least {need} values (got {n_ok})",
                value.role.token()
            ),
            width,
            height,
        ));
    }
    match kind {
        Kind::Pie => return render_pie(value, cols, title.as_deref(), 0.0, width, height),
        Kind::Donut => return render_pie(value, cols, title.as_deref(), 0.55, width, height),
        Kind::Gauge => return render_gauge(o, value, cols, title.as_deref(), width, height),
        Kind::Histogram => return render_histogram(o, value, title.as_deref(), width, height),
        Kind::Density => return render_density(o, value, cols, title.as_deref(), width, height),
        Kind::QQ => return render_qq(o, value, title.as_deref(), width, height),
        Kind::Heatmap => return render_heatmap(o, value, cols, title.as_deref(), width, height),
        Kind::Calendar => return render_calendar(o, value, cols, title.as_deref(), width, height),
        Kind::Candlestick => {
            return render_candlestick(value, cols, title.as_deref(), width, height)
        }
        Kind::Radar => return render_radar(o, value, cols, title.as_deref(), width, height),
        Kind::Sparkline => return render_sparkline(o, value, width, height),
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
        Kind::Line
            | Kind::LinePercent
            | Kind::Point
            | Kind::Bubble
            | Kind::Step
            | Kind::Smooth
            | Kind::Jitter
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
    // the same channel's colour in the other charts. `color_col` holds the
    // series levels; `x_coloured` suppresses the then-redundant legend.
    let mut x_coloured = false;
    let color_col: Option<&Column> = if let Some(cat) = category {
        data.push(("cat".to_string(), cat.values.clone()));
        aes = if by_colour {
            aes.color("cat")
        } else {
            aes.fill("cat")
        };
        Some(cat)
    } else if bar && x_discrete {
        aes = aes.fill("x");
        x_coloured = true;
        Some(x)
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

    // Single-series marks take the brand colour (geoms added with explicit
    // `geom_*_with` styles below set it themselves; mapped colours win).
    let brand = o.brand();
    let mut plot = GGPlot::new(data).aes(aes).primary_color(brand);

    // Shaded x-region (`::MARKAREA`): a light band behind the data spanning
    // [min, max] of the mark column's x-values, the full panel height
    // (ymin/ymax = ∓Inf reach the panel edges and don't train the y scale).
    if let Some(ma) = cols.iter().find(|c| c.role == Role::MarkArea) {
        let xs = ma.values.iter().filter(|v| v.as_f64().is_some());
        let by_x = |a: &&Value, b: &&Value| {
            a.as_f64()
                .partial_cmp(&b.as_f64())
                .unwrap_or(std::cmp::Ordering::Equal)
        };
        if let (Some(x0), Some(x1)) = (xs.clone().min_by(by_x), xs.max_by(by_x)) {
            let frame = vec![
                ("mx0".to_string(), vec![x0.clone()]),
                ("mx1".to_string(), vec![x1.clone()]),
                ("my0".to_string(), vec![Value::Float(f64::NEG_INFINITY)]),
                ("my1".to_string(), vec![Value::Float(f64::INFINITY)]),
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

    // A ::BAND (prediction interval) matches the forecast — the last coloured
    // series — at half opacity, so it reads as that series' uncertainty.
    let band_color: (u8, u8, u8) = color_col.and_then(last_level_color).unwrap_or(brand);
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
        color: brand,
        width: 1.0,
        ..Default::default()
    };
    let small_point = || GeomPoint {
        color: brand,
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
                color: brand,
                width: 1.2,
                ..Default::default()
            })
            .geom_point_with(small_point()),
        // Scatter + a LOESS trend line (no CI ribbon) — an analytical "smooth".
        Kind::Smooth => plot
            .geom_point_with(small_point())
            .geom_smooth_with(GeomSmooth {
                color: brand,
                se: false,
                line_width: 2.0,
                method: ggplot_rs::stat::smooth::SmoothMethod::Loess { span: 0.75 },
                ..Default::default()
            }),
        Kind::Area => plot.geom_area().geom_point_with(small_point()),
        Kind::AreaStacked => plot
            .geom_area_with(GeomArea {
                fill: brand,
                color: brand,
                alpha: 0.85,
                ..Default::default()
            })
            .position(PositionStack),
        Kind::Point | Kind::Bubble => plot.geom_point(),
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
                    color: brand,
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
    if let Some(col) = color_col {
        // DataZoo palette, levels sorted so a series keeps its colour across
        // charts; `#rrggbb` levels are drawn in that colour.
        plot = if by_colour {
            plot.scale_color(dz_scale(Aesthetic::Color, col))
        } else {
            plot.scale_fill(dz_scale(Aesthetic::Fill, col))
        };
    }
    if !combo_names.is_empty() {
        // Combo measures → distinct palette colours in column order, keyed by
        // column header.
        plot = plot.scale_color(
            ScaleColorDiscrete::new(Aesthetic::Color)
                .with_levels(combo_names.clone())
                .with_palette((0..DZ_COLORS.len()).map(dz_color).collect()),
        );
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
        if let Some((xlim, ylim)) = o.zoom {
            plot = plot.coord_cartesian_zoom(Some(xlim), Some(ylim));
        }
    }
    plot = plot.theme_minimal();
    plot = plot.legend_position(ggplot_rs::theme::LegendPosition::Top);
    if let Some(t) = &title {
        plot = plot.title(t);
    }
    Ok(Panel::plot(plot, width, height))
}

/// A histogram of the measure column (ggplot bins + counts).
fn render_histogram(
    o: &RenderOptions,
    value: &Column,
    title: Option<&str>,
    width: u32,
    height: u32,
) -> Result<Panel, String> {
    let data = vec![("x".to_string(), value.values.clone())];
    let mut plot = GGPlot::new(data)
        .aes(Aes::new().x("x"))
        .geom_histogram()
        .theme_minimal()
        .primary_color(o.brand());
    if let Some(t) = title {
        plot = plot.title(t);
    }
    Ok(Panel::plot(plot, width, height))
}

/// A kernel-density curve of the measure column. An optional `CATEGORY` splits
/// it into one filled curve per group (overlaid, semi-transparent).
fn render_density(
    o: &RenderOptions,
    value: &Column,
    cols: &[Column],
    title: Option<&str>,
    width: u32,
    height: u32,
) -> Result<Panel, String> {
    let category = cols.iter().find(|c| c.role == Role::Category);
    let mut data: Vec<(String, Vec<Value>)> = vec![("x".to_string(), value.values.clone())];
    let mut aes = Aes::new().x("x");
    let mut plot;
    if let Some(cat) = category {
        data.push(("cat".to_string(), cat.values.clone()));
        aes = aes.fill("cat").color("cat");
        plot = GGPlot::new(data)
            .aes(aes)
            .geom_density_with(GeomDensity {
                alpha: 0.4,
                ..Default::default()
            })
            .scale_fill(dz_scale(Aesthetic::Fill, cat))
            .scale_color(dz_scale(Aesthetic::Color, cat))
            .theme_minimal()
            .legend_position(ggplot_rs::theme::LegendPosition::Top);
    } else {
        plot = GGPlot::new(data)
            .aes(aes)
            .geom_density()
            .theme_minimal()
            .primary_color(o.brand());
    }
    if let Some(t) = title {
        plot = plot.title(t);
    }
    Ok(Panel::plot(plot, width, height))
}

/// Build an axis tick formatter from a short spec — a keyword or a currency
/// symbol — mirroring ggplot2's `scales::` label helpers. Numbers are always
/// thousands-grouped. A leading space marks a suffix ("` kg`" → "12 kg");
/// otherwise the spec is a prefix ("€" → "€1,200", "CHF " → "CHF 1,200").
fn axis_formatter(spec: &str) -> Option<Box<dyn Fn(f64) -> String + Send + Sync>> {
    use ggplot_rs::scale::format::{label_comma, label_dollar};
    let s = spec.trim_end_matches(';');
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
    o: &RenderOptions,
    value: &Column,
    title: Option<&str>,
    width: u32,
    height: u32,
) -> Result<Panel, String> {
    let data = vec![("y".to_string(), value.values.clone())];
    let mut plot = GGPlot::new(data)
        .aes(Aes::new().y("y"))
        .geom_qq()
        .geom_qq_line()
        .xlab("Theoretical")
        .ylab("Sample")
        .theme_minimal()
        .primary_color(o.brand());
    if let Some(t) = title {
        plot = plot.title(t);
    }
    Ok(Panel::plot(plot, width, height))
}

/// A heatmap: `x` × `y` tiles coloured by the measure (light → steel blue).
fn render_heatmap(
    o: &RenderOptions,
    value: &Column,
    cols: &[Column],
    title: Option<&str>,
    width: u32,
    height: u32,
) -> Result<Panel, String> {
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
            ggplot_rs::scale::color::RGBAColor::new(o.brand().0, o.brand().1, o.brand().2),
        )
        .theme_minimal();
    if let Some(t) = title {
        plot = plot.title(t);
    }
    Ok(Panel::plot(plot, width, height))
}

/// Max span a calendar heatmap draws (one column per week; beyond this the
/// cells are sub-pixel). Longer spans are rejected with a clear error rather
/// than silently clipped (ggplot-rs would clip to the most recent years).
pub const MAX_CALENDAR_YEARS: i64 = ggplot_rs::stat::calendar::MAX_CALENDAR_YEARS;

/// A GitHub-style calendar heatmap (`geom_calendar`): a date `::XAXIS` + a
/// measure laid out as week-columns × weekday-rows (month labels on top,
/// Mon/Wed/Fri down the left), cells coloured light → brand by value. Spans
/// longer than [`MAX_CALENDAR_YEARS`] are rejected with a clear error.
fn render_calendar(
    o: &RenderOptions,
    value: &Column,
    cols: &[Column],
    title: Option<&str>,
    width: u32,
    height: u32,
) -> Result<Panel, String> {
    use ggplot_rs::stat::calendar::day_number;
    let x = cols
        .iter()
        .find(|c| c.role == Role::X)
        .ok_or("calendar needs an XAXIS date column")?;
    // Days since the epoch. ggplot-rs ignores out-of-range epoch numbers; count
    // them here (clamped, so they can't overflow) so the span check rejects
    // them instead of silently dropping the row.
    let day_of = |v: &Value| {
        day_number(v).or_else(|| {
            v.as_f64()
                .filter(|f| f.is_finite())
                .map(|secs| (secs / 86_400.0).floor().clamp(-1e12, 1e12) as i64)
        })
    };
    // Only rows with a date and a finite measure become cells.
    let (mut dates, mut vals, mut days) = (Vec::new(), Vec::new(), Vec::new());
    for (dv, vv) in x.values.iter().zip(value.values.iter()) {
        if let (Some(day), Some(val)) = (day_of(dv), vv.as_f64().filter(|f| f.is_finite())) {
            dates.push(dv.clone());
            vals.push(Value::Float(val));
            days.push(day);
        }
    }
    let (Some(min_day), Some(max_day)) = (days.iter().min(), days.iter().max()) else {
        return Ok(note_svg(title, "calendar needs a date axis", width, height));
    };
    let span_years = (max_day - min_day) / 365;
    if span_years > MAX_CALENDAR_YEARS {
        return Err(format!(
            "calendar spans {span_years} years — at most {MAX_CALENDAR_YEARS} are drawn; filter the date range"
        ));
    }
    let labels: Vec<Value> = vals.iter().map(|v| Value::Str(fmt_label(v))).collect();
    let data = vec![
        ("x".to_string(), dates),
        ("fill".to_string(), vals),
        ("label".to_string(), labels),
    ];
    let mut plot = GGPlot::new(data)
        .aes(Aes::new().x("x").fill("fill").label("label"))
        .geom_calendar()
        .scale_fill_gradient(rgba((0xeb, 0xf1, 0xf7)), rgba(o.brand()))
        .xlab("")
        .ylab("")
        .theme_minimal();
    if let Some(t) = title {
        plot = plot.title(t);
    }
    Ok(Panel::plot(plot, width, height))
}

/// An OHLC candlestick chart (`geom_candlestick`): `::XAXIS` period,
/// `::OPEN`/`::HIGH`/`::LOW` prices and the close as the measure
/// (`::CANDLESTICK`). Up candles green, down red; each candle's hover reads
/// "period — O … H … L … C …".
fn render_candlestick(
    value: &Column,
    cols: &[Column],
    title: Option<&str>,
    width: u32,
    height: u32,
) -> Result<Panel, String> {
    let find = |role: Role, msg: &'static str| cols.iter().find(|c| c.role == role).ok_or(msg);
    let x = find(Role::X, "candlestick needs an XAXIS column")?;
    let open = find(Role::Open, "candlestick needs an ::OPEN column")?;
    let high = find(Role::High, "candlestick needs a ::HIGH column")?;
    let low = find(Role::Low, "candlestick needs a ::LOW column")?;
    let complete = (0..value.values.len()).any(|i| {
        [open, high, low, value]
            .iter()
            .all(|c| c.values.get(i).and_then(|v| v.as_f64()).is_some())
    });
    if !complete {
        return Ok(note_svg(
            title,
            "candlestick needs numeric OHLC",
            width,
            height,
        ));
    }
    let data = vec![
        ("x".to_string(), x.values.clone()),
        ("open".to_string(), open.values.clone()),
        ("high".to_string(), high.values.clone()),
        ("low".to_string(), low.values.clone()),
        ("close".to_string(), value.values.clone()),
    ];
    let mut plot = GGPlot::new(data)
        .aes(
            Aes::new()
                .x("x")
                .open("open")
                .high("high")
                .low("low")
                .close("close"),
        )
        .geom_candlestick()
        .xlab("")
        .ylab("")
        .theme_minimal();
    if let Some(t) = title {
        plot = plot.title(t);
    }
    Ok(Panel::plot(plot, width, height))
}

/// A radar / spider chart (`coord_radar`): axes from the `::XAXIS` (metric
/// names), values from the measure (`::RADAR`), one translucent polygon (+
/// vertex markers) per `::CATEGORY` series.
fn render_radar(
    o: &RenderOptions,
    value: &Column,
    cols: &[Column],
    title: Option<&str>,
    width: u32,
    height: u32,
) -> Result<Panel, String> {
    let x = cols
        .iter()
        .find(|c| c.role == Role::X)
        .ok_or("radar needs an XAXIS (axis) column")?;
    let category = cols.iter().find(|c| c.role == Role::Category);
    // Spokes in first-seen order (the order the query lists the metrics).
    let mut axes: Vec<String> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for v in &x.values {
        let s = value_str(v);
        if seen.insert(s.clone()) {
            axes.push(s);
        }
    }
    if axes.len() < 3 {
        return Ok(note_svg(
            title,
            "radar needs at least 3 axes",
            width,
            height,
        ));
    }
    let x_str: Vec<Value> = x.values.iter().map(|v| Value::Str(value_str(v))).collect();
    let mut data = vec![
        ("x".to_string(), x_str),
        ("y".to_string(), value.values.clone()),
    ];
    let mut aes = Aes::new().x("x").y("y").label("label");
    let label = match category {
        Some(cat) => {
            data.push(("cat".to_string(), cat.values.clone()));
            aes = aes.color("cat").fill("cat");
            cat.values.clone()
        }
        None => x.values.clone(),
    };
    data.push(("label".to_string(), label));
    let brand = o.brand();
    let mut plot = GGPlot::new(data)
        .aes(aes)
        .geom_polygon_with(GeomPolygon {
            fill: brand,
            color: brand,
            alpha: 0.18,
            line_width: 1.6,
        })
        .geom_point_with(GeomPoint {
            size: 2.6,
            color: brand,
            alpha: 1.0,
        })
        .scale_x_discrete(
            ggplot_rs::scale::discrete::ScaleDiscrete::new()
                .with_limits(axes.iter().map(String::as_str).collect()),
        )
        .coord_radar_with(ggplot_rs::coord::radar::CoordRadar::new().radius_frac(0.86))
        .xlab("")
        .ylab("")
        .theme_minimal()
        .legend_position(ggplot_rs::theme::LegendPosition::Top);
    if let Some(cat) = category {
        plot = plot
            .scale_color(dz_scale(Aesthetic::Color, cat))
            .scale_fill(dz_scale(Aesthetic::Fill, cat));
    }
    if let Some(t) = title {
        plot = plot.title(t);
    }
    Ok(Panel::plot(plot, width, height))
}

fn rgba((r, g, b): (u8, u8, u8)) -> ggplot_rs::scale::color::RGBAColor {
    ggplot_rs::scale::color::RGBAColor::new(r, g, b)
}

/// A minimal inline trend line (no axes) — a sparkline over the row order: a
/// light steel area under a crisp line, with a marker on the latest value.
fn render_sparkline(
    o: &RenderOptions,
    value: &Column,
    width: u32,
    height: u32,
) -> Result<Panel, String> {
    let n = value.values.len();
    let xs: Vec<Value> = (0..n).map(|i| Value::Float(i as f64)).collect();
    let data = vec![
        ("x".to_string(), xs),
        ("y".to_string(), value.values.clone()),
    ];

    let steel = o.brand();
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

    let plot = GGPlot::new(data)
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
        .theme_void();
    // Render at a compact width so strokes stay crisp when the inline
    // sparkline is scaled down into a narrow panel column.
    Ok(Panel::plot(plot, width.min(240), height))
}

/// Blend a colour toward white by `t` (0 = unchanged, 1 = white).
fn lighten((r, g, b): (u8, u8, u8), t: f64) -> (u8, u8, u8) {
    let f = |c: u8| (c as f64 + (255.0 - c as f64) * t).round() as u8;
    (f(r), f(g), f(b))
}

/// Distinct finite numeric values from a column, in first-seen order — used to
/// draw one reference line per value.
fn distinct_nums(vals: &[Value]) -> Vec<f64> {
    // O(n) dedup on the exact bit pattern (+0.0 normalised); capped so a
    // pathological column can't draw thousands of reference lines.
    let mut seen = std::collections::HashSet::new();
    let mut out: Vec<f64> = Vec::new();
    for v in vals {
        if let Some(f) = v.as_f64() {
            let key = if f == 0.0 { 0u64 } else { f.to_bits() };
            if f.is_finite() && seen.insert(key) {
                out.push(f);
                if out.len() >= 64 {
                    break;
                }
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

/// A gauge: a 270° arc (opening downward) showing a single value within a
/// `min,max` `::RANGE` (default `0,100`) — `geom_rect` bands under
/// `CoordPolar::with_span`, a `geom_segment` needle at the value and the value
/// in the centre. Optional `::COLORS` paints equal threshold zones (else a
/// light track with a brand-coloured progress band); optional `::LABELS`
/// names the zones (drawn outside the arc; without `::COLORS` they still split
/// the arc into equal zones).
fn render_gauge(
    o: &RenderOptions,
    value: &Column,
    cols: &[Column],
    title: Option<&str>,
    width: u32,
    height: u32,
) -> Result<Panel, String> {
    use ggplot_rs::scale::continuous::ScaleContinuous;
    let val = value
        .values
        .iter()
        .find_map(|v| v.as_f64().filter(|f| f.is_finite()))
        .unwrap_or(0.0);
    let (min, max) = gauge_range(cols);
    let span = max - min;
    let at = |frac: f64| min + span * frac;
    let shown = val.clamp(min, max);

    // Optional zone colours (comma-separated hex) and labels (`'low,ok,high'`).
    let zone_cols: Vec<(u8, u8, u8)> = cols
        .iter()
        .find(|c| c.role == Role::GaugeColors)
        .and_then(|c| c.values.first())
        .map(value_str)
        .map(|s| s.split(',').filter_map(parse_rgb).take(64).collect())
        .unwrap_or_default();
    let zone_labels: Vec<String> = cols
        .iter()
        .find(|c| c.role == Role::GaugeLabels)
        .and_then(|c| c.values.first())
        .map(value_str)
        .map(|s| {
            s.split(',')
                .map(|t| t.trim().to_string())
                .take(64)
                .collect()
        })
        .unwrap_or_default();
    let n_zones = if zone_cols.is_empty() {
        zone_labels.len()
    } else {
        zone_cols.len()
    };

    // Radius (y) layout in 0..1: the band, the needle across it, labels
    // outside it; the centre (y = 0) holds the value.
    let (band0, band1, label_r) = (0.64, 0.80, 0.93);
    let num = |vals: Vec<f64>| vals.into_iter().map(Value::Float).collect::<Vec<_>>();
    let text = |vals: Vec<String>| vals.into_iter().map(Value::Str).collect::<Vec<_>>();

    // Bands: the zones, or a light track + a brand progress band.
    let (mut x0, mut x1, mut fill, mut pairs) = (vec![], vec![], vec![], vec![]);
    if zone_cols.is_empty() {
        x0.extend([min, min]);
        x1.extend([max, shown]);
        fill.extend(["track".to_string(), "value".to_string()]);
        pairs.push(("track".to_string(), lighten(o.brand(), 0.87)));
        pairs.push(("value".to_string(), o.brand()));
    } else {
        for (i, c) in zone_cols.iter().enumerate() {
            let n = zone_cols.len() as f64;
            x0.push(at(i as f64 / n));
            x1.push(at((i + 1) as f64 / n));
            fill.push(format!("z{i}"));
            pairs.push((format!("z{i}"), *c));
        }
    }
    let n_bands = x0.len();
    let bands = vec![
        ("x0".to_string(), num(x0)),
        ("x1".to_string(), num(x1)),
        ("y0".to_string(), num(vec![band0; n_bands])),
        ("y1".to_string(), num(vec![band1; n_bands])),
        ("fill".to_string(), text(fill)),
    ];
    let mut plot = GGPlot::new(bands)
        .geom_rect_with(GeomRect {
            line_width: 0.0,
            alpha: 1.0,
            ..Default::default()
        })
        .layer_aes(
            Aes::new()
                .xmin("x0")
                .xmax("x1")
                .ymin("y0")
                .ymax("y1")
                .fill("fill"),
        );
    // Zone boundaries: thin white separators across the band (label-only zones).
    if zone_cols.is_empty() && n_zones > 1 {
        let xs: Vec<f64> = (1..n_zones)
            .map(|i| at(i as f64 / n_zones as f64))
            .collect();
        let k = xs.len();
        plot = plot
            .geom_segment_with(GeomSegment {
                color: (255, 255, 255),
                width: 2.0,
                alpha: 1.0,
            })
            .layer_data(vec![
                ("x".to_string(), num(xs.clone())),
                ("y".to_string(), num(vec![band0 - 0.04; k])),
                ("yend".to_string(), num(vec![band1 + 0.04; k])),
            ])
            .layer_aes(Aes::new().x("x").xend("x").y("y").yend("yend"));
    }
    // The needle: a dark bar across the band at the (clamped) value.
    plot = plot
        .geom_segment_with(GeomSegment {
            color: (31, 41, 55),
            width: 3.5,
            alpha: 1.0,
        })
        .layer_data(vec![
            ("x".to_string(), num(vec![shown])),
            ("y".to_string(), num(vec![band0 - 0.08])),
            ("yend".to_string(), num(vec![band1 + 0.05])),
        ])
        .layer_aes(Aes::new().x("x").xend("x").y("y").yend("yend"));

    // Text: zone labels outside the arc, min/max under the arc ends, the value
    // and an "of max" caption in the centre.
    let text_layer = |plot: GGPlot, xs: Vec<f64>, r: f64, labels: Vec<String>, t: GeomText| {
        let k = xs.len();
        plot.geom_text_with(t)
            .layer_data(vec![
                ("x".to_string(), num(xs)),
                ("y".to_string(), num(vec![r; k])),
                ("label".to_string(), text(labels)),
            ])
            .layer_aes(Aes::new().x("x").y("y").label("label"))
    };
    let grey = (0x5a, 0x64, 0x72);
    if n_zones > 0 {
        let (xs, ls): (Vec<f64>, Vec<String>) = zone_labels
            .iter()
            .take(n_zones)
            .enumerate()
            .filter(|(_, l)| !l.is_empty())
            .map(|(i, l)| (at((i as f64 + 0.5) / n_zones as f64), l.clone()))
            .unzip();
        if !xs.is_empty() {
            plot = text_layer(
                plot,
                xs,
                label_r,
                ls,
                GeomText {
                    size: 10.0,
                    color: grey,
                    ..Default::default()
                },
            );
        }
    }
    let ends = GeomText {
        size: 11.0,
        color: (0x8a, 0x93, 0xa6),
        // Shift below the arc ends (text is centred on its anchor).
        vjust: 0.5 + 16.0 / 11.0,
        ..Default::default()
    };
    plot = text_layer(
        plot,
        vec![min, max],
        (band0 + band1) / 2.0,
        vec![fmt_g(min), fmt_g(max)],
        ends,
    );
    let big = (width.min(height) as f64 * 0.17).clamp(14.0, 64.0);
    plot = plot.annotate(Annotation::Text {
        label: fmt_g(val),
        x: at(0.5),
        y: 0.0,
        size: big,
        color: (0x1f, 0x29, 0x37),
    });
    let caption = GeomText {
        size: 11.0,
        color: (0x8a, 0x93, 0xa6),
        vjust: 0.5 + (big * 0.8) / 11.0,
        ..Default::default()
    };
    plot = text_layer(
        plot,
        vec![at(0.5)],
        0.0,
        vec![format!("of {}", fmt_g(max))],
        caption,
    );

    let fills: Vec<(&str, ggplot_rs::scale::color::RGBAColor)> =
        pairs.iter().map(|(k, c)| (k.as_str(), rgba(*c))).collect();
    let quarter = std::f64::consts::FRAC_PI_4;
    plot = plot
        .scale_fill_manual(fills)
        .scale_x_continuous(
            ScaleContinuous::new()
                .with_limits(min, max)
                .with_expand(0.0, 0.0),
        )
        .scale_y_continuous(
            ScaleContinuous::new()
                .with_limits(0.0, 1.0)
                .with_expand(0.0, 0.0),
        )
        .coord_polar_with(
            ggplot_rs::coord::polar::CoordPolar::new().with_span(-3.0 * quarter, 3.0 * quarter),
        )
        .theme_void()
        .legend_position(ggplot_rs::theme::LegendPosition::None);
    if let Some(t) = title {
        plot = plot.title(t);
    }
    Ok(Panel::plot(plot, width, height))
}

/// A gauge's `::RANGE` — `'min,max'` (or `'min;max'`, or a 2-element list).
/// Anything else (a single number, NULL, NaN, min == max) falls back to
/// `0..100`; a reversed range is swapped.
fn gauge_range(cols: &[Column]) -> (f64, f64) {
    let parsed = cols
        .iter()
        .find(|c| c.role == Role::Range)
        .and_then(|c| c.values.first())
        .map(value_str)
        .and_then(|s| {
            let p: Vec<f64> = s
                .trim_matches(|c| c == '[' || c == ']')
                .split([',', ';'])
                .filter_map(|t| t.trim().parse::<f64>().ok())
                .filter(|f| f.is_finite())
                .collect();
            match p.as_slice() {
                [a, b] if (a - b).abs() > 1e-12 => Some((a.min(*b), a.max(*b))),
                _ => None,
            }
        });
    parsed.unwrap_or((0.0, 100.0))
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
    o: &RenderOptions,
    cols: &[Column],
    _title: Option<&str>,
    width: u32,
    height: u32,
) -> Result<Panel, String> {
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
    let mut plot = match o.zoom {
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
                ggplot_rs::scale::color::RGBAColor::new(o.brand().0, o.brand().1, o.brand().2),
            )
        };
    }
    Ok(Panel::plot(plot, width, height))
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
) -> Result<Panel, String> {
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
    let mut plot = GGPlot::new(data)
        .aes(Aes::new().x("x").y("y").fill("cat").label("label"))
        .geom_col()
        .position(PositionStack)
        .scale_fill(dz_scale(Aesthetic::Fill, category))
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
    Ok(Panel::plot(plot, width, height))
}

/// A minimal SVG heading (for a `::LABEL`-only result).
fn heading_svg(text: &str, width: u32) -> Panel {
    Panel::Own {
        width,
        height: 40,
        body: format!(
            "<text x=\"4\" y=\"26\" font-family=\"system-ui,sans-serif\" font-size=\"20\" font-weight=\"600\" fill=\"#1f2430\">{}</text>",
            format::escape_xml(text)
        ),
    }
}

/// A full-size placeholder panel with a centred note ("No data", "needs ≥ 2
/// values", …) and the optional title — instead of a confusing ggplot error.
fn note_svg(title: Option<&str>, note: &str, width: u32, height: u32) -> Panel {
    let (w, h) = (width as f64, height as f64);
    Panel::Own {
        width,
        height,
        body: format!(
            "{}<text x=\"{:.1}\" y=\"{:.1}\" text-anchor=\"middle\" dominant-baseline=\"middle\" font-family=\"system-ui,sans-serif\" font-size=\"13\" fill=\"#8a93a6\">{}</text>",
            title_text(title, w),
            w / 2.0,
            h / 2.0,
            format::escape_xml(note)
        ),
    }
}

/// The `<text>` title strip of a [`note_svg`] placeholder, or `""` without a
/// title.
fn title_text(title: Option<&str>, w: f64) -> String {
    match title.filter(|t| !t.is_empty()) {
        Some(t) => format!(
            "<text x=\"{:.1}\" y=\"18\" text-anchor=\"middle\" font-family=\"system-ui,sans-serif\" font-size=\"14\" font-weight=\"700\" fill=\"#1f2430\">{}</text>",
            w / 2.0,
            format::escape_xml(t)
        ),
        None => String::new(),
    }
}

/// Minimum number of non-missing measure values a chart kind needs to draw
/// something meaningful (`None` = no requirement beyond "some data").
fn min_values(kind: Kind) -> usize {
    match kind {
        Kind::Density | Kind::Violin | Kind::QQ | Kind::Boxplot => 2,
        Kind::Smooth => 3,
        _ => 1,
    }
}
