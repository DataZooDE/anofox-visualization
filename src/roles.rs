//! The role registry — the **single source of truth** for the `::ROLE`
//! vocabulary.
//!
//! Every consumer derives from [`REGISTRY`]: [`crate::parse_role`] (token →
//! [`Role`]), the SQL rewriter's "is this `::TOKEN` a role?" check, the wasm
//! `plan()` output ([`Role::token`]), the static renderers' / linter's "does
//! this statement draw a panel?" check ([`Role::renders`]), `dashboard --roles`
//! ([`text`]), the browser's role sets ([`roles_json`], exported to wasm as
//! `roles_json()`), and the role table in `docs/DOCS.md` ([`markdown_table`]; a
//! test keeps the doc in sync).
//!
//! Each entry lists a canonical token (what [`Role::token`] emits — the wire
//! format the browser matches on), its aliases, the [`Role`] it parses to, a
//! category, whether a statement carrying it renders a visible panel, and a
//! one-line description.

use crate::{DownloadFmt, InputKind, Kind, MetricFmt, Role, Scale, TableOpt, TextSize};

/// One registered role.
#[derive(Debug)]
pub struct RoleSpec {
    /// Canonical token (the form [`Role::token`] returns).
    pub token: &'static str,
    /// Accepted alternative spellings.
    pub aliases: &'static [&'static str],
    pub role: Role,
    /// `encoding` | `chart` | `annotation` | `modifier` | `kpi` | `table` |
    /// `input` | `layout` | `chrome`.
    pub category: &'static str,
    /// `true` when a statement carrying this role draws a visible panel;
    /// `false` for directives (layout, inputs, downloads, reload, chrome) that
    /// static renderers skip.
    pub renders: bool,
    pub summary: &'static str,
}

/// Back-compat name for the old catalog entry type.
pub type RoleDoc = RoleSpec;

const fn e(
    token: &'static str,
    aliases: &'static [&'static str],
    role: Role,
    category: &'static str,
    renders: bool,
    summary: &'static str,
) -> RoleSpec {
    RoleSpec {
        token,
        aliases,
        role,
        category,
        renders,
        summary,
    }
}

use Role as R;

/// The registry. Order = documentation order (grouped by category).
#[rustfmt::skip]
pub static REGISTRY: &[RoleSpec] = &[
    // ── encoding ───────────────────────────────────────────────────────────
    e("XAXIS", &["X"], R::X, "encoding", true, "x position"),
    e("YAXIS", &["Y"], R::Y, "encoding", true, "y position (a heatmap's second axis)"),
    e("CATEGORY", &["SERIES", "COLOR", "COLOUR"], R::Category, "encoding", true,
      "grouping / colour series (discrete)"),
    e("LABEL", &[], R::Label, "encoding", true,
      "section heading when alone; chart title / per-mark or per-feature label otherwise"),
    e("TITLE", &["HEADING"], R::Title, "encoding", true, "a title bar above one panel"),
    e("ALT", &["ALT_TEXT"], R::Alt, "encoding", true,
      "an image's text alternative ('' = decorative); falls back to ::TITLE, then ::LABEL"),
    e("INFO", &["DESCRIPTION", "DEFINITION"], R::Info, "encoding", true,
      "a definition behind an ⓘ on the panel's title bar (hover, focus or click)"),
    e("OPEN", &[], R::Open, "encoding", true, "candlestick open price"),
    e("HIGH", &[], R::High, "encoding", true, "candlestick high price"),
    e("LOW", &[], R::Low, "encoding", true, "candlestick low price"),
    e("SIZE", &[], R::Size, "encoding", true, "bubble size for a scatter (maps a measure to point area); on a map a symbol's area (graduated symbols) or a flow's width; a constant (4::SIZE) sets the point size"),
    e("Q1", &["QUARTILE1", "LOWER_QUARTILE"], R::Q1, "encoding", true,
      "a box plot from statistics, one row per box: the lower quartile (with ::MEDIAN and ::Q3; no ::BOXPLOT)"),
    e("MEDIAN", &["Q2"], R::Median, "encoding", true, "the median line of a box from statistics (see ::Q1)"),
    e("Q3", &["QUARTILE3", "UPPER_QUARTILE"], R::Q3, "encoding", true,
      "the upper quartile of a box from statistics (see ::Q1)"),
    e("WHISKER_LOW", &["WHISKER_LOWER", "LOWER_WHISKER"], R::WhiskerLow, "encoding", true,
      "the lower whisker end of a box from statistics (without it: none)"),
    e("WHISKER_HIGH", &["WHISKER_UPPER", "UPPER_WHISKER"], R::WhiskerHigh, "encoding", true,
      "the upper whisker end of a box from statistics"),
    e("MEAN", &["AVERAGE"], R::Mean, "encoding", true, "a box's mean from statistics, drawn as a dot"),
    e("OUTLIERS", &["OUTLIER_LIST"], R::Outliers, "encoding", true,
      "a box's outliers from statistics: a list of numbers, drawn as open circles"),
    e("YMIN", &["Y_MIN"], R::YMin, "encoding", true,
      "lower end of a y interval: a ::SCATTER becomes a pointrange, other measures get error bars"),
    e("YMAX", &["Y_MAX"], R::YMax, "encoding", true, "upper end of a y interval (see ::YMIN)"),
    e("XMIN", &["X_MIN"], R::XMin, "encoding", true,
      "lower end of a horizontal interval through each point"),
    e("XMAX", &["X_MAX"], R::XMax, "encoding", true, "upper end of a horizontal interval (see ::XMIN)"),
    e("FACET", &["FACET_WRAP", "PANEL_BY"], R::Facet, "encoding", true,
      "small multiples: one panel per distinct value, shared axes; on a map: one map per value, sharing projection, extent, classes and one legend"),
    e("FACET_FREE", &["FACET_WRAP_FREE"], R::FacetFree, "encoding", true,
      "small multiples with independent axes per panel (y only on a discrete x)"),
    e("RANK", &["LABEL_RANK"], R::Rank, "encoding", true,
      "score ranking points for ::LABEL_TOP (higher = labelled first)"),
    e("CENSOR", &["CENSORED", "N_CENSOR"], R::Censor, "encoding", true,
      "censoring marks (+) on a ::STEP curve where the value is > 0 / true (Kaplan-Meier)"),
    e("UPSTREAM", &["PARENTS"], R::Upstream, "encoding", true,
      "a ::DIAGRAM node's input(s): an id or a list of ids → edges into the row's node"),
    e("LANE", &["SWIMLANE"], R::Lane, "encoding", true,
      "a ::DIAGRAM node's swimlane: nodes of one lane share a labelled band"),
    e("EDGES", &[], R::Edges, "encoding", true,
      "a ::DIAGRAM's edges: (SELECT list({'from': a, 'to': b, 'value': n, 'label': t, 'style': 'flow', 'status': 'delayed'}) FROM e) — width by value, colour by status"),
    // ── charts (cast the measure column) ───────────────────────────────────
    e("BARCHART", &["BAR"], R::Value(Kind::Bar), "chart", true,
      "bar chart (dodged by CATEGORY)"),
    e("BARCHART_STACKED", &["BAR_STACKED", "STACKED_BAR"], R::Value(Kind::BarStacked), "chart", true,
      "stacked bars (by CATEGORY)"),
    e("BARCHART_PERCENT", &["BAR_PERCENT"], R::Value(Kind::BarPercent), "chart", true,
      "dodged bars as % of each x's total (fractions ≤ 1 are drawn as given)"),
    e("BARCHART_STACKED_PERCENT", &["BAR_STACKED_PERCENT"], R::Value(Kind::BarStackedPercent),
      "chart", true, "bars normalised to 100% per x"),
    e("LINECHART", &["LINE"], R::Value(Kind::Line), "chart", true, "line chart"),
    e("LINECHART_PERCENT", &["LINE_PERCENT"], R::Value(Kind::LinePercent), "chart", true,
      "line chart, percent y-axis"),
    e("STEP", &["STEPLINE", "STEP_LINE"], R::Value(Kind::Step), "chart", true, "step line"),
    e("SMOOTH", &["TRENDLINE", "TREND_LINE"], R::Value(Kind::Smooth), "chart", true,
      "scatter + trend line (LOESS; ::SMOOTH_METHOD 'lm'/'gam')"),
    e("AREACHART", &["AREA"], R::Value(Kind::Area), "chart", true, "area chart"),
    e("AREACHART_STACKED", &["AREA_STACKED", "STACKED_AREA"], R::Value(Kind::AreaStacked), "chart",
      true, "stacked areas (by CATEGORY)"),
    e("SCATTER", &["POINT", "SCATTERCHART"], R::Value(Kind::Point), "chart", true,
      "scatter; add a ::SIZE column for a bubble chart"),
    e("BUBBLE", &["BUBBLECHART"], R::Value(Kind::Bubble), "chart", true,
      "bubble chart: alone = the y measure (size via ::SIZE); beside a ::SCATTER = its size"),
    e("JITTER", &["JITTERCHART", "STRIP"], R::Value(Kind::Jitter), "chart", true,
      "jittered scatter (reveals overlapping points)"),
    e("PIE", &["PIECHART", "PIECHART_PERCENT"], R::Value(Kind::Pie), "chart", true,
      "pie — slices by CATEGORY, sized by the measure"),
    e("DONUTCHART", &["DONUT", "DONUTCHART_PERCENT"], R::Value(Kind::Donut), "chart", true,
      "donut (pie with a hole)"),
    e("GAUGE", &["GAUGE_PERCENT"], R::Value(Kind::Gauge), "chart", true,
      "value as an arc toward ::RANGE 'min,max' (zones: ::COLORS, ::LABELS)"),
    e("RADAR", &["SPIDER"], R::Value(Kind::Radar), "chart", true,
      "radar / spider chart — axes from XAXIS, one polygon per CATEGORY"),
    e("WATERFALL", &["BRIDGE"], R::Value(Kind::Waterfall), "chart", true,
      "bridge from a start total through signed steps (NULL / ::CATEGORY 'total' rows = totals)"),
    e("BULLET", &["BULLETCHART"], R::Value(Kind::Bullet), "chart", true,
      "measure per XAXIS row vs a ::GOAL tick over ::RANGE bands '60,80,100'"),
    e("FUNNEL", &["FUNNELCHART"], R::Value(Kind::Funnel), "chart", true,
      "ordered XAXIS stages, widths by value, conversion % from the previous stage"),
    e("HISTOGRAM", &["HIST"], R::Value(Kind::Histogram), "chart", true, "histogram of the measure"),
    e("DENSITY", &["KDE"], R::Value(Kind::Density), "chart", true,
      "kernel density curve (one per CATEGORY)"),
    e("BOXPLOT", &["BOX_PLOT"], R::Value(Kind::Boxplot), "chart", true,
      "box plot — XAXIS groups, measure on y (raw rows)"),
    e("VIOLIN", &["VIOLINPLOT"], R::Value(Kind::Violin), "chart", true,
      "violin plot — XAXIS groups, measure on y (raw rows)"),
    e("QQ", &["QQPLOT"], R::Value(Kind::QQ), "chart", true, "normal quantile-quantile plot with a 95% pointwise band"),
    e("HEATMAP", &["TILE", "TILES"], R::Value(Kind::Heatmap), "chart", true,
      "tiles at XAXIS×YAXIS coloured by the measure (rows in SQL order; diverging around 0); \
       a numeric measure sizes every tile alike regardless of how many x points each row has; \
       a text measure (status words) takes ::COLORS as a level map for a categorical legend, \
       like a ::CATEGORY column's"),
    e("CALENDAR", &["CALENDAR_HEATMAP", "CAL_HEATMAP"], R::Value(Kind::Calendar), "chart", true,
      "calendar heatmap (date XAXIS, ≤ 50 years)"),
    e("CANDLESTICK", &["CANDLE", "OHLC"], R::Value(Kind::Candlestick), "chart", true,
      "OHLC candlesticks: XAXIS + ::OPEN/::HIGH/::LOW, close as the measure"),
    e("SPARKLINE", &["SPARK"], R::Value(Kind::Sparkline), "chart", true,
      "minimal trend line (no axes); a list() column in a table; on a map: a mini chart in each feature's tooltip"),
    e("SPARKBAR", &["SPARK_BAR"], R::Value(Kind::Sparkbar), "chart", true,
      "minimal bar histogram (no axes); a table cell's list(v ORDER BY i), or two overlaid \
       series sharing one y scale from list({'ref': r, 'cur': c} ORDER BY i); on a map: a mini chart in each feature's tooltip"),
    e("MAP", &["GEOMETRY", "GEO", "CHOROPLETH"], R::Geometry, "chart", true,
      "WKT-geometry map, coloured by a measure"),
    e("REGION", &["REGION_CODE", "ISO_CODE"], R::Region, "chart", true,
      "a map region by code, drawn from the built-in boundaries: ISO 3166-1 ('DE', 'DEU'), ISO 3166-2 ('DE-BY'), German AGS ('09162') or NUTS ('DE212'), else a name"),
    e("LAT", &["LATITUDE"], R::Lat, "chart", true, "a map point's latitude (degrees, WGS84), with ::LON"),
    e("LON", &["LONGITUDE", "LNG"], R::Lon, "chart", true, "a map point's longitude (degrees, WGS84), with ::LAT"),
    e("ORIGIN", &["FROM_PLACE"], R::Origin, "chart", true, "a flow map's start: a region code ('DE-BY'), a WKT POINT or {'lon': …, 'lat': …, 'name': …} — with ::DEST, width by ::SIZE"),
    e("DEST", &["DESTINATION", "TO_PLACE"], R::Dest, "chart", true, "a flow map's end (see ::ORIGIN)"),
    e("BIVARIATE", &["BIVAR"], R::Bivariate, "encoding", true,
      "a map's second measure: regions coloured by tertiles of both (a 3×3 palette and legend) — with the measure column"),
    e("BASEMAP", &["MAPBASE", "BACKDROP"], R::Basemap, "chart", true,
      "a backdrop under a map: WKT, or a built-in layer 'world', 'de-states', 'de-districts'"),
    e("DIAGRAM", &["NODE"], R::Diagram, "chart", true,
      "node/edge diagram, one row per node (this column = the id; ::LABEL, a KPI format, ::BADGE in the box; edges from ::UPSTREAM / ::EDGES)"),
    // ── annotations on a chart ─────────────────────────────────────────────
    e("REFLINE", &["TARGET", "GOAL", "YLINE"], R::RefLine, "annotation", true,
      "horizontal reference line per distinct value; in a KPI statement the KPI's goal → a progress bar; on a ::BULLET the per-row goal tick"),
    e("XLINE", &[], R::VLine, "annotation", true,
      "vertical reference line at an x (a number, date or timestamp)"),
    e("COOKS_CONTOUR", &["COOKS"], R::CooksContour, "annotation", true,
      "Cook's distance contours (0.5, 1) on residuals vs leverage; value = the model's parameter count p"),
    e("BAND_LOWER", &["BANDLOWER"], R::BandLower, "annotation", true,
      "lower edge of a shaded band around a line; several ::BAND_LOWER / ::BAND_UPPER pairs = a nested fan"),
    e("BAND_UPPER", &["BANDUPPER"], R::BandUpper, "annotation", true,
      "upper edge of a shaded band (pairs with the ::BAND_LOWER in the same position)"),
    e("MARKAREA", &["MARK_AREA", "SHADE"], R::MarkArea, "annotation", true,
      "shade each run of this column's x-values: CASE WHEN flag THEN day END shades every spell"),
    e("XLINE_LABEL", &["EVENT_LABEL"], R::VLineLabel, "annotation", true,
      "label of the ::XLINE on the same row, at the top of the line (events from data)"),
    e("REFLINE_LABEL", &["YLINE_LABEL", "TARGET_LABEL"], R::RefLineLabel, "annotation", true,
      "label of the ::REFLINE on the same row, above its right end"),
    e("XBAND_FROM", &["SHADE_FROM"], R::XBandFrom, "annotation", true,
      "start of a shaded x-range, one per row (a number, date or timestamp)"),
    e("XBAND_TO", &["SHADE_TO"], R::XBandTo, "annotation", true,
      "end of the shaded x-range started by ::XBAND_FROM on the same row"),
    e("XBAND_LABEL", &["SHADE_LABEL"], R::XBandLabel, "annotation", true,
      "label of the shaded x-range on the same row"),
    e("ANNOTATE", &["ANNOTATION"], R::Annotate, "annotation", true,
      "a text callout at the row's (x, y); NULL rows draw nothing"),
    e("DATALABELS", &["DATALABEL", "VALUELABELS", "SHOWLABELS"], R::DataLabels, "annotation", true,
      "draw the value on each mark or heatmap cell (value = font size)"),
    e("ABLINE", &["AB_LINE"], R::AbLine, "annotation", true,
      "straight line y = slope·x + intercept per distinct 'slope,intercept' value"),
    e("IDENTITY", &["DIAGONAL", "IDENTITY_LINE"], R::Identity, "annotation", true,
      "dashed grey y = x line (predicted vs actual, calibration, QQ)"),
    e("LABEL_TOP", &["LABELTOP", "TOPLABELS"], R::LabelTop, "annotation", true,
      "label the top-k points (k = value) with the ::LABEL text, ranked by ::RANK or |y|"),
    // ── chart modifiers ────────────────────────────────────────────────────
    e("FLIP", &["COORD_FLIP", "HORIZONTAL"], R::Flip, "modifier", true,
      "swap the axes (horizontal bars)"),
    e("YFORMAT", &["YAXISFORMAT", "YUNIT", "YCURRENCY"], R::YFormat, "modifier", true,
      "value format — y ticks, tooltips, data labels, heatmap legend: '€', '$', 'percent' (values 0–100), 'comma', ' kg'…"),
    e("XFORMAT", &["XAXISFORMAT", "XUNIT", "XCURRENCY"], R::XFormat, "modifier", true,
      "x-axis tick format (continuous x), like ::YFORMAT"),
    e("XSCALE", &["XTRANS"], R::XScale, "modifier", true,
      "x-axis transform: 'log10', 'sqrt' or 'reverse'"),
    e("YSCALE", &["YTRANS"], R::YScale, "modifier", true,
      "y-axis transform: 'log10', 'sqrt' or 'reverse'"),
    e("XBREAKS", &["X_BREAKS"], R::XBreaks, "modifier", true,
      "explicit x-axis tick positions, in data units — '100,1000,10000'; overrides the automatic ticks (and, under ::XSCALE, their log/sqrt round-number placement)"),
    e("YBREAKS", &["Y_BREAKS"], R::YBreaks, "modifier", true,
      "explicit y-axis tick positions, like ::XBREAKS"),
    e("FACET_NCOL", &["NCOL"], R::FacetCols, "modifier", true,
      "panels per row of a ::FACET / ::FACET_FREE"),
    e("SMOOTH_METHOD", &["METHOD"], R::SmoothMethod, "modifier", true,
      "trend line method of a ::SMOOTH: 'loess' (default), 'lm', 'gam' ('glm' = Gaussian lm)"),
    e("HIST_BINS", &[], R::HistBins, "modifier", true,
      "a ::HISTOGRAM's bin count ('20') or explicit edges ('0,10,20,30'), shared by every ::CATEGORY / ::FACET group — distinct from ::BINS, which aliases ::CLASSES (a map's colour-class breaks)"),
    e("HIST_NORM", &[], R::HistNorm, "modifier", true,
      "a ::HISTOGRAM's per-group normalisation: 'share' draws each ::CATEGORY / ::FACET group's bars as a share of that group's own count (groups of unequal size stay comparable); default: raw counts"),
    e("ALPHA", &["OPACITY"], R::Alpha, "modifier", true, "map layer opacity 0..1"),
    e("SYNC_ZOOM", &["ZOOM_GROUP"], R::SyncZoom, "modifier", true,
      "maps with the same value (e.g. 'routes') zoom and pan together in the browser"),
    e("SYNC_FRAME", &["FRAME_GROUP"], R::SyncFrame, "modifier", true,
      "time maps (::FRAME) with the same value (e.g. 'weeks') step and play together in the browser, by frame label"),
    e("PROJECTION", &["PROJ"], R::Projection, "modifier", true,
      "map projection: 'auto' (default, by the data's extent), 'equal-earth', 'mercator', 'albers-europe', 'conic', 'utm', 'none' (planar x/y)"),
    e("CLASSES", &["CLASS_BREAKS", "BINS"], R::Classes, "modifier", true,
      "map colour classes: 'quantile 5', 'jenks 5', 'equal 5' or edges '0,10,50,100' — a stepped legend with a count per class"),
    e("MIDPOINT", &[], R::Midpoint, "modifier", true,
      "centre of a map's diverging colour ramp (default 0 when the values span it)"),
    e("BOUNDARIES", &["REGION_SET"], R::Boundaries, "modifier", true,
      "the built-in layer a ::REGION is looked up in: 'world', 'world-50m', 'de-states', 'de-districts' (default: the layer most codes match)"),
    e("HEXBIN", &["HEX"], R::Hexbin, "modifier", true,
      "aggregate a map's points into hexagons ('auto' or a width across, '25 km'; planar: '2.5'; sums the measure, else counts rows) or, on a planar ::SCATTER, bin the points into a 2D count density ('auto' or a bin count across each axis)"),
    e("FRAME", &["PLAY_BY", "TIME_FRAME"], R::Frame, "encoding", true,
      "a time map's (or a bar/line/area/scatter chart's) frame key (a week, a date): one result holds every frame; Play steps through them with shared colours, classes, sizes and axes; static output shows the last frame"),
    e("THRESHOLDS", &["STATUS_RULES"], R::Thresholds, "modifier", true,
      "status rules '<0:bad; <0.05:warn; good' — colour a KPI value, gauge zones, chart threshold lines, a ::HEATMAP's tiles in steps; alone: the next table's 'column: rules | …'"),
    e("RANGE", &[], R::Range, "modifier", true, "gauge domain 'min,max' (default 0,100); bullet band edges '60,80,100'; a map's colour domain 'lo,hi' (caps outliers)"),
    e("COLORS", &["COLOURS"], R::GaugeColors, "modifier", true,
      "gauge zone colours, comma-separated hex; beside a ::CATEGORY (a chart or a ::MAP) a level map 'Critical:bad; Watch:warn; OK:good' (status colours or #rrggbb)"),
    e("LABELS", &[], R::GaugeLabels, "modifier", true,
      "gauge zone labels, comma-separated (drawn at each zone)"),
    e("DIRECTION", &[], R::Direction, "modifier", true,
      "a ::DIAGRAM's direction: 'LR' (left to right, default) or 'TB' (top to bottom)"),
    e("ANIMATE", &["ANIMATION", "ENTRANCE"], R::Animate, "modifier", true,
      "entrance animation: 'auto' (lines draw, bars grow, points stagger), 'draw', 'grow', 'reveal' (forward from the ::XLINE, e.g. Today) or 'none'"),
    e("STORY", &["STORYLINE", "NARRATIVE"], R::Story, "annotation", true,
      "per row: a caption → a story step that lights up the row's x (Step / Play controls, the rest dims)"),
    e("STORY_BY", &["STORYBY"], R::StoryBy, "modifier", true,
      "what a ::STORY step lights up: 'x' (default) or 'series' (the row's ::CATEGORY)"),
    e("TRANSITION", &["TWEEN"], R::Transition, "modifier", true,
      "on an input / cross-filter re-run: 'tween' moves the marks to their new values (≤ 300 ms), 'none' (default) swaps the chart"),
    e("HOVER_FOCUS", &["HOVERFOCUS"], R::HoverFocus, "modifier", true,
      "hovering a mark: 'series' (default) emphasises its whole series and fades the others, as hovering its legend entry does; 'none' keeps the plain hover (dense charts)"),
    e("POINTS", &["MARKERS"], R::Points, "modifier", true,
      "line/area point markers: 'auto' (default) hides them on a dense series (x positions under 3 px apart) and keeps them as hover/click targets, 'none' always hides them, 'always' always draws them"),
    e("NORTH_ARROW", &["NORTH", "NORTHARROW"], R::NorthArrow, "modifier", true,
      "a map's north arrow, pointing to true north: 'print' (only in print, PNG, PDF and --svg output; hidden on screen) or 'always'"),
    e("LINESTYLE", &["LINE_STYLE"], R::LineStyle, "modifier", true,
      "per row, by ::CATEGORY: 'flow' = moving dashes (a forecast, \"predicted\"; still dashes in static output), 'dashed', 'solid'"),
    e("LEGEND", &[], R::Legend, "modifier", true,
      "a chart's legend / colourbar: 'none' hides it, or 'top'/'right'/'bottom'/'left' moves it; without it, unchanged"),
    // ── KPIs & text ────────────────────────────────────────────────────────
    e("METRIC", &["KPI", "BIGNUMBER"], R::Metric(MetricFmt::Plain), "kpi", true,
      "big-number KPI (add ::LABEL for a caption)"),
    e("MONEY", &["DOLLAR", "CURRENCY"], R::Metric(MetricFmt::Money), "kpi", true,
      "currency KPI / table column format"),
    e("PERCENT", &["PCT"], R::Metric(MetricFmt::Percent), "kpi", true,
      "percent KPI / table column format; the value is in percent (12.5 → 12.5%; a fraction × 100)"),
    e("COMPACT", &[], R::Metric(MetricFmt::Compact), "kpi", true,
      "compact-number KPI / table column format (1.2K)"),
    e("DELTA", &["COMPARE", "PREVIOUS"], R::Delta, "kpi", true,
      "comparison value → trend arrow + % on a KPI (up = good)"),
    e("DELTA_LOWER_GOOD", &["COMPARE_LOWER_GOOD", "DELTA_INVERSE"], R::DeltaLowerGood, "kpi", true,
      "comparison value where lower is better (churn, cost): a rise is drawn red"),
    e("DELTA_FORMAT", &["DELTAFORMAT"], R::DeltaFormat, "kpi", true,
      "how a KPI's change reads: 'pct' (default), 'abs' (in the KPI's format: +$1,200, +2.3 pp) or 'both'"),
    e("DELTA_LABEL", &["DELTALABEL", "COMPARE_LABEL"], R::DeltaLabel, "kpi", true,
      "what the change compares against, shown after it: 'vs last month', getvariable('p_prev_label')"),
    e("KPI_SIZE", &["KPI_DENSITY"], R::KpiSize, "kpi", true,
      "KPI tile density: 'compact' (single line, value + label, no ::SPARKLINE/::GOAL) or 'normal' (default); on ::METRIC it sets that tile, on ::GROUP every tile in the box"),
    e("TEXT_SMALL", &[], R::Text(TextSize::Small), "kpi", true, "small text card"),
    e("TEXT_MEDIUM", &[], R::Text(TextSize::Medium), "kpi", true, "medium text card"),
    e("TEXT_LARGE", &[], R::Text(TextSize::Large), "kpi", true, "large text card"),
    e("MARKDOWN", &["MD", "TEXTBOX", "RICHTEXT"], R::Markdown, "kpi", true,
      "a Markdown box (browser); {{col}} / {{col:fmt}} fill in values from its own row; $…$ / $$…$$ TeX formulas; ![alt](src) images use the same source rules as ::IMAGE"),
    e("CALLOUT", &["ADMONITION"], R::Callout, "kpi", true,
      "draw the ::MARKDOWN box as a callout: 'note', 'warning' or 'success'"),
    e("MARKDOWN_STYLE", &["MD_STYLE"], R::MarkdownStyle, "kpi", true,
      "a ::MARKDOWN box's chrome: 'plain' (page background, no card/tint/bar), 'card' (plain white card) or 'callout' (today's tinted box, the default; ::CALLOUT implies it); a dashboard-wide default comes from -- @markdown_style"),
    e("IMAGE", &["PICTURE", "IMG"], R::Image, "kpi", true,
      "a static picture panel: 'assets/x.svg', data:image/… or https:; ::ALT is its text alternative, ::IMAGE_DARK a dark-theme variant, ::LABEL a caption"),
    e("IMAGE_DARK", &["IMG_DARK"], R::ImageDark, "kpi", true,
      "the ::IMAGE shown while the dashboard is dark"),
    // ── tables ─────────────────────────────────────────────────────────────
    e("TABLE", &["GRID"], R::Table, "table", true,
      "the whole result as a table (one marker per panel)"),
    e("PAGED", &["TABLE_PAGED", "PAGINATED"], R::PagedTable, "table", true,
      "SQL-paginated table for large/remote data"),
    e("TREND", &[], R::Trend, "table", true, "▲/▼ arrow in a table cell"),
    e("COLORSCALE", &["COLOURSCALE", "HEAT", "GRADIENT"], R::ColorScale(Scale::LowGood), "table", true,
      "heatmap-colour a table column's cells (low = green, high = red); on a ::HEATMAP / ::CALENDAR its tiles, good → warn → bad"),
    e("COLORSCALE_HIGH_GOOD", &["COLORSCALE_HIGH", "HEAT_HIGH"], R::ColorScale(Scale::HighGood), "table", true,
      "heatmap-colour a table column's cells (or a ::HEATMAP's tiles), high = green (scores, accuracy)"),
    e("COLORSCALE_DIVERGING", &["DIVERGING", "HEAT_DIVERGING"], R::ColorScale(Scale::Diverging), "table", true,
      "colour a table column's cells (or a ::HEATMAP's tiles) around zero: negative red, positive green (deltas)"),
    e("LINK", &["DRILLTHROUGH", "DRILL_THROUGH", "HREF"], R::Link, "table", true,
      "drill-through on a chart click / a table link cell: '#Tab', '?v.x={value}', '/d/<id>?v.x={value}' or an https: URL ({value}, {column} placeholders)"),
    e("BADGE", &["STATUS", "PILL"], R::Badge, "table", true, "render a table column as status pills"),
    e("PLAIN", &["NOBAR"], R::Plain, "table", true, "a plain table column (no in-cell bar)"),
    e("DOWNLOAD_CSV", &[], R::Download(DownloadFmt::Csv), "table", false, "CSV export button"),
    e("DOWNLOAD_XLSX", &["DOWNLOAD_EXCEL"], R::Download(DownloadFmt::Xlsx), "table", false,
      "Excel export button"),
    e("DOWNLOAD_PDF", &[], R::Download(DownloadFmt::Pdf), "table", false,
      "print-to-PDF button"),
    // ── inputs (the column name becomes getvariable('<name>')) ─────────────
    e("DROPDOWN", &["OPTIONS", "SELECT_INPUT"], R::Input(InputKind::Dropdown), "input", false,
      "single-select → a DuckDB variable"),
    e("MULTISELECT", &["MULTI"], R::Input(InputKind::Multiselect), "input", false,
      "multi-select → a list variable"),
    e("NUMBER", &["SLIDER", "NUMERIC"], R::Input(InputKind::Number), "input", false,
      "numeric input (value = default)"),
    e("DATE", &["DATEPICKER"], R::Input(InputKind::Date), "input", false,
      "date picker (value = default)"),
    e("TEXT", &["SEARCH", "STRING"], R::Input(InputKind::Text), "input", false,
      "free-text input (value = default)"),
    e("DATERANGE", &["DATE_RANGE"], R::Input(InputKind::DateRange), "input", false,
      "two date columns → from/to variables"),
    e("PERIOD", &["PERIOD_PICKER", "DATE_PERIOD"], R::Input(InputKind::Period), "input", false,
      "date presets + grain + comparison → <name>_from/_to/_prev_from/_prev_to/_grain/_compare/_label/_prev_label"),
    e("BUTTONS", &["BUTTON_GROUP", "SEGMENTED"], R::Input(InputKind::Buttons), "input", false,
      "segmented button group: one of a few options → a DuckDB variable (a metric / dimension switcher)"),
    e("TOGGLE", &["SWITCH", "CHECKBOX"], R::Input(InputKind::Toggle), "input", false,
      "on/off switch → a BOOLEAN variable (value = default)"),
    e("HINT", &[], R::Hint, "input", true, "per-option hint next to a dropdown option"),
    e("OPTION_LABEL", &[], R::OptionLabel, "input", true, "the shown text of a dropdown / button / multiselect option (the variable keeps the value)"),
    e("DROPDOWN_STYLE", &[], R::DropdownStyle, "input", false,
      "forces a ::DROPDOWN's widget: 'search' (searchable combobox) or 'native' (plain <select>), overriding the option-count threshold"),
    e("SUBMIT", &[], R::Submit, "input", false,
      "a button (value = its label) that records SUBMIT_VARS for this viewer, append-only — requires a locked server with options.state naming this dashboard (#517); disabled, with a visible reason, on a published static site, a server without options.state, or the wasm/live-authoring builder's own preview session (preview submissions stay in the tab, never reach a server)"),
    e("SUBMIT_VARS", &[], R::SubmitVars, "input", false,
      "comma-separated variable names the preceding ::SUBMIT records, each declared by one of this dashboard's own input statements"),
    e("SUBMIT_DONE", &[], R::SubmitDone, "input", false,
      "optional confirmation text shown after a successful ::SUBMIT"),
    // ── layout ─────────────────────────────────────────────────────────────
    e("COLUMNS", &["COLS"], R::Columns, "layout", false, "default panels per row"),
    e("SPAN", &["COL", "WIDTH"], R::Span, "layout", false, "next panel's width (of 12)"),
    e("HEIGHT", &["TALL"], R::Height, "layout", false, "next panel's height in px"),
    e("ROWSELECT", &["ROW_SELECT", "SELECTABLE"], R::RowSelect, "layout", false,
      "next table's row selection: 'none' (default), 'single' or 'multi' — a click sets getvariable('<first column>')"),
    e("TOTALS", &["TOTAL", "AGGREGATE"], R::TableOpt(TableOpt::Totals), "layout", false,
      "next table's totals: 'sum'|'mean'|'median'|'min'|'max'|'count'|'none', or per column 'Revenue: sum; Per 10k: none'"),
    e("COLGROUP", &["COL_GROUP", "HEADER_GROUP"], R::TableOpt(TableOpt::ColGroup), "layout", false,
      "next table's grouped header: 'Backtest: mae, rmse | Holdout: mae_h, rmse_h'"),
    e("FLAG", &["THRESHOLD"], R::TableOpt(TableOpt::Flag), "layout", false,
      "next table: flag cells meeting a condition with ⚠ — 'mape > 0.2, bias < -5'"),
    e("HIGHLIGHT", &["TOPN"], R::TableOpt(TableOpt::Highlight), "layout", false,
      "next table: emphasise the top / bottom n of a column — 'Revenue: top 3, Margin: bottom 2'"),
    e("SUBTOTAL", &["SUBTOTALS", "GROUP_TOTAL"], R::TableOpt(TableOpt::Subtotal), "layout", false,
      "next table: a subtotal row after each group of a column — 'region' (not for ::PAGED)"),
    e("TOOLBAR", &[], R::TableOpt(TableOpt::Toolbar), "layout", false,
      "next table's toolbar: 'none' hides it entirely (row count and chips too; a multi-select's \"N selected\" chip stays), or an allow-list of the parts to keep — 'search,filters,totals,stats,columns,density,export' ('totals' only hides its toggle button, not the row; 'density' covers Pages|Scroll too; each kept part still needs its own row/column threshold); an excluded 'export' still gets a small hover CSV button"),
    e("SELECT_AS", &["SELECTAS", "SELECTION"], R::SelectAs, "layout", false,
      "next chart's named selection: 'region' → a click sets getvariable('region'); 'region multi' → a list (shift/ctrl-click adds); a brush on a continuous x sets region_from / region_to"),
    e("EMPTY", &["EMPTY_MESSAGE", "NODATA"], R::Empty, "layout", false,
      "next panel's message when its result is empty: 'No orders in range — widen the dates'"),
    e("SHOW_IF", &["SHOWIF", "VISIBLE_IF"], R::ShowIf, "layout", false,
      "show the next panel (or ::GROUP box) only while this BOOLEAN is TRUE — re-checked on every input change: getvariable('q1') <> ''"),
    e("DRILL", &["DRILLDOWN", "DRILL_DOWN"], R::Drill, "layout", false,
      "next chart drills down 'region > country > city' ('geo: …' names the variables): drill_level / drill_depth / drill_path + a variable per level, a breadcrumb back"),
    e("GROUP", &["BOX", "ROW"], R::GroupStart, "layout", false, "open a box; ::ENDGROUP closes it"),
    e("ENDGROUP", &["ENDBOX", "ENDROW"], R::GroupEnd, "layout", false, "close the current box"),
    e("TAB", &["PAGE"], R::Tab, "layout", false, "start a tab/page"),
    e("SUBTAB", &["SUB_TAB"], R::SubTab, "layout", false, "a nested tab inside a ::TAB"),
    e("GUIDE_STEP", &["TUTORIAL_STEP", "WIZARD_STEP"], R::GuideStep, "layout", false,
      "a step of a guided sequence (onboarding, tutorials): Back / Next, 'Step 2 of 5', \
       ?v.<name>=n; the alias names the step variable (default 'step')"),
    e("GATE", &["STEP_GATE", "NEXT_IF"], R::Gate, "layout", false,
      "on a ::GUIDE_STEP: Next stays disabled until this BOOLEAN is TRUE"),
    e("GATE_MESSAGE", &["GATE_HINT"], R::GateMessage, "layout", false,
      "on a ::GUIDE_STEP: why Next is disabled"),
    e("VIEW", &["ALT_VIEW"], R::View, "layout", false,
      "one of several alternative views of one panel: consecutive `::VIEW` sections share a segmented switcher; `'Lines' AS name ::VIEW` names the group (`?view.name=`)"),
    e("ENDVIEW", &["END_VIEW"], R::ViewEnd, "layout", false,
      "close a `::VIEW` group (`::TAB`, `::SUBTAB`, `::SECTION` also close it)"),
    e("VIEW_PLACEMENT", &[], R::ViewPlacement, "layout", false,
      "a ::VIEW group's switcher placement: 'row' (default, its own row above the card) or \
       'header' (right-aligned, compact, in the card's title row — folds into a dropdown \
       below a minimum card width; only when every alternative is exactly one panel, else \
       --check warns and it falls back to 'row'); set directly before the group's first \
       ::VIEW marker, like ::COL/::SPAN"),
    e("FIT", &["FIT_SCREEN"], R::Fit, "layout", false,
      "this ::TAB / ::SUBTAB / ::VIEW (no tabs: the whole dashboard) fits the window while shown — rows scale to fill it, no page scroll ('fit'::FIT; ?fit=1 / ?fit=0 override it; ignored below 600 px wide)"),
    e("PLACEHOLDER", &[], R::Placeholder, "layout", false, "an empty grid cell"),
    e("TRACK", &["STRIP_BELOW", "STRIP_UNDER"], R::Track, "layout", true,
      "a thin panel under the previous chart, sharing its x axis (value = its name): ticks, ::HEATMAP tiles per ::YAXIS lane, or a line / bar"),
    // ── chrome ─────────────────────────────────────────────────────────────
    e("RELOAD", &["REFRESH"], R::Reload, "chrome", false, "auto-refresh interval (seconds)"),
    e("HEADER_IMAGE", &["HEADERIMAGE"], R::HeaderImage, "chrome", false, "banner image URL"),
    e("FOOTER_LINK", &["FOOTERLINK"], R::FooterLink, "chrome", false, "link at the bottom"),
    e("SECTION", &["SECTION_HEADING"], R::Section, "layout", false,
      "a section heading that always starts a new row"),
    e("THEME", &[], R::Theme, "chrome", false,
      "dashboard theme: 'light'/'dark'/'print'/'colorblind'/'high-contrast', a stack 'acme+dark', or JSON"),
    e("LOCALE", &[], R::Locale, "chrome", false,
      "number/date locale for KPIs, tables and axes: 'en-US', 'en-GB', 'en-IE' (English with €), 'de-DE', 'fr-FR'"),
    e("DASHBOARD_TITLE", &["DASHBOARDTITLE", "PAGE_TITLE"], R::DashboardTitle, "chrome", false,
      "dashboard title: page header, browser tab and export title"),
    e("BRAND", &["BRAND_NAME"], R::Brand, "chrome", false,
      "brand / organisation name shown above the dashboard title"),
    e("LOGO", &[], R::Logo, "chrome", false, "header logo image URL (relative, https: or data:image/)"),
    e("FAVICON", &[], R::Favicon, "chrome", false, "browser-tab icon URL"),
    e("ASOF", &["AS_OF", "FRESHNESS", "DATA_AS_OF"], R::AsOf, "chrome", false,
      "data freshness: a 'Data as of 14:02 (2 h ago)' chip in the header; a 'stale=24h' column turns it amber"),
];

/// Role tokens that are **also DuckDB type names** (`x::DATE` is a real cast).
/// They count as roles only in a query statement whose SELECT list has another,
/// unambiguous role cast — or when every cast in a plain top-level `SELECT` is
/// one of these (a standalone input like `SELECT DATE '2024-01-01' AS d::DATE`).
/// They are never roles inside DDL/DML, and inside a `::TABLE`/`::PAGED`/
/// `::DOWNLOAD_*`/`::DATERANGE` panel they stay real casts.
pub const SQL_TYPE_TOKENS: &[&str] = &["DATE", "TEXT", "STRING", "NUMERIC", "MAP", "GEOMETRY"];

/// Is `token` (case-insensitive) one of [`SQL_TYPE_TOKENS`]?
pub fn is_sql_type_token(token: &str) -> bool {
    SQL_TYPE_TOKENS
        .iter()
        .any(|t| t.eq_ignore_ascii_case(token.trim()))
}

/// Look up a token or alias (case-insensitive).
pub fn lookup(token: &str) -> Option<&'static RoleSpec> {
    let t = token.trim();
    REGISTRY.iter().find(|s| {
        s.token.eq_ignore_ascii_case(t) || s.aliases.iter().any(|a| a.eq_ignore_ascii_case(t))
    })
}

/// The registry entry for a parsed role.
pub fn spec_of(role: Role) -> &'static RoleSpec {
    REGISTRY
        .iter()
        .find(|s| s.role == role)
        .expect("every Role variant is registered (tested)")
}

impl Role {
    /// The canonical token (e.g. `Role::Value(Kind::Bar)` → `"BARCHART"`).
    pub fn token(&self) -> &'static str {
        spec_of(*self).token
    }
    /// Does a statement carrying this role draw a visible panel? `false` for
    /// layout directives, inputs, downloads, reload and header/footer chrome.
    pub fn renders(&self) -> bool {
        spec_of(*self).renders
    }
    /// Registry category (`chart`, `input`, `layout`, …).
    pub fn category(&self) -> &'static str {
        spec_of(*self).category
    }
}

/// True when any role in a planned panel is a non-rendering directive — static
/// renderers (CLI, `serve`, headless SVG) and the linter skip such statements.
pub fn is_directive_panel(roles: &[(usize, Role)]) -> bool {
    roles.iter().any(|(_, r)| !r.renders()) || is_table_thresholds(roles)
}

/// A lone `::THRESHOLDS` statement: the next table's per-column rules (a
/// directive), not a panel. Beside other roles it modifies that panel.
pub fn is_table_thresholds(roles: &[(usize, Role)]) -> bool {
    matches!(roles, [(_, Role::Thresholds)])
}

/// Canonical tokens by category (aliases exist but prefer these).
pub fn catalog() -> &'static [RoleSpec] {
    REGISTRY
}

/// Human-readable, grouped listing (for `dashboard --roles`).
pub fn text() -> String {
    let mut out = String::new();
    let mut cat = "";
    for d in REGISTRY {
        if d.category != cat {
            cat = d.category;
            out.push_str(&format!("\n[{cat}]\n"));
        }
        let alias = if d.aliases.is_empty() {
            String::new()
        } else {
            format!("  (also {})", d.aliases.join(", "))
        };
        out.push_str(&format!("  ::{:<26} {}{alias}\n", d.token, d.summary));
    }
    out
}

/// The role table for `docs/DOCS.md` (generated; a test checks the doc embeds
/// exactly this text between the `roles-table` markers).
pub fn markdown_table() -> String {
    let mut out = String::from(
        "| Cast | Aliases | Category | Meaning |\n|------|---------|----------|---------|\n",
    );
    for d in REGISTRY {
        let aliases = d
            .aliases
            .iter()
            .map(|a| format!("`::{a}`"))
            .collect::<Vec<_>>()
            .join(", ");
        out.push_str(&format!(
            "| `::{}` | {} | {} | {} |\n",
            d.token,
            aliases,
            d.category,
            d.summary.replace('|', "\\|")
        ));
    }
    out
}

/// `::CALLOUT` value → `'note'` | `'warning'` | `'success'` (an unknown or
/// missing value draws `'note'` — lint warns separately on an unknown one:
/// `callout/unknown`). Mirrors `src/lint.rs`'s `CALLOUT_KINDS` and the
/// browser's `calloutKind` (web/app.js).
pub fn callout_kind(v: &str) -> &'static str {
    match v.trim().to_ascii_lowercase().as_str() {
        "warning" | "caution" | "danger" => "warning",
        "success" | "tip" => "success",
        _ => "note",
    }
}

/// `::MARKDOWN_STYLE` / `-- @markdown_style` values (#536): a `::MARKDOWN`
/// box's chrome. The single source of truth for both — `src/lint.rs`'s panel
/// and header checks, `src/dashboard.rs`'s SVG renderer and the browser's
/// `MD_STYLE_KINDS` (web/app.js) all list the same three.
pub const MARKDOWN_STYLE_KINDS: &[&str] = &["plain", "card", "callout"];

/// `v`, trimmed and lower-cased, if it names one of [`MARKDOWN_STYLE_KINDS`].
pub fn markdown_style_of(v: &str) -> Option<&'static str> {
    MARKDOWN_STYLE_KINDS
        .iter()
        .find(|k| v.trim().eq_ignore_ascii_case(k))
        .copied()
}

/// The effective `::MARKDOWN_STYLE` for one `::MARKDOWN` panel: an explicit,
/// valid `explicit` (the panel's own `::MARKDOWN_STYLE` cell) wins outright —
/// even over `::CALLOUT` (lint warns there, `markdown_style/conflicts-with-
/// callout`); else `::CALLOUT` (`callout_present`) always draws 'callout',
/// since that's the whole point of setting it; else the dashboard's own
/// `-- @markdown_style` default; else 'callout' (today's look).
pub fn resolve_markdown_style(
    explicit: Option<&str>,
    callout_present: bool,
    dashboard_default: Option<&str>,
) -> &'static str {
    if let Some(v) = explicit.and_then(markdown_style_of) {
        return v;
    }
    if callout_present {
        return "callout";
    }
    dashboard_default
        .and_then(markdown_style_of)
        .unwrap_or("callout")
}

/// JSON export of the registry for the browser (wasm `roles_json()`):
/// `{"roles":[{token, aliases, category, renders, summary}], "sets":{…}}`.
/// `sets` are the derived token lists `web/app.js` matches on — `inputs`,
/// `metrics`, `directives` (layout statements), `table_formats` (per-column
/// table formatting roles) and `text_sizes`.
pub fn roles_json() -> String {
    let roles: Vec<serde_json::Value> = REGISTRY
        .iter()
        .map(|d| {
            serde_json::json!({
                "token": d.token,
                "aliases": d.aliases,
                "category": d.category,
                "renders": d.renders,
                "summary": d.summary,
            })
        })
        .collect();
    let tokens = |f: &dyn Fn(Role) -> bool| -> Vec<&'static str> {
        REGISTRY
            .iter()
            .filter(|d| f(d.role))
            .map(|d| d.token)
            .collect()
    };
    let sets = serde_json::json!({
        "inputs": tokens(&|r| matches!(r, Role::Input(_))),
        "metrics": tokens(&|r| matches!(r, Role::Metric(_))),
        "directives": tokens(&|r| matches!(
            r,
            Role::Columns | Role::GroupStart | Role::GroupEnd | Role::Span | Role::Height
                | Role::RowSelect | Role::TableOpt(_) | Role::Tab | Role::SubTab | Role::Placeholder
                | Role::SelectAs | Role::Drill | Role::Empty | Role::ShowIf | Role::View | Role::ViewEnd
                | Role::ViewPlacement | Role::Fit | Role::GuideStep | Role::Gate | Role::GateMessage
        )),
        // Dashboard-level chrome directives (theme, locale, title, branding).
        "chrome": tokens(&|r| is_chrome_directive(r)),
        "table_formats": tokens(&|r| is_table_format(r)),
        "text_sizes": tokens(&|r| matches!(r, Role::Text(_))),
        // Chart kinds whose panel honours a zoom window (scroll/drag zoom).
        "zoomable": tokens(&|r| matches!(r, Role::Value(k) if k.zoomable())),
    });
    serde_json::json!({ "roles": roles, "sets": sets }).to_string()
}

/// Dashboard-level chrome directives collected before the panels render
/// (see [`crate::theme::DashboardChrome`]).
pub fn is_chrome_directive(r: Role) -> bool {
    matches!(
        r,
        Role::Theme
            | Role::Locale
            | Role::DashboardTitle
            | Role::Brand
            | Role::Logo
            | Role::Favicon
            | Role::AsOf
    )
}

/// Per-column formatting roles honoured inside a `::TABLE`/`::PAGED` panel.
pub fn is_table_format(r: Role) -> bool {
    matches!(
        r,
        Role::Metric(_)
            | Role::Trend
            | Role::ColorScale(_)
            | Role::Badge
            | Role::Value(Kind::Sparkline)
            | Role::Value(Kind::Sparkbar)
            | Role::Plain
            | Role::Link
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse_role;

    #[test]
    fn every_token_and_alias_parses_to_its_role() {
        for d in REGISTRY {
            assert_eq!(parse_role(d.token), Some(d.role), "::{}", d.token);
            for a in d.aliases {
                assert_eq!(parse_role(a), Some(d.role), "alias ::{a}");
                assert_eq!(parse_role(&a.to_lowercase()), Some(d.role), "alias ::{a}");
            }
        }
    }

    #[test]
    fn markdown_style_of_accepts_only_the_three_kinds() {
        assert_eq!(markdown_style_of("plain"), Some("plain"));
        assert_eq!(markdown_style_of(" Card "), Some("card"));
        assert_eq!(markdown_style_of("CALLOUT"), Some("callout"));
        assert_eq!(markdown_style_of(""), None);
        assert_eq!(markdown_style_of("plainish"), None);
    }

    #[test]
    fn resolve_markdown_style_precedence() {
        // nothing set at all: 'callout', today's look.
        assert_eq!(resolve_markdown_style(None, false, None), "callout");
        // the dashboard's own default, with no panel-level override.
        assert_eq!(resolve_markdown_style(None, false, Some("plain")), "plain");
        // ::CALLOUT always draws 'callout', even over a 'plain' dashboard default.
        assert_eq!(resolve_markdown_style(None, true, Some("plain")), "callout");
        // an explicit, valid ::MARKDOWN_STYLE wins outright — even over ::CALLOUT.
        assert_eq!(
            resolve_markdown_style(Some("card"), true, Some("callout")),
            "card"
        );
        assert_eq!(resolve_markdown_style(Some("plain"), true, None), "plain");
        // an unrecognized explicit value is ignored, same as not set.
        assert_eq!(
            resolve_markdown_style(Some("weird"), false, Some("card")),
            "card"
        );
        // an unrecognized dashboard default is ignored too.
        assert_eq!(
            resolve_markdown_style(None, false, Some("weird")),
            "callout"
        );
    }

    #[test]
    fn callout_kind_groups_its_aliases() {
        assert_eq!(callout_kind("warning"), "warning");
        assert_eq!(callout_kind("caution"), "warning");
        assert_eq!(callout_kind("danger"), "warning");
        assert_eq!(callout_kind("success"), "success");
        assert_eq!(callout_kind("tip"), "success");
        assert_eq!(callout_kind("note"), "note");
        assert_eq!(callout_kind("unknown"), "note");
        assert_eq!(callout_kind(""), "note");
    }

    #[test]
    fn docs_role_table_is_in_sync() {
        let doc = include_str!("../docs/DOCS.md");
        let start = "<!-- roles-table:start -->\n";
        let a = doc.find(start).expect("start marker") + start.len();
        let b = doc.find("<!-- roles-table:end -->").expect("end marker");
        assert_eq!(
            &doc[a..b],
            markdown_table(),
            "docs/DOCS.md role table is stale — regenerate with `dashboard --roles-md`"
        );
    }

    #[test]
    fn tokens_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for d in REGISTRY {
            for t in std::iter::once(&d.token).chain(d.aliases.iter()) {
                assert!(seen.insert(*t), "duplicate token ::{t}");
            }
        }
    }

    #[test]
    fn canonical_token_round_trips() {
        // Role → token → Role (the wire format the browser sees).
        for d in REGISTRY {
            assert_eq!(d.role.token(), d.token);
            assert_eq!(parse_role(d.role.token()), Some(d.role));
        }
    }

    #[test]
    fn json_export_is_valid_and_complete() {
        let v: serde_json::Value = serde_json::from_str(&roles_json()).unwrap();
        assert_eq!(v["roles"].as_array().unwrap().len(), REGISTRY.len());
        let metrics = v["sets"]["metrics"].as_array().unwrap();
        assert_eq!(metrics.len(), 4);
        let dirs: Vec<&str> = v["sets"]["directives"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x.as_str().unwrap())
            .collect();
        for t in [
            "COLUMNS",
            "GROUP",
            "ENDGROUP",
            "SPAN",
            "HEIGHT",
            "ROWSELECT",
            "TOTALS",
            "COLGROUP",
            "FLAG",
            "HIGHLIGHT",
            "SUBTOTAL",
            "TOOLBAR",
            "TAB",
            "SUBTAB",
            "PLACEHOLDER",
            "SELECT_AS",
            "DRILL",
            "VIEW",
            "ENDVIEW",
            "VIEW_PLACEMENT",
            "FIT",
            "SHOW_IF",
            "GUIDE_STEP",
            "GATE",
            "GATE_MESSAGE",
        ] {
            assert!(dirs.contains(&t), "{t}");
        }
    }

    #[test]
    fn fit_is_a_registered_layout_directive() {
        // #400: `'fit'::FIT` (alias ::FIT_SCREEN) is a non-rendering layout
        // directive the browser matches on, like ::TAB / ::HEIGHT.
        let s = lookup("FIT").expect("::FIT registered");
        assert_eq!(s.role, Role::Fit);
        assert_eq!(s.category, "layout");
        assert!(!s.renders);
        assert_eq!(parse_role("fit_screen"), Some(Role::Fit));
        assert_eq!(Role::Fit.token(), "FIT");
        assert!(is_directive_panel(&[(0, Role::Fit)]));
    }

    #[test]
    fn previously_drifted_tokens_are_registered() {
        for t in [
            "YAXISFORMAT",
            "YUNIT",
            "XAXISFORMAT",
            "XUNIT",
            "AREACHART_STACKED",
            "JITTER",
            "OPEN",
            "HIGH",
            "LOW",
            "SIZE",
            "DATALABELS",
            "MARKAREA",
            "FLIP",
            "ALPHA",
            "BASEMAP",
            "HINT",
            "OPTION_LABEL",
            "XFORMAT",
            "BUBBLE",
        ] {
            assert!(lookup(t).is_some(), "::{t}");
        }
        assert_eq!(parse_role("BUBBLE"), Some(Role::Value(Kind::Bubble)));
    }

    /// Every `Role` value. The `match` below has no wildcard, so adding a
    /// variant without listing it here (and registering it) fails to compile.
    fn all_roles() -> Vec<Role> {
        use crate::{DownloadFmt as D, InputKind as I, Kind as K, MetricFmt as M, TextSize as T};
        let kinds = [
            K::Bar,
            K::BarStacked,
            K::BarPercent,
            K::BarStackedPercent,
            K::Line,
            K::LinePercent,
            K::Step,
            K::Smooth,
            K::Area,
            K::AreaStacked,
            K::Point,
            K::Pie,
            K::Donut,
            K::Histogram,
            K::Boxplot,
            K::Violin,
            K::Density,
            K::QQ,
            K::Heatmap,
            K::Sparkline,
            K::Sparkbar,
            K::Gauge,
            K::Calendar,
            K::Jitter,
            K::Candlestick,
            K::Radar,
            K::Bubble,
            K::Waterfall,
            K::Bullet,
            K::Funnel,
        ];
        let mut v: Vec<Role> = kinds.into_iter().map(Role::Value).collect();
        v.extend(
            [
                I::Dropdown,
                I::Number,
                I::Date,
                I::Text,
                I::Multiselect,
                I::DateRange,
                I::Period,
                I::Buttons,
                I::Toggle,
            ]
            .map(Role::Input),
        );
        v.extend([M::Plain, M::Money, M::Percent, M::Compact].map(Role::Metric));
        v.extend([T::Small, T::Medium, T::Large].map(Role::Text));
        v.extend([D::Csv, D::Xlsx, D::Pdf].map(Role::Download));
        v.extend([
            Role::KpiSize,
            Role::X,
            Role::Y,
            Role::Category,
            Role::Label,
            Role::Title,
            Role::Columns,
            Role::GroupStart,
            Role::GroupEnd,
            Role::Span,
            Role::Height,
            Role::RowSelect,
            Role::TableOpt(TableOpt::Totals),
            Role::TableOpt(TableOpt::ColGroup),
            Role::TableOpt(TableOpt::Flag),
            Role::TableOpt(TableOpt::Highlight),
            Role::TableOpt(TableOpt::Subtotal),
            Role::TableOpt(TableOpt::Toolbar),
            Role::Table,
            Role::PagedTable,
            Role::Delta,
            Role::DeltaLowerGood,
            Role::DeltaFormat,
            Role::DeltaLabel,
            Role::RefLine,
            Role::VLine,
            Role::BandLower,
            Role::BandUpper,
            Role::Trend,
            Role::ColorScale(Scale::LowGood),
            Role::ColorScale(Scale::HighGood),
            Role::ColorScale(Scale::Diverging),
            Role::Badge,
            Role::Plain,
            Role::Hint,
            Role::OptionLabel,
            Role::DropdownStyle,
            Role::Submit,
            Role::SubmitVars,
            Role::SubmitDone,
            Role::Placeholder,
            Role::HeaderImage,
            Role::FooterLink,
            Role::Reload,
            Role::Range,
            Role::GaugeLabels,
            Role::GaugeColors,
            Role::Geometry,
            Role::Basemap,
            Role::Flip,
            Role::Alpha,
            Role::SyncZoom,
            Role::SyncFrame,
            Role::Projection,
            Role::Classes,
            Role::Midpoint,
            Role::Hexbin,
            Role::Frame,
            Role::Region,
            Role::Lat,
            Role::Lon,
            Role::Origin,
            Role::Dest,
            Role::Bivariate,
            Role::Boundaries,
            Role::Tab,
            Role::SubTab,
            Role::View,
            Role::ViewEnd,
            Role::ViewPlacement,
            Role::Fit,
            Role::Size,
            Role::YFormat,
            Role::XFormat,
            Role::DataLabels,
            Role::MarkArea,
            Role::VLineLabel,
            Role::RefLineLabel,
            Role::XBandFrom,
            Role::XBandTo,
            Role::XBandLabel,
            Role::Annotate,
            Role::Thresholds,
            Role::Markdown,
            Role::Image,
            Role::ImageDark,
            Role::Alt,
            Role::Open,
            Role::High,
            Role::Low,
            Role::Q1,
            Role::Median,
            Role::Q3,
            Role::WhiskerLow,
            Role::WhiskerHigh,
            Role::Mean,
            Role::Outliers,
            Role::YMin,
            Role::YMax,
            Role::XMin,
            Role::XMax,
            Role::Facet,
            Role::FacetFree,
            Role::FacetCols,
            Role::XScale,
            Role::YScale,
            Role::XBreaks,
            Role::YBreaks,
            Role::AbLine,
            Role::Identity,
            Role::LabelTop,
            Role::Rank,
            Role::Censor,
            Role::CooksContour,
            Role::SmoothMethod,
            Role::HistBins,
            Role::HistNorm,
            Role::Theme,
            Role::Locale,
            Role::DashboardTitle,
            Role::Brand,
            Role::Logo,
            Role::Favicon,
            Role::AsOf,
            Role::Info,
            Role::Callout,
            Role::MarkdownStyle,
            Role::Section,
            Role::SelectAs,
            Role::Drill,
            Role::Link,
            Role::Empty,
            Role::ShowIf,
            Role::Animate,
            Role::LineStyle,
            Role::Story,
            Role::StoryBy,
            Role::Transition,
            Role::HoverFocus,
            Role::Points,
            Role::NorthArrow,
            Role::Track,
            Role::Diagram,
            Role::Upstream,
            Role::Edges,
            Role::Direction,
            Role::Lane,
            Role::Legend,
            Role::GuideStep,
            Role::Gate,
            Role::GateMessage,
        ]);
        for r in &v {
            // Exhaustiveness guard (no `_` arm).
            match r {
                Role::X
                | Role::Y
                | Role::Category
                | Role::Label
                | Role::Title
                | Role::Value(_)
                | Role::Input(_)
                | Role::Columns
                | Role::GroupStart
                | Role::GroupEnd
                | Role::Span
                | Role::Height
                | Role::RowSelect
                | Role::TableOpt(_)
                | Role::Table
                | Role::PagedTable
                | Role::Metric(_)
                | Role::Delta
                | Role::DeltaLowerGood
                | Role::DeltaFormat
                | Role::DeltaLabel
                | Role::RefLine
                | Role::VLine
                | Role::BandLower
                | Role::BandUpper
                | Role::Trend
                | Role::ColorScale(_)
                | Role::Badge
                | Role::Plain
                | Role::Hint
                | Role::OptionLabel
                | Role::DropdownStyle
                | Role::Submit
                | Role::SubmitVars
                | Role::SubmitDone
                | Role::Text(_)
                | Role::KpiSize
                | Role::Placeholder
                | Role::HeaderImage
                | Role::FooterLink
                | Role::Download(_)
                | Role::Reload
                | Role::Range
                | Role::GaugeLabels
                | Role::GaugeColors
                | Role::Geometry
                | Role::Basemap
                | Role::Flip
                | Role::Alpha
                | Role::SyncZoom
                | Role::SyncFrame
                | Role::Projection
                | Role::Classes
                | Role::Midpoint
                | Role::Hexbin
                | Role::Frame
                | Role::Region
                | Role::Lat
                | Role::Lon
                | Role::Origin
                | Role::Dest
                | Role::Bivariate
                | Role::Boundaries
                | Role::Tab
                | Role::SubTab
                | Role::View
                | Role::ViewEnd
                | Role::ViewPlacement
                | Role::Fit
                | Role::Size
                | Role::YFormat
                | Role::XFormat
                | Role::DataLabels
                | Role::MarkArea
                | Role::VLineLabel
                | Role::RefLineLabel
                | Role::XBandFrom
                | Role::XBandTo
                | Role::XBandLabel
                | Role::Annotate
                | Role::Thresholds
                | Role::Markdown
                | Role::Image
                | Role::ImageDark
                | Role::Alt
                | Role::Open
                | Role::High
                | Role::Low
                | Role::Q1
                | Role::Median
                | Role::Q3
                | Role::WhiskerLow
                | Role::WhiskerHigh
                | Role::Mean
                | Role::Outliers
                | Role::YMin
                | Role::YMax
                | Role::XMin
                | Role::XMax
                | Role::Facet
                | Role::FacetFree
                | Role::FacetCols
                | Role::XScale
                | Role::YScale
                | Role::XBreaks
                | Role::YBreaks
                | Role::AbLine
                | Role::Identity
                | Role::LabelTop
                | Role::Rank
                | Role::Censor
                | Role::CooksContour
                | Role::SmoothMethod
                | Role::HistBins
                | Role::HistNorm
                | Role::Theme
                | Role::Locale
                | Role::DashboardTitle
                | Role::Brand
                | Role::Logo
                | Role::Favicon
                | Role::AsOf
                | Role::Info
                | Role::Callout
                | Role::MarkdownStyle
                | Role::Section
                | Role::SelectAs
                | Role::Drill
                | Role::Link
                | Role::Empty
                | Role::ShowIf
                | Role::Animate
                | Role::LineStyle
                | Role::Story
                | Role::StoryBy
                | Role::Transition
                | Role::HoverFocus
                | Role::Points
                | Role::NorthArrow
                | Role::Track
                | Role::Diagram
                | Role::Upstream
                | Role::Edges
                | Role::Direction
                | Role::Lane
                | Role::Legend
                | Role::GuideStep
                | Role::Gate
                | Role::GateMessage => {}
            }
        }
        v
    }

    #[test]
    fn every_role_variant_is_registered_once() {
        for r in all_roles() {
            let n = REGISTRY.iter().filter(|s| s.role == r).count();
            assert_eq!(n, 1, "{r:?} registered {n} times");
        }
        assert_eq!(all_roles().len(), REGISTRY.len());
    }
}
