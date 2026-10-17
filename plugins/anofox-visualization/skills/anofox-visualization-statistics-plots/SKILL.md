---
name: anofox-visualization-statistics-plots
description: >
  Plot model and test output of anofox-statistics (v0.11) and anofox-forecast
  with the anofox_visualization contract macros: anofox_plot_terms
  (coefficient forest, lambda / LARS paths, quantile process, rolling
  coefficients), anofox_plot_summary (model comparison of glance metrics),
  anofox_plot_obs and anofox_plot_diagnostics (residuals vs fitted, QQ,
  scale-location, leverage, Cook's distance, the plot.lm 2x2),
  anofox_plot_prediction (fit + band, train / test / future split,
  forecasts, effect plots, model differences), anofox_plot_curve (ROC, PR,
  calibration, Kaplan-Meier, ACF / PACF, CV error with lambda.min /
  lambda.1se), anofox_plot_tests (post-hoc comparison forest),
  anofox_plot_compare (box / violin / points with significance brackets),
  anofox_plot_partial (component + residual, added-variable),
  anofox_plot_events (alarm / parameter-change timeline; events := over a
  forecast), anofox_plot_diagram (a process / pipeline graph from node and
  edge tables) and anofox_plot (auto), with every option (title, x_label,
  y_label, highlight, lambda_min, lambda_1se, facet, reference, band, kind,
  events, base_rate, metrics,
  baseline, label_top, alpha, value, label, factor, tests, hide_ns, width,
  height), anofox_contract_version(), the alias-the-row calling convention,
  GROUP BY for one plot per group, the producer to macro table, and using
  them in ::ROLE dashboards and the browser builder. Use when the user wants
  to plot a regression, coefficients, residual diagnostics, a forecast, a
  ROC curve, survival curves or test results, asks "how do I plot
  ols_tidy_by / ts_forecast_by / statsforecast output", or a contract plot
  call errors. Also for results computed in Python or R: the anofox_viz
  Python package (from_statsmodels, from_sklearn, from_estimator,
  from_scipy, from_pingouin, from_lifelines, from_statsforecast,
  from_prophet, plot), pandas / polars DataFrames in-process through the
  duckdb package, Jupyter, R broom tables, and Parquet files into a
  dashboard.
version: 2026.10.18
user-invocable: false
---

# Anofox Visualization — Statistics Plots (contract macros) Cheat Sheet

**Extension:** `anofox_visualization` (CalVer 2026.10.18) | **Contract:** `anofox_contract_version()` = `'1'` | **Producers:** `anofox_statistics` v0.11, `anofox_forecast` | **DuckDB:** v1.5.6

anofox-statistics and anofox-forecast return plot-ready **long tables** with fixed columns (the anofox integration contract: `terms`, `summary`, `obs`, `prediction`, `curve`, `test`, `events`). One macro per schema renders such a table to SVG. The schemas and their producers are documented in the **anofox-statistics-plotting** skill of the anofox-statistics plugin; this skill is the consumer side.

## Calling convention

The macros are **aggregates over whole rows**: alias the table and pass the alias. DuckDB hands each row over as a STRUCT, so the macro sees the column names.

```sql skip
SELECT anofox_plot_terms(t) FROM ols_tidy_by('d', grp, y, [x1, x2]) t;               -- one SVG
SELECT grp, anofox_plot_terms(t) FROM ols_tidy_by('d', grp, y, [x1, x2]) t GROUP BY grp;  -- one per group
```

- Not `anofox_plot_terms(*)` and not a table-name string.
- `WHERE` / `FILTER` / `GROUP BY` work like for any aggregate; combine models with `UNION ALL BY NAME`.
- Every macro takes `title := NULL, x_label := NULL, y_label := NULL, width := 640, height := 400` (diagnostics 820 × 640). No inner title by default; axes are labelled from the contract's meaning.

## Macros and options

| Macro | Needs (**bold** = required) | Options |
|---|---|---|
| `anofox_plot_terms(t, …)` | **term, estimate**, conf_low, conf_high, model_id, id, index_name, index_value, window_end | `highlight := ['tv', …]` (grey the other path lines), `lambda_min :=` / `lambda_1se :=` (dashed λ marks; else read from `lambda_min` / `is_min` columns), `facet := true` (one panel per term on a λ path), `reference := (SELECT list(r) FROM ols_tidy_by(…) r)` (full-sample reference line on rolling / quantile panels) or a number (`reference := 1`: the no-effect line, e.g. odds ratios). A coefficient whose interval contains the null (0, or the numeric `reference`) is drawn faded (half opacity, "n.s." in its tooltip, caption "faded = interval crosses 0"). One model's coefficients are blue above the null and red below it; several models keep their series colours |
| `anofox_plot_summary(s, …)` | **metric, value**, model_id \| model_name, id, conf_low, conf_high | `metrics := ['r_squared', 'aic', …]` (panels and their order; `n_observations`, `n_features`, `iterations` only when asked), `baseline := 'range'` (zoom to the values) \| `'zero'` |
| `anofox_plot_obs(o, …)` | **fitted, residual**, row_id, std_residual, leverage, cooks_d, n_params | `kind := 'resid_fitted'` \| `'qq'` \| `'scale_location'` \| `'leverage'` \| `'cooks'`, `label_top := 3` (label the rows with the largest Cook's distance) |
| `anofox_plot_diagnostics(o, …)` | as `obs` | `label_top := 3`; the plot.lm 2×2 in one SVG (`x_label` / `y_label` ignored) |
| `anofox_plot_prediction(p, …)` | **x \| ds \| period, yhat**, y, yhat_lower, yhat_upper, level, split \| is_training, model_id \| model_name, id, focal | `x := '<col>'`, `y := '<col>'` (other columns as x / y), `facet := '<col>'` (default: panels by `id` when there are several), `band := '<model>'` (which model's band; default `ols` or the first), `kind := 'fit'` \| `'difference'` (each model minus the band model), `events := (SELECT list(e) FROM alarms e)` (an `events` table as vertical lines coloured by severity). Several `level`s (one row per x and level) draw a nested fan; an integer `period` is the x when there is no `ds` |
| `anofox_plot_events(e, …)` | **ds \| period, kind**, label, severity (`alarm` / `warning`), value, threshold, model_id, series | `x := '<col>'`; a timeline, one lane per series (else kind), dots coloured by severity, label / value / threshold on hover (default 640 × 260) |
| `anofox_plot_curve(c, …)` | **curve_type, x, y**, y_low, y_high, model_id \| series \| id, threshold, weight, n_censor, n_risk, is_min, is_1se | `base_rate :=` (PR base-rate line). Geom by `curve_type`: `roc`, `pr`, `calibration`, `km`, `acf`, `pacf`, `lambda_cv`, `qq`, `pdp`, `lift`, `null_dist`, `lowess` (`lowess_by`). **One `curve_type` per call** |
| `anofox_plot_tests(t, …)` | **p_value \| p_adj \| statistic**, test_id, group1, group2, estimate, conf_low, conf_high, method | `alpha := 0.05` (significance colour), `value := 'estimate'` \| `'statistic'` \| `'p'` (measure; default estimate, else statistic, else −log10 p), `label := 'p'` (default: p and stars) \| `'stars'`. A STRUCT / LIST column (`tukey_hsd_agg(y, g) AS r`) is expanded |
| `anofox_plot_compare(r, …)` | the raw rows | **`value := '<numeric col>'`, `factor := '<group col>'`**, `tests :=` a LIST of test STRUCTs (`(SELECT list(t) FROM tukey_hsd_by(…) t WHERE …)`, `tukey_hsd_agg(y, g)`, a two-sample STRUCT such as `t_test_agg(y, g)`), `kind := 'box'` \| `'violin'` \| `'points'`, `hide_ns :=` (drop p > 0.05; default true above 3 groups), `label := 'stars'` (default) \| `'p'` \| a template (`'{p.signif}'`, `'p = {p.adj}'`) |
| `anofox_plot_partial(p, …)` | **term, x, partial_residual** (+ component) or **term, x_residual, y_residual** | `kind := 'component'` \| `'added_variable'` |
| `anofox_plot_diagram(n, …)` | **id** (or `stage`), title \| label, mainstat, status, group, lane, story, x, y, sparkline, seq | `edges :=` a LIST of **source, target** (or `from_stage` / `to_stage`), value \| count, label, style, status; `mainstat := '<col>'`, `format := 'compact'`, `direction := 'LR'` \| `'TB'`, `style :=` (default edge style). The `::DIAGRAM` panel from two tables (dashboards skill → *Diagrams*); 800 × 400 |
| `anofox_plot(t, …)` | any of the above | picks the chart from the columns: term+estimate → terms, curve_type → curve, yhat → prediction, fitted+residual → diagnostics, metric+value → summary, partial_residual → partial, p_value / test_id → tests, kind \| severity + ds \| period → events. Only the common options |

Every macro also takes `theme :=` (a built-in name, `'a+b'`, or theme JSON) and `locale :=` (`'de-DE'`, …); unset they default to `getvariable('anofox_theme')` / `getvariable('anofox_locale')`, so `SET VARIABLE anofox_theme = 'dark';` restyles every plot of a session. Model colours keep their palette *slot* (ols = slot 0) in every theme.

Errors are SQL errors naming the problem (missing column, unknown `kind`, mixed `curve_type`, the contract version); no rows give a "No data" SVG. A missing-column error lists the columns it got, then either the near miss ("did you mean \`estimat\` for estimate?") or the other names the column accepts ("estimate is also read from coef, params, coefficient"); for a table of another schema it adds "the columns follow the prediction schema (anofox_plot_prediction)". `anofox_plot(t)` names the schema it detected ("detected the curve schema"), and when none matches, the closest one and what it misses ("closest: terms, missing estimate"). Tests naming groups that are not in the rows of `anofox_plot_compare` are skipped and reported on the SVG root (`data-warnings`).

## Without anofox_statistics: contract-shaped tables

Any table with the contract columns works, e.g. your own coefficients:

```sql
SELECT anofox_plot_terms(c, title := 'Two models', x_label := 'effect (k€)')
FROM (VALUES ('area', 'ols', 3.3, 2.9, 3.7), ('area', 'huber', 3.1, 2.8, 3.4),
             ('age', 'ols', -0.9, -1.4, -0.4), ('age', 'huber', -0.8, -1.2, -0.4))
     c(term, model_id, estimate, conf_low, conf_high);

SELECT anofox_plot_curve(r, base_rate := 0.3)
FROM (VALUES ('pr', 0.0, 1.0), ('pr', 0.5, 0.7), ('pr', 1.0, 0.3)) r(curve_type, x, y);

SELECT anofox_plot_summary(s, baseline := 'zero')
FROM (VALUES ('ols', 'r_squared', 0.81), ('ridge', 'r_squared', 0.79),
             ('ols', 'aic', 72.4), ('ridge', 'aic', 75.0)) s(model_id, metric, value);

SELECT anofox_plot_tests(t, alpha := 0.01, label := 'stars')
FROM (VALUES ('b-a', 'a', 'b', 1.2, 0.4, 2.0, 0.003), ('c-a', 'a', 'c', 0.3, -0.5, 1.1, 0.41))
     t(test_id, group1, group2, estimate, conf_low, conf_high, p_adj);

SELECT anofox_contract_version();   -- '1'
```

**Events and integer periods** (a digital twin's projection and alarms): `period` is the x when there is no `ds`, one row per level draws a nested fan, and `events :=` puts the alarms on it:

```sql
CREATE TABLE alarms AS SELECT * FROM (VALUES
  (4, 'stockout', 'on hand 0', 'alarm', 0.0, 5.0, 'SKU-1'),
  (9, 'drift', 'CUSUM', 'warning', 4.2, 4.0, 'SKU-1')) t(period, kind, label, severity, value, threshold, series);
SELECT anofox_plot_prediction(p, events := (SELECT list(e) FROM alarms e), y_label := 'on hand')
FROM (SELECT p AS period, CASE WHEN p <= 8 THEN 20.0 - p END AS y, 20.0 - p AS yhat,
             20.0 - p - w AS yhat_lower, 20.0 - p + w AS yhat_upper, l AS level
      FROM range(1, 17) r(p), (VALUES (0.8, 2.0), (0.95, 3.5)) v(l, w)) p;
SELECT anofox_plot_events(e) FROM alarms e;   -- the timeline on its own
```

**A pipeline from a node and an edge table** (`graph` schema; Grafana node-graph names):

```sql
CREATE TABLE stages AS SELECT * FROM (VALUES
  ('ingest', 'Ingest', 10000, 'ok'), ('build', 'Build twin', 9800, 'ok'), ('monitor', 'Monitor', 412, 'alarm')) t(id, title, mainstat, status);
CREATE TABLE flows AS SELECT * FROM (VALUES ('ingest', 'build', 9800), ('build', 'monitor', 412)) t(source, target, value);
SELECT anofox_plot_diagram(n, edges := (SELECT list(e) FROM flows e)) FROM stages n;
```

**Other producers' column names are read as contract columns**, so raw statsmodels / R / broom / pingouin tables plot without renaming. Names compare with case, `.`, spaces and symbols ignored (`Std. Error` = `std.error` = `std_error`, broom's `.fitted` = `fitted`), plus these spellings (the first present wins; a column the contract name already has, or one an option names, is left alone; `anofox_plot_compare` reads its rows as given):

| Contract column | Also read from |
|---|---|
| `estimate` | `coef`, `params`, `coefficient` |
| `std_error` | `std_err`, `bse`, `std_errors`, `se` |
| `statistic` | `t`, `z`, `tvalues`, `zvalues`, `t value`, `z value`, `t_stat` |
| `p_value` | `P>\|t\|`, `P>\|z\|`, `Pr(>\|t\|)`, `Pr(>\|z\|)`, `p-val`, `p-unc`, `pvalues`, `pval`, `p` |
| `p_adj` | `p-corr`, `padj`, `p_adjusted` |
| `conf_low` / `conf_high` | `[0.025` / `0.975]` (any level), `ci_lower` / `ci_upper`, `lower` / `upper`, `mean_ci_lower` / `mean_ci_upper`, `conf_lower` / `conf_upper` |
| `fitted`, `residual`, `std_residual` | `fittedvalues`; `resid`, `residuals`; `std_resid`, `standard_resid` |
| `leverage`, `cooks_d` | `hat`, `hat_diag`; `cooksd`, `cooks_distance`, `cook_d` |

In a table with `yhat`, the interval columns (`lower` / `upper`, `mean_ci_*`, `ci_*`, `conf_*`, `[0.025` / `0.975]`) become `yhat_lower` / `yhat_upper`. Also read without reshaping:

- an interval in **one** column — a `[low, high]` LIST or two-field STRUCT named `conf_int`, `ci`, `confint` or `CI95%` (pingouin; the level in the name fills `conf_level`);
- `level` / `conf_level` 95 = 0.95;
- **statsforecast's wide forecast** (`unique_id, ds, AutoARIMA, AutoARIMA-lo-95, AutoARIMA-hi-95, …`, also mlforecast / neuralforecast): one row per model and level, so `anofox_plot_prediction(f)` draws a line per model and a nested fan. Without `level=` there are no `-lo-` columns: `UNPIVOT f ON COLUMNS(* EXCLUDE (unique_id, ds)) INTO NAME model_id VALUE yhat` first;
- `unique_id` = `id` (panels).

`anofox_plot_summary` marks the best model for metrics with a known direction: R², explained variance, AUC, average precision, accuracy, F1, precision, recall, MCC, Cohen's κ, Jaccard (higher); RMSE, MSE, MAE, MAPE, sMAPE, MASE, CRPS, Brier score, log loss, the hinge / pinball / Poisson / gamma / Tweedie losses, max error, AIC, BIC (lower). scikit-learn's long names (`mean_absolute_error`, `roc_auc_score`) work, and a scorer's `neg_` prefix flips the direction. In `anofox_plot_tests` a text column whose values are all numbers (pingouin's `BF10`) is a value, not a panel key. A statsmodels coefficient frame still needs its index as `term`:

```python
tbl = res.summary2().tables[1].reset_index(names="term")   # Coef., Std.Err., t, P>|t|, [0.025, 0.975]
duckdb.sql("SELECT anofox_plot_terms(t) FROM tbl t")
```

## From Python (and R)

The macros read any contract-shaped table, wherever it was computed (`docs/python-r.md`). In-process, the `duckdb` package scans a pandas / polars / Arrow frame by its variable name; the extension is unsigned, so allow that before `LOAD`:

```python
import duckdb
con = duckdb.connect(config={"allow_unsigned_extensions": "true"})
con.execute("LOAD './anofox_visualization.duckdb_extension'")
svg = con.execute("SELECT anofox_plot_terms(r) FROM coefs AS r").fetchone()[0]   # coefs: a DataFrame
```

The **`anofox_viz` package** (`pip install "git+https://github.com/DataZooDE/anofox-visualization#subdirectory=python"`) does the reshaping and the round trip; `plot()` returns an SVG that Jupyter shows inline (`.save("x.svg")`):

```python
import anofox_viz as av
av.connect("./anofox_visualization.duckdb_extension")   # or $ANOFOX_VIZ_EXTENSION
t = av.from_statsmodels(res)                 # {"terms", "obs", "summary", "prediction"}
av.plot(t, "diagnostics")                    # the plot.lm 2x2; options are macro arguments
c = av.from_sklearn(y_true, y_score, model_id="logistic")   # {"curve", "summary"}
roc = av.plot(c["curve"], "curve", by="curve_type")["roc"]
av.plot(av.from_statsforecast(fc, level=95, history=hist), "prediction")
```

Converters: `from_statsmodels` (OLS, GLM, Logit, linearmodels; Tukey HSD), `from_estimator` (any fitted scikit-learn estimator), `from_sklearn`, `from_scipy` (`scipy.stats` tests), `from_pingouin`, `from_lifelines` (Kaplan–Meier), `from_statsforecast` (`forecast()` / `cross_validation()`), `from_prophet`. Without `chart`, `plot` uses `anofox_plot` (auto). R: `broom::tidy` / `augment` / `glance` tables plot as they are (`std.error`, `.fitted`, `.cooksd` are aliases). For dashboards and locked servers, write the tables to Parquet and read them in the dashboard SQL. End to end: `examples/python/plot-results.ipynb`.

## Producer → macro (anofox-statistics v0.11, anofox-forecast)

| Producer | Schema | Macro |
|---|---|---|
| `<model>_tidy_by` | terms | `anofox_plot_terms(t)` — forest; models (`UNION ALL BY NAME`) or groups (`id`) dodged + coloured |
| `ridge_path_by`, `elasticnet_path_by`, `lars_path_by` | terms (`index_name` λ / alpha) | `anofox_plot_terms(t, highlight := […])` — one panel, log-λ axis, direct labels |
| `quantile_process_by`, `<model>_rolling_tidy_by` | terms (`tau` / `window_end`) | `anofox_plot_terms(t, reference := …)` — one panel per term |
| `<model>_glance_by` | summary | `anofox_plot_summary(s, metrics := …)` |
| `ols_augment_by`, `wls_augment_by`, `<glm>_augment_by` | obs | `anofox_plot_diagnostics(o)`, `anofox_plot_obs(o, kind := …)` |
| `<model>_prediction_by`, `<model>_effects_by` | prediction | `anofox_plot_prediction(p)` |
| `<model>_fit_predict_by` | source columns + yhat… | `anofox_plot_prediction(p, x := 'x1', facet := 'grp')` |
| `roc_curve_by`, `pr_curve_by`, `calibration_curve_by`, `kaplan_meier_by`, `acf_by`, `pacf_by`, `ridge_cv_by`, `elasticnet_cv_by` | curve | `anofox_plot_curve(c)` |
| `tukey_hsd_by`, `pairwise_t_test_by`, `dunn_test_by`, the `*_agg` tests | test | `anofox_plot_tests(t)`; raw values + brackets: `anofox_plot_compare(r, value :=, factor :=, tests :=)` |
| `<model>_partial_residuals_by` | (not a contract schema) | `anofox_plot_partial(p, kind := …)` |
| `ts_forecast_by` (anofox-forecast) | prediction (no `y`) | `anofox_plot_prediction(f)`; union the history to show actuals |
| statsforecast `sf.forecast(…, level=[80, 95])` | prediction (wide, melted on read) | `anofox_plot_prediction(f)` — a line per model, nested fan |
| twin alarms / parameter changes (inventory-rs) | events | `anofox_plot_events(e)`, or `anofox_plot_prediction(p, events := (SELECT list(e) FROM alarms e))` |

Discover the producers of the loaded extensions:

```sql
LOAD anofox_statistics;
SELECT DISTINCT function_name, tags['anofox.output'] AS output
FROM duckdb_functions() WHERE tags['anofox.contract'] = '1'
ORDER BY output, function_name LIMIT 10;
```

## Worked examples (anofox_statistics)

```sql
LOAD anofox_statistics;
CREATE TABLE d AS
SELECT i AS id, CASE WHEN i % 2 = 0 THEN 'A' ELSE 'B' END AS grp, i::DOUBLE AS x1,
       ((i * 3) % 7)::DOUBLE AS x2, 1.0 + 0.5 * i - 0.3 * ((i * 3) % 7) + sin(i) AS y,
       round(exp(0.2 + 0.05 * i))::DOUBLE AS y_count, (sin(i) + 0.05 * i > 0.5) AS label,
       1.0 / (1.0 + exp(-(sin(i) + 0.05 * i - 0.5))) AS prob,
       (ARRAY['a', 'b', 'c'])[1 + i % 3] AS level3, 2.0 + (i % 3) + cos(i) AS score
FROM range(1, 61) t(i);

-- terms: OLS vs Huber for group A
SELECT anofox_plot_terms(t)
FROM (SELECT * FROM ols_tidy_by('d', grp, y, [x1, x2], names := ['trend', 'cycle'])
      UNION ALL BY NAME
      SELECT * FROM huber_tidy_by('d', grp, y, [x1, x2], names := ['trend', 'cycle'])) t
WHERE grp = 'A';

-- summary: two models, chosen metrics
SELECT anofox_plot_summary(s, metrics := ['r_squared', 'aic'])
FROM (SELECT * FROM ols_glance_by('d', grp, y, [x1, x2])
      UNION ALL BY NAME SELECT * FROM huber_glance_by('d', grp, y, [x1, x2])) s
WHERE grp = 'A';

-- obs: plot.lm 2x2 and one panel per group
SELECT anofox_plot_diagnostics(o) FROM ols_augment_by('d', grp, y, [x1, x2]) o WHERE grp = 'A';
SELECT grp, anofox_plot_obs(o, kind := 'cooks', label_top := 5)
FROM ols_augment_by('d', grp, y, [x1, x2]) o GROUP BY grp ORDER BY grp;

-- prediction: 90% band, the last rows held out; two models' difference; effect plot
SELECT anofox_plot_prediction(p)
FROM ols_prediction_by('d', grp, id, y, [x1, x2], level := 0.9,
                       split := CASE WHEN id > 50 THEN 'test' ELSE 'train' END) p;
SELECT anofox_plot_prediction(p, kind := 'difference', band := 'ols')
FROM (SELECT * FROM ols_prediction_by('d', grp, id, y_count, [x1, x2])
      UNION ALL BY NAME SELECT * FROM poisson_prediction_by('d', grp, id, y_count, [x1, x2])) p
WHERE grp = 'A';
SELECT anofox_plot_prediction(p) FROM poisson_effects_by('d', grp, y_count, [x1, x2], focal := 1) p;
SELECT anofox_plot_prediction(p, x := 'x1', facet := 'grp')
FROM ols_fit_predict_by('d', grp, y, [x1, x2]) p;

-- curve: one plot per curve_type; CV error with lambda.min / lambda.1se
SELECT curve_type, anofox_plot_curve(c)
FROM (SELECT * FROM roc_curve_by('d', grp, label, prob)
      UNION ALL SELECT * FROM calibration_curve_by('d', grp, label, prob, n_bins := 4)) c
GROUP BY curve_type ORDER BY curve_type;
SELECT anofox_plot_curve(c) FROM ridge_cv_by('d', grp, y, [x1, x2]) c WHERE grp = 'A';

-- paths: ridge lambda path with highlighted terms
SELECT anofox_plot_terms(t, highlight := ['x1'])
FROM ridge_path_by('d', grp, y, [x1, x2], names := ['x1', 'x2']) t WHERE grp = 'A';

-- tests: comparison forest per group; one-row tests per group; brackets on raw values
SELECT anofox_plot_tests(t) FROM tukey_hsd_by('d', grp, score, level3) t;
SELECT anofox_plot_tests(t, value := 'p')
FROM (SELECT grp, unnest(shapiro_wilk_agg(y)) FROM d GROUP BY grp) t;
SELECT anofox_plot_compare(r, value := 'score', factor := 'level3', kind := 'violin',
         tests := (SELECT list(t) FROM tukey_hsd_by('d', grp, score, level3) t WHERE t.grp = 'A'))
FROM d r WHERE grp = 'A';
SELECT grp, anofox_plot_compare(r, value := 'score', factor := 'level3', kind := 'points',
         label := 'p = {p.adj}', tests := (SELECT list(t) FROM dunn_test_by('d', grp, score, level3) t WHERE t.grp = r.grp))
FROM d r GROUP BY grp ORDER BY grp;

-- partial residuals
SELECT anofox_plot_partial(p, kind := 'added_variable')
FROM ols_partial_residuals_by('d', grp, y, [x1, x2]) p WHERE grp = 'A';

-- auto
SELECT anofox_plot(t) FROM ols_tidy_by('d', grp, y, [x1, x2]) t WHERE grp = 'B';
```

## Forecasts (anofox_forecast)

`ts_forecast_by(table, id, ds, y, model, horizon, freq)` returns `id, forecast_step, ds, yhat, yhat_lower, yhat_upper, model_name` (no `y`). Union the history so the plot shows actuals, then the forecast fan; several ids become panels, `GROUP BY id` gives one SVG each.

```sql
LOAD anofox_forecast;
CREATE TABLE sales AS
SELECT store AS id, (DATE '2024-01-01' + INTERVAL (i) DAY)::TIMESTAMP AS ds,
       base + 0.4 * i + 6 * sin(i * 2 * pi() / 7) AS y
FROM (VALUES ('north', 50.0), ('south', 80.0)) s(store, base), range(70) r(i);

SELECT anofox_plot_prediction(p, y_label := 'units')
FROM (SELECT id, ds, y, NULL::DOUBLE AS yhat, NULL::DOUBLE AS yhat_lower,
             NULL::DOUBLE AS yhat_upper, NULL AS model_name
      FROM sales WHERE ds >= TIMESTAMP '2024-02-15'
      UNION ALL
      SELECT id, ds, NULL, yhat, yhat_lower, yhat_upper, model_name
      FROM ts_forecast_by('sales', id, ds, y, 'AutoETS', 14, '1d')) p;

SELECT id, anofox_plot_prediction(f) FROM ts_forecast_by('sales', id, ds, y, 'AutoETS', 14, '1d') f
GROUP BY id ORDER BY id;
```

## In dashboards and the browser builder

A select item `anofox_plot_*(alias, …)` is a dashboard panel: the planner rewrites it to `to_json(list(alias))` and draws it with the same renderer, so the script works with `dashboard --check`, `serve` and in the browser builder (DuckDB-Wasm loads the anofox_statistics wasm extension; the visualization extension is not needed there). Add a `'…'::TITLE` column for the panel title; extra unannotated columns plus `GROUP BY` give one panel per row; named arguments may use `getvariable(…)` and scalar subqueries; `width` / `height` are ignored.

```sql dashboard
LOAD anofox_statistics;
CREATE TABLE d AS
SELECT i AS id, CASE WHEN i % 2 = 0 THEN 'A' ELSE 'B' END AS grp, i::DOUBLE AS x1,
       ((i * 3) % 7)::DOUBLE AS x2, 1.0 + 0.5 * i - 0.3 * ((i * 3) % 7) + sin(i) AS y
FROM range(1, 61) t(i);

SELECT grp AS grp ::DROPDOWN FROM d GROUP BY grp ORDER BY grp;
SELECT 6::COL;
SELECT anofox_plot_terms(t, x_label := 'effect on y'), 'Both features matter'::TITLE
FROM ols_tidy_by('d', grp, y, [x1, x2]) t WHERE grp = getvariable('grp');
SELECT 6::COL;
SELECT anofox_plot_obs(o, kind := 'qq'), 'Residuals are close to normal'::TITLE
FROM ols_augment_by('d', grp, y, [x1, x2]) o WHERE grp = getvariable('grp');
```

A full example of every producer in ten tabs: `web/examples/statistics-showcase.sql` (the live demo's "Statistics showcase"). For a hand-made chart from the same columns, the dashboard roles (`term::XAXIS, estimate::SCATTER, conf_low::YMIN, conf_high::YMAX, 1::FLIP, 0::REFLINE`) are in the dashboards skill.

## Setup

```sh
duckdb -unsigned        # DuckDB v1.5.6: get.erpl.io serves stale anofox_statistics builds for v1.5.5 / v1.5.4
```

```sql skip
INSTALL anofox_statistics FROM 'https://get.erpl.io';   -- once
LOAD anofox_statistics;
INSTALL anofox_forecast FROM community;                 -- once
LOAD anofox_forecast;
LOAD '/path/anofox_visualization.duckdb_extension';      -- or INSTALL … FROM community
```

Both the erpl.io build and a local anofox_visualization build are unsigned: without `-unsigned` the `LOAD` fails with "signature is either missing or invalid".

## Gotchas

1. **Alias the row**: `SELECT anofox_plot_terms(t) FROM ols_tidy_by(…) t`.
2. **`factor :=`, not `group :=`**: `group` is a reserved word (`group := 'g'` is a syntax error). `value :=` / `factor :=` name columns of the aliased rows.
3. **One `curve_type` per curve plot**: ROC and calibration rows stack with `UNION ALL`, but plot them with `GROUP BY curve_type` (mixing is an error: "rows mix curve_type …").
4. **Combine models with `UNION ALL BY NAME`**; `model_id` tells methods apart, `id` the groups. Filter (`WHERE grp = 'A'`) or `GROUP BY grp` for one SVG per group.
5. **`_augment_by` has no `id`**: filter it to one group for a clean 2×2.
6. **LARS paths are on upstream's alpha scale**, not glmnet's λ: do not overlay `lars_path_by` with `ridge_path_by`.
7. **`*_fit_predict_by` is not the `prediction` schema**: name the x column (`x := 'x1'`) or use `<model>_prediction_by`.
8. **`ts_forecast_by` has no `y`**: union the history, or the plot shows only the forecast.
9. **Bookkeeping metrics** (`n_observations`, `n_features`, `iterations`) are hidden in `anofox_plot_summary` unless `metrics :=` asks for them.
10. **`lambda` is a SQL keyword**: quote `"lambda"` when you select it from CV tables.
11. **Versions**: DuckDB v1.5.6, `-unsigned`, `INSTALL anofox_statistics FROM 'https://get.erpl.io'`; `anofox_contract_version()` must be `'1'` (the function is shared: whichever anofox extension loads first registers it).
