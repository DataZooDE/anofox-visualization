# duckplot

SQL-defined dashboards, Shaper-style: annotate SQL result columns with *roles*
(`XAXIS`, `CATEGORY`, `LABEL`, and a chart kind on the value column like
`BARCHART` / `LINECHART`) and render them to SVG with
[ggplot-rs](https://crates.io/crates/ggplot-rs).

```sql
SELECT 'Sessions per week'::LABEL;
SELECT week::XAXIS, category::CATEGORY, count()::BARCHART_STACKED
FROM sessions GROUP BY ALL ORDER BY ALL;
```

## Layers

1. **Core** (`src/lib.rs`) — dependency-light, **wasm-compatible**: maps an
   annotated result set (`Column { name, role, values }`) onto ggplot-rs and
   returns an SVG. This is the heart; it has no DuckDB dependency.
2. **DuckDB extension** (planned) — registers SQL functions that call the core:
   `SELECT ggplot_render(...) → SVG`. Native first; a WASM build (emscripten
   side-module loaded by DuckDB-Wasm) is the stretch goal.

Status: core implemented + tested. Extension packaging in progress.

## Build and see a dashboard

Write a `.sql` file where result columns are annotated with `::ROLE` casts
(Shaper-style). Statements without a role are setup; annotated `SELECT`s become
panels. Then:

```sh
cargo run --bin dashboard -- dashboards/sessions.sql   # writes dashboard.html
xdg-open dashboard.html                                # open in a browser
```

Roles: `::XAXIS`, `::CATEGORY`, `::LABEL` (heading), and a chart kind on the
measure — `::BARCHART`, `::BARCHART_STACKED`, `::LINECHART`, `::AREACHART`,
`::SCATTER`. Example (`dashboards/sessions.sql`):

```sql
CREATE TABLE sessions AS SELECT * FROM (VALUES ('W1','app',30), ...) t(week,channel,n);

SELECT 'Weekly sessions'::LABEL;
SELECT week::XAXIS, channel::CATEGORY, sum(n)::BARCHART_STACKED FROM sessions GROUP BY ALL;
SELECT week::XAXIS, sum(n)::LINECHART FROM sessions GROUP BY ALL ORDER BY week;
```

The runner shells out to the `duckdb` CLI, so no bundled DuckDB compile. (Known
quirk: a `BARCHART` *with* a `CATEGORY` currently stacks rather than dodges —
use `BARCHART_STACKED`, or a `BARCHART` without a category, for now.)
