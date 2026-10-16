-- How did our bakeries sell last week? — the weekly sales review of a
-- made-up chain of 12 bakeries in three regions. Reads the session's tables
-- (stores, products, sales), i.e. the server's snapshot: a new week shows up
-- after `serve.sh week` (or the next interval refresh).

SELECT 'How did our bakeries sell last week?'::DASHBOARD_TITLE;
SELECT 'en-GB'::LOCALE;
SELECT max(loaded_at)::ASOF, 'stale=8d' FROM sales;

SELECT r AS region ::DROPDOWN, 'Region'::LABEL
FROM (SELECT 'All regions' AS r, 0 AS o UNION ALL SELECT DISTINCT region, 1 FROM stores)
ORDER BY o, r;

-- The sales of the chosen region, by week.
CREATE OR REPLACE TEMP VIEW wk AS
SELECT date_trunc('week', s.day)::DATE AS week, st.region, s.store, p.category, s.product,
       sum(s.units) AS units, sum(s.revenue) AS revenue
FROM sales s JOIN stores st USING (store) JOIN products p USING (product)
WHERE getvariable('region') IN ('All regions', st.region)
GROUP BY ALL;

SELECT 'Last week'::GROUP;
SELECT sum(revenue) FILTER (WHERE week = (SELECT max(week) FROM wk))::MONEY,
       sum(revenue) FILTER (WHERE week = (SELECT max(week) - 7 FROM wk))::DELTA,
       'vs the week before'::DELTA_LABEL,
       (SELECT list(r ORDER BY week) FROM (SELECT week, sum(revenue) AS r FROM wk GROUP BY week))::SPARKLINE,
       'Revenue'::LABEL
FROM wk;
SELECT sum(units) FILTER (WHERE week = (SELECT max(week) FROM wk))::METRIC,
       sum(units) FILTER (WHERE week = (SELECT max(week) - 7 FROM wk))::DELTA,
       'vs the week before'::DELTA_LABEL,
       'Items sold'::LABEL
FROM wk;
SELECT avg(r)::MONEY, 'Revenue per store'::LABEL
FROM (SELECT store, sum(revenue) AS r FROM wk WHERE week = (SELECT max(week) FROM wk) GROUP BY store);
SELECT 1::ENDGROUP;

SELECT 8::COL;
SELECT week::XAXIS, region::CATEGORY, sum(revenue)::LINECHART, '£'::YFORMAT, 'Weekly revenue by region'::TITLE
FROM wk GROUP BY ALL ORDER BY region, week;

SELECT 4::COL;
SELECT store::XAXIS,
       round(100 * (sum(revenue) FILTER (WHERE week = (SELECT max(week) FROM wk))
                    / sum(revenue) FILTER (WHERE week = (SELECT max(week) - 7 FROM wk)) - 1), 1)::BARCHART,
       1::FLIP, 0::REFLINE, 'Change vs the week before (%)'::TITLE
FROM wk GROUP BY store ORDER BY 2;

SELECT 12::COL;
SELECT week::XAXIS, category::CATEGORY, sum(units)::BARCHART_STACKED, 'Items sold per week'::TITLE
FROM wk GROUP BY ALL ORDER BY category, week;

SELECT 12::COL;
SELECT s.day AS "Day", st.region AS "Region", s.store AS "Store", s.product AS "Product",
       s.revenue AS "Revenue" ::MONEY, 'Daily sales by store and product'::TITLE,
       s.units AS "Items" ::PAGED
FROM sales s JOIN stores st USING (store)
WHERE getvariable('region') IN ('All regions', st.region)
ORDER BY s.day DESC, s.store, s.product;
