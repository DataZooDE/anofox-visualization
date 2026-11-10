-- @from coverage-20261110-0707 ts-trend: two statements without a ';' between them
-- @expect silent-setup without a ';'
CREATE TABLE sales AS SELECT * FROM (VALUES
  (DATE '2024-01-01', 10, 39.0, 'North'), (DATE '2024-02-01', 12, 41.0, 'South'),
  (DATE '2025-01-01', 15, 38.5, 'North'), (DATE '2025-02-01', 14, 40.0, 'South')) t(month, units, price, region);
SELECT month, sum(units) AS units FROM sales GROUP BY month
SELECT month::XAXIS, units::LINECHART, units::SMOOTH, 'lm'::SMOOTH_METHOD, 'Trend'::TITLE FROM (SELECT month, sum(units) AS units FROM sales GROUP BY month) ORDER BY 1;
