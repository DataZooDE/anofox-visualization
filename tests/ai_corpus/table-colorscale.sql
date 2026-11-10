-- @from coverage-20261110-0707 tbl-colorscale: a table with colour-scaled columns (the harness wrongly refused it)
-- @expect clean
CREATE TABLE regions AS SELECT * FROM (VALUES ('North', 1200, 1350.5, 18.2, 0.91, 2.5), ('South', 1100, 980.0, 9.5, 0.84, -4.1)) t(region, plan, revenue, margin, accuracy, bias);
SELECT region::TABLE, plan::MONEY, revenue::MONEY, margin, accuracy::COLORSCALE_HIGH_GOOD, bias::COLORSCALE_DIVERGING, 'Revenue against plan'::TITLE FROM regions ORDER BY revenue DESC;
