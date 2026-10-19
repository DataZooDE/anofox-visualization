---
name: anofox-visualization-dashboards
description: >
  Build an interactive SQL dashboard with anofox-visualization from a
  plain-text request (the build-dashboard workflow). Use when the user asks
  to "build / make / create a dashboard", "visualize this data", "turn this
  query into a dashboard", "explore this table", or describes the charts,
  KPIs, tables, filters or a process diagram they want. Covers the ::ROLE
  cast model (each annotated SELECT is a panel, unannotated statements are
  setup), every role of the registry (charts, waterfall / bullet / funnel,
  heatmaps, maps, intervals, facets, nested forecast fans, ::MARKAREA,
  ::TRACK strips, event annotations, thresholds), KPI tiles with delta,
  sparkline and goal, ::PERIOD comparisons, inputs (dropdown, multiselect,
  ::BUTTONS, ::TOGGLE, dates) bound to getvariable(), cross-filter,
  ::SELECT_AS, ::DRILL / ::LINK drill-down and drill-through, URL state,
  ::ROWSELECT tables (toolbar, ::TOTALS, ::COLGROUP, ::FLAG, ::HIGHLIGHT,
  ::SUBTOTAL, ::PAGED), ::DIAGRAM pipelines / lineage / process flows
  (::UPSTREAM, ::EDGES, ::LANE, flow styles), motion (::ANIMATE, ::STORY,
  ::TRANSITION, ::LINESTYLE 'flow'), Markdown with {{col:fmt}} values and
  $LaTeX$, ::INFO / ::CALLOUT, ::ASOF, ::EMPTY / -- @requires panel states,
  layout, themes / branding / locale, anofox_plot_* contract plots as
  panels, the Explore data page, the `dashboard --describe` / `--check`
  generate-validate-repair loop with its lint codes, the design contract
  and the gotchas that silently break a panel.
version: 2026.10.18
user-invocable: false
---

# Anofox Visualization — ::ROLE Dashboards (build-dashboard)

**Extension:** `anofox_visualization` (CalVer 2026.10.18) | **Tools:** `dashboard` CLI (`--describe`, `--check`, `--roles`), browser builder, `serve` | **Docs:** `docs/DOCS.md`, `docs/dashboard-design.md` in the repository

A dashboard is a `.sql` script. Tag output columns with `::ROLE` casts; each annotated `SELECT` becomes a panel (chart, KPI, table, input). Statements without a role (`CREATE TABLE`, `SET`, `LOAD`, …) run as setup, in order, on one session. Your output is one SQL script.

## Workflow

1. **Ground on the schema (required).** Never invent columns. `dashboard --describe <table | 'file.parquet' | read_csv(…)> [--db file.duckdb]` prints each column's type, min / max, approx-distinct (low = discrete → bar / category; high = search / paged) and null %.
2. **Pick panels**: KPIs for headline numbers, a line for trends, bars for category comparisons, a table for detail, inputs for filtering (table *Pick the chart* below).
3. **Lay it out**: `SELECT n::COLUMNS;`, `::COL` per panel, KPIs in a `::GROUP` box, `::TAB` / `::SUBTAB` for sections.
4. **Write the SQL**: one annotated `SELECT` per panel, top to bottom.
5. **Wire interactivity** if asked: inputs and cross-filter.
6. **Validate and repair (required).** `dashboard --check file.sql [--json] [--strict] [--no-design] [--db prebuilt.duckdb]` runs every statement in a fresh session. Repeat until it prints clean (exit 0). Do not finish on unvalidated SQL.
7. **Self-critique against the design contract** below, re-check.
8. **Visual review (required for a finished dashboard).** `--check` never looks at the render: `scripts/shoot.sh --dash <dir> [--init <file> --workdir <dir>] --tabs "A,B" --out <dir>` screenshots each tab; grade the PNGs against `docs/dashboard-visual-review.md` (clipped labels, colliding colours, broken number formats, empty panels, no hierarchy) and fix every blocker / major.

### `dashboard --check` diagnostics

| Code | Severity | Meaning / fix |
|---|---|---|
| `sql-error` | error | the statement failed (message from DuckDB) |
| `silent-setup` | error | a query with `::ROLE` casts outside its main SELECT list ran as setup: the panel vanished (casts in a CTE body / subquery) |
| `render-error` | error | the chart cannot be drawn (missing required aesthetic, e.g. `::HEATMAP` without `::YAXIS`) |
| `render-warning` | warning | renders, but the engine dropped data: non-finite values (`NaN`, `±Inf`, `log` of ≤ 0), a stat layer with too little data (density of one value, smooth on two points). Filter in SQL (`WHERE isfinite(x)`, `NULLIF`) |
| `empty-panel` | warning | 0 rows: a blank card (often a filter on an unset variable); not raised after an `::EMPTY` message |
| `optional/missing` | note | a `-- @requires <path>` input is missing: the panel reads "Not available", a `::TRACK` strip is left out, a `::TAB` / `::SUBTAB` is hidden with its panels (not a failure, `--strict` passes) |
| `totals/summed-rate` | note | a `'sum'::TOTALS` (or a bare `sum` in the list) would also sum a rate / percentage column it doesn't name — name it: `'Per 10k: none; sum'` |
| `box/missing`, `box/unordered`, `box/null` | error / warning / warning | a box from statistics without `::Q1`/`::MEDIAN`/`::Q3`, with one whisker, without `::XAXIS` or beside a chart measure; Q1 ≤ median ≤ Q3 broken or a whisker inside the box; a NULL quartile (not drawn) |
| `requires/unknown-variable`, `requires/empty-alternative`, `requires/bad-variable`, `requires/on-setup`, `requires/tab-always-hidden` | warning (`--strict` fails) | a `${name}` in `-- @requires` with no value here (that path counts as missing; `--check --data <dir>` sets `anofox_data`); `a \|` / `a \|\| b` / a line with no path; a malformed `${…}`; `@requires` on a setup statement that reads no file (a viewer could skip it: put it on the statement that reads the file); a tab's `${name}` set only by a `SET VARIABLE` below the tab (a locked server always hides it: set it above) |
| `animate/unknown`, `animate/reveal-no-xline`, `linestyle/unknown`, `story/bad-by`, `transition/unknown` | warning | an `::ANIMATE` value that is not `auto`/`draw`/`grow`/`reveal`/`none`; `'reveal'` without an `::XLINE` to start from; a `::LINESTYLE` that is not `flow`/`dashed`/`solid`; a `::STORY_BY` that is not `x`/`series` (or `series` without a `::CATEGORY`); a `::TRANSITION` that is not `tween`/`none` |
| `track/no-chart` | warning | a `::TRACK` that does not directly follow a chart with a numeric / date `::XAXIS` (only a `::HEIGHT` may sit between): it is drawn as a panel of its own |
| `annotation/unpaired` | warning | a label / range end without its partner, or unequal numbers of `::BAND_LOWER` and `::BAND_UPPER` |
| `diagram/null-id`, `diagram/duplicate-node`, `diagram/unknown-node`, `diagram/bad-edges`, `diagram/bad-style`, `diagram/bad-direction`, `diagram/too-large` | warning | a `::DIAGRAM` row without an id; an id twice (the first row wins); an edge naming an id that is no row (not drawn); `::EDGES` that is not a list of `{from, to}` structs; an edge style that is not `solid`/`dashed`/`flow`; a `::DIRECTION` that is not `LR`/`TB`; more than 150 nodes / 300 edges (the first ones are drawn) |
| `chart/needs-xaxis`, `input/many-buttons`, `thresholds/bad-rule`, `thresholds/no-column`, `thresholds/in-table`, `asof/unparseable`, `asof/bad-stale`, `callout/unknown`, `callout/no-text`, `markdown/unknown-placeholder`, `markdown/unknown-format`, `markdown/bad-math`, `diagram/bad-pin`, `diagram/partial-pins`, `policy/*` | mostly warning (`chart/needs-xaxis`: error) | a waterfall / bullet / funnel without `::XAXIS`; too many `::BUTTONS` options (use a dropdown); a malformed or misplaced `::THRESHOLDS`; an `::ASOF` that is no date / a bad `stale=`; a bad `::CALLOUT`; a `{{col}}` naming no column or an unknown `:fmt`; TeX KaTeX can't parse; diagram pins on only some nodes; with `--policy`: what an access policy refuses or hides (access-control skill) |
| `unknown-cast` | warning | `::BARCHAT`: not a role and not a SQL type — the column lost its role |
| `design/untitled-chart`, `design/pie-slices`, `design/unsorted-bars`, `design/many-series`, `design/too-many-panels`, `design/ungrouped-kpis`, `design/raw-table` | warning | design advisories (below); `--strict` makes them fail, `--no-design` hides them |

`--check` gives each input a default (`SET VARIABLE`: the first option of a single input, every option of a `::MULTISELECT`, both ends of a `::DATERANGE`, the default period of a `::PERIOD`), so filtered panels are checked with real data. Variables nothing sets (`selected`, a clicked table's key) are unset: guard them with `COALESCE` / `NULLIF` as in the master-detail example. `--json` prints `{"ok": …, "diagnostics": [{"severity", "stmt", "code", "message", "sql"}]}`.

<!-- assist:begin (the builder assistant's guide: scripts/gen-assist-guide.mjs) -->
## Roles

`dashboard --roles` prints the authoritative list from the registry (`src/roles.rs`); `dashboard --roles-md` the table of `docs/DOCS.md`. Unknown casts are flagged by `--check`.

| Group | Roles (aliases in brackets) |
|---|---|
| Chart kind (on the measure) | `::BARCHART` `::BARCHART_STACKED` `::BARCHART_PERCENT` `::BARCHART_STACKED_PERCENT` · `::LINECHART` `::LINECHART_PERCENT` `::STEP` `::SMOOTH` · `::AREACHART` `::AREACHART_STACKED` · `::SCATTER` `::BUBBLE` `::JITTER` · `::PIE` `::DONUTCHART` · `::GAUGE` · `::RADAR` · `::WATERFALL` `::BULLET` `::FUNNEL` · `::HISTOGRAM` `::DENSITY` `::BOXPLOT` `::VIOLIN` `::QQ` · `::HEATMAP` `::CALENDAR` · `::CANDLESTICK` · `::SPARKLINE` · `::MAP` (WKT) · `::REGION` (a code from the built-in boundaries: `'DE'`/`'DEU'`, `'DE-BY'`, AGS `'09162'`, NUTS `'DE212'`) · `::LAT` + `::LON` (points) · `::BASEMAP` (WKT or `'world'`, `'de-states'`, `'de-districts'`) · `::DIAGRAM` (on the node id, see *Diagrams*) |
| Encoding | `::XAXIS` (`::X`) · `::YAXIS` (`::Y`, heatmap rows) · `::CATEGORY` (`::SERIES`, `::COLOR`) discrete colour · `::LABEL` (section heading alone; else chart / per-mark label) · `::TITLE` (panel title bar) · `::SIZE` (bubble) · `::OPEN` `::HIGH` `::LOW` (candles) |
| Box plot from statistics (one row per box, no `::BOXPLOT`) | `::Q1` `::MEDIAN` `::Q3` (required) · `::WHISKER_LOW` + `::WHISKER_HIGH` · `::MEAN` (a dot) · `::OUTLIERS` (a list) — e.g. `quantile_cont(v, 0.25)::Q1, median(v)::MEDIAN, quantile_cont(v, 0.75)::Q3` with `GROUP BY` the `::XAXIS`, for populations too large to send as raw rows |
| Intervals, facets | `::YMIN` + `::YMAX` (a `::SCATTER` → pointrange, bars / lines → error bars, dodged by `::CATEGORY`) · `::XMIN` + `::XMAX` · `::FACET` / `::FACET_FREE` + `::FACET_NCOL` · `::RANK` + `::LABEL_TOP` · `::CENSOR` (Kaplan-Meier `+`) |
| Annotations | `::REFLINE` (`::YLINE`, `::TARGET`) · `::XLINE` (number, date or timestamp) · `::BAND_LOWER` + `::BAND_UPPER` (several pairs, in column order = a nested fan: 95 / 80 / 50 %) · `CASE WHEN flag THEN day END::MARKAREA` (a shaded region per run of flagged days) · `::DATALABELS` · `'slope,intercept'::ABLINE` · `1::IDENTITY` · `p::COOKS_CONTOUR` |
| Annotations from data (per row — `LEFT JOIN events ON events.day = t.day`) | `::XLINE` + `::XLINE_LABEL` (labelled event line) · `::XBAND_FROM` + `::XBAND_TO` + `::XBAND_LABEL` (labelled shaded range) · `::REFLINE` + `::REFLINE_LABEL` · `::ANNOTATE` (text callout at the row's x, y; NULL = none) |
| Thresholds | `'<0:bad; <5:warn; good'::THRESHOLDS` beside a KPI (value colour), a `::GAUGE` (zones) or a chart (threshold lines) · alone before a table: `'Margin %: <10:bad; good \| accuracy: <0.85:bad'::THRESHOLDS` (cell tint + ● ▲ ◆) · first match wins; value's own units |
| Modifiers | `1::FLIP` · `'€'::YFORMAT` / `::XFORMAT` (`'$'`, `'percent'`, `'comma'`, `' kg'`) · `'log10' \| 'sqrt' \| 'reverse'::XSCALE` / `::YSCALE` · `'lm' \| 'gam' \| 'loess'::SMOOTH_METHOD` · `::ALPHA` · `'routes'::SYNC_ZOOM` (maps with the same value zoom together) · map `'auto'::PROJECTION` (`'equal-earth'`, `'conic'`, `'utm'`, `'mercator'`, `'none'` for planar x/y), `'quantile 5'::CLASSES` (`'jenks 5'`, `'equal 5'`, `'0,10,50'`), `0::MIDPOINT`, `'lo,hi'::RANGE`, `'de-districts'::BOUNDARIES` (force the layer), `8::LABEL_TOP` / `''::DATALABELS` (region names on the map) · gauge `'0,100'::RANGE`, `'#a,#b'::COLORS`, `'low,high'::LABELS` |
| KPI and text | `::METRIC` · `::MONEY` · `::PERCENT` (the value in percent, 0–100: `12.5` → 12.5%; a fraction × 100 — also `'percent'::YFORMAT` and `{{x:percent}}`) · `::COMPACT` (1.2K) · `::DELTA` (comparison → arrow + %, up = green) · `::DELTA_LOWER_GOOD` (the same, but a rise is red: churn, cost, errors) · `::DELTA_FORMAT` (`'pct'`/`'abs'`/`'both'`: `▲ 12% (+$1,200)`, `+2.3 pp` for a `::PERCENT`) · `::DELTA_LABEL` (`'vs last month'`, `getvariable('p_prev_label')`) · in a KPI: `list(v ORDER BY d)::SPARKLINE` (trend line), `::GOAL` (progress bar) · `::TEXT_SMALL` / `_MEDIUM` / `_LARGE` · `::MARKDOWN` · `'warning'::CALLOUT` beside a `::MARKDOWN` (`note` / `warning` / `success` box) · `'Net of returns; source: ERP'::INFO` on any panel (an ⓘ with the definition on its title bar — use it instead of a footnote box) · numbers inside Markdown: `'Grew {{growth:pct}} to {{revenue:money}}'::MARKDOWN, … AS growth, … AS revenue` (the same row; `money` `percent` `compact` `metric` `date` `text`, locale-formatted — never build the text with `||`) · formulas in Markdown: `$\hat{y}_t$` inline, `$$…$$` display (KaTeX; `\$` = a literal dollar, `$5 and $10` stays text; `{{col}}` works inside; `--check` → `markdown/bad-math`) |
| Tables | `::TABLE` (one marker per panel) · `::PAGED` (SQL-paginated) · per column `::MONEY` `::PERCENT` `::COMPACT` `::METRIC` `::TREND` `::COLORSCALE` (low = good) `::COLORSCALE_HIGH_GOOD` `::COLORSCALE_DIVERGING` (around 0) `::BADGE` `::SPARKLINE` (a `list()` column) `::PLAIN` · `::DOWNLOAD_CSV` / `_XLSX` / `_PDF` |
| Table directives (before the table) | `'none' \| 'single' \| 'multi'::ROWSELECT` · `'sum'::TOTALS` (`::TOTAL`) · `'A: x, y \| B: z'::COLGROUP` · `'mape > 0.2'::FLAG` · `'Revenue: top 3'::HIGHLIGHT` (`::TOPN`) · `'region'::SUBTOTAL` — see *Tables* |
| Chart interactions (before the chart) | `'region' \| 'region multi'::SELECT_AS` (own selection variable + brushed `region_from` / `region_to`) · `'region > country > city'::DRILL` (drill-down + breadcrumb) · a `'#Tab?v.x={value}'::LINK` column (drill-through, charts and tables) |
| Inputs | `::DROPDOWN` · `::MULTISELECT` (a list) · `::BUTTONS` (segmented, ≤ ~5 options) · `::TOGGLE` (a BOOLEAN; value = default) · `::NUMBER` · `::DATE` · `::TEXT` · `::DATERANGE` (two columns → from / to) · `::PERIOD` (presets + grain + comparison) · `::HINT` |
| Strips under a chart | right after a chart with a numeric / date x: `day::XAXIS, 'Promotions'::TRACK, name::LABEL` (ticks) · `… , detector::YAXIS, '#d64545'-style ::CATEGORY, 1::HEATMAP, 'Alarms'::TRACK` (lanes of tiles) · `day::XAXIS, price::STEP, 'Price'::TRACK` (sparkline) — same plot area and x window as the chart; a click on a strip mark shades its span on the chart |
| Layout and chrome | `n::COLUMNS` · `n::COL` (`::SPAN`, `::WIDTH`; of 12) · `n::HEIGHT` (px) · `'Box'::GROUP` … `1::ENDGROUP` · `'Name'::TAB` (`::PAGE`) / `::SUBTAB` · `'Heading'::SECTION` · `::PLACEHOLDER` · `n::RELOAD` · `::HEADER_IMAGE` · `::FOOTER_LINK` |
| Theme, branding, locale | `'dark'::THEME` (`light` `dark` `print` `colorblind` `high-contrast`, a stack `'acme+dark'`, or JSON: `content::THEME FROM read_text('acme.json')`) · `'de-DE'::LOCALE` (`en-US` `en-GB` `de-DE` `fr-FR`) · `'Title'::DASHBOARD_TITLE` · `'ACME'::BRAND` · `'logo.svg'::LOGO` · `'favicon.svg'::FAVICON` · `max(loaded_at)::ASOF, 'stale=24h'` (a "Data as of …" chip, amber when stale — give every locked / exported dashboard one) |

A combo chart is several measure columns: `week::XAXIS, sales::BARCHART, revenue / 50::LINECHART`. A `::ROLE` may follow an `AS` alias: for a measure the alias is the legend name, for an input it is the variable name, in a table the header.

## Inputs and cross-filter

- The **output column name of an input is a DuckDB variable**: `SELECT region AS region ::DROPDOWN FROM t GROUP BY 1 ORDER BY 1;` → `getvariable('region')`. A `::MULTISELECT` is a list: `list_contains(getvariable('channel'), channel)`; "nothing selected = all": `(len(getvariable('channel')) = 0 OR list_contains(getvariable('channel'), channel))`.
- **Metric / dimension switcher**: a `::BUTTONS` input picks the view, the chart reads it — measure: `CASE getvariable('metric') WHEN 'Revenue' THEN sum(revenue) WHEN 'Units' THEN sum(units) END::BARCHART`; grouping column: `COLUMNS(getvariable('dim'))::XAXIS` (options = real column names). A `::TOGGLE` is a BOOLEAN: `WHERE NOT getvariable('net') OR …`.
- An input's caption is its variable name unless a `::LABEL` column gives one: `SELECT 50 AS refund_at_pct ::NUMBER, 'Refund threshold (%)'::LABEL;`.
- **Named chart selections**: `SELECT 'region'::SELECT_AS;` before a chart makes its click set `getvariable('region')` (`''` = none) instead of `selected`; `'region multi'` keeps a `VARCHAR[]` (`[]` = none; shift/ctrl-click adds). A toolbox brush on a continuous/date x sets `region_from` / `region_to` (NULL = none; dates as text → `::DATE`). Guards: `getvariable('region') IN ('', region)`, `len(getvariable('region')) = 0 OR list_contains(getvariable('region'), region)`, `d BETWEEN coalesce(getvariable('week_from')::DATE, d) AND coalesce(getvariable('week_to')::DATE, d)`. Use it whenever two charts should filter independently.
- **Drill-down**: `SELECT 'region > country > city'::DRILL;` before a chart → a click goes one level deeper, a breadcrumb goes back. The chart's x is `CASE getvariable('drill_level') WHEN 'region' THEN region WHEN 'country' THEN country ELSE city END`, filtered by `getvariable('region') IN ('', region) AND getvariable('country') IN ('', country)` (each level variable is `''` until drilled past; also `drill_depth`, `drill_path`; `'geo: …'` renames the prefix). **Drill-through**: a `::LINK` column holds a template — `'#Tab?v.x={value}'` (tab + variables), `'/d/<id>?v.x={value}'` (another served dashboard), `'https://…/{value}'` (new browser tab); `{value}` = the clicked key, `{column}` = a table row's column. In a table it renders as a link cell.
- **The URL is the view**: `?tab=<name>` opens a tab, `?v.<name>=<value>` seeds a variable (an input — only a valid option / number / date —, `selected`, or a named selection; repeat the key for a list: `?v.channel=app&v.channel=web`). The address bar follows every change and **Share** includes it, so a link restores the exact view; use it to link one dashboard to another pre-filtered.
- **Cross-filter**: a click on a chart mark sets `getvariable('selected')`. **Table rows are clickable only when the table opts in** (`'none'` is the default) with a `::ROWSELECT` directive before it: `SELECT 'single'::ROWSELECT;` (a click sets `getvariable('selected')` and `getvariable('<first column>')`) or `SELECT 'multi'::ROWSELECT;` (rows get check marks; `getvariable('<first column>')` is a list — guard with `len(COALESCE(getvariable('sku'), []::VARCHAR[])) = 0 OR list_contains(getvariable('sku'), sku)`). Without it rows are not selectable. Panels opt in: `WHERE COALESCE(getvariable('selected'), '') IN ('', channel)` (`''` = nothing clicked; the browser starts with `''`, `--check` and `serve` with the variable unset, so keep the `COALESCE`). Sources (comparison charts) should not filter themselves; targets do. Put the key column first in a table.
- **Period vs previous period** — use one `::PERIOD`, never hand-written date math. The alias is the variable prefix:
  ```sql
  SELECT 'presets=7d,30d,90d,mtd,qtd,ytd,last_month; default=30d; compare=prev' AS p ::PERIOD, 'Period'::LABEL;
  CREATE OR REPLACE MACRO in_period(d) AS d BETWEEN getvariable('p_from')::DATE AND getvariable('p_to')::DATE;
  CREATE OR REPLACE MACRO in_prev(d) AS d BETWEEN getvariable('p_prev_from')::DATE AND getvariable('p_prev_to')::DATE;
  SELECT sum(revenue) FILTER (WHERE in_period(day))::MONEY, sum(revenue) FILTER (WHERE in_prev(day))::DELTA, 'Revenue'::LABEL FROM orders;
  SELECT date_trunc(getvariable('p_grain'), day)::DATE ::XAXIS, sum(revenue)::LINECHART, 'Revenue'::TITLE FROM orders WHERE in_period(day) GROUP BY ALL ORDER BY 1;
  ```
  Variables (text): `p_from`, `p_to`, `p_prev_from`, `p_prev_to` (NULL with "No comparison"), `p_grain` (`day`/`week`/`month`/`quarter`), `p_compare` (`none`/`prev`/`yoy`), `p_label` ("Sep 2026"), `p_prev_label` ("vs Aug 2026"). Spec keys: `presets` (`Nd`, `Nw`, `Nm`, `today`, `yesterday`, `wtd`, `mtd`, `qtd`, `ytd`, `last_week`, `last_month`, `last_quarter`, `last_year`), `default`, `from`/`to`, `grain` (`auto`), `grains` (`none` hides the switch), `compare` (`prev`/`yoy`/`none`), `today` (pin it for fixed sample data). Bad specs → `period/bad-spec`.
- **KPI tile with change, trend and goal** (one statement; `NULL` comparison → no change line):

  ```sql
  SELECT sum(rev) FILTER (WHERE cur)::MONEY, sum(rev) FILTER (WHERE prev)::DELTA, 'both'::DELTA_FORMAT,
         getvariable('p_prev_label')::DELTA_LABEL, list(rev ORDER BY b) FILTER (WHERE cur)::SPARKLINE,
         750000::GOAL, 'Revenue'::LABEL
  FROM (SELECT date_trunc(getvariable('p_grain'), day) AS b, in_period(day) AS cur, coalesce(in_prev(day), false) AS prev,
               sum(revenue) AS rev FROM orders GROUP BY ALL);
  ```
  Lint: `kpi/bad-delta-format`, `kpi/no-delta`, `kpi/sparkline-not-list`.
- Inputs, tabs, downloads and cross-filter work in the browser builder and `serve`; the static `dashboard file.sql` render draws no controls but binds each input's default.

## Tables

A `::TABLE` / `::PAGED` panel has a toolbar the viewer drives: row count with filter chips, **search**, **Filters** (a dropdown / `> n` `< n` range / "contains" under each header), **Σ Totals** (over every matching row; each total has a "sum ▾" menu: sum, mean, median, min, max, count), **Columns** (show / hide), **Pages | Scroll** (virtualised), **Stats** (a strip under the header: histogram per number column, top 3 values per text column), **Compact | Comfortable** density and **⤓ Export** (CSV of the raw values, or an Excel `.xlsx` formatted like the table: units, number formats, live subtotal / total formulas). Hovering a header shows min / max / mean, empty and distinct counts.

Formatting is automatic, so do not pre-format numbers as text: raw names become headers (`load_pct` → "Load %", `avg_mae` → "Avg MAE"; an `AS "Header"` alias is kept), each numeric column gets its own decimals and grouping (ids and years ungrouped), a true minus, muted zeros, "—" for NULL, ✓ / ✗ for booleans, and one unit per `::MONEY` / `::COMPACT` column moved into the header ("Revenue ($M)"). Plain numeric columns get a slim in-cell bar (`::PLAIN` turns it off).

Directives for the **next** table (columns named by raw name or header, case-insensitive):

| Directive | Value | Effect |
|---|---|---|
| `::ROWSELECT` | `'none'` (default) \| `'single'` \| `'multi'` | row click → `getvariable('<first column>')` (+ `'selected'` for single); multi = a list |
| `::TOTALS` | `'sum'` \| `'mean'` \| `'median'` \| `'min'` \| `'max'` \| `'count'` \| `'none'`, or `'revenue: sum; per 10k: none'` (`;` or `,`) | totals aggregation (default: sums of amounts / counts, no total for rates and percentages, means of the rest; name distances: 'km: mean'); `'none'` hides the row |
| `::COLGROUP` | `'Money: plan, revenue \| Quality: accuracy, bias'` | a grouped header over adjacent columns |
| `::FLAG` | `'margin < 12, status = breach'` (`>` `>=` `<` `<=` `=` `!=`) | ⚠ + warning tint on matching cells |
| `::HIGHLIGHT` | `'revenue: top 2, margin: bottom 1'` (`'top 3'` = first numeric column) | bold + rank on the top / bottom n (a `::PAGED` table: within the page) |
| `::SUBTOTAL` | `'region'` | rows grouped by that column, a subtotal row after each group (each column's total aggregation); `::TABLE` only, not `::PAGED` |

```sql dashboard
CREATE TABLE regions AS SELECT * FROM (VALUES
  ('North', 1200, 1350.5, 18.2, 0.91, 2.5), ('South', 1100, 980.0, 9.5, 0.84, -4.1),
  ('West', 800, 860.25, 14.0, 0.88, 0.0), ('East', 950, 1010.0, 11.3, 0.79, -1.2)) t(region, plan, revenue, margin, accuracy, bias);

SELECT 'single'::ROWSELECT;
SELECT 'revenue: sum, margin: mean, accuracy: median'::TOTALS;
SELECT 'Money: plan, revenue | Quality: accuracy, bias'::COLGROUP;
SELECT 'margin < 12'::FLAG;
SELECT 'revenue: top 2'::HIGHLIGHT;
SELECT region::TABLE, plan::MONEY, revenue::MONEY, margin,
       accuracy::COLORSCALE_HIGH_GOOD, bias::COLORSCALE_DIVERGING,
       'Revenue against plan by region'::TITLE
FROM regions ORDER BY revenue DESC;
SELECT sum(revenue)::MONEY, 'Revenue of the clicked region'::LABEL FROM regions
WHERE COALESCE(getvariable('region'), '') IN ('', region);
```

`::PAGED` runs everything in DuckDB — page, sort, search, column filters, totals, column statistics and filter values — so it suits Parquet on S3, MotherDuck or a served database; `::TABLE` loads the whole result (up to ~10k rows). A theme's `chrome.negatives` (`"minus"` default, `"red"`, `"parens"`) and `chrome.zero` (`"muted"` default, `"dash"`) set how tables show negatives and zeros.

## Heatmaps

`x::XAXIS, y::YAXIS, value::HEATMAP` (+ `'$'::YFORMAT`, `''::DATALABELS` to print values in the cells). Text axes keep the **SQL row order** — the first `::YAXIS` level is the top row, so `ORDER BY` the rows you want first. Colour is the theme's sequential ramp; values spanning zero switch to its diverging ramp around 0. When a value is good or bad, colour by status instead: `''::COLORSCALE` (continuous green → yellow → red, low = good; `::COLORSCALE_HIGH_GOOD`, `::COLORSCALE_DIVERGING`) or `'>=50:bad; >=20:warn; good'::THRESHOLDS` (one status colour per band, stepped legend `≥ 50%` / `20%–50%` / `< 20%`; reuse the rules of the tab's KPIs / table so the colours agree; rules in the values' unit — percent vs fraction). Theme status colours, safe in dark mode and the colour-blind theme; also on `::CALENDAR` and `::TRACK` tiles. Cells part with card-coloured gaps; labels flip dark / light for contrast. Tooltip, labels and legend use `::YFORMAT` and the locale. A click cross-filters: `getvariable('selected')` = the cell's **row** (`::YAXIS`) value.

Chart tooltips and `::DATALABELS` everywhere follow `::YFORMAT` / `::XFORMAT` and the `::LOCALE` (`$1,234.50`, `1.234,50 €`); percent charts read as percent.

## Themes, branding and locale

One theme styles the whole dashboard — charts, cards, tabs, KPI tiles, tables, inputs, tooltips — in every output (browser, `serve`, locked pages, `dashboard` HTML / `--svg`, exports). Directives are dashboard-level: put them at the top; the last one of each kind wins.

```sql dashboard
SELECT 'Sales cockpit'::DASHBOARD_TITLE;
SELECT 'ACME Analytics'::BRAND;
SELECT 'colorblind+dark'::THEME;   -- Okabe-Ito palette; dark when the viewer prefers dark
SELECT 'de-DE'::LOCALE;            -- 1.234,5 · 12.400 € · 12,5 % · Okt. 2026

CREATE OR REPLACE TABLE sales AS SELECT * FROM (VALUES
  ('North', 1200.5), ('South', 950.25), ('West', 610.0)) t(region, revenue);

SELECT 'Revenue'::SECTION;
SELECT 'Totals'::GROUP;
SELECT sum(revenue)::MONEY, 'Revenue'::LABEL FROM sales;
SELECT count(*)::METRIC, 'Regions'::LABEL FROM sales;
SELECT 1::ENDGROUP;
SELECT region::XAXIS, sum(revenue)::BARCHART, 'Revenue by region'::TITLE
FROM sales GROUP BY ALL ORDER BY 2 DESC;
```

- **Default look**: graphite + electric blue (cool grey page, flat white cards) — the built-in `light` (aliases `default`, `datazoo`). `colorblind` (Okabe-Ito palette) is a built-in theme too, never implied.
- **Variant.** No `::THEME`, or `'x+dark'`, follows the viewer (OS setting, the 🌙 toggle, `?theme=dark`); `'light'` / `'dark'` / `'print'` alone pin it. Charts are re-rendered dark, not just the cards.
- **A corporate theme** is a JSON object (`{"extends":"light","palette":[…],"colors":{"accent":"#0b5fff",…},"chrome":{"negatives":"parens","zero":"dash",…},"brand":{…},"dark":{…}}`) — from a file (`SELECT content::THEME FROM read_text('acme.json')`), a table (`SELECT to_json(t)::THEME FROM themes t WHERE name = 'acme'`) or inline. Colours must be `#rrggbb`; unknown keys are an error. Full key list: `docs/DOCS.md` → *Themes*.
- **The "Powered by DataZoo GmbH" credit is mandatory**: shown under every theme and in every output; author branding is added next to it, and no role, theme key or option hides or replaces it.
- `dashboard --check` reports `theme/unknown`, `theme/multiple`, `locale/unknown`, `period/bad-spec`, `kpi/bad-delta-format`, `kpi/no-delta`, `kpi/sparkline-not-list`, `chrome/bad-url` (logos: relative, `https:` or `data:image/`).
- For interactions it reports `interaction/bad-value` (a malformed `::SELECT_AS` / `::DRILL` value), `interaction/name-clash` (an input, a `::SELECT_AS` and a `::DRILL` level writing the same variable — rename one) and `link/bad-url` (a `::LINK` that is not `#tab`, `?…`, a path or http(s)); it primes `::SELECT_AS` / `::DRILL` variables to "nothing selected / top level" like the browser.
- `dashboard --themes` lists the built-ins; `dashboard file.sql --theme dark` / `serve … --theme T` set a default the script's `::THEME` overrides.

## Contract plots as panels

`anofox_plot_*` calls (statistics-plots skill) are panels too: the planner rewrites `SELECT anofox_plot_terms(t, …), 'Title'::TITLE FROM producer(…) t` to `to_json(list(t))` and draws it with the same renderer, so the panel works in the browser builder (DuckDB-Wasm, no extension needed) and with `dashboard` / `serve`. Extra unannotated columns (`SELECT method, anofox_plot_tests(t) … GROUP BY method`) give one panel per result row. `width :=` / `height :=` are ignored (the layout sizes panels).

## Worked examples

```sql dashboard
-- Weekly revenue and sessions, KPIs, filterable by channel
CREATE TABLE sales AS
SELECT 'W' || lpad((w + 1)::VARCHAR, 2, '0') AS week, c.channel,
       (40 + 9 * w + 7 * c.k + (hash(w, c.channel) % 15)::INTEGER) AS n,
       round((40 + 9 * w + 7 * c.k) * c.price, 2) AS revenue
FROM range(12) r(w), (VALUES ('app', 1, 2.5), ('web', 2, 3.1), ('store', 3, 4.0)) c(channel, k, price);

SELECT 'Filter'::GROUP;
SELECT DISTINCT channel::MULTISELECT FROM sales ORDER BY channel;
SELECT 1::ENDGROUP;

SELECT 'Weekly performance'::GROUP;
SELECT sum(revenue)::MONEY, 'Revenue'::LABEL FROM sales
WHERE list_contains(getvariable('channel'), channel);
SELECT sum(n)::COMPACT, 'Sessions'::LABEL FROM sales
WHERE list_contains(getvariable('channel'), channel);
SELECT 1::ENDGROUP;

SELECT 12::COL;
SELECT week::XAXIS, channel::CATEGORY, sum(revenue)::BARCHART_STACKED, '€'::YFORMAT,
       'Revenue grows every week'::TITLE
FROM sales WHERE list_contains(getvariable('channel'), channel)
GROUP BY ALL ORDER BY week, channel;
```

```sql dashboard
-- Cross-filter: click a channel bar, the trend narrows to it
CREATE TABLE sessions AS SELECT * FROM (VALUES
  ('W1', 'app', 30), ('W1', 'web', 22), ('W2', 'app', 41), ('W2', 'web', 28), ('W3', 'app', 38), ('W3', 'web', 35)) t(week, channel, n);

SELECT channel::XAXIS, sum(n)::BARCHART, 'App brings most sessions (click a bar)'::TITLE
FROM sessions GROUP BY channel ORDER BY 2 DESC;
SELECT week::XAXIS, sum(n)::LINECHART, 'Sessions per week'::TITLE
FROM sessions WHERE COALESCE(getvariable('selected'), '') IN ('', channel)
GROUP BY ALL ORDER BY week;
```

```sql dashboard
-- A gauge of goal completion and a donut of spend by category
CREATE TABLE tasks AS SELECT 72 AS done;
CREATE TABLE expenses AS SELECT * FROM (VALUES ('rent', 1200), ('travel', 400), ('tools', 250)) t(category, spend);

SELECT 6::COL;
SELECT sum(done)::GAUGE, '0,100'::RANGE, '#e03131,#efc94c,#0ca678'::COLORS, 'Completion at 72%'::TITLE FROM tasks;
SELECT 6::COL;
SELECT category::CATEGORY, sum(spend)::DONUTCHART, 'Rent is two thirds of spend'::TITLE FROM expenses GROUP BY ALL;
```

```sql dashboard
-- Master-detail: click a SKU row, plot its monthly series
CREATE TABLE ts AS
SELECT 'SKU-' || s AS sku, DATE '2024-01-01' + to_months(m::INTEGER) AS month,
       (20 * s + 3 * m + (hash(s, m) % 9)::INTEGER) AS sales
FROM range(1, 6) a(s), range(12) b(m);

SELECT 5::COL;
SELECT 'single'::ROWSELECT;
SELECT sku AS "SKU" ::TABLE, sum(sales) AS "Total" FROM ts GROUP BY sku ORDER BY "Total" DESC;
SELECT 7::COL;
SELECT month::XAXIS, sales::LINECHART, 'Monthly sales of the selected SKU'::TITLE FROM ts
WHERE sku = COALESCE(NULLIF(getvariable('SKU'), ''),
                     (SELECT sku FROM ts GROUP BY sku ORDER BY sum(sales) DESC LIMIT 1))
ORDER BY month;
```

```sql dashboard
-- Model output from roles: coefficient forest, residuals with a GAM trend
CREATE TABLE coefs AS SELECT * FROM (VALUES
  ('area', 'ols', 3.3, 2.9, 3.7), ('area', 'huber', 3.1, 2.8, 3.4),
  ('age', 'ols', -0.9, -1.4, -0.4), ('age', 'huber', -0.8, -1.2, -0.4)) t(term, model_id, estimate, conf_low, conf_high);
CREATE TABLE fit AS SELECT i AS row_id, i::DOUBLE AS fitted, sin(i / 3.0) + (i % 5 - 2) / 4.0 AS residual,
  abs(sin(i / 7.0)) / 4 AS cooks_d FROM range(1, 60) r(i);

SELECT term::XAXIS, model_id::CATEGORY, estimate::SCATTER, conf_low::YMIN, conf_high::YMAX,
       1::FLIP, 0::REFLINE, 'Area raises the price, age lowers it'::TITLE FROM coefs;
SELECT fitted::XAXIS, residual::SMOOTH, 'gam'::SMOOTH_METHOD, 0::REFLINE,
       row_id::LABEL, 3::LABEL_TOP, cooks_d::RANK, 'Residuals vs fitted'::TITLE FROM fit;
```

```sql dashboard
-- Tabs, a 2-up row with equal heights, a formatted detail table
CREATE TABLE orders AS
SELECT DATE '2024-01-01' + INTERVAL (i) DAY AS day, (ARRAY['EU', 'US', 'APAC'])[1 + i % 3] AS region,
       100 + (hash(i) % 50)::INTEGER AS amount, (i % 7 = 0) AS late
FROM range(90) r(i);

SELECT 'Overview'::TAB;
SELECT 6::COL; SELECT 320::HEIGHT;
SELECT region::XAXIS, sum(amount)::BARCHART, 'EU leads on revenue'::TITLE FROM orders GROUP BY 1 ORDER BY 2 DESC;
SELECT 6::COL; SELECT 320::HEIGHT;
SELECT amount::HISTOGRAM, 'Order values cluster at 100–150'::TITLE FROM orders;
SELECT 'Detail'::TAB;
SELECT region AS "Region" ::TABLE, sum(amount) AS "Revenue" ::MONEY,
       round(100 * avg(late::INTEGER), 1) AS "Late share" ::PERCENT, list(amount ORDER BY day) AS "Trend" ::SPARKLINE,
       'Revenue by region'::TITLE
FROM orders GROUP BY region ORDER BY "Revenue" DESC;
```

```sql dashboard
-- Will we run out of stock before Christmas? (history, nested fan from Today, events, strip, story)
CREATE TABLE outlook AS
SELECT DATE '2026-11-01' + INTERVAL (i) DAY AS day,
       CASE WHEN i <= 20 THEN 'Actual' ELSE 'Forecast' END AS series,
       400 - 9 * i + CASE WHEN i <= 20 THEN (hash(i) % 15)::INTEGER ELSE 0 END AS units,
       CASE WHEN i > 20 THEN 400 - 9 * i - 3 * (i - 20) END AS lo95, CASE WHEN i > 20 THEN 400 - 9 * i + 3 * (i - 20) END AS hi95,
       CASE WHEN i > 20 THEN 400 - 9 * i - (i - 20) END AS lo50, CASE WHEN i > 20 THEN 400 - 9 * i + (i - 20) END AS hi50
FROM range(45) r(i);
CREATE TABLE events AS SELECT * FROM (VALUES (DATE '2026-11-27', 'Black Friday')) t(day, note);
CREATE TABLE promos AS SELECT * FROM (VALUES (DATE '2026-11-24'), (DATE '2026-11-27'), (DATE '2026-12-04')) t(day);

SELECT 12::COL;
SELECT o.day::XAXIS, o.series::CATEGORY, o.units::LINECHART,
       o.lo95::BAND_LOWER, o.hi95::BAND_UPPER, o.lo50::BAND_LOWER, o.hi50::BAND_UPPER,
       e.day::XBAND_FROM, e.day + INTERVAL 3 DAY::XBAND_TO, e.note::XBAND_LABEL,
       CASE WHEN o.units < 50 THEN o.day END::MARKAREA,
       DATE '2026-11-21'::XLINE, 'reveal'::ANIMATE,
       CASE WHEN o.series = 'Forecast' THEN 'flow' END::LINESTYLE,
       CASE o.day WHEN DATE '2026-11-21' THEN 'Today: 220 units left'
                  WHEN DATE '2026-12-12' THEN 'Below 50 units from here — reorder now' END::STORY,
       0::REFLINE, 'Stock runs low by mid-December'::TITLE
FROM outlook o LEFT JOIN events e ON e.day = o.day ORDER BY o.day;
SELECT day::XAXIS, 'Promotions'::TRACK FROM promos;
```

```sql dashboard
-- Where did this quarter's profit come from? (as-of chip, switch, KPI with thresholds, bridge, bullet, funnel, drill-down, notes)
CREATE TABLE loads AS SELECT TIMESTAMPTZ '2026-10-17 06:00:00+00' AS loaded_at;
CREATE TABLE bridge AS SELECT * FROM (VALUES
  (1, 'Budget', 1200.0), (2, 'Volume', 180.0), (3, 'Price', -60.0), (4, 'Mix', 25.0)) t(i, step, keur);
CREATE TABLE attain AS SELECT * FROM (VALUES (1, 'North', 92.0), (2, 'South', 71.0), (3, 'West', 104.0)) t(i, region, pct);
CREATE TABLE funnel AS SELECT * FROM (VALUES (1, 'Visits', 48000), (2, 'Sign-ups', 6200), (3, 'Trials', 4600), (4, 'Paid', 1150)) t(i, stage, n);
CREATE TABLE sales AS SELECT * FROM (VALUES
  ('EU', 'DE', 'Berlin', 410.0), ('EU', 'DE', 'Munich', 260.0), ('EU', 'FR', 'Paris', 330.0),
  ('US', 'US', 'Boston', 520.0), ('US', 'US', 'Austin', 300.0)) t(region, country, city, revenue);

SELECT loaded_at::ASOF, 'stale=24h' FROM loads;
SELECT 'Revenue' AS metric ::BUTTONS UNION ALL SELECT 'Orders';
SELECT false AS eu_only ::TOGGLE, 'EU only'::LABEL;

SELECT 'This quarter'::GROUP;
SELECT 1345.0::METRIC, 1200.0::DELTA, 'vs budget'::DELTA_LABEL, '<1200:bad; <1300:warn; good'::THRESHOLDS,
       'Profit (k€)'::LABEL, 'Operating profit before tax, in thousand euros.'::INFO;
SELECT 'Profit beat budget by **{{gap:money}}**; the margin rule is $m = \frac{p}{r}$.'::MARKDOWN,
       'success'::CALLOUT, 145.0 AS gap;
SELECT 1::ENDGROUP;

SELECT 6::COL;
SELECT step::XAXIS, keur::WATERFALL, '€'::YFORMAT, 'Volume carried the quarter'::TITLE FROM bridge ORDER BY i;
SELECT 6::COL;
SELECT region::XAXIS, pct::BULLET, 100::GOAL, '60,85,110'::RANGE, 'South is short of target'::TITLE FROM attain ORDER BY i;
SELECT 6::COL;
SELECT stage::XAXIS, n::FUNNEL, 'A quarter of sign-ups never start a trial'::TITLE FROM funnel ORDER BY i;
SELECT 6::COL;
SELECT 'region > country > city'::DRILL;
SELECT CASE getvariable('drill_level') WHEN 'region' THEN region WHEN 'country' THEN country ELSE city END::XAXIS,
       CASE getvariable('metric') WHEN 'Orders' THEN count(*)::DOUBLE ELSE sum(revenue) END::BARCHART,
       'tween'::TRANSITION,
       'US leads; click to drill into countries and cities'::TITLE
FROM sales
WHERE getvariable('region') IN ('', region) AND getvariable('country') IN ('', country)
  AND (NOT getvariable('eu_only') OR region = 'EU')
GROUP BY ALL ORDER BY 2 DESC;
```

In the browser the Markdown placeholders and formula render, the drill chart drills on click and tweens when the buttons change; the static `--svg` / PNG output draws panels only.

## Design contract

Full rationale: `docs/dashboard-design.md`. **[lint]** = checked by `--check` (`design/*`).

| You have | Use |
|---|---|
| one headline number | `::METRIC` / `::MONEY` / `::PERCENT` + a `::LABEL` caption |
| a number over time | `::LINECHART` (+ `::SMOOTH` for a trend); ≤ ~6 series |
| a number by a discrete category | `::BARCHART` sorted by value; part of whole → `::BARCHART_STACKED` or `::DONUTCHART` (≤ 6 slices) |
| a distribution | `::HISTOGRAM`, or `::BOXPLOT` / `::VIOLIN` by a category; too many rows to send → a box from statistics (`::Q1` `::MEDIAN` `::Q3`, computed in SQL) |
| two numbers | `::SCATTER` (third as `::BUBBLE` / `::SIZE`) |
| two categories + a measure | `::HEATMAP` |
| a forecast with its uncertainty | `::LINECHART` + one `::BAND_LOWER` / `::BAND_UPPER` pair per level (pivot long quantiles with `max(lower) FILTER (WHERE level = 0.95)`) |
| events / features to read against a time chart (promotions, alarms, price) | `::TRACK` strips right after the chart, not a separate panel |
| progress toward a goal | `::GAUGE` + `::RANGE`; several rows vs their targets → `::BULLET` + `::GOAL` + `::RANGE 'poor,ok,good edges'` |
| how a total moved (budget → actual, last year → this year) | `::WATERFALL`: first row = start, then signed steps (NULL row = subtotal) |
| conversion through ordered stages | `::FUNNEL` (stages in SQL order, first on top) |
| where it happens: by country / state / district code | `code::REGION` + the measure (`'quantile 5'::CLASSES`, `8::LABEL_TOP`); built-in countries, German states and districts — no geodata needed |
| where it happens: by location | `lat::LAT, lon::LON` points over `'world'::BASEMAP` (or `'de-states'`) |
| any other geography (WKT) | `::MAP` |
| a process, pipeline or lineage: steps and what flows between them | `::DIAGRAM` (one row per step; edges from `::UPSTREAM` or `::EDGES`) |
| row-level detail | `::TABLE` (`::PAGED` if large / remote); flag / rank with `::FLAG` / `::HIGHLIGHT`, not colour alone |

- **Inverted pyramid**: KPI strip of 2–6 tiles inside `::GROUP … ::ENDGROUP` **[lint: ungrouped-kpis]** → the primary full-width chart → 2-up breakdowns → a detail table.
- **Title every chart with the takeaway** (`'Revenue up 12% YoY'`, not `'Revenue'`) **[lint: untitled-chart]**; format money / percent axes (`'€'::YFORMAT`).
- **Sort bars by value** unless x is time **[lint: unsorted-bars]**; long labels → `1::FLIP`.
- **No pie / donut past ~6 slices** **[lint: pie-slices]**; ≤ ~7 colour series (top-N + "Other") **[lint: many-series]**.
- **≤ ~7 panels per view**, split topics with `::TAB` **[lint: too-many-panels]**.
- **Don't dump raw rows**: aggregate or top-N a big detail table **[lint: raw-table]**.
- **Placement** (`docs/dashboard-design.md` §7): default to 2-up (`6::COL`), full width for time series and tables; equalise a 2-up row with `::HEIGHT`; title 2-up panels with a `::TITLE` column (a lone `::LABEL` heading forces a new row); a `::GROUP` is always full width (use it for KPI strips and filter boxes, not for side by side); keep the top to one slim row (a bare `::DROPDOWN` filter, no banner repeating the KPIs).
- **Colour is discrete**: band a continuous value with `CASE` before `::COLOR`.
- **Name a dashboard by the business question it answers** (`'Which regions are behind plan?'::DASHBOARD_TITLE`, `-- @title`), never by method or technology. Place it with `-- @group <area>` (` / ` for one sub-level), find it with `-- @tags a, b` (≤10), describe it in one line with `-- @description`, and say who looks after it with `-- @owner <team>` and, once checked, `-- @certified YYYY-MM-DD` (a ✓ badge) header lines. Where the numbers come from (stored model runs, the ETL job) goes in `--` header comments and the README, not as a note or banner on the page; freshness goes in an `::ASOF` chip.
- **Default look**: the built-in graphite theme (`light`); add a `::THEME` only for a real brand.

Complete references: `examples/dashboards/sales-overview.sql` (self-contained, lints clean), `examples/sp500-dashboard/sp500.sql` (cross-filtered), `web/examples/statistics-showcase.sql` (contract plots in ten tabs).

## Templates

Fill the `<placeholders>` from `--describe`, then `--check`.

```sql skip
-- Overview: filter → KPIs → trend → breakdown → table
SELECT <cat> AS <cat> ::DROPDOWN FROM <t> GROUP BY <cat> ORDER BY 1;
SELECT 'Totals'::GROUP;
SELECT sum(<measure>)::MONEY, 'Total'::LABEL FROM <t> WHERE <cat> = getvariable('<cat>');
SELECT count(*)::COMPACT, 'Rows'::LABEL FROM <t> WHERE <cat> = getvariable('<cat>');
SELECT 1::ENDGROUP;
SELECT 12::COL;
SELECT <date>::XAXIS, sum(<measure>)::LINECHART, '€'::YFORMAT, '<takeaway>'::TITLE
FROM <t> WHERE <cat> = getvariable('<cat>') GROUP BY <date> ORDER BY <date>;
SELECT <cat2>::XAXIS, sum(<measure>)::BARCHART, '<takeaway>'::TITLE
FROM <t> WHERE <cat> = getvariable('<cat>') GROUP BY <cat2> ORDER BY 2 DESC;
SELECT 12::COL;
SELECT <cat2> AS "<Cat2>" ::TABLE, sum(<measure>) AS "Total"
FROM <t> WHERE <cat> = getvariable('<cat>') GROUP BY <cat2> ORDER BY "Total" DESC;

-- Distribution: multiselect → histogram + boxplot
SELECT DISTINCT <cat>::MULTISELECT FROM <t> ORDER BY 1;
SELECT <measure>::HISTOGRAM, '<takeaway>'::TITLE FROM <t>
WHERE (len(getvariable('<cat>')) = 0 OR list_contains(getvariable('<cat>'), <cat>));
SELECT <cat>::XAXIS, <measure>::BOXPLOT, '<takeaway>'::TITLE FROM <t>
WHERE (len(getvariable('<cat>')) = 0 OR list_contains(getvariable('<cat>'), <cat>));
```

## Diagrams

One row per node; the `::DIAGRAM` (`::NODE`) column is the id. In the box: `::LABEL`, a KPI format (`::COMPACT`, `::MONEY`, …) and `::BADGE` (status word → pill colour; `::THRESHOLDS` on the value wins). Edges: `inputs::UPSTREAM` (an id or a list of ids feeding this row), or for edges with a count, label or style one scalar subquery:

```sql dashboard
CREATE OR REPLACE TABLE run_manifest AS SELECT * FROM (VALUES
  (1, 'ingest', 'Ingest', 10000, 'ok'), (2, 'build', 'Build twin', 9800, 'ok'),
  (3, 'monitor', 'Monitor drift', 412, 'alarm'), (4, 'calibrate', 'Calibrate', 37, 'warn')
) t(seq, stage, title, records, status);
CREATE OR REPLACE TABLE run_edges AS SELECT * FROM (VALUES
  ('ingest', 'build', 9800, false), ('build', 'monitor', 9800, false),
  ('monitor', 'calibrate', 412, false), ('calibrate', 'build', 29, true)
) t(from_stage, to_stage, n, loop);

SELECT 12::COL; SELECT 320::HEIGHT;
SELECT stage::DIAGRAM, title::LABEL, records::COMPACT, status::BADGE,
       (SELECT list({'from': from_stage, 'to': to_stage, 'value': n, 'style': CASE WHEN loop THEN 'flow' END}) FROM run_edges)::EDGES,
       'Drift sends 29 calibrations back to the twin'::TITLE
FROM run_manifest ORDER BY seq;
```

Layers run left to right (`'TB'::DIRECTION` for top to bottom); loops are drawn as edges running back. Row order breaks ties, so order the rows by the process. Give it `12::COL` and a `::HEIGHT`; a click on a node cross-filters (`'stage'::SELECT_AS` → `getvariable('stage')`), edges are hover-only. Not for free drawings: no positions or shapes yet.

Motion: stages appear one after another by default (`'none'::ANIMATE` = off). `style` in `::EDGES` or `::LINESTYLE` on a row (styles the edges into that node; a constant = every edge): `'flow'` moving dashes, `'flow-dots'` travelling dots, `'dashed'`. `CASE stage WHEN 'monitor' THEN '412 twins drift' END::STORY` makes Step / Play steps that light the node and every edge touching it. Keyboard: the arrow keys follow the connections. Edges with a `value` are as wide as what they carry; a `status` word (`ok` / `delayed` / `failed` / `late`) colours them. A run picker plus `'tween'::TRANSITION` changes widths, colours and values in place. See the example "How does data move through our pipeline?". `team::LANE` groups nodes into labelled swimlanes; `stage_no::XAXIS` pins a node's stage (`::XAXIS` + `::YAXIS` on every node: placed by hand); `list(v ORDER BY t)::SPARKLINE` draws a trend in the box; `'flow-volume'` edges move faster the more they carry. From two tables without dashboard SQL: `SELECT anofox_plot_diagram(n, edges := (SELECT list(e) FROM run_edges e)) FROM run_manifest n` (Grafana names `id/title/mainstat`, `source/target/value`; `stage`, `from_stage`, `to_stage`, `count` read too).

<!-- assist:end -->

## Motion (browser only)

Use motion where it tells the story, not as decoration; `--svg`/PNG/PDF stay static and *reduce motion* shows the final state.
- Default entrance (`'auto'`): lines draw, bars grow by x, points pop in. Opt out per chart with `'none'::ANIMATE`.
- Forecasts: `DATE '…'::XLINE` (Today) + `'reveal'::ANIMATE` — history wipes in, then the fan grows forward from Today.
- `CASE WHEN series = 'Forecast' THEN 'flow' END ::LINESTYLE` — moving dashes for the predicted series (`'dashed'` = still); static output shows dashes.
- `CASE x WHEN 'W48' THEN 'caption' END ::STORY` (on a `::DIAGRAM`: per node) — Step / Play controls that light up one x (or series: `'series'::STORY_BY`) and dim the rest, with the caption. 2–5 steps that each say what to look at and why. Never name it `::STEP` (that is the step-line chart).
- `'tween'::TRANSITION` on a chart an input or cross-filter changes: its marks move to the new values (≤ 300 ms) instead of a swap — the eye follows what changed.

## Accessibility (built in)

Charts are keyboard-navigable (Tab into a chart, arrows across marks, Enter = click, Esc leaves), each gets a generated screen-reader summary, a *Charts as tables* switch swaps every chart for its data, and reduced motion is honoured. The summary uses the panel's `::TITLE` — give every chart one (also `design/untitled-chart`).

## Explore data (builder, before writing SQL)

In the builder (`web/`, `serve <db>`, `anofox_serve`) the sidebar's **Explore data** page lists every table / view / added file (CSV, Parquet, JSON, `https://` URL); a table opens as column cards (type, empty %, distinct, histogram / top values / date range, quality hints, likely joins) over its `::PAGED` rows. **Chart this** previews a chart and **Add to dashboard** appends its annotated SQL; **New dashboard from this table** writes a starter (KPIs, `::ASOF`, `::PERIOD`, a filter, trend, top-N, distribution, rows); **Compare…** diffs two tables or two time ranges and writes a starter with `::DELTA`s; with `anofox_tabular` loaded each hint can **Run the check**. Locked servers have no Explore page. From a terminal, `dashboard --describe` gives the same grounding.

<!-- assist:begin (the builder assistant's guide: scripts/gen-assist-guide.mjs) -->
## Gotchas

These silently break a panel (it renders wrong or vanishes):

1. **Casts go on the main (outermost) SELECT list.** A leading `WITH` is fine (`WITH s AS (…) SELECT x::XAXIS, n::BARCHART FROM s`); casts inside a CTE body or subquery are plain SQL, and DDL / DML (`CREATE … AS SELECT`, `INSERT … SELECT`) is always setup.
2. **`::DATE`, `::TEXT`, `::STRING`, `::NUMERIC` are also SQL types.** They make an input only in an input statement; in a chart they stay casts. Cast and tag by chaining: `ts::DATE::XAXIS`.
3. **One `::TABLE` marker per table**, not one per column; the other columns keep their `AS "Header"` aliases.
4. **`::CATEGORY` / `::COLOR` is discrete**: a raw continuous column gives one legend entry per value.
5. **A KPI caption is `::LABEL`, not `::TITLE`** (`::TITLE` is a title bar).
6. **Don't pre-alias a chart column** (`x AS a ::BARCHART` names the legend entry) unless you mean it; for inputs the alias is the variable name.
7. **Aggregating charts need `GROUP BY ALL`**; a `::ROLE` must be in the SELECT list, not after `FROM`.
8. **Large or remote sources** (parquet, MotherDuck): `::PAGED` instead of `::TABLE`.
9. **Clickable table rows need `'single'::ROWSELECT` / `'multi'::ROWSELECT`** before the table; without it a click does nothing and `getvariable('<first column>')` stays unset. The first column is the key and must be non-numeric (a text id / name). Table directives (`::ROWSELECT`, `::TOTALS`, `::COLGROUP`, `::FLAG`, `::HIGHLIGHT`, `::SUBTOTAL`) apply to the next table only.
10. **No data yet?** Generate it with `range(n)` / `VALUES` in setup statements, as above.
11. **Say what an empty result means**: `SELECT 'No orders in range — widen the dates'::EMPTY;` before a filtered panel replaces the generic "No data for this selection". **Optional outputs** (a file only some runs write): put `-- @requires data/run/x.parquet` on its own line directly above the statement — a missing file then shows "Not available" instead of an error card. Commas = all needed; `a.parquet | b.parquet` = either name will do; `${name}` = `getvariable('name')` (e.g. `-- @requires ${rd}/forecasts.parquet` after `SET VARIABLE rd = getvariable('anofox_data') || '/' || getvariable('run')`); a relative path resolves against `anofox_data` when set. On a locked server `${name}` must be server-side (`anofox_data`, a `path_vars` variable, or set by the dashboard) and the path must stay inside the data folder. Above a `::TRACK` a missing input leaves the strip out. **Optional tabs:** `-- @requires` directly above `SELECT 'Forecastability'::TAB;` (or a `::SUBTAB`) hides the whole tab — button, pane, panels (never run, also on a locked server) — when the input is missing, instead of a tab of "Not available" cards; `?tab=` to it opens the first shown tab. Set the tab's `${name}` above the tab statement.
12. **Annotations are per row.** `::XLINE_LABEL`, `::XBAND_FROM`/`_TO`/`_LABEL` and `::ANNOTATE` read the row they are on: `LEFT JOIN events e ON e.day = s.day` and select `e.day::XLINE, e.note::XLINE_LABEL` — only the rows that carry an event draw it.
13. **`::THRESHOLDS` inside a `::TABLE` statement is just a column.** Put the table's rules in their own statement before it, naming the columns.

<!-- assist:end -->

## Running it

| Where | How |
|---|---|
| Static HTML | `dashboard file.sql [out.html]` (inputs, tabs and directives are skipped) |
| Browser builder | the live demo or `web/` (DuckDB-Wasm, no server): paste the script, **Run**; **Explore data** profiles tables; **Share** puts the SQL and the view in the URL, **⤓ HTML** saves a standalone file |
| Live DuckDB, authoring | `SELECT anofox_serve(8080);` (from-source build) or the `serve <db>` binary |
| To consumers, locked | `SELECT anofox_serve_dashboards('<dir>', 8095);` — each `.sql` file of the folder is a page `/d/<name>` with a shared nav bar; `::TAB` pages deep-link via `?tab=<name>`; `?v.<variable>=<value>` pre-sets an input or a selection (validated against the input's options) |

Modes, options, limits and deployment: the serving skill; login, row / column policies and embedding: the access-control skill.
