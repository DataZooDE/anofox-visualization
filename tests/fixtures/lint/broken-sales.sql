-- @title Which regions are behind? (a broken draft)
-- @group Lint fixtures
--
-- A dashboard with mistakes on purpose, for the lint parity tests
-- (tests/lint_step.rs, tests/web/lint_test.mjs) and the Check e2e: each
-- statement below trips one `dashboard --check` diagnostic.

CREATE OR REPLACE TABLE sales AS
SELECT (DATE '2026-01-01' + INTERVAL (i % 90) DAY)::DATE AS day,
       ['North', 'South', 'East', 'West', 'Central', 'Coast', 'Hills', 'Lakes'][1 + i % 8] AS region,
       ['Tea', 'Coffee', 'Cocoa'][1 + i % 3] AS product,
       round(100 + 50 * sin(i / 7.0) + (i % 13) * 4, 2) AS revenue
FROM range(0, 720) t(i);

SELECT 'graphiet'::THEME;

SELECT region ::DROPDOWN FROM (SELECT DISTINCT region FROM sales ORDER BY region);

-- the roles sit inside a subquery: the statement silently becomes setup
SELECT * FROM (SELECT region ::XAXIS, sum(revenue) ::BARCHART FROM sales GROUP BY region);

-- a typo in the role
SELECT day ::XAXIS, sum(revenue) ::LINECHRT FROM sales GROUP BY day ORDER BY day;

-- a column that doesn't exist
SELECT region ::XAXIS, sum(revnue) ::BARCHART FROM sales GROUP BY region;

-- filters on the dropdown's default (the first region): no rows match
SELECT day ::XAXIS, revenue ::LINECHART FROM sales WHERE region = getvariable('region') AND revenue < 0;

-- eight slices
SELECT region ::CATEGORY, sum(revenue) ::PIE FROM sales GROUP BY region;

-- bars in no particular order
SELECT region ::XAXIS, sum(revenue) ::BARCHART, 'Revenue by region'::TITLE FROM sales GROUP BY region ORDER BY hash(region);

-- a raw table of every row
SELECT * ::TABLE FROM sales;
