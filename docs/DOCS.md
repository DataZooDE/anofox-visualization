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
| `::BARCHART` | `::BAR` | chart | bar chart (dodged by CATEGORY) |
| `::BARCHART_STACKED` | `::BAR_STACKED`, `::STACKED_BAR` | chart | stacked bars (by CATEGORY) |
| `::BARCHART_PERCENT` | `::BAR_PERCENT` | chart | dodged bars, percent y-axis |
| `::BARCHART_STACKED_PERCENT` | `::BAR_STACKED_PERCENT` | chart | bars normalised to 100% per x |
| `::LINECHART` | `::LINE` | chart | line chart |
| `::LINECHART_PERCENT` | `::LINE_PERCENT` | chart | line chart, percent y-axis |
| `::STEP` | `::STEPLINE`, `::STEP_LINE` | chart | step line |
| `::SMOOTH` | `::TRENDLINE`, `::TREND_LINE` | chart | scatter + LOESS trend line |
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
| `::QQ` | `::QQPLOT` | chart | normal quantile-quantile plot |
| `::HEATMAP` | `::TILE`, `::TILES` | chart | tiles at XAXIS×YAXIS coloured by the measure |
| `::CALENDAR` | `::CALENDAR_HEATMAP`, `::CAL_HEATMAP` | chart | calendar heatmap (date XAXIS, ≤ 50 years) |
| `::CANDLESTICK` | `::CANDLE`, `::OHLC` | chart | OHLC candlesticks: XAXIS + ::OPEN/::HIGH/::LOW, close as the measure |
| `::SPARKLINE` | `::SPARK` | chart | minimal trend line (no axes); a list() column in a table |
| `::MAP` | `::GEOMETRY`, `::GEO`, `::CHOROPLETH` | chart | WKT-geometry map, coloured by a measure |
| `::BASEMAP` | `::MAPBASE`, `::BACKDROP` | chart | grey WKT backdrop layer under a ::MAP |
| `::REFLINE` | `::TARGET`, `::GOAL`, `::YLINE` | annotation | horizontal reference line per distinct value |
| `::XLINE` |  | annotation | vertical reference line at an x |
| `::BAND_LOWER` | `::BANDLOWER` | annotation | lower edge of a shaded band around a line |
| `::BAND_UPPER` | `::BANDUPPER` | annotation | upper edge of a shaded band |
| `::MARKAREA` | `::MARK_AREA`, `::SHADE` | annotation | shade the x-region [min, max] of this column |
| `::DATALABELS` | `::DATALABEL`, `::VALUELABELS`, `::SHOWLABELS` | annotation | draw the value on each mark (value = font size) |
| `::FLIP` | `::COORD_FLIP`, `::HORIZONTAL` | modifier | swap the axes (horizontal bars) |
| `::YFORMAT` | `::YAXISFORMAT`, `::YUNIT`, `::YCURRENCY` | modifier | y-axis tick format: '€', '$', 'percent', 'comma', ' kg'… |
| `::XFORMAT` | `::XAXISFORMAT`, `::XUNIT`, `::XCURRENCY` | modifier | x-axis tick format (continuous x), like ::YFORMAT |
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
| `::COLORSCALE` | `::COLOURSCALE`, `::HEAT`, `::GRADIENT` | table | heatmap-colour a table column's cells |
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
| `::GROUP` | `::BOX`, `::ROW` | layout | open a box; ::ENDGROUP closes it |
| `::ENDGROUP` | `::ENDBOX`, `::ENDROW` | layout | close the current box |
| `::TAB` | `::PAGE` | layout | start a tab/page |
| `::SUBTAB` | `::SUB_TAB` | layout | a nested tab inside a ::TAB |
| `::PLACEHOLDER` |  | layout | an empty grid cell |
| `::RELOAD` | `::REFRESH` | chrome | auto-refresh interval (seconds) |
| `::HEADER_IMAGE` | `::HEADERIMAGE` | chrome | banner image URL |
| `::FOOTER_LINK` | `::FOOTERLINK` | chrome | link at the bottom |
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

Inputs work in the **browser builder** and **`serve`** (they re-query on change);
the static CLI runner skips them.

### Combo charts, auto-refresh, dark mode

A panel with **multiple measure columns** overlays them as combo layers, e.g.
`SELECT week::XAXIS, sessions::BARCHART, revenue::LINECHART`. The toolbar also has
an **auto-refresh** interval and a **dark-mode** toggle.

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

Rendered panels carry an SVG `<title>` per mark. Both the CLI output and the
browser builder attach a small hover layer that shows a styled tooltip
(`web: 22`) and highlights the mark. It’s pure DOM — no chart runtime, works on
static HTML.

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
| `render_panel(rows_json, roles_json, width, height, primary, zoom_json)` | one panel → SVG. `primary` = brand `rrggbb` (or `""`), `zoom_json` = `[x0,x1,y0,y1]` (or `""`). Errors come back as a small error SVG. |
| `map_bounds(rows_json, roles_json)` / `panel_bounds(…)` | data extents for the zoom UI |
| `roles_json()` | the role registry + derived role sets (the browser's single source) |
| `format_number(value, fmt)` | KPI/table number formatting shared with the headless renderer |

The SQL parsing in `src/sql.rs` is shared with every native host, so the CLI,
`serve`, the DuckDB extension and the browser behave identically. Every export
is wrapped so failures return an error value; note that on
`wasm32-unknown-unknown` a panic aborts rather than unwinds, so the real
guarantee is that the core never panics on user input (fuzz-tested).

### Host API (Rust)

- `render_spec_checked(json) -> Result<String, RenderError>` — the checked
  entry point for hosts (the DuckDB extension's FFI): `BadSpec` / `Render` /
  `Panic` (a caught internal panic). `render_spec(json) -> String` keeps the old
  shape (SVG or an escaped `<pre>error</pre>`).
- `render_with(&cols, w, h, &RenderOptions)` — brand colour, zoom window,
  category cap and LTTB threshold passed explicitly (`render(&cols, w, h)` and
  `set_brand`/`set_panel_zoom` remain as compatibility wrappers).
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
