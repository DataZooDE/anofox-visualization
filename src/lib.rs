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

mod boxstats;
pub mod contract;
pub mod dashboard;
pub mod diagram;
mod downsample;
mod facet_position;
pub mod format;
pub mod freshness;
pub mod host;
pub mod inputs;
pub mod kpi;
mod legend_wrap;
pub mod lint;
pub mod macros;
pub(crate) mod map;
pub mod math;
pub mod page;
pub mod period;
pub mod roles;
mod smooth;
pub mod sql;
pub mod table;
pub mod theme;
pub mod thresholds;
pub mod ticks;
pub mod track;
mod variance;
#[cfg(feature = "wasm")]
pub mod wasm;

#[cfg(test)]
mod build_version;

/// The build's version as people see it — the builder's sidebar footer,
/// `dashboard`/`serve --version`, the locked server's `/healthz`:
/// `v2026.10.18` for a release tag build, else `v2026.10.18+37 · 739aba4`
/// (commits since the tag · short sha, `-dirty` with uncommitted changes),
/// `v<Cargo.toml version>` without git. Stamped by build.rs from `git
/// describe` (#216). Extension metadata keeps the plain CalVer.
pub const BUILD_VERSION: &str = env!("ANOFOX_VIZ_BUILD_VERSION");

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
    /// A bridge from a start total to an end total through signed steps
    /// (`::WATERFALL`).
    Waterfall,
    /// A measure against a `::GOAL` over `::RANGE` bands, one row per
    /// category (`::BULLET`).
    Bullet,
    /// Ordered conversion stages, widths by value (`::FUNNEL`).
    Funnel,
}

impl Kind {
    /// Drawn on plain cartesian axes by the main renderer, which honours a
    /// [`RenderOptions::zoom`] window (bars, lines, areas, scatters, box/violin
    /// plots). The browser only offers scroll/drag zoom for these.
    pub fn zoomable(self) -> bool {
        !matches!(
            self,
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
                | Kind::Sparkline
                | Kind::Waterfall
                | Kind::Bullet
                | Kind::Funnel
        )
    }
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
    /// `::PERIOD`: date presets + grain + comparison ([`period`]).
    Period,
    /// A segmented button group (`::BUTTONS`): one of a few options.
    Buttons,
    /// A boolean switch (`::TOGGLE`): `TRUE` / `FALSE`.
    Toggle,
}

/// How a KPI value is formatted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MetricFmt {
    Plain,
    Money,
    Percent,
    Compact,
}

/// Direction of a table column's colour scale (`::COLORSCALE…`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scale {
    /// Low values green, high values red (`::COLORSCALE`).
    LowGood,
    /// High values green, low values red (`::COLORSCALE_HIGH_GOOD`).
    HighGood,
    /// Diverging around zero: negative red, positive green (`::COLORSCALE_DIVERGING`).
    Diverging,
}

/// A "next table" option directive; the value configures the next table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TableOpt {
    /// `::TOTALS`: the totals row's aggregation (`'sum'`, …, `'none'`, or per column).
    Totals,
    /// `::COLGROUP`: a header row spanning labels over columns.
    ColGroup,
    /// `::FLAG`: mark cells meeting a condition (`'MAPE > 0.2'`).
    Flag,
    /// `::HIGHLIGHT`: emphasise the top / bottom n of a column (`'Revenue: top 3'`).
    Highlight,
    /// `::SUBTOTAL`: a subtotal row after each group of a column (`'region'`).
    Subtotal,
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
    /// Layout: `::ROWSELECT` sets how the *next* table's rows can be selected:
    /// `'none'` (default), `'single'` or `'multi'` (the value).
    RowSelect,
    /// Layout: an option for the *next* table (the value), see [`TableOpt`].
    TableOpt(TableOpt),
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
    /// A comparison value where lower is better (`::DELTA_LOWER_GOOD`: churn,
    /// cost, errors) — the same arrow, a rise drawn in the negative colour.
    DeltaLowerGood,
    /// How a KPI's change reads (`::DELTA_FORMAT`): `'pct'` (default),
    /// `'abs'` (the absolute change in the KPI's own format) or `'both'`.
    DeltaFormat,
    /// What a KPI's change compares against (`::DELTA_LABEL`), e.g.
    /// `'vs last month'` — shown after the change.
    DeltaLabel,
    /// A horizontal reference/target line on a chart (`::REFLINE`/`::YLINE`);
    /// in a KPI statement (`::GOAL`/`::TARGET`) the KPI's goal → a progress bar.
    RefLine,
    /// A vertical reference line at an x-position (`::XLINE`).
    VLine,
    /// Lower edge of a confidence band around a line (`::BAND_LOWER`).
    BandLower,
    /// Upper edge of a confidence band around a line (`::BAND_UPPER`).
    BandUpper,
    /// A trend arrow rendered inside a table cell (`::TREND`).
    Trend,
    /// Heatmap-colour a table column's cells by value (`::COLORSCALE`,
    /// `::COLORSCALE_HIGH_GOOD`, `::COLORSCALE_DIVERGING`).
    ColorScale(Scale),
    /// Render a table column's text as coloured status pills (`::BADGE`).
    Badge,
    /// A plain table column with no in-cell bar (`::PLAIN`/`::NOBAR`).
    Plain,
    /// A count/metadata hint shown next to a dropdown option (`::HINT`).
    Hint,
    /// The shown text of a `::DROPDOWN`/`::BUTTONS`/`::MULTISELECT` option (`::OPTION_LABEL`); the variable keeps the value (#170).
    OptionLabel,
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
    /// The zoom group of a `::MAP` (`::SYNC_ZOOM`): maps with the same name
    /// zoom and pan together in the browser. Read from the first value.
    SyncZoom,
    /// A map's projection (`::PROJECTION`): `'auto'` (by the data's extent),
    /// `'equal-earth'`, `'mercator'`, `'albers-europe'`, `'conic'`, `'utm'`
    /// or `'none'` (planar coordinates). Read from the first value.
    Projection,
    /// A map's class breaks (`::CLASSES`): `'quantile 5'`, `'jenks 5'`,
    /// `'equal 5'` or edges `'0,10,50,100'`; a stepped legend with counts.
    Classes,
    /// The centre of a map's diverging colour ramp (`::MIDPOINT`).
    Midpoint,
    /// A region code a map looks up in the built-in boundary pack
    /// (`::REGION`): ISO 3166-1 (`'DE'`, `'DEU'`), ISO 3166-2 (`'DE-BY'`), a
    /// German AGS (`'09162'`) or NUTS code (`'DE212'`), else a name.
    Region,
    /// A point's latitude on a map (`::LAT`), with `::LON`.
    Lat,
    /// A point's longitude on a map (`::LON`), with `::LAT`.
    Lon,
    /// The pack layer a map's `::REGION` codes are looked up in
    /// (`::BOUNDARIES`): `'world'`, `'world-50m'`, `'de-states'`,
    /// `'de-districts'`. Read from the first value.
    Boundaries,
    /// Layout: `::TAB` starts a new tab; following panels live under it.
    Tab,
    /// Layout: `::SUBTAB` starts a nested tab inside the current `::TAB`.
    SubTab,
    /// Layout: `::VIEW` opens (or extends) a group of alternative views of
    /// one panel; consecutive `::VIEW` markers with no closer between them
    /// are alternatives of the same group (#214).
    View,
    /// Layout: `::ENDVIEW` closes the current `::VIEW` group (`::TAB`,
    /// `::SUBTAB` and `::SECTION` also close it).
    ViewEnd,
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
    /// The label of the `::XLINE` on the same row (`::XLINE_LABEL`), drawn at
    /// the top of the line — an event ("Launch", "Outage") from data rows.
    VLineLabel,
    /// The label of the `::REFLINE` on the same row (`::REFLINE_LABEL`), drawn
    /// above the line's right end.
    RefLineLabel,
    /// A thin panel under the previous chart that shares its x axis
    /// (`::TRACK`; the value names it): ticks, `::HEATMAP` tiles per lane or a
    /// small line / bar, see [`track`].
    Track,
    /// Start of a labelled shaded x-range, one per row (`::XBAND_FROM`).
    XBandFrom,
    /// End of a labelled shaded x-range (`::XBAND_TO`), see [`Role::XBandFrom`].
    XBandTo,
    /// The label of the shaded range on the same row (`::XBAND_LABEL`).
    XBandLabel,
    /// A text callout at the row's (x, y) (`::ANNOTATE`); NULL rows draw nothing.
    Annotate,
    /// Threshold rules (`::THRESHOLDS '<0:bad; <0.05:warn; good'`): the status
    /// colours a KPI value, a gauge's zones and a chart's threshold lines; a
    /// lone `::THRESHOLDS` statement is the next table's per-column rules.
    Thresholds,
    /// A rich-text panel whose (string) value is rendered as Markdown
    /// (`::MARKDOWN`/`::MD`). Presentation-only — handled by the browser.
    Markdown,
    /// A panel's definition (`::INFO`): an ⓘ on its title bar shows the text.
    Info,
    /// A `::MARKDOWN` box drawn as a callout (`::CALLOUT`): `'note'`,
    /// `'warning'` or `'success'` (the value).
    Callout,
    /// Candlestick open price (`::OPEN`).
    Open,
    /// Candlestick high price (`::HIGH`).
    High,
    /// Candlestick low price (`::LOW`).
    Low,
    /// A box plot from statistics, one row per box: the lower quartile
    /// (`::Q1`). With [`Role::Median`] and [`Role::Q3`] the statement draws a
    /// box plot without the raw rows.
    Q1,
    /// A pre-computed box's median line (`::MEDIAN`), see [`Role::Q1`].
    Median,
    /// A pre-computed box's upper quartile (`::Q3`), see [`Role::Q1`].
    Q3,
    /// A pre-computed box's lower whisker end (`::WHISKER_LOW`); without it the
    /// whisker stops at the box.
    WhiskerLow,
    /// A pre-computed box's upper whisker end (`::WHISKER_HIGH`).
    WhiskerHigh,
    /// A pre-computed box's mean, drawn as a dot (`::MEAN`).
    Mean,
    /// A pre-computed box's outliers, a list of numbers (`::OUTLIERS`), drawn
    /// as open circles.
    Outliers,
    /// Lower end of a y interval (`::YMIN`): a point measure becomes a
    /// pointrange, any other measure gets error bars.
    YMin,
    /// Upper end of a y interval (`::YMAX`), see [`Role::YMin`].
    YMax,
    /// Lower end of an x interval (`::XMIN`): a horizontal interval through
    /// each point.
    XMin,
    /// Upper end of an x interval (`::XMAX`), see [`Role::XMin`].
    XMax,
    /// Small multiples (`::FACET`): one panel per distinct value, shared axes.
    Facet,
    /// Small multiples with independent axes per panel (`::FACET_FREE`).
    FacetFree,
    /// Panels per row of a `::FACET` (`::FACET_NCOL`). Read from the first value.
    FacetCols,
    /// x-axis scale transform (`::XSCALE`): `'log10'`, `'sqrt'` or `'reverse'`.
    XScale,
    /// y-axis scale transform (`::YSCALE`), like [`Role::XScale`].
    YScale,
    /// A straight reference line `y = slope·x + intercept` per distinct
    /// `'slope,intercept'` value (`::ABLINE`).
    AbLine,
    /// The identity line `y = x`, dashed grey (`::IDENTITY`). A marker column.
    Identity,
    /// Label only the top-k points (`::LABEL_TOP`, k = the first value) with
    /// the panel's `::LABEL` text — ranked by `::RANK` if present, else |y|.
    LabelTop,
    /// Ranking score for `::LABEL_TOP` (`::RANK`): higher = labelled first.
    Rank,
    /// Cook's-distance contours at 0.5 and 1 on a residuals-vs-leverage
    /// scatter (`::COOKS_CONTOUR p`, p = the model's parameter count; x =
    /// leverage, y = standardised residual), as R's `plot.lm(which = 5)`.
    CooksContour,
    /// Censoring marks (`::CENSOR`): a `+` on a ::STEP curve at every row
    /// whose value is > 0 / true (Kaplan–Meier `n_censor`).
    Censor,
    /// Trend-line method for `::SMOOTH` (`::SMOOTH_METHOD`): `'loess'`
    /// (default), `'lm'`, `'gam'`; `'glm'` draws the Gaussian GLM (= lm).
    SmoothMethod,
    /// Dashboard theme (`::THEME`): a built-in name, a stack `'acme+dark'` or
    /// a JSON definition (see [`theme`]).
    Theme,
    /// Dashboard locale for numbers and dates (`::LOCALE 'de-DE'`).
    Locale,
    /// Dashboard title (`::DASHBOARD_TITLE`): page header, `<title>`, exports.
    DashboardTitle,
    /// Brand / organisation name shown above the dashboard title (`::BRAND`).
    Brand,
    /// Header logo image URL (`::LOGO`).
    Logo,
    /// Page favicon URL (`::FAVICON`).
    Favicon,
    /// The data's freshness (`::ASOF`): a "Data as of …" chip in the header,
    /// amber once older than an optional `'stale=24h'` column.
    AsOf,
    /// A section heading that always starts a new row (`::SECTION`).
    Section,
    /// Interaction: `::SELECT_AS` names the *next* chart's selection — a click
    /// sets `getvariable('<name>')` instead of the shared `selected` (value
    /// `'name'` or `'name multi'`), a brush on a continuous x sets
    /// `<name>_from` / `<name>_to`.
    SelectAs,
    /// Layout: `::EMPTY` — the *next* panel's message when its query returns
    /// no rows (default: "No data for this selection").
    Empty,
    /// Interaction: `::DRILL` makes the *next* chart a drill-down over a level
    /// path (`'region > country > city'`, `'geo: …'` names the variables) —
    /// `<prefix>_level` / `_depth` / `_path` plus one variable per level, and a
    /// breadcrumb back.
    Drill,
    /// A drill-through target (`::LINK`): a template — `#Tab`, `?v.x=…`,
    /// `/d/<id>?v.x=…` or an `https:` URL — with `{value}` / `{column}`
    /// placeholders, followed on a chart click or from a table's link cell.
    Link,
    /// Motion: the panel's entrance animation (`::ANIMATE`) — `'auto'`
    /// (default: lines draw, bars grow, points stagger), `'draw'`, `'grow'`,
    /// `'reveal'` (forward from the panel's `::XLINE`) or `'none'`. Browser only.
    Animate,
    /// A series' line style (`::LINESTYLE`, per row, keyed by its
    /// `::CATEGORY`): `'flow'` = dashes that move in the browser (a forecast,
    /// "predicted"), `'dashed'` = still dashes; static output shows dashes.
    LineStyle,
    /// A story caption per row (`::STORY`): rows with text become steps that
    /// light up their x (or their series, see [`Role::StoryBy`]) in the browser.
    Story,
    /// What a `::STORY` step lights up (`::STORY_BY`): `'x'` (default) or
    /// `'series'` (the row's `::CATEGORY`).
    StoryBy,
    /// How the chart changes on a re-run (`::TRANSITION`): `'tween'` moves
    /// the marks to their new values, `'none'` (default) swaps the chart.
    Transition,
    /// A node/edge diagram (`::DIAGRAM`/`::NODE`): the column is the node id,
    /// one row per node (see [`diagram`]).
    Diagram,
    /// The node(s) feeding a `::DIAGRAM` row (`::UPSTREAM`): an id or a list
    /// of ids → edges `upstream → this node`.
    Upstream,
    /// A `::DIAGRAM`'s edges with attributes (`::EDGES`): a list of structs
    /// `{from, to [, value, label, style]}` (the first non-NULL cell).
    Edges,
    /// A `::DIAGRAM`'s direction (`::DIRECTION`): `'LR'` (default) or `'TB'`.
    Direction,
    /// A `::DIAGRAM` node's swimlane (`::LANE`): nodes of one lane share a
    /// band across the layers.
    Lane,
}

/// A single annotated result column: a name, its [`Role`], and its values.
#[derive(Clone)]
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

/// The DataZoo palette — the slot colours chart code writes directly
/// (contract plots). Twelve distinct hues so a chart with up to ~12
/// categories keeps every series separable. The rendered colours come from
/// the active theme ([`theme::ResolvedTheme::palette`], by default
/// [`theme::DEFAULT_PALETTE`]); its SVG pass maps these slot by slot.
pub const DZ_COLORS: [(u8, u8, u8); 12] = theme::DZ_PALETTE;

/// Palette colour `i` of the render's theme.
pub(crate) fn dz_color(o: &RenderOptions, i: usize) -> ggplot_rs::scale::color::RGBAColor {
    let (r, g, b) = o.theme.color(i);
    ggplot_rs::scale::color::RGBAColor::new(r, g, b)
}

/// The whole palette of the render's theme as scale colours.
pub(crate) fn dz_palette(o: &RenderOptions) -> Vec<ggplot_rs::scale::color::RGBAColor> {
    (0..o.theme.palette.len()).map(|i| dz_color(o, i)).collect()
}

/// The house chart theme — ggplot's minimal look painted with the
/// dashboard's tokens: tick labels and axis titles in the muted ink, legend
/// entries a step stronger, titles in the heading colour, a hairline major
/// grid a step quieter than the card border, no minor grid, no ticks and
/// transparent backgrounds (the card shows through). `base` is the base font
/// size (ggplot's 11 pt for role charts).
pub(crate) fn chart_theme(o: &RenderOptions, base: f64) -> Theme {
    let mut th = ggplot_rs::theme::presets::theme_minimal_base(base);
    paint_theme(&mut th, o);
    th.panel_grid_major = ggplot_rs::theme::ElementLine {
        color: o.theme.chart_grid(),
        width: 0.6,
        ..Default::default()
    };
    th.panel_grid_minor = ggplot_rs::theme::ElementLine::blank();
    // a colourbar (heatmap, calendar) framed in the quiet card line
    th.legend_bar_border = Some(ggplot_rs::theme::ElementLine {
        color: o.theme.colors.line,
        width: 0.6,
        ..Default::default()
    });
    th
}

/// [`chart_theme`] without axes or grid (pie, gauge, map, sparkline).
pub(crate) fn void_theme(o: &RenderOptions) -> Theme {
    let mut th = ggplot_rs::theme::presets::theme_void();
    paint_theme(&mut th, o);
    th
}

/// Paint a theme's text with the dashboard's tokens.
pub(crate) fn paint_theme(th: &mut Theme, o: &RenderOptions) {
    let t = &o.theme;
    let font = t.svg_font();
    let paint = |e: &mut ggplot_rs::theme::ElementText, c: (u8, u8, u8)| {
        e.color = c;
        e.family = font.clone();
    };
    paint(&mut th.text, t.chart_label());
    paint(&mut th.title, t.colors.heading);
    paint(&mut th.subtitle, t.chart_text());
    paint(&mut th.caption, t.chart_text());
    paint(&mut th.axis_text_x, t.chart_text());
    paint(&mut th.axis_text_y, t.chart_text());
    paint(&mut th.axis_title_x, t.chart_text());
    paint(&mut th.axis_title_y, t.chart_text());
    paint(&mut th.legend_title, t.chart_text());
    paint(&mut th.legend_text, t.chart_label());
    paint(&mut th.strip_text, t.colors.heading);
}

/// `.theme(chart_theme(..))` / `.theme(void_theme(..))` as plot builders.
pub(crate) trait HouseTheme {
    fn house_theme(self, o: &RenderOptions) -> Self;
    fn house_void(self, o: &RenderOptions) -> Self;
}

impl HouseTheme for GGPlot {
    fn house_theme(self, o: &RenderOptions) -> Self {
        self.theme(chart_theme(o, 11.0))
    }
    fn house_void(self, o: &RenderOptions) -> Self {
        self.theme(void_theme(o))
    }
}

/// A light wash of `c` on the card (`t` = how far towards the card): a
/// pale tint on a light card, a deep one on a dark card.
pub(crate) fn wash(o: &RenderOptions, c: (u8, u8, u8), t: f64) -> (u8, u8, u8) {
    theme::mix(c, o.theme.colors.card, t)
}

/// Bars and pie slices are outlined in the card colour, so adjoining marks
/// (stacked segments, slices) part with a hairline gap instead of a dark rule.
pub(crate) fn card_col(o: &RenderOptions) -> GeomCol {
    GeomCol {
        fill: o.brand(),
        color: o.theme.colors.card,
        ..Default::default()
    }
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
    /// The theme variant to paint with (palette, sequential fills, dark
    /// chart ink). Default: the built-in light look.
    pub theme: std::sync::Arc<theme::ResolvedTheme>,
    /// Number/date locale of axis and legend labels (default en-US).
    pub locale: format::Locale,
    /// Line the plot area and x window up with other panels (a chart and
    /// its `::TRACK`s, see [`track::render_stack`]); `None` = the panel's own.
    pub align: Option<track::Align>,
}

impl Default for RenderOptions {
    fn default() -> Self {
        RenderOptions {
            brand: None,
            zoom: None,
            max_categories: DEFAULT_MAX_CATEGORIES,
            max_line_points: DEFAULT_MAX_LINE_POINTS,
            theme: theme::default_light(),
            locale: format::Locale::EN_US,
            align: None,
        }
    }
}

impl RenderOptions {
    /// The effective brand colour: the explicit `brand`, else the theme's
    /// first palette colour.
    pub fn brand(&self) -> (u8, u8, u8) {
        self.brand.unwrap_or(self.theme.palette[0])
    }

    /// Apply a render context from a spec / wasm call: `"theme"` (a spec
    /// string or a JSON object), `"variant"` (`light`/`dark`), `"locale"` and
    /// `"primary"`. Unknown themes are an error; an absent key keeps the
    /// current value.
    pub fn apply_context(&mut self, ctx: &serde_json::Value) -> Result<(), String> {
        let theme = match ctx.get("theme") {
            None | Some(serde_json::Value::Null) => None,
            Some(serde_json::Value::String(t)) => Some(theme::resolve(t)?),
            Some(v @ serde_json::Value::Object(_)) => Some(theme::resolve_json(v)?),
            Some(_) => return Err("`theme` must be a string or an object".into()),
        };
        let variant = ctx
            .get("variant")
            .and_then(|v| v.as_str())
            .map(theme::Variant::parse);
        if theme.is_some() || variant.is_some() {
            let set = match theme {
                Some(s) => s,
                None => theme::default_set().clone(),
            };
            let t = set.pick(variant.unwrap_or(theme::Variant::Light)).clone();
            if self.locale.is_default() {
                if let Some(l) = t.locale.as_deref().and_then(format::Locale::from_tag) {
                    self.locale = l;
                }
            }
            self.theme = std::sync::Arc::new(t);
        }
        if let Some(l) = ctx
            .get("locale")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
        {
            self.locale = format::Locale::from_tag(l).ok_or_else(|| {
                format!(
                    "unknown locale `{l}` (known: {})",
                    format::Locale::TAGS.join(", ")
                )
            })?;
        }
        if let Some(p) = ctx
            .get("primary")
            .and_then(|v| v.as_str())
            .and_then(parse_rgb)
        {
            self.brand = Some(p);
        }
        Ok(())
    }

    /// Options from a wasm/host "primary" argument: a brand colour `rrggbb`
    /// (or ""), or a JSON render context `{"primary","theme","variant","locale"}`.
    pub fn from_primary_arg(primary: &str) -> Result<RenderOptions, String> {
        let mut o = RenderOptions::default();
        let p = primary.trim();
        if p.starts_with('{') {
            let v: serde_json::Value =
                serde_json::from_str(p).map_err(|e| format!("render context JSON: {e}"))?;
            o.apply_context(&v)?;
        } else {
            o.brand = parse_rgb(p);
        }
        Ok(o)
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
fn dz_scale(o: &RenderOptions, aesthetic: Aesthetic, col: &Column) -> ScaleColorDiscrete {
    let scale = ScaleColorDiscrete::new(aesthetic).sorted();
    let hex_level = |v: &Value| matches!(v, Value::Str(s) if parse_hex(s).is_some());
    if !col.values.iter().any(hex_level) {
        return scale.with_palette(dz_palette(o));
    }
    let levels = distinct_labels(col);
    let palette = levels
        .iter()
        .enumerate()
        .map(|(i, s)| parse_hex(s).unwrap_or_else(|| dz_color(o, i)))
        .collect();
    scale.with_levels(levels).with_palette(palette)
}

/// A `::COLORS` level map on a chart (`'Critical:bad; Watch:warn; OK:good'`):
/// each named `::CATEGORY` level in its status colour (the theme's
/// negative / warning / positive / muted) or `#rrggbb`, levels in the
/// written order (the legend order); levels it does not name follow, sorted,
/// in palette colours. `None` without a level map (a gauge's zone colours).
fn mapped_scale(
    o: &RenderOptions,
    aesthetic: Aesthetic,
    col: &Column,
    cols: &[Column],
) -> Option<ScaleColorDiscrete> {
    let spec = cols
        .iter()
        .find(|c| c.role == Role::GaugeColors)
        .and_then(|c| c.values.first())
        .map(value_str)
        .filter(|s| thresholds::is_level_map(s))?;
    let map = thresholds::parse_level_colors(&spec).ok()?;
    let present = distinct_labels(col);
    let mut levels: Vec<String> = map
        .iter()
        .map(|(l, _)| l.clone())
        .filter(|l| present.contains(l))
        .collect();
    levels.extend(
        present
            .into_iter()
            .filter(|l| !map.iter().any(|(m, _)| m == l)),
    );
    let palette = levels
        .iter()
        .enumerate()
        .map(|(i, l)| match map.iter().find(|(m, _)| m == l) {
            Some((_, c)) => {
                let (r, g, b) = c.rgb(&o.theme);
                ggplot_rs::scale::color::RGBAColor::new(r, g, b)
            }
            None => dz_color(o, i),
        })
        .collect();
    Some(
        ScaleColorDiscrete::new(aesthetic)
            .with_levels(levels)
            .with_palette(palette),
    )
}

/// [`dz_scale`], or the `::COLORS` level map's scale when the chart has one.
fn category_scale(
    o: &RenderOptions,
    aesthetic: Aesthetic,
    col: &Column,
    cols: &[Column],
) -> ScaleColorDiscrete {
    mapped_scale(o, aesthetic.clone(), col, cols).unwrap_or_else(|| dz_scale(o, aesthetic, col))
}

/// The colour [`dz_scale`] gives the last (sorted) level of `col`.
fn last_level_color(o: &RenderOptions, col: &Column) -> Option<(u8, u8, u8)> {
    let levels = distinct_labels(col);
    let i = levels.len().checked_sub(1)?;
    let c = parse_hex(&levels[i]).unwrap_or_else(|| dz_color(o, i));
    Some((c.r, c.g, c.b))
}

/// The [`dz_scale`] colour of the series (`col`'s level) of the row with the
/// largest x among rows that carry a band value.
fn band_series_color(
    o: &RenderOptions,
    col: &Column,
    x: &[Value],
    band: Option<&Column>,
) -> Option<(u8, u8, u8)> {
    let band = band?;
    let row = (0..col.values.len())
        .filter(|&i| band.values.get(i).and_then(|v| v.as_f64()).is_some())
        .filter_map(|i| Some((i, x.get(i)?.as_f64()?)))
        .max_by(|a, b| a.1.total_cmp(&b.1))?
        .0;
    let level = value_str(&col.values[row]);
    let levels = distinct_labels(col);
    let i = levels.iter().position(|l| *l == level)?;
    let c = parse_hex(&level).unwrap_or_else(|| dz_color(o, i));
    Some((c.r, c.g, c.b))
}

/// The shaded x-ranges of a `::MARKAREA` column: one per run of marked
/// x-values, i.e. marked values with no unmarked x of the chart (`xs`) in
/// between — `CASE WHEN alarm THEN day END` shades every alarm spell. A run
/// spans [first, last]; a run of one value spans ± half the chart's x step,
/// so a single flagged period shows. Dates stay dates.
fn mark_regions(col: &Column, xs: &[Value]) -> Vec<(Value, Value)> {
    let sorted = |vals: &mut dyn Iterator<Item = &Value>| {
        let mut v: Vec<f64> = vals
            .filter_map(Value::as_f64)
            .filter(|f| f.is_finite())
            .collect();
        v.sort_by(f64::total_cmp);
        v.dedup();
        v
    };
    let marked = sorted(&mut col.values.iter());
    if marked.is_empty() {
        return Vec::new();
    }
    let all = sorted(&mut xs.iter().chain(&col.values));
    let mut steps: Vec<f64> = all.windows(2).map(|w| w[1] - w[0]).collect();
    steps.sort_by(f64::total_cmp);
    let half = steps.get(steps.len() / 2).copied().unwrap_or(0.0) / 2.0;
    let date = col.values.iter().any(|v| matches!(v, Value::DateTime(_)));
    let val = |f: f64| {
        if date {
            Value::DateTime(f.round() as i64)
        } else {
            Value::Float(f)
        }
    };
    let mut out = Vec::new();
    let mut run: Option<(f64, f64)> = None;
    for &x in &all {
        if marked.binary_search_by(|m| m.total_cmp(&x)).is_ok() {
            run = Some(run.map_or((x, x), |(a, _)| (a, x)));
        } else if let Some(r) = run.take() {
            out.push(r);
        }
    }
    out.extend(run);
    out.into_iter()
        .map(|(a, b)| {
            if a == b {
                (val(a - half), val(b + half))
            } else {
                (val(a), val(b))
            }
        })
        .collect()
}

/// The `::BAND_LOWER` / `::BAND_UPPER` pairs of a panel, matched in column
/// order (the first lower with the first upper, …): one band, or the bands of
/// a nested quantile fan. An unpaired edge draws nothing (the linter says so).
fn band_pairs(cols: &[Column]) -> Vec<(&Column, &Column)> {
    let of = |r: Role| cols.iter().filter(move |c| c.role == r);
    of(Role::BandLower).zip(of(Role::BandUpper)).collect()
}

/// The data column of the `k`-th band edge (`bandlo`, `bandlo2`, …).
fn band_key(edge: &str, k: usize) -> String {
    if k == 0 {
        edge.to_string()
    } else {
        format!("{edge}{}", k + 1)
    }
}

/// Band indices widest first (by mean width over the rows that carry both
/// edges), so a fan's inner bands are drawn on top of its outer ones.
fn fan_order(bands: &[(&Column, &Column)]) -> Vec<usize> {
    let width = |(lo, hi): &(&Column, &Column)| {
        let w: Vec<f64> = lo
            .values
            .iter()
            .zip(&hi.values)
            .filter_map(|(a, b)| Some(b.as_f64()? - a.as_f64()?))
            .filter(|w| w.is_finite())
            .collect();
        w.iter().sum::<f64>() / w.len().max(1) as f64
    };
    let mut order: Vec<usize> = (0..bands.len()).collect();
    order.sort_by(|&a, &b| width(&bands[b]).total_cmp(&width(&bands[a])));
    order
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

/// Name `cols` (built one per role, as [`sql::columns_from_rows`] does) as
/// the query names its measures — the plan's `names`.
pub fn name_columns(
    mut cols: Vec<Column>,
    roles: &[(usize, Role)],
    names: &[(usize, String)],
) -> Vec<Column> {
    for (c, (i, _)) in cols.iter_mut().zip(roles) {
        if let Some((_, n)) = names.iter().find(|(j, _)| j == i) {
            c.name = n.clone();
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
/// A `TITLE` role draws a title bar above the chart (its first non-NULL
/// value), as a dashboard card does; a KPI tile has none.
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
        let plot = match spec.get("plot") {
            None | Some(serde_json::Value::Null) => None,
            Some(serde_json::Value::String(p)) => Some(p.clone()),
            Some(_) => return Err(RenderError::BadSpec("`plot` must be a string".into())),
        };
        let entries = match spec.get("roles") {
            None => Vec::new(),
            Some(_) if plot.is_some() => Vec::new(),
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
        let mut opts = RenderOptions {
            max_categories: count("max_categories", DEFAULT_MAX_CATEGORIES),
            max_line_points: count("max_line_points", DEFAULT_MAX_LINE_POINTS),
            ..RenderOptions::default()
        };
        opts.apply_context(&spec).map_err(RenderError::BadSpec)?;
        if let Some(plot) = plot {
            // A contract plot (`terms`, `prediction`, …) over named columns.
            let options = match spec.get("options") {
                Some(serde_json::Value::Object(m)) => m.clone(),
                None | Some(serde_json::Value::Null) => serde_json::Map::new(),
                Some(_) => return Err(RenderError::BadSpec("`options` must be an object".into())),
            };
            return guard(|| {
                contract::render(&plot, &rows, &options, width, height, &opts, Place::Doc)
                    .map(|(svg, w)| finish_contract_svg(svg, &w, &opts))
            });
        }
        let cols = columns_from_entries(&rows, &entries);
        match panel_title(&cols, &opts) {
            Some(title) => render_titled(&cols, &title, width, height, &opts),
            None => render_with(&cols, width, height, &opts),
        }
    })
}

/// Height of the `::TITLE` bar above a single panel (a dashboard card's):
/// `anofox_render` and the static pages.
const TITLE_BAR_H: u32 = 26;

/// The longest `::TITLE` a static page draws, in characters: a 460 px bar
/// shows about 50, and escaping can make a title up to 6× longer
/// (sec-review-250 L1, #251). `anofox_render` keeps the whole title.
const PAGE_TITLE_MAX_CHARS: usize = 200;

/// `title` cut to [`PAGE_TITLE_MAX_CHARS`] characters (on a `char` boundary,
/// trailing whitespace dropped) plus `…`; a shorter title is unchanged.
fn cap_page_title(title: String) -> String {
    match title.char_indices().nth(PAGE_TITLE_MAX_CHARS) {
        Some((i, _)) => format!("{}…", title[..i].trim_end()),
        None => title,
    }
}

/// The first non-NULL value of a `::TITLE` column, trimmed; `None` when that
/// value is blank. Every single-panel output reads a title this way:
/// `anofox_render`, the static pages, the `dashboard --svg` card (#246).
pub(crate) fn title_value(values: &[Value]) -> Option<String> {
    values
        .iter()
        .find(|v| !matches!(v, Value::Na))
        .map(value_str)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// The `::TITLE` of a single panel ([`title_value`]). A KPI tile has none
/// (its caption is `::LABEL`), as in a dashboard.
fn panel_title(cols: &[Column], o: &RenderOptions) -> Option<String> {
    let title = title_value(&cols.iter().find(|c| c.role == Role::Title)?.values)?;
    kpi::Kpi::from_columns(cols, &o.locale)
        .is_none()
        .then_some(title)
}

/// [`render_with`] under a `::TITLE` bar, drawn as a dashboard card draws it
/// (`dashboard::render_panel_svg`): the chart is nested below the bar, the
/// panel keeps the requested height and its build warnings go on the root.
fn render_titled(
    cols: &[Column],
    title: &str,
    width: u32,
    height: u32,
    o: &RenderOptions,
) -> Result<String, RenderError> {
    let (w, h) = (clamp_dim(width as u64), clamp_dim(height as u64));
    let body_h = h.saturating_sub(TITLE_BAR_H).max(MIN_DIM);
    let (body, warnings) = render_placed(cols, Place::Doc, w, body_h, o)?;
    let svg = format!(
        "{open}{text}<g transform=\"translate(0,{TITLE_BAR_H})\">{body}</g></svg>",
        open = svg_open(Place::Doc, w, body_h + TITLE_BAR_H),
        text = title_bar_text(title, o),
    );
    Ok(with_warnings_attr(svg, &warnings))
}

/// The `::TITLE` bar's text as a dashboard card draws it
/// (`dashboard::render_panel_svg`), the title XML-escaped.
fn title_bar_text(title: &str, o: &RenderOptions) -> String {
    format!(
        "<text x=\"14\" y=\"20\" font-family=\"{font}\" font-size=\"14\" \
         font-weight=\"700\" fill=\"{ink}\">{title}</text>",
        font = format::escape_xml(&o.theme.font.display),
        ink = theme::hex(o.theme.colors.heading),
        title = format::escape_xml(title),
    )
}

/// A static page's chart (`dashboard file.sql out.html`, `serve
/// --dashboards`): [`render_with`] (or, with `::TRACK`s under it,
/// [`track::render_stack`]) under the `::TITLE` bar a dashboard card draws —
/// the first non-NULL, non-blank title; none on a KPI tile. The bar is its own
/// `width`×26 `<svg>` before the chart, which gets the requested height less
/// 26 px, so every `<svg>` stays top-level in the page (the page CSS sizes
/// each one). The title is XML-escaped. Without a title this is exactly
/// `render_with` / the concatenated `render_stack`. A title longer than 200
/// characters is cut to 200 and an ellipsis (#251).
pub fn render_with_title(
    cols: &[Column],
    tracks: &[(Vec<Column>, u32)],
    width: u32,
    height: u32,
    o: &RenderOptions,
) -> Result<String, RenderError> {
    let title = panel_title(cols, o).map(cap_page_title);
    let h = if title.is_some() {
        clamp_dim(height as u64)
            .saturating_sub(TITLE_BAR_H)
            .max(MIN_DIM)
    } else {
        height
    };
    let chart = if tracks.is_empty() {
        render_with(cols, width, h, o)?
    } else {
        track::render_stack(cols, tracks, width, h, o)?.concat()
    };
    Ok(match title {
        Some(t) => format!(
            "{open}{text}</svg>{chart}",
            open = svg_open(Place::Doc, clamp_dim(width as u64), TITLE_BAR_H),
            text = title_bar_text(&t, o),
        ),
        None => chart,
    })
}

/// The last step of every contract render: drop non-finite marks, apply the
/// theme variant, record the build warnings.
pub(crate) fn finish_contract_svg(svg: String, warnings: &[String], o: &RenderOptions) -> String {
    with_warnings_attr(o.theme.finish_svg(strip_nonfinite_marks(svg)), warnings)
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
        "{}<text x=\"4\" y=\"24\" font-family=\"system-ui,sans-serif\" font-size=\"12\" fill=\"{}\">{}</text></svg>",
        svg_open(place, clamp_dim(width as u64), 40),
        theme::SVG_ERROR,
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
        /// Extra root attributes (e.g. `data-xticks`) written on the `<svg>`.
        attrs: Vec<(&'static str, String)>,
        /// The plot margin its theme set, for a pass that grows it (a top
        /// legend wrapping onto more rows); `None` when not known.
        margin: Option<ggplot_rs::theme::Margin>,
    },
    Own {
        width: u32,
        height: u32,
        /// The SVG content (without the `<svg>` wrapper).
        body: String,
        /// Extra root attributes (e.g. `data-map`) written on the `<svg>`.
        attrs: Vec<(&'static str, String)>,
        /// Render warnings (reported like the plotting engine's).
        warnings: Vec<String>,
    },
}

impl Panel {
    /// Label axes and legends in the render's locale (no-op for en-US, so the
    /// default output is unchanged). A year-like x axis (integers
    /// 1000..=2999) is not thousands-grouped.
    pub(crate) fn localized(self, o: &RenderOptions, cols: &[Column]) -> Panel {
        match self {
            Panel::Plot {
                plot,
                width,
                height,
                attrs,
                margin,
            } if !o.locale.is_default() => {
                let yearish = cols.iter().filter(|c| c.role == Role::X).any(|c| {
                    let mut any = false;
                    c.values.iter().all(|v| match v {
                        Value::Na => true,
                        v => {
                            v.as_f64().is_some_and(|f| {
                                any = true;
                                f.fract() == 0.0 && (1000.0..3000.0).contains(&f)
                            }) && !matches!(v, Value::Str(_))
                        }
                    }) && any
                });
                let mut l = o.locale.gg();
                if yearish {
                    l = l.with_min_grouping_digits(2);
                }
                Panel::Plot {
                    plot: Box::new(plot.locale(l)),
                    width,
                    height,
                    attrs,
                    margin,
                }
            }
            p => p,
        }
    }

    /// One of the extension's own SVGs, without root attributes.
    pub(crate) fn own(width: u32, height: u32, body: String) -> Panel {
        Panel::Own {
            width,
            height,
            body,
            attrs: Vec::new(),
            warnings: Vec::new(),
        }
    }

    pub(crate) fn plot(plot: GGPlot, width: u32, height: u32) -> Panel {
        Panel::Plot {
            plot: Box::new(plot),
            width,
            height,
            attrs: Vec::new(),
            margin: None,
        }
    }

    /// Record the plot margin the panel's theme set (see
    /// [`legend_rows`](Self::legend_rows)).
    pub(crate) fn with_margin(mut self, m: ggplot_rs::theme::Margin) -> Panel {
        if let Panel::Plot { margin, .. } = &mut self {
            *margin = Some(m);
        }
        self
    }

    /// Reserve `rows.rows` legend rows: the plot margin above (or below) the
    /// panel grows by the rows past the first, which ggplot-rs reserves
    /// itself. A panel whose margin is not known is left as it is.
    pub(crate) fn legend_rows(self, rows: Option<legend_wrap::Rows>) -> Panel {
        match (self, rows) {
            (
                Panel::Plot {
                    plot,
                    width,
                    height,
                    attrs,
                    margin: Some(mut m),
                },
                Some(r),
            ) if r.rows > 1 => {
                let extra = (r.rows - 1) as f64 * legend_wrap::ROW;
                if r.top {
                    m.top += extra;
                } else {
                    m.bottom += extra;
                }
                let plot = plot.theme_update(ggplot_rs::theme::ThemeUpdate {
                    plot_margin: Some(m.clone()),
                    ..Default::default()
                });
                Panel::Plot {
                    plot: Box::new(plot),
                    width,
                    height,
                    attrs,
                    margin: Some(m),
                }
            }
            (p, _) => p,
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
                attrs,
                ..
            } => {
                let svg = match place {
                    Place::Doc => plot.render_svg_native_with_warnings(width, height),
                    Place::At(x, y) => plot
                        .render_svg_native_at(x, y, width, height)
                        .map(|s| (s, Vec::new())),
                };
                svg.map(|(s, w)| {
                    let mut s = ticks::fit_x_labels(s);
                    for (k, v) in &attrs {
                        s = with_root_attr(s, k, v);
                    }
                    (s, w)
                })
                .map_err(|e| format!("render failed: {e:?}"))
            }
            Panel::Own {
                width,
                height,
                body,
                attrs,
                warnings,
            } => {
                let mut s = format!("{}{body}</svg>", svg_open(place, width, height));
                for (k, v) in &attrs {
                    s = with_root_attr(s, k, v);
                }
                Ok((s, warnings))
            }
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
///
/// Build warnings from the plotting engine (see [`render_with_warnings`]) are
/// also recorded on the SVG root as `data-warnings="[…]"` (a JSON array).
pub fn render_with(
    cols: &[Column],
    width: u32,
    height: u32,
    o: &RenderOptions,
) -> Result<String, RenderError> {
    render_with_warnings(cols, width, height, o).map(|r| r.svg)
}

/// A rendered panel and the plotting engine's build warnings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rendered {
    /// The SVG document (carrying `data-warnings` when `warnings` is non-empty).
    pub svg: String,
    /// ggplot-rs build warnings, e.g. `"geom_point: removed 3 rows containing
    /// non-finite values"` or a layer whose stat produced no data and was
    /// skipped — things that render fine but silently drop data.
    pub warnings: Vec<String>,
}

/// [`render_with`], also returning the plotting engine's build warnings
/// (`dashboard --check` reports them as `render-warning` diagnostics).
pub fn render_with_warnings(
    cols: &[Column],
    width: u32,
    height: u32,
    o: &RenderOptions,
) -> Result<Rendered, RenderError> {
    let (svg, warnings) = render_placed(cols, Place::Doc, width, height, o)?;
    Ok(Rendered {
        svg: with_warnings_attr(svg, &warnings),
        warnings,
    })
}

/// Record `warnings` on the SVG root as `data-warnings="[…]"` (escaped JSON).
/// The root start tag ends at the first `>`: every writer escapes `>` inside
/// attribute values.
fn with_warnings_attr(svg: String, warnings: &[String]) -> String {
    if warnings.is_empty() {
        return svg;
    }
    let json = serde_json::to_string(warnings).unwrap_or_default();
    with_root_attr(svg, "data-warnings", &json)
}

/// Add `name="value"` (escaped) to the root `<svg>` start tag. The root start
/// tag ends at the first `>`: every writer escapes `>` inside attribute values.
pub(crate) fn with_root_attr(mut svg: String, name: &str, value: &str) -> String {
    if !svg.starts_with("<svg") {
        return svg;
    }
    if let Some(end) = svg.find('>') {
        let at = if svg[..end].ends_with('/') {
            end - 1
        } else {
            end
        };
        svg.insert_str(at, &format!(" {name}=\"{}\"", format::escape_xml(value)));
    }
    svg
}

/// Like [`render_with`], but renders a nested `<svg x y width height
/// viewBox>` fragment (no `xmlns`) positioned at `(x, y)` in a parent SVG —
/// for composing dashboards without string surgery. (ggplot-rs reports no
/// build warnings for fragments; use [`render_with_warnings`] to lint.)
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
        // A diagram draws its own SVG: none of the chart post-passes apply.
        if let Some(d) = diagram::Diagram::from_columns(cols, &o.locale) {
            let svg = diagram::render(&d, place, w, h, o);
            return Ok((o.theme.finish_svg(svg), Vec::new()));
        }
        let cols = downsample::prepare(cols, o);
        let cols = boxstats::prepare(cols).map_err(RenderError::Render)?;
        let draw = |fit: Option<track::Fit>, rows: Option<legend_wrap::Rows>| {
            render_inner(&cols, w, h, o)
                .map(|p| p.localized(o, &cols))
                .map(|p| match fit {
                    Some(f) => p.fitted(f),
                    None => p,
                })
                .map(|p| p.legend_rows(rows))
                .and_then(|p| p.finish(place))
        };
        // Aligned with other panels: draw, measure, draw again fitted.
        let run = |rows: Option<legend_wrap::Rows>| match &o.align {
            Some(a) => {
                let first = track::Fit::first(o, a);
                let (svg, _) = draw(Some(first.clone()), rows)?;
                draw(Some(first.correct(a, &svg, h as f64)), rows)
            }
            None => draw(None, rows),
        };
        let (mut svg, mut warnings) = run(None).map_err(RenderError::Render)?;
        // A top/bottom legend too long for one line: draw again with its rows
        // reserved (once more if the panel's new axis labels moved it), then
        // wrap it.
        let mut reserved = 1;
        for _ in 0..2 {
            match legend_wrap::plan(&svg).filter(|r| r.rows > reserved) {
                Some(r) => {
                    (svg, warnings) = run(Some(r)).map_err(RenderError::Render)?;
                    reserved = r.rows;
                }
                None => break,
            }
        }
        let svg = legend_wrap::wrap(svg, reserved, &mut warnings);
        // A map paints with the variant's own tokens: no neutral remap.
        let svg = if map::is_map(&cols) {
            o.theme.with_font(svg)
        } else {
            o.theme.finish_svg(strip_nonfinite_marks(svg))
        };
        let svg = with_line_styles(with_rule_marks(with_xline_dates(svg, &cols)), &cols);
        let mut svg = with_motion_attrs(with_formatted_values(svg, &cols, o), &cols);
        if boxstats::is_stats(&cols) {
            svg = boxstats::with_tips(svg, &cols, o);
        }
        if find_role(&cols, Role::Track).is_some() {
            svg = track::lane_tips(svg);
        }
        Ok((svg, warnings))
    })
}

/// A date `::XLINE` is drawn at its epoch seconds; its tooltip and `data-x`
/// say the date it was given (`2026-10-12`), not `x = 1791763200`.
fn with_xline_dates(mut svg: String, cols: &[Column]) -> String {
    let Some(c) = find_role(cols, Role::VLine) else {
        return svg;
    };
    for v in &c.values {
        let (e, d) = match v {
            Value::DateTime(e) => (*e, xline_date_text(*e)),
            Value::Str(s) => {
                let s = s.trim();
                if s.parse::<f64>().is_ok() {
                    continue;
                }
                let Some(e) = sql::parse_iso_epoch(s) else {
                    continue;
                };
                (e, s.strip_suffix(" 00:00:00").unwrap_or(s).to_string())
            }
            _ => continue,
        };
        let d = format::escape_xml(&d);
        svg = svg
            .replace(
                &format!(" data-x=\"{e}\" data-value=\"{e}\">"),
                &format!(" data-x=\"{d}\">"),
            )
            .replace(
                &format!("<title>x = {e}</title>"),
                &format!("<title>{d}</title>"),
            );
    }
    svg
}

/// Mark the reference rules (`::XLINE`, `::REFLINE`/`::YLINE`, threshold
/// lines) `data-rule="x"` / `"y"`: they carry a tooltip like the data marks,
/// but hosts must not read them as data (a chart's summary, its keyboard
/// walk). A rule is a dashed, titled 2-point polyline along one axis.
fn with_rule_marks(svg: String) -> String {
    if !svg.contains(" stroke-dasharray=\"") {
        return svg;
    }
    let mut out = String::with_capacity(svg.len() + 64);
    let mut rest = svg.as_str();
    while let Some(i) = rest.find("<polyline ") {
        let Some(end) = rest[i..].find('>').map(|e| i + e) else {
            break;
        };
        let tag = &rest[i..end];
        let pts: Vec<(&str, &str)> = tag
            .find(" points=\"")
            .map(|k| &tag[k + 9..])
            .and_then(|p| p.split('"').next())
            .map(|p| {
                p.split_whitespace()
                    .filter_map(|xy| xy.split_once(','))
                    .collect()
            })
            .unwrap_or_default();
        let axis = match pts[..] {
            [(x1, _), (x2, _)] if x1 == x2 => Some("x"),
            [(_, y1), (_, y2)] if y1 == y2 => Some("y"),
            _ => None,
        };
        let rule = axis.filter(|_| {
            tag.contains(" stroke-dasharray=\"")
                && !tag.ends_with('/')
                && rest[end + 1..].starts_with("<title>")
        });
        out.push_str(&rest[..end]);
        if let Some(a) = rule {
            out.push_str(&format!(" data-rule=\"{a}\""));
        }
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

/// Epoch seconds as `2026-10-12` (or `2026-10-12 08:30:00` off midnight).
fn xline_date_text(secs: i64) -> String {
    let day = crate::period::Date::from_days(secs.div_euclid(86_400)).iso();
    let s = secs.rem_euclid(86_400);
    if s == 0 {
        day
    } else {
        format!("{day} {:02}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    }
}

/// The `::LINESTYLE` values.
pub const LINE_STYLES: &[&str] = &["solid", "dashed", "flow"];

/// `::LINESTYLE`: dash the lines of the series whose rows say `'dashed'` or
/// `'flow'` (every data line when there is no `::CATEGORY`), and mark the
/// flowing ones `class="dp-flow"` — the browser moves their dashes (pure CSS),
/// static output keeps plain dashes.
fn with_line_styles(svg: String, cols: &[Column]) -> String {
    let Some(st) = find_role(cols, Role::LineStyle) else {
        return svg;
    };
    let cat = find_role(cols, Role::Category);
    // series → flowing?  (None key = every line: no ::CATEGORY)
    let mut styled: Vec<(Option<String>, bool)> = Vec::new();
    for (i, v) in st.values.iter().enumerate() {
        let Some(s) = v.as_str().map(|s| s.trim().to_ascii_lowercase()) else {
            continue;
        };
        if s != "flow" && s != "dashed" {
            continue;
        }
        let key = match cat {
            Some(c) => match c.values.get(i) {
                Some(Value::Str(k)) => Some(k.clone()),
                Some(v) => v.as_f64().map(|f| f.to_string()),
                None => None,
            },
            None => None,
        };
        if cat.is_some() && key.is_none() {
            continue;
        }
        match styled.iter_mut().find(|(k, _)| *k == key) {
            Some(e) => e.1 |= s == "flow",
            None => styled.push((key, s == "flow")),
        }
    }
    if styled.is_empty() {
        return svg;
    }
    let attr = |tag: &str, name: &str| -> Option<String> {
        let k = tag.find(&format!(" {name}=\""))? + name.len() + 3;
        Some(tag[k..k + tag[k..].find('"')?].to_string())
    };
    let mut out = String::with_capacity(svg.len() + 256);
    let mut rest = svg.as_str();
    while let Some(i) = rest.find("<polyline ") {
        let Some(end) = rest[i..].find('>').map(|e| i + e) else {
            break;
        };
        let tag = &rest[i..end];
        let points = attr(tag, "points").unwrap_or_default();
        // a data line: not a dashed rule, not a 2-point grid / axis line
        let data_line = !tag.contains(" stroke-dasharray=\"")
            && (tag.contains(" data-series=\"") || points.split_whitespace().count() > 2);
        let hit = if !data_line {
            None
        } else if cat.is_some() {
            let s = attr(tag, "data-series").map(|s| s.replace("&amp;", "&"));
            styled.iter().find(|(k, _)| s.is_some() && *k == s)
        } else {
            styled.first()
        };
        let (head, close) = match tag.strip_suffix('/') {
            Some(h) => (h, "/>"),
            None => (tag, ">"),
        };
        out.push_str(&rest[..i]);
        out.push_str(head);
        if let Some((_, flow)) = hit {
            out.push_str(" stroke-dasharray=\"6 4\"");
            if *flow {
                out.push_str(" class=\"dp-flow\"");
            }
        }
        out.push_str(close);
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    out
}

/// The `::ANIMATE` modes the browser plays.
pub const ANIMATE_MODES: &[&str] = &["auto", "draw", "grow", "reveal", "none"];

/// Pass a panel's motion directives to the browser as attributes on the SVG
/// root (static output ignores them): `data-animate` (`::ANIMATE`) and, for
/// `'reveal'`, `data-xline` — where the reveal starts, in the x axis' unit
/// (epoch seconds on a date axis, a level on a discrete one); `data-story` +
/// `data-story-by` (`::STORY`, `::STORY_BY`); `data-transition`
/// (`'tween'::TRANSITION`).
fn with_motion_attrs(mut svg: String, cols: &[Column]) -> String {
    if let Some(mode) = role_str(cols, Role::Animate).map(|m| m.to_ascii_lowercase()) {
        if ANIMATE_MODES.contains(&mode.as_str()) {
            if mode == "reveal" {
                if let Some(c) = find_role(cols, Role::VLine) {
                    let at = xline_nums(&c.values)
                        .first()
                        .map(|f| f.to_string())
                        .or_else(|| c.values.iter().find_map(|v| v.as_str().map(str::to_string)));
                    if let Some(at) = at {
                        svg = with_root_attr(svg, "data-xline", &at);
                    }
                }
            }
            svg = with_root_attr(svg, "data-animate", &mode);
        }
    }
    if role_str(cols, Role::Transition).is_some_and(|t| t.eq_ignore_ascii_case("tween")) {
        svg = with_root_attr(svg, "data-transition", "tween");
    }
    if let Some(steps) = story_steps(cols) {
        let by = match role_str(cols, Role::StoryBy).map(|b| b.to_ascii_lowercase()) {
            Some(b) if b == "series" && find_role(cols, Role::Category).is_some() => "series",
            _ => "x",
        };
        svg = with_root_attr(svg, "data-story-by", by);
        svg = with_root_attr(svg, "data-story", &steps);
    }
    svg
}

/// A cell as text: a string as is, a number without a trailing `.0`.
pub(crate) fn key_text(v: &Value) -> Option<String> {
    match v {
        Value::Str(s) => Some(s.clone()),
        Value::Na => None,
        v => v.as_f64().map(|f| f.to_string()),
    }
}

/// A cell as the mark's `data-x` / `data-series` spells it — ggplot-rs's
/// own formatting: a date (time) as `2026-09-01` (`2026-09-01 09:30:00`),
/// not its epoch seconds; a number rounded to 3 decimals.
fn mark_key_text(v: &Value) -> Option<String> {
    match v {
        Value::Na => None,
        v => Some(ggplot_rs::format::format_value(v)).filter(|s| !s.is_empty()),
    }
}

/// `::STORY` steps as JSON `[{"k": key, "t": caption}, …]`, in row order:
/// one per distinct key (the row's x, or its `::CATEGORY` with
/// `'series'::STORY_BY`) whose caption is non-empty; the first caption wins.
fn story_steps(cols: &[Column]) -> Option<String> {
    let story = find_role(cols, Role::Story)?;
    let by_series = role_str(cols, Role::StoryBy).is_some_and(|b| b.eq_ignore_ascii_case("series"));
    let keys = if by_series {
        find_role(cols, Role::Category)
    } else {
        None
    }
    .or_else(|| find_role(cols, Role::X))?;
    let mut steps: Vec<(String, String)> = Vec::new();
    for (i, v) in story.values.iter().enumerate() {
        let Some(t) = v.as_str().map(str::trim).filter(|t| !t.is_empty()) else {
            continue;
        };
        let Some(k) = keys.values.get(i).and_then(mark_key_text) else {
            continue;
        };
        if !steps.iter().any(|(sk, _)| *sk == k) {
            steps.push((k, t.to_string()));
        }
    }
    if steps.is_empty() {
        return None;
    }
    let json: Vec<serde_json::Value> = steps
        .into_iter()
        .map(|(k, t)| serde_json::json!({ "k": k, "t": t }))
        .collect();
    serde_json::to_string(&json).ok()
}

/// Give every hoverable mark its value as the panel formats it: the
/// plotting engine writes the raw measure (`data-value="1234.5"`); this adds
/// `data-fvalue="$1,234.50"` per the panel's `::YFORMAT` (or a percent kind)
/// and the dashboard locale, and — when the axis formats x (`::XFORMAT`, a
/// `::LOCALE` date order) — `data-fx` beside `data-x`. Hosts show these in
/// tooltips; the raw attributes stay for selection and filtering.
fn with_formatted_values(svg: String, cols: &[Column], o: &RenderOptions) -> String {
    let Some(value) = cols.iter().find(|c| matches!(c.role, Role::Value(_))) else {
        return svg;
    };
    let Role::Value(kind) = value.role else {
        return svg;
    };
    if !svg.contains(" data-value=\"") {
        return svg;
    }
    let fmt = value_fmt(cols, kind, &value.values);
    let xfmt = role_fmt(cols, Role::XFormat).map(|s| format::ValueFmt::parse(&s));
    let attr = |tag: &str, name: &str| -> Option<String> {
        let k = tag.find(&format!(" {name}=\""))? + name.len() + 3;
        Some(tag[k..k + tag[k..].find('"')?].to_string())
    };
    let mut out = String::with_capacity(svg.len() + svg.len() / 8);
    let mut rest = svg.as_str();
    while let Some(i) = rest.find(" data-value=\"") {
        // The whole start tag around this attribute.
        let start = rest[..i].rfind('<').unwrap_or(0);
        let Some(end) = rest[i..].find('>').map(|e| i + e) else {
            break;
        };
        let tag = &rest[start..end];
        out.push_str(&rest[..end]);
        let mut extra = String::new();
        if let Some(v) = attr(tag, "data-value").and_then(|v| v.parse::<f64>().ok()) {
            extra.push_str(&format!(
                " data-fvalue=\"{}\"",
                format::escape_xml(&fmt.full(v, &o.locale))
            ));
        }
        if let Some(x) = attr(tag, "data-x") {
            let fx = match (&xfmt, x.parse::<f64>()) {
                (Some(f), Ok(n)) => Some(f.full(n, &o.locale)),
                _ if !o.locale.is_default() => Some(o.locale.format_date(&x)).filter(|d| *d != x),
                _ => None,
            };
            if let Some(fx) = fx {
                extra.push_str(&format!(" data-fx=\"{}\"", format::escape_xml(&fx)));
            }
        }
        // Before a self-closing `/`.
        if out.ends_with('/') {
            out.pop();
            out.push_str(&extra);
            out.push('/');
        } else {
            out.push_str(&extra);
        }
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

/// The ggplot of one panel, for composing into a [`PlotGrid`] (`None` when
/// the panel is one of the extension's own SVGs, e.g. a "No data" note).
pub(crate) fn panel_plot(
    cols: &[Column],
    width: u32,
    height: u32,
    o: &RenderOptions,
) -> Result<Option<GGPlot>, String> {
    let (w, h) = (clamp_dim(width as u64), clamp_dim(height as u64));
    let cols = boxstats::prepare(downsample::prepare(cols, o))?;
    Ok(match render_inner(&cols, w, h, o)?.localized(o, &cols) {
        Panel::Plot { plot, .. } => Some(*plot),
        Panel::Own { .. } => None,
    })
}

/// Write a [`PlotGrid`] at `place` (with its sub-plots' warnings for a
/// standalone document).
pub(crate) fn finish_grid(
    grid: PlotGrid,
    place: Place,
    width: u32,
    height: u32,
) -> Result<(String, Vec<String>), String> {
    let (w, h) = (clamp_dim(width as u64), clamp_dim(height as u64));
    match place {
        Place::Doc => grid.render_svg_native_with_warnings(w, h),
        Place::At(x, y) => grid
            .render_svg_native_at(x, y, w, h)
            .map(|s| (s, Vec::new())),
    }
    .map(|(s, warnings)| (ticks::fit_x_labels(strip_nonfinite_marks(s)), warnings))
    .map_err(|e| format!("render failed: {e:?}"))
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

    // A thin panel under a chart, sharing its x axis.
    if find_role(cols, Role::Track).is_some() {
        return track::render(o, cols, width, height);
    }
    // A KPI / text card (value, change, sparkline, goal) — the same tile as
    // the headless dashboard; at most its natural height.
    if let Some(k) = kpi::Kpi::from_columns(cols, &o.locale) {
        let h = height.min(k.height().ceil() as u32).max(MIN_DIM);
        return Ok(Panel::own(width, h, k.body(width as f64, h as f64, o)));
    }
    // A map is driven by a ::MAP (geometry) column, coloured by an optional measure.
    if cols.iter().any(|c| {
        matches!(
            c.role,
            Role::Geometry | Role::Region | Role::Lat | Role::Lon
        )
    }) {
        return Ok(match map::render(o, cols, width, height) {
            Some(m) => Panel::Own {
                width,
                height,
                body: m.body,
                attrs: m.attrs,
                warnings: m.warnings,
            },
            None => note_svg(None, "No data", width, height),
        });
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
    // (A box from statistics is one row per box.)
    let need = if boxstats::is_stats(cols) {
        1
    } else {
        min_values(kind)
    };
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
        Kind::Pie => return render_pie(o, value, cols, title.as_deref(), 0.0, width, height),
        Kind::Donut => return render_pie(o, value, cols, title.as_deref(), 0.55, width, height),
        Kind::Gauge => return render_gauge(o, value, cols, title.as_deref(), width, height),
        Kind::Histogram => return render_histogram(o, value, title.as_deref(), width, height),
        Kind::Density => return render_density(o, value, cols, title.as_deref(), width, height),
        Kind::QQ => return render_qq(o, value, title.as_deref(), width, height),
        Kind::Heatmap => return render_heatmap(o, value, cols, title.as_deref(), width, height),
        Kind::Calendar => return render_calendar(o, value, cols, title.as_deref(), width, height),
        Kind::Candlestick => {
            return render_candlestick(o, value, cols, title.as_deref(), width, height)
        }
        Kind::Radar => return render_radar(o, value, cols, title.as_deref(), width, height),
        Kind::Sparkline => return render_sparkline(o, value, width, height),
        Kind::Waterfall => {
            return variance::render_waterfall(o, value, cols, title.as_deref(), width, height)
        }
        Kind::Bullet => {
            return variance::render_bullet(o, value, cols, title.as_deref(), width, height)
        }
        Kind::Funnel => {
            return variance::render_funnel(o, value, cols, title.as_deref(), width, height)
        }
        _ => {}
    }
    render_cartesian(o, kind, value, cols, title, width, height)
}

/// The first column carrying `role`.
fn find_role(cols: &[Column], role: Role) -> Option<&Column> {
    cols.iter().find(|c| c.role == role)
}

/// The first non-empty string value of a `role` column (directive-style
/// modifiers such as `'€'::YFORMAT`, `'log10'::YSCALE`).
fn role_str(cols: &[Column], role: Role) -> Option<String> {
    find_role(cols, role).and_then(|c| {
        c.values.iter().find_map(|v| match v {
            Value::Str(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
            _ => None,
        })
    })
}

/// The panel's `::THRESHOLDS` rules: `None` without the role, an error for a
/// malformed rule set.
fn thresholds_of(cols: &[Column]) -> Result<Option<thresholds::Rules>, String> {
    role_str(cols, Role::Thresholds)
        .map(|s| thresholds::Rules::parse(&s))
        .transpose()
}

/// A format spec (`::YFORMAT` / `::XFORMAT`) as written: a leading space is
/// meaningful (`' kg'` = a suffix), so only an all-blank value counts as unset.
fn role_fmt(cols: &[Column], role: Role) -> Option<String> {
    find_role(cols, role).and_then(|c| {
        c.values.iter().find_map(|v| match v {
            Value::Str(s) if !s.trim().is_empty() => Some(s.trim_end().to_string()),
            _ => None,
        })
    })
}

/// The first finite numeric value of a `role` column.
fn role_num(cols: &[Column], role: Role) -> Option<f64> {
    find_role(cols, role).and_then(|c| {
        c.values
            .iter()
            .find_map(|v| v.as_f64().filter(|f| f.is_finite()))
    })
}

/// Whether grouped marks on a discrete x sit side by side: bars, and point
/// ranges (a point measure with ::YMIN/::YMAX), dodged by ::CATEGORY.
fn discrete_dodge(kind: Kind, cols: &[Column]) -> bool {
    let x_discrete = find_role(cols, Role::X)
        .is_some_and(|x| x.values.iter().any(|v| matches!(v, Value::Str(_))));
    let category =
        find_role(cols, Role::Category).is_some_and(|c| c.values.iter().any(|v| *v != Value::Na));
    let interval = find_role(cols, Role::YMin).is_some() && find_role(cols, Role::YMax).is_some();
    x_discrete
        && category
        && (matches!(kind, Kind::Bar | Kind::BarPercent)
            || (matches!(kind, Kind::Point | Kind::Bubble) && interval))
}

/// A discrete x axis turned into numeric slots `1, 2, …` with each group
/// offset inside its slot — only for grouped marks with ::DATALABELS, which
/// cannot follow ggplot-rs's discrete `position_dodge` (geom_text ignores the
/// stored dodge offset).
///
/// TODO(ggplot-rs): drop once geom_text honours `position_dodge` on a
/// discrete x.
struct Dodged {
    x: Vec<Value>,
    breaks: Vec<f64>,
    labels: Vec<String>,
    /// Width of one group's sub-slot.
    step: f64,
}

fn dodge_discrete(x: &[Value], group: &[Value], width: f64) -> Dodged {
    let mut labels: Vec<String> = Vec::new();
    let mut slot: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for v in x.iter().filter(|v| !matches!(v, Value::Na)) {
        let s = value_str(v);
        if !slot.contains_key(&s) {
            slot.insert(s.clone(), labels.len());
            labels.push(s);
        }
    }
    // Groups in the sorted order of the colour scale, so the left-to-right
    // order inside a slot matches the legend.
    let groups: Vec<String> = distinct_labels(&Column::new("", Role::Category, group.to_vec()));
    let n = groups.len().max(1) as f64;
    let step = width / n;
    let xs = x
        .iter()
        .zip(group)
        .map(|(v, g)| {
            if matches!(v, Value::Na) {
                return Value::Na;
            }
            let base = slot[&value_str(v)] as f64 + 1.0;
            let gi = groups.iter().position(|l| *l == value_str(g)).unwrap_or(0) as f64;
            Value::Float(base + (gi - (n - 1.0) / 2.0) * step)
        })
        .collect();
    Dodged {
        x: xs,
        breaks: (1..=labels.len()).map(|i| i as f64).collect(),
        labels,
        step,
    }
}

/// Half-width of error-bar caps in normalized panel units: 18 % of one bar
/// (`slot` x units wide) on a discrete x, else 20 % of the smallest gap
/// between x values.
fn cap_half_width(
    x: &[Value],
    levels: Option<&[String]>,
    dodged: &Option<Dodged>,
    slot: f64,
) -> f64 {
    if let Some(d) = dodged {
        // Slots 1..=n on limits [0.4, n + 0.6].
        return 0.18 * d.step / (d.labels.len() as f64 + 0.2);
    }
    if x.iter().any(|v| matches!(v, Value::Str(_))) {
        let n = levels.map_or_else(
            || distinct_labels(&Column::new("", Role::X, x.to_vec())).len(),
            <[String]>::len,
        );
        return 0.18 * slot / n.max(1) as f64;
    }
    let mut v: Vec<f64> = x
        .iter()
        .filter_map(|v| v.as_f64())
        .filter(|f| f.is_finite())
        .collect();
    v.sort_by(f64::total_cmp);
    v.dedup();
    let gap = v
        .windows(2)
        .map(|w| w[1] - w[0])
        .fold(f64::INFINITY, f64::min);
    match (v.first(), v.last()) {
        // The default 5 % expansion on each side.
        (Some(lo), Some(hi)) if gap.is_finite() && hi > lo => {
            (0.2 * gap / ((hi - lo) * 1.1)).min(0.03)
        }
        _ => 0.03,
    }
}

/// `::BARCHART_PERCENT` values as fractions: input already in `[-1, 1]` is a
/// fraction (a rate or share) and is drawn as given; anything else (counts,
/// amounts) becomes each bar's share of its x's total — or of the grand total
/// without a `::CATEGORY` — so the percent axis reads 0–100 %.
fn percent_shares(x: &[Value], y: &[Value], by_x: bool) -> Vec<Value> {
    let nums = || y.iter().filter_map(|v| v.as_f64());
    if nums().all(|f| f.abs() <= 1.0) {
        return y.to_vec();
    }
    let mut totals: std::collections::HashMap<String, f64> = std::collections::HashMap::new();
    for (xv, yv) in x.iter().zip(y) {
        let k = if by_x { value_str(xv) } else { String::new() };
        *totals.entry(k).or_insert(0.0) += yv.as_f64().unwrap_or(0.0);
    }
    x.iter()
        .zip(y)
        .map(|(xv, yv)| {
            let k = if by_x { value_str(xv) } else { String::new() };
            match (yv.as_f64(), totals.get(&k)) {
                (Some(f), Some(t)) if *t != 0.0 => Value::Float(f / t),
                _ => Value::Na,
            }
        })
        .collect()
}

/// Each value's share of its stack — the sum of the values at its x (and
/// `::FACET` panel) — for `::BARCHART_STACKED_PERCENT`; missing when the stack
/// sums to zero. Unlike [`percent_shares`], values that are fractions
/// already are divided too: the stack is drawn as their shares (#218).
fn stack_shares(x: &[Value], facet: Option<&[Value]>, y: &[Value]) -> Vec<Value> {
    let key = |i: usize| {
        let panel = facet
            .and_then(|f| f.get(i))
            .map(value_str)
            .unwrap_or_default();
        (panel, value_str(&x[i]))
    };
    let mut totals: std::collections::HashMap<(String, String), f64> = Default::default();
    for (i, v) in y.iter().enumerate() {
        *totals.entry(key(i)).or_default() += v.as_f64().unwrap_or(0.0);
    }
    y.iter()
        .enumerate()
        .map(|(i, v)| match (v.as_f64(), totals.get(&key(i))) {
            (Some(f), Some(t)) if *t != 0.0 => Value::Float(f / t),
            _ => Value::Na,
        })
        .collect()
}

/// Percent tick labels for `::LINECHART_PERCENT`: fractions (all |v| ≤ 1) are
/// scaled ×100 (`0.42` → `42%`); larger values are already percentages.
fn percent_axis(values: &[Value]) -> Box<dyn Fn(f64) -> String + Send + Sync> {
    if values
        .iter()
        .filter_map(|v| v.as_f64())
        .all(|f| f.abs() <= 1.0)
    {
        Box::new(ggplot_rs::scale::format::label_percent)
    } else {
        Box::new(|v: f64| format!("{}%", ggplot_rs::scale::format::label_comma(v)))
    }
}

/// A `'log10' | 'sqrt' | 'reverse'` axis transform (`::XSCALE`/`::YSCALE`).
fn axis_transform(
    spec: &str,
    role: Role,
) -> Result<ggplot_rs::scale::transform::ScaleTransform, String> {
    use ggplot_rs::scale::transform::ScaleTransform as T;
    match spec.to_ascii_lowercase().as_str() {
        "log10" | "log" => Ok(T::Log10),
        "sqrt" => Ok(T::Sqrt),
        "reverse" | "rev" => Ok(T::Reverse),
        "identity" | "linear" | "none" => Ok(T::Identity),
        other => Err(format!(
            "::{} '{other}' is not one of 'log10', 'sqrt', 'reverse'",
            role.token()
        )),
    }
}

/// Rows `idx` of a plot frame (for per-group / subset layers).
fn subset(data: &[(String, Vec<Value>)], idx: &[usize]) -> Vec<(String, Vec<Value>)> {
    data.iter()
        .map(|(k, v)| (k.clone(), idx.iter().map(|&i| v[i].clone()).collect()))
        .collect()
}

/// Row indices grouped by the (string) keys of the given columns, in
/// first-seen order.
fn group_rows(n: usize, keys: &[&[Value]]) -> Vec<Vec<usize>> {
    let mut order: Vec<Vec<usize>> = Vec::new();
    let mut at: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for i in 0..n {
        let k: String = keys
            .iter()
            .map(|c| value_str(&c[i]))
            .collect::<Vec<_>>()
            .join("\u{1}");
        let g = *at.entry(k).or_insert_with(|| {
            order.push(Vec::new());
            order.len() - 1
        });
        order[g].push(i);
    }
    order
}

/// The cartesian chart kinds (bars, lines, areas, points, box/violin plots)
/// with every encoding/annotation/modifier role.
///
/// Grouped marks on a discrete x (see [`discrete_dodge`]) are drawn with
/// `position_dodge`, which orders the groups by first appearance: the rows
/// are reordered by ::CATEGORY (the sorted legend order) and rows without an
/// x dropped, and the x levels keep their original first-seen order.
fn render_cartesian(
    o: &RenderOptions,
    kind: Kind,
    value: &Column,
    cols: &[Column],
    title: Option<String>,
    width: u32,
    height: u32,
) -> Result<Panel, String> {
    let (Some(x), Some(cat)) = (find_role(cols, Role::X), find_role(cols, Role::Category)) else {
        return cartesian(o, kind, value, cols, title, width, height, None);
    };
    if !discrete_dodge(kind, cols) || find_role(cols, Role::DataLabels).is_some() {
        return cartesian(o, kind, value, cols, title, width, height, None);
    }
    let n = x.values.len();
    let mut levels: Vec<String> = Vec::new();
    for v in x.values.iter().filter(|v| **v != Value::Na) {
        let s = value_str(v);
        if !levels.contains(&s) {
            levels.push(s);
        }
    }
    let mut order: Vec<usize> = (0..n).filter(|&i| x.values[i] != Value::Na).collect();
    order.sort_by_cached_key(|&i| cat.values.get(i).map(value_str).unwrap_or_default());
    let reorder = |c: &Column| -> Column {
        if c.values.len() != n {
            return c.clone();
        }
        let values = order.iter().map(|&i| c.values[i].clone()).collect();
        Column::new(c.name.clone(), c.role, values)
    };
    let sorted: Vec<Column> = cols.iter().map(reorder).collect();
    let value = match cols.iter().position(|c| std::ptr::eq(c, value)) {
        Some(i) => sorted[i].clone(),
        None => reorder(value),
    };
    cartesian(o, kind, &value, &sorted, title, width, height, Some(levels))
}

/// [`render_cartesian`]; `x_levels` (the discrete x levels in order) is set
/// when grouped marks are dodged.
#[allow(clippy::too_many_arguments)]
fn cartesian(
    o: &RenderOptions,
    kind: Kind,
    value: &Column,
    cols: &[Column],
    title: Option<String>,
    width: u32,
    height: u32,
    x_levels: Option<Vec<String>>,
) -> Result<Panel, String> {
    let x = find_role(cols, Role::X).ok_or("no XAXIS column")?;
    // A ::CATEGORY that is entirely missing (e.g. a macro's NULL default) is
    // no category at all.
    let category =
        find_role(cols, Role::Category).filter(|c| c.values.iter().any(|v| *v != Value::Na));
    // `::LABEL_TOP k` turns ::LABEL into per-point text, so it is no title.
    let label_top = role_num(cols, Role::LabelTop).map(|k| k.clamp(0.0, 10_000.0) as usize);
    let title = if label_top.is_some() { None } else { title };

    let x_discrete = x.values.iter().any(|v| matches!(v, Value::Str(_)));
    let bar = matches!(
        kind,
        Kind::Bar | Kind::BarStacked | Kind::BarPercent | Kind::BarStackedPercent
    );
    let point_kind = matches!(kind, Kind::Point | Kind::Bubble);
    let interval = |lo: Role, hi: Role| -> Result<Option<(&Column, &Column)>, String> {
        match (find_role(cols, lo), find_role(cols, hi)) {
            (Some(a), Some(b)) => Ok(Some((a, b))),
            (None, None) => Ok(None),
            _ => Err(format!(
                "::{} needs a matching ::{}",
                lo.token(),
                hi.token()
            )),
        }
    };
    let y_interval = interval(Role::YMin, Role::YMax)?;
    let x_interval = interval(Role::XMin, Role::XMax)?;

    if x_discrete && x_interval.is_some() {
        return Err("::XMIN/::XMAX need a numeric ::XAXIS".into());
    }
    // Grouped marks on a discrete x sit side by side: `position_dodge` (rows
    // already ordered by `render_cartesian`), or — with ::DATALABELS, which
    // cannot follow it — numeric slots.
    let dodge_width = if bar { 0.9 } else { 0.6 };
    let dodge = x_levels.is_some();
    let dodged = (discrete_dodge(kind, cols) && !dodge)
        .then_some(category)
        .flatten()
        .map(|cat| dodge_discrete(&x.values, &cat.values, dodge_width));
    let xvals = dodged
        .as_ref()
        .map(|d| d.x.clone())
        .unwrap_or_else(|| x.values.clone());
    let yvals = match kind {
        Kind::BarPercent => percent_shares(&x.values, &value.values, category.is_some()),
        // Each segment's share of its bar, so the hover value and the data
        // label read e.g. `25%`, not the raw measure as a percent (#218).
        Kind::BarStackedPercent => stack_shares(
            &x.values,
            cols.iter()
                .find(|c| matches!(c.role, Role::Facet | Role::FacetFree))
                .map(|f| f.values.as_slice()),
            &value.values,
        ),
        _ => value.values.clone(),
    };

    let mut data: Vec<(String, Vec<Value>)> =
        vec![("x".to_string(), xvals), ("y".to_string(), yvals)];
    // Extra measure columns → additional overlaid layers (combo charts).
    let extras: Vec<&Column> = cols
        .iter()
        .filter(|c| matches!(c.role, Role::Value(_)))
        .skip(1)
        .collect();
    for (k, ev) in extras.iter().enumerate() {
        data.push((format!("y{}", k + 2), ev.values.clone()));
    }
    // Interval ends under non-canonical names: a column called `ymin` would be
    // read by geom_col as a stacked bar's base.
    // A missing interval end (e.g. a ridge coefficient, which has no
    // inference) collapses to the estimate: the mark is still drawn, just
    // without whiskers, instead of being dropped as non-finite.
    if let Some((lo, hi)) = y_interval {
        let y = &value.values;
        let or_y = |v: &[Value]| -> Vec<Value> {
            v.iter()
                .zip(y)
                .map(|(e, yv)| match e.as_f64() {
                    Some(f) if f.is_finite() => e.clone(),
                    _ => yv.clone(),
                })
                .collect()
        };
        data.push(("ilo".to_string(), or_y(&lo.values)));
        data.push(("ihi".to_string(), or_y(&hi.values)));
    }
    if let Some((lo, hi)) = x_interval {
        data.push(("jlo".to_string(), lo.values.clone()));
        data.push(("jhi".to_string(), hi.values.clone()));
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
    let percent = matches!(
        kind,
        Kind::BarPercent | Kind::BarStackedPercent | Kind::LinePercent
    );

    // Optional confidence bands around a line (`::BAND_LOWER`/`::BAND_UPPER`
    // pairs, matched in column order): one band, or a nested quantile fan.
    let bands = band_pairs(cols);
    let band_lo = bands.first().map(|b| b.0);
    for (k, (lo, hi)) in bands.iter().enumerate() {
        data.push((band_key("bandlo", k), lo.values.clone()));
        data.push((band_key("bandhi", k), hi.values.clone()));
    }
    // A box from statistics: its quartiles, whiskers and outliers.
    let box_stats = kind == Kind::Boxplot && boxstats::is_stats(cols);
    if box_stats {
        data.extend(boxstats::data_columns(cols));
    }
    // Censoring marks on a step curve (`::CENSOR`).
    let censor = kind == Kind::Step
        && find_role(cols, Role::Censor)
            .map(|c| data.push(("censor".to_string(), c.values.clone())))
            .is_some();
    // Small multiples (`::FACET` / `::FACET_FREE`): one panel per level.
    let facet = cols
        .iter()
        .find(|c| matches!(c.role, Role::Facet | Role::FacetFree));
    if let Some(f) = facet {
        let levels = f
            .values
            .iter()
            .map(|v| match v {
                Value::Na => Value::Str("NA".into()),
                v => Value::Str(value_str(v)),
            })
            .collect();
        data.push(("facet".to_string(), levels));
    }
    let mut aes = Aes::new().x("x").y("y");

    // Colour dimension: an explicit CATEGORY only. A bar chart without one
    // is drawn in the single brand colour — colouring each bar by its own x
    // level encodes nothing the axis does not already say.
    let color_col: Option<&Column> = if let Some(cat) = category {
        data.push(("cat".to_string(), cat.values.clone()));
        aes = if by_colour {
            aes.color("cat")
        } else {
            aes.fill("cat")
        };
        Some(cat)
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

    // Bubble scatter: a `::SIZE` measure maps to point area. A constant
    // (`4::SIZE`) is no measure: it sets the point size, without a legend.
    let fixed_size = find_role(cols, Role::Size).and_then(constant_size);
    if let Some(sz) = find_role(cols, Role::Size).filter(|_| fixed_size.is_none()) {
        data.push(("size".to_string(), sz.values.clone()));
        aes = aes.size("size");
    }
    // Data labels (`::DATALABELS`): the measure value drawn above each mark. The
    // label column's (first numeric) value, if any, sets the font size — e.g.
    // `14::DATALABELS` — otherwise a readable default.
    let datalabels = find_role(cols, Role::DataLabels);
    let show_labels = datalabels.is_some();
    let dlabel_size = datalabels
        .and_then(|c| c.values.iter().find_map(|v| v.as_f64()))
        .filter(|s| *s >= 5.0 && *s <= 40.0)
        .unwrap_or(11.0);
    if show_labels {
        // In the panel's value format (`$1,235`, `12%`), short form.
        let fmt = value_fmt(cols, kind, &data[1].1);
        let dl: Vec<Value> = data[1]
            .1
            .iter()
            .map(|v| match v.as_f64() {
                Some(f) => Value::Str(fmt.short(f, &o.locale)),
                None => Value::Str(fmt_label(v)),
            })
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
    let n_rows = value.values.len();
    let col_of = |data: &[(String, Vec<Value>)], k: &str| -> Vec<Value> {
        data.iter()
            .find(|(n, _)| n == k)
            .map(|(_, v)| v.clone())
            .unwrap_or_default()
    };
    // Per-series row groups (category × facet) for layers drawn per group.
    let series_rows = {
        let cat = col_of(&data, "cat");
        let fac = col_of(&data, "facet");
        let mut keys: Vec<&[Value]> = Vec::new();
        if !cat.is_empty() {
            keys.push(&cat);
        }
        if !fac.is_empty() {
            keys.push(&fac);
        }
        group_rows(n_rows, &keys)
    };
    // A missing measure is a gap: as NaN the plotting engine drops the row
    // for that layer (a `Value::Na` y would be drawn at the bottom edge) — e.g.
    // a forecast's history rows have no yhat but do have y.
    for (name, vals) in data.iter_mut() {
        let measure = matches!(name.as_str(), "y" | "ilo" | "ihi" | "jlo" | "jhi")
            || name.starts_with("band")
            || (name.starts_with('y') && name[1..].chars().all(|c| c.is_ascii_digit()));
        if measure {
            for v in vals.iter_mut().filter(|v| **v == Value::Na) {
                *v = Value::Float(f64::NAN);
            }
        }
    }
    let mut plot = GGPlot::new(data.clone()).aes(aes).primary_color(brand);

    // Shaded x-regions (`::MARKAREA`, one or more columns): a light band behind
    // the data per run of marked x-values (see `mark_regions`), the full panel
    // height (ymin/ymax = ∓Inf reach the panel edges and don't train the y
    // scale).
    let regions: Vec<(Value, Value)> = cols
        .iter()
        .filter(|c| c.role == Role::MarkArea)
        .flat_map(|c| mark_regions(c, &x.values))
        .collect();
    if !regions.is_empty() {
        let (x0, x1): (Vec<Value>, Vec<Value>) = regions.into_iter().unzip();
        let n = x0.len();
        let frame = vec![
            ("mx0".to_string(), x0),
            ("mx1".to_string(), x1),
            ("my0".to_string(), vec![Value::Float(f64::NEG_INFINITY); n]),
            ("my1".to_string(), vec![Value::Float(f64::INFINITY); n]),
        ];
        plot = plot
            .geom_rect_with(GeomRect {
                fill: (148, 160, 178),
                color: (148, 160, 178),
                alpha: 0.14,
                line_width: 0.0,
            })
            .layer_data(frame)
            // (x as well: the layer is drawn first, and its x column is
            // what makes a date axis a date scale)
            .layer_aes(
                Aes::new()
                    .x("mx0")
                    .xmin("mx0")
                    .xmax("mx1")
                    .ymin("my0")
                    .ymax("my1"),
            );
    }

    // A ::BAND (prediction interval) matches the forecast at half opacity, so it
    // reads as that series' uncertainty: the series of the right-most row that
    // carries a band (else the last coloured series).
    let band_color: (u8, u8, u8) = color_col
        .and_then(|c| band_series_color(o, c, &x.values, band_lo))
        .or_else(|| color_col.and_then(|c| last_level_color(o, c)))
        .unwrap_or(brand);
    // The band is drawn first so the line sits on top of it; one band per
    // series. A step chart (Kaplan–Meier) gets a step ribbon in each series'
    // colour.
    let step_band = kind == Kind::Step && !bands.is_empty();
    // Several series each with their own band (e.g. coefficient paths of two
    // models): each band takes its series' colour. A forecast's band, carried
    // by one series only, keeps the single band colour.
    let multi_band = match (category, band_lo) {
        (Some(cat), Some(lo)) => {
            let mut with_band = std::collections::HashSet::new();
            for (c, b) in cat.values.iter().zip(&lo.values) {
                if b.as_f64().is_some_and(f64::is_finite) {
                    with_band.insert(value_str(c));
                }
            }
            with_band.len() > 1
        }
        _ => false,
    };
    // A nested fan (several pairs, e.g. 95 / 80 / 50 %) is drawn widest first,
    // each band at a low opacity: where they overlap the shade deepens, so the
    // inner quantiles read darkest.
    let fan_alpha = |one: f64| match bands.len() {
        0 | 1 => one,
        n => (0.4 / n as f64).clamp(0.1, 0.2),
    };
    for k in fan_order(&bands) {
        let (lo, hi) = (band_key("bandlo", k), band_key("bandhi", k));
        // Only the rows that carry the band (a forecast's, not its history's):
        // the rest would just be dropped as non-finite, with a warning.
        let (los, his) = (col_of(&data, &lo), col_of(&data, &hi));
        let finite = |v: &Value| v.as_f64().is_some_and(f64::is_finite);
        let rows: Vec<usize> = (0..n_rows)
            .filter(|&i| los.get(i).is_some_and(finite) && his.get(i).is_some_and(finite))
            .collect();
        if rows.is_empty() {
            continue;
        }
        let mut aes = Aes::new().x("x").ymin(&lo).ymax(&hi);
        if step_band {
            if category.is_some() {
                aes = aes.fill("cat");
            }
            plot = plot
                .geom_stepribbon_with(GeomStepribbon {
                    fill: band_color,
                    alpha: fan_alpha(0.2),
                    direction: StepDirection::Hv,
                })
                .layer_data(subset(&data, &rows))
                .layer_aes(aes);
        } else {
            if multi_band {
                aes = aes.fill("cat");
            }
            plot = plot
                .geom_ribbon_with(GeomRibbon {
                    fill: band_color,
                    alpha: fan_alpha(if multi_band { 0.18 } else { 0.25 }),
                })
                .layer_data(subset(&data, &rows))
                .layer_aes(aes);
        }
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
    // A light brand wash for box/violin bodies (unless a CATEGORY fills them).
    let body_fill = wash(o, brand, 0.72);
    plot = match kind {
        Kind::Bar | Kind::BarPercent if dodge => plot
            .geom_col_with(card_col(o))
            .position(position_dodge(dodge_width)),
        // (A slotted x is already dodged, see `dodge_discrete`.)
        Kind::Bar | Kind::BarPercent => plot.geom_col_with(card_col(o)),
        Kind::BarStacked => plot
            .geom_col_with(card_col(o))
            .position(facet_position::PerFacet(PositionStack)),
        Kind::BarStackedPercent => plot
            .geom_col_with(card_col(o))
            .position(facet_position::PerFacet(PositionFill)),
        // Lines/areas also get point markers — they carry the per-point `<title>`
        // so every chart is hoverable (and clickable for linking).
        Kind::Line | Kind::LinePercent => plot
            .geom_line_with(thin_line())
            .geom_point_with(small_point()),
        // One step line per series (+ `::CENSOR` marks).
        Kind::Step => {
            plot = plot
                .geom_step_with(GeomStep {
                    color: brand,
                    width: 1.2,
                    ..Default::default()
                })
                .geom_point_with(small_point());
            if censor {
                plot = plot.geom_censor_marks_with(
                    GeomCensorMarks {
                        color: brand,
                        size: 4.0,
                        ..Default::default()
                    },
                    "censor",
                );
            }
            plot
        }
        // Scatter + a trend line (no CI ribbon) — an analytical "smooth".
        Kind::Smooth => {
            plot = plot.geom_point_with(small_point());
            let method = role_str(cols, Role::SmoothMethod).map(|s| s.to_ascii_lowercase());
            use ggplot_rs::stat::smooth::SmoothMethod;
            let fit = |plot: GGPlot, method: SmoothMethod| {
                plot.geom_smooth_with(GeomSmooth {
                    color: brand,
                    se: false,
                    line_width: 2.0,
                    method,
                    ..Default::default()
                })
            };
            match method.as_deref() {
                None | Some("loess") => fit(plot, SmoothMethod::Loess { span: 0.75 }),
                // A Gaussian GLM with identity link is ordinary least squares.
                // ggplot-rs's GeomSmooth::glm (binomial/gamma/negative-binomial
                // families) needs its `regression` feature — not used here, see
                // `smooth`.
                Some("lm") | Some("glm") | Some("linear") => fit(plot, SmoothMethod::Lm),
                Some("gam") => {
                    // P-spline per series (see `smooth::pspline`).
                    let xs = col_of(&data, "x");
                    let ys = col_of(&data, "y");
                    for rows in &series_rows {
                        let pts: Vec<(f64, f64)> = rows
                            .iter()
                            .filter_map(|&i| Some((xs[i].as_f64()?, ys[i].as_f64()?)))
                            .collect();
                        let Some(curve) = smooth::pspline(&pts, 80) else {
                            continue;
                        };
                        let mut frame = vec![
                            (
                                "x".to_string(),
                                curve.iter().map(|p| Value::Float(p.0)).collect(),
                            ),
                            (
                                "y".to_string(),
                                curve.iter().map(|p| Value::Float(p.1)).collect(),
                            ),
                        ];
                        for key in ["cat", "facet"] {
                            let c = col_of(&data, key);
                            if let Some(v) = c.get(rows[0]) {
                                frame.push((key.to_string(), vec![v.clone(); curve.len()]));
                            }
                        }
                        plot = plot
                            .geom_line_with(GeomLine {
                                color: brand,
                                width: 2.0,
                                ..Default::default()
                            })
                            .layer_data(frame);
                    }
                    plot
                }
                Some(other) => {
                    return Err(format!(
                        "::SMOOTH_METHOD '{other}' is not one of 'loess', 'lm', 'gam', 'glm'"
                    ))
                }
            }
        }
        Kind::Area => plot.geom_area().geom_point_with(small_point()),
        Kind::AreaStacked => plot
            .geom_area_with(GeomArea {
                fill: brand,
                color: brand,
                alpha: 0.85,
                ..Default::default()
            })
            .position(facet_position::PerFacet(PositionStack)),
        // With a y interval a point becomes a pointrange (drawn below).
        Kind::Point | Kind::Bubble if y_interval.is_some() => plot,
        Kind::Point | Kind::Bubble => match fixed_size {
            Some(size) => plot.geom_point_with(GeomPoint {
                size,
                color: brand,
                ..Default::default()
            }),
            None => plot.geom_point(),
        },
        Kind::Jitter => plot.geom_jitter(),
        // Box plots and violins: a light brand body with a dark outline (a
        // CATEGORY fill still wins).
        Kind::Boxplot if box_stats => {
            let mut p = plot
                .geom_boxplot_with(GeomBoxplot {
                    fill: body_fill,
                    color: o.theme.chart_label(),
                    width: 0.6,
                    alpha: 1.0,
                })
                .stat(boxstats::StatBoxStats)
                .layer_aes(boxstats::layer_aes());
            // The outliers and means lie beyond the whiskers the y scale sees.
            let (ex, ey) = boxstats::extent_rows(cols);
            if !ex.is_empty() {
                p = p
                    .geom_blank()
                    .layer_data(vec![("x".to_string(), ex), ("y".to_string(), ey)])
                    .layer_aes(Aes::new().x("x").y("y"));
            }
            if let Some(mean) = find_role(cols, Role::Mean) {
                let n = mean.values.len();
                p = p
                    .geom_point_with(GeomPoint {
                        size: 2.6,
                        color: o.theme.chart_label(),
                        alpha: 1.0,
                    })
                    .layer_data(vec![
                        ("x".to_string(), x.values.clone()),
                        ("y".to_string(), mean.values.clone()),
                        (
                            "label".to_string(),
                            vec![Value::Str(boxstats::MEAN_LABEL.into()); n],
                        ),
                    ])
                    .layer_aes(Aes::new().x("x").y("y").label("label"));
            }
            p
        }
        Kind::Boxplot => plot.geom_boxplot_with(GeomBoxplot {
            fill: body_fill,
            color: o.theme.chart_label(),
            width: 0.6,
            alpha: 1.0,
        }),
        Kind::Violin => plot.geom_violin_with(GeomViolin {
            fill: body_fill,
            color: theme::mix(brand, o.theme.colors.ink, 0.35),
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
        | Kind::Sparkline
        | Kind::Waterfall
        | Kind::Bullet
        | Kind::Funnel => {
            unreachable!("handled above")
        }
    };
    // Intervals: a point measure becomes a pointrange; any other measure gets
    // capped error bars; an x interval is a horizontal error bar through y.
    if y_interval.is_some() {
        let span = Aes::new().x("x").ymin("ilo").ymax("ihi");
        plot = if point_kind {
            plot.geom_pointrange_with(GeomPointrange {
                color: brand,
                width: 1.4,
                size: 2.6,
                alpha: 1.0,
            })
            .layer_aes(span.y("y"))
        } else {
            plot.geom_errorbar_with(GeomErrorbar {
                color: (60, 60, 60),
                width: 1.0,
                cap_width: cap_half_width(&col_of(&data, "x"), x_levels.as_deref(), &dodged, {
                    let groups = category.map_or(1, |c| distinct_labels(c).len());
                    dodge_width / if dodge { groups.max(1) as f64 } else { 1.0 }
                }),
                alpha: 1.0,
            })
            .layer_aes(span)
        };
        if dodge {
            plot = plot.position(position_dodge(dodge_width));
        }
    }
    if x_interval.is_some() {
        plot = plot
            .geom_errorbarh_with(GeomErrorbarh {
                color: brand,
                width: 1.4,
                cap_height: 0.012,
                alpha: 1.0,
            })
            .layer_aes(Aes::new().y("y").xmin("jlo").xmax("jhi"));
    }
    // Data labels (`::DATALABELS`): draw the measure value just above each mark
    // — on a stacked mark, in the middle of its segment, stacked the way the
    // mark is (#219).
    if show_labels {
        let stacked = matches!(
            kind,
            Kind::BarStacked | Kind::BarStackedPercent | Kind::AreaStacked
        );
        plot = plot
            .geom_text_with(GeomText {
                size: dlabel_size,
                color: o.theme.chart_label(),
                // Lift the label clear of the mark (a small gap above the
                // top); a stacked segment's label is centred in it.
                vjust: if stacked { 0.5 } else { -0.35 },
                ..Default::default()
            })
            .layer_aes(Aes::new().x("x").y("y").label("dlab"));
        plot = match kind {
            Kind::BarStacked | Kind::AreaStacked => plot.position(facet_position::Centred(
                facet_position::PerFacet(PositionStack),
            )),
            Kind::BarStackedPercent => plot.position(facet_position::Centred(
                facet_position::PerFacet(PositionFill),
            )),
            _ => plot,
        };
    }
    // Combo layers: overlay each extra measure with its own geom + y column.
    for (k, ev) in extras.iter().enumerate() {
        if let Role::Value(ekind) = ev.role {
            let yk = format!("y{}", k + 2);
            let mut lay = Aes::new().x("x").y(&yk);
            if !combo_names.is_empty() {
                lay = lay.color(&format!("__s{}", k + 1));
            }
            // A bar-based combo keeps its bars in the brand colour (no combo
            // legend): each overlay takes the next palette colour, so a line
            // never disappears into bars of the same colour.
            let own = (bar && combo_names.is_empty()).then(|| o.theme.color(k + 1));
            plot = match (ekind, own) {
                (Kind::Line, Some(color)) => plot.geom_line_with(GeomLine {
                    color,
                    width: 2.0,
                    ..Default::default()
                }),
                (Kind::Area, Some(color)) => plot.geom_area_with(GeomArea {
                    fill: color,
                    color,
                    alpha: 0.35,
                    ..Default::default()
                }),
                (Kind::Line, None) => plot.geom_line(),
                (Kind::Area, None) => plot.geom_area(),
                // A point overlay (e.g. detected changepoints/peaks) reads as a
                // marker on the base line, so make it a touch larger.
                (Kind::Point, own) => plot.geom_point_with(GeomPoint {
                    color: own.unwrap_or(brand),
                    size: 3.4,
                    ..Default::default()
                }),
                _ => plot.geom_col_with(card_col(o)),
            }
            .layer_aes(lay);
        }
    }
    // Top-k point labels (`::LABEL_TOP k` + `::LABEL`, ranked by `::RANK` or
    // |y|), repelled from each other and from their points.
    if let Some(k) = label_top {
        let xs = col_of(&data, "x");
        let ys = col_of(&data, "y");
        let rank = find_role(cols, Role::Rank);
        let score = |i: usize| -> f64 {
            match rank {
                Some(r) => r.values.get(i).and_then(|v| v.as_f64()),
                None => ys[i].as_f64().map(f64::abs),
            }
            .filter(|f| f.is_finite())
            .unwrap_or(f64::NEG_INFINITY)
        };
        let mut idx: Vec<usize> = (0..n_rows)
            .filter(|&i| xs[i] != Value::Na && ys[i].as_f64().is_some_and(f64::is_finite))
            .collect();
        idx.sort_by(|&a, &b| score(b).total_cmp(&score(a)).then(a.cmp(&b)));
        idx.truncate(k);
        if !idx.is_empty() {
            let text = find_role(cols, Role::Label);
            let mut frame = subset(&data, &idx);
            frame.retain(|(n, _)| matches!(n.as_str(), "x" | "y" | "facet"));
            frame.push((
                "txt".to_string(),
                idx.iter()
                    .map(|&i| match text.and_then(|t| t.values.get(i)) {
                        Some(v) if *v != Value::Na => Value::Str(value_str(v)),
                        _ => Value::Str(fmt_label(&ys[i])),
                    })
                    .collect(),
            ));
            plot = plot
                .geom_text_repel_with(GeomTextRepel {
                    size: 10.0,
                    color: o.theme.chart_label(),
                    repel: RepelParams {
                        segment_color: Some((150, 156, 168)),
                        ..Default::default()
                    },
                    ..Default::default()
                })
                .layer_data(frame)
                .layer_aes(Aes::new().x("x").y("y").label("txt"));
        }
    }
    // Reference lines, one per distinct value of the column: horizontal
    // (`::REFLINE`/`::YLINE` — an average line, min/max bands, thresholds…)
    // and vertical (`::XLINE`, a continuous x). Data-mapped hlines/vlines:
    // they train their axis, appear in every facet panel and survive
    // coord_flip.
    let flipped = cols.iter().any(|c| c.role == Role::Flip);
    for (role, col, vertical) in [
        (Role::RefLine, "yintercept", false),
        (Role::VLine, "xintercept", true),
    ] {
        let Some(c) = find_role(cols, role) else {
            continue;
        };
        let vals = if vertical {
            xline_nums(&c.values)
        } else {
            distinct_nums(&c.values)
        };
        if vals.is_empty() {
            continue;
        }
        let frame = vec![(
            col.to_string(),
            vals.into_iter().map(Value::Float).collect(),
        )];
        // A quiet dashed rule in the chart text colour (not ggplot's black).
        let ink = o.theme.chart_text();
        plot = if vertical {
            plot.geom_vline_aes_with(
                GeomVline {
                    xintercept: 0.0,
                    color: ink,
                    width: 0.8,
                    linetype: Linetype::Dashed,
                    alpha: 1.0,
                },
                Aes::new().xintercept(col),
            )
        } else {
            plot.geom_hline_aes_with(
                GeomHline {
                    yintercept: 0.0,
                    color: ink,
                    width: 0.8,
                    linetype: Linetype::Dashed,
                    alpha: 1.0,
                },
                Aes::new().yintercept(col),
            )
        }
        .layer_data(frame);
    }
    plot = data_annotations(plot, o, kind, cols, &data, x_discrete);
    // Threshold lines (`::THRESHOLDS`): one at each bound where the status
    // changes, in the colour of the worse side.
    if let Some(rules) = thresholds_of(cols)? {
        for (y, st) in rules.boundaries() {
            plot = plot
                .geom_hline_aes_with(
                    GeomHline {
                        yintercept: 0.0,
                        color: st.color(&o.theme),
                        width: 1.1,
                        linetype: Linetype::Dashed,
                        alpha: 0.9,
                    },
                    Aes::new().yintercept("ty"),
                )
                .layer_data(vec![("ty".to_string(), vec![Value::Float(y)])]);
        }
    }
    // Cook's-distance contours (`::COOKS_CONTOUR p`) on residuals vs leverage.
    if let Some(p) = role_num(cols, Role::CooksContour).filter(|p| *p >= 1.0) {
        plot = plot.stat_cooks_contour(p.min(1e6) as usize, &[0.5, 1.0]);
    }
    // Straight lines in data units, across the panel: `::ABLINE
    // 'slope,intercept'` and `::IDENTITY` (y = x, dashed).
    let mut lines: Vec<(f64, f64)> = Vec::new();
    if let Some(ab) = find_role(cols, Role::AbLine) {
        let mut seen = std::collections::HashSet::new();
        for v in ab.values.iter().filter(|v| **v != Value::Na) {
            let s = value_str(v);
            if !seen.insert(s.clone()) {
                continue;
            }
            let p: Vec<f64> = s
                .trim_matches(|c| c == '[' || c == ']')
                .split([',', ';'])
                .filter_map(|t| t.trim().parse::<f64>().ok())
                .collect();
            match p.as_slice() {
                [slope, intercept] if slope.is_finite() && intercept.is_finite() => {
                    lines.push((*slope, *intercept))
                }
                _ => return Err(format!("::ABLINE '{s}' must be 'slope,intercept'")),
            }
        }
    }
    if !x_discrete {
        use ggplot_rs::render::backend::Linetype;
        let identity = find_role(cols, Role::Identity).is_some();
        let styled = identity
            .then_some((1.0, 0.0, (130, 136, 148), Linetype::Dashed))
            .into_iter()
            .chain(
                lines
                    .iter()
                    .map(|&(b, a)| (b, a, (90, 98, 112), Linetype::Solid)),
            );
        for (slope, intercept, color, linetype) in styled {
            plot = plot.geom_abline_with(GeomAbline {
                slope,
                intercept,
                color,
                width: 1.1,
                linetype,
                alpha: 0.9,
            });
        }
    }
    if let Some(col) = color_col {
        // The theme palette, levels sorted so a series keeps its colour across
        // charts; `#rrggbb` levels are drawn in that colour.
        // A `::COLORS` level map pins named levels to status colours.
        plot = if by_colour {
            plot.scale_color(category_scale(o, Aesthetic::Color, col, cols))
        } else {
            plot.scale_fill(category_scale(o, Aesthetic::Fill, col, cols))
        };
        if (step_band && category.is_some()) || multi_band {
            plot = plot.scale_fill(category_scale(o, Aesthetic::Fill, col, cols));
        }
    }
    if !combo_names.is_empty() {
        // Combo measures → distinct palette colours in column order, keyed by
        // column header.
        plot = plot.scale_color(
            ScaleColorDiscrete::new(Aesthetic::Color)
                .with_levels(combo_names.clone())
                .with_palette(dz_palette(o)),
        );
    }
    if bar && category.is_none() && combo_names.is_empty() {
        // A plain bar chart is one series: a legend of one says nothing.
        plot = plot.show_legend(false);
    }
    // One continuous scale per axis, combining tick format (`::YFORMAT`,
    // percent kinds), transform (`::YSCALE`) and — for a dodged discrete x —
    // the level breaks/labels.
    use ggplot_rs::scale::continuous::ScaleContinuous;
    let y_fmt = role_fmt(cols, Role::YFormat).and_then(|s| axis_formatter(&s, &o.locale));
    let y_fmt = if percent {
        let f: Box<dyn Fn(f64) -> String + Send + Sync> = if kind == Kind::LinePercent {
            percent_axis(&value.values)
        } else {
            Box::new(ggplot_rs::scale::format::label_percent)
        };
        Some(f)
    } else {
        y_fmt
    };
    let y_tr = role_str(cols, Role::YScale)
        .map(|s| axis_transform(&s, Role::YScale))
        .transpose()?;
    if y_fmt.is_some() || y_tr.is_some() {
        let mut s = ScaleContinuous::new();
        if let Some(f) = y_fmt {
            s = s.with_label_formatter(f);
        }
        if let Some(t) = y_tr {
            s = s.with_transform(t);
        }
        plot = plot.scale_y_continuous(s);
    }
    // An x formatter/transform only makes sense on a continuous x (numeric/date),
    // not on the discrete category axis of a bar chart.
    if let Some(levels) = &x_levels {
        plot = plot.scale_x_discrete(
            ScaleDiscrete::new().with_limits(levels.iter().map(String::as_str).collect()),
        );
    } else if let Some(d) = &dodged {
        plot = plot.scale_x_continuous(
            ScaleContinuous::new()
                .with_limits(0.4, d.labels.len() as f64 + 0.6)
                .with_expand(0.0, 0.0)
                .with_breaks(d.breaks.clone())
                .with_labels(d.labels.clone()),
        );
    } else if !x_discrete {
        let x_fmt = role_fmt(cols, Role::XFormat).and_then(|s| axis_formatter(&s, &o.locale));
        let x_tr = role_str(cols, Role::XScale)
            .map(|s| axis_transform(&s, Role::XScale))
            .transpose()?;
        if x_fmt.is_some() || x_tr.is_some() {
            let mut s = ScaleContinuous::new();
            if let Some(f) = x_fmt {
                s = s.with_label_formatter(f);
            }
            if let Some(t) = x_tr {
                s = s.with_transform(t);
            }
            plot = plot.scale_x_continuous(s);
        }
    }
    if let Some(f) = facet {
        // ggplot-rs (still 0.17) draws free y-axis labels only on the
        // left-most panels, so free facets stack in one column unless
        // ::FACET_NCOL says otherwise. TODO(ggplot-rs): drop the default once
        // every free panel carries its own axis labels.
        let ncol = role_num(cols, Role::FacetCols)
            .filter(|n| *n >= 1.0)
            .map(|n| n.min(64.0) as usize)
            .or((f.role == Role::FacetFree).then_some(1));
        plot = if f.role == Role::FacetFree {
            // A discrete x is the same set of levels in every panel: free y only.
            let scales = if x_discrete {
                FacetScales::FreeY
            } else {
                FacetScales::Free
            };
            plot.facet_wrap_free("facet", ncol, scales)
        } else {
            plot.facet_wrap("facet", ncol)
        };
    }
    // `::FLIP` swaps the axes — e.g. a horizontal bar chart.
    if flipped {
        plot = plot.coord_flip();
    }
    // Scroll/drag-zoom window (from the UI) — clip a continuous cartesian panel to
    // the given data rectangle. The UI only sets it for continuous/datetime x.
    if !flipped && facet.is_none() {
        if let Some((xlim, ylim)) = o.zoom {
            plot = plot.coord_cartesian_zoom(Some(xlim), Some(ylim));
        }
    }
    plot = plot.house_theme(o);
    plot = plot.legend_position(ggplot_rs::theme::LegendPosition::Top);
    if let Some(t) = &title {
        plot = plot.title(t);
    }
    let mut panel = Panel::plot(plot, width, height).with_margin(chart_theme(o, 11.0).plot_margin);
    // A dodged discrete x is drawn on numeric slots: tell hosts which level
    // each slot is, so a tooltip header reads "W1" rather than "1.225".
    if let (Some(d), Panel::Plot { attrs, .. }) = (&dodged, &mut panel) {
        let ticks: serde_json::Map<String, serde_json::Value> = d
            .breaks
            .iter()
            .zip(&d.labels)
            .map(|(b, l)| (format!("{b}"), serde_json::Value::String(l.clone())))
            .collect();
        attrs.push(("data-xticks", serde_json::Value::Object(ticks).to_string()));
    }
    Ok(panel)
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
        .geom_histogram_with(GeomHistogram {
            fill: o.brand(),
            color: o.theme.colors.card,
            ..Default::default()
        })
        .house_theme(o)
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
            .scale_fill(dz_scale(o, Aesthetic::Fill, cat))
            .scale_color(dz_scale(o, Aesthetic::Color, cat))
            .house_theme(o)
            .legend_position(ggplot_rs::theme::LegendPosition::Top);
    } else {
        plot = GGPlot::new(data)
            .aes(aes)
            .geom_density()
            .house_theme(o)
            .primary_color(o.brand());
    }
    if let Some(t) = title {
        plot = plot.title(t);
    }
    Ok(Panel::plot(plot, width, height).with_margin(chart_theme(o, 11.0).plot_margin))
}

/// Build an axis tick formatter from a short spec — a keyword or a currency
/// symbol — mirroring ggplot2's `scales::` label helpers. Numbers are always
/// thousands-grouped. A leading space marks a suffix ("` kg`" → "12 kg");
/// otherwise the spec is a prefix ("€" → "€1,200", "CHF " → "CHF 1,200").
///
/// With a dashboard locale (`::LOCALE 'de-DE'`) the numbers take its marks
/// and a currency keyword its placement (`1.200 €`); en-US is unchanged.
fn axis_formatter(
    spec: &str,
    locale: &format::Locale,
) -> Option<Box<dyn Fn(f64) -> String + Send + Sync>> {
    if !locale.is_default() {
        return axis_formatter_locale(spec, locale);
    }
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

/// [`axis_formatter`] in a non-default locale (ggplot-rs `label_*_locale`).
fn axis_formatter_locale(
    spec: &str,
    locale: &format::Locale,
) -> Option<Box<dyn Fn(f64) -> String + Send + Sync>> {
    use ggplot_rs::scale::format::{label_comma_locale, label_currency_locale};
    let s = spec.trim_end_matches(';');
    let low = s.trim().to_ascii_lowercase();
    if low.is_empty() {
        return None;
    }
    let gg = locale.gg();
    let currency = |sym: &str| -> Box<dyn Fn(f64) -> String + Send + Sync> {
        Box::new(label_currency_locale(
            &gg.clone().with_currency(sym),
            Some(1.0),
        ))
    };
    let comma = label_comma_locale(&gg);
    let f: Box<dyn Fn(f64) -> String + Send + Sync> = match low.as_str() {
        "$" | "usd" | "dollar" | "dollars" => currency("$"),
        "€" | "eur" | "euro" | "euros" => currency("€"),
        "£" | "gbp" | "pound" | "pounds" => currency("£"),
        "¥" | "jpy" | "yen" | "cny" | "yuan" => currency("¥"),
        "%" | "percent" | "pct" => {
            let suffix = gg.percent_suffix.clone();
            Box::new(move |v: f64| format!("{}{suffix}", comma(v)))
        }
        "," | "comma" | "thousands" | "number" => Box::new(comma),
        _ => {
            if let Some(suffix) = s.strip_prefix(' ') {
                let suffix = suffix.to_string();
                Box::new(move |v: f64| format!("{} {}", comma(v), suffix))
            } else {
                let prefix = s.to_string();
                Box::new(move |v: f64| {
                    if v < 0.0 {
                        format!("-{}{}", prefix, comma(-v))
                    } else {
                        format!("{}{}", prefix, comma(v))
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
    // Points on a 95 % pointwise envelope around the quartile line (qqplotr).
    let mut plot = GGPlot::new(data)
        .aes(Aes::new().y("y"))
        .geom_qq_band_with(
            GeomQQBand {
                fill: o.brand(),
                alpha: 0.18,
            },
            StatQQBand::default(),
        )
        .geom_qq()
        .geom_qq_line()
        .xlab("Theoretical")
        .ylab("Sample")
        .house_theme(o)
        .primary_color(o.brand());
    if let Some(t) = title {
        plot = plot.title(t);
    }
    Ok(Panel::plot(plot, width, height))
}

/// A heatmap: `x` × `y` tiles coloured by the measure.
///
/// Cells part with a thin card-coloured gap; the fill is the theme's
/// sequential ramp (pale → brand → deep brand; dark-mode aware), or its
/// diverging ramp around zero when the values span it — or, with a
/// `::COLORSCALE…` / `::THRESHOLDS` column, the status colours ([`TileFill`]). Discrete axes keep
/// the SQL row order (first level at the top, like a table). `::DATALABELS`
/// writes each value into its cell (dark text on light cells, light text on
/// dark ones); `::YFORMAT` formats the values (labels, legend, tooltip).
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
    let fmt = value_fmt(cols, Kind::Heatmap, &value.values);
    let nums: Vec<f64> = value
        .values
        .iter()
        .filter_map(|v| v.as_f64())
        .filter(|f| f.is_finite())
        .collect();
    let (min, max) = nums
        .iter()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), &v| {
            (a.min(v), b.max(v))
        });
    let fill = TileFill::of(o, cols, o.theme.seq_low(), min, max)?;
    let tips: Vec<Value> = value
        .values
        .iter()
        .map(|v| match v.as_f64() {
            Some(f) => Value::Str(fmt.full(f, &o.locale)),
            None => Value::Na,
        })
        .collect();
    let data = vec![
        ("x".to_string(), x.values.clone()),
        ("y".to_string(), y.values.clone()),
        ("fill".to_string(), value.values.clone()),
        ("label".to_string(), tips),
    ];
    let mut th = chart_theme(o, 11.0);
    // Tiles fill the panel: no grid behind them; a slim, borderless colourbar.
    th.panel_grid_major = ggplot_rs::theme::ElementLine::blank();
    th.legend_key.color = Some(o.theme.colors.card);
    th.legend_key_width = 9.0;
    let plot = GGPlot::new(data.clone())
        .aes(Aes::new().x("x").y("y").fill("fill").label("label"))
        .geom_tile_with(GeomTile {
            color: o.theme.colors.card,
            line_width: 2.0,
            ..Default::default()
        });
    let mut plot = fill.scale(plot, o, &fmt).theme(th);
    if let Some(levels) = str_levels(&x.values) {
        plot = plot.scale_x_discrete(
            ScaleDiscrete::new().with_limits(levels.iter().map(String::as_str).collect()),
        );
    }
    if let Some(mut levels) = str_levels(&y.values) {
        levels.reverse();
        plot = plot.scale_y_discrete(
            ScaleDiscrete::new().with_limits(levels.iter().map(String::as_str).collect()),
        );
    }
    if let Some(dl) = find_role(cols, Role::DataLabels) {
        let short: Vec<String> = value
            .values
            .iter()
            .map(|v| {
                v.as_f64()
                    .map(|f| fmt.short(f, &o.locale))
                    .unwrap_or_default()
            })
            .collect();
        let size = dl
            .values
            .iter()
            .find_map(|v| v.as_f64())
            .filter(|s| (5.0..=40.0).contains(s))
            .unwrap_or_else(|| {
                let nx = distinct_labels_of(&x.values).max(1) as f64;
                let ny = distinct_labels_of(&y.values).max(1) as f64;
                let chars = short.iter().map(|s| s.chars().count()).max().unwrap_or(1) as f64;
                let cw = width as f64 * 0.8 / nx;
                let ch = height as f64 * 0.85 / ny;
                (cw / (chars * 0.62 + 1.2)).min(ch * 0.4).clamp(7.0, 12.0)
            });
        // One text layer per label colour (ggplot text has one colour).
        let mut by_ink: Vec<((u8, u8, u8), Vec<usize>)> = Vec::new();
        for (i, v) in value.values.iter().enumerate() {
            let Some(f) = v.as_f64().filter(|f| f.is_finite()) else {
                continue;
            };
            let ink = o.theme.text_on(fill.color_of(o, f));
            match by_ink.iter_mut().find(|(c, _)| *c == ink) {
                Some((_, rows)) => rows.push(i),
                None => by_ink.push((ink, vec![i])),
            }
        }
        for (ink, rows) in by_ink {
            let pick =
                |vals: &[Value]| -> Vec<Value> { rows.iter().map(|&i| vals[i].clone()).collect() };
            plot = plot
                .geom_text_with(GeomText {
                    size,
                    color: ink,
                    ..Default::default()
                })
                .layer_data(vec![
                    ("x".to_string(), pick(&x.values)),
                    ("y".to_string(), pick(&y.values)),
                    (
                        "cell".to_string(),
                        rows.iter().map(|&i| Value::Str(short[i].clone())).collect(),
                    ),
                ])
                .layer_aes(Aes::new().x("x").y("y").label("cell"))
                .show_legend(false);
        }
    }
    if let Some(t) = title {
        plot = plot.title(t);
    }
    let mut panel = Panel::plot(plot, width, height);
    // Hosts: cell labels let hover/click through to the cell; no legend toggle.
    if let Panel::Plot { attrs, .. } = &mut panel {
        attrs.push(("data-kind", "heatmap".to_string()));
    }
    Ok(panel)
}

/// The value format of a panel: its `::YFORMAT`, or a share for the
/// percent kinds (whose values are fractions).
pub(crate) fn value_fmt(cols: &[Column], kind: Kind, values: &[Value]) -> format::ValueFmt {
    let fractions = || {
        values
            .iter()
            .filter_map(|v| v.as_f64())
            .all(|f| f.abs() <= 1.0)
    };
    match kind {
        Kind::BarPercent | Kind::BarStackedPercent => return format::ValueFmt::Share,
        Kind::LinePercent if fractions() => return format::ValueFmt::Share,
        Kind::LinePercent => return format::ValueFmt::Percent,
        _ => {}
    }
    role_fmt(cols, Role::YFormat)
        .map(|s| format::ValueFmt::parse(&s))
        .unwrap_or(format::ValueFmt::Plain)
}

/// The first-seen order of a text column's levels (`None` when the column
/// holds numbers, which keep a continuous axis).
fn str_levels(values: &[Value]) -> Option<Vec<String>> {
    let mut out: Vec<String> = Vec::new();
    for v in values {
        match v {
            Value::Str(s) => {
                if !out.contains(s) {
                    out.push(s.clone());
                }
            }
            Value::Na => {}
            _ => return None,
        }
    }
    (!out.is_empty()).then_some(out)
}

/// The number of distinct (non-missing) values.
fn distinct_labels_of(values: &[Value]) -> usize {
    let mut seen: Vec<String> = Vec::new();
    for v in values {
        if matches!(v, Value::Na) {
            continue;
        }
        let k = value_str(v);
        if !seen.contains(&k) {
            seen.push(k);
        }
    }
    seen.len()
}

/// The fill ramp of a heatmap / calendar over the values `[min, max]`:
/// gradient stops at 0..1. Sequential: `low` → brand → a deeper brand (one
/// hue, light → dark; in the dark variant the deep end is lighter). Values
/// spanning zero: the theme's diverging ramp with a neutral midpoint at 0,
/// both arms scaled to the larger |end| (−2…+10 reads as a little red, a lot
/// of blue). An explicit theme `sequential` pair is used as given.
pub(crate) struct Ramp {
    min: f64,
    max: f64,
    stops: Vec<(f64, (u8, u8, u8))>,
}

impl Ramp {
    pub(crate) fn new(o: &RenderOptions, low: (u8, u8, u8), min: f64, max: f64) -> Ramp {
        let t = &o.theme;
        let stops = if min < 0.0 && max > 0.0 {
            let [lo, mid, hi] = t.diverging_ramp();
            let m = (-min).max(max);
            vec![
                (0.0, theme::mix(mid, lo, -min / m)),
                (-min / (max - min), mid),
                (1.0, theme::mix(mid, hi, max / m)),
            ]
        } else if let Some([_, high]) = t.seq_pair() {
            vec![(0.0, low), (1.0, high)]
        } else {
            let brand = o.brand();
            vec![
                (0.0, low),
                (0.72, brand),
                (1.0, theme::mix(brand, t.colors.ink, 0.32)),
            ]
        };
        Ramp { min, max, stops }
    }

    /// The colour a value gets (as the gradient scale maps it).
    pub(crate) fn color_of(&self, v: f64) -> (u8, u8, u8) {
        let span = self.max - self.min;
        let p = if span > 0.0 {
            ((v - self.min) / span).clamp(0.0, 1.0)
        } else {
            0.5
        };
        self.color_at(p)
    }

    /// The colour at position `p` (0..1) along the ramp.
    pub(crate) fn color_at(&self, p: f64) -> (u8, u8, u8) {
        let p = p.clamp(0.0, 1.0);
        let i = self
            .stops
            .windows(2)
            .position(|w| p <= w[1].0)
            .unwrap_or(self.stops.len().saturating_sub(2));
        let ((a, ca), (b, cb)) = (self.stops[i], self.stops[(i + 1).min(self.stops.len() - 1)]);
        let f = if b > a { (p - a) / (b - a) } else { 0.0 };
        theme::mix(ca, cb, f)
    }
}

impl Ramp {
    /// A status ramp over `[min, max]` in the theme's status tile colours
    /// ([`theme::ResolvedTheme::status_tiles`]): `::COLORSCALE` runs good →
    /// warn → bad from low to high, `::COLORSCALE_HIGH_GOOD` the other way,
    /// `::COLORSCALE_DIVERGING` from bad (negative) through the neutral
    /// midpoint at 0 to good (positive), both arms scaled to the larger |end|.
    pub(crate) fn status(o: &RenderOptions, scale: Scale, min: f64, max: f64) -> Ramp {
        let t = &o.theme;
        let [good, warn, bad] = t.status_tiles();
        let stops = match scale {
            Scale::LowGood => vec![(0.0, good), (0.5, warn), (1.0, bad)],
            Scale::HighGood => vec![(0.0, bad), (0.5, warn), (1.0, good)],
            Scale::Diverging => {
                let mid = t.diverging_ramp()[1];
                let [neg, pos] = t.status_arms();
                let m = min.abs().max(max.abs());
                let at = |v: f64| match v {
                    _ if m <= 0.0 || !v.is_finite() => mid,
                    v if v < 0.0 => theme::mix(mid, neg, -v / m),
                    v => theme::mix(mid, pos, v / m),
                };
                let span = max - min;
                if min < 0.0 && max > 0.0 && span > 0.0 {
                    vec![(0.0, at(min)), (-min / span, mid), (1.0, at(max))]
                } else {
                    vec![(0.0, at(min)), (1.0, at(max))]
                }
            }
        };
        Ramp { min, max, stops }
    }
}

/// How a heatmap / calendar / track colours its tiles: the theme's
/// sequential ramp (default), a status ramp good → warn → bad (a
/// `::COLORSCALE…` column, [`Ramp::status`]), or one status colour per
/// `::THRESHOLDS` band with a stepped legend naming the bands.
pub(crate) enum TileFill {
    Ramp(Ramp),
    Steps(Vec<thresholds::Band>),
}

impl TileFill {
    /// The panel's tile fill over the values `[min, max]`; `low` is the
    /// default ramp's pale end. `::THRESHOLDS` win over a `::COLORSCALE`.
    pub(crate) fn of(
        o: &RenderOptions,
        cols: &[Column],
        low: (u8, u8, u8),
        min: f64,
        max: f64,
    ) -> Result<TileFill, String> {
        if let Some(rules) = thresholds_of(cols)? {
            return Ok(TileFill::Steps(rules.bands()));
        }
        let scale = cols.iter().find_map(|c| match c.role {
            Role::ColorScale(s) => Some(s),
            _ => None,
        });
        Ok(TileFill::Ramp(match scale {
            Some(s) => Ramp::status(o, s, min, max),
            None => Ramp::new(o, low, min, max),
        }))
    }

    /// The band holding `v` (stepped fills).
    fn band(&self, v: f64) -> Option<&thresholds::Band> {
        match self {
            TileFill::Steps(bands) => bands.iter().find(|b| b.contains(v)),
            TileFill::Ramp(_) => None,
        }
    }

    /// The colour a value's tile gets (data-label ink is chosen against it).
    pub(crate) fn color_of(&self, o: &RenderOptions, v: f64) -> (u8, u8, u8) {
        match self {
            TileFill::Ramp(r) => r.color_of(v),
            TileFill::Steps(_) => self
                .band(v)
                .map_or(thresholds::Status::Neutral, |b| b.status)
                .tile(&o.theme),
        }
    }

    /// Add the fill scale: the formatted colourbar, or a stepped legend of
    /// the bands high to low (`≥ 50`, `20–50`, `< 20` — like a colourbar,
    /// high on top) in their status colours. The tiles keep their values (a
    /// tooltip reads the value, not its band).
    pub(crate) fn scale(&self, plot: GGPlot, o: &RenderOptions, fmt: &format::ValueFmt) -> GGPlot {
        match self {
            TileFill::Ramp(r) => plot.scale_fill(FmtGradient::new(r, fmt.clone(), o.locale)),
            TileFill::Steps(bands) => plot.scale_fill(StepFill {
                bands: bands
                    .iter()
                    .rev()
                    .map(|b| (b.clone(), band_label(o, b, fmt), b.status.tile(&o.theme)))
                    .collect(),
                name: String::new(),
            }),
        }
    }
}

/// The stepped fill scale of `::THRESHOLDS` tiles: a number takes its band's
/// status colour; the legend lists the bands by label (discrete keys).
#[derive(Clone)]
struct StepFill {
    bands: Vec<(thresholds::Band, String, (u8, u8, u8))>,
    name: String,
}

impl ggplot_rs::scale::Scale for StepFill {
    fn aesthetic(&self) -> Aesthetic {
        Aesthetic::Fill
    }
    fn train(&mut self, _values: &[Value]) {}
    fn map(&self, value: &Value) -> f64 {
        let i = match value {
            Value::Str(s) => self.bands.iter().position(|(_, l, _)| l == s),
            v => v
                .as_f64()
                .and_then(|f| self.bands.iter().position(|(b, _, _)| b.contains(f))),
        };
        i.map_or(f64::NAN, |i| i as f64)
    }
    fn breaks(&self) -> Vec<(f64, String)> {
        self.bands
            .iter()
            .enumerate()
            .map(|(i, (_, l, _))| (i as f64, l.clone()))
            .collect()
    }
    fn is_discrete(&self) -> bool {
        true
    }
    fn map_to_color(&self, value: &Value) -> Option<(u8, u8, u8)> {
        let i = self.map(value);
        (i.is_finite()).then(|| self.bands[i as usize].2)
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn set_name(&mut self, name: &str) {
        self.name = name.to_string();
    }
    fn clone_box(&self) -> Box<dyn ggplot_rs::scale::Scale> {
        Box::new(self.clone())
    }
}

/// A `::THRESHOLDS` band as the legend names it, in the panel's format.
fn band_label(o: &RenderOptions, b: &thresholds::Band, fmt: &format::ValueFmt) -> String {
    b.label(|v| fmt.short(v, &o.locale))
}

/// A continuous fill scale over a [`Ramp`] whose colourbar labels use the
/// panel's value format (`$1.2M`, `12 %`).
#[derive(Clone)]
pub(crate) struct FmtGradient {
    inner: ggplot_rs::scale::gradient_n::ScaleColorGradientN,
    fmt: format::ValueFmt,
    locale: format::Locale,
}

impl FmtGradient {
    pub(crate) fn new(ramp: &Ramp, fmt: format::ValueFmt, locale: format::Locale) -> FmtGradient {
        FmtGradient {
            inner: ggplot_rs::scale::gradient_n::ScaleColorGradientN::new(
                Aesthetic::Fill,
                ramp.stops.iter().map(|&(p, c)| (p, rgba(c))).collect(),
            ),
            fmt,
            locale,
        }
    }
}

impl ggplot_rs::scale::Scale for FmtGradient {
    fn aesthetic(&self) -> Aesthetic {
        self.inner.aesthetic()
    }
    fn train(&mut self, values: &[Value]) {
        self.inner.train(values)
    }
    fn map(&self, value: &Value) -> f64 {
        self.inner.map(value)
    }
    fn breaks(&self) -> Vec<(f64, String)> {
        let (lo, hi) = self.inner.domain().unwrap_or((0.0, 1.0));
        self.inner
            .breaks()
            .into_iter()
            .map(|(p, l)| {
                let v = lo + p * (hi - lo);
                if hi > lo {
                    (p, self.fmt.short(v, &self.locale))
                } else {
                    (p, l)
                }
            })
            .collect()
    }
    fn name(&self) -> &str {
        self.inner.name()
    }
    fn set_name(&mut self, name: &str) {
        self.inner.set_name(name)
    }
    fn map_to_color(&self, value: &Value) -> Option<(u8, u8, u8)> {
        self.inner.map_to_color(value)
    }
    fn domain(&self) -> Option<(f64, f64)> {
        self.inner.domain()
    }
    fn clone_box(&self) -> Box<dyn ggplot_rs::scale::Scale> {
        Box::new(self.clone())
    }
    fn reset_training(&mut self) {
        self.inner.reset_training()
    }
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
    let fmt = value_fmt(cols, Kind::Calendar, &vals);
    let labels: Vec<Value> = vals
        .iter()
        .map(|v| {
            Value::Str(
                v.as_f64()
                    .map(|f| fmt.full(f, &o.locale))
                    .unwrap_or_default(),
            )
        })
        .collect();
    let (min, max) = vals
        .iter()
        .filter_map(|v| v.as_f64())
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), v| {
            (a.min(v), b.max(v))
        });
    let fill = TileFill::of(o, cols, o.theme.calendar_low(), min, max)?;
    let data = vec![
        ("x".to_string(), dates),
        ("fill".to_string(), vals),
        ("label".to_string(), labels),
    ];
    let mut th = chart_theme(o, 11.0);
    th.panel_grid_major = ggplot_rs::theme::ElementLine::blank();
    th.legend_key.color = Some(o.theme.colors.card);
    th.legend_key_width = 9.0;
    // Months read as blocks: the month boundary is a gutter in the card colour
    // (light and dark), about a fifth of a tile wide — 2–5 px, so a multi-year
    // calendar with small tiles keeps its tiles.
    let weeks = ((max_day - min_day) as f64 / 7.0).ceil().max(1.0) + 1.0;
    let tile = (width as f64 - 110.0).max(60.0) / weeks;
    th.calendar_month_border = ggplot_rs::theme::ElementLine {
        color: o.theme.colors.card,
        width: (tile * 0.22).clamp(2.0, 5.0),
        ..Default::default()
    };
    let plot = GGPlot::new(data)
        .aes(Aes::new().x("x").fill("fill").label("label"))
        .geom_calendar_with(
            GeomTile {
                color: o.theme.colors.card,
                line_width: 1.5,
                ..Default::default()
            },
            false,
        );
    let mut plot = fill.scale(plot, o, &fmt).xlab("").ylab("").theme(th);
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
    o: &RenderOptions,
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
        .house_theme(o);
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
        .house_theme(o)
        .legend_position(ggplot_rs::theme::LegendPosition::Top);
    if let Some(cat) = category {
        plot = plot
            .scale_color(dz_scale(o, Aesthetic::Color, cat))
            .scale_fill(dz_scale(o, Aesthetic::Fill, cat));
    }
    if let Some(t) = title {
        plot = plot.title(t);
    }
    Ok(Panel::plot(plot, width, height).with_margin(chart_theme(o, 11.0).plot_margin))
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
    let fill = wash(o, steel, 0.62); // wash under the line
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
            color: o.theme.color(2),
            alpha: 1.0,
        })
        .layer_data(end) // restrict the point layer to just the endpoint
        .scale_y_continuous(
            ggplot_rs::scale::continuous::ScaleContinuous::new().with_expand(0.1, 0.0),
        )
        .house_void(o);
    // Render at a compact width so strokes stay crisp when the inline
    // sparkline is scaled down into a narrow panel column.
    Ok(Panel::plot(plot, width.min(240), height))
}

/// Blend a colour toward black by `t` (0 = unchanged, 1 = black).
pub(crate) fn darken((r, g, b): (u8, u8, u8), t: f64) -> (u8, u8, u8) {
    let f = |c: u8| (c as f64 * (1.0 - t)).round() as u8;
    (f(r), f(g), f(b))
}

/// Blend a colour toward white by `t` (0 = unchanged, 1 = white).
pub(crate) fn lighten((r, g, b): (u8, u8, u8), t: f64) -> (u8, u8, u8) {
    let f = |c: u8| (c as f64 + (255.0 - c as f64) * t).round() as u8;
    (f(r), f(g), f(b))
}

/// Annotations driven by data rows (#31): a label on the `::XLINE` /
/// `::REFLINE` of the same row (`::XLINE_LABEL`, `::REFLINE_LABEL`), labelled
/// shaded x-ranges (`::XBAND_FROM` + `::XBAND_TO` [+ `::XBAND_LABEL`]) and
/// text callouts at a row's (x, y) (`::ANNOTATE`). Events usually come from a
/// `LEFT JOIN events ON events.day = t.day`, so only the event rows carry
/// values; NULL / empty cells draw nothing. Ink and shading come from the
/// theme (`muted` wash, chart label colour), so dark mode follows.
fn data_annotations(
    mut plot: GGPlot,
    o: &RenderOptions,
    kind: Kind,
    cols: &[Column],
    data: &[(String, Vec<Value>)],
    x_discrete: bool,
) -> GGPlot {
    const MAX_NOTES: usize = 64;
    let text_at = |c: Option<&Column>, i: usize| -> Option<String> {
        c.and_then(|c| c.values.get(i))
            .filter(|v| **v != Value::Na)
            .map(|v| value_str(v).trim().to_string())
            .filter(|t| !t.is_empty())
    };
    let finite = |v: &Value| v.as_f64().is_some_and(f64::is_finite);
    let num = |v: Vec<f64>| v.into_iter().map(Value::Float).collect::<Vec<_>>();
    let text = |v: Vec<String>| v.into_iter().map(Value::Str).collect::<Vec<_>>();
    let ink = o.theme.chart_label();
    let top = annotation_top(kind, cols, data);
    // Text anchored at its bottom-left, just right of an x position, in the
    // headroom above the data (the y scale's expansion).
    let top_labels = |plot: GGPlot, xs: Vec<Value>, labels: Vec<String>| -> GGPlot {
        let Some(y) = top.filter(|_| !xs.is_empty()) else {
            return plot;
        };
        let k = xs.len();
        plot.geom_text_with(GeomText {
            size: 9.5,
            color: ink,
            hjust: 0.0,
            vjust: 0.0,
            check_overlap: true,
            ..Default::default()
        })
        .layer_data(vec![
            ("x".to_string(), xs),
            ("y".to_string(), num(vec![y; k])),
            ("label".to_string(), text(labels)),
        ])
        .layer_aes(Aes::new().x("x").y("y").label("label"))
    };

    // Labelled shaded ranges, one per distinct (from, to, label) row.
    if let (false, Some(from), Some(to)) = (
        x_discrete,
        find_role(cols, Role::XBandFrom),
        find_role(cols, Role::XBandTo),
    ) {
        let lab = find_role(cols, Role::XBandLabel);
        let mut seen = std::collections::HashSet::new();
        let (mut x0, mut x1, mut lx, mut ls) = (vec![], vec![], vec![], vec![]);
        for (i, (a, b)) in from.values.iter().zip(&to.values).enumerate() {
            if !finite(a) || !finite(b) || x0.len() >= MAX_NOTES {
                continue;
            }
            let (a, b) = if a.as_f64() <= b.as_f64() {
                (a, b)
            } else {
                (b, a)
            };
            let label = text_at(lab, i);
            if !seen.insert((value_str(a), value_str(b), label.clone())) {
                continue;
            }
            x0.push(a.clone());
            x1.push(b.clone());
            if let Some(l) = label {
                lx.push(a.clone());
                ls.push(format!("\u{a0}{l}"));
            }
        }
        if !x0.is_empty() {
            let k = x0.len();
            let shade = o.theme.colors.muted;
            plot = plot
                .geom_rect_with(GeomRect {
                    fill: shade,
                    color: shade,
                    alpha: 0.13,
                    line_width: 0.0,
                })
                .layer_data(vec![
                    ("bx0".to_string(), x0),
                    ("bx1".to_string(), x1),
                    ("by0".to_string(), num(vec![f64::NEG_INFINITY; k])),
                    ("by1".to_string(), num(vec![f64::INFINITY; k])),
                ])
                .layer_aes(Aes::new().xmin("bx0").xmax("bx1").ymin("by0").ymax("by1"));
            plot = top_labels(plot, lx, ls);
        }
    }

    // Event labels at the top of their ::XLINE.
    if let (false, Some(line), Some(lab)) = (
        x_discrete,
        find_role(cols, Role::VLine),
        find_role(cols, Role::VLineLabel),
    ) {
        let mut seen = std::collections::HashSet::new();
        let (mut xs, mut ls) = (vec![], vec![]);
        for (i, v) in line.values.iter().enumerate() {
            let Some(l) = text_at(Some(lab), i).filter(|_| finite(v)) else {
                continue;
            };
            if xs.len() < MAX_NOTES && seen.insert((value_str(v), l.clone())) {
                xs.push(v.clone());
                ls.push(format!("\u{a0}{l}"));
            }
        }
        plot = top_labels(plot, xs, ls);
    }

    // Reference-line labels above the line's right end.
    if let (Some(line), Some(lab)) = (
        find_role(cols, Role::RefLine),
        find_role(cols, Role::RefLineLabel),
    ) {
        let xcol = data.iter().find(|(k, _)| k == "x").map(|(_, v)| v);
        let right = xcol.and_then(|xs| {
            if x_discrete {
                xs.iter().rev().find(|v| **v != Value::Na).cloned()
            } else {
                xs.iter()
                    .filter(|v| finite(v))
                    .max_by(|a, b| a.as_f64().partial_cmp(&b.as_f64()).unwrap())
                    .cloned()
            }
        });
        let mut seen = std::collections::HashSet::new();
        let (mut ys, mut ls) = (vec![], vec![]);
        for (i, v) in line.values.iter().enumerate() {
            let Some(y) = v.as_f64().filter(|f| f.is_finite()) else {
                continue;
            };
            if let Some(l) = text_at(Some(lab), i) {
                if ys.len() < MAX_NOTES && seen.insert((y.to_bits(), l.clone())) {
                    ys.push(y);
                    ls.push(l);
                }
            }
        }
        if let (Some(rx), false) = (right, ys.is_empty()) {
            let k = ys.len();
            plot = plot
                .geom_text_with(GeomText {
                    size: 9.5,
                    color: ink,
                    hjust: 1.0,
                    vjust: -0.25,
                    ..Default::default()
                })
                .layer_data(vec![
                    ("x".to_string(), vec![rx; k]),
                    ("y".to_string(), num(ys)),
                    ("label".to_string(), text(ls)),
                ])
                .layer_aes(Aes::new().x("x").y("y").label("label"));
        }
    }

    // Point callouts: the annotated mark gets a ring and a repelled label.
    if let Some(note) = find_role(cols, Role::Annotate) {
        let xs = data.iter().find(|(k, _)| k == "x").map(|(_, v)| v);
        let ys = data.iter().find(|(k, _)| k == "y").map(|(_, v)| v);
        if let (Some(xs), Some(ys)) = (xs, ys) {
            let idx: Vec<usize> = (0..xs.len().min(ys.len()))
                .filter(|&i| xs[i] != Value::Na && finite(&ys[i]))
                .filter(|&i| text_at(Some(note), i).is_some())
                .take(MAX_NOTES)
                .collect();
            if !idx.is_empty() {
                let mut frame = subset(data, &idx);
                frame.retain(|(n, _)| matches!(n.as_str(), "x" | "y" | "facet"));
                frame.push((
                    "txt".to_string(),
                    text(idx.iter().filter_map(|&i| text_at(Some(note), i)).collect()),
                ));
                plot = plot
                    .geom_point_with(GeomPoint {
                        color: ink,
                        size: 3.2,
                        ..Default::default()
                    })
                    .layer_data(frame.clone())
                    .layer_aes(Aes::new().x("x").y("y"))
                    .geom_text_repel_with(GeomTextRepel {
                        size: 10.0,
                        color: ink,
                        fontface: FontFace::Bold,
                        repel: RepelParams {
                            segment_color: Some(o.theme.chart_text()),
                            ..Default::default()
                        },
                        ..Default::default()
                    })
                    .layer_data(frame)
                    .layer_aes(Aes::new().x("x").y("y").label("txt"));
            }
        }
    }
    plot
}

/// The top of a chart's data in y units — where event / range labels sit
/// (above it, in the scale's headroom). Stacked marks stack per x (and per
/// `::FACET` panel).
fn annotation_top(kind: Kind, cols: &[Column], data: &[(String, Vec<Value>)]) -> Option<f64> {
    let col = |n: &str| data.iter().find(|(k, _)| k == n).map(|(_, v)| v);
    let max = |it: &mut dyn Iterator<Item = f64>| it.filter(|f| f.is_finite()).reduce(f64::max);
    match kind {
        Kind::BarStackedPercent => return Some(1.0),
        Kind::BarStacked | Kind::AreaStacked => {
            let (x, y) = (col("x")?, col("y")?);
            let facet = col("facet");
            let mut sums: std::collections::HashMap<(String, String), f64> = Default::default();
            for (i, (xv, yv)) in x.iter().zip(y).enumerate() {
                if let Some(f) = yv.as_f64().filter(|f| f.is_finite() && *f > 0.0) {
                    let panel = facet.map(|c| value_str(&c[i])).unwrap_or_default();
                    *sums.entry((panel, value_str(xv))).or_default() += f;
                }
            }
            return max(&mut sums.into_values());
        }
        _ => {}
    }
    let ycols = data
        .iter()
        .filter(|(k, _)| {
            k == "y" || k == "ihi" || (k.starts_with('y') && k[1..].parse::<u32>().is_ok())
        })
        .flat_map(|(_, v)| v.iter());
    let extra = cols
        .iter()
        .filter(|c| matches!(c.role, Role::RefLine | Role::BandUpper))
        .flat_map(|c| c.values.iter());
    max(&mut ycols.chain(extra).filter_map(Value::as_f64))
}

/// An `::XLINE`'s positions (a DATE arrives as epoch seconds, text as is): numbers as they
/// are, a date / timestamp (`DATE '2026-06-30'::XLINE`, "Today" on a date
/// axis) as its epoch seconds — the unit a date x is drawn in.
fn xline_nums(vals: &[Value]) -> Vec<f64> {
    let nums = distinct_nums(vals);
    if !nums.is_empty() {
        return nums;
    }
    let parsed: Vec<Value> = vals
        .iter()
        .filter_map(|v| {
            let s = v.as_str()?.trim();
            s.parse::<f64>()
                .ok()
                .or_else(|| sql::parse_iso_epoch(s).map(|t| t as f64))
        })
        .map(Value::Float)
        .collect();
    distinct_nums(&parsed)
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

/// A `::SIZE` column holding one number on every row: that point size
/// (clamped to 1–12; ggplot's default is 3), else `None` (a measure).
fn constant_size(col: &Column) -> Option<f64> {
    let first = col.values.first()?.as_f64().filter(|f| f.is_finite())?;
    col.values
        .iter()
        .all(|v| v.as_f64() == Some(first))
        .then_some(first.clamp(1.0, 12.0))
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
    // `::THRESHOLDS` zones (status colours over the range) replace the equal
    // `::COLORS` zones; the zone holding the value is drawn at full strength.
    let rules = thresholds_of(cols)?;
    let status_zones = rules
        .as_ref()
        .map(|r| r.zones(min, max))
        .unwrap_or_default();
    let value_status = rules.as_ref().and_then(|r| r.status(val));
    let n_zones = if !status_zones.is_empty() {
        0 // ::LABELS name equal zones only
    } else if zone_cols.is_empty() {
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
    if !status_zones.is_empty() {
        for (i, (a, b, st)) in status_zones.iter().enumerate() {
            x0.push(*a);
            x1.push(*b);
            fill.push(format!("z{i}"));
            let on = (*a..=*b).contains(&shown);
            let c = st.color(&o.theme);
            pairs.push((format!("z{i}"), if on { c } else { wash(o, c, 0.6) }));
        }
    } else if zone_cols.is_empty() {
        x0.extend([min, min]);
        x1.extend([max, shown]);
        fill.extend(["track".to_string(), "value".to_string()]);
        pairs.push(("track".to_string(), wash(o, o.brand(), 0.87)));
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
    if zone_cols.is_empty() && status_zones.is_empty() && n_zones > 1 {
        let xs: Vec<f64> = (1..n_zones)
            .map(|i| at(i as f64 / n_zones as f64))
            .collect();
        let k = xs.len();
        plot = plot
            .geom_segment_with(GeomSegment {
                color: o.theme.colors.card,
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
            color: o.theme.colors.ink,
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
        color: o.theme.chart_text(),
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
        color: value_status.map_or(o.theme.colors.ink, |st| st.color(&o.theme)),
    });
    let caption = GeomText {
        size: 11.0,
        color: o.theme.chart_text(),
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
        .house_void(o)
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

/// A cell as a number: numeric values, or text that parses as one (DuckDB's
/// JSON writes DECIMALs as strings, e.g. `"1.0"`).
fn num_str(s: &str) -> Option<f64> {
    s.trim().parse::<f64>().ok().filter(|f| f.is_finite())
}

fn cell_num(v: &Value) -> Option<f64> {
    match v {
        Value::Str(s) => num_str(s),
        v => v.as_f64().filter(|f| f.is_finite()),
    }
}

/// A pie/donut: one stacked bar (x constant) wrapped into polar coords, sliced
/// and coloured by `CATEGORY`.
fn render_pie(
    o: &RenderOptions,
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
        // full-width slices + no discrete padding: a solid pie (a donut keeps
        // its `inner_radius` hole)
        .geom_col_with(GeomCol {
            width: 1.0,
            ..card_col(o)
        })
        .position(PositionStack)
        .scale_fill(dz_scale(o, Aesthetic::Fill, category))
        // No y-axis padding, so the stack maps to a full 360° (closes the pie).
        .scale_y_continuous(
            ggplot_rs::scale::continuous::ScaleContinuous::new().with_expand(0.0, 0.0),
        )
        .coord_polar_with(
            ggplot_rs::coord::polar::CoordPolar::new()
                .theta("y")
                .inner_radius(inner)
                .expand(false),
        )
        .house_void(o)
        .legend_position(ggplot_rs::theme::LegendPosition::Top);
    if let Some(t) = title {
        plot = plot.title(t);
    }
    Ok(Panel::plot(plot, width, height).with_margin(void_theme(o).plot_margin))
}

/// A minimal SVG heading (for a `::LABEL`-only result).
fn heading_svg(text: &str, width: u32) -> Panel {
    Panel::own(
        width,
        40,
        format!(
            "<text x=\"4\" y=\"26\" font-family=\"system-ui,sans-serif\" font-size=\"20\" font-weight=\"600\" fill=\"{}\">{}</text>",
            theme::SVG_HEADING,
            format::escape_xml(text)
        ),
    )
}

/// A full-size placeholder panel with a centred note ("No data", "needs ≥ 2
/// values", …) and the optional title — instead of a confusing ggplot error.
fn note_svg(title: Option<&str>, note: &str, width: u32, height: u32) -> Panel {
    let (w, h) = (width as f64, height as f64);
    Panel::own(
        width,
        height,
        format!(
            "{}<text x=\"{:.1}\" y=\"{:.1}\" text-anchor=\"middle\" dominant-baseline=\"middle\" font-family=\"system-ui,sans-serif\" font-size=\"13\" fill=\"{}\">{}</text>",
            title_text(title, w),
            w / 2.0,
            h / 2.0,
            theme::SVG_NOTE,
            format::escape_xml(note)
        ),
    )
}

/// The `<text>` title strip of a [`note_svg`] placeholder, or `""` without a
/// title.
fn title_text(title: Option<&str>, w: f64) -> String {
    match title.filter(|t| !t.is_empty()) {
        Some(t) => format!(
            "<text x=\"{:.1}\" y=\"18\" text-anchor=\"middle\" font-family=\"system-ui,sans-serif\" font-size=\"14\" font-weight=\"700\" fill=\"{}\">{}</text>",
            w / 2.0,
            theme::SVG_HEADING,
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
