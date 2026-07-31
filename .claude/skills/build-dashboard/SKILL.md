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

1. **Ground on the schema — required.** Never invent column names or guess types.
   Run `dashboard --describe <source>` (a table, `'file.parquet'`, or
   `read_csv(...)`) — it prints each column's type, min/max, **approx-distinct**
   (low = discrete → bar/category; high = search/paged), and null %. Use it to
   pick the right chart per column (time → line, discrete category → bar,
   numeric → histogram/box, geo → map).
2. **Pick panels.** Map the request to panels: KPIs for headline numbers, a
   line for trends over time, bars for category comparisons, a table for detail,
   inputs for filtering.
3. **Lay it out.** Set `SELECT n::COLUMNS;` for the grid, use `::COL` for
   per-panel width, group KPIs in a `::GROUP` box, use `::TAB`/`::SUBTAB` for
   sections. **Follow the placement do/don'ts** in
   [`docs/dashboard-design.md` §7](../../docs/dashboard-design.md) — especially:
   keep the top to one slim row (a bare `::DROPDOWN` filter, no banner that
   repeats the KPIs), title 2-up panels with a `::TITLE` column (not a full-width
   `::LABEL` that stacks them), and equalise a row's heights with `::HEIGHT`.
4. **Write the SQL.** One annotated `SELECT` per panel, in top-to-bottom order.
5. **Add interactivity** if asked: inputs (`::DROPDOWN`/`::MULTISELECT`/…) and
   cross-filter (`getvariable('selected')`).
6. **Validate & repair — required. Do not finish on unvalidated SQL.** Run
   `dashboard --check <file.sql>` (add `--json` for structured output). It runs
   every statement and reports two classes of problem:
   - **Correctness (`error`) — must be zero.** `silent-setup` (a panel that lost
     its roles — usually a leading `WITH`), `sql-error`, `render-error` (missing
     required aesthetics), `empty-panel` (0 rows), `unknown-cast` (a typo'd
     `::ROLE`). These are invisible in the output — the linter is how you catch
     them.
   - **Design (`design/*` warnings) — resolve or justify each.** `pie-slices`,
     `unsorted-bars`, `untitled-chart`, `many-series`, `too-many-panels`,
     `ungrouped-kpis`, `raw-table`. Fix each and re-run until it prints **clean**
     (exit 0). CI can gate on design too with `--strict`; `--no-design` silences
     the advisory pass.
7. **Self-critique against the design contract.** Correctness is not taste. Hold
   the dashboard to the **Design contract** below (full rationale:
   [`docs/dashboard-design.md`](../../docs/dashboard-design.md)). Refine, then
   re-run `--check` until both classes are clean.
8. **Visual review — required. `--check` never looks at the render.** Serve and
   screenshot the dashboard, then grade the *image*: `scripts/shoot.sh --dash
   <dir> [--init <file> --workdir <dir>] --tabs "A,B" --out /tmp/shots`. Review
   each PNG against [`docs/dashboard-visual-review.md`](../../docs/dashboard-visual-review.md)
   — a vision agent (or you, looking at the images) returns
   `{severity, panel, issue, fix}`; fix every `blocker`/`major` (clipped labels,
   colliding colours, locale-broken numbers, empty panels, no hierarchy) and
   re-shoot until clean. Only now is the dashboard done. Save the reviewed PNGs
   as the golden baseline next to the dashboard.

## The cast is on the OUTPUT column

`sum(revenue)::MONEY` means "this output column is a money KPI". The cast must
sit in the SELECT list (not after FROM). Don't pre-alias a cast column
(`x AS a ::BARCHART` breaks) except inside `::TABLE`/`::PAGED`/`::DOWNLOAD_*`.

**Only use real role tokens.** `dashboard --roles` prints the authoritative,
always-current list (grouped, from the engine itself) — use it instead of
guessing. `--check` warns on any unrecognised `::ROLE` (a typo silently drops
that column's role).

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

## Design contract

Correctness (it lints clean) is not taste. Hold to these — full rationale and
sources in [`docs/dashboard-design.md`](../../docs/dashboard-design.md). Items
marked **[lint]** are enforced by `dashboard --check` (`design/*`).

**Pick the chart from the data (use `--describe` for cardinality):**

| You have | Use |
|---|---|
| one headline number | `::METRIC` / `::MONEY` / `::PERCENT` + a `::LABEL` caption |
| a number over time (date x) | `::LINECHART` (+ `::SMOOTH` for trend); keep ≤ ~6 series |
| a number by a discrete category | `::BARCHART`; part-of-whole → `::BARCHART_STACKED` or `::DONUTCHART` (≤ 6 slices) |
| the distribution of a number | `::HISTOGRAM`, or `::BOXPLOT` split by a category |
| the relationship of two numbers | `::SCATTER` (size a 3rd → `::BUBBLE`) |
| two categories + a measure | `::HEATMAP` |
| progress toward a goal | `::GAUGE` + `::RANGE 'min,max'` |
| geography (WKT geometry) | `::MAP` |
| row-level detail / many columns | `::TABLE` (`::PAGED` if large/remote) |

**Compose it well (inverted pyramid):**

- **Lead with a KPI strip** — 2–6 headline tiles wrapped in `::GROUP … ::ENDGROUP`
  (not scattered top-level cards) — *then* the primary full-width chart that
  answers the main question, *then* 2-up breakdowns, *then* a detail table.
  **[lint: ungrouped-kpis]**
- **Filters up top**, inside a `::GROUP` box, and actually wire them: every panel
  that should react must reference `getvariable('<name>')`.
- **Title every chart** with the *takeaway* (`::TITLE 'Revenue up 12% YoY'`, not
  `'Revenue'`) and **format money/percent axes** (`::YFORMAT '€'`). **[lint: untitled-chart]**
- **Sort bars by value** (`ORDER BY <measure> DESC`) unless x is time; long
  category labels → `::FLIP` horizontal bars. **[lint: unsorted-bars]**
- **No pie/donut past ~6 slices** (use a sorted bar); **≤ ~7 colour series**
  (top-N + "Other" beyond). **[lint: pie-slices, many-series]**
- **Stay focused**: ≤ ~7 panels per view; split topics with `::TAB` rather than
  one endless page. Full-width (`::COL 12`) for time series/heatmaps; 2-up
  (default) for comparisons. **[lint: too-many-panels]**
- **Don't dump raw rows** — `::TABLE` paginates the view but loads every row;
  cut a big detail table to a top-N or aggregate (`::PAGED` for remote sources).
  **[lint: raw-table]**
- **Colour is discrete** — band a continuous value into a `CASE` before `::COLOR`.

A complete, well-composed reference dashboard (self-contained, lints clean):
`examples/dashboards/sales-overview.sql`.

## Templates

Start from the shape that fits, then fill the `<placeholders>` from the schema.

**Overview** — filter → KPIs → trend → breakdown → table:
```sql
SELECT 'Filter'::GROUP;
SELECT <cat> AS <cat> ::DROPDOWN FROM <t> GROUP BY <cat> ORDER BY 1;
SELECT 1::ENDGROUP;
SELECT SUM(<measure>)::MONEY, 'Total'::LABEL FROM <t> WHERE <cat> = getvariable('<cat>');
SELECT 12::SPAN;
SELECT <date>::XAXIS, SUM(<measure>)::LINECHART, '€'::YFORMAT, 'Trend'::TITLE
FROM <t> WHERE <cat> = getvariable('<cat>') GROUP BY <date> ORDER BY <date>;
SELECT <cat2>::XAXIS, SUM(<measure>)::BARCHART, 'By <cat2>'::TITLE
FROM <t> WHERE <cat> = getvariable('<cat>') GROUP BY <cat2> ORDER BY 2 DESC;
SELECT 12::SPAN;
SELECT <cat2> AS "<Cat2>" ::TABLE, SUM(<measure>) AS "Total"
FROM <t> WHERE <cat> = getvariable('<cat>') GROUP BY <cat2> ORDER BY "Total" DESC;
```

**Master–detail** — click a row to drive a chart:
```sql
SELECT 5::COL;
SELECT <id> AS "<Id>" ::TABLE, SUM(<measure>) AS total FROM <t> GROUP BY <id> ORDER BY total DESC;
SELECT 7::COL;
SELECT <date>::XAXIS, <measure>::LINECHART, 'Detail'::TITLE FROM <t>
WHERE <id> = COALESCE(NULLIF(getvariable('<id>'),''),
                      (SELECT <id> FROM <t> GROUP BY <id> ORDER BY SUM(<measure>) DESC LIMIT 1))
ORDER BY <date>;
```

**Distribution** — multiselect → histogram + boxplot:
```sql
SELECT 'Filter'::GROUP;
SELECT DISTINCT <cat>::MULTISELECT FROM <t> ORDER BY 1;
SELECT 1::ENDGROUP;
SELECT <measure>::HISTOGRAM, 'Distribution'::TITLE FROM <t>
WHERE (len(getvariable('<cat>'))=0 OR list_contains(getvariable('<cat>'), <cat>));
SELECT <cat>::XAXIS, <measure>::BOXPLOT, 'By <cat>'::TITLE FROM <t>
WHERE (len(getvariable('<cat>'))=0 OR list_contains(getvariable('<cat>'), <cat>));
```

## Gotchas

**These silently break a panel — it renders wrong or vanishes. Get them right:**
- **A chart panel must NOT start with `WITH`.** The role detector keys off the
  *first* `SELECT`, so a leading CTE hides the projection and the whole statement
  is treated as **setup** (no panel appears). Push the CTE into a
  `FROM (SELECT …)` subquery so the outer `SELECT` (with the casts) comes first,
  or into a setup `CREATE TEMP VIEW`.
- **A table takes ONE `::TABLE` marker**, not one per column. Tag a single column
  `::TABLE`; the others keep their `AS "Header"` aliases and all still show.
- **`::CATEGORY`/`::COLOR` is DISCRETE.** To colour by a continuous value, bucket
  it into a `CASE` band (`'at risk'/'watch'/'ok'`); a raw continuous column makes
  an ugly per-value legend.
- **A KPI caption is `::LABEL`, not `::TITLE`.** `::METRIC`/`::MONEY`/… show only
  the number; add a `::LABEL` column for the caption. (`::TITLE` is a title *bar*
  and doesn't caption a metric tile.)
- **"Nothing selected = all"** for a multiselect: guard the filter with
  `(len(getvariable('name'))=0 OR list_contains(getvariable('name'), col))`.

**General:**
- Aggregating charts usually need `GROUP BY ALL`.
- A `::ROLE` must be in the SELECT list, not after `FROM`.
- Don't invent columns — inspect the schema first (`DESCRIBE`/`SUMMARIZE`).
- For very large tables use `::PAGED`, not `::TABLE`.
- Generate data ad-hoc with `range(n)` / `VALUES` if the user has no table yet.

## Serving the dashboard

Your output is a `.sql` script — here's how it gets run:
- **Authoring / local:** `SELECT anofox_serve(port)` in a DuckDB session opens the
  full builder wired to your live tables.
- **To consumers (locked, read-only):** put the `.sql` file(s) in a folder and
  `SELECT anofox_serve_dashboards('<dir>', <port>)`. Same UI, but the editor is
  removed and `/query` is allow-listed to the dashboards' own SQL and served
  read-only (a snapshot). Each file is a page at `/d/<name>`; a folder of files
  gets a shared cross-dashboard nav bar.
- **Multi-page:** `::TAB` (alias `::PAGE`; `::SUBTAB` nests) makes pages *within* a
  dashboard; the active tab is deep-linkable via `?tab=<name>`.

Trust model + deployment: `docs/secure-serving.md`.
