-- When could we run out of stock? One item's on-hand stock with a 4-week
-- projection, and what happened around it, on one shared time axis:
--   * the projection is a nested quantile fan — 95 %, 80 % and 50 % bands
--     around the median (three ::BAND_LOWER / ::BAND_UPPER pairs; the inner
--     bands read darker);
--   * past stockouts are shaded — one region per stockout spell from a single
--     CASE WHEN on_hand = 0 THEN day END ::MARKAREA column;
--   * three strips under the chart share its x axis exactly (::TRACK):
--     promotion starts as ticks, the monitoring alarms as a heatmap with one
--     lane per detector (amber = warning, red = alarm), and the price as a
--     step line. Click an alarm tile to shade its day on the chart above.
--
-- Synthetic, deterministic data (hash-based noise, no files): 14 weeks of
-- history up to Today (2026-10-12) and a 28-day projection stored long, one
-- row per day and quantile level — the shape a simulation writes — and turned
-- into one column pair per level with FILTER.

CREATE OR REPLACE MACRO so_noise(k) AS (hash(k::VARCHAR) % 1000) / 1000.0 - 0.5;

-- Promotion weeks: demand runs ~70 % higher.
CREATE OR REPLACE TABLE so_promos AS SELECT * FROM (VALUES
  (DATE '2026-07-20', 'Summer sale'),
  (DATE '2026-08-31', 'Back to school'),
  (DATE '2026-09-28', 'Autumn coupon'),
  (DATE '2026-10-26', 'Black week preview (planned)')
) t(day, promotion);

-- 14 weeks of days: demand, and a delivery of 280 every 14 days — except the
-- one due on 31 Aug, which the supplier delivered four days late.
CREATE OR REPLACE TABLE so_days AS
SELECT i, day, promo,
       round((CASE WHEN promo THEN 34 ELSE 20 END) * (1 + 0.3 * so_noise(i))) AS demand,
       CASE WHEN i = 60 OR (i % 14 = 0 AND i <> 56) THEN 280 ELSE 0 END AS receipt
FROM (
  SELECT i, (DATE '2026-07-06' + INTERVAL (i) DAY)::DATE AS day,
         EXISTS (SELECT 1 FROM so_promos p
                 WHERE (DATE '2026-07-06' + INTERVAL (i) DAY)::DATE BETWEEN p.day AND p.day + 6) AS promo
  FROM range(0, 98) t(i)
);

-- End-of-day stock: yesterday's + today's delivery − what sold (never < 0).
CREATE OR REPLACE TABLE so_hist AS
WITH RECURSIVE s(i, on_hand) AS (
  SELECT 0, 60 + receipt - demand FROM so_days WHERE i = 0
  UNION ALL
  SELECT d.i, greatest(0, s.on_hand + d.receipt - d.demand) FROM s JOIN so_days d ON d.i = s.i + 1
)
SELECT d.day, d.i, d.promo, s.on_hand, CASE WHEN d.promo THEN 7.49 ELSE 9.99 END AS price
FROM s JOIN so_days d USING (i);

-- The projection: end-of-day on-hand quantiles per future day, widening with
-- the horizon (levels 0.5 / 0.8 / 0.95 = central intervals); deliveries on
-- 13 and 27 Oct, the planned promotion from 26 Oct.
CREATE OR REPLACE TABLE so_projection AS
WITH h AS (SELECT h, (DATE '2026-10-12' + INTERVAL (h) DAY)::DATE AS day FROM range(1, 29) t(h)),
m AS (SELECT h, day, (SELECT on_hand FROM so_hist ORDER BY i DESC LIMIT 1)
                     + 300 * (1 + (h >= 15)::INT) - 17 * h - 8 * greatest(0, h - 13) AS yhat FROM h),
lv(level, z) AS (VALUES (0.5, 0.674), (0.8, 1.2816), (0.95, 1.96))
SELECT day, level, greatest(0, yhat) AS yhat,
       greatest(0, yhat - z * 12 * sqrt(h)) AS lower,
       greatest(0, yhat + z * 12 * sqrt(h)) AS upper
FROM m, lv;

-- Monitoring alarms per detector and day: demand above plan during the
-- promotions, the late delivery on the lead-time detector.
CREATE OR REPLACE TABLE so_alarms AS
SELECT day, 'Demand drift' AS detector,
       CASE WHEN demand >= 36 THEN 'alarm' ELSE 'warning' END AS tier, 'demand above plan' AS reason
FROM so_days WHERE promo
UNION ALL
SELECT day, 'Lead time', CASE WHEN i >= 58 THEN 'alarm' ELSE 'warning' END, 'delivery overdue'
FROM so_days WHERE i BETWEEN 57 AND 59;

SELECT 'When could we run out of stock?'::DASHBOARD_TITLE;

SELECT 4::COL;
SELECT count(*) FILTER (WHERE on_hand = 0)::METRIC, 'Days out of stock, last 14 weeks'::LABEL FROM so_hist;
SELECT 4::COL;
SELECT min(day)::TEXT_MEDIUM, 'First day the 95 % band reaches zero'::LABEL
FROM so_projection WHERE level = 0.95 AND lower = 0;
SELECT 4::COL;
SELECT count(*)::METRIC, 'Alarm days, last 14 weeks'::LABEL FROM so_alarms WHERE tier = 'alarm';

SELECT 12::COL;
WITH fan AS (
  SELECT day, any_value(yhat) AS yhat,
         max(lower) FILTER (WHERE level = 0.95) AS lo95, max(upper) FILTER (WHERE level = 0.95) AS hi95,
         max(lower) FILTER (WHERE level = 0.8)  AS lo80, max(upper) FILTER (WHERE level = 0.8)  AS hi80,
         max(lower) FILTER (WHERE level = 0.5)  AS lo50, max(upper) FILTER (WHERE level = 0.5)  AS hi50
  FROM so_projection GROUP BY day
)
SELECT day ::XAXIS,
       'On hand' ::CATEGORY,
       on_hand ::LINECHART,
       NULL::DOUBLE ::BAND_LOWER, NULL::DOUBLE ::BAND_UPPER,
       NULL::DOUBLE ::BAND_LOWER, NULL::DOUBLE ::BAND_UPPER,
       NULL::DOUBLE ::BAND_LOWER, NULL::DOUBLE ::BAND_UPPER,
       CASE WHEN on_hand = 0 THEN day END ::MARKAREA,
       DATE '2026-10-12' ::XLINE,
       'Today' ::XLINE_LABEL,
       'On-hand stock with a 4-week projection (bands: 95 / 80 / 50 %, grey: out of stock)' ::TITLE
FROM so_hist
UNION ALL
SELECT day, 'Projected (median)', yhat, lo95, hi95, lo80, hi80, lo50, hi50, NULL, NULL, NULL, NULL
FROM fan
ORDER BY 1;
SELECT day ::XAXIS, 'Promotions' ::TRACK, promotion ::LABEL FROM so_promos;
SELECT day ::XAXIS, detector ::YAXIS,
       CASE tier WHEN 'alarm' THEN '#d64545' ELSE '#e8a23b' END ::CATEGORY,
       1 ::HEATMAP,
       tier || ': ' || reason ::LABEL,
       'Alarms' ::TRACK
FROM so_alarms;
SELECT 56::HEIGHT;
SELECT day ::XAXIS, price ::STEP, 'Price' ::TRACK FROM so_hist;
