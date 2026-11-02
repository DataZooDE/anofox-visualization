-- @title Which materials will run short next quarter?
-- @group Demand & stock
-- @tags  stock, forecast, prediction intervals
-- @description Each material's demand forecast for the next 13 weeks with prediction intervals, against its reorder point and supplier lead time.
--
-- Which materials will run short next quarter? The inventory twin's 12
-- materials (web/data/inventory-twin/trajectory.csv: daily demand and stock,
-- March–June 2024), reordered as before: when the stock position (on hand +
-- on order − backorders) falls to a reorder point, an order of a fixed size
-- goes out and arrives after the supplier's lead time. Per material:
--   * weekly demand (full weeks only), forecast 13 weeks ahead with AutoETS
--     (anofox_forecast ts_forecast_by) and its 95 % prediction interval; the
--     80 % and 50 % intervals follow from it (normal errors: z 1.96 / 1.28 /
--     0.67);
--   * the reorder point and order size as seen in the history (the position
--     the day before each order, the order sizes), the lead time as the twin
--     last calibrated it (supplier.lead_time_mean / _std_dev), else as
--     measured from order to delivery in the history;
--   * an order cycle runs short when the demand during the lead time exceeds
--     the reorder point: its mean is the forecast's weekly mean × lead time,
--     its spread from the forecast's one-week interval and the lead-time
--     spread;
--   * the chance of running short at least once next quarter =
--     1 − (1 − chance per cycle)^(orders next quarter), the orders being the
--     quarter's median demand / the order size. Φ by a logistic
--     approximation (error < 0.01).
--
-- Natively, run from web/ with a duckdb that has anofox_forecast installed
-- (see examples/forecast-energy-load/README.md):
--   dashboard --check --strict examples/materials-quarter-shortage.sql

LOAD anofox_forecast;

SELECT 'Which materials will run short next quarter?'::DASHBOARD_TITLE;

-- ── Data ──────────────────────────────────────────────────────────────────────
CREATE OR REPLACE TABLE mq_days AS FROM read_csv('data/inventory-twin/trajectory.csv');

-- Weekly demand, full weeks only (the history starts and ends mid-week).
CREATE OR REPLACE TABLE mq_weeks AS
SELECT sku, date_trunc('week', ds)::DATE AS week, sum(demand)::DOUBLE AS demand
FROM mq_days GROUP BY ALL HAVING count(DISTINCT ds) = 7;

-- 13 weeks ahead, 95 % intervals; σ per week from the interval's half width.
CREATE OR REPLACE TABLE mq_fc AS
SELECT sku, forecast_step AS step, ds AS week, greatest(yhat, 0) AS yhat,
       (yhat_upper - yhat_lower) / (2 * 1.96) AS sigma
FROM ts_forecast_by('mq_weeks', sku, week, demand, 'AutoETS', 13, '1w', MAP{'confidence_level': '0.95'});

-- Standard normal CDF, logistic approximation.
CREATE OR REPLACE MACRO mq_phi(z) AS 1 / (1 + exp(-1.702 * z));

-- The reorder policy per material. Orders show as a rise in on_order_end;
-- deliveries as a fall. Lead time: the twin's last calibration, else measured.
CREATE OR REPLACE TABLE mq_pol AS
WITH ev AS (
  SELECT sku, ds, on_order_end - lag(on_order_end) OVER w AS dq, lag(position_end) OVER w AS pos_before
  FROM mq_days WINDOW w AS (PARTITION BY sku ORDER BY ds)
),
measured AS (
  SELECT o.sku, avg(a.ds - o.ds) AS lt_mean, stddev(a.ds - o.ds) AS lt_sd
  FROM ev o, LATERAL (SELECT min(ds) AS ds FROM ev d WHERE d.sku = o.sku AND d.ds > o.ds AND d.dq < 0) a
  WHERE o.dq > 0 AND a.ds IS NOT NULL GROUP BY o.sku
),
twin AS (
  SELECT sku, arg_max(after_num, period) FILTER (WHERE parameter = 'supplier.lead_time_mean') AS lt_mean,
         arg_max(after_num, period) FILTER (WHERE parameter = 'supplier.lead_time_std_dev') AS lt_sd
  FROM read_csv('data/inventory-twin/parameter_changes.csv') WHERE status = 'applied' GROUP BY sku
)
SELECT ev.sku, avg(pos_before) FILTER (WHERE dq > 0) AS rop, avg(dq) FILTER (WHERE dq > 0) AS q,
       coalesce(any_value(t.lt_mean), any_value(m.lt_mean)) AS lt_mean,
       coalesce(any_value(t.lt_sd), any_value(m.lt_sd), 0) AS lt_sd,
       CASE WHEN any_value(t.lt_mean) IS NULL THEN 'measured' ELSE 'twin' END AS lt_source
FROM ev LEFT JOIN twin t USING (sku) LEFT JOIN measured m USING (sku)
GROUP BY ev.sku;

-- Today's stock position per material (the last day of the history).
CREATE OR REPLACE TABLE mq_pos AS
SELECT sku, max(ds) AS today, arg_max(position_end, ds) AS position
FROM mq_days GROUP BY sku;

-- Per material: the quarter's demand with its spread (weekly errors taken as
-- independent), the demand during a lead time, the chance per cycle and per
-- quarter, the reorder point a 1-in-20 chance per cycle would need, and when
-- the next order goes out (the median position reaching the reorder point).
CREATE OR REPLACE TABLE mq_risk AS
WITH f AS (
  SELECT sku, sum(yhat) AS demand_q, sqrt(sum(sigma * sigma)) AS sigma_q,
         avg(yhat) AS mu_w, arg_min(sigma, step) AS sigma_w
  FROM mq_fc GROUP BY sku
),
l AS (
  SELECT p.*, f.demand_q, f.sigma_q, f.mu_w,
         f.mu_w * p.lt_mean / 7 AS ltd,
         sqrt(p.lt_mean / 7 * f.sigma_w ^ 2 + (f.mu_w / 7) ^ 2 * p.lt_sd ^ 2) AS ltd_sd
  FROM mq_pol p JOIN f USING (sku)
)
SELECT l.*, s.position, s.today,
       mq_phi((ltd - rop) / greatest(ltd_sd, 1e-9)) AS p_cycle,
       demand_q / q AS orders_q,
       1 - (1 - mq_phi((ltd - rop) / greatest(ltd_sd, 1e-9))) ^ (demand_q / q) AS p_short,
       ltd + 1.645 * ltd_sd AS rop_95,
       s.today + CAST(greatest(s.position - rop, 0) / greatest(mu_w / 7, 1e-9) AS INTEGER) AS next_order
FROM l JOIN mq_pos s USING (sku);

CREATE OR REPLACE MACRO mq_sku() AS
  coalesce(nullif(getvariable('Material'), ''), (SELECT sku FROM mq_risk ORDER BY p_short DESC, sku LIMIT 1));

-- ── Headline ──────────────────────────────────────────────────────────────────
SELECT 'Next quarter at a glance'::GROUP;
SELECT count(*) FILTER (WHERE p_short >= 0.5)::METRIC, 'materials more likely than not to run short'::LABEL,
       '>=3:bad; >=1:warn; good'::THRESHOLDS,
       'Of the 12 materials, those with a chance of 50 % or more of running short at least once in the next 13 weeks, if they keep being reordered as before.'::INFO
FROM mq_risk;
SELECT round(sum(p_cycle * orders_q), 1)::METRIC, 'orders expected to run short next quarter'::LABEL,
       'Over all 12 materials: the orders expected next quarter (median demand / order size) times each one''s chance of running short before its delivery arrives.'::INFO
FROM mq_risk;
SELECT count(*) FILTER (WHERE rop < rop_95)::METRIC, 'reorder points below a 1-in-20 risk per order'::LABEL,
       '>=3:bad; >=1:warn; good'::THRESHOLDS,
       'Materials whose reorder point is lower than the demand during a lead time at its 95 % upper bound, so more than 1 order in 20 runs short.'::INFO
FROM mq_risk;
SELECT 1::ENDGROUP;

-- ── Ranking ───────────────────────────────────────────────────────────────────
SELECT 12::COL;
SELECT 'Chance next quarter: >=50:bad; >=10:warn; good'::THRESHOLDS;
SELECT 'single'::ROWSELECT;
SELECT sku AS "Material" ::TABLE,
       round(demand_q)::INTEGER AS "Demand next quarter",
       round(greatest(demand_q - 1.2816 * sigma_q, 0))::INTEGER || '–' || round(demand_q + 1.2816 * sigma_q)::INTEGER AS "80 % interval",
       round(rop)::INTEGER AS "Reorder point",
       round(lt_mean, 1) AS "Lead time (days)",
       round(ltd)::INTEGER || ' (up to ' || round(ltd + 1.645 * ltd_sd)::INTEGER || ')' AS "Demand in a lead time",
       round(100 * p_cycle, 1) AS "Chance per order" ::PERCENT,
       round(100 * p_short, 1) AS "Chance next quarter" ::PERCENT,
       round(rop_95)::INTEGER AS "Reorder point for 1 in 20",
       strftime(next_order, '%-d %b') AS "Next order",
       'Materials by their chance of running short next quarter (click one for its forecast)'::TITLE,
       'Demand next quarter: the sum of the 13 weekly AutoETS forecasts (median), with the 80 % prediction interval of that sum. Reorder point: the stock position at which orders went out in the history. Lead time: the twin''s last calibration, or measured from order to delivery. Demand in a lead time: its median and 95 % upper bound. Chance per order: that this demand exceeds the reorder point, so stock runs out before the delivery. Chance next quarter: of at least one such order among the quarter''s orders (median demand / the usual order size). Reorder point for 1 in 20: the one at which 1 order in 20 runs short. Next order: when the median forecast brings the position down to the reorder point.'::INFO
FROM mq_risk ORDER BY p_short DESC, sku;

-- ── One material ──────────────────────────────────────────────────────────────
SELECT 6::COL;
SELECT 340::HEIGHT;
SELECT week ::XAXIS, 'Weekly demand' ::CATEGORY, demand ::LINECHART,
       NULL::DOUBLE ::BAND_LOWER, NULL::DOUBLE ::BAND_UPPER,
       NULL::DOUBLE ::BAND_LOWER, NULL::DOUBLE ::BAND_UPPER,
       NULL::DOUBLE ::BAND_LOWER, NULL::DOUBLE ::BAND_UPPER,
       (mq_sku() || ': weekly demand and its forecast (median, bands 95 / 80 / 50 %)')::TITLE,
       'The last 15 full weeks and the AutoETS forecast for the next 13, with its 95 %, 80 % and 50 % prediction intervals.'::INFO
FROM mq_weeks WHERE sku = mq_sku()
UNION ALL
SELECT week, 'Forecast (median)', yhat,
       greatest(yhat - 1.96 * sigma, 0), yhat + 1.96 * sigma,
       greatest(yhat - 1.2816 * sigma, 0), yhat + 1.2816 * sigma,
       greatest(yhat - 0.6745 * sigma, 0), yhat + 0.6745 * sigma, NULL, NULL
FROM mq_fc WHERE sku = mq_sku()
ORDER BY 1;

-- The demand during one lead time (normal, ±4σ): the part above the reorder
-- point is the chance an order runs short.
SELECT 6::COL;
SELECT 340::HEIGHT;
WITH r AS (SELECT * FROM mq_risk WHERE sku = mq_sku()),
grid AS (
  SELECT greatest(r.ltd - 4 * r.ltd_sd, 0) + i * (8 * r.ltd_sd / 120) AS x, r.ltd, r.ltd_sd, r.rop
  FROM r, range(121) t(i)
)
SELECT round(x, 1) ::XAXIS,
       CASE WHEN x <= rop THEN 'covered by the reorder point' ELSE 'more than the reorder point: runs short' END ::CATEGORY,
       exp(-0.5 * ((x - ltd) / ltd_sd) ^ 2) / (ltd_sd * sqrt(2 * pi())) ::AREACHART, 'none'::POINTS,
       round(rop) ::XLINE, 'reorder point'::XLINE_LABEL,
       'units'::XTITLE,
       (mq_sku() || ': demand during one lead time against the reorder point')::TITLE,
       'The demand between placing an order and its delivery, from the forecast''s weekly mean and spread and the lead time''s mean and spread (normal approximation). The shaded part to the right of the reorder point is the chance that an order runs short.'::INFO
FROM grid ORDER BY x;
