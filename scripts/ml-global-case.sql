-- #1341: forecasting many series with one global model (anofox_ml) on M5.
-- Run by scripts/gen-ml-global-case.sh in web/; writes web/data/ml-global/*.csv.
-- Settings are fixed up front: no tuning on the held-out months.
LOAD anofox_ml;
SET threads = 8;

-- One store's items (M5, Walmart, CA_1), monthly, each series from its first
-- month to April 2015 with the missing months as 0 sales.
CREATE OR REPLACE TABLE raw AS SELECT series, ds, y FROM read_parquet('m5_monthly.parquet') WHERE series LIKE '%\_CA\_1' ESCAPE '\';
CREATE OR REPLACE TABLE g AS
WITH b AS (SELECT series, min(ds) AS s0 FROM raw GROUP BY 1),
grid AS (SELECT b.series, unnest(generate_series(b.s0, DATE '2015-04-01', INTERVAL 1 MONTH))::DATE AS ds FROM b)
SELECT grid.series, grid.ds, coalesce(r.y, 0) AS y,
       split_part(grid.series, '_', 1) AS cat, split_part(grid.series, '_', 1) || '_' || split_part(grid.series, '_', 2) AS dept
FROM grid LEFT JOIN raw r ON r.series = grid.series AND r.ds = grid.ds;

-- the forecast origin (last month known) and the held-out months after it
SET VARIABLE cutoff = DATE '2014-10-01';

-- features at an origin month, per series (window over the series' months)
CREATE OR REPLACE TABLE f AS
SELECT series, ds AS origin, cat, dept, y AS l0,
       coalesce(lag(y, 1) OVER w, 0) AS l1, coalesce(lag(y, 2) OVER w, 0) AS l2,
       avg(y) OVER (w ROWS BETWEEN 2 PRECEDING AND CURRENT ROW) AS ma3,
       avg(y) OVER (w ROWS BETWEEN 5 PRECEDING AND CURRENT ROW) AS ma6,
       avg(y) OVER (w ROWS BETWEEN 11 PRECEDING AND CURRENT ROW) AS ma12,
       avg((y = 0)::INT) OVER (w ROWS BETWEEN 11 PRECEDING AND CURRENT ROW) AS zero12,
       row_number() OVER w AS age
FROM g WINDOW w AS (PARTITION BY series ORDER BY ds);

-- one row per (series, origin, horizon): the target h months ahead, the
-- same month a year before it; sales scaled by the series' recent level
CREATE OR REPLACE TABLE ex AS
SELECT f.*, month(f.origin + INTERVAL (f.h) MONTH) AS mon,
       1 + f.ma12 AS scale, t.y AS target, coalesce(s.y, f.ma12) AS sn
FROM (SELECT * FROM f, range(1, 7) r(h)) f
LEFT JOIN g t ON t.series = f.series AND t.ds = f.origin + INTERVAL (f.h) MONTH
LEFT JOIN g s ON s.series = f.series AND s.ds = f.origin + INTERVAL (f.h - 12) MONTH;

CREATE OR REPLACE MACRO feats(e) AS [e.l0 / e.scale, e.l1 / e.scale, e.l2 / e.scale, e.ma3 / e.scale, e.ma6 / e.scale,
  e.ma12 / e.scale, e.sn / e.scale, e.zero12, least(e.age, 36)::DOUBLE, e.h::DOUBLE, e.mon::DOUBLE,
  CASE e.cat WHEN 'FOODS' THEN 0 WHEN 'HOBBIES' THEN 1 ELSE 2 END::DOUBLE];
CREATE OR REPLACE TABLE fnames AS SELECT unnest(['last month', 'a month before', 'two months before', 'mean of 3 months',
  'mean of 6 months', 'mean of 12 months', 'same month last year', 'share of months without sales', 'months on sale',
  'horizon', 'calendar month', 'category']) AS feature, unnest(range(12)) AS k;

-- training: origins in the two years before the cutoff whose target is known
CREATE OR REPLACE TABLE train AS
SELECT * FROM ex WHERE origin >= getvariable('cutoff') - INTERVAL 24 MONTH
  AND origin + INTERVAL (h) MONTH <= getvariable('cutoff') AND target IS NOT NULL AND age >= 3;
CREATE OR REPLACE TABLE test AS
SELECT * FROM ex WHERE origin = getvariable('cutoff') AND target IS NOT NULL;

-- the global model: one LGBM for all series
CREATE OR REPLACE TABLE gm AS
SELECT ml_fit_agg('LgbmRegressor', target / scale, feats(train),
       '{"n_estimators": 400, "learning_rate": 0.05, "num_leaves": 31, "min_child_samples": 40, "seed": 7, "n_jobs": 1}' ORDER BY series, origin, h) AS m
FROM train;

-- conformal intervals (split conformal, out of sample): the same model fitted
-- on the months up to six before the cutoff, its scaled errors on the six
-- months after that give, per horizon, the 80 % and 95 % margins; they go
-- around the final model's forecast, times the item's scale
CREATE OR REPLACE TABLE cal_model AS
SELECT ml_fit_agg('LgbmRegressor', target / scale, feats(train),
       '{"n_estimators": 400, "learning_rate": 0.05, "num_leaves": 31, "min_child_samples": 40, "seed": 7, "n_jobs": 1}' ORDER BY series, origin, h) AS m
FROM train WHERE origin + INTERVAL (h) MONTH <= getvariable('cutoff') - INTERVAL 6 MONTH;
CREATE OR REPLACE TABLE margins AS
SELECT c.h, count(*) AS n,
       ml_conformal_margin(c.target / c.scale, ml_predict(m.m, feats(c)), 0.2) AS m80,
       ml_conformal_margin(c.target / c.scale, ml_predict(m.m, feats(c)), 0.05) AS m95
FROM train c, cal_model m
WHERE c.origin + INTERVAL (c.h) MONTH > getvariable('cutoff') - INTERVAL 6 MONTH
  AND c.origin <= getvariable('cutoff') - INTERVAL 6 MONTH
GROUP BY c.h;

-- the local models: the same features, one LGBM per series on its own rows
CREATE OR REPLACE TABLE loc_src AS
SELECT series, origin, h, target / scale AS yt, scale,
       fx[1] AS x0, fx[2] AS x1, fx[3] AS x2, fx[4] AS x3, fx[5] AS x4, fx[6] AS x5,
       fx[7] AS x6, fx[8] AS x7, fx[9] AS x8, fx[10] AS x9, fx[11] AS x10, fx[12] AS x11
FROM (SELECT x.*, feats(x) AS fx FROM (SELECT * FROM train UNION ALL BY NAME SELECT * REPLACE (NULL::DOUBLE AS target) FROM test) x);
CREATE OR REPLACE TABLE loc AS
SELECT * FROM ml_fit_predict_by('loc_src', series, yt, [x0, x1, x2, x3, x4, x5, x6, x7, x8, x9, x10, x11], 'LgbmRegressor',
       params := '{"n_estimators": 100, "learning_rate": 0.1, "num_leaves": 7, "min_child_samples": 3, "seed": 7}');

-- the four forecasts of every held-out month; a series too short for its own
-- model (local) falls back to the moving average
CREATE OR REPLACE TABLE fc AS
SELECT t.series, (t.origin + INTERVAL (t.h) MONTH)::DATE AS ds, t.h, t.cat, t.age, t.zero12, t.target AS actual,
       greatest(ml_predict((SELECT m FROM gm), feats(t)), 0) * t.scale AS global_model,
       coalesce(greatest(l.prediction, 0) * t.scale, t.ma3) AS local_models,
       t.sn AS seasonal_naive, t.ma3 AS moving_average
FROM test t LEFT JOIN loc l ON l.series = t.series AND l.origin = t.origin AND l.h = t.h;
CREATE OR REPLACE TABLE pi AS
SELECT f.series, f.ds, f.h, f.actual, f.global_model,
       greatest(f.global_model - m.m80 * t.scale, 0) AS lo80, f.global_model + m.m80 * t.scale AS hi80,
       greatest(f.global_model - m.m95 * t.scale, 0) AS lo95, f.global_model + m.m95 * t.scale AS hi95
FROM fc f JOIN test t ON t.series = f.series AND (t.origin + INTERVAL (t.h) MONTH)::DATE = f.ds JOIN margins m ON m.h = f.h;
-- how often the actual fell inside, per horizon and overall, against the aim
COPY (SELECT h::VARCHAR AS horizon, round(100 * avg((actual BETWEEN lo80 AND hi80)::INT), 1) AS covered80,
             round(100 * avg((actual BETWEEN lo95 AND hi95)::INT), 1) AS covered95,
             round(avg(hi80 - lo80), 1) AS width80, round(avg(hi95 - lo95), 1) AS width95
      FROM pi GROUP BY h
      UNION ALL SELECT 'all', round(100 * avg((actual BETWEEN lo80 AND hi80)::INT), 1), round(100 * avg((actual BETWEEN lo95 AND hi95)::INT), 1),
             round(avg(hi80 - lo80), 1), round(avg(hi95 - lo95), 1) FROM pi
      ORDER BY horizon) TO 'data/ml-global/intervals.csv';
CREATE OR REPLACE TABLE long AS
UNPIVOT fc ON global_model, local_models, seasonal_naive, moving_average INTO NAME method VALUE forecast;
CREATE OR REPLACE MACRO label(m) AS CASE m WHEN 'global_model' THEN 'one global model (anofox_ml)'
  WHEN 'local_models' THEN 'a model per series (anofox_ml)' WHEN 'seasonal_naive' THEN 'same month last year'
  ELSE 'mean of the last 3 months' END;

COPY (SELECT method, label(method) AS label, round(100 * sum(abs(forecast - actual)) / sum(actual), 1) AS wape,
             round(avg(abs(forecast - actual)), 2) AS mae, round(100 * (sum(forecast) - sum(actual)) / sum(actual), 1) AS bias
      FROM long GROUP BY ALL ORDER BY wape) TO 'data/ml-global/methods.csv';
COPY (WITH seg AS (
        SELECT 'all items' AS segment, * FROM long
        UNION ALL SELECT CASE WHEN age < 30 THEN 'new items (on sale < 2½ years)' ELSE 'items on sale 2½ years or more' END, * FROM long
        UNION ALL SELECT CASE WHEN zero12 >= 0.25 THEN 'sold in fewer than 9 of 12 months' ELSE 'sold in 9 months or more' END, * FROM long
        UNION ALL SELECT lower(cat), * FROM long)
      SELECT segment, method, label(method) AS label, count(DISTINCT series) AS series,
             round(100 * sum(abs(forecast - actual)) / sum(actual), 1) AS wape
      FROM seg GROUP BY ALL ORDER BY segment, wape) TO 'data/ml-global/segments.csv';
COPY (SELECT ds, 'actual' AS method, 'actual' AS label, round(sum(y)) AS units FROM g
        WHERE ds >= DATE '2013-05-01' GROUP BY ALL
      UNION ALL SELECT ds, method, label(method), round(sum(forecast)) FROM long GROUP BY ALL
      ORDER BY ds, method) TO 'data/ml-global/monthly.csv';
-- six items to look at: the best seller, a new item, one that sells in few
-- months, the best seller of each other category, and a typical one
CREATE OR REPLACE TABLE picks AS
WITH tot AS (SELECT series, any_value(cat) AS cat, any_value(age) AS age, any_value(zero12) AS z, sum(actual) AS s FROM fc GROUP BY 1)
SELECT * FROM (
  (SELECT 'best seller' AS why, series FROM tot ORDER BY s DESC LIMIT 1)
  UNION ALL (SELECT 'new item', series FROM tot WHERE age < 18 ORDER BY s DESC LIMIT 1)
  UNION ALL (SELECT 'sells in few months', series FROM tot WHERE z >= 0.5 ORDER BY s DESC LIMIT 1)
  UNION ALL (SELECT 'household best seller', series FROM tot WHERE cat = 'HOUSEHOLD' ORDER BY s DESC LIMIT 1)
  UNION ALL (SELECT 'hobbies best seller', series FROM tot WHERE cat = 'HOBBIES' ORDER BY s DESC LIMIT 1)
  UNION ALL (SELECT 'a typical item', series FROM tot ORDER BY abs(s - (SELECT median(s) FROM tot)) LIMIT 1));
COPY (SELECT p.why, g.series, g.ds, 'actual' AS method, g.y AS units FROM picks p JOIN g USING (series) WHERE g.ds >= DATE '2013-05-01'
      UNION ALL SELECT p.why, l.series, l.ds, l.method, round(l.forecast, 1) FROM picks p JOIN long l USING (series)
      UNION ALL SELECT p.why, i.series, i.ds, unnest(['lo80', 'hi80', 'lo95', 'hi95']),
             round(unnest([i.lo80, i.hi80, i.lo95, i.hi95]), 1) FROM picks p JOIN pi i USING (series)
      ORDER BY 1, 2, 3, 4) TO 'data/ml-global/examples.csv';
COPY (SELECT n.feature, round(i.v, 4) AS importance
      FROM (SELECT unnest(ml_feature_importances(m)) AS v, unnest(range(12)) AS k FROM gm) i JOIN fnames n USING (k)
      ORDER BY importance DESC) TO 'data/ml-global/importance.csv';
COPY (SELECT (SELECT count(DISTINCT series) FROM fc) AS series, (SELECT count(*) FROM train) AS training_rows,
             (SELECT min(ds) FROM fc) AS first_month, (SELECT max(ds) FROM fc) AS last_month,
             (SELECT extension_version FROM duckdb_extensions() WHERE extension_name = 'anofox_ml') AS anofox_ml,
             version() AS duckdb) TO 'data/ml-global/provenance.csv';
