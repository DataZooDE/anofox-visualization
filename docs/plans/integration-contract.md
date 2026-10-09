# Anofox integration contract (draft v1)

Goal: any anofox analysis extension (statistics, forecast, tabular, tabfm, …)
emits results that anofox-visualization can plot **without hand-written
unnest/join SQL**, and one plot macro per *shape* works across all of them.

Principles

1. **Long tables, one row per mark.** No parallel LIST columns in the plotting
   path. Struct/list outputs stay available, but every model also has a long
   (`tidy`/`augment`/`glance`) form.
2. **broom names.** `estimate`, `std_error`, `statistic`, `p_value`,
   `conf_low`, `conf_high`, `fitted`, `residual`, so R/Python users recognise them.
   Forecast-style prediction columns keep the established `yhat`, `yhat_lower`,
   `yhat_upper` (already shared by statistics `_by`, forecast `ts_forecast_by`,
   tabfm).
3. **Stable shape.** Optional columns are always present (NULL when not
   computed), never conditional on options.
4. **Keys.** Every table carries `model_id VARCHAR` (or `model_name`) plus the
   user's group columns unchanged; per-row tables carry `row_id BIGINT` (or the
   time key `ds`) so nothing is joined back by list position.
5. **Discoverable.** Every producing function sets `duckdb_functions().tags`
   `{'anofox.output': '<schema>', 'anofox.contract': '1'}` and has a description,
   including table macros.

## Schemas

| Schema | Columns (required **bold**) | Produced by | Plots |
|---|---|---|---|
| `terms` (tidy) | **model_id, term, estimate**, std_error, statistic, p_value, **conf_low, conf_high**, conf_level, index_name, index_value | statistics `tidy()`, path/CV/quantile/rolling fits, GLMM ranef, forecast model coefficients | coefficient forest, multi-model forest, regularisation path, quantile process, rolling coefficients, caterpillar |
| `obs` (augment) | **model_id, row_id, y, fitted, residual**, resid_type, std_residual, stud_residual, leverage, cooks_d, weight, is_training, ds, n_params | statistics `<model>_augment_by` (new), forecast in-sample residuals | plot.lm 2×2, influence, Cook's index, binned residuals, residual-over-time, predicted-vs-actual |
| `prediction` | **model_id, x \| ds, yhat**, y, yhat_lower, yhat_upper, level, split (`train`/`test`/`future`) | statistics `_fit_predict_by`, forecast `ts_forecast_by`, marginal-effect grids | fit + band, forecast fan, marginal effects, PDP |
| `curve` | **curve_type, x, y**, model_id, series, y_low, y_high, threshold, weight, n_censor (km) | ROC/PR/calibration aggregates, lambda-CV, KM, ACF/PACF, QQ, null distribution | every line/step curve; macro picks geom + reference line from `curve_type` |
| `summary` (glance) | **model_id, metric, value**, conf_low, conf_high | statistics `glance()`, forecast accuracy metrics | model comparison dots/bars |
| `test` | **method, statistic, p_value**, test_id, group1, group2, p_adj, df1, df2, estimate, conf_low, conf_high, effect_size, effect_name, alternative, alpha | all `*_test_agg`, post-hoc pairwise, Diebold–Mariano | p-value brackets, effect-size forest, critical-region plot |

`curve_type` vocabulary: `roc, pr, calibration, lambda_cv, km, acf, pacf, qq,
null_dist, pdp, lift`.

## Model objects between extensions

The fit STRUCT stays the in-SQL model object, with a `model_type` tag
(`'ols'`, `'glm'`, …; GLMs also `family`, `link`) and field names stable under
`to_json`. `predict(model, x)`, `tidy(model)`, `glance(model)` consume it, and
`anofox_plot_model(model)` (visualization) can dispatch on `model_type`.

## Versioning & conformance

- `anofox_contract_version()` returns `'1'` in every participating extension.
- A shared conformance test (`test/sql/contract_*.test`) per extension asserts
  column names/types of each producer.
- A cross-extension CI job installs released `anofox_statistics`,
  `anofox_forecast`, `anofox_visualization` from the community repo and renders
  one plot per schema (catches drift between releases).
