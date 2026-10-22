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
| `prediction` | **model_id, x \| ds \| period, yhat**, y, yhat_lower, yhat_upper, level, split (`train`/`test`/`future`) | statistics `_fit_predict_by`, forecast `ts_forecast_by`, marginal-effect grids, twin stock projections (inventory-rs) | fit + band, forecast fan (one row per level → nested fan), marginal effects, PDP |
| `curve` | **curve_type, x, y**, model_id, series, y_low, y_high, threshold, weight, n_censor (km) | ROC/PR/calibration aggregates, lambda-CV, KM, ACF/PACF, QQ, null distribution | every line/step curve; macro picks geom + reference line from `curve_type` |
| `summary` (glance) | **model_id, metric, value**, conf_low, conf_high | statistics `glance()`, forecast accuracy metrics | model comparison dots/bars |
| `check` | **check_name, status** (`pass`/`warn`/`fail`), **value**, lower_threshold, upper_threshold, identifier, message, run_ts \| ds, check_type, table_name, column_name | anofox-tabular `run_checks`, `agg_check` (as they come); the other one-row checks with their measure as `value` and their bounds as `lower_threshold`/`upper_threshold` | acceptance band per check coloured by status (`anofox_plot_checks`); a status strip over run_ts / ds |
| `test` | **method, statistic, p_value**, test_id, group1, group2, p_adj, df1, df2, estimate, conf_low, conf_high, effect_size, effect_name, alternative, alpha | all `*_test_agg`, post-hoc pairwise, Diebold–Mariano | p-value brackets, effect-size forest, critical-region plot |
| `events` | **ds \| period, kind**, label, severity (`warning`/`alarm`), value, threshold, model_id, series | twin alarms (inventory-rs `Alarm`: tier → severity, category/name → kind/label, stream → series), parameter changes, calibration proposals | event timeline (`anofox_plot_events`), event lines over a forecast (`anofox_plot_prediction(p, events := (SELECT list(e) FROM alarms e))`) |
| `cv` (provisional) | **fold_id, split** (`train`/`test`), **ds \| period**, id, y; or **fold_id**, train_start, train_end, test_start, test_end | anofox-forecast `ts_cv_folds_by`, `ts_cv_split_by`, `ts_cv_split_folds_by` (as they come) | fold timeline (`anofox_plot_cv`) |
| `schedule` | **resource, task, start, end**, model_id, due, status (`on_time`/`late`) | anofox-optimize `schedule_*` solvers (as a long-form `*_by`, DataZooDE/anofox-optimize#6), one row per task | Gantt chart: one lane per resource, bars coloured by status, a due-date mark, faceted by model_id to compare algorithms (`anofox_plot_schedule`) |
| `graph` (nodes + edges) | nodes: **id**, title, mainstat, status, group, lane, story, x, y, sparkline, seq, upstream; edges (a LIST passed as `edges :=`): **source, target**, value, label, style, status — Grafana node-graph names; also read: `stage`, `label`, `from_stage`, `to_stage`, `count`, … | pipeline run manifests (inventory-rs#28 `run_manifest` + `run_edges`), lineage, process mining | node/edge diagram (`anofox_plot_diagram(n, edges := (SELECT list(e) FROM run_edges e))`), the `::DIAGRAM` panel |
| `decomposition` (provisional) | **id, ds \| period, component, value** (`observed`, `level`, `trend`, `seasonal`, `seasonal_<period>`, `remainder`), model_id; also read wide (one column per component) and as LISTs | anofox-forecast `ts_mstl_decomposition_by`, `ts_forecast_explain_by` (as they come, no ds); anofox-orchestration `decomposition` (wide, `t`) | stacked panels, shared x, own y (`anofox_plot_decomposition`) |
| `matrix` | **row, col, value**, model_id — also read: `column_a` / `column_b` (and `var1`/`var2`, `row_name`/`col_name`), and `pearson` / `spearman` / `kendall` / `correlation` / `corr` / `count` / `n` as the value | anofox-tabular `profile_correlations` (as it comes; a pair list is mirrored), correlation / confusion / setup matrices | heatmap, diverging around 0, −1…1 for correlations (`anofox_plot_matrix`) |

Provisional: the long-form names are to be agreed with anofox-orchestration
(DataZooDE/anofox-orchestration#539) and anofox-forecast
(DataZooDE/anofox-forecast#282).

`period` is an integer period index (a twin advancing by period); producers
write `ds` instead when a calendar is configured. `level` is the interval's
coverage as a fraction (0.8, 0.95); 80 / 95 read the same. A prediction
with several levels has one row per (x, model, level), `yhat` repeated.

`curve_type` vocabulary: `roc, pr, calibration, lambda_cv, km, acf, pacf, qq,
null_dist, pdp, lift, lowess` (`lowess`: a smoothed y over x per group, e.g.
anofox-statistics `lowess_by` — drawn as a plain line like `pdp`).

Producers write these names. The visualization side also reads common other
spellings (statsmodels `coef` / `P>|t|` / `[0.025`, broom `.fitted`, R
`Std. Error`, pingouin `p-unc`; table in `docs/DOCS.md`, *Other column
names*), so outside tables plot without a reshaping step — this does not
change contract version `1`. Nor do the readers for an interval held in one
`[low, high]` column (`conf_int`, pingouin's `CI95%`), statsforecast's wide
`<model>-lo-<L>` / `-hi-<L>` forecast, `level` 95 vs 0.95, the `period`
x key, the `events` schema, the `check` schema, the `decomposition` schema
and the `matrix` schema: all are additions (new optional columns, a new
schema, more accepted spellings) and no column changed name, type or
meaning, so the version stays `1`. A version bump is reserved for a renamed,
re-typed or re-interpreted column.

### anofox-tabular producers

anofox-tabular has no `anofox.contract` tag yet (§5 of the contract plan), so
its outputs are read through an adapter today. `scripts/tabular_conformance.py`
renders every one end to end.

| Producer | Schema | Columns read | Today |
|---|---|---|---|
| `run_checks`, `agg_check` | `check` | as it comes | works as it comes |
| `compliance`, `distinct_count`, `duplicate_count`, `freshness`, `iqr`, `match_rate`, `null_rate`, `occurrence`, `regex_match`, `schema_check`, `values_in_set`, `volume`, `zscore` | `check` | the measure renamed to `value`, the bounds to `lower_threshold` / `upper_threshold` | adapter in `scripts/tabular_conformance.py` |
| `metric_anomaly_iqr`, `rel_count_change`, `rolling_values_in_set` | `events` (or `check`) | `reference_date` → `ds`, `fail` / `warn` → `alarm` / `warning`, `message` → `label` (or `reference_date` → `ds` for `check`) | adapter |
| `metric_anomaly_iqr` | `prediction` (band) | `metric_value` → `y`, `(q1+q3)/2` → `yhat`, `lower_bound` / `upper_bound` → `yhat_lower` / `yhat_upper` | adapter |
| `profile_table`, `profile_summary`, `isolation_forest[_mv]` / `outlier_tree` / `dbscan[_mv]` summary modes | `summary` | as it comes (unpivoted) | adapter |
| `outlier_tree` outliers mode | `terms` | as it comes | adapter |
| `profile_correlations` | `matrix` | as it comes (`column_a × column_b`, mirrored) | works as it comes |
| diffs (`diff_hashdiff`, `diff_joindiff`), PII (`pii_scan_table`, `pii_audit_table`) | — | — | generic charts, no schema |

Provisional: anofox-orchestration's backtest numbers folds as `fold_idx` (0 =
most recent) and anofox-forecast as `fold_id` (1 = oldest); one convention is
to be agreed in DataZooDE/anofox-orchestration#539.

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
  `anofox_forecast`, `anofox_tabular`, `anofox_visualization` from the
  community repo and renders one plot per schema (catches drift between
  releases).
