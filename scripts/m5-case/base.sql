-- The baselines from anofox_forecast at the final cutoff d_1941: forecasts of the
-- test days d_1942–d_1969. "Mean" is SMA over 28 days (no plain mean method).
LOAD anofox_forecast;
SET threads = 16;
CREATE OR REPLACE TABLE train_long AS SELECT s.id, c.ds::TIMESTAMP AS ds, s.y::DOUBLE AS y FROM sales s JOIN cal c USING (t) WHERE s.t <= 1941;
CREATE OR REPLACE TABLE base AS
SELECT 'naive' AS method, * FROM ts_forecast_by('train_long', id, ds, y, 'Naive', 28, '1d', MAP{})
UNION ALL BY NAME SELECT 'sma28' AS method, * FROM ts_forecast_by('train_long', id, ds, y, 'SMA', 28, '1d', MAP{'window': '28'})
UNION ALL BY NAME SELECT 'snaive7' AS method, * FROM ts_forecast_by('train_long', id, ds, y, 'SeasonalNaive', 28, '1d', MAP{'seasonal_period': '7'});
