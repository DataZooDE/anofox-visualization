-- #1341: the M5 data, daily, as in the competition: 30,490 item×store series,
-- d_1–d_1941 from sales_train_evaluation.csv and the test days d_1942–d_1969
-- from sales_test_evaluation.csv. getvariable('m5_dir') is the folder of the CSVs.
SET threads = 16;
CREATE OR REPLACE TABLE cal AS SELECT date::DATE AS ds, wm_yr_wk, weekday, wday, month, year, nullif(event_name_1,'NA') AS event_name_1, nullif(event_type_1,'NA') AS event_type_1, nullif(event_name_2,'NA') AS event_name_2, nullif(event_type_2,'NA') AS event_type_2, snap_CA, snap_TX, snap_WI,
  (date::DATE - DATE '2011-01-28')::INT AS t FROM read_csv(getvariable('m5_dir') || '/calendar.csv', nullstr='NA', all_varchar=false);
CREATE OR REPLACE TABLE prices AS SELECT * FROM read_csv(getvariable('m5_dir') || '/sell_prices.csv');
CREATE OR REPLACE TABLE meta AS SELECT item_id || '_' || store_id AS id, item_id, dept_id, cat_id, store_id, state_id FROM read_csv(getvariable('m5_dir') || '/sales_train_evaluation.csv');
CREATE OR REPLACE TABLE sales AS
WITH tr AS (UNPIVOT (SELECT item_id || '_' || store_id AS id, * EXCLUDE (item_id, dept_id, cat_id, store_id, state_id) FROM read_csv(getvariable('m5_dir') || '/sales_train_evaluation.csv')) ON COLUMNS('^d_') INTO NAME d VALUE y),
te AS (UNPIVOT (SELECT item_id || '_' || store_id AS id, * EXCLUDE (item_id, dept_id, cat_id, store_id, state_id) FROM read_csv(getvariable('m5_dir') || '/sales_test_evaluation.csv')) ON COLUMNS('^d_') INTO NAME d VALUE y)
SELECT id, replace(d, 'd_', '')::SMALLINT AS t, y::INT AS y FROM (SELECT * FROM tr UNION ALL SELECT * FROM te) ORDER BY id, t;
