-- @title Can one model forecast every item in a store?
-- @group Demand & stock
-- @tags  forecast, machine learning, global model, anofox_ml, M5, backtest, conformal prediction
-- @description 2,959 items of one Walmart store: one model learned across all of them against a model per item and the usual rules.
--
-- Can one model forecast every item in a store? Monthly sales of the 2,959
-- items of one Walmart store (M5, store CA_1, 2011–2015). Most items have a
-- short or patchy history: a model per item has little to learn from. A
-- global model learns one pattern across all items — from the same
-- features, scaled by each item's level — and forecasts each of them.
--
-- Fitted natively with the anofox_ml DuckDB extension
-- (scripts/gen-ml-global-case.sh, scripts/ml-global-case.sql); this page reads
-- the stored results in data/ml-global/. The backtest: the model is fitted on
-- the months up to October 2014 and forecasts November 2014 to April 2015
-- (1–6 months ahead); the settings were fixed before looking at those months.

CREATE OR REPLACE TEMP TABLE mg_methods AS FROM read_csv('data/ml-global/methods.csv');
CREATE OR REPLACE TEMP TABLE mg_segments AS FROM read_csv('data/ml-global/segments.csv');
CREATE OR REPLACE TEMP TABLE mg_monthly AS FROM read_csv('data/ml-global/monthly.csv');
CREATE OR REPLACE TEMP TABLE mg_examples AS FROM read_csv('data/ml-global/examples.csv');
CREATE OR REPLACE TEMP TABLE mg_importance AS FROM read_csv('data/ml-global/importance.csv');
CREATE OR REPLACE TEMP TABLE mg_info AS FROM read_csv('data/ml-global/provenance.csv');
CREATE OR REPLACE TEMP TABLE mg_intervals AS FROM read_csv('data/ml-global/intervals.csv', types = {'horizon': 'VARCHAR'});
-- short names for the charts' legends
CREATE OR REPLACE TEMP TABLE mg_short AS
FROM (VALUES ('actual', 'actual'), ('global_model', 'global model'), ('local_models', 'model per item'),
             ('seasonal_naive', 'same month last year'), ('moving_average', 'last 3 months')) t(method, short);

SELECT 'The question'::TAB;

SELECT 12::COL;
SELECT '**Who asks:** the store''s demand planner, who orders 2,959 items every month. **The data:** monthly sales of every item of one Walmart store (the M5 competition data, store CA_1). A third of the items are on sale for less than 2½ years; a fifth sell in fewer than 9 months of 12. **The question:** forecast each item — with a model per item, the usual rules (the last 3 months, the same month last year), or one model that learns from all items at once?'::MARKDOWN,
       'The question'::TITLE;

SELECT 'Backtest: November 2014 to April 2015, 1–6 months ahead'::GROUP;
SELECT wape::PERCENT, 'error of one global model (WAPE)'::LABEL,
       'WAPE: the absolute errors of all items and months, as a share of what was sold. Lower is better.'::INFO
FROM mg_methods WHERE method = 'global_model';
SELECT (SELECT min(wape) FROM mg_methods WHERE method IN ('moving_average', 'seasonal_naive'))::PERCENT,
       'error of the best usual rule'::LABEL;
SELECT wape::PERCENT, 'error of a model per item'::LABEL FROM mg_methods WHERE method = 'local_models';
SELECT bias::METRIC, 'bias of the global model, % (over- or under-forecast)'::LABEL FROM mg_methods WHERE method = 'global_model';
SELECT 1::ENDGROUP;

SELECT 12::COL;
SELECT 340::HEIGHT;
SELECT ds::XAXIS, short::CATEGORY, units AS "units sold" ::LINECHART,
       'The whole store, units per month: the four forecasts against what sold'::TITLE
FROM mg_monthly JOIN mg_short USING (method) ORDER BY ds, short;

SELECT 'Who wins where'::TAB;

SELECT 4::COL;
SELECT '**One model for all items wins.** It cuts the error by a sixth against the best usual rule, and by a third against a model per item, which starves on short histories: on **new items** a model per item misses by two thirds of what sold, the global model by under two fifths.

**Items that sell in few months** are hard for every method; the global model still errs least. The **bias** tells the same story: the per-item models and the moving average over-forecast by 9–11 %, the global model is within 1 %.

WAPE: the absolute errors as a share of what was sold, lower is better.'::MARKDOWN;

SELECT 8::COL;
SELECT 300::HEIGHT;
SELECT short::XAXIS, wape AS "error, WAPE (%)" ::BARCHART, 1::FLIP,
       'All 2,959 items: error by method, %'::TITLE
FROM mg_methods JOIN mg_short USING (method) ORDER BY wape;

SELECT 12::COL;
SELECT 560::HEIGHT;
SELECT segment || ' (' || series || ')' AS "items"::XAXIS, short::CATEGORY, wape AS "error, WAPE (%)" ::BARCHART, 1::FLIP,
       'Error by kind of item, % — the global model errs least in every group'::TITLE
FROM mg_segments JOIN mg_short USING (method) WHERE segment <> 'all items' ORDER BY segment, short;

SELECT 'How sure'::TAB;

SELECT 4::COL;
SELECT '**Conformal prediction intervals.** A second, identical model fitted on the months up to April 2014 forecasts May to October 2014; its errors there, scaled by each item''s level and taken per horizon, set the widths (split conformal, `ml_conformal_margin`). They go around the final model''s forecasts, so each item gets an interval as wide as its own level of sales.

**Do they hold?** In the backtest months the 80 % intervals held the actual sales 82.5 % of the time, the 95 % intervals 95.7 %, at every horizon from 1 to 6 months: a little on the safe side, never too narrow.'::MARKDOWN;

SELECT 'Backtest coverage, all items and months'::GROUP;
SELECT covered80::PERCENT, 'inside the 80 % interval'::LABEL FROM mg_intervals WHERE horizon = 'all';
SELECT covered95::PERCENT, 'inside the 95 % interval'::LABEL FROM mg_intervals WHERE horizon = 'all';
SELECT 1::ENDGROUP;

SELECT 8::COL;
SELECT 320::HEIGHT;
SELECT horizon || ' months ahead' AS "horizon"::XAXIS, covered80 AS "inside the 80 % interval (%)" ::BARCHART,
       covered95 AS "inside the 95 % interval (%)" ::BARCHART, 80::REFLINE,
       'How often the actual fell inside, by horizon, % — the line marks the 80 % aim'::TITLE
FROM mg_intervals WHERE horizon <> 'all' ORDER BY horizon;

SELECT 'Look at items'::TAB;

SELECT why AS mg_pick ::DROPDOWN, 'Item'::LABEL, why || ' — ' || series AS label::OPTION_LABEL
FROM (SELECT DISTINCT why, series FROM mg_examples) ORDER BY why;

SELECT 12::COL;
SELECT 380::HEIGHT;
SELECT ds::XAXIS, (SELECT short FROM mg_short s WHERE s.method = e.method)::CATEGORY,
       units AS "units sold" ::LINECHART,
       (SELECT any_value(series) FROM mg_examples WHERE why = coalesce(nullif(getvariable('mg_pick'), ''), 'best seller'))
         || ': two years of sales and the six forecast months'::TITLE
FROM mg_examples e WHERE why = coalesce(nullif(getvariable('mg_pick'), ''), 'best seller') AND method NOT IN ('lo80', 'hi80', 'lo95', 'hi95')
ORDER BY ds;

SELECT 12::COL;
SELECT 320::HEIGHT;
SELECT ds::XAXIS,
       max(units) FILTER (WHERE method = 'actual') AS "actual" ::LINECHART,
       max(units) FILTER (WHERE method = 'global_model') AS "global model" ::LINECHART,
       max(units) FILTER (WHERE method = 'lo80') ::BAND_LOWER, max(units) FILTER (WHERE method = 'hi80') ::BAND_UPPER,
       'The six forecast months: the global model and its 80 % conformal interval'::TITLE
FROM mg_examples WHERE why = coalesce(nullif(getvariable('mg_pick'), ''), 'best seller') AND ds >= DATE '2014-11-01'
GROUP BY ds ORDER BY ds;

SELECT 'What the model looks at'::TAB;

SELECT 4::COL;
SELECT '**What the global model uses.** The features describe each item''s recent sales, scaled by its own level, plus the calendar month, the horizon and the category. The model leans most on **how often an item sells at all** and **how long it has been on sale**: what a single item''s history can''t tell a model of its own, the 2,959 items together can.'::MARKDOWN;

SELECT 8::COL;
SELECT 360::HEIGHT;
SELECT feature::XAXIS, importance AS "share of the splits' gain" ::BARCHART, 1::FLIP,
       'Feature importance of the global model'::TITLE
FROM mg_importance ORDER BY importance DESC;

SELECT 'How it is built'::TAB;

SELECT 12::COL;
SELECT '**One model for all items, in SQL.** Each training row is an item at a forecast month with a horizon of 1–6 months: its recent sales scaled by its level, and the scaled sales that followed. `anofox_ml` fits one LightGBM-style model on all rows, then forecasts every item:

```sql
LOAD anofox_ml;
CREATE TABLE gm AS
SELECT ml_fit_agg(''LgbmRegressor'', target / scale, feats(train),
                  ''{"n_estimators": 400, "learning_rate": 0.05, "num_leaves": 31}'') AS m
FROM train;                                   -- 319,254 rows: 2,959 items × months × horizons

SELECT series, ds, greatest(ml_predict((SELECT m FROM gm), feats(t)), 0) * t.scale AS forecast
FROM test t;
```

The per-item models use the same features with `ml_fit_predict_by(…, series, …)`: one model per item on its own rows. Fitted natively in about 10 seconds; the dashboard reads the stored results. The settings were fixed before the backtest months were looked at.'::MARKDOWN,
       'The model'::TITLE;

SELECT 'Provenance'::TITLE, series AS "items" ::TABLE, training_rows AS "training rows", first_month AS "first forecast month",
       last_month AS "last forecast month", anofox_ml AS "anofox_ml", duckdb AS "DuckDB"
FROM mg_info;
