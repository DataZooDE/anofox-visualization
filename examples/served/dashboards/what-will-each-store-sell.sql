-- What will each store sell in the next four weeks? — compares a forecast
-- run with the sales so far. The runs live in the server's data folder
-- (options.data), one directory per run; the run is picked by the `run`
-- variable (options.path_vars), so a new run can be opened as soon as it is
-- written: /d/what-will-each-store-sell?v.run=<run id>.

SELECT 'What will each store sell in the next four weeks?'::DASHBOARD_TITLE;
SELECT 'en-GB'::LOCALE;
SELECT max(loaded_at)::ASOF, 'stale=8d' FROM sales;

SELECT 'Choose'::GROUP;
SELECT run_id AS run ::DROPDOWN, 'Forecast run'::LABEL
FROM read_parquet(getvariable('anofox_data') || '/runs/*/run.parquet')
ORDER BY created_at DESC, run_id;
SELECT store ::DROPDOWN, 'Store'::LABEL FROM stores ORDER BY region, store;
SELECT 1::ENDGROUP;

CREATE OR REPLACE TEMP VIEW fc AS
SELECT * FROM read_parquet(getvariable('anofox_data') || '/runs/' || getvariable('run') || '/forecast.parquet');
CREATE OR REPLACE TEMP VIEW wk AS
SELECT store, date_trunc('week', day)::DATE AS week, sum(revenue) AS revenue
FROM sales GROUP BY ALL;

SELECT 'Next four weeks, all stores'::GROUP;
SELECT sum(yhat)::MONEY,
       (SELECT sum(revenue) FROM wk WHERE week > (SELECT max(week) FROM wk) - 28)::DELTA,
       'vs the last four weeks'::DELTA_LABEL,
       'Expected revenue'::LABEL
FROM fc;
SELECT count(*) FILTER (WHERE yhat < last4)::METRIC, 'Stores expected to sell less'::LABEL
FROM (SELECT store, sum(yhat) AS yhat FROM fc GROUP BY store)
JOIN (SELECT store, sum(revenue) AS last4 FROM wk WHERE week > (SELECT max(week) FROM wk) - 28 GROUP BY store)
USING (store);
SELECT 1::ENDGROUP;

SELECT 12::COL;
SELECT 360::HEIGHT;
SELECT week::XAXIS, kind::CATEGORY, revenue::LINECHART, '£'::YFORMAT,
       lo80::BAND_LOWER, hi80::BAND_UPPER, lo50::BAND_LOWER, hi50::BAND_UPPER,
       (SELECT getvariable('store') || ': weekly revenue and the ' || replace(model, '-', ' ')
               || ' forecast from ' || strftime(cutoff, '%-d %b') || ' (bands: 80 / 50 %)'
        FROM read_parquet(getvariable('anofox_data') || '/runs/' || getvariable('run') || '/run.parquet'))::TITLE
FROM (
  SELECT week, 'Sales' AS kind, revenue, NULL::DOUBLE AS lo80, NULL::DOUBLE AS hi80,
         NULL::DOUBLE AS lo50, NULL::DOUBLE AS hi50
  FROM wk WHERE store = getvariable('store')
  UNION ALL
  SELECT week, 'Forecast', yhat, lo80, hi80, lo50, hi50
  FROM fc WHERE store = getvariable('store')
) ORDER BY kind DESC, week;

SELECT 12::COL;
SELECT f.region AS "Region", f.store AS "Store" ::TABLE,
       a.last4 AS "Last 4 weeks" ::MONEY,
       f.yhat AS "Next 4 weeks" ::MONEY,
       round(100 * (f.yhat / a.last4 - 1), 1) AS "Change %" ::TREND,
       'Expected revenue per store'::TITLE
FROM (SELECT region, store, sum(yhat) AS yhat FROM fc GROUP BY ALL) f
JOIN (SELECT store, sum(revenue) AS last4 FROM wk WHERE week > (SELECT max(week) FROM wk) - 28 GROUP BY store) a
USING (store)
ORDER BY 5;
