# duckplot — documentation

SQL-defined dashboards, Shaper-style: you annotate SQL result columns with
**roles** (`::XAXIS`, `::CATEGORY`, a chart kind on the measure), and duckplot
renders them with [ggplot-rs](../ggplot-rs). The core is dependency-light and
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
| `::LABEL` (`::TITLE`) | a section heading (renders as a title-only panel) |
| `::BARCHART` (`::BAR`) | bar chart (measure) |
| `::BARCHART_STACKED` | stacked bar (measure) |
| `::LINECHART` (`::LINE`) | line chart (measure) |
| `::AREACHART` (`::AREA`) | area chart (measure) |
| `::SCATTER` (`::POINT`) | scatter (measure) |

The cast on the **measure** column selects the geom; `XAXIS`/`CATEGORY` position
and colour it; `LABEL` alone makes a heading. Measures are cast to `DOUBLE`
automatically (so `sum()`/`BIGINT`/`HUGEINT` render numerically everywhere).

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

A single-page app: type SQL, **DuckDB-Wasm** runs it in the page, **duckplot
compiled to wasm** renders the panels. Everything is client-side — static files,
no backend, no DuckDB extension.

```sh
wasm-pack build --target web --out-dir web/pkg --no-default-features --features wasm
python3 -m http.server -d web 8000   # then open http://localhost:8000
```

Edit the SQL, press **Run**, hover the marks. Load your own data with DuckDB’s
readers, e.g. `CREATE TABLE t AS SELECT * FROM read_csv_auto('https://…');`.

### c) DuckDB extension (native works; wasm ~99%)

Packages the renderer *inside* DuckDB so `SELECT ggplot(...)` returns an SVG.
See `duckext/BUILD.md`. Native works today; the wasm side-module links and
instantiates in DuckDB-Wasm with one emscripten ABI detail remaining. For
browser use, **(b) is the recommended path** — it avoids the extension ABI
entirely.

---

## 3. Interactivity

Rendered panels carry an SVG `<title>` per mark. Both the CLI output and the
browser builder attach a small hover layer that shows a styled tooltip
(`web: 22`) and highlights the mark. It’s pure DOM — no chart runtime, works on
static HTML.

---

## 4. WASM compatibility

The **core** (`duckplot`) has no `polars`/native-only deps; it renders through
ggplot-rs’s plotters-free `render_svg_native`, which compiles to
`wasm32-unknown-unknown`. That’s what makes the browser builder possible:

```
 ┌── browser tab ───────────────────────────────────────────┐
 │  SQL editor                                               │
 │     │ plan(sql)         (duckplot-wasm)                   │
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
