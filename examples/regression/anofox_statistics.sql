-- anofox_statistics → contract plots. Needs the anofox_statistics community
-- extension:
--   INSTALL anofox_statistics FROM community;
--   python3 examples/regression/render.py examples/regression/anofox_statistics.sql out/
LOAD anofox_statistics;

CREATE TABLE d AS
SELECT i, 'g' || (i % 2) AS grp,
       CASE WHEN i < 50 THEN 2.0 + 0.5 * i - 0.3 * (i % 5) + sin(i) END AS y,
       i::DOUBLE AS x1, (i % 5)::DOUBLE AS x2
FROM range(60) r(i);

-- *_fit_predict_by keeps the input columns and adds yhat, yhat_lower,
-- yhat_upper, is_training: name the x column, panel by the group column.
-- (Rows with y = NULL are predicted only: is_training = false → 'future'.)
SELECT 'fit_predict' AS name,
       anofox_plot_prediction(p, x := 'x1', facet := 'grp', width := 720, height := 560) AS svg
FROM ols_fit_predict_by('d', grp, y, [x1, x2]) p;

-- obs: residuals from the in-sample fit → the plot.lm 2x2.
-- (std_residual / leverage / cooks_d come with <model>_augment_by, #151.)
SELECT 'diagnostics' AS name, anofox_plot_diagnostics(o) AS svg
FROM (SELECT i AS row_id, yhat AS fitted, y - yhat AS residual
      FROM ols_fit_predict_by('d', grp, y, [x1, x2])
      WHERE is_training AND grp = 'g0') o;

-- terms: unnest the coefficient list of ols_fit_agg (tidy() long form, #152,
-- will add std_error/conf_low/conf_high); compare two specifications.
SELECT 'terms' AS name, anofox_plot_terms(t) AS svg
FROM (
    SELECT spec AS model_id, unnest(['(Intercept)'] || names) AS term,
           unnest([fit.intercept] || fit.coefficients) AS estimate
    FROM (SELECT 'x1' AS spec, ['x1'] AS names, ols_fit_agg(y, [x1]) AS fit FROM d
          UNION ALL
          SELECT 'x1+x2', ['x1', 'x2'], ols_fit_agg(y, [x1, x2]) FROM d)
) t;

-- summary: glance-style metrics of the same two fits.
SELECT 'summary' AS name, anofox_plot_summary(s, height := 420) AS svg
FROM (
    SELECT spec AS model_id, unnest(['r_squared', 'adj_r_squared', 'residual_std_error']) AS metric,
           unnest([fit.r_squared, fit.adj_r_squared, fit.residual_std_error]) AS value
    FROM (SELECT 'x1' AS spec, ols_fit_agg(y, [x1]) AS fit FROM d
          UNION ALL
          SELECT 'x1+x2', ols_fit_agg(y, [x1, x2]) FROM d)
) s;
