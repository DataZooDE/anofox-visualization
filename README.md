<h1 align="center">anofox-visualization</h1>
<p align="center"><b>Charts &amp; dashboards for DuckDB — the grammar of graphics, straight from SQL.</b></p>

<p align="center">
  <a href="LICENSE"><img alt="License: BSL 1.1" src="https://img.shields.io/badge/license-BSL%201.1-blue"></a>
  <img alt="DuckDB" src="https://img.shields.io/badge/DuckDB-v1.2%2B-yellow?logo=duckdb&logoColor=black">
  <a href="https://sipemu.github.io/anofox-visualization/"><img alt="Live demo" src="https://img.shields.io/badge/live%20demo-online-2ea44f"></a>
</p>

> ⚠️ **Early development.** APIs and the extension packaging are still moving; expect rough edges.

You write ordinary **DuckDB SQL** and tag output columns with `::ROLE` casts.
Each annotated `SELECT` becomes a **panel** — a chart, KPI, table, or input;
statements with no cast (`CREATE`, `SET`, …) run as setup. The columnar result
is mapped onto the [ggplot-rs](https://github.com/sipemu/ggplot-rs)
grammar-of-graphics engine and rendered to **SVG**. No JS charting library, no
bespoke config format — the SQL *is* the dashboard.

```sql
SELECT 'Sessions per week'::LABEL;
SELECT week::XAXIS, channel::CATEGORY, sum(n)::BARCHART_STACKED
FROM sessions GROUP BY ALL ORDER BY ALL;
```

**[▶ Live demo](https://sipemu.github.io/anofox-visualization/)** — the full
builder running in your browser on DuckDB-Wasm, with a gallery of every chart
kind below.

## What you can build

The `::ROLE` on a column decides how it's drawn. In full:

- **Charts** (cast the measure column) — bars (`::BARCHART`, `_STACKED`,
  `_PERCENT`, `_STACKED_PERCENT`), lines (`::LINECHART`, `_PERCENT`, `::STEP`,
  `::SMOOTH` trend), `::AREACHART`, `::SCATTER`, `::PIE`/`::DONUTCHART`,
  `::GAUGE` (with `::RANGE` + `::COLORS` zones), `::RADAR`, `::HISTOGRAM`,
  `::DENSITY`, `::BOXPLOT`, `::VIOLIN`, `::HEATMAP` (x×y tiles), calendar
  heatmap, `::CANDLESTICK`/`::OHLC`, `::SPARKLINE`, and **maps**
  (`::MAP` — WKT-geometry choropleths). **Combo charts** = several measure
  columns on one x-axis. Overlay `::REFLINE`/`::XLINE` reference lines and
  `::BAND_LOWER`/`::BAND_UPPER` confidence bands.
- **KPIs & text** — big-number tiles (`::METRIC`, `::MONEY`, `::PERCENT`,
  `::COMPACT`) with a `::LABEL` caption and a `::DELTA` trend arrow; text cards
  (`::TEXT_*`); Markdown boxes (`::MARKDOWN`).
- **Tables** — sortable, with in-cell bars, per-column number formats,
  heatmap-coloured cells (`::COLORSCALE`), status pills (`::BADGE`), trend arrows
  (`::TREND`), and in-cell sparklines. `::PAGED` pages huge/remote tables in SQL;
  `::DOWNLOAD_CSV`/`_XLSX`/`_PDF` add export buttons.
- **Inputs** — each becomes a DuckDB variable read with `getvariable('name')`:
  `::DROPDOWN`, `::MULTISELECT` (searchable past 50 options), `::NUMBER`,
  `::DATE`, `::TEXT`, `::DATERANGE`. Moving a control re-runs the panels that
  reference it.
- **Layout** — a 12-column grid (`::COLUMNS`/`::COL`/`::SPAN`/`::HEIGHT`),
  `::GROUP` boxes, tabbed pages (`::TAB`/`::PAGE`, nested `::SUBTAB`), header
  image / footer link, and a per-dashboard auto-refresh interval.
- **Interactivity** — hover tooltips, **click-to-cross-filter** (chart marks and
  table rows set variables other panels filter on), range zoom, value/brush
  filters, full-size a panel, dark mode, and view-only share links. Pure DOM, so
  it works on static exported HTML too.
- **Formatting** — ggplot2-style axis formatters: `::YFORMAT '€'`,
  `::XFORMAT '$'`, percent, compact.

The complete reference is [`docs/DOCS.md`](docs/DOCS.md); an agent skill that
writes these dashboards from a prompt lives in
[`.claude/skills/build-dashboard`](.claude/skills/build-dashboard/SKILL.md).

## Where it runs — three surfaces, one renderer

1. **Core library** (`src/lib.rs`) — annotated result set → SVG. No DuckDB
   dependency; compiles to wasm. Link it directly to render charts in-process.
2. **DuckDB extension** (`duckext/`) — render inside SQL (`anofox_render`, plus
   `anofox_bar`/`_line`/… macros) and **serve** dashboards (`anofox_serve` for
   authoring, `anofox_serve_dashboards` for locked serving). See `duckext/BUILD.md`.
3. **Web app** (`web/`) — the in-browser builder on DuckDB-Wasm (the live demo).

**Data is whatever DuckDB can read**: CSV / Parquet / JSON / Arrow, MotherDuck,
PostgreSQL / MySQL / SQLite via `ATTACH`, remote HTTP, and spatial layers
(`ST_AsText` → WKT → `::MAP`).

## Render inside SQL

```sql
LOAD 'anofox_visualization.duckdb_extension';

-- Convenience macros: pass columns, get one SVG per group.
WITH sales AS (SELECT * FROM (VALUES ('app',30),('web',22),('api',12)) t(ch,n))
SELECT anofox_bar(ch, n) FROM sales;                 -- <svg> bar chart
--   also anofox_line/_scatter/_area(x, y), anofox_xy(x, y, kind := 'VIOLIN'),
--   anofox_xyc(x, y, series)  (coloured by series)

-- Or the raw spec (rows + role annotations), the same JSON the browser uses:
SELECT anofox_render('{"rows":[{"c0":"a","c1":3}],"roles":[[0,"XAXIS"],[1,"BARCHART"]]}');
```

Build the extension with `duckext/scripts/build-native.sh`
(→ `anofox_visualization.duckdb_extension`, loaded with `duckdb -unsigned`); see
`duckext/DISTRIBUTING.md` for shipping it.

## Launch the builder from DuckDB

Like [DuckDB UI](https://duckdb.org/2025/03/12/duckdb-ui), one SQL call opens the
builder in your browser wired to the **live** session:

```sql
LOAD 'anofox_visualization.duckdb_extension';
SELECT anofox_serve(8080);   -- opens http://localhost:8080 against THIS database
```

Everything you query in the builder runs on your real tables. This is the
**authoring** mode — its `/query` runs whatever SQL the client sends, so keep it
to localhost.

## Serve dashboards to consumers (locked, read-only)

To hand a fixed dashboard to *untrusted* consumers, serve a folder of `.sql`
files locked down:

```sql
LOAD 'anofox_visualization.duckdb_extension';
SELECT anofox_serve_dashboards('dashboards', 8095);
-- http://127.0.0.1:8095/          a list of the folder's dashboards
-- http://127.0.0.1:8095/d/<name>  one dashboard — full UI, editor removed
```

Same interactive client as the builder, but locked:

- **The server owns the SQL.** `POST /query` is **allow-listed** to each
  dashboard's own planned panel SQL (plus validated `SET VARIABLE`s); arbitrary
  SQL is rejected `403`.
- **Read-only by construction.** At startup it snapshots the live database and
  serves through a fresh read-only DuckDB handle with no writable database
  attached — so even a gate bypass can't write (an allow-listed `INSERT` is
  rejected *"attached in read-only mode"*). It serves that snapshot; re-run to
  refresh.
- **Multi-user safe.** Each request inlines its own input variables, so
  concurrent viewers never clobber one another.
- **Multi-page.** `::TAB`/`::PAGE` are pages within a dashboard (deep-linkable
  via `?tab=`); a folder of files is a linked set with a shared nav bar.

A stricter alternative — the `serve` binary — renders **server-side SVG only**
(no client `/query` at all); consumers pick a dashboard id + whitelisted params.
Put **TLS + auth at a reverse proxy** in front of either for public use. Full
trust model and deployment: [`docs/secure-serving.md`](docs/secure-serving.md).

## CLI & web app

```sh
# CLI: render an annotated .sql to an interactive HTML file (shells out to duckdb)
cargo run --bin dashboard -- dashboards/sessions.sql   # → dashboard.html

# Web app: build the wasm module, then serve web/ as a static site
wasm-pack build --target web --out-dir web/pkg --no-default-features --features wasm
python3 -m http.server -d web 8080                     # → http://localhost:8080
```

`web/` is a static site (ES modules + WebAssembly + DuckDB-Wasm from the jsDelivr
CDN — single-threaded, no COOP/COEP). Publish it to any static host; `web/pkg/`
is gitignored and built in CI (`.github/workflows/pages.yml` powers the live demo).

## License

**Business Source License 1.1 (BSL 1.1)** — see [`LICENSE`](LICENSE).

- ✅ **Free for internal / production use** — use it in your own business
- ✅ **Free for development & research**
- ❌ **Not** for offering the work to third parties on a **hosted** or
  **embedded** basis (contact [DataZoo GmbH](https://data-zoo.de) for a
  commercial license)
- 🔄 **Converts to MPL 2.0** five years after first publication

Licensor: DataZoo GmbH. Change License: Mozilla Public License 2.0.

## Acknowledgments

Rendering by [ggplot-rs](https://github.com/sipemu/ggplot-rs); data engine by
[DuckDB](https://duckdb.org) / DuckDB-Wasm. © DataZoo GmbH.
