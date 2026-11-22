-- Features of the global model, one row per (series, origin, horizon).
-- Every feature uses data up to the origin only; the 28 days after it are the targets.
-- Origins every 28 days back from d_1941: the final model trains on the blocks
-- before d_1941 and forecasts d_1942–d_1969; the calibration model trains on the
-- blocks before d_1913 and forecasts d_1914–d_1941 (the public window).
LOAD anofox_forecast;
SET threads = 12;
SET memory_limit = '24GB';
SET preserve_insertion_order = false;

CREATE OR REPLACE TABLE origins AS SELECT 1941 - 28 * k AS o FROM range(0, 15) r(k);

-- rolling state at every day, per series (windows end at that day)
CREATE OR REPLACE TABLE roll AS
SELECT id, t, y AS last,
       avg(y) OVER (w ROWS BETWEEN 6 PRECEDING AND CURRENT ROW) AS m7,
       avg(y) OVER (w ROWS BETWEEN 27 PRECEDING AND CURRENT ROW) AS m28,
       avg(y) OVER (w ROWS BETWEEN 90 PRECEDING AND CURRENT ROW) AS m91,
       stddev_pop(y) OVER (w ROWS BETWEEN 27 PRECEDING AND CURRENT ROW) AS sd28,
       avg((y = 0)::INT) OVER (w ROWS BETWEEN 27 PRECEDING AND CURRENT ROW) AS zero28,
       t - max(CASE WHEN y > 0 THEN t END) OVER (w ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW) AS since_sale,
       t - min(CASE WHEN y > 0 THEN t END) OVER (w ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW) AS age
FROM sales WHERE t <= 1941
WINDOW w AS (PARTITION BY id ORDER BY t)
QUALIFY t IN (SELECT o FROM origins);

-- series features from anofox_forecast on the year before each origin
CREATE OR REPLACE TABLE sfeat AS
SELECT s.id, o.o, ts_features_agg(c.ds::TIMESTAMP, s.y::DOUBLE,
         ['mean', 'standard_deviation', 'variation_coefficient', 'quantile_0.9', 'maximum', 'skewness',
          'linear_trend_slope', 'autocorrelation_lag1', 'autocorrelation_lag7', 'longest_strike_below_mean',
          'mean_abs_change', 'permutation_entropy', 'number_peaks']) AS f
FROM origins o JOIN sales s ON s.t > o.o - 365 AND s.t <= o.o JOIN cal c ON c.t = s.t
GROUP BY ALL;
-- rounded to 12 decimals: permutation_entropy's last bits vary between identical
-- calls (DataZooDE/anofox-forecast#321), which would change every forecast of a rerun
CREATE OR REPLACE TABLE sf AS
SELECT id, o, round(f.mean, 12) AS fc_mean, round(f.standard_deviation, 12) AS fc_sd, round(f.variation_coefficient, 12) AS fc_cv,
       round(f."quantile_0.9", 12) AS fc_q90, round(f.maximum, 12) AS fc_max, round(f.skewness, 12) AS fc_skew, round(f.linear_trend_slope, 12) AS fc_trend,
       round(f.autocorrelation_lag1, 12) AS fc_acf1, round(f.autocorrelation_lag7, 12) AS fc_acf7, round(f.longest_strike_below_mean, 12) AS fc_strike,
       round(f.mean_abs_change, 12) AS fc_mac, round(f.permutation_entropy, 12) AS fc_pent, round(f.number_peaks, 12) AS fc_peaks
FROM sfeat;

-- prices per series and day, and the item's highest price up to each day
CREATE OR REPLACE TABLE price AS
SELECT m.id, c.t, p.sell_price AS price,
       max(p.sell_price) OVER (PARTITION BY m.id ORDER BY c.t ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW) AS price_max
FROM meta m JOIN prices p ON p.store_id = m.store_id AND p.item_id = m.item_id
JOIN cal c ON c.wm_yr_wk = p.wm_yr_wk
QUALIFY c.t >= 1100;

-- the rows: an item is on the shelf on a day when it has a price that week
CREATE OR REPLACE TABLE rows AS
SELECT s.id, o.o, s.t - o.o AS h, s.t, s.y,
       m.cat_id, m.dept_id, m.store_id, m.state_id
FROM origins o JOIN sales s ON s.t > o.o AND s.t <= o.o + 28
JOIN meta m USING (id)
SEMI JOIN price p ON p.id = s.id AND p.t = s.t;

-- each series' daily sales from d_1100 to d_1941 as one list (index 1 = d_1100)
CREATE OR REPLACE TABLE ys AS SELECT id, list(y ORDER BY t) AS ys FROM sales WHERE t >= 1100 AND t <= 1941 GROUP BY id;
CREATE OR REPLACE MACRO yat(ys, day) AS ys[day - 1099];

CREATE OR REPLACE TABLE ex AS
SELECT r.id, r.o, r.h, r.t, r.y,
       -- the latest days before the origin
       ro.last, ro.m7, ro.m28, ro.m91, ro.sd28, ro.zero28, coalesce(ro.since_sale, 999) AS since_sale, coalesce(ro.age, 0) AS age,
       -- the same weekday in the latest weeks before the origin
       yat(ys.ys, r.t - 7 * ((r.h + 6) // 7)) AS sw1,
       (yat(ys.ys, r.t - 7 * ((r.h + 6) // 7)) + yat(ys.ys, r.t - 7 * ((r.h + 6) // 7) - 7)
        + yat(ys.ys, r.t - 7 * ((r.h + 6) // 7) - 14) + yat(ys.ys, r.t - 7 * ((r.h + 6) // 7) - 21)) / 4.0 AS sw4,
       -- calendar of the target day
       c.wday, day(c.ds) AS mday, c.month,
       CASE c.event_type_1 WHEN 'Sporting' THEN 1 WHEN 'Cultural' THEN 2 WHEN 'National' THEN 3 WHEN 'Religious' THEN 4 ELSE 0 END AS event,
       CASE r.state_id WHEN 'CA' THEN c.snap_CA WHEN 'TX' THEN c.snap_TX ELSE c.snap_WI END AS snap,
       -- price on the target day against the origin's price and the highest so far
       p.price, p.price / nullif(po.price, 0) AS price_vs_origin, p.price / nullif(po.price_max, 0) AS price_vs_max,
       -- the series' group, as category codes
       CASE r.cat_id WHEN 'FOODS' THEN 0 WHEN 'HOBBIES' THEN 1 ELSE 2 END AS cat,
       CASE r.dept_id WHEN 'FOODS_1' THEN 0 WHEN 'FOODS_2' THEN 1 WHEN 'FOODS_3' THEN 2 WHEN 'HOBBIES_1' THEN 3
         WHEN 'HOBBIES_2' THEN 4 WHEN 'HOUSEHOLD_1' THEN 5 ELSE 6 END AS dept,
       CASE r.store_id WHEN 'CA_1' THEN 0 WHEN 'CA_2' THEN 1 WHEN 'CA_3' THEN 2 WHEN 'CA_4' THEN 3 WHEN 'TX_1' THEN 4
         WHEN 'TX_2' THEN 5 WHEN 'TX_3' THEN 6 WHEN 'WI_1' THEN 7 WHEN 'WI_2' THEN 8 ELSE 9 END AS store,
       sf.* EXCLUDE (id, o)
FROM rows r
JOIN roll ro ON ro.id = r.id AND ro.t = r.o
JOIN ys ON ys.id = r.id
JOIN cal c ON c.t = r.t
JOIN price p ON p.id = r.id AND p.t = r.t
LEFT JOIN price po ON po.id = r.id AND po.t = r.o
JOIN sf ON sf.id = r.id AND sf.o = r.o;

SELECT count(*) AS rows, count(DISTINCT o) AS origins, min(o), max(o), count(DISTINCT id) AS series FROM ex;
