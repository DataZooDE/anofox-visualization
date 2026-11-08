CREATE TABLE sales AS
SELECT DATE '2026-01-05' + INTERVAL (w * 7) DAY AS week, channel,
       round(100 + w * 4 + (CASE channel WHEN 'Web' THEN 40 WHEN 'Store' THEN 10 ELSE -20 END) + 12 * sin(w / 2.0)) AS revenue
FROM range(0, 26) r(w), (VALUES ('Web'), ('Store'), ('Partner')) c(channel);

SELECT 'Headline'::GROUP;
SELECT sum(revenue) * 1000 ::MONEY, 'Revenue'::LABEL,
       sum(revenue) FILTER (WHERE week < DATE '2026-04-01') * 1000 * 1.6 ::DELTA,
       (SELECT list(r ORDER BY week) FROM (SELECT week, sum(revenue) AS r FROM sales GROUP BY 1)) ::SPARKLINE FROM sales;
SELECT max(r) * 1000 ::MONEY, 'Best week'::LABEL FROM (SELECT week, sum(revenue) AS r FROM sales GROUP BY 1);
SELECT round(100 * sum(revenue) FILTER (WHERE channel = 'Web') / sum(revenue), 1) ::PERCENT, 'Web share'::LABEL FROM sales;
SELECT 1::ENDGROUP;

SELECT 8::COL;
SELECT week ::XAXIS, channel ::CATEGORY, revenue ::AREACHART_STACKED, 'Web carries the growth'::TITLE FROM sales ORDER BY week;
SELECT 4::COL;
SELECT channel ::CATEGORY, sum(revenue) ::DONUTCHART, 'Channel mix'::TITLE FROM sales GROUP BY ALL ORDER BY 2 DESC;
