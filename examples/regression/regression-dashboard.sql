-- A regression report as a dashboard, built from ::ROLE casts only (no
-- extension needed): `dashboard examples/regression/regression-dashboard.sql`
-- or paste into the browser builder. The same plots come ready-made from the
-- anofox_plot_* macros (see contract_fixtures.sql).

CREATE TABLE coefs AS SELECT * FROM (VALUES
  ('ols',   '(Intercept)', 1.20,  0.80, 1.60), ('ols',   'price', -0.52, -0.71, -0.33),
  ('ols',   'promo',       0.31,  0.12, 0.50), ('ols',   'season', 0.05, -0.10, 0.20),
  ('ridge', '(Intercept)', 1.10,  0.78, 1.42), ('ridge', 'price', -0.41, -0.57, -0.25),
  ('ridge', 'promo',       0.25,  0.10, 0.40), ('ridge', 'season', 0.03, -0.09, 0.15)
) t(model_id, term, estimate, conf_low, conf_high);

CREATE TABLE aug AS
SELECT i AS row_id, 5 + i * 0.2 AS fitted,
       CASE WHEN i IN (3, 57) THEN 3.2 ELSE 0.8 * sin(i * 1.7) + 0.02 * (i - 30) END AS residual,
       0.02 + 0.3 * power(abs(i - 30) / 30.0, 4) AS leverage
FROM range(60) r(i);

SELECT 'Regression report'::LABEL;

SELECT 2::COLUMNS;

-- Coefficient forest: two models dodged, zero line.
SELECT term::XAXIS, model_id::CATEGORY, estimate::SCATTER, conf_low::YMIN, conf_high::YMAX,
       1::FLIP, 0::REFLINE, 'Coefficients (95% CI)'::TITLE
FROM coefs;

-- Residuals vs fitted with a GAM trend; the two largest residuals labelled.
SELECT fitted::XAXIS, residual::SMOOTH, 'gam'::SMOOTH_METHOD, 0::REFLINE,
       row_id::LABEL, 2::LABEL_TOP, 'Residuals vs fitted'::TITLE
FROM aug;

-- Predicted vs actual with the identity line.
SELECT fitted + residual AS actual ::XAXIS, fitted::SCATTER, 1::IDENTITY,
       'Predicted vs actual'::TITLE
FROM aug;

-- Leverage on a log scale, labelled by leverage rank.
SELECT leverage::XAXIS, residual::SCATTER, 'log10'::XSCALE, 0::REFLINE,
       row_id::LABEL, 3::LABEL_TOP, leverage::RANK, 'Residuals vs leverage'::TITLE
FROM aug;

-- Estimates per model as small multiples with error bars.
SELECT term::XAXIS, estimate::BARCHART, conf_low::YMIN, conf_high::YMAX, model_id::FACET,
       'Estimates by model'::TITLE
FROM coefs;
