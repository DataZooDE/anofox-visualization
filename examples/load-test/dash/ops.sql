-- @title Sales operations (load-test reference)
SELECT 'Sales operations'::LABEL;
SELECT 'presets=30d,90d,ytd,last_year; default=90d; today=2025-12-31; compare=prev' AS p ::PERIOD, 'Period'::LABEL;
SELECT r AS region ::BUTTONS, 'Region'::LABEL FROM (VALUES (1, 'All'), (2, 'EU'), (3, 'Americas'), (4, 'APAC'), (5, 'MEA')) t(o, r) ORDER BY o;

SELECT 4::COL;
SELECT sum(qty * price) FILTER (WHERE day BETWEEN getvariable('p_from')::DATE AND getvariable('p_to')::DATE)::MONEY,
       sum(qty * price) FILTER (WHERE day BETWEEN getvariable('p_prev_from')::DATE AND getvariable('p_prev_to')::DATE)::DELTA,
       getvariable('p_prev_label')::DELTA_LABEL, 'Revenue'::LABEL
FROM orders WHERE getvariable('region') IN ('All', region);
SELECT 4::COL;
SELECT count(*) FILTER (WHERE day BETWEEN getvariable('p_from')::DATE AND getvariable('p_to')::DATE)::METRIC,
       count(*) FILTER (WHERE day BETWEEN getvariable('p_prev_from')::DATE AND getvariable('p_prev_to')::DATE)::DELTA,
       getvariable('p_prev_label')::DELTA_LABEL, 'Order lines'::LABEL
FROM orders WHERE getvariable('region') IN ('All', region);
SELECT 4::COL;
SELECT count(DISTINCT customer)::METRIC, 'Customers'::LABEL
FROM orders WHERE getvariable('region') IN ('All', region)
  AND day BETWEEN getvariable('p_from')::DATE AND getvariable('p_to')::DATE;

SELECT 8::COL;
SELECT date_trunc('week', day) AS week ::XAXIS, channel ::CATEGORY, sum(qty * price) ::LINECHART, 'Weekly revenue by channel'::TITLE
FROM orders WHERE getvariable('region') IN ('All', region)
  AND day BETWEEN getvariable('p_from')::DATE AND getvariable('p_to')::DATE
GROUP BY 1, 2 ORDER BY 2, 1;
SELECT 4::COL;
SELECT category ::XAXIS, sum(qty * price) ::BARCHART, 'Revenue by category'::TITLE
FROM orders WHERE getvariable('region') IN ('All', region)
  AND day BETWEEN getvariable('p_from')::DATE AND getvariable('p_to')::DATE
GROUP BY 1 ORDER BY 2 DESC;

SELECT 12::COL;
SELECT order_id AS "Order", day AS "Day", region AS "Region", channel AS "Channel", category AS "Category",
       customer AS "Customer", qty AS "Qty", price AS "Price", qty * price AS "Revenue" ::PAGED
FROM orders WHERE getvariable('region') IN ('All', region)
  AND day BETWEEN getvariable('p_from')::DATE AND getvariable('p_to')::DATE;
