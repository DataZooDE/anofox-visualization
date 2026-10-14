-- Statistics showcase — every anofox_statistics plot, in modelling order.
--
-- Northwind Grocers, a fictional grocery chain, asks: what drives weekly store
-- sales, and how far can we trust the answer? The script builds small synthetic
-- datasets (deterministic: hash-based noise, no files) and then calls the
-- anofox_statistics producers live in each panel — ols_tidy_by, ols_augment_by,
-- ridge_cv_by, kaplan_meier_by, tukey_hsd_by, … — handing their long tables to
-- the anofox_plot_* contract plots. The same SQL runs
--   * in the browser demo: DuckDB-Wasm + the anofox_statistics wasm extension
--     (loaded on first use); the anofox_plot_* calls are rewritten by the
--     dashboard planner to `to_json(list(t))` and drawn by the wasm renderer;
--   * natively: `dashboard --check` / `serve` with a duckdb that has
--     anofox_statistics installed (see examples/statistics-showcase/README.md).
--
-- Sections (tabs): 1 overview · 2 model comparison · 3 check the fit ·
-- 4 predictions · 5 regularisation · 6 beyond the mean · 7 over time ·
-- 8 classification · 9 survival · 10 group comparisons.

LOAD anofox_statistics;

-- ─────────────────────────────────────────────────────────────────────────────
-- Data (setup statements — plain SQL, no files)
-- ─────────────────────────────────────────────────────────────────────────────
-- Uniform(0,1) from the md5 of a key tuple; a standard normal via Box–Muller.
CREATE OR REPLACE MACRO nw_unif(a, b, c) AS
    ((hash(md5(concat_ws('|', a, b, c))) >> 11)::DOUBLE + 0.5) / 9007199254740992.0;
CREATE OR REPLACE MACRO nw_rnorm(a, b, c) AS
    sqrt(-2.0 * ln(nw_unif(a, b, c || ':u1'))) * cos(2.0 * pi() * nw_unif(a, b, c || ':u2'));
-- One *_test_agg result as a row of the `test` schema (+ a plain-language question).
CREATE OR REPLACE MACRO nw_test_row(model_id_, test_id_, question_, r) AS {
    'model_id': model_id_::VARCHAR, 'test_id': test_id_::VARCHAR, 'question': question_::VARCHAR,
    'method': r.method::VARCHAR, 'statistic': r.statistic::DOUBLE, 'p_value': r.p_value::DOUBLE,
    'df1': r.df1::DOUBLE, 'df2': r.df2::DOUBLE, 'estimate': r.estimate::DOUBLE,
    'conf_low': r.conf_low::DOUBLE, 'conf_high': r.conf_high::DOUBLE,
    'effect_size': r.effect_size::DOUBLE, 'effect_name': r.effect_name::VARCHAR};

-- A. nw_sales — 6 stores × 104 weeks (k€):
--    sales ≈ 120 − 18·price + 14·promo + 9·season + 0.08·t + store effect + ε.
--    temp has no effect. Planted problems: three Airport weeks keyed in ×2.5
--    (outliers) and a Riverside clearance week at price 1.20 (high leverage).
CREATE OR REPLACE TABLE nw_sales AS
WITH stores(store, store_no, store_effect, base_price) AS (VALUES
    ('Harbor', 1, 6.0, 3.05), ('Market St', 2, 4.0, 3.45), ('Uptown', 3, -5.0, 3.80),
    ('Riverside', 4, -3.0, 2.75), ('Airport', 5, 2.0, 3.70), ('Old Town', 6, -4.0, 2.95)
), b AS (
    SELECT s.*, w.t, DATE '2023-01-02' + INTERVAL ((w.t - 1) * 7) DAY AS week,
           (nw_unif('sales', s.store_no * 1000 + w.t, 'promo') < 0.25)::INTEGER::DOUBLE AS promo,
           sin(2 * pi() * w.t / 52.0) AS season
    FROM stores s, range(1, 105) w(t)
), p AS (
    SELECT b.*,
           CASE WHEN store = 'Riverside' AND t = 50 THEN 1.20
                ELSE greatest(2.5, least(4.0, base_price - 0.25 * promo
                         + 0.25 * nw_rnorm('sales', store_no * 1000 + t, 'price'))) END AS price,
           10.5 - 9.5 * cos(2 * pi() * (t - 4) / 52.0) + 2.0 * nw_rnorm('sales', store_no * 1000 + t, 'temp') AS temp
    FROM b
), y AS (
    SELECT p.*, 120 - 18 * price + 14 * promo + 9 * season + 0.08 * t + store_effect
                + 4.0 * nw_rnorm('sales', store_no * 1000 + t, 'eps') AS sales_true
    FROM p
)
SELECT store, CAST(week AS DATE) AS week, t::INTEGER AS t, round(price, 2) AS price, promo,
       round(season, 4) AS season, round(temp, 1) AS temp,
       round(CASE WHEN store = 'Airport' AND t IN (31, 62, 83) THEN 2.5 * sales_true ELSE sales_true END, 2) AS sales,
       'all' AS chain
FROM y ORDER BY store, t;

-- B. nw_online — the online shop: 156 observed weeks + 13 future weeks (y NULL).
--    The price effect drifts from −10 to −25; the errors are AR(1), φ = 0.6.
CREATE OR REPLACE TABLE nw_online AS
WITH RECURSIVE eps AS (
    SELECT t::INTEGER AS t, 6.0 * nw_rnorm('ts', t, 'eps') AS e FROM range(1, 170) r(t)
), ar(t, e) AS (
    SELECT t, e FROM eps WHERE t = 1
    UNION ALL
    SELECT n.t, 0.6 * ar.e + n.e FROM ar JOIN eps n ON n.t = ar.t + 1
), x AS (
    SELECT r.t, (nw_unif('ts', r.t, 'promo') < 0.25)::INTEGER::DOUBLE AS promo,
           sin(2 * pi() * r.t / 52.0) AS season, -10.0 - 15.0 * least(1.0, (r.t - 1) / 155.0) AS beta_price
    FROM range(1, 170) r(t)
), p AS (
    SELECT x.*, round(3.2 - 0.3 * promo + 0.35 * nw_rnorm('ts', t, 'price'), 2) AS price FROM x
)
SELECT 'online' AS chan, p.t::INTEGER AS t, CAST(DATE '2023-01-02' + INTERVAL ((p.t - 1) * 7) DAY AS DATE) AS week,
       p.price, p.promo, round(p.season, 4) AS season,
       CASE WHEN p.t <= 156 THEN round(180 + p.beta_price * p.price + 18 * p.promo + 12 * p.season
                                       + 0.35 * p.t + a.e, 2) END AS y,
       CASE WHEN p.t <= 130 THEN 'train' WHEN p.t <= 156 THEN 'test' ELSE 'future' END AS split
FROM p JOIN ar a USING (t) ORDER BY t;
CREATE OR REPLACE TABLE nw_online_obs AS SELECT * FROM nw_online WHERE y IS NOT NULL ORDER BY t;
-- the online model's training residuals (ACF/PACF, Durbin–Watson, Ljung–Box)
CREATE OR REPLACE TABLE nw_online_resid AS
SELECT chan, t, price, promo, season, round(residual, 4) AS resid
FROM ols_augment_by('nw_online', chan, y, [price, promo, season, t], split := split, order_by := t)
WHERE is_training ORDER BY t;

-- C. nw_mmm — marketing mix: 200 weeks, 8 collinear spend channels (k€);
--    only tv, search, social and email drive revenue.
CREATE OR REPLACE TABLE nw_mmm AS
WITH w AS (
    SELECT t, 0.6 * sin(2 * pi() * t / 52.0) + 0.5 * nw_rnorm('mmm', t, 'budget') AS f FROM range(1, 201) r(t)
), s AS (
    SELECT t,
           round(50 * exp(0.5 * f + 0.25 * nw_rnorm('mmm', t, 'tv')), 2) AS tv,
           round(15 * exp(0.5 * f + 0.25 * nw_rnorm('mmm', t, 'radio')), 2) AS radio,
           round(20 * exp(0.5 * f + 0.25 * nw_rnorm('mmm', t, 'social')), 2) AS social,
           round(30 * exp(0.5 * f + 0.25 * nw_rnorm('mmm', t, 'search')), 2) AS search,
           round( 5 * exp(0.5 * f + 0.25 * nw_rnorm('mmm', t, 'email')), 2) AS email,
           round(12 * exp(0.5 * f + 0.25 * nw_rnorm('mmm', t, 'display')), 2) AS display,
           round( 8 * exp(0.5 * f + 0.25 * nw_rnorm('mmm', t, 'print')), 2) AS print,
           round(10 * exp(0.5 * f + 0.25 * nw_rnorm('mmm', t, 'ooh')), 2) AS ooh
    FROM w
)
SELECT 'Northwind' AS brand, t::INTEGER AS t, tv, radio, social, search, email, display, print, ooh,
       round(400 + 2.5 * tv + 3.0 * search + 2.0 * social + 6.0 * email + 30 * nw_rnorm('mmm', t, 'eps'), 2) AS revenue
FROM s ORDER BY t;

-- D. nw_churn — 1500 loyalty-app subscribers: churned within 3 months (a logit in
--    tenure, tickets, fee, plan) and lifetime since signup (censored at the
--    study end). p_logit = a logistic model fitted here; p_rule = a crude
--    points score.
CREATE OR REPLACE TABLE nw_churn_base AS
WITH c AS (
    SELECT i AS customer_id,
           CASE WHEN nw_unif('churn', i, 'plan') < 0.5 THEN 'Basic'
                WHEN nw_unif('churn', i, 'plan') < 0.8 THEN 'Plus' ELSE 'Pro' END AS plan,
           1 + floor(59 * nw_unif('churn', i, 'tenure'))::INTEGER AS tenure_months
    FROM range(1, 1501) r(i)
), f AS (
    SELECT c.*,
           floor(-ln(nw_unif('churn', customer_id, 'tickets'))
                 * CASE plan WHEN 'Basic' THEN 1.6 WHEN 'Plus' THEN 1.2 ELSE 0.9 END
                 * (1.4 - 0.012 * tenure_months))::INTEGER AS tickets,
           round(CASE plan WHEN 'Basic' THEN 9.99 WHEN 'Plus' THEN 19.99 ELSE 39.99 END
                 + 8 * nw_unif('churn', customer_id, 'addons'), 2) AS monthly_fee
    FROM c
), l AS (
    SELECT f.*,
           -0.4 - 0.045 * tenure_months + 0.45 * tickets + 0.02 * monthly_fee
               - CASE plan WHEN 'Plus' THEN 0.3 WHEN 'Pro' THEN 1.0 ELSE 0.0 END AS logit,
           -ln(nw_unif('churn', customer_id, 'life')) * CASE plan WHEN 'Basic' THEN 16 WHEN 'Plus' THEN 26 ELSE 44 END AS lifetime,
           3 + 45 * nw_unif('churn', customer_id, 'followup') AS followup
    FROM f
)
SELECT customer_id::INTEGER AS customer_id, 'all' AS segment, plan, tenure_months::DOUBLE AS tenure_months,
       tickets::DOUBLE AS tickets, monthly_fee,
       (plan = 'Plus')::INTEGER::DOUBLE AS is_plus, (plan = 'Pro')::INTEGER::DOUBLE AS is_pro,
       nw_unif('churn', customer_id, 'churn') < 1 / (1 + exp(-logit)) AS churned,
       (nw_unif('churn', customer_id, 'churn') < 1 / (1 + exp(-logit)))::INTEGER::DOUBLE AS churn_flag,
       greatest(1, ceil(least(lifetime, followup)))::INTEGER AS lifetime_months,
       lifetime <= followup AS event
FROM l ORDER BY customer_id;
CREATE OR REPLACE TABLE nw_churn AS
SELECT customer_id, segment, plan, tenure_months, tickets, monthly_fee, is_plus, is_pro,
       churned, churn_flag, lifetime_months, event, round(yhat, 4) AS p_logit,
       least(0.95, 0.08 + 0.30 * (tickets >= 3)::INTEGER + 0.25 * (tenure_months < 12)::INTEGER
                 + 0.10 * (plan = 'Basic')::INTEGER)::DOUBLE AS p_rule
FROM logistic_fit_predict_by('nw_churn_base', segment, churn_flag,
                             [tenure_months, tickets, monthly_fee, is_plus, is_pro])
ORDER BY customer_id;
-- both scores stacked long: one curve per model
CREATE OR REPLACE TABLE nw_churn_scores AS
SELECT 'logistic' AS model, customer_id, churned, p_logit AS score FROM nw_churn
UNION ALL
SELECT 'rule-based' AS model, customer_id, churned, p_rule AS score FROM nw_churn;

-- E. nw_orders — 1200 web orders: right-skewed basket values whose spread grows
--    with the discount, so the discount effect rises across quantiles.
CREATE OR REPLACE TABLE nw_orders AS
WITH o AS (
    SELECT i AS order_id, 'web' AS shop, round(30 * nw_unif('orders', i, 'discount'), 1) AS discount_pct,
           (nw_unif('orders', i, 'member') < 0.4)::INTEGER::DOUBLE AS is_member
    FROM range(1, 1201) r(i)
)
SELECT order_id::INTEGER AS order_id, shop, discount_pct, is_member,
       round(30 + 0.5 * discount_pct + 8 * is_member
             + (6 + 0.6 * discount_pct) * exp(0.7 * nw_rnorm('orders', order_id, 'eps')), 2) AS basket_value
FROM o ORDER BY order_id;

-- F. nw_ab — checkout experiment: 4 page variants, ~150 sessions each.
CREATE OR REPLACE TABLE nw_ab AS
WITH s AS (
    SELECT i AS session_id,
           CASE floor(4 * nw_unif('ab', i, 'variant'))::INTEGER
                WHEN 0 THEN 'A control' WHEN 1 THEN 'B hero' WHEN 2 THEN 'C social proof' ELSE 'D price anchor' END AS variant
    FROM range(1, 601) r(i)
)
SELECT 'checkout' AS experiment, session_id::INTEGER AS session_id, variant,
       round(greatest(5, CASE variant WHEN 'A control' THEN 42.0 WHEN 'B hero' THEN 42.5
                                      WHEN 'C social proof' THEN 46.0 ELSE 48.5 END
                         + 10 * nw_rnorm('ab', session_id, 'value')), 2) AS order_value,
       (nw_unif('ab', session_id, 'conv') < CASE variant WHEN 'A control' THEN 0.10 WHEN 'B hero' THEN 0.105
                                                         WHEN 'C social proof' THEN 0.13 ELSE 0.145 END)::INTEGER AS converted
FROM s ORDER BY session_id;

-- Hypothesis tests (the *_test_agg aggregates, one `test` row each) — a view,
-- so each tab computes only the tests it shows.
CREATE OR REPLACE VIEW nw_tests AS
SELECT unnest(nw_test_row('online', 'durbin_watson', 'Online residuals autocorrelated at lag 1?',
                          durbin_watson_agg(resid, [price, promo, season, t::DOUBLE] ORDER BY t))) FROM nw_online_resid
UNION ALL
SELECT unnest(nw_test_row('online', 'ljung_box', 'Online residuals autocorrelated at lags 1–10?',
                          ljung_box_agg(resid, 10 ORDER BY t))) FROM nw_online_resid
UNION ALL
SELECT unnest(nw_test_row('sales', 'shapiro_wilk', 'Store-sales residuals normal?', shapiro_wilk_agg(residual)))
FROM ols_augment_by('nw_sales', chain, sales, [price, promo, season, temp, t])
UNION ALL
SELECT unnest(nw_test_row('sales', 'breusch_pagan', 'Store-sales residual variance constant?',
                          breusch_pagan_agg(residual, [price, promo, season, temp, t::DOUBLE])))
FROM ols_augment_by('nw_sales', chain, sales, [price, promo, season, temp, t])
UNION ALL
SELECT unnest(nw_test_row('churn', 'log_rank', 'Do the three plans churn alike?',
                          log_rank_test_agg(lifetime_months, event, plan))) FROM nw_churn
UNION ALL
SELECT unnest(nw_test_row('checkout', 'anova', 'Do the four pages differ in order value?',
                          one_way_anova_agg(order_value, variant))) FROM nw_ab
UNION ALL
SELECT unnest(nw_test_row('checkout', 'kruskal_wallis', 'Same, without assuming normality?',
                          kruskal_wallis_agg(order_value, variant))) FROM nw_ab
UNION ALL
SELECT unnest(nw_test_row('checkout', 'chisq', 'Does conversion depend on the page?',
                          chisq_test_agg((ascii(variant) - 65)::INTEGER, converted))) FROM nw_ab;

-- ─────────────────────────────────────────────────────────────────────────────
-- Dashboard
-- ─────────────────────────────────────────────────────────────────────────────
SELECT 'Northwind Grocers — what drives weekly sales? A regression workflow, computed live'::LABEL;

SELECT 12::COL;
SELECT 'Every chart below is computed **in your browser**: DuckDB-Wasm builds six synthetic Northwind datasets with plain SQL, the **anofox_statistics** extension fits the models (`ols_tidy_by`, `ols_augment_by`, `ridge_cv_by`, `kaplan_meier_by`, `tukey_hsd_by`, …) and each panel hands the producer''s long table to a **contract plot**, e.g. `SELECT anofox_plot_terms(t) FROM ols_tidy_by(…) t`. The tabs follow the modelling workflow: **state the question → compare models → check the fit → predict → regularise → look beyond the mean → time → classification → survival → group comparisons.** Open the SQL editor to see (and change) every query.'::MARKDOWN, 'About this dashboard'::TITLE;

-- ── 1 · Overview ─────────────────────────────────────────────────────────────
SELECT '1 Overview'::TAB;

SELECT 12::COL;
SELECT '**Question:** how much do **price**, **promotions** and the **season** move weekly sales across Northwind''s six stores (two years, 624 store-weeks)? We fit the same linear model with four estimators — **OLS**, robust **Huber**, **ridge** (shrunk) and the **median** (quantile) regression. **What to look for:** the coefficient forest shows each effect with its 95% interval; intervals that cross the dashed zero line are not distinguishable from noise (here: **temperature**). When the four estimators agree, the answer does not hinge on the method.'::MARKDOWN, 'The question'::TITLE;

SELECT 'Store-sales model at a glance'::GROUP;
SELECT round(max(value), 3)::METRIC, ('Best R² — ' || arg_max(model_id, value))::LABEL
FROM (SELECT * FROM ols_glance_by('nw_sales', chain, sales, [price, promo, season, temp, t])
      UNION ALL BY NAME SELECT * FROM huber_glance_by('nw_sales', chain, sales, [price, promo, season, temp, t])
      UNION ALL BY NAME SELECT * FROM ridge_glance_by('nw_sales', chain, sales, [price, promo, season, temp, t], options := {'alpha': 5.0})
      UNION ALL BY NAME SELECT * FROM theil_sen_glance_by('nw_sales', chain, sales, [price, promo, season, temp, t]))
WHERE metric = 'r_squared';
SELECT round(value, 1)::METRIC, 'AIC — OLS'::LABEL
FROM ols_glance_by('nw_sales', chain, sales, [price, promo, season, temp, t]) WHERE metric = 'aic';
SELECT value::COMPACT, 'store-weeks (n)'::LABEL
FROM ols_glance_by('nw_sales', chain, sales, [price, promo, season, temp, t]) WHERE metric = 'n_observations';
SELECT round(estimate, 1)::METRIC, 'k€ per +1 € price (OLS)'::LABEL
FROM ols_tidy_by('nw_sales', chain, sales, [price, promo, season, temp, t], names := ['price', 'promo', 'season', 'temp', 'trend'])
WHERE term = 'price';
SELECT 1::ENDGROUP;

SELECT 12::COL;
SELECT 380::HEIGHT;
SELECT anofox_plot_terms(t), 'Coefficient forest — four estimators agree: price −17.6, promo +14.3, temperature is noise'::TITLE
FROM (SELECT * FROM ols_tidy_by('nw_sales', chain, sales, [price, promo, season, temp, t], names := ['price', 'promo', 'season', 'temp', 'trend'])
      UNION ALL BY NAME
      SELECT * FROM huber_tidy_by('nw_sales', chain, sales, [price, promo, season, temp, t], names := ['price', 'promo', 'season', 'temp', 'trend'])
      UNION ALL BY NAME
      SELECT * FROM ridge_tidy_by('nw_sales', chain, sales, [price, promo, season, temp, t], options := {'alpha': 5.0}, names := ['price', 'promo', 'season', 'temp', 'trend'])
      UNION ALL BY NAME
      SELECT * FROM quantile_tidy_by('nw_sales', chain, sales, [price, promo, season, temp, t], options := {'tau': 0.5}, names := ['price', 'promo', 'season', 'temp', 'trend'])) t
WHERE term <> '(Intercept)';

-- ── 2 · Model comparison ─────────────────────────────────────────────────────
SELECT '2 Compare'::TAB;

SELECT 12::COL;
SELECT 'Five estimators of the same store-sales model, compared on their **glance** statistics (`<model>_glance_by`, the `summary` schema): one panel per metric, one dot per model. **What to look for:** higher **R²** and lower **AIC / BIC / residual standard error** are better — but the differences here are tiny. OLS, Huber, ridge and Theil–Sen fit equally well; the elastic net (a strong penalty) gives up fit for sparsity. Information criteria are only reported by likelihood-based fits, so some models have no AIC dot.'::MARKDOWN, 'What to look for'::TITLE;

SELECT 12::COL;
SELECT 420::HEIGHT;
SELECT anofox_plot_summary(s, metrics := ['r_squared', 'adj_r_squared', 'residual_std_error', 'aic', 'bic']),
       'Fit statistics by model — near-identical fits; the elastic net trades fit for sparsity'::TITLE
FROM (SELECT * FROM ols_glance_by('nw_sales', chain, sales, [price, promo, season, temp, t])
      UNION ALL BY NAME SELECT * FROM huber_glance_by('nw_sales', chain, sales, [price, promo, season, temp, t])
      UNION ALL BY NAME SELECT * FROM ridge_glance_by('nw_sales', chain, sales, [price, promo, season, temp, t], options := {'alpha': 5.0})
      UNION ALL BY NAME SELECT * FROM theil_sen_glance_by('nw_sales', chain, sales, [price, promo, season, temp, t])
      UNION ALL BY NAME SELECT * FROM elasticnet_glance_by('nw_sales', chain, sales, [price, promo, season, temp, t], options := {'alpha': 0.5})) s;

SELECT 12::COL;
SELECT 'Glance statistics'::TITLE, model_id AS "Model" ::TABLE,
       round(max(value) FILTER (WHERE metric = 'r_squared'), 4) AS "R²" ::PLAIN,
       round(max(value) FILTER (WHERE metric = 'adj_r_squared'), 4) AS "adj. R²" ::PLAIN,
       round(max(value) FILTER (WHERE metric = 'residual_std_error'), 2) AS "resid. SE" ::PLAIN,
       round(max(value) FILTER (WHERE metric = 'aic'), 1) AS "AIC" ::PLAIN,
       round(max(value) FILTER (WHERE metric = 'bic'), 1) AS "BIC" ::PLAIN
FROM (SELECT * FROM ols_glance_by('nw_sales', chain, sales, [price, promo, season, temp, t])
      UNION ALL BY NAME SELECT * FROM huber_glance_by('nw_sales', chain, sales, [price, promo, season, temp, t])
      UNION ALL BY NAME SELECT * FROM ridge_glance_by('nw_sales', chain, sales, [price, promo, season, temp, t], options := {'alpha': 5.0})
      UNION ALL BY NAME SELECT * FROM theil_sen_glance_by('nw_sales', chain, sales, [price, promo, season, temp, t])
      UNION ALL BY NAME SELECT * FROM elasticnet_glance_by('nw_sales', chain, sales, [price, promo, season, temp, t], options := {'alpha': 0.5}))
GROUP BY model_id ORDER BY 3 DESC;

-- ── 3 · Check the fit ────────────────────────────────────────────────────────
SELECT '3 Fit checks'::TAB;

SELECT 12::COL;
SELECT 'Residual diagnostics from `ols_augment_by` (the `obs` schema: fitted, residual, leverage, Cook''s distance per row). **What to look for:** in **residuals vs fitted**, a flat smoother around zero (no curvature left); on the **normal Q-Q**, points on the line; in **scale-location**, constant spread; and no point beyond the **Cook''s distance** contours. Here three rows stand out everywhere — the **Airport weeks 31, 62 and 83** that were keyed in at 2.5× (labelled by row id) — while the **Riverside clearance week** at €1.20 has the highest leverage but follows the line, so it barely moves the fit.'::MARKDOWN, 'What to look for'::TITLE;

SELECT 12::COL;
SELECT 600::HEIGHT;
SELECT anofox_plot_diagnostics(o), 'plot.lm 2×2 — three keyed-in Airport weeks dominate the residuals'::TITLE
FROM ols_augment_by('nw_sales', chain, sales, [price, promo, season, temp, t]) o;

SELECT 12::COL;
SELECT anofox_plot_obs(o, kind := 'cooks'), 'Cook''s distance — the 4/n line flags the three outliers'::TITLE
FROM ols_augment_by('nw_sales', chain, sales, [price, promo, season, temp, t]) o;

SELECT 12::COL;
SELECT '**Partial-residual plots** (`ols_partial_residuals_by`) look at one term at a time with the others held fixed. In the **component + residual** plot the straight line is the term''s fitted effect and the smoother shows what the data say — a bend means the term needs a transformation. The **added-variable** plot regresses what is left of the outcome on what is left of the term; its slope *is* the coefficient, and points far from the line are the observations that pull it.'::MARKDOWN, 'Partial residuals'::TITLE;

SELECT 12::COL;
SELECT 460::HEIGHT;
SELECT anofox_plot_partial(p, kind := 'component'), 'Component + residual — every effect is linear (the smoother follows the line)'::TITLE
FROM ols_partial_residuals_by('nw_sales', chain, sales, [price, promo, season, temp, t], names := ['price', 'promo', 'season', 'temp', 'trend']) p;

SELECT 12::COL;
SELECT 460::HEIGHT;
SELECT anofox_plot_partial(p, kind := 'added_variable'), 'Added-variable plots — the slope of each panel is that term''s coefficient'::TITLE
FROM ols_partial_residuals_by('nw_sales', chain, sales, [price, promo, season, temp, t], names := ['price', 'promo', 'season', 'temp', 'trend']) p;

SELECT 12::COL;
SELECT 'Residual tests — the outliers break normality, the variance is fine'::TITLE,
       question AS "Question" ::TABLE, method AS "Test", round(statistic, 3) AS "Statistic" ::PLAIN,
       printf('%.2g', p_value) AS "p-value" ::PLAIN,
       CASE WHEN p_value < 0.05 THEN 'reject' ELSE 'ok' END AS "at 5%" ::BADGE
FROM nw_tests WHERE model_id = 'sales';

-- ── 4 · Predictions ──────────────────────────────────────────────────────────
SELECT '4 Predict'::TAB;

SELECT 12::COL;
SELECT 'The online shop''s weekly sales, modelled on price, promo, season and trend (`<model>_prediction_by`, the `prediction` schema). The model is fitted on the **train** weeks only; the last 26 observed weeks are held out as **test**, and the 13 **future** weeks have no outcome yet. **What to look for:** the observed points of the test weeks should fall inside the 90% band — they do, so the model generalises. The overlay compares three estimators on the hold-out; the **GLM effect plots** show one feature''s effect on the response scale with the others at their mean.'::MARKDOWN, 'What to look for'::TITLE;

SELECT 12::COL;
SELECT 400::HEIGHT;
SELECT anofox_plot_prediction(p), 'Online shop — fit, 26-week hold-out and 13-week forecast (90% band)'::TITLE
FROM ols_prediction_by('nw_online', chan, week, y, [price, promo, season, t], level := 0.9, split := split) p;

SELECT 12::COL;
SELECT 360::HEIGHT;
SELECT anofox_plot_prediction(p), 'Three estimators on the hold-out and forecast weeks — OLS, Huber and the median line'::TITLE
FROM (SELECT * FROM ols_prediction_by('nw_online', chan, week, y, [price, promo, season, t], level := 0.9, split := split)
      UNION ALL BY NAME
      SELECT * FROM huber_prediction_by('nw_online', chan, week, y, [price, promo, season, t], level := 0.9, split := split)
      UNION ALL BY NAME
      SELECT * FROM quantile_prediction_by('nw_online', chan, week, y, [price, promo, season, t], split := split)) p
WHERE x > (SELECT week FROM nw_online WHERE t = 120);

SELECT 4::COL;
SELECT anofox_plot_prediction(p), 'Churn probability falls with tenure (logistic)'::TITLE
FROM logistic_effects_by('nw_churn', segment, churn_flag, [tenure_months, tickets, monthly_fee, is_plus, is_pro],
                         focal := 1, n_points := 30) p;
SELECT 4::COL;
SELECT anofox_plot_prediction(p), 'Support tickets fall with tenure (Poisson)'::TITLE
FROM poisson_effects_by('nw_churn', segment, tickets, [tenure_months, monthly_fee], focal := 1, n_points := 30) p;
SELECT 4::COL;
SELECT anofox_plot_prediction(p), 'Store sales by price, others at their mean (OLS)'::TITLE
FROM ols_effects_by('nw_sales', chain, sales, [price, promo, season, temp, t], focal := 1, n_points := 30) p;

-- ── 5 · Regularisation & selection ───────────────────────────────────────────
SELECT '5 Regularise'::TAB;

SELECT 12::COL;
SELECT 'Northwind''s marketing mix: weekly revenue on **eight collinear spend channels** (they share one budget), of which only **tv, search, social and email** matter. **Ridge** shrinks all coefficients towards zero as the penalty λ grows; the **elastic net** and **LARS** set them to zero one by one — the order they enter is a variable ranking. **Cross-validation** picks λ: **λ.min** has the lowest error, **λ.1se** is the simplest model within one standard error of it. **What to look for:** the four real channels stay non-zero longest; the noise channels hover around zero.'::MARKDOWN, 'What to look for'::TITLE;

SELECT 'Ridge'::SUBTAB;
SELECT 12::COL;
SELECT 560::HEIGHT;
SELECT anofox_plot_terms(t), 'Ridge path — every channel shrinks smoothly towards zero as λ grows'::TITLE
FROM ridge_path_by('nw_mmm', brand, revenue, [tv, radio, social, search, email, display, print, ooh],
                   options := {'n_lambda': 40, 'standardize': true},
                   names := ['tv', 'radio', 'social', 'search', 'email', 'display', 'print', 'ooh']) t
WHERE term <> '(Intercept)';
SELECT 12::COL;
SELECT anofox_plot_curve(c), 'Ridge — 5-fold CV error by λ (λ.min and λ.1se marked)'::TITLE
FROM ridge_cv_by('nw_mmm', brand, revenue, [tv, radio, social, search, email, display, print, ooh],
                 options := {'n_lambda': 40, 'folds': 5, 'standardize': true}) c;

SELECT 'Elastic net'::SUBTAB;
SELECT 12::COL;
SELECT 560::HEIGHT;
SELECT anofox_plot_terms(t), 'Elastic-net path (l1 ratio 0.8) — noise channels drop to exactly zero first'::TITLE
FROM elasticnet_path_by('nw_mmm', brand, revenue, [tv, radio, social, search, email, display, print, ooh],
                        options := {'n_lambda': 40, 'l1_ratio': 0.8, 'standardize': true},
                        names := ['tv', 'radio', 'social', 'search', 'email', 'display', 'print', 'ooh']) t
WHERE term <> '(Intercept)';
SELECT 12::COL;
SELECT anofox_plot_curve(c), 'Elastic net — CV error by λ: λ.1se keeps four channels'::TITLE
FROM elasticnet_cv_by('nw_mmm', brand, revenue, [tv, radio, social, search, email, display, print, ooh],
                      options := {'n_lambda': 40, 'folds': 5, 'l1_ratio': 0.8, 'standardize': true}) c;

SELECT 'LARS'::SUBTAB;
SELECT 12::COL;
SELECT 560::HEIGHT;
SELECT anofox_plot_terms(t), 'LARS / lasso path — channels enter in order: tv, search, social, email, then noise'::TITLE
FROM lars_path_by('nw_mmm', brand, revenue, [tv, radio, social, search, email, display, print, ooh],
                  names := ['tv', 'radio', 'social', 'search', 'email', 'display', 'print', 'ooh']) t
WHERE term <> '(Intercept)';

-- ── 6 · Beyond the mean ──────────────────────────────────────────────────────
SELECT '6 Quantiles'::TAB;

SELECT 12::COL;
SELECT 'Least squares describes the **average** basket. A **quantile regression** fits each quantile τ of the basket value separately; `quantile_process_by` runs it over a grid of τ (the `terms` schema with `index_name = ''tau''`). **What to look for:** a flat line means the effect is the same for small and large baskets; here the **discount** effect grows from about 0.7 € per point at τ = 0.05 to 2.4 € at τ = 0.95 — discounts mostly enlarge the already-large stock-up baskets — while **membership** shifts small baskets but not the largest ones.'::MARKDOWN, 'What to look for'::TITLE;

SELECT 12::COL;
SELECT 400::HEIGHT;
SELECT anofox_plot_terms(t), 'Quantile process — the discount effect triples from the 5% to the 95% quantile'::TITLE
FROM quantile_process_by('nw_orders', shop, basket_value, [discount_pct, is_member],
                         options := {'taus': [0.05, 0.1, 0.15, 0.2, 0.25, 0.3, 0.35, 0.4, 0.45, 0.5,
                                              0.55, 0.6, 0.65, 0.7, 0.75, 0.8, 0.85, 0.9, 0.95]},
                         names := ['discount_pct', 'is_member']) t
WHERE term <> '(Intercept)';

SELECT 12::COL;
SELECT discount_pct::XAXIS, basket_value::SCATTER,
       'The data — basket value by discount: the spread fans out as the discount grows'::TITLE
FROM nw_orders;

-- ── 7 · Over time ────────────────────────────────────────────────────────────
SELECT '7 Over time'::TAB;

SELECT 12::COL;
SELECT 'Two time questions about the online shop. **Is the price effect stable?** `ols_rolling_tidy_by` refits the model on a sliding 26-week window and returns one coefficient (± interval) per window end — the price elasticity steepens from about −9 to −29. **Are the residuals independent?** The **ACF / PACF** of the training residuals (`acf_by`, `pacf_by`) should stay inside the dashed significance bounds; a lag-1 spike that decays in the ACF and cuts off in the PACF is the signature of **AR(1)** errors, confirmed by **Durbin–Watson** and **Ljung–Box**. Then the OLS standard errors are too small — use HAC errors or model the autocorrelation.'::MARKDOWN, 'What to look for'::TITLE;

SELECT 12::COL;
SELECT 380::HEIGHT;
SELECT anofox_plot_terms(t), 'Rolling 26-week coefficients — the price elasticity steepens over three years'::TITLE
FROM ols_rolling_tidy_by('nw_online_obs', chan, t, y, [price, promo], window_size := 26, names := ['price', 'promo']) t
WHERE term <> '(Intercept)';

SELECT 6::COL;
SELECT CASE curve_type WHEN 'acf' THEN 'ACF: decays from lag 1' ELSE 'PACF: one spike at lag 1 (AR(1))' END AS what,
       anofox_plot_curve(c), 'Training residuals'::TITLE
FROM (SELECT * FROM acf_by('nw_online_resid', chan, resid, t, max_lag := 20)
      UNION ALL BY NAME
      SELECT * FROM pacf_by('nw_online_resid', chan, resid, t, max_lag := 20)) c
GROUP BY curve_type ORDER BY curve_type;

SELECT 12::COL;
SELECT 'Autocorrelation tests — both reject independence'::TITLE,
       question AS "Question" ::TABLE, method AS "Test", round(statistic, 3) AS "Statistic" ::PLAIN,
       printf('%.2g', p_value) AS "p-value" ::PLAIN,
       CASE WHEN p_value < 0.05 THEN 'reject' ELSE 'ok' END AS "at 5%" ::BADGE
FROM nw_tests WHERE model_id = 'online';

SELECT 12::COL;
SELECT t::XAXIS, resid::LINECHART, 0::REFLINE, 'Training residuals over time — runs above and below zero'::TITLE
FROM nw_online_resid ORDER BY t;

-- ── 8 · Classification ───────────────────────────────────────────────────────
SELECT '8 Classify'::TAB;

SELECT 12::COL;
SELECT 'Which subscribers will churn? Two scores for the same 1,500 customers: a **logistic regression** on tenure, tickets, fee and plan, and a **rule-based** points score. `roc_curve_by`, `pr_curve_by` and `calibration_curve_by` return the `curve` schema, one curve per model. **What to look for:** the **ROC** curve further from the diagonal (higher AUC) ranks churners better; the **precision-recall** curve shows the trade-off at the 32% churn base rate; in the **calibration** plot, a calibrated model''s predicted probabilities lie on the diagonal — the rule score is both less sharp and miscalibrated.'::MARKDOWN, 'What to look for'::TITLE;

SELECT 'Discrimination and calibration'::GROUP;
SELECT round(auc_agg(churned, score), 3)::METRIC, 'AUC — logistic'::LABEL FROM nw_churn_scores WHERE model = 'logistic';
SELECT round(auc_agg(churned, score), 3)::METRIC, 'AUC — rule-based'::LABEL FROM nw_churn_scores WHERE model = 'rule-based';
SELECT round(brier_agg(churned, score), 3)::METRIC, 'Brier score — logistic (lower is better)'::LABEL FROM nw_churn_scores WHERE model = 'logistic';
SELECT round(brier_agg(churned, score), 3)::METRIC, 'Brier score — rule-based'::LABEL FROM nw_churn_scores WHERE model = 'rule-based';
SELECT 1::ENDGROUP;

SELECT 4::COL;
SELECT CASE curve_type WHEN 'roc' THEN 'ROC: who is ranked first' WHEN 'pr' THEN 'precision vs recall'
                       ELSE 'calibration: are the probabilities right?' END AS what,
       anofox_plot_curve(c), 'Churn scores'::TITLE
FROM (SELECT * FROM roc_curve_by('nw_churn_scores', model, churned, score)
      UNION ALL BY NAME
      SELECT * FROM pr_curve_by('nw_churn_scores', model, churned, score)
      UNION ALL BY NAME
      SELECT * FROM calibration_curve_by('nw_churn_scores', model, churned, score, n_bins := 10)) c
GROUP BY curve_type
ORDER BY CASE curve_type WHEN 'roc' THEN 1 WHEN 'pr' THEN 2 ELSE 3 END;

-- ── 9 · Survival ─────────────────────────────────────────────────────────────
SELECT '9 Survival'::TAB;

SELECT 12::COL;
SELECT 'How long do subscribers stay? `kaplan_meier_by` estimates the survival curve per plan from lifetimes that are partly **censored** (customers still active at the study end, marked +). **What to look for:** the curve that stays higher keeps customers longer; where the step curve crosses 50% is the **median lifetime**. Pro members stay about three times as long as Basic ones, and the **log-rank** test confirms the curves differ.'::MARKDOWN, 'What to look for'::TITLE;

SELECT 8::COL;
SELECT 400::HEIGHT;
SELECT anofox_plot_curve(k), 'Kaplan–Meier survival by plan (95% band, + = censored)'::TITLE
FROM kaplan_meier_by('nw_churn', plan, lifetime_months, event) k;

SELECT 4::COL;
SELECT 'Median lifetime by plan'::TITLE, series AS "Plan" ::TABLE,
       min(x) FILTER (WHERE y <= 0.5) AS "Median (months)" ::PLAIN, max(n_risk) AS "Customers" ::PLAIN
FROM kaplan_meier_by('nw_churn', plan, lifetime_months, event)
GROUP BY series ORDER BY 3;

SELECT 12::COL;
SELECT 'Log-rank test'::TITLE, question AS "Question" ::TABLE, method AS "Test", round(statistic, 1) AS "χ²" ::PLAIN,
       df1 AS "df" ::PLAIN, printf('%.2g', p_value) AS "p-value" ::PLAIN,
       CASE WHEN p_value < 0.05 THEN 'curves differ' ELSE 'no difference' END AS "at 5%" ::BADGE
FROM nw_tests WHERE model_id = 'churn';

-- ── 10 · Group comparisons ───────────────────────────────────────────────────
SELECT '10 Groups'::TAB;

SELECT 12::COL;
SELECT 'A checkout experiment with **four page variants** (~150 sessions each). The **ANOVA** says the mean order values differ; **post-hoc tests** say *which* pairs do. `tukey_hsd_by` and `pairwise_t_test_by` (Holm-adjusted) return the `test` schema — one row per pair with the difference, its interval and the adjusted p-value. **What to look for:** in the comparison forest, intervals that cross zero are not significant; the box plot marks each pair with its adjusted p-value — **C** and **D** beat **A** and **B**, but A vs B and C vs D are indistinguishable.'::MARKDOWN, 'What to look for'::TITLE;

SELECT 12::COL;
SELECT 460::HEIGHT;
SELECT anofox_plot_compare(r, value := 'order_value', factor := 'variant',
                           tests := (SELECT list(t) FROM tukey_hsd_by('nw_ab', experiment, order_value, variant) t)),
       'Order value by checkout page — Tukey HSD brackets'::TITLE
FROM nw_ab r;

SELECT 6::COL;
SELECT method, anofox_plot_tests(t), 'Pairwise differences'::TITLE
FROM (SELECT * FROM tukey_hsd_by('nw_ab', experiment, order_value, variant)
      UNION ALL BY NAME
      SELECT * FROM pairwise_t_test_by('nw_ab', experiment, order_value, variant, options := {'p_adjust': 'holm'})) t
GROUP BY method ORDER BY method;

SELECT 6::COL;
SELECT 'Omnibus tests'::TITLE, question AS "Question" ::TABLE, method AS "Test", round(statistic, 2) AS "Statistic" ::PLAIN,
       printf('%.2g', p_value) AS "p-value" ::PLAIN,
       CASE WHEN p_value < 0.05 THEN 'differ' ELSE 'same' END AS "at 5%" ::BADGE
FROM nw_tests WHERE model_id = 'checkout';
SELECT 6::COL;
SELECT variant::XAXIS, round(100 * avg(converted), 1)::BARCHART, ''::DATALABELS,
       'Conversion rate % — differences are not significant (χ²)'::TITLE
FROM nw_ab GROUP BY variant ORDER BY 2 DESC;
