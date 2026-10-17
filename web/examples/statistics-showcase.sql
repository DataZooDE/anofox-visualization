-- @title What sets the price of a home?
-- @group Sales & customers
-- @tags  statistics, regression, pricing
-- @description What drives home prices in one city, from the first look at the data to a checked model.
--
-- Statistics showcase — every anofox_statistics plot, in modelling order.
--
-- A property market in a made-up mid-sized city asks: what sets the price of a
-- home, and how far can we trust the answer? The script builds small synthetic
-- datasets (deterministic: hash-based noise, no files) — 900 home sales, 320
-- flats with their amenities and 1,400 listings followed until they sell — and
-- then calls the anofox_statistics producers live in each panel — ols_tidy_by,
-- ols_augment_by, ridge_cv_by, kaplan_meier_by, tukey_hsd_by, … — handing their
-- long tables to the anofox_plot_* contract plots. The same SQL runs
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
CREATE OR REPLACE MACRO pm_unif(a, b, c) AS
    ((hash(md5(concat_ws('|', a, b, c))) >> 11)::DOUBLE + 0.5) / 9007199254740992.0;
CREATE OR REPLACE MACRO pm_rnorm(a, b, c) AS
    sqrt(-2.0 * ln(pm_unif(a, b, c || ':u1'))) * cos(2.0 * pi() * pm_unif(a, b, c || ':u2'));
-- One *_test_agg result as a row of the `test` schema (+ a plain-language question).
CREATE OR REPLACE MACRO pm_test_row(model_id_, test_id_, question_, r) AS {
    'model_id': model_id_::VARCHAR, 'test_id': test_id_::VARCHAR, 'question': question_::VARCHAR,
    'method': r.method::VARCHAR, 'statistic': r.statistic::DOUBLE, 'p_value': r.p_value::DOUBLE,
    'df1': r.df1::DOUBLE, 'df2': r.df2::DOUBLE, 'estimate': r.estimate::DOUBLE,
    'conf_low': r.conf_low::DOUBLE, 'conf_high': r.conf_high::DOUBLE,
    'effect_size': r.effect_size::DOUBLE, 'effect_name': r.effect_name::VARCHAR};

-- A. pm_sales — 900 home sales in a mid-sized city, Jan 2023 – Dec 2025, one
--    per sale date (price in k€). The planted model:
--      price ≈ 30 + β_area(t)·living_area + 6·rooms − 0.9·age + 14·condition
--              − 6·dist_centre + 22·has_garden + district effect + market + ε
--    β_area drifts from 2.4 to 3.6 k€ per m² over the three years; ε grows with
--    the living area (so the size effect grows across price quantiles); `market`
--    is a slowly moving sentiment shared by neighbouring sales (AR-like, φ≈0.6).
--    Agencies change the price by a few per cent. Planted problems: three luxury
--    villas keyed in at 2.5× their price (outliers, `flagged`) and one 620 m²
--    country estate far outside the data (high leverage, priced on the model).
CREATE OR REPLACE TABLE pm_sales AS
WITH d(district, d_no, d_eff, d_lo, d_hi, garden_p) AS (VALUES
    ('Old Town', 1, 45.0, 0.3, 3.0, 0.15), ('Harbour', 2, 25.0, 1.0, 5.0, 0.25),
    ('North Park', 3, 0.0, 2.0, 7.0, 0.45), ('Hillside', 4, 12.0, 3.0, 9.0, 0.60),
    ('West End', 5, -25.0, 3.5, 11.0, 0.55)
), a(agency, a_no, a_mult) AS (VALUES
    ('Aster Homes', 0, 1.00), ('Brick & Key', 1, 1.01), ('Cornerstone', 2, 1.07), ('Dovetail', 3, 0.94)
), base AS (
    SELECT i, DATE '2023-01-01' + CAST(floor((i - 1) * 1095 / 899.0) AS INTEGER) AS sale_date,
           (i - 1) / 899.0 * 3.0 AS yrs,
           1 + floor(5 * pm_unif('home', i, 'district'))::INTEGER AS d_no,
           floor(4 * pm_unif('home', i, 'agency'))::INTEGER AS a_no
    FROM range(1, 901) r(i)
), u AS (   -- market sentiment: a moving sum of shocks of neighbouring sales (≈ AR(1), φ = 0.6)
    SELECT b.i, sum(power(0.6, b.i - s.i) * pm_rnorm('market', s.i, 'shock')) AS market
    FROM range(1, 901) b(i) JOIN range(-12, 901) s(i) ON s.i BETWEEN b.i - 12 AND b.i
    GROUP BY b.i
), x AS (
    SELECT base.*, d.*, a.*,
           least(260.0, greatest(38.0, round(exp(ln(100) + 0.33 * pm_rnorm('home', i, 'area'))))) AS area0,
           floor(80 * power(pm_unif('home', i, 'age'), 1.4))::INTEGER AS age0,
           1 + floor(5 * pm_unif('home', i, 'cond'))::INTEGER AS condition0,
           round(d_lo + (d_hi - d_lo) * pm_unif('home', i, 'dist'), 1) AS dist0,
           (pm_unif('home', i, 'garden') < garden_p)::INTEGER AS garden0
    FROM base JOIN d USING (d_no) JOIN a USING (a_no)
), v AS (   -- the planted special homes
    SELECT x.* REPLACE (
               CASE WHEN i IN (137, 482, 771) THEN 230 + 10 * (i % 3) WHEN i = 600 THEN 620 ELSE area0 END AS area0,
               CASE WHEN i IN (137, 482, 771) THEN 5 ELSE condition0 END AS condition0,
               CASE WHEN i = 600 THEN 95 ELSE age0 END AS age0,
               CASE WHEN i = 600 THEN 12.5 ELSE dist0 END AS dist0,
               CASE WHEN i IN (137, 482, 771, 600) THEN 1 ELSE garden0 END AS garden0),
           CASE WHEN i IN (137, 482, 771) THEN 'Hillside' WHEN i = 600 THEN 'West End' END AS d_override
    FROM x
), y AS (
    SELECT v.*, coalesce(d_override, district) AS district2,
           CASE WHEN i = 600 THEN 14
                ELSE least(8, greatest(1, round(area0 / 28.0 + 0.6 * pm_rnorm('home', i, 'rooms')))) END::INTEGER AS rooms0
    FROM v
), p AS (
    SELECT y.*, (SELECT d_eff FROM d WHERE d.district = y.district2) AS d_eff2,
           (2.4 + 1.2 * yrs / 3.0) AS beta_area
    FROM y
)
SELECT i::INTEGER AS sale_id, sale_date, 'all' AS market, district2 AS district, agency,
       area0::DOUBLE AS living_area, area0 / 10.0 AS area_10, rooms0::DOUBLE AS rooms,
       age0::DOUBLE AS age, age0 / 10.0 AS age_10,
       condition0::DOUBLE AS condition, dist0::DOUBLE AS dist_centre, garden0::DOUBLE AS has_garden,
       (district2 = 'Old Town')::INTEGER::DOUBLE AS old_town, (district2 = 'Harbour')::INTEGER::DOUBLE AS harbour,
       (district2 = 'Hillside')::INTEGER::DOUBLE AS hillside, (district2 = 'West End')::INTEGER::DOUBLE AS west_end,
       round((CASE WHEN i IN (137, 482, 771) THEN 2.5 ELSE 1.0 END)
             * a_mult * (30 + beta_area * area0 + 6 * rooms0 - 0.9 * age0 + 14 * condition0 - 6 * dist0
                         + 22 * garden0 + d_eff2
                         + CASE WHEN i = 600 THEN 0 ELSE 20 * market
                                + (10 + 0.32 * area0) * pm_rnorm('home', i, 'eps') END), 1) AS price_k,
       i IN (137, 482, 771) AS flagged,
       CASE WHEN pm_unif('home', i, 'split') < 0.2 AND i NOT IN (137, 482, 771, 600) THEN 'holdout' ELSE 'train' END AS split
FROM p JOIN u USING (i) ORDER BY i;

-- B. pm_flats — 320 three-room flats (70–90 m²) in one segment: what are the
--    extras worth? Ten amenities, correlated through one hidden "upmarket"
--    factor; only garage (+20 k€), heat pump (+16), renovated kitchen (+12) and
--    balcony (+8) move the price.
CREATE OR REPLACE TABLE pm_flats AS
WITH f AS (
    SELECT i, pm_rnorm('flat', i, 'upmarket') AS up,
           70 + round(20 * pm_unif('flat', i, 'area')) AS area
    FROM range(1, 321) r(i)
), a AS (
    SELECT f.*,
           (pm_unif('flat', i, 'garage')   < 1 / (1 + exp(-(-0.4 + 2.0 * up))))::INTEGER::DOUBLE AS garage,
           (pm_unif('flat', i, 'balcony')  < 1 / (1 + exp(-( 0.4 + 2.0 * up))))::INTEGER::DOUBLE AS balcony,
           (pm_unif('flat', i, 'lift')     < 1 / (1 + exp(-(-0.2 + 2.0 * up))))::INTEGER::DOUBLE AS lift,
           (pm_unif('flat', i, 'cellar')   < 1 / (1 + exp(-( 0.6 + 2.0 * up))))::INTEGER::DOUBLE AS cellar,
           (pm_unif('flat', i, 'solar')    < 1 / (1 + exp(-(-1.0 + 2.0 * up))))::INTEGER::DOUBLE AS solar,
           (pm_unif('flat', i, 'heatpump') < 1 / (1 + exp(-(-0.8 + 2.0 * up))))::INTEGER::DOUBLE AS heat_pump,
           (pm_unif('flat', i, 'kitchen')  < 1 / (1 + exp(-(-0.3 + 2.0 * up))))::INTEGER::DOUBLE AS new_kitchen,
           (pm_unif('flat', i, 'fire')     < 1 / (1 + exp(-(-1.4 + 2.0 * up))))::INTEGER::DOUBLE AS fireplace,
           (pm_unif('flat', i, 'smart')    < 1 / (1 + exp(-(-0.9 + 2.0 * up))))::INTEGER::DOUBLE AS smart_home,
           (pm_unif('flat', i, 'parquet')  < 1 / (1 + exp(-( 0.1 + 2.0 * up))))::INTEGER::DOUBLE AS parquet
    FROM f
)
SELECT i::INTEGER AS flat_id, 'flats' AS segment, area AS living_area,
       garage, balcony, lift, cellar, solar, heat_pump, new_kitchen, fireplace, smart_home, parquet,
       round(255 + 2.5 * (area - 80) + 20 * garage + 16 * heat_pump + 12 * new_kitchen + 8 * balcony
             + 14 * pm_rnorm('flat', i, 'eps'), 1) AS price_k
FROM a ORDER BY i;

-- C. pm_listings — 1,400 listings put on the market 2023–2025, each followed to
--    31 Dec 2025: days on the market (Weibull; slower with a high asking-price
--    premium, faster with more photos; fastest in the Old Town, slowest in the
--    West End; Dovetail sells fast, Cornerstone slowly) — still-listed homes are
--    censored. sold_30 = sold within 30 days (for listings online ≥ 30 days);
--    viewings ~ Poisson, more with photos, fewer with a high premium.
CREATE OR REPLACE TABLE pm_listings_base AS
WITH d(district, d_no, d_eff) AS (VALUES
    ('Old Town', 1, -0.40), ('Harbour', 2, -0.15), ('North Park', 3, 0.0), ('Hillside', 4, 0.15), ('West End', 5, 0.45)
), a(agency, a_no, a_eff) AS (VALUES
    ('Aster Homes', 0, 0.0), ('Brick & Key', 1, 0.0), ('Cornerstone', 2, 0.20), ('Dovetail', 3, -0.40)
), l AS (
    SELECT i, DATE '2023-01-01' + floor(1095 * pm_unif('list', i, 'listed'))::INTEGER AS listed_date,
           1 + floor(5 * pm_unif('list', i, 'district'))::INTEGER AS d_no,
           floor(4 * pm_unif('list', i, 'agency'))::INTEGER AS a_no,
           round(least(30, greatest(-10, 6 + 7 * pm_rnorm('list', i, 'premium'))), 1) AS premium_pct,
           4 + floor(36 * pm_unif('list', i, 'photos'))::INTEGER AS photos
    FROM range(1, 1401) r(i)
), t AS (
    SELECT l.*, d.district, a.agency,
           DATE '2025-12-31' - listed_date AS followup,
           exp(ln(50) + 0.08 * premium_pct - 0.025 * (photos - 20) + d_eff + a_eff)
               * power(-ln(pm_unif('list', i, 'dom')), 1 / 2.0) AS dom,
           exp(1.6 + 0.03 * photos - 0.025 * premium_pct) AS view_rate
    FROM l JOIN d USING (d_no) JOIN a USING (a_no)
)
SELECT i::INTEGER AS listing_id, 'all' AS market, listed_date, district, agency, premium_pct::DOUBLE AS premium_pct,
       photos::DOUBLE AS photos,
       (district = 'Old Town')::INTEGER::DOUBLE AS old_town, (district = 'Harbour')::INTEGER::DOUBLE AS harbour,
       (district = 'Hillside')::INTEGER::DOUBLE AS hillside, (district = 'West End')::INTEGER::DOUBLE AS west_end,
       greatest(0, round(view_rate + sqrt(view_rate) * pm_rnorm('list', i, 'views')))::DOUBLE AS viewings,
       -- The study follows each listing for at most a year (or until 31 Dec 2025).
       greatest(1, ceil(least(dom, followup, 365)))::INTEGER AS days_on_market,
       dom <= least(followup, 365) AS sold,
       CASE WHEN followup >= 30 THEN dom <= 30 END AS sold_30
FROM t ORDER BY i;
-- The logistic fit (p_logit) and a rule of thumb (p_rule) for every listing online ≥ 30 days
CREATE OR REPLACE TABLE pm_listings_30 AS
SELECT *, sold_30::INTEGER::DOUBLE AS sold_30_flag FROM pm_listings_base WHERE sold_30 IS NOT NULL ORDER BY listing_id;
CREATE OR REPLACE TABLE pm_listings AS
SELECT b.*, round(f.yhat, 4) AS p_logit,
       (0.10 + 0.30 * (b.premium_pct <= 5)::INTEGER + 0.20 * (b.photos >= 20)::INTEGER
             + 0.15 * (b.district IN ('Old Town', 'Harbour'))::INTEGER)::DOUBLE AS p_rule
FROM pm_listings_base b
LEFT JOIN (SELECT listing_id, yhat
           FROM logistic_fit_predict_by('pm_listings_30', market, sold_30_flag,
                                        [premium_pct, photos, old_town, harbour, hillside, west_end])) f
       USING (listing_id)
ORDER BY listing_id;
-- both scores stacked long: one curve per model
CREATE OR REPLACE TABLE pm_scores AS
SELECT 'logistic' AS model, listing_id, sold_30, p_logit AS score FROM pm_listings WHERE sold_30 IS NOT NULL
UNION ALL
SELECT 'rule of thumb' AS model, listing_id, sold_30, p_rule AS score FROM pm_listings WHERE sold_30 IS NOT NULL;

-- D. pm_homes — the sales without the three rows the fit checks (tab 3) flag
--    and without the one-off estate, plus the price per m²; the model's residuals in sale-date order.
CREATE OR REPLACE TABLE pm_homes AS
SELECT * EXCLUDE (flagged), round(1000 * price_k / living_area) AS eur_m2 FROM pm_sales
WHERE NOT flagged AND sale_id <> 600 ORDER BY sale_id;
CREATE OR REPLACE TABLE pm_resid AS
SELECT market, sale_id, sale_date, area_10, rooms, age_10, condition, dist_centre, has_garden,
       old_town, harbour, hillside, west_end, round(residual, 3) AS resid
FROM ols_augment_by('pm_homes', market, price_k,
                    [area_10, rooms, age_10, condition, dist_centre, has_garden, old_town, harbour, hillside, west_end],
                    order_by := sale_date)
ORDER BY sale_date;

-- Hypothesis tests (the *_test_agg aggregates, one `test` row each) — a view,
-- so each tab computes only the tests it shows.
CREATE OR REPLACE VIEW pm_tests AS
SELECT unnest(pm_test_row('price', 'shapiro_wilk', 'Price residuals normal?', shapiro_wilk_agg(residual)))
FROM ols_augment_by('pm_sales', market, price_k,
                    [area_10, rooms, age_10, condition, dist_centre, has_garden, old_town, harbour, hillside, west_end])
UNION ALL
SELECT unnest(pm_test_row('price', 'breusch_pagan', 'Residual spread constant across homes?',
                          breusch_pagan_agg(residual, [area_10, rooms, age_10, condition, dist_centre, has_garden,
                                                       old_town, harbour, hillside, west_end])))
FROM ols_augment_by('pm_sales', market, price_k,
                    [area_10, rooms, age_10, condition, dist_centre, has_garden, old_town, harbour, hillside, west_end])
UNION ALL
SELECT unnest(pm_test_row('time', 'durbin_watson', 'Consecutive sales'' residuals correlated?',
                          durbin_watson_agg(resid, [area_10, rooms, age_10, condition, dist_centre, has_garden,
                                                    old_town, harbour, hillside, west_end] ORDER BY sale_date)))
FROM pm_resid
UNION ALL
SELECT unnest(pm_test_row('time', 'ljung_box', 'Residuals correlated over the last 10 sales?',
                          ljung_box_agg(resid, 10 ORDER BY sale_date))) FROM pm_resid
UNION ALL
SELECT unnest(pm_test_row('market', 'log_rank', 'Do homes sell equally fast in all five districts?',
                          log_rank_test_agg(days_on_market, sold, district))) FROM pm_listings
UNION ALL
SELECT unnest(pm_test_row('agency', 'anova', 'Does the price per m² differ between agencies?',
                          one_way_anova_agg(eur_m2, agency))) FROM pm_homes
UNION ALL
SELECT unnest(pm_test_row('agency', 'kruskal_wallis', 'Same, without assuming normality?',
                          kruskal_wallis_agg(eur_m2, agency))) FROM pm_homes
UNION ALL
SELECT unnest(pm_test_row('agency', 'chisq', 'Does the 30-day sale rate depend on the agency?',
                          chisq_test_agg((ascii(agency) - 65)::INTEGER, sold_30::INTEGER)))
FROM pm_listings WHERE sold_30 IS NOT NULL;

-- ─────────────────────────────────────────────────────────────────────────────
-- Dashboard
-- ─────────────────────────────────────────────────────────────────────────────
SELECT 'What sets the price of a home? A regression workflow on 900 sales'::LABEL;

SELECT 12::COL;
SELECT 'A made-up city''s property market, 2023–2025 — 900 home sales, 1,400 listings and 320 flats. The **anofox_statistics** extension fits the models (`ols_tidy_by`, `ols_augment_by`, `ridge_cv_by`, `kaplan_meier_by`, `tukey_hsd_by`, …) and each panel hands the producer''s long table to a **contract plot**, e.g. `SELECT anofox_plot_terms(t) FROM ols_tidy_by(…) t`. The tabs follow the modelling workflow: **state the question → compare models → check the fit → predict → regularise → look beyond the mean → over time → classification → survival → group comparisons.** Open the SQL editor to see (and change) every query.'::MARKDOWN, 'About this dashboard'::TITLE;

-- ── 1 · Overview ─────────────────────────────────────────────────────────────
SELECT '1 Overview'::TAB;

SELECT 12::COL;
SELECT '**Question:** what sets the sale price of a home? We model the price (k€) of 900 sales on **living area** (per 10 m²), **rooms**, **age** (per decade), **condition** (1–5), **distance to the centre**, a **garden** and the **district** (dummies; North Park is the reference) — with four estimators: **OLS**, robust **Huber**, **ridge** (shrunk) and the **median** (quantile) regression. **What to look for:** each dot is an effect with its 95% interval; intervals crossing the dashed zero line are noise (here: **rooms** for three of the four estimators — once the area is known, an extra room adds little). Where the estimators disagree, a few rows are pulling some of them: OLS and ridge put the **Hillside** premium at **37 k€** and **area at 35 k€ per 10 m²**, the robust Huber and median fits at **13–16 k€** and **31–32 k€** — tab 3 finds the culprits.'::MARKDOWN, 'The question'::TITLE;

SELECT 'The price model at a glance'::GROUP;
SELECT round(max(value), 3)::METRIC, ('Best R² — ' || arg_max(model_id, value))::LABEL,
       'R²: the share of the variation in sale prices that the model explains (0 = none, 1 = all).'::INFO
FROM (SELECT * FROM ols_glance_by('pm_sales', market, price_k, [area_10, rooms, age_10, condition, dist_centre, has_garden, old_town, harbour, hillside, west_end])
      UNION ALL BY NAME SELECT * FROM huber_glance_by('pm_sales', market, price_k, [area_10, rooms, age_10, condition, dist_centre, has_garden, old_town, harbour, hillside, west_end])
      UNION ALL BY NAME SELECT * FROM ridge_glance_by('pm_sales', market, price_k, [area_10, rooms, age_10, condition, dist_centre, has_garden, old_town, harbour, hillside, west_end], options := {'alpha': 5.0}))
WHERE metric = 'r_squared';
SELECT count(*)::COMPACT, 'homes sold, 2023–2025'::LABEL FROM pm_sales;
SELECT round(median(price_k))::METRIC, 'median price (k€)'::LABEL FROM pm_sales;
SELECT round(10 * estimate)::METRIC, '€ per extra m² (Huber)'::LABEL,
       'The area coefficient of the robust Huber fit, with rooms, age, condition, distance, garden and district held fixed. Huber down-weights the few mis-keyed sales that pull least squares.'::INFO
FROM huber_tidy_by('pm_sales', market, price_k, [area_10, rooms, age_10, condition, dist_centre, has_garden, old_town, harbour, hillside, west_end])
WHERE term = 'x1';
SELECT round(median(1000 * price_k / living_area))::METRIC, 'median € per m²'::LABEL FROM pm_sales;
SELECT 1::ENDGROUP;

SELECT 12::COL;
SELECT 420::HEIGHT;
SELECT anofox_plot_terms(t, x_label := 'effect on the sale price (k€)'),
       'Coefficient forest — area, condition and district set the price; OLS overstates Hillside (37 vs 16 k€ robust)'::TITLE
FROM (SELECT * FROM ols_tidy_by('pm_sales', market, price_k, [area_10, rooms, age_10, condition, dist_centre, has_garden, old_town, harbour, hillside, west_end],
                                names := ['area (10 m²)', 'rooms', 'age (decade)', 'condition', 'km to centre', 'garden', 'Old Town', 'Harbour', 'Hillside', 'West End'])
      UNION ALL BY NAME
      SELECT * FROM huber_tidy_by('pm_sales', market, price_k, [area_10, rooms, age_10, condition, dist_centre, has_garden, old_town, harbour, hillside, west_end],
                                  names := ['area (10 m²)', 'rooms', 'age (decade)', 'condition', 'km to centre', 'garden', 'Old Town', 'Harbour', 'Hillside', 'West End'])
      UNION ALL BY NAME
      SELECT * FROM ridge_tidy_by('pm_sales', market, price_k, [area_10, rooms, age_10, condition, dist_centre, has_garden, old_town, harbour, hillside, west_end], options := {'alpha': 5.0},
                                  names := ['area (10 m²)', 'rooms', 'age (decade)', 'condition', 'km to centre', 'garden', 'Old Town', 'Harbour', 'Hillside', 'West End'])
      UNION ALL BY NAME
      SELECT * FROM quantile_tidy_by('pm_sales', market, price_k, [area_10, rooms, age_10, condition, dist_centre, has_garden, old_town, harbour, hillside, west_end], options := {'tau': 0.5},
                                     names := ['area (10 m²)', 'rooms', 'age (decade)', 'condition', 'km to centre', 'garden', 'Old Town', 'Harbour', 'Hillside', 'West End'])) t
WHERE term <> '(Intercept)';

-- ── 2 · Model comparison ─────────────────────────────────────────────────────
SELECT '2 Compare'::TAB;

SELECT 12::COL;
SELECT 'Five estimators of the same price model, compared on their **glance** statistics (`<model>_glance_by`, the `summary` schema): one panel per metric, one dot per model. **What to look for:** higher **R²** and lower **AIC / BIC / residual standard error** are better. The differences are small: **OLS** and **ridge** reach R² = 0.703, **Theil–Sen** and the **elastic net** 0.695, **Huber** 0.692 — the robust fits score a little *lower* because they refuse to chase the three mis-keyed villas, which least squares bends towards. A residual standard error of about **100 k€** is large for homes with a median price of 357 k€: those few bad rows inflate it (tab 3). Information criteria are only reported by likelihood-based fits, so some models have no AIC dot.'::MARKDOWN, 'What to look for'::TITLE;

SELECT 12::COL;
SELECT 420::HEIGHT;
SELECT anofox_plot_summary(s, metrics := ['r_squared', 'adj_r_squared', 'residual_std_error', 'aic', 'bic']),
       'Fit statistics by model — near-identical fits (R² 0.69–0.70); the robust fits don''t chase the outliers'::TITLE
FROM (SELECT * FROM ols_glance_by('pm_sales', market, price_k, [area_10, rooms, age_10, condition, dist_centre, has_garden, old_town, harbour, hillside, west_end])
      UNION ALL BY NAME SELECT * FROM huber_glance_by('pm_sales', market, price_k, [area_10, rooms, age_10, condition, dist_centre, has_garden, old_town, harbour, hillside, west_end])
      UNION ALL BY NAME SELECT * FROM ridge_glance_by('pm_sales', market, price_k, [area_10, rooms, age_10, condition, dist_centre, has_garden, old_town, harbour, hillside, west_end], options := {'alpha': 5.0})
      UNION ALL BY NAME SELECT * FROM theil_sen_glance_by('pm_sales', market, price_k, [area_10, rooms, age_10, condition, dist_centre, has_garden, old_town, harbour, hillside, west_end])
      UNION ALL BY NAME SELECT * FROM elasticnet_glance_by('pm_sales', market, price_k, [area_10, rooms, age_10, condition, dist_centre, has_garden, old_town, harbour, hillside, west_end], options := {'alpha': 5.0})) s;

SELECT 12::COL;
SELECT 'Glance statistics'::TITLE, model_id AS "Model" ::TABLE,
       round(max(value) FILTER (WHERE metric = 'r_squared'), 4) AS "R²" ::PLAIN,
       round(max(value) FILTER (WHERE metric = 'adj_r_squared'), 4) AS "adj. R²" ::PLAIN,
       round(max(value) FILTER (WHERE metric = 'residual_std_error'), 1) AS "resid. SE (k€)" ::PLAIN,
       round(max(value) FILTER (WHERE metric = 'aic'), 1) AS "AIC" ::PLAIN,
       round(max(value) FILTER (WHERE metric = 'bic'), 1) AS "BIC" ::PLAIN
FROM (SELECT * FROM ols_glance_by('pm_sales', market, price_k, [area_10, rooms, age_10, condition, dist_centre, has_garden, old_town, harbour, hillside, west_end])
      UNION ALL BY NAME SELECT * FROM huber_glance_by('pm_sales', market, price_k, [area_10, rooms, age_10, condition, dist_centre, has_garden, old_town, harbour, hillside, west_end])
      UNION ALL BY NAME SELECT * FROM ridge_glance_by('pm_sales', market, price_k, [area_10, rooms, age_10, condition, dist_centre, has_garden, old_town, harbour, hillside, west_end], options := {'alpha': 5.0})
      UNION ALL BY NAME SELECT * FROM theil_sen_glance_by('pm_sales', market, price_k, [area_10, rooms, age_10, condition, dist_centre, has_garden, old_town, harbour, hillside, west_end])
      UNION ALL BY NAME SELECT * FROM elasticnet_glance_by('pm_sales', market, price_k, [area_10, rooms, age_10, condition, dist_centre, has_garden, old_town, harbour, hillside, west_end], options := {'alpha': 5.0}))
GROUP BY model_id ORDER BY 3 DESC;

-- ── 3 · Check the fit ────────────────────────────────────────────────────────
SELECT '3 Fit checks'::TAB;

SELECT 12::COL;
SELECT 'Residual diagnostics from `ols_augment_by` (the `obs` schema: fitted, residual, leverage, Cook''s distance per row). **What to look for:** in **residuals vs fitted**, a flat smoother around zero; on the **normal Q-Q**, points on the line; in **scale-location**, constant spread; no point beyond the **Cook''s distance** contours. Three rows stand out everywhere — sales **137, 482 and 771**, Hillside villas whose price was keyed in at 2.5× (1.5–2.8 M€ instead of 0.6–1.1 M€); sale 482 alone has a Cook''s distance of 1.08. Sale **600**, a 620 m² country estate, has by far the highest **leverage** (0.28, ten times the next home) but a correctly recorded price — it sits near the line, and its Cook''s distance (0.19) comes from leverage, not from a wrong price. The funnel in scale-location is real too: the spread of prices grows with the size of the home (Breusch–Pagan rejects) — tab 6 picks that up. From tab 4 on, the three mis-keyed rows are dropped, and so is the estate — a one-off far outside the market the model describes.'::MARKDOWN, 'What to look for'::TITLE;

SELECT 12::COL;
SELECT 600::HEIGHT;
SELECT anofox_plot_diagnostics(o), 'plot.lm 2×2 — three mis-keyed villas dominate the residuals; the 620 m² estate has the leverage'::TITLE
FROM ols_augment_by('pm_sales', market, price_k, [area_10, rooms, age_10, condition, dist_centre, has_garden, old_town, harbour, hillside, west_end]) o;

SELECT 12::COL;
SELECT anofox_plot_obs(o, kind := 'cooks', x_label := 'sale (row id)', y_label := 'Cook''s distance'),
       'Cook''s distance — sale 482 alone reaches 1.08; the villas and the estate sit above the 4/n line'::TITLE
FROM ols_augment_by('pm_sales', market, price_k, [area_10, rooms, age_10, condition, dist_centre, has_garden, old_town, harbour, hillside, west_end]) o;

SELECT 12::COL;
SELECT '**Partial-residual plots** (`ols_partial_residuals_by`) look at one term at a time with the others held fixed. In the **component + residual** plot the straight line is the term''s fitted effect and the smoother shows what the data say — a bend means the term needs a transformation. The **added-variable** plot regresses what is left of the price on what is left of the term; its slope *is* the coefficient, and points far from the line pull it. **Garden** is a 0/1 term: its points stack in two columns and the line joins their means. Watch the three villas at the top of the **area** panels and the estate far to the right.'::MARKDOWN, 'Partial residuals'::TITLE;

SELECT 12::COL;
SELECT 460::HEIGHT;
SELECT anofox_plot_partial(p, kind := 'component', y_label := 'partial residual (k€)'),
       'Component + residual — area, age and distance act linearly; the villas sit far above'::TITLE
FROM ols_partial_residuals_by('pm_sales', market, price_k, [area_10, age_10, condition, dist_centre, has_garden, old_town, harbour, hillside, west_end],
                              names := ['area (10 m²)', 'age (decade)', 'condition', 'km to centre', 'garden', 'Old Town', 'Harbour', 'Hillside', 'West End']) p
WHERE term IN ('area (10 m²)', 'age (decade)', 'km to centre', 'garden');

SELECT 12::COL;
SELECT 460::HEIGHT;
SELECT anofox_plot_partial(p, kind := 'added_variable'),
       'Added-variable plots — each slope is that term''s coefficient'::TITLE
FROM ols_partial_residuals_by('pm_sales', market, price_k, [area_10, age_10, condition, dist_centre, has_garden, old_town, harbour, hillside, west_end],
                              names := ['area (10 m²)', 'age (decade)', 'condition', 'km to centre', 'garden', 'Old Town', 'Harbour', 'Hillside', 'West End']) p
WHERE term IN ('area (10 m²)', 'age (decade)', 'km to centre', 'garden');

SELECT 12::COL;
SELECT 'Residual tests — the villas break normality; the spread grows with the home'::TITLE,
       question AS "Question" ::TABLE, method AS "Test", round(statistic, 3) AS "Statistic" ::PLAIN,
       printf('%.2g', p_value) AS "p-value" ::PLAIN,
       CASE WHEN p_value < 0.05 THEN 'reject' ELSE 'ok' END AS "at 5%" ::BADGE
FROM pm_tests WHERE model_id = 'price';

-- ── 4 · Predictions ──────────────────────────────────────────────────────────
SELECT '4 Predict'::TAB;

SELECT 12::COL;
SELECT 'How well does the model price a home it has not seen? One in five sales (172 homes) is held out (`split := split`); the model is fitted on the other 724 (`<model>_prediction_by`, the `prediction` schema). **What to look for:** the held-out sales are labelled **test** (the producer''s name for any non-training row); about 90% of them should fall inside the **90% prediction band** around the fitted line. The x axis is the **predicted price**, so the fitted line is the diagonal and every home — whatever its size, age, condition and district — sits above or below it by its residual. The overlay compares OLS, Huber and the median regression on the hold-out only. The **effect plots** below show one feature at a time on the response scale, the others held at their mean.'::MARKDOWN, 'What to look for'::TITLE;

SELECT 12::COL;
SELECT 400::HEIGHT;
SELECT anofox_plot_prediction(p, x := 'yhat', x_label := 'predicted price (k€)', y_label := 'sale price (k€)'),
       'Observed vs predicted — 94% of the held-out sales land inside the 90% band'::TITLE
FROM ols_prediction_by('pm_homes', market, living_area, price_k,
                       [area_10, rooms, age_10, condition, dist_centre, has_garden, old_town, harbour, hillside, west_end],
                       level := 0.9, split := split) p;

SELECT 12::COL;
SELECT 360::HEIGHT;
SELECT anofox_plot_prediction(p, x := 'yhat', x_label := 'predicted price (k€)', y_label := 'sale price (k€)'),
       'Three estimators on the 172 held-out sales — OLS, Huber and the median line nearly coincide'::TITLE
FROM (SELECT * FROM ols_prediction_by('pm_homes', market, living_area, price_k,
                                      [area_10, rooms, age_10, condition, dist_centre, has_garden, old_town, harbour, hillside, west_end],
                                      level := 0.9, split := split)
      UNION ALL BY NAME
      SELECT * FROM huber_prediction_by('pm_homes', market, living_area, price_k,
                                        [area_10, rooms, age_10, condition, dist_centre, has_garden, old_town, harbour, hillside, west_end],
                                        level := 0.9, split := split)
      UNION ALL BY NAME
      SELECT * FROM quantile_prediction_by('pm_homes', market, living_area, price_k,
                                           [area_10, rooms, age_10, condition, dist_centre, has_garden, old_town, harbour, hillside, west_end],
                                           split := split)) p
WHERE split <> 'train';

SELECT 4::COL;
SELECT anofox_plot_prediction(p, x_label := 'living area (m²)', y_label := 'sale price (k€)'),
       'Price rises 3.3 k€ per m² (OLS)'::TITLE
FROM ols_effects_by('pm_homes', market, price_k, [living_area, age, condition, dist_centre, has_garden, old_town, harbour, hillside, west_end],
                    focal := 1, n_points := 30) p;
SELECT 4::COL;
SELECT anofox_plot_prediction(p, x_label := 'asking price over estimated value (%)', y_label := 'P(sold within 30 days)'),
       'Over-pricing kills the quick sale (logistic)'::TITLE
FROM logistic_effects_by('pm_listings_30', market, sold_30_flag, [premium_pct, photos, old_town, harbour, hillside, west_end],
                         focal := 1, n_points := 30) p;
SELECT 4::COL;
SELECT anofox_plot_prediction(p, x_label := 'listing photos', y_label := 'viewings'),
       'More photos, more viewings (Poisson)'::TITLE
FROM poisson_effects_by('pm_listings', market, viewings, [photos, premium_pct], focal := 1, n_points := 30) p;

-- ── 5 · Regularisation & selection ───────────────────────────────────────────
SELECT '5 Regularise'::TAB;

SELECT 12::COL;
SELECT 'What are the extras worth? 320 comparable three-room flats (70–90 m²) with **ten amenities** that come in bundles — upmarket flats tend to have several — of which only **garage, heat pump, renovated kitchen and balcony** change the price. All three minimise the squared error plus a penalty on the coefficients,

$$\min_{\beta}\; \frac{1}{2n}\lVert y - X\beta \rVert_2^2 + \lambda\, P_\alpha(\beta)$$

$$P_\alpha(\beta) = \alpha \lVert \beta \rVert_1 + \tfrac{1-\alpha}{2} \lVert \beta \rVert_2^2$$

**Ridge** ($\alpha = 0$) shrinks all coefficients towards zero as the penalty λ grows; the **elastic net** ($\alpha = 0.8$ here) and **LARS** (the lasso path, $\alpha = 1$) set them to zero one by one — the order they enter is a variable ranking. **Cross-validation** picks λ: **λ.min** has the lowest error, **λ.1se** is the simplest model within one standard error of it. **What to look for:** the four real amenities (highlighted) stay large the longest; the six others hover around zero.'::MARKDOWN, 'What to look for'::TITLE;

SELECT 'Ridge'::SUBTAB;
SELECT 12::COL;
SELECT 560::HEIGHT;
SELECT anofox_plot_terms(t, highlight := ['garage', 'heat_pump', 'new_kitchen', 'balcony'], y_label := 'coefficient (k€)'),
       'Ridge path — every amenity shrinks smoothly; garage, heat pump and kitchen stay on top'::TITLE
FROM ridge_path_by('pm_flats', segment, price_k, [garage, balcony, lift, cellar, solar, heat_pump, new_kitchen, fireplace, smart_home, parquet],
                   options := {'n_lambda': 40, 'standardize': true},
                   names := ['garage', 'balcony', 'lift', 'cellar', 'solar', 'heat_pump', 'new_kitchen', 'fireplace', 'smart_home', 'parquet']) t
WHERE term <> '(Intercept)';
SELECT 12::COL;
SELECT anofox_plot_curve(c, y_label := 'CV mean squared error'), 'Ridge — 5-fold CV error by λ (λ.min and λ.1se marked)'::TITLE
FROM ridge_cv_by('pm_flats', segment, price_k, [garage, balcony, lift, cellar, solar, heat_pump, new_kitchen, fireplace, smart_home, parquet],
                 options := {'n_lambda': 40, 'folds': 5, 'standardize': true}) c;

SELECT 'Elastic net'::SUBTAB;
SELECT 12::COL;
SELECT 560::HEIGHT;
SELECT anofox_plot_terms(t, highlight := ['garage', 'heat_pump', 'new_kitchen', 'balcony'], y_label := 'coefficient (k€)'),
       'Elastic-net path (l1 ratio 0.8) — the six idle amenities drop to exactly zero first'::TITLE
FROM elasticnet_path_by('pm_flats', segment, price_k, [garage, balcony, lift, cellar, solar, heat_pump, new_kitchen, fireplace, smart_home, parquet],
                        options := {'n_lambda': 40, 'l1_ratio': 0.8, 'standardize': true},
                        names := ['garage', 'balcony', 'lift', 'cellar', 'solar', 'heat_pump', 'new_kitchen', 'fireplace', 'smart_home', 'parquet']) t
WHERE term <> '(Intercept)';
SELECT 12::COL;
SELECT anofox_plot_curve(c, y_label := 'CV mean squared error'), 'Elastic net — CV error by λ: λ.min keeps 8 amenities, λ.1se 5'::TITLE
FROM elasticnet_cv_by('pm_flats', segment, price_k, [garage, balcony, lift, cellar, solar, heat_pump, new_kitchen, fireplace, smart_home, parquet],
                      options := {'n_lambda': 40, 'folds': 5, 'l1_ratio': 0.8, 'standardize': true}) c;

SELECT 'LARS'::SUBTAB;
SELECT 12::COL;
SELECT 560::HEIGHT;
SELECT anofox_plot_terms(t, highlight := ['garage', 'heat_pump', 'new_kitchen', 'balcony'], y_label := 'coefficient (k€)'),
       'LARS / lasso path — amenities enter in order: garage, heat pump, kitchen, balcony, then noise'::TITLE
FROM lars_path_by('pm_flats', segment, price_k, [garage, balcony, lift, cellar, solar, heat_pump, new_kitchen, fireplace, smart_home, parquet],
                  names := ['garage', 'balcony', 'lift', 'cellar', 'solar', 'heat_pump', 'new_kitchen', 'fireplace', 'smart_home', 'parquet']) t
WHERE term <> '(Intercept)';

-- ── 6 · Beyond the mean ──────────────────────────────────────────────────────
SELECT '6 Quantiles'::TAB;

SELECT 12::COL;
SELECT 'Least squares describes the **average** home. A **quantile regression** fits each quantile τ of the price separately, minimising the asymmetric check loss $\sum_i \rho_\tau(y_i - x_i^\top\beta)$ with $\rho_\tau(u) = u\,(\tau - \mathbf{1}[u < 0])$; `quantile_process_by` runs it over a grid of τ (the `terms` schema with `index_name = ''tau''`), and the dashed line is the OLS estimate for comparison. **What to look for:** a flat line means the effect is the same for cheap and expensive homes. Here **each extra m²** is worth about **2.8 k€ at the 5% quantile** but **4.1 k€ at the 95% quantile** (OLS: 3.3) — the price spread fans out with size, so at the top of the market a m² costs much more. **Distance to the centre**, by contrast, costs 4.5–6.7 k€ per km at every quantile: a nearly flat line.'::MARKDOWN, 'What to look for'::TITLE;

SELECT 12::COL;
SELECT 400::HEIGHT;
SELECT anofox_plot_terms(t, x_label := 'quantile τ', y_label := 'effect (k€)',
                         reference := (SELECT list(r) FROM ols_tidy_by('pm_homes', market, price_k,
                                                       [living_area, age, condition, dist_centre, has_garden, old_town, harbour, hillside, west_end],
                                                       names := ['living area (m²)', 'age (years)', 'condition', 'km to centre', 'garden',
                                                                 'Old Town', 'Harbour', 'Hillside', 'West End']) r
                                       WHERE r.term IN ('living area (m²)', 'km to centre'))),
       'Quantile process — a m² is worth 2.8 k€ at the bottom of the market, 4.1 k€ at the top'::TITLE
FROM quantile_process_by('pm_homes', market, price_k, [living_area, age, condition, dist_centre, has_garden, old_town, harbour, hillside, west_end],
                         options := {'taus': [0.05, 0.1, 0.15, 0.2, 0.25, 0.3, 0.35, 0.4, 0.45, 0.5,
                                              0.55, 0.6, 0.65, 0.7, 0.75, 0.8, 0.85, 0.9, 0.95]},
                         names := ['living area (m²)', 'age (years)', 'condition', 'km to centre', 'garden',
                                   'Old Town', 'Harbour', 'Hillside', 'West End']) t
WHERE term IN ('living area (m²)', 'km to centre');

SELECT 12::COL;
SELECT living_area::XAXIS, price_k::SCATTER,
       'The data — price (k€) by living area (m²): the spread fans out as homes get bigger'::TITLE
FROM pm_homes;

-- ── 7 · Over time ────────────────────────────────────────────────────────────
SELECT '7 Over time'::TAB;

SELECT 12::COL;
SELECT 'The sales are ordered by sale date (one per day, Jan 2023 – Dec 2025). Two questions. **Is the price of a m² stable?** `ols_rolling_tidy_by` refits the model on a sliding window of 180 consecutive sales (≈ 7 months) and returns one coefficient (± interval) per window end — **2.9 k€ per m²** in the window ending Aug 2023, **3.65 k€** at the end of 2025; the dashed line is the single full-period estimate, which hides that drift. **Are neighbouring sales independent?** No, for two reasons. The model has no sale-date term, but the market rose: the mean residual climbs from **−37 k€ in 2023** to **+41 k€ in 2025** (bottom chart). That left-over trend keeps the **ACF** near 0.25–0.3 at *every* lag; on top, sales close in time share a short-lived market mood, so the **PACF** starts with a lag-1 spike (0.31) that fades over a few lags. **Durbin–Watson** (1.39, well below 2) and **Ljung–Box** reject independence — the OLS standard errors are too small. Add a time term (or the year as a factor), then use HAC errors for what remains.'::MARKDOWN, 'What to look for'::TITLE;

SELECT 12::COL;
SELECT 380::HEIGHT;
SELECT anofox_plot_terms(t, x_label := 'window end (year)', y_label := 'k€ per m²',
                         reference := (SELECT list(r) FROM ols_tidy_by('pm_homes', market, price_k,
                                                       [living_area, age, condition, dist_centre, has_garden],
                                                       names := ['living area (m²)', 'age (years)', 'condition', 'km to centre', 'garden']) r
                                       WHERE r.term = 'living area (m²)')),
       'Rolling 180-sale coefficient — a m² got about 27% dearer over three years'::TITLE
-- index_value = the window end as a decimal year (2023.6 … 2026.0)
FROM (SELECT * REPLACE (year(window_end) + (dayofyear(window_end) - 1) / 365.0 AS index_value)
      FROM ols_rolling_tidy_by('pm_homes', market, sale_date, price_k, [living_area, age, condition, dist_centre, has_garden],
                               window_size := 180, names := ['living area (m²)', 'age (years)', 'condition', 'km to centre', 'garden'])) t
WHERE term = 'living area (m²)';

SELECT 6::COL;
SELECT CASE curve_type WHEN 'acf' THEN 'ACF: high at every lag (trend left over)' ELSE 'PACF: lag-1 spike, fading' END AS what,
       anofox_plot_curve(c, x_label := 'lag (sales)'), 'Price residuals in sale-date order'::TITLE
FROM (SELECT * FROM acf_by('pm_resid', market, resid, sale_date, max_lag := 20)
      UNION ALL BY NAME
      SELECT * FROM pacf_by('pm_resid', market, resid, sale_date, max_lag := 20)) c
GROUP BY curve_type ORDER BY curve_type;

SELECT 12::COL;
SELECT 'Autocorrelation tests — both reject independence'::TITLE,
       question AS "Question" ::TABLE, method AS "Test", round(statistic, 3) AS "Statistic" ::PLAIN,
       printf('%.2g', p_value) AS "p-value" ::PLAIN,
       CASE WHEN p_value < 0.05 THEN 'reject' ELSE 'ok' END AS "at 5%" ::BADGE
FROM pm_tests WHERE model_id = 'time';

SELECT 12::COL;
SELECT date_trunc('month', sale_date)::DATE::XAXIS, round(avg(resid), 1)::LINECHART, 0::REFLINE,
       'Mean residual by month (k€) — from −42 in Jan 2023 to +65 in Dec 2025: the model misses the rising market'::TITLE
FROM pm_resid GROUP BY 1 ORDER BY 1;

-- ── 8 · Classification ───────────────────────────────────────────────────────
SELECT '8 Classify'::TAB;

SELECT 12::COL;
SELECT 'Will a listing sell **within 30 days**? Two scores for the same 1,363 listings (those online for at least 30 days): a **logistic regression** on the asking-price premium, the number of photos and the district, and an agent''s **rule of thumb** (points for a modest premium, ≥ 20 photos and a central district). `roc_curve_by`, `pr_curve_by` and `calibration_curve_by` return the `curve` schema, one curve per model. **What to look for:** the **ROC** curve further from the diagonal (higher AUC) ranks the quick sales better; the **precision-recall** curve shows the trade-off against the 25.5% base rate (dashed); in the **calibration** plot, a calibrated model''s predicted probabilities lie on the diagonal — the rule of thumb is both less sharp (AUC 0.76 vs 0.81) and miscalibrated (Brier 0.178 vs 0.141).'::MARKDOWN, 'What to look for'::TITLE;

SELECT 'Discrimination and calibration'::GROUP;
SELECT round(auc_agg(sold_30, score), 3)::METRIC, 'AUC — logistic'::LABEL,
       'AUC: the chance that a listing which sold within 30 days gets a higher score than one which did not. 0.5 is a coin flip, 1 a perfect ranking.'::INFO
FROM pm_scores WHERE model = 'logistic';
SELECT round(auc_agg(sold_30, score), 3)::METRIC, 'AUC — rule of thumb'::LABEL FROM pm_scores WHERE model = 'rule of thumb';
SELECT round(brier_agg(sold_30, score), 3)::METRIC, 'Brier score — logistic (lower is better)'::LABEL FROM pm_scores WHERE model = 'logistic';
SELECT round(brier_agg(sold_30, score), 3)::METRIC, 'Brier score — rule of thumb'::LABEL FROM pm_scores WHERE model = 'rule of thumb';
SELECT 1::ENDGROUP;

SELECT 4::COL;
SELECT CASE curve_type WHEN 'roc' THEN 'ROC: who is ranked first' WHEN 'pr' THEN 'precision vs recall'
                       ELSE 'calibration: are the probabilities right?' END AS what,
       anofox_plot_curve(c, base_rate := (SELECT avg(sold_30::INTEGER) FROM pm_listings WHERE sold_30 IS NOT NULL)),
       'Sold within 30 days'::TITLE
FROM (SELECT * FROM roc_curve_by('pm_scores', model, sold_30, score)
      UNION ALL BY NAME
      SELECT * FROM pr_curve_by('pm_scores', model, sold_30, score)
      UNION ALL BY NAME
      SELECT * FROM calibration_curve_by('pm_scores', model, sold_30, score, n_bins := 10)) c
GROUP BY curve_type
ORDER BY CASE curve_type WHEN 'roc' THEN 1 WHEN 'pr' THEN 2 ELSE 3 END;

-- ── 9 · Survival ─────────────────────────────────────────────────────────────
SELECT '9 Survival'::TAB;

SELECT 12::COL;
SELECT 'How long does a home stay on the market? `kaplan_meier_by` estimates, per district, the share of listings **still unsold** after each day, $\hat S(t) = \prod_{t_i \le t} \big(1 - d_i / n_i\big)$ ($d_i$ sold on day $t_i$ of the $n_i$ still listed) — from times that are partly **censored**: homes still listed after a year, or at the end of the study on 31 Dec 2025 (marked +), only tell us they took *at least* that long. **What to look for:** a curve that drops faster sells faster; where it crosses 50% is the **median time on market** — about 37 days in the Old Town against 99 in the West End. The **log-rank** test confirms the districts differ.'::MARKDOWN, 'What to look for'::TITLE;

SELECT 8::COL;
SELECT 400::HEIGHT;
SELECT anofox_plot_curve(k, x_label := 'days on the market', y_label := 'share still listed'),
       'Time on market by district — the Old Town sells in weeks, the West End takes months (95% band, + = still listed)'::TITLE
FROM kaplan_meier_by('pm_listings', district, days_on_market, sold) k;

SELECT 4::COL;
SELECT 'Median time on market'::TITLE, series AS "District" ::TABLE,
       min(x) FILTER (WHERE y <= 0.5) AS "Median (days)" ::PLAIN, max(n_risk) AS "Listings" ::PLAIN
FROM kaplan_meier_by('pm_listings', district, days_on_market, sold)
GROUP BY series ORDER BY 2;

SELECT 12::COL;
SELECT 'Log-rank test'::TITLE, question AS "Question" ::TABLE, method AS "Test", round(statistic, 1) AS "χ²" ::PLAIN,
       df1 AS "df" ::PLAIN, printf('%.2g', p_value) AS "p-value" ::PLAIN,
       CASE WHEN p_value < 0.05 THEN 'curves differ' ELSE 'no difference' END AS "at 5%" ::BADGE
FROM pm_tests WHERE model_id = 'market';

-- ── 10 · Group comparisons ───────────────────────────────────────────────────
SELECT '10 Groups'::TAB;

SELECT 12::COL;
SELECT 'Four estate agencies sell in the same city. Do they get different prices? The **ANOVA** on the price per m² says yes; **post-hoc tests** say *which* pairs differ. `tukey_hsd_by` and `pairwise_t_test_by` (Holm-adjusted) return the `test` schema — one row per pair with the difference, its interval and the adjusted p-value. **What to look for:** in the comparison forest, intervals that cross zero are not significant. **Cornerstone** gets about **360 € per m² more** than Aster Homes, **Dovetail** about **260 € less**; Aster Homes and Brick & Key are indistinguishable. The other side of the coin: Dovetail sells **37%** of its listings within 30 days, Cornerstone only **19%**.'::MARKDOWN, 'What to look for'::TITLE;

SELECT 12::COL;
SELECT 460::HEIGHT;
SELECT anofox_plot_compare(r, value := 'eur_m2', factor := 'agency', label := '{p.signif}',
                           tests := (SELECT list(t) FROM tukey_hsd_by('pm_homes', market, eur_m2, agency) t),
                           y_label := 'price per m² (€)'),
       'Price per m² by agency — Cornerstone highest, Dovetail lowest (Tukey HSD brackets)'::TITLE
FROM pm_homes r;

SELECT 6::COL;
SELECT method, anofox_plot_tests(t, label := 'p', x_label := 'difference in € per m²'), 'Pairwise differences'::TITLE
FROM (SELECT * FROM tukey_hsd_by('pm_homes', market, eur_m2, agency)
      UNION ALL BY NAME
      SELECT * FROM pairwise_t_test_by('pm_homes', market, eur_m2, agency, options := {'p_adjust': 'holm'})) t
GROUP BY method ORDER BY method;

SELECT 12::COL;
SELECT 'Omnibus tests'::TITLE, question AS "Question" ::TABLE, method AS "Test", round(statistic, 2) AS "Statistic" ::PLAIN,
       printf('%.2g', p_value) AS "p-value" ::PLAIN,
       CASE WHEN p_value < 0.05 THEN 'differ' ELSE 'same' END AS "at 5%" ::BADGE
FROM pm_tests WHERE model_id = 'agency';
SELECT 12::COL;
SELECT agency::XAXIS, 'sold within 30 days'::CATEGORY, round(100 * r.estimate, 1)::BARCHART, round(100 * r.conf_low, 1)::YMIN, round(100 * r.conf_high, 1)::YMAX,
       'Sold within 30 days, % (95% CI) — Dovetail sells fastest'::TITLE
FROM (SELECT agency, prop_test_one_agg(sold_30::INTEGER) AS r
      FROM pm_listings WHERE sold_30 IS NOT NULL GROUP BY agency)
ORDER BY r.estimate DESC;
