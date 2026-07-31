# Anofox Visualization - DuckDB Extension

Charts & dashboards for DuckDB — the grammar of graphics, straight from SQL. You
write ordinary DuckDB SQL and tag output columns with `::ROLE` casts; each
annotated `SELECT` becomes a panel (chart / KPI / table / input), rendered to
SVG by the [ggplot-rs](https://github.com/sipemu/ggplot-rs) engine. No JS
charting library, no bespoke config — the SQL *is* the dashboard.

[![License: BSL 1.1](https://img.shields.io/badge/License-BSL%201.1-blue.svg)](LICENSE)
[![DuckDB Version](https://img.shields.io/badge/DuckDB-v1.4.x%20LTS%20%7C%20v1.5.x-brightgreen.svg)](https://duckdb.org)
[![Live demo](https://img.shields.io/badge/live%20demo-online-2ea44f.svg)](https://datazoode.github.io/anofox-visualization/)

> [!IMPORTANT]
> This extension is in early development, so bugs and breaking changes are expected.
> Please use the [issues page](https://github.com/DataZooDE/anofox-visualization/issues) to report bugs or request features.

```sql
LOAD anofox_visualization;

-- Convenience macros: pass columns, get one SVG per group.
WITH sales AS (SELECT * FROM (VALUES ('app',30),('web',22),('api',12)) t(ch,n))
SELECT anofox_bar(ch, n) FROM sales;                 -- <svg> bar chart
```

**[▶ Live demo](https://datazoode.github.io/anofox-visualization/)** — the full
builder running in your browser on DuckDB-Wasm.

## Features

### Chart kinds

Cast the **measure** column with the chart role; add `::XAXIS` / `::CATEGORY` for
the axes and colour series.

| Family | Roles |
|--------|-------|
| Bars | `::BARCHART`, `_STACKED`, `_PERCENT`, `_STACKED_PERCENT`, `::FLIP` (horizontal) |
| Lines & areas | `::LINECHART`, `_PERCENT`, `::STEP`, `::SMOOTH` (trend), `::AREACHART`, `_STACKED` |
| Points | `::SCATTER`, `::BUBBLE` (sized), `::JITTER` |
| Part-to-whole | `::PIE`, `::DONUTCHART` |
| Distribution | `::HISTOGRAM`, `::DENSITY`, `::BOXPLOT`, `::VIOLIN`, `::QQ` |
| Matrix / geo | `::HEATMAP`, calendar heatmap, `::MAP` (WKT-geometry choropleth) |
| Finance | `::CANDLESTICK`, `::OHLC`, `::SPARKLINE` |
| Gauge / radar | `::GAUGE` (+ `::RANGE`, `::COLORS` zones), `::RADAR` |
| Annotations | `::REFLINE`/`::XLINE` reference lines, `::BAND_LOWER`/`::BAND_UPPER` bands |

### KPIs, text & tables

| Group | Roles |
|-------|-------|
| KPI tiles | `::METRIC`, `::MONEY`, `::PERCENT`, `::COMPACT` (+ `::LABEL` caption, `::DELTA` arrow) |
| Text | `::TEXT_SMALL`/`_MEDIUM`/`_LARGE`, `::MARKDOWN` |
| Tables | `::TABLE`, `::PAGED` (SQL-paginated), `::COLORSCALE`, `::BADGE`, `::TREND`, `::SPARKLINE`, `::DOWNLOAD_CSV`/`_XLSX`/`_PDF` |

### Inputs, layout & interactivity

| Group | Roles / behaviour |
|-------|-------------------|
| Inputs | `::DROPDOWN`, `::MULTISELECT`, `::NUMBER`, `::DATE`, `::TEXT`, `::DATERANGE` → each is a DuckDB variable read with `getvariable('name')` |
| Layout | 12-column grid (`::COLUMNS`/`::COL`/`::HEIGHT`), `::GROUP` boxes, tabs (`::TAB`/`::SUBTAB`) |
| Interactivity | hover tooltips, **click-to-cross-filter**, range zoom, brush/value filters, dark mode, share links — pure DOM, so it works on exported HTML too |
| Formatting | ggplot2-style axis formatters (`::YFORMAT '€'`, `::XFORMAT '$'`, percent, compact) |

### Key capabilities
- **Three surfaces, one renderer** — a core library (annotated result → SVG, wasm-compatible), a **DuckDB extension** (render + serve), and a browser builder on DuckDB-Wasm.
- **Serve locked, read-only dashboards** — hand a folder of `.sql` files to untrusted consumers; the server owns the SQL (allow-listed `/query`) over a read-only snapshot.
- **Any DuckDB source** — CSV / Parquet / JSON / Arrow, MotherDuck, Postgres / MySQL / SQLite via `ATTACH`, remote HTTP, and spatial layers (`ST_AsText` → WKT → `::MAP`).
- **Authoring aids** — a `dashboard --check` linter (structure + `design/*` quality checks) and a `build-dashboard` agent skill that writes dashboards from a prompt.

## Quick Start

All rendering functions use the `anofox_` prefix.

```sql
LOAD anofox_visualization;

-- 1) Convenience macros — pass columns, get one SVG per group.
WITH sales AS (SELECT * FROM (VALUES ('app',30),('web',22),('api',12)) t(ch,n))
SELECT anofox_bar(ch, n) FROM sales;                 -- <svg> bar chart
--   also anofox_line / _scatter / _area(x, y),
--        anofox_xy(x, y, kind := 'VIOLIN', width := 640, height := 400),
--        anofox_xyc(x, y, series)   -- coloured by series

-- 2) The raw spec (rows + role annotations) — the same JSON the browser uses:
SELECT anofox_render('{"rows":[{"c0":"a","c1":3}],"roles":[[0,"XAXIS"],[1,"BARCHART"]]}');

-- 3) A dashboard panel — annotate the SELECT list with ::ROLE casts:
SELECT week::XAXIS, channel::CATEGORY, sum(n)::BARCHART_STACKED, 'Sessions'::TITLE
FROM sessions GROUP BY ALL ORDER BY ALL;
```

Open the **interactive builder** wired to your live database, DuckDB-UI style:

```sql
SELECT anofox_serve(8080);   -- http://localhost:8080 against THIS database (localhost only)
```

Serve a folder of `.sql` dashboards **locked, read-only** to consumers:

```sql
SELECT anofox_serve_dashboards('dashboards', 8095);
-- http://127.0.0.1:8095/           a list of the folder's dashboards
-- http://127.0.0.1:8095/d/<name>   one dashboard — full UI, editor removed
```

## Installation

### Community Extension

Once accepted into the [DuckDB Community Extensions](https://community-extensions.duckdb.org)
repository:

```sql
INSTALL anofox_visualization FROM community;
LOAD anofox_visualization;
```

### From source

DuckDB must be started with `-unsigned` (the binary is not signed by the DuckDB
Foundation).

```sh
git clone --recurse-submodules https://github.com/DataZooDE/anofox-visualization
cd anofox-visualization
make                 # → build/release/extension/anofox_visualization/anofox_visualization.duckdb_extension
```

```sql
LOAD './build/release/extension/anofox_visualization/anofox_visualization.duckdb_extension';
```

## Documentation

- **[Annotation model & role reference](docs/DOCS.md)** — every `::ROLE`, layout, and serving mode.
- **[Dashboard design principles](docs/dashboard-design.md)** — what makes a dashboard good (enforced by `dashboard --check`).
- **[Visual review rubric](docs/dashboard-visual-review.md)** — the appearance gate.
- **[Secure serving](docs/secure-serving.md)** — the trust model for locked serving.
- **[`build-dashboard` skill](.claude/skills/build-dashboard/SKILL.md)** — an agent that authors dashboards from a prompt.

## Dependencies

- **DuckDB**: v1.4.x LTS or v1.5.x (latest)
- **Rust**: stable toolchain (for building from source)
- **[ggplot-rs](https://github.com/sipemu/ggplot-rs)**: the grammar-of-graphics rendering engine
- **plotters / image / ab_glyph**: SVG & raster backends (via Cargo)

## Telemetry

This extension collects **no telemetry** — no usage data, no query contents,
nothing leaves your machine.

## License

This project is licensed under the **Business Source License 1.1** (BSL 1.1).

### Key Terms

- **Usage Grant**: free to use, modify, and distribute for development, testing, research, and internal use
- **Production Use**: hosted / embedded redistribution to third parties requires a commercial license until the Change Date
- **Change Date**: five years after first publication
- **Change License**: Mozilla Public License 2.0

See [LICENSE](LICENSE) for full terms.

### Why BSL?

The BSL keeps the source open for development, research, and internal use, and
converts to a fully open licence over time — while protecting the project's
long-term viability. For commercial hosted/embedded use before the Change Date,
contact **contact@datazoo.de**.

## Contributing

Contributions are welcome:
1. Fork the repository
2. Create a feature branch
3. Make your changes with tests (`cargo test`; `dashboard --check` for example dashboards)
4. Submit a pull request

### Areas for Contribution
- New chart kinds and roles
- Additional `design/*` lint checks and the visual-review loop
- Documentation, guides, and example dashboards
- Bug reports and fixes; performance work

## Support

- **Documentation**: [docs/](docs/)
- **Issues**: [GitHub Issues](https://github.com/DataZooDE/anofox-visualization/issues)
- **Discussions**: [GitHub Discussions](https://github.com/DataZooDE/anofox-visualization/discussions)
- **Email**: contact@datazoo.de

## Citation

```bibtex
@software{anofox_visualization,
  title  = {Anofox Visualization: Charts & Dashboards for DuckDB from SQL},
  author = {DataZoo DE},
  year   = {2025},
  url    = {https://github.com/DataZooDE/anofox-visualization}
}
```

## Acknowledgments

- **DuckDB Team** — the database and extension framework
- **[ggplot-rs](https://github.com/sipemu/ggplot-rs)** — the grammar-of-graphics rendering engine
- **Open Source Community** — for contributions and feedback

© DataZoo GmbH.
