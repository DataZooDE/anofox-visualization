-- @title M5 analytics — MFLES vs MSTL
-- @param category [FOODS, HOBBIES, HOUSEHOLD] = FOODS
--
-- Reads the tables materialised in --init (forecast MFLES+MSTL, MSTL
-- decomposition, 12-month-holdout backtest). No forecast function at request
-- time → read-only and fast. The `category` param drives the decomposition +
-- residual panels; the forecast overview + backtest cover all categories.

SELECT 'Forecast — history + 12-month MFLES vs MSTL'::LABEL;
SELECT ds ::XAXIS, phase ::CATEGORY, val ::LINECHART, 'FOODS'::TITLE
FROM (SELECT ds, 'Actual' AS phase, y AS val FROM an_cat WHERE category='FOODS'
      UNION ALL SELECT ds, method, yhat FROM an_fc WHERE category='FOODS'
      UNION ALL SELECT ds, 'MFLES', y FROM an_cat WHERE category='FOODS' AND ds=(SELECT max(ds) FROM an_cat)
      UNION ALL SELECT ds, 'MSTL', y FROM an_cat WHERE category='FOODS' AND ds=(SELECT max(ds) FROM an_cat)) ORDER BY phase, ds;
SELECT ds ::XAXIS, phase ::CATEGORY, val ::LINECHART, 'HOBBIES'::TITLE
FROM (SELECT ds, 'Actual' AS phase, y AS val FROM an_cat WHERE category='HOBBIES'
      UNION ALL SELECT ds, method, yhat FROM an_fc WHERE category='HOBBIES'
      UNION ALL SELECT ds, 'MFLES', y FROM an_cat WHERE category='HOBBIES' AND ds=(SELECT max(ds) FROM an_cat)
      UNION ALL SELECT ds, 'MSTL', y FROM an_cat WHERE category='HOBBIES' AND ds=(SELECT max(ds) FROM an_cat)) ORDER BY phase, ds;
SELECT ds ::XAXIS, phase ::CATEGORY, val ::LINECHART, 'HOUSEHOLD'::TITLE
FROM (SELECT ds, 'Actual' AS phase, y AS val FROM an_cat WHERE category='HOUSEHOLD'
      UNION ALL SELECT ds, method, yhat FROM an_fc WHERE category='HOUSEHOLD'
      UNION ALL SELECT ds, 'MFLES', y FROM an_cat WHERE category='HOUSEHOLD' AND ds=(SELECT max(ds) FROM an_cat)
      UNION ALL SELECT ds, 'MSTL', y FROM an_cat WHERE category='HOUSEHOLD' AND ds=(SELECT max(ds) FROM an_cat)) ORDER BY phase, ds;

SELECT 'Backtest MAE — MFLES vs MSTL (12-month holdout; lower is better)'::LABEL;
SELECT category ::XAXIS, method ::CATEGORY, mae ::BARCHART, 'MAE by method per category'::TITLE
FROM (SELECT category, method, round(avg(abs(actual-predicted)),0) AS mae
      FROM an_bt GROUP BY 1, 2) ORDER BY category, method;

SELECT 'Decomposition & residuals (selected category)'::LABEL;
SELECT ds ::XAXIS, observed ::LINECHART, trend ::LINECHART, 'Observed + MSTL trend'::TITLE
FROM an_decomp WHERE category = getvariable('category') ORDER BY ds;
SELECT ds ::XAXIS, seasonal ::LINECHART, 'Seasonal'::TITLE
FROM an_decomp WHERE category = getvariable('category') ORDER BY ds;
SELECT ds ::XAXIS, remainder ::LINECHART, 'Remainder'::TITLE
FROM an_decomp WHERE category = getvariable('category') ORDER BY ds;
SELECT remainder ::HISTOGRAM, 'Residual histogram'::TITLE
FROM an_decomp WHERE category = getvariable('category');
SELECT remainder ::QQ, 'Residual normal Q-Q'::TITLE
FROM an_decomp WHERE category = getvariable('category');
