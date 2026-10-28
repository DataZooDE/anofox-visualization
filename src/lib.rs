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
mod chart_frames;
pub mod contract;
pub mod dashboard;
pub mod diagram;
mod downsample;
pub mod format;
pub mod freshness;
pub mod host;
pub mod image;
pub mod inputs;
pub mod kpi;
pub mod lint;
pub mod macros;
pub(crate) mod map;
pub mod math;
pub mod page;
pub mod period;
pub mod roles;
pub mod sql;
pub mod table;
pub mod theme;
pub mod thresholds;
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
    /// A minimal inline bar histogram, no axes (`::SPARKBAR`); a table cell's
    /// `list(v ORDER BY i)`, or two overlaid series from
    /// `list({'ref': r, 'cur': c} ORDER BY i)` sharing one y scale.
    Sparkbar,
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
                | Kind::Sparkbar
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
    /// `::TOOLBAR`: the next table's toolbar — `'none'` hides it entirely, or
    /// a comma-separated allow-list of parts to keep (`'search,export'`).
    Toolbar,
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
    /// An image's text alternative (`::ALT`); `''` means decorative
    /// (`alt=""`, no lint warning).
    Alt,
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
    /// Forces a `::DROPDOWN`'s widget (`::DROPDOWN_STYLE`): `'search'` (a
    /// searchable combobox) or `'native'` (a plain `<select>`), overriding
    /// the option-count threshold the browser otherwise switches on (#411).
    DropdownStyle,
    /// A button that records this viewer's answers (`::SUBMIT`, #517): the
    /// value is the button's label. **Requires a server with
    /// `options.state`; not active until that server-side support lands
    /// (#517 slice S2) — until then a dashboard with `::SUBMIT` lints clean
    /// but the button has nowhere to post to.** The statement carrying it is
    /// run as the viewer: exactly one row makes the button available, zero
    /// rows hides it, more than one row is an error (see
    /// [`roles::REGISTRY`]'s `submit/rows` lint).
    Submit,
    /// The variables a `::SUBMIT` button records (`::SUBMIT_VARS`): a
    /// comma-separated list of names, each declared by one of this
    /// dashboard's own input statements (#517).
    SubmitVars,
    /// Optional confirmation text shown after a successful `::SUBMIT`
    /// (`::SUBMIT_DONE`, #517).
    SubmitDone,
    /// A single-value text card (`::TEXT_SMALL`/`_MEDIUM`/`_LARGE`).
    Text(TextSize),
    /// A KPI tile's density (`::KPI_SIZE`): `'compact'` (single-line
    /// value + label, no sparkline/goal) or `'normal'` (default). On a
    /// `::METRIC`/`::MONEY`/`::PERCENT`/`::COMPACT` panel it sets that tile;
    /// on a `::GROUP` it sets every tile in the box (a tile's own
    /// `::KPI_SIZE` wins over the box's).
    KpiSize,
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
    /// A map's hexagonal density-bin width (`::HEXBIN`, M2c): `'auto'` or a
    /// width across (`'25 km'`; planar: a bare number) — sums `::Value`
    /// when present, else counts points per hexagon.
    Hexbin,
    /// A region code a map looks up in the built-in boundary pack
    /// (`::REGION`): ISO 3166-1 (`'DE'`, `'DEU'`), ISO 3166-2 (`'DE-BY'`), a
    /// German AGS (`'09162'`) or NUTS code (`'DE212'`), else a name.
    Region,
    /// A point's latitude on a map (`::LAT`), with `::LON`.
    Lat,
    /// A point's longitude on a map (`::LON`), with `::LAT`.
    Lon,
    /// A flow map's start (`::ORIGIN`): a region code, WKT POINT, or
    /// `{'lon':…,'lat':…,'name':…}` struct-as-JSON. With `::DEST`.
    Origin,
    /// A flow map's end (`::DEST`), see `Origin`.
    Dest,
    /// A map's second measure (`::BIVARIATE`): with a first measure
    /// (`Role::Value`), regions/points are coloured by tertiles of both — a
    /// 3×3 palette and legend (M2d, plan-maps.md §15 D10). Ignored (lint)
    /// with `::ORIGIN`/`::DEST`, `::HEXBIN` or `::SIZE`.
    Bivariate,
    /// The pack layer a map's `::REGION` codes are looked up in
    /// (`::BOUNDARIES`): `'world'`, `'world-50m'`, `'de-states'`,
    /// `'de-districts'`. Read from the first value.
    Boundaries,
    /// A time map's frame key (`::FRAME`, M2e, #149): a week, a date — one
    /// result holds every frame; the browser's Play steps through them with
    /// shared colours, classes and sizes (D21); static output shows the
    /// last frame, named in the subtitle (D19). Maps only (lint
    /// `frame/not-a-map` elsewhere).
    Frame,
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
    /// Layout: a `::VIEW` group's switcher placement (#534) — `'row'`
    /// (default: its own row above the card) or `'header'` (right-aligned
    /// in the card's title row, compact, folding into a `<select>` below a
    /// minimum card width — only when every alternative of the group is
    /// exactly one panel; otherwise `--check` warns and it falls back to
    /// `'row'`). Set directly before the group's first `::VIEW` marker,
    /// like `::COL`/`::SPAN` setting a pending value for it. Browser only —
    /// static renderers skip it (the switcher itself never shows there).
    ViewPlacement,
    /// Layout: `::FIT` makes the enclosing `::TAB` / `::SUBTAB` / `::VIEW`
    /// section (with no tabs at all: the whole dashboard) fit the browser
    /// window while it is the one shown — its rows of panels scale to fill
    /// the height left under the app and tab bars, charts are re-rendered at
    /// that size, and the page does not scroll (#400). The value is a marker
    /// (`'fit'::FIT`); `?fit=1` / `?fit=0` override it, and it is ignored on
    /// a phone (< 600 px wide). Browser only — static renderers skip it.
    Fit,
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
    /// A static picture panel (`::IMAGE`/`::PICTURE`/`::IMG`): `assets/x.svg`,
    /// a `data:image/…` URI or an `https:` URL. See `src/image.rs::src_ok`.
    Image,
    /// The `::IMAGE` shown instead, while the dashboard is in dark mode
    /// (`::IMAGE_DARK`/`::IMG_DARK`).
    ImageDark,
    /// A panel's definition (`::INFO`): an ⓘ on its title bar shows the text.
    Info,
    /// A `::MARKDOWN` box drawn as a callout (`::CALLOUT`): `'note'`,
    /// `'warning'` or `'success'` (the value).
    Callout,
    /// A `::MARKDOWN` box's chrome (`::MARKDOWN_STYLE`, #536): `'plain'` (text
    /// on the page background — no card, no tint, no accent bar), `'card'`
    /// (a plain white card, like a chart panel) or `'callout'` (today's
    /// tinted box — the default, and what `::CALLOUT` implies). A
    /// `-- @markdown_style` header sets the dashboard's own default.
    MarkdownStyle,
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
    /// Explicit x-axis tick positions (`::XBREAKS '100,1000,10000'`), in the
    /// axis's own pre-transform data units — overrides the automatic ticks
    /// (and, under `::XSCALE`, their automatic log/sqrt round-number
    /// placement). Read from the first value.
    XBreaks,
    /// Explicit y-axis tick positions (`::YBREAKS`), like [`Role::XBreaks`].
    YBreaks,
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
    /// A `::HISTOGRAM`'s bin count or explicit edges (`::HIST_BINS`): a bare
    /// number (`'20'::HIST_BINS`) or comma-separated edges
    /// (`'0,10,20,30'::HIST_BINS`) — computed over every row (every
    /// `::CATEGORY` / `::FACET` group shares the same bins, #423). A
    /// deliberately distinct token from `::BINS` (an alias of
    /// [`Role::Classes`], a map's colour-class breaks) rather than
    /// repurposing it, since one token can't parse to two different roles.
    HistBins,
    /// A `::HISTOGRAM`'s per-group count normalisation (`::HIST_NORM`):
    /// `'share'` draws each `::CATEGORY` / `::FACET` group's bars as a share
    /// of that group's own row count, so groups of unequal size (e.g. a
    /// reference vs. a current period) are still comparable (#423). Without
    /// it, bars are raw counts.
    HistNorm,
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
    /// Layout: `::SHOW_IF` — show the next panel (or `::GROUP` box) only
    /// while this BOOLEAN is `TRUE`, re-checked on every run. A statement on
    /// its own, like `::SPAN`/`::EMPTY`; not access control (`/api/panel`
    /// still answers a hidden statement). See [`crate::sql::condition`].
    ShowIf,
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
    /// Hovering a mark (`::HOVER_FOCUS`): `'series'` (default) emphasises the
    /// mark's whole series and fades the others, as hovering its legend entry
    /// does; `'none'` keeps the plain hover (dense charts). Browser only.
    HoverFocus,
    /// Line/area point markers (`::POINTS`): `'auto'` (default) hides them
    /// on a dense series but keeps them as hover/click targets, `'none'`
    /// always hides them, `'always'` always draws them (ggplot-rs
    /// `PointVisibility`).
    Points,
    /// A map's north arrow (`::NORTH_ARROW`, M3e-1 #646): `'print'` draws it
    /// for print/PNG/PDF/`--svg` only (a `dp-print-only` class the screen
    /// CSS hides), `'always'` everywhere. Rotated to true north (ggplot-rs
    /// `GeomNorthArrow`).
    NorthArrow,
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
    /// Hide or place a chart's legend / colourbar (`::LEGEND`): `'none'`,
    /// `'top'`, `'right'`, `'bottom'` or `'left'`. Without it, a chart's
    /// legend sits where it always has (unchanged). Read from the first value.
    Legend,
    /// Layout: `::GUIDE_STEP` (aliases `::TUTORIAL_STEP`, `::WIZARD_STEP`) —
    /// one step of a guided sequence (onboarding, tutorials): Back / Next,
    /// "Step 2 of 5", `?v.<name>=n`; an alias on the group's first marker
    /// names the step variable (default `step`), #516.
    GuideStep,
    /// On a `::GUIDE_STEP` (`::GATE`, aliases `::STEP_GATE`/`::NEXT_IF`):
    /// Next stays disabled until this BOOLEAN is `TRUE` — the same
    /// fail-closed evaluator as `::SHOW_IF` (`crate::sql::condition`), #516.
    Gate,
    /// On a `::GUIDE_STEP` (`::GATE_MESSAGE`, alias `::GATE_HINT`): why Next
    /// is disabled while `::GATE` is not `TRUE` — "Finish this step to
    /// continue" without one.
    GateMessage,
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

pub(crate) fn value_str(v: &Value) -> String {
    match v {
        Value::Str(s) => s.clone(),
        Value::Na => String::new(),
        _ => match v.as_f64() {
            None => String::new(),
            // A non-finite number (`f64::INFINITY`/`NAN`, e.g. from a
            // 0-divide upstream) must never reach `format!("{}", ..)`, which
            // prints the literal `inf`/`NaN` — that string then ends up
            // verbatim in a `data-label`/category key (map markers'
            // `label_of`, #319). Spell it out instead of folding it into
            // the same blank level as missing data (#340 D): a discrete
            // axis, facet or group key keeps `∞`/`−∞`/`NaN` as its own
            // visible level rather than silently merging it with `Na`.
            Some(f) if f.is_nan() => "NaN".to_string(),
            Some(f) if f == f64::INFINITY => "\u{221E}".to_string(),
            Some(f) if f == f64::NEG_INFINITY => "\u{2212}\u{221E}".to_string(),
            Some(f) => format!("{}", (f * 1000.0).round() / 1000.0),
        },
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
    // ggplot-rs#318 step 2: keep outer x labels on-canvas (ggplot-rs 0.22's
    // `AxisLabelFit`) and thin crowded ones (ggplot2's `check.overlap`).
    // `::TRACK`-aligned panels must keep their exact plot area so sibling
    // tracks stay lined up: `PlotGrid::axis_collect`
    // (`AxisCollect::Align`/`CollectX`, ggplot-rs#175, `track::stack`) forces
    // every cell of an aligned column to `Shift` itself, overriding
    // whatever this sets — so a plain chart (never grid-aligned) is the
    // only case this default actually governs.
    th.axis_text_x_fit = AxisLabelFit::Reserve;
    th.axis_text_x_check_overlap = true;
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
    // #606: `theme.geom.ink` — every geom's `apply_element_geom` hook for
    // recolouring its own hardcoded default ink (ggplot-rs's generic
    // "unmapped mark" colour, distinct from the text elements painted
    // above) reads this, not `th.text`/`th.axis_text_x` etc. A map's own
    // geoms (`GeomSf`, symbols, …) never rely on this fallback — every one
    // of their colours is set explicitly in `map::plot::build` — so this
    // was never missed there. `GeomBivariateLegend`'s two axis captions
    // are the one geom this crate attaches that DOES rely on it (its own
    // `text_color` default, `(80, 80, 80)`, only gets replaced by
    // `apply_element_geom` when `ink` is `Some`): left unset, they stayed
    // at that fixed mid-grey regardless of theme, barely visible against
    // a dark card. Same token as `axis_text_x`/`axis_text_y` above —
    // reads as "ordinary chart text" either way.
    th.geom.ink = Some(t.chart_text());
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
/// #457: ggplot-rs's own layout engine floors a panel's *drawable* plot
/// area at 50px tall (`plot_height.max(50.0)` in its `render/layout.rs`),
/// but the nested `<svg … viewBox="0 0 w h">` it wraps that panel in is
/// still sized to whatever `w`/`h` we asked for (`render_svg_native_at`
/// reuses the same number for both the layout and the SVG canvas/viewBox).
/// Ask for a panel shorter than this floor — as the KPI tile's and a
/// table's `::SPARKLINE` cell both used to, via `MIN_DIM` (32px) — and the
/// *drawn* content (sized against the 50px floor) overflows the *visible*
/// viewBox (sized against our smaller request) and gets clipped: a
/// series' low points fall outside the box and the line looks torn apart.
/// Every embedder of a `GGPlot` panel into a slot shorter than this must
/// request at least `SPARKLINE_MIN_H` and centre/pad the smaller visible
/// slot around it, never ask ggplot-rs for less. The floor itself is
/// generic to any ggplot-rs panel (not just a line geom), so
/// `render_sparkbar` (#425) clamps to it too — the name predates that
/// second consumer.
pub(crate) const SPARKLINE_MIN_H: f64 = 50.0;

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
    /// Record the plotting engine's build warnings on the SVG root as
    /// `data-warnings` (see [`render_with_warnings`])? Default `true`. A
    /// host with no script that reads it — the script-free served page
    /// (#315/sec-review-342 L2) — sets this `false` so a warning naming the
    /// panel's own data (e.g. `anofox_plot_compare`'s "not in `<factor>`")
    /// is simply never attached, on every SVG a render produces (a title
    /// bar's chart, each `::TRACK`, each contract row) — not stripped back
    /// out of some of them after the fact.
    pub record_warnings: bool,
    /// The dashboard's own `assets/` folder, canonicalized, if a relative
    /// `::IMAGE`/`::IMAGE_DARK` source should be inlined from it (#514 PR B:
    /// [`crate::image::resolve_relative`]) — `None` wherever the caller
    /// hasn't computed one (the no-server browser builder, wasm, or a
    /// dashboard with no `assets/` folder next to it), so every relative
    /// image draws the alt-text placeholder instead. Unused off the
    /// static/headless render paths (`dashboard.rs`, `image_panel_svg`
    /// below) — the browser's own `assetUrl()`/`imageSrc()` do this job for
    /// the interactive builder.
    pub asset_root: Option<std::path::PathBuf>,
    /// Which `::FRAME` to show on a time map (M2e/M2f, #149): `None` means
    /// the last frame (static output, D12) — the browser passes the
    /// stepped-to index here, like `zoom` passes its window. Ignored by
    /// every other chart kind.
    pub frame: Option<usize>,
    /// A framed bar/line/area/point chart's axes over every frame (#663),
    /// set by the render itself (`chart_frames::prepare`), never by a caller.
    #[doc(hidden)]
    pub frame_axes: Option<std::sync::Arc<chart_frames::ChartFrameAxes>>,
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
            record_warnings: true,
            asset_root: None,
            frame: None,
            frame_axes: None,
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
        // `"frame"` (M2f, #149): which frame of a `::FRAME` time map to
        // draw, like `zoom`'s own window — a non-negative integer; anything
        // else (negative, a float, absent) is ignored, not an error (the map
        // code clamps to the last frame either way).
        if let Some(n) = ctx.get("frame").and_then(|v| v.as_u64()) {
            self.frame = Some(n as usize);
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
    kpi::Kpi::from_columns(cols, &o.locale, false)
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
    Ok(with_warnings_attr(svg, &warnings, o.record_warnings))
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

/// The last step of every contract render: apply the theme variant and
/// record the build warnings. Non-finite marks are dropped by ggplot-rs
/// itself (0.21, B2-1): `draw_line` splits runs at a non-finite vertex and
/// `draw_quadratic`/arrows drop a non-finite path outright.
pub(crate) fn finish_contract_svg(svg: String, warnings: &[String], o: &RenderOptions) -> String {
    // `with_font`: a no-op on the (usual) `GGPlot` case, needed for a
    // contract path's own hand-drawn fallback note (`note_svg`) — see its
    // doc comment.
    with_warnings_attr(
        o.theme.finish_svg(o.theme.with_font(svg)),
        warnings,
        o.record_warnings,
    )
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
        /// Extra root attributes (e.g. `data-track`) written on the `<svg>`.
        attrs: Vec<(&'static str, String)>,
        /// Extra warnings (e.g. `::MAP`'s `MARK_WARN`), merged with
        /// `render_svg_native_with_warnings`'s own at [`Panel::finish`].
        warnings: Vec<String>,
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
                warnings,
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
                    warnings,
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

    /// Every `GGPlot` panel, wherever it's built, turns on ggplot-rs's own
    /// per-mark `data-fvalue`/`data-fx` (B2-4, `svg_formatted_values`) right
    /// here — the one place every builder funnels through — instead of each
    /// caller remembering to (#326, C2 of #318). A builder that wants a
    /// custom `data-fvalue` (`::YFORMAT`, a percent kind, …) wires
    /// `Scale::with_value_formatter` on its own value scale; one that
    /// doesn't still gets ggplot-rs's plain formatted number for free. A
    /// scale whose aesthetic can't carry a custom formatter yet
    /// (gradient/colour-continuous fills — `::HEATMAP`/`::CALENDAR`,
    /// sipemu/ggplot-rs#164) keeps the string-surgery fallback in
    /// `with_formatted_values`.
    pub(crate) fn plot(plot: GGPlot, width: u32, height: u32) -> Panel {
        Panel::Plot {
            plot: Box::new(plot.svg_formatted_values(true)),
            width,
            height,
            attrs: Vec::new(),
            warnings: Vec::new(),
        }
    }

    /// Queue this panel's [`motion_attrs`] as root attributes — before
    /// [`finish`](Self::finish) writes them, so a `GGPlot` panel gets them
    /// through `svg_attr`/`svg_attrs` (B2-3) rather than `with_root_attr`.
    pub(crate) fn with_motion_attrs(mut self, cols: &[Column]) -> Panel {
        let extra = motion_attrs(cols);
        if extra.is_empty() {
            return self;
        }
        match &mut self {
            Panel::Plot { attrs, .. } | Panel::Own { attrs, .. } => attrs.extend(extra),
        }
        self
    }

    /// A framed chart (#663): the frame named in the subtitle (static output
    /// shows the last, D19) and the root attributes the browser's player
    /// reads — the same `data-frames`/`data-frame`/`data-frame-name` a time
    /// map carries.
    pub(crate) fn with_frame(mut self, frame: Option<&chart_frames::ChartFrame>) -> Panel {
        let Some(f) = frame else {
            return self;
        };
        let extra = [
            (
                "data-frames",
                serde_json::to_string(&f.labels).unwrap_or_default(),
            ),
            ("data-frame", f.k.to_string()),
            ("data-frame-name", f.name.clone()),
        ];
        match &mut self {
            Panel::Plot { plot, attrs, .. } => {
                let p = std::mem::replace(
                    plot.as_mut(),
                    GGPlot::new(Vec::<(String, Vec<Value>)>::new()),
                );
                **plot = p.subtitle(&format!("{}: {}", f.name, f.labels[f.k]));
                attrs.extend(extra);
            }
            Panel::Own { attrs, .. } => attrs.extend(extra),
        }
        self
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
                warnings: extra_warnings,
            } => {
                // B2-3: every root attribute (`data-track`, the motion
                // attrs, …) goes through ggplot-rs's own `svg_attr`/
                // `svg_attrs` — set before rendering — rather than
                // `with_root_attr`, which stays for the extension's own
                // hand-drawn SVGs (`Panel::Own`) below.
                let plot = if attrs.is_empty() {
                    plot
                } else {
                    Box::new(plot.svg_attrs(attrs.into_iter().map(|(k, v)| (k.to_string(), v))))
                };
                let svg = match place {
                    Place::Doc => plot.render_svg_native_with_warnings(width, height),
                    Place::At(x, y) => plot
                        .render_svg_native_at(x, y, width, height)
                        .map(|s| (s, Vec::new())),
                };
                svg.map(|(s, mut w)| {
                    w.extend(extra_warnings);
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
        svg: with_warnings_attr(svg, &warnings, o.record_warnings),
        warnings,
    })
}

/// Record `warnings` on the SVG root as `data-warnings="[…]"` (escaped
/// JSON), unless `record` is `false` (`RenderOptions::record_warnings`,
/// #315/sec-review-342 L2) — a host with no script that ever reads the
/// attribute, such as the script-free served page, then never gets it on
/// any SVG a render produces, instead of it being stripped back out of
/// some of them (a title bar's chart, `::TRACK`s, …) after the fact — and
/// omitting it entirely when `warnings` is empty, `record` aside.
///
/// ggplot-rs's own `svg_warnings_attr` (B2-3) always writes the attribute,
/// `"[]"` included, once turned on — it has no "only if non-empty" mode and
/// no per-render on/off switch either, so it can't reproduce either of
/// this function's two conditions (several `src/contract.rs` tests pin the
/// attribute's absence on a warning-free panel, and #342's whole point was
/// that the script-free page never gets it at all). #326 keeps
/// `with_root_attr` here on purpose — the one root attribute that stays
/// string surgery even on a `GGPlot` panel, unlike `with_motion_attrs`'s
/// move to `svg_attrs` — pending a `svg_warnings_attr` with that shape
/// upstream. The root start tag ends at the first `>`: every writer
/// escapes `>` inside attribute values.
fn with_warnings_attr(svg: String, warnings: &[String], record: bool) -> String {
    if !record || warnings.is_empty() {
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
        // A diagram draws its own SVG: none of the chart post-passes apply,
        // but it does write the generic font family by hand, like every
        // other hand-drawn panel — see `Theme::with_font`'s doc comment.
        if let Some(d) = diagram::Diagram::from_columns(cols, &o.locale) {
            let svg = diagram::render(&d, place, w, h, o);
            return Ok((o.theme.finish_svg(o.theme.with_font(svg)), Vec::new()));
        }
        // `::FRAME` on a bar/line/area/point chart (#663): one frame's rows,
        // on every frame's axes
        let framed = chart_frames::prepare(cols, o);
        let (cols, o, frame) = match &framed {
            Some((view, o2, f)) => (view.as_slice(), o2, Some(f)),
            None => (cols, o, None),
        };
        let cols = downsample::prepare(cols, o);
        let cols = boxstats::prepare(cols).map_err(RenderError::Render)?;
        let (svg, warnings) = render_inner(&cols, w, h, o)
            .map(|p| p.localized(o, &cols))
            .map(|p| p.with_motion_attrs(&cols))
            .map(|p| p.with_frame(frame))
            .and_then(|p| p.finish(place))
            .map_err(RenderError::Render)?;
        Ok((finish_touches(svg, &cols, o), warnings))
    })
}

/// The post-render touches every panel gets, whatever drew it: the
/// plotting engine's own build warnings are threaded through separately
/// ([`render_placed`]'s caller attaches `data-warnings`) — this is just the
/// SVG text itself.
///
/// A horizontal legend too long for one line auto-wraps (ggplot-rs's own
/// `layout_built`: it plans the wrap once and draws exactly that plan, then
/// pushes an overflow warning into the caller's `warnings` when even that
/// isn't enough room — see ggplot-rs#318 step 2). Every panel's hand-drawn
/// SVG text (headings, notes, a map's own labels/legend, …) still writes the
/// generic font family by hand, so `with_font` swaps it in here — the one
/// place a GGPlot panel and the extension's own panels are both still just
/// `svg: String` (a no-op on the GGPlot part, #326). A map paints with the
/// variant's own tokens already: no neutral remap on top. `::XLINE` dates
/// and every mark's `data-fvalue`/`data-fx` are ggplot-rs's own job (B2-4
/// `svg_formatted_values`, on for every `GGPlot` panel since `Panel::plot`,
/// plus the `Value::DateTime` reflines now carry through `xline_values`);
/// the motion attrs (`data-animate`, …) went onto the panel before it was
/// rendered, via `svg_attrs` (B2-3, `Panel::with_motion_attrs`).
/// `with_formatted_values` below only still runs for a map's hand-drawn
/// marks and a gradient-fill measure ggplot-rs can't give a custom
/// formatter yet.
fn finish_touches(svg: String, cols: &[Column], o: &RenderOptions) -> String {
    let svg = o.theme.with_font(svg);
    // A map (`Panel::Plot` since #420) paints with the variant's own
    // resolved tokens already (`o.theme.mix(...)`/`hex(...)` computed
    // straight from this render's own theme, not ggplot-rs's hard-coded
    // black/white geom defaults or the generic `DZ_PALETTE` indices
    // `finish_svg`'s `remap_neutrals` targets) — running it on top would
    // corrupt those colours, not just skip a no-op. Verified empirically
    // while cleaning up #420 slice 6: forcing `finish_svg` to run
    // unconditionally breaks `themes_paint_the_map_with_their_own_tokens`
    // (`tests/maps.rs`) — the land backdrop's `theme::mix(muted, card,
    // 0.92)` fill stops appearing in the SVG verbatim, remapped to
    // something else. Not a leftover from the old hand-written writer;
    // still required for the `geom_sf` path.
    let svg = if map::is_map(cols) {
        svg
    } else {
        o.theme.finish_svg(svg)
    };
    with_formatted_values(svg, cols, o)
}

/// Build (but don't render) a chart's plot for `::TRACK` stacking
/// (`track::stack`, sipemu/ggplot-rs#175): the same pipeline
/// [`render_placed`] runs up to [`Panel::finish`], minus it — the caller
/// composes the returned `GGPlot` together with every track's own plot into
/// one `PlotGrid` and renders them in a single pass, instead of each panel
/// separately. `None` when the chart isn't backed by a plain `GGPlot` panel
/// at all (a diagram, map, table, KPI, …), in which case a `::TRACK` under
/// it stays today's "needs a numeric or date x axis" error note. Also
/// returns the chart's columns post `downsample`/`boxstats` prep, for the
/// caller's own [`finish_touches`] call once the grid is rendered.
/// The chart's own root attributes (motion attrs, …) go on the `GGPlot`
/// via `svg_attrs` right here — the same thing [`Panel::finish`] does for
/// a lone render — and [`track::stack`] composes the result straight into
/// a `PlotGrid`: a cell's own `svg_attr`s land on a `<g>` wrapping its
/// content (sipemu/ggplot-rs#208).
pub(crate) fn chart_plot_for_track(
    cols: &[Column],
    width: u32,
    height: u32,
    o: &RenderOptions,
) -> Result<Option<(GGPlot, Vec<Column>)>, RenderError> {
    guard(|| {
        if diagram::Diagram::from_columns(cols, &o.locale).is_some() {
            return Ok(None);
        }
        let (w, h) = (clamp_dim(width as u64), clamp_dim(height as u64));
        let cols = downsample::prepare(cols, o);
        let cols = boxstats::prepare(cols).map_err(RenderError::Render)?;
        let panel = render_inner(&cols, w, h, o)
            .map(|p| p.localized(o, &cols))
            .map(|p| p.with_motion_attrs(&cols))
            .map_err(RenderError::Render)?;
        match panel {
            Panel::Plot { plot, attrs, .. } => {
                let plot = if attrs.is_empty() {
                    *plot
                } else {
                    plot.svg_attrs(attrs.into_iter().map(|(k, v)| (k.to_string(), v)))
                };
                Ok(Some((plot, cols)))
            }
            Panel::Own { .. } => Ok(None),
        }
    })
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

/// The `::DROPDOWN_STYLE` values: `'search'` forces the searchable combobox
/// below the option-count threshold the browser otherwise switches on;
/// `'native'` forces the plain `<select>` above it.
pub const DROPDOWN_STYLES: &[&str] = &["search", "native"];

/// The `::ANIMATE` modes the browser plays.
pub const ANIMATE_MODES: &[&str] = &["auto", "draw", "grow", "reveal", "none"];

/// The `::NORTH_ARROW` values.
pub const NORTH_ARROW_MODES: &[&str] = &["print", "always"];

/// The `::POINTS` values.
pub const POINTS_MODES: &[&str] = &["auto", "none", "always"];

/// The `::HOVER_FOCUS` values the browser knows.
pub const HOVER_FOCUS_MODES: &[&str] = &["series", "none"];

/// The `::LEGEND` values — `'none'` plus every [`ggplot_rs::theme::LegendPosition`]
/// variant that is a side (not `Inside`, which `::LEGEND` has no syntax for).
pub const LEGEND_POSITIONS: &[&str] = &["none", "top", "right", "bottom", "left"];

/// A panel's motion directives, as root attributes for the browser (static
/// output ignores them): `data-animate` (`::ANIMATE`) and, for `'reveal'`,
/// `data-xline` — where the reveal starts, in the x axis' unit (epoch
/// seconds on a date axis, a level on a discrete one); `data-story` +
/// `data-story-by` (`::STORY`, `::STORY_BY`); `data-transition`
/// (`'tween'::TRANSITION`); `data-hover-focus="none"` (`'none'::HOVER_FOCUS`).
///
/// Returned as `(name, value)` pairs rather than spliced into a rendered SVG
/// string: [`Panel::with_motion_attrs`] sets them on the panel *before* it's
/// rendered, so a `GGPlot` panel gets them through ggplot-rs's own
/// `svg_attr`/`svg_attrs` (B2-3) in [`Panel::finish`] — the same allow-listed,
/// collision-checked path as every other ggplot-rs root attribute — never
/// `with_root_attr`.
fn motion_attrs(cols: &[Column]) -> Vec<(&'static str, String)> {
    let mut out: Vec<(&'static str, String)> = Vec::new();
    if let Some(mode) = role_str(cols, Role::Animate).map(|m| m.to_ascii_lowercase()) {
        if ANIMATE_MODES.contains(&mode.as_str()) {
            if mode == "reveal" {
                if let Some(c) = find_role(cols, Role::VLine) {
                    let at = xline_nums(&c.values)
                        .first()
                        .map(|f| f.to_string())
                        .or_else(|| c.values.iter().find_map(|v| v.as_str().map(str::to_string)));
                    if let Some(at) = at {
                        out.push(("data-xline", at));
                    }
                }
            }
            out.push(("data-animate", mode));
        }
    }
    if role_str(cols, Role::Transition).is_some_and(|t| t.eq_ignore_ascii_case("tween")) {
        out.push(("data-transition", "tween".to_string()));
    }
    if role_str(cols, Role::HoverFocus).is_some_and(|m| m.eq_ignore_ascii_case("none")) {
        out.push(("data-hover-focus", "none".to_string()));
    }
    if let Some(steps) = story_steps(cols) {
        let by = match role_str(cols, Role::StoryBy).map(|b| b.to_ascii_lowercase()) {
            Some(b) if b == "series" && find_role(cols, Role::Category).is_some() => "series",
            _ => "x",
        };
        out.push(("data-story-by", by.to_string()));
        out.push(("data-story", steps));
    }
    out
}

/// A cell as text: a string as is, a number without a trailing `.0`. A
/// non-finite number (`Infinity`/`NaN`) is treated like `Na` rather than
/// passed to `f64::to_string` — which prints the literal `inf`/`NaN` — since
/// callers write this straight into ids, `::DIAGRAM` node/lane names and
/// their `data-*` attributes (#319).
pub(crate) fn key_text(v: &Value) -> Option<String> {
    match v {
        Value::Str(s) => Some(s.clone()),
        Value::Na => None,
        v => v.as_f64().filter(|f| f.is_finite()).map(|f| f.to_string()),
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

/// Give every hoverable mark its value as the panel formats it:
/// `data-fvalue="$1,234.50"` per the panel's `::YFORMAT` (or a percent
/// kind) and the dashboard locale, and — when the axis formats x
/// (`::XFORMAT`, or a `::LOCALE` date order) — `data-fx` beside `data-x`.
///
/// `data-fvalue` on an ordinary `GGPlot` mark is ggplot-rs's own job now
/// (B2-4 `svg_formatted_values`, on since `Panel::plot`, through each
/// scale's `Scale::format_value` — `with_value_formatter`, wired per
/// builder, keeps the text identical to what this function used to write
/// by hand); this pass only still replaces it — overriding whatever
/// ggplot-rs already wrote natively, since there's no per-mark opt-out —
/// for the cases that can't go through a `Scale` at all:
///  - a Fill-mapped measure (`::HEATMAP`/`::CALENDAR`) banded by
///    `::THRESHOLDS` (`TileFill::Steps`/`StepFill`): a plain discrete
///    `Scale`, not one of the three gradient scales `with_value_formatter`
///    (ggplot-rs 0.22.2, sipemu/ggplot-rs#164) was added to. A
///    `::COLORSCALE` ramp (`TileFill::Ramp`/`FmtGradient`) wires that hook
///    straight into `ScaleColorGradientN` instead (`FmtGradient::new`) and
///    so no longer needs this fallback;
///  - a map's own `::CLASSES`/`::BIVARIATE` fill — `ggplot_rs::scale::
///    binned::ScaleFillBinned`/`scale::bivariate::ScaleFillBivariate`
///    (`src/map/fill.rs`'s `scale_for`), the same "plain discrete `Scale`,
///    no formatter hook" family as `TileFill::Steps` above. A map's plain
///    continuous or `::CATEGORY` fill (`MapFill`/`fill::Reaimed`'s own
///    `format_value`) *is* `svg_formatted_values`-native, as is a
///    colour-less graduated `::SIZE`'s own size scale — verified by
///    forcing this pass off for every map kind and re-running
///    `tests/maps.rs` (`fvalue_is_written_once`, `hexbin_counts_or_sums`'s
///    "never both" case): only a `::CLASSES`/`::BIVARIATE` map actually
///    lost its `data-fvalue`, caught by the before/after example diff
///    (`delivery-coverage.sql`'s districts tab) this pass's own narrowing
///    (below) now keeps green. #420 slice 6 was the first to widen the
///    condition past `map::is_map(cols)` wholesale and catch this — a map
///    used to need the wholesale version too (its hand-drawn `Panel::Own`
///    marks were never a `GGPlot` at all), dropped once every map kind
///    routed through `plot::build`.
///
/// `data-fx`, though, stays unconditional: a date x column rendered on a
/// *discrete* axis (`::XAXIS` on a bar/category chart whose values happen
/// to be dates, like a map's) is a plain `ScaleDiscrete` — ggplot-rs has no
/// date-aware formatting for it at all, native or otherwise — so the
/// `::LOCALE`/`::XFORMAT` fallback below is still the only thing that can
/// write `data-fx` for it. On a *continuous* date x (`ScaleDateTime`)
/// ggplot-rs already wrote the right `data-fx` natively; this just
/// recomputes the identical string and replaces it, which costs a little
/// work but changes nothing.
fn with_formatted_values(svg: String, cols: &[Column], o: &RenderOptions) -> String {
    let value = if map::is_map(cols) {
        map::measure_col(cols)
    } else {
        cols.iter().find(|c| matches!(c.role, Role::Value(_)))
    };
    let Some(value) = value else {
        return svg;
    };
    let Role::Value(kind) = value.role else {
        return svg;
    };
    let do_fvalue = (matches!(kind, Kind::Heatmap | Kind::Calendar)
        && thresholds_of(cols).ok().flatten().is_some())
        || (map::is_map(cols)
            && (find_role(cols, Role::Classes).is_some()
                || find_role(cols, Role::Bivariate).is_some()));
    let xfmt = role_fmt(cols, Role::XFormat).map(|s| format::ValueFmt::parse(&s));
    if !do_fvalue && xfmt.is_none() && o.locale.is_default() {
        return svg;
    }
    if !svg.contains(" data-value=\"") {
        return svg;
    }
    let fmt = value_fmt(cols, kind, &value.values);
    let attr = |tag: &str, name: &str| -> Option<String> {
        let k = tag.find(&format!(" {name}=\""))? + name.len() + 3;
        Some(tag[k..k + tag[k..].find('"')?].to_string())
    };
    // Drop an existing `name="…"` attribute from `tag` (ggplot-rs's own
    // native pass may already have written a plain-formatted one — this
    // pass's own value replaces it rather than duplicating the attribute).
    let strip = |tag: &str, name: &str| -> String {
        let needle = format!(" {name}=\"");
        let Some(k) = tag.find(&needle) else {
            return tag.to_string();
        };
        let val_start = k + needle.len();
        let Some(rel) = tag[val_start..].find('"') else {
            return tag.to_string();
        };
        format!("{}{}", &tag[..k], &tag[val_start + rel + 1..])
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
        let self_closing = tag.ends_with('/');
        let body = if self_closing {
            &tag[..tag.len() - 1]
        } else {
            tag
        };
        // Only strip (and recompute) `data-fvalue` when this pass means to
        // write it — otherwise leave whatever ggplot-rs wrote natively
        // alone (B2-4), rather than stripping it with nothing to replace
        // it with.
        let body = if do_fvalue {
            strip(body, "data-fvalue")
        } else {
            body.to_string()
        };
        let body = strip(&body, "data-fx");
        let mut extra = String::new();
        if do_fvalue {
            if let Some(v) = attr(tag, "data-value").and_then(|v| v.parse::<f64>().ok()) {
                extra.push_str(&format!(
                    " data-fvalue=\"{}\"",
                    format::escape_xml(&fmt.full(v, &o.locale))
                ));
            }
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
        out.push_str(&rest[..start]);
        out.push_str(&body);
        out.push_str(&extra);
        if self_closing {
            out.push('/');
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
    .map_err(|e| format!("render failed: {e:?}"))
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
    // A static ::IMAGE / ::IMAGE_DARK panel (#514): checked before the
    // measure-column branches below, which would otherwise error with "no
    // measure column" (this render path has no Role::Value column for an
    // image panel, same as a ::MARKDOWN or ::TABLE one would today).
    if find_role(cols, Role::Image).is_some() || find_role(cols, Role::ImageDark).is_some() {
        return Ok(image_panel_svg(cols, width, height, o));
    }
    // A static ::MARKDOWN panel (#568): checked before the measure-column
    // branches below for the same reason as ::IMAGE above — a markdown
    // column set has no Role::Value column either, and must not fall
    // through to the Kpi/map/value logic.
    if find_role(cols, Role::Markdown).is_some() {
        return Ok(markdown_panel_svg(cols, width, height, o));
    }
    // A KPI / text card (value, change, sparkline, goal) — the same tile as
    // the headless dashboard; at most its natural height.
    if let Some(k) = kpi::Kpi::from_columns(cols, &o.locale, false) {
        let h = height.min(k.height().ceil() as u32).max(MIN_DIM);
        return Ok(Panel::own(width, h, k.body(width as f64, h as f64, o)));
    }
    // A map is driven by a ::MAP (geometry) column, coloured by an optional measure.
    if cols.iter().any(|c| {
        matches!(
            c.role,
            Role::Geometry | Role::Region | Role::Lat | Role::Lon | Role::Origin | Role::Dest
        )
    }) {
        return Ok(match map::build(o, cols, width, height) {
            // #420: every map is a real GGPlot now — the same Panel::Plot
            // every other chart kind renders through.
            Some((plot, attrs, warnings)) => {
                let mut p = Panel::plot(plot, width, height);
                if let Panel::Plot {
                    attrs: a,
                    warnings: w,
                    ..
                } = &mut p
                {
                    *a = attrs;
                    *w = warnings;
                }
                p
            }
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
    // #332: a ::HEATMAP's measure may be text (a status word, coloured with
    // ::COLORS as a level map) — render_heatmap's own categorical branch
    // handles it, so the numeric guards below (which exist for every other
    // kind, whose measure really must be a number) don't apply to it.
    let categorical_heatmap =
        kind == Kind::Heatmap && value.values.iter().any(|v| matches!(v, Value::Str(_)));
    // Empty / too-small inputs get a clear note instead of a confusing
    // "requires aesthetic 'x'" error from the plotting engine.
    let n_ok = value.values.iter().filter(|v| v.as_f64().is_some()).count();
    if value.values.is_empty() {
        return Ok(note_svg(title.as_deref(), "No data", width, height));
    }
    if n_ok == 0 && !categorical_heatmap {
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
    if n_ok < need && !categorical_heatmap {
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
        Kind::Histogram => {
            return render_histogram(o, value, cols, title.as_deref(), width, height)
        }
        Kind::Density => return render_density(o, value, cols, title.as_deref(), width, height),
        Kind::QQ => return render_qq(o, value, cols, title.as_deref(), width, height),
        Kind::Heatmap => {
            return render_heatmap(o, value, cols, title.as_deref(), width, height, None)
        }
        Kind::Calendar => return render_calendar(o, value, cols, title.as_deref(), width, height),
        Kind::Candlestick => {
            return render_candlestick(o, value, cols, title.as_deref(), width, height)
        }
        Kind::Radar => return render_radar(o, value, cols, title.as_deref(), width, height),
        Kind::Sparkline => return render_sparkline(o, value, cols, width, height),
        Kind::Sparkbar => return render_sparkbar(o, value, cols, width, height),
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

/// `::LEGEND` ('none'/'top'/'right'/'bottom'/'left') as a
/// [`ggplot_rs::theme::LegendPosition`] override — `None` without the role
/// or with an unrecognised value (lint warns on that separately; the panel
/// keeps its own default position, unchanged).
pub(crate) fn legend_position_of(cols: &[Column]) -> Option<ggplot_rs::theme::LegendPosition> {
    use ggplot_rs::theme::LegendPosition as L;
    match role_str(cols, Role::Legend)?.to_ascii_lowercase().as_str() {
        "none" => Some(L::None),
        "top" => Some(L::Top),
        "right" => Some(L::Right),
        "bottom" => Some(L::Bottom),
        "left" => Some(L::Left),
        _ => None,
    }
}

/// Below this panel width, an un-overridden continuous (gradient) fill
/// legend moves to the bottom instead of ggplot-rs's own default `Right`
/// (heatmap/calendar/hexbin, #510). `Right` is a tall vertical colourbar
/// that eats a fixed slice of panel *width* — proportionally large on a
/// narrow panel, where a horizontal bar at `Bottom` only costs *height*
/// instead (ggplot-rs 0.31.1 draws a continuous colourbar at `Top`/`Bottom`
/// horizontally, slicing low→high left to right like an x-axis, rather
/// than clipping the old vertical bar into that short band — sipemu/
/// ggplot-rs#211). 480px: `web/theme-base.css`'s own `minmax(440px,1fr)`
/// static-grid tile width, rounded up — a panel at or under that is
/// narrow by the same convention the dashboard grid already uses; above
/// it, a vertical bar beside the panel has always looked fine. An explicit
/// `::LEGEND` still wins (checked by each caller via [`legend_position_of`]
/// before this is ever consulted).
const NARROW_PANEL_PX: f64 = 480.0;

/// [`NARROW_PANEL_PX`]'s own position pick — `Some(Bottom)` under the
/// threshold, `None` above it (meaning: keep ggplot-rs's own default).
fn narrow_continuous_legend(width: u32) -> Option<ggplot_rs::theme::LegendPosition> {
    (width as f64 <= NARROW_PANEL_PX).then_some(ggplot_rs::theme::LegendPosition::Bottom)
}

/// The first finite numeric value of a `role` column.
fn role_num(cols: &[Column], role: Role) -> Option<f64> {
    find_role(cols, role).and_then(|c| {
        c.values
            .iter()
            .find_map(|v| v.as_f64().filter(|f| f.is_finite()))
    })
}

/// `::XBREAKS`/`::YBREAKS`: a comma-separated list of explicit tick
/// positions (`'100,1000,10000'`), in the axis's own pre-transform data
/// units. Non-numeric tokens are dropped — the linter warns separately
/// (`lint::breaks_diags`) — so an all-invalid spec yields `None`, same as
/// not setting the role at all.
fn role_breaks(cols: &[Column], role: Role) -> Option<Vec<f64>> {
    let raw = role_str(cols, role)?;
    let vals: Vec<f64> = raw
        .split(',')
        .filter_map(|t| t.trim().parse::<f64>().ok())
        .filter(|f| f.is_finite())
        .collect();
    (!vals.is_empty()).then_some(vals)
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

/// Half-width of error-bar caps in normalized panel units: 18 % of one bar
/// (`slot` x units wide) on a discrete x, else 20 % of the smallest gap
/// between x values. `slot` is the whole dodge-group's width even when
/// dodged (not pre-divided by the group count): ggplot-rs's GeomErrorbar
/// shrinks `cap_width` by the dodge count itself once `position_dodge` is
/// on the layer (ggplot-rs#318 B3-7).
fn cap_half_width(x: &[Value], levels: Option<&[String]>, slot: f64) -> f64 {
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
    if !discrete_dodge(kind, cols) {
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

/// `'auto'::HEXBIN` bin count on a planar scatter (#426): roughly one
/// hexagon every 16 px across the panel's narrower side, so a small panel
/// isn't swamped with cells and a large one isn't left looking sparse —
/// clamped to a range that stays readable either way.
fn auto_hex_bins(width: u32, height: u32) -> usize {
    let px = width.min(height).max(1) as f64;
    (px / 16.0).round().clamp(14.0, 50.0) as usize
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

    // #533: a VARCHAR ::XAXIS whose values all happen to parse as ISO dates
    // (`strftime(…, '%Y-%m-%d')`) is reinterpreted as `Value::DateTime` by
    // `sql::coerce_for_role` — intentionally, so a daily line/bar chart gets
    // a sensible continuous datetime axis (#424-adjacent). But a violin/box/
    // strip plot's x is never a continuous position: every mark at the same
    // x already groups into one body, the same as any other VARCHAR level
    // (#444 made that true for heatmaps too) — so grouping still has to
    // split on distinct *days*, not silently merge every row into the first
    // one because a datetime scale has nowhere discrete to put them.
    let group_by_day = matches!(kind, Kind::Violin | Kind::Boxplot | Kind::Jitter)
        && x.values.iter().any(|v| matches!(v, Value::DateTime(_)))
        && x.values
            .iter()
            .all(|v| matches!(v, Value::DateTime(_) | Value::Na));
    let x_discrete = group_by_day || x.values.iter().any(|v| matches!(v, Value::Str(_)));
    let bar = matches!(
        kind,
        Kind::Bar | Kind::BarStacked | Kind::BarPercent | Kind::BarStackedPercent
    );
    let point_kind = matches!(kind, Kind::Point | Kind::Bubble);
    // `::HEXBIN` on a planar scatter (#426): a 2D count density, in place of
    // the individual points — the predicted-vs-actual-at-5,000-rows case a
    // sampled scatter loses. `'auto'` sizes the lattice from the panel's own
    // pixel size; a number sets the bin count across *each* axis (ggplot2's
    // own single-`bins` convention for `stat_bin_hex` — a lopsided x/y data
    // range can stretch the hexagons, same as ggplot2's). Ignored off a
    // point/bubble chart, like any other geom-specific role.
    let hex_bins: Option<usize> = if point_kind {
        role_str(cols, Role::Hexbin)
            .map(|s| map::hexbin::HexSize::parse(&s))
            .transpose()?
            .map(|spec| match spec {
                map::hexbin::HexSize::Auto => auto_hex_bins(width, height),
                map::hexbin::HexSize::Width(n, _) => (n.round() as usize).clamp(2, 300),
            })
    } else {
        None
    };
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
    // Grouped marks on a discrete x sit side by side under `position_dodge`
    // (rows already ordered by `render_cartesian`); `GeomText`/`GeomLabel`
    // honour it too (ggplot-rs#318 B3-4), so ::DATALABELS dodges the same way.
    let dodge_width = if bar { 0.9 } else { 0.6 };
    let dodge = x_levels.is_some();
    // On a discrete x, normalise every value to the same `value_str` text
    // the facet/radar levels already use, rather than handing ggplot-rs a
    // mixed Value column — e.g. a non-finite level `downsample::prepare`
    // kept as its own `∞`/`−∞`/`NaN` text (#340 D) is a `Value::Str`
    // already, but a `Value::Na` gap on the same axis isn't, and a mixed
    // column would otherwise make ggplot-rs guess the column's type.
    let xvals = if x_discrete {
        x.values
            .iter()
            .map(|v| match v {
                // A date/timestamp group label (`group_by_day` above, or a
                // genuine DATE/TIMESTAMP ::XAXIS on these same chart kinds)
                // reads like the date axis elsewhere, not `value_str`'s
                // epoch-seconds number.
                Value::DateTime(secs) => Value::Str(xline_date_text(*secs)),
                v => Value::Str(value_str(v)),
            })
            .collect()
    } else {
        x.values.clone()
    };
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
        data.extend(boxstats::data_columns(cols, o));
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

    // Several measure columns of the same bar family (e.g. two ::BARCHART
    // columns, no overlay of a different kind): still a legend-worthy combo
    // like the non-bar case below, just coloured through `fill` — a bar's
    // own colour is its fill, not its stroke (#292). A *mixed* combo (bars
    // with a line/area/point overlay) keeps the plain "overlay takes the
    // next palette colour, no legend" treatment just below instead.
    let bar_family = |k: Kind| {
        matches!(
            k,
            Kind::Bar | Kind::BarStacked | Kind::BarPercent | Kind::BarStackedPercent
        )
    };
    let pure_bar_combo = bar
        && extras
            .iter()
            .all(|c| matches!(c.role, Role::Value(k) if bar_family(k)));

    // Multi-measure combo with no explicit CATEGORY (e.g. observed+trend,
    // actual+predicted, or a line with a changepoint-point overlay): colour each
    // measure distinctly and name it in the legend by its column header, so the
    // series are separable. The default single-series theming would otherwise
    // paint every measure the same brand colour.
    let combo_names: Vec<String> =
        if category.is_none() && !extras.is_empty() && (!bar || pure_bar_combo) {
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
            aes = if pure_bar_combo {
                aes.fill("__s0")
            } else {
                aes.color("__s0")
            };
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
    // ::LINESTYLE 'flow'/'dashed' rows dash their line through the native
    // linetype aesthetic (ggplot-rs's own 6,3 dash, an accepted visual delta)
    // instead of the old `with_line_styles` string surgery, which wrote
    // `class="dp-flow"` — malformed since 0.21 already writes `class="gg-mark
    // …"` on every mark. `data_attr` exposes the raw value so the browser can
    // still drive the moving-dash animation off `[data-linestyle="flow"]`.
    // Mapped only on the line-drawing layer itself (`dash_aes`/`undash`
    // below), not the shared/global aes: a point marker has no line to dash,
    // and giving every layer the aesthetic would force every layer's legend
    // contribution to Some(false) before ggplot-rs would agree to suppress
    // the (otherwise unwanted) linetype legend — which would also swallow
    // the real ::CATEGORY colour legend.
    let line_style = find_role(cols, Role::LineStyle);
    if let Some(ls) = line_style {
        let vals: Vec<Value> = ls
            .values
            .iter()
            .map(
                |v| match v.as_str().map(|s| s.trim().to_ascii_lowercase()) {
                    Some(s) if s == "flow" || s == "dashed" => Value::Str(s),
                    // `Value::Na` maps to the scale's `"NA"` level (`key_str`)
                    // without writing a `data-linestyle` attribute at all
                    // (`set_row_extras` skips NA `data_attr` columns) — unlike
                    // the literal string `"NA"`, which would write one on every
                    // plain/solid row.
                    _ => Value::Na,
                },
            )
            .collect();
        data.push(("linestyle".to_string(), vals));
    }
    // Add the linetype aesthetic + its manual scale and `data-linestyle` to
    // the most recently added layer, and keep it (and the `cat` colour it
    // inherits) out of the legend box.
    let dash_aes = |plot: GGPlot| -> GGPlot {
        if line_style.is_none() {
            return plot;
        }
        plot.layer_aes(
            Aes::new()
                .linetype("linestyle")
                .data_attr("linestyle", "linestyle"),
        )
        .scale_linetype_manual(vec![
            ("NA", Linetype::Solid),
            ("flow", Linetype::Dashed),
            ("dashed", Linetype::Dashed),
        ])
        .show_legend(false)
    };
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
                ..Default::default()
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
    // `::POINTS`: on a dense series the markers swamp the line, so by
    // default ggplot-rs leaves them uninked (still hover/click targets).
    let visibility = match role_str(cols, Role::Points)
        .map(|s| s.to_ascii_lowercase())
        .as_deref()
    {
        Some("none") => PointVisibility::Hidden,
        Some("always") => PointVisibility::Visible,
        _ => PointVisibility::AUTO,
    };
    let small_point = || GeomPoint {
        color: brand,
        size: 1.8,
        visibility,
        ..Default::default()
    };
    // A light brand wash for box/violin bodies (unless a CATEGORY fills them).
    let body_fill = wash(o, brand, 0.72);
    plot = match kind {
        Kind::Bar | Kind::BarPercent if dodge => plot
            .geom_col_with(card_col(o))
            .position(position_dodge(dodge_width)),
        // The plain (non-stacked, non-dodged) `::BARCHART` role: usually one
        // bar per x (stacking is then a no-op), but a numeric x with a
        // CATEGORY can put more than one group's row at the same x without
        // `dodge` (`discrete_dodge` only dodges a *discrete* x). ggplot-rs
        // 0.24's new `geom_col()` default (`position_stack()`, #169) would
        // silently stack that case; `::BARCHART_STACKED` is the role for
        // that, so keep this one's documented overlap-from-0 meaning.
        Kind::Bar | Kind::BarPercent => plot.geom_col_with(card_col(o)).position(PositionIdentity),
        Kind::BarStacked => plot.geom_col_with(card_col(o)).position(PositionStack),
        Kind::BarStackedPercent => plot.geom_col_with(card_col(o)).position(PositionFill),
        // Lines/areas also get point markers — they carry the per-point `<title>`
        // so every chart is hoverable (and clickable for linking).
        Kind::Line | Kind::LinePercent => {
            plot = dash_aes(plot.geom_line_with(thin_line()));
            plot.geom_point_with(small_point())
        }
        // One step line per series (+ `::CENSOR` marks).
        Kind::Step => {
            plot = dash_aes(plot.geom_step_with(GeomStep {
                color: brand,
                width: 1.2,
                ..Default::default()
            }))
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
                // families) needs its `regression` feature — not used here.
                Some("lm") | Some("glm") | Some("linear") => fit(plot, SmoothMethod::Lm),
                // Penalized B-spline (P-spline, λ by GCV) — ggplot-rs's own
                // wasm-safe builtin without the `regression` feature (#174);
                // grouped by series/facet the same way as `lm`/`loess` (its
                // `StatSmooth::compute_group` groups on the mapped
                // color/fill aesthetic).
                Some("gam") => fit(plot, SmoothMethod::Gam),
                Some(other) => {
                    return Err(format!(
                        "::SMOOTH_METHOD '{other}' is not one of 'loess', 'lm', 'gam', 'glm'"
                    ))
                }
            }
        }
        // The plain (non-stacked) `::AREA` role: several CATEGORY series
        // overlapping at shared x values (e.g. comparing a few series over
        // the same date range) is the common case this role exists for, as
        // opposed to `::AREA_STACKED`. ggplot-rs 0.24's new `geom_area()`
        // default (`position_stack()`, same parity gap as #169) would
        // silently stack those bands; keep the documented overlap look.
        Kind::Area => plot
            .geom_area()
            .position(PositionIdentity)
            .geom_point_with(small_point()),
        Kind::AreaStacked => plot
            .geom_area_with(GeomArea {
                fill: brand,
                color: brand,
                alpha: 0.85,
                ..Default::default()
            })
            .position(PositionStack),
        // `::HEXBIN` (above): a count-density layer in place of the points —
        // checked before the pointrange/fixed-size arms below, so it wins
        // over either (a y-interval or ::SIZE alongside ::HEXBIN is not a
        // meaningful combination; this one does). `::CATEGORY`/`::SIZE`
        // would otherwise be inherited from the plot-level aes (`color_col`
        // above), but this layer's own `layer_data` carries only `x`/`y`/
        // `count`/`width`/`height`, so those mappings simply find no column
        // to resolve and are dropped — one hexbin density for the whole
        // panel, not one `::CATEGORY` value (the map's own `::HEXBIN` has
        // the same "category ignored" rule, `map/hexbin.rs`'s
        // `hexbin_diags`; `--check` flags it here too,
        // `chart/hexbin-category`).
        //
        // Pre-binned with ggplot-rs's own `StatBinHex::compute_group`
        // (issue #318: density-binning logic belongs in ggplot-rs) — the
        // same call `map/hexbin.rs`'s `of` already makes — rather than
        // leaving `GeomHex`'s *default* stat to bin lazily inside the
        // pipeline: `GeomHex::draw` reads the post-stat `fill` column
        // directly, bypassing the aes-mapping system entirely (there's no
        // raw column to map a stat-computed aesthetic from), and
        // ggplot-rs's legend-width reservation runs off that mapping
        // system. Binning here ourselves makes `count` a genuine *raw*
        // column the layer's own `layer_data` carries, so `.fill("count")`
        // is a real aes mapping `StatIdentity` (no further binning) leaves
        // alone — the legend gets its margin like any other mapped fill
        // (`GeomTile`/`GeomPoint`). ggplot-rs < 0.28 only reserved that
        // margin for a plot-level mapping, not a layer-only one, and lost
        // it again once a second layer (`::IDENTITY`/`::ABLINE`) was
        // present — fixed upstream in sipemu/ggplot-rs#201 (0.28.0, #449).
        Kind::Point | Kind::Bubble if hex_bins.is_some() => {
            use ggplot_rs::data::DataFrame;
            use ggplot_rs::scale::{Scale, ScaleSet};
            use ggplot_rs::stat::binhex::StatBinHex;
            use ggplot_rs::stat::identity::StatIdentity;
            use ggplot_rs::stat::Stat;
            let bins = hex_bins.unwrap_or(30);
            let mut raw = DataFrame::new();
            raw.add_column("x".to_string(), col_of(&data, "x"));
            raw.add_column("y".to_string(), col_of(&data, "y"));
            let binned = StatBinHex::new()
                .bins(bins)
                .compute_group(&raw, &ScaleSet::new());
            let col = |name: &str| {
                binned
                    .column(name)
                    .map(<[Value]>::to_vec)
                    .unwrap_or_default()
            };
            let hex_data: Vec<(String, Vec<Value>)> = vec![
                ("x".to_string(), col("x")),
                ("y".to_string(), col("y")),
                ("count".to_string(), col("count")),
                ("width".to_string(), col("width")),
                ("height".to_string(), col("height")),
            ];
            // `min`/`max` left non-finite: `FmtGradient` then skips fixing the
            // scale's domain (`with_limits`), so it self-trains off the real
            // binned counts instead of a guess made before binning ran.
            let ramp = Ramp::new(o, o.theme.seq_low(), f64::NAN, f64::NAN);
            let mut fill_scale = FmtGradient::new(&ramp, format::ValueFmt::Plain, o.locale);
            fill_scale.set_name("count");
            plot.geom_hex_with(GeomHex {
                color: o.theme.colors.card,
                line_width: 0.3,
                alpha: 0.95,
            })
            .stat(StatIdentity)
            .layer_data(hex_data)
            .layer_aes(Aes::new().x("x").y("y").fill("count"))
            .scale_fill(fill_scale)
        }
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
            // identity: ggplot-rs 0.22 reads lower/middle/upper/ymin/ymax/
            // outliers straight off the data (ggplot-rs#318 B3-5) — and
            // widens the y scale to the outlier extent itself, so no more
            // geom_blank extent layer for outliers is needed. `tooltip` in
            // `boxstats::layer_aes()` is the box's own hover text (`btip`,
            // built as data in `boxstats::data_columns`, not a post-render
            // `<title>` rewrite).
            let p = plot
                .geom_boxplot_with(GeomBoxplot {
                    fill: body_fill,
                    color: o.theme.chart_label(),
                    width: 0.6,
                    alpha: 1.0,
                })
                .stat(StatIdentity)
                .layer_aes(boxstats::layer_aes());
            match find_role(cols, Role::Mean) {
                Some(mean) => p
                    .geom_point_with(GeomPoint {
                        size: 2.6,
                        color: o.theme.chart_label(),
                        alpha: 1.0,
                        ..Default::default()
                    })
                    .layer_data(vec![
                        ("x".to_string(), x.values.clone()),
                        ("y".to_string(), mean.values.clone()),
                    ])
                    .layer_aes(Aes::new().x("x").y("y"))
                    // The mean dot is drawn by its own (real) GeomPoint
                    // layer purely to mark the spot the box's own tooltip
                    // already names: `GGPlot::decorative()` (ggplot-rs
                    // 0.22.2, #161) strips its title/data-* so it carries
                    // no mark metadata of its own, instead of the old
                    // mapped `data_attr("decorative", …)` web/app.js had to
                    // special-case.
                    .decorative(),
                None => p,
            }
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
        | Kind::Sparkbar
        | Kind::Waterfall
        | Kind::Bullet
        | Kind::Funnel => {
            unreachable!("handled above")
        }
    };
    // Intervals: a point measure becomes a pointrange; any other measure gets
    // capped error bars; an x interval is a horizontal error bar through y.
    // Neither makes sense once `::HEXBIN` has turned the points into a
    // density (its own match arm above already wins over this one).
    if y_interval.is_some() && hex_bins.is_none() {
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
                cap_width: cap_half_width(&col_of(&data, "x"), x_levels.as_deref(), dodge_width),
                alpha: 1.0,
            })
            .layer_aes(span)
        };
        if dodge {
            plot = plot.position(position_dodge(dodge_width));
        }
    }
    if x_interval.is_some() && hex_bins.is_none() {
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
            Kind::BarStacked | Kind::AreaStacked => plot.position(position_stack().with_vjust(0.5)),
            Kind::BarStackedPercent => plot.position(position_fill().with_vjust(0.5)),
            // Grouped (dodged) bars/points: each layer's position adjustment
            // is independent, so the label layer needs its own
            // position_dodge too — GeomText honouring it (ggplot-rs#318
            // B3-4) only means it *can* dodge, not that it inherits the
            // bars' own position() call.
            _ if dodge => plot.position(position_dodge(dodge_width)),
            _ => plot,
        };
    }
    // Combo layers: overlay each extra measure with its own geom + y column.
    for (k, ev) in extras.iter().enumerate() {
        if let Role::Value(ekind) = ev.role {
            let yk = format!("y{}", k + 2);
            let mut lay = Aes::new().x("x").y(&yk);
            if !combo_names.is_empty() {
                let s = format!("__s{}", k + 1);
                lay = if pure_bar_combo {
                    lay.fill(&s)
                } else {
                    lay.color(&s)
                };
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
    // coord_flip. A statement can carry more than one `::REFLINE` (or
    // `::XLINE`) column — e.g. an alert band's lower and upper edge — and
    // every one of them draws its own line(s) (#471; previously only the
    // first `::REFLINE` column was read via `find_role`, so the second was
    // silently dropped).
    let flipped = cols.iter().any(|c| c.role == Role::Flip);
    for (role, col, vertical) in [
        (Role::RefLine, "yintercept", false),
        (Role::VLine, "xintercept", true),
    ] {
        for c in cols.iter().filter(|c| c.role == role) {
            let vals: Vec<Value> = if vertical {
                xline_values(&c.values)
            } else {
                distinct_nums(&c.values)
                    .into_iter()
                    .map(Value::Float)
                    .collect()
            };
            if vals.is_empty() {
                continue;
            }
            let frame = vec![(col.to_string(), vals)];
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
                .layer_data(frame)
                // An `::XLINE`'s tooltip is just its value (`2026-10-12`), not
                // `x = 2026-10-12`: `geom_vline` now reads a `.tooltip()`
                // template (ggplot-rs 0.22.2, sipemu/ggplot-rs#165), replacing
                // its default `"x = …"` label, instead of the old
                // `without_xline_prefix` SVG string-surgery. `{xintercept}` is
                // formatted through the x scale (B2-4), so a `Value::DateTime`
                // refline still shows its date text, not a raw epoch number.
                // A `::REFLINE`/`::YLINE`'s default `"y = …"` label is left
                // untouched, as before #326.
                .tooltip("{xintercept}")
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
                .layer_data(frame)
            };
        }
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
    // `::HEXBIN` drops `::CATEGORY` (above): no colour column survives onto
    // any layer, so skip registering a colour/fill scale for it at all —
    // training an unused scale from no data would otherwise risk an empty
    // legend entry.
    if let Some(col) = color_col.filter(|_| hex_bins.is_none()) {
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
        // column header. A pure bar combo is coloured through `fill` (a
        // bar's own colour), everything else through `color` (#292).
        plot = if pure_bar_combo {
            plot.scale_fill(
                ScaleColorDiscrete::new(Aesthetic::Fill)
                    .with_levels(combo_names.clone())
                    .with_palette(dz_palette(o)),
            )
        } else {
            plot.scale_color(
                ScaleColorDiscrete::new(Aesthetic::Color)
                    .with_levels(combo_names.clone())
                    .with_palette(dz_palette(o)),
            )
        };
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
    // #663: a framed chart's axes span every frame — a layer that draws
    // nothing, with every frame's extreme points
    if let Some(ax) = &o.frame_axes {
        if !ax.x.is_empty() {
            plot = plot
                .geom_blank()
                .layer_data(vec![
                    ("x".to_string(), ax.x.clone()),
                    ("y".to_string(), ax.y.clone()),
                ])
                .layer_aes(Aes::new().x("x").y("y"));
        }
    }
    // `::YBREAKS` is given in the axis's own (pre-transform) data units —
    // e.g. `'100,1000,10000'` under `'log10'::YSCALE` — so it's
    // forward-transformed the same way the scale itself transforms data,
    // matching `ScaleContinuous::with_breaks`'s transformed-space contract.
    let y_breaks = role_breaks(cols, Role::YBreaks).map(|bs| match &y_tr {
        Some(t) => bs.iter().map(|&v| t.apply(v)).collect::<Vec<_>>(),
        None => bs,
    });
    // B2-4: the measure's own `data-fvalue` (`::YFORMAT`/a percent kind,
    // same locale) — wired unconditionally so ggplot-rs's native
    // `svg_formatted_values` (on since `Panel::plot`) writes exactly what
    // the old string-surgery `with_formatted_values` used to, byte for byte,
    // even in the plain (no `::YFORMAT`) case.
    let vfmt = value_fmt(cols, kind, &value.values);
    let locale = o.locale;
    {
        let mut s = ScaleContinuous::new();
        if let Some(f) = y_fmt {
            s = s.with_label_formatter(f);
        }
        if let Some(t) = y_tr {
            s = s.with_transform(t);
        }
        if let Some(bs) = y_breaks {
            s = s.with_breaks(bs);
        }
        s = s.with_value_formatter(move |n: f64| vfmt.full(n, &locale));
        plot = plot.scale_y_continuous(s);
    }
    // An x formatter/transform only makes sense on a continuous x (numeric/date),
    // not on the discrete category axis of a bar chart.
    // #663: a framed chart keeps every frame's levels, in order
    let x_levels = o
        .frame_axes
        .as_ref()
        .and_then(|a| a.x_levels.clone())
        .filter(|_| x_discrete)
        .or(x_levels);
    if let Some(levels) = &x_levels {
        plot = plot.scale_x_discrete(
            ScaleDiscrete::new().with_limits(levels.iter().map(String::as_str).collect()),
        );
    } else if !x_discrete {
        let x_fmt = role_fmt(cols, Role::XFormat).and_then(|s| axis_formatter(&s, &o.locale));
        let x_tr = role_str(cols, Role::XScale)
            .map(|s| axis_transform(&s, Role::XScale))
            .transpose()?;
        // See `y_breaks` above — same forward-transform contract.
        let x_breaks = role_breaks(cols, Role::XBreaks).map(|bs| match &x_tr {
            Some(t) => bs.iter().map(|&v| t.apply(v)).collect::<Vec<_>>(),
            None => bs,
        });
        // B2-4: `data-fx` for a custom `::XFORMAT` — the only case the old
        // pass ever set it for a numeric x (a date x already formats its
        // own `data-x`/`data-fx` natively through `ScaleDateTime`).
        let x_vfmt = role_fmt(cols, Role::XFormat).map(|s| format::ValueFmt::parse(&s));
        if x_fmt.is_some() || x_tr.is_some() || x_vfmt.is_some() || x_breaks.is_some() {
            let mut s = ScaleContinuous::new();
            if let Some(f) = x_fmt {
                s = s.with_label_formatter(f);
            }
            if let Some(t) = x_tr {
                s = s.with_transform(t);
            }
            if let Some(bs) = x_breaks {
                s = s.with_breaks(bs);
            }
            if let Some(vfmt) = x_vfmt {
                let locale = o.locale;
                s = s.with_value_formatter(move |n: f64| vfmt.full(n, &locale));
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
    // `::HEXBIN`'s "count" fill is a continuous colourbar, not a discrete
    // category row — it keeps ggplot-rs's own default legend position (a
    // right-hand colourbar), same as `render_heatmap`'s `legend_position_of`
    // handling, instead of the "Top" every other cartesian chart defaults
    // to: a tall vertical gradient bar squeezed into a short top strip
    // overlaps the panel instead of sitting beside it. Below
    // `NARROW_PANEL_PX` though, `Bottom` (not ggplot-rs's own `Right`
    // default) — same width-saving swap `render_heatmap`/`render_calendar`
    // make, now safe since ggplot-rs 0.31.1 draws a continuous bar at
    // `Bottom` horizontally instead of clipping (#510, #211). An explicit
    // `::LEGEND` still wins either way.
    plot = match (hex_bins, legend_position_of(cols)) {
        (Some(_), None) => match narrow_continuous_legend(width) {
            Some(pos) => plot.legend_position(pos),
            None => plot,
        },
        (_, pos) => plot.legend_position(pos.unwrap_or(ggplot_rs::theme::LegendPosition::Top)),
    };
    if let Some(t) = &title {
        plot = plot.title(t);
    }
    let panel = Panel::plot(plot, width, height);
    Ok(panel)
}

/// ggplot2's own histogram bin alignment: a bin is centered on 0
/// (`boundary = width/2`), so a plain (no `::FACET`/`::HIST_BINS`/
/// `::HIST_NORM`) `::HISTOGRAM` keeps the exact pixels it always has —
/// `render_histogram`'s own fast path leaves `GeomHistogram`'s bundled
/// `StatBin` to compute this. [`hist_edges`] replicates it (ggplot-rs's own
/// copy, `StatBin::aligned_bins`, is private) only when #423's new roles need
/// bins computed *before* the plot is built: shared across every
/// `::CATEGORY`/`::FACET` group, or as the denominator for `::HIST_NORM`.
fn hist_aligned_edges(min: f64, max: f64, bins: usize) -> Vec<f64> {
    let width = if bins <= 1 {
        max - min
    } else {
        (max - min) / (bins - 1) as f64
    };
    let width = if width > 0.0 { width } else { 1.0 };
    let boundary = width / 2.0;
    let shift = ((min - boundary) / width).floor();
    let origin = boundary + shift * width;
    let n = (((max - origin) / width).ceil() as usize).max(1);
    (0..=n).map(|i| origin + i as f64 * width).collect()
}

/// `::HIST_BINS`-aware bin edges, pooled over every row (every `::CATEGORY`
/// / `::FACET` group shares them, #423): a bare number is a bin count (like
/// ggplot2's default 30), comma-separated numbers with more than one value
/// are explicit edges (irregular widths allowed — `GeomHistogram`/`StatBin`
/// can't do this, so these are drawn as pre-aggregated bars, see
/// [`render_histogram`]).
fn hist_edges(values: &[Value], bins_spec: Option<&str>) -> Vec<f64> {
    let nums: Vec<f64> = values
        .iter()
        .filter_map(|v| v.as_f64())
        .filter(|f| f.is_finite())
        .collect();
    let (min, max) = nums
        .iter()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), &v| {
            (a.min(v), b.max(v))
        });
    let (min, max) = if !min.is_finite() || !max.is_finite() {
        (0.0, 1.0)
    } else if (max - min).abs() < f64::EPSILON {
        (min - 0.5, max + 0.5)
    } else {
        (min, max)
    };
    if let Some(spec) = bins_spec {
        let parts: Vec<f64> = spec
            .split(',')
            .filter_map(|t| t.trim().parse::<f64>().ok())
            .filter(|f| f.is_finite())
            .collect();
        if parts.len() > 1 {
            let mut edges = parts;
            edges.sort_by(f64::total_cmp);
            edges.dedup();
            if edges.len() > 1 {
                return edges;
            }
        } else if let [n] = parts[..] {
            return hist_aligned_edges(min, max, (n.round().max(1.0)) as usize);
        }
    }
    hist_aligned_edges(min, max, 30)
}

/// The bin `edges[i..i+1]` (0-based) a finite value falls in — right-closed
/// `(lo, hi]`, like ggplot2/`StatBin`; the lowest bin's left edge is closed
/// too, so the pooled minimum lands in bin 0 rather than nowhere. `edges` is
/// sorted ascending with at least 2 entries, possibly unequal widths
/// (explicit `::HIST_BINS` edges). `None` outside `[edges[0], edges[last]]`.
fn hist_bin_of(v: f64, edges: &[f64]) -> Option<usize> {
    if !v.is_finite() || v < edges[0] || v > *edges.last()? {
        return None;
    }
    if v == edges[0] {
        return Some(0);
    }
    edges
        .windows(2)
        .position(|w| v > w[0] && v <= w[1])
        .or(Some(edges.len() - 2))
}

/// A histogram of the measure column. `::CATEGORY` draws one bar series per
/// group, overlaid with alpha (#446 — ggplot-rs 0.28's `GeomHistogram` now
/// reads a per-row fill aesthetic, same as `GeomDensity`); `::FACET`/
/// `::FACET_FREE` (one panel per value, #423), `::HIST_BINS` (a bin count or
/// explicit edges) and `::HIST_NORM 'share'` (each group normalised to its
/// own share of 1, so reference/current periods of different sizes compare)
/// all work too, any combined. Without any of them, this takes ggplot-rs's
/// own `GeomHistogram`/`StatBin` unchanged (byte-identical output to before
/// #423); with any of them, bins are computed here first — pooled over every
/// row, so every group/panel shares the same edges — and drawn as
/// pre-aggregated bars (`StatIdentity`) so groups/panels/shares line up. No
/// `::POSITION` role exists yet for an explicit dodge; overlay (the default)
/// is the only layout `::CATEGORY` draws.
fn render_histogram(
    o: &RenderOptions,
    value: &Column,
    cols: &[Column],
    title: Option<&str>,
    width: u32,
    height: u32,
) -> Result<Panel, String> {
    let facet = cols
        .iter()
        .find(|c| matches!(c.role, Role::Facet | Role::FacetFree));
    let category = cols.iter().find(|c| c.role == Role::Category);
    let bins_spec = role_str(cols, Role::HistBins);
    let share = role_str(cols, Role::HistNorm).is_some_and(|s| s.eq_ignore_ascii_case("share"));

    if facet.is_none() && category.is_none() && bins_spec.is_none() && !share {
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
        return Ok(Panel::plot(plot, width, height));
    }

    let edges = hist_edges(&value.values, bins_spec.as_deref());
    let n_bins = edges.len() - 1;
    // Each (category, facet) group's own total (valid, in-range) row count,
    // for `::HIST_NORM`: a group's share is of its own count, not pooled
    // across categories or panels.
    let cat_of =
        |i: usize| -> String { category.map_or_else(String::new, |c| value_str(&c.values[i])) };
    let facet_of =
        |i: usize| -> String { facet.map_or_else(String::new, |f| value_str(&f.values[i])) };
    let group_of = |i: usize| -> (String, String) { (cat_of(i), facet_of(i)) };
    let mut counts: std::collections::HashMap<(String, String), Vec<f64>> =
        std::collections::HashMap::new();
    let mut totals: std::collections::HashMap<(String, String), f64> =
        std::collections::HashMap::new();
    for i in 0..value.values.len() {
        let Some(v) = value.values[i].as_f64().filter(|f| f.is_finite()) else {
            continue;
        };
        let Some(bin) = hist_bin_of(v, &edges) else {
            continue;
        };
        let g = group_of(i);
        counts.entry(g.clone()).or_insert_with(|| vec![0.0; n_bins])[bin] += 1.0;
        *totals.entry(g).or_insert(0.0) += 1.0;
    }
    // Groups in first-seen order (facet panels, and the category legend,
    // keep the SQL's row order).
    let mut groups: Vec<(String, String)> = Vec::new();
    for i in 0..value.values.len() {
        let g = group_of(i);
        if !groups.contains(&g) {
            groups.push(g);
        }
    }
    let (mut xmin, mut xmax, mut xmid, mut y, mut fac, mut cat) = (
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
    );
    for g in &groups {
        let cs = counts.get(g).cloned().unwrap_or_else(|| vec![0.0; n_bins]);
        let total = totals.get(g).copied().unwrap_or(0.0);
        for b in 0..n_bins {
            let (lo, hi) = (edges[b], edges[b + 1]);
            xmin.push(Value::Float(lo));
            xmax.push(Value::Float(hi));
            xmid.push(Value::Float((lo + hi) / 2.0));
            let raw = cs[b];
            y.push(Value::Float(if share && total > 0.0 {
                raw / total
            } else {
                raw
            }));
            cat.push(Value::Str(g.0.clone()));
            fac.push(Value::Str(g.1.clone()));
        }
    }
    let mut data: Vec<(String, Vec<Value>)> = vec![
        ("x".to_string(), xmid),
        ("xmin".to_string(), xmin),
        ("xmax".to_string(), xmax),
        ("y".to_string(), y),
    ];
    let mut aes = Aes::new().x("x").xmin("xmin").xmax("xmax").y("y");
    if category.is_some() {
        data.push(("cat".to_string(), cat));
        aes = aes.fill("cat");
    }
    if facet.is_some() {
        data.push(("facet".to_string(), fac));
    }
    let mut plot = GGPlot::new(data)
        .aes(aes)
        .geom_histogram_with(GeomHistogram {
            fill: o.brand(),
            color: o.theme.colors.card,
            // Overlaid groups need to show through each other; a single
            // series keeps the solid default (unused when fill isn't
            // data-mapped, since `GeomHistogram` only applies alpha, not a
            // flat tint, per row).
            alpha: if category.is_some() { 0.6 } else { 1.0 },
            ..Default::default()
        })
        .stat(StatIdentity)
        .house_theme(o)
        .primary_color(o.brand());
    if let Some(cat_col) = category {
        plot = plot
            .scale_fill(category_scale(o, Aesthetic::Fill, cat_col, cols))
            .legend_position(
                legend_position_of(cols).unwrap_or(ggplot_rs::theme::LegendPosition::Top),
            );
    }
    if let Some(f) = facet {
        let ncol = role_num(cols, Role::FacetCols)
            .filter(|n| *n >= 1.0)
            .map(|n| n.min(64.0) as usize)
            .or((f.role == Role::FacetFree).then_some(1));
        plot = if f.role == Role::FacetFree {
            plot.facet_wrap_free("facet", ncol, FacetScales::Free)
        } else {
            plot.facet_wrap("facet", ncol)
        };
    }
    if share {
        plot = plot.ylab("Share");
    }
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
            .scale_fill(category_scale(o, Aesthetic::Fill, cat, cols))
            .scale_color(category_scale(o, Aesthetic::Color, cat, cols))
            .house_theme(o)
            .legend_position(
                legend_position_of(cols).unwrap_or(ggplot_rs::theme::LegendPosition::Top),
            );
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
    Ok(Panel::plot(plot, width, height))
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
/// skew/heavy tails. Handy for checking a model's residuals. An optional
/// `::CATEGORY` draws one point series (plus its own reference line and 95%
/// band) per group — `geom_qq`/`geom_qq_line`/`geom_qq_band` each already
/// group by the `color`/`fill` aesthetic and re-run their stat per group, so
/// this is the same `fill("cat").color("cat")` idiom as `::DENSITY` (#423).
/// `::FACET`/`::FACET_FREE` draws one panel per group instead.
fn render_qq(
    o: &RenderOptions,
    value: &Column,
    cols: &[Column],
    title: Option<&str>,
    width: u32,
    height: u32,
) -> Result<Panel, String> {
    let category = cols.iter().find(|c| c.role == Role::Category);
    let facet = cols
        .iter()
        .find(|c| matches!(c.role, Role::Facet | Role::FacetFree));
    let mut data: Vec<(String, Vec<Value>)> = vec![("y".to_string(), value.values.clone())];
    let mut aes = Aes::new().y("y");
    if let Some(cat) = category {
        data.push(("cat".to_string(), cat.values.clone()));
        aes = aes.fill("cat").color("cat");
    }
    if let Some(f) = facet {
        data.push(("facet".to_string(), f.values.clone()));
    }
    // Points on a 95 % pointwise envelope around the quartile line (qqplotr).
    let mut plot = GGPlot::new(data)
        .aes(aes)
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
    if let Some(cat) = category {
        plot = plot
            .scale_fill(category_scale(o, Aesthetic::Fill, cat, cols))
            .scale_color(category_scale(o, Aesthetic::Color, cat, cols))
            .legend_position(
                legend_position_of(cols).unwrap_or(ggplot_rs::theme::LegendPosition::Top),
            );
    }
    if let Some(f) = facet {
        let ncol = role_num(cols, Role::FacetCols)
            .filter(|n| *n >= 1.0)
            .map(|n| n.min(64.0) as usize)
            .or((f.role == Role::FacetFree).then_some(1));
        plot = if f.role == Role::FacetFree {
            plot.facet_wrap_free("facet", ncol, FacetScales::Free)
        } else {
            plot.facet_wrap("facet", ncol)
        };
    }
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
/// `::FACET`/`::FACET_FREE` draws one tile grid per distinct value (#424,
/// e.g. a confusion matrix, reference vs. current): the fill scale (and so
/// the legend/colourbar) is still trained on every panel's rows together,
/// so panels stay visually comparable, exactly as a non-faceted heatmap's
/// colour scale already covers its whole `value` column.
fn render_heatmap(
    o: &RenderOptions,
    value: &Column,
    cols: &[Column],
    title: Option<&str>,
    width: u32,
    height: u32,
    domain: Option<(f64, f64)>,
) -> Result<Panel, String> {
    let x = cols
        .iter()
        .find(|c| c.role == Role::X)
        .ok_or("heatmap needs an XAXIS column")?;
    let y = cols
        .iter()
        .find(|c| c.role == Role::Y)
        .ok_or("heatmap needs a YAXIS column")?;
    let facet = cols
        .iter()
        .find(|c| matches!(c.role, Role::Facet | Role::FacetFree));
    // #332: a string measure (a status word such as 'blocked', coloured
    // with ::COLORS as a level map) gets a categorical fill scale and
    // legend — the words themselves, not a numeric ramp/::THRESHOLDS steps,
    // which need real numbers (the SQL planner no longer forces this column
    // to DOUBLE, see sql::item_role's Role::Value(Kind::Heatmap) case).
    if value.values.iter().any(|v| matches!(v, Value::Str(_))) {
        return render_heatmap_categorical(o, value, cols, title, width, height);
    }
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
    let (min, max) = domain.unwrap_or((min, max));
    let fill = TileFill::of(o, cols, o.theme.seq_low(), min, max)?;
    let tips: Vec<Value> = value
        .values
        .iter()
        .map(|v| match v.as_f64() {
            Some(f) => Value::Str(fmt.full(f, &o.locale)),
            None => Value::Na,
        })
        .collect();
    let (x_data, x_levels, x_thin) = heatmap_x_axis(&x.values, width);
    let mut data = vec![
        ("x".to_string(), x_data.clone()),
        ("y".to_string(), y.values.clone()),
        ("fill".to_string(), value.values.clone()),
        ("label".to_string(), tips),
    ];
    if let Some(f) = facet {
        data.push(("facet".to_string(), f.values.clone()));
    }
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
    if let Some(pos) = legend_position_of(cols) {
        plot = plot.legend_position(pos);
    } else if matches!(fill, TileFill::Ramp(_)) {
        // A continuous (gradient) fill, no `::LEGEND` override: on a
        // narrow panel, `Bottom` instead of ggplot-rs's own `Right`
        // default (#510) — see `narrow_continuous_legend`. A
        // `TileFill::Steps` legend is a short discrete list regardless of
        // width, so it keeps ggplot-rs's default unconditionally.
        if let Some(pos) = narrow_continuous_legend(width) {
            plot = plot.legend_position(pos);
        }
    }
    if let Some(levels) = &x_levels {
        let mut sx = ScaleDiscrete::new().with_limits(levels.iter().map(String::as_str).collect());
        if let Some(thin) = x_thin {
            sx = sx.with_labels(thin);
        }
        plot = plot.scale_x_discrete(sx);
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
            let mut layer_data = vec![
                ("x".to_string(), pick(&x_data)),
                ("y".to_string(), pick(&y.values)),
                (
                    "cell".to_string(),
                    rows.iter().map(|&i| Value::Str(short[i].clone())).collect(),
                ),
            ];
            // So `::FACET` still splits each ink colour's labels into their
            // own panel, not all of them into the first one.
            if let Some(f) = facet {
                layer_data.push(("facet".to_string(), pick(&f.values)));
            }
            plot = plot
                .geom_text_with(GeomText {
                    size,
                    color: ink,
                    ..Default::default()
                })
                .layer_data(layer_data)
                .layer_aes(Aes::new().x("x").y("y").label("cell"))
                .show_legend(false);
        }
    }
    if let Some(f) = facet {
        let ncol = role_num(cols, Role::FacetCols)
            .filter(|n| *n >= 1.0)
            .map(|n| n.min(64.0) as usize)
            .or((f.role == Role::FacetFree).then_some(1));
        plot = if f.role == Role::FacetFree {
            plot.facet_wrap_free("facet", ncol, FacetScales::Free)
        } else {
            plot.facet_wrap("facet", ncol)
        };
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

/// `render_heatmap`'s categorical branch (#332): a string `::HEATMAP`
/// measure gets [`category_scale`] — the same `::COLORS` level map /
/// default palette + legend a `::CATEGORY` column gets elsewhere — so the
/// legend prints the status words themselves (`blocked`, `QP-ready`, …)
/// instead of numeric band boundaries. `GeomTile`'s own fallback (no
/// `"label"` aesthetic given) uses the fill value as each tile's tooltip, so
/// the raw word is still there on hover; `::DataLabels` isn't drawn here —
/// it formats a number, which a status word isn't.
fn render_heatmap_categorical(
    o: &RenderOptions,
    value: &Column,
    cols: &[Column],
    title: Option<&str>,
    width: u32,
    height: u32,
) -> Result<Panel, String> {
    let x = find_role(cols, Role::X).ok_or("heatmap needs an XAXIS column")?;
    let y = find_role(cols, Role::Y).ok_or("heatmap needs a YAXIS column")?;
    let facet = cols
        .iter()
        .find(|c| matches!(c.role, Role::Facet | Role::FacetFree));
    let (x_data, x_levels, x_thin) = heatmap_x_axis(&x.values, width);
    let mut data = vec![
        ("x".to_string(), x_data),
        ("y".to_string(), y.values.clone()),
        ("fill".to_string(), value.values.clone()),
    ];
    if let Some(f) = facet {
        data.push(("facet".to_string(), f.values.clone()));
    }
    let mut th = chart_theme(o, 11.0);
    th.panel_grid_major = ggplot_rs::theme::ElementLine::blank();
    th.legend_key.color = Some(o.theme.colors.card);
    th.legend_key_width = 9.0;
    let plot = GGPlot::new(data)
        .aes(Aes::new().x("x").y("y").fill("fill"))
        .geom_tile_with(GeomTile {
            color: o.theme.colors.card,
            line_width: 2.0,
            ..Default::default()
        });
    let mut plot = plot
        .scale_fill(category_scale(o, Aesthetic::Fill, value, cols))
        .theme(th);
    if let Some(pos) = legend_position_of(cols) {
        plot = plot.legend_position(pos);
    }
    if let Some(levels) = &x_levels {
        let mut sx = ScaleDiscrete::new().with_limits(levels.iter().map(String::as_str).collect());
        if let Some(thin) = x_thin {
            sx = sx.with_labels(thin);
        }
        plot = plot.scale_x_discrete(sx);
    }
    if let Some(mut levels) = str_levels(&y.values) {
        levels.reverse();
        plot = plot.scale_y_discrete(
            ScaleDiscrete::new().with_limits(levels.iter().map(String::as_str).collect()),
        );
    }
    if let Some(f) = facet {
        let ncol = role_num(cols, Role::FacetCols)
            .filter(|n| *n >= 1.0)
            .map(|n| n.min(64.0) as usize)
            .or((f.role == Role::FacetFree).then_some(1));
        plot = if f.role == Role::FacetFree {
            plot.facet_wrap_free("facet", ncol, FacetScales::Free)
        } else {
            plot.facet_wrap("facet", ncol)
        };
    }
    if let Some(t) = title {
        plot = plot.title(t);
    }
    let mut panel = Panel::plot(plot, width, height);
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

/// The sorted distinct days of a date/timestamp column, each formatted the
/// way a reference line's date label is (`xline_date_text`) — `None` unless
/// every value is a `Value::DateTime` (so a plain numeric column keeps its
/// continuous axis).
fn date_levels(values: &[Value]) -> Option<Vec<String>> {
    let mut secs: Vec<i64> = Vec::new();
    for v in values {
        match v {
            Value::DateTime(s) => secs.push(*s),
            Value::Na => {}
            _ => return None,
        }
    }
    secs.sort_unstable();
    secs.dedup();
    (!secs.is_empty()).then(|| secs.into_iter().map(xline_date_text).collect())
}

/// Every Nth label kept, the rest blanked (""), so a `ScaleDiscrete`'s
/// `breaks()` still positions every grid column but a crowded axis only
/// *prints* a readable handful (`with_labels`, which `breaks()` checks
/// before falling back to the level string — an empty override prints
/// nothing, not the level's name). `None` when `levels` already fits
/// without thinning.
fn thin_labels(levels: &[String], max: usize) -> Option<Vec<String>> {
    if levels.len() <= max {
        return None;
    }
    let stride = levels.len().div_ceil(max);
    Some(
        levels
            .iter()
            .enumerate()
            .map(|(i, l)| {
                if i % stride == 0 {
                    l.clone()
                } else {
                    String::new()
                }
            })
            .collect(),
    )
}

/// #332: `::HEATMAP`'s x axis, discretized — one grid column per distinct
/// value, text in first-seen order or a date/timestamp sorted
/// chronologically — so `GeomTile` sizes every tile to its full column
/// (`width / breaks`) instead of ggplot2/ggplot-rs's own continuous-axis
/// `resolution()` (the smallest gap *anywhere in the whole column*): on a
/// date x with more than one `::YAXIS` row at different, interleaved
/// cadences, that global minimum can be much narrower than any one row's
/// own spacing, so a row's tiles fill only a fraction of the gap to their
/// neighbour instead of the whole cell. Returns the data column to actually
/// plot (text already *is* the label; a date/timestamp is replaced with its
/// formatted label, the same device the discrete `::YAXIS` already uses),
/// the axis's levels (`None`: a genuinely continuous numeric x, unchanged),
/// and — a date axis only, where "every distinct day" can be far more than
/// `width` (px) can label — the thinned labels to print instead of all of
/// them (a date label like `2026-09-01` runs ~70px at the axis font size;
/// below that many labels, this is `None` — all of them print, as before).
fn heatmap_x_axis(
    values: &[Value],
    width: u32,
) -> (Vec<Value>, Option<Vec<String>>, Option<Vec<String>>) {
    if let Some(levels) = str_levels(values) {
        return (values.to_vec(), Some(levels), None);
    }
    if let Some(levels) = date_levels(values) {
        let data = values
            .iter()
            .map(|v| match v {
                Value::DateTime(s) => Value::Str(xline_date_text(*s)),
                other => other.clone(),
            })
            .collect();
        let max_labels = ((width as f64 / 70.0).floor() as usize).max(2);
        let thinned = thin_labels(&levels, max_labels);
        return (data, Some(levels), thinned);
    }
    (values.to_vec(), None, None)
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
#[derive(Clone)]
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
        // `with_value_formatter` (ggplot-rs 0.22.2, sipemu/ggplot-rs#164)
        // gives ggplot-rs's own B2-4 `data-fvalue`/tooltip pass the panel's
        // format directly, the same hook every other continuous scale
        // already had — replacing the old `with_formatted_values` string-
        // surgery this heatmap/calendar fill used to need (its `do_fvalue`
        // check below is now scoped to the `TileFill::Steps` case, which
        // isn't one of the three gradient scales #164 covers).
        let value_fmt = fmt.clone();
        let mut inner = ggplot_rs::scale::gradient_n::ScaleColorGradientN::new(
            Aesthetic::Fill,
            ramp.stops.iter().map(|&(p, c)| (p, rgba(c))).collect(),
        )
        .with_value_formatter(move |v| value_fmt.full(v, &locale));
        // Fix the domain to `Ramp::min`/`Ramp::max` — the domain a `matrix`
        // heatmap's `domain :=` option asks for (e.g. a correlation's fixed
        // −1..1, even when the data itself spans a narrower range after
        // mirroring). `Squish` (not the default `Censor`) keeps a value
        // fractionally outside the domain (float noise in the common case,
        // where the ramp's own domain already matches the data) at the
        // nearest end colour rather than turning it grey (#324's pre-0.21
        // workaround trained the scale's own min/max instead, since
        // `ScaleColorGradientN` had no fixed-domain builder; this is the
        // same "never narrower than the ramp" intent through the native
        // API). A no-op for every other heatmap, whose ramp is already
        // built from that same data (`Ramp::new`/`Ramp::status`).
        if ramp.min.is_finite() && ramp.max.is_finite() && ramp.min <= ramp.max {
            inner = inner.with_limits(ramp.min, ramp.max).with_oob(Oob::Squish);
        }
        FmtGradient { inner, fmt, locale }
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
    fn format_value(&self, value: &Value) -> Option<String> {
        self.inner.format_value(value)
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
    if let Some(pos) = legend_position_of(cols) {
        plot = plot.legend_position(pos);
    } else if matches!(fill, TileFill::Ramp(_)) {
        // Same narrow-panel swap as `render_heatmap` — see
        // `narrow_continuous_legend` (#510).
        if let Some(pos) = narrow_continuous_legend(width) {
            plot = plot.legend_position(pos);
        }
    }
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
    // B2-4: the close's own `data-fvalue` (`::YFORMAT`), same as every other
    // measure — see `cartesian`'s matching comment.
    let vfmt = value_fmt(cols, Kind::Candlestick, &value.values);
    let locale = o.locale;
    plot = plot.scale_y_continuous(
        ggplot_rs::scale::continuous::ScaleContinuous::new()
            .with_value_formatter(move |n: f64| vfmt.full(n, &locale)),
    );
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
            ..Default::default()
        })
        .scale_x_discrete(
            ggplot_rs::scale::discrete::ScaleDiscrete::new()
                .with_limits(axes.iter().map(String::as_str).collect()),
        )
        .coord_radar_with(ggplot_rs::coord::radar::CoordRadar::new().radius_frac(0.86))
        .xlab("")
        .ylab("")
        .house_theme(o)
        .legend_position(legend_position_of(cols).unwrap_or(ggplot_rs::theme::LegendPosition::Top));
    if let Some(cat) = category {
        plot = plot
            .scale_color(category_scale(o, Aesthetic::Color, cat, cols))
            .scale_fill(category_scale(o, Aesthetic::Fill, cat, cols));
    }
    // B2-4: the measure's own `data-fvalue` (`::YFORMAT`) — see
    // `cartesian`'s matching comment.
    let vfmt = value_fmt(cols, Kind::Radar, &value.values);
    let locale = o.locale;
    plot = plot.scale_y_continuous(
        ggplot_rs::scale::continuous::ScaleContinuous::new()
            .with_value_formatter(move |n: f64| vfmt.full(n, &locale)),
    );
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
    cols: &[Column],
    width: u32,
    height: u32,
) -> Result<Panel, String> {
    let n = value.values.len();
    let xs: Vec<Value> = (0..n).map(|i| Value::Float(i as f64)).collect();
    let data = vec![
        ("x".to_string(), xs.clone()),
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

    // #457: a sparkline shows *shape*, not a value-vs-zero comparison, so the
    // y axis must scale to the series' own min/max (a small pad), never a
    // zero baseline — matching the browser's `cellSpark` (web/app.js) and
    // the KPI tile's own scaling before #419. `GeomArea::include_zero_
    // baseline` forces 0 into the trained domain unconditionally (right for
    // a real area chart, wrong here), so the fill-under-the-line is drawn
    // with `geom_ribbon` instead (band from the series' own minimum up to
    // the line) — a ribbon doesn't carry that zero-baseline behaviour, so
    // the y scale trains on the data alone.
    let (mn, mx) = value
        .values
        .iter()
        .filter_map(Value::as_f64)
        .filter(|f| f.is_finite())
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), v| {
            (a.min(v), b.max(v))
        });
    let floor = vec![Value::Float(if mn.is_finite() { mn } else { 0.0 }); n];
    let band = vec![
        ("x".to_string(), xs),
        ("ymin".to_string(), floor),
        ("ymax".to_string(), value.values.clone()),
    ];

    let plot = GGPlot::new(data)
        .aes(Aes::new().x("x").y("y"))
        .geom_ribbon_with(GeomRibbon { fill, alpha: 0.55 })
        .layer_data(band)
        .layer_aes(Aes::new().x("x").ymin("ymin").ymax("ymax"))
        .geom_line_with(GeomLine {
            color: steel,
            width: 2.6,
            alpha: 1.0,
            ..Default::default()
        })
        .geom_point_with(GeomPoint {
            size: 4.2,
            color: o.theme.color(2),
            alpha: 1.0,
            ..Default::default()
        })
        .layer_data(end) // restrict the point layer to just the endpoint
        .scale_y_continuous({
            // B2-4: the measure's own `data-fvalue` (`::YFORMAT`) — see
            // `cartesian`'s matching comment.
            let vfmt = value_fmt(cols, Kind::Sparkline, &value.values);
            let locale = o.locale;
            let mut scale = ggplot_rs::scale::continuous::ScaleContinuous::new()
                .with_expand(0.1, 0.0)
                .with_value_formatter(move |n: f64| vfmt.full(n, &locale));
            if mn.is_finite() && mx <= mn {
                // A constant (or single-point) series has no range to show:
                // centre a flat line in the panel with a visible `±pad`
                // window instead of collapsing the domain to zero width.
                let pad = if mn.abs() > 1e-9 {
                    mn.abs() * 0.05
                } else {
                    1.0
                };
                scale = scale.with_limits(mn - pad, mn + pad);
            }
            scale
        })
        .house_void(o);
    // Render at a compact width so strokes stay crisp when the inline
    // sparkline is scaled down into a narrow panel column.
    let w = width.min(240);
    // #457: never hand ggplot-rs a panel shorter than its own layout floor
    // (`SPARKLINE_MIN_H` — see its doc comment). The nested `<svg viewBox>`
    // this panel renders into is sized to whatever height we pass here, so
    // asking for less than the floor draws the line against a taller
    // virtual plot area than the visible box, clipping off whatever falls
    // below it — every dip, once the min-to-max y scale above stopped
    // flattening the line against a zero baseline. Every embedder (the KPI
    // tile, a table's `::SPARKLINE` cell) requests at least this floor too
    // and centres/pads its own smaller visible slot around it; this clamp
    // is the backstop for any caller that doesn't.
    let h = height.max(SPARKLINE_MIN_H.ceil() as u32);
    Ok(Panel::plot(plot, w, h))
}

/// A minimal inline bar histogram (no axes) — one series of bars, or two
/// overlaid (a muted reference behind the accent-coloured current), sharing
/// one y scale so bar heights stay comparable (#425).
///
/// `cols` carries one or two `Role::Value(Kind::Sparkbar)` columns (built by
/// the table-cell / KPI callers from `kpi::spark_values`/`spark_pairs`):
/// one → a single series; two → `[ref, cur]`, drawn ref-then-cur so cur
/// paints on top. `value` (`render_inner`'s own lookup) is always the
/// *first* such column, so the one-series case can read it directly.
fn render_sparkbar(
    o: &RenderOptions,
    value: &Column,
    cols: &[Column],
    width: u32,
    height: u32,
) -> Result<Panel, String> {
    let series: Vec<&Column> = cols
        .iter()
        .filter(|c| c.role == Role::Value(Kind::Sparkbar))
        .collect();
    let (ref_vals, cur_vals): (&[Value], &[Value]) = if series.len() >= 2 {
        (&series[0].values, &series[1].values)
    } else {
        (&[], &value.values)
    };
    let xs = |len: usize| -> Vec<Value> { (0..len).map(|i| Value::Float(i as f64)).collect() };

    // Train the shared y scale on the combined ref+cur values so one bar
    // series never dwarfs or is dwarfed by the other's own scale. `x` is
    // built to the same (doubled) length as `combined` — every column of a
    // base [`GGData`] frame must have the same row count.
    let mut combined = cur_vals.to_vec();
    combined.extend_from_slice(ref_vals);
    let mut combined_x = xs(cur_vals.len());
    combined_x.extend_from_slice(&xs(ref_vals.len()));
    let base_data = vec![("x".to_string(), combined_x), ("y".to_string(), combined)];

    let accent = o.brand();
    let muted = wash(o, o.theme.colors.ink, 0.72); // low-contrast reference bars

    let mut plot = GGPlot::new(base_data).aes(Aes::new().x("x").y("y"));
    let two_series = !ref_vals.is_empty();

    if two_series {
        let ref_data = vec![
            ("x".to_string(), xs(ref_vals.len())),
            ("y".to_string(), ref_vals.to_vec()),
            (
                "label".to_string(),
                vec![Value::Str("ref".into()); ref_vals.len()],
            ),
        ];
        plot = plot
            .geom_col_with(GeomCol {
                fill: muted,
                color: muted,
                alpha: 0.45,
                width: 0.75,
            })
            .layer_data(ref_data)
            .layer_aes(Aes::new().x("x").y("y").label("label"))
            .position(PositionIdentity);
    }

    let cur_data = vec![
        ("x".to_string(), xs(cur_vals.len())),
        ("y".to_string(), cur_vals.to_vec()),
        (
            "label".to_string(),
            vec![Value::Str(if two_series { "cur" } else { "" }.into()); cur_vals.len()],
        ),
    ];
    plot = plot
        .geom_col_with(GeomCol {
            fill: accent,
            color: accent,
            alpha: 1.0,
            width: if two_series { 0.45 } else { 0.75 },
        })
        .layer_data(cur_data)
        .layer_aes(Aes::new().x("x").y("y").label("label"))
        .position(PositionIdentity)
        .scale_y_continuous({
            let vfmt = value_fmt(cols, Kind::Sparkbar, cur_vals);
            let locale = o.locale;
            ggplot_rs::scale::continuous::ScaleContinuous::new()
                .with_expand(0.1, 0.0)
                .with_value_formatter(move |n: f64| vfmt.full(n, &locale))
        })
        .house_void(o);
    // Same compact-width clamp as render_sparkline, for the same reason.
    let w = width.min(240);
    // Same `SPARKLINE_MIN_H` floor render_sparkline clamps to (#457): the
    // 50px layout floor `render/layout.rs` enforces is generic to every
    // ggplot-rs panel, `GeomCol` included, not just a line geom — ask for
    // less and the bars' own baseline (the bottom of the virtual plot area,
    // sized against the 50px floor) falls outside the smaller requested
    // viewBox and gets clipped, same class of bug, every bar floating with
    // its bottom cut off instead of sitting on the baseline (#425).
    let h = height.max(SPARKLINE_MIN_H.ceil() as u32);
    Ok(Panel::plot(plot, w, h))
}

/// Finish a [`Panel`] built outside the usual `cols`-driven pipeline
/// (#463's mini-chart renderers, below): the same `localized`/
/// `with_motion_attrs`/`finish`/[`finish_touches`] steps
/// [`render_placed`] runs, with an empty `cols` — these charts have no
/// `::ROLE` cast to carry a locale-affecting x axis or a motion-attr
/// role, and no `::YFORMAT`-style custom formatter to thread through
/// `finish_touches`'s `with_formatted_values`.
fn finish_mini(panel: Panel, o: &RenderOptions, place: Place) -> Result<String, String> {
    let (svg, _warnings) = panel
        .localized(o, &[])
        .with_motion_attrs(&[])
        .finish(place)?;
    Ok(finish_touches(svg, &[], o))
}

/// Shared by the #463 "mini chart" wasm exports (`render_mini_hist`,
/// `render_mini_overlay`) — not reachable from SQL (no `::ROLE`): these
/// draw app-chrome widgets built from data the browser already has
/// client-side (a column's own histogram bin counts, computed by Explore
/// data's profiler or the table's own column-stats row), not a dashboard
/// query result, so there's no `Column`/`Role` to route through
/// `render_inner`'s dispatch. One bar per value, `x` = its index.
/// `labels[i]`, if given, becomes bar `i`'s own hover tip prefix
/// (`"{label}: {value}"`, ggplot-rs's own `GeomCol` tip format) — e.g. a
/// histogram's bin range; `None` leaves every bar with just its bare
/// value (ggplot-rs's default). `alpha` is the fill's own opacity (`0.0`
/// draws an outline only — the stroke is unaffected by `fill-opacity`,
/// ggplot-rs's `RectStyle`/`svg_backend::rect_markup`), so the same
/// building block covers a single always-filled histogram (xpMini,
/// st-hist, #463) and (via [`render_mini_overlay`]) a filled B over an
/// outline-only A.
fn mini_hist_plot(
    values: &[f64],
    labels: Option<&[String]>,
    fill: (u8, u8, u8),
    stroke: (u8, u8, u8),
    alpha: f64,
    bar_width: f64,
) -> GGPlot {
    let n = values.len();
    let xs: Vec<Value> = (0..n).map(|i| Value::Float(i as f64)).collect();
    let mut data = vec![
        ("x".to_string(), xs),
        (
            "y".to_string(),
            values.iter().map(|&v| Value::Float(v)).collect(),
        ),
    ];
    let mut aes = Aes::new().x("x").y("y");
    if let Some(labels) = labels {
        data.push((
            "label".to_string(),
            labels.iter().cloned().map(Value::Str).collect(),
        ));
        aes = aes.label("label");
    }
    GGPlot::new(data).aes(aes).geom_col_with(GeomCol {
        fill,
        color: stroke,
        alpha,
        width: bar_width,
    })
}

/// A pre-binned histogram (the caller already computed the bin counts) —
/// Explore data's card histogram (`xpMini`) and the table's own
/// column-profile row (`st-hist`), #463. `labels_json`, if not `""`, is a
/// JSON array of per-bar hover-tip text, same length as `values` (a bin's
/// own range, e.g. `"10 – 20"`); `fill_alpha` matches each caller's own
/// previous hand-drawn look (`st-hist`: `0.55`, translucent;`xpMini`:
/// `1.0`, opaque).
pub fn render_mini_hist(
    values_json: &str,
    labels_json: &str,
    fill_alpha: f64,
    width: u32,
    height: u32,
    primary: &str,
) -> Result<String, RenderError> {
    guard(|| {
        let o = RenderOptions::from_primary_arg(primary).map_err(RenderError::BadSpec)?;
        let values: Vec<f64> = serde_json::from_str(values_json)
            .map_err(|e| RenderError::BadSpec(format!("values JSON: {e}")))?;
        let labels: Option<Vec<String>> = if labels_json.is_empty() {
            None
        } else {
            Some(
                serde_json::from_str(labels_json)
                    .map_err(|e| RenderError::BadSpec(format!("labels JSON: {e}")))?,
            )
        };
        let accent = o.brand();
        let plot = mini_hist_plot(&values, labels.as_deref(), accent, accent, fill_alpha, 0.86)
            .scale_y_continuous(
                ggplot_rs::scale::continuous::ScaleContinuous::new().with_expand(0.05, 0.0),
            )
            .house_void(&o);
        // Same compact-width clamp as render_sparkbar, for the same reason
        // (crisp strokes once scaled down) — a higher cap than the fixed
        // table-spark cells' 240: both callers (a fluid-width Explore card,
        // the table's own fluid stats row) can legitimately ask for more.
        let w = width.min(400);
        let h = height.max(SPARKLINE_MIN_H.ceil() as u32);
        let svg = finish_mini(Panel::plot(plot, w, h), &o, Place::At(0.0, 0.0))
            .map_err(RenderError::Render)?;
        Ok(svg)
    })
}

/// Two pre-binned histograms on one shared y scale, A as an outline, B
/// filled — Explore data's distribution-comparison overlay (`xpOverlay`),
/// #463. No per-bar labels (the caller's own single whole-svg
/// `aria-label` already names the comparison).
pub fn render_mini_overlay(
    a_json: &str,
    b_json: &str,
    width: u32,
    height: u32,
    primary: &str,
) -> Result<String, RenderError> {
    guard(|| {
        let o = RenderOptions::from_primary_arg(primary).map_err(RenderError::BadSpec)?;
        let a: Vec<f64> = serde_json::from_str(a_json)
            .map_err(|e| RenderError::BadSpec(format!("a JSON: {e}")))?;
        let b: Vec<f64> = serde_json::from_str(b_json)
            .map_err(|e| RenderError::BadSpec(format!("b JSON: {e}")))?;
        let xs = |len: usize| -> Vec<Value> { (0..len).map(|i| Value::Float(i as f64)).collect() };
        // Train the shared y scale on both series, same reasoning as
        // render_sparkbar's own combined-training base layer (#425): A
        // never dwarfs, or is dwarfed by, B's own scale.
        let mut combined: Vec<Value> = b.iter().map(|&v| Value::Float(v)).collect();
        combined.extend(a.iter().map(|&v| Value::Float(v)));
        let mut combined_x = xs(b.len());
        combined_x.extend(xs(a.len()));
        let base_data = vec![("x".to_string(), combined_x), ("y".to_string(), combined)];

        let accent = o.brand();
        let ink = o.theme.colors.ink;

        // B first (filled, underneath), A second (outline, on top) — the
        // same paint order the hand-drawn version used (`bars(h.b) +
        // bars(h.a)`), so A's outline stays crisp over B's fill where the
        // two overlap.
        let b_data = vec![
            ("x".to_string(), xs(b.len())),
            (
                "y".to_string(),
                b.iter().map(|&v| Value::Float(v)).collect(),
            ),
        ];
        let a_data = vec![
            ("x".to_string(), xs(a.len())),
            (
                "y".to_string(),
                a.iter().map(|&v| Value::Float(v)).collect(),
            ),
        ];
        let plot = GGPlot::new(base_data)
            .aes(Aes::new().x("x").y("y"))
            .geom_col_with(GeomCol {
                fill: accent,
                color: accent,
                alpha: 0.55,
                width: 0.86,
            })
            .layer_data(b_data)
            .layer_aes(Aes::new().x("x").y("y"))
            .position(PositionIdentity)
            .geom_col_with(GeomCol {
                fill: ink,
                color: ink,
                alpha: 0.0,
                width: 0.86,
            })
            .layer_data(a_data)
            .layer_aes(Aes::new().x("x").y("y"))
            .position(PositionIdentity)
            .scale_y_continuous(
                ggplot_rs::scale::continuous::ScaleContinuous::new().with_expand(0.05, 0.0),
            )
            .house_void(&o);
        let w = width.min(400);
        let h = height.max(SPARKLINE_MIN_H.ceil() as u32);
        let svg = finish_mini(Panel::plot(plot, w, h), &o, Place::At(0.0, 0.0))
            .map_err(RenderError::Render)?;
        Ok(svg)
    })
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
                    ..Default::default()
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

    // Reference-line labels above the line's right end. A statement can
    // carry more than one `::REFLINE` column (#471); a single
    // `::REFLINE_LABEL` column labels every one of them, and several label
    // columns pair with the `::REFLINE` columns by position (same rule as
    // `::BAND_LOWER`/`::BAND_UPPER`, see `band_pairs`) — the trailing,
    // unpaired lines (more lines than labels) draw without a label.
    let ref_lines: Vec<&Column> = cols.iter().filter(|c| c.role == Role::RefLine).collect();
    let ref_labels: Vec<&Column> = cols
        .iter()
        .filter(|c| c.role == Role::RefLineLabel)
        .collect();
    if !ref_lines.is_empty() && !ref_labels.is_empty() {
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
        for (li, line) in ref_lines.iter().enumerate() {
            let lab = if ref_labels.len() == 1 {
                Some(ref_labels[0])
            } else {
                ref_labels.get(li).copied()
            };
            let Some(lab) = lab else { continue };
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

/// An `::XLINE`'s distinct positions, in first-seen order (capped like
/// [`distinct_nums`]): a date / timestamp (`DATE '2026-06-30'::XLINE`,
/// "Today" on a date axis) keeps its [`Value::DateTime`] rather than
/// collapsing to a plain number, so ggplot-rs's own reference-line code
/// (`geom_vline`'s `mark_line`, via [`Value::as_f64`] for the position but
/// [`Value::is_datetime`]-aware formatting for the tooltip and `data-x`)
/// draws it as the date it was given (`2026-06-30`), not `x = 1791763200` —
/// a plain number stays [`Value::Float`].
fn xline_values(vals: &[Value]) -> Vec<Value> {
    let mut seen = std::collections::HashSet::new();
    let mut out: Vec<Value> = Vec::new();
    for v in vals {
        let parsed = match v {
            Value::DateTime(secs) => Some(Value::DateTime(*secs)),
            Value::Str(s) => {
                let s = s.trim();
                match s.parse::<f64>() {
                    Ok(f) => Some(Value::Float(f)),
                    Err(_) => sql::parse_iso_epoch(s).map(Value::DateTime),
                }
            }
            v => v.as_f64().map(Value::Float),
        };
        let Some(pv) = parsed.filter(|v| v.as_f64().is_some_and(f64::is_finite)) else {
            continue;
        };
        let f = pv.as_f64().expect("just checked finite");
        let key = if f == 0.0 { 0u64 } else { f.to_bits() };
        if seen.insert(key) {
            out.push(pv);
            if out.len() >= 64 {
                break;
            }
        }
    }
    out
}

/// [`xline_values`]'s positions as plain numbers (epoch seconds for a date),
/// for callers that only need the number (e.g. a `'reveal'::ANIMATE` start).
fn xline_nums(vals: &[Value]) -> Vec<f64> {
    xline_values(vals)
        .iter()
        .filter_map(Value::as_f64)
        .collect()
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
                ..Default::default()
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
            ..Default::default()
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
        .scale_fill(category_scale(o, Aesthetic::Fill, category, cols))
        // No y-axis padding, so the stack maps to a full 360° (closes the
        // pie). B2-4: the slice's own `data-fvalue` (`::YFORMAT`) — see
        // `cartesian`'s matching comment.
        .scale_y_continuous({
            let kind = match value.role {
                Role::Value(k) => k,
                _ => Kind::Pie,
            };
            let vfmt = value_fmt(cols, kind, &value.values);
            let locale = o.locale;
            ggplot_rs::scale::continuous::ScaleContinuous::new()
                .with_expand(0.0, 0.0)
                .with_value_formatter(move |n: f64| vfmt.full(n, &locale))
        })
        .coord_polar_with(
            ggplot_rs::coord::polar::CoordPolar::new()
                .theta("y")
                .inner_radius(inner)
                .expand(false),
        )
        .house_void(o)
        .legend_position(legend_position_of(cols).unwrap_or(ggplot_rs::theme::LegendPosition::Top));
    if let Some(t) = title {
        plot = plot.title(t);
    }
    Ok(Panel::plot(plot, width, height))
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

/// The column's single value as text, or `None` if the role isn't present.
fn column_text(cols: &[Column], role: Role) -> Option<String> {
    find_role(cols, role)?.values.first().map(value_str)
}

/// A static `::MARKDOWN` panel on this render path (#568: `render_spec` /
/// `serve.rs`'s script-free page previously had no `::MARKDOWN` branch at
/// all here and fell through to an empty title-only heading — see the
/// `::IMAGE` comment below, which already anticipated this gap). Reuses
/// `dashboard.rs`'s own composer's text machinery (`markdown_text` /
/// `markdown_fill` / `markdown_lines` / `markdown_line_svg`) rather than a
/// second SVG text writer. No title drawn here: `render_with_title` /
/// `panel_title` draws the title bar separately, same as the `::IMAGE` /
/// KPI / table panels in this file.
fn markdown_panel_svg(cols: &[Column], width: u32, height: u32, o: &RenderOptions) -> Panel {
    let rows: Vec<(String, Vec<Value>)> = cols
        .iter()
        .map(|c| (c.name.clone(), c.values.clone()))
        .collect();
    let roles: Vec<(usize, Role)> = cols.iter().enumerate().map(|(i, c)| (i, c.role)).collect();
    let Some(raw) = dashboard::markdown_text(&rows, &roles) else {
        return Panel::own(width, height, String::new());
    };
    let md = dashboard::markdown_fill(&raw, &rows, &o.locale);
    let t = &o.theme;
    let pad = 14.0;
    let mut body = format!(
        "<g class=\"md-text\" font-family=\"{}\" font-size=\"13\" fill=\"{}\">",
        format::escape_xml(&t.font.family),
        theme::hex(t.colors.ink)
    );
    let mut y = 22.0;
    // #601 review S1: a fresh per-panel budget, same as dashboard.rs's own
    // render_panel_svg — resolves (reads/decodes, builds the href) only as
    // many of this panel's own block images as the byte cap and its own
    // height both allow.
    let mut img_budget = dashboard::MD_IMAGE_BUDGET_BYTES;
    for line in dashboard::markdown_lines(&md, width as f64) {
        if y > height as f64 - 8.0 {
            break;
        }
        match line {
            dashboard::MdLine::Text(text) => {
                if !text.is_empty() {
                    body.push_str(&format!(
                        "<text x=\"{pad}\" y=\"{y:.0}\">{}</text>",
                        dashboard::markdown_line_svg(&text)
                    ));
                }
                y += dashboard::MD_LINE;
            }
            // #524: mirrors dashboard.rs's own render_panel_svg handling of
            // a block markdown image — resolved lazily, right here, only
            // now that it's known to actually need drawing (#601 S1).
            dashboard::MdLine::Image { src, alt } => {
                let avail_w = width as f64 - 2.0 * pad;
                match dashboard::md_image(&src, o.asset_root.as_deref(), &mut img_budget) {
                    Some((href, aspect)) => {
                        match dashboard::markdown_image_height(
                            aspect,
                            avail_w,
                            height as f64 - y - 8.0,
                        ) {
                            Some(ih) => {
                                body.push_str(&dashboard::markdown_image_svg(
                                    &href, &alt, pad, y, avail_w, ih,
                                ));
                                y += ih + dashboard::MD_IMAGE_GAP; // #601 N5
                            }
                            None => break,
                        }
                    }
                    None => {
                        body.push_str(&format!(
                            "<text x=\"{pad}\" y=\"{y:.0}\">{}</text>",
                            dashboard::markdown_line_svg(&format!("[{alt}]"))
                        ));
                        y += dashboard::MD_LINE;
                    }
                }
            }
        }
    }
    body.push_str("</g>");
    Panel::own(width, height, body)
}

/// A static `::IMAGE` panel on this render path (#514: `render_spec` /
/// `serve.rs`'s script-free page), mirroring `dashboard.rs`'s own `::IMAGE`
/// branch: a `data:`/`https:` source is drawn through `<image href>`; a
/// relative path or a rejected source draws the alt-text placeholder
/// (the script-free page doesn't yet serve `assets/…` — PR B) rather than
/// the generic "panel render failed" card its caller would otherwise show.
/// No title bar or caption here: this render path draws neither for any
/// panel kind (its caller, `render_with_title`, already draws one bar from
/// `::LABEL`, above whatever this returns).
fn image_panel_svg(cols: &[Column], width: u32, height: u32, o: &RenderOptions) -> Panel {
    use crate::image::{resolve_relative, src_ok, SrcKind};
    let dark = o.theme.variant == theme::Variant::Dark;
    let src = (dark.then(|| column_text(cols, Role::ImageDark)).flatten())
        .or_else(|| column_text(cols, Role::Image))
        .filter(|s| !s.trim().is_empty());
    let alt = column_text(cols, Role::Alt)
        .or_else(|| column_text(cols, Role::Title))
        .or_else(|| column_text(cols, Role::Label))
        .unwrap_or_default();
    let (w, h) = (width as f64, height as f64);
    let pad = 8.0;
    let iw = (w - 2.0 * pad).max(1.0);
    let ih = (h - 2.0 * pad).max(1.0);
    // A `data:`/`https:` source is used as-is; a relative one (PR B) is
    // inlined from `o.asset_root`'s `assets/` folder as a `data:` URI —
    // missing, refused, or no root at all draws the alt-text placeholder,
    // same as a rejected source.
    let href = match src.as_deref().map(src_ok) {
        Some(SrcKind::Data(_)) | Some(SrcKind::Https) => src.clone(),
        Some(SrcKind::Relative) => src
            .as_deref()
            .and_then(|s| resolve_relative(s, o.asset_root.as_deref()))
            .map(|(mime, bytes)| crate::host::app_brand::data_uri(&mime, &bytes)),
        _ => None,
    };
    let body = match href {
        Some(href) => {
            let alt_title = if alt.is_empty() {
                String::new()
            } else {
                format!("<title>{}</title>", format::escape_xml(&alt))
            };
            format!(
                "<image href=\"{}\" x=\"{pad:.1}\" y=\"{pad:.1}\" width=\"{iw:.1}\" height=\"{ih:.1}\" preserveAspectRatio=\"xMidYMid meet\">{alt_title}</image>",
                format::escape_xml(&href),
            )
        }
        None => {
            let label = if alt.is_empty() { "image" } else { &alt };
            format!(
                "<rect x=\"{pad:.1}\" y=\"{pad:.1}\" width=\"{iw:.1}\" height=\"{ih:.1}\" fill=\"none\" stroke=\"{muted}\" stroke-dasharray=\"4 3\" rx=\"4\"/><text x=\"{cx:.1}\" y=\"{cy:.1}\" text-anchor=\"middle\" dominant-baseline=\"middle\" font-family=\"system-ui,sans-serif\" font-size=\"12\" font-style=\"italic\" fill=\"{muted}\">{}</text>",
                format::escape_xml(label),
                muted = theme::SVG_NOTE,
                cx = w / 2.0,
                cy = h / 2.0,
            )
        }
    };
    Panel::own(width, height, body)
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
