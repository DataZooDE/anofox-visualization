-- Intervals, CRPS, MAE, WRMSSE and the forecast-comparison tests of every method.
-- Needs pf(method, cutoff, id, t, h, f): the point forecasts at the calibration
-- cutoff (d_1913, forecasting d_1914–d_1941) and the final cutoff (d_1941,
-- forecasting the test days d_1942–d_1969).
LOAD anofox_statistics;
SET threads = 12;
SET memory_limit = '24GB';

-- each series' level at the cutoff, and a level bin (fixed thresholds)
CREATE OR REPLACE TABLE lvl_at AS
SELECT id, t AS cutoff, 1 + m28 AS s,
       CASE WHEN m28 < 0.25 THEN 0 WHEN m28 < 1 THEN 1 WHEN m28 < 3 THEN 2 WHEN m28 < 10 THEN 3 ELSE 4 END AS bin
FROM roll WHERE t IN (1913, 1941);

-- forecasts with actuals; a day without a price (item not on the shelf) is forecast 0
CREATE OR REPLACE TABLE pfa AS
SELECT p.method, p.cutoff, p.id, p.t, p.h, greatest(coalesce(p.f, 0), 0) AS f, s.y::DOUBLE AS y, l.s, l.bin, (p.h + 6) // 7 AS week
FROM pf p JOIN sales s ON s.id = p.id AND s.t = p.t JOIN lvl_at l ON l.id = p.id AND l.cutoff = p.cutoff;

-- quantile levels: 20 equally spaced for CRPS, plus the 50 % and 80 % interval ends
CREATE OR REPLACE TABLE taus AS
SELECT tau::DOUBLE AS tau, NOT list_contains([0.1, 0.25, 0.75, 0.9]::DOUBLE[], tau::DOUBLE) AS crps
FROM (SELECT unnest([0.025, 0.075, 0.1, 0.125, 0.175, 0.225, 0.25, 0.275, 0.325, 0.375, 0.425, 0.475, 0.525, 0.575, 0.625, 0.675, 0.725, 0.75, 0.775, 0.825, 0.875, 0.9, 0.925, 0.975]::DOUBLE[]) AS tau);

-- calibration: the quantiles of each method's scaled errors on d_1914–d_1941,
-- per level bin and forecast week; the same recipe for every method
CREATE OR REPLACE TABLE resq AS
SELECT method, bin, week, quantile_cont((y - f) / s, [0.025, 0.075, 0.1, 0.125, 0.175, 0.225, 0.25, 0.275, 0.325, 0.375, 0.425, 0.475, 0.525, 0.575, 0.625, 0.675, 0.725, 0.75, 0.775, 0.825, 0.875, 0.9, 0.925, 0.975]) AS q
FROM pfa WHERE cutoff = 1913 GROUP BY ALL;
CREATE OR REPLACE TABLE qf AS
SELECT a.method, a.id, a.t, a.h, a.y, a.f, t.tau, t.crps,
       greatest(a.f + r.q[list_position([0.025, 0.075, 0.1, 0.125, 0.175, 0.225, 0.25, 0.275, 0.325, 0.375, 0.425, 0.475, 0.525, 0.575, 0.625, 0.675, 0.725, 0.75, 0.775, 0.825, 0.875, 0.9, 0.925, 0.975]::DOUBLE[], t.tau)] * a.s, 0) AS q
FROM pfa a JOIN resq r USING (method, bin, week), taus t
WHERE a.cutoff = 1941;

-- per series and day: absolute error, squared error and CRPS (2 × mean pinball loss over the 20 levels)
CREATE OR REPLACE TABLE loss AS
SELECT method, id, t, h, any_value(y) AS y, any_value(f) AS f,
       abs(any_value(y) - any_value(f)) AS ae, (any_value(y) - any_value(f)) ^ 2 AS se,
       2 * avg((y - q) * (tau - (y < q)::INT)) FILTER (WHERE crps) AS crps
FROM qf GROUP BY method, id, t, h;
CREATE OR REPLACE TABLE cover AS
SELECT method,
       avg((y BETWEEN lo50 AND hi50)::INT) AS cov50, avg((y BETWEEN lo80 AND hi80)::INT) AS cov80, avg((y BETWEEN lo95 AND hi95)::INT) AS cov95
FROM (SELECT method, id, t, any_value(y) AS y,
             max(q) FILTER (WHERE tau = 0.25) AS lo50, max(q) FILTER (WHERE tau = 0.75) AS hi50,
             max(q) FILTER (WHERE tau = 0.1) AS lo80, max(q) FILTER (WHERE tau = 0.9) AS hi80,
             max(q) FILTER (WHERE tau = 0.025) AS lo95, max(q) FILTER (WHERE tau = 0.975) AS hi95
      FROM qf GROUP BY ALL)
GROUP BY 1;

-- WRMSSE on both windows (score.sql reads fc and the 'actual' window)
-- (with every seed alone as well, for the spread of single fits)
CREATE OR REPLACE TABLE fc AS SELECT method, id, t, f FROM pfa WHERE cutoff = 1941
UNION ALL SELECT 'seed ' || seed, id, t, f FROM fc_seeds WHERE cutoff = 1941;
.read score.sql
CREATE OR REPLACE TABLE wrmsse_test AS SELECT * FROM wrmsse_level;
CREATE OR REPLACE TABLE rmsse_test AS SELECT * FROM rmsse;
CREATE OR REPLACE TABLE actual AS SELECT * FROM agg WHERE t BETWEEN 1914 AND 1941;
CREATE OR REPLACE TABLE fc AS SELECT method, id, t, f FROM pfa WHERE cutoff = 1913
UNION ALL SELECT 'seed ' || seed, id, t, f FROM fc_seeds WHERE cutoff = 1913;
-- the public window is scored with the weights and scales of its own cutoff
CREATE OR REPLACE TABLE weight_test AS FROM weight;
CREATE OR REPLACE TABLE scale_test AS FROM scale;
CREATE OR REPLACE TABLE scale AS
WITH a AS (SELECT level, key, t, y, y - lag(y) OVER (PARTITION BY level, key ORDER BY t) AS dy,
                  min(CASE WHEN y <> 0 THEN t END) OVER (PARTITION BY level, key) AS t0
           FROM agg WHERE t <= 1913)
SELECT level, key, avg(dy * dy) AS scale FROM a WHERE t > t0 GROUP BY ALL;
CREATE OR REPLACE TABLE dollars_pub AS
SELECT s.id, sum(s.y * p.sell_price) AS usd
FROM sales s JOIN meta m USING (id) JOIN cal c ON c.t = s.t
JOIN prices p ON p.store_id = m.store_id AND p.item_id = m.item_id AND p.wm_yr_wk = c.wm_yr_wk
WHERE s.t BETWEEN 1886 AND 1913 GROUP BY 1;
CREATE OR REPLACE TABLE weight AS SELECT level, key, usd, usd / sum(usd) OVER (PARTITION BY level) AS w
FROM (SELECT level, key, sum(coalesce(d.usd, 0)) AS usd FROM lvl l LEFT JOIN dollars_pub d USING (id) GROUP BY ALL);
.read score.sql
CREATE OR REPLACE TABLE wrmsse_public AS SELECT * FROM wrmsse_level;
CREATE OR REPLACE TABLE weight AS FROM weight_test;
CREATE OR REPLACE TABLE scale AS FROM scale_test;
CREATE OR REPLACE TABLE actual AS SELECT * FROM agg WHERE t BETWEEN 1942 AND 1969;

-- the daily WRMSSE part of every aggregate: weight/12 × squared error / scale
CREATE OR REPLACE TABLE fc AS SELECT method, id, t, f FROM pfa WHERE cutoff = 1941;
CREATE OR REPLACE TABLE fc_agg AS SELECT f.method, l.level, l.key, f.t, sum(f.f) AS f FROM fc f JOIN lvl l USING (id) GROUP BY ALL;
CREATE OR REPLACE TABLE wse AS
SELECT f.method, f.t, sum(w.w / 12 * (a.y - f.f) ^ 2 / s.scale) AS wse
FROM fc_agg f JOIN actual a USING (level, key, t) JOIN scale s USING (level, key) JOIN weight w USING (level, key)
GROUP BY ALL;

-- the summary per method
CREATE OR REPLACE TABLE methods AS
SELECT l.method, avg(ae) AS mae, avg(crps) AS crps, 100 * (sum(f) - sum(y)) / sum(y) AS bias,
       any_value(w.wrmsse) AS wrmsse, any_value(p.wrmsse) AS wrmsse_public,
       any_value(c.cov50) AS cov50, any_value(c.cov80) AS cov80, any_value(c.cov95) AS cov95
FROM loss l
JOIN (SELECT method, avg(wrmsse) AS wrmsse FROM wrmsse_test GROUP BY 1) w USING (method)
JOIN (SELECT method, avg(wrmsse) AS wrmsse FROM wrmsse_public GROUP BY 1) p USING (method)
JOIN cover c USING (method)
GROUP BY 1;

-- Diebold–Mariano, pooled: the daily mean loss over all series (MAE, CRPS) and
-- the daily WRMSSE part, tree model against each baseline over the 28 test days.
-- The losses go in as forecasts against an actual of 0 with absolute loss, so
-- the test compares exactly these loss series. Newey–West over a week (horizon 7).
CREATE OR REPLACE TABLE daily AS
SELECT method, t, avg(ae) AS mae, avg(crps) AS crps FROM loss GROUP BY ALL;
CREATE OR REPLACE TABLE daily AS SELECT d.*, w.wse FROM daily d JOIN wse w USING (method, t);
CREATE OR REPLACE TABLE dm_pooled AS
WITH x AS (UNPIVOT daily ON mae, crps, wse INTO NAME metric VALUE v),
     p AS (SELECT a.metric, b.method AS baseline, a.t, a.v AS tree, b.v AS base
           FROM x a JOIN x b ON a.metric = b.metric AND a.t = b.t AND a.method = 'perpetual' AND b.method <> 'perpetual')
SELECT metric, baseline, avg(tree) AS tree_loss, avg(base) AS baseline_loss, 100 * (1 - avg(tree) / avg(base)) AS gain_pct,
       diebold_mariano_agg(0.0, tree, base, {'loss': 'absolute', 'horizon': 7, 'var_estimator': 'bartlett'} ORDER BY t) AS dm
FROM p GROUP BY ALL;

-- Diebold–Mariano per series (level 12): absolute error, CRPS and squared error
-- (each series' RMSSE scale is a constant, so its squared error decides its RMSSE)
CREATE OR REPLACE TABLE dm_series AS
WITH p AS (SELECT a.id, b.method AS baseline, a.t, a.y, a.f AS f1, b.f AS f2, a.crps AS c1, b.crps AS c2,
                  a.ae AS ae1, b.ae AS ae2, a.se AS se1, b.se AS se2
           FROM loss a JOIN loss b ON a.id = b.id AND a.t = b.t AND a.method = 'perpetual' AND b.method <> 'perpetual')
SELECT id, baseline,
       avg(ae1) - avg(ae2) AS d_mae, avg(c1) - avg(c2) AS d_crps, avg(se1) - avg(se2) AS d_se,
       (diebold_mariano_agg(y, f1, f2, {'loss': 'absolute', 'horizon': 7, 'var_estimator': 'bartlett'} ORDER BY t)).p_value AS p_mae,
       (diebold_mariano_agg(0.0, c1, c2, {'loss': 'absolute', 'horizon': 7, 'var_estimator': 'bartlett'} ORDER BY t)).p_value AS p_crps,
       (diebold_mariano_agg(y, f1, f2, {'loss': 'squared', 'horizon': 7, 'var_estimator': 'bartlett'} ORDER BY t)).p_value AS p_se
FROM p GROUP BY ALL;

-- the Model Confidence Set over all methods, per pooled loss
CREATE OR REPLACE TABLE mcs AS
WITH x AS (UNPIVOT daily ON mae, crps, wse INTO NAME metric VALUE v)
SELECT metric, (SELECT list(m ORDER BY m) FROM (SELECT DISTINCT method AS m FROM daily)) AS models,
       model_confidence_set_agg(v, {'seed': 7} ORDER BY t) AS mcs
FROM (SELECT metric, t, list(v ORDER BY method) AS v FROM x GROUP BY ALL) GROUP BY 1;
