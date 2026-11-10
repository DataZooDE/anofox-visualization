-- @from #1076 mutation fuzz: a new statement without a ';' after CREATE (#1083)
-- @expect sql-error without a ';' before it
CREATE TABLE sales AS SELECT * FROM (VALUES (DATE '2025-01-01', 15), (DATE '2025-02-01', 14)) t(month, units)
SELECT month::XAXIS, sum(units)::BARCHART, 'Units'::TITLE FROM sales GROUP BY ALL ORDER BY 1;
