-- @title Does one model beat the M5 benchmarks on every Walmart item?
-- @group Demand & stock
-- @tags  forecast, machine learning, global model, anofox_ml, anofox_forecast, anofox_statistics, M5, WRMSSE, CRPS, Diebold-Mariano
-- @description All 30,490 Walmart item-store series, 28 days ahead, scored as in the competition: one global model against the usual baselines, and tested.
--
-- Does one model beat the M5 benchmarks on every Walmart item? Daily sales of
-- the 30,490 item-store series of the M5 competition (10 Walmart stores in
-- California, Texas and Wisconsin, 2011–2016), forecast 28 days ahead in the
-- competition's own setting: trained on d_1–d_1941, scored on d_1942–d_1969
-- with WRMSSE over its 12 levels.
--
-- Three DuckDB extensions work together: anofox_forecast computes series
-- features for the model and forecasts the baselines (Naive, the mean of the
-- last 28 days, the same weekday last week); anofox_ml fits one global
-- PerpetualRegressor on all series; anofox_statistics tests the differences
-- (Diebold–Mariano, Model Confidence Set). Generated natively
-- (scripts/gen-m5-case.sh); this page reads the stored results in data/m5/.
-- The settings were fixed before the test days were looked at.

CREATE OR REPLACE TEMP TABLE m5_methods AS FROM read_parquet('data/m5/methods.parquet');
CREATE OR REPLACE TEMP TABLE m5_levels AS FROM read_parquet('data/m5/levels.parquet');
CREATE OR REPLACE TEMP TABLE m5_board AS FROM read_parquet('data/m5/leaderboard.parquet');
CREATE OR REPLACE TEMP TABLE m5_tests AS FROM read_parquet('data/m5/tests.parquet');
CREATE OR REPLACE TEMP TABLE m5_mcs AS FROM read_parquet('data/m5/mcs.parquet');
CREATE OR REPLACE TEMP TABLE m5_totals AS FROM read_parquet('data/m5/totals.parquet');
CREATE OR REPLACE TEMP TABLE m5_examples AS FROM read_parquet('data/m5/examples.parquet');
CREATE OR REPLACE TEMP TABLE m5_importance AS FROM read_parquet('data/m5/importance.parquet');
CREATE OR REPLACE TEMP TABLE m5_info AS FROM read_parquet('data/m5/provenance.parquet');
CREATE OR REPLACE TEMP TABLE m5_seeds AS FROM read_parquet('data/m5/seeds.parquet');
-- short names for the legends
CREATE OR REPLACE TEMP TABLE m5_short AS
FROM (VALUES ('actual', 'actual'), ('perpetual', 'global model'), ('naive', 'last day'),
             ('sma28', 'mean of 28 days'), ('snaive7', 'same weekday last week')) t(method, short);

SELECT 'The question'::TAB;

SELECT 12::COL;
SELECT '**Who asks:** a retailer''s demand planner, who forecasts 30,490 item-store series every day. **The data:** the M5 competition, daily unit sales of 3,049 Walmart items in 10 stores, from January 2011 to June 2016. Most items sell a few units a day or nothing at all. **The question:** does one model learned across all items beat the usual baselines — scored the way the competition scored 5,507 teams, and tested so that the gain is not luck?'::MARKDOWN,
       'The question'::TITLE;

SELECT 'The 28 test days, May 23 to June 19, 2016 (as in the competition)'::GROUP;
SELECT wrmsse::METRIC, 'WRMSSE of the global model'::LABEL,
       'WRMSSE: the competition''s score. The error of every series at 12 levels (total, states, stores, … items), scaled by how much the series moves from day to day and weighted by its dollar sales. Lower is better; 1 is about as good as yesterday''s sales.'::INFO
FROM m5_methods WHERE method = 'perpetual';
SELECT wrmsse::METRIC, 'best baseline: same weekday last week'::LABEL FROM m5_methods WHERE method = 'snaive7';
SELECT wrmsse::METRIC, 'competition winner (YJ_STU)'::LABEL FROM m5_board WHERE name = 'YJ_STU';
SELECT crps::METRIC, 'CRPS per item and day, global model'::LABEL,
       'CRPS: the error of the whole forecast distribution, in units; for a point forecast it equals the absolute error. Lower is better.'::INFO
FROM m5_methods WHERE method = 'perpetual';
SELECT 1::ENDGROUP;

SELECT 12::COL;
SELECT 340::HEIGHT;
SELECT ds::XAXIS, short::CATEGORY, units AS "units sold" ::LINECHART,
       'All 10 stores, units per day: the last 12 weeks and the four forecasts of the 28 test days'::TITLE
FROM m5_totals JOIN m5_short USING (method) WHERE level = 1 ORDER BY ds, short;

SELECT 'Against the competition'::TAB;

SELECT 4::COL;
SELECT '**Better than every benchmark, short of the top 50.** The global model scores **0.618**: better than all 24 of the competition''s benchmarks (the best, bottom-up exponential smoothing, 0.671) and 27 % better than the same weekday last week (0.847). The winner scored 0.520 and the 50th of 5,507 teams 0.576, with ensembles of many tuned models.

**Scored as the organizers scored.** anofox_forecast''s Naive and seasonal-naive forecasts score exactly the competition''s own benchmark values, 1.752 and 0.847, and every weight agrees with the official file. The competition''s numbers are the organizers'' published scores (Makridakis, Spiliotis & Assimakopoulos 2022, *International Journal of Forecasting* 38(4); the score sheet in the M5-methods repository).'::MARKDOWN;

SELECT 8::COL;
SELECT 620::HEIGHT;
SELECT CASE WHEN kind = 'team' THEN '#' || rank || ' ' || name ELSE name END AS "method"::XAXIS, kind::CATEGORY,
       wrmsse AS "WRMSSE" ::BARCHART, 1::FLIP,
       'WRMSSE, lower is better: top teams, the competition''s benchmarks and this page''s global model'::TITLE
FROM m5_board
WHERE (kind = 'team' AND rank IN (1, 2, 3, 10, 25, 50)) OR (kind = 'benchmark' AND wrmsse < 1.2)
   OR (kind = 'this case' AND name = 'one global model (anofox_ml)')
ORDER BY wrmsse;

SELECT 4::COL;
SELECT '**One fit is a noisy draw, so the model is five.** PerpetualRegressor fitted on 11 million rows lands somewhere else with every seed, and even a change in the 13th decimal of the features moves it. The five seeds'' single fits score between 0.611 and 0.725 on the test days. The case averages them; the average was chosen on the 28 days before the test days, where it beat four of the five single fits.'::MARKDOWN;

SELECT 8::COL;
SELECT 260::HEIGHT;
SELECT 'seed ' || seed AS "fit"::XAXIS, wrmsse AS "WRMSSE, test days" ::BARCHART, wrmsse_public AS "WRMSSE, the 28 days before" ::BARCHART,
       (SELECT wrmsse FROM m5_methods WHERE method = 'perpetual')::REFLINE,
       'Single fits of the global model; the line marks the average of the five on the test days'::TITLE
FROM m5_seeds ORDER BY seed;

SELECT 12::COL;
SELECT 340::HEIGHT;
SELECT lpad(level::VARCHAR, 2, '0') || ' ' || level_name AS "level"::XAXIS, short::CATEGORY, wrmsse AS "WRMSSE" ::BARCHART,
       'WRMSSE by level, lower is better: the global model gains most where items add up'::TITLE
FROM m5_levels JOIN m5_short USING (method) ORDER BY level, short;

SELECT 'Is it luck?'::TAB;

SELECT 4::COL;
SELECT '**Where items add up, it is no luck.** On the competition''s score (the daily weighted part of WRMSSE, all 12 levels) the global model beats every baseline by 42–85 %, each with p < 0.0001 (Diebold–Mariano, anofox_statistics `diebold_mariano_agg`, pooled over the 28 test days).

**On single items the gain is small.** Against the mean of the last 28 days its CRPS is 0.7 % lower (significant) and its MAE 1.2 % lower (p = 0.07, not significant at 5 %). Series by series most items show no difference either way, and in MAE more are worse than better: a model of the expected demand forecasts a little above 0 for an item that usually sells nothing, while the absolute error rewards forecasting 0 itself.

**The Model Confidence Set** (`model_confidence_set_agg`, 90 %) keeps only the global model, for all three losses.'::MARKDOWN;

SELECT 8::COL;
SELECT metric_label AS "loss"::TABLE, baseline_label AS "against", gain_pct AS "global model's gain, %",
       round(dm_p_value, 4) AS "p (pooled)", series_better AS "series better", series_worse AS "series worse",
       series_no_difference AS "no difference"
FROM m5_tests ORDER BY metric, baseline;

SELECT 12::COL;
SELECT CASE metric WHEN 'mae' THEN 'MAE' WHEN 'crps' THEN 'CRPS' ELSE 'WRMSSE (daily part)' END AS "loss"::TABLE, method AS "method",
       in_set AS "in the set of best methods", round(mcs_p_value, 3) AS "MCS p-value",
       'Model Confidence Set (90 %): the methods that can''t be told apart from the best'::TITLE
FROM m5_mcs ORDER BY metric, mcs_p_value DESC;

SELECT 'How sure'::TAB;

SELECT 4::COL;
SELECT '**Prediction intervals for every method, by one recipe.** Each method also forecast the 28 days before the test days (April 25 to May 22, from d_1913). The quantiles of its errors there — scaled by each item''s level, per band of sales and forecast week — give its intervals and quantiles on the test days. **CRPS** comes from 20 of these quantiles (2 × the mean pinball loss).

**They hold.** The global model''s 50 %, 80 % and 95 % intervals held 51.0 %, 79.8 % and 95.2 % of the actual sales on the test days.'::MARKDOWN;

SELECT 8::COL;
SELECT label AS "method"::TABLE, cov50 AS "inside 50 %", cov80 AS "inside 80 %", cov95 AS "inside 95 %",
       crps AS "CRPS", mae AS "MAE", bias AS "bias, %"
FROM m5_methods ORDER BY crps;

SELECT why AS m5_pick ::DROPDOWN, 'Item'::LABEL, why || ' — ' || id AS label::OPTION_LABEL
FROM (SELECT DISTINCT why, id FROM m5_examples) ORDER BY why;

SELECT 12::COL;
SELECT 340::HEIGHT;
SELECT f.ds::XAXIS,
       max(f.units) FILTER (WHERE f.method = 'actual') AS "actual" ::LINECHART,
       max(f.units) FILTER (WHERE f.method = 'perpetual') AS "global model" ::LINECHART,
       max(e.lo80) ::BAND_LOWER, max(e.hi80) ::BAND_UPPER,
       (SELECT any_value(id) FROM m5_examples WHERE why = coalesce(nullif(getvariable('m5_pick'), ''), 'best seller'))
         || ': the 28 test days, the global model and its 80 % interval'::TITLE
FROM read_parquet('data/m5/forecasts.parquet') f
JOIN m5_examples e ON e.id = f.id AND e.ds = f.ds
WHERE e.why = coalesce(nullif(getvariable('m5_pick'), ''), 'best seller')
GROUP BY f.ds ORDER BY f.ds;

SELECT 'Any item'::TAB;

SELECT DISTINCT store AS m5_store ::DROPDOWN, 'Store'::LABEL FROM read_parquet('data/m5/series.parquet') ORDER BY store;
SELECT DISTINCT dept AS m5_dept ::DROPDOWN, 'Department'::LABEL FROM read_parquet('data/m5/series.parquet') ORDER BY dept;

SELECT 12::COL;
SELECT id AS "item"::TABLE, units_test AS "units in the test days", rmsse_tree AS "RMSSE, global model",
       rmsse_naive AS "RMSSE, last day", rmsse_mean28 AS "RMSSE, mean of 28 days",
       crps_tree AS "CRPS, global model", crps_mean28 AS "CRPS, mean of 28 days", p_mae_vs_mean28 AS "p, MAE vs mean of 28 days",
       'The items of a store and department, best sellers first'::TITLE
FROM read_parquet('data/m5/series.parquet')
WHERE store = coalesce(nullif(getvariable('m5_store'), ''), 'CA_1') AND dept = coalesce(nullif(getvariable('m5_dept'), ''), 'FOODS_3')
ORDER BY units_test DESC LIMIT 200;

SELECT 'What the model looks at'::TAB;

SELECT 4::COL;
SELECT '**Where the features come from.** Recent sales (SQL windows), the calendar and prices of the competition, the item''s group, and 13 series features from **anofox_forecast** (`ts_features_agg` on the year before each forecast day: mean, spread, trend, weekly autocorrelation, runs, entropy, …). **Importance** is out of sample: on 200,000 rows of the 28 calibration days, how much the error (MAE) rises when one feature is shuffled. The model leans on the recent level (the mean of the last 28 days), then on the 90 % quantile of the past year from anofox_forecast and on how often the item sells at all. (Shuffling makes a few combinations the model never saw, which the log link turns into absurd forecasts; here every forecast is capped at the item''s highest day of the past year.)'::MARKDOWN;

SELECT 8::COL;
SELECT 640::HEIGHT;
SELECT feature::XAXIS, source::CATEGORY, mae_rise_pct AS "MAE rises by, % (feature shuffled)" ::BARCHART, 1::FLIP,
       'Permutation importance of the global model'::TITLE
FROM m5_importance ORDER BY mae_rise_pct DESC;

SELECT 'How it is built'::TAB;

SELECT 12::COL;
SELECT '**Three extensions, one pipeline in SQL.** Each training row is a series on a forecast day with a horizon of 1–28 days; all features use data up to that day only.

```sql
LOAD anofox_forecast; LOAD anofox_ml; LOAD anofox_statistics;

-- series features from anofox_forecast, on the year before each forecast day
SELECT id, o, ts_features_agg(ds, y, [''mean'', ''standard_deviation'', ''autocorrelation_lag7'', …]) AS f
FROM history GROUP BY id, o;

-- the baselines from anofox_forecast
SELECT * FROM ts_forecast_by(''train'', id, ds, y, ''Naive'', 28, ''1d'', MAP{});
SELECT * FROM ts_forecast_by(''train'', id, ds, y, ''SMA'', 28, ''1d'', MAP{''window'': ''28''});

-- one global model from anofox_ml: 30,490 series, one year of rows, five seeds averaged
SELECT seed, ml_fit_agg(''PerpetualRegressor'', y, feats(ex),
         ''{"budget": 0.5, "objective": {"TweedieLoss": {"p": 1.5}}, "seed": '' || seed || ''}'' ORDER BY id, o, h) AS m
FROM ex, range(7, 12) s(seed) WHERE o < 1941 GROUP BY seed;

-- the test from anofox_statistics: daily losses of the two methods
SELECT diebold_mariano_agg(0.0, tree, base, {''loss'': ''absolute'', ''horizon'': 7} ORDER BY t) FROM daily;
```

PerpetualRegressor needs no tuning: it takes one `budget` and stops on its own. The Tweedie loss suits counts that are mostly 0; its predictions are the log of the mean, so the forecasts are `exp(ml_predict(…))` (DataZooDE/anofox-ml#107). The fits — five seeds at d_1913 for the intervals and five at d_1941 for the test days, 11 million rows each — take 5–9 minutes each natively; the dashboard reads the stored results.'::MARKDOWN,
       'The pipeline'::TITLE;

SELECT 'Provenance'::TITLE, series AS "series" ::TABLE, training_rows AS "training rows", seeds AS "seeds averaged", first_day AS "first test day",
       last_day AS "last test day", anofox_ml AS "anofox_ml", anofox_forecast AS "anofox_forecast",
       anofox_statistics AS "anofox_statistics", duckdb AS "DuckDB"
FROM m5_info;
