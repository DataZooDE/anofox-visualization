# Plan: anofox-visualization

Context: review of 2026-10-08 (hardening in progress), research on regression
visualisation needs, evaluation of anofox-statistics v0.10.0 (+ WIP
`refactor/delegate-glm-aft`). Contract: [integration-contract.md](integration-contract.md).

## Phase 0 — Hardening (in progress)

- Serving security (command execution via `serve`, allow-list bypass, snapshot
  external access, CSRF/DNS-rebinding), FFI `catch_unwind` + real SQL errors,
  resource caps, CI on `master`, lockfiles, version single source.
- Core: gauge panic, XSS escaping, SQL lexer, role registry, NaN/Infinity JSON,
  calendar bound, linear-time categories, docs drift.
- Migrate to ggplot-rs 0.16 from crates.io (drop the git pin): replace
  `render_candlestick/radar/gauge/calendar`, `panel_bounds`, `<svg` string
  surgery and `<title>` parsing with the new upstream APIs (`geom_candlestick`,
  `coord_radar`, `CoordPolar::with_span`, `geom_calendar`, `data-domain`,
  `render_svg_native_at`, `data-series`/`data-value`). Surface
  `render_svg_native_with_warnings` messages in panels.

## Phase 1 — Roles for statistical graphics

| Role | Maps to (ggplot-rs) | Unlocks |
|---|---|---|
| `::YMIN` / `::YMAX` (+ `::XMIN`/`::XMAX`) | `geom_pointrange` / `geom_errorbar(h)` | coefficient forest, CV curve, binned residuals, caterpillar |
| `::ABLINE 'slope,intercept'`, `::IDENTITY` | `geom_abline` | predicted-vs-actual, ROC/calibration diagonal, QQ line |
| data-driven `::REFLINE` / `::XLINE` per category | data-mapped `geom_hline/vline` (upstream U1) | ACF bounds, per-model zero lines |
| `::FACET`, `::FACET_FREE` | `facet_wrap(_free)` | added-variable plots, rolling coefficients, quantile process |
| `::XSCALE`/`::YSCALE 'log10'\|'sqrt'` | `scale_*_log10/sqrt` | regularisation path, scale-location |
| `::LABEL` on scatter + `::LABEL_TOP k` | `geom_text_repel` (upstream U3) | influence plots (top-k Cook's) |
| `::SMOOTH 'lm'\|'loess'\|'gam'\|'glm'` | `geom_smooth(method)` | residual trend lines, GLM fits |
| `::QQ` with `::BAND` / distribution | `stat_qq_band` (upstream U2) | QQ with envelope |
| `::STEP` + band, censor marks | step ribbon (upstream U7) | Kaplan–Meier |
| `::PANEL n` composite | `ggarrange`/patchwork (upstream U5) | 2×2 diagnostics as one SVG |

All roles go through the role registry (one table → parser, SQL list, docs,
JS); each gets a DOCS entry and a render test.

## Phase 2 — Contract-driven plot macros (work in the render-only community build)

Implemented as DuckDB table macros that aggregate the input into a spec and
call `anofox_render`, so they need no server:

- `anofox_plot_terms(tbl, by := NULL)` → coefficient forest (multi-model dodge;
  `index_name` set → path/process/rolling line+ribbon facetted by term).
- `anofox_plot_diagnostics(tbl)` → plot.lm 2×2 (residuals-vs-fitted+loess, QQ
  with band, scale-location, residuals-vs-leverage with Cook's contours and
  top-k labels).
- `anofox_plot_influence(tbl, k := 5)`, `anofox_plot_cooks(tbl)`.
- `anofox_plot_prediction(tbl)` → fit/forecast with interval fan (shared with
  anofox-forecast; `split` colours train/test/future).
- `anofox_plot_curve(tbl)` → picks step/path/segment + reference line from
  `curve_type`.
- `anofox_plot_summary(tbl)`, `anofox_plot_tests(tbl)`.
- `anofox_plot(tbl)` → dispatch by schema (column-name signature) — the
  "just plot it" entry point; `anofox_plot_model(model_struct)` dispatches on
  `model_type` via `tidy`/`augment` when anofox_statistics is loaded.

Each macro: description + `tags {'anofox.consumes': '<schema>'}` in
`duckdb_functions()`, a sqllogictest with a fixture table per schema, and an
example dashboard under `examples/regression/`.

## Phase 3 — Integration infrastructure

- `anofox_contract_version()`; conformance tests for the consumer side.
- Cross-extension CI job: install `anofox_statistics` + `anofox_forecast` from
  community, run `examples/regression/*.sql` and `examples/forecast/*.sql`,
  render, compare against golden SVG structure (not pixels).
- `build-dashboard` skill + `dashboard --check` lint know the contract: they
  discover plottable producers via `duckdb_functions()` tags and suggest the
  matching `anofox_plot_*` macro.
- Evals: add regression-dashboard cases (diagnostics, model comparison,
  GLM classifier) to `evals/`.
- Dashboard templates: "Regression report" (terms + diagnostics + summary),
  "Classifier report" (ROC, PR, calibration, confusion table), "Forecast
  report" (prediction fan + residual ACF + accuracy summary).

## Dependencies on other repos

anofox-statistics issue numbers refer to DataZooDE/anofox-statistics; tracking issue #161.

| Need | Repo | Blocks |
|---|---|---|
| U1–U7 (below) | ggplot-rs | Phase 1 roles marked "upstream" |
| `<model>_augment_by` with cooks_d/leverage/std/stud residuals, `row_id` (#151) | anofox-statistics | diagnostics, influence |
| `tidy()` long (WIP) + `conf_low/conf_high` unified + intercept row (#152) | anofox-statistics | terms |
| `glance()` with stable fields, `model_type/family/link` (#152) | anofox-statistics | summary, model dispatch |
| `roc_agg/pr_agg/calibration_agg`, `*_path_agg/_cv_agg`, `kaplan_meier_agg`, `acf_agg` emitting `curve`/`terms` (#153–#157) | anofox-statistics | curves |
| Unified test-result struct (WIP) + pairwise post-hoc table (#159) | anofox-statistics | tests |
| tags + descriptions on table macros (#160) | statistics, forecast | discovery |
| forecast in-sample residuals in `obs` schema; `model_name` ↔ `model_id` | anofox-forecast | shared diagnostics |

### Bugs found in anofox-statistics v0.10.0 (filed: DataZooDE/anofox-statistics#142–#150)

1. #143 — `*_fit_predict(...) OVER ()` / `OVER (PARTITION BY …)` returns the frame's
   last-row prediction for every row (60 identical yhat).
2. #142 — `OVER (ORDER BY i)` / expanding frame → `INTERNAL Error: Attempted to access
   index 0 within vector of size 0`.
3. #144 — Prediction intervals ignore leverage (constant half-width per group).
4. #145 — `residuals_diagnostics` description promises Cook's distance; struct lacks it.
5. #146 — `t_test_agg` with 3 group codes runs silently (n1=20 vs n2=40), `effect_size` NaN.
6. #147 — `lars_fit_agg` reports `adj_r_squared` and `residual_std_error` = 0 with r²=0.99.
7. #148 — Collinear `[x1,x1]` splits the coefficient instead of flagging aliasing.
8. #149 — Option keys: global (not per-function) validation; MAP-of-strings (forecast
   style) rejected — align option conventions across extensions.
9. #150 — Doc/field drift (`n_obs` vs `n_observations`, conventions doc labelled v0.3.0,
   Cargo version 0.1.0 vs release 0.10.0).
