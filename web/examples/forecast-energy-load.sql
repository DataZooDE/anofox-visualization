-- Energy / load forecasting — will any substation exceed its firm capacity
-- next week? Live anofox_forecast in the browser.
--
-- A made-up city utility runs six primary substations. Every Thursday the
-- grid-operations planner decides, for the coming week, where to pre-arrange a
-- load transfer, call demand response or postpone planned maintenance. The
-- script builds 8 weeks of hourly load (deterministic: hash-based noise, no
-- files) — EV-truck charging growing at the harbour, heat pumps reacting to an
-- autumn cold spell at Riverside, two-shift industry in the north — and then,
-- live:
--   * backtests three methods over three rolling one-week folds
--     (ts_cv_folds_by + ts_cv_forecast_by: SeasonalNaive, MSTL, AutoTheta) and
--     picks the most accurate one per substation;
--   * forecasts the next 168 hours with every method (ts_forecast_by);
--   * replaces the models' own intervals, which cover everything but are far
--     too wide to decide on, by split-conformal intervals calibrated on the
--     backtest errors (ts_conformal_quantile) — and checks their coverage on
--     the latest fold (ts_coverage);
--   * finds next week's daily peaks (ts_detect_peaks_by).
-- The same SQL runs in the browser demo (DuckDB-Wasm + the anofox_forecast
-- wasm extension, loaded on first use) and natively (dashboard --check / serve
-- with a duckdb that has anofox_forecast installed; see
-- examples/forecast-energy-load/README.md).
--
-- Tabs: Capacity risk · Substation drill-down · Backtest · Decision.

LOAD anofox_forecast;

SELECT 'anofox_forecast · live in your browser'::BRAND;
SELECT 'Will any substation exceed its firm capacity next week?'::DASHBOARD_TITLE;

-- ─────────────────────────────────────────────────────────────────────────────
-- Data (setup statements — plain SQL, no files)
-- ─────────────────────────────────────────────────────────────────────────────
-- Uniform(0,1) from the md5 of a key tuple; a standard normal via Box–Muller.
CREATE OR REPLACE MACRO el_unif(a, b, c) AS
    ((hash(md5(concat_ws('|', a, b, c))) >> 11)::DOUBLE + 0.5) / 9007199254740992.0;
CREATE OR REPLACE MACRO el_rnorm(a, b, c) AS
    sqrt(-2.0 * ln(el_unif(a, b, c || ':u1'))) * cos(2.0 * pi() * el_unif(a, b, c || ':u2'));

-- Six substations: what they serve, base load, the mix of a residential /
-- commercial / EV-charging daily profile, load growth over the 8 weeks, MW per
-- degree below 14 °C (electric heating) and the firm (N-1) capacity.
CREATE OR REPLACE TABLE el_sub AS SELECT * FROM (VALUES
  ('Harbour',          'EV truck depot + shore power', 58.0, 0.20, 0.30, 0.50, 0.24, 0.6, 68.0),
  ('Riverside',        'new housing, heat pumps',      36.0, 0.75, 0.15, 0.10, 0.04, 2.1, 62.0),
  ('North Industrial', 'two-shift manufacturing',      66.0, 0.10, 0.85, 0.05, 0.03, 0.4, 100.0),
  ('Airport',          'terminals + apron charging',   47.0, 0.20, 0.60, 0.20, 0.07, 0.7, 62.0),
  ('Old Town',         'mixed residential / retail',   31.0, 0.60, 0.40, 0.00, 0.00, 0.8, 52.0),
  ('Campus',           'university + labs',            24.0, 0.30, 0.70, 0.00, 0.01, 0.5, 40.0)
) t(substation, served, base_mw, w_res, w_com, w_ev, growth, heat_mw_per_k, firm_mw);

-- City temperature: an autumn cooling trend, multi-day weather swings and the
-- daily cycle (coldest before dawn).
CREATE OR REPLACE TABLE el_weather AS
SELECT h, 17.0 - 6.0 * h / 1344.0 + 3.0 * sin(2 * pi() * h / 103.0 + 1.3) + 2.0 * sin(2 * pi() * h / 190.0 + 0.4)
          - 4.0 * cos(2 * pi() * ((h % 24) - 15) / 24.0) AS temp_c
FROM range(1344) r(h);

-- el_load — hourly load (MW) per substation, Mon 24 Aug – Sun 18 Oct 2026.
CREATE OR REPLACE TABLE el_load AS
WITH x AS (
  SELECT s.*, w.h, w.temp_c, w.h % 24 AS hod, (w.h // 24) % 7 AS dow,
         TIMESTAMP '2026-08-24 00:00:00' + INTERVAL (w.h) HOUR AS ds
  FROM el_sub s, el_weather w)
SELECT substation, ds,
  round(base_mw * (
      w_res * (0.70 + 0.18 * exp(-power(hod - 7.5, 2) / 2.0) + 0.50 * exp(-power(hod - 19.0, 2) / 5.0))
            * (CASE WHEN dow >= 5 THEN 1.06 ELSE 1 END)
    + w_com * (0.55 + 0.60 / (1 + exp(-(hod - 6.5) * 1.6)) / (1 + exp((hod - 18.5) * 1.6)))
            * (CASE WHEN dow = 5 THEN 0.70 WHEN dow = 6 THEN 0.58 ELSE 1 END)
    + w_ev  * (0.60 + 0.70 * exp(-power(((hod + 2) % 24) - 2.0, 2) / 5.0)) * (CASE WHEN dow = 6 THEN 0.7 ELSE 1 END))
    * (1 + growth * h / 1344.0)
    + heat_mw_per_k * greatest(0, 14.0 - temp_c)
    + 0.025 * base_mw * el_rnorm(substation, h, 'noise'), 2) AS load_mw
FROM x;

-- ─────────────────────────────────────────────────────────────────────────────
-- Forecasting (anofox_forecast, live) — runs once per full run
-- ─────────────────────────────────────────────────────────────────────────────
-- Three rolling-origin folds, each forecasting one week (168 h) ahead.
CREATE OR REPLACE TABLE el_folds AS
SELECT * FROM ts_cv_folds_by('el_load', substation, ds, load_mw, 3, 168, MAP{});

-- Backtest: each method forecasts every fold's test week from its training
-- data. SeasonalNaive (same hour last week) is the benchmark to beat.
CREATE OR REPLACE TABLE el_cv AS
SELECT 'SeasonalNaive' AS method, fold_id, substation, ds, y AS actual, yhat, yhat_lower, yhat_upper
FROM ts_cv_forecast_by('el_folds', substation, ds, load_mw, 'SeasonalNaive', MAP{'seasonal_period': '168'})
UNION ALL
SELECT 'MSTL', fold_id, substation, ds, y, yhat, yhat_lower, yhat_upper
FROM ts_cv_forecast_by('el_folds', substation, ds, load_mw, 'MSTL', MAP{'seasonal_periods': '[24,168]'})
UNION ALL
SELECT 'AutoTheta', fold_id, substation, ds, y, yhat, yhat_lower, yhat_upper
FROM ts_cv_forecast_by('el_folds', substation, ds, load_mw, 'AutoTheta', MAP{'seasonal_period': '168'});

-- Backtest scores per substation × method; the winner (lowest MAE) forecasts.
CREATE OR REPLACE TABLE el_score AS
SELECT substation, method,
       avg(abs(actual - yhat))                       AS mae,
       100 * avg(abs(actual - yhat) / actual)        AS mape,
       100 * avg((actual - yhat) / actual)           AS bias_pct,
       100 * ts_coverage(list(actual), list(yhat_lower), list(yhat_upper)) AS model_cov,
       ts_mean_interval_width(list(yhat_lower), list(yhat_upper))         AS model_width
FROM el_cv GROUP BY ALL;
CREATE OR REPLACE TABLE el_winner AS
SELECT substation, arg_min(method, mae) AS method FROM el_score GROUP BY substation;

-- Next week (Mon 19 – Sun 25 Oct): each substation is refit on all 8 weeks
-- with its winning method only.
CREATE OR REPLACE VIEW el_refit_sn AS
SELECT * FROM el_load WHERE substation IN (SELECT substation FROM el_winner WHERE method = 'SeasonalNaive');
CREATE OR REPLACE VIEW el_refit_mstl AS
SELECT * FROM el_load WHERE substation IN (SELECT substation FROM el_winner WHERE method = 'MSTL');
CREATE OR REPLACE VIEW el_refit_theta AS
SELECT * FROM el_load WHERE substation IN (SELECT substation FROM el_winner WHERE method = 'AutoTheta');
CREATE OR REPLACE TABLE el_fc AS
SELECT 'SeasonalNaive' AS method, substation, ds, yhat, yhat_lower, yhat_upper
FROM ts_forecast_by('el_refit_sn', substation, ds, load_mw, 'SeasonalNaive', 168, '1h', MAP{'seasonal_period': '168'})
UNION ALL
SELECT 'MSTL', substation, ds, yhat, yhat_lower, yhat_upper
FROM ts_forecast_by('el_refit_mstl', substation, ds, load_mw, 'MSTL', 168, '1h', MAP{'seasonal_periods': '[24,168]'})
UNION ALL
SELECT 'AutoTheta', substation, ds, yhat, yhat_lower, yhat_upper
FROM ts_forecast_by('el_refit_theta', substation, ds, load_mw, 'AutoTheta', 168, '1h', MAP{'seasonal_period': '168'});

-- ─────────────────────────────────────────────────────────────────────────────
-- Input-driven views (interval level, alarm) — they recompute instantly
-- ─────────────────────────────────────────────────────────────────────────────

CREATE OR REPLACE MACRO el_alpha() AS 1 - replace(getvariable('interval_level'), ' %', '')::DOUBLE / 100;
CREATE OR REPLACE MACRO el_alarm() AS getvariable('alarm_pct')::DOUBLE / 100;

-- These are views, not tables: they read the inputs, so they must follow every
-- change of them (a setup table is only rebuilt on a full run).
--
-- Split-conformal intervals: the (1 − α) quantile of the absolute backtest
-- errors, per substation × method. q_cal uses folds 1–2 only, so fold 3 can
-- check the coverage honestly; q_all (all folds) widens next week's forecast.
CREATE OR REPLACE VIEW el_conf AS
WITH q AS (
  SELECT substation, method,
         ts_conformal_quantile(list(abs(actual - yhat)) FILTER (WHERE fold_id < 3), el_alpha()) AS q_cal,
         ts_conformal_quantile(list(abs(actual - yhat)), el_alpha())                            AS q_all
  FROM el_cv GROUP BY ALL)
SELECT q.substation, q.method, q.q_cal, q.q_all,
       100 * ts_coverage(list(c.actual), list(c.yhat - q.q_cal), list(c.yhat + q.q_cal)) AS conf_cov,
       100 * ts_coverage(list(c.actual), list(c.yhat_lower), list(c.yhat_upper))         AS model_cov3,
       2 * q.q_cal AS conf_width
FROM q JOIN el_cv c ON c.substation = q.substation AND c.method = q.method AND c.fold_id = 3
GROUP BY ALL;

-- The plan: each substation's winning method, conformal band, firm capacity.
CREATE OR REPLACE VIEW el_plan AS
SELECT f.substation, f.method, f.ds, f.yhat, f.yhat - c.q_all AS lo, f.yhat + c.q_all AS hi, s.firm_mw
FROM el_fc f
JOIN el_winner w USING (substation, method)
JOIN el_conf c USING (substation, method)
JOIN el_sub s USING (substation);

-- The selected substation's plan (the drill-down's peak detection reads it).
CREATE OR REPLACE VIEW el_plan_sel AS SELECT * FROM el_plan WHERE substation = getvariable('substation');

-- Risk per substation for the coming week.
CREATE OR REPLACE VIEW el_risk AS
WITH p AS (
  SELECT substation, any_value(method) AS method, any_value(firm_mw) AS firm_mw,
         max(yhat) AS peak_p50, max(lo) AS peak_lo, max(hi) AS peak_hi, arg_max(ds, hi) AS peak_at,
         count(*) FILTER (WHERE hi > firm_mw)                 AS hours_over_firm,
         count(*) FILTER (WHERE hi > el_alarm() * firm_mw)    AS hours_over_alarm
  FROM el_plan GROUP BY substation),
h AS (
  SELECT substation, max(load_mw) AS peak_last_week
  FROM el_load WHERE ds >= TIMESTAMP '2026-10-12' GROUP BY substation)
SELECT p.*, h.peak_last_week, s.served,
       100 * p.peak_p50 / p.firm_mw AS util_p50, 100 * p.peak_hi / p.firm_mw AS util_hi,
       CASE WHEN p.peak_p50 > p.firm_mw THEN 'breach expected'
            WHEN p.peak_hi > p.firm_mw THEN 'at risk'
            WHEN p.peak_hi > el_alarm() * p.firm_mw THEN 'watch'
            ELSE 'ok' END AS status,
       CASE WHEN p.peak_p50 > p.firm_mw THEN 1 WHEN p.peak_hi > p.firm_mw THEN 2
            WHEN p.peak_hi > el_alarm() * p.firm_mw THEN 3 ELSE 4 END AS status_rank
FROM p JOIN h USING (substation) JOIN el_sub s USING (substation);

-- ─────────────────────────────────────────────────────────────────────────────
-- Header: who asks, and the headline
-- ─────────────────────────────────────────────────────────────────────────────
SELECT 12::COL;
SELECT 'Asked every Thursday by the **grid-operations planner** of a city utility: for the coming week (Mon 19 – Sun 25 Oct), which of our six primary substations could run above its **firm (N-1) capacity**, and when? The answer decides where to pre-arrange a **load transfer**, call **demand response** or **postpone planned maintenance**. Everything below is forecast **live** with anofox_forecast from 8 weeks of hourly load: three methods are backtested per substation, the best one forecasts, and its interval is calibrated on its own backtest errors.'::MARKDOWN,
       'The question'::TITLE;

-- The planning assumptions (inputs). The views above read them at query time.
SELECT 'Planning assumptions'::GROUP;
SELECT level AS interval_level ::DROPDOWN, 'Plan against the upper bound of a … interval'::LABEL
FROM (VALUES (1, '90 %'), (2, '80 %'), (3, '95 %'), (4, '99 %')) t(k, level) ORDER BY k;
SELECT 90 AS alarm_pct ::NUMBER, 'Alarm at (% of firm capacity)'::LABEL;
SELECT 1::ENDGROUP;

SELECT 'Next week at a glance'::GROUP;
SELECT count(*) FILTER (WHERE status_rank <= 2)::METRIC,
       ('of 6 substations may exceed firm capacity (' || getvariable('interval_level') || ' upper bound)')::LABEL
FROM el_risk;
SELECT round(max(sys))::METRIC, round((SELECT max(s) FROM (SELECT sum(load_mw) AS s FROM el_load WHERE ds >= TIMESTAMP '2026-10-12' GROUP BY ds)))::DELTA,
       'system peak forecast (MW) vs this week'::LABEL
FROM (SELECT sum(yhat) AS sys FROM el_plan GROUP BY ds);
SELECT round(min(firm_mw - peak_hi), 1)::METRIC, 'tightest headroom at the upper bound (MW)'::LABEL FROM el_risk;
SELECT sum(hours_over_alarm)::METRIC, 'substation-hours above the alarm level'::LABEL FROM el_risk;
SELECT round(100 - avg(s.mape), 1)::PERCENT,
       (SELECT round(100 - avg(mape), 1) FROM el_score WHERE method = 'SeasonalNaive')::DELTA,
       'backtest accuracy (100 − MAPE), chosen methods vs repeating last week'::LABEL
FROM el_score s JOIN el_winner USING (substation, method);
SELECT round(avg(conf_cov), 1)::PERCENT, ('coverage of the ' || getvariable('interval_level') || ' interval on the latest fold')::LABEL
FROM el_conf JOIN el_winner USING (substation, method);
SELECT 1::ENDGROUP;

-- ─────────────────────────────────────────────────────────────────────────────
-- 1 · Capacity risk (overview)
-- ─────────────────────────────────────────────────────────────────────────────
SELECT 'Capacity risk'::TAB;

SELECT 12::COL;
SELECT 340::HEIGHT;
SELECT substation ::XAXIS, round(util_p50, 1) AS "Forecast peak" ::SCATTER,
       round(100 * peak_lo / firm_mw, 1) ::YMIN, round(util_hi, 1) ::YMAX,
       100 ::REFLINE, 1 ::FLIP, ' %'::YFORMAT,
       ('Peak load next week, % of firm capacity — ' || (SELECT string_agg(substation, ' and ' ORDER BY util_hi DESC) FROM el_risk WHERE status_rank <= 2)
        || ' can cross 100 % (dot = forecast, bar = ' || getvariable('interval_level') || ' interval)') ::TITLE
FROM el_risk ORDER BY util_hi DESC;

SELECT 12::COL;
SELECT 300::HEIGHT;
SELECT strftime(ds, '%a %d') ::XAXIS, p.substation ::YAXIS, round(100 * max(p.hi) / any_value(p.firm_mw)) ::HEATMAP, ''::DATALABELS,
       ('Daily peak upper bound, % of firm capacity — ' || (SELECT count(*) FROM (SELECT substation, date_trunc('day', ds) FROM el_plan GROUP BY ALL HAVING max(hi) > any_value(firm_mw)))
        || ' substation-days above 100 %')::TITLE
FROM el_plan p JOIN el_risk r USING (substation)
GROUP BY p.substation, r.util_hi, date_trunc('day', ds), strftime(ds, '%a %d') ORDER BY date_trunc('day', ds), r.util_hi DESC;

-- ─────────────────────────────────────────────────────────────────────────────
-- 2 · Substation drill-down
-- ─────────────────────────────────────────────────────────────────────────────
SELECT 'Substation drill-down'::TAB;

SELECT substation ::DROPDOWN, 'Substation'::LABEL FROM el_risk ORDER BY util_hi DESC;

SELECT 12::COL;
SELECT 360::HEIGHT;
SELECT ds ::XAXIS, 'Actual' ::CATEGORY, load_mw ::LINECHART, load_mw ::BAND_LOWER, load_mw ::BAND_UPPER,
       (SELECT firm_mw FROM el_sub WHERE substation = getvariable('substation')) ::REFLINE, ' MW'::YFORMAT,
       (getvariable('substation') || ': last two weeks and next week''s forecast (' || (SELECT method FROM el_winner WHERE substation = getvariable('substation'))
        || ', shaded = ' || getvariable('interval_level') || ' conformal interval) against firm capacity (dashed)') ::TITLE
FROM el_load WHERE substation = getvariable('substation') AND ds >= TIMESTAMP '2026-10-05'
UNION ALL
SELECT ds, 'Forecast', round(yhat, 2), round(lo, 2), round(hi, 2), NULL, NULL, NULL
FROM el_plan WHERE substation = getvariable('substation')
ORDER BY 1;

SELECT 6::COL;
SELECT 420::HEIGHT;
SELECT strftime(ds, '%a') ::XAXIS, lpad(hour(ds)::VARCHAR, 2, '0') || ':00' ::YAXIS, round(avg(hi)) ::HEATMAP,
       'When the load is high next week (upper bound, MW by weekday × hour)'::TITLE
FROM el_plan WHERE substation = getvariable('substation')
GROUP BY strftime(ds, '%a'), hour(ds), isodow(ds) ORDER BY hour(ds), isodow(ds);

SELECT 6::COL;
SELECT 420::HEIGHT;
SELECT 'Next week''s daily peaks (ts_detect_peaks_by on the forecast) — the hours to watch'::TITLE,
       strftime(p.ds, '%a %d %b, %H:00') AS "Peak hour" ::TABLE,
       round(p.yhat, 1) AS "Forecast (MW)", round(p.hi, 1) AS "Upper bound (MW)",
       round(p.firm_mw - p.hi, 1) AS "Headroom (MW)" ::COLORSCALE_HIGH_GOOD
FROM (SELECT substation, unnest(peaks, recursive := true)
      FROM ts_detect_peaks_by('el_plan_sel', substation, ds, yhat, MAP{'min_distance': '16'})) k
JOIN (SELECT *, row_number() OVER (ORDER BY ds) - 1 AS idx FROM el_plan_sel) p
  ON p.idx = k.index
ORDER BY p.ds;

-- ─────────────────────────────────────────────────────────────────────────────
-- 3 · Backtest — can we trust the forecast and its interval?
-- ─────────────────────────────────────────────────────────────────────────────
SELECT 'Backtest'::TAB;

SELECT 12::COL;
SELECT 'Three rolling folds, each forecasting a full week (168 h) from the data before it. **SeasonalNaive** (repeat last week) is the benchmark; **MSTL** (daily + weekly seasonality) and **AutoTheta** must beat it to be used. The models'' own intervals cover nearly everything but are far wider than the load itself — no use for a capacity decision. The **conformal** interval is the (1 − α) quantile of each method''s own backtest errors: calibrated on folds 1–2, checked on fold 3.'::MARKDOWN,
       'How to read it'::TITLE;

SELECT 6::COL;
SELECT 320::HEIGHT;
SELECT substation ::XAXIS, method ::CATEGORY, round(mape, 1) ::BARCHART, ' %'::YFORMAT,
       'Backtest error (MAPE) — Riverside is hardest: its heat pumps follow the weather'::TITLE
FROM el_score ORDER BY substation, method;

SELECT 6::COL;
SELECT 320::HEIGHT;
SELECT method ::XAXIS, kind ::CATEGORY, round(avg(width), 1) ::BARCHART, ' MW'::YFORMAT,
       ('Interval width, fold 3 — the ' || getvariable('interval_level') || ' conformal band is a fraction of the model''s') ::TITLE
FROM (SELECT method, 'model interval' AS kind, model_width AS width FROM el_score
      UNION ALL SELECT method, 'conformal ' || getvariable('interval_level'), conf_width FROM el_conf)
GROUP BY ALL ORDER BY method, kind;

SELECT 12::COL;
SELECT 'Accuracy: mae, mape, bias | Model interval: model cov, model width | Conformal: conformal cov, conformal width'::COLGROUP;
SELECT 'mape > 10, conformal cov < 80'::FLAG;
SELECT 'mape: bottom 1'::HIGHLIGHT;
SELECT 'substation'::SUBTOTAL;
SELECT 'mae: mean, mape: mean, bias: mean, model cov: mean, model width: mean, conformal cov: mean, conformal width: mean'::TOTALS;
SELECT s.substation AS "Substation" ::TABLE, s.method AS "Method",
       CASE WHEN s.method = w.method THEN 'ok: chosen' ELSE '' END AS "Use" ::BADGE,
       round(s.mae, 2) AS mae, round(s.mape, 1) AS mape, round(s.bias_pct, 1) AS bias ::COLORSCALE_DIVERGING,
       round(c.model_cov3, 1) AS "model cov", round(s.model_width, 1) AS "model width",
       round(c.conf_cov, 1) AS "conformal cov", round(c.conf_width, 1) AS "conformal width",
       'Every substation × method: accuracy over 3 folds, interval coverage (%) and width (MW) on fold 3'::TITLE
FROM el_score s JOIN el_conf c USING (substation, method) JOIN el_winner w USING (substation)
ORDER BY s.substation, s.mape;

-- ─────────────────────────────────────────────────────────────────────────────
-- 4 · Decision
-- ─────────────────────────────────────────────────────────────────────────────
SELECT 'Decision'::TAB;

SELECT 12::COL;
SELECT ('**Recommendation for next week.** '
        || coalesce((SELECT string_agg('**' || substation || '** (' || status || ', up to ' || round(util_hi)::INTEGER || ' % of firm on ' || strftime(peak_at, '%a %H:00') || ')', ', ' ORDER BY status_rank, util_hi DESC)
                     FROM el_risk WHERE status_rank <= 2), 'No substation')
        || CASE WHEN (SELECT count(*) FROM el_risk WHERE status_rank <= 2) = 1 THEN ' needs' ELSE ' need' END || ' an action before Monday; '
        || coalesce((SELECT string_agg(substation, ', ' ORDER BY util_hi DESC) FROM el_risk WHERE status_rank = 3), 'no other substation')
        || CASE WHEN (SELECT count(*) FROM el_risk WHERE status_rank = 3) = 1 THEN ' stays' ELSE ' stay' END || ' on watch above the ' || getvariable('alarm_pct') || ' % alarm. Raise the interval level above to plan more conservatively; the table updates instantly.')::MARKDOWN,
       'What to do'::TITLE;

SELECT 12::COL;
SELECT 'Status = breach expected, Status = at risk'::FLAG;
SELECT 'Peak % of firm: top 2'::HIGHLIGHT;
SELECT 'Peak % of firm: max, Hours ≥ alarm: sum, Headroom (MW): min'::TOTALS;
SELECT substation AS "Substation" ::TABLE, status AS "Status" ::BADGE,
       round(firm_mw, 0) AS "Firm (MW)", round(peak_p50, 1) AS "Forecast peak (MW)",
       round(peak_hi, 1) AS "Upper bound (MW)", round(firm_mw - peak_hi, 1) AS "Headroom (MW)",
       round(util_hi, 1) AS "Peak % of firm" ::COLORSCALE,
       strftime(peak_at, '%a %H:00') AS "Peak hour", hours_over_alarm AS "Hours ≥ alarm",
       CASE status_rank
         WHEN 1 THEN 'Transfer load, call demand response'
         WHEN 2 THEN 'Arm the switching plan'
         WHEN 3 THEN 'Monitor, maintenance off-peak'
         ELSE 'No action' END AS "Recommended action",
       'Next week per substation — status at the chosen interval level and alarm, with the action for each'::TITLE
FROM el_risk ORDER BY status_rank, util_hi DESC;
