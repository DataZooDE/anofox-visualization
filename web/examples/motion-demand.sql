-- How will demand develop after today? A weekly demand outlook whose charts
-- move with the story they tell:
--   * the outlook chart wipes in up to "Today", pauses, then the forecast fan
--     grows forward from it ('reveal'::ANIMATE + a DATE ::XLINE), and the
--     forecast line is drawn with moving dashes ('flow'::LINESTYLE);
--   * the weekly bars grow in week by week (the default 'auto' entrance), and
--     a ::STORY walks through three weeks: Step / Play light one week up and
--     dim the rest, with a caption pointing at it.
-- Motion is browser only — PNG/PDF/--svg output is static — and under
-- "reduce motion" every chart shows its final state at once.
--
-- Synthetic, deterministic data: 41 weeks of history up to Today
-- (2026-10-12) and an 11-week forecast with a widening 80 % interval, for two
-- channels.

CREATE OR REPLACE TABLE md_weeks AS
WITH w AS (SELECT i, (DATE '2026-01-05' + INTERVAL (7 * i) DAY)::DATE AS week FROM range(0, 52) t(i)),
c(channel, base, growth) AS (VALUES ('Online', 420.0, 2.6), ('Store', 610.0, -0.8))
SELECT w.week, w.i, c.channel,
       round(c.base + c.growth * w.i + 55 * sin(w.i / 4.0)
             + 30 * ((hash(w.week::VARCHAR || c.channel) % 1000) / 1000.0 - 0.5)) AS demand,
       w.i > 40 AS future
FROM w, c;

SELECT 'How will demand develop after today?'::DASHBOARD_TITLE;

CREATE OR REPLACE TABLE md_total AS
SELECT week, i, future, sum(demand) AS demand FROM md_weeks GROUP BY ALL;

SELECT 12::COL;
SELECT week ::XAXIS,
       CASE WHEN future THEN 'Forecast' ELSE 'Observed' END ::CATEGORY,
       demand ::LINECHART,
       demand - CASE WHEN future THEN 9.0 * (i - 40) ELSE 0 END ::BAND_LOWER,
       demand + CASE WHEN future THEN 11.0 * (i - 40) ELSE 0 END ::BAND_UPPER,
       DATE '2026-10-12' ::XLINE,
       CASE WHEN future THEN 'flow' END ::LINESTYLE,
       'reveal' ::ANIMATE,
       'Weekly demand — observed up to Today, forecast with 80 % interval' ::TITLE
FROM md_total
UNION ALL
-- the forecast starts where the history ends, so the two lines join at Today
SELECT week, 'Forecast', demand, demand, demand, DATE '2026-10-12', 'flow', 'reveal', ''
FROM md_total WHERE i = 40
ORDER BY 1, 2;

SELECT 12::COL;
SELECT strftime(week, 'W%V') ::XAXIS, channel ::CATEGORY, demand ::BARCHART,
       CASE strftime(week, 'W%V')
         WHEN 'W43' THEN 'W43, the first forecast week: Store still sells about 50 units a week more than Online'
         WHEN 'W48' THEN 'W48: Online dips for one week — a quiet slot for the shop migration'
         WHEN 'W53' THEN 'W53, year end: both channels reach their highest forecast demand'
       END ::STORY,
       'Forecast demand per channel, next 11 weeks — step through the story' ::TITLE
FROM md_weeks WHERE future ORDER BY week, channel;
