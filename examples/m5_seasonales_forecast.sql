-- ── M5 monthly sales — 12-month SeasonalES forecast ──────────────────────────
-- A pure-SQL duckplot dashboard powered by the anofox-forecast DuckDB extension.
--
-- Run it against a DuckDB that has the extension (duckplot `serve` / escurel /
-- the DuckDB CLI). The extension is native code, so the browser DuckDB-Wasm
-- build can't load it — use a native backend for this dashboard.
--
-- Point read_parquet() at your M5 monthly file (item_id, ds, y). Here we use the
-- anofox-forecast benchmark extract.
--
-- Note: keep the charts/table as plain SELECT/UNION (no WITH/CTE) — duckplot's
-- ::ROLE rewriter reads the first SELECT..FROM, which a CTE would hide. Do the
-- heavy lifting in CREATE TABLE setup steps instead (as below).

INSTALL anofox_forecast;   -- first run only; or LOAD '<path>/anofox_forecast.duckdb_extension'
LOAD anofox_forecast;

-- Monthly totals per category (FOODS / HOBBIES / HOUSEHOLD)
CREATE OR REPLACE TABLE cat_monthly AS
  SELECT split_part(item_id, '_', 1) AS category, ds::DATE AS ds, sum(y) AS y
  FROM read_parquet('m5_monthly_train.parquet')
  WHERE ds >= DATE '2011-02-01'   -- drop the partial first month (M5 starts Jan 29)
  GROUP BY 1, 2;

-- 12-month SeasonalES forecast (annual seasonality on monthly data)
CREATE OR REPLACE TABLE fc AS
  SELECT category, ds, round(yhat, 0) AS yhat, round(yhat_lower, 0) AS lo, round(yhat_upper, 0) AS hi
  FROM ts_forecast_by('cat_monthly', category, ds, y,
                      'SeasonalES', 12, '1mo', MAP{'seasonal_period': '12'});

-- History + forecast in one long table (one row per category-month)
CREATE OR REPLACE TABLE series AS
  SELECT category, ds, y AS actual, NULL::DOUBLE AS yhat, NULL::DOUBLE AS lo, NULL::DOUBLE AS hi FROM cat_monthly
  UNION ALL
  SELECT category, ds, NULL, yhat, lo, hi FROM fc;

-- Per-category summary (last actual, next & 12-month forecast, YoY change)
CREATE OR REPLACE TABLE summary AS
  SELECT h.category, h.last_actual, c.next_fc, c.fc_total,
         round(100.0 * (c.fc_total - h.actual_12) / h.actual_12, 1) AS growth
  FROM (SELECT category, arg_max(actual, ds) AS last_actual,
               sum(actual) FILTER (WHERE ds > (SELECT max(ds) FROM cat_monthly) - INTERVAL 12 MONTH) AS actual_12
        FROM series WHERE actual IS NOT NULL GROUP BY 1) h
  JOIN (SELECT category, sum(yhat) AS fc_total, arg_min(yhat, ds) AS next_fc
        FROM series WHERE yhat IS NOT NULL GROUP BY 1) c USING (category);

SELECT 'M5 sales — 12-month SeasonalES forecast (click a category row)'::LABEL;

-- Summary table. The first column (category) is the cross-filter key: clicking a
-- row sets getvariable('selected'), which the line chart below filters on.
SELECT 12::COL;
SELECT category   AS "Category"        ::TABLE,
       last_actual AS "Last actual"    ::COMPACT,
       next_fc    AS "Next month"      ::COMPACT,
       fc_total   AS "12-mo forecast"  ::COMPACT,
       growth     AS "vs prior 12mo %" ::TREND,
       'SeasonalES' AS "Method"        ::BADGE
FROM summary ORDER BY fc_total DESC;

-- History + forecast line for the selected category (default FOODS). The 95%
-- interval is shaded; the forecast series repeats the last actual point (a bridge
-- row) so the history and forecast lines join up.
SELECT 12::COL;
SELECT ds     ::XAXIS,
       'Actual' ::CATEGORY,
       actual ::LINECHART,
       actual ::BAND_LOWER,
       actual ::BAND_UPPER,
       'History + 12-month SeasonalES forecast (shaded = 95% interval)'::TITLE
FROM series
WHERE actual IS NOT NULL AND category = COALESCE(NULLIF(getvariable('selected'), ''), 'FOODS')
UNION ALL
SELECT ds, 'Forecast', actual, actual, actual, ''          -- bridge: last actual point
FROM series
WHERE actual IS NOT NULL AND category = COALESCE(NULLIF(getvariable('selected'), ''), 'FOODS')
  AND ds = (SELECT max(ds) FROM series WHERE actual IS NOT NULL)
UNION ALL
SELECT ds, 'Forecast', yhat, lo, hi, ''
FROM series
WHERE yhat IS NOT NULL AND category = COALESCE(NULLIF(getvariable('selected'), ''), 'FOODS')
ORDER BY 1;
