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
| `::REFLINE` | `::TARGET`, `::GOAL`, `::YLINE` | annotation | horizontal reference line per distinct value |
| `::XLINE` |  | annotation | vertical reference line at an x |
| `::COOKS_CONTOUR` | `::COOKS` | annotation | Cook's distance contours (0.5, 1) on residuals vs leverage; value = the model's parameter count p |
| `::BAND_LOWER` | `::BANDLOWER` | annotation | lower edge of a shaded band around a line |
| `::BAND_UPPER` | `::BANDUPPER` | annotation | upper edge of a shaded band |
| `::MARKAREA` | `::MARK_AREA`, `::SHADE` | annotation | shade the x-region [min, max] of this column |
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
| `::RANGE` |  | modifier | gauge domain 'min,max' (default 0,100) |
| `::COLORS` | `::COLOURS` | modifier | gauge zone colours, comma-separated hex |
| `::LABELS` |  | modifier | gauge zone labels, comma-separated (drawn at each zone) |
| `::METRIC` | `::KPI`, `::BIGNUMBER` | kpi | big-number KPI (add ::LABEL for a caption) |
| `::MONEY` | `::DOLLAR`, `::CURRENCY` | kpi | currency KPI / table column format |
| `::PERCENT` | `::PCT` | kpi | percent KPI / table column format |
| `::COMPACT` |  | kpi | compact-number KPI / table column format (1.2K) |
| `::DELTA` | `::COMPARE`, `::PREVIOUS` | kpi | comparison value → trend arrow + % on a KPI |
| `::TEXT_SMALL` |  | kpi | small text card |
| `::TEXT_MEDIUM` |  | kpi | medium text card |
| `::TEXT_LARGE` |  | kpi | large text card |
| `::MARKDOWN` | `::MD`, `::TEXTBOX`, `::RICHTEXT` | kpi | a Markdown box (browser) |
| `::TABLE` | `::GRID` | table | the whole result as a table (one marker per panel) |
| `::PAGED` | `::TABLE_PAGED`, `::PAGINATED` | table | SQL-paginated table for large/remote data |
| `::TREND` |  | table | ▲/▼ arrow in a table cell |
| `::COLORSCALE` | `::COLOURSCALE`, `::HEAT`, `::GRADIENT` | table | heatmap-colour a table column's cells (low = green, high = red) |
| `::COLORSCALE_HIGH_GOOD` | `::COLORSCALE_HIGH`, `::HEAT_HIGH` | table | heatmap-colour a table column's cells, high = green (scores, accuracy) |
| `::COLORSCALE_DIVERGING` | `::DIVERGING`, `::HEAT_DIVERGING` | table | colour a table column's cells around zero: negative red, positive green (deltas) |
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
| `::GROUP` | `::BOX`, `::ROW` | layout | open a box; ::ENDGROUP closes it |
| `::ENDGROUP` | `::ENDBOX`, `::ENDROW` | layout | close the current box |
| `::TAB` | `::PAGE` | layout | start a tab/page |
| `::SUBTAB` | `::SUB_TAB` | layout | a nested tab inside a ::TAB |
| `::PLACEHOLDER` |  | layout | an empty grid cell |
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

Inputs work in the **browser builder** and **`serve`** (they re-query on change);
the static CLI runner skips them.

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
| `anofox_plot_terms(tbl, highlight := NULL, lambda_min := NULL, lambda_1se := NULL, facet := NULL, reference := NULL)` | **term, estimate**, conf_low, conf_high, conf_level, model_id, id, index_name, index_value, window_end | coefficient forest (x "estimate (95% CI)"; models — or groups, `id` — dodged + coloured with the legend in stacking order; rows without an interval as hollow rings, noted under the plot; a coefficient whose interval contains 0 — not significant — is drawn at half opacity (point and interval, "n.s." in its tooltip, "faded = interval crosses 0" under the plot); a numeric `reference := 1` moves that no-effect line, e.g. for odds ratios; models × groups: one panel per group). With `index_value`: a **λ path** (ridge / elastic net) is one panel with every coefficient overlaid on a log-λ axis and labelled at its line end (`highlight := ['tv', …]` greys the others; λ.min / λ.1se as dashed lines from `lambda_min :=` / `lambda_1se :=` or the table's `lambda_min`/`is_min` columns; `facet := true` keeps one panel per term); a **LARS path** on x = \|β\|₁ / max \|β\|₁; the **quantile process** (x "τ (quantile)") and **rolling coefficients** (x = `window_end`, a date axis) one panel per term, with a dashed full-sample reference (± CI shaded) from the input's index-less rows (e.g. an `ols` tidy table `UNION ALL BY NAME`) or `reference := (SELECT list(t) FROM ols_tidy_by(…) t)` |
| `anofox_plot_prediction(tbl, x := NULL, y := NULL, facet := NULL, band := NULL, kind := NULL)` | **x \| ds, yhat**, y, yhat_lower, yhat_upper, split \| is_training, model_id \| model_name, id, focal, focal_name | observed y as neutral grey points, the fit as a coloured line with a light band; split changes along x as labelled dashed vertical lines ("hold-out", "forecast", else the split name; when splits interleave along x the non-train points are marked instead). Several models in a panel: one band only (`band := 'model'`, default `ols` or the first model) and a line per model in its stable colour; `kind := 'difference'` plots each model minus the reference. Effects tables (`focal`): y "P(y = 1)" for logistic / binomial, "expected count" for poisson / negbinom, else "predicted y"; x from `focal_name` when present. Several `id`s become panels |
| `anofox_plot_curve(tbl, base_rate := NULL)` | **curve_type, x, y**, y_low, y_high, model_id \| series \| id, threshold, weight, auc, n_censor \| censored, n_risk, n_event (km), is_min, is_1se (lambda_cv) | roc: square, diagonal, the AUC (trapezoid, or an `auc` column) in each legend label; pr: the recall-0 points trimmed, a dashed base-rate line (`base_rate :=`, else the precision at the lowest threshold with full recall); calibration: square, diagonal, point size by the bin count (`weight`); km: step curves from S(0) = 1, step-ribbon band, small `+` censor marks, a dashed 50 % line with drops at each stratum's median and a number-at-risk table under the curves; acf/pacf: lollipops without lag 0 (ACF), significance bounds (one panel per series); lambda_cv: CV error ± SE on a log-λ axis with λ.min / λ.1se; qq / pdp / lift / null_dist: line or points (+ band). One `curve_type` per call (`GROUP BY curve_type`) |
| `anofox_plot_summary(tbl, metrics := NULL, baseline := 'range')` | **metric, value**, model_id \| model_name, id, conf_low, conf_high | a grid of small panels, one per metric with its own scale; one model order in every panel (best first by the first metric), the value beside each dot, "n/a" for a missing metric, the best model highlighted for metrics with a direction; `baseline := 'range'` zooms to the values and prints the range ("Δ = …") in the panel title, `'zero'` starts the axis at 0; n_observations / n_features / iterations only when `metrics := [...]` asks |
| `anofox_plot_tests(tbl, alpha := 0.05, value := NULL, label := 'p')` | **p_value \| p_adj \| statistic**, test_id, group1, group2, estimate, conf_low, conf_high, method | comparison forest: one row per test (pairs as "B − A"), x "difference (95% CI)" (else the statistic, else −log10 p; `value :=` forces one), coloured by significance at `alpha`, `p = … **` beside each row (`label := 'stars'`: stars only). Estimates without any interval (pairwise_t_test_by today) fall back to −log10(p) bars with a labelled significance line, said in the axis label. Several groups get a panel each. A column holding a `*_agg` STRUCT / LIST is expanded |
| `anofox_plot_compare(tbl, value :=, factor :=, tests := NULL, kind := 'box', hide_ns := NULL, label := 'stars')` | the raw rows: the **value** and **factor** columns named by the arguments | box / violin / points per group with significance brackets from `tests :=` (a LIST of test STRUCTs: `(SELECT list(t) FROM tukey_hsd_by(…) t)` or `tukey_hsd_agg(y, g)`; one two-sample STRUCT for two groups); `hide_ns` defaults to true above 3 groups; `label := 'stars' \| 'p'` or a template (`'p = {p.adj}'`, `'{p.signif}'`) |
| `anofox_plot_partial(tbl, kind := 'component')` | **term, x, partial_residual** (component), or **term, x_residual, y_residual** (added_variable) | one panel per term: component + residual (points, component line, loess — a binary / ≤ 3-level term gets jittered points and the group means instead) or added-variable (points + least-squares line through the origin, slope in the title) |
| `anofox_plot_obs(tbl, kind := 'resid_fitted', label_top := 3)` | **fitted, residual**, row_id, std_residual, leverage, cooks_d, n_params | one diagnostic: `resid_fitted`, `qq`, `scale_location`, `leverage` (Cook's distance contours 0.5 and 1 from `n_params`, both signs in view), `cooks` (index plot, the 4/n line labelled, top rows labelled above their stems); the `label_top` largest-Cook's rows are labelled by `row_id` |
| `anofox_plot_diagnostics(tbl, label_top := 3)` | as `obs` | the plot.lm 2×2 as one SVG (a ggplot-rs `PlotGrid`: root `data-grid="2 2"`, each sub-plot a nested `<svg data-panel="i">`; one legend when several models are compared) |
| `anofox_plot(tbl)` | any of the above | picks the plot from the columns: term+estimate → terms, curve_type → curve, yhat → prediction, fitted+residual → diagnostics, metric+value → summary, partial_residual → partial, p_value / test_id / statistic+method → tests. Errors name the contract version (`anofox_contract_version()` = `'1'`) |

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

### Export & share

- Every chart/table panel has a hover **⤓** button — charts download as **PNG**,
  tables as **CSV**.
- **Share** copies a link with the whole dashboard SQL encoded in the URL hash
  (no server) — open it to reproduce the dashboard.
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

### Tables

A `::TABLE` / `::PAGED` panel comes with a toolbar: the row count ("8 of 24
rows") with a chip per active filter, a **search** box, **Filters** (a filter
under each header: a dropdown for a column with few values, `> n` / `< n` and a
range slider for numbers, "contains" for text), **Σ Totals** (a totals row
over every matching row, not just the page: sums of amounts and counts, means of
rates, shares and scores, counts of badge values — each numeric total carries a
small tag, e.g. "sum ▾", to switch it to mean, median, min, max or count; the
choice is kept per table), **Columns** (show / hide),
**Pages | Scroll**, **Compact | Comfortable** row density (compact by default
above 50 rows) and **⤓ CSV** (the matching rows, visible columns). Hovering a
header shows the column's min / max / mean, empty and distinct counts.

Formatting: raw column names read as headers (`load_pct` → "Load %", `avg_mae` →
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
| good for | up to ~10k rows | any size: Parquet on S3, MotherDuck, a served database |

A `::PAGED` request is the same everywhere: in the browser and the authoring
`serve` the query is built by the wasm `page_sql()`; a **locked** server
(`anofox_serve_dashboards`) receives only the structured request — page, sort,
search, column filters (`eq`, `contains`, `gt`, `ge`, `lt`, `le`), a statistics
or distinct-values request — and builds the identical SQL itself, with every
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

**Examples in the sidebar** (open one directly with `?dashboard=<name>`):

- **Examples** — the core feature tour: *Start here*, *Charts*,
  *Interactivity*, *Tables*, *Layout* and *Theming* ("Theming: your branding
  (ACME)", "Locale: de-DE"; sources in `examples/theming/`).
- **anofox extensions** — one group per anofox DuckDB extension:
  - *Statistics* — "Statistics showcase: property market (live
    anofox-statistics)", computed live by the anofox_statistics wasm extension;
  - *Forecast* — "M5 forecast (live extension)" and "M5 analytics", live
    anofox_forecast;
  - *Tabular*, *Optimize*, *Decide* — "Stored results: CRM data quality",
    "Stored results: distribution centre", "Stored results: support inbox
    triage". These extensions can't run in DuckDB-Wasm (no loadable wasm
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
with external access, extension loading and configuration changes disabled (it
serves that snapshot; `anofox_serve_stop` + re-run to refresh). Each request
gets its own connection, a row cap and a timeout (optional third argument:
`'{"max_rows": …, "timeout_ms": …, "threads": …}'`).

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
