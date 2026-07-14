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

| Cast | Meaning |
|------|---------|
| `::XAXIS` (`::X`) | x position |
| `::CATEGORY` (`::SERIES`, `::COLOR`) | grouping / colour series |
| `::LABEL` | a section heading (title-only panel); or a per-mark/feature label on a chart/map |
| `::TITLE` (`::HEADING`) | a title bar above a single panel (chart, table, gauge, …) |
| `::BARCHART` (`::BAR`) | bar chart (measure) |
| `::BARCHART_STACKED` | stacked bar (measure) |
| `::BARCHART_PERCENT` | dodged bars with a percent-formatted y-axis |
| `::BARCHART_STACKED_PERCENT` | bars normalised to 100% per x |
| `::LINECHART` (`::LINE`) | line chart (measure) |
| `::LINECHART_PERCENT` | line chart with a percent-formatted y-axis |
| `::AREACHART` (`::AREA`) | area chart (measure) |
| `::SCATTER` (`::POINT`) | scatter (measure) |
| `::PIE` (`::PIECHART`) | pie — slices by `CATEGORY`, sized by the measure |
| `::DONUTCHART` (`::DONUT`) | donut (pie with a centre hole) |
| `::GAUGE` | single value as a progress arc toward a `::RANGE` (`min,max`); optional `::COLORS` zones |
| `::HISTOGRAM` | histogram of the measure column (binned + counted) |
| `::BOXPLOT` | box plot — `x` = `XAXIS` groups, `y` = the measure (raw rows) |
| `::HEATMAP` | tiles at `XAXIS` × `YAXIS`, coloured by the measure |
| `::SPARKLINE` | a minimal inline trend line (no axes), dot on the latest value |
| `::MAP` (`::GEOMETRY`) | choropleth from a WKT geometry column, coloured by a measure |
| `::REFLINE` (`::YLINE`, `::TARGET`) | a horizontal reference/target line on a chart |
| `::XLINE` | a vertical reference line at an x-position |
| `::BAND_LOWER` / `::BAND_UPPER` | a shaded confidence band around a line |
| `::METRIC` (`::KPI`) | a single big-number KPI (add `::LABEL` caption, `::DELTA` for a trend arrow) |
| `::MONEY`, `::PERCENT`, `::COMPACT` | a KPI with a value format (`$12,220` / `46%` / `1.2K`) |
| `::TEXT_SMALL`, `::TEXT_MEDIUM`, `::TEXT_LARGE` | a single-value text card at the chosen size |
| `::TABLE` | data table — sortable headers, in-cell bars, clickable rows (cross-filter), paginated 50/page client-side. Per-column formatting below. |
| `::PAGED` (`::PAGINATED`) | like `::TABLE` but paged in SQL (`LIMIT`/`OFFSET`+`COUNT(*)`, server-side sort) — fetches one page at a time, for huge results (e.g. a large parquet in S3/MotherDuck) |
| `::TREND` | table column: a coloured ▲/▼ arrow (by sign) |
| `::MONEY` / `::PERCENT` / `::COMPACT` | table column: number format (also KPI formats) |
| `::COLORSCALE` (`::HEAT`) | table column: heatmap-colour the cells light→steel by value |
| `::BADGE` (`::STATUS`) | table column: render text as coloured status pills (good/warn/bad by keyword) |
| `::SPARKLINE` | table column: a mini inline trend line from a numeric array (`list(x ORDER BY y)`) |
| `::DROPDOWN` (`::OPTIONS`) | dropdown input (the column's values become options); add a `::HINT` column for per-option hints |
| `::NUMBER`, `::DATE`, `::TEXT` | number / date / text inputs (the value is the default) |
| `::MULTISELECT` | multi-value picker → a DuckDB list; filter with `list_contains(getvariable('name'), col)` |
| `::DATERANGE` | a from→to date pair (query returns two columns → two variables) |
| `::DOWNLOAD_CSV`, `::DOWNLOAD_XLSX`, `::DOWNLOAD_PDF` | an export button (CSV / Excel of the rows; PDF prints the dashboard) |
| `::HEADER_IMAGE` / `::FOOTER_LINK` | a banner image at the top / a link at the bottom |
| `::PLACEHOLDER` | reserve an empty grid cell |
| `::RELOAD` (`::REFRESH`) | auto-refresh the dashboard every N seconds (the value) |

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

- **A chart panel must not start with `WITH`.** The role detector keys off the
  first `SELECT`, so a leading CTE hides the projection and the statement is
  treated as *setup* (no panel). Use a `FROM (SELECT …)` subquery so the outer
  `SELECT` with the casts comes first, or a setup `CREATE TEMP VIEW`.
- **A table uses one `::TABLE` marker, not one per column.** Tag a single column
  `::TABLE`; the rest keep their `AS "Header"` aliases and all show. (Only inside
  `::TABLE`/`::PAGED`/`::DOWNLOAD_*` may a `::ROLE` follow an `AS` alias.)
- **`::CATEGORY`/`::COLOR` is discrete.** To colour by a continuous value, bucket
  it into a `CASE` band; a raw continuous column yields a per-value legend.
- **A KPI caption is `::LABEL`, not `::TITLE`.** `::METRIC`/`::MONEY`/… render the
  number; add a `::LABEL` column for the caption (`::TITLE` is a panel title bar).
- **A `::ROLE` must be in the `SELECT` list**, not after `FROM`; aggregating
  charts usually need `GROUP BY ALL`.

---

## 2. Three ways to run it

### a) CLI runner (native) — fastest to try

Renders an interactive HTML dashboard by shelling out to the `duckdb` CLI (no
bundled DuckDB compile).

```sh
cargo run --bin dashboard -- dashboards/sessions.sql   # → dashboard.html
xdg-open dashboard.html
```

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
./target/debug/serve mydata.duckdb          # opens http://127.0.0.1:8080
#   --port N     choose the port
#   --no-open    don't launch a browser
```

The UI **auto-detects**: if a `/query` bridge answers it uses live DuckDB,
otherwise it falls back to DuckDB-Wasm (mode ii). Same editor, same rendering.
*(Next: `CALL anofox_serve()` to launch this from inside a DuckDB session — see
the roadmap.)*

### c) DuckDB extension — launch the UI *from a DuckDB session*

The `anofox-visualization` C-API extension (in `duckext/`) adds
`anofox_serve(port)`: start the browser builder wired to the **current**
session, from inside DuckDB.

```sql
LOAD 'anofox_visualization.duckdb_extension';  -- (duckdb -unsigned; see duckext/BUILD.md)
SELECT anofox_serve(8080);                    -- serves http://127.0.0.1:8080 + opens the browser
```

The extension embeds the same UI and answers `/query` on a live connection
(reused serially), so panels render your **actual session tables** — big data
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

Same interactive client, but locked: the editor is gone and `POST /query` is
**allow-listed** to each dashboard's own planned panel SQL (plus validated
`SET VARIABLE`s) — arbitrary SQL is rejected `403`. It is **read-only by
construction**: at startup it snapshots the live database and serves through a
fresh read-only DuckDB handle, so even a gate bypass can't write (it serves that
snapshot; re-run to refresh). Requests are self-contained, so it is multi-user
safe.

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
 │     │ plan(sql)         (anofox-visualization-wasm)                   │
 │     ▼                                                     │
 │  DuckDB-Wasm  ──rows──▶  render_panel(rows, roles)  ─SVG─▶ dashboard
 └───────────────────────────────────────────────────────────┘
```

Two wasm exports (`src/wasm.rs`): `plan(script)` returns the statements + roles;
`render_panel(rows_json, roles_json, w, h)` returns SVG. The SQL parsing in
`src/sql.rs` is shared with the native bin, so the CLI and browser behave
identically.

---

## 5. Architecture

```
src/lib.rs    core: Role/Kind/Column + render() → SVG (via ggplot-rs)
src/sql.rs    ::ROLE + statement parsing (shared native/wasm)
src/wasm.rs   wasm-bindgen: plan() + render_panel()   (feature = "wasm")
src/bin/…     dashboard CLI runner (duckdb CLI + shared sql module)
web/          no-server browser builder (index.html + app.js + pkg/)
duckext/      DuckDB C-API extension (native + wasm side-module)
```

## 6. Known limitations

- `::BARCHART` *with* a `::CATEGORY` currently stacks rather than dodges — use
  `::BARCHART_STACKED`, or a `::BARCHART` without a category.
- One panel = one measure. Multi-measure / mixed-geom panels are future work.
- No cross-panel filtering yet (see the roadmap notes).
