---
title: The anofox extensions
description: How statistics, forecast, tabular, optimize and decide results become charts — one contract, one anofox_plot call.
---

The anofox DuckDB extensions emit their results as tables in a shared **integration contract**: each producer (`ols_tidy_by`, `forecast_by`, `profile`, `pack_by`, …) tags its output schema (`terms`, `summary`, `prediction`, `curve`, `tests`, `obs`, `assignment`, …). anofox-visualization knows each schema, so one call plots any of them:

```sql
LOAD anofox_statistics; LOAD anofox_visualization;
SELECT anofox_plot(t) FROM ols_tidy_by('sales', region, revenue, [price, promo]) t;   -- a coefficient plot per region
```

`anofox_plot(t)` picks the plot from the schema; `anofox_plot_terms`, `_prediction`, `_curve`, `_tests`, … choose it explicitly. In a dashboard, the same producers feed panels.

| Extension | What it produces | In the browser | CI conformance |
|---|---|---|---|
| **anofox-statistics** | regression (OLS … GLMM), survival, tests, ANOVA, calibration, bootstrap | live (Wasm build) | all 211 producers |
| **anofox-forecast** | forecasts with intervals, backtests, decomposition | live (Wasm build) | ✔ |
| **anofox-tabular** | profiling, data quality, deduplication | precomputed | ✔ |
| **anofox-optimize** | packing, assignment, sequencing, replenishment | precomputed | ✔ |
| **anofox-decide** | decision models, triage | precomputed | ✔ |

Every producer of every extension is rendered in CI against the extension's latest build (`*-conformance` jobs), so a contract change is caught the day it lands.

Coming in this section (#1017 P2): one page per extension with a runnable example, the contract schemas, Python and R ([today: python-r.md](https://github.com/DataZooDE/anofox-visualization/blob/master/docs/python-r.md)), and the Claude Code plugin. Until then: the [integration contract](https://github.com/DataZooDE/anofox-visualization/blob/master/docs/plans/integration-contract.md) and the [conformance sections of the README](https://github.com/DataZooDE/anofox-visualization/blob/master/README.md#conformance-with-anofox-statistics).
