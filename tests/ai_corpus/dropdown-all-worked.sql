-- @from #1086: the 'All' option the skill now shows
-- @expect clean
CREATE TABLE sales AS SELECT * FROM (VALUES ('North', 10.0), ('South', 20.0), ('West', 5.0)) t(region, revenue);
SELECT r AS region ::DROPDOWN, r = 'All' ::DEFAULT FROM (SELECT 'All' AS r UNION ALL SELECT DISTINCT region FROM sales) ORDER BY r <> 'All', r;
SELECT sum(revenue)::MONEY, 'Revenue'::LABEL FROM sales WHERE getvariable('region') IN ('All', region);
