-- Sales overview — a well-composed reference dashboard.
-- Self-contained (generates its own data), so `dashboard --check` runs it as-is.
-- Shape: filter → KPI row → primary trend → breakdowns → detail table.

-- data: 36 months × 3 regions × 3 channels
CREATE TABLE sales AS
SELECT (DATE '2023-01-01' + to_months(m::INTEGER)) AS month,
       r.region, c.channel, c.price,
       GREATEST(0, (400 + 15*m + 50*sin(2*pi()*(m%12)/12.0)
                    + ((hash(r.region||c.channel||m::VARCHAR)%80)::INTEGER - 40)))::INTEGER AS units
FROM (VALUES ('EU'),('US'),('APAC')) r(region),
     (VALUES ('web',39.0),('app',29.0),('store',49.0)) c(channel,price),
     range(0,36) t(m);

-- Filter (drives every panel below via getvariable('region'))
SELECT 'Filter' ::GROUP;
SELECT region AS region ::DROPDOWN FROM sales GROUP BY region ORDER BY region;
SELECT 1 ::ENDGROUP;

-- KPI row: headline numbers first
SELECT ROUND(SUM(units*price),0) ::MONEY, 'Revenue' ::LABEL
FROM sales WHERE region = getvariable('region');
SELECT SUM(units) ::COMPACT, 'Units' ::LABEL
FROM sales WHERE region = getvariable('region');
SELECT ROUND(SUM(units*price)/SUM(units),2) ::MONEY, 'Avg unit price' ::LABEL
FROM sales WHERE region = getvariable('region');
SELECT COUNT(DISTINCT channel) ::METRIC, 'Channels' ::LABEL
FROM sales WHERE region = getvariable('region');

-- Primary question: revenue over time by channel (full width, € axis)
SELECT 12 ::SPAN;
SELECT month ::XAXIS, channel ::CATEGORY, ROUND(SUM(units*price),0) ::LINECHART,
       '€' ::YFORMAT, 'Monthly revenue by channel' ::TITLE
FROM sales WHERE region = getvariable('region')
GROUP BY month, channel ORDER BY month, channel;

-- Supporting breakdowns, side by side
SELECT channel ::XAXIS, ROUND(SUM(units*price),0) ::BARCHART,
       '€' ::YFORMAT, 'Revenue by channel' ::TITLE
FROM sales WHERE region = getvariable('region') GROUP BY channel ORDER BY 2 DESC;
SELECT channel ::CATEGORY, SUM(units) ::DONUTCHART, 'Unit share by channel' ::TITLE
FROM sales WHERE region = getvariable('region') GROUP BY channel;

-- Detail table (one ::TABLE marker; other columns keep their aliases)
SELECT 12 ::SPAN;
SELECT channel                          AS "Channel" ::TABLE,
       SUM(units)                       AS "Units",
       ROUND(SUM(units*price),0)        AS "Revenue €",
       ROUND(AVG(units),0)              AS "Avg units/mo"
FROM sales WHERE region = getvariable('region')
GROUP BY channel ORDER BY "Revenue €" DESC;
