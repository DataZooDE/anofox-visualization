---
title: In the browser
description: The builder on DuckDB-Wasm — no install, data stays in the browser.
---

The [builder](../../app/) is the full authoring app compiled to WebAssembly: DuckDB-Wasm runs the SQL, the renderer draws the SVG, nothing leaves the browser.

- **Open data** with *Explore data*: CSV, Parquet, JSON files from your disk, or URLs.
- **Write SQL** in the editor; *Run* draws the dashboard; *Edit* / *View* switch between editor and full page.
- **Save** keeps dashboards in this browser; *⋯ → import* loads `.sql` files; **Share** encodes the SQL in a link; **⤓ HTML** downloads a self-contained page.
- **Extensions in the browser:** anofox-statistics and anofox-forecast run live in DuckDB-Wasm; tabular, optimize and decide results are read precomputed (they have no Wasm build yet).
- **38 examples** in the sidebar, grouped by business area — each one a plain `.sql` file you can copy.

The same builder is embedded in the full toolkit's authoring server, wired to a real DuckDB file instead ([Authoring on your data](../authoring/)).
