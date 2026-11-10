-- @from coverage-20261110-0707 ts-yoy: a role cast on an earlier alias of the same SELECT (#1081)
-- @expect sql-error Referenced column
CREATE TABLE sales AS SELECT * FROM (VALUES
  (DATE '2024-01-01', 10, 39.0, 'North'), (DATE '2024-02-01', 12, 41.0, 'South'),
  (DATE '2025-01-01', 15, 38.5, 'North'), (DATE '2025-02-01', 14, 40.0, 'South')) t(month, units, price, region);
SELECT strftime(month, '%b') AS mo_name, sum(units) AS "Units"::BARCHART, mo_name::XAXIS, 'Units'::TITLE FROM sales GROUP BY ALL;
