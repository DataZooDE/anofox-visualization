-- @from coverage-20261110-0707 tbl-suppliers: a WITH of an earlier statement (#1082)
-- @expect sql-error does not exist
CREATE TABLE sales AS SELECT * FROM (VALUES
  (DATE '2024-01-01', 10, 39.0, 'North'), (DATE '2024-02-01', 12, 41.0, 'South'),
  (DATE '2025-01-01', 15, 38.5, 'North'), (DATE '2025-02-01', 14, 40.0, 'South')) t(month, units, price, region);
WITH agg AS (SELECT region, sum(units) AS u FROM sales GROUP BY ALL)
SELECT '<50:bad; good'::THRESHOLDS;
SELECT region::XAXIS, u::BARCHART, 'Units'::TITLE FROM agg;
