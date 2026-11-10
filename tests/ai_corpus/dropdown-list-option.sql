-- @from coverage-rerun-20261110-1814 dash-filter: a list as the one option of a dropdown (#1086)
-- @expect input/list-option one option per row
CREATE TABLE sales AS SELECT * FROM (VALUES ('North', 10.0), ('South', 20.0)) t(region, revenue);
SELECT 'All'::LABEL, list(DISTINCT region)::DROPDOWN AS region FROM sales;
SELECT sum(revenue)::MONEY, 'Revenue'::LABEL FROM sales WHERE COALESCE(getvariable('region')::VARCHAR, '') IN ('', region);
