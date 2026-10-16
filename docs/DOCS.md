# anofox-visualization — documentation

SQL-defined dashboards: you annotate SQL result columns with
**roles** (`::XAXIS`, `::CATEGORY`, a chart kind on the measure), and anofox-visualization
renders them with [ggplot-rs](https://github.com/sipemu/ggplot-rs). The core is dependency-light and
**wasm-compatible**, so the same renderer runs on the CLI and in the browser.

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
| `::INFO` | `::DESCRIPTION`, `::DEFINITION` | encoding | a definition behind an ⓘ on the panel's title bar (hover, focus or click) |
| `::OPEN` |  | encoding | candlestick open price |
| `::HIGH` |  | encoding | candlestick high price |
| `::LOW` |  | encoding | candlestick low price |
| `::SIZE` |  | encoding | bubble size for a scatter (maps a measure to point area) |
| `::YMIN` | `::Y_MIN` | encoding | lower end of a y interval: a ::SCATTER becomes a pointrange, other measures get error bars |
| `::YMAX` | `::Y_MAX` | encoding | upper end of a y interval (see ::YMIN) |
| `::XMIN` | `::X_MIN` | encoding | lower end of a horizontal interval through each point |
| `::XMAX` | `::X_MAX` | encoding | upper end of a horizontal interval (see ::XMIN) |
| `::FACET` | `::FACET_WRAP`, `::PANEL_BY` | encoding | small multiples: one panel per distinct value, shared axes |
| `::FACET_FREE` | `::FACET_WRAP_FREE` | encoding | small multiples with independent axes per panel (y only on a discrete x) |
| `::RANK` | `::LABEL_RANK` | encoding | score ranking points for ::LABEL_TOP (higher = labelled first) |
| `::CENSOR` | `::CENSORED`, `::N_CENSOR` | encoding | censoring marks (+) on a ::STEP curve where the value is > 0 / true (Kaplan-Meier) |
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
| `::HEATMAP` | `::TILE`, `::TILES` | chart | tiles at XAXIS×YAXIS coloured by the measure (rows in SQL order; diverging around 0) |
| `::CALENDAR` | `::CALENDAR_HEATMAP`, `::CAL_HEATMAP` | chart | calendar heatmap (date XAXIS, ≤ 50 years) |
| `::CANDLESTICK` | `::CANDLE`, `::OHLC` | chart | OHLC candlesticks: XAXIS + ::OPEN/::HIGH/::LOW, close as the measure |
| `::SPARKLINE` | `::SPARK` | chart | minimal trend line (no axes); a list() column in a table |
| `::MAP` | `::GEOMETRY`, `::GEO`, `::CHOROPLETH` | chart | WKT-geometry map, coloured by a measure |
| `::BASEMAP` | `::MAPBASE`, `::BACKDROP` | chart | grey WKT backdrop layer under a ::MAP |
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
| `::YFORMAT` | `::YAXISFORMAT`, `::YUNIT`, `::YCURRENCY` | modifier | value format — y ticks, tooltips, data labels, heatmap legend: '€', '$', 'percent', 'comma', ' kg'… |
| `::XFORMAT` | `::XAXISFORMAT`, `::XUNIT`, `::XCURRENCY` | modifier | x-axis tick format (continuous x), like ::YFORMAT |
| `::XSCALE` | `::XTRANS` | modifier | x-axis transform: 'log10', 'sqrt' or 'reverse' |
| `::YSCALE` | `::YTRANS` | modifier | y-axis transform: 'log10', 'sqrt' or 'reverse' |
| `::FACET_NCOL` | `::NCOL` | modifier | panels per row of a ::FACET / ::FACET_FREE |
| `::SMOOTH_METHOD` | `::METHOD` | modifier | trend line method of a ::SMOOTH: 'loess' (default), 'lm', 'gam' ('glm' = Gaussian lm) |
| `::ALPHA` | `::OPACITY` | modifier | map layer opacity 0..1 |
| `::THRESHOLDS` | `::STATUS_RULES` | modifier | status rules '<0:bad; <0.05:warn; good' — colour a KPI value, gauge zones, chart threshold lines; alone: the next table's 'column: rules \| …' |
| `::RANGE` |  | modifier | gauge domain 'min,max' (default 0,100); bullet band edges '60,80,100' |
| `::COLORS` | `::COLOURS` | modifier | gauge zone colours, comma-separated hex |
| `::LABELS` |  | modifier | gauge zone labels, comma-separated (drawn at each zone) |
| `::ANIMATE` | `::ANIMATION`, `::ENTRANCE` | modifier | entrance animation: 'auto' (lines draw, bars grow, points stagger), 'draw', 'grow', 'reveal' (forward from the ::XLINE, e.g. Today) or 'none' |
| `::STORY` | `::STORYLINE`, `::NARRATIVE` | annotation | per row: a caption → a story step that lights up the row's x (Step / Play controls, the rest dims) |
| `::STORY_BY` | `::STORYBY` | modifier | what a ::STORY step lights up: 'x' (default) or 'series' (the row's ::CATEGORY) |
| `::TRANSITION` | `::TWEEN` | modifier | on an input / cross-filter re-run: 'tween' moves the marks to their new values (≤ 300 ms), 'none' (default) swaps the chart |
| `::LINESTYLE` | `::LINE_STYLE` | modifier | per row, by ::CATEGORY: 'flow' = moving dashes (a forecast, "predicted"; still dashes in static output), 'dashed', 'solid' |
| `::METRIC` | `::KPI`, `::BIGNUMBER` | kpi | big-number KPI (add ::LABEL for a caption) |
| `::MONEY` | `::DOLLAR`, `::CURRENCY` | kpi | currency KPI / table column format |
| `::PERCENT` | `::PCT` | kpi | percent KPI / table column format |
| `::COMPACT` |  | kpi | compact-number KPI / table column format (1.2K) |
| `::DELTA` | `::COMPARE`, `::PREVIOUS` | kpi | comparison value → trend arrow + % on a KPI (up = good) |
| `::DELTA_LOWER_GOOD` | `::COMPARE_LOWER_GOOD`, `::DELTA_INVERSE` | kpi | comparison value where lower is better (churn, cost): a rise is drawn red |
| `::DELTA_FORMAT` | `::DELTAFORMAT` | kpi | how a KPI's change reads: 'pct' (default), 'abs' (in the KPI's format: +$1,200, +2.3 pp) or 'both' |
| `::DELTA_LABEL` | `::DELTALABEL`, `::COMPARE_LABEL` | kpi | what the change compares against, shown after it: 'vs last month', getvariable('p_prev_label') |
| `::TEXT_SMALL` |  | kpi | small text card |
| `::TEXT_MEDIUM` |  | kpi | medium text card |
| `::TEXT_LARGE` |  | kpi | large text card |
| `::MARKDOWN` | `::MD`, `::TEXTBOX`, `::RICHTEXT` | kpi | a Markdown box (browser); {{col}} / {{col:fmt}} fill in values from its own row; $…$ / $$…$$ TeX formulas |
| `::CALLOUT` | `::ADMONITION` | kpi | draw the ::MARKDOWN box as a callout: 'note', 'warning' or 'success' |
| `::TABLE` | `::GRID` | table | the whole result as a table (one marker per panel) |
| `::PAGED` | `::TABLE_PAGED`, `::PAGINATED` | table | SQL-paginated table for large/remote data |
| `::TREND` |  | table | ▲/▼ arrow in a table cell |
| `::COLORSCALE` | `::COLOURSCALE`, `::HEAT`, `::GRADIENT` | table | heatmap-colour a table column's cells (low = green, high = red) |
| `::COLORSCALE_HIGH_GOOD` | `::COLORSCALE_HIGH`, `::HEAT_HIGH` | table | heatmap-colour a table column's cells, high = green (scores, accuracy) |
| `::COLORSCALE_DIVERGING` | `::DIVERGING`, `::HEAT_DIVERGING` | table | colour a table column's cells around zero: negative red, positive green (deltas) |
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
| `::HINT` |  | input | per-option hint next to a dropdown option |
| `::COLUMNS` | `::COLS` | layout | default panels per row |
| `::SPAN` | `::COL`, `::WIDTH` | layout | next panel's width (of 12) |
| `::HEIGHT` | `::TALL` | layout | next panel's height in px |
| `::ROWSELECT` | `::ROW_SELECT`, `::SELECTABLE` | layout | next table's row selection: 'none' (default), 'single' or 'multi' — a click sets getvariable('<first column>') |
| `::TOTALS` | `::TOTAL`, `::AGGREGATE` | layout | next table's totals: 'sum'\|'mean'\|'median'\|'min'\|'max'\|'count'\|'none', or per column 'Revenue: sum, Margin: mean' |
| `::COLGROUP` | `::COL_GROUP`, `::HEADER_GROUP` | layout | next table's grouped header: 'Backtest: mae, rmse \| Holdout: mae_h, rmse_h' |
| `::FLAG` | `::THRESHOLD` | layout | next table: flag cells meeting a condition with ⚠ — 'mape > 0.2, bias < -5' |
| `::HIGHLIGHT` | `::TOPN` | layout | next table: emphasise the top / bottom n of a column — 'Revenue: top 3, Margin: bottom 2' |
| `::SUBTOTAL` | `::SUBTOTALS`, `::GROUP_TOTAL` | layout | next table: a subtotal row after each group of a column — 'region' (not for ::PAGED) |
| `::SELECT_AS` | `::SELECTAS`, `::SELECTION` | layout | next chart's named selection: 'region' → a click sets getvariable('region'); 'region multi' → a list (shift/ctrl-click adds); a brush on a continuous x sets region_from / region_to |
| `::EMPTY` | `::EMPTY_MESSAGE`, `::NODATA` | layout | next panel's message when its result is empty: 'No orders in range — widen the dates' |
| `::DRILL` | `::DRILLDOWN`, `::DRILL_DOWN` | layout | next chart drills down 'region > country > city' ('geo: …' names the variables): drill_level / drill_depth / drill_path + a variable per level, a breadcrumb back |
| `::GROUP` | `::BOX`, `::ROW` | layout | open a box; ::ENDGROUP closes it |
| `::ENDGROUP` | `::ENDBOX`, `::ENDROW` | layout | close the current box |
| `::TAB` | `::PAGE` | layout | start a tab/page |
| `::SUBTAB` | `::SUB_TAB` | layout | a nested tab inside a ::TAB |
| `::PLACEHOLDER` |  | layout | an empty grid cell |
| `::TRACK` | `::STRIP_BELOW`, `::STRIP_UNDER` | layout | a thin panel under the previous chart, sharing its x axis (value = its name): ticks, ::HEATMAP tiles per ::YAXIS lane, or a line / bar |
| `::RELOAD` | `::REFRESH` | chrome | auto-refresh interval (seconds) |
| `::HEADER_IMAGE` | `::HEADERIMAGE` | chrome | banner image URL |
| `::FOOTER_LINK` | `::FOOTERLINK` | chrome | link at the bottom |
| `::SECTION` | `::SECTION_HEADING` | layout | a section heading that always starts a new row |
| `::THEME` |  | chrome | dashboard theme: 'light'/'dark'/'print'/'colorblind'/'high-contrast', a stack 'acme+dark', or JSON |
| `::LOCALE` |  | chrome | number/date locale for KPIs, tables and axes: 'en-US', 'en-GB', 'de-DE', 'fr-FR' |
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

Inputs work in the **browser builder** and **`serve`** (they re-query on change).
The static outputs (`dashboard file.sql`, `dashboard --svg`, the view-only
`serve --dashboards` page) draw no control but bind each input's **default**
(the first option, every option of a `::MULTISELECT`, both ends of a
`::DATERANGE`, the default period of a `::PERIOD`), so `getvariable()` panels
render as on first load in the browser.

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

Dodged marks keep their discrete x level in `data-x`. The one exception is a
grouped chart with `::DATALABELS` (labels cannot follow a discrete dodge), drawn
on numeric x slots: its SVG root carries `data-xticks` (`{"1":"W1",…}`) so hosts
can map a mark's numeric `data-x` back to its level.

For whole model outputs there are ready-made plot macros — see
[Plotting model output](#plotting-model-output-contract-macros).

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
| `anofox_plot_curve(tbl, base_rate := NULL)` | **curve_type, x, y**, y_low, y_high, model_id \| series \| id, threshold, weight, auc, n_censor \| censored, n_risk, n_event (km), is_min, is_1se (lambda_cv) | roc: square, diagonal, the AUC (trapezoid, or an `auc` column) in each legend label; pr: the recall-0 points trimmed, a dashed base-rate line (`base_rate :=`, else the precision at the lowest threshold with full recall); calibration: square, diagonal, point size by the bin count (`weight`); km: step curves from S(0) = 1, step-ribbon band, small `+` censor marks, a dashed 50 % line with drops at each stratum's median and a number-at-risk table under the curves; acf/pacf: lollipops without lag 0 (ACF), significance bounds (one panel per series); lambda_cv: CV error ± SE on a log-λ axis with λ.min / λ.1se; qq / pdp / lift / null_dist / lowess: line or points (+ band). One `curve_type` per call (`GROUP BY curve_type`) |
| `anofox_plot_summary(tbl, metrics := NULL, baseline := 'range')` | **metric, value**, model_id \| model_name, id, conf_low, conf_high | a grid of small panels, one per metric with its own scale; one model order in every panel (best first by the first metric), the value beside each dot, "n/a" for a missing metric, the best model highlighted for metrics with a direction; `baseline := 'range'` zooms to the values and prints the range ("Δ = …") in the panel title, `'zero'` starts the axis at 0; n_observations / n_features / iterations only when `metrics := [...]` asks |
| `anofox_plot_tests(tbl, alpha := 0.05, value := NULL, label := 'p')` | **p_value \| p_adj \| statistic**, test_id, group1, group2, estimate, conf_low, conf_high, method | comparison forest: one row per test (pairs as "B − A"), x "difference (95% CI)" (else the statistic, else −log10 p; `value :=` forces one), coloured by significance at `alpha`, `p = … **` beside each row (`label := 'stars'`: stars only). Estimates without any interval (pairwise_t_test_by today) fall back to −log10(p) bars with a labelled significance line, said in the axis label. Several groups get a panel each. A column holding a `*_agg` STRUCT / LIST is expanded |
| `anofox_plot_compare(tbl, value :=, factor :=, tests := NULL, kind := 'box', hide_ns := NULL, label := 'stars')` | the raw rows: the **value** and **factor** columns named by the arguments | box / violin / points per group with significance brackets from `tests :=` (a LIST of test STRUCTs: `(SELECT list(t) FROM tukey_hsd_by(…) t)` or `tukey_hsd_agg(y, g)`; one two-sample STRUCT for two groups); `hide_ns` defaults to true above 3 groups; `label := 'stars' \| 'p'` or a template (`'p = {p.adj}'`, `'{p.signif}'`) |
| `anofox_plot_partial(tbl, kind := 'component')` | **term, x, partial_residual** (component), or **term, x_residual, y_residual** (added_variable) | one panel per term: component + residual (points, component line, loess — a binary / ≤ 3-level term gets jittered points and the group means instead) or added-variable (points + least-squares line through the origin, slope in the title) |
| `anofox_plot_obs(tbl, kind := 'resid_fitted', label_top := 3)` | **fitted, residual**, row_id, std_residual, leverage, cooks_d, n_params | one diagnostic: `resid_fitted`, `qq`, `scale_location`, `leverage` (Cook's distance contours 0.5 and 1 from `n_params`, both signs in view), `cooks` (index plot, the 4/n line labelled, top rows labelled above their stems); the `label_top` largest-Cook's rows are labelled by `row_id` |
| `anofox_plot_diagnostics(tbl, label_top := 3)` | as `obs` | the plot.lm 2×2 as one SVG (a ggplot-rs `PlotGrid`: root `data-grid="2 2"`, each sub-plot a nested `<svg data-panel="i">`; one legend when several models are compared) |
| `anofox_plot_events(tbl, x := NULL)` | **ds \| period**, kind, label, severity, value, threshold, model_id, series | a timeline: one lane per series (else per kind), a dot per event coloured by severity (alarm red, warning amber, else grey; a legend when there are several), kind · label · value (threshold …) on hover. Width 640, height 260 |
| `anofox_plot(tbl)` | any of the above | picks the plot from the columns: term+estimate → terms, curve_type → curve, yhat → prediction, fitted+residual → diagnostics, metric+value → summary, partial_residual → partial, p_value / test_id / statistic+method → tests, kind \| severity with ds \| period → events. Errors name the contract version (`anofox_contract_version()` = `'1'`) |

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
| `id` | `unique_id` |
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
best model: higher is better for R², adjusted / pseudo R², AUC (`roc_auc`),
accuracy, balanced accuracy, F1, precision, recall and log-likelihood; lower
for RMSE, MSE, MAE, MAPE, sMAPE, MASE, RMSSE, CRPS, Brier score, log loss,
residual SE, sigma, AIC, AICc, BIC and deviance (names compared without case).
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
  missing estimate (did you mean \`estimat\`?)*;
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
| `roc_curve_by`, `pr_curve_by`, `calibration_curve_by`, `kaplan_meier_by`, `acf_by`, `pacf_by`, `ridge_cv_by`, `elasticnet_cv_by` | curve | `anofox_plot_curve(c)` — one `curve_type` per call |
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
                       split := CASE WHEN id > 50 THEN 'test' END) p;
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
(see the README).

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
- **Labels** (`::DATALABELS`): the short value (`$1,235`, `12K`, `1,2 Mio.` in
  `de-DE`), dark text on light cells and light text on dark ones; the font size
  fits the cell unless the role's value sets it.
- **Hover / click**: the tooltip shows the column (header), the row and the
  formatted value; a click cross-filters like any chart — it sets
  `getvariable('selected')` to the cell's **row** (`::YAXIS`) value.
- `::CALENDAR` uses the same ramp, card-coloured cell gaps and formatted
  tooltips.

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
`::MARKDOWN` text (`callout/no-text`). Like `::MARKDOWN`, callouts are drawn by the
browser (builder, `serve`, locked servers). The *A sales cockpit in your brand*
sample has both.

### Formatted tooltips

Hover tooltips show values the way the panel formats them: its `::YFORMAT`
(`'$'`, `'€'`, `'percent'`, `'comma'`, `' kg'`, `'CHF '` — the same keywords as
the axis) in the dashboard `::LOCALE`, with cents kept for currency
(`$1,234.50`, `1.234,50 €`). Share charts (`::BARCHART_PERCENT`,
`::BAR_STACKED_PERCENT`, a fractional `::LINECHART_PERCENT`) read as percent.
The x value follows `::XFORMAT`, and ISO dates take the locale's date order
(`01.03.2024`) when a `::LOCALE` is set. Data labels (`::DATALABELS`) use the
same format in short form. The renderer writes these as `data-fvalue` /
`data-fx` next to the raw `data-value` / `data-x` on each mark, so any host can
show them; selection and filtering keep using the raw values.

### Combo charts, auto-refresh

A panel with **multiple measure columns** overlays them as combo layers, e.g.
`SELECT week::XAXIS, sessions::BARCHART, revenue::LINECHART`. The toolbar also has
an **auto-refresh** interval.

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

**The credit.** "Powered by DataZoo GmbH" is shown on every dashboard, in every
output and under every theme (it takes the theme's colours so it stays
readable). Author branding is shown *in addition*; there is no role to hide or
replace the credit.

**Locale** (`::LOCALE`, or the theme's `locale`; `en-US` by default): `en-US`,
`en-GB`, `de-DE`, `fr-FR` (other regions fall back to their language). It
formats KPI tiles, table cells (numbers and ISO dates) and chart axes /
legends / month names identically in the browser and the CLI:

| | en-US | de-DE | fr-FR |
|---|---|---|---|
| `::METRIC` 1234567.5 | 1,234,567.5 | 1.234.567,5 | 1 234 567,5 |
| `::MONEY` 12400 | $12,400 | 12.400 € | 12 400 € |
| `::PERCENT` 12.5 | 12.5% | 12,5 % | 12,5 % |
| `::COMPACT` 1234567 | 1.2M | 1,2 Mio. | 1,2 M |
| date 2026-10-15 (tables) | 2026-10-15 (unchanged) | 15.10.2026 | 15/10/2026 |
| axis | `1,000` · `Oct 2026` | `1.000` · `Okt. 2026` | `1 000` · `oct. 2026` |

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
written with one). The chip shows the time in the viewer's zone and the
dashboard locale (the date is added when it isn't today) and keeps its relative
age current while the page is open. It is in every output: the browser, a
locked server, the static HTML of `dashboard file.sql out.html` /
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
- **Charts as tables.** The *▦ Charts as tables* switch at the top right of
  every dashboard (also in embed and served views; `?tables=1` turns it on, the
  choice is remembered) swaps every chart for its data table.
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
  file (interactive hover + tabs, no server).

### Layout (from SQL)

The grid is a **12-column bootstrap grid**; panel widths are spans out of 12.
Directives (browser builder / `serve`; the CLI skips them):

| Directive | Effect |
|-----------|--------|
| `SELECT n::COL;` (`::SPAN`, `::WIDTH`) | the **next** panel's width — `n` of 12 (`12`=full, `6`=half, `4`=third) |
| `SELECT n::COLUMNS;` | default panels per row (each unspecified panel spans `12/n`) |
| `SELECT 'Title'::GROUP;` … `SELECT 1::ENDGROUP;` | wrap the enclosed controls/charts in one box (a flex row) |
| `SELECT 'Name'::TAB;` | start a tab — following panels live under it (panels before the first `::TAB` form a fixed header) |
| `SELECT 'Name'::SUBTAB;` | start a nested tab inside the current `::TAB` |
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
  comment line directly above the statement (several: comma-separated):

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
  `dashboard --check` checks the paths with DuckDB's `glob()` and reports a
  missing one as a **note** (`optional/missing`), not an error — `--strict`
  still passes.

### Interactivity

Every chart is hoverable (bars/points/areas carry per-mark tooltips; lines get
point markers and dim as whole lines). **Linked highlighting**: click any
series/bar to highlight it across all panels and dim the rest — click empty
space (or the mark again) to clear. Series colours are **consistent across
charts**.

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
before it (tables without one are not clickable):

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

#### Viewer state in the URL

The URL carries the view: `?tab=<name>` opens a tab and `?v.<name>=<value>`
seeds a variable — an input's value, the shared `selected`, or a named
selection. After every change the address bar is rewritten (no reload) with the
inputs that differ from their defaults and the live selections, so a reload, a
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

A value that fails its input's check is ignored (the input keeps its default).
Only variable **values** travel, never SQL: on a locked server
(`anofox_serve_dashboards`, `/d/<id>?v.region=EU`) they are bound as typed
literals like any input change, so an embedding app or another dashboard can
open a dashboard pre-filtered. Example:
`?dashboard=Filters%20%26%20inputs&tab=Cross-filter&v.sku=SKU-A&v.market=EU`.

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
over every matching row, not just the page: sums of amounts and counts, means of
rates, shares and scores, counts of badge values — each numeric total carries a
small tag, e.g. "sum ▾", to switch it to mean, median, min, max or count; the
choice is kept per table), **Stats** (a strip under the header: a 10-bin
histogram with min and max for each number column, the top 3 values with their
share for the others, "all N values distinct" for ids and names, empty counts —
over the matching rows), **Columns** (show / hide),
**Pages | Scroll**, **Compact | Comfortable** row density (compact by default
above 50 rows) and **⤓ Export**: CSV (the matching rows, visible columns, raw
values) or Excel (the same rows as an `.xlsx` that reads like the table: the headers
with their units, the grouped header, each column's number format — scaled units
keep the raw value, e.g. `#,##0.00,,` shows 5,350,000 as 5.35 — dates as dates,
subtotal and totals rows as live `SUBTOTAL()` formulas, a frozen header with a
filter; a `::PAGED` table exports the loaded rows and its SQL totals as values).
`::DOWNLOAD_XLSX` writes the raw query rows as a real `.xlsx` (numbers, dates and
booleans typed). Hovering a
header shows the column's min / max / mean, empty and distinct counts.

Formatting (one set of rules, `src/table.rs`, for the browser table and the
headless `dashboard --svg` / static HTML table — which also right-aligns
numbers, moves units into the header and adds the default totals row): raw column names read as headers (`load_pct` → "Load %", `avg_mae` →
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
| `::TOTALS` (`::TOTAL`, `::AGGREGATE`) | `'sum'` \| `'mean'` \| `'median'` \| `'min'` \| `'max'` \| `'count'` \| `'none'`, or `'Revenue: sum, Margin: mean, Score: median'` | the totals row's aggregation for every numeric column, or per column (a bare value inside the list sets the rest); `'none'` hides the row and its button. Without it: sums of amounts and counts, means of rates / shares / scores. The viewer's pick in a cell wins. |
| `::COLGROUP` (`::COL_GROUP`, `::HEADER_GROUP`) | `'Backtest: mae, rmse \| Holdout: mae_h, rmse_h'` | a header row with a label over adjacent columns (a thin rule under each label) |
| `::FLAG` (`::THRESHOLD`) | `'mape > 0.2, bias < -5, status = breach'` | cells meeting a condition (`>` `>=` `<` `<=` `=` `!=`) get a ⚠ and the warning tint — never colour alone |
| `::HIGHLIGHT` (`::TOPN`) | `'Revenue: top 3, Margin: bottom 2'` (`'top 3'` alone: the first numeric column) | the top / bottom n values in bold with their rank (#1, #2 / ↓1, ↓2); a `::PAGED` table ranks within the page |
| `::SUBTOTAL` (`::SUBTOTALS`, `::GROUP_TOTAL`) | `'region'` | the rows grouped by that column (groups in the order of their first row; sorting by the column orders the groups, any other column sorts within them), the group's value shown once, and a subtotal row after each group — each numeric column with the aggregation its total uses. A `::TABLE` only (a `::PAGED` table holds one page). |

```sql
SELECT 'revenue: sum, margin: mean, accuracy: median'::TOTALS;
SELECT 'Money: plan, revenue | Quality: accuracy, bias'::COLGROUP;
SELECT 'margin < 12'::FLAG;
SELECT 'revenue: top 2'::HIGHLIGHT;
SELECT region::TABLE, plan::MONEY, revenue::MONEY, margin,
       accuracy::COLORSCALE_HIGH_GOOD, bias::COLORSCALE_DIVERGING
FROM regions;
```

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
  body or subquery are ordinary SQL.
- **A `::ROLE` may follow an `AS` alias on any column.** The alias is replaced by
  the internal `c{i}` name. For a measure it becomes the legend name of a combo
  chart; for an input it *is* the DuckDB variable name
  (`SELECT region AS zone ::DROPDOWN` → `getvariable('zone')`); in a
  `::TABLE`/`::PAGED`/`::DOWNLOAD_*` panel it is the column header.
- **Role tokens that are also SQL types** — `::DATE`, `::TEXT`, `::STRING`,
  `::NUMERIC` (inputs) and `::MAP`, `::GEOMETRY` (maps). In a query statement
  `::MAP`/`::GEOMETRY` are always the map role. The input tokens are roles only
  in an **input statement** (no other role except `::HINT`/`::LABEL`/`::TITLE`,
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

- **Discrete axes are capped**: more than 30 levels on a bar/box/violin/jitter
  x axis or in a `::CATEGORY` keep the largest 29 (by total |measure|, or row
  count for raw-row charts) and fold the rest into **"Other"** (summed for
  bars/lines/areas/pies). Hosts can change it (`max_categories` in a
  `render_spec` JSON, `RenderOptions::max_categories`; `0` disables).
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

The same binary lints and inspects — useful when authoring (by hand or with an
LLM), since most dashboard failures are **silent** (a broken panel renders
nothing, with no error):

```sh
# Validate: run every statement, report what breaks. Exit 1 on errors.
dashboard --check mydash.sql            # (add --json for machine output)
#   silent-setup  a query with ::ROLE casts outside its main SELECT list
#   sql-error     the query failed         render-error  missing required aesthetic
#   empty-panel   query returned 0 rows (blank card)
#   render-warning  the chart renders but the engine dropped data (non-finite rows, empty stat layer)

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
wasm-pack build --target web --out-dir web/pkg --no-default-features --features wasm
python3 -m http.server -d web 8000   # then open http://localhost:8000
```

Edit the SQL, press **Run**, hover the marks. Load your own data with DuckDB’s
readers, e.g. `CREATE TABLE t AS SELECT * FROM read_csv_auto('https://…');`.

The app opens in **View** (the dashboard); **✎ Edit** — or `?mode=edit` in the
URL — shows the SQL editor, and makes the dashboard name and logo in the header
editable in place (see *Themes, dark mode, branding and locale*).

**Examples in the sidebar** (open one directly with `?dashboard=<name>`):

- **Examples** — the core feature tour: *Start here*, *Charts*,
  *Interactivity*, *Tables*, *Layout* and *Theming* ("A sales cockpit in your
  brand", "A German branch report"; sources in `examples/theming/`).
- **anofox extensions** — one group per anofox DuckDB extension:
  - *Statistics* — "What sets the price of a home?", computed live by the
    anofox_statistics wasm extension;
  - *Forecast* — "How much will each item sell next year?" and "Which forecast
    method fits our demand?", live anofox_forecast;
  - *Tabular*, *Optimize*, *Decide* — one dashboard per business question
    (e.g. "Can we switch on the new CRM?", "Which van serves which stop
    tomorrow?", "Can a model triage the support inbox?"); the sidebar names
    are the questions, each dashboard says on the page that its results are
    stored. These extensions can't run in DuckDB-Wasm (no loadable wasm
    build, or a native model runtime), so the results were computed natively
    (`examples/<ext>-showcase/compute.sql` → `scripts/gen-showcase-data.sh` →
    `web/data/<ext>/*.csv`) and the dashboards only read them — each page says so.

The live extensions are fetched by `scripts/fetch-wasm-extensions.sh` into
`web/localext/` and loaded on first use; a statement that reads
`'data/<ext>/<file>.csv'` gets that file fetched and registered with
DuckDB-Wasm first.

### b2) Serve the UI on a live DuckDB — explore existing data

`anofox-visualization serve` starts a tiny local HTTP server that serves the same builder UI
plus a `/query` endpoint backed by a **live** DuckDB — so the UI operates on your
real tables (big data stays in DuckDB), and opens the browser for you:

```sh
wasm-pack build --target web --out-dir web/pkg --no-default-features --features wasm
cargo build --bin serve --features serve
./target/debug/serve mydata.duckdb          # opens http://127.0.0.1:8080/?token=…
#   --port N     choose the port
#   --no-open    don't launch a browser (open the printed URL yourself)
```

It runs whatever SQL the builder sends, so it is locked to you: loopback only,
and the printed URL carries a per-run token (kept in a cookie) that every
request needs. See [`secure-serving.md`](secure-serving.md).

The UI **auto-detects**: if a `/query` bridge answers it uses live DuckDB,
otherwise it falls back to DuckDB-Wasm (mode ii). Same editor, same rendering.

### c) DuckDB extension — launch the UI *from a DuckDB session*

The `anofox-visualization` C-API extension (in `duckext/`) adds
`anofox_serve(port)`: start the browser builder wired to the **current**
session, from inside DuckDB.

```sql
LOAD 'anofox_visualization.duckdb_extension';  -- (duckdb -unsigned; see duckext/BUILD.md)
SELECT anofox_serve(8080);                    -- serves http://127.0.0.1:8080/?token=… + opens the browser
SELECT anofox_serve_stop(8080);               -- stop it again
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
identical concurrent requests run once, and a `::PAGED` table that is paged or
scrolled quickly cancels its superseded queries.

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

**Login and per-viewer data.** Behind an auth proxy (oauth2-proxy, Caddy
`forward_auth`, nginx `auth_request`, Cloudflare Access), `options.auth`
(`{"mode": "proxy", "shared_secret_env": "…"}`) makes every request carry the
viewer's identity. The server exposes it to the panels as the reserved
variables `getvariable('__claim_user')` and, if the proxy sends groups and
`header_groups` is set, `getvariable('__claim_groups')`, so a
panel can filter rows by the viewer
(`WHERE region IN (SELECT region FROM entitlements WHERE viewer = getvariable('__claim_user'))`).
Viewers can't set `__claim_*`, and cached results are kept per viewer.
`options.log` writes a JSON-lines request log.

**Multi-page & navigation.** `::TAB` (alias `::PAGE`; `::SUBTAB` nests) makes
pages within a dashboard, deep-linkable via `?tab=<name>`. A folder of `.sql`
files is a linked set, served with a shared cross-dashboard nav bar.

For the full trust model, the static-render alternative (`serve` bin), and
deployment (reverse proxy, TLS, auth), see
[`secure-serving.md`](secure-serving.md).

---

## 3. Interactivity

Rendered panels carry an SVG `<title>` per mark plus ggplot-rs's hover
metadata: `data-x` (the mark's x), `data-series` (its colour/fill/group level)
and `data-value` (the raw measured value; a stacked segment's own value). Both
the CLI output and the browser builder attach a small hover layer that shows a
styled tooltip (`web: 22`) and highlights the mark; the browser keys its
cross-filter / series highlight / legend toggle on `data-series` and reads the
value from `data-value`, falling back to the `"series: value"` title text only
for marks without them (map features). The root `<svg>` carries the drawn
domain (`data-domain`, `data-xdomain`/`data-ydomain`, `data-xlevels`,
`data-flip`), which seeds the scroll/drag zoom. It’s pure DOM — no chart
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
| `render_panel(rows_json, roles_json, width, height, primary, zoom_json)` | one panel → SVG. `primary` = brand `rrggbb` (or `""`), or a render context `{"primary","theme","variant","locale"}`; `zoom_json` = `[x0,x1,y0,y1]` (or `""`). Errors come back as a small error SVG. |
| `resolve_dashboard_theme(spec, chrome_json)` | a theme spec (+ `{title,brand,logo,favicon,locale}`) → `{ok, name, mode, light:{css,…}, dark:{css,…}}` |
| `format_number_locale(value, fmt, locale)` / `format_date_locale(iso, locale)` | KPI / table formatting in a dashboard locale |
| `map_bounds(rows_json, roles_json)` | a map's lon/lat extent for the zoom UI |
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
  positions, a layer whose stat produced no data, …): the chart renders, but
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
- `::MARKDOWN`, inputs, downloads, tabs and other directives are interactive
  features of the browser builder / `serve`; the static CLI and headless SVG
  renderers skip them.
- Downsampling applies to line/area/step charts only; scatter plots with very
  many points still draw every point.
- In the browser (wasm), a Rust panic aborts the module instead of returning an
  error; the core is fuzz-tested to never panic on user input.
