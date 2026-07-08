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
