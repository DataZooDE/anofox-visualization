-- @from #1076 mutation fuzz: a chained role in a ::TABLE gets the table's fix (#1083)
-- @expect sql-error in a table, ::TABLE goes on the first column only
CREATE TABLE regions AS SELECT * FROM (VALUES ('North', 1200, 1350.5), ('South', 1100, 980.0)) t(region, plan, revenue);
SELECT region::MONEY::TABLE, plan::MONEY, revenue::MONEY, 'Plan'::TITLE FROM regions;
