LOAD anofox_forecast;
SET threads = 16;
-- the same baselines at the calibration cutoff d_1913: forecasts of d_1914–d_1941
CREATE OR REPLACE TABLE train_cal AS SELECT * FROM train_long WHERE ds <= (SELECT ds FROM cal WHERE t = 1913)::TIMESTAMP;
CREATE OR REPLACE TABLE base_cal AS
SELECT 'naive' AS method, * FROM ts_forecast_by('train_cal', id, ds, y, 'Naive', 28, '1d', MAP{})
UNION ALL BY NAME SELECT 'sma28' AS method, * FROM ts_forecast_by('train_cal', id, ds, y, 'SMA', 28, '1d', MAP{'window': '28'})
UNION ALL BY NAME SELECT 'snaive7' AS method, * FROM ts_forecast_by('train_cal', id, ds, y, 'SeasonalNaive', 28, '1d', MAP{'seasonal_period': '7'});
