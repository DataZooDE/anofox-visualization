# anofox-visualization — documentation

SQL-defined dashboards: you annotate SQL result columns with
**roles** (`::XAXIS`, `::CATEGORY`, a chart kind on the measure), and anofox-visualization
renders them with [ggplot-rs](https://github.com/sipemu/ggplot-rs). The core is dependency-light and
**wasm-compatible**, so the same renderer runs on the CLI and in the browser.

### DuckDB 2.0

DuckDB 2.0.0 LTS shipped 2026-10-21. It's supported now: the community/CMake
build (`csrc/` + `CMakeLists.txt`) compiles and passes its sqllogictests
against both v1.5.6 (`extension-ci-tools@v1.5-variegata`, the pinned,
blocking build) and v2.0.0 (`extension-ci-tools@v2.0-cyanoptera`, a
non-blocking CI job for now — see `.github/workflows/MainDistributionPipeline.yml`
and `.github/workflows/ci.yml`'s `duckdb-2.0-smoke` job). The C-API
full-toolkit build (`duckext/`) already loads unchanged on 2.0 (it targets
C-API v1.2.0, which is version-portable). `dashboard`/`serve` warn on stderr
if the `duckdb` CLI on PATH is a different major version than this build's
pin, since DuckDB's on-disk format is one-way across major versions (a 2.0
file can't be opened by a 1.x CLI).

This repository's own pin (`.duckdb-version`) stays at **v1.5.6** until
around **2.0.1** (planned 2026-11-18) — once duckdb-wasm 2.0 is on npm and
the other anofox_* extensions this one conformance-tests against
(statistics, optimize, tabular, forecast) have 2.0 builds. See issue #352 and
`research-duckdb-2.0.md` for the full migration plan.

---

## 1. The annotation model

A dashboard is a `.sql` script. Two kinds of statement:

- **Setup** — any statement *without* a role cast (`CREATE TABLE`, `INSTALL`,
  `SET`, …). Run for effect against a shared connection.
- **Panels** — a `SELECT` whose output columns carry `::ROLE` casts. Each becomes
  one chart.

### Roles

The table below is **generated from the role registry** (`src/roles.rs`, the
single source of truth — `dashboard --roles-md` prints it; a test keeps this
copy in sync). `dashboard --roles` prints the same list grouped for the
terminal.

<!-- roles-table:start -->
| Cast | Aliases | Category | Meaning |
|------|---------|----------|---------|
| `::XAXIS` | `::X` | encoding | x position |
| `::YAXIS` | `::Y` | encoding | y position (a heatmap's second axis) |
| `::CATEGORY` | `::SERIES`, `::COLOR`, `::COLOUR` | encoding | grouping / colour series (discrete) |
| `::LABEL` |  | encoding | section heading when alone; chart title / per-mark or per-feature label otherwise |
| `::TITLE` | `::HEADING` | encoding | a title bar above one panel |
| `::ALT` | `::ALT_TEXT` | encoding | an image's text alternative ('' = decorative); falls back to ::TITLE, then ::LABEL |
| `::INFO` | `::DESCRIPTION`, `::DEFINITION` | encoding | a definition behind an ⓘ on the panel's title bar (hover, focus or click) |
| `::OPEN` |  | encoding | candlestick open price |
| `::HIGH` |  | encoding | candlestick high price |
| `::LOW` |  | encoding | candlestick low price |
| `::SIZE` |  | encoding | bubble size for a scatter (maps a measure to point area); on a map a symbol's area (graduated symbols) or a flow's width; a constant (4::SIZE) sets the point size |
| `::Q1` | `::QUARTILE1`, `::LOWER_QUARTILE` | encoding | a box plot from statistics, one row per box: the lower quartile (with ::MEDIAN and ::Q3; no ::BOXPLOT) |
| `::MEDIAN` | `::Q2` | encoding | the median line of a box from statistics (see ::Q1) |
| `::Q3` | `::QUARTILE3`, `::UPPER_QUARTILE` | encoding | the upper quartile of a box from statistics (see ::Q1) |
| `::WHISKER_LOW` | `::WHISKER_LOWER`, `::LOWER_WHISKER` | encoding | the lower whisker end of a box from statistics (without it: none) |
| `::WHISKER_HIGH` | `::WHISKER_UPPER`, `::UPPER_WHISKER` | encoding | the upper whisker end of a box from statistics |
| `::MEAN` | `::AVERAGE` | encoding | a box's mean from statistics, drawn as a dot |
| `::OUTLIERS` | `::OUTLIER_LIST` | encoding | a box's outliers from statistics: a list of numbers, drawn as open circles |
| `::YMIN` | `::Y_MIN` | encoding | lower end of a y interval: a ::SCATTER becomes a pointrange, other measures get error bars |
| `::YMAX` | `::Y_MAX` | encoding | upper end of a y interval (see ::YMIN) |
| `::XMIN` | `::X_MIN` | encoding | lower end of a horizontal interval through each point |
| `::XMAX` | `::X_MAX` | encoding | upper end of a horizontal interval (see ::XMIN) |
| `::FACET` | `::FACET_WRAP`, `::PANEL_BY` | encoding | small multiples: one panel per distinct value, shared axes; on a map: one map per value, sharing projection, extent, classes and one legend |
| `::FACET_FREE` | `::FACET_WRAP_FREE` | encoding | small multiples with independent axes per panel (y only on a discrete x) |
| `::RANK` | `::LABEL_RANK` | encoding | score ranking points for ::LABEL_TOP (higher = labelled first) |
| `::CENSOR` | `::CENSORED`, `::N_CENSOR` | encoding | censoring marks (+) on a ::STEP curve where the value is > 0 / true (Kaplan-Meier) |
| `::UPSTREAM` | `::PARENTS` | encoding | a ::DIAGRAM node's input(s): an id or a list of ids → edges into the row's node |
| `::LANE` | `::SWIMLANE` | encoding | a ::DIAGRAM node's swimlane: nodes of one lane share a labelled band |
| `::EDGES` |  | encoding | a ::DIAGRAM's edges: (SELECT list({'from': a, 'to': b, 'value': n, 'label': t, 'style': 'flow', 'status': 'delayed'}) FROM e) — width by value, colour by status |
| `::BARCHART` | `::BAR` | chart | bar chart (dodged by CATEGORY) |
| `::BARCHART_STACKED` | `::BAR_STACKED`, `::STACKED_BAR` | chart | stacked bars (by CATEGORY) |
| `::BARCHART_PERCENT` | `::BAR_PERCENT` | chart | dodged bars as % of each x's total (fractions ≤ 1 are drawn as given) |
| `::BARCHART_STACKED_PERCENT` | `::BAR_STACKED_PERCENT` | chart | bars normalised to 100% per x |
| `::LINECHART` | `::LINE` | chart | line chart |
| `::LINECHART_PERCENT` | `::LINE_PERCENT` | chart | line chart, percent y-axis |
| `::STEP` | `::STEPLINE`, `::STEP_LINE` | chart | step line |
| `::SMOOTH` | `::TRENDLINE`, `::TREND_LINE` | chart | scatter + trend line (LOESS; ::SMOOTH_METHOD 'lm'/'gam') |
| `::AREACHART` | `::AREA` | chart | area chart |
| `::AREACHART_STACKED` | `::AREA_STACKED`, `::STACKED_AREA` | chart | stacked areas (by CATEGORY) |
| `::SCATTER` | `::POINT`, `::SCATTERCHART` | chart | scatter; add a ::SIZE column for a bubble chart |
| `::BUBBLE` | `::BUBBLECHART` | chart | bubble chart: alone = the y measure (size via ::SIZE); beside a ::SCATTER = its size |
| `::JITTER` | `::JITTERCHART`, `::STRIP` | chart | jittered scatter (reveals overlapping points) |
| `::PIE` | `::PIECHART`, `::PIECHART_PERCENT` | chart | pie — slices by CATEGORY, sized by the measure |
| `::DONUTCHART` | `::DONUT`, `::DONUTCHART_PERCENT` | chart | donut (pie with a hole) |
| `::GAUGE` | `::GAUGE_PERCENT` | chart | value as an arc toward ::RANGE 'min,max' (zones: ::COLORS, ::LABELS) |
| `::RADAR` | `::SPIDER` | chart | radar / spider chart — axes from XAXIS, one polygon per CATEGORY |
| `::WATERFALL` | `::BRIDGE` | chart | bridge from a start total through signed steps (NULL / ::CATEGORY 'total' rows = totals) |
| `::BULLET` | `::BULLETCHART` | chart | measure per XAXIS row vs a ::GOAL tick over ::RANGE bands '60,80,100' |
| `::FUNNEL` | `::FUNNELCHART` | chart | ordered XAXIS stages, widths by value, conversion % from the previous stage |
| `::HISTOGRAM` | `::HIST` | chart | histogram of the measure |
| `::DENSITY` | `::KDE` | chart | kernel density curve (one per CATEGORY) |
| `::BOXPLOT` | `::BOX_PLOT` | chart | box plot — XAXIS groups, measure on y (raw rows) |
| `::VIOLIN` | `::VIOLINPLOT` | chart | violin plot — XAXIS groups, measure on y (raw rows) |
| `::QQ` | `::QQPLOT` | chart | normal quantile-quantile plot with a 95% pointwise band |
| `::HEATMAP` | `::TILE`, `::TILES` | chart | tiles at XAXIS×YAXIS coloured by the measure (rows in SQL order; diverging around 0); a numeric measure sizes every tile alike regardless of how many x points each row has; a text measure (status words) takes ::COLORS as a level map for a categorical legend, like a ::CATEGORY column's |
| `::CALENDAR` | `::CALENDAR_HEATMAP`, `::CAL_HEATMAP` | chart | calendar heatmap (date XAXIS, ≤ 50 years) |
| `::CANDLESTICK` | `::CANDLE`, `::OHLC` | chart | OHLC candlesticks: XAXIS + ::OPEN/::HIGH/::LOW, close as the measure |
| `::SPARKLINE` | `::SPARK` | chart | minimal trend line (no axes); a list() column in a table; on a map: a mini chart in each feature's tooltip |
| `::SPARKBAR` | `::SPARK_BAR` | chart | minimal bar histogram (no axes); a table cell's list(v ORDER BY i), or two overlaid series sharing one y scale from list({'ref': r, 'cur': c} ORDER BY i); on a map: a mini chart in each feature's tooltip |
| `::MAP` | `::GEOMETRY`, `::GEO`, `::CHOROPLETH` | chart | WKT-geometry map, coloured by a measure |
| `::REGION` | `::REGION_CODE`, `::ISO_CODE` | chart | a map region by code, drawn from the built-in boundaries: ISO 3166-1 ('DE', 'DEU'), ISO 3166-2 ('DE-BY'), German AGS ('09162') or NUTS ('DE212'), else a name |
| `::LAT` | `::LATITUDE` | chart | a map point's latitude (degrees, WGS84), with ::LON |
| `::LON` | `::LONGITUDE`, `::LNG` | chart | a map point's longitude (degrees, WGS84), with ::LAT |
| `::ORIGIN` | `::FROM_PLACE` | chart | a flow map's start: a region code ('DE-BY'), a WKT POINT or {'lon': …, 'lat': …, 'name': …} — with ::DEST, width by ::SIZE |
| `::DEST` | `::DESTINATION`, `::TO_PLACE` | chart | a flow map's end (see ::ORIGIN) |
| `::BIVARIATE` | `::BIVAR` | encoding | a map's second measure: regions coloured by tertiles of both (a 3×3 palette and legend) — with the measure column |
| `::BASEMAP` | `::MAPBASE`, `::BACKDROP` | chart | a backdrop under a map: WKT, or a built-in layer 'world', 'de-states', 'de-districts' |
| `::DIAGRAM` | `::NODE` | chart | node/edge diagram, one row per node (this column = the id; ::LABEL, a KPI format, ::BADGE in the box; edges from ::UPSTREAM / ::EDGES) |
| `::REFLINE` | `::TARGET`, `::GOAL`, `::YLINE` | annotation | horizontal reference line per distinct value; in a KPI statement the KPI's goal → a progress bar; on a ::BULLET the per-row goal tick |
| `::XLINE` |  | annotation | vertical reference line at an x (a number, date or timestamp) |
| `::COOKS_CONTOUR` | `::COOKS` | annotation | Cook's distance contours (0.5, 1) on residuals vs leverage; value = the model's parameter count p |
| `::BAND_LOWER` | `::BANDLOWER` | annotation | lower edge of a shaded band around a line; several ::BAND_LOWER / ::BAND_UPPER pairs = a nested fan |
| `::BAND_UPPER` | `::BANDUPPER` | annotation | upper edge of a shaded band (pairs with the ::BAND_LOWER in the same position) |
| `::MARKAREA` | `::MARK_AREA`, `::SHADE` | annotation | shade each run of this column's x-values: CASE WHEN flag THEN day END shades every spell |
| `::XLINE_LABEL` | `::EVENT_LABEL` | annotation | label of the ::XLINE on the same row, at the top of the line (events from data) |
| `::REFLINE_LABEL` | `::YLINE_LABEL`, `::TARGET_LABEL` | annotation | label of the ::REFLINE on the same row, above its right end |
| `::XBAND_FROM` | `::SHADE_FROM` | annotation | start of a shaded x-range, one per row (a number, date or timestamp) |
| `::XBAND_TO` | `::SHADE_TO` | annotation | end of the shaded x-range started by ::XBAND_FROM on the same row |
| `::XBAND_LABEL` | `::SHADE_LABEL` | annotation | label of the shaded x-range on the same row |
| `::ANNOTATE` | `::ANNOTATION` | annotation | a text callout at the row's (x, y); NULL rows draw nothing |
| `::DATALABELS` | `::DATALABEL`, `::VALUELABELS`, `::SHOWLABELS` | annotation | draw the value on each mark or heatmap cell (value = font size) |
| `::ABLINE` | `::AB_LINE` | annotation | straight line y = slope·x + intercept per distinct 'slope,intercept' value |
| `::IDENTITY` | `::DIAGONAL`, `::IDENTITY_LINE` | annotation | dashed grey y = x line (predicted vs actual, calibration, QQ) |
| `::LABEL_TOP` | `::LABELTOP`, `::TOPLABELS` | annotation | label the top-k points (k = value) with the ::LABEL text, ranked by ::RANK or \|y\| |
| `::FLIP` | `::COORD_FLIP`, `::HORIZONTAL` | modifier | swap the axes (horizontal bars) |
| `::YFORMAT` | `::YAXISFORMAT`, `::YUNIT`, `::YCURRENCY` | modifier | value format — y ticks, tooltips, data labels, heatmap legend: '€', '$', 'percent' (values 0–100), 'comma', ' kg'… |
| `::XFORMAT` | `::XAXISFORMAT`, `::XUNIT`, `::XCURRENCY` | modifier | x-axis tick format (continuous x), like ::YFORMAT; on a date/timestamp x a strftime pattern: '%H:%M', '%-d %b', '%b %Y', '%a %H:%M' |
| `::XSCALE` | `::XTRANS` | modifier | x-axis transform: 'log10', 'sqrt' or 'reverse' |
| `::YSCALE` | `::YTRANS` | modifier | y-axis transform: 'log10', 'sqrt' or 'reverse' |
| `::XBREAKS` | `::X_BREAKS` | modifier | explicit x-axis tick positions, in data units — '100,1000,10000'; overrides the automatic ticks (and, under ::XSCALE, their log/sqrt round-number placement) |
| `::YBREAKS` | `::Y_BREAKS` | modifier | explicit y-axis tick positions, like ::XBREAKS |
| `::FACET_NCOL` | `::NCOL` | modifier | panels per row of a ::FACET / ::FACET_FREE |
| `::SMOOTH_METHOD` | `::METHOD` | modifier | trend line method of a ::SMOOTH: 'loess' (default), 'lm', 'gam' ('glm' = Gaussian lm) |
| `::HIST_BINS` |  | modifier | a ::HISTOGRAM's bin count ('20') or explicit edges ('0,10,20,30'), shared by every ::CATEGORY / ::FACET group — distinct from ::BINS, which aliases ::CLASSES (a map's colour-class breaks) |
| `::HIST_NORM` |  | modifier | a ::HISTOGRAM's per-group normalisation: 'share' draws each ::CATEGORY / ::FACET group's bars as a share of that group's own count (groups of unequal size stay comparable); default: raw counts |
| `::ALPHA` | `::OPACITY` | modifier | map layer opacity 0..1 |
| `::SYNC_ZOOM` | `::ZOOM_GROUP` | modifier | maps with the same value (e.g. 'routes') zoom and pan together in the browser |
| `::SYNC_FRAME` | `::FRAME_GROUP` | modifier | time maps (::FRAME) with the same value (e.g. 'weeks') step and play together in the browser, by frame label |
| `::XTITLE` | `::X_TITLE`, `::XLAB` | modifier | x-axis title, e.g. 'Residual load (GW)' — names the ::XAXIS variable, drawn under the x ticks (on a ::FLIP chart it moves with the x axis); charts with axes only |
| `::YTITLE` | `::Y_TITLE`, `::YLAB` | modifier | y-axis title, e.g. 'Day-ahead price (EUR/MWh)' — names the measure, drawn rotated beside the y ticks; charts with axes only |
| `::PROJECTION` | `::PROJ` | modifier | map projection: 'auto' (default, by the data's extent), 'equal-earth', 'mercator', 'albers-europe', 'conic', 'utm', 'none' (planar x/y) |
| `::CLASSES` | `::CLASS_BREAKS`, `::BINS` | modifier | map colour classes: 'quantile 5', 'jenks 5', 'equal 5' or edges '0,10,50,100' — a stepped legend with a count per class |
| `::MIDPOINT` |  | modifier | centre of a map's diverging colour ramp (default 0 when the values span it) |
| `::BOUNDARIES` | `::REGION_SET` | modifier | the built-in layer a ::REGION is looked up in: 'world', 'world-50m', 'de-states', 'de-districts' (default: the layer most codes match) |
| `::HEXBIN` | `::HEX` | modifier | aggregate a map's points into hexagons ('auto' or a width across, '25 km'; planar: '2.5'; sums the measure, else counts rows) or, on a planar ::SCATTER, bin the points into a 2D count density ('auto' or a bin count across each axis) |
| `::FRAME` | `::PLAY_BY`, `::TIME_FRAME` | encoding | a time map's (or a bar/line/area/scatter chart's) frame key (a week, a date): one result holds every frame; Play steps through them with shared colours, classes, sizes and axes; static output shows the last frame |
| `::THRESHOLDS` | `::STATUS_RULES` | modifier | status rules '<0:bad; <0.05:warn; good' — colour a KPI value, gauge zones, chart threshold lines, a ::HEATMAP's tiles in steps; alone: the next table's 'column: rules \| …' |
| `::RANGE` |  | modifier | gauge domain 'min,max' (default 0,100); bullet band edges '60,80,100'; a map's colour domain 'lo,hi' (caps outliers) |
| `::COLORS` | `::COLOURS` | modifier | gauge zone colours, comma-separated hex; beside a ::CATEGORY (a chart or a ::MAP) a level map 'Critical:bad; Watch:warn; OK:good' (status colours or #rrggbb) |
| `::LABELS` |  | modifier | gauge zone labels, comma-separated (drawn at each zone) |
| `::DIRECTION` |  | modifier | a ::DIAGRAM's direction: 'LR' (left to right, default) or 'TB' (top to bottom) |
| `::ANIMATE` | `::ANIMATION`, `::ENTRANCE` | modifier | entrance animation: 'auto' (lines draw, bars grow, points stagger), 'draw', 'grow', 'reveal' (forward from the ::XLINE, e.g. Today) or 'none' |
| `::STORY` | `::STORYLINE`, `::NARRATIVE` | annotation | per row: a caption → a story step that lights up the row's x (Step / Play controls, the rest dims) |
| `::STORY_BY` | `::STORYBY` | modifier | what a ::STORY step lights up: 'x' (default) or 'series' (the row's ::CATEGORY) |
| `::TRANSITION` | `::TWEEN` | modifier | on an input / cross-filter re-run: 'tween' moves the marks to their new values (≤ 300 ms), 'none' (default) swaps the chart |
| `::HOVER_FOCUS` | `::HOVERFOCUS` | modifier | hovering a mark: 'series' (default) emphasises its whole series and fades the others, as hovering its legend entry does; 'none' keeps the plain hover (dense charts) |
| `::POINTS` | `::MARKERS` | modifier | line/area point markers: 'auto' (default) hides them on a dense series (x positions under 3 px apart) and keeps them as hover/click targets, 'none' always hides them, 'always' always draws them |
| `::NORTH_ARROW` | `::NORTH`, `::NORTHARROW` | modifier | a map's north arrow, pointing to true north: 'print' (only in print, PNG, PDF and --svg output; hidden on screen) or 'always' |
| `::LINESTYLE` | `::LINE_STYLE` | modifier | per row, by ::CATEGORY: 'flow' = moving dashes (a forecast, "predicted"; still dashes in static output), 'dashed', 'solid' |
| `::LEGEND` |  | modifier | a chart's legend / colourbar: 'none' hides it, or 'top'/'right'/'bottom'/'left' moves it; without it, unchanged |
| `::METRIC` | `::KPI`, `::BIGNUMBER` | kpi | big-number KPI (add ::LABEL for a caption) |
| `::MONEY` | `::DOLLAR`, `::CURRENCY` | kpi | currency KPI / table column format |
| `::PERCENT` | `::PCT` | kpi | percent KPI / table column format; the value is in percent (12.5 → 12.5%; a fraction × 100) |
| `::COMPACT` |  | kpi | compact-number KPI / table column format (1.2K) |
| `::DELTA` | `::COMPARE`, `::PREVIOUS` | kpi | comparison value → trend arrow + % on a KPI (up = good) |
| `::DELTA_LOWER_GOOD` | `::COMPARE_LOWER_GOOD`, `::DELTA_INVERSE` | kpi | comparison value where lower is better (churn, cost): a rise is drawn red |
| `::DELTA_FORMAT` | `::DELTAFORMAT` | kpi | how a KPI's change reads: 'pct' (default), 'abs' (in the KPI's format: +$1,200, +2.3 pp) or 'both' |
| `::DELTA_LABEL` | `::DELTALABEL`, `::COMPARE_LABEL` | kpi | what the change compares against, shown after it: 'vs last month', getvariable('p_prev_label') |
| `::KPI_SIZE` | `::KPI_DENSITY` | kpi | KPI tile density: 'compact' (single line, value + label, no ::SPARKLINE/::GOAL) or 'normal' (default); on ::METRIC it sets that tile, on ::GROUP every tile in the box |
| `::TEXT_SMALL` |  | kpi | small text card |
| `::TEXT_MEDIUM` |  | kpi | medium text card |
| `::TEXT_LARGE` |  | kpi | large text card |
| `::MARKDOWN` | `::MD`, `::TEXTBOX`, `::RICHTEXT` | kpi | a Markdown box (browser); {{col}} / {{col:fmt}} fill in values from its own row; $…$ / $$…$$ TeX formulas; ![alt](src) images use the same source rules as ::IMAGE |
| `::CALLOUT` | `::ADMONITION` | kpi | draw the ::MARKDOWN box as a callout: 'note', 'warning' or 'success' |
| `::MARKDOWN_STYLE` | `::MD_STYLE` | kpi | a ::MARKDOWN box's chrome: 'plain' (page background, no card/tint/bar), 'card' (plain white card) or 'callout' (today's tinted box, the default; ::CALLOUT implies it); 'caption' puts the text under the chart right before it, in its card; a dashboard-wide default comes from -- @markdown_style |
| `::MARKDOWN_TONE` | `::MD_TONE` | kpi | a quiet accent on a 'card' ::MARKDOWN box: 'info', 'example', 'tip', 'caution', 'details' or 'neutral' (none) — a thin coloured bar plus a small label; 'example: Beispiel' sets the label, 'example:' drops it; 'details' folds (the label opens it, in the browser); implies 'card' |
| `::IMAGE` | `::PICTURE`, `::IMG` | kpi | a static picture panel: 'assets/x.svg', data:image/… or https:; ::ALT is its text alternative, ::IMAGE_DARK a dark-theme variant, ::LABEL a caption |
| `::IMAGE_DARK` | `::IMG_DARK` | kpi | the ::IMAGE shown while the dashboard is dark |
| `::TABLE` | `::GRID` | table | the whole result as a table (one marker per panel) |
| `::PAGED` | `::TABLE_PAGED`, `::PAGINATED` | table | SQL-paginated table for large/remote data |
| `::TREND` |  | table | ▲/▼ arrow in a table cell |
| `::COLORSCALE` | `::COLOURSCALE`, `::HEAT`, `::GRADIENT` | table | heatmap-colour a table column's cells (low = green, high = red); on a ::HEATMAP / ::CALENDAR its tiles, good → warn → bad |
| `::COLORSCALE_HIGH_GOOD` | `::COLORSCALE_HIGH`, `::HEAT_HIGH` | table | heatmap-colour a table column's cells (or a ::HEATMAP's tiles), high = green (scores, accuracy) |
| `::COLORSCALE_DIVERGING` | `::DIVERGING`, `::HEAT_DIVERGING` | table | colour a table column's cells (or a ::HEATMAP's tiles) around zero: negative red, positive green (deltas) |
| `::LINK` | `::DRILLTHROUGH`, `::DRILL_THROUGH`, `::HREF` | table | drill-through on a chart click / a table link cell: '#Tab', '?v.x={value}', '/d/<id>?v.x={value}' or an https: URL ({value}, {column} placeholders) |
| `::BADGE` | `::STATUS`, `::PILL` | table | render a table column as status pills |
| `::PLAIN` | `::NOBAR` | table | a plain table column (no in-cell bar) |
| `::DOWNLOAD_CSV` |  | table | CSV export button |
| `::DOWNLOAD_XLSX` | `::DOWNLOAD_EXCEL` | table | Excel export button |
| `::DOWNLOAD_PDF` |  | table | print-to-PDF button |
| `::DROPDOWN` | `::OPTIONS`, `::SELECT_INPUT` | input | single-select → a DuckDB variable |
| `::MULTISELECT` | `::MULTI` | input | multi-select → a list variable |
| `::NUMBER` | `::SLIDER`, `::NUMERIC` | input | numeric input (value = default) |
| `::DATE` | `::DATEPICKER` | input | date picker (value = default) |
| `::TEXT` | `::SEARCH`, `::STRING` | input | free-text input (value = default) |
| `::DATERANGE` | `::DATE_RANGE` | input | two date columns → from/to variables |
| `::PERIOD` | `::PERIOD_PICKER`, `::DATE_PERIOD` | input | date presets + grain + comparison → <name>_from/_to/_prev_from/_prev_to/_grain/_compare/_label/_prev_label |
| `::BUTTONS` | `::BUTTON_GROUP`, `::SEGMENTED` | input | segmented button group: one of a few options → a DuckDB variable (a metric / dimension switcher) |
| `::TOGGLE` | `::SWITCH`, `::CHECKBOX` | input | on/off switch → a BOOLEAN variable (value = default) |
| `::FORM` | `::PARAM_FORM`, `::PARAMETERS` | input | a form generated from rows, one per field: the ::FORM column is the field key (its alias names the variable), columns type (number/slider/dropdown/toggle/date/text), value, options, min, max, step, label, unit, help, show_if ('model=normal'), source; the values come back as one JSON object: getvariable('name')->>'field' |
| `::HINT` |  | input | per-option hint next to a dropdown option |
| `::OPTION_LABEL` |  | input | the shown text of a dropdown / button / multiselect option (the variable keeps the value) |
| `::DEFAULT` | `::IS_DEFAULT` | input | true on the default option of a dropdown / buttons (the first marked row) or the default picks of a multiselect (every marked row); without it: the first option / all |
| `::DROPDOWN_STYLE` |  | input | forces a ::DROPDOWN's widget: 'search' (searchable combobox) or 'native' (plain <select>), overriding the option-count threshold |
| `::SUBMIT` |  | input | a button (value = its label) that records SUBMIT_VARS for this viewer, append-only — requires a locked server with options.state naming this dashboard (#517); disabled, with a visible reason, on a published static site, a server without options.state, or the wasm/live-authoring builder's own preview session (preview submissions stay in the tab, never reach a server) |
| `::SUBMIT_VARS` |  | input | comma-separated variable names the preceding ::SUBMIT records, each declared by one of this dashboard's own input statements |
| `::SUBMIT_DONE` |  | input | optional confirmation text shown after a successful ::SUBMIT |
| `::COLUMNS` | `::COLS` | layout | default panels per row |
| `::SPAN` | `::COL`, `::WIDTH` | layout | next panel's width (of 12) |
| `::HEIGHT` | `::TALL` | layout | next panel's height in px |
| `::ROWSELECT` | `::ROW_SELECT`, `::SELECTABLE` | layout | next table's row selection: 'none' (default), 'single' or 'multi' — a click sets getvariable('<first column>') |
| `::TOTALS` | `::TOTAL`, `::AGGREGATE` | layout | next table's totals: 'sum'\|'mean'\|'median'\|'min'\|'max'\|'count'\|'none', or per column 'Revenue: sum; Per 10k: none' |
| `::COLGROUP` | `::COL_GROUP`, `::HEADER_GROUP` | layout | next table's grouped header: 'Backtest: mae, rmse \| Holdout: mae_h, rmse_h' |
| `::FLAG` | `::THRESHOLD` | layout | next table: flag cells meeting a condition with ⚠ — 'mape > 0.2, bias < -5' |
| `::HIGHLIGHT` | `::TOPN` | layout | next table: emphasise the top / bottom n of a column — 'Revenue: top 3, Margin: bottom 2' |
| `::SUBTOTAL` | `::SUBTOTALS`, `::GROUP_TOTAL` | layout | next table: a subtotal row after each group of a column — 'region' (not for ::PAGED) |
| `::TOOLBAR` |  | layout | next table's toolbar: 'none' hides it entirely (row count and chips too; a multi-select's "N selected" chip stays), or an allow-list of the parts to keep — 'search,filters,totals,stats,columns,density,export' ('totals' only hides its toggle button, not the row; 'density' covers Pages\|Scroll too; each kept part still needs its own row/column threshold); an excluded 'export' still gets a small hover CSV button |
| `::SELECT_AS` | `::SELECTAS`, `::SELECTION` | layout | next chart's named selection: 'region' → a click sets getvariable('region'); 'region multi' → a list (shift/ctrl-click adds); a brush on a continuous x sets region_from / region_to |
| `::EMPTY` | `::EMPTY_MESSAGE`, `::NODATA` | layout | next panel's message when its result is empty: 'No orders in range — widen the dates' |
| `::SHOW_IF` | `::SHOWIF`, `::VISIBLE_IF` | layout | show the next panel (or ::GROUP box) only while this BOOLEAN is TRUE — re-checked on every input change: getvariable('q1') <> '' |
| `::DRILL` | `::DRILLDOWN`, `::DRILL_DOWN` | layout | next chart drills down 'region > country > city' ('geo: …' names the variables): drill_level / drill_depth / drill_path + a variable per level, a breadcrumb back |
| `::GROUP` | `::BOX`, `::ROW` | layout | open a box; ::ENDGROUP closes it |
| `::ENDGROUP` | `::ENDBOX`, `::ENDROW` | layout | close the current box |
| `::DIAGRAM_MODE` | `::DIAGRAM_EDIT` | encoding | on a ::DIAGRAM: 'edit' lets the viewer add, connect, rename and remove nodes; the alias names the variable holding the edited graph as JSON {nodes:[{id,type,label}], edges:[{from,to}]} |
| `::DIAGRAM_PALETTE` | `::NODE_TYPES` | layout | before an editable ::DIAGRAM: the node types it may add — the column is the type (its ::CATEGORY), plus label and connects_to (the types it may feed) |
| `::DIAGRAM_CHECK` | `::GRAPH_CHECK` | layout | before an editable ::DIAGRAM: a query over the edited graph returning its problems — the column is a node id, 'from -> to' for an edge or NULL for the whole graph, plus status ('bad', 'warn', …) and message; drawn on the nodes / edges and listed in the toolbar |
| `::FIGURE` | `::PATCHWORK` | layout | open a figure: the charts up to ::ENDFIGURE become the cells of one patchwork-style picture with aligned axes; the value is its title |
| `::ENDFIGURE` | `::END_FIGURE` | layout | close the current ::FIGURE |
| `::LAYOUT` | `::FIGURE_LAYOUT` | layout | on ::FIGURE: '(hist / scatter) \| checks' — \| side by side, / stacked (binds tighter), ( ) groups, _ an empty cell; a cell is its -- @name or its position (1, 2, …) |
| `::WIDTHS` | `::FIGURE_WIDTHS` | layout | on ::FIGURE: relative widths '1,2' — of every side-by-side group with that many cells |
| `::HEIGHTS` | `::FIGURE_HEIGHTS` | layout | on ::FIGURE: relative heights '1,4' — of every stacked group with that many cells |
| `::AXES_COLLECT` | `::COLLECT_AXES` | layout | on ::FIGURE: 'x' (stacked charts share one x axis, drawn once at the bottom), 'align' (line up the plot areas only) or 'none' (default) |
| `::COLLECT_LEGENDS` | `::COLLECT_GUIDES` | layout | on ::FIGURE: true draws identical legends once, beside the figure |
| `::TAG_LEVELS` | `::FIGURE_TAGS` | layout | on ::FIGURE: tag the cells 'A' (A, B, …), 'a', '1', 'i' or 'I' |
| `::FIGURE_NCOL` |  | layout | on ::FIGURE without ::LAYOUT: the cells fill a grid this many columns wide |
| `::TAB` | `::PAGE` | layout | start a tab/page |
| `::SUBTAB` | `::SUB_TAB` | layout | a nested tab inside a ::TAB |
| `::GUIDE_STEP` | `::TUTORIAL_STEP`, `::WIZARD_STEP` | layout | a step of a guided sequence (onboarding, tutorials): Back / Next, 'Step 2 of 5', ?v.<name>=n; the alias names the step variable (default 'step') |
| `::GATE` | `::STEP_GATE`, `::NEXT_IF` | layout | on a ::GUIDE_STEP: Next stays disabled until this BOOLEAN is TRUE |
| `::GATE_MESSAGE` | `::GATE_HINT` | layout | on a ::GUIDE_STEP: why Next is disabled |
| `::VIEW` | `::ALT_VIEW` | layout | one of several alternative views of one panel: consecutive `::VIEW` sections share a segmented switcher; `'Lines' AS name ::VIEW` names the group (`?view.name=`) |
| `::ENDVIEW` | `::END_VIEW` | layout | close a `::VIEW` group (`::TAB`, `::SUBTAB`, `::SECTION` also close it) |
| `::VIEW_PLACEMENT` |  | layout | a ::VIEW group's switcher placement: 'row' (default, its own row above the card) or 'header' (right-aligned, compact, in the card's title row — folds into a dropdown below a minimum card width; only when every alternative is exactly one panel, else --check warns and it falls back to 'row'); set directly before the group's first ::VIEW marker, like ::COL/::SPAN |
| `::FIT` | `::FIT_SCREEN` | layout | this ::TAB / ::SUBTAB / ::VIEW (no tabs: the whole dashboard) fits the window while shown — rows scale to fill it, no page scroll ('fit'::FIT; ?fit=1 / ?fit=0 override it; ignored below 600 px wide) |
| `::PLACEHOLDER` |  | layout | an empty grid cell |
| `::TRACK` | `::STRIP_BELOW`, `::STRIP_UNDER` | layout | a thin panel under the previous chart, sharing its x axis (value = its name): ticks, ::HEATMAP tiles per ::YAXIS lane, or a line / bar |
| `::RELOAD` | `::REFRESH` | chrome | auto-refresh interval (seconds) |
| `::HEADER_IMAGE` | `::HEADERIMAGE` | chrome | banner image URL |
| `::FOOTER_LINK` | `::FOOTERLINK` | chrome | link at the bottom |
| `::SECTION` | `::SECTION_HEADING` | layout | a section heading that always starts a new row |
| `::THEME` |  | chrome | dashboard theme: 'light'/'dark'/'print'/'colorblind'/'high-contrast', a stack 'acme+dark', or JSON |
| `::LOCALE` |  | chrome | number/date locale for KPIs, tables and axes: 'en-US', 'en-GB', 'en-IE' (English with €), 'de-DE', 'fr-FR' |
| `::DASHBOARD_TITLE` | `::DASHBOARDTITLE`, `::PAGE_TITLE` | chrome | dashboard title: page header, browser tab and export title |
| `::BRAND` | `::BRAND_NAME` | chrome | brand / organisation name shown above the dashboard title |
| `::LOGO` |  | chrome | header logo image URL (relative, https: or data:image/) |
| `::FAVICON` |  | chrome | browser-tab icon URL |
| `::ASOF` | `::AS_OF`, `::FRESHNESS`, `::DATA_AS_OF` | chrome | data freshness: a 'Data as of 14:02 (2 h ago)' chip in the header; a 'stale=24h' column turns it amber |
<!-- roles-table:end -->

**Inside a `::TABLE`/`::PAGED` panel** the per-column roles `::MONEY`,
`::PERCENT`, `::COMPACT`, `::METRIC` (number formats), `::TREND` (▲/▼),
`::COLORSCALE`, `::BADGE`, `::SPARKLINE` (a `list()` column) and `::PLAIN`
format that column; `::TITLE` becomes the table's title bar.

**`::TITLE`** is a panel's title bar in every output: the browser, the locked
server, `dashboard --svg`, the static HTML of `dashboard file.sql out.html` /
`serve --dashboards` and `anofox_render`. `anofox_render`, `dashboard --svg`
and the static pages draw its first non-NULL value, trimmed (a blank one
draws none), in a 26 px bar above the chart (the chart gets 26 px less); a
KPI tile has none (its caption is `::LABEL`). The static pages cut a title
longer than 200 characters to 200 and an ellipsis.

**Percent units.** `::PERCENT` (KPI tiles and table columns),
`'percent'::YFORMAT` / `::XFORMAT` (axes, tooltips, data labels) and
`{{x:percent}}` in Markdown all take the value **in percent, 0–100**: `12.5`
reads `12.5%`. A fraction such as a rate from `avg(flag::INTEGER)` (`0.287`)
would read `0.3%` — multiply it: `round(100 * avg(late::INTEGER), 1) ::PERCENT`.
Only the share charts (`::BARCHART_PERCENT`, `::BAR_STACKED_PERCENT`, a
fractional `::LINECHART_PERCENT`) compute their own shares.

The cast on the **measure** column selects the geom; `XAXIS`/`CATEGORY` position
and colour it; `LABEL` alone becomes a **spanning section heading** (not a card).
Measures are cast to `DOUBLE` automatically (so `sum()`/`BIGINT`/`HUGEINT` render
numerically everywhere).

### Inputs & parameters (dropdowns from SQL)

A `SELECT … ::DROPDOWN` becomes a dropdown control: the query's values are the
options, and the **output column name is a DuckDB variable** you read elsewhere
with `getvariable('name')`. Changing the control re-runs the dashboard.

```sql
SELECT DISTINCT channel::DROPDOWN FROM sessions ORDER BY channel;  -- variable `channel`

SELECT week::XAXIS, sum(n)::BARCHART
FROM sessions WHERE channel = getvariable('channel') GROUP BY ALL ORDER BY week;
```

A `::LABEL` column sets the control's caption (default: the variable name), so
the variable can stay a short identifier:

```sql
SELECT 50 AS refund_at_pct ::NUMBER, 'Refund threshold (%)'::LABEL;
```

An `::OPTION_LABEL` column gives each option of a `::DROPDOWN`, `::BUTTONS` or `::MULTISELECT` the text it shows (and a long multiselect's search reads it), while the variable, `?v.name=` and a locked server's `path_vars` keep the value; a NULL or empty label shows the value. Put the input column first. Other inputs ignore it.

```sql
SELECT run AS run ::DROPDOWN,
       strftime(written, '%d %b %Y %H:%M') || ' — ' || run AS run_label ::OPTION_LABEL,
       'Run'::LABEL
FROM runs ORDER BY written DESC;
```

A `::DEFAULT` column picks the default without reordering the options (#638):
true on the default row of a `::DROPDOWN` / `::BUTTONS` (the first true row
wins), on every default pick of a `::MULTISELECT`. No true row keeps the
first option / every option. A `?v.name=` seed and the viewer's own pick
still win over it, and the static renders bind it too. On any other input
it's a warning (`input/default-ignored`): their own value is the default.

```sql
SELECT w AS ov_window ::BUTTONS, w = '7 days' AS d ::DEFAULT
FROM (VALUES ('1 day', 1), ('7 days', 2), ('30 days', 3), ('90 days', 4)) t(w, n) ORDER BY n;
SELECT key AS sources ::MULTISELECT, key IN ('solar', 'wind_onshore') AS d ::DEFAULT FROM series;
```

An input renders **where it appears in the script**: before the first `::TAB`
it sits above the tabs, after a `::TAB` (or `::SUBTAB`) inside that tab, shown
only while the tab is. Put an input in the tab whose panels it changes, and
keep only those that act on every tab (or on a KPI row above the tabs) at the
top. Its variable is global either way — any panel can read it, it lands in
the URL as `?v.name=` — and a `#Tab?v.name=…` link still sets it.

A `SET VARIABLE` of your own can derive from an input's variable —
`SET VARIABLE sel = coalesce(nullif(getvariable('channel'), ''), 'all')` —
**wherever it sits in the script**, before or after the input. Every viewer
value (an input's pick, a `?v.name=` seed, a cross-filter/selection) is
applied before any of the dashboard's own `SET VARIABLE`s run, the same
order the browser, the builder and a locked server all use — so the
derived variable always sees the current pick, never the input's previous
one (#416).

An input's own **OPTIONS query** (or default) can likewise read another
input's variable — a dropdown narrowed by a search box's text, say —
**wherever the two sit relative to each other in the script**: every other
input's known value (a previous pick, a `?v.name=` seed) is applied before
any input's OPTIONS query runs, so it always filters by the viewer's
current value, not a run-stale one — and never, unconditionally, on a
locked server reading it for the very first time, since a fresh connection
per request gets no second chance to catch up (#428). When an input's
*current* pick then falls out of its own, narrower, list — the search box
above excluding the picked value — it falls back to the **first remaining
option** (a `::MULTISELECT`'s several picks instead keep whichever of them
are still offered, dropping the rest); nothing errors or stays on a value
no longer offered.

```sql
SELECT DISTINCT region::DROPDOWN FROM sales;          -- above the tabs: every tab
SELECT 'Detail'::TAB;
SELECT 10 AS min_n ::NUMBER, 'Minimum orders'::LABEL;  -- only on "Detail"
```

Inputs work in the **browser builder** and **`serve`** (they re-query on change).
The static outputs (`dashboard file.sql`, `dashboard --svg`, the view-only
`serve --dashboards` page) draw no control but bind each input's **default**
(the first option — or the `::DEFAULT`-marked one(s) — every option of a `::MULTISELECT`, both ends of a
`::DATERANGE`, the default period of a `::PERIOD`), so `getvariable()` panels
render as on first load in the browser.

### Forms from rows (`::FORM`)

When every node of a model has its own typed parameters — a supplier's
lead-time model, a stocking point's policy — one hand-written input per node
per parameter doesn't scale. `::FORM` (#683) builds a form from a query that
returns **one row per field**, and returns the values as **one** variable
holding a JSON object:

```sql
SELECT field AS lead_time ::FORM,   -- the field key; the alias names the variable
       type, value, options, min, max, step, label, unit, help, show_if, source
FROM node_params
WHERE node_id = getvariable('node_id')        -- e.g. the clicked row / node
ORDER BY ord;

SELECT (getvariable('lead_time')->>'mean')::DOUBLE AS mean ::METRIC, 'Mean' ::LABEL;
```

| Column | Meaning |
|---|---|
| (the `::FORM` column) | The field's key — its name in the JSON object. |
| `type` | `number` (default), `slider` (needs `min`/`max`), `dropdown` (needs `options`), `toggle`, `date`, `text`. |
| `value` | The field's current value — the variable's starting point. |
| `options` | A dropdown's choices: a list (`['fixed', 'normal']`) or comma-separated text. |
| `min` / `max` / `step` | A number's / slider's range and step. |
| `label`, `unit` | Shown left of / right of the control (default label: the key). |
| `help` | An ⓘ beside the label with this text. |
| `show_if` | `'model=normal'` or `'model=normal\|fitted'`: the field shows only while that other field has one of those values. |
| `source` | Provenance under the control: "fitted 2026-10-20 from 214 POs", "manual". |

Only the `::FORM` column and `value` are required; the others are read by
name (any case). Numbers and toggles are JSON numbers / booleans in the
variable, everything else text: read them with `->>` and cast,
`(getvariable('lead_time')->>'mean')::DOUBLE`. An edit re-runs the panels
that read the variable and is kept in the URL (`?v.lead_time={…}`). When
the form's own rows change — another node picked — its values start again
from the new rows.

The form is a card in the grid (`::COL` before it sets its width). `--check`
reports an unknown `type`, a dropdown without `options`, a `show_if` naming
no field or written wrongly, a missing `value` column (errors) and a value
outside its options or min/max, a slider without a range, a repeated key
(warnings). The static outputs bind the rows' own values, like any input's
default.

**On a locked server** the variable travels like any other string variable
(the JSON text, max 4 KB). `min`/`max`/`options` are checked in the page,
not by the server — as with `::TEXT`, a viewer could send any text, so the
dashboard's SQL must treat the values as untrusted input (cast them, clamp
them) — never build SQL text from them. `::SUBMIT` can't record a form yet;
shared, saved edits are #684.

### Recording answers (`::SUBMIT`, `viewer_state`, #517)

Inputs change what a dashboard shows; `::SUBMIT` *records* what a viewer
picked, append-only, one row per recorded variable per click — a quiz a
learner answers, a check-in a field worker files, any form whose answers
need to outlive the page. It needs a **locked server** (`serve --locked`)
with `options.state` naming this dashboard — the browser builder and
`serve`'s live-authoring backend only ever preview it (below).

```sql
SELECT q1 ::BUTTONS FROM (VALUES ('A'), ('B'), ('C')) t(q1);
SELECT '' AS note ::TEXT, 'Anything else?' ::LABEL;

SELECT 'Check my answer' AS quiz_1 ::SUBMIT,       -- the button; column name = submit id
       'q1, note'        AS r      ::SUBMIT_VARS,  -- which inputs it records (≤ 32 names)
       'Saved — thanks.' AS d      ::SUBMIT_DONE;  -- optional text shown after a successful click
```

- The `::SUBMIT` statement's own `SELECT` is the author's gate: zero rows
  hides the button entirely (an "only once", "after step 1" or "until a
  date" condition in SQL), exactly one row shows it (its value is the
  label), more than one row is a lint/server error. It runs **as the
  viewer** — same variables, claims and `-- @requires`/tab gating as every
  other panel — so what the page shows and what the server accepts always
  agree.
- Only `::DROPDOWN`, `::BUTTONS`, `::MULTISELECT`, `::TOGGLE`, `::NUMBER`,
  `::DATE` and `::TEXT` inputs can be named in `SUBMIT_VARS` (`::DATERANGE`/
  `::PERIOD` cover several variables each and are refused, a lint error).
  The server re-runs each named input as the viewer and checks the
  recorded value is one of its real options (or, for a credential-locked
  variable — an embed link's own fixed `vars`, say — records that locked
  value instead, list or scalar alike, never the viewer's own pick).
- Re-submitting appends a new row; it never overwrites. `note` (`::TEXT`)
  is capped at `options.state.text_max_chars` (default 1000) and is the
  one input kind worth a moment's thought before showing it back on a
  trainer's page (see the CSV/XLSX and markup notes in
  `docs/secure-serving.md`'s state section).
- **Reading it back**: a dashboard named in `options.state.dashboards`,
  `.read` or `.trainers.dashboards` gets two server-provided temp objects,
  scoped by construction — the rows inserted into *this* connection are the
  only rows it can ever see, never a `WHERE __claim_user = …` filter:
  ```sql
  -- viewer_state: seq, ts, "user", dashboard, submit, name, value, value_list
  -- viewer_state_latest: the same columns, one row per (user, dashboard, submit, name) — the current answer
  SELECT dashboard::XAXIS, count(*)::BARCHART FROM viewer_state GROUP BY ALL ::TABLE;
  ```
  A plain state dashboard sees only the signed-in viewer's own rows (a "my
  progress" page can read several dashboards' state). A
  `options.state.trainers` dashboard additionally shows **every** viewer's
  rows, but only to a viewer in `trainers.groups`/`trainers.users` — anyone
  else opening the same trainer dashboard still gets only their own rows
  (fails safe, never refused).
- **Preview (browser builder, and `serve`'s live-authoring backend)**:
  clicking appends to an in-page `viewer_state` the preview session creates
  empty on load, with a banner saying submissions stay in the tab and are
  never saved — useful for authoring the SQL above, never a substitute for
  a real server when checking who can see what.
- Full contract, limits, logging and the threat model: `docs/secure-serving.md`
  → "Per-viewer state (`options.state`, #517)". Retention/erasure is the
  operator's job — `dashboard state export|erase`, same document.

### Periods: presets, grain and comparison (`::PERIOD`)

One input for "this period vs the previous one": a date range from presets
(or a custom range), a time grain and a comparison. The statement is a spec
string; the column's **alias is the variable prefix**:

```sql
SELECT 'presets=7d,30d,90d,mtd,qtd,ytd,last_month; default=30d; compare=prev' AS p ::PERIOD,
       'Period'::LABEL;

CREATE OR REPLACE MACRO in_period(d) AS d BETWEEN getvariable('p_from')::DATE AND getvariable('p_to')::DATE;
CREATE OR REPLACE MACRO in_prev(d)   AS d BETWEEN getvariable('p_prev_from')::DATE AND getvariable('p_prev_to')::DATE;

SELECT sum(revenue) FILTER (WHERE in_period(day))::MONEY,
       sum(revenue) FILTER (WHERE in_prev(day))::DELTA, 'Revenue'::LABEL FROM orders;
SELECT date_trunc(getvariable('p_grain'), day)::DATE ::XAXIS, sum(revenue)::LINECHART,
       'Revenue per ' || getvariable('p_grain')::TITLE
FROM orders WHERE in_period(day) GROUP BY ALL ORDER BY 1;
```

The control is a preset menu (*Last 30 days*, *Month to date*, …, *Custom range*
with two date pickers), a Day / Week / Month / Quarter switch, a comparison menu
(*No comparison* / *Previous period* / *Same period last year*) and the resolved
range as text. It sets eight variables, all **text** (cast dates with `::DATE`;
ranges are inclusive):

| Variable | Value |
|---|---|
| `<p>_from`, `<p>_to` | the selected range, `YYYY-MM-DD` |
| `<p>_prev_from`, `<p>_prev_to` | the comparison window — `NULL` with *No comparison* (a `::DELTA` over it is then empty) |
| `<p>_grain` | `day` \| `week` \| `month` \| `quarter` — for `date_trunc(getvariable('<p>_grain'), d)` |
| `<p>_compare` | `none` \| `prev` \| `yoy` |
| `<p>_label` | the range in words: `Sep 2026`, `Q3 2026`, `Sep 1 – 15, 2026` |
| `<p>_prev_label` | `vs Aug 2026`, `vs Aug 2 – 31, 2026` (`NULL` without a comparison) |

Spec keys (`key=value; …`, all optional — an empty string gives presets
`7d,30d,90d,mtd,qtd,ytd`, the first as default, an automatic grain and
`compare=prev`):

| Key | Values |
|---|---|
| `presets` | comma list: `Nd` / `Nw` / `Nm` (the last N days / weeks / months, ending today), `today`, `yesterday`, `wtd` (ISO week, from Monday), `mtd`, `qtd`, `ytd`, `last_week`, `last_month`, `last_quarter`, `last_year`; *Custom range* is always offered |
| `default` | the preset selected first (default: the first one); `custom` with `from=` / `to=` |
| `from`, `to` | an initial custom range (`YYYY-MM-DD`) |
| `grain` | `auto` (default: ≤ 31 days daily, ≤ 120 weekly, ≤ 730 monthly, else quarterly), `day`, `week`, `month`, `quarter` |
| `grains` | the grains the switch offers (`day,week,month`); `none` hides the switch |
| `compare` | `prev` (default), `yoy`, `none` |
| `today` | pin "today" (reproducible examples / tests); default: the viewer's local date in the browser, the UTC date headless |

The comparison window: for a rolling preset (`30d`) and a custom range, the
window of the same length right before it; for `wtd` / `mtd` / `qtd` / `ytd`,
the same span of the previous week / month / quarter / year (a month end stays
a month end: *month to date* on Mar 31 compares with Feb 1 – 28); for
`last_month` & co., the month / quarter / … before. `yoy` moves both ends back
one year (Feb 29 → Feb 28). All of it is computed in one place (Rust,
`src/period.rs`; the browser calls it through wasm). `dashboard --check`
reports a bad spec (unknown key, preset, grain or compare, a bad date) as
`period/bad-spec`; the control then falls back to its defaults.

### KPI tiles: change, trend and goal

A KPI is one statement with a `::METRIC` / `::MONEY` / `::PERCENT` /
`::COMPACT` column; more columns of the same statement add to the tile:

| Column | Shows |
|---|---|
| `::LABEL` | the caption |
| `::DELTA` / `::DELTA_LOWER_GOOD` | the comparison value → `▲`/`▼` and the change, green when it moves the good way (for `::DELTA_LOWER_GOOD` a fall is good). `NULL` (e.g. a `::PERIOD` with *No comparison*) → no change line |
| `::DELTA_FORMAT` | `'pct'` (default) `▲ 12.3%`, `'abs'` `▲ +$1,200`, `'both'` `▲ 12.3% (+$1,200)`. The absolute change uses the KPI's own format with an explicit sign: `+$1,200` / `−$0.31` (`::MONEY`), `+2.3 pp` percentage points (`::PERCENT`), `+1.2K` (`::COMPACT`). A comparison of 0 has no percentage: the absolute part is shown |
| `::DELTA_LABEL` | what the change compares against, after it: `'vs last month'`, or the period's own `getvariable('p_prev_label')` ("vs Aug 2026") |
| `::SPARKLINE` | a list, `list(v ORDER BY d)`, → a trend line under the caption, the last point marked |
| `::GOAL` / `::TARGET` | the goal → a progress bar, `82% of goal $1.2M` (the bar stops at 100%, the text does not; green once reached). Outside a KPI statement these stay a chart's reference line (`::REFLINE`) |

```sql
SELECT 'presets=30d,mtd,ytd; default=30d' AS p ::PERIOD;
SELECT sum(rev) FILTER (WHERE cur)::MONEY, sum(rev) FILTER (WHERE prev)::DELTA,
       'both'::DELTA_FORMAT, getvariable('p_prev_label')::DELTA_LABEL,
       list(rev ORDER BY b) FILTER (WHERE cur)::SPARKLINE,
       750000::GOAL, 'Revenue'::LABEL
FROM (SELECT date_trunc(getvariable('p_grain'), day) AS b,
             day BETWEEN getvariable('p_from')::DATE AND getvariable('p_to')::DATE AS cur,
             coalesce(day BETWEEN getvariable('p_prev_from')::DATE AND getvariable('p_prev_to')::DATE, false) AS prev,
             sum(revenue) AS rev
      FROM orders GROUP BY ALL);
```

The browser, `serve`, the static `dashboard` HTML and `dashboard --svg` draw
the same tile (value, change line, caption, sparkline, goal bar, on the
theme's positive / negative / accent colours). KPIs in a `::GROUP` (or two or
more in a row) share one row, sized to the tallest tile. `dashboard --check`
reports `kpi/bad-delta-format` (an unknown format; the change is shown as a %),
`kpi/no-delta` (a format or label without a `::DELTA`) and
`kpi/sparkline-not-list`.

**A denser strip: `::KPI_SIZE`.** The tile above (~96 px) is sized for a
reader studying one number; a status row of 4-6 KPIs someone just glances at
— e.g. next to a `::BUTTONS` picker — doesn't need that much. `'compact'::KPI_SIZE`
on a `::METRIC`/`::MONEY`/`::PERCENT`/`::COMPACT` statement packs it onto one
line (`::LABEL`, the value, the change, in that order) at ~36 px; `::THRESHOLDS`
colouring, `::DELTA`/`::DELTA_FORMAT` and number formats still apply, but
`::SPARKLINE` and `::GOAL` are dropped — there's no room for them on one line.
Put it on the `::GROUP` instead to set every tile in the box at once; a tile's
own `::KPI_SIZE` (either value) then wins over the box's:

```sql
SELECT 'Status'::GROUP, 'compact'::KPI_SIZE;
SELECT 42::METRIC, 'Latest value'::LABEL;
SELECT 7::METRIC, 'Alerts'::LABEL, '<1:good; <4:warn; bad'::THRESHOLDS;
SELECT 3::METRIC, 'Drifting inputs'::LABEL, '<1:good; <3:warn; bad'::THRESHOLDS;
SELECT 91::PERCENT, 'Coverage'::LABEL;
SELECT 1::ENDGROUP;
```

`dashboard --check` reports `kpi/bad-kpi-size` (a value that isn't 'compact'
or 'normal' — treated as 'normal') and `kpi/kpi-size-misplaced` (not on a KPI
tile or a `::GROUP`). `::FIT` (#400) sizes a compact tile's row at its own,
lower minimum, not the normal tile's ~96 px floor.

#### Button groups, toggles and the metric / dimension switcher

For a handful of choices, `::BUTTONS` draws a **segmented button group** instead
of a dropdown (the first option is the default, a `::HINT` column becomes each
button's tooltip, the arrow keys move between the buttons). `::TOGGLE` is an
**on/off switch**: the query's value is the default and the variable is always a
`BOOLEAN` (`TRUE`/`FALSE`; a default written as `'on'`, `1` or `'true'` counts as
on). Both are seeded from and written to the URL (`?v.metric=Peak%20day`,
`?v.launches_only=true`) and, on a locked server, travel as typed values like
every other input.

The two make a **switcher**: pick the shown measure with `CASE getvariable(…)`
and the grouping column with `COLUMNS(getvariable(…))` — one chart, any view:

```sql
SELECT m AS metric::BUTTONS, 'Measure'::LABEL
FROM (VALUES (1, 'Sessions'), (2, 'Peak day'), (3, 'Active days')) t(o, m) ORDER BY o;
SELECT d AS dim::BUTTONS, 'Split by'::LABEL
FROM (VALUES (1, 'channel'), (2, 'region')) t(o, d) ORDER BY o;
SELECT false AS launches_only::TOGGLE, 'Launch days only'::LABEL;

SELECT COLUMNS(getvariable('dim'))::XAXIS,
       CASE getvariable('metric')
         WHEN 'Sessions' THEN sum(n)
         WHEN 'Peak day' THEN max(n)
         ELSE count(DISTINCT day)
       END::BARCHART,
       (getvariable('metric') || ' by ' || getvariable('dim'))::TITLE
FROM events
WHERE NOT getvariable('launches_only') OR note = 'launch'
GROUP BY ALL ORDER BY 2 DESC;
```

`COLUMNS(getvariable('dim'))` resolves the column by name at bind time, so the
options must be real column names; the `CASE` branches must share a type (cast
them, e.g. `sum(n)::DOUBLE`, if they differ). The *Filters & inputs* sample's
**Switch the view** tab is this dashboard.

### Statistical graphics (intervals, facets, scales, reference lines)

Roles for model output — coefficient forests, residual diagnostics, CV curves:

| Need | SQL | Notes |
|---|---|---|
| point ± interval | `term::XAXIS, est::SCATTER, lo::YMIN, hi::YMAX` | a `::SCATTER` becomes a pointrange; a bar/line gets capped error bars |
| horizontal forest | `… , 1::FLIP, 0::REFLINE` | `::FLIP` turns the intervals horizontal, `::REFLINE` the zero line vertical |
| several models | `… , model::CATEGORY` | pointranges (and `::BARCHART` bars) on a discrete x are dodged side by side |
| x interval | `x::XAXIS, y::SCATTER, a::XMIN, b::XMAX` | a horizontal error bar through each point (numeric x) |
| straight lines | `'0.5,1'::ABLINE`, `1::IDENTITY` | `y = 0.5x + 1` (one per distinct value); `y = x` dashed grey — both drawn across the data box |
| small multiples | `g::FACET` / `g::FACET_FREE`, `3::FACET_NCOL` | one panel per value; `_FREE` gives every panel its own axes (y only on a discrete x) and stacks the panels in one column unless `::FACET_NCOL` is set |
| axis transform | `'log10'::XSCALE`, `'sqrt'::YSCALE`, `'reverse'::YSCALE` | combine freely with `::XFORMAT`/`::YFORMAT` |
| date tick labels | `'%H:%M'::XFORMAT`, `'%-d %b'::XFORMAT`, `'%b %Y'::XFORMAT` | on a DATE/TIMESTAMP x, a value with `%` is a strftime pattern (`%Y %y %m %-m %b %B %d %-d %e %a %A %H %M %S`; names follow `::LOCALE`): a time-of-day profile reads `06:00`, not `1 Jan 2000 06:00`; monthly ticks read `Jan 2025`, not `2025-01`; any other `::XFORMAT` stays a number format |
| axis titles | `'Residual load (GW)'::XTITLE`, `'Price (EUR/MWh)'::YTITLE` | what the axes are; the x title names the `::XAXIS` variable, so on a `::FLIP` chart it's drawn beside the (now vertical) category axis; none on pies, gauges, maps |
| label outliers | `name::LABEL, 5::LABEL_TOP [, cooks_d::RANK]` | labels the 5 points with the largest `::RANK` (else \|y\|); overlapping labels are skipped. `::LABEL` is then per-point text, not the chart title |
| trend method | `y::SMOOTH, 'lm'::SMOOTH_METHOD` | `'loess'` (default), `'lm'`, `'gam'` (penalised spline, λ by GCV), `'glm'` (= Gaussian lm) |

```sql
-- Coefficient forest of two models
SELECT term::XAXIS, model_id::CATEGORY, estimate::SCATTER,
       conf_low::YMIN, conf_high::YMAX, 1::FLIP, 0::REFLINE
FROM coefficients;

-- Residuals vs fitted with a GAM trend, top-3 influential rows labelled
SELECT fitted::XAXIS, residual::SMOOTH, 'gam'::SMOOTH_METHOD, 0::REFLINE,
       row_id::LABEL, 3::LABEL_TOP, cooks_d::RANK
FROM augmented;
```

`::BARCHART_PERCENT` draws each bar's share of its x's total (of the grand
total without a `::CATEGORY`); values already given as fractions (all within
±1) are drawn as they are. `::LINECHART_PERCENT` formats fractions ×100 and
treats larger values as percentages already.

Dodged marks keep their discrete x level in `data-x`, including a grouped
chart with `::DATALABELS`.

For whole model outputs there are ready-made plot macros — see
[Plotting model output](#plotting-model-output-contract-macros).

#### Box plots from statistics

`::BOXPLOT` computes the box from raw rows, which must all reach the page
(`max_rows`). For a whole population (every series of every method) compute the
numbers in SQL and send one row per box:

```sql
SELECT method::XAXIS,
       quantile_cont(abs_err, 0.25)::Q1, median(abs_err)::MEDIAN,
       quantile_cont(abs_err, 0.75)::Q3,
       quantile_cont(abs_err, 0.05)::WHISKER_LOW,
       quantile_cont(abs_err, 0.95)::WHISKER_HIGH,
       avg(abs_err)::MEAN,
       'Absolute error by method'::TITLE
FROM backtest GROUP BY method ORDER BY median(abs_err);
```

- `::Q1`, `::MEDIAN` and `::Q3` are required, and no chart measure: the
  statement draws a box plot by itself. `::WHISKER_LOW` / `::WHISKER_HIGH`
  (a pair) are the whisker ends — your rule: the 5 / 95 % quantiles, min / max,
  or Tukey's 1.5 × IQR fences; without them there are no whiskers.
- `::MEAN` adds a dot; `::OUTLIERS` (a list of numbers, e.g.
  `list(v) FILTER (WHERE v > hi)`) draws open circles like a raw box plot's.
- The box looks like a raw-rows `::BOXPLOT` (theme, dark mode, `::CATEGORY`
  fill, `::FLIP`, `::YFORMAT`, `::YSCALE`, `::FACET`, reference lines); its
  tooltip names every number (*ets: median 4.1 · Q1 3 · Q3 5.2 · whiskers 1 –
  8 · mean 4.4 · 2 outliers*), and the screen-reader summary and *charts as
  tables* read the rows.
- `dashboard --check` reports a missing quartile role, unpaired whiskers or a
  chart measure beside them (`box/missing`, an error), a box where
  Q1 ≤ median ≤ Q3 does not hold or a whisker ends inside the box
  (`box/unordered`), and a box with a NULL quartile (`box/null`, not drawn).

### Annotations from data (events, ranges, callouts)

Label what explains a chart — a launch, an outage, a record — with rows from
SQL. Each annotation role is read **per row**, so the usual source is an events
table joined onto the series by its date; rows without an event carry NULL and
draw nothing:

```sql
SELECT s.day::XAXIS, s.revenue::LINECHART,
       e.day::XLINE,  e.note::XLINE_LABEL,                    -- labelled event lines
       o.start_day::XBAND_FROM, o.end_day::XBAND_TO, o.what::XBAND_LABEL,  -- shaded ranges
       CASE WHEN s.revenue = max(s.revenue) OVER () THEN 'Record day' END::ANNOTATE,
       150::REFLINE, 'Target'::REFLINE_LABEL,
       'Revenue — the launch lifted it, the outage cost a week'::TITLE
FROM sales s
LEFT JOIN events  e ON e.day = s.day
LEFT JOIN outages o ON o.start_day = s.day
ORDER BY s.day;
```

| Role | Draws |
|------|-------|
| `::XLINE` + `::XLINE_LABEL` | a dashed event line with its label at the top, one per row |
| `::XBAND_FROM` + `::XBAND_TO` (+ `::XBAND_LABEL`) | a shaded x-range per row, labelled at its start |
| `::REFLINE` + `::REFLINE_LABEL` | the reference line's label above its right end |
| `::ANNOTATE` | a text callout at the row's (x, y), with a ring on the mark |

`::XLINE`, `::MARKAREA` and `::XBAND_FROM`/`::XBAND_TO` take numbers, dates
or timestamps — on a date axis a date is placed like the x data. Labels and
shading use the theme's chart text and muted colours, so they follow dark mode;
labels that would overlap are skipped. Up to 64 annotations of each kind are
drawn. The same roles render in the browser and headless (`dashboard --svg`).
`dashboard --check` warns (`annotation/unpaired`) about a label or range end
without its partner.

### Fans, shaded regions and strips under a chart

**A nested quantile fan.** Give a line several `::BAND_LOWER` / `::BAND_UPPER`
pairs — they pair up in column order (the first lower with the first upper, …).
The bands are drawn widest first, each at a low opacity, so where they overlap
the shade deepens: the 95 % band is the lightest, the 50 % band the darkest. A
band covers only the rows that carry both edges (a forecast's, not its
history's). Quantiles stored long — one row per period and level, as a
simulation writes them — become one column pair per level with `FILTER`:

```sql
WITH fan AS (
  SELECT day, any_value(yhat) AS yhat,
         max(lower) FILTER (WHERE level = 0.95) AS lo95, max(upper) FILTER (WHERE level = 0.95) AS hi95,
         max(lower) FILTER (WHERE level = 0.8)  AS lo80, max(upper) FILTER (WHERE level = 0.8)  AS hi80,
         max(lower) FILTER (WHERE level = 0.5)  AS lo50, max(upper) FILTER (WHERE level = 0.5)  AS hi50
  FROM projection GROUP BY day)
SELECT day::XAXIS, yhat::LINECHART,
       lo95::BAND_LOWER, hi95::BAND_UPPER, lo80::BAND_LOWER, hi80::BAND_UPPER,
       lo50::BAND_LOWER, hi50::BAND_UPPER,
       'Projected stock (95 / 80 / 50 %)'::TITLE
FROM fan ORDER BY day;
```

**Several shaded regions.** A `::MARKAREA` column shades every *run* of its
values: x-values with no unmarked x of the chart in between form one region,
from the first to the last; a run of a single value spans ± half the x step, so
one flagged day shows. `CASE WHEN on_hand = 0 THEN day END::MARKAREA` shades
each stockout spell, and several `::MARKAREA` columns shade several kinds of
event. For labelled ranges from an events table, use `::XBAND_FROM` /
`::XBAND_TO` (above).

**Strips under a chart (`::TRACK`).** A statement with a `'Name'::TRACK`
column, right after a chart, is drawn *under* that chart in the same panel, on
exactly its x axis — same plot area, same x window, also when the chart is
zoomed — so features (promotions, alarms, price) read against it. The strip
takes the chart's numeric or date `::XAXIS`, and draws:

| Strip | SQL | Draws |
|---|---|---|
| ticks | `day::XAXIS, 'Promotions'::TRACK [, name::LABEL]` | a tick per row, the `::LABEL` as its tooltip |
| lanes | `… , detector::YAXIS, tier::CATEGORY` | one lane per `::YAXIS` value (the strip's name without it), coloured by `::CATEGORY` (`'#d64545'` levels are drawn in that colour) |
| one-row heatmap | `day::XAXIS, n::HEATMAP, 'Alarms'::TRACK [, lane::YAXIS]` | a tile per row, one x step of the chart wide, coloured by the value (or by a `::CATEGORY`) |
| sparkline | `day::XAXIS, price::LINECHART, 'Price'::TRACK` | a small line / `::STEP` / `::AREACHART` / `::BARCHART` / `::SCATTER` |

```sql
SELECT day::XAXIS, on_hand::LINECHART, 'On-hand stock'::TITLE FROM stock ORDER BY day;
SELECT day::XAXIS, 'Promotions'::TRACK, promotion::LABEL FROM promos;
SELECT day::XAXIS, detector::YAXIS,
       CASE tier WHEN 'alarm' THEN '#d64545' ELSE '#e8a23b' END::CATEGORY,
       1::HEATMAP, tier || ': ' || reason::LABEL, 'Alarms'::TRACK
FROM alarms;
SELECT 56::HEIGHT;                                  -- a ::HEIGHT may sit before a strip
SELECT day::XAXIS, price::STEP, 'Price'::TRACK FROM stock;
```

A strip is as tall as its lanes (20 px each) or 60 px for a sparkline; a
`::HEIGHT` right before it sets its height. The strip's name (or its lanes'
names) stands where the chart's y labels are; a strip has no x axis of its own
and no legend — its tooltips say what each mark is. In the browser the
crosshair runs down through the strips, a strip mark shows its own tooltip, and
**clicking a strip mark shades its x span on the chart above** (click again to
clear) — "did the alarm come before the stockout?". Each strip has its own
screen-reader summary and keyboard navigation; "Charts as tables" lists its
rows under the chart's; the PNG / full-size views include the strips. The
headless `dashboard --svg` and HTML outputs stack them the same way.
`dashboard --check` warns (`track/no-chart`) about a `::TRACK` that does not
directly follow a chart with a numeric or date x axis — it is then drawn as a
panel of its own. See the **"When could we run out of stock?"** sample.

### Thresholds (good / warn / bad)

One rule set gives a value a status, and the status colours it the same way
everywhere — in the theme's `positive` / `warning` / `negative` colours, so dark
mode and the colour-blind theme follow:

```sql
SELECT margin::PERCENT, 'Margin'::LABEL, '<0:bad; <5:warn; good'::THRESHOLDS FROM kpis;       -- KPI value colour
SELECT 72::GAUGE, '0,100'::RANGE, '<50:bad; <80:warn; good'::THRESHOLDS, 'Service level'::TITLE;  -- gauge zones
SELECT day::XAXIS, sl::LINECHART, '<50:bad; <80:warn; good'::THRESHOLDS, 'Service level by day'::TITLE FROM t;  -- threshold lines

SELECT 'Margin %: <10:bad; <15:warn; good | accuracy: <0.85:bad; good'::THRESHOLDS;  -- the next table's cells
SELECT region::TABLE, margin AS "Margin %", accuracy FROM regions;
```

- **Rules** are `condition:status`, separated by `;`, checked in order — the
  first match wins. Conditions: `<x`, `<=x`, `>x`, `>=x`, `=x`, `a..b`
  (inclusive), `else` (or a bare status) for the rest. Numbers take `%` (÷100)
  and `k` / `M` / `B`. Statuses: `good` (`ok`, `positive`), `warn` (`warning`),
  `bad` (`critical`, `negative`), `neutral`. Use the value's own units (a
  `::PERCENT` KPI of `12.5` means 12.5 %).
- **KPI**: the value takes its status colour (`th-good` / `th-warn` / `th-bad`
  on `.metric-value`; a tooltip names the rule).
- **Gauge**: the rules become zones over `::RANGE`; the zone holding the value
  is drawn at full strength, the value in its colour. They replace `::COLORS`.
- **Chart**: a dashed line at each bound where the status changes, in the
  colour of the worse side.
- **Table**: a lone `::THRESHOLDS` statement before the table names columns
  (`'column: rules | column: rules'`, raw or readable header names); each
  cell gets its status tint and a shape — ● good, ▲ warn, ◆ bad — so colour is
  never the only signal.
- The rule set is SQL like any value, so it can come from data:
  `'<' || target * 0.9 || ':bad; <' || target || ':warn; good'`.
- KPI, gauge and chart thresholds render headless too (`dashboard --svg`); table
  thresholds are browser-only, like `::FLAG`. `dashboard --check` reports
  `thresholds/bad-rule`, `thresholds/no-column` (a table rule set without a
  column) and `thresholds/in-table` (`::THRESHOLDS` inside the `::TABLE`
  statement, where it would be shown as a column).

#### Status colours for categories (`::COLORS` level map)

A chart's `::CATEGORY` levels take the same status colours when a `::COLORS`
column maps them: `'level:status'` entries separated by `;` (or `,`), the
status `good` / `warn` / `bad` / `neutral` (or an alias, or `#rrggbb`). The
named levels are drawn in the theme's `positive` / `warning` / `negative` /
`muted` colours — dark mode and the colour-blind theme follow — and the legend
lists them by name, in the written order; levels the map does not name keep
their palette colours.

```sql
SELECT days::XAXIS, chance::SCATTER, status::CATEGORY,
       'Critical:bad; Watch:warn; OK:good'::COLORS
FROM items;
```

`dashboard --check` reports `colors/bad-map` (an entry without a status) and
`colors/no-category` (a level map on a chart without a `::CATEGORY`). Without
a `:` the value is still a gauge's comma-separated zone colours.

### Plotting model output (contract macros)

The DuckDB extension bundles one plot macro per *shape* of model output, the
long-table schemas of the [anofox integration
contract](plans/integration-contract.md) shared by anofox-statistics,
anofox-forecast and friends. They are aggregate macros over **whole rows**:
name the table with an alias and pass the alias — DuckDB hands each row over
as a STRUCT, so the macro sees the column names:

```sql
SELECT anofox_plot_terms(c) FROM coefficients c;                    -- one SVG
SELECT model_id, anofox_plot_terms(c) FROM coefficients c GROUP BY ALL;  -- one per model
SELECT anofox_plot_prediction(f) FROM ts_forecast_by('sales', id, ds, y, 'AutoETS', 14, '1d') f;
```

| Macro | Schema (required **bold**) | Draws |
|---|---|---|
| `anofox_plot_terms(tbl, highlight := NULL, lambda_min := NULL, lambda_1se := NULL, facet := NULL, reference := NULL)` | **term, estimate**, conf_low, conf_high, conf_level, model_id, id, index_name, index_value, window_end | coefficient forest (x "estimate (95% CI)"; models — or groups, `id` — dodged + coloured with the legend in stacking order; rows without an interval as hollow rings, noted under the plot; a coefficient whose interval contains 0 — not significant — is drawn at half opacity (point and interval, "n.s." in its tooltip, "faded = interval crosses 0" under the plot); one model's coefficients are blue above the no-effect line and red below it (the theme's brand and negative colours; several models keep their series colours); a numeric `reference := 1` moves that no-effect line, e.g. for odds ratios; models × groups: one panel per group). With `index_value`: a **λ path** (ridge / elastic net) is one panel with every coefficient overlaid on a log-λ axis and labelled at its line end (`highlight := ['tv', …]` greys the others; λ.min / λ.1se as dashed lines from `lambda_min :=` / `lambda_1se :=` or the table's `lambda_min`/`is_min` columns; `facet := true` keeps one panel per term); a **LARS path** on x = \|β\|₁ / max \|β\|₁; the **quantile process** (x "τ (quantile)") and **rolling coefficients** (x = `window_end`, a date axis) one panel per term, with a dashed full-sample reference (± CI shaded) from the input's index-less rows (e.g. an `ols` tidy table `UNION ALL BY NAME`) or `reference := (SELECT list(t) FROM ols_tidy_by(…) t)` |
| `anofox_plot_prediction(tbl, x := NULL, y := NULL, facet := NULL, band := NULL, kind := NULL, events := NULL)` | **x \| ds \| period, yhat**, y, yhat_lower, yhat_upper, level, split \| is_training, model_id \| model_name, id, focal, focal_name | observed y as neutral grey points, the fit as a coloured line with a light band (several `level`s: a nested fan, one band per level, "bands: 80 %, 95 % intervals" under the plot; an integer `period` is the x when there is no `ds`); `events := (SELECT list(e) FROM alarms e)` draws an `events` table as vertical lines in the severity's colour (alarm red, warning amber, else grey; the event in its own panel when it carries the facet column or `series`), its kind, label, value and threshold on hover; split changes along x as labelled dashed vertical lines ("hold-out", "forecast", else the split name; when splits interleave along x the non-train points are marked instead). Several models in a panel: one band only (`band := 'model'`, default `ols` or the first model) and a line per model in its stable colour; `kind := 'difference'` plots each model minus the reference. Effects tables (`focal`): y "P(y = 1)" for logistic / binomial, "expected count" for poisson / negbinom, else "predicted y"; x from `focal_name` when present. Several `id`s become panels |
| `anofox_plot_curve(tbl, base_rate := NULL)` | **curve_type, x, y**, y_low, y_high, model_id \| series \| id, threshold, weight, auc, n_censor \| censored, n_risk, n_event (km), is_min, is_1se (lambda_cv), fold, lambda, inner_cv_mean, metric (nested_cv) | roc: square, diagonal, the AUC (trapezoid, or an `auc` column) in each legend label; pr: the recall-0 points trimmed, a dashed base-rate line (`base_rate :=`, else the precision at the lowest threshold with full recall); calibration: square, diagonal, point size by the bin count (`weight`); km: step curves from S(0) = 1, step-ribbon band, small `+` censor marks, a dashed 50 % line with drops at each stratum's median and a number-at-risk table under the curves; acf/pacf: lollipops without lag 0 (ACF), significance bounds (one panel per series); lambda_cv: CV error ± SE on a log-λ axis with λ.min / λ.1se; nested_cv: outer-fold test error points (`series = 'fold'`, x = fold number) plus the across-folds mean ± SE as a quiet horizontal band (`series = 'mean'`), one panel per model/id (`ridge_nested_cv_by` / `elasticnet_nested_cv_by`); qq / pdp / lift / null_dist / lowess: line or points (+ band). One `curve_type` per call (`GROUP BY curve_type`) |
| `anofox_plot_summary(tbl, metrics := NULL, baseline := 'range')` | **metric, value**, model_id \| model_name, id, conf_low, conf_high | a grid of small panels, one per metric with its own scale; one model order in every panel (best first by the first metric), the value beside each dot, "n/a" for a missing metric, the best model highlighted for metrics with a direction; `baseline := 'range'` zooms to the values and prints the range ("Δ = …") in the panel title, `'zero'` starts the axis at 0; n_observations / n_features / iterations only when `metrics := [...]` asks |
| `anofox_plot_tests(tbl, alpha := 0.05, value := NULL, label := 'p')` | **p_value \| p_adj \| statistic**, test_id, group1, group2, estimate, conf_low, conf_high, method | comparison forest: one row per test (pairs as "B − A"), x "difference (95% CI)" (else the statistic, else −log10 p; `value :=` forces one), coloured by significance at `alpha`, `p = … **` beside each row (`label := 'stars'`: stars only). Estimates without any interval (pairwise_t_test_by today) fall back to −log10(p) bars with a labelled significance line, said in the axis label. Several groups (`id`, `model_id`, else a text column such as an unnested aggregate's GROUP BY key; text that holds numbers, like pingouin's `BF10`, is not a group) get a panel each. Several tests (`method`: ADF and KPSS, say) are not on one scale: when the forest plots estimates or statistics, each method gets its own panel — its own x axis, the method as its title, the groups on one shared y axis; on the −log10 p axis (`value := 'p'`, or the no-CI fallback), which every test shares, they stay in one panel with rows such as "A · ADF". A column holding a `*_agg` STRUCT / LIST is expanded |
| `anofox_plot_compare(tbl, value :=, factor :=, tests := NULL, kind := 'box', hide_ns := NULL, label := 'stars')` | the raw rows: the **value** and **factor** columns named by the arguments | box / violin / points per group with significance brackets from `tests :=` (a LIST of test STRUCTs: `(SELECT list(t) FROM tukey_hsd_by(…) t)` or `tukey_hsd_agg(y, g)`; one two-sample STRUCT for two groups); `hide_ns` defaults to true above 3 groups; `label := 'stars' \| 'p'` or a template (`'p = {p.adj}'`, `'{p.signif}'`) |
| `anofox_plot_partial(tbl, kind := 'component')` | **term, x, partial_residual** (component), or **term, x_residual, y_residual** (added_variable) | one panel per term: component + residual (points, component line, loess — a binary / ≤ 3-level term gets jittered points and the group means instead) or added-variable (points + least-squares line through the origin, slope in the title) |
| `anofox_plot_obs(tbl, kind := 'resid_fitted', label_top := 3)` | **fitted, residual**, row_id, std_residual, leverage, cooks_d, n_params | one diagnostic: `resid_fitted`, `qq`, `scale_location`, `leverage` (Cook's distance contours 0.5 and 1 from `n_params`, both signs in view), `cooks` (index plot, the 4/n line labelled, top rows labelled above their stems); the `label_top` largest-Cook's rows are labelled by `row_id` |
| `anofox_plot_diagnostics(tbl, label_top := 3)` | as `obs` | the plot.lm 2×2 as one SVG (a ggplot-rs `PlotGrid`: root `data-grid="2 2"`, each sub-plot a nested `<svg data-panel="i">`; one legend when several models are compared) |
| `anofox_plot_cv(tbl, x := NULL)` (provisional) | **fold_id, split** (`train`/`test`), **ds \| period \| x**, id, y; or **fold_id**, train_start, train_end, test_start, test_end | one lane per fold, the train and test windows as bars along ds (or an integer period), coloured by split; a caption says the fold count, whether the test windows overlap or leave gaps, and how many series share the same folds. Series with identical folds are drawn once; up to 6 different ones get a panel per id, else an error. Width 640, height 260 |
| `anofox_plot_schedule(tbl)` | **resource, task, start, end**, model_id, due, status | a Gantt chart: one lane per resource, a bar per task from start to end, coloured by status (`on time`/`late`, else muted); a late task gets a connector from where it finished to its `due` date (on time tasks keep `due` in their tooltip only); faceted by `model_id` (algorithm) when there are several, up to 6, else an error. A caption says the task and resource counts, how many are late (across all models, when faceted), and whether due dates are marked. NULL start/end rows are dropped, end < start is dropped, start = end is widened to a minimum visible bar — each counted in a warning; a NULL resource draws on an "(unassigned)" lane, a NULL model_id (while others facet) gets its own "(none)" panel. Replaces drawing a schedule as a scatter with xmin/xmax error bars. Width 640, height 400 |
| `anofox_plot_events(tbl, x := NULL)` | **ds \| period**, kind, label, severity, value, threshold, model_id, series | a timeline: one lane per series (else per kind), a dot per event coloured by severity (alarm red, warning amber, else grey; a legend when there are several), kind · label · value (threshold …) on hover. Width 640, height 260 |
| `anofox_plot_checks(tbl, x := NULL, kind := NULL)` | **check_name, status, value**, lower_threshold, upper_threshold, identifier, message, run_ts \| ds, check_type, table_name, column_name | a data-quality check band (default): one panel per check (own x scale, sorted), the value as a dot coloured by status (fail red, warn amber, pass green, else muted) against the lower_threshold..upper_threshold acceptance band (light green), one lane per `identifier`; over several `run_ts` / `ds` values (or `kind := 'strip'`) a status strip instead, one lane per check, a dot per run. The status, check label, identifier and message are on hover; `x :=` names the time column (default `run_ts`, else `ds`) |
| `anofox_plot_decomposition(tbl, x := NULL, components := NULL)` *(provisional)* | **ds \| period, component, value** (long), or wide **observed \| y, level, trend, seasonal, seasonal_<period>, remainder \| residual** columns, or anofox-forecast's list outputs; optional `id`, `model_id` \| `model_name` \| `method` | stacked panels sharing one x axis, each with its own y scale: observed, level, trend, each seasonal component, the remainder/residual as points (drawn last); `components := ['trend', 'seasonal_7']` picks and orders the panels; several models (model_id / model_name / method) are coloured. One series per call (`GROUP BY id`) |
| `anofox_plot_matrix(tbl, row := NULL, col := NULL, value := NULL, domain := NULL, mirror := NULL, labels := NULL)` | **row, col, value** — also read: `column_a` / `column_b` (and `var1`/`var2`, `row_name`/`col_name`); value also read from `pearson` \| `spearman` \| `kendall` \| `correlation` \| `corr` \| `count` \| `n`; optional `model_id` | a heatmap, one tile per cell: a diverging fill scale around 0, `−1..1` for a correlation measure (`domain :=` overrides); a pair list (anofox-tabular's `profile_correlations` as it comes) is mirrored to the full matrix with a diagonal of 1 (`mirror := false` keeps it as given); cell values are written when there are at most 144 cells (`labels :=` overrides); `value := 'spearman'` picks another measure. Width 520, height 440 |
| `anofox_plot_diagram(tbl, edges := NULL, id := NULL, mainstat := NULL, format := 'compact', source := NULL, target := NULL, value := NULL, direction := 'LR', style := NULL)` | `graph`: one row per node — **id**, title, mainstat, status, group, lane, story, x, y, sparkline, seq; `edges :=` a LIST of **source, target**, value, label, style, status (Grafana node-graph names; `stage`, `label`, `from_stage`, `to_stage`, `count` read too; the arguments name any other column) | the `::DIAGRAM` panel (see *Diagrams*): boxes with title, mainstat (in `format`), status pill and sparkline, swimlanes, edges as wide as their value and coloured by status, loops running back; nodes in `seq` order. Width 800, height 400 |
| `anofox_plot_assignment(tbl, x := NULL)` | one row per item: **bin \| wave, item, size**, capacity, model_id | an optimizer's item → bin/wave assignment (anofox-optimize's `pack_*` / `wave_*`): items stacked per bin/wave (`::CATEGORY` = item, `::BARCHART_STACKED` = size), the capacity as a quiet reference line so the fill level is visible; `x :=` names the bin/wave column when both are present; several `model_id`s are faceted to compare algorithms; a `group_col` (`pack_by`/`wave_by`'s own instance split, kept under its own name) facets too, so two groups' bin 1 never stack into one over-capacity bar — alone, or combined into one `"group · model_id"` facet title when both vary |
| `anofox_plot_selection(tbl)` | one row per item: **item, selected \| listed**, value, weight, capacity, margin, demand, model_id | which items an optimizer picked (#499; anofox-optimize's knapsack `selected`, assortment `listed`): a bar per item, picked ones first in the theme's good colour, the rest neutral; the bar is `value`, else margin × demand, else margin, else weight; with weight + capacity the default title is "Selected weight 9 of capacity 10 (90 %)"; several `model_id`s and a producer group column facet |
| `anofox_plot_distribution(tbl)` | one row per option: **id, option, probability**, model_id — also read: `question_id` for id, `model` for model_id | a categorical distribution (anofox-decide's `decide_choice_distribution` MAP / `decide_table.distribution`, reached with `SELECT t.question_id AS id, u.e.key AS option, u.e.value AS probability FROM t, unnest(map_entries(t.distribution)) AS u(e)` — see *Without anofox_statistics*, "A categorical distribution"): one `id` draws a plain bar per option; several `id`s draw a 100% stacked bar per id (`::CATEGORY` = option); several `model_id`s are faceted to compare models |
| `anofox_plot(tbl)` | any of the above | picks the plot from the columns: term+estimate → terms, curve_type → curve, yhat → prediction, fitted+residual → diagnostics, metric+value → summary, check_name \| lower_threshold \| upper_threshold with status+value → checks, partial_residual → partial, fold_id + (split \| test_start \| train_end) → cv, resource+start+end → schedule, component+value \| trend → decomposition, p_value / test_id / statistic+method → tests, row+col \| column_a+column_b → matrix, kind \| severity with ds \| period → events, (bin \| wave)+item+size → assignment, option+probability → distribution. Errors name the contract version (`anofox_contract_version()` = `'1'`) |

Besides these, the column macros `anofox_bar(x, y)`, `anofox_line(x, y)`,
`anofox_scatter(x, y)`, `anofox_area(x, y)`, `anofox_xy(x, y, kind := …)`
and `anofox_xyc(x, y, series, kind := …)` chart two or three columns.

All take `width := 640, height := 400` (diagnostics 820×640),
`title := NULL, x_label := NULL, y_label := NULL`, and `theme := NULL,
locale := NULL` (defaulting to `getvariable('anofox_theme')` /
`getvariable('anofox_locale')`; see *Themes* below).

**Titles and labels.** A contract plot draws no inner title by default — in a
dashboard the panel caption (`'…'::TITLE`) is the title, and the standalone
SVG of the SQL macro is the same chart; `title := '…'` adds one (grid cells
keep their term / metric / group headings). Axes are labelled from the
contract's meaning (forest "estimate (95% CI)", λ path "λ (log scale)" /
"coefficient", ROC "false positive rate" / "true positive rate", Cook's
"observation" / "Cook's distance", prediction: the x and y column names
humanised, `order_value` → "order value", …); `x_label :=` / `y_label :=`
override them (ignored by `diagnostics`, whose four panels have fixed axes,
and by the automatic −log10(p) fallback of `tests`).

**Look.** Contract plots use a base font of 13.2 pt (ggplot2's 11 × 1.2),
bold strip / cell titles, tick labels of at least 10 px; reference lines
(zero, 4/n, diagonals, 50 %) are thin light-grey dashes, strong colour is kept
for semantic marks (λ.min, significance). Models keep their colour across
every chart: known ids have a fixed palette slot (ols / logistic steel blue,
huber / poisson orange, ridge green, elasticnet purple, lasso pink, theil_sen
yellow, wls sky blue, lars brown, ransac lime, quantile mauve), other names
hash onto the palette. A plain bar chart (no `::CATEGORY`) is one colour. Missing
required columns, non-numeric values, an unknown `kind`/`curve_type` or a
mixed `curve_type` are SQL errors naming the problem; no rows give a "No data"
SVG.

**Other column names.** Raw statsmodels, R, broom or pingouin tables plot
without renaming: before the schema lookup every column name is compared with
case, `.`, spaces and symbols ignored (`Std. Error`, `std.error` and
`std_error` are one name; broom's `.fitted` is `fitted`), and these common
spellings are read as the contract column (the first one present wins; a
column the contract name already has, or one an option names — `x :=
'Price'` — is left alone; `anofox_plot_compare` reads its rows as given):

<!-- contract-aliases:start -->
| Contract column | Also read from (case, `.`, spaces and symbols ignored) |
|---|---|
| `estimate` | `coef`, `params`, `coefficient` |
| `std_error` | `std_err`, `bse`, `std_errors`, `se` |
| `statistic` | `t`, `z`, `tvalues`, `zvalues`, `t_value`, `z_value`, `t_stat` |
| `p_value` | `p_t`, `p_z`, `pr_t`, `pr_z`, `p_val`, `p_unc`, `pvalues`, `pval`, `p` |
| `p_adj` | `p_corr`, `padj`, `p_adjusted` |
| `conf_low` | `ci_lower`, `lower`, `mean_ci_lower`, `conf_lower`, `[0.025` |
| `conf_high` | `ci_upper`, `upper`, `mean_ci_upper`, `conf_upper`, `0.975]` |
| `fitted` | `fittedvalues`, `fitted_values` |
| `residual` | `resid`, `residuals` |
| `std_residual` | `std_resid`, `standard_resid` |
| `leverage` | `hat`, `hat_diag` |
| `cooks_d` | `cooksd`, `cooks_distance`, `cook_d` |
| `id` | `unique_id`, `question_id` |
<!-- contract-aliases:end -->

So `p_t` is statsmodels' `P>|t|`, `pr_t` R's `Pr(>|t|)`, `p_unc` pingouin's
`p-unc`; statsmodels' interval columns are matched by their bracket (`[0.025`
/ `0.975]`, any level). In a table with `yhat`, `lower` / `upper`,
`mean_ci_lower` / `mean_ci_upper`, `ci_lower` / `ci_upper`, `conf_low` /
`conf_high` and the bracket columns are the forecast interval (`yhat_lower` /
`yhat_upper`).

Three more shapes are read as the contract's:

- **An interval in one column.** A `[low, high]` LIST or a two-field STRUCT
  (`{lower, upper}`, `{0, 1}`) named `conf_int`, `ci`, `confint` or `CI95%`
  (pingouin) is split into `conf_low` / `conf_high` (`yhat_lower` /
  `yhat_upper` next to `yhat`); a level in the name (`CI95%`) fills
  `conf_level` (`level`). statsmodels' `conf_int()` passed as a LIST or
  STRUCT column plots the same way.
- **Levels.** `level` and `conf_level` of 95 and 0.95 are the same level
  (values above 1 are percentages).
- **statsforecast's wide forecast.** `ds, unique_id, AutoARIMA,
  AutoARIMA-lo-95, AutoARIMA-hi-95, AutoETS, …` (also mlforecast and
  neuralforecast) is melted to one row per model and level (`model_id`,
  `yhat`, `yhat_lower`, `yhat_upper`, `level`; `unique_id` is `id`), so
  `anofox_plot_prediction(f)` draws a line per model and a nested fan. A
  forecast without `level=` has no `-lo-` columns to tell models from other
  columns: melt it in SQL (`UNPIVOT f ON COLUMNS(* EXCLUDE (unique_id, ds))
  INTO NAME model_id VALUE yhat`).

`anofox_plot_summary` knows which way these metrics improve and marks the
best model: higher is better for R² (`r2`), adjusted / pseudo R², explained
variance, AUC (`roc_auc`), average precision, accuracy, balanced accuracy,
F1, precision, recall, Matthews correlation (`matthews_corrcoef`, `mcc`),
Cohen's kappa, Jaccard and log-likelihood; lower for RMSE, MSE, MAE, MAPE,
sMAPE, MASE, RMSSE, CRPS, Brier score, expected calibration error (`ece`,
anofox-decide's `decide_ece`), log loss, hamming / zero-one / hinge loss,
pinball loss, max error, the mean Poisson / gamma / Tweedie deviance,
residual SE, sigma, AIC, AICc, BIC and deviance. scikit-learn's long names
(`mean_absolute_error`, `brier_score_loss`, `roc_auc_score`, …) count too, and
a scorer's `neg_` prefix flips the direction (`neg_mean_squared_error`: higher
is better). Names are compared without case, `-` and spaces read as `_`.
A ROC legend keeps a third decimal when two would round a near-perfect AUC up
to 1.00 (0.9952 reads "AUC 0.995").

**When a table doesn't plot**, the error says why:

- a missing column, the columns it got (a renamed one as `coef → estimate`)
  and either the near miss — *needs column(s) estimate (got: term, estimat) —
  did you mean \`estimat\` for estimate?* — or the other names it accepts —
  *… — estimate is also read from coef, params, coefficient*;
- a table that follows another schema says so — *anofox_plot_terms: needs
  column(s) term, estimate (got: ds, yhat) — … — the columns follow the
  prediction schema (anofox_plot_prediction)*;
- `anofox_plot(tbl)` names the schema it detected — *anofox_plot (anofox
  contract v1, detected the curve schema): needs column(s) y …* — and when
  none matches, the closest one and what it misses — *…; closest: terms,
  missing estimate (did you mean \`estimat\`?)* — but only when the table has
  (or nearly has) a column that identifies that schema (\`term\` / \`estimate\`,
  \`yhat\`, \`curve_type\`, \`metric\`, \`check_name\` / \`lower_threshold\` /
  \`upper_threshold\`, \`fitted\` / \`residual\`, \`partial_residual\`,
  \`fold_id\`, \`trend\` / \`remainder\`, \`p_value\`, \`column_a\` / \`column_b\`,
  \`ds\` / \`period\`) — a short one
  (\`term\`, \`yhat\`, \`trend\`) only as itself or with a suffix (\`terms\`, \`yhat_1\`):
  \`team\` is no \`term\`. A shared generic name such as \`kind\`, \`value\`, \`x\`
  or \`y\` alone gives no hint;
- a list where a number is read — *column \`estimate\` must be numeric, got
  a list — take one element (estimate[1]) or unnest it*;
- a column where the row belongs — the table alias is also a column name
  (statsmodels' `t`), so `anofox_plot_terms(t) … FROM tbl t` passes that
  column: *the argument is a column, not the row — pass the table alias …
  and pick an alias that is not also a column name*.

The same messages show in `dashboard --check` and on the dashboard panel.

**Adapting producer output.** `*_fit_predict_by` (anofox-statistics) keeps
the input columns: `anofox_plot_prediction(p, x := 'x1', facet := 'grp')`.
`ts_forecast_by` (anofox-forecast) has no `y`; union the history to draw it:

```sql
SELECT anofox_plot_prediction(p) FROM (
  SELECT id, ds, y, NULL AS yhat, NULL AS yhat_lower, NULL AS yhat_upper FROM sales
  UNION ALL BY NAME
  SELECT id, ds, NULL AS y, yhat, yhat_lower, yhat_upper
  FROM ts_forecast_by('sales', id, ds, y, 'AutoETS', 14, '1d')) p;
```

`anofox_plot_decomposition` (provisional, `decomposition` schema) reads
anofox-forecast's `ts_mstl_decomposition_by` (its `trend`/`seasonal`/
`remainder` LISTs, `seasonal` per `periods`) and `ts_forecast_explain_by`
(its `decomposition` STRUCT) as they come. anofox-orchestration's
`decomposition_seasonal` file's `period` is the *seasonal* period, not a
time step, so write:

```sql
SELECT unique_id AS id, t AS period, 'seasonal_' || period AS component, seasonal AS value
FROM decomposition_seasonal;
```

See `examples/regression/` (fixtures for every schema, plus scripts against
the real anofox-statistics / anofox-forecast extensions) and
`python3 examples/regression/render.py <script.sql> <out_dir>`.

**Results from Python or R.** statsmodels, scikit-learn, scipy, pingouin,
lifelines, statsforecast, Prophet and broom tables plot the same way — in
process through the `duckdb` Python / R package, or written to Parquet for a
dashboard. Recipes per library and the `anofox_viz` helper package:
[Plotting results from Python and R](python-r.md).

**Why row macros.** DuckDB macros cannot take a table *name* and return a
scalar (that needs a table function per build), and positional column
arguments (`anofox_plot_terms(term, estimate, conf_low, …)`) would be long,
order-sensitive and differ per schema. Passing the row struct needs no
introspection, keeps the contract's column names (so one renderer serves every
producer), aggregates like any other aggregate (`GROUP BY`, `FILTER`, window-free)
and works the same in the render-only community build and the C-API build.
The macro bodies live once in `src/macros.rs`; the C++ build registers them
through the FFI with descriptions and `tags` (`anofox.consumes`,
`anofox.contract`), the C-API build with `CREATE MACRO` (no descriptions — a
limitation of that API). Under the hood a macro builds `{"plot": "terms",
"rows": [...], "options": {...}}` for `anofox_render`, which maps the columns
onto the roles above (`term::XAXIS, estimate::SCATTER, conf_low::YMIN, …`).

### Plotting anofox-statistics results

[anofox-statistics](https://github.com/DataZooDE/anofox-statistics) v0.11
emits every plot-ready result as a long table (or, for the hypothesis-test
aggregates, a STRUCT) of the anofox integration contract — version
`anofox_contract_version()` = `'1'`, shared by both extensions. Every
producer carries `duckdb_functions()` tags, so you can list them:

```sql
SELECT DISTINCT function_name, tags['anofox.output'] AS output
FROM duckdb_functions() WHERE tags['anofox.contract'] = '1' ORDER BY output, function_name;
```

Native: `INSTALL anofox_statistics FROM 'https://get.erpl.io'` in `duckdb
-unsigned` on **DuckDB v1.5.6** (get.erpl.io serves stale builds for v1.5.5 /
v1.5.4). The plot macros are the same in the browser builder (DuckDB-Wasm), so
a dashboard query written against the CLI renders unchanged there.

| Producer (per group of `source`) | `anofox.output` | Plot |
|---|---|---|
| `<model>_tidy_by` (21 models) | terms | `anofox_plot_terms(t)` — forest; several models (`UNION ALL BY NAME`) or groups (`id`) dodged + coloured |
| `<model>_rolling_tidy_by`, `ridge_path_by`, `elasticnet_path_by`, `lars_path_by`, `quantile_process_by` | terms (with `index_name` / `index_value`) | `anofox_plot_terms(t)` — estimate (+ band) over the index, one panel per term; λ on a log axis; LARS `alpha` is labelled (not glmnet's λ) |
| `<model>_glance_by` (21) | summary | `anofox_plot_summary(s, metrics := NULL)` — one panel per metric |
| `ols/wls/<glm>_augment_by` | obs | `anofox_plot_diagnostics(o)` (plot.lm 2×2), `anofox_plot_obs(o, kind := …)` |
| `<model>_prediction_by` (19), `<model>_effects_by` (9) | prediction | `anofox_plot_prediction(p)` — fit + band per `id`, split as colour; several models: a line + band each |
| `roc_curve_by`, `pr_curve_by`, `calibration_curve_by`, `kaplan_meier_by`, `acf_by`, `pacf_by`, `ridge_cv_by`, `elasticnet_cv_by`, `ridge_nested_cv_by`, `elasticnet_nested_cv_by` | curve | `anofox_plot_curve(c)` — one `curve_type` per call |
| `tukey_hsd_by`, `pairwise_t_test_by`, `dunn_test_by`, and the 40 `*_agg` tests (`t_test_agg`, `shapiro_wilk_agg`, `pearson_agg`, …) | test | `anofox_plot_tests(t)` — comparison forest; `anofox_plot_compare(r, …, tests := …)` — the raw values with significance brackets |
| `<model>_partial_residuals_by` (8) | — (not a contract schema) | `anofox_plot_partial(p, kind := 'component' \| 'added_variable')` |

`anofox_plot(t)` picks the chart from the columns for all of them. The
examples use the table `d` of `examples/regression/anofox_statistics.sql`
(groups `grp` A/B, features `x1`, `x2`, responses `y`, `y_count`, `score`, a
classifier `label`/`prob`, survival `time`/`event`, factors `level3`, `g2`):

```sql
-- terms: OLS vs Huber coefficients for group A
SELECT anofox_plot_terms(t)
FROM (SELECT * FROM ols_tidy_by('d', grp, y, [x1, x2], names := ['trend', 'cycle'])
      UNION ALL BY NAME
      SELECT * FROM huber_tidy_by('d', grp, y, [x1, x2], names := ['trend', 'cycle'])) t
WHERE grp = 'A';

-- summary: R², AIC, … of three models — each metric on its own scale
SELECT anofox_plot_summary(s)
FROM (SELECT * FROM ols_glance_by('d', grp, y, [x1, x2])
      UNION ALL BY NAME SELECT * FROM huber_glance_by('d', grp, y, [x1, x2])
      UNION ALL BY NAME SELECT * FROM ridge_glance_by('d', grp, y, [x1, x2])) s
WHERE grp = 'A';
SELECT anofox_plot_summary(s, metrics := ['r_squared', 'aic', 'n_observations'])
FROM ols_glance_by('d', grp, y, [x1, x2]) s;            -- one row per group (id)

-- obs: plot.lm 2×2 of a Poisson GLM
SELECT anofox_plot_diagnostics(o) FROM poisson_augment_by('d', grp, y_count, [x1, x2]) o WHERE grp = 'A';

-- prediction: 90 % band per group, rows after id 50 held out ('test')
SELECT anofox_plot_prediction(p)
FROM ols_prediction_by('d', grp, id, y, [x1, x2], level := 0.9,
                       split := CASE WHEN id > 50 THEN 'test' ELSE 'train' END) p;
-- … two models in one panel; marginal effect of x1
SELECT anofox_plot_prediction(p)
FROM (SELECT * FROM ols_prediction_by('d', grp, id, y_count, [x1, x2])
      UNION ALL BY NAME SELECT * FROM poisson_prediction_by('d', grp, id, y_count, [x1, x2])) p
WHERE grp = 'A';
SELECT anofox_plot_prediction(p) FROM poisson_effects_by('d', grp, y_count, [x1, x2], focal := 1) p;

-- curve: one plot per curve_type; KM with censoring marks; CV with λ.min / λ.1se
SELECT curve_type, anofox_plot_curve(c)
FROM (SELECT * FROM roc_curve_by('d', grp, label, prob)
      UNION ALL SELECT * FROM calibration_curve_by('d', grp, label, prob, n_bins := 5)) c
GROUP BY curve_type;
SELECT anofox_plot_curve(c) FROM kaplan_meier_by('d', grp, time, event) c;
SELECT anofox_plot_curve(c) FROM ridge_cv_by('d', grp, y, [x1, x2]) c WHERE grp = 'A';

-- test: post-hoc comparisons as a forest (one panel per group) …
SELECT anofox_plot_tests(t) FROM tukey_hsd_by('d', grp, score, level3) t;
SELECT anofox_plot_tests(t, alpha := 0.01, value := 'p') FROM pairwise_t_test_by('d', grp, score, level3) t;
-- … a one-row test per group (unnest the aggregate's STRUCT) …
SELECT anofox_plot_tests(t) FROM (SELECT grp, unnest(shapiro_wilk_agg(y)) FROM d GROUP BY grp) t;
-- … or pass the STRUCT / LIST column as it is
SELECT anofox_plot_tests(s) FROM (SELECT grp, tukey_hsd_agg(score, level3) AS r FROM d GROUP BY grp) s;

-- compare: raw values per level with significance brackets from the tests
SELECT anofox_plot_compare(r, value := 'score', factor := 'level3',
         tests := (SELECT list(t) FROM tukey_hsd_by('d', grp, score, level3) t WHERE t.grp = 'A'))
FROM d r WHERE grp = 'A';
SELECT anofox_plot_compare(r, value := 'score', factor := 'level3', kind := 'violin',
         tests := pairwise_t_test_agg(score, level3)) FROM d r;    -- the aggregate inline
SELECT grp, anofox_plot_compare(r, value := 'y', factor := 'level3', kind := 'points',
         tests := (SELECT list(t) FROM dunn_test_by('d', grp, y, level3) t WHERE t.grp = r.grp))
FROM d r GROUP BY grp;                                             -- correlated: one per group
SELECT anofox_plot_compare(r, value := 'y', factor := 'g2', tests := t_test_agg(y, g2)) FROM d r;

-- partial residuals: component + residual, added-variable
SELECT anofox_plot_partial(p) FROM ols_partial_residuals_by('d', grp, y, [x1, x2]) p WHERE grp = 'A';
SELECT anofox_plot_partial(p, kind := 'added_variable')
FROM ols_partial_residuals_by('d', grp, y, [x1, x2]) p WHERE grp = 'A';
```

**`tests :=` and `factor :=`.** A macro argument can be any expression, so
the test table goes in as a LIST of STRUCTs: a scalar subquery `(SELECT
list(t) FROM tukey_hsd_by(…) t WHERE …)` (correlate it with the outer row's
group for one plot per group), a list literal, or the aggregate itself
(`tukey_hsd_agg(y, g)` returns that LIST; a two-sample STRUCT such as
`t_test_agg(y, g)` brackets the two groups). The column naming the groups is
`factor :=` (the anofox-statistics name for it) because `group` is a reserved
word in SQL — `group := 'g'` is a syntax error. `value :=` / `factor :=` name
columns of the aliased rows. Brackets are drawn with ggplot-rs's
`geom_bracket_table`, narrow spans lowest, labelled `p = … **` from `p_adj`
(else `p_value`); `label := '{p.signif}'` uses a ggpubr-style template
instead, `hide_ns := true` drops pairs with p > 0.05. Tests naming groups
that are not in the rows are not drawn and reported as a render warning
(`data-warnings`).

**Gotchas.**

1. **Alias the row.** The plot macros aggregate whole rows: `SELECT
   anofox_plot_terms(t) FROM ols_tidy_by(…) t` — not `anofox_plot_terms(*)`.
2. **One `curve_type` per curve plot.** ROC and calibration rows stack with
   `UNION ALL`, but plot them separately (`GROUP BY curve_type`); mixing them
   is an error.
3. **Compare models with `UNION ALL BY NAME`.** Same schema, `model_id` tells
   the methods apart, `id` the groups: filter (`WHERE grp = 'A'`), facet
   (`GROUP BY grp` for one SVG each) or let the macro colour / panel them.
4. **Groups.** Several `id`s become panels (prediction), colours (terms,
   summary labels `model · id`) or grid cells (tests, lambda_cv); `_augment_by`
   has no `id` — filter it to one group for a clean 2×2.
5. **LARS paths are on upstream's `alpha` scale**, not glmnet's λ: do not
   overlay `lars_path_by` with `ridge_path_by`.
6. **`*_fit_predict_by` is not the `prediction` schema**: name the position
   column (`anofox_plot_prediction(p, x := 'x1', facet := 'grp')`) or use
   `<model>_prediction_by`.
7. **Bookkeeping metrics** (`n_observations`, `n_features`, `iterations`) are
   hidden in `anofox_plot_summary` unless `metrics := [...]` asks for them.

`examples/regression/anofox_statistics.sql` renders one example per family;
`scripts/statistics_conformance.py` renders the output of every producer
(see the README); `scripts/decide_conformance.py` does the same for
anofox-decide's outputs, through the SQL that makes them contract tables.
`scripts/tabular_conformance.py` does the same for anofox-tabular, whose
outputs need an adapter to a contract schema first.

### Plotting anofox-tabular results

[anofox-tabular](https://github.com/DataZooDE/anofox-tabular)'s suite check
(`anofox_tab_run_checks`) and its single-check aggregate
(`anofox_tab_agg_check`) already emit the `check` schema (`check_name`,
`status`, `value`, `lower_threshold`, `upper_threshold`, …) as they come, so
they plot without reshaping:

```sql
SELECT anofox_plot(t) FROM anofox_tab_run_checks('checks') t;
```

The other one-row checks (`null_rate`, `compliance`, `distinct_count`, …) use
their own column names for the measure and its bounds, so name them in a
subquery (the adapter `scripts/tabular_conformance.py` uses):

```sql
SELECT anofox_plot_checks(t) FROM (
  SELECT 'customer present' AS check_name, status, null_rate AS value,
         NULL::DOUBLE AS lower_threshold, threshold AS upper_threshold, message
  FROM anofox_tab_null_rate('d', 'customer', 0.05)
) t;
```

The time-aware checks (`metric_anomaly_iqr`, `rel_count_change`,
`rolling_values_in_set`) read as `events` (`reference_date` → `ds`, `fail` /
`warn` → `alarm` / `warning`, `message` → `label`) for a timeline, or as
`check` (`reference_date` → `ds`) for a status strip over several runs.

`anofox_tab_profile_correlations` (`column_a, column_b, pearson, spearman,
n`) already follows the `matrix` schema as it comes — a pair list, mirrored
to the full matrix with a diagonal of 1:

```sql
SELECT anofox_plot_matrix(t) FROM anofox_tab_profile_correlations('d') t;
SELECT anofox_plot_matrix(t, value := 'spearman') FROM anofox_tab_profile_correlations('d') t;
```

See `docs/plans/integration-contract.md` ("anofox-tabular producers") for the
full adapter table, and `scripts/tabular_conformance.py` for every producer
rendered end to end.

### Heatmaps

```sql
SELECT week::XAXIS, channel::YAXIS, sum(revenue)::HEATMAP,
       '$'::YFORMAT,      -- cell labels, legend and tooltip in dollars
       ''::DATALABELS,    -- write each value into its cell (a number = font size)
       'Revenue by week × channel'::TITLE
FROM sales GROUP BY ALL ORDER BY week, channel;
```

- **Order**: text axes keep the SQL row order — the first `::YAXIS` level is the
  top row (a matrix reads like a table), the first `::XAXIS` level the left
  column. Numeric axes stay continuous.
- **Colour**: the theme's sequential ramp, one hue light → dark (pale → brand →
  a deeper brand; in dark mode the low end recedes into the card and the high
  end is the brightest). When the values **span zero** the theme's diverging
  ramp is used instead: two hues around a neutral midpoint at 0, both arms
  scaled to the larger |end|, so −2…+10 reads as a little red and a lot of blue.
  A theme's explicit `sequential` pair is used as given.
- **Status colours** — when a value is good or bad, not just big or small:
  - `''::COLORSCALE` — a continuous green → yellow → red ramp, low = good
    (`::COLORSCALE_HIGH_GOOD`: high = good; `::COLORSCALE_DIVERGING`: negative
    red, positive green around a neutral 0). The colourbar keeps the value
    format.
  - `'>=50:bad; >=20:warn; good'::THRESHOLDS` — one status colour per band
    (the same rules as a KPI, gauge or table cell), with a stepped legend
    naming the bands high to low: `≥ 50%`, `20%–50%`, `< 20%`. A value no
    rule covers is neutral grey. `::THRESHOLDS` win over a `::COLORSCALE`.

  The colours are the theme's `positive` / `warning` / `negative`, tinted
  towards the card — palest for good, strongest for bad (in dark mode dimmest →
  brightest) — so the steps differ in lightness too and stay apart in the
  `colorblind` theme, under colour-vision deficiency and in greyscale. Tooltips
  still show the value. `::CALENDAR` and `::TRACK` heatmap strips take the same
  roles. The linter flags a colour scale on a chart without tiles, both roles
  at once, and rules in another unit than the values (fractions 0–1 against
  `>=50`).

  ```sql
  SELECT ds::XAXIS, item::YAXIS, 100 * p_out::HEATMAP, 'percent'::YFORMAT,
         '>=50:bad; >=20:warn; good'::THRESHOLDS,
         'Chance each item has run out by that day'::TITLE
  FROM projection ORDER BY rnk, ds;
  ```
- **Labels** (`::DATALABELS`): the short value (`$1,235`, `12K`, `1,2 Mio.` in
  `de-DE`), dark text on light cells and light text on dark ones; the font size
  fits the cell unless the role's value sets it.
- **Hover / click**: the tooltip shows the column (header), the row and the
  formatted value; a click cross-filters like any chart — it sets
  `getvariable('selected')` to the cell's **row** (`::YAXIS`) value.
- `::CALENDAR` uses the same ramp, card-coloured cell gaps and formatted
  tooltips.
- **Legend** (`::LEGEND`): `'none'::LEGEND` hides the colourbar — useful on a
  dense dashboard where the tooltip already gives the value —
  and `'top'`/`'right'`/`'bottom'`/`'left'` move it. The same role hides or
  moves a categorised chart's legend (`::CATEGORY` on a bar/line/area/
  scatter, a pie/donut) and a map's. Without it nothing changes.

  ```sql
  SELECT d::XAXIS, feature::YAXIS, alert_share::HEATMAP, ''::COLORSCALE,
         'none'::LEGEND, 'When does each input alert?'::TITLE
  FROM per_method;
  ```

### Maps

```sql
-- a region code: the shape comes from the built-in boundaries
SELECT ags::REGION,              -- '09162' (AGS), 'DE-BY', 'DE', 'DEU', 'DE212' (NUTS 3) …
       sales AS "Sales"::BARCHART,  -- the measure that colours each region
       'quantile 5'::CLASSES,    -- optional: 5 classes with a stepped legend
       8::LABEL_TOP,             -- name the 8 most important regions on the map
       'Sales by district'::TITLE
FROM districts;

-- points by latitude / longitude over a built-in backdrop
SELECT lat::LAT, lon::LON, city::LABEL, orders::BARCHART, 'de-states'::BASEMAP FROM depots;

-- any geometry as WKT (POLYGON, POINT, LINESTRING, MULTI…)
SELECT ST_AsText(geom)::MAP, name::LABEL, sales::BARCHART FROM my_shapes;
```

Maps are drawn by the extension's own writer: the WKT of each row is parsed
once, projected, fitted into the panel and simplified in pixel space (0.5 px),
then written as compact `<path>`s (holes cut out with `fill-rule="evenodd"`)
and `<circle>`s for points. Features are drawn in row order.

- **Projection** (`::PROJECTION`): `'auto'` (default) picks one from the
  data's extent — coordinates beyond lon ±180 / lat ±90, or a vertex exactly
  at (0, 0) (a planar grid's origin) → `'none'`; a longitude span over 120° →
  `'equal-earth'`; at most 6° → `'utm'` (the zone of the centre); otherwise
  `'conic'` (Albers equal-area, standard parallels at 1/6 and 5/6 of the
  latitude span). `'albers-europe'` is fixed (λ₀ 10°, φ₁ 43°, φ₂ 62°);
  `'mercator'` is Web Mercator (`dashboard --check` hints `map/mercator-choropleth`
  on a filled map: it inflates areas towards the poles); `'none'` draws x/y as
  they are (km grids, floor plans).
- **Colour**: a measure colours on the theme's sequential ramp (over a
  `::BASEMAP` it starts a quarter in, so pale points don't vanish); when the
  values span 0, or with `::MIDPOINT n`, the theme's diverging ramp around that
  centre. `::RANGE 'lo,hi'` caps the colour domain (the legend then reads
  `≤lo` / `≥hi`). Without a measure a `::CATEGORY` colours by the theme palette
  (levels sorted, `#rrggbb` levels in their own colour); with neither, a light
  brand tint. Rows with geometry but no value get a hatch and a "No data (n)"
  legend entry.
- **Legend**: in the theme's legend typography, titled with the measure's
  name and unit (`sales AS "Revenue"` with `'€'::YFORMAT` → "Revenue (€)"),
  ticks at nice values in the panel's format (`€500K`, `1.4B`). It sits over
  a corner of the map that covers no data, else beside or above the map.
- **Size**: the map fills its panel at its own aspect; without `::HEIGHT` a
  map panel takes the height its shape needs (a world map is wide and low, a
  country tall).
- **Classes** (`::CLASSES`): `'quantile n'` (equal counts; ties give fewer
  classes), `'jenks n'` (Fisher-Jenks natural breaks; above 2,000 values on an
  even sample), `'equal n'` (equal width) — n from 2 to 9, default 5 — or
  custom edges `'0,10,50,100'` (values outside go to the open end classes,
  `< 10` / `≥ 50`). The legend shows each class's colour with its count and
  the edges under it; the tooltip names the class ("class 2 of 5: 10 – 50").
  `dashboard --check` reports `map/bad-classes` and `map/unknown-projection`.
- **Backdrop** (`::BASEMAP`): a built-in layer (`'world'` — without
  Antarctica —, `'de-states'`, `'de-districts'`, `'world-50m'`) or a second
  WKT column, drawn first in a pale land colour with hairline borders;
  `::ALPHA` sets the data layer's opacity.

#### Graduated symbols (`::SIZE`)

```sql
SELECT ags::REGION, customers::SIZE, dc_name::CATEGORY, district::LABEL,
       'Customers per district, by distribution centre'::TITLE
FROM coverage;
```

A numeric `::SIZE` column with at least two distinct finite values draws a
circle at each feature's anchor instead of filling it — the point itself
(`::LAT`/`::LON`, a WKT `POINT`), or a region's/polygon's pole of
inaccessibility — scaled so its **area** is proportional to the value:
`r = rmax · √(v / vmax)`, `rmax` a 4 % share of the panel's shorter side
(clamped 6–28 px). Lines ignore `::SIZE` and draw as today. A constant
`::SIZE` (e.g. `4::SIZE`) sets every symbol to that fixed radius in px, with
no size legend. Symbols are drawn **largest first** (so small ones stay
visible), which is the one place a map breaks row order — `data-row` still
holds the row.

Colour follows the same rule as a choropleth: the measure through the
sequential/diverging ramp (classes, `::RANGE` and `::MIDPOINT` all work),
else `::CATEGORY`, else the brand tint; `::ALPHA` sets the fill opacity
(default 0.75).

On a `::REGION` map with no explicit `::BASEMAP`, the whole matched layer
(Antarctica excluded) is drawn automatically as an unfitted backdrop — the
map still fits to the drawn symbols, not to the whole layer — and the rows'
own regions are not filled individually (D4); an explicit `::BASEMAP` keeps
today's behaviour (fitted too).

A **size legend** stacks under the colour legend: nested circles at three
"nice" values (the largest, a quarter and a sixteenth of it, deduplicated,
dropping any under 3 px), bottom-aligned, each with a leader line to its
formatted value.

Each symbol carries `data-size` (the raw value) and `data-fsize` (formatted,
always present); `data-value` is the colour measure, or the size value when
there is no colour measure. The summary reads "`n` symbols
(`projection`). Largest: `name` `value`; smallest: `name` `value`. Area ∝
`measure`", then the colour classes/no-data as a choropleth, then "`k` not
drawn (zero or negative size)" when any are skipped. `dashboard --check`
flags skipped rows with `map/size-not-positive`.

- **Labels** (`::LABEL_TOP n`: the n most important; `::DATALABELS`: all
  that fit): a region's name sits at its pole of inaccessibility (inside even
  a C-shaped region), a point's to its right, in the ink with a halo in the
  card colour. Importance is `::RANK`, else |value|, else area; a label that
  would overlap another, the legend or the attribution, or is wider than its
  region, is left out.
- **Scale bar**: on a map of lon/lat data less than about 3,000 km wide, a
  "nice" length (1/2/5 × 10ⁿ km) at the map's centre, bottom left.
- **North arrow** (`::NORTH_ARROW`): off by default. `'print'` draws it
  only in print, PNG, PDF and `--svg` output (the screen hides it with the
  `dp-print-only` class); `'always'` shows it on screen too. It sits top
  right and points to **true north** at the centre of the window shown —
  under a conic or Equal Earth projection, away from the central meridian,
  that is not straight up. `--check` warns on another value
  (`map/north-arrow-value`).
- **Hover / click / zoom**: each feature carries `data-row`, `data-series`
  (its `::CATEGORY`, else its `::LABEL`), `data-value`, `data-label`,
  `data-x` (its key — the row — when it has no label; else the label is the
  key) and `data-class`; a graduated symbol adds `data-size`/`data-fsize`
  (above); the root carries `data-map`, `data-proj` and `data-classes`.
  Scroll zooms and drag pans in the browser (double-click a feature zooms to
  it, elsewhere resets — see *Zoom* below); the zoom window is in the
  projected units, and each zoom re-renders, so simplification acts as level
  of detail. The root's `data-viewport` (ggplot-rs) is the rect the window is
  drawn into, so the point under the pointer stays put while zooming.
- **Accessibility**: the SVG's `<desc>` (and the chart's aria label) is one
  sentence — "400 regions (equal-area conic). Highest: München 980; lowest:
  Zweibrücken 12. 5 quantile classes: 80 / 80 / 80 / 80 / 80; 3 without
  data." "Charts as tables" lists name, series, value and class per feature,
  highest first, without the WKT.
- **Size**: above 100,000 drawn marks the render warns (aggregate in SQL, e.g.
  with the h3 extension); `map/too-many-marks` flags more than 20,000 points.

#### Flow maps (`::ORIGIN`, `::DEST`)

```sql
SELECT {'lon': k.lon, 'lat': k.lat, 'name': k.name || ' DC'}::ORIGIN,
       d.state::DEST, sum(v.deliveries) AS "Deliveries"::SIZE,
       k.name || ' DC'::CATEGORY,
       'Deliveries from each DC into each state, last 12 weeks; width by volume.'::INFO
FROM dc_deliveries v JOIN dc_dcs k USING (dc) JOIN dc_districts d USING (ags)
GROUP BY ALL ORDER BY 3 DESC;
```

An `::ORIGIN`/`::DEST` pair draws a bent arc between two places. Each end
takes one of three forms:

- a **region code** (`'DE-BY'::ORIGIN`), looked up in the boundary pack the
  same way `::REGION` is;
- a **WKT `POINT`** (`'POINT(9.18 48.78)'::ORIGIN`);
- a **struct-as-JSON** (`{'lon': 9.18, 'lat': 48.78, 'name': 'Stuttgart'}::ORIGIN`)
  — `lon`/`lng`/`x` and `lat`/`y` (any case), plus an optional `name`; without
  one, the coordinates are shown ("48.78° N, 9.18° E").

**Arc shape (D6):** a quadratic curve whose control point sits 0.2 × the
chord's length to the **right of the direction of travel** — so A→B and B→A
draw as two separate, visible arcs rather than overlapping. The arrowhead
sits at 85 % along the arc, sized from the stroke width.

**Width:** a non-constant numeric `::SIZE` scales `width = 0.75 +
7.25·√(v / vmax)` px (`v ≤ 0`/NA isn't drawn); a constant `::SIZE` fixes
every arc's width (clamped 0.5–12 px, no legend); with no `::SIZE` every
flow is 1.5 px. A **width legend** (three nice values, the same derivation
as the symbol size legend) stacks under the colour legend when widths vary.

**Colour** follows a measure through the usual ramp/classes, else
`::CATEGORY`, else the brand tint; stroke opacity is 0.85, or `::ALPHA`.
`'flow'::LINESTYLE` marks the arc `class="dp-flow"` and dashes it (the CSS
animates the dashes, so goods/orders read as moving); `'dashed'::LINESTYLE`
dashes without the animation class.

**Backdrop (D4):** when both ends are region codes and there is no explicit
`::BASEMAP`, the whole matched layer (Antarctica excluded) is drawn as an
unfitted backdrop automatically — the map still fits to the flows' own
extent (padded 8 %, wider than a choropleth's 4 %, so the arcs' bulge
fits), not to the whole layer.

**End labels:** `::LABEL_TOP`/`::DATALABELS` label the distinct **ends**, not
the arcs — an end's importance is the sum of the sizes of every flow
touching it.

**Attributes:** each arc carries `data-row`, `data-series` (`::CATEGORY`,
else the origin's name), `data-label` (`::LABEL`, else `"{origin} →
{dest}"`), `data-x` (the end-pair key), `data-orig`/`data-dest` (the
resolved names), `data-value`, `data-size`/`data-fsize` and `data-fvalue`
(once — never duplicated with a colour measure's own `data-fvalue`). Each
arrowhead carries `data-head`, `data-series` and `data-x`, but **no**
`data-row` — it is not a keyboard stop or an as-table row, though it dims
with its arc's series on hover. A small dot marks every distinct end.

**Caps:** above 10,000 flows, only the 10,000 widest are drawn, with the
render warning "map: `n` flows, the 10,000 widest drawn — aggregate in
SQL". `dashboard --check` warns above 2,000 flows
(`map/too-many-flows`), flags an `::ORIGIN` with no `::DEST` or the reverse
(`map/flow-needs-both`), an end that matches no region
(`map/flow-missing-end`), and notes flows whose two ends are the same place
(`map/flow-self`) — none of those three are drawn.

The summary reads "`n` flows between `o` origins and `d` destinations
(`projection`). Largest: `A` → `B` `value`; smallest: `C` → `D` `value`",
plus "; `k` not drawn (no location)" and "; `s` start and end at the same
place" when either is non-zero.

#### Density (`::HEXBIN`)

```sql
SELECT lat ::LAT, lon ::LON, 'auto'::HEXBIN, 'quantile 5'::CLASSES,
       'de-states'::BASEMAP, 'Customers cluster around three hubs'::TITLE
FROM customers;
```

`::HEXBIN` aggregates a map's points — `::LAT`/`::LON`, or WKT
`POINT`/`MULTIPOINT` — into a grid of hexagons, so thousands of points read
as a density surface instead of an overplotted smear. Its value is `'auto'`
(default), a width across (`'25 km'`), or a bare number — metres, or on a
`'none'` projection planar units (`'km'` is accepted and ignored there, since
the written number is already the planar width). Applying it to a map with
no points (a `::REGION`/WKT-area map) does nothing — `dashboard --check`
flags it (`map/hexbin-needs-points`).

- **Auto size:** `nice_floor(longer side of the projected box / 40)` — a
  "nice" 1/2/2.5/5 × 10ⁿ width, from the data alone. Zooming or exporting
  never re-bins: the grid is anchored to the **unzoomed** projected data
  box, so the same point always falls in the same hexagon.
- **Value:** the sum of the chart's measure column when there is one, else
  a count of points per hexagon (D8 — no mean in M2). `::CLASSES`,
  `::RANGE` and `::MIDPOINT` all work exactly as on a choropleth; a bare
  `::HEXBIN` with no measure colours by count, and the legend defaults to
  "Points per hexagon".
- **Equal area:** under the default projections (`'auto'`, `'equal-earth'`,
  a planar `'none'`), every hexagon covers the same amount of ground
  regardless of where it sits on the map. `'mercator'` inflates hexagons
  towards the poles, same as a Mercator choropleth — `'auto'` keeps them
  equal-area (`dashboard --check` notes `map/hexbin-mercator`).
  `::CATEGORY` is ignored on a hexbin map (`map/hexbin-category`) — it is
  always coloured by count or sum.
- **Legend:** titled with the measure's name (as a choropleth), or "Points
  per hexagon" when counting; a note under it reads "Hexagons 2.5 km
  across" (planar: "Hexagons 2.5 across", no unit).
- **Attributes:** each hexagon carries `data-row`, `data-x` (its grid
  coordinate, `"q,r"`), `data-value`, `data-class` (classed), `data-count`
  (points in it) and `data-label` ("around 48.78° N, 9.18° E", the mean
  position of its points; planar: "around x 4.2, y 1.8"); no
  `data-series`. The root carries `data-hexbin` (the resolved width).
  Above 20,000 hexagons the render warns ("use a larger `::HEXBIN`") —
  points never count toward that cap, only cells do.
- **Served data:** a hexbin map only bins the rows the server actually
  returns — `serve`'s `max_rows` (default 100,000) still applies, so a
  truly huge table should pre-aggregate (e.g. `GROUP BY` a grid) in SQL.

**Finer binning with h3.** Nothing about h3 exists in this extension's own
code — `::HEXBIN` is a fixed, regular grid, not h3 cells. For an h3-indexed
view (DuckDB's community extension), load it yourself and draw the cell
boundaries as ordinary `::MAP` polygons:

```sql
INSTALL h3 FROM community; LOAD h3;
SELECT h3_cell_to_boundary_wkt(h3_latlng_to_cell(lat, lon, 6))::MAP, count(*)::BARCHART, 'quantile 5'::CLASSES
FROM customers GROUP BY 1;
```

This needs the community extension, installed either natively (`INSTALL …
FROM community`) or, in the browser, by fetching it from outside this host
— which breaks on an offline or locked-down server (D9). `::HEXBIN` needs
neither: it has no extension dependency and no outside fetch.

#### Bivariate choropleths (`::BIVARIATE`)

```sql
SELECT ags ::REGION, dc_km ::BARCHART, per_10k ::BIVARIATE,
       district ::LABEL, 'Far and thinly covered districts share one corner'::TITLE
FROM dc_cover;
```

`::BIVARIATE` adds a second measure to a region/point map: with the chart's
own measure (e.g. `dc_km::BARCHART`), regions or points are coloured by
**tertiles of both** — a 3×3 palette and legend, the classic
bivariate-choropleth look. It needs a first measure to pair with
(`dashboard --check` warns `map/bivariate-needs-measure` and draws a plain
map otherwise), and has no effect alongside `::ORIGIN`/`::DEST`, `::HEXBIN`
or `::SIZE` (`map/bivariate-ignored`) — a statement is one layer.

- **Classes:** each measure is split into tertiles (`quantile 3`)
  independently; `::CLASSES` is ignored (`map/bivariate-classes`) — both
  measures are always thirds. Fewer than 3 distinct values in either
  measure gives fewer classes (`map/bivariate-few-levels`): 2 distinct
  values makes a low/high split, 1 makes everything the middle cell.
  A row missing either measure is no data (the hatch pattern).
- **Palette:** derived from the theme, not a fixed Stevens palette — each
  cell multiply-tints the theme's first two palette colours over white
  (dark theme: over the card colour), so the 9 cells always suit the active
  theme and stay distinguishable (a ΔE test checks this in light, dark,
  colourblind and high-contrast).
- **Legend:** a 3×3 grid, the first measure increasing to the right, the
  second increasing upward, with each cell's row count and a "{measure} →"
  caption under/beside the grid, plus a no-data key for rows missing either
  value.
- **Tooltip:** `"{label}: {measure A} {value} (low/mid/high), {measure B}
  {value} (low/mid/high)"`.
- **Attributes:** each feature carries `data-value`/`data-fvalue` (the
  first measure, same as any other measured map), `data-value2`/
  `data-fvalue2` (the second measure) and `data-bivar="{i},{j}"`
  (0-indexed tertile levels); no `data-class`. The root carries
  `data-bivar-titles` (both measures' names, as a JSON array).

#### Time maps (`::FRAME`)

```sql
SELECT state ::REGION, week AS "Week" ::FRAME,
       round(100 * on_time / deliveries, 1) AS "On time" ::BARCHART, ' %'::YFORMAT,
       '0,80,85,90,100'::CLASSES
FROM dc_weekly_on_time;
```

`::FRAME` turns a choropleth, graduated-symbol, flow, hexbin or bivariate
map into a time map: one result holds **every** frame (a week, a date, …)
of every feature, and the map shows one frame at a time. Static output
(`--svg`, PNG, PDF, a DuckDB
function) shows the **last** frame, named in the subtitle ("Week: 12") so
it's always clear which one that is. The browser adds a Play bar under the
map — ‹/› step a frame, Play/Pause auto-advances every second (1.5 s under
reduced motion), the slider and arrow/Home/End/Page keys jump to any frame,
and the current frame's position is announced (`aria-live`, muted while
Play is running so it doesn't flood a screen reader). Re-rendering for
zoom, resize, a theme switch and `::FIT` all keep the frame shown; "Charts
as tables" and an open tooltip follow it too. The step itself tweens
(300 ms): fills/strokes blend in RGB (a "no data" pattern swaps at the end,
no lerp) and `stroke-width` interpolates, same as `'tween'::TRANSITION`'s
re-run join elsewhere. Reduced motion keeps Play available (stepping
through time *is* the content) with instant, 1.5 s-apart steps, and it
never autoplays on its own. The step persists per statement across a
re-run, like `::STORY`'s step does; a map with a `-- @name` also keeps it in
the URL (`?frame.<name>=<label>`, see *Viewer state in the URL*), so a shared
link opens on the same week. Time maps that carry the same `'weeks'::SYNC_FRAME` value
step and play together: a step, the slider or Play on one moves every other
map of the group to the same frame label (a map without that label stays
where it is), and a map drawn later (a re-run) joins on the group's frame;
without a `::FRAME` it's a warning (`map/sync-frame-no-frame`). The ⤓ HTML export embeds every frame
(pre-rendered, so it still plays with no live query engine) and rebuilds the
same bar offline, as long as their total stays within 3 MB per map; above
that, a static note takes its place instead — "12 frames — Play in the live
dashboard".

- **Frame order:** ascending when every `::FRAME` value is numeric,
  chronological when every value looks like a date, else first
  appearance in the result (the SQL `ORDER BY`).
- **A frame's view:** every feature appears in every frame — a feature
  with no row in a given frame is drawn as no data (areas: the hatch
  pattern; points/symbols/flows/hex cells: not drawn at all) rather than
  vanishing, so the map's extent and projection never move between
  frames — including on a flow map, where this means every flow's
  endpoints count toward the shared extent even on a frame where that
  flow isn't drawn (a flow only present in one frame doesn't zoom or
  re-project the map on every step).
- **Shared scales:** the measure's colour domain and `::CLASSES` breaks,
  a `::CATEGORY`'s levels, and a graduated `::SIZE`'s domain are all
  computed once over **every** frame and held fixed — the same value
  gets the same colour/size whichever frame is on screen, the legend
  never drops a level or re-wraps as counts change. A flow's `::SIZE`
  (linewidth) shares that same domain. A hexbin map's grid (origin and
  width) is anchored to every frame's points at once, same as zooming
  never re-bins it (D7); a cell's colour is classed against every
  frame's cells binned once, not just the frame on screen, so a cell
  reads the same magnitude whichever frame is shown — a feature absent
  from a frame simply isn't binned into it that frame (not drawn with a
  stale value). `::BIVARIATE`'s tertile breaks for both measures are
  built once over every frame too — a cell's position in the 3×3 grid
  means the same thing whichever frame is shown; only its count is the
  frame's own. A feature's key for the browser's cross-frame animation
  (M2f) is its `::REGION` code, its `::LAT`/`::LON` pair, its WKT
  geometry, or — on a flow map — its `"{origin}→{dest}"` pair (M2b's own
  key) — `::XAXIS` is ignored on a time map (`map/frame-xaxis`).
- **Duplicates:** a repeated (feature, frame) row keeps its first row
  (`map/frame-duplicate`). On a hexbin map, a feature is a distinct point
  location (D22's `"{lat:.5},{lon:.5}"` key) — several events at the exact
  same coordinates in one frame (several incidents at one address, say)
  are duplicates of *that point*, so only the first is binned that frame;
  the same data without `::FRAME` bins every one of them. Join a small
  jitter, a nearby but distinct coordinate, or a count column summed
  before `::FRAME` instead of relying on exact coincidence.
- **Caps:** over 200 distinct `::FRAME` values, the last 200 are kept
  (`map/too-many-frames`); over 100,000 feature × frame cells, the last
  frames that fit are kept (`map/frame-cells`) — aggregate to a coarser
  period instead.
- **Charts too (#663):** on a bar, stacked bar, line, area or scatter chart
  with an `::XAXIS`, `::FRAME` plays the same way: one frame's rows at a
  time, named in the subtitle, on axes that span **every** frame (the y
  range, a stacked bar's tallest total, and a category axis keeps every
  frame's levels in order), so bars and lines move instead of the axes.
  On any other kind it's ignored (`frame/not-a-map`).

  ```sql
  SELECT region ::XAXIS, units AS "Units" ::BARCHART, year AS "Year" ::FRAME
  FROM sales_by_region_year;
  ```
- `::STORY` is ignored on a frame map (`map/frame-story`) — Play already
  steps through the frames. WKT geometry repeated per frame over 5 MB
  total gets a note (`map/frame-wkt`) to join a `::REGION` code or one
  geometry table instead.
- **Attributes:** the root `<svg>` carries `data-frames` (every frame's
  label, JSON), `data-frame` (the one shown, 0-indexed) and
  `data-frame-name` (the `::FRAME` column's display name); each feature's
  `data-row` follows the frame actually shown, like `::LINK` expects.

#### Small multiples (`::FACET` on a map)

```sql
SELECT d.state ::REGION, v.week AS "Week" ::FACET,
       round(100 * sum(v.on_time) / sum(v.deliveries), 1) AS "On time" ::BARCHART, ' %'::YFORMAT,
       '0,80,85,90,100'::CLASSES
FROM deliveries v JOIN districts d USING (ags)
WHERE v.week IN (3, 6, 9, 12) GROUP BY ALL ORDER BY v.week, d.state;
```

`<col>::FACET` next to a map's input draws one map per value, side by side
in one panel. Every map shares the **projection, extent, classes, colours,
symbol sizes and the one legend**, so the panels compare at a glance.

- **Layout:** each map takes the data's shape; the column count is the one
  that keeps every map at least 160 px wide (fewer on a phone), or
  `n::FACET_NCOL`. In the browser the panel takes the grid's natural height;
  for `--svg`/static output give it a `::HEIGHT`.
- **Order:** numbers and dates ascending, else the SQL order (as `::FRAME`).
- **Missing rows:** a feature with no row in one facet is drawn grey with no
  value there, still named in its tooltip.
- **Legend and furniture:** the legend shows no per-class counts (they
  would add up features × maps); the attribution is drawn once under the
  grid, the scale bar once in the last map.
- **Labels:** `n::LABEL_TOP` labels the top n regions of **each** map.
- **Flows, hexbin, bivariate:** flows share one width scale (the same
  volume draws equally thick in every map). Hexagons share one grid over
  every map's points, so a cell is the same place in each, and one colour
  scale; each map bins only its own points. A bivariate map's tertiles come
  from all rows, and its 3×3 legend is drawn once, in the last map, without
  counts.
- **Interaction:** hovering a region lights it up in every map; the zoom
  tools act on all maps at once (they share one window).
- **Limits:** at most 12 maps — the first 12 are drawn (`map/too-many-facets`).
  Not with `::FRAME` (`map/facet-frame`: use one or the other). `::FACET_FREE` behaves
  like `::FACET` on a map (`map/facet-free`); `::STORY` is ignored
  (`map/facet-story`).
- **Attributes:** the root carries `data-facets` (the values, JSON),
  `data-facet-name` and ggplot-rs's `data-panels`; each feature
  `data-facet`.

#### Trends in the tooltip (`::SPARKLINE` on a map)

```sql
SELECT state ::REGION,
       100.0 * sum(on_time) / sum(deliveries) AS "On time" ::BARCHART, ' %'::YFORMAT,
       list(weekly ORDER BY week)              AS "Weekly"  ::SPARKLINE
FROM weekly_by_state GROUP BY state;
```

A `list(..)::SPARKLINE` (or `::SPARKBAR`) column next to a map's input is
not a measure: hovering a feature shows a small trend chart for it under
its value, plus a line such as `Trend: 12 → 18 (+50%) · min 9 · max 21 ·
12 points`. It works on area, point, graduated-symbol and flow maps, and on
time maps (each frame shows its own row's list).

- **Static output** (`--svg`, PNG/PDF, the HTML export) keeps the trend
  line in the native tooltip; the chart itself is drawn by the browser
  only, so the SVG doesn't grow.
- **Attributes:** each mark with a trend carries `data-trend`.
- **Scale:** every feature's chart has its own y range, like table cells.
- **Not on hexbin maps** (`map/sparkline-hexbin`): a cell aggregates rows.
  A value that isn't a list warns (`map/sparkline-not-list`).

#### Density on a planar scatter (`::HEXBIN` off the map)

```sql
SELECT x ::XAXIS, y ::SCATTER, 'auto'::HEXBIN, 1::IDENTITY
FROM predictions;
```

`::HEXBIN` on a planar (non-map) `::SCATTER`/bubble chart bins the points
into a 2D count density instead of drawing each one — the fix for a
predicted-vs-actual scatter at thousands of rows, where individual points
blur into a solid blob and the identity line (`::IDENTITY`) is the point of
the chart. Drawn through ggplot-rs's own `geom_hex`/`StatBinHex` (the same
stat the map's `::HEXBIN` uses), with a "count" fill legend; `::IDENTITY`,
`::ABLINE` and `::REFLINE` are still drawn on top, unaffected.

- **Bin count, not a width:** `'auto'` (default) sizes the lattice from the
  panel's own pixel size; a bare number sets the bin count *across each
  axis* (ggplot2's own `stat_bin_hex(bins = …)` convention) — unlike the
  map's `::HEXBIN`, which takes a width. A lopsided x/y data range can
  stretch the hexagons for a given bin count, same as ggplot2's.
- **Count only:** always a count of points per hexagon — there is no
  "sum the measure" case here (a planar scatter's y *is* the measure).
- **`::CATEGORY` is ignored** (`chart/hexbin-category`, a Note under
  `dashboard --check`) — one density for the whole panel, not one hexagon
  grid per category value, the same rule the map's own `::HEXBIN` has.
- **Tooltip:** each hexagon's hover reads "n = `<count>`" (`GeomHex`'s own
  default).
- A malformed value (not `'auto'` or a number) is a hard error, same as
  any other role.

#### Built-in boundaries (`::REGION`, `::BOUNDARIES`)

Countries, German states and German districts ship with the app as
same-origin files in `web/data/boundaries/` (sources, licences and the build
in [`SOURCES.md`](../web/data/boundaries/SOURCES.md),
`scripts/build-boundaries.sh`). The browser fetches a layer the first time a
panel needs it (no outside request; it works offline and on the locked
server; `dashboard --site` publishes them with the app); native renders (`dashboard --svg`, `serve`, the extension) carry the
default layers embedded, so they need no file access. The pack is not part of
the wasm.

| Layer | Codes a `::REGION` may hold | Features |
|---|---|---|
| `world` | ISO 3166-1 alpha-2 / alpha-3 (`'FR'`, `'FRA'`), Natural Earth `ADM0_A3` | 177 countries (Natural Earth 1:110m) |
| `world-50m` | as `world` (only with `'world-50m'::BOUNDARIES`; browser only) | 242 countries (1:50m) |
| `de-states` | ISO 3166-2 (`'DE-BY'`), the 2-digit AGS (`'09'`), NUTS 1 (`'DE2'`) | 16 Länder (BKG VG2500) |
| `de-districts` | the 5-digit AGS (`'09162'`), NUTS 3 (`'DE212'`); a NUTS 2 code (`'DE21'`) draws all its districts | 400 Kreise (BKG VG2500) |

The layer most codes match is used; `'de-districts'::BOUNDARIES` forces one.
When no code matches, names are tried (`'Bayern'`, `'Köln'`, `'Germany'`;
case and umlauts folded) and `dashboard --check` notes `map/region-by-name`.
A code read as a number keeps working (`9162` → `09162`). Codes that match
nothing are not drawn and are listed by `map/unknown-region`; a `::LAT`
beyond ±90 next to a `::LON` within ±90 is flagged as `map/lat-lon-swapped`.

The hover and the map labels use the region's name in the dashboard locale
(`::LOCALE 'de-DE'` → "Bayern", else "Bavaria"); a `::LABEL` from the data
always wins. Every map drawn from a layer carries its attribution, small and
muted in the bottom-right corner: "Made with Natural Earth" for the world
layers; for the BKG layers the source note the licence requires — "© BKG
(2026) dl-de/by-2-0, Datenquellen: https://sgx.geodatenzentrum.de/web_public/gdz/datenquellen/datenquellen_vg_nuts.pdf"
with "BKG" and the licence linked — and "geometry modified (simplified)".

The files are plain CSV (`key,codes,name_en,name_de,alt,wkt`), so SQL can
join them too: `FROM 'data/boundaries/world.csv'` (with the `spatial`
extension, `ST_GeomFromText(wkt)` for point-in-polygon).

**EU NUTS regions are not included**: Eurostat/GISCO's terms allow only
non-commercial use. `scripts/fetch-nuts.sh --accept-terms` downloads NUTS
2024 (1:20M) for you — you hold the licence — and writes `nuts0` … `nuts3`
layers next to the pack (gitignored); use them with `'nuts2'::BOUNDARIES`.
Maps drawn from them carry "© EuroGeographics for the administrative
boundaries", as the terms require.

### Waterfall, bullet and funnel

The classic variance views. All three are drawn from plain bars, segments and
text, so headless SVG / PDF exports match the browser, and every bar carries the
usual hover metadata (`data-x`, `data-series`, `data-value`; the series is the
row's own `::XAXIS` value): tooltips show the formatted value, and a click
anywhere on a step / row / stage cross-filters — `getvariable('selected')` (or
the `::SELECT_AS` name) is its `::XAXIS` value.

```sql
-- A bridge: the first row is the start total, the others signed steps.
SELECT step::XAXIS, eur::WATERFALL, '€'::YFORMAT, 'Volume beat budget'::TITLE
FROM bridge ORDER BY i;

-- Measure vs goal over qualitative bands, one horizontal row per category.
SELECT region::XAXIS, attained::BULLET, goal::GOAL, '60,85,110'::RANGE,
       'Two regions short of target'::TITLE
FROM attainment ORDER BY i;

-- Conversion stages, first on top.
SELECT stage::XAXIS, n::FUNNEL, 'A quarter of sign-ups never start a trial'::TITLE
FROM funnel ORDER BY i;
```

- **`::WATERFALL`** (alias `::BRIDGE`): rows in SQL order. The first row is the
  **start total** (a full bar from 0), every further row a **signed step**
  floating from the running total — increases in the theme's `positive`
  colour, decreases in `negative`, totals in the brand colour. A row whose value
  is **NULL** is a subtotal (the running total so far); a row whose
  `::CATEGORY` is `'total'` / `'subtotal'` is a total at its own value (e.g. the
  booked actual). Unless the last row is a total, the bridge closes with an
  **End** bar. Thin connectors carry the level from bar to bar; every bar is
  labelled (steps signed, `::YFORMAT` short form). `data-value` is the step's
  own change (a total's level).
- **`::BULLET`** (alias `::BULLETCHART`): one horizontal row per `::XAXIS`
  value, first row on top. `::GOAL` (an alias of `::REFLINE`) is each row's
  target, drawn as a strong tick across the bar; its tooltip reads
  `Goal 85%`, a band's `Range 60 – 85` (only the measure carries a value).
  `::RANGE` holds the qualitative band edges, ascending — `'60,85,110'` gives
  three graded washes 0–60, 60–85, 85–110; one value applies to every row, a
  per-row column gives each row its own bands. Without `::RANGE` the bullet is
  a bar with a goal tick.
- **`::FUNNEL`** (alias `::FUNNELCHART`): stages in SQL order, first on top,
  each bar centred with its width by value. The label beside each bar is the
  value and the conversion from the previous stage (`6,200 · 13%`). Negative
  values are skipped; the value axis is hidden (widths read relative to the
  first stage).
- Each needs an `::XAXIS` — `--check` reports `chart/needs-xaxis` otherwise.

### Diagrams: pipelines, lineage and processes (`::DIAGRAM`)

A node/edge diagram drawn from data: boxes for the steps, arrows for what flows
between them, laid out automatically. One row per node; the `::DIAGRAM` column
is the node's id.

```sql
-- Edges from each node's inputs (a list of ids).
SELECT stage::DIAGRAM, title::LABEL, records::COMPACT, status::BADGE,
       inputs::UPSTREAM, 'How data moves through the pipeline'::TITLE
FROM run_manifest ORDER BY seq;

-- Edges with a value, a label or a style: a list of structs (one scalar subquery).
SELECT stage::DIAGRAM, title::LABEL, records::COMPACT,
       (SELECT list({'from': from_stage, 'to': to_stage, 'value': n,
                     'label': format('{:,}', n), 'style': 'flow'})
        FROM run_edges)::EDGES,
       'TB'::DIRECTION
FROM run_manifest ORDER BY seq;
```

- **Nodes.** `::DIAGRAM` (alias `::NODE`) is the id (text or a number; unique —
  the first row wins). In the box: `::LABEL` (default: the id; wrapped to two
  lines), a value in a KPI format (`::METRIC` / `::MONEY` / `::PERCENT` /
  `::COMPACT`, in the dashboard locale) and a `::BADGE` status pill.
  `::THRESHOLDS` gives a node its status from its value (else the badge's
  word does, as in a table's pills: `ok`, `at risk`, `failed`, `alarm`, …): a status bar on the box, a `warn` /
  `bad` border. `::CATEGORY` gives nodes a palette accent; a `::COLORS` level
  map pins a type's colour (`'supplier:#2a9d8f; stock:good; demand:#e76f51'`,
  status words or `#rrggbb`, #699) — so a type keeps its colour whatever order
  the rows (or an edit) bring the types in.
- **Edges.** `::UPSTREAM` (alias `::PARENTS`) holds the node(s) feeding the
  row's node: one id or a list. `::EDGES` (the first non-NULL cell) is a list
  of structs `{from, to [, value, label, style, status]}` — `source` / `target`
  work too. With values, an edge is **as wide as what it carries** (area-true,
  1.4–7 px, the widest edge 7 px); `status` is a health word (`ok`, `delayed`,
  `slow`, `failed`, `late`, … — the words of a table's badges) that colours the
  edge amber or red. `style` is `'solid'`, `'dashed'`, `'flow'` (moving dashes in the
  browser) or `'flow-dots'` (dots travelling along the edge); static output
  keeps the dashes / dots. Both may be combined; the same `(from, to)` pair is
  one edge. `::LINESTYLE` on a row styles the edges **into** that row's node
  (`'flow'::LINESTYLE` as a constant: every edge); a `style` in `::EDGES`
  wins.
- **Layout.** Layers run left to right (`'TB'::DIRECTION`: top to bottom); a
  loop (`calibrate → build`) is drawn as an edge running back. The layout is a
  pure function of the rows (ties follow row order), so the browser,
  `dashboard --svg` and exports draw the same picture, scaled down to fit the
  panel (`::HEIGHT` gives a large diagram room). On a phone the panel keeps
  the text readable and scrolls sideways instead. At most 150 nodes and 300
  edges are drawn (the first ones), with a note.
- **Swimlanes, pins, sparklines.** `::LANE` puts each node into a named
  band (lanes in first-appearance order; a lane's nodes stay together in
  every stage, unlaned nodes go last, without a band). `::XAXIS` pins a node to
  a stage (0, 1, …; the rest are laid out around it, empty stages kept, an
  edge the pins make run within or against the stages is drawn straight);
  `::XAXIS` **and** `::YAXIS` on every node place each box on a grid (x across,
  y down, in box widths / heights; fractions allowed) with straight edges —
  for a hand-placed explainer; on only some nodes `--check` notes
  `diagram/partial-pins`. A `list(v ORDER BY t)::SPARKLINE` column draws a
  mini trend at the bottom of the box (amber / red with the node's status).
- **Speed by volume.** The styles `'flow-volume'` and `'flow-dots-volume'`
  move faster the more an edge carries (the busiest edge cycles in 0.6 s, a
  trickle in 2.4 s; with *reduce motion* nothing moves).
- **From a contract table.** `anofox_plot_diagram(n, edges := (SELECT list(e)
  FROM run_edges e))` draws the same diagram from a `graph` table (see
  *Plotting model output*): `SELECT anofox_plot_diagram(m, edges := (SELECT
  list(e) FROM run_edges e), mainstat := 'records_accepted') FROM
  run_manifest m` reads inventory-rs's run manifest as written.
- **Interaction.** A node is a mark like a bar: its tooltip shows its label,
  value and status, a click cross-filters (`getvariable('selected')` is the
  node id), `::SELECT_AS` names the selection and a `::LINK` column follows
  the clicked node's row. Edges show a tooltip only (a click on an edge does
  nothing: the node is what a selection or link names).
- **Motion** (browser only; *reduce motion* shows the final state). The
  entrance plays stage by stage: each layer's nodes fade in, then every edge
  draws from its source (`'none'::ANIMATE` turns it off). A `::STORY` caption
  on a row makes a step that lights that node and **every edge touching it**
  (in and out) and dims the rest; Step / Play walk the steps in row order, as
  on a chart. With `'tween'::TRANSITION`, a re-run that keeps the nodes in place
  (a run picker, a cross-filter) changes edge widths and colours, node borders
  and box contents in place instead of redrawing.
- **Accessibility.** The summary reads *"Diagram: How data moves through the
  pipeline; 7 steps in 5 stages, from Ingest to Recommend orders; 8
  connections; 1 loop (Calibrate → Build twin); largest Ingest (10K)."*. The
  diagram is one Tab stop; the arrow keys follow the **connections**: → to the
  next step (pressed again on the same node: its next connection in turn), ←
  back the way you came, ↑/↓ within a stage (on a `'TB'` diagram ↓/↑ follow the
  connections and ←/→ move within a stage), Home / End the first / last step.
  Each node reads its tooltip and where it connects (*"… from Ingest,
  Calibrate. to Step & project, Replay history"*); reached along an edge it
  first names that edge (*"Ingest → Build twin: 9.8K. …"*). *Charts as
  tables* shows the steps as an ordered list (each with where it leads) and the
  connections as a table; the SVG's `<desc>` carries the same list headless.
- `dashboard --check` reports `diagram/null-id`, `diagram/duplicate-node`,
  `diagram/unknown-node` (an edge to an id that is no row),
  `diagram/bad-edges`, `diagram/bad-style`, `diagram/bad-direction` and
  `diagram/too-large`, `diagram/bad-pin` (a non-numeric `::XAXIS` /
  `::YAXIS`) and `diagram/partial-pins`. A loop is no problem: it is drawn as an edge running
  back and named in the summary.

### Drawing the graph by hand (`'edit'::DIAGRAM_MODE`)

A model graph — suppliers feeding warehouses feeding stores — can be edited
in the page (#682): add a node, connect two, rename or remove one. The edit
produces **rows, not pixels**: one variable holding the whole edited graph,
which the dashboard's SQL reads like any other variable (the twin re-runs on
the edited graph).

```sql
-- optional: the node types, and which types each may feed
SELECT type ::DIAGRAM_PALETTE, label, connects_to FROM (VALUES
  ('supplier', 'Supplier',       ['stock']),
  ('stock',    'Stocking point', ['stock', 'demand']),
  ('demand',   'Demand',         [])) t(type, label, connects_to);

SELECT id ::DIAGRAM, name ::LABEL, kind ::CATEGORY, upstream ::UPSTREAM, stock ::METRIC,
       'edit' AS graph ::DIAGRAM_MODE                  -- the alias names the variable
FROM network_node;

-- the edited graph as rows
SELECT n->>'id' AS id, n->>'type' AS type, n->>'label' AS label
FROM (SELECT unnest((getvariable('graph')->'nodes')::JSON[]) AS n);
SELECT e->>'from' AS src, e->>'to' AS dst
FROM (SELECT unnest((getvariable('graph')->'edges')::JSON[]) AS e);
```

The variable (default `graph`) holds `{"nodes": [{"id", "type", "label"}],
"edges": [{"from", "to"}]}` — the type is the node's `::CATEGORY`. It starts
as the diagram's own rows; a toolbar above the diagram edits it:

| Control | Does |
|---|---|
| type + name + **Add** | a new node of the chosen type (its id from the name: `Supplier 2` → `supplier_2`) |
| **Connect** | click the source node, then the target; the palette's `connects_to` refuses an edge a type may not draw, with the reason |
| drag a node onto another | the same as Connect, in one gesture (#699); a press that doesn't move still picks the node |
| click a node / edge, then **Rename** / **Remove** (or the Delete key on a focused node) | rename the node (the name field), remove it with its edges, or remove the edge |
| **Reset** | back to the graph as the data has it |

Clicking a node still selects it, so a `::FORM` filtered by the selection
shows that node's parameters. The diagram is redrawn from its rows with the
edits applied: an existing node keeps its row (value, badge, lane,
sparkline), an existing edge its `::EDGES` value / label / style; a new node
is a plain box in its type's colour (pin it with a `::COLORS` level map); edges come from the edited list. When
the diagram's own rows change, the edits start again from them. Edits are
kept in the URL (`?v.graph=…`) but not saved for other viewers (#684).

Without `::DIAGRAM_PALETTE` the type list is the types the rows use, and any
node may feed any other.

**Checking the edited graph** (`::DIAGRAM_CHECK`, #699). A statement right
before the diagram (next to the palette, either order) reads the edited graph
and returns its problems, one row each: the `::DIAGRAM_CHECK` column names a
node id, `'from -> to'` for an edge, or NULL for the graph as a whole;
`status` is a status word (`bad` / `error`, `warn` / `warning`, `ok`, … — the
`::BADGE` words; anything else counts as `warn`) and `message` says what is
wrong. It runs again after every edit:

```sql
SELECT id ::DIAGRAM_CHECK, status, message FROM (
  -- a node nothing connects to
  SELECT n->>'id' AS id, 'bad' AS status, 'not connected to anything' AS message
  FROM (SELECT unnest((getvariable('graph')->'nodes')::JSON[]) AS n)
  WHERE n->>'id' NOT IN (SELECT e->>'from' FROM (SELECT unnest((getvariable('graph')->'edges')::JSON[]) AS e)
                         UNION SELECT e->>'to' FROM (SELECT unnest((getvariable('graph')->'edges')::JSON[]) AS e))
  UNION ALL
  -- the whole graph
  SELECT NULL, 'warn', 'only one supplier — no fallback'
  WHERE (SELECT count(*) FROM (SELECT unnest((getvariable('graph')->'nodes')::JSON[]) AS n)
         WHERE n->>'type' = 'supplier') < 2);
```

A flagged node gets the status colour (its dot, over its `::THRESHOLDS` /
`::BADGE` status), a flagged edge its colour; under the toolbar the problems
are listed worst first (at most 12, then "… n more"), and a node's or edge's
entry picks it for Rename / Remove. A check query that fails says so in the
list. On a locked server it is one more declared statement — the server runs
it, the viewer only sends the graph. `--check` warns when a `::DIAGRAM_CHECK`
has no editable `::DIAGRAM` right after it (`diagram_check/no-diagram`); the
static outputs draw the rows' own graph without checks. `--check` reports an unknown mode
(`diagram_mode/unknown`, an error) and a `::DIAGRAM_MODE` without a
`::DIAGRAM` column (`diagram_mode/no-diagram`); the static outputs draw the
rows' own graph and bind it to the variable. **On a locked server** the
graph travels as one string variable — at most 4 KB of JSON (roughly 40
nodes); a larger edit isn't applied and the toolbar says so. As with any
viewer input, the dashboard's SQL must treat the graph as untrusted (ids
and labels are text the viewer typed).

### Formatted values in Markdown (`{{col}}`, `{{col:fmt}}`)

A `::MARKDOWN` text can carry numbers from **its own row**, formatted in the
dashboard locale exactly like the KPIs — no string concatenation that loses the
locale (`1234.5` instead of `1.235 €`):

```sql
SELECT 'Revenue grew **{{growth:pct}}** to **{{revenue:money}}** (data until {{day:date}}).'::MARKDOWN,
       100 * (sum(rev) FILTER (WHERE yr = 2026) / sum(rev) FILTER (WHERE yr = 2025) - 1) AS growth,
       sum(rev) FILTER (WHERE yr = 2026) AS revenue,
       max(day) AS day
FROM sales;
```

`{{name}}` names a column of the same `SELECT` (case-insensitive; the other
columns of a `::MARKDOWN` statement keep their names), `:fmt` picks the format:
`money` (`currency`, `eur`, `usd`), `percent` (`pct`; the value is in percent
points, like `::PERCENT`), `compact`, `metric` (`number`), `date`, `text`. No
format: a number or an ISO date in the locale, other text as is. Values are
inserted as **text** — HTML-escaped, never Markdown or markup, and a placeholder
inside a link URL is dropped — after the Markdown is rendered and sanitised (the
sanitiser is unchanged). An unknown name stays as written; `--check` warns
(`markdown/unknown-placeholder`, `markdown/unknown-format`). The *A German branch
report* sample opens with such a summary.

### Formulas in Markdown (`$…$`, `$$…$$`)

A `::MARKDOWN` text (and so a `::CALLOUT`) can explain a model or a KPI
definition with **TeX math**: `$…$` inline, `$$…$$` as a centred display
formula (on its own lines or in a paragraph):

```sql
SELECT 'Simple exponential smoothing weighs the latest observation by $\alpha$:

$$\hat{y}_{t+1} = \alpha y_t + (1-\alpha)\hat{y}_t$$

With $\alpha = {{alpha}}$ the forecast reacts within a few weeks.'::MARKDOWN,
       0.3 AS alpha;
```

- The delimiters follow KaTeX auto-render / pandoc: an opening `$` needs a
  non-space right after it, a closing `$` a non-space right before it and no
  digit right after it, and an inline formula ends at its first unescaped `$`
  — so *"costs $5 and $10"* stays text. `\$` is always a literal dollar; a
  `` `code` `` span is never math.
- `{{col}}` / `{{col:fmt}}` work inside a formula too: the formatted value is
  set as TeX text (`\text{…}`, its `$ % & # _ { } \ ^ ~` escaped), so a value
  can never become a TeX command.
- **Browser:** KaTeX (pinned, with SRI) is loaded from jsDelivr only when a
  dashboard has a formula. It writes MathML for screen readers next to the
  visual HTML and takes the text colour, so it follows light and dark themes.
  KaTeX runs with `trust: false`: `\href`, `\url`, `\includegraphics` and
  `\htmlClass`/`\htmlId`/`\htmlStyle`/`\htmlData` draw as error text, never
  as links or markup. Offline, or for a formula KaTeX rejects, the TeX source
  stays in monospace (marked in the error colour, the reason on hover).
- **⤓ HTML export:** the typeset formulas and KaTeX's stylesheet with its
  fonts (woff2, inlined) are embedded, so the file draws them offline.
- **Headless** (`dashboard`, `serve`, `--svg`): the TeX source in a monospace
  font; a display formula gets lines of its own.
- `--check` reports a formula KaTeX can't parse as `markdown/bad-math`
  (unbalanced braces, `\left` without `\right`, `\begin`/`\end` that don't
  match, an unknown command or environment, a disabled command).

### Definitions and callouts (`::INFO`, `::CALLOUT`)

A panel's fine print doesn't need its own box. An `::INFO` column puts an **ⓘ**
on the panel's title bar (top-right corner when it has no title); hovering it,
tabbing to it or clicking it shows the text, `Esc` dismisses it:

```sql
SELECT sum(revenue)::MONEY, 'Revenue'::LABEL,
       'Net of returns, in USD; source: the ERP order ledger.'::INFO
FROM orders;
SELECT week::XAXIS, sum(n)::BARCHART, 'Weekly orders'::TITLE,
       'Orders by ISO week; cancelled orders excluded.'::INFO
FROM orders GROUP BY ALL ORDER BY 1;
```

It works on charts, KPIs, `::TABLE`s (the column is not shown as a cell) and
Markdown boxes; in headless SVG the ⓘ follows the title and carries the text as
its tooltip (`<title>`).

`::CALLOUT` turns a `::MARKDOWN` box into a highlighted **note**, **warning**
or **success** box (tinted, with an icon; theme tokens, so dark mode follows):

```sql
SELECT '**Margins of the last two weeks are provisional** — late returns still come in.'::MARKDOWN,
       'warning'::CALLOUT;
```

`info` counts as `note`, `tip` as `success`, `caution` / `danger` as `warning`;
`--check` warns on an unknown kind (`callout/unknown`) or a `::CALLOUT` without
`::MARKDOWN` text (`callout/no-text`). The icon is browser-only (builder,
`serve`, locked servers); the tinted box itself (see `::MARKDOWN_STYLE`
below) is also drawn in `--svg` and other static output. The *A sales
cockpit in your brand* sample has both.

### Plain, card or callout (`::MARKDOWN_STYLE`)

By default every `::MARKDOWN` box is drawn as a tinted callout — fine for a
note or warning, but a methodology tab or a detail pane next to a diagram
reads as a stack of "info boxes" even without `::CALLOUT`. `::MARKDOWN_STYLE`
(#536) picks the box's chrome instead:

```sql
SELECT 'Monitoring answers three questions, chunk by chunk: is the input the
same shape as training, does the model still predict well, and has the
world it predicts moved on?'::MARKDOWN, 'plain'::MARKDOWN_STYLE;
```

| Value | Looks like |
|---|---|
| `plain` | Text on the page background: no card, no tint, no accent bar, normal page padding. |
| `card` | A plain white card, like a chart or KPI panel — no tint, no bar. |
| `callout` | Today's tinted box (the default). `::CALLOUT` implies this. |
| `caption` | No box: the text goes under the chart right before it, in that chart's card (#692). |

A dashboard-wide default comes from a `-- @markdown_style <plain\|card\|callout>`
header (see *Dashboard header comments*); a panel's own `::MARKDOWN_STYLE`
overrides it, and `::CALLOUT` always draws `'callout'` for its own panel
regardless of that default. `::CALLOUT` together with `'plain'`/`'card'` is a
lint warning (`markdown_style/conflicts-with-callout`): the explicit style
wins, so the callout's tint, bar and icon are dropped. `--check` also warns
on an unknown value (`markdown_style/unknown`) or a `::MARKDOWN_STYLE` without
`::MARKDOWN` text (`markdown_style/no-text`). Browser, `--svg` and other
static/HTML output all draw the same choice.

**A chart's caption** (`'caption'`, #692): a source note, a definition or a
"how to read this" right after a chart becomes that chart card's footer — a
thin rule, then the text in a smaller, muted ink — so chart and caption read
as one unit and share the chart's column and width:

```sql
SELECT day::XAXIS, orders::LINECHART, 'Orders per day'::TITLE FROM daily;
SELECT 'Source: ERP, returns netted. Below **{{target}}** is on plan.'::MARKDOWN,
       'caption'::MARKDOWN_STYLE, 120 AS target;
```

Its `{{values}}` and formulas work as in any `::MARKDOWN`. Several captions in
a row stack under the same chart; its `::TRACK`s and a `::HEIGHT` may sit
between the chart and the caption. With no chart right before it the text is
drawn as `'plain'`, and `--check` warns (`markdown_style/caption-no-chart`).
`'caption'` is a panel's own value only, not a `-- @markdown_style` default.
`--svg` draws the caption as plain wrapped text in the card; the HTML pages
(the `dashboard` export, a served dashboard without scripts) put it in the
chart's figure.

### Quiet tones on a card (`::MARKDOWN_TONE`)

On a teaching or reference page, explanation, worked example, good practice,
limits and formulas all look alike as plain cards. `::MARKDOWN_TONE` (#659)
gives a `'card'` box a thin coloured bar on its left edge and a small label
above the text — no tint, so it never reads as an alert:

```sql
SELECT 'A worked example: 120 orders, 9 of them late.'::MARKDOWN,
       'example'::MARKDOWN_TONE;
SELECT 'What it cannot tell you: *why* the input moved.'::MARKDOWN,
       'caution: Grenzen'::MARKDOWN_TONE;   -- your own label after the colon
```

| Tone | For | Accent |
|---|---|---|
| `neutral` | explanation | none (a plain card) |
| `info` | background, "why" | the theme's info colour (blue) |
| `example` | a worked example | violet |
| `tip` | good practice | the theme's positive colour (green) |
| `caution` | limits, "what it can't tell you" | the theme's warning colour (amber — not an alert's red) |
| `details` | formulas, technical details — folded | muted grey |

The label is the tone's name (`Info`, `Example`, `Tip`, `Caution`,
`Details`); `'tone: label'` sets your own (e.g. in German), `'tone:'` drops it.
A tone implies `'card'`, even over a `-- @markdown_style` default; next to an
explicit `'plain'`/`'callout'` style or a `::CALLOUT` it is dropped (a lint
warning, `markdown_tone/ignored`). An unknown tone is an error
(`markdown_tone/unknown`). The bar is drawn in `--svg` and the other
static/HTML output too; the label in the browser and the HTML pages. Colours
are theme tokens (`--ax-tone-info`, `--ax-tone-example`, `--ax-tone-tip`,
`--ax-tone-caution`, `--ax-tone-details`), light and dark.

`details` folds (#691): in the browser its label is a click-to-open line
(`<details>`/`<summary>`: Enter or Space opens it too) and the text stays
hidden until then, so a one-screen page keeps long formula blocks out of the
way. `'details: How MASE is computed'` makes that line say what is inside.
Printing opens every folded box; `--svg`, the HTML pages and the served
static pages always show the text.

```sql
SELECT '$$\mathrm{MASE} = \frac{\mathrm{MAE}}{\mathrm{MAE}_{\text{naive}}}$$'::MARKDOWN,
       'details: How MASE is computed'::MARKDOWN_TONE;
```

### Images (`::IMAGE`, `![alt](src)`)

A dashboard panel can show a static picture — a diagram, a logo, a screenshot
— drawn through `::IMAGE`, or a small inline figure in a `::MARKDOWN` text via
Markdown's `![alt](src)` syntax. Both accept the same sources and are never
sanitised as markup: every source is drawn through an `<img>` (browser) or an
SVG `<image href=…>` (static output), so a browser runs no script and loads
no external resource from inside an SVG used this way. **Never** inline SVG
markup into the DOM, an `<object>` or an `<iframe>`.

```sql
SELECT 'assets/concept-drift.svg' ::IMAGE,
       'assets/concept-drift-dark.svg' ::IMAGE_DARK,        -- optional; shown while the dashboard is dark
       'Concept drift: inputs look the same, the input→outcome relationship moved' ::ALT,
       'Concept drift' ::TITLE,
       'Why input checks miss it' ::LABEL,                   -- optional caption under the image
       'Drift in P(y|x) with P(x) unchanged' ::INFO;         -- optional ⓘ on the title bar
SELECT 'Below: ![decision flow from signals to actions](data:image/svg+xml;base64,PHN2Zy…)'::MARKDOWN;
```

**Accepted sources** (`src/image.rs::src_ok`; the browser's `imageSrc()` gives
the same verdict for every case):

| Source | Accepted | Notes |
|---|---|---|
| `data:image/(png\|jpeg\|jpg\|gif\|webp\|svg+xml)` | yes | followed by `;` or `,` |
| `https://…` | yes | never `http:` — mixed content on a served https page |
| a relative path | yes, if it stays inside the folder | no scheme, no leading `/` or `//`, no `\`, no `..`/`.` segment, no segment starting with `.`, no `%`; a warning unless it starts with `assets/` — the dashboards folder's `assets/` subfolder is the only one ever served/copied, see "Where `assets/` files come from" below |
| anything else | no | the alt text shows in a muted box, never a broken-image icon |

Quotes, `<`, `>`, `(`, `)`, backticks and control characters are never
allowed, the same rule as `::LOGO`/`::FAVICON` (`theme::safe_url`). A source
over 2 000 000 characters is rejected (`image/too-large`); `--check` warns
above 512 000.

**Alt text.** `::ALT` is the image's text alternative; `''::ALT` means
decorative (`alt=""`, no lint warning). Without an `::ALT`, the alt text
falls back to `::TITLE`, then `::LABEL`, then `""` — `--check` warns
(`image/no-alt`). In Markdown, `![](src)` gets the same warning
(`markdown/image-no-alt`). Alt and caption text are only ever set with
`textContent`/`setAttribute` (`::IMAGE`) or HTML-escaped (Markdown) — never
built as a string of markup.

**Sizing.** The image fills its panel's width, height following its own
aspect; `::HEIGHT n` gives it a fixed `n` px box instead, with
`object-fit: contain` (centred, not stretched). Under `::FIT` an image panel
takes a share of the row height like a chart. Several `::IMAGE` rows stack in
one card, each with its own alt (and `::LABEL` caption); `--check` warns
above 6 (`image/many`).

**Dark variant.** `::IMAGE_DARK` is the same idea as the app bar's
`logo_dark`: the browser draws both `<img>`s and shows one, keyed on the
dashboard's own dark toggle (`body.dark`), not the OS setting. Static output
(`--svg`, the CLI, `serve --dashboards`) uses the dark variant with `--dark`.
An SVG that switches itself with `prefers-color-scheme` follows the **OS**
setting inside an `<img>`, not the dashboard's own toggle.

**Static output.** `::IMAGE` becomes an SVG `<image href=… preserveAspectRatio="xMidYMid meet">`
sized from the image's own pixel size (`src/image.rs::intrinsic_size` reads
PNG/GIF/JPEG/WebP headers and an SVG's `viewBox`/`width`+`height`; a 9:16
aspect otherwise) inside the panel frame, with the title bar and caption like
any other panel — this is panel composition (`src/dashboard.rs`), not a new
SVG writer for charts (plotting stays through ggplot-rs). `data:` sources are
embedded as-is; `https:` is kept as a link (it may not show offline, and the
assistant's preview CSP blocks it — `image/https-offline`); a relative path
under `assets/` is inlined as a `data:` URI (below) — missing or refused, it
draws the alt-text placeholder. Markdown images in static output render as
their alt text in brackets (`[decision flow …]`) rather than as pixels. The
browser's ⤓ HTML export fetches and inlines any non-`data:` image so the
download works fully offline (falling back to the alt text if that fetch
fails, e.g. CORS on an `https:` source, or a non-image response).

**Where `assets/` files come from.** A relative path means a file in
`assets/` next to the dashboards, never anywhere else in that folder (which
also holds `.sql`, `policy.json`, `blueprint.json`) — a path outside `assets/`
is a `not-in-assets` warning and never resolves, on any host:

| Host | Reads `assets/` from |
|---|---|
| Locked server (`anofox_serve_dashboards`) | `GET /d/<id>/assets/<rest>` — the same login and scope as the dashboard page, `options.max_asset_bytes` (default 5 MiB); see docs/secure-serving.md |
| `--site` | copies `<folder>/assets/` to `<out>/assets/`, refusing the whole publish on a symlink or a non-image/oversized file |
| `dashboard file.sql out.html` / `--svg` | `<dir of file.sql>/assets/<rest>`, inlined as a `data:` URI |
| `serve --dashboards <dir>` (the script-free page) | `<dir>/assets/<rest>`, inlined as a `data:` URI |
| Browser builder (no server) | unchanged — resolves against the app's own URL |
| Authoring server (`serve [db]`, `anofox_serve`) | off by default (no dashboards folder to derive a root from); opt-in via `--assets <dir>` / `options.assets`, then `GET /assets/<rest>` under the same token/loopback guard as every other authoring route — see docs/secure-serving.md |

Every reader validates the same way: the resolved path must stay inside
`assets/` (symlinks included), the extension must be `png`/`jpg`/`jpeg`/
`gif`/`webp`/`svg`, and the file's own bytes must actually sniff as that kind
of image — not just its extension (`src/image.rs::read_asset`).

**Limits.** `{{col}}`/`{{col:fmt}}` is **not** filled inside a Markdown image
`src` (`markdownWithValues` strips tokens inside tags) — compute the path in
an `::IMAGE` instead (`markdown/placeholder-in-image`, a warning).

**Lint codes:** `image/bad-src`, `image/outside-folder` (errors);
`image/not-in-assets`, `image/svg-no-size`, `image/too-large`, `image/empty`,
`image/no-alt`, `image/many`, `image/missing-file` (a relative `assets/…`
that doesn't exist — only when `--check`/`--site` know the folder),
`image/https-offline` (warnings); `markdown/image-bad-src` (error);
`markdown/image-not-in-assets`, `markdown/image-svg-no-size`,
`markdown/image-too-large`, `markdown/image-no-alt`,
`markdown/placeholder-in-image` (warnings).

**Images from data are a trust boundary worth knowing about.** A `::MARKDOWN`
box built from a text column can point an `<img>` at any `https:` host —
automatically, no click — which reveals the viewer's IP to it (the CSP's
`img-src` already allows `https:` for `::LOGO`). This is the same trust
`::LINK` already carries, just without a click; avoid rendering untrusted
text as `::MARKDOWN`. See `docs/secure-serving.md`.

### Formatted tooltips

Hover tooltips show values the way the panel formats them: its `::YFORMAT`
(`'$'`, `'€'`, `'percent'` — values 0–100, `'comma'`, `' kg'`, `'CHF '` — the same keywords as
the axis) in the dashboard `::LOCALE`, with cents kept for currency
(`$1,234.50`, `1.234,50 €`). Share charts (`::BARCHART_PERCENT`,
`::BAR_STACKED_PERCENT`, a fractional `::LINECHART_PERCENT`) read as percent:
a stacked-percent segment shows its share of the bar (its `data-value` is that
share, 0–1). On stacked bars and areas, `::DATALABELS` sit in the middle of
each segment. The x value follows `::XFORMAT`, and ISO dates take the locale's date order
(`01.03.2024`) when a `::LOCALE` is set. Data labels (`::DATALABELS`) use the
same format in short form. The renderer writes these as `data-fvalue` /
`data-fx` next to the raw `data-value` / `data-x` on each mark, so any host can
show them; selection and filtering keep using the raw values.

### Combo charts, auto-refresh

A panel with **multiple measure columns** overlays them as combo layers, e.g.
`SELECT week::XAXIS, sessions::BARCHART, revenue::LINECHART`. Without a
`::CATEGORY`, several columns of the **same** chart family (e.g.
`events::LINECHART, failures::LINECHART`, or `events::BARCHART,
failures::BARCHART`) get a real legend, one entry per column, named by the
column's own header (an `AS alias` wins; a bare column name otherwise) — not
a positional `c1`, `c2`, … placeholder. A **mixed** combo (a different-kind
overlay on a base chart, e.g. `sessions::BARCHART, revenue::LINECHART`) keeps
the base in the brand colour with no legend; each overlay just takes the next
palette colour so it never disappears into the base. For a legend that also
dodges (bars side by side instead of overlaid) or needs a title other than
the column header, pivot to the long form instead: one shared measure column
plus a `::CATEGORY` naming the series.

The toolbar also has an **auto-refresh** interval.

### Themes, dark mode, branding and locale

One theme definition (`src/theme.rs`) styles the **whole dashboard**: the charts
(palette, sequential fills, dark ink), the page chrome (background, header,
cards, tabs, KPI tiles, Markdown callouts, tables, inputs, tooltips, buttons,
footer) and every output — the browser, embed / served / locked pages, the CLI's
static HTML, `dashboard --svg`, and the ⤓ HTML / PNG exports. The page reads
it as CSS custom properties (`--ax-*`, see `web/theme-base.css`); colour
literals live only in the theme.

**Charts wear the theme too.** Every chart — the role charts and the
contract plots (`anofox_plot_*`: calibration, ROC, terms, …) — draws its text
in the theme's font family, tick labels and axis titles in the muted ink,
legend entries a step stronger, titles in the heading colour, a hairline grid a
step quieter than the card border (no minor grid), and leaves the background
transparent so the card shows through. Bars, slices and heatmap cells part with
a thin card-coloured gap instead of a dark outline; reference lines
(`::REFLINE`/`::XLINE`) are quiet dashed rules; box/violin bodies and gauge
tracks are a tint of the brand on the card. Series colours follow the palette.
All of it follows the light/dark variant and a custom theme's `colors`.

```sql
SELECT 'dark'::THEME;                                    -- a built-in
SELECT 'acme+dark'::THEME;                               -- stacked: acme, with a dark variant
SELECT content::THEME FROM read_text('themes/acme.json');-- a corporate theme from a file
SELECT to_json(t)::THEME FROM themes t WHERE name = 'acme'; -- …or from a table
SELECT 'de-DE'::LOCALE;                                  -- numbers and dates
SELECT 'Sales cockpit'::DASHBOARD_TITLE;                 -- header, browser tab, export title
SELECT 'ACME Analytics'::BRAND;                          -- small brand line above the title
SELECT 'assets/acme.svg'::LOGO;                          -- header logo (relative, https: or data:image/)
SELECT 'assets/favicon.svg'::FAVICON;
SELECT 'Revenue'::SECTION;                               -- a section heading (always a new row)
```

**Built-in themes** (`dashboard --themes`):

| Theme | Look |
|-------|------|
| `light` (= `default`, `datazoo`) | the default graphite look: cool grey page, white cards, electric-blue accent, flat; follows the viewer's light/dark preference |
| `dark` | always dark: dark cards, light ink, lifted palette |
| `print` | white page, black ink, no shadows or gradients |
| `colorblind` | the light look with the Okabe-Ito palette |
| `high-contrast` | maximum text contrast, solid accents |

**Which variant a viewer sees.** With no `::THEME` (or `'x+dark'`) the dashboard
*follows the viewer*: the OS `prefers-color-scheme`, the 🌙 toggle (remembered in
the browser) or `?theme=dark` / `?theme=light` in the URL. `'light'`, `'dark'`,
`'print'` alone pin the variant. Charts are re-rendered with the dark theme (not
just recoloured cards), and a PNG / HTML export keeps the variant on screen. The
builder chrome (sidebar, editor, toolbar) stays neutral and product-branded; only
the dashboard follows the dashboard theme.

**In the builder's Edit mode** the name and logo are edited in the dashboard
header: click the name (or Tab to it and press Enter), type, and press Enter or
leave the field to save (Esc cancels); **Add logo** / **Change logo** takes an
image URL (the `::LOGO` rules above) or an uploaded PNG, JPEG, GIF, WebP or SVG
of up to 256 KB, embedded as a `data:image/` URL. Both write plain SQL: the
`::DASHBOARD_TITLE` / `::LOGO` statement is updated, or added at the end of the
`-- ── Look` section (else after the leading comments); an empty name or
**Remove logo** deletes it. So the change travels with Share links, the HTML
export and `dashboard` / `serve`. A computed title (`name::DASHBOARD_TITLE FROM
…`) is replaced by the typed text. View mode shows the header as it is.

**A theme file** is JSON; every key is optional and merges over `extends`
(default `light`):

```json
{"name": "acme", "extends": "light",
 "palette": ["#0b5fff", "#e11d48", "#16a34a", "#f59e0b"],
 "sequential": ["#eef3ff", "#0b5fff"],
 "colors": {"bg": "#f4f6f9", "bg2": "#f4f6f9", "card": "#ffffff", "ink": "#1f2937",
            "muted": "#6b7280", "line": "#e5e7eb", "heading": "#111827",
            "accent": "#0b5fff", "accent2": "#3b82f6",
            "positive": "#16a34a", "negative": "#dc2626", "warning": "#d97706"},
 "font": {"family": "Inter, system-ui, sans-serif", "display": "Inter, sans-serif"},
 "radius": 10,
 "chrome": {"gradient": false, "background": "plain", "kpi_value": "accent",
            "table_zebra": true, "shadow": "sm", "negatives": "parens", "zero": "dash"},
 "brand": {"name": "ACME Analytics", "logo": "logo.svg", "logo_dark": "logo-dark.svg",
           "favicon": "favicon.svg", "title_suffix": " · ACME"},
 "locale": "de-DE",
 "dark": {"colors": {"bg": "#0f1729", "card": "#162033", "ink": "#e5e7eb"}}}
```

`chrome.negatives` sets how tables show negative numbers — `"minus"` (−5, the
default), `"red"` (−5 in the negative colour) or `"parens"` (accounting: (5)) —
and `chrome.zero` shows zero as a muted `"muted"` 0 (default) or a `"dash"` (–).
Zebra rows (`table_zebra`) are left out of a table that draws in-cell bars.

Theme JSON is data: colours must be `#rgb`/`#rrggbb`, fonts plain font-family
lists, logos relative paths, `https:` or `data:image/` URIs; anything else (and
unknown keys) is an error. `?theme=` only switches light/dark — a URL never
injects a theme. See `examples/theming/` (`acme.json`, `branded.sql`,
`de-locale.sql`).

**Locale** (`::LOCALE`, or the theme's `locale`; `en-US` by default): `en-US`,
`en-GB`, `en-IE`, `de-DE`, `fr-FR` (other regions fall back to their
language; English with a euro-area region, e.g. `en-DE`, `en-NL` or `en-EU`,
reads as `en-IE`). It formats KPI tiles, table cells (numbers and ISO dates)
and chart axes / legends / month names identically in the browser and the
CLI:

| | en-US | de-DE | fr-FR |
|---|---|---|---|
| `::METRIC` 1234567.5 | 1,234,567.5 | 1.234.567,5 | 1 234 567,5 |
| `::MONEY` 12400 | $12,400 | 12.400 € | 12 400 € |
| `::PERCENT` 12.5 | 12.5% | 12,5 % | 12,5 % |
| `::COMPACT` 1234567 | 1.2M | 1,2 Mio. | 1,2 M |
| date 2026-10-15 (tables) | 2026-10-15 (unchanged) | 15.10.2026 | 15/10/2026 |
| axis | `1,000` · `Oct 2026` | `1.000` · `Okt. 2026` | `1 000` · `oct. 2026` |

`en-IE` is English with the euro: `€12,400`, `12.5%`, `1,234,567.5`, dates
`15/10/2026`, the choice for English labels with euro amounts.

`?locale=de-DE` overrides it for one viewer (not in served/locked mode).

**Everywhere else:** `dashboard file.sql out.html --theme dark` /
`dashboard --svg file.sql out.svg [--theme T] [--dark]`,
`serve [db] --theme T`, `serve --dashboards dir --theme T`,
`anofox_serve_dashboards(dir, port, '{"theme":"dark"}')` (resolved and frozen
when the server starts). A theme argument is a built-in, a stack or a `.json`
file; the dashboard's own `::THEME` wins. The SQL macros take
`theme := …` and `locale := …`, defaulting to the session variables:

```sql
SET VARIABLE anofox_theme = 'dark';      -- every anofox_plot_* / anofox_xy … call
SET VARIABLE anofox_locale = 'de-DE';
SELECT anofox_plot_terms(t, theme := 'colorblind') FROM coefs t;
```

`dashboard --theme-css <T> [--dark]` prints a theme's CSS tokens (to style your
own page around exported SVGs).

#### Data freshness (`::ASOF`)

Viewers of a locked dashboard or an exported page can't tell how current the
data is. `::ASOF` puts a chip in the dashboard header — **"Data as of 14:02
(2 h ago)"** — and an optional `'stale=…'` column turns it **amber** (with a ⚠)
once the data is older than that:

```sql
SELECT max(loaded_at)::ASOF, 'stale=24h' FROM etl_log;   -- s, m, h, d or w
```

The value is a `TIMESTAMP`, `TIMESTAMPTZ` or `DATE`; a timestamp without a time
zone is read as **UTC** (prefer `TIMESTAMPTZ`, e.g. `now()` or a `loaded_at`
written with one). A `DATE` (or `'YYYY-MM-DD'` text) is a calendar day: the
chip shows the date alone, with no time and no time-zone shift ("Data as of
01.01.2026 (291 d ago)", "today" on the day). The chip shows the time in the
viewer's zone and the dashboard locale (the date is added when it isn't today)
and keeps its relative age current while the page is open. It is in every
output: the browser, a locked server, the static HTML of
`dashboard file.sql out.html` /
`serve --dashboards` (rendered at build time, then refreshed by the page's
script against the viewer's clock) and headless SVG (`dashboard --svg`,
`render_dashboard_svg`: rendered at render time, in UTC). `--check` warns when
the value isn't a date or timestamp (`asof/unparseable`) or the threshold
can't be read (`asof/bad-stale`). The last `::ASOF` wins.

### Accessibility

Charts are usable without a mouse and without sight of the screen:

- **Keyboard.** Each chart with data marks is one Tab stop (a visible focus
  ring). Arrow keys walk the marks — ←/→ along a series, ↑/↓ to the
  neighbouring series, Home/End — and the focused mark shows its tooltip.
  Enter (or Space) acts like a click: cross-filter, `::SELECT_AS`, `::DRILL`,
  `::LINK`; after the re-run the focus returns to the same mark. Esc goes back
  to the chart, Tab moves on.
- **Screen readers.** Every chart SVG carries a generated one-sentence summary
  (`aria-label` and `<desc>`): kind, title, series, the x range and the
  extremes — *"Bar chart: Sessions by channel; 3 series (api, app, web);
  4 points from W1 to W4; highest 48 (app, W4), lowest 9 (api, W3)."* Each
  mark is labelled with its tooltip text.
- **Charts as tables.** Swaps every chart for its data table. Reached from the
  toolbar's **⋯ → ▦ Charts as tables** menu item (also in embed and served
  views; `?tables=1`/`?tables=0` turns it on/off explicitly, the choice is
  remembered across dashboards). The *▦ Charts as tables* bar at the top of
  the dashboard itself is off by default and shown only when the author opts
  a dashboard in with `-- @astables on` (see *Dashboard header comments*) —
  a viewer who never saw a bar can still reach the feature from the menu. An
  installation can default the viewer's choice on (`charts_as_tables: true`
  in `app-config.json` or the serving process's options, `--app-charts-as-tables`
  for `serve`/`anofox_serve`; #403) without forcing the bar itself on — an
  explicit `?tables=`/remembered choice always wins over the default,
  whichever way it was set.
- **Reduced motion.** With the system's *reduce motion* setting there are no
  entrance animations, line draw-ins, shimmers or transitions.
- Tables keep `aria-sort` and keyboard row navigation; the `colorblind` and
  `high-contrast` themes cover colour.

`scripts/a11y-e2e.mjs` checks this in a real browser (Tab into a chart, arrows,
Enter, Esc, the summaries, the table switch; axe-core with `--axe <axe.min.js>`).

### Export & share

- Every chart/table panel has a hover **⤓** button — charts download as **PNG**,
  tables as **CSV**.
- **Share** copies a link with the whole dashboard SQL encoded in the URL hash
  (no server) plus the open tab and the viewer state (`?v.*`, see *Viewer
  state in the URL*) — open it to reproduce exactly that view.
- **⤓ HTML** downloads the current dashboard as a standalone, self-contained HTML
  file (interactive hover + tabs, no server). A `::VIEW` switcher works in the
  export too (click or arrow keys, offline) and the file carries **every
  alternative that isn't `-- @requires`-hidden** — not the default view only,
  the same as it already carries every tab's rows. That is the viewer's own
  data (they could already click through to each alternative on the live
  page); only a hidden alternative is left out of the export entirely.

### Dashboard header comments

A dashboard file can start with `-- @…` comment lines. They are not SQL; the
builder and the servers read them (`parse_dashboard_meta`, one parser, shared
with the browser through wasm):

```sql skip
-- @title  Weekly sales by region
-- @nav    Weekly sales
-- @group  Sales / EMEA
-- @tags   weekly, kpi, emea
-- @description Orders and revenue per region, week by week.
-- @owner  Sales controlling
-- @certified 2026-10-15
-- @refresh 300
-- @astables on
```

| Line | Meaning |
|---|---|
| `-- @title <name>` | The dashboard's name: the sidebar entry of an example, the page title and nav entry on a server (without it, the file id). Name it as a plain business question. |
| `-- @nav <short label>` | A compact nav bar / start-page card label, shown in place of `@title` there when set (`@title` still stays the page's own heading); without it, the nav bar and cards show `@title`. Use it when the business-question title would wrap in a nav bar. |
| `-- @group <name>` | Where it sits: one group per dashboard, a ` / ` adds one level (`Sales / EMEA`, drawn as a sub-heading; deeper paths are kept as written and drawn flat). No `@group`, or an empty one, means ungrouped. |
| `-- @tags a, b` | Free labels for finding it: trimmed, lower-cased, without duplicates, at most 10 (several `@tags` lines add up). |
| `-- @description <text>` | One line on what the dashboard answers: the line under its title on a server's landing page, and part of the tooltip in the builder's sidebar and the served nav. Several `@description` lines are joined. |
| `-- @owner <text>` | Who looks after it — a name, a team, an address. Shown as a small line under the name in the builder's sidebar, in the tooltip of a served nav link and on the `/` list. |
| `-- @certified <YYYY-MM-DD>` | The day it was last checked. Shows a **✓ certified** badge (sidebar, served nav, `/` list) and lets viewers filter on it (the sidebar's *✓ certified* chip). A date that isn't `YYYY-MM-DD` counts as not certified. |
| `-- @order N` | Where it sits, ascending, among its siblings (same group, or ungrouped) in the served nav bar and the `/` start page's card grid — the server's default is alphabetical by file name, which ignores `blueprint.json`'s own `dashboards[]` order. `N` is a plain integer (ties, and dashboards without `@order`, keep sorting by id afterwards). |
| `-- @refresh <s>` | Reload the page every *s* seconds (served dashboards). |
| `-- @astables on` \| `off` | Shows the "▦ Charts as tables" bar on this dashboard for every viewer — off (no bar) by default, or with `off`. A viewer can still turn the feature itself on or off from the toolbar's **⋯ → Charts as tables** menu item whether or not the bar is shown; their choice is remembered across dashboards. |
| `-- @markdown_style <plain\|card\|callout>` | The dashboard's own default `::MARKDOWN_STYLE` (#536) — used by every `::MARKDOWN` panel that sets no `::MARKDOWN_STYLE` of its own and carries no `::CALLOUT` (which always draws `'callout'`). Without it, every such panel falls back to `'callout'`, today's tinted box. A panel's own `::MARKDOWN_STYLE` overrides this. |
| `-- @load <ext>` · `-- @param name [a, b] = a` · `-- @vars a, b` | For the servers, see [`secure-serving.md`](secure-serving.md). |
| `-- @requires <path>` | Directly above one statement (above a `::TAB` / `::SUBTAB`: the whole tab, hidden when not met): see *Loading, empty, error and "not available" panels*. |
| `-- @shared <name>, …` | Directly above one `::ROWSELECT` / `::SELECT_AS` / `::DRILL` / input statement: documents that *this* directive deliberately reuses `<name>` with another one, so `dashboard --check --strict` skips `interaction/rowselect-shared-name` / `interaction/name-clash` for it — every other clash still is flagged. See *Selecting table rows* below. |
| `-- @lint_allow <code>, …` | Directly above one statement: a deliberate exception to a `design/*` advisory (#639), e.g. `-- @lint_allow design/many-series` on a chart with 12 conventional colour series. `--check` still lists it as a `lint/allowed` note, so it stays visible in review, but `--strict` passes. Only `design/*` codes: anything else is flagged (`lint/allow-not-design`), as is a code the statement doesn't trigger (`lint/allow-unused`). On the first statement it also covers `design/too-many-panels`. |
| `-- @name <name>` | Directly above any statement that draws something — a panel, an input, a `::GROUP` box, a `::TAB`/`::SUBTAB` button, a `::VIEW` switcher, or a `::LABEL`/`::SECTION` heading: tags its element `data-dp-name="<name>"`, a stable anchor for e2e selectors and a guided tour's target (`-- @tour`, below). `name` matches `^[A-Za-z_][A-Za-z0-9_-]{0,63}$`; names must be unique on one dashboard (`name/duplicate`); `page` is reserved (`name/reserved`); a bad name is `name/bad`; one on a statement that draws nothing warns (`name/not-drawn`). See *Interactivity* below. |
| `-- @tour <id> [title]` + step lines | A guided tour over the dashboard's own panels: see *Guided tours* below. |
| `-- @tour_autostart <id>` | Start that tour once per browser, on a viewer's first visit: see *Guided tours* below. |

`dashboard --check` warns about an empty `@nav` (`header/empty-nav`), one
longer than 24 characters (`header/long-nav`), an empty `@group`
(`header/empty-group`), one longer than 80 characters (`header/long-group`),
more than 10 tags (`header/too-many-tags`), an empty `@owner`
(`header/empty-owner`), a `@certified` that isn't a `YYYY-MM-DD` date
(`header/bad-certified`), an `@order` that isn't a plain integer
(`header/bad-order`), a `@description` over 160 characters
(`header/long-description`), an `@astables` that isn't `on` or `off`
(`header/bad-astables`) and a `@markdown_style` that isn't `plain`, `card`
or `callout` (`header/bad-markdown-style`), reported as statement 0.
`dashboard --check
<folder>` lints every `.sql` of a
served folder and adds what only the folder shows: two dashboards with the
same title (`folder/duplicate-title`, ignoring case), a file name the
servers skip (`folder/bad-id`), and a setup statement that materialises a
table (`CREATE [TEMP] TABLE … AS`, `folder/setup-table`). The locked server
runs a panel's setup on every connection that serves it: with warm
connections (`warm`, #388) once per kept connection and again whenever a
variable the setup reads changes, without them for every panel request
(#387: 17.6 s against 0.76 s per click with a view, on a 30-panel, 20M-row
dashboard). Each kept connection also holds the table in memory; a
`CREATE VIEW` costs only what each panel reads.

### Layout (from SQL)

The grid is a **12-column bootstrap grid**; panel widths are spans out of 12.
Directives (browser builder / `serve`; the CLI skips them):

| Directive | Effect |
|-----------|--------|
| `SELECT n::COL;` (`::SPAN`, `::WIDTH`) | the **next** panel's width — `n` of 12 (`12`=full, `6`=half, `4`=third); a statement of its own: written inside a chart's SELECT it turns that statement into the directive and the chart isn't drawn (`layout/inline-layout-role`) |
| `SELECT n::COLUMNS;` | default panels per row (each unspecified panel spans `12/n`) |
| `SELECT 'Title'::GROUP;` … `SELECT 1::ENDGROUP;` | wrap the enclosed controls/charts in one box (a flex row) |
| `SELECT 'Title'::FIGURE, '(a / b) \| c'::LAYOUT;` … `SELECT 1::ENDFIGURE;` | compose the enclosed charts into **one** picture — aligned plot areas, a shared x axis, one legend, panel tags; see *Figures* below |
| `SELECT 'Name'::TAB;` | start a tab — following panels live under it (panels before the first `::TAB` form a fixed header) |
| `SELECT 'Name'::SUBTAB;` | start a nested tab inside the current `::TAB` |
| `SELECT 'Name' AS id ::VIEW;` | start (or continue) a group of alternative views of one panel — a segmented switcher; see *Alternative views of one panel* below |
| `SELECT 1::ENDVIEW;` | close a `::VIEW` group (optional when a `::TAB`/`::SUBTAB`/`::SECTION` or the end of the script follows) |
| `SELECT 'Name' AS id ::GUIDE_STEP, cond ::GATE, 'msg' ::GATE_MESSAGE;` | open the next step of a guided sequence (Back/Next, "Step n of N") — see *Guided steps* below |
| `SELECT 'fit'::FIT;` | the `::TAB` / `::SUBTAB` / `::VIEW` it is in fits the window, no page scroll — see *Fit to screen* below |
| `SELECT cond::SHOW_IF;` | the **next** panel (or `::GROUP` box) shows only while `cond` is `TRUE` — see *Show a panel only when …* below |
| KPIs in a `::GROUP` box | metrics inside a `::GROUP`…`::ENDGROUP` render as a compact KPI strip (dividers) instead of full cards |
| `SELECT day::XAXIS, 'Name'::TRACK …;` right after a chart | a thin strip **under** that chart, in its panel, on the same x axis (see [strips](#fans-shaded-regions-and-strips-under-a-chart)) |

```sql
SELECT 'Filters'::GROUP;                 -- two dropdowns together in one box
SELECT DISTINCT region::DROPDOWN  FROM sessions ORDER BY region;
SELECT DISTINCT channel::DROPDOWN FROM sessions ORDER BY channel;
SELECT 1::ENDGROUP;

SELECT 12::COL;                          -- full-width chart
SELECT week::XAXIS, channel::CATEGORY, sum(n)::BARCHART_STACKED FROM sessions …;

SELECT 8::COL;   SELECT … ::LINECHART …; -- 8/12, beside…
SELECT 4::COL;   SELECT … ::BARCHART  …; -- …a 4/12 chart (8+4 = one row)
```

Panels wrap to a new row when their spans exceed 12, and collapse to full-width
on narrow screens.

A `::GROUP` box also holds charts, so you can place a dropdown *beside* a graph.
See the **"Layout & filters"** sample.

### Figures: several charts as one picture (`::FIGURE`)

An explanatory figure often combines plots with aligned axes — a histogram of
`x` directly above the scatter of `x` vs `y`, a forecast over its residuals, a
before/after pair — like R's *patchwork*. As separate panels they fall apart:
separate cards, no shared axis, legends repeated. `::FIGURE` … `::ENDFIGURE`
(#661) composes the charts between the markers into one picture
(ggplot-rs's `PlotGrid`):

```sql
SELECT 'Data drift: the inputs move, the rule holds' ::FIGURE,
       '(hist / scatter) | rule' ::LAYOUT,      -- | side by side, / stacked
       '1,4' ::HEIGHTS, '2,1' ::WIDTHS,
       'x' ::AXES_COLLECT,                        -- hist and scatter share one x axis
       true ::COLLECT_LEGENDS,                    -- reference / now once, not per cell
       'A' ::TAG_LEVELS;                          -- cells tagged A, B, C
-- @name hist
SELECT x ::HISTOGRAM, period ::CATEGORY, 'share' ::HIST_NORM FROM drift;
-- @name scatter
SELECT x ::XAXIS, y ::SCATTER, period ::CATEGORY FROM drift;
-- @name rule
SELECT x ::XAXIS, fitted ::LINECHART FROM rule ORDER BY x;
SELECT 1 ::ENDFIGURE;
```

The `::FIGURE` value is the card's title; every other column of that
statement is an option:

| Option | Effect |
|---|---|
| `::LAYOUT` | How the cells are arranged: `\|` side by side, `/` stacked (binds tighter, as in patchwork: `a \| b / c` = `a \| (b / c)`), `( )` groups, `_` an empty cell. A cell is its `-- @name` or its position (`1`, `2`, …). Without `::LAYOUT` the cells fill a grid in order. |
| `::FIGURE_NCOL` | Without `::LAYOUT`: the grid's column count (default: near-square). |
| `::WIDTHS` / `::HEIGHTS` | Relative widths / heights, `'1,2'` — applied to every side-by-side (`\|`) / stacked (`/`) group with that many cells. In the example, `'1,4'` makes the scatter four times as tall as the histogram above it. |
| `::AXES_COLLECT` | `'x'`: charts stacked in a column share one x domain and draw it once, at the bottom (needs a numeric/date x — a `::HISTOGRAM` or a scatter, not categorical bars); `'align'`: line up the plot areas only; `'none'` (default). |
| `::COLLECT_LEGENDS` | `true`: identical legends are drawn once, beside the figure. |
| `::TAG_LEVELS` | `'A'`, `'a'`, `'1'`, `'i'` or `'I'`: tag the cells in reading order. |

The figure takes the full row (`::COL` before it narrows it) and is 450 px
tall (`::HEIGHT` before it changes that). Each cell is an ordinary chart
statement: its own roles, `::TITLE`, `::CATEGORY`, … — a cell needs no
`::TITLE` (`design/untitled-chart` skips it). Hovering a mark shows its own
cell's values, and the x-crosshair works per cell: over a line or area cell
it reads that cell's column (#695); a scatter, histogram or bar cell keeps its
marks' own tooltips. Each cell's `<svg>` content carries `data-cell` (its
statement order) and `data-dp-name` (its `-- @name`). A cell's own `::LINK`
column follows from its marks, as on a single chart; a click on a cell without
one cross-filters as usual. The toolbox's data view (and "Charts as tables")
shows a table per cell, captioned by its `-- @name`. `::SELECT_AS` / `::DRILL`
and zoom stay off on a figure for now. The
browser, `--svg` / `--site`, the HTML export and the served page all draw the
same picture.

`--check` reports: a `::LAYOUT` name that is no cell (`figure/unknown-cell`),
a cell placed twice (`figure/duplicate-cell`), `::WIDTHS`/`::HEIGHTS` that fit
no group (`figure/bad-weights`), an unknown option value (`figure/bad-option`),
a cell that is not a chart — a table, KPI, text, map or `::TRACK`
(`figure/not-a-chart`), a missing `::ENDFIGURE` (`figure/unclosed`), a
`::FIGURE` inside another (`figure/nested`) — all errors — and, as warnings, a
cell `::LAYOUT` leaves out (`figure/unplaced-cell`), an empty figure and a
stray `::ENDFIGURE`.

Not yet: tables, KPIs or Markdown as cells (patchwork's `wrap_elements`), and
on a phone-width screen the collected legend still sits to the right
(sipemu/ggplot-rs#363).

### Fit to screen (`::FIT`)

A wall display or a one-screen overview should not scroll. `SELECT 'fit'::FIT;`
right after a `::TAB` makes **that** tab fill the browser window while it is
the one shown, with no page scroll; other tabs scroll as usual. With no tabs at
all, a `::FIT` anywhere fits the whole dashboard (a `::FIT` before the first
`::TAB` of a tabbed dashboard fits every tab — `layout/fit-ambiguous` warns).

In a `::SUBTAB` or a `::VIEW` alternative, `::FIT` fits **only that pane's own
panels**: they share the window height under the app bar and the sub-tab /
view bar. The parent tab's panels above the bar keep their normal height and
are not counted; the page may scroll to bring the fitted sub-tab / view into
view, and once there all of its panels are on screen.

```sql
SELECT 'Overview'::TAB;
SELECT 'fit'::FIT;                       -- this tab fills the window, no page scroll
SELECT count(*)::METRIC, 'Orders'::LABEL FROM orders;
SELECT sum(amount)::MONEY, 'Revenue'::LABEL FROM orders;
SELECT region::XAXIS, sum(amount)::BARCHART FROM orders GROUP BY ALL;
SELECT 360::HEIGHT;
SELECT day::XAXIS, sum(amount)::LINECHART FROM orders GROUP BY ALL;

SELECT 'Details'::TAB;                   -- no ::FIT: scrolls as before
SELECT * ::TABLE FROM orders;
```

- The height under the app bar and the tab bar is measured, and the rows of
  panels share it **in the ratio of their heights** (`::HEIGHT`, else the
  default) — a `360::HEIGHT` row stays taller than a default one. A
  **KPI-only row** (a `::GROUP` of `::METRIC`s, or a row of bare KPI tiles)
  is the exception: it keeps its own **natural height** instead — its
  measured content, at least ~96 px (`'compact'::KPI_SIZE` tiles, ~36 px) —
  and is left out of the share; only chart / table rows divide the rest.
  `::HEIGHT` right before the `::GROUP` opts it back into sharing, exactly
  like a chart row's (#418).
- Charts are **drawn again at their new size** (text and lines stay their real
  size, nothing is stretched); a resize, a rotation, entering full screen and
  switching tabs re-fit (cross-filtering keeps the fitted sizes).
- Tables, `::PAGED` tables and text boxes keep their content and **scroll
  inside their panel**; KPI tiles keep at least ~96 px (`'compact'::KPI_SIZE`
  tiles, ~36 px) — see the KPI-only row note above.
- If a panel would get under ~120 px the tab scrolls as usual and a small
  notice says so — `dashboard --check` warns about this ahead of time
  (`layout/fit-too-many`, judged at 1366×768; for a sub-tab / view only its own panels under its own bar), and about a `::FIT` whose
  scope is unclear (`layout/fit-ambiguous`: before the first `::TAB` of a
  tabbed dashboard, twice for one tab, or cast beside a panel's columns).
- **URL override** (kiosk screens): `?fit=1` fits the shown tab even without
  `::FIT`; `?fit=0` never fits.
- Phones (narrower than 600 px) ignore `::FIT` and `?fit=1`: the usual single
  scrolling column.
- **Downloaded HTML** (⤓ HTML): the file fits itself the same way on load, on
  resize and on a tab switch. It has no chart engine, so instead of drawing a
  chart again it **scales the already drawn SVG** to its fitted panel (its
  `viewBox` with a new height: crisp and undistorted, centred where the shape
  differs; text and lines grow or shrink with the chart rather than keeping
  their size).
- **Pages without the app** (`dashboard <file.sql> out.html`, the script-free
  page of `serve --dashboards`): CSS only, best-effort. A `::FIT` that applies
  to the whole dashboard (no `::TAB` / `::SUBTAB`, not inside a `::VIEW`) makes
  the page the window tall, with the panels in **equal rows** (not in their
  `::HEIGHT` ratio) of at least 120 px, each SVG scaled into its panel. With
  more rows than fit, the page scrolls. These pages show every tab on one
  page, so a tab's, sub-tab's or view's `::FIT` is ignored there. `dashboard
  --svg` draws one fixed-size image: `::FIT` has no effect.

### Show a panel only when … (`::SHOW_IF`)

`SELECT cond::SHOW_IF;` is a layout directive, like `::SPAN`/`::EMPTY`: a
statement on its own, applying to whatever comes right after it. Its target
shows only while `cond` evaluates to the BOOLEAN `TRUE` — `FALSE`, `NULL`, no
row, a non-boolean value or a query error all hide it (fail closed). It is
re-checked on every run: a full run, an input change or a cross-filter.

A common pattern is a quiz-style dropdown with an empty first option (a
`::DROPDOWN` defaults to its **first** option, so `getvariable('q1') IS NOT
NULL` is always true — check against `''`/`<> ''` instead):

```sql
SELECT v AS q1 ::DROPDOWN, l ::OPTION_LABEL, 'Which check sees concept drift?' ::LABEL
FROM (VALUES (1, '', '— choose —'), (2, 'input drift', NULL), (3, 'measured error', NULL)) t(o, v, l)
ORDER BY o;

SELECT getvariable('q1') <> '' ::SHOW_IF;               -- only after an answer
SELECT CASE getvariable('q1') WHEN 'measured error' THEN '**Right.**'
            ELSE 'Not quite — try again.' END ::MARKDOWN;
```

The target is the first statement after the `::SHOW_IF` that is not setup,
not another stacked `::SHOW_IF` (stacked conditions AND together), and not a
next-panel modifier (`::SPAN`/`::COL`, `::HEIGHT`, `::EMPTY`, `::ROWSELECT`, a
table option, a lone `::THRESHOLDS`, `::SELECT_AS`, `::DRILL`) — those travel
with the target, so a `::COL` right before a hidden panel never leaks onto
the next one. A **`::GROUP` target** hides the whole box, including its
`::ENDGROUP`:

```sql
SELECT getvariable('view') = 'detail' ::SHOW_IF;        -- a whole box
SELECT 'Detail'::GROUP;
SELECT …;
SELECT 1::ENDGROUP;
```

A hidden panel gives up its grid slot (no card; the row reflows) rather than
leaving a blank one. A hidden input draws no control, but its variable keeps
its value — a learner's answer survives the control being hidden.

Not every statement can be a target: `::TAB`, `::SUBTAB`, `::VIEW`,
`::ENDVIEW`, `::ENDGROUP`, `::FIT`, `::COLUMNS` and chrome directives always
show (`dashboard --check` reports `show_if/bad-target`); use `-- @requires`
to hide those instead. Nothing after a `::SHOW_IF` at all is
`show_if/no-target`. `--check` also flags a condition beside other roles
(`show_if/not-alone`, ignored — the panel shows), a non-boolean or no-row
result under the defaults used while checking (`show_if/not-boolean` /
`show_if/no-row`), a `getvariable()` naming no variable this dashboard sets
(`show_if/unknown-variable`) and a condition that reads no variable at all
(`show_if/constant`).

**`::SHOW_IF` is not access control.** Unlike a hidden `-- @requires` tab
(which the server also refuses, answering "Not available" to a direct
request), `::SHOW_IF` only hides a panel on the page — in served mode the
hidden statement's own query can still be requested through `/api/panel`
(see `docs/secure-serving.md`). Static output (`--svg`, the CLI's HTML, the
script-free page of `serve --dashboards`) evaluates every condition with the
defaults in scope at render time, same as the browser's first load.

### Alternative views of one panel (`::VIEW`)

`::VIEW` is a statement of its own, like `::SUBTAB`: it opens one alternative
of a segmented switcher that lets a viewer flip between a few ways of looking
at the same data (Lines / Bars / Table, say) without leaving the layout:

```sql
SELECT 'Lines' AS yoy ::VIEW;      -- opens group "yoy", view 0 (the default)
SELECT 12::COL; SELECT 420::HEIGHT;
SELECT week::XAXIS, revenue::LINECHART, 'Revenue'::TITLE FROM sales GROUP BY ALL ORDER BY 1;
SELECT 'Bars' AS yoy ::VIEW;       -- the next alternative (repeating the alias is optional)
SELECT week::XAXIS, revenue::BARCHART, 'Revenue'::TITLE FROM sales GROUP BY ALL ORDER BY 1;
SELECT 'Table' ::VIEW;
SELECT 'none'::TOTALS;
SELECT week::TABLE, revenue::MONEY FROM sales GROUP BY ALL ORDER BY 1;
SELECT 1::ENDVIEW;                 -- optional when a ::TAB/::SUBTAB/::SECTION or the end of the script follows
```

- **Grammar.** A `::VIEW` statement opens a view section; it runs until the
  next `::VIEW` (the next alternative, same group) or a closer. A run of view
  sections with no closer between them is one group. Views do not nest — put
  `::ENDVIEW` between two groups that follow each other directly.
- **Closers**: `::ENDVIEW`; `::TAB` / `::SUBTAB`; `::SECTION`; an `::ENDGROUP`
  closing a `::GROUP` opened before the group; end of script.
- **Label**: the marker's value is the button text and should be a string
  literal (a computed label still works but gets a lint warning
  `view/computed-label`).
- **Group id** (used in the URL, `?view.<id>=<label>`): the alias of the
  first marker that has one (`AS yoy` above; must match
  `^[A-Za-z_][A-Za-z0-9_]{0,63}$`), else the group's 1-based position in the
  script (`?view.2=Bars`).
- **Default view**: the first alternative that is shown (not hidden by a
  `-- @requires` on its `::VIEW` marker). To make another alternative the
  default, reorder the views — there is no default marker in v1.
- **Width**: a `::COL` pending when the group opens sets the whole group's
  width; otherwise the group spans the full row (12). Inside a view, panels
  lay out on the group's own 12-column grid, and a panel with no `::COL`
  fills the group.
- **Shared controls**: inputs and other panels placed above the first
  `::VIEW` of a group are shared by every view in it; an input placed inside
  a view still works, but its control only shows while that view is open
  (the variable itself stays global — lint note `view/input-inside`).
- **Lazy**: a view that isn't the one showing is not queried — there is no
  idle prefetch of the other alternatives. Switching views re-runs only the
  queries of the view being opened.
- **Static output and the script-free served page** (`--svg`, the CLI
  `dashboard file.sql out.html`, a dashboard served with no `.sql` on the
  page) draw the **default view only** — the other alternatives are never
  queried, and `?view.*` is ignored on the script-free page (it reads only
  declared `@param`s).
- **Accessibility**: the switcher is a `tablist` of `tab`s, one per
  alternative, each owning a `tabpanel`; arrow keys move focus between the
  buttons, Enter/Space opens the focused one, matching the `::TAB` /
  `::SUBTAB` pattern.

`dashboard --check` reports `view/lone` (warning: a group with a single
alternative), `view/duplicate-label` and `view/empty-label` (error),
`view/computed-label` (warning), `view/mixed-names` (warning: different
aliases used in one group), `view/duplicate-name` (error: two groups share
one id), `view/bad-name` (error: the alias is not a plain identifier),
`view/empty` (warning: an alternative with no rendering statement),
`view/open-ended` (warning: no `::ENDVIEW`, and the last view holds more
panels than any other), `view/unbalanced-group` (error: a `::GROUP` opened
inside a view was not closed inside it), `view/stray-end` (warning: an
`::ENDVIEW` with no open group), `view/input-inside` (note) and
`view/unnamed` (note: 2+ groups in the script, one without an alias).
`optional/missing` also covers a `-- @requires`-hidden alternative ("this
::VIEW is hidden"), and `design/too-many-panels` counts only the shown
alternative of each group.

### Guided steps (`::GUIDE_STEP`)

`::GUIDE_STEP` (aliases `::TUTORIAL_STEP`, `::WIZARD_STEP`) turns a run of
statements into a guided sequence — Back/Next, a progress strip ("Step 2 of
5"), and an optional gate that holds Next until a condition is met. It's for
onboarding and tutorials, not wizards that submit at the end (there is no
submit here — see `-- @requires`/`::SHOW_IF` for that).

```sql
SELECT 'The three doors' AS lesson ::GUIDE_STEP;
SELECT '…intro text…' ::MARKDOWN;

SELECT 'Drift sandbox' ::GUIDE_STEP,
       getvariable('q2') = 'measured error' ::GATE,
       'Answer question 2 correctly to go on' ::GATE_MESSAGE;
SELECT v AS q2 ::DROPDOWN, l ::OPTION_LABEL FROM
  (VALUES (1, '', '— choose —'), (2, 'input drift', NULL), (3, 'measured error', NULL)) t(o, v, l)
  ORDER BY o;

SELECT 'Wrap-up' ::GUIDE_STEP;
SELECT '…closing text…' ::MARKDOWN;
```

- **Grammar.** A `::GUIDE_STEP` statement opens a step; everything after it
  — panels, inputs, a `::GROUP` box, a `::VIEW` group, `::SHOW_IF` — belongs
  to it, up to the next marker or the group's close. Unlike `::VIEW`, the
  markers of one group are *not* consecutive — each one opens the next step
  of the sequence.
- **Grouping.** A **step group** is the run of markers inside one top-level
  section: the whole dashboard with no `::TAB`s, otherwise the current
  `::TAB`. A group closes at the next `::TAB`, a `::SUBTAB`
  (`step/mixed-with-subtab`: a sequence never nests inside a sub-tab) or end
  of script. A marker reached while inside a `::SUBTAB` never opens a
  sequence at all (`step/inside-subtab`, an error) — it draws as a plain
  `::SECTION` heading instead. A `::GUIDE_STEP` marker closes an open
  `::VIEW` group and an open `::GROUP` box, the same way a `::TAB` does.
- **The step variable.** Each group keeps its current step (1-based) in a
  variable, readable with `getvariable()` like any other: the alias of the
  group's first marker when it's a valid identifier (`lesson` above), else
  `step` for the first unnamed group on the dashboard and `step_2`, `step_3`,
  … for the ones after it. It travels in the URL as `?v.<name>=n` (only when
  not on step 1) through the same mechanism as an input, and on a locked
  server it must be listed in `-- @vars` if the dashboard declares one
  (`step/var-not-allowed`). Reusing the name for an input or a `SET
  VARIABLE` elsewhere is `step/var-clash` (error).
- **The gate.** `::GATE` is a BOOLEAN column on the marker: Next stays
  disabled until it evaluates to `TRUE` — the same fail-closed rule as
  `::SHOW_IF` (`FALSE`, `NULL`, no row, a non-boolean value or a query error
  all count as closed). `::GATE_MESSAGE` is the text shown beside the
  disabled Next; without one it reads "Finish this step to continue". A
  gate is re-evaluated on every run (the marker is a plan statement like any
  other, so a change to a variable it reads re-triggers it). It's common for
  a gate to read a variable its own step's input sets (the "answer this
  step's quiz to continue" shape) — `dashboard --check` checks that reading
  with that input's own default already bound, the same as the interactive
  browser does. The step list lets a viewer jump to any step up to the
  furthest one whose gate holds; a
  URL or seed past that clamps back to it. `::GATE`/`::GATE_MESSAGE` beside
  a statement without `::GUIDE_STEP` does nothing (`step/gate-without-step`,
  error); a gate on the last step does nothing either (`step/gate-on-last`,
  warning, since there's no further step to hold back). Because a gate is
  re-evaluated on every run, changing an earlier step's answer while
  further along can re-close a gate that already opened — the viewer is
  clamped back to that step, same as a stale URL/seed would be.
- **Accessibility.** Keyboard Back/Next/step-list buttons; focus moves to
  the new step's heading on every change. The visible progress strip
  ("Step 2 of 5 · Drift sandbox") is not itself a live region (it is
  rebuilt every run, and a screen reader usually only announces a text
  change on a node already in the page, not a brand-new one); a small
  visually-hidden, persistent announcer — one per sequence, updated in
  place, only when its text actually changes, and only while its section
  is shown — carries the same text, plus the gate message while Next is
  closed. A disabled Next is `aria-disabled="true"` (not `disabled`, so it
  stays focusable) and explains the gate through `aria-describedby`.
- **Static output and the script-free served page** (`--svg`, the CLI
  `dashboard file.sql out.html`, and a dashboard served with no `.sql` on
  the page) **flatten the sequence**: every step is drawn in order,
  preceded by a `::SECTION`-style "Step n of N · Label" heading (`N` counts
  only the steps shown — one hidden by `-- @requires` is left out), gates
  are ignored (there is no Next to disable), and the step variable is bound
  in order while walking the script, so a panel reading `getvariable('step')`
  shows that step's own value.
- **The ⤓ HTML export** (#523) keeps the stepper itself — Back/Next, the
  progress strip, the step list, the live announcer — re-wired by a small
  self-contained script, so a downloaded copy still walks one step at a
  time offline. There is no query engine in that copy, so a `::GATE` can't
  be re-checked: it is dropped, same as the static output above ("gates are
  ignored"), every step stays reachable, and the footer carries one line
  saying so ("Checks are not active in this offline copy.") on any sequence
  that had a gate to drop. Printing the export, unlike printing the live
  page, shows every step in order with its "Step n of N · Label" heading —
  like the flattened output above — because there is no second chance at
  the rest of the sequence once the page is on paper.

**`::GUIDE_STEP`/`::GATE` are not access control.** Like `::SHOW_IF`, a
gated or hidden step only changes what the page shows — `/api/panel` still
answers every step's statement, gated or not (see `docs/secure-serving.md`).

`dashboard --check` also reports `step/lone` (warning: a sequence with a
single step), `step/empty` (warning: a step with no rendering statement),
`step/duplicate-label` (warning), `step/computed-label` (note: not a plain
string literal), `step/fit-inside` (warning: `::FIT` inside a step does
nothing) and, for the gate, `step/gate-not-boolean` / `step/gate-no-row`
(warnings, the condition's result under the defaults used while checking)
and `step/gate-constant` / `step/gate-unknown-variable` (warnings, the same
checks as `::SHOW_IF`'s).

### Guided tours (`-- @tour`)

A tour is a client-side walkthrough over a dashboard's *existing* panels —
a spotlight and a callout per step, Back/Next/Done — for onboarding and
product tours. Unlike `::GUIDE_STEP`, it adds no panels and changes nothing
a viewer can save or share by SQL alone; it lives entirely in the header
comments, next to `-- @title`:

```sql
-- @title Which models need attention?
-- @tour cockpit_intro Cockpit in three steps
--   page: This page lists every monitored model and what to do next.
--   severity_kpi: This is a model's severity.
--   status_table: Click a model to open its Model Monitoring page.
-- @tour_autostart cockpit_intro

-- @name severity_kpi
SELECT max(severity) ::METRIC, 'Worst severity' ::LABEL FROM problems;
-- @name status_table
SELECT model_id, status ::BADGE, … ::TABLE FROM status;
```

- **Syntax.** `-- @tour <id> [title]` starts a tour; `id` matches
  `^[A-Za-z_][A-Za-z0-9_-]{0,63}$` (the same shape as `-- @name`); the title
  is the rest of the line, or `id` with `_`/`-` turned into spaces when
  omitted. Its steps are the comment lines *directly following* it (no
  other line — even a blank one, or another header key — in between): `--`
  then at least two spaces or a tab, then `<target>: <text>`. `target` is
  either the reserved word `page` (a centred callout with no spotlight, for
  an intro/outro) or a `-- @name` on this dashboard; `text` is up to 400
  characters and is shown as **plain text**, never Markdown or HTML. One
  tour holds up to 20 steps; a dashboard holds up to 10 tours. The parser
  scans the whole file, so put `-- @tour` blocks in the header, near
  `-- @title`.
- **Starting.** `?tour=<id>` on the URL, the toolbar's **🧭 Tour** button
  (a small menu when the dashboard declares more than one), or
  `-- @tour_autostart <id>`: once per browser, per dashboard, keyed by
  `localStorage` (never on a server, and never in the assistant's preview
  or the ⤓ HTML export, which has no tour script at all). While a tour
  runs, `?tour=<id>` stays in the address bar; ending it removes it.
- **Reaching a hidden target.** Before showing a step, the tour opens
  whichever `::TAB`/`::SUBTAB`/`::VIEW` pane its target sits in, the same
  way a click would (`?tab=`/`?view.*` update as usual). A target inside a
  `::GUIDE_STEP` pane is only opened when that step is **reachable** — a
  tour never bypasses a `::GATE`. A target hidden by `::SHOW_IF` or
  `-- @requires`, or a name that doesn't exist, is skipped (logged to the
  console); "k of m" still counts every step as written, skipped ones
  included. If nothing resolves at all, the tour doesn't start.
- **Accessibility.** The callout is a non-modal dialog
  (`role="dialog"`, `aria-modal="false"`) — the page stays clickable
  outside it, so "click a model" keeps working. Esc ends the tour from
  anywhere; focus moves to Next/Done on every step and returns to whatever
  had it when the tour started once it ends. The step text is also carried
  by a visually-hidden live region, for a screen reader that doesn't
  announce the focus move alone. `prefers-reduced-motion` drops the
  scroll-into-view and ring-repositioning animation; under 600px the
  callout becomes a full-width bottom sheet, clamped on screen either way.
- **Not access control.** Like `::SHOW_IF`/`::GATE`, a tour only changes
  what the page shows for a moment — `/api/panel` still answers every
  panel's statement regardless of whether a tour ever points at it.

`dashboard --check` reports `tour/bad-id` / `tour/duplicate-id` (errors: not
a plain identifier, or reused by another tour), `tour/unknown-panel` (error:
a step target that is neither `page` nor any `-- @name`),
`tour/autostart-unknown` (error: `-- @tour_autostart` names no tour),
`tour/empty` (warning: no steps), `tour/long-text` (warning: over 400
characters), `tour/too-many-steps` (warning: over 20 steps), `tour/too-many`
(warning: over 10 tours) and `tour/stray-step` (warning: a
`--  target: text` line right after some other header line, with no
`-- @tour` above it — likely a mis-indented tour; as written, it's read as
an ordinary comment).

### Remembering progress for a signed-in viewer (#525)

Both `::GUIDE_STEP` sequences and `-- @tour` tours are client-only by
default: "furthest step" and "tour seen" live in this browser's
`localStorage`. On a server with `options.state` (`::SUBMIT`, see "Recording
answers" above and `docs/secure-serving.md`), a signed-in viewer's progress
can instead be remembered across devices — with **no new endpoint and no
change to what Next does**. Both patterns are ordinary SQL and comments,
reading `viewer_state` the same way a `::GATE` already can; neither needs a
server change.

**A tour that doesn't autostart twice.** Give the `::SUBMIT` that marks the
tour done the **same id as the tour itself** (its own column alias), and
gate it "record once" the usual way:

```sql skip
-- @tour cockpit_intro Cockpit in three steps
--   page: This page lists every monitored model and what to do next.
--   severity_kpi: This is a model's severity.
-- @tour_autostart cockpit_intro

-- @name done_toggle
SELECT true AS done ::TOGGLE;
SELECT 'Got it' AS cockpit_intro ::SUBMIT, 'done' AS r ::SUBMIT_VARS
FROM (SELECT 1) t
WHERE NOT EXISTS (SELECT 1 FROM viewer_state WHERE submit = 'cockpit_intro');
```

Once the viewer clicks that button, the statement returns zero rows and its
panel disappears — exactly like any other empty-gated input. The browser
reads that absence (no extra request: the `::SUBMIT` is already a
statement in the page's own plan) to skip `-- @tour_autostart` on this
viewer's next visit, from any browser, as long as they're signed in and
`options.state` is enabled for this dashboard. Keep that panel out of a
hidden `::TAB`/`::SUBTAB`/`::GUIDE_STEP` pane — a panel that simply hasn't
been lazily rendered yet looks the same as one that returned zero rows.
Without a matching `::SUBMIT`, or when `options.state` isn't on, tours keep
working exactly as before — `localStorage` only.

**Resuming a guide sequence.** Record progress with an explicit `::SUBMIT`
on each gated step — never on Next itself, which stays pure navigation —
then offer a link back to it from the intro, shown only once there's
somewhere to resume to:

```sql skip
SELECT 'Drift sandbox' ::GUIDE_STEP,
       getvariable('q2') = 'measured error' ::GATE,
       'Answer question 2 correctly to go on' ::GATE_MESSAGE;
SELECT v AS q2 ::DROPDOWN, l ::OPTION_LABEL FROM
  (VALUES (1, '', '— choose —'), (2, 'input drift', NULL), (3, 'measured error', NULL)) t(o, v, l)
  ORDER BY o;
SELECT 'Mark step 2 done' AS step2_done ::SUBMIT, 'q2' AS r ::SUBMIT_VARS
FROM (SELECT 1) t
WHERE getvariable('q2') = 'measured error'
  AND NOT EXISTS (SELECT 1 FROM viewer_state WHERE submit = 'step2_done');

-- on step 1, the intro:
SELECT EXISTS (SELECT 1 FROM viewer_state WHERE submit = 'step2_done') ::SHOW_IF;
SELECT '[Resume at step 2 →](?v.lesson=2)' ::MARKDOWN;
```

`?v.<name>=n` is the same URL the step strip already writes, so clicking the
link is an ordinary reload, not a new mechanism; a sequence with more than
one recorded step chains the same `EXISTS` check per step (or a `CASE`
picking the highest one recorded) to compute which step number to link to.
The step variable's own default stays `1`: nothing here changes what step a
visit without a stored answer, or without a sign-in, lands on.

### Loading, empty, error and "not available" panels

Every panel carries its own state — there is no full-page spinner:

- **Loading.** While a dashboard re-runs, only the panels whose query reads a
  variable that changed (`getvariable('…')`, e.g. the input or selection you just
  touched) get a soft shimmer and `aria-busy`; everything else stays as it is.
  The first load shows skeleton cards. With *reduce motion* the shimmer is a
  still veil.
- **Empty.** A panel whose query returns no rows says *No data for this
  selection* — or your own words with **`::EMPTY`** before it (like `::SPAN`, it
  applies to the next panel):

  ```sql
  SELECT 'No orders in range — widen the dates'::EMPTY;
  SELECT day::XAXIS, sum(amount)::BARCHART FROM orders
  WHERE day BETWEEN getvariable('from_day') AND getvariable('to_day') GROUP BY ALL;
  ```

  `dashboard --check` does not warn `empty-panel` for a panel with an `::EMPTY`.
- **Error.** A failing query becomes an error card in that panel's slot: a short
  cause (*Binder Error: Referenced column "revnue" not found…*) with the full
  message and the SQL behind *Details*. The other panels keep rendering.
- **Not available.** One dashboard often serves many runs or projects whose
  outputs are optional. Name the optional file in a `-- @requires <path>`
  comment line directly above the statement:

  ```sql
  -- @requires data/run/anomalies.parquet
  SELECT day::XAXIS, score::LINECHART, 'Anomaly score'::TITLE
  FROM 'data/run/anomalies.parquet';
  ```

  When the statement fails because an input is missing (*No files found*,
  *does not exist*, a 404), the panel reads **Not available** instead of an
  error; a `-- @requires` setup statement is skipped. Any other error is still an
  error card. This works the same in the browser, `serve`, the locked server
  (`serve --dashboards`, `anofox_serve_dashboards`) and the extension.

  The paths:

  - **Several inputs**, all needed: separate them with commas (or spaces), or
    write several lines.
  - **Alternatives**: `a.parquet | b.parquet` is met when either exists
    (producers name the same output differently). The message names them all:
    *This run has no a.parquet or b.parquet*.
  - **Variables**: `${name}` is the value of the DuckDB variable `name`
    (`getvariable('name')`) where the statement runs: a `SET VARIABLE` above
    it, an input, a `?v.name=` link, or `anofox_data` (the data folder a locked
    server sets). Patterns (`*`, `**`, `?`) work, as in `glob()`.
  - **Relative paths** resolve against `getvariable('anofox_data')` when it is
    set, else where DuckDB resolves the statement's own relative paths (the
    working directory).

  ```sql
  SET VARIABLE rd = getvariable('anofox_data') || '/' || getvariable('run');
  -- @requires ${rd}/_anofox_meta_forecasts.parquet | ${rd}/forecasts.parquet
  SELECT ds::XAXIS, yhat::LINECHART FROM read_parquet(getvariable('rd') || '/*forecasts.parquet');
  ```

  `dashboard --check` and a locked server with a data folder check the paths
  before the statement runs (DuckDB's `glob()`, after expanding the variables
  with the values the statement would see): a requirement none of whose
  alternatives exists skips the statement. `--check --data <dir>`, `dashboard
  --data <dir>` and `dashboard --svg --data <dir>` all set `anofox_data` as
  the locked server does (#414). On the locked server a `${name}`
  must be a server-side value (`anofox_data`, a `path_vars` variable, a
  credential's locked variable, the login's claims, or what the dashboard's own
  statements set), and the expanded path must stay inside the data folder —
  see *A live data folder* in [secure serving](secure-serving.md).
  Elsewhere (the builder, `serve`) the statement's own missing-input error
  decides.
  The script-free page of `serve --dashboards` checks them as well, with the
  request's variables, but its session can open no file
  (`enable_external_access = false`): there every `-- @requires` input is
  missing — a gated panel reads *Not available* (without naming the file), a
  gated strip is left out of its chart, and a gated tab or `::VIEW`
  alternative is left out with its panels (the next shown alternative is
  drawn).

  A statement skipped for a missing input is skipped for whoever picked that
  run: never put `@requires` on a setup statement whose effect narrows what a
  viewer sees (a `SET VARIABLE region = 'EU'`, a filtering view — also one that
  reads a file *and* filters rows, which `requires/on-setup` cannot detect).
  Put it on the statement that reads the file, and keep filtering elsewhere.

  **A strip** (`::TRACK`) with a missing `-- @requires` input is left out of
  its chart: the chart and its other strips render, nothing marks the gap
  (never a card of its own).

  **A tab.** `-- @requires` directly above a `::TAB` or `::SUBTAB` statement
  applies to the whole tab. When it is not met, the tab is **hidden**: no
  button, no pane, and its panels are not run — rather than a tab full of
  *Not available* cards. A hidden `::TAB` takes its sub-tabs with it; a
  hidden `::SUBTAB` leaves its siblings. `?tab=` naming a hidden tab opens the
  first shown one. The paths follow the rules above; `${name}` takes the value
  set *above* the tab statement.

  ```sql
  SET VARIABLE rd = getvariable('anofox_data') || '/' || getvariable('run');
  -- @requires ${rd}/forecastability.parquet
  SELECT 'Forecastability'::SUBTAB;
  SELECT entropy::METRIC FROM read_parquet(getvariable('rd') || '/forecastability.parquet');
  ```

  **An alternative view.** `-- @requires` directly above a `::VIEW` marker
  gates that whole alternative the same way: its button, pane and panels
  disappear and never run. If the default (first) alternative is hidden, the
  first shown one becomes the default; if every alternative of the group is
  hidden, the whole switcher disappears. See *Alternative views of one panel*
  above.

  Setup statements in a hidden tab's section still run (a `CREATE TEMP VIEW`
  there may serve later tabs); one that fails because the tab's input is
  missing is expected and silent. `@requires` on the tab and on its panels as
  well is fine. Chrome directives (`::THEME`, `::DASHBOARD_TITLE`, `::ASOF`,
  …) apply to the whole dashboard wherever they sit, so they still run in a
  hidden tab's section: keep them above the first tab. Hiding is
  presentation, not secrecy: a served page's source still holds the hidden
  tab's SQL. On a locked server the tab is decided on the server (it
  answers the tab's statement *Not available*), and so is every panel in it,
  whoever asks: a hidden tab's panels never run. As for panels, a locked
  server checks the paths only with a data folder. The static renderers
  (`dashboard file.sql`, `dashboard --svg`) and the script-free page of
  `serve --dashboards` leave the tab's panels out.

  `dashboard --check` reports a missing input as a **note** (`optional/missing`),
  not an error — `--strict` still passes. It warns about a `${name}` with no
  value (`requires/unknown-variable`: the path counts as missing), an empty
  alternative (`requires/empty-alternative`: `a |`, `a || b`, a line with no
  path), a malformed `${…}` (`requires/bad-variable`) and `@requires` on a
  setup statement that reads no file (`requires/on-setup`: a `SET VARIABLE x =
  (SELECT … FROM read_parquet(…))` reads one, a `SET VARIABLE` that only
  builds a path does not); `--strict` fails
  on these warnings. A hidden tab is one note at its `::TAB` / `::SUBTAB`
  (*this ::TAB is hidden, with its panels*); its panels are not checked. A
  tab whose `${name}` is set only by a `SET VARIABLE` below it is always hidden
  on a locked server (it decides the tab before that statement):
  `requires/tab-always-hidden`. On the locked server a pattern (`*`, `?`, `[…]`) counts
  only in the `@requires` text itself, not in a variable's value.

### Interactivity

Every chart is hoverable (bars/points/areas carry per-mark tooltips; lines get
point markers and dim as whole lines). **Linked highlighting**: click any
series/bar to highlight it across all panels and dim the rest — click empty
space (or the mark again) to clear. Series colours are **consistent across
charts**.

> **Point markers** (`::POINTS`, #631): line, step and area charts put a marker on every data point; it carries the point's tooltip and the click target for `::LINK`. On a dense series (its x positions closer than 3 px apart on average, e.g. 7 days at 15-minute resolution) the markers are drawn without ink, so the line reads as a line, but they stay in the chart: hovering, the crosshair tooltip, keyboard focus and `::LINK` clicks work as before, and the point under the pointer shows. A short series (12 monthly points) keeps its dots. `'none'::POINTS` always hides the markers, `'always'::POINTS` always draws them, `'auto'` is the default. The rule is decided per facet panel. `--check` warns on another value (`points/unknown`).

> **Hover focus**: hovering a bar, point, line or area of a chart with several series (a `::CATEGORY`, or several measures) emphasises that whole series and fades the others, as hovering its legend entry does; moving off clears it. Focusing a mark from the keyboard does the same. A click still sets the sticky linked highlighting: while it (or a brush, a named selection or a `::STORY` step) is on, hovering changes nothing. It is off where each series is a single mark (a pie, a single-series bar), on maps (hovering a map feature emphasises its series on every map instead), diagrams, heatmaps, calendars and `::TRACK` strips, above 2,000 marks, on a touch tap (a tap is a click), and with `'none'::HOVER_FOCUS`. The change is instant (no animation). Static output (the HTML export, `dashboard file.sql out.html`, the script-free served page) has no hover focus. `--check` warns on a value other than `'series'` / `'none'` (`hover-focus/unknown`).

**Cross-filter**: a click also sets a DuckDB variable `selected` to the clicked
value and re-runs the dashboard. Panels *opt in* by referencing it — filtered
panels re-query, the rest just highlight:

```sql
SELECT week::XAXIS, sum(n)::LINECHART
FROM sessions
WHERE getvariable('selected') IN ('', channel)   -- '' (nothing clicked) = all
GROUP BY ALL ORDER BY week;
```

Clicking a channel narrows those panels to that channel; clicking empty space
clears. See the **"Cross-filter"** sample. (Browser builder / `serve` only.)

**Selecting table rows** is opt-in per table, with a `::ROWSELECT` directive
before it (tables without one are not clickable). Like `::COL`/`::HEIGHT`,
`::ROWSELECT` (and `::TOTALS`, `::COLGROUP`, `::FLAG`, `::HIGHLIGHT`,
`::SUBTOTAL`, `::SELECT_AS`, `::DRILL`, `::EMPTY` — every `[layout]` role
described as "next table's …" / "next panel's …") is its own **preceding**
statement, never a column on the table/chart's own `SELECT`:

```sql skip
-- Wrong: looks like a column, passes `--check` clean, but 'single' and
-- 'none' render as literal extra table columns and the row click does
-- nothing (`dashboard --check --strict` now flags this: `layout/next-panel`).
SELECT id ::TABLE, name, 'single'::ROWSELECT, 'none'::TOTALS FROM t;

-- Right: each directive is its own statement, read for the table that follows.
SELECT 'single'::ROWSELECT;
SELECT 'none'::TOTALS;
SELECT id ::TABLE, name FROM t;
```

| `SELECT … ::ROWSELECT;` | A click on a row… |
|---|---|
| `'none'` (default) | does nothing |
| `'single'` | sets `getvariable('selected')` and `getvariable('<first column>')` to the row's first value; click it again (or empty space) to clear |
| `'multi'` | toggles a check mark; `getvariable('<first column>')` is the list of checked values (`[]::VARCHAR[]` when none) |

```sql
SELECT 'multi'::ROWSELECT;
SELECT sku, sum(amount) AS total ::TABLE FROM sales GROUP BY sku;
SELECT sum(amount)::METRIC, 'Selected SKUs'::LABEL FROM sales
WHERE len(COALESCE(getvariable('sku'), []::VARCHAR[])) = 0 OR list_contains(getvariable('sku'), sku);
```

The click-to-variable binding fires regardless of whether the first column's
*rendered text* happens to look numeric (an id like `791`, even cast to
`VARCHAR`) — only the cell's right-alignment cares about that. The variable
it writes is named after that table's first column, never chosen by you:
two `::ROWSELECT` tables that end up with the same first-column name (even
in different `::TAB`s) silently share one selection — a click in either
toggles both, which looks like nothing happened. `dashboard --check` flags
this (`interaction/rowselect-shared-name`); alias one table's first column
to a different name if the sharing isn't intended.

Sometimes it *is* intended — several registers deliberately fanning in to one
detail panel, or a dropdown input and a `::ROWSELECT` table meant to hand a
selection to each other either way. Put `-- @shared <name>` directly above
the `::ROWSELECT` (or input) that would otherwise be flagged, naming the
variable it shares:

```sql
-- @shared item
SELECT 'single'::ROWSELECT;
SELECT name AS item ::TABLE, id * 2 AS "Doubled" FROM items;
```

`--check --strict` then skips `interaction/rowselect-shared-name` /
`interaction/name-clash` for just that variable on that one directive — every
other clash on the dashboard still is flagged. A `-- @shared` name this
directive doesn't itself end up setting (a typo, or the wrong column) is
flagged on its own (`interaction/shared-unused`), so a mistyped name never
silently disables nothing.

#### Stable element names (`-- @name`)

`-- @name <name>` directly above a statement tags the first element it draws
with `data-dp-name="<name>"` — a panel, an input, a `::GROUP` box, a
`::TAB`/`::SUBTAB` button, a `::VIEW` switcher, or a `::LABEL`/`::SECTION`
heading:

```sql
-- @name severity_kpi
SELECT max(severity) ::METRIC, 'Worst severity' ::LABEL FROM problems;
```

Names make stable test selectors (`[data-dp-name="severity_kpi"]` instead of
a positional `[data-stmt="3"]`, which shifts whenever a statement is added or
removed above it) and, later, deep links and guided-tour targets (#518). A
re-run (cross-filter, an input change, `::RELOAD`) re-tags the element
whether it was patched in place or rebuilt. `dashboard --check` flags a name
that isn't a plain identifier (`name/bad`), the reserved `page` (used by a
guided tour's intro/outro, #518 PR B — `name/reserved`), one reused on
another statement (`name/duplicate`), and one on a statement that draws
nothing — setup, or a layout directive with no element of its own
(`name/not-drawn`).

#### Named chart selections (`::SELECT_AS`)

By default every chart click writes the one shared `selected`. A
`::SELECT_AS` directive before a chart gives that chart **its own variable**, so
two charts filter independently (and compose):

| `SELECT … ::SELECT_AS;` | A click on a mark… | A brush (toolbox ▧)… |
|---|---|---|
| `'region'` | sets `getvariable('region')` to the mark's key (`''` = nothing; click it again to clear) | on a continuous / date x: sets `region_from` / `region_to` |
| `'region multi'` | selects just that value; **shift / ctrl / ⌘-click** adds or removes one — `getvariable('region')` is a `VARCHAR[]` (`[]` = nothing) | same range; on a discrete x it selects the brushed values |

The mark's key is the one the shared `selected` would get (its `::CATEGORY`
series, else its x). The brushed range is the smallest and largest x of the
marks inside the band — numbers on a numeric x, `'YYYY-MM-DD'` text on a date x
(cast it: `getvariable('week_from')::DATE`); `NULL` when nothing is brushed.
The selected marks stay bright, the rest dim, a brushed range stays shaded, and
a chip above the chart shows the selection (× clears it). **Esc** or a click on
empty space clears every selection. Selections are part of the URL state.

```sql
SELECT 'region multi'::SELECT_AS;
SELECT region::XAXIS, sum(revenue)::BARCHART FROM orders GROUP BY 1 ORDER BY 2 DESC;
SELECT 'week'::SELECT_AS;
SELECT week::XAXIS, sum(revenue)::LINECHART FROM orders GROUP BY 1 ORDER BY 1;

SELECT sum(revenue)::MONEY FROM orders
WHERE (len(getvariable('region')) = 0 OR list_contains(getvariable('region'), region))
  AND week BETWEEN coalesce(getvariable('week_from')::DATE, week)
               AND coalesce(getvariable('week_to')::DATE, week);
```

The browser (and `--check`) start with nothing selected — `''` / `[]` and NULL
ranges — so the guards above are all a panel needs. See the **"Selections &
drill"** sample.

#### Drill-down (`::DRILL`) and drill-through (`::LINK`)

A `::DRILL` directive before a chart makes it a **drill-down** over a level
path. A click on a mark goes one level deeper; a **breadcrumb** above the chart
(`All › EU › Germany · by city`) goes back. The chart's SQL picks its x column
and filters from the drill variables:

| Variable | Value |
|---|---|
| `drill_level` | the level on screen, e.g. `'country'` |
| `drill_depth` | 0-based depth (`0` = the top level) |
| `drill_path` | the values drilled through, `VARCHAR[]` (`['EU']`) |
| `region`, `country`, … | one per level: the value chosen at that level, `''` until drilled past |

`'geo: region > country > city'` names the variables `geo_level`, `geo_depth`,
`geo_path` instead (for two drill charts on one page). A level variable is a
DuckDB variable like any other, so other panels follow the drill too (do not
name an input like a drill level). On the last level a click does what it
would without `::DRILL` (a `::LINK`, a `::SELECT_AS`, or `selected`). The path
is part of the URL state (`?v.drill_path=EU&v.drill_path=Germany`).

```sql
SELECT 'region > country > city'::DRILL;
SELECT CASE getvariable('drill_level') WHEN 'region' THEN region
            WHEN 'country' THEN country ELSE city END AS place ::XAXIS,
       sum(revenue)::BARCHART, ('Revenue by ' || getvariable('drill_level'))::TITLE
FROM orders
WHERE getvariable('region') IN ('', region) AND getvariable('country') IN ('', country)
GROUP BY 1 ORDER BY 2 DESC;
```

A **`::LINK`** column is a **drill-through** target. Its value is a template;
`{value}` is the clicked mark's key (its x, else its series) or a table row's
first column, `{x}` / `{series}` a chart mark's x and series, `{column}` any
column of a table row, `{region}` … the drill path — each URL-encoded:

| Template | A click… |
|---|---|
| `'#Detail'`, `'#City detail?v.pick_city={value}'` | opens that `::TAB`, after seeding the variables (an input only takes one of its own options) |
| `'?v.region={value}'` | sets variables on this dashboard |
| `'/d/sales?v.region={value}'` | opens another served dashboard pre-filtered (see *Viewer state in the URL*) |
| `'https://en.wikipedia.org/wiki/{value}'` | opens the URL in a new browser tab |

Any other scheme (`javascript:`, `data:`) is refused (`--check` warns:
`link/bad-url`; it also warns `interaction/bad-value` on a malformed
`::SELECT_AS` / `::DRILL` value and `interaction/name-clash` when an input, a
selection and a drill level write the same variable). On a chart the click follows the link of the row the mark was
drawn from; in a `::TABLE` / `::PAGED` table the column renders as a link cell
(`City detail →`, `Open ↗`).

```sql
SELECT city::XAXIS, sum(revenue)::BARCHART, '#City detail?v.pick_city={value}'::LINK
FROM orders GROUP BY city ORDER BY 2 DESC;
SELECT city::TABLE, sum(revenue) AS revenue ::MONEY,
       '#City detail?v.pick_city={city}' AS detail ::LINK
FROM orders GROUP BY ALL ORDER BY revenue DESC;
```

#### Maps

A `::MAP` panel takes part in all of the above. Each feature (a region, a
route, a point) is a mark keyed by its **`::CATEGORY`** — or, without one, by
its **`::LABEL`** (a choropleth's region name):

- **Hover** shows the feature's `::LABEL` as the heading (several lines with
  `chr(10)`), then its series and measure, and lights that key up on every
  map that has it (a van's whole route on both maps); a line is hoverable a
  few pixels either side. The plain `<title>` is unchanged.
- **Click** highlights every feature of that key and dims the rest, linked
  across panels like any series — clicking `Van 3` on one map picks out van 3
  on every map (and every chart) that has it. It sets `getvariable('selected')`
  and honours `::SELECT_AS`, `::DRILL` and `::LINK` like other charts; a
  `::LINK` follows the template of the row the feature was drawn from. Click
  empty space (or **Esc**) to clear.
- **Keyboard**: Tab into the map, arrows walk the features (series by series),
  Enter / Space click the focused one.
- **Zoom**: scroll (about the pointer) and drag to pan. **Double-click a
  feature** to zoom to it (a point or a small region gets a sensible area
  around it; the first click's selection stays), double-click empty space to
  go back to the whole map. While zoomed, a **⟳ chip** in the corner resets.
  The chart toolbox adds **Reset view**, **Zoom to selection** (the clicked
  series' features) and **Find a place** — a search box over the map's own
  feature labels (no geocoding): Enter or a pick zooms to it, focuses it and
  opens its tooltip ("Zoomed to …" is announced). Keys on a focused map:
  **+** / **−** zoom about the centre, **0** resets, **z** zooms to the
  focused feature. Zoom stops at 1/64 of the whole map's width in and twice
  it out; each step is instant. The ⤓ HTML export keeps the view on screen,
  without the chip. Maps that carry the same **`::SYNC_ZOOM`** value zoom and
  pan together (same centre and scale, each in its own aspect); a map that
  comes back after a re-run takes the group's view.

```sql
-- Two plans side by side: routes and stops coloured by van, hover text per
-- feature, zoom linked. A click on a van highlights it on both maps.
SELECT wkt::MAP, 'Van ' || van ::CATEGORY, hover ::LABEL, 'routes'::SYNC_ZOOM,
       'Today'::TITLE
FROM today_layers ORDER BY van;
SELECT wkt::MAP, 'Van ' || van ::CATEGORY, hover ::LABEL, 'routes'::SYNC_ZOOM,
       'Plan'::TITLE
FROM plan_layers ORDER BY van;
```

Features are drawn grouped by colour (in the order of the legend), so a layer
meant to sit on top — a marker around some points — needs a key that sorts
after the others, and should leave the point it marks uncovered (a ring rather
than a disc) so the point stays clickable. The brush and the ◧ value filter
are not offered on maps.

A **`::COLORS`** level map beside the `::CATEGORY` (`'Depot:neutral; Late
stop:bad'`) pins the levels it names to a status colour or `#rrggbb`, first in
the legend; the other levels keep the palette in sorted order from its first
slot, so a named level missing from one map doesn't shift the colours of the
rest. An **`::XAXIS`** keys each feature for a **`::STORY`**: one caption per
x, and a step lights the features with that x — a route leg by leg, with a
Play button. See *"Which van serves which stop tomorrow?"*.

#### Viewer state in the URL

The URL carries the view: `?tab=<name>` opens a tab, `?view.<id>=<label>`
picks an alternative of a `::VIEW` switcher, and `?v.<name>=<value>` seeds a
variable — an input's value, the shared `selected`, or a named selection.
After every change the address bar is rewritten (no reload) with the inputs
that differ from their defaults and the live selections, so a reload, a
bookmark or **Share** restores the same view.

| Parameter | Seeds |
|---|---|
| `?v.region=EU` | a `::DROPDOWN` — only if `EU` is one of its options |
| `?v.channel=app&v.channel=web` | a `::MULTISELECT` (repeat the key; `?v.channel=` = none) — unknown values are dropped |
| `?v.min_n=20` | a `::NUMBER` — a finite number |
| `?v.since=2024-02-01` | a `::DATE` / either end of a `::DATERANGE` — a real calendar date |
| `?v.note=promo` | a `::TEXT` (≤ 500 characters) |
| `?v.p=ytd` · `?v.p=2026-01-01..2026-03-31` · `&v.p_grain=week&v.p_compare=yoy` | a `::PERIOD` named `p`: a preset it offers or a custom range, a grain its switch offers, `prev` / `yoy` / `none` — each part checked on its own |
| `?v.selected=web` | the shared click cross-filter |
| `?v.sku=SKU-A&v.sku=SKU-C` | a named selection (a `::ROWSELECT` table's first column, a chart's `::SELECT_AS`) |
| `?view.2=Bars` · `?view.yoy=Bars` | a `::VIEW` group's open alternative, by its button label — the group's alias (`AS yoy`) when it has one, else its 1-based position in the script; never a cache key, and ignored on the script-free served page |
| `?frame.weekly=Week%205` | a time map's (`::FRAME`) shown frame, by its label — only for a panel with a `-- @name` (here `-- @name weekly`), and only when it isn't the last frame (the default); written as you step or play (at most every 0.5 s), ignored when the label no longer exists |

A value that fails its input's check is ignored (the input keeps its default).
Only variable **values** travel, never SQL: on a locked server
(`anofox_serve_dashboards`, `/d/<id>?v.region=EU`) they are bound as typed
literals like any input change, so an embedding app or another dashboard can
open a dashboard pre-filtered. Example:
`?dashboard=how-do-filters-change-what-we-see&tab=Cross-filter&v.sku=SKU-A&v.market=EU`.

### Motion: entrances, flow lines, stories and transitions

Charts move only where the motion carries meaning, and only in the browser:
`--svg`, PNG and PDF output stay static, and under the viewer's *reduce
motion* setting every chart shows its final state at once. Motion runs on
data marks only (lines, bands, points, bars) — never on grid lines or axes —
and a finished animation leaves no state behind.

**Entrance (`::ANIMATE`).** On a full run (load, *Run*) each panel fades in
and its chart plays an entrance; cross-filter and input re-runs stay instant.

| `::ANIMATE` | Entrance |
|---|---|
| `'auto'` (default) | by chart kind: lines and areas draw in left to right, bars grow from their baseline staggered by x, points pop in left to right; other charts only fade in with their panel |
| `'draw'` | lines draw in; points, bars and bands are wiped in with them |
| `'grow'` | bars grow staggered by x, points pop in, lines draw in |
| `'reveal'` | the chart wipes in up to the panel's `::XLINE` (e.g. Today), pauses, then the rest — a forecast fan, its median, its points — grows forward from it |
| `'none'` | no motion at all, not even the panel fade |

```sql
SELECT week ::XAXIS, series ::CATEGORY, demand ::LINECHART,
       lo ::BAND_LOWER, hi ::BAND_UPPER,
       DATE '2026-10-12' ::XLINE,          -- Today: a date works on a date axis
       'reveal' ::ANIMATE
FROM outlook;
```

Above about 500 marks the per-mark stagger becomes a single wipe. An
`::XLINE` may be a number or, on a date axis, a `DATE` / `TIMESTAMP`; its
tooltip shows the date. `--check` warns on an unknown mode
(`animate/unknown`) and on `'reveal'` without an `::XLINE`
(`animate/reveal-no-xline`, the chart then wipes in from the left). The
static HTML export replays the entrances.

**Flow lines (`::LINESTYLE`).** A per-row column keyed by the row's
`::CATEGORY`: the series whose rows say `'flow'` is drawn with dashes that move
along the line — the "predicted" look for a forecast or a connector;
`'dashed'` gives still dashes, `'solid'` (or NULL) the default. Without a
`::CATEGORY` it styles the chart's line. Static output (`--svg`, PNG, PDF) and
*reduce motion* show plain dashes. `--check` warns on an unknown style
(`linestyle/unknown`).

```sql
SELECT week ::XAXIS, series ::CATEGORY, demand ::LINECHART,
       CASE WHEN series = 'Forecast' THEN 'flow' END ::LINESTYLE
FROM outlook;
```

**Stories (`::STORY`).** A text column whose non-empty rows become story
steps on the chart: *‹ / Play / ›* controls appear under it; each step lights
up the marks at that row's x (or its series with `'series'::STORY_BY`), dims
the rest and shows the caption with a connector to them. Step 0 is the
overview. Several rows at the same x share the first caption.

```sql
SELECT week ::XAXIS, channel ::CATEGORY, demand ::BARCHART,
       CASE week WHEN 'W43' THEN 'First forecast week: Store ahead of Online'
                 WHEN 'W48' THEN 'Online dips — a quiet slot for the migration' END ::STORY
FROM plan;
```

The controls are buttons (keyboard: Tab, Enter / Space) and the caption is an
`aria-live="polite"` region, so screen readers hear each step. Under *reduce
motion* steps switch instantly and there is no Play. A re-run (input change,
cross-filter) keeps the current step. Dimming is one shared state: a story
step, a click selection, a brush or named selection and legend focus each
dim on their own, and clearing one keeps the others. The HTML export carries
a working stepper, starting at the overview. `--check` warns on a
`::STORY_BY` that is not `'x'` / `'series'` (`story/bad-by`).

**Transitions (`::TRANSITION`).** By default a re-run (an input change, a
cross-filter click) swaps the chart for its new render. With `'tween'`, the
new chart (with its new axes) comes in and its marks move from where they
were to their new values in 300 ms: bars and points by position and size,
lines and bands point by point when the number of points is unchanged (else
they cross-fade). Marks are matched by their x and series; new ones fade in,
removed ones fade out. Above about 500 marks the chart cross-fades instead.
`'none'` is the default. A tween only plays on re-runs (a full run plays the
entrance), not under *reduce motion*, and not in the static export (which has
no inputs). `--check` warns on an unknown value (`transition/unknown`).

```sql
SELECT week ::XAXIS, channel ::CATEGORY,
       demand * md_factor(week, getvariable('scenario')) ::BARCHART,
       'tween' ::TRANSITION
FROM plan;
```

The builder example *How will demand develop after today?* (Interactivity)
combines all four: a reveal from Today, a flowing forecast line, a story on
the weekly bars and a scenario input whose changes tween both charts.

### Tables

A `::TABLE` / `::PAGED` panel comes with a toolbar: the row count ("8 of 24
rows") with a chip per active filter, a **search** box, **Filters** (a filter
under each header: a dropdown for a column with few values, `> n` / `< n` and a
range slider for numbers, "contains" for text), **Σ Totals** (a totals row
over every matching row, not just the page: sums of amounts and counts, no total
for rates and percentages (their tag reads "none ▾"), means of the other
measures, counts of badge values — each numeric total carries a
small tag, e.g. "sum ▾", to switch it to mean, median, min, max or count; the
choice is kept per table), **Stats** (a strip under the header: a 10-bin
histogram with min and max for each number column, the top 3 values with their
share for the others, "all N values distinct" for ids and names, empty counts —
over the matching rows), **Columns** (show / hide),
**Pages | Scroll** (a table of up to 20 rows opens with all of them on one
page, a longer one pages by 10), **Compact | Comfortable** row density (compact by default
above 50 rows) and **⤓ Export**: CSV (the matching rows, visible columns, raw
values) or Excel (the same rows as an `.xlsx` that reads like the table: the headers
with their units, the grouped header, each column's number format — scaled units
keep the raw value, e.g. `#,##0.00,,` shows 5,350,000 as 5.35 — dates as dates,
subtotal and totals rows as live `SUBTOTAL()` formulas, a frozen header with a
filter; a `::PAGED` table exports the loaded rows and its SQL totals as values).
`::DOWNLOAD_XLSX` writes the raw query rows as a real `.xlsx` (numbers, dates and
booleans typed). Hovering a
header shows the column's min / max / mean, empty and distinct counts.
`::TOOLBAR` drops some or all of this chrome for a table that never needs it
(a small lookup table beside a chart) — see the directive table below.

Formatting (one set of rules, `src/table.rs`, for the browser table and the
headless `dashboard --svg` / static HTML table — which also right-aligns
numbers, moves units into the header and adds the totals row, `::TOTALS`
included (badge counts in the totals are browser-only)): raw column names read as headers (`load_pct` → "Load %", `avg_mae` →
"Avg MAE"; an `AS "Header"` alias is kept as written) and wrap onto two lines;
numbers are formatted per column — aligned decimals, grouping (ids and years stay
ungrouped), a true minus sign, muted zeros, and one unit per `::MONEY` /
`::COMPACT` column ("$5.87M … $1.71M", never "$4M" next to "$5.9M"); a currency or K / M / B unit
moves into the header once ("Revenue ($M)" over plain 4.61, 3.42, …); `::TREND`
shows a signed change (▲ +2.7, ▼ −3.1, – 0.0). The totals row sits under a double
rule and stays in view on a long page; a line under the table says which
aggregation each column shows ("sum of Revenue · median of Accuracy"). Columns take their natural width (header and formatted cells; long text wraps),
and a table with few columns stays narrower than a wide card. Plain
numeric columns get a slim in-cell bar, empty cells show "—" and booleans ✓ / ✗.

Options for the *next* table (directives, like `::ROWSELECT`; columns are named
by their raw name or their header, case-insensitive):

| Directive | Value | Effect |
|---|---|---|
| `::TOTALS` (`::TOTAL`, `::AGGREGATE`) | `'sum'` \| `'mean'` \| `'median'` \| `'min'` \| `'max'` \| `'count'` \| `'none'`, or `'Revenue: sum; Per 10k: none; km to DC: mean'` | the totals row's aggregation for every numeric column, or per column (a bare value inside the list sets the rest); entries are separated by `;` (or `,` when there is no `;` — use `;` when a header contains a comma). `'none'` hides the row and its button; `'Column: none'` leaves that cell empty. Without it: sums of amounts and counts (`::MONEY`, `::COMPACT`, whole numbers), no total for rates and percentages (`::PERCENT`, a `%` in the name, or a name with the word pct / percent / rate / ratio / share / per / pro — whole words: `conversion_rate`, `churnRate`, `Per 10k` are rates, `generated` and `accurate` are not; write `win_rate`, not `winrate`; German compounds ending in -quote / -anteil count too), means of the rest (and of averages: a name with the word avg / average / mean / median / score / index / percentile). Distances are not recognised by name: a distance per row ("km to DC") is summed unless you name it ('km to DC: mean'). `dashboard --svg` draws the same totals. The viewer's pick in a cell wins. |
| `::COLGROUP` (`::COL_GROUP`, `::HEADER_GROUP`) | `'Backtest: mae, rmse \| Holdout: mae_h, rmse_h'` | a header row with a label over adjacent columns (a thin rule under each label) |
| `::FLAG` (`::THRESHOLD`) | `'mape > 0.2, bias < -5, status = breach'` | cells meeting a condition (`>` `>=` `<` `<=` `=` `!=`) get a ⚠ and the warning tint — never colour alone |
| `::HIGHLIGHT` (`::TOPN`) | `'Revenue: top 3, Margin: bottom 2'` (`'top 3'` alone: the first numeric column) | the top / bottom n values in bold with their rank (#1, #2 / ↓1, ↓2); a `::PAGED` table ranks within the page |
| `::SUBTOTAL` (`::SUBTOTALS`, `::GROUP_TOTAL`) | `'region'` | the rows grouped by that column (groups in the order of their first row; sorting by the column orders the groups, any other column sorts within them), the group's value shown once, and a subtotal row after each group — each numeric column with the aggregation its total uses. A `::TABLE` only (a `::PAGED` table holds one page). |
| `::TOOLBAR` | `'none'`, or an allow-list of the parts to keep — `'search'`, `'filters'`, `'totals'`, `'stats'`, `'columns'`, `'density'`, `'export'` (comma- or semicolon-separated, e.g. `'search,export'`) | `'none'` drops the whole `.tbl-bar` — row count, search, Filters, Σ Totals, Stats, Columns, Compact/Comfortable, Pages\|Scroll and ⤓ Export — not just empties it; a small lookup table next to a chart (a handful of rows, 2–3 columns) that can never need paging, column search or filtering sits under nothing wider than its own columns. The one exception: a `::ROWSELECT 'multi'` table with an active selection keeps the bar for just its "N selected" chip. An allow-list keeps only the named parts and hides the rest; each kept part still shows only past its own usual row/column threshold (e.g. `'filters'` on a 3-row table still shows no Filters button — that needs > 10 rows, same as without `::TOOLBAR`). The row count and any active filter chips are not a part of their own: they show whenever the bar is drawn at all, under any allow-list. `'totals'` governs only the Σ Totals *toggle button* — the totals row itself keeps following the author's `::TOTALS` or the automatic default either way, since hiding data a viewer never asked to hide would be a bigger change than hiding a button; only a viewer's own saved on/off pick is ignored once the button is gone. `'density'` also covers the Pages\|Scroll view switcher, not just Compact\|Comfortable row height. Excluding `'export'` (or `'none'`) still leaves a small hover ⤓ CSV button on the panel — see below. Unrecognised part names are dropped, never an error (like an `::TOTALS` entry naming no column). No `::TOOLBAR` at all: every part shows, at its own threshold — unchanged. #354 |

```sql
SELECT 'revenue: sum, margin: mean, accuracy: median'::TOTALS;
SELECT 'Money: plan, revenue | Quality: accuracy, bias'::COLGROUP;
SELECT 'margin < 12'::FLAG;
SELECT 'revenue: top 2'::HIGHLIGHT;
SELECT region::TABLE, plan::MONEY, revenue::MONEY, margin,
       accuracy::COLORSCALE_HIGH_GOOD, bias::COLORSCALE_DIVERGING
FROM regions;

-- a 2-row lookup table next to it: no toolbar at all
SELECT 'none'::TOOLBAR;
SELECT metric::TABLE, value FROM (VALUES ('Plan', 4050), ('Actual', 4220.75)) t(metric, value);
```

A table excluding `'export'` (including `'none'`) still gets a small hover ⤓
CSV button — the same fallback any table with no export menu of its own gets
(`addExportButtons`, `web/app.js`) — exporting the same matching, sorted,
visible rows and columns its own ⤓ CSV button would (never just the page on
screen). Add `::DOWNLOAD_CSV` / `::DOWNLOAD_XLSX` / `::DOWNLOAD_PDF` as their
own panel for a bigger, explicit export button when the table's own export
menu is suppressed but downloading still matters.

`dashboard --check` notes `totals/summed-rate` when a `'sum'::TOTALS` (or a bare
`sum` in the list) would also sum a rate or percentage column the value does
not name. Give that column its own total: `'Per 10k: none; sum'`.
It notes `totals/unknown-column` when an entry names no column of the table (by
raw name or header, case-insensitive). That entry would be ignored. With `,`
separators a header holding a comma splits in two: use `;`.

Colour scales: `::COLORSCALE` shades low values green and high values red (errors,
latency); `::COLORSCALE_HIGH_GOOD` the other way round (accuracy, scores);
`::COLORSCALE_DIVERGING` shades around zero (positive green with a "+", negative
red, zero neutral) for deltas and biases.

Accessibility and print: each table has a `<caption>` (the panel title, for
screen readers), `scope` on its headers, `aria-sort` on the sorted column and
`aria-selected` on selectable rows. Printing (or the browser's PDF) hides the
toolbar, filters, check marks and pager, prints every matching row of a
`::TABLE` (a `::PAGED` table prints its page), repeats the header on each page
and puts the totals at the end.

**Scroll** is a virtualised view: only the rows around the viewport are drawn,
so it stays smooth over any number of rows.

| | `::TABLE` | `::PAGED` |
|---|---|---|
| data in the browser | the whole result | one page (Scroll: 100-row blocks, fetched as you scroll) |
| sort, search, column filters | in the browser | in DuckDB (`ORDER BY`, `WHERE`) |
| totals and column statistics | in the browser | one aggregate query in DuckDB (count, distinct, sum, mean, median, min, max) |
| filter dropdown values | from the rows | `SELECT DISTINCT` in DuckDB (under the other filters) |
| Stats strip | in the browser | one query in DuckDB: a 10-bin histogram and the top 3 values per column |
| good for | up to ~10k rows | any size: Parquet on S3, MotherDuck, a served database |

A `::PAGED` request is the same everywhere: in the browser and the authoring
`serve` the query is built by the wasm `page_sql()`; a **locked** server
(`anofox_serve_dashboards`) receives only the structured request — page, sort,
search, column filters (`eq`, `contains`, `gt`, `ge`, `lt`, `le`), a statistics,
column-profile or distinct-values request — and builds the identical SQL itself, with every
value a bound parameter and every column name a quoted identifier.

### Example

```sql
CREATE TABLE sessions AS SELECT * FROM (VALUES
  ('W1','app',30),('W1','web',22),('W2','app',41),('W2','web',28)
) t(week, channel, n);

SELECT 'Weekly sessions'::LABEL;                                    -- heading
SELECT week::XAXIS, channel::CATEGORY, sum(n)::BARCHART_STACKED     -- stacked bar
FROM sessions GROUP BY ALL ORDER BY week, channel;
SELECT week::XAXIS, sum(n)::LINECHART FROM sessions GROUP BY ALL;   -- line
```

### Authoring rules (avoid silent breakage)

A few rules where the wrong form makes a panel render incorrectly or disappear:

- **Only query statements are panels.** A statement whose first keyword is
  `SELECT`, `WITH` or `FROM` (or that starts with `(`) may carry roles; DDL/DML
  (`CREATE … AS SELECT`, `INSERT … SELECT`, `SET`, `COPY`, …) is always setup,
  run for effect, its casts untouched.
- **Casts are read from the main `SELECT` list** — the first `SELECT` at
  bracket depth 0. A leading `WITH` CTE list is fine
  (`WITH s AS (…) SELECT week::XAXIS, n::LINECHART FROM s`); casts inside a CTE
  body or subquery are ordinary SQL. A top-level `UNION`/`UNION ALL`/
  `INTERSECT`/`EXCEPT` reads as one SELECT list: the **first** branch's roles
  decide the panel's shape, and every later branch's own `::ROLE` casts are
  stripped the same way (repeat them or not — a bare column just takes the
  same position).
- **A `::ROLE` may follow an `AS` alias on any column.** The alias is replaced by
  the internal `c{i}` name. For a measure it becomes the legend name of a combo
  chart; for an input it *is* the DuckDB variable name
  (`SELECT region AS zone ::DROPDOWN` → `getvariable('zone')`); in a
  `::TABLE`/`::PAGED`/`::DOWNLOAD_*` panel it is the column header.
- **Role tokens that are also SQL types** — `::DATE`, `::TEXT`, `::STRING`,
  `::NUMERIC` (inputs) and `::MAP`, `::GEOMETRY` (maps). In a query statement
  `::MAP`/`::GEOMETRY` are always the map role. The input tokens are roles only
  in an **input statement** (no other role except `::HINT`/`::OPTION_LABEL`/`::LABEL`/`::TITLE`,
  e.g. `SELECT DATE '2024-01-01' AS start ::DATE`); in a chart panel
  (`SELECT day::DATE, n::BARCHART …`) and inside a `::TABLE` they stay real
  casts. To cast *and* tag, chain them: `ts::DATE::XAXIS`.
- **A table uses one `::TABLE` marker, not one per column.** Tag a single column
  `::TABLE`; the rest keep their `AS "Header"` aliases and all show.
- **`::CATEGORY`/`::COLOR` is discrete.** To colour by a continuous value, bucket
  it into a `CASE` band; a raw continuous column yields a per-value legend.
- **A KPI caption is `::LABEL`, not `::TITLE`.** `::METRIC`/`::MONEY`/… render the
  number; add a `::LABEL` column for the caption (`::TITLE` is a panel title bar).
- **`*` expands in place**: `SELECT *, v::BARCHART …` works (roles are looked up
  by their `c{i}` alias, not by position).
- **A `::ROLE` must be in the `SELECT` list**, not after `FROM`; aggregating
  charts usually need `GROUP BY ALL`.

### Rendering limits & data hygiene

- **Discrete axes are capped**: more than 30 levels on a bar/box/violin/jitter x axis or in a `::CATEGORY` keep 29. By default those are the largest (by total |measure|, or row count for raw-row charts). When the rows come ranked by the measure (an `ORDER BY` on it, either direction), they are the **first 29 in row order**, so a worst-first ranking keeps its worst. The rest fold into **"Other"** (summed for bars/lines/areas/pies). A **rate or an average** is never summed: with `'percent'::YFORMAT`, or a measure named like one (`%`, `pct`, `rate`, `ratio`, `share`, `per`, `avg`, `mean`, `score`, …, the table totals' rule), the chart keeps 30 levels and leaves the rest out. To show fewer, `LIMIT` the query. Hosts can change it (`max_categories` in a `render_spec` JSON, `RenderOptions::max_categories`; `0` disables).
- **Long lines are downsampled**: a line/area/step series with more than 5,000
  points is reduced with LTTB (Largest-Triangle-Three-Buckets), which keeps the
  visual shape (`max_line_points` / `RenderOptions::max_line_points`).
- **Calendars** draw at most 50 years; a wider span is an error.
- **Sizes** are clamped to 32–8192 px.
- **Non-finite numbers** (`NaN`, `±Infinity` — bare in DuckDB's JSON, or as
  `"NaN"`/`"inf"` strings in a measure) are treated as missing; no `NaN` ever
  reaches the SVG.
- **Empty or too-small results** render a clear note — "No data", or e.g.
  "::DENSITY needs at least 2 values" — instead of a plotting error.

---

## 2. Three ways to run it

### a) CLI runner (native) — fastest to try

Renders an interactive HTML dashboard by shelling out to the `duckdb` CLI (no
bundled DuckDB compile).

```sh
cargo run --bin dashboard -- dashboards/sessions.sql   # → dashboard.html
xdg-open dashboard.html
```

The render (and `dashboard --svg file.sql out.svg`, the whole dashboard as
one SVG) runs the script's statements in **one DuckDB session, in script
order** — a `CREATE OR REPLACE TEMP VIEW`, `SET VARIABLE` or other setup
statement stays in scope for every later statement, the same session
semantics as `dashboard --check` and a locked server's cold path (#414).
There is no long-lived `duckdb` CLI connection to carry that across the
separate `duckdb` calls the render makes, so it is simulated: a growing
prelude of every setup statement run so far (plus the bound default of any
input above it, as the browser's own first-load pre-pass picks) is replayed
in full before each later statement. `--data <dir>` sets
`getvariable('anofox_data')` to that directory (resolved to an absolute
path) before the script's own statements run, exactly as `dashboard --check
--data` and a locked server's `options.data` do — so a dashboard written
for a locked server (setup views, `-- @requires ${anofox_data}/…`,
`getvariable('anofox_data')` panels) renders to static HTML or SVG without
one running:

```sh
dashboard dashboards/sessions.sql out.html --data /srv/runs/latest
dashboard --svg dashboards/sessions.sql out.svg --data /srv/runs/latest
```

The same binary lints and inspects — useful when authoring (by hand or with an
LLM), since most dashboard failures are **silent** (a broken panel renders
nothing, with no error):

```sh
# Validate: run every statement, report what breaks. Exit 1 on errors.
dashboard --check mydash.sql            # (add --json for machine output)
#   silent-setup  a query with ::ROLE casts outside its main SELECT list
#   sql-error     the query failed         render-error  missing required aesthetic
#   empty-panel   query returned 0 rows (blank card)
#   render-warning  the chart renders but the engine dropped data (non-finite rows, empty stat layer, legend entries that don't fit)
# With an access policy (locked server): what it refuses, which panels use hidden columns.
dashboard --check mydash.sql --db data.duckdb --policy policy.json
#   policy/refused  policy/denied-column  policy/masked-column

# Ground on the data before writing SQL: types, cardinality, min/max, null %.
dashboard --describe 'sales.parquet'    # or a table name, read_csv(...), --db file.db
```

`--check` is the validate step of a **generate → validate → repair** loop, and
doubles as a guardrail before serving a dashboard to consumers.

### b) Browser builder (no server) — interactive analysis

A single-page app: type SQL, **DuckDB-Wasm** runs it in the page, **anofox-visualization
compiled to wasm** renders the panels. Everything is client-side — static files,
no backend, no DuckDB extension.

```sh
wasm-pack build --target web --out-dir web/pkg --profile wasm --no-default-features --features wasm
python3 -m http.server -d web 8000   # then open http://localhost:8000
```

Edit the SQL, press **Run**, hover the marks. Load your own data with DuckDB’s
readers, e.g. `CREATE TABLE t AS SELECT * FROM read_csv_auto('https://…');`.

The app opens in **View** (the dashboard); **✎ Edit** — or `?mode=edit` in the
URL — shows the SQL editor, and makes the dashboard name and logo in the header
editable in place (see *Themes, dark mode, branding and locale*).

**Check** (above the editor) lints the SQL in the page with the same
diagnostics as `dashboard --check`: each statement runs in a scratch session of
DuckDB-Wasm (its own in-memory database, so the page's tables are never
changed; tables you added stay readable). The list shows each finding's
severity, statement and code; a click or Enter selects that statement in the
editor. *Strict* is `--strict` (a `design/*` advisory or a `requires/*` warning
fails the check too) and unticking *Design advice* is `--no-design`. An edit
dims the list until the next Check. On a live backend (`serve`,
`anofox_serve`) Check is not shown yet: use `dashboard --check` there.

**The sidebar.** *My dashboards* comes first: what you saved in this browser
(**Save**), in your own groups. Below it, one **Examples** section groups the
examples by business area: *Start here*, *Demand & stock*, *Logistics &
production*, *Sales & customers*, *Service & operations*, *Data quality* and
*How it works* (the feature tours: charts, inputs, selections and drill,
tables, layout, theming and a de-DE locale). Each area folds open and shut and
shows how many examples it has; *How it works* starts closed. The section is
open while you have no dashboards of your own and closed once you have one;
whatever you choose is remembered in this browser. **Hide** removes it, and
**Show examples** under *My dashboards* brings it back. The toggles are buttons
(`aria-expanded`) that keep the keyboard focus, on a phone too, where the
sidebar is a drawer behind ☰. Long names wrap to two lines; the full name is
the tooltip. Without a link (`?dashboard=` or a Share hash) the builder
reopens the dashboard you had open last in this browser, else your first saved
dashboard, else the first example.

**My dashboards.** **Save** keeps the editor's dashboard under the name in the
toolbar. Saving the dashboard you opened updates it (renames it, when you
changed the name); a name another of your dashboards already has asks
*Replace "…"?* first. The toolbar name and the page title
(`::DASHBOARD_TITLE`, when the SQL has one) are one name: change either and
the other follows. Each dashboard's **⋯** menu has *Rename* (in place: Enter
saves, Esc cancels), *Duplicate* ("… copy", below it), *Move to group…*,
*Download .sql* and *Delete*, which a toast lets you **Undo** for about 8
seconds. A dashboard's group is the `-- @group` line of its SQL: moving it,
renaming a group or deleting one (its dashboards stay, ungrouped) rewrites
that line, so the group travels with the SQL. Groups fold open and shut and
keep their order (drag a group, or Alt+↑/↓ on it); empty ones stay until you
delete them. Dashboards without a group come first; an example you save as
your own starts there too (its library area isn't kept as a group; its
`@tags` are). An example's **⋯** has *Use as a template* (a copy of it in My
dashboards, opened) and *Download .sql*. Questions (a group's name, *Replace
…?*, an import's name clash) come in small dialogs that keep the keyboard
inside until they close.

The header's **⋯** sorts your dashboards — *manual* (drag, or Alt+↑/↓),
*name* or *last edited* — and has *New group…*, *Import .sql / .zip…*,
*Export all as .zip* and *Show / Hide examples*:

- **Export all** writes `my-dashboards.zip`, laid out as a served folder:
  one `<id>.sql` per dashboard, each with `-- @title <name>` (and its
  `@group` / `@tags`), so `anofox_serve_dashboards` or `serve --dashboards`
  serves it unchanged. *Download .sql* gives one such file.
- **Import** takes `.sql` files and `.zip` files (the picker, or drop them on
  the sidebar). A dashboard's name is its `-- @title`, else the file name.
  The zip must be uncompressed ("store", e.g. `zip -0`, or an *Export all*);
  a compressed one is refused with that hint. When a name is taken you
  choose *Keep both* (the import gets "(2)"), *Replace* or *Skip* — once for
  all the clashes, if you like.

**Search and tags.** The field under *New dashboard* (in the phone drawer too)
filters every section at once — your dashboards, *Recently opened* and the
examples — by name, page title, group, tags and owner; every word has to
match, and groups and areas with a match are shown open. Esc clears it. The tag chips
under it are the tags of what is in view: press one to show only dashboards
with that tag (several: all of them); *✓ certified* (shown when something in
view is certified) keeps only dashboards with a `-- @certified` date; × clears
them all. A dashboard's `-- @owner` is a small line under its name, and ✓
marks a certified one. **Recently opened** lists
the last 5 dashboards and examples you opened, per browser.

Everything lives in this browser's `localStorage` (`dp_dashboards`, version
2: `{v, items: {id: {name, sql, updated}}, order, groups, collapsed, sort,
recent, recentOpen}`); a store from before this version is converted on first
load, the original kept as `dp_dashboards_v1`. `?dashboard=my:<id>` opens one
of yours (the id is the slug of the name it was first saved under).

Each example says where it belongs in its own header comments (`-- @title`,
`-- @group`, `-- @tags`, see *Dashboard header comments*), so
`web/examples/*.sql` is the single source of truth for the library. A long,
business-question `-- @title` can carry a shorter `-- @nav` for the sidebar
row — the id, the tooltip and everything below still use the full name.
Open one directly with `?dashboard=<id>`: the id is the slug of its name
(`?dashboard=who-should-sales-call-first`). The name works too, and so do the
names the examples had before they were grouped (`?dashboard=Overview`,
`?dashboard=Chart%20gallery`, …), so old links keep opening the same example.
Examples are looked up first, then your saved dashboards by name;
`?dashboard=my:<id>` opens only one of yours. A linked example shows its area
in the sidebar, even when the section is closed.

Some examples run an anofox extension live in the browser (statistics,
forecast); others (tabular, optimize, decide) read results computed natively,
because those extensions can't run in DuckDB-Wasm (no loadable wasm build, or
a native model runtime): `examples/<ext>-showcase/compute.sql` →
`scripts/gen-showcase-data.sh` → `web/data/<ext>/*.csv`, and the dashboards
only read them.

The live extensions are fetched by `scripts/fetch-wasm-extensions.sh` into
`web/localext/` and loaded on first use; a statement that reads
`'data/<ext>/<file>.csv'` gets that file fetched and registered with
DuckDB-Wasm first.

#### Your name and logo in the app bar

The bar at the top of the builder shows **anofox-visualization** and
*Dashboards from SQL* by default. An installation can put its own name,
tagline and logo there. This is the deployment's bar, not a dashboard's
header: a dashboard still brands itself with `::BRAND`, `::DASHBOARD_TITLE`,
`::LOGO` and `::FAVICON`.

| Setting   | What it does |
|-----------|--------------|
| `name`    | The name in the bar and the browser tab (1–80 characters). |
| `tagline` | The line next to it (up to 160 characters); `""` hides it. Not shown on phones. |
| `logo`    | An image left of the name, 28 px high (24 px on phones). |
| `logo_dark` | The logo while the dark theme is on (🌙, `?theme=dark`, the OS setting); the same image rules and size as `logo`. Needs a `logo`; without one it is ignored. |
| `link`    | Where the name and logo go when clicked (opens in a new tab). |
| `favicon` | The tab icon while the open dashboard sets no `::FAVICON`. |
| `name_visible` | `false` shows the logo alone, for a logo that already spells the name (a wordmark); the name becomes the logo's alt text and stays the tab title. Needs a `logo`, so the bar is never empty. |

`logo`, `logo_dark` and `favicon` take an `https://` (or `http://`) address or a
`data:image/png|jpeg|gif|webp|svg+xml` URI. In `app-config.json` they can also
be a path next to `index.html` (`brand/logo.svg`). `link` must be an
`http(s)://` address. A value that breaks these rules is skipped (the console
says why), so a typo never empties the bar. The text is shown as text, never
as HTML. The browser tab reads `name — tagline` while the open dashboard has
no `::DASHBOARD_TITLE`.

Where the settings come from, later wins:

1. the defaults above;
2. **`app-config.json` next to `index.html`**, for static hosting (GitHub
   Pages, any web server). Copy
   [`web/app-config.example.json`](../web/app-config.example.json) to
   `web/app-config.json` and edit it. The builder loads it at start; without
   the file it uses the defaults. `web/app-config.json` is in `.gitignore`,
   and `serve` and the extension never serve it, even when it was in `web/`
   at build time: a binary is configured by its flags or options only.
3. **the serving process**: `serve --app-name …` (see *b2*) or
   `anofox_serve(port, options)` (see *c*).

```json
{
  "name": "Acme Insights",
  "tagline": "Sales and stock, every morning",
  "logo": "brand/logo.svg",
  "link": "https://intranet.acme.example/"
}
```

The bar stays hidden until the settings are known (at most 1.5 s), so the
default name doesn't flash first. In embed mode (`?embed=1`) and on a locked
server there is no app bar.

`app-config.json` also has an `assistant` block (§4.5): `"remember": true`
opts in to the AI assistant remembering its API key in `localStorage`
across reloads. It is off by default, and always off on a shared static
host (`*.github.io`, `*.gitlab.io`) regardless.

**Self-hosting the static files behind your own web server** (not GitHub
Pages): the app inserts its own `<meta>` Content-Security-Policy when the
assistant is in use (§4.5) — there is nothing to configure for that. If
your server *also* sends a `Content-Security-Policy` response header
(stricter than a meta tag, since it can't be overridden by page content),
make sure it allows at least the same destinations: the app's own
directory, `https://cdn.jsdelivr.net` (DuckDB-Wasm, KaTeX),
`https://extensions.duckdb.org`, `https://api.anthropic.com` (the
assistant's model calls) and `https://earthquake.usgs.gov` (one example's
live feed) — see `web/assist-csp.js`'s `assistCsp()` for the exact,
versioned list.

#### The assistant (off by default)

A panel (✦, or Ctrl/⌘+I) that turns a plain-language request into a
dashboard, and answers follow-up questions about the one you're editing. It
calls Claude directly from the browser with your own API key — never a
DataZooDE server, never any server at all. The sandbox it runs in is §4.5;
this section is the panel itself.

**Enabling it.** Off until `app-config.json` (or `window.__anofoxApp`, for
`serve`/the extension) has an `assistant` block with `"enabled": true`:

```json
"assistant": {
  "enabled": true,
  "transport": "browser-key",
  "models": { "default": "claude-sonnet-5", "harder": "claude-opus-5-5" },
  "sample_rows_allowed": true,
  "max_rounds": 3,
  "remember": false
}
```

| Key | What it does |
|-----|--------------|
| `enabled` | Turns the panel on. Never on a locked server, an embed, a frame, or from a URL parameter or a dashboard's own SQL/comment — only this config. |
| `transport` | `"browser-key"` is the only one in this release (your own key, direct to `api.anthropic.com`). |
| `models` | The two model choices the panel's *Model* select offers ("Standard" / "Try harder"); a malformed id is ignored. |
| `sample_rows_allowed` | Whether the conversation's "Include common values" toggle may be on (it still defaults to the viewer's own choice). |
| `max_rounds` | How many times the assistant may fix and re-check its own proposal before showing it with open issues (1-3). |
| `remember` | Opt-in for the key dialog's *Remember on this device* checkbox (§4.5) — off by default, always off on `*.github.io`/`*.gitlab.io` regardless. |

**What is sent.** Before anything leaves the browser, *What will be sent*
shows: the destination (`api.anthropic.com`), the static reference material's
size, the full dynamic part (your request and the table/column profiles in
scope — names, types, counts, ranges; common values only for text columns,
and never for anything that looks personal), and a token estimate. A
checkbox skips this dialog for the rest of the conversation. Only up to 15
tables go with a profile; the rest go by name only. Secrets in your own SQL
(a `CREATE SECRET`, a `token=`/`password=` in a literal) are replaced by
`***` before they're ever read into the context.

**The key.** Typed once into the key dialog, kept in `sessionStorage`
(gone when the tab closes) unless you tick *Remember* — shown only when the
installation opted in (above). *Forget key* clears it; reload the page to
lift the tab's Content-Security-Policy (§4.5) the key's presence puts in
place.

**The sandbox.** Every tool call, and the proposal's live preview, run in
the locked, second DuckDB-Wasm instance §4.5 describes — never the page's
own. While a proposal is being previewed, the dashboard renders it in place
(a bar above the grid says so); your own SQL in the editor is untouched
until you apply it.

**Preview, apply, undo.** A proposal replaces the dashboard's *view* only:
the preview bar's *Show changes* is a line-level diff against your current
SQL; *Discard* throws the proposal away; *Apply* puts the SQL in the editor
(native undo — Ctrl+Z works, and a toast offers *Undo* for about 8 seconds)
and runs it. When the proposal reads a file or URL your dashboard doesn't
already use, or runs a statement the assistant may not run by itself (an
`INSTALL`, a write, a dynamic query), Apply asks first and lists them —
*Apply and run*, *Apply, don't run* (the SQL goes in, nothing executes until
you press Run), or *Cancel*.

**Phone.** The panel opens as a full-height sheet; once a proposal arrives
it closes and the dashboard (with the fixed preview bar) takes over, with
focus on the preview's heading.

**Limits.** One request at a time (Cancel stops it); a clarifying question
and your answer continue the same exchange, but a fresh Send after a
proposal starts a new one; the sandbox's table snapshot can be sampled or
truncated for a very large table (noted in the profile); links in a preview
don't navigate, and the model's own text is always shown as plain text,
never links, images or HTML.

**The eval.** `tests/assist-eval/` is a fixed 10-prompt seed exercising new
dashboards, edits and two adversarial safety prompts, run live against your
own key (`node scripts/assist-eval.mjs --yes`, about $3 total on Sonnet 5);
see `tests/assist-eval/README.md`. Results aren't committed.

### b2) Serve the UI on a live DuckDB — explore existing data

`anofox-visualization serve` starts a tiny local HTTP server that serves the same builder UI
plus a `/query` endpoint backed by a **live** DuckDB — so the UI operates on your
real tables (big data stays in DuckDB), and opens the browser for you:

```sh
wasm-pack build --target web --out-dir web/pkg --profile wasm --no-default-features --features wasm
cargo build --bin serve --features serve
./target/debug/serve mydata.duckdb          # opens http://127.0.0.1:8080/?token=…
#   --port N     choose the port
#   --no-open    don't launch a browser (open the printed URL yourself)
```

Your name and logo in the app bar (see *Your name and logo in the app bar*):

```sh
./target/debug/serve mydata.duckdb --app-name "Acme Insights" \
    --app-tagline "Sales and stock, every morning" --app-logo brand/logo.svg \
    --app-logo-dark brand/logo-dark.svg \
    --app-link https://intranet.acme.example/   # also --app-favicon FILE|URL
# a logo that spells the name: --app-logo wordmark.svg --app-hide-name
```

`--app-logo`, `--app-logo-dark` and `--app-favicon` take an image file (read once and put in the
page, so nothing else is served) or an `https://` / `data:image/` address.
A file that isn't a PNG, JPEG, GIF, WebP or SVG, or is larger than 512 KB, is
refused with an error. `--app-tagline ""` hides the tagline. The flags style
the authoring builder only; `serve --dashboards` refuses them, because a
locked server shows no app bar.

It runs whatever SQL the builder sends, so it is locked to you: loopback only,
and the printed URL carries a per-run token (kept in a cookie) that every
request needs. See [`secure-serving.md`](secure-serving.md).

The UI **auto-detects**: if a `/query` bridge answers it uses live DuckDB,
otherwise it falls back to DuckDB-Wasm (mode ii). Same editor, same rendering.

#### Explore data

**🔍 Explore data** (sidebar) works on either engine:

- **Catalog**: every table, view and added file with its row and column
  counts. Add a CSV / Parquet / JSON file (pick or drop it; in the browser
  engine) or an `https://` address — each becomes a view. The last table you
  picked comes back next time (per browser).
- **A table at a glance**: one card per column — type, empty %, distinct
  values, a mini histogram (numbers), the top values (text), the first / last
  date and rows per day / week / month (dates), min / p05 / median / mean /
  p95 / max, and data-quality hints (mostly empty, constant, mixed numbers and
  text, extreme values, repeated keys). Above 200,000 rows it profiles a
  100,000-row sample and says so. Picking another table cancels the running
  profile. Below the cards: likely joins (same-named keys whose values match)
  and the rows in the `::PAGED` table.
- **Chart this** on a card previews a histogram, box plot, top-value bars, a
  line over time or a scatter; **Add to dashboard** appends its annotated SQL
  to the dashboard editor. **New dashboard from this table** writes a starter
  dashboard: KPI tiles (an average for a rate, score, price, age, margin,
  coordinate …: whole words of the column name, as the table totals read
  them — `attempts` and `page_views` are summed), the data's latest date
  (`::ASOF`), a `::PERIOD` and a `::BUTTONS` / `::DROPDOWN` filter, a line
  over time, top-N bars, the main measure's distribution and the paged rows.
- **Queries**: the SQL box highlights, completes `::` roles, runs on
  Ctrl/⌘+Enter, keeps your recent queries and shows errors at their line and
  column. A result shows its rows and time, downloads as CSV / Excel /
  Parquet (Parquet in the browser engine), and **Chart this result** picks a
  chart for it. A click on a column name inserts it; a click on a top value
  adds a `WHERE`.

- **Data-quality checks**: when `anofox_tabular` is loaded in the session
  (the native engine; a DuckDB-Wasm build once there is one), each hint offers
  **Run the check** — its `run_checks` check (`null_rate`, `distinct_count`,
  `duplicate_count`, or `compliance` for mixed types and extreme values) —
  and shows the value and pass / fail. The checks table lives in an attached
  in-memory database, so a read-only database works too.
- **Compare…** a table with another table, or two time ranges of it (split in
  halves by default): the row-count change, schema differences (added,
  removed, retyped), per column the empty %, distinct, mean and median shifts
  with both distributions on one scale and, when both sides share a unique
  key, the rows added, removed and changed. **New dashboard from this
  comparison** writes a starter with B against A as `::DELTA`s.

A locked server (`serve --dashboards`, `anofox_serve_dashboards`) has no
Explore page.

### c) DuckDB extension — launch the UI *from a DuckDB session*

The `anofox-visualization` C-API extension (in `duckext/`) adds
`anofox_serve(port)`: start the browser builder wired to the **current**
session, from inside DuckDB.

```sql
LOAD 'anofox_visualization.duckdb_extension';  -- (duckdb -unsigned; see duckext/BUILD.md)
SELECT anofox_serve(8080);                    -- serves http://127.0.0.1:8080/?token=… + opens the browser
SELECT anofox_serve_stop(8080);               -- stop it again
```

`anofox_serve(port, options)` sets the app bar (see *Your name and logo in
the app bar*): a JSON object with `name`, `tagline`, `logo`, `logo_dark`,
`link`, `favicon` and `name_visible` (`true` / `false`). The extension doesn't open files itself; read a logo with DuckDB,
so its file access settings apply:

```sql
SELECT anofox_serve(8080, json_object(
         'name', 'Acme Insights', 'tagline', 'Sales and stock, every morning',
         'logo', 'data:image/svg+xml;base64,' || to_base64(content)))
FROM read_blob('brand/logo.svg');
```

The extension embeds the same UI and answers `/query` on a live connection
(reused serially; loopback + per-server token only), so panels render your
**actual session tables** — big data
stays in DuckDB. `SELECT anofox_render()` also returns an SVG directly. Native
works today; the wasm side-module links + instantiates in DuckDB-Wasm with one
emscripten ABI detail remaining (browsers use **(b)/(b2)** instead).

### d) Serve locked dashboards to consumers (read-only)

`anofox_serve(port)` above is the **authoring** mode — its `/query` runs whatever
the client sends, so it's for you on localhost. To hand a dashboard to *untrusted*
consumers, serve a folder of `.sql` files locked down:

```sql
LOAD 'anofox_visualization.duckdb_extension';
SELECT anofox_serve_dashboards('dashboards', 8095);
-- http://127.0.0.1:8095/       a list of the folder's dashboards
-- http://127.0.0.1:8095/d/<name>  one dashboard, full UI, editor removed
```

Same interactive client, but locked: the editor is gone and **no SQL crosses
the wire** — the client asks for panel *n* of dashboard *id* with variable
*values* (`POST /api/panel`), which the server binds as typed literals. It is
**read-only by construction**: at startup it snapshots the session's databases
into a private temp directory and serves them through a fresh DuckDB instance
with external access, extension loading and configuration changes disabled.
Each request gets its own connection, a row cap and a timeout (optional third
argument: `'{"max_rows": …, "timeout_ms": …, "threads": …, "cache_mb": …}'`).
Panel results are cached per snapshot (`cache_mb`, default 64, `0` = off),
keyed by the variables each panel can depend on — changing a variable re-runs
only the panels that read it, directly or through a `SET VARIABLE` before
them —, identical concurrent requests run once, and a `::PAGED` table that is
paged or scrolled quickly cancels its superseded queries.

**Deployment options.** `bind` sets the listen address (default `127.0.0.1`;
`"0.0.0.0"` in a container or behind a load balancer on another host — the
server warns at start if that happens without `auth`). `memory_limit`
(`"2GB"`, `"50%"`) and `duckdb_threads` set DuckDB's limits for the snapshot,
shared by all requests. Bad values fail when the server starts. Running it
under systemd or in a container, behind a TLS proxy, as several replicas, and
sizing from the load test: [`deployment.md`](deployment.md).

**Fresh data without a restart.** `'{"refresh_s": 300}'` rebuilds the snapshot
every 5 minutes, `SELECT anofox_serve_refresh(8095)` right now; the new
snapshot is swapped in atomically (requests already running finish on the old
one) and the result cache starts over. `/healthz` reports the snapshot's
`epoch` and `snapshot_age_s`.

**A live data folder.** For files too large or too live to snapshot (e.g. one
parquet folder per forecast run), `'{"data": "/srv/runs", "path_vars": ["run"]}'`
lets the panels read that one folder — and nothing else on disk — with the run
picked by a variable:

```sql
SELECT ds::XAXIS, yhat::LINECHART
FROM read_parquet(getvariable('anofox_data') || '/' || getvariable('run') || '/forecast.parquet');
```

`anofox_data` is set by the server; a `path_vars` value must be a plain name
(`[A-Za-z0-9_.-]`, no leading `.`), so `../` never reaches DuckDB, which refuses
any path outside the folder anyway. New runs are readable immediately.
Parquet footers are cached for the snapshot's lifetime (setup views over
large files aren't re-read per request) — in RAM well past what
`memory_limit` counts for them, so size for it ([`deployment.md`](deployment.md)
§6); only a refresh drops the cache. DuckDB re-checks each file's size and
modification time (to the second) before using a cached footer: publish a
run by renaming a new file into place (`mv`), which is always read fresh; a
file overwritten in place at the same size within the same second, or with
its mtime restored, can be read with its old footer until a refresh. With
`data` set, every statement must be a query (or `SET VARIABLE` / `CREATE
TEMP TABLE|VIEW … AS <query>`); mount the folder read-only for a hard
guarantee
([`secure-serving.md`](secure-serving.md)).

**The served nav keeps `path_vars`, and can pick one.** Switching dashboards
in the served nav carries the current value of every `path_vars` variable
along (`/d/other?v.run=2026-10-01`), so every page stays on the same run
without its own `?v.run=` link. `nav_picker` adds one dropdown to the nav
bar, right of the dashboard links, that sets a `path_vars` variable for
every page instead of each dashboard repeating its own `::DROPDOWN`:

```sql
SELECT anofox_serve_dashboards('dashboards', 8095,
  '{"data": "/srv/runs", "path_vars": ["run"],
    "nav_picker": {"var": "run", "label": "Run",
                   "sql": "SELECT run_id, label FROM runs ORDER BY run_id DESC"}}');
```

`nav_picker.sql` is server-side config text (never from a viewer), checked
query-only and run once per snapshot, like a dashboard's own setup; full
rules, including why it refuses to combine with an access policy, are in
[`secure-serving.md`](secure-serving.md).

`nav_pickers` (a list, drawn left to right) draws several of these controls
at once, and adds a `"type": "daterange"` picker — two linked date inputs
plus preset buttons ("last 7d", "last 30d", "last 90d", "all") for a global
time window, e.g. `view_from`/`view_to` carried to every page instead of
each dashboard repeating its own `::DATERANGE`. `nav_picker` (the bare
object above) still works unchanged — a one-element `nav_pickers`. Full
shape, validation and the request-time rules in
[`secure-serving.md`](secure-serving.md).

**Login and per-viewer data.** Behind an auth proxy (oauth2-proxy, Caddy
`forward_auth`, nginx `auth_request`, Cloudflare Access), `options.auth`
(`{"mode": "proxy", "shared_secret_env": "…"}`) makes every request carry the
viewer's identity. The server exposes it to the panels as the reserved
variables `getvariable('__claim_user')` and, if the proxy sends groups and
`header_groups` is set, `getvariable('__claim_groups')`, so a
panel can filter rows by the viewer
(`WHERE region IN (SELECT region FROM entitlements WHERE viewer = getvariable('__claim_user'))`).
Viewers can't set `__claim_*`, and cached results are kept per viewer.
`options.log` writes a JSON-lines request log. With `auth`, pass an access
policy (below) — or `"policy": "none"` to rely on the panels' own claim
filters; otherwise the server refuses to start.
The server can also sign viewers in itself — JWT bearer tokens and an OIDC
login (`"mode": "jwt"` / `"oidc"`), signed embed links for a host application
(`"embed"`), and hashed API keys for scripts (`"api_keys"`); `auth` takes a
list to combine them. Every method sets the same `__claim_*` variables.
`options.audit` logs every login, logout and refused credential.

**Row and column policies.** Instead of a claim filter in every panel, pass
`options.policy` (a path; keep it outside the dashboard folder) or put a
`policy.json` next to the dashboards: every table a
dashboard reads needs a rule — a row condition over the claims and per-column
`deny` / `null` / `mask` / `hash` unless the viewer is in an allowed group.
The server rewrites every statement to read per-viewer views and refuses to
start when a statement could get around them (a table without a rule,
`query()`, catalog functions, macros, database-qualified names, a function
not on the reviewed allowlist, …). Under a policy, viewers see generic panel
errors (details in the server log).

```json
{"tables": {
  "sales": {"rows": "region IN (SELECT region FROM entitlements WHERE viewer = getvariable('__claim_user'))"},
  "employees": {"rows": true, "columns": {"salary": {"allow_groups": ["hr"], "else": "null"}, "ssn": "deny"}}
}}
```

`dashboard --check dash.sql --db data.duckdb --policy policy.json` shows what
the policy refuses and which panels use a column some viewers can't see. Only
the locked server enforces policies: a static HTML export and the browser demo
hold the data in the client and can't. Reference and threat model:
[`secure-serving.md`](secure-serving.md#row-and-column-policies-access-control-p2).

**Multi-page & navigation.** `::TAB` (alias `::PAGE`; `::SUBTAB` nests) makes
pages within a dashboard, deep-linkable via `?tab=<name>`. A `::VIEW` group
switches between a few alternative renderings of one panel in place, without
a new page, deep-linkable via `?view.<id>=<label>` (see *Alternative views of
one panel*). A folder of `.sql` files is a linked set, served with a shared
cross-dashboard nav bar.

**Grouped folders.** The folder stays flat (ids are file names, so the id,
policy and path rules don't change); the header comments group it. The nav
(`window.__served.nav`, items `{id, title, nav, group, tags, owner,
certified}`, only the dashboards the viewer's scope allows) shows the
ungrouped dashboards first, then each `-- @group` under its name, each link
labelled with its `-- @nav` when it has one (else `-- @title`, with the full
title then leading the link's tooltip); a certified one carries ✓, and the
owner, the certification date and the tags are also in the tooltip. Within a
group (and among the ungrouped dashboards), the order is `-- @order
N` ascending; dashboards without it sort after every explicitly ordered one,
alphabetically by id — the server's old (and still the default) behaviour.
This is also the order the start page's cards use, below.
From 9 dashboards on, the nav is one button naming the open dashboard; it
opens the grouped list with a search field (Enter opens it, Esc closes it).
**The landing page.** `/` on the locked server and on `serve --dashboards` is
a landing page in the server's theme (light and dark follow the viewer's
system setting when the theme does; phone widths get one column). Its header
is the installation's name, tagline and logo — `options.app` of
`anofox_serve_dashboards`, `--app-name` / `--app-tagline` / `--app-logo` /
`--app-logo-dark` of `serve --dashboards` — or *Dashboards* and a count. The dashboards are cards,
grouped the same way as the nav (`Sales / EMEA`: an *EMEA* sub-heading under
*Sales*, each group with its count); a card shows the `-- @nav` label when
set — the full `@title` is then the card's tooltip — or just the `@title`
when there is no `@nav` (and no such tooltip), plus the
`@description`, the owner, **✓ certified**, the day its file last changed and
the tags. From 9 dashboards on, a search field, tag chips and *✓ certified
only* filter the cards. It lists only what the viewer's scope allows, like the
nav, and is plain server-rendered HTML (no wasm). To open a dashboard instead
— a run list, say — set `options.home` (`serve --home <id>`): `/` then
redirects there. Before serving a folder, `dashboard --check <folder>` finds
duplicate titles.

For the full trust model, the static-render alternative (`serve` bin), and
deployment (reverse proxy, TLS, auth), see
[`secure-serving.md`](secure-serving.md).

### Telemetry of the anofox extensions

anofox-visualization sends nothing. The anofox extensions whose results it
plots (anofox_statistics, anofox_forecast, anofox_tabular, anofox_optimize,
anofox_decide) send pseudonymous usage telemetry by default, to PostHog EU:
that the extension loaded and which of its functions ran, with call counts —
per their `TELEMETRY.md`, never data, SQL, table or column names. It is
switched off for all five with one variable, checked before anything is sent:

```bash
export DATAZOO_DISABLE_TELEMETRY=1      # 1, true or yes
```

**Off by default wherever anofox-visualization runs** (#333): the
`dashboard` and `serve` binaries, and the `anofox_visualization` extension
itself, set `DATAZOO_DISABLE_TELEMETRY=1` for the process the moment they
start — before any thread or DuckDB instance exists for the binaries, and as
the first thing the extension's own load callback does — *unless the
variable is already set to a non-empty value*. An unset variable, or one set
to `""` or only whitespace, gets the default: a deployment template that
renders the variable empty when nobody actually set it (a compose
`VAR=${VAR}`, a bare systemd `Environment=VAR=`, an unset Helm value) is not
an explicit choice and must not read as opting in. That covers the DuckDB
CLI loading `anofox_visualization` first, `dashboard`, `dashboard serve`
(`serve --dashboards`), and the session that calls
`anofox_serve_dashboards`, including every anofox extension it loads
afterwards in that process (they all check the same variable).

To opt back in, set the variable yourself to a non-empty value other than
`1`/`true`/`yes` — most simply `DATAZOO_DISABLE_TELEMETRY=0` — in the
environment *before* anofox-visualization loads; once it has set the
default, it never overwrites an explicit non-empty value either way. Loading
anofox-visualization into a session that already has telemetry enabled for
some other reason, and that you want to keep that way, needs no action: an
explicit value is always left alone.

**Load order still matters for an anofox extension loaded *before*
anofox-visualization in the same session**: its load event may already have
been reported by the time anofox-visualization's default takes effect (the
env var is read at send time, on a background worker, so it may still be in
time if that send hasn't happened yet — but this isn't guaranteed).
anofox-visualization still runs a best-effort
`SET anofox_telemetry_enabled = false` right after —
only when its own effective default is "disabled" — which silences that
extension (and the locked server's own re-`LOAD` of it into its snapshot) for
the rest of the process, if the setting exists at all (checked via
`duckdb_settings()` first, so this is a silent no-op otherwise); with several
anofox extensions already loaded it only reaches whichever one registered the
setting first (they share the name). `SET anofox_telemetry_enabled = false`
run by hand has the same limits: it comes after `LOAD`, which already reports
the load, and reaches only the first anofox extension loaded. **Load
anofox-visualization first — or set the variable yourself before anything
loads — when that order matters to you.**

**Windows (#333/#335):** setting an environment variable from inside a
process does not update every view of "the environment" the way it does on
Linux/macOS. The OS process environment block (what `SetEnvironmentVariableW`
— and so Rust's `std::env::set_var`, used above — writes) is a separate thing
from each module's own C runtime (CRT) environment cache (what `getenv`
reads); a statically-linked CRT (`/MT`, via `VCPKG_CRT_LINKAGE static`) keeps
a *private* copy per module, snapshotted from the OS block once, when that
module starts. The official DuckDB CLI release and the `csrc/` C++ community
build (built via the `x64-windows-static-release` vcpkg triplet) both use
that static CRT; the Rust-only `duckext` cdylib (the "full toolkit" release
build, a plain `cargo build --release` with no `crt-static`) uses the
*dynamic* UCRT instead — so which of the two linkage models applies depends
on which of anofox-visualization's two Windows builds is asked. Either way, a
module that already finished its own CRT snapshot before
anofox-visualization's default took effect (notably `duckdb.exe` itself,
which starts long before it `LOAD`s anything) can never see the default
through its own `getenv()`, regardless of what anofox-visualization does.
anofox-visualization therefore writes the default through both APIs on
Windows — `SetEnvironmentVariableW` (reaches a static-CRT module `LOAD`ed
*after* it, which snapshots from the then-updated OS block) and the CRT's
own `_wputenv_s` (reaches anofox-visualization's own `getenv()`, and any
module sharing a dynamic UCRT with it) — which is as far as a single module
can reach into every other module's own, independent view of "the
environment". This is also why `SELECT getenv('DATAZOO_DISABLE_TELEMETRY')`
through the DuckDB CLI is not a meaningful check on Windows (it reads
`duckdb.exe`'s own pre-`LOAD` snapshot, not anofox-visualization's write);
`duckext/scripts/build-native.sh`'s Windows smoke test checks through the
extension's own diagnostic function instead (`anofox_telemetry_probe()`,
registered on Windows builds only — not part of the public API).

One consequence for the "load order" paragraph above: on Windows, an anofox
extension loaded *before* anofox-visualization is never reached through the
environment at all — its own CRT table was already snapshotted at its own
load, well before anofox-visualization's write exists, so there is no later
moment (unlike the background-worker send-time read on Linux/macOS) where it
could still pick the write up. The best-effort `SET anofox_telemetry_enabled
= false` is the *only* thing that reaches it on Windows, with the same
first-registrant limit described above.

A locked server (`dashboard serve --dashboards`, `anofox_serve_dashboards`)
sets `lock_configuration` right after a dashboard's `-- @load` lines, so a
dashboard's own SQL can never `SET anofox_telemetry_enabled` back on for the
rest of that session either; the process-wide environment default is what
actually keeps every anofox extension silent regardless of load order. This
repository's CI, tests and scripts force the variable to `1` regardless of
what's inherited, same as any other caller that wants to guarantee it.

The browser builder runs only anofox_statistics and anofox_forecast, and their
DuckDB-Wasm builds have the telemetry compiled out: a visitor's browser sends
nothing for them (`tests/web/wasm_extensions_test.mjs` checks the builds,
`scripts/telemetry-e2e.mjs` every request the demo makes).

anofox_tabular's `pii_scan_table` / `pii_audit_table` download a NER model
from Hugging Face on first use, for names, organisations, locations and misc
entities. The pattern recognizers alone need no download:

```sql
SET anofox_pii_enabled_types = 'EMAIL,PHONE,CREDIT_CARD,US_SSN,IP_ADDRESS,IBAN,DE_TAX_ID,URL,US_PASSPORT,CRYPTO_ADDRESS,UK_NINO,MAC_ADDRESS,API_KEY';
```

---

## 3. Interactivity

Rendered panels carry an SVG `<title>` per mark plus ggplot-rs's hover
metadata: `data-x` (the mark's x), `data-series` (its colour/fill/group level)
and `data-value` (the raw measured value; a stacked segment's own value). Both
the CLI output and the browser builder attach a small hover layer that shows a
styled tooltip (`web: 22`) and highlights the mark; the browser keys its
cross-filter / series highlight / legend toggle on `data-series` and reads the
value from `data-value`, falling back to the `"series: value"` title text only
for marks without them (SVGs from older renderers). Map features carry
`data-row` (the row they were drawn from), `data-series` (their `::CATEGORY`,
else their `::LABEL`), `data-value` (the measure) and `data-label` (the
`::LABEL` text), and a map's root `<svg>` carries `data-map` (see Maps). The root `<svg>` carries the drawn
domain (`data-domain`, `data-xdomain`/`data-ydomain`, `data-xlevels`,
`data-flip`), and `data-hover-focus="none"` (`'none'::HOVER_FOCUS`), which seeds the scroll/drag zoom. It’s pure DOM — no chart
runtime, works on static HTML.

---

## 4. WASM compatibility

The **core** (`anofox-visualization`) has no `polars`/native-only deps; it renders through
ggplot-rs’s plotters-free `render_svg_native`, which compiles to
`wasm32-unknown-unknown`. That’s what makes the browser builder possible:

```
 ┌── browser tab ───────────────────────────────────────────┐
 │  SQL editor                                               │
 │     │ plan(sql)         (anofox-visualization-wasm)       │
 │     ▼                                                     │
 │  DuckDB-Wasm  ──rows──▶  render_panel(rows, roles, …) ─SVG─▶ dashboard
 └───────────────────────────────────────────────────────────┘
```

wasm exports (`src/wasm.rs`):

| Export | Purpose |
|--------|---------|
| `plan(script)` | statements + roles as JSON: `[{setup, sql, roles: [[i, "ROLE", name]]}]` |
| `render_panel(rows_json, roles_json, width, height, primary, zoom_json)` | one panel → SVG. `primary` = brand `rrggbb` (or `""`), or a render context `{"primary","theme","variant","locale","frame"}` (`"frame"`: which `::FRAME` to draw on a time map, 0-indexed — absent/negative/non-integer means the last frame); `zoom_json` = `[x0,x1,y0,y1]` (or `""`). Errors come back as a small error SVG. |
| `resolve_dashboard_theme(spec, chrome_json)` | a theme spec (+ `{title,brand,logo,favicon,locale}`) → `{ok, name, mode, light:{css,…}, dark:{css,…}}` |
| `format_number_locale(value, fmt, locale)` / `format_date_locale(iso, locale)` | KPI / table formatting in a dashboard locale |
| `map_bounds(rows_json, roles_json, width, height)` | the projected window an unzoomed map panel shows (`[x0, x1, y0, y1]`, y up), the zoom UI's start |
| `map_layers(rows_json, roles_json)` | the built-in boundary layers a map panel needs that aren't loaded yet (`["de-districts"]`) |
| `register_boundaries(layer, csv)` | load a boundary layer fetched from `data/boundaries/<layer>.csv`; returns its region count (−1 on a bad name or file) |
| `panel_bounds(…)` | *deprecated* raw data extent of a cartesian panel; the UI now reads the rendered SVG's `data-domain` / `data-flip` attributes (ggplot-rs) and the `zoomable` role set instead |
| `roles_json()` | the role registry + derived role sets (inputs, metrics, directives, table formats, text sizes, zoomable chart kinds — the browser's single source) |
| `format_number(value, fmt)` | KPI/table number formatting shared with the headless renderer |

The SQL parsing in `src/sql.rs` is shared with every native host, so the CLI,
`serve`, the DuckDB extension and the browser behave identically. Every export
is wrapped so failures return an error value; note that on
`wasm32-unknown-unknown` a panic aborts rather than unwinds, so the real
guarantee is that the core never panics on user input (fuzz-tested).

### Host API (Rust)

- `render_spec_checked(json)` also accepts `{"plot": "terms"|"prediction"|"curve"|"summary"|"obs"|"diagnostics"|"auto", "rows": [...], "options": {...}}` — a contract plot over named columns (`contract` module); `macros::MACROS` is the bundled macro table.
- `render_spec_checked(json) -> Result<String, RenderError>` — the checked
  entry point for hosts (the DuckDB extension's FFI): `BadSpec` / `Render` /
  `Panic` (a caught internal panic). `render_spec(json) -> String` keeps the old
  shape (SVG or an escaped `<pre>error</pre>`).
- `render_with(&cols, w, h, &RenderOptions)` — brand colour, zoom window,
  category cap and LTTB threshold passed explicitly (`render(&cols, w, h)` and
  `set_brand`/`set_panel_zoom` remain as compatibility wrappers).
- `render_with_warnings(…) -> Result<Rendered { svg, warnings }, RenderError>`
  — also returns ggplot-rs's build warnings (rows dropped for non-finite
  positions, a layer whose stat produced no data, a top legend too long for
  the chart that ends in "+N more", …): the chart renders, but
  data went missing. Every SVG document (`render_with`, `render_spec*`, the
  wasm `render_panel`) also records them on its root as
  `data-warnings="[…]"` (escaped JSON; absent when there are none; the
  browser logs them to the console), and `dashboard --check` reports each as
  a `render-warning` diagnostic.
- `render_with_at(&cols, x, y, w, h, &RenderOptions)` — a nested,
  positioned `<svg x y width height viewBox>` fragment (no `xmlns`) for
  composing pages; `dashboard::render_dashboard_svg` uses it. Fragments carry
  no warnings (lint with `render_with_warnings`).
- `sql::plan`, `sql::rewrite`, `sql::parse_rows_json` (DuckDB JSON with bare
  `NaN` tolerated), `sql::sanitize_json_numbers`, and the shared lexer
  `sql::lex` (`tokenize`, `split_statements`, `split_top_commas`,
  `strip_comments`, `find_top_level_keyword`, `trailing_cast`, …).
- `roles::REGISTRY`, `roles::lookup`, `Role::token()`, `Role::renders()`,
  `roles::is_directive_panel(&roles)` (does a planned statement draw nothing?),
  `roles::roles_json()`.
- `format::escape_xml` (attribute-safe) and `format::format_number`.

## 4.5 The assistant's sandbox (#204)

The builder's AI assistant (`web/assist.js`, `web/assist-engine.js`,
`web/assist-csp.js` — the panel itself is a separate, later change) never
lets model-written SQL reach the page's own DuckDB-Wasm instance before the
user applies it:

- **What it reads.** A one-time **snapshot**: the page's own tables and
  views, copied in by Arrow, capped by rows and total bytes, with a short
  per-query timeout. Remote-attached databases (MotherDuck included) and
  anything not a plain local DuckDB database are **not included** — the
  assistant is told they exist, never sent their data.
- **What it cannot reach.** The assistant's queries run in a *second*,
  locked DuckDB-Wasm instance: no network, no filesystem beyond the exact
  files the current dashboard already reads, no new extensions beyond a
  fixed set loaded before the lock. This holds independent of any
  function-name list — it is enforced by DuckDB's own
  `enable_external_access=false` + `lock_configuration=true`, re-checked
  before every call. `allowed_paths`' exact-name allow-list grants a
  **write**, not just a read, to that one name — `COPY … TO` an allowed
  path overwrites the engine's own in-memory copy of that file, never the
  page's; the write gate (classifying `COPY` as a statement that needs the
  user's confirmation) is what actually keeps the model from doing this on
  its own, the lock is the backstop.
- **The tab's Content-Security-Policy.** Static hosting sends no headers,
  so the app inserts a `<meta>` CSP: at boot, whenever a browser-stored API
  key is present (it then holds across reloads), and whenever the
  assistant panel opens. While it is in effect, the tab's images, styles
  and document-level network are restricted to the app's own files and a
  short, fixed list of destinations (the model API, the DuckDB-Wasm CDN,
  DuckDB's extension CDN, web fonts). The page's own dashboard rendering —
  including a remote dashboard's data fetches — is unaffected: only
  Workers created *after* the policy is applied are bound by it, and the
  page's DuckDB Worker predates it.
- ***Remember* (localStorage) for the API key.** Off by default; an
  installation opts in with `app-config.json`'s `assistant.remember: true`.
  It is **always off** on a shared static host (`*.github.io`,
  `*.gitlab.io`), because every project on the same account shares that
  storage origin.
- **Verified in Chromium** (the lock, the Worker-inheritance of the CSP,
  the enforcement probe, the Arrow snapshot's types). On other browsers
  the DuckDB-Wasm lock is the layer that holds regardless; the CSP's
  Worker-inheritance is Chromium-specific unless `scripts/assist-sandbox-e2e.mjs`
  has been re-run there too.

## 5. Architecture

```
src/lib.rs        core: Role/Kind/Column + render_with()/render_spec_checked() → SVG
src/roles.rs      the role registry (single source of the ::ROLE vocabulary)
src/sql.rs        ::ROLE + statement planning (shared by every host)
src/sql/lex.rs    the shared SQL lexer
src/format.rs     escaping + number formatting (shared with the browser)
src/downsample.rs input hygiene: NaN, category cap ("Other"), LTTB
src/wasm.rs       wasm-bindgen exports                     (feature = "wasm")
src/bin/…     dashboard CLI runner (duckdb CLI + shared sql module)
web/          no-server browser builder (index.html + app.js + pkg/)
duckext/      DuckDB C-API extension (native + wasm side-module)
```

## 6. Known limitations

- `::MAP` uses `::LABEL` as the per-feature hover label, so a map panel has no
  in-SVG title — use `::TITLE` (the panel title bar) instead.
- Maps take clicks, the keyboard, cross-filter, `::SELECT_AS`, `::DRILL` and
  `::LINK`, but not the brush or the value filter; `::SYNC_ZOOM` (like all
  zooming) is a browser feature — the static renderers draw the full extent.
- `::MARKDOWN`, inputs, downloads, tabs and other directives are interactive
  features of the browser builder / `serve`; the static CLI and headless SVG
  renderers skip them.
- Downsampling applies to line/area/step charts only; scatter plots with very
  many points still draw every point.
- In the browser (wasm), a Rust panic aborts the module instead of returning an
  error; the core is fuzz-tested to never panic on user input.
