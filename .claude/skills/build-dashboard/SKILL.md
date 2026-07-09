---
name: build-dashboard
description: Build an interactive SQL dashboard for anofox-visualization from a plain-text request. Use when the user asks to "build/make/create a dashboard", "visualize this data", or describes charts/KPIs/tables they want. Produces annotated DuckDB SQL where result columns are tagged with `::ROLE` casts.
---

# Build an anofox-visualization dashboard

anofox-visualization turns **annotated SQL** into an interactive dashboard. You
write ordinary DuckDB SQL and tag output columns with `::ROLE` casts; each
annotated `SELECT` becomes a panel (chart / KPI / table / input). Statements
with **no** role (e.g. `CREATE TABLE`, `SET`) run as setup.

Your job: given a plain-text request (and, ideally, the table schema), output a
single SQL script that renders the requested dashboard.

## Workflow

1. **Know the data.** If you don't have the schema, ask for it or run
   `DESCRIBE <table>` / `SUMMARIZE <table>` first. Never invent column names.
2. **Pick panels.** Map the request to panels: KPIs for headline numbers, a
   line for trends over time, bars for category comparisons, a table for detail,
   inputs for filtering.
3. **Lay it out.** Set `SELECT n::COLUMNS;` for the grid, use `::COL` for
   per-panel width, group KPIs in a `::GROUP` box, use `::TAB`/`::SUBTAB` for
   sections.
4. **Write the SQL.** One annotated `SELECT` per panel, in top-to-bottom order.
5. **Add interactivity** if asked: inputs (`::DROPDOWN`/`::MULTISELECT`/…) and
   cross-filter (`getvariable('selected')`).

## The cast is on the OUTPUT column

`sum(revenue)::MONEY` means "this output column is a money KPI". The cast must
sit in the SELECT list (not after FROM). Don't pre-alias a cast column
(`x AS a ::BARCHART` breaks) except inside `::TABLE`/`::PAGED`/`::DOWNLOAD_*`.

## Roles reference

### Chart kinds (put on the measure column)
`::BARCHART` `::BARCHART_STACKED` `::BARCHART_PERCENT` `::BARCHART_STACKED_PERCENT`
· `::LINECHART` `::LINECHART_PERCENT` · `::AREACHART` · `::SCATTER` · `::PIE`
· `::DONUTCHART` · `::GAUGE` (+ `::RANGE 'min,max'`, optional `::COLORS '#a,#b'`
zones) · `::HISTOGRAM` (of the measure) · `::BOXPLOT` (x groups, y measure)
· `::HEATMAP` (x×y tiles coloured by measure) · `::SPARKLINE` · `::MAP` (WKT
geometry choropleth).

### Axes / encoding
`::XAXIS` `::YAXIS` · `::CATEGORY` (colour/series; also `::COLOR`) · `::LABEL`
(section heading when alone; per-mark label otherwise) · `::TITLE` (a panel's
title bar). Combo chart = extra measure columns, e.g.
`week::XAXIS, sales::BARCHART, revenue/50::LINECHART`.

### Annotations on a chart
`::REFLINE`/`::YLINE` (horizontal line at the value) · `::XLINE` (vertical) ·
`::BAND_LOWER` + `::BAND_UPPER` (shaded band around a line).

### KPIs & text
`::METRIC` (plain big number) · `::MONEY` · `::PERCENT` · `::COMPACT` (1.2K) ·
`::DELTA` (a comparison value → trend arrow) · `::TEXT_SMALL`/`_MEDIUM`/`_LARGE`
(a text card). Add `::LABEL` for the caption.

### Tables (`::TABLE`, or `::PAGED` for huge data)
Sortable, in-cell bars, clickable rows. Per-column formatting: `::MONEY`
`::PERCENT` `::COMPACT` (number formats) · `::COLORSCALE` (green→red heatmap
cells) · `::BADGE` (status pills) · `::TREND` (▲/▼ arrow) · `::SPARKLINE`
(mini chart from a `list(x ORDER BY y)` column). `::PAGED` pages in SQL
(LIMIT/OFFSET) — use it for large / remote (parquet, MotherDuck) tables.

### Inputs (become DuckDB variables)
`::DROPDOWN` (single) · `::MULTISELECT` (a list) · `::NUMBER` · `::DATE` ·
`::TEXT` · `::DATERANGE` (two date columns → from/to). The **output column
name** is the variable name — reference it with `getvariable('name')` (a
multiselect is a list → `list_contains(getvariable('name'), col)`). A daterange
returns two columns bound to two pickers.

### Layout & chrome
`SELECT n::COLUMNS;` grid columns · `SELECT n::COL;` next panel width (of 12) ·
`SELECT n::HEIGHT;` next panel height in px · `SELECT n::SPAN;` alias of COL ·
`SELECT 'Box'::GROUP; … SELECT 1::ENDGROUP;` wrap panels in a box (KPIs render
as a compact strip) · `SELECT 'Name'::TAB;` / `::SUBTAB` nested tabs ·
`::PLACEHOLDER` empty cell · `::HEADER_IMAGE` / `::FOOTER_LINK` · `::RELOAD n`
auto-refresh · `::DOWNLOAD_CSV`/`_XLSX`/`_PDF` export buttons.

## Cross-filter (click to filter)

Clicking a chart mark or a table row sets two variables:
- `getvariable('selected')` — the generic last-clicked value.
- `getvariable('<first column name of the clicked table>')` — a **named**
  cross-filter, so two tables give two independent live selections.

A panel reacts only if its query references the variable. Opt in with:
`WHERE getvariable('selected') IN ('', channel)` (`''` = nothing selected →
show all). Combine several: `(COALESCE(getvariable('sku'),'')='' OR sku=getvariable('sku'))`.
Cross-filter SOURCES should NOT self-filter (so they stay clickable); TARGETS
opt in. Put the category column FIRST in a table so row-clicks key off it.

## Worked examples

**"Show weekly revenue and sessions with KPIs, filterable by channel."**
```sql
-- (assume table sales(week, channel, n, revenue))
SELECT 'Filter'::GROUP;
SELECT DISTINCT channel::MULTISELECT FROM sales ORDER BY channel;
SELECT 1::ENDGROUP;

SELECT 'Weekly performance'::LABEL;
SELECT 4::COL; SELECT sum(revenue)::MONEY, 'Revenue'::LABEL FROM sales
  WHERE list_contains(getvariable('channel'), channel);
SELECT 4::COL; SELECT sum(n)::COMPACT, 'Sessions'::LABEL FROM sales
  WHERE list_contains(getvariable('channel'), channel);

SELECT 12::COL;
SELECT week::XAXIS, channel::CATEGORY, sum(revenue)::BARCHART_STACKED, 'Revenue by week'::TITLE
FROM sales WHERE list_contains(getvariable('channel'), channel)
GROUP BY ALL ORDER BY week, channel;
```

**"A gauge of goal completion and a donut of spend by category."**
```sql
SELECT 6::COL;
SELECT sum(done)::GAUGE, '0,100'::RANGE, '#e03131,#efc94c,#0ca678'::COLORS, 'Completion'::TITLE FROM tasks;
SELECT 6::COL;
SELECT category::CATEGORY, sum(spend)::DONUTCHART, 'Spend by category'::TITLE FROM expenses GROUP BY ALL;
```

**"Master–detail: pick a SKU, plot its monthly series."**
```sql
SELECT 5::COL;
SELECT sku, sum(sales) AS total ::TABLE FROM ts GROUP BY sku ORDER BY total DESC;  -- click a row
SELECT 7::COL;
SELECT month::XAXIS, sales::LINECHART, 'Monthly sales'::TITLE FROM ts
WHERE sku = COALESCE(NULLIF(getvariable('sku'),''), (SELECT sku FROM ts ORDER BY sales DESC LIMIT 1))
ORDER BY month;
```

## Gotchas
- Aggregating charts usually need `GROUP BY ALL`.
- A `::ROLE` must be in the SELECT list, not after `FROM`.
- Don't invent columns — inspect the schema first (`DESCRIBE`/`SUMMARIZE`).
- For very large tables use `::PAGED`, not `::TABLE`.
- Generate data ad-hoc with `range(n)` / `VALUES` if the user has no table yet.
