# anofox-visualization

SQL-defined dashboards: annotate SQL result columns with *roles* (`XAXIS`,
`CATEGORY`, `LABEL`, and a chart kind on the value column like `BARCHART` /
`LINECHART`) and render them to SVG with
[ggplot-rs](https://crates.io/crates/ggplot-rs).

```sql
SELECT 'Sessions per week'::LABEL;
SELECT week::XAXIS, channel::CATEGORY, sum(n)::BARCHART_STACKED
FROM sessions GROUP BY ALL ORDER BY ALL;
```

## Three surfaces, one renderer

1. **Core library** (`src/lib.rs`) — dependency-light and **wasm-compatible**:
   maps an annotated result set (`Column { name, role, values }`) onto ggplot-rs
   and returns an SVG. No DuckDB dependency.
2. **Web app** (`web/`) — a browser dashboard builder. DuckDB-Wasm runs the SQL,
   the core (compiled to wasm) renders each panel to SVG. Interactive: hover
   tooltips, cross-filter, tabs, full-size, view-only share links. Ships a
   gallery of example dashboards.
3. **DuckDB extension** (`duckext/`) — registers SQL functions that call the
   core: `SELECT anofox_render(spec) → SVG`, plus convenience macros
   (`anofox_bar/_line/_scatter(x, y)`, `anofox_xy(x, y, kind := …)`). Works
   natively today; see `duckext/BUILD.md` and `duckext/DISTRIBUTING.md`.

## Deploy the web app

The app in `web/` is a **static site** (`index.html` + `app.js` + a compiled
wasm module + sample data). It must be served over HTTP(S) — ES-module imports,
WebAssembly, and DuckDB-Wasm don't work from `file://`.

### 1. Build the wasm module

Needs [`wasm-pack`](https://rustwasm.github.io/wasm-pack/) (and `wasm-opt`):

```sh
wasm-pack build --target web --out-dir web/pkg --no-default-features --features wasm
```

This writes `web/pkg/anofox_visualization.js` + `…_bg.wasm` (~310 KB), which
`web/app.js` imports. `web/pkg/` is gitignored, so it's a build artifact.

### 2. Serve `web/` locally

```sh
cd web && python3 -m http.server 8080     # then open http://localhost:8080
```

### 3. Publish to any static host

Upload the whole `web/` directory (including the freshly built `pkg/`) to any
static host — Netlify, Cloudflare Pages, S3 + CloudFront, nginx, GitHub Pages, …
No server code runs; DuckDB-Wasm loads from the jsDelivr CDN at runtime (needs
outbound internet, but no special COOP/COEP headers — it runs single-threaded).

Bundled runtime assets already in `web/`: `m5_monthly.parquet` (sample data for
the forecast demos) and `localext/` (the `anofox_forecast` wasm extension). The
map examples fetch Natural Earth GeoJSON at runtime via DuckDB-Wasm's spatial
extension.

### GitHub Pages (CI)

Because `web/pkg/` is gitignored, a Pages deploy must **build the wasm in CI**
then publish `web/`. Sketch of a workflow:

```yaml
# .github/workflows/pages.yml
name: Pages
on: { push: { branches: [master] } }
permissions: { pages: write, id-token: write, contents: read }
jobs:
  deploy:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - run: curl https://rustwasm.github.io/wasm-pack/installer/init.sh -sSf | sh
      - run: wasm-pack build --target web --out-dir web/pkg --no-default-features --features wasm
      - uses: actions/upload-pages-artifact@v3
        with: { path: web }
      - uses: actions/deploy-pages@v4
```

Enable Pages with **build type = GitHub Actions** in the repo settings first.

## CLI: render a `.sql` file to a dashboard

Write a `.sql` file where result columns are annotated with `::ROLE` casts.
Statements without a role are setup; annotated `SELECT`s become panels:

```sh
cargo run --bin dashboard -- dashboards/sessions.sql   # writes dashboard.html
xdg-open dashboard.html
```

Roles: `::XAXIS`, `::CATEGORY`, `::LABEL` (heading), and a chart kind on the
measure — `::BARCHART`, `::BARCHART_STACKED`, `::LINECHART`, `::AREACHART`,
`::SCATTER`, and more. The runner shells out to the `duckdb` CLI, so there's no
bundled DuckDB compile.
