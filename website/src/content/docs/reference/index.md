---
title: Reference
description: Every role, the CLI commands, and where the full reference lives.
---

- **[Roles](roles/):** every `::ROLE` cast by category, generated from the role registry.
- **[Chart kinds](../build/charts/):** one page per chart, with the data it needs.
- **[The plot macros](../build/model-output/):** `anofox_plot_*` for the anofox extensions' results.

## The `dashboard` CLI

| Command | Does |
|---|---|
| `dashboard file.sql out.html` | renders an interactive static HTML page |
| `dashboard --svg file.sql out.svg` | renders the whole dashboard headless to one SVG |
| `dashboard --check file.sql [--strict]` | lints: broken SQL, empty panels, design rules |
| `dashboard --site folder/ out/` | publishes a folder of dashboards as a static DuckDB-Wasm site |
| `dashboard --describe source` | schema and statistics of a table or file |
| `dashboard --publish file.sql --out d` | the agent bundle of a dashboard's `-- @question` lines |
| `dashboard --roles` | every role, grouped |

`dashboard --help` lists every option. The complete written reference is [docs/DOCS.md](https://github.com/DataZooDE/anofox-visualization/blob/master/docs/DOCS.md); serving and security are in [secure-serving.md](https://github.com/DataZooDE/anofox-visualization/blob/master/docs/secure-serving.md).
