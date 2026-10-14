-- anofox_statistics v0.11 → contract plots, one query per producer family.
-- Needs DuckDB v1.5.6 and the anofox_statistics extension:
--   duckdb -unsigned -c "INSTALL anofox_statistics FROM 'https://get.erpl.io'"
--   DUCKDB=duckdb python3 examples/regression/render.py examples/regression/anofox_statistics.sql out/
-- Every query returns (name, svg). The full producer list (141 functions) is
--   SELECT DISTINCT function_name, tags['anofox.output'] FROM duckdb_functions()
--   WHERE tags['anofox.contract'] = '1';
-- and scripts/statistics_conformance.py plots all of them.
LOAD anofox_statistics;

CREATE TABLE d AS
SELECT i AS id, CASE WHEN i % 2 = 0 THEN 'A' ELSE 'B' END AS grp,
       i::DOUBLE AS x1, ((i * 3) % 7)::DOUBLE AS x2,
       1.0 + 0.5 * i - 0.3 * ((i * 3) % 7) + sin(i) AS y,
       round(exp(0.2 + 0.03 * i + 0.3 * sin(i)))::DOUBLE AS y_count,
       (sin(i) + 0.05 * i > 0.8) AS label,
       1.0 / (1.0 + exp(-(sin(i) + 0.05 * i - 0.8))) AS prob,
       (2 + (i % 13) + (i % 3) * 2)::DOUBLE AS time, (i % 4 <> 0) AS event,
       CASE i % 3 WHEN 0 THEN 'low' WHEN 1 THEN 'mid' ELSE 'high' END AS level3,
       (i % 3) * 1.5 + 0.5 * sin(i) AS score, ((i // 2) % 2)::BIGINT AS g2
FROM range(1, 61) t(i);

-- terms: two models' coefficients (model_id = the method), group A.
SELECT 'terms' AS name, anofox_plot_terms(t) AS svg
FROM (SELECT * FROM ols_tidy_by('d', grp, y, [x1, x2], names := ['trend', 'cycle'])
      UNION ALL BY NAME
      SELECT * FROM huber_tidy_by('d', grp, y, [x1, x2], names := ['trend', 'cycle'])) t
WHERE grp = 'A';

-- terms over an index: rolling coefficients, a ridge path (log λ), the
-- quantile process.
SELECT 'rolling' AS name, anofox_plot_terms(t, height := 520) AS svg
FROM ols_rolling_tidy_by('d', grp, id, y, [x1, x2], window_size := 15, names := ['trend', 'cycle']) t
WHERE grp = 'A';
SELECT 'ridge_path' AS name, anofox_plot_terms(t, height := 520) AS svg
FROM ridge_path_by('d', grp, y, [x1, x2], names := ['trend', 'cycle']) t WHERE grp = 'A';
SELECT 'quantile_process' AS name, anofox_plot_terms(t, height := 520) AS svg
FROM quantile_process_by('d', grp, y, [x1, x2]) t WHERE grp = 'A';

-- summary: fit statistics of three models, one panel (scale) per metric.
SELECT 'summary' AS name, anofox_plot_summary(s, width := 760, height := 460) AS svg
FROM (SELECT * FROM ols_glance_by('d', grp, y, [x1, x2])
      UNION ALL BY NAME SELECT * FROM huber_glance_by('d', grp, y, [x1, x2])
      UNION ALL BY NAME SELECT * FROM ridge_glance_by('d', grp, y, [x1, x2])) s
WHERE grp = 'A';

-- obs: the plot.lm 2×2 of a Poisson GLM.
SELECT 'diagnostics' AS name, anofox_plot_diagnostics(o) AS svg
FROM poisson_augment_by('d', grp, y_count, [x1, x2]) o WHERE grp = 'A';

-- prediction: fit + 90 % band per group, the last rows held out as 'test'.
SELECT 'prediction' AS name, anofox_plot_prediction(p, height := 520) AS svg
FROM ols_prediction_by('d', grp, id, y, [x1, x2], level := 0.9,
                       split := CASE WHEN id > 50 THEN 'test' END) p;

-- prediction: two models in one panel (one coloured line + band each).
SELECT 'prediction_models' AS name, anofox_plot_prediction(p) AS svg
FROM (SELECT * FROM ols_prediction_by('d', grp, id, y_count, [x1, x2])
      UNION ALL BY NAME SELECT * FROM poisson_prediction_by('d', grp, id, y_count, [x1, x2])) p
WHERE grp = 'A';

-- prediction: marginal effect of x1 (x2 at its mean), GLM band.
SELECT 'effects' AS name, anofox_plot_prediction(p, height := 520) AS svg
FROM poisson_effects_by('d', grp, y_count, [x1, x2], focal := 1) p;

-- partial residuals (not a contract schema): component + residual and
-- added-variable plots.
SELECT 'partial' AS name, anofox_plot_partial(p, width := 760) AS svg
FROM ols_partial_residuals_by('d', grp, y, [x1, x2], names := ['trend', 'cycle']) p WHERE grp = 'A';
SELECT 'added_variable' AS name, anofox_plot_partial(p, kind := 'added_variable', width := 760) AS svg
FROM ols_partial_residuals_by('d', grp, y, [x1, x2], names := ['trend', 'cycle']) p WHERE grp = 'A';

-- curve: one plot per curve_type.
SELECT 'curve_' || curve_type AS name, anofox_plot_curve(c) AS svg
FROM (SELECT * FROM roc_curve_by('d', grp, label, prob)
      UNION ALL SELECT * FROM pr_curve_by('d', grp, label, prob)
      UNION ALL SELECT * FROM calibration_curve_by('d', grp, label, prob, n_bins := 5)) c
GROUP BY curve_type;
SELECT 'kaplan_meier' AS name, anofox_plot_curve(c) AS svg
FROM kaplan_meier_by('d', grp, time, event) c;
SELECT 'lambda_cv' AS name, anofox_plot_curve(c) AS svg
FROM ridge_cv_by('d', grp, y, [x1, x2]) c WHERE grp = 'A';
SELECT 'acf' AS name, anofox_plot_curve(c, height := 520) AS svg
FROM acf_by('d', grp, y, id) c;

-- test: Tukey HSD as a comparison forest, one panel per group …
SELECT 'tukey' AS name, anofox_plot_tests(t, width := 760) AS svg
FROM tukey_hsd_by('d', grp, score, level3) t;
-- … one-row tests (an aggregate per group, unnested) …
SELECT 'shapiro' AS name, anofox_plot_tests(t) AS svg
FROM (SELECT grp, unnest(shapiro_wilk_agg(y)) FROM d GROUP BY grp) t;
-- … and the raw values with significance brackets from the same tests.
SELECT 'compare' AS name,
       anofox_plot_compare(r, value := 'score', factor := 'level3',
                           tests := (SELECT list(t) FROM tukey_hsd_by('d', grp, score, level3) t
                                     WHERE t.grp = 'A')) AS svg
FROM d r WHERE grp = 'A';
SELECT 'compare_agg' AS name,
       anofox_plot_compare(r, value := 'score', factor := 'level3', kind := 'violin',
                           tests := pairwise_t_test_agg(score, level3)) AS svg
FROM d r;
