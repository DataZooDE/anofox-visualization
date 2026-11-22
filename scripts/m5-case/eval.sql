-- The M5 scoring (WRMSSE): the 12 aggregation levels, each series' scale and weight.
-- Checked against the official weights_evaluation.csv (all 42,840 weights agree to
-- 1e-12) and the organizers' Naive (1.752) and sNaive (0.847) scores.
-- The 12 M5 aggregation levels: every bottom series maps to one key per level.
CREATE OR REPLACE TABLE lvl AS
SELECT m.id, l.level, CASE l.level
   WHEN 1 THEN 'Total' WHEN 2 THEN state_id WHEN 3 THEN store_id WHEN 4 THEN cat_id WHEN 5 THEN dept_id
   WHEN 6 THEN state_id || '|' || cat_id WHEN 7 THEN state_id || '|' || dept_id WHEN 8 THEN store_id || '|' || cat_id
   WHEN 9 THEN store_id || '|' || dept_id WHEN 10 THEN item_id WHEN 11 THEN item_id || '|' || state_id ELSE item_id || '|' || store_id END AS key
FROM meta m, (SELECT unnest(range(1, 13)) AS level) l;
-- aggregated daily sales up to the last training day, and the 28 test days
CREATE OR REPLACE TABLE agg AS
SELECT l.level, l.key, s.t, sum(s.y)::DOUBLE AS y FROM sales s JOIN lvl l USING (id) GROUP BY ALL;
-- scale: mean squared one-step difference from the series' first non-zero day to d_1941
CREATE OR REPLACE TABLE scale AS
WITH a AS (SELECT level, key, t, y, y - lag(y) OVER (PARTITION BY level, key ORDER BY t) AS dy,
                  min(CASE WHEN y <> 0 THEN t END) OVER (PARTITION BY level, key) AS t0
           FROM agg WHERE t <= 1941)
SELECT level, key, avg(dy * dy) AS scale FROM a WHERE t > t0 GROUP BY ALL;
-- weights: dollar sales over d_1914–d_1941, each level sums to 1
CREATE OR REPLACE TABLE dollars AS
SELECT s.id, sum(s.y * p.sell_price) AS usd
FROM sales s JOIN meta m USING (id) JOIN cal c ON c.t = s.t
JOIN prices p ON p.store_id = m.store_id AND p.item_id = m.item_id AND p.wm_yr_wk = c.wm_yr_wk
WHERE s.t BETWEEN 1914 AND 1941 GROUP BY 1;
CREATE OR REPLACE TABLE weight AS
SELECT level, key, usd, usd / sum(usd) OVER (PARTITION BY level) AS w
FROM (SELECT level, key, sum(coalesce(d.usd, 0)) AS usd FROM lvl l LEFT JOIN dollars d USING (id) GROUP BY ALL);
-- the actuals of the test window per aggregate
CREATE OR REPLACE TABLE actual AS SELECT * FROM agg WHERE t BETWEEN 1942 AND 1969;
