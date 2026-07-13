-- @title Live forecast
-- @load anofox_forecast
-- @refresh 60
-- History + a 6-month forecast, computed live per request (read-only) via the
-- anofox_forecast extension. No CREATE/materialisation — the forecast function
-- is called inline, so it reflects the current data (bounded by --cache).
SELECT ds::XAXIS, phase::CATEGORY, val::LINECHART, 'History + 6-month forecast'::TITLE
FROM (
  SELECT ds, 'Actual' AS phase, y AS val FROM ts
  UNION ALL
  SELECT ds, 'Forecast', yhat
    FROM ts_forecast_by('ts', series, ds, y, 'SeasonalNaive', 6, '1mo', MAP{'seasonal_period':'12'})
) ORDER BY phase, ds;
