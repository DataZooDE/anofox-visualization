-- @from coverage-20261110-0707 tbl-suppliers: a bar chart without ::XAXIS (#1082)
-- @expect render-error draws its categories from ::XAXIS: `region::XAXIS`
CREATE TABLE sales AS SELECT * FROM (VALUES
  (DATE '2024-01-01', 10, 39.0, 'North'), (DATE '2024-02-01', 12, 41.0, 'South'),
  (DATE '2025-01-01', 15, 38.5, 'North'), (DATE '2025-02-01', 14, 40.0, 'South')) t(month, units, price, region);
SELECT region AS supplier, sum(units)::BARCHART, 1::FLIP, 'Units'::TITLE FROM sales GROUP BY ALL;
