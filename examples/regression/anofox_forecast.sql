-- anofox_forecast → anofox_plot_prediction. Needs the anofox_forecast
-- community extension:
--   INSTALL anofox_forecast FROM community;
--   python3 examples/regression/render.py examples/regression/anofox_forecast.sql out/
LOAD anofox_forecast;

CREATE TABLE sales AS
SELECT store AS id, (DATE '2024-01-01' + INTERVAL (i) DAY)::TIMESTAMP AS ds,
       base + 0.4 * i + 6 * sin(i * 2 * pi() / 7) AS y
FROM (VALUES ('north', 50.0), ('south', 80.0)) s(store, base), range(70) r(i);

-- ts_forecast_by returns id, forecast_step, ds, yhat, yhat_lower, yhat_upper,
-- model_name. Union the history (y, no yhat) so the plot shows actuals then
-- the forecast fan; rows without y are the 'future' split, several ids
-- become panels.
SELECT 'forecast' AS name, anofox_plot_prediction(p, width := 720, height := 560) AS svg
FROM (
    SELECT id, ds, y, NULL::DOUBLE AS yhat, NULL::DOUBLE AS yhat_lower,
           NULL::DOUBLE AS yhat_upper, NULL AS model_name
    FROM sales WHERE ds >= TIMESTAMP '2024-02-01'
    UNION ALL
    SELECT id, ds, NULL, yhat, yhat_lower, yhat_upper, model_name
    FROM ts_forecast_by('sales', id, ds, y, 'AutoETS', 14, '1d')
) p;

-- One SVG per series instead of panels: GROUP BY the key.
SELECT 'forecast_' || id AS name, anofox_plot_prediction(f) AS svg
FROM ts_forecast_by('sales', id, ds, y, 'AutoETS', 14, '1d') f
GROUP BY id ORDER BY id;
