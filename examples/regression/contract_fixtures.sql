-- Contract plot macros on self-contained fixture tables, one per schema of
-- docs/plans/integration-contract.md. Needs only anofox_visualization:
--   python3 examples/regression/render.py examples/regression/contract_fixtures.sql out/
-- Every query returns (name, svg); render.py writes out/<name>.svg.

-- terms: two models' coefficients with 95% intervals ------------------------
CREATE TABLE coefs AS SELECT * FROM (VALUES
  ('ols',   '(Intercept)', 1.20,  0.80, 1.60),
  ('ols',   'price',      -0.52, -0.71, -0.33),
  ('ols',   'promo',       0.31,  0.12, 0.50),
  ('ols',   'season',      0.05, -0.10, 0.20),
  ('ridge', '(Intercept)', 1.10,  0.78, 1.42),
  ('ridge', 'price',      -0.41, -0.57, -0.25),
  ('ridge', 'promo',       0.25,  0.10, 0.40),
  ('ridge', 'season',      0.03, -0.09, 0.15)
) t(model_id, term, estimate, conf_low, conf_high);
SELECT 'terms_forest' AS name, anofox_plot_terms(c) AS svg FROM coefs c;

-- terms with an index: a lasso regularisation path ---------------------------
CREATE TABLE path AS
SELECT 'lasso' AS model_id, term, 'lambda' AS index_name, lam AS index_value,
       b * greatest(0, 1 - lam / s) AS estimate,
       b * greatest(0, 1 - lam / s) - 0.05 AS conf_low,
       b * greatest(0, 1 - lam / s) + 0.05 AS conf_high
FROM (VALUES ('price', -0.6, 2.0), ('promo', 0.4, 1.0), ('season', 0.1, 0.3)) v(term, b, s),
     (SELECT pow(10, -3 + i / 10.0) AS lam FROM range(31) r(i));
SELECT 'terms_path' AS name, anofox_plot_terms(p, width := 640, height := 560) AS svg FROM path p;

-- prediction: fit on train, evaluate on test, forecast the future ------------
CREATE TABLE pred AS
SELECT i AS x,
       CASE WHEN i < 50 THEN 10 + 0.3 * i + 2 * sin(i / 3.0) END AS y,
       10 + 0.3 * i AS yhat,
       10 + 0.3 * i - 2.5 - 0.02 * greatest(0, i - 40) AS yhat_lower,
       10 + 0.3 * i + 2.5 + 0.02 * greatest(0, i - 40) AS yhat_upper,
       CASE WHEN i < 40 THEN 'train' WHEN i < 50 THEN 'test' ELSE 'future' END AS split
FROM range(60) r(i);
SELECT 'prediction' AS name, anofox_plot_prediction(p) AS svg FROM pred p;

-- curve: ROC, calibration, ACF, lambda CV, Kaplan-Meier, QQ -----------------
CREATE TABLE curves AS
SELECT 'roc' AS curve_type, m AS model_id, f AS x, power(f, k) AS y, NULL::DOUBLE AS y_low, NULL::DOUBLE AS y_high
FROM (VALUES ('logit', 0.35), ('tree', 0.55)) v(m, k), (SELECT i / 20.0 AS f FROM range(21) r(i))
UNION ALL
SELECT 'calibration', 'logit', p, least(1, p * 0.9 + 0.08 * sin(p * 6)), NULL, NULL
FROM (SELECT i / 10.0 AS p FROM range(11) r(i))
UNION ALL
SELECT 'acf', 'resid', lag, power(0.6, lag) * cos(lag), -0.2, 0.2 FROM range(1, 21) r(lag)
UNION ALL
SELECT 'lambda_cv', 'lasso', pow(10, -3 + i / 4.0), 1 + power((i - 7) / 6.0, 2), 1 + power((i - 7) / 6.0, 2) - 0.1, 1 + power((i - 7) / 6.0, 2) + 0.1 FROM range(13) r(i)
UNION ALL
SELECT 'km', g, t, exp(-t / s), NULL, NULL FROM (VALUES ('control', 8.0), ('treated', 14.0)) v(g, s), range(0, 25, 2) r(t)
UNION ALL
SELECT 'qq', 'resid', q, q * 1.1 + 0.1 * q * q * q, NULL, NULL FROM (SELECT (i - 15) / 6.0 AS q FROM range(31) r(i));
SELECT 'curve_' || curve_type AS name, anofox_plot_curve(c, width := 480, height := 340) AS svg
FROM curves c GROUP BY curve_type ORDER BY curve_type;

-- summary: model comparison --------------------------------------------------
CREATE TABLE glance AS SELECT * FROM (VALUES
  ('ols', 'r2', 0.81, NULL, NULL), ('ridge', 'r2', 0.79, NULL, NULL), ('lasso', 'r2', 0.77, NULL, NULL),
  ('ols', 'rmse', 1.92, 1.80, 2.05), ('ridge', 'rmse', 1.97, 1.85, 2.10), ('lasso', 'rmse', 2.05, 1.90, 2.20),
  ('ols', 'aic', 412.0, NULL, NULL), ('ridge', 'aic', 415.5, NULL, NULL), ('lasso', 'aic', 409.1, NULL, NULL)
) t(model_id, metric, value, conf_low, conf_high);
SELECT 'summary' AS name, anofox_plot_summary(g, width := 560, height := 520) AS svg FROM glance g;

-- obs: per-row output of a fit, with a few influential rows -------------------
CREATE TABLE aug AS
SELECT i AS row_id, f AS fitted, r AS residual, r / 1.1 AS std_residual,
       0.02 + 0.3 * power(abs(i - 30) / 30.0, 4) AS leverage,
       power(r / 1.1, 2) / 3 * (0.02 + 0.3 * power(abs(i - 30) / 30.0, 4)) / power(1 - (0.02 + 0.3 * power(abs(i - 30) / 30.0, 4)), 2) AS cooks_d,
       3 AS n_params
FROM (SELECT i, 5 + i * 0.2 AS f,
             CASE WHEN i IN (3, 57) THEN 3.2 ELSE 0.8 * sin(i * 1.7) + 0.02 * (i - 30) END AS r
      FROM range(60) r(i));
SELECT 'obs_' || k AS name, anofox_plot_obs(a, kind := k) AS svg
FROM aug a, (VALUES ('resid_fitted'), ('qq'), ('scale_location'), ('leverage'), ('cooks')) v(k)
GROUP BY k ORDER BY k;
SELECT 'diagnostics' AS name, anofox_plot_diagnostics(a) AS svg FROM aug a;

-- anofox_plot: dispatch on the column signature ------------------------------
SELECT 'auto_terms' AS name, anofox_plot(c) AS svg FROM coefs c;
