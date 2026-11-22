LOAD anofox_ml;
SET threads = 16;
SET memory_limit = '24GB';
.read feats.sql
-- permutation importance of the calibration model, out of sample (d_1914–d_1941):
-- how much the MAE rises when one feature is shuffled across 200,000 rows.
-- Shuffling makes a few combinations the model never saw (a trend slope from
-- another item) and the log link turns them into absurd forecasts, so every
-- forecast here is capped at the item's highest day of the past year (fx[27])
CREATE OR REPLACE TABLE imp_s AS
SELECT row_number() OVER (ORDER BY id, t) AS rn, feats(ex) AS fx, y::DOUBLE AS y
FROM (SELECT * FROM ex WHERE o = 1913 ORDER BY hash(id, t), id, t LIMIT 200000) ex;  -- a fixed sample
-- the forecast is the mean of the five seeds' models, as in the case
CREATE OR REPLACE TABLE imp_base AS
SELECT avg(abs(y - least(f, cap))) AS mae
FROM (SELECT rn, any_value(y) AS y, any_value(fx[27]) AS cap, avg(exp(ml_predict(m.m, fx))) AS f
      FROM imp_s, models m WHERE m.cutoff = 1913 GROUP BY rn);
CREATE OR REPLACE TABLE imp AS
WITH p AS (SELECT k, rn, row_number() OVER (PARTITION BY k ORDER BY hash(rn, k)) AS rn2
           FROM imp_s, range(1, 36) r(k)),
x AS (SELECT p.k, p.rn, a.y, a.fx[27] AS cap, list_concat(a.fx[1:p.k - 1], [b.fx[p.k]], a.fx[p.k + 1:]) AS fx
      FROM p JOIN imp_s a ON a.rn = p.rn JOIN imp_s b ON b.rn = p.rn2)
SELECT k, avg(abs(y - least(f, cap))) AS mae
FROM (SELECT k, rn, any_value(y) AS y, any_value(cap) AS cap, avg(exp(ml_predict(m.m, fx))) AS f
      FROM x, models m WHERE m.cutoff = 1913 GROUP BY k, rn)
GROUP BY k;
SELECT n.feature, n.source, round(100 * (i.mae / b.mae - 1), 2) AS mae_rise_pct
FROM imp i JOIN fnames n USING (k), imp_base b ORDER BY mae_rise_pct DESC;
