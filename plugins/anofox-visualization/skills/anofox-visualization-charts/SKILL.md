---
name: anofox-visualization-charts
description: >
  SQL to SVG with the anofox_visualization DuckDB extension: the
  anofox_render(spec) scalar (rows + ::ROLE annotations as JSON, width /
  height, brand colour, category cap, line downsampling), the column macros
  anofox_bar, anofox_line, anofox_scatter, anofox_area, anofox_xy(x, y,
  kind :=) and anofox_xyc(x, y, series, kind :=), which chart kinds `kind :=`
  accepts (bar, stacked / percent bars, line, area, step, scatter, bubble,
  jitter, histogram, density, boxplot, violin, QQ, pie, donut, radar,
  sparkline, heatmap, calendar), one SVG per GROUP BY group, saving the SVG
  to a file, the error messages, and the community (render-only) build versus
  the full-toolkit C-API build (a GitHub release or from source: render +
  serving). Use when the user wants "a
  chart from this query", "render an SVG in DuckDB", "plot these columns",
  "a bar / line / scatter chart in SQL", asks what anofox_render expects,
  how to install or load anofox_visualization, or why a chart call fails.
  For model output tables (coefficients, residuals, ROC, forecasts) see the
  statistics-plots skill; for ::ROLE dashboards the dashboards skill.
version: 2026.10.18
user-invocable: false
---

# Anofox Visualization — Charts in SQL Cheat Sheet

**Extension:** `anofox_visualization` (CalVer 2026.10.18) | **DuckDB:** v1.5.x (pinned v1.5.6) | **Engine:** [ggplot-rs](https://github.com/sipemu/ggplot-rs), every chart is an SVG string

## Install and builds

| | Community build | Full toolkit: release file or from-source C-API build |
|---|---|---|
| Get it | `INSTALL anofox_visualization FROM community; LOAD anofox_visualization;` (once it is accepted; until then `make` gives the same binary) | the `anofox_visualization.<tag>.duckdb-v1.5.6.<platform>.duckdb_extension` asset of a GitHub release (with the `dashboard` / `serve` CLIs), or `duckext/scripts/build-native.sh --release` → `/tmp/anofox_visualization.duckdb_extension` (`OUT=…`; keep the file name: DuckDB derives the entry point from it) |
| Rendering: `anofox_render`, column macros, `anofox_plot_*`, `anofox_contract_version` | yes | yes |
| Serving: `anofox_serve`, `anofox_serve_dashboards`, `anofox_serve_refresh`, `anofox_serve_stop` | **no** | yes (embeds the web builder; serving skill) |
| Macro descriptions / `tags` in `duckdb_functions()` | yes | no (the C API cannot attach them) |
| DuckDB | exactly the pinned version | any DuckDB with C-API v1.2.0+ |

Release and local builds are unsigned: start `duckdb -unsigned` and `LOAD '/path/anofox_visualization.duckdb_extension';` (Python: `duckdb.connect(config={"allow_unsigned_extensions": "true"})`).

## Functions

| Function | Signature | Returns |
|---|---|---|
| `anofox_render` | `anofox_render(spec VARCHAR)` | SVG `VARCHAR`; `NULL` spec → `NULL`; a bad spec raises `Invalid Input Error: anofox_render: …` |
| `anofox_xy` | `anofox_xy(x, y, kind := 'BARCHART', width := 640, height := 400)` | aggregate macro: one SVG over the rows (per group) |
| `anofox_xyc` | `anofox_xyc(x, y, series, kind := 'BARCHART_STACKED', width := 640, height := 400)` | as above, `series` = colour (`::CATEGORY`) |
| `anofox_bar` / `anofox_line` / `anofox_scatter` / `anofox_area` | `(x, y)` — **no named options** | shorthand for `anofox_xy(x, y, kind := 'BARCHART' / 'LINECHART' / 'SCATTER' / 'AREACHART')` |
| `anofox_contract_version` | `()` | `'1'` (see the statistics-plots skill) |
| `anofox_plot_*` | `(row_alias, …)` | contract plots of model output, and `anofox_plot_diagram(nodes, edges := …)` for a process / pipeline graph — statistics-plots skill |

The macros are aggregates: `list({…} ORDER BY x, y)` makes the row order deterministic, so `GROUP BY` gives one SVG per group.

### `kind :=` values

`kind` is a chart role token (`dashboard --roles` lists all of them, with aliases such as `'BAR'`, `'LINE'`):

| Works with `anofox_xy` | `BARCHART`, `BARCHART_PERCENT`, `LINECHART`, `LINECHART_PERCENT`, `AREACHART`, `STEP`, `SCATTER`, `BUBBLE`, `JITTER`, `SMOOTH` (needs a numeric x), `HISTOGRAM`, `DENSITY`, `BOXPLOT`, `VIOLIN`, `QQ`, `SPARKLINE`, `RADAR`, `GAUGE`, `CALENDAR` (date x) |
| Also with `anofox_xyc` (needs the series) | `BARCHART_STACKED`, `BARCHART_STACKED_PERCENT`, `AREACHART_STACKED`, `PIE`, `DONUTCHART` (slices = `series`) |
| Needs `anofox_render` | `HEATMAP` (`::YAXIS`), `CANDLESTICK` (`::OPEN/HIGH/LOW`), `MAP` (WKT), combo charts (several measures), intervals (`::YMIN/YMAX`), facets, scales, reference lines |

## `anofox_render` spec

```json
{"rows":  [{"c0": "W1", "c1": 30, "c2": 4.5}, …],
 "roles": [[0, "XAXIS"], [1, "BARCHART", "sessions"], [2, "LINECHART", "revenue"]],
 "width": 640, "height": 400, "primary": "2563eb",
 "theme": "dark", "locale": "de-DE",
 "max_categories": 30, "max_line_points": 5000}
```

| Key | Meaning |
|---|---|
| `rows` | array of objects; role `[i, …]` reads key `c{i}` |
| `roles` | `[column_index, "ROLE"(, "legend name")]` — any `::ROLE` of the registry (case-insensitive, aliases allowed) |
| `width`, `height` | integers in 16–8192 (default 640 × 400) |
| `primary` | brand colour `rrggbb` for single-series marks (default: the theme's) |
| `theme` | `light` (default: graphite + electric blue; alias `default`, `datazoo`) · `dark` · `print` · `colorblind` · `high-contrast` · `'a+b'` · a theme JSON object/string; `variant` `light`/`dark` picks the side of an auto theme |
| `locale` | `en-US` (default) · `en-GB` · `de-DE` · `fr-FR`: axis/legend numbers, currency and month names |
| `max_categories` | discrete axis / colour cap (default 30: the largest 29 + "Other"; `0` = off) |
| `max_line_points` | LTTB downsampling threshold per line series (default 5,000) |
| `plot` + `options` | a contract plot instead of `roles` (`terms`, `prediction`, `curve`, `summary`, `obs`, `diagnostics`, `tests`, `compare`, `partial`, `auto`) — what the `anofox_plot_*` macros send |

A `[i, "TITLE"]` role draws a title bar above the chart — its first non-NULL value, as on a dashboard card (the panel keeps `height`; the chart gets 26 px less). A KPI tile (`METRIC`, `MONEY`, …) has no title bar: its caption is `LABEL`.

Limits: 200,000 rows and 64 MiB per spec. Build the spec with `json_object` / `to_json(list({c0: …, c1: …}))` so values are quoted correctly.

## Themes and locale

Every chart macro takes `theme :=` and `locale :=`; unset, they read the session variables `anofox_theme` / `anofox_locale`:

```sql
SET VARIABLE anofox_theme = 'dark';
SET VARIABLE anofox_locale = 'de-DE';
SELECT anofox_bar(x, y) FROM (VALUES ('Q1', 1200.5), ('Q2', 3400.25)) t(x, y);
SELECT anofox_xy(x, y, kind := 'LINECHART', theme := 'colorblind', locale := 'en-US')
FROM (VALUES (1, 10.5), (2, 12.0), (3, 9.75)) t(x, y);
```

Charts wear the theme: its font, muted axis text, a hairline grid, a transparent background, card-coloured gaps between bars / slices / tiles. Tooltips (`data-fvalue` / `data-fx` on each mark) and data labels follow the `YFORMAT` / `XFORMAT` roles and the locale (`$1,234.50`, `1.234,50 €`).

**Heatmaps** (`[i, "XAXIS"], [j, "YAXIS"], [k, "HEATMAP"]`): text axes keep the row order of the spec (first `YAXIS` level = top row, so `ORDER BY` in the `list(…)`); the theme's sequential ramp, or its diverging ramp around 0 when the values span zero; `[k, "DATALABELS"]` prints the formatted value in each cell with automatic contrast.

## Worked examples

```sql
CREATE TABLE sales AS SELECT * FROM (VALUES
  ('2024-01', 'app', 30), ('2024-01', 'web', 22), ('2024-02', 'app', 41),
  ('2024-02', 'web', 28), ('2024-03', 'app', 38), ('2024-03', 'web', 35)) t(month, channel, n);

-- One chart over the table
SELECT anofox_bar(channel, n) FROM sales;

-- Size and kind via anofox_xy (anofox_bar/_line/... take no options)
SELECT anofox_xy(month, n, kind := 'LINECHART', width := 480, height := 300) FROM sales;

-- Coloured by a third column; one SVG per channel with GROUP BY
SELECT anofox_xyc(month, n, channel, kind := 'BARCHART_STACKED') FROM sales;
SELECT channel, anofox_line(month, n) AS svg FROM sales GROUP BY channel ORDER BY channel;

-- Distribution kinds take raw rows
SELECT anofox_xy(channel, n, kind := 'BOXPLOT') FROM sales;
SELECT anofox_xyc(channel, n, channel, kind := 'DONUTCHART') FROM sales;
```

```sql
-- anofox_render: a combo chart (bars + line) and a heatmap
SELECT anofox_render(json_object(
         'rows', to_json(list({c0: week, c1: sessions, c2: revenue} ORDER BY week)),
         'roles', json('[[0,"XAXIS"],[1,"BARCHART","sessions"],[2,"LINECHART","revenue"]]'),
         'width', 640, 'height', 360))
FROM (VALUES ('W1', 30, 12.5), ('W2', 41, 18.0), ('W3', 35, 15.2)) t(week, sessions, revenue);

SELECT anofox_render(json_object(
         'rows', to_json(list({c0: d, c1: h, c2: n})),
         'roles', json('[[0,"XAXIS"],[1,"YAXIS"],[2,"HEATMAP"]]')))
FROM (SELECT d, h, (d * 7 + h * 3) % 11 AS n FROM range(7) a(d), range(24) b(h));

-- a diverging heatmap (values span 0) with dollar labels in the cells; rows top-down in SQL order
SELECT anofox_render(json_object(
         'rows', to_json(list({c0: q, c1: region, c2: delta, c3: '$', c4: ''} ORDER BY r, q)),
         'roles', json('[[0,"XAXIS"],[1,"YAXIS"],[2,"HEATMAP"],[3,"YFORMAT"],[4,"DATALABELS"]]')))
FROM (VALUES (1, 'North', 'Q1', 120.5), (1, 'North', 'Q2', -40.0), (2, 'South', 'Q1', -15.25),
             (2, 'South', 'Q2', 80.0)) t(r, region, q, delta);
```

```sql
-- Save an SVG to a file from SQL (no quoting, no header)
COPY (SELECT anofox_bar(ch, n) FROM (VALUES ('app', 30), ('web', 22)) t(ch, n))
TO 'chart.svg' (FORMAT csv, HEADER false, QUOTE '', ESCAPE '', DELIMITER E'\x01');
```

From the shell: `duckdb -unsigned -noheader -list -c "LOAD …; SELECT anofox_bar(ch, n) FROM t" > chart.svg`. Convert with `rsvg-convert -o chart.png chart.svg`.

## Errors

| Message (`Invalid Input Error: anofox_render: …`) | Cause / fix |
|---|---|
| `unknown role 'BARCHAT'` | typo in `kind :=` or a role name |
| `pie needs a CATEGORY column` | `PIE` / `DONUTCHART` with `anofox_xy`: use `anofox_xyc(x, y, series, kind := 'PIE')` |
| `heatmap needs a YAXIS column` | `HEATMAP` needs `anofox_render` with `[i, "YAXIS"]` |
| `spec needs a 'roles' array, …` | spec without `roles` (and without `plot`) |
| `'width' = 10 is outside [16, 8192]` | width / height out of range |
| `N rows exceed the limit of 200000` | aggregate first (or sample) |
| `ORDER BY non-integer literal has no effect` (Binder) | column macros over constants (`anofox_xy('a', 1)`): pass columns of a table or `VALUES` |

## Gotchas

1. **The column macros take columns, not constants.** They aggregate with `list(… ORDER BY x, y)`; call them over a `FROM`.
2. **Measures are cast to DOUBLE**; `BIGINT`, `HUGEINT` and `DECIMAL` render numerically. `NaN` / `±Inf` are treated as missing and reported on the SVG root as `data-warnings="[…]"` (escaped JSON).
3. **Empty input renders a "No data" SVG**, not an error; a `NULL` spec returns `NULL`.
4. **More than 30 levels** on a discrete axis or colour fold into "Other"; set `max_categories` in an `anofox_render` spec to change that.
5. **`SMOOTH` on a discrete x** draws the points but no trend (render warning): use a numeric x.
6. **Charts are SVG strings.** In a ::ROLE dashboard you do not call these macros: tag the columns instead (dashboards skill).
