-- One-time, read-write setup for `serve --dashboards … --init this-file`.
-- In production this is where you ATTACH your live source and expose read-only
-- VIEWS the dashboards read from — e.g.:
--   ATTACH 'md:my_db' AS live;                     -- MotherDuck
--   ATTACH '' AS pg (TYPE postgres, ...);          -- PostgreSQL
--   CREATE VIEW sales AS SELECT region, channel, n FROM live.sales;
-- Here we just seed a demo table.
CREATE OR REPLACE TABLE sales AS SELECT * FROM (VALUES
  ('EU','app',30),('EU','web',22),('EU','api',12),
  ('US','app',41),('US','web',28),('US','api', 9),
  ('APAC','app',18),('APAC','web',25),('APAC','api',14)
) t(region, channel, n);

-- A monthly series for the live-forecast dashboard.
CREATE OR REPLACE TABLE ts AS
  SELECT 'a' AS series, DATE '2022-01-01' + INTERVAL (i) MONTH AS ds,
         round(50 + 10*sin(i/2.0) + i, 1) AS y
  FROM range(0, 24) t(i);

-- M5 analytics (materialised once here; served read-only). Forecast MFLES + MSTL,
-- an MSTL decomposition, and a 12-month-holdout backtest across the M5 category
-- series. Needs anofox_forecast + web/m5_monthly.parquet (run serve from repo root).
LOAD anofox_forecast;
CREATE OR REPLACE TABLE an_cat AS
  SELECT split_part(series,'_',1) AS category, ds, sum(y) AS y
  FROM read_parquet('web/m5_monthly.parquet') GROUP BY 1,2;
CREATE OR REPLACE TABLE an_fc AS
  SELECT 'MFLES' AS method, category, ds, round(yhat,0) AS yhat
    FROM ts_forecast_by('an_cat', category, ds, y, 'MFLES', 12, '1mo', MAP{'seasonal_period':'12'})
  UNION ALL SELECT 'MSTL', category, ds, round(yhat,0)
    FROM ts_forecast_by('an_cat', category, ds, y, 'MSTL', 12, '1mo', MAP{'seasonal_period':'12'});
CREATE OR REPLACE TABLE an_decomp AS
  WITH d AS (SELECT category, generate_subscripts(trend,1) AS rn, unnest(trend) AS trend
             FROM ts_mstl_decomposition_by('an_cat', category, ds, y, MAP{'periods':'[12]'})),
       o AS (SELECT category, ds, y, row_number() OVER (PARTITION BY category ORDER BY ds) AS rn FROM an_cat),
       j AS (SELECT o.category, o.ds, o.y AS observed, d.trend, o.y - d.trend AS detrended, month(o.ds) AS mo
             FROM o JOIN d USING (category, rn)),
       s AS (SELECT category, mo, avg(detrended) AS smean FROM j GROUP BY 1,2),
       sc AS (SELECT category, mo, smean - avg(smean) OVER (PARTITION BY category) AS seasonal FROM s)
  SELECT j.category, j.ds, round(j.observed,0) AS observed, round(j.trend,0) AS trend,
         round(sc.seasonal,0) AS seasonal, round(j.observed - j.trend - sc.seasonal,0) AS remainder
  FROM j JOIN sc USING (category, mo);
CREATE OR REPLACE TABLE an_train AS SELECT * FROM an_cat WHERE ds <= (SELECT max(ds) FROM an_cat)-INTERVAL 12 MONTH;
CREATE OR REPLACE TABLE an_bt AS
  SELECT 'MFLES' AS method, f.category, f.ds, round(f.yhat,0) AS predicted, t.y AS actual
    FROM ts_forecast_by('an_train', category, ds, y, 'MFLES', 12, '1mo', MAP{'seasonal_period':'12'}) f
    JOIN an_cat t ON f.category=t.category AND f.ds=t.ds
  UNION ALL SELECT 'MSTL', f.category, f.ds, round(f.yhat,0), t.y
    FROM ts_forecast_by('an_train', category, ds, y, 'MSTL', 12, '1mo', MAP{'seasonal_period':'12'}) f
    JOIN an_cat t ON f.category=t.category AND f.ds=t.ds;
CREATE OR REPLACE TABLE an_scores AS
  SELECT category, arg_min(method, mae) AS winner,
         min(mae) FILTER (WHERE method='MFLES') AS mfles_mae, min(mae) FILTER (WHERE method='MSTL') AS mstl_mae,
         min(rmse) FILTER (WHERE method='MFLES') AS mfles_rmse, min(rmse) FILTER (WHERE method='MSTL') AS mstl_rmse
  FROM (SELECT category, method, round(avg(abs(actual-predicted)),0) AS mae,
               round(sqrt(avg(pow(actual-predicted,2))),0) AS rmse FROM an_bt GROUP BY 1,2) m
  GROUP BY category;
