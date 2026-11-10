-- @from #1076 mutation fuzz: two aliases around a role cast (#1083)
-- @expect sql-error one alias per column
CREATE TABLE sales AS SELECT * FROM (VALUES
  (DATE '2024-01-01', 10, 39.0, 'North'), (DATE '2024-02-01', 12, 41.0, 'South'),
  (DATE '2025-01-01', 15, 38.5, 'North'), (DATE '2025-02-01', 14, 40.0, 'South')) t(month, units, price, region);
SELECT month::XAXIS, sum(units) AS "v" ::BARCHART AS "Units", 'Units'::TITLE FROM sales GROUP BY ALL ORDER BY 1;
