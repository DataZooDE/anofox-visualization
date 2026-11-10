-- @from coverage-20261110-0707 rel-bubble: two roles chained on one column
-- @expect sql-error a column takes one role
CREATE TABLE sales AS SELECT * FROM (VALUES
  (DATE '2024-01-01', 10, 39.0, 'North'), (DATE '2024-02-01', 12, 41.0, 'South'),
  (DATE '2025-01-01', 15, 38.5, 'North'), (DATE '2025-02-01', 14, 40.0, 'South')) t(month, units, price, region);
SELECT price::XAXIS, units::YAXIS::SCATTER, 'Units vs price'::TITLE FROM sales;
