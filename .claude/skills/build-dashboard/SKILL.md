---
name: build-dashboard
description: Build an interactive SQL dashboard for anofox-visualization from a plain-text request. Use when the user asks to "build/make/create a dashboard", "visualize this data", or describes charts/KPIs/tables they want. Produces annotated DuckDB SQL where result columns are tagged with `::ROLE` casts.
---

# Build an anofox-visualization dashboard

This project-local skill is a pointer: the workflow now lives in the Claude Code
plugin of this repository, so there is one copy to keep correct (its SQL
examples are executed in CI by `scripts/validate_skills_sql.py`).

**Read and follow
[`plugins/anofox-visualization/skills/anofox-visualization-dashboards/SKILL.md`](../../../plugins/anofox-visualization/skills/anofox-visualization-dashboards/SKILL.md)**
(path from the repository root): the `::ROLE` model, every role, layout, inputs and
cross-filter, contract plots as panels, the `dashboard --describe` /
`dashboard --check` generate-validate-repair loop with its lint codes, the
design contract, templates and gotchas.

Related skills of the same plugin, in `plugins/anofox-visualization/skills/`:

- `anofox-visualization-statistics-plots` — `anofox_plot_*` for
  anofox-statistics / anofox-forecast output (also usable as dashboard panels);
- `anofox-visualization-charts` — `anofox_render` and the column chart macros;
- `anofox-visualization-serving` — `anofox_serve`, `anofox_serve_dashboards`,
  options, refresh and deployment for handing a dashboard to others;
- `anofox-visualization-access-control` — login (proxy, JWT, OIDC, embed links,
  API keys), claims and row / column policies on a locked server.
