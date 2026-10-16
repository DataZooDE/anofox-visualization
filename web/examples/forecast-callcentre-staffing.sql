-- Call-centre staffing — how many agents do we need, every half-hour, for the
-- next three weeks? Live anofox_forecast in the browser.
--
-- A made-up energy supplier's customer-service centre has three queues
-- (Billing, Technical support, Sales). Every Friday the workforce-management
-- (WFM) planner publishes the roster for the next three weeks and needs to know
-- where the current shift plan falls short. The script builds 26 weeks of
-- half-hourly contacts (deterministic: hash-based noise, no files) — Monday
-- peaks, a billing spike after the monthly bill run, a growing sales campaign,
-- one storm outage in tech support — and then, live:
--   * backtests four methods per queue over four rolling three-week folds
--     (ts_cv_folds_by + ts_cv_forecast_by: SeasonalNaive, AutoETS, AutoTheta,
--     MSTL), scored with ts_mape / ts_bias, and keeps the best one per queue;
--   * forecasts daily contacts for the next 21 days (ts_forecast_by);
--   * turns the backtest errors into an uncertainty quantile (plan at P50, P80
--     or P90 of the actual/forecast ratio), spreads each day over its
--     half-hours with the recent intraday profile, and converts contacts into
--     required agents with Erlang C — all in SQL, for the service level,
--     answer time and shrinkage you choose.
-- The same SQL runs in the browser demo (DuckDB-Wasm + the anofox_forecast
-- wasm extension, loaded on first use) and natively (dashboard --check / serve
-- with a duckdb that has anofox_forecast installed; see
-- examples/forecast-callcentre-staffing/README.md).
--
-- Tabs: Demand · Staffing gap · Backtest · Roster recommendation.

LOAD anofox_forecast;

SELECT 'anofox_forecast · live in your browser'::BRAND;
SELECT 'How many agents do we need per half-hour for the next three weeks?'::DASHBOARD_TITLE;

-- ─────────────────────────────────────────────────────────────────────────────
-- Data (setup statements — plain SQL, no files)
-- ─────────────────────────────────────────────────────────────────────────────
-- Uniform(0,1) from the md5 of a key tuple; a standard normal via Box–Muller.
CREATE OR REPLACE MACRO cc_unif(a, b, c) AS
    ((hash(md5(concat_ws('|', a, b, c))) >> 11)::DOUBLE + 0.5) / 9007199254740992.0;
CREATE OR REPLACE MACRO cc_rnorm(a, b, c) AS
    sqrt(-2.0 * ln(cc_unif(a, b, c || ':u1'))) * cos(2.0 * pi() * cc_unif(a, b, c || ':u2'));

-- Three queues: contacts on an average day, average handling time (s), growth
-- per day, the uplift on the first working days of a month (bill run), and the
-- current shift plan — agents on the early (08:00–16:30), mid (09:00–17:30)
-- and evening (11:30–20:00) shift Mon–Fri, and on Saturday (09:00–14:00).
CREATE OR REPLACE TABLE cc_queue AS SELECT * FROM (VALUES
  ('Billing',           1650.0, 360, 0.0,    0.35, 24, 20, 15, 32),
  ('Technical support', 1150.0, 540, 0.0008, 0.0,  25, 21, 32, 38),
  ('Sales',              520.0, 300, 0.0025, 0.0,  10,  9,  8, 15)
) t(queue, base_day, aht_s, growth_per_day, month_start_uplift, early_agents, mid_agents, late_agents, sat_agents);

-- Opening hours as half-hour slots: Mon–Fri 08:00–20:00, Sat 09:00–14:00.
-- Intraday shape per queue: a late-morning and an early-afternoon hump; tech
-- support also gets after-work calls.
CREATE OR REPLACE TABLE cc_slot AS
WITH s AS (
  SELECT q.queue, d.dow, make_time(8 + i // 2, 30 * (i % 2), 0) AS slot, 8 + i / 2.0 AS hod
  FROM cc_queue q, range(1, 7) d(dow), range(24) r(i)
  WHERE d.dow <= 5 OR (i >= 2 AND i < 12)),
w AS (
  SELECT *, 0.35 + exp(-power(hod - 10.25, 2) / 2.5) + 0.8 * exp(-power(hod - 14.0, 2) / 3.0)
            + CASE WHEN queue = 'Technical support' THEN 0.7 * exp(-power(hod - 18.0, 2) / 1.5) ELSE 0 END AS wt
  FROM s)
SELECT queue, dow, slot, wt / sum(wt) OVER (PARTITION BY queue, dow) AS share FROM w;

-- cc_contacts — half-hourly contacts, Mon 20 Apr – Sun 18 Oct 2026 (26 weeks).
CREATE OR REPLACE TABLE cc_contacts AS
WITH d AS (SELECT (DATE '2026-04-20' + INTERVAL (i) DAY)::DATE AS ds, i FROM range(182) r(i)),
day AS (
  SELECT q.queue, d.ds, d.i, isodow(d.ds) AS dow,
         q.base_day
         * (CASE isodow(d.ds) WHEN 1 THEN 1.28 WHEN 2 THEN 1.10 WHEN 3 THEN 1.0 WHEN 4 THEN 0.96 WHEN 5 THEN 0.90 ELSE 0.42 END)
         * (1 + q.growth_per_day * d.i)
         * (1 + q.month_start_uplift * (CASE WHEN day(d.ds) <= 3 THEN 1 WHEN day(d.ds) <= 6 THEN 0.4 ELSE 0 END))
         * (1 - 0.12 * (month(d.ds) IN (7, 8))::INT * (q.queue <> 'Sales')::INT)
         * (1 + CASE WHEN q.queue = 'Technical support' AND d.ds = DATE '2026-09-08' THEN 1.6 ELSE 0 END)
         * (1 + 0.06 * cc_rnorm(q.queue, d.i, 'day')) AS expected
  FROM cc_queue q, d)
SELECT day.queue, day.ds, s.slot,
       greatest(0, round(day.expected * s.share * (1 + 0.10 * cc_rnorm(day.queue, day.i, s.slot::VARCHAR))))::INTEGER AS contacts
FROM day JOIN cc_slot s ON s.queue = day.queue AND s.dow = day.dow;

-- Daily totals (Sundays closed = 0, so every series is a regular daily one).
CREATE OR REPLACE TABLE cc_daily AS
SELECT q.queue, d.ds, coalesce(sum(c.contacts), 0)::DOUBLE AS contacts
FROM cc_queue q
CROSS JOIN (SELECT (DATE '2026-04-20' + INTERVAL (i) DAY)::DATE AS ds FROM range(182) r(i)) d
LEFT JOIN cc_contacts c ON c.queue = q.queue AND c.ds = d.ds
GROUP BY ALL;

-- Intraday profile: each slot's share of its weekday's contacts, last 8 weeks.
CREATE OR REPLACE TABLE cc_profile AS
SELECT queue, isodow(ds) AS dow, slot, sum(contacts) / sum(sum(contacts)) OVER (PARTITION BY queue, isodow(ds)) AS share
FROM cc_contacts WHERE ds > DATE '2026-08-23' GROUP BY queue, isodow(ds), slot;

-- ─────────────────────────────────────────────────────────────────────────────
-- Forecasting (anofox_forecast, live) — runs once per full run
-- ─────────────────────────────────────────────────────────────────────────────
-- Four rolling-origin folds, each forecasting three weeks (21 days) ahead.
CREATE OR REPLACE TABLE cc_folds AS
SELECT * FROM ts_cv_folds_by('cc_daily', queue, ds, contacts, 4, 21, MAP{});

CREATE OR REPLACE TABLE cc_cv AS
SELECT 'SeasonalNaive' AS method, fold_id, queue, ds::DATE AS ds, y AS actual, greatest(yhat, 0) AS yhat
FROM ts_cv_forecast_by('cc_folds', queue, ds, contacts, 'SeasonalNaive', MAP{'seasonal_period': '7'})
UNION ALL
SELECT 'AutoETS', fold_id, queue, ds::DATE, y, greatest(yhat, 0)
FROM ts_cv_forecast_by('cc_folds', queue, ds, contacts, 'AutoETS', MAP{'seasonal_period': '7'})
UNION ALL
SELECT 'AutoTheta', fold_id, queue, ds::DATE, y, greatest(yhat, 0)
FROM ts_cv_forecast_by('cc_folds', queue, ds, contacts, 'AutoTheta', MAP{'seasonal_period': '7'})
UNION ALL
SELECT 'MSTL', fold_id, queue, ds::DATE, y, greatest(yhat, 0)
FROM ts_cv_forecast_by('cc_folds', queue, ds, contacts, 'MSTL', MAP{'seasonal_periods': '[7]'});

-- Scores on open days (Sundays are closed, not forecast errors).
CREATE OR REPLACE TABLE cc_score AS
SELECT queue, method,
       ts_mape(list(actual), list(yhat)) AS mape,
       100 * ts_bias(list(actual), list(yhat)) / avg(actual) AS bias_pct,  -- + = over-forecast
       ts_rmse(list(actual), list(yhat)) AS rmse
FROM cc_cv WHERE isodow(ds) <> 7 GROUP BY ALL;
CREATE OR REPLACE TABLE cc_winner AS
SELECT queue, arg_min(method, mape) AS method FROM cc_score GROUP BY queue;

-- The next 21 days (Mon 19 Oct – Sun 8 Nov), every method; the winner is used.
CREATE OR REPLACE TABLE cc_fc AS
SELECT 'SeasonalNaive' AS method, queue, ds::DATE AS ds, greatest(yhat, 0) AS yhat
FROM ts_forecast_by('cc_daily', queue, ds, contacts, 'SeasonalNaive', 21, '1d', MAP{'seasonal_period': '7'})
UNION ALL
SELECT 'AutoETS', queue, ds::DATE, greatest(yhat, 0)
FROM ts_forecast_by('cc_daily', queue, ds, contacts, 'AutoETS', 21, '1d', MAP{'seasonal_period': '7'})
UNION ALL
SELECT 'AutoTheta', queue, ds::DATE, greatest(yhat, 0)
FROM ts_forecast_by('cc_daily', queue, ds, contacts, 'AutoTheta', 21, '1d', MAP{'seasonal_period': '7'})
UNION ALL
SELECT 'MSTL', queue, ds::DATE, greatest(yhat, 0)
FROM ts_forecast_by('cc_daily', queue, ds, contacts, 'MSTL', 21, '1d', MAP{'seasonal_periods': '[7]'});

-- Calendar adjustment: no method sees the monthly bill run, so the backtest
-- errors on the 1st–3rd and 4th–6th of a month (vs other days) give a factor
-- per queue that the plan applies on those days.
CREATE OR REPLACE MACRO cc_bucket(d) AS
    CASE WHEN day(d) <= 3 THEN 'bill run' WHEN day(d) <= 6 THEN 'after bill run' ELSE 'normal' END;
CREATE OR REPLACE TABLE cc_event AS
WITH r AS (
  SELECT c.queue, cc_bucket(c.ds) AS bucket, median(c.actual / c.yhat) AS ratio
  FROM cc_cv c JOIN cc_winner USING (queue, method)
  WHERE isodow(c.ds) <> 7 AND c.yhat > 0 GROUP BY ALL)
SELECT r.queue, r.bucket, r.ratio / n.ratio AS factor
FROM r JOIN r n ON n.queue = r.queue AND n.bucket = 'normal';

-- Uncertainty from the backtest: quantiles of actual / (adjusted) forecast per
-- queue for the chosen method — the models' own intervals are far too wide
-- to staff to.
CREATE OR REPLACE TABLE cc_ratio AS
SELECT c.queue,
       quantile_cont(c.actual / (c.yhat * e.factor), 0.10) AS p10, quantile_cont(c.actual / (c.yhat * e.factor), 0.50) AS p50,
       quantile_cont(c.actual / (c.yhat * e.factor), 0.80) AS p80, quantile_cont(c.actual / (c.yhat * e.factor), 0.90) AS p90
FROM cc_cv c JOIN cc_winner USING (queue, method)
JOIN cc_event e ON e.queue = c.queue AND e.bucket = cc_bucket(c.ds)
WHERE isodow(c.ds) <> 7 AND c.yhat > 0 GROUP BY c.queue;

-- ─────────────────────────────────────────────────────────────────────────────
-- Header: who asks
-- ─────────────────────────────────────────────────────────────────────────────
SELECT 12::COL;
SELECT 'Asked every Friday by the **workforce-management planner** of a customer-service centre (three queues: Billing, Technical support, Sales) before the roster for the **next three weeks** (Mon 19 Oct – Sun 8 Nov) is published: how many agents are needed **every half-hour**, and where does the current shift plan fall short? Daily contacts are forecast **live** with anofox_forecast (four methods backtested per queue, the best one kept), widened to the quantile you plan for, spread over the day with the recent intraday profile and turned into agents with **Erlang C**.'::MARKDOWN,
       'The question'::TITLE;

-- ─────────────────────────────────────────────────────────────────────────────
-- Staffing rules (inputs) and the input-driven plan
-- ─────────────────────────────────────────────────────────────────────────────
SELECT 'Staffing rules'::GROUP;
SELECT plan_at ::DROPDOWN, 'Staff for'::LABEL
FROM (VALUES (1, 'P80 (busier than 4 days in 5)'), (2, 'P50 (median)'), (3, 'P90 (busier than 9 days in 10)')) t(k, plan_at) ORDER BY k;
SELECT 80 AS sl_target_pct ::NUMBER, 'Service level (% answered …)'::LABEL;
SELECT 20 AS answer_s ::NUMBER, '… within (seconds)'::LABEL;
SELECT 30 AS shrinkage_pct ::NUMBER, 'Shrinkage (breaks, training, absence, %)'::LABEL;
SELECT 1::ENDGROUP;

-- Daily plan: the chosen method's forecast × the planning quantile (rebuilt
-- whenever an input changes — the statement reads getvariable).
CREATE OR REPLACE TABLE cc_plan AS
SELECT f.queue, f.ds, f.method, f.yhat * e.factor AS yhat,
       f.yhat * e.factor * r.p10 AS lo, f.yhat * e.factor * r.p90 AS hi,
       f.yhat * e.factor * CASE left(getvariable('plan_at'), 3) WHEN 'P50' THEN r.p50 WHEN 'P90' THEN r.p90 ELSE r.p80 END AS plan
FROM cc_fc f JOIN cc_winner USING (queue, method) JOIN cc_ratio r USING (queue)
JOIN cc_event e ON e.queue = f.queue AND e.bucket = cc_bucket(f.ds)
WHERE isodow(f.ds) <> 7;

-- Erlang C per queue × day × half-hour: offered load A = calls × AHT / 1800 s;
-- for N agents, Erlang B by the cumulative Poisson terms, then
--   C = N·B / (N − A·(1 − B)),  SL = 1 − C·exp(−(N − A)·T / AHT).
-- Required agents = the smallest N reaching the service level; scheduled heads
-- add the shrinkage. Rostered heads come from the current shift plan.
CREATE OR REPLACE TABLE cc_erlang AS
WITH iv AS (
  SELECT p.queue, p.ds, s.slot, p.plan * s.share AS calls, q.aht_s,
         p.plan * s.share * q.aht_s / 1800.0 AS a,
         CASE WHEN isodow(p.ds) = 6 THEN q.sat_agents
              ELSE (CASE WHEN s.slot < TIME '16:30' THEN q.early_agents ELSE 0 END)
                 + (CASE WHEN s.slot >= TIME '09:00' AND s.slot < TIME '17:30' THEN q.mid_agents ELSE 0 END)
                 + (CASE WHEN s.slot >= TIME '11:30' THEN q.late_agents ELSE 0 END) END AS rostered
  FROM cc_plan p JOIN cc_profile s ON s.queue = p.queue AND s.dow = isodow(p.ds) JOIN cc_queue q ON q.queue = p.queue),
k AS (
  SELECT iv.*, n.k,
         sum(exp(n.k * ln(greatest(iv.a, 1e-9)) - lgamma(n.k + 1))) OVER (PARTITION BY iv.queue, iv.ds, iv.slot ORDER BY n.k) AS cum,
         exp(n.k * ln(greatest(iv.a, 1e-9)) - lgamma(n.k + 1)) AS term
  FROM iv, range(0, 90) n(k)),
sl AS (
  SELECT *, term / cum AS b FROM k WHERE k > a),
sv AS (
  SELECT *, 1 - (k * b / (k - a * (1 - b))) * exp(-(k - a) * getvariable('answer_s')::DOUBLE / aht_s) AS service,
         floor(rostered * (1 - getvariable('shrinkage_pct')::DOUBLE / 100)) AS on_phone
  FROM sl)
SELECT queue, ds, slot, any_value(calls) AS calls, any_value(a) AS erlangs, any_value(rostered) AS rostered,
       min(k) FILTER (WHERE service >= getvariable('sl_target_pct')::DOUBLE / 100) AS required,
       ceil(min(k) FILTER (WHERE service >= getvariable('sl_target_pct')::DOUBLE / 100)
            / (1 - getvariable('shrinkage_pct')::DOUBLE / 100))::INTEGER AS heads,
       coalesce(max(service) FILTER (WHERE k <= on_phone), 0) AS sl_rostered
FROM sv GROUP BY queue, ds, slot;

-- Per queue × day: the decision rows.
CREATE OR REPLACE TABLE cc_day AS
SELECT queue, ds, 'Week of ' || strftime(date_trunc('week', ds), '%d %b') AS week,
       sum(calls) AS contacts, max(heads) AS peak_heads,
       arg_max(rostered, heads - rostered) AS rostered_at_gap,
       max(heads - rostered) AS short_by, arg_max(slot, heads - rostered) AS worst_slot,
       count(*) FILTER (WHERE heads > rostered) AS slots_short,
       100 * sum(sl_rostered * calls) / sum(calls) AS sl_rostered,
       sum(heads) / 2.0 AS agent_hours, sum(rostered) / 2.0 AS rostered_hours,
       getvariable('plan_at') AS plan_at
FROM cc_erlang GROUP BY queue, ds;

-- ─────────────────────────────────────────────────────────────────────────────
-- KPI strip
-- ─────────────────────────────────────────────────────────────────────────────
SELECT 'Next three weeks at a glance'::GROUP;
SELECT round(sum(contacts))::COMPACT,
       (SELECT sum(contacts) FROM cc_daily WHERE ds > DATE '2026-09-27')::DELTA,
       ('contacts to plan for (' || left(getvariable('plan_at'), 3) || ') vs the last three weeks')::LABEL
FROM cc_day;
SELECT round(sum(agent_hours))::COMPACT, (SELECT sum(rostered_hours) FROM cc_day)::DELTA,
       'agent-hours needed vs rostered — enough hours, in the wrong half-hours'::LABEL
FROM cc_day;
SELECT max(peak_heads)::METRIC, ('agents needed in the busiest half-hour (' || (SELECT queue || ', ' || strftime(ds, '%a %d %b') FROM cc_day ORDER BY peak_heads DESC LIMIT 1) || ')')::LABEL
FROM cc_day;
SELECT sum(slots_short)::METRIC, 'half-hours the current roster is short'::LABEL FROM cc_day;
SELECT round(sum(sl_rostered * contacts) / sum(contacts), 1)::PERCENT,
       ('service level the current roster would reach (target ' || getvariable('sl_target_pct') || ' % in ' || getvariable('answer_s') || ' s)')::LABEL
FROM cc_day;
SELECT round(100 - avg(s.mape), 1)::PERCENT, (SELECT round(100 - avg(mape), 1) FROM cc_score WHERE method = 'SeasonalNaive')::DELTA,
       'backtest accuracy (100 − MAPE), chosen methods vs same-day-last-week'::LABEL
FROM cc_score s JOIN cc_winner USING (queue, method);
SELECT 1::ENDGROUP;

-- ─────────────────────────────────────────────────────────────────────────────
-- 1 · Demand
-- ─────────────────────────────────────────────────────────────────────────────
SELECT 'Demand'::TAB;

SELECT 12::COL;
SELECT 380::HEIGHT;
SELECT ds ::XAXIS, queue ::CATEGORY, contacts ::LINECHART, contacts ::BAND_LOWER, contacts ::BAND_UPPER,
       'Daily contacts: last 8 weeks and the 3-week forecast (shaded = P10–P90 from the backtest errors) — the bill run lifts Billing on Mon 2 Nov'::TITLE
FROM cc_daily WHERE ds > DATE '2026-08-23' AND isodow(ds) <> 7
UNION ALL
SELECT ds, queue, round(yhat), round(lo), round(hi), NULL FROM cc_plan
ORDER BY 1, 2;

SELECT 6::COL;
SELECT 320::HEIGHT;
SELECT strftime(DATE '2000-01-01' + slot, '%H:%M') ::XAXIS, queue ::CATEGORY, round(avg(share) * 100, 2) ::LINECHART, ' %'::YFORMAT,
       'Intraday profile, Mon–Fri (share of the day per half-hour) — tech support peaks again after work'::TITLE
FROM cc_profile WHERE dow <= 5 GROUP BY ALL ORDER BY 1, 2;

SELECT 6::COL;
SELECT 320::HEIGHT;
SELECT queue ::XAXIS, round(sum(contacts)) AS "Contacts to plan for" ::BARCHART, 1 ::FLIP,
       'Billing carries most of the next three weeks'' contacts'::TITLE
FROM cc_day GROUP BY queue ORDER BY 2 DESC;

-- ─────────────────────────────────────────────────────────────────────────────
-- 2 · Staffing gap (drill-down per queue)
-- ─────────────────────────────────────────────────────────────────────────────
SELECT 'Staffing gap'::TAB;

SELECT queue ::DROPDOWN, 'Queue'::LABEL FROM cc_queue ORDER BY base_day DESC;

SELECT 12::COL;
SELECT 420::HEIGHT;
SELECT strftime(ds, '%a %d %b') ::XAXIS, strftime(DATE '2000-01-01' + slot, '%H:%M') ::YAXIS, rostered - heads ::HEATMAP,
       (getvariable('queue') || ': rostered minus needed agents per half-hour (red = short) — short at the morning ramp and in the evening, surplus at midday') ::TITLE
FROM cc_erlang WHERE queue = getvariable('queue') ORDER BY ds, slot;

SELECT 6::COL;
SELECT 340::HEIGHT;
SELECT strftime(DATE '2000-01-01' + slot, '%H:%M') ::XAXIS, 'needed (heads incl. shrinkage)' ::CATEGORY, heads ::STEP,
       ('Worst day ' || strftime(ds, '%a %d %b') || ': needed vs rostered agents') ::TITLE
FROM cc_erlang
WHERE queue = getvariable('queue')
  AND ds = (SELECT ds FROM cc_day WHERE queue = getvariable('queue') ORDER BY short_by DESC, ds LIMIT 1)
UNION ALL
SELECT strftime(DATE '2000-01-01' + slot, '%H:%M'), 'rostered', rostered, NULL FROM cc_erlang
WHERE queue = getvariable('queue')
  AND ds = (SELECT ds FROM cc_day WHERE queue = getvariable('queue') ORDER BY short_by DESC, ds LIMIT 1)
ORDER BY 1, 2;

SELECT 6::COL;
SELECT 340::HEIGHT;
WITH pk AS (
  SELECT * FROM cc_erlang WHERE queue = getvariable('queue') ORDER BY erlangs DESC LIMIT 1),
n AS (SELECT pk.*, r.k FROM pk, range(1, 90) r(k) WHERE r.k > pk.erlangs AND r.k <= pk.erlangs + 14),
t AS (
  SELECT n.*, exp(n.k * ln(n.erlangs) - lgamma(n.k + 1))
              / (SELECT sum(exp(j * ln(n.erlangs) - lgamma(j + 1))) FROM range(0, n.k + 1) jj(j)) AS b,
         (SELECT aht_s FROM cc_queue q WHERE q.queue = n.queue) AS aht
  FROM n)
SELECT k ::XAXIS, round(100 * (1 - (k * b / (k - erlangs * (1 - b))) * exp(-(k - erlangs) * getvariable('answer_s')::DOUBLE / aht)), 1) ::LINECHART,
       getvariable('sl_target_pct')::DOUBLE ::REFLINE, ' %'::YFORMAT,
       ('Erlang C at the busiest half-hour (' || round(any_value(calls) OVER ())::INTEGER || ' calls): service level by agents on the phone — each agent near the target matters') ::TITLE
FROM t ORDER BY k;

-- ─────────────────────────────────────────────────────────────────────────────
-- 3 · Backtest
-- ─────────────────────────────────────────────────────────────────────────────
SELECT 'Backtest'::TAB;

SELECT 12::COL;
SELECT 'Four rolling folds, each forecasting three weeks of daily contacts from the data before it, scored on open days with **ts_mape** and **ts_bias**. **SeasonalNaive** (same weekday last week) is the benchmark. The models'' own intervals are far too wide to staff to, so the plan uses the **quantiles of actual ÷ forecast** over these folds instead — P50 also corrects a systematic bias.'::MARKDOWN,
       'How to read it'::TITLE;

SELECT 6::COL;
SELECT 320::HEIGHT;
SELECT queue ::XAXIS, method ::CATEGORY, round(mape, 1) ::BARCHART, ' %'::YFORMAT,
       'Backtest error (MAPE) by queue and method — Billing is hardest (the bill-run spike)'::TITLE
FROM cc_score ORDER BY queue, method;

SELECT 6::COL;
SELECT 320::HEIGHT;
SELECT queue ::XAXIS, round(100 * p50, 1) AS "P50" ::SCATTER, round(100 * p10, 1) ::YMIN, round(100 * p90, 1) ::YMAX,
       100 ::REFLINE, ' %'::YFORMAT, 1 ::FLIP,
       'Actual as % of forecast in the backtest (dot = median, bar = P10–P90) — the planning quantile'::TITLE
FROM cc_ratio ORDER BY queue;

SELECT 12::COL;
SELECT 'Accuracy: mape, bias, rmse'::COLGROUP;
SELECT 'mape > 12'::FLAG;
SELECT 'queue'::SUBTOTAL;
SELECT 'mape: mean, bias: mean, rmse: mean'::TOTALS;
SELECT s.queue AS "Queue" ::TABLE, s.method AS "Method",
       CASE WHEN s.method = w.method THEN 'ok: chosen' ELSE '' END AS "Use" ::BADGE,
       round(s.mape, 1) AS mape, round(s.bias_pct, 1) AS bias ::COLORSCALE_DIVERGING, round(s.rmse) AS rmse,
       'Every queue × method over 4 folds (MAPE and bias in %, RMSE in contacts per day)'::TITLE
FROM cc_score s JOIN cc_winner w USING (queue)
ORDER BY s.queue, s.mape;

-- ─────────────────────────────────────────────────────────────────────────────
-- 4 · Roster recommendation (decision)
-- ─────────────────────────────────────────────────────────────────────────────
SELECT 'Roster recommendation'::TAB;

SELECT 12::COL;
SELECT ('**Recommendation.** With the current shift plan ' || (SELECT count(*) FROM cc_day WHERE short_by > 0) || ' of '
        || (SELECT count(*) FROM cc_day) || ' queue-days are short at some point. Biggest gaps: '
        || (SELECT string_agg('**' || queue || ' ' || strftime(ds, '%a %d %b') || '** (+' || short_by || ' at ' || strftime(DATE '2000-01-01' + worst_slot, '%H:%M') || ')', ', ' ORDER BY short_by DESC)
            FROM (SELECT * FROM cc_day WHERE short_by > 0 ORDER BY short_by DESC LIMIT 3))
        || '. Add the agents to the shift that covers the worst half-hour (table below); lower the planning quantile or the service level above to see the cost of a leaner roster.')::MARKDOWN,
       'What to do'::TITLE;

SELECT 12::COL;
SELECT 'Short by: top 3'::HIGHLIGHT;
SELECT 'Short by > 0, Service level as rostered (%) < 70'::FLAG;
SELECT 'Week'::SUBTOTAL;
SELECT 'Contacts: sum, Peak agents needed: max, Rostered at the gap: max, Short by: max, Half-hours short: sum, Service level as rostered (%): mean'::TOTALS;
SELECT week AS "Week" ::TABLE, strftime(ds, '%a %d %b') AS "Day", queue AS "Queue",
       round(contacts) AS "Contacts", peak_heads AS "Peak agents needed", rostered_at_gap AS "Rostered at the gap",
       greatest(short_by, 0) AS "Short by", slots_short AS "Half-hours short",
       round(sl_rostered, 1) AS "Service level as rostered (%)" ::COLORSCALE_HIGH_GOOD,
       CASE WHEN short_by <= 0 THEN 'ok'
            WHEN isodow(ds) = 6 THEN '+' || short_by || ' Saturday'
            WHEN worst_slot < TIME '09:00' THEN '+' || short_by || ' early'
            WHEN worst_slot < TIME '11:30' THEN '+' || short_by || ' early / mid'
            WHEN worst_slot >= TIME '17:30' THEN '+' || short_by || ' evening'
            ELSE '+' || short_by || ' any shift' END AS "Add agents (shift)",
       'Per day and queue — where the current roster falls short and what to add'::TITLE
FROM cc_day WHERE short_by > 0 OR sl_rostered < getvariable('sl_target_pct')::DOUBLE
ORDER BY ds, queue;
