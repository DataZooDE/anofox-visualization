<h1 align="center">anofox-visualization</h1>
<p align="center"><b>Charts &amp; dashboards for DuckDB — the grammar of graphics, straight from SQL.</b></p>

<p align="center">
  <a href="LICENSE"><img alt="License: BSL 1.1" src="https://img.shields.io/badge/license-BSL%201.1-blue"></a>
  <img alt="DuckDB" src="https://img.shields.io/badge/DuckDB-v1.2%2B-yellow?logo=duckdb&logoColor=black">
  <a href="https://sipemu.github.io/anofox-visualization/"><img alt="Live demo" src="https://img.shields.io/badge/live%20demo-online-2ea44f"></a>
</p>

> ⚠️ **Early development.** APIs and the extension packaging are still moving; expect rough edges.

Annotate SQL result columns with *roles* — `::XAXIS`, `::CATEGORY`, a chart kind
on the value column like `::BARCHART` / `::LINECHART` — and render them to SVG
with the [ggplot-rs](https://crates.io/crates/ggplot-rs) grammar-of-graphics
engine. No plotting library on the client, no server: just SQL.

```sql
SELECT 'Sessions per week'::LABEL;
SELECT week::XAXIS, channel::CATEGORY, sum(n)::BARCHART_STACKED
FROM sessions GROUP BY ALL ORDER BY ALL;
```

## ✨ Live demo

**[sipemu.github.io/anofox-visualization](https://sipemu.github.io/anofox-visualization/)** —
the full dashboard builder running in your browser on DuckDB-Wasm, with a gallery
of examples (bars, lines, distributions, pie/gauge/radar, heatmaps, candlesticks,
maps, forecasts, interactivity).

## Key features

- **40+ chart kinds** — bar/line/area/scatter/step/smooth, box/violin/histogram/
  density, pie/donut/gauge/radar, heatmap/calendar/candlestick, contour, maps.
- **SQL-native** — every panel is a `SELECT` whose columns carry `::ROLE` casts.
  Un-annotated statements are setup.
- **Interactive** — hover tooltips, cross-filter, tabs, data view, brush/value
  filter, range zoom, full-size, and view-only share links.
- **Dashboards from SQL** — `::COL`, `::GROUP`, `::TAB`/`::SUBTAB`, `::HEIGHT`,
  KPI tiles, rich tables (money/badge/sparkline), inputs (`::DROPDOWN`,
  `::MULTISELECT`, `::DATERANGE`, …), Markdown boxes.
- **Runs everywhere** — a browser app (DuckDB-Wasm), a DuckDB extension, and a CLI
  — all backed by one dependency-light, wasm-compatible renderer.

## Three surfaces, one renderer

1. **Core library** (`src/lib.rs`) — maps an annotated result set onto ggplot-rs
   and returns an SVG. No DuckDB dependency; compiles to wasm.
2. **DuckDB extension** (`duckext/`) — `SELECT anofox_render(spec) → SVG` plus
   convenience macros. See `duckext/BUILD.md`.
3. **Web app** (`web/`) — the browser dashboard builder (the live demo above).

## 🦆 Launch the dashboard from DuckDB

Like [DuckDB UI](https://duckdb.org/2025/03/12/duckdb-ui), the extension can open
the builder in your browser wired to the **live** session — one SQL call starts
an in-process server (a `/query` bridge to your current database) and opens it:

```sql
LOAD 'anofox_visualization.duckdb_extension';
SELECT anofox_serve(8080);   -- opens http://localhost:8080 against THIS database
```

Everything you query in the builder runs on your real tables. (Building the
extension with a bundled UI needs `web/pkg` present — run the wasm build first;
see below.)

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

Build it with `duckext/scripts/build-native.sh` (→ `anofox_visualization.duckdb_extension`,
loaded with `duckdb -unsigned`). See `duckext/DISTRIBUTING.md` for shipping it.

## Deploy the web app

`web/` is a **static site** (`index.html` + `app.js` + a compiled wasm module +
sample data). Serve it over HTTP(S) — ES modules, WebAssembly, and DuckDB-Wasm
don't work from `file://`.

```sh
# 1. build the wasm module (needs wasm-pack + wasm-opt)
wasm-pack build --target web --out-dir web/pkg --no-default-features --features wasm
# 2. serve web/ locally
cd web && python3 -m http.server 8080     # → http://localhost:8080
```

Publish the whole `web/` dir (with the freshly built `pkg/`) to any static host —
Netlify, Cloudflare Pages, S3, nginx, GitHub Pages. DuckDB-Wasm loads from the
jsDelivr CDN at runtime (no special COOP/COEP headers; single-threaded).
`web/pkg/` is gitignored, so a **GitHub Pages** deploy builds it in CI —
see `.github/workflows/pages.yml` (that's what powers the live demo).

## CLI

```sh
cargo run --bin dashboard -- dashboards/sessions.sql   # writes dashboard.html
```

Shells out to the `duckdb` CLI, so there's no bundled DuckDB compile.

## Live dashboards for consumers

Because DuckDB is the connector and a dashboard is just annotated SQL, the
end-to-end flow is:

> **Attach** a live source through DuckDB (MotherDuck, PostgreSQL, remote
> Parquet/CSV) → **write** the dashboard SQL (an LLM does this well — it's just
> `SELECT`s with `::ROLE` casts) → **deploy** it view-only → **send a link**.
> Consumers see current data, because every panel re-queries the live backend
> (set an auto-refresh interval to poll).

Where the query runs decides which sources work and how consumers are served:

| | **Server-side** (`anofox_serve` on a host) | **Browser** (static site / Pages) |
|---|---|---|
| PostgreSQL source | ✅ (connection stays server-side) | ❌ a browser can't reach Postgres |
| MotherDuck / remote Parquet | ✅ | ✅ (browser connects, needs a token / public files) |
| Consumer needs DB credentials? | **No** | Yes |
| Best for "send a link to consumers" | **this one** | great for MotherDuck / public data |

For untrusted consumers, read the trust model below first —
[`docs/secure-serving.md`](docs/secure-serving.md) is the design + plan for
serving safely.

## Security & trust model

**View-only is presentation, not an access boundary.** `?embed=1` (and the
"view-only share link") hides the editor for a clean display, but it is **not**
a security control:

- **Browser model** — the query engine (DuckDB-Wasm) runs *in the consumer's
  browser*. A technical viewer can un-hide the editor, edit the `#sql=` hash, or
  run SQL from the console, and can extract any credential embedded in the page.
- **`anofox_serve` today** — the `/query` bridge executes **whatever SQL the
  client sends**. That's convenient for *you on localhost*, but it means an
  untrusted consumer could `curl` arbitrary SQL against your live database. Do
  **not** expose it to untrusted users as-is; treat it as a single-user
  authoring tool.

**Serving untrusted consumers safely** needs three things (design + plan in
[`docs/secure-serving.md`](docs/secure-serving.md)):

1. **The server owns the SQL** — the client requests a dashboard/panel by **id**
   plus **whitelisted parameters**; the server runs the fixed, pre-defined query.
   The client never sends SQL.
2. **A read-only, least-privilege connection** — scoped to just the views a
   dashboard needs, so a mistake or leak is bounded.
3. **Auth + TLS** at a reverse proxy in front of the serving process.

Today's free-form `anofox_serve` is kept as a future **admin / authoring mode**.

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
