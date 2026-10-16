-- Period & KPIs: one ::PERIOD input drives the whole page — a date range from
-- presets (last 30 days, month/quarter/year to date, last quarter, …, or a
-- custom range), a time grain (day/week/month/quarter) and a comparison
-- (previous period or the same period last year). The KPIs compare the range
-- with the comparison window; the chart buckets by the chosen grain.
--
-- The ::PERIOD column's alias (`p`) is the variable prefix: p_from / p_to,
-- p_prev_from / p_prev_to (NULL with "No comparison"), p_grain, p_compare,
-- p_label and p_prev_label — all text, dates as YYYY-MM-DD (cast with ::DATE).
--
-- Synthetic, deterministic data: two years of daily orders (Oct 2024 – Sep
-- 2026) for three channels, with growth, a December peak, quiet weekends and
-- hash noise. `today=2026-09-30` pins "today" to the last day of the data; a
-- real dashboard leaves it out (the viewer's date is used).

CREATE OR REPLACE TABLE shop AS
WITH d AS (SELECT (DATE '2024-10-01' + INTERVAL (i) DAY)::DATE AS day, i FROM range(0, 730) t(i)),
c(channel, cf, aov) AS (VALUES ('Online', 1.0, 58.0), ('Store', 0.7, 74.0), ('Marketplace', 0.4, 39.0))
SELECT d.day, c.channel,
       round(140 * c.cf
             * (1 + CASE c.channel WHEN 'Online' THEN 0.35 WHEN 'Store' THEN 0.05 ELSE 0.2 END * d.i / 365.0)
             * (1 + 0.3 * exp(-power((dayofyear(d.day) - 345) / 20.0, 2)))
             * CASE WHEN dayofweek(d.day) IN (0, 6) THEN 0.7 ELSE 1.1 END
             * (0.85 + 0.3 * (hash(d.day::VARCHAR || c.channel) % 1000) / 1000.0))::INT AS orders,
       c.aov * (0.95 + 0.1 * (hash(c.channel || d.i::VARCHAR) % 1000) / 1000.0) AS aov
FROM d, c;

-- The selected range and the comparison window, as reusable predicates.
CREATE OR REPLACE MACRO in_period(d) AS d BETWEEN getvariable('p_from')::DATE AND getvariable('p_to')::DATE;
CREATE OR REPLACE MACRO in_prev(d) AS d BETWEEN getvariable('p_prev_from')::DATE AND getvariable('p_prev_to')::DATE;

SELECT 'Period & KPIs — this period vs the previous one'::LABEL;

SELECT 12::COL;
SELECT 'One **::PERIOD** input: pick a **date range** (presets or *Custom range*), a **grain** (Day / Week / Month / Quarter) and a **comparison** (*Previous period* or *Same period last year*). It sets **getvariable(''p_from'')** / **''p_to''**, the comparison window **''p_prev_from''** / **''p_prev_to''** and the grain **''p_grain''**, so each KPI is `sum(x) FILTER (WHERE in_period(day))` with `… FILTER (WHERE in_prev(day))` as its **::DELTA**, and the chart buckets with `date_trunc(getvariable(''p_grain''), day)`.'::MARKDOWN, 'What this shows'::TITLE;

SELECT 'presets=7d,30d,90d,mtd,qtd,ytd,last_month,last_quarter,12m; default=30d; compare=prev; today=2026-09-30' AS p ::PERIOD, 'Period'::LABEL;

SELECT 'This period vs the comparison'::GROUP;
SELECT sum(orders * aov) FILTER (WHERE in_period(day))::MONEY, sum(orders * aov) FILTER (WHERE in_prev(day))::DELTA, 'Revenue'::LABEL
FROM shop;
SELECT sum(orders) FILTER (WHERE in_period(day))::COMPACT, sum(orders) FILTER (WHERE in_prev(day))::DELTA, 'Orders'::LABEL
FROM shop;
SELECT (sum(orders * aov) FILTER (WHERE in_period(day)) / sum(orders) FILTER (WHERE in_period(day)))::MONEY,
       (sum(orders * aov) FILTER (WHERE in_prev(day)) / sum(orders) FILTER (WHERE in_prev(day)))::DELTA, 'Average order'::LABEL
FROM shop;
SELECT (100 * sum(orders) FILTER (WHERE in_period(day) AND channel = 'Online') / sum(orders) FILTER (WHERE in_period(day)))::PERCENT,
       (100 * sum(orders) FILTER (WHERE in_prev(day) AND channel = 'Online') / sum(orders) FILTER (WHERE in_prev(day)))::DELTA, 'Online share'::LABEL
FROM shop;
SELECT 1::ENDGROUP;

-- The comparison window is shifted onto the selected range, so both lines
-- share the x axis; its series is named by p_prev_label ("vs Aug 2026") —
-- no rows with "No comparison".
SELECT 8::COL;
SELECT date_trunc(getvariable('p_grain'), day)::DATE ::XAXIS, 'This period' ::CATEGORY, sum(orders * aov) / 1000 ::LINECHART, '$'::YFORMAT,
       'Revenue per ' || getvariable('p_grain') || ' ($k), ' || getvariable('p_label') ::TITLE
FROM shop WHERE in_period(day) GROUP BY ALL
UNION ALL
SELECT date_trunc(getvariable('p_grain'), day + (getvariable('p_from')::DATE - getvariable('p_prev_from')::DATE)::INTEGER)::DATE, getvariable('p_prev_label'),
       sum(orders * aov) / 1000, '$', 'Revenue per ' || getvariable('p_grain') || ' ($k), ' || getvariable('p_label')
FROM shop WHERE in_prev(day) GROUP BY ALL
ORDER BY 1, 2;

SELECT 4::COL;
SELECT channel ::XAXIS, period ::CATEGORY, revenue / 1000 ::BARCHART, '$'::YFORMAT, 'Revenue by channel ($k)'::TITLE
FROM (SELECT channel, 'This period' AS period, sum(orders * aov) AS revenue FROM shop WHERE in_period(day) GROUP BY ALL
      UNION ALL
      SELECT channel, getvariable('p_prev_label'), sum(orders * aov) FROM shop WHERE in_prev(day) GROUP BY ALL)
ORDER BY channel, period;
